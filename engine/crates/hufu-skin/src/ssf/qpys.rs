//! 【QPYS】QQ输入法皮肤（.qpys）解析 → SSF 模型。
//!
//! 容器三层（2026-10-16 逆向实证，样本 3 个官方皮肤）：
//! 1. `SYPQ` 头 12B：magic + 4B（id/hash）+ u32LE 声明解压尺寸；
//! 2. 偏移 12 起 zlib 流（所谓「加密」实为标准 zlib 压缩）；
//! 3. 解压后 `FSPQ` 头：16B 头 + 填充，**偏移 0x114 起是 OLE 复合文档**
//!    （标准 CFB，老 Office 格式）——目录树直出「文件名→流」：
//!    *.png 资源（原中文名）+ skininfo-ver2.0.xml（名称/作者）
//!    + skinConfig-{divide|unite|status}-ver2.0.xml（布局配置）。
//!
//! 布局 XML 语义（分窗口 divide / 一体化 unite）与 SSF 九宫格同构：
//! - `Stretch(l,t,r,b)` = 四边固定宽 → `SsfStretch{before,after}` 两轴；
//! - `TextOutMargin(l,t,r,b)` = 文字内边距 → `SsfInsets`；
//! - `CandFont/CompFont` 的 `chfont_size/chfont_name/normal_color(文字)/
//!   hot_color(高亮)/chinese_color(编码)` → `SsfDisplay`；
//! - `PY_DIALOG_BKGROUND.PYCS_NORMAL` = 面板背景图引用。
//!
//! 映射目标变体：
//! - divide `CandDialog`（候选窗，竖图）→ `vertical_candidates`；
//! - divide `CompDialog`（组词窗）→ `vertical_code`；
//! - unite `UniteDialog`（一体化横窗）→ `horizontal`（完整窗）+ 平行填充
//!   `horizontal_code`/`horizontal_candidates`（回退链受益）。
//!
//! 尽力加载原则与 ssf 一致：坏字段降级 warning，整包不可读才 Err。

use super::container::Files;
use super::model::{SsfDefinition, SsfDisplay, SsfInsets, SsfLayout, SsfSchemes, SsfStretch};
use super::SsfError;
use std::collections::BTreeMap;

/// 是否 QQ 皮肤（SYPQ magic）——`ssf::parse` 入口嗅探用。
pub fn is_qpys(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"SYPQ"
}

/// 一站式：.qpys 字节 → (条目表, 皮肤定义)。
pub fn parse(bytes: &[u8]) -> Result<(Files, SsfDefinition), SsfError> {
    let files = decode(bytes)?;
    let definition = parse_definition(&files)?;
    Ok((files, definition))
}

// --------------------------------------------------------------------------------
// 容器
// --------------------------------------------------------------------------------

/// SYPQ → zlib → FSPQ → OLE 流表（名字经 `sanitize_path` 归一）。
fn decode(raw: &[u8]) -> Result<Files, SsfError> {
    if raw.len() > super::MAX_INPUT_BYTES {
        return Err(SsfError::Message("皮肤文件超过输入上限".into()));
    }
    if !is_qpys(raw) {
        return Err(SsfError::Message("不是 QQ 皮肤（缺 SYPQ 头）".into()));
    }
    // zlib 解压（flate2::read::ZlibDecoder）
    use std::io::Read;
    let mut z = flate2::read::ZlibDecoder::new(&raw[12..]);
    let mut fspq = Vec::new();
    z.read_to_end(&mut fspq)
        .map_err(|e| SsfError::Message(format!("zlib 解压失败：{e}")))?;
    if fspq.len() > super::MAX_EXPANDED_BYTES {
        return Err(SsfError::Message("解压后超过预算（疑似炸弹）".into()));
    }
    if fspq.len() < 0x118 || &fspq[0..4] != b"FSPQ" {
        return Err(SsfError::Message("解压后缺 FSPQ 头（格式不识别）".into()));
    }
    // OLE 复合文档（0x114 起）。名字可能 GBK/任意——CFB 目录名按 UTF-16
    // 交付（cfb crate 内部处理），中文皮肤名直出。
    let cursor = std::io::Cursor::new(&fspq[0x114..]);
    let mut comp = cfb::CompoundFile::open(cursor)
        .map_err(|e| SsfError::Message(format!("OLE 复合文档打开失败：{e}")))?;
    let mut map: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut total = 0usize;
    let mut entries = 0usize;
    // 先收集流清单（walk 借用 comp，open_stream 需 &mut——分两段）
    let mut listing: Vec<(std::path::PathBuf, u64)> = Vec::new();
    for entry in comp.walk() {
        if entry.is_stream() {
            listing.push((entry.path().to_path_buf(), entry.len()));
        }
    }
    for (path, len) in listing {
        entries += 1;
        if entries > super::MAX_ENTRIES {
            return Err(SsfError::Message("条目数超过上限".into()));
        }
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.len() > super::MAX_NAME_BYTES {
            return Err(SsfError::Message(format!("条目名过长：{name:.20}…")));
        }
        let normalized = super::container::sanitize_path(&name)
            .map_err(|e| SsfError::Message(format!("条目名非法（{name:.20}）：{e}")))?;
        let mut data = Vec::with_capacity(len as usize);
        let mut stream = comp
            .open_stream(&path)
            .map_err(|e| SsfError::Message(format!("流读取失败（{name:.20}）：{e}")))?;
        stream
            .read_to_end(&mut data)
            .map_err(|e| SsfError::Message(format!("流读取失败（{name:.20}）：{e}")))?;
        total += data.len();
        if total > super::MAX_EXPANDED_BYTES {
            return Err(SsfError::Message("资源总量超过预算".into()));
        }
        map.insert(normalized, data);
    }
    if map.is_empty() {
        return Err(SsfError::Message("QQ 皮肤内无任何资源".into()));
    }
    Ok(Files::from_map(map))
}

// --------------------------------------------------------------------------------
// XML（微型三层解析：CONTROL class → MEMBER name → ATTRIBUTE name=value）
// --------------------------------------------------------------------------------

/// 一个 MEMBER：name + 属性表（值 String 化——避开 XML 文本生命周期）。
struct Member {
    name: String,
    attrs: Vec<(String, String)>,
}

impl Member {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// 一个 CONTROL：class + MEMBER 列表（不含子 CONTROL——平铺扫描足够）。
struct Control {
    class: String,
    members: Vec<Member>,
}

impl Control {
    fn member(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.name == name)
    }
}

/// 微型 XML 扫描：抽所有 `<CONTROL class="X">`（只取其直属
/// `<CTRL_PROPE>` 内的 MEMBER——跳过 CTRL_CHILD 子树由深度计数保证：
/// QQ 布局约定 CTRL_PROPE 总在 CTRL_CHILD 之前，扫描遇到 CTRL_CHILD 即
/// 停止该 CONTROL 的成员收集）。
fn scan_controls(xml: &str) -> Vec<Control> {
    let mut out = Vec::new();
    let bytes = xml.as_bytes();
    let mut i = 0usize;
    while let Some(start) = find_sub(bytes, b"<CONTROL ", i) {
        // class="..."
        let Some(rel) = attr_after(bytes, start, b"class") else {
            i = start + 9;
            continue;
        };
        let class = rel.to_string();
        let mut members = Vec::new();
        let mut p = start;
        // 收集 MEMBER 直到 </CTRL_PROPE> 或 <CTRL_CHILD>
        loop {
            let Some(m_start) = find_sub(bytes, b"<MEMBER ", p) else { break };
            // 终止条件：先撞上 CTRL_CHILD / </CONTROL> 再撞 MEMBER
            let stop = ["<CTRL_CHILD", "</CONTROL"]
                .iter()
                .filter_map(|t| find_sub(bytes, t.as_bytes(), p))
                .min();
            match stop {
                Some(s) if s < m_start => break,
                _ => {}
            }
            let Some(mname) = attr_after(bytes, m_start, b"name") else {
                p = m_start + 7;
                continue;
            };
            let m_name = mname.to_string();
            let mut attrs = Vec::new();
            let mut q = m_start;
            while let Some(a_start) = find_sub(bytes, b"<ATTRIBUTE ", q) {
                let close = find_sub(bytes, b"</MEMBER>", q.min(a_start));
                match close {
                    Some(c) if c < a_start => break,
                    _ => {}
                }
                if let Some((a_name, a_val)) = attr_pair_after(bytes, a_start, b"name") {
                    attrs.push((a_name.to_string(), a_val.to_string()));
                }
                q = a_start + 11;
            }
            members.push(Member {
                name: m_name,
                attrs,
            });
            p = m_start + 7;
        }
        out.push(Control { class, members });
        i = start + 9;
    }
    out
}

/// XML 声明/编码无关的文本解码：QQ XML 头声明 utf-8（实证样本是 ASCII
/// 属性 + GBK 中文 value 混杂——按 GBK 兜底转 String（非法字节替换）。
fn xml_text(bytes: &[u8]) -> String {
    let head = bytes.len().min(200);
    let ascii_head = String::from_utf8_lossy(&bytes[..head]).to_string();
    if ascii_head.to_ascii_lowercase().contains("utf-8") {
        if let Ok(s) = std::str::from_utf8(bytes) {
            return s.to_string();
        }
    }
    let (decoded, _, _) = encoding_rs::GBK.decode(bytes);
    decoded.to_string()
}

fn find_sub(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// `<TAG ... name="值"` → 值（&str 借自原文）。
fn attr_after<'a>(bytes: &'a [u8], tag_start: usize, attr: &[u8]) -> Option<&'a str> {
    let tag_end = find_sub(bytes, b">", tag_start)?;
    let region = &bytes[tag_start..tag_end];
    let pat_len = attr.len();
    let mut i = 0;
    while i + pat_len + 2 <= region.len() {
        if &region[i..i + pat_len] == attr {
            let mut j = i + pat_len;
            while j < region.len() && (region[j] == b' ' || region[j] == b'\t') {
                j += 1;
            }
            if j < region.len() && region[j] == b'=' {
                j += 1;
                while j < region.len() && (region[j] == b' ' || region[j] == b'\t') {
                    j += 1;
                }
                if j < region.len() && region[j] == b'"' {
                    let v_start = j + 1;
                    let v_end = (v_start..region.len()).find(|&k| region[k] == b'"')?;
                    return std::str::from_utf8(&region[v_start..v_end]).ok();
                }
            }
        }
        i += 1;
    }
    None
}

/// `<ATTRIBUTE name="键" value="值"` 对。
fn attr_pair_after<'a>(
    bytes: &'a [u8],
    tag_start: usize,
    first: &[u8],
) -> Option<(String, &'a str)> {
    let name = attr_after(bytes, tag_start, first)?;
    let value = attr_after(bytes, tag_start, b"value")?;
    Some((name.to_string(), value))
}

// --------------------------------------------------------------------------------
// 定义
// --------------------------------------------------------------------------------

/// `0xBBGGRR` → 0xAARRGGBB（QQ 与搜狗同为 BGR 序；实证 0x6a84cb 解出
/// 蓝紫与皮肤观感一致）。非法/缺省 → None。
fn color_bgr(text: &str) -> Option<u32> {
    let t = text.trim();
    let hex = t.strip_prefix("0x")?;
    if hex.is_empty() || hex.len() > 8 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    let r = v & 0xFF;
    let g = (v >> 8) & 0xFF;
    let b = (v >> 16) & 0xFF;
    Some(0xFF00_0000 | (r << 16) | (g << 8) | b)
}

/// MEMBER → SsfStretch：Stretch(l,t,r,b) 四边固定宽；mode≠1 时中段平铺
///（QQ 编辑器 mode 枚举：1=拉伸 2=平铺——按平铺兜底）。
fn stretch_of(m: &Member) -> SsfStretch {
    let n = |k: &str| m.attr(k).and_then(|v| v.trim().parse::<i32>().ok()).unwrap_or(0);
    SsfStretch {
        tile: m.attr("mode").and_then(|v| v.trim().parse::<i32>().ok()) != Some(1),
        before: n("left").clamp(0, 2048),
        after: n("right").clamp(0, 2048),
    }
    // 注意：水平轴 before/after 用 left/right；垂直轴调用方另行取 top/bottom。
}

fn stretch_v_of(m: &Member) -> SsfStretch {
    let n = |k: &str| m.attr(k).and_then(|v| v.trim().parse::<i32>().ok()).unwrap_or(0);
    SsfStretch {
        tile: m.attr("mode").and_then(|v| v.trim().parse::<i32>().ok()) != Some(1),
        before: n("top").clamp(0, 2048),
        after: n("bottom").clamp(0, 2048),
    }
}

/// MEMBER(TextOutMargin) → SsfInsets（top,bottom,left,right）。
fn insets_of(m: &Member) -> SsfInsets {
    let n = |k: &str| {
        m.attr(k)
            .and_then(|v| v.trim().parse::<i32>().ok())
            .unwrap_or(0)
            .clamp(0, 2048)
    };
    SsfInsets {
        top: n("top"),
        bottom: n("bottom"),
        left: n("left"),
        right: n("right"),
    }
}

fn default_insets() -> SsfInsets {
    SsfInsets {
        top: 4,
        bottom: 4,
        left: 6,
        right: 6,
    }
}

/// 图片引用：值存在条目表才采用（否则 warning）。
fn image_ref(files: &Files, m: &Member, warnings: &mut Vec<String>, what: &str) -> Option<String> {
    let raw = m.attr("PYCS_NORMAL").or_else(|| m.attr("value"))?;
    let name = raw.trim();
    if name.is_empty() {
        return None;
    }
    let norm = super::container::sanitize_path(name).ok()?;
    if files.get(&norm).is_some() {
        Some(norm)
    } else {
        warnings.push(format!("{what}背景图缺失：{name}"));
        None
    }
}

/// 条目表 → 定义（divide/unite XML 映射）。
pub fn parse_definition(files: &Files) -> Result<SsfDefinition, SsfError> {
    let mut warnings = Vec::new();
    let mut def = SsfDefinition {
        name: String::new(),
        author: String::new(),
        preview: String::new(),
        display: SsfDisplay::default(),
        schemes: SsfSchemes::default(),
        warnings: Vec::new(),
    };

    // skininfo：名称/作者
    for (name, data) in files.iter() {
        if name.ends_with("skininfo-ver2.0.xml") {
            let xml = xml_text(data);
            let bytes = xml.as_bytes();
            if let Some(start) = find_sub(bytes, b"<FILE ", 0) {
                if let Some(n) = attr_after(bytes, start, b"name") {
                    def.name = n.to_string();
                }
            }
            if let Some(start) = find_sub(bytes, b"<AUTHOR ", 0) {
                if let Some(a) = attr_after(bytes, start, b"author") {
                    def.author = a.to_string();
                }
            }
        }
    }
    if def.name.is_empty() {
        warnings.push("skininfo 缺皮肤名（显示为文件名）".into());
    }

    // preview.png（OLE 内通用名）
    if files.get("preview.png").is_some() {
        def.preview = "preview.png".into();
    }

    // 布局配置：divide 优先（分窗口=竖排语义），unite 补横排。
    let mut divide_xml = None;
    let mut unite_xml = None;
    for (name, data) in files.iter() {
        if name.ends_with("skinconfig-divide-ver2.0.xml") {
            divide_xml = Some(xml_text(data));
        } else if name.ends_with("skinconfig-unite-ver2.0.xml") {
            unite_xml = Some(xml_text(data));
        }
    }

    let mut font_size: Option<f32> = None;
    let mut font_ch = String::new();
    let mut text_color: Option<u32> = None;
    let mut hot_color: Option<u32> = None;
    let mut code_color: Option<u32> = None;

    if let Some(xml) = &divide_xml {
        for c in scan_controls(xml) {
            let is_cand = c.class == "CandDialog";
            let is_comp = c.class == "CompDialog";
            if !is_cand && !is_comp {
                continue;
            }
            let image = c
                .member("PY_DIALOG_BKGROUND")
                .and_then(|m| image_ref(files, m, &mut warnings, &c.class));
            let Some(image) = image else { continue };
            let st = c.member("Stretch").map(stretch_of).unwrap_or_default();
            let st_v = c.member("Stretch").map(stretch_v_of).unwrap_or_default();
            let ins = c
                .member("TextOutMargin")
                .map(insets_of)
                .unwrap_or_else(default_insets);
            // 字体（CandDialog 的 CandFont / CompDialog 的 CompFont——同键族）
            let font = c.member("CandFont").or_else(|| c.member("CompFont"));
            if let Some(f) = font {
                if font_size.is_none() {
                    font_size = f
                        .attr("chfont_size")
                        .and_then(|v| v.trim().parse::<f32>().ok())
                        .filter(|v| (6.0..=96.0).contains(v));
                }
                if font_ch.is_empty() {
                    font_ch = f.attr("chfont_name").unwrap_or("").trim().to_string();
                }
                let col = |k: &str| f.attr(k).and_then(color_bgr);
                if is_cand {
                    text_color = text_color.or_else(|| col("normal_color"));
                    hot_color = hot_color.or_else(|| col("hot_color"));
                } else {
                    code_color = code_color.or_else(|| col("chinese_color"));
                }
            }
            let layout = SsfLayout {
                image,
                horizontal: st,
                vertical: st_v,
                code: ins,
                candidates: ins,
                decorations: Vec::new(),
            };
            if is_cand {
                def.schemes.vertical_candidates = Some(layout);
            } else {
                def.schemes.vertical_code = Some(layout);
            }
        }
    }

    if let Some(xml) = &unite_xml {
        for c in scan_controls(xml) {
            if c.class != "UniteDialog" {
                continue;
            }
            let image = c
                .member("PY_DIALOG_BKGROUND")
                .and_then(|m| image_ref(files, m, &mut warnings, "一体化窗"));
            let Some(image) = image else { continue };
            let st = c.member("Stretch").map(stretch_of).unwrap_or_default();
            let st_v = c.member("Stretch").map(stretch_v_of).unwrap_or_default();
            let ins = c
                .member("TextOutMargin")
                .map(insets_of)
                .unwrap_or_else(default_insets);
            // 一体化窗：top 大边距属组词区，候选区贴其下——统一用同 insets
            //（渲染端编码行+候选行各自消费）。
            let mut font = c.member("CandFont");
            if font.is_none() {
                font = c.member("CompFont");
            }
            if let Some(f) = font {
                if font_size.is_none() {
                    font_size = f
                        .attr("chfont_size")
                        .and_then(|v| v.trim().parse::<f32>().ok())
                        .filter(|v| (6.0..=96.0).contains(v));
                }
                if font_ch.is_empty() {
                    font_ch = f.attr("chfont_name").unwrap_or("").trim().to_string();
                }
                text_color = text_color.or_else(|| {
                    f.attr("normal_color").and_then(color_bgr)
                });
                hot_color = hot_color.or_else(|| f.attr("hot_color").and_then(color_bgr));
                code_color = code_color.or_else(|| {
                    f.attr("chinese_color").and_then(color_bgr)
                });
            }
            let layout = SsfLayout {
                image,
                horizontal: st,
                vertical: st_v,
                code: ins,
                candidates: ins,
                decorations: Vec::new(),
            };
            def.schemes.horizontal = Some(layout.clone());
            def.schemes.horizontal_code = Some(layout.clone());
            def.schemes.horizontal_candidates = Some(layout);
        }
    }

    // display 汇总
    if let Some(fs) = font_size {
        def.display.font_size = fs;
    }
    def.display.font_ch = font_ch;
    if let Some(c) = code_color {
        def.display.code_color = c;
    }
    if let Some(c) = hot_color {
        def.display.first_color = c;
    }
    if let Some(c) = text_color {
        def.display.text_color = c;
        def.display.annotation_color = c;
    }

    // 整皮无可用布局 → Err（与 ssf 口径一致）
    if def.schemes.used_images().is_empty() {
        return Err(SsfError::Message(
            "QQ 皮肤内无可用布局（缺 divide/unite 配置或背景图）".into(),
        ));
    }
    def.warnings = warnings;
    Ok(def)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三个真实 QQ 皮肤端到端：解容器→定义全链（含 XML 映射）。
    #[test]
    fn real_qpys_skins_end_to_end() {
        let dir = std::path::Path::new(r"C:\Users\Leafyin\Desktop\新建文件夹 (2)");
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("qpys") {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let (files, def) = parse(&bytes)
                .unwrap_or_else(|e| panic!("{} 解析失败：{e}", path.display()));
            checked += 1;
            // 条目表：图片+xml 齐
            assert!(files.len() >= 15, "{} 条目过少：{}", path.display(), files.len());
            assert!(files.get("preview.png").is_some(), "{} 缺 preview", path.display());
            // 皮肤名/作者（skininfo）
            assert!(!def.name.is_empty(), "{} 缺名称", path.display());
            assert!(!def.author.is_empty(), "{} 缺作者", path.display());
            // 布局：竖排候选+编码（divide），横排（unite）
            assert!(
                def.schemes.vertical_candidates.is_some(),
                "{} 缺竖排候选变体",
                path.display()
            );
            assert!(def.schemes.vertical_code.is_some(), "{} 缺竖排编码变体", path.display());
            assert!(def.schemes.horizontal.is_some(), "{} 缺横排变体", path.display());
            // 字号/颜色都解到了
            assert!((6.0..=96.0).contains(&def.display.font_size));
            assert_ne!(def.display.text_color, 0xFF33_3333, "{} 文字色未解到", path.display());
            eprintln!(
                "{} → 「{}」作者={} 字号={} 条目={} 竖候选图={:?}",
                path.display(),
                def.name,
                def.author,
                def.display.font_size,
                files.len(),
                def.schemes
                    .vertical_candidates
                    .as_ref()
                    .map(|l| l.image.clone())
            );
        }
        assert!(checked >= 3, "测试目录没有 3 个 qpys（找到 {checked}）");
    }
}
