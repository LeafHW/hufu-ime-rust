//! 【SSF M1】skin.ini → 皮肤定义模型。
//!
//! 语义对齐虎娘 `native/SsfSkin.cpp`（变体选择/刀口/marge/颜色 BGR 序/
//! 装饰键前缀），但按「尽力加载」原则：坏字段 → 默认值 + 中文告警
//! （进 [`SsfDefinition::warnings`]，设置页可见），整皮无可用布局才 Err。
//!
//! 颜色统一 0xAARRGGBB（u32）；间距/字号是「画布单位」——渲染端按
//! `scale = 用户字号 ÷ display.font_size` 全局等比缩放（虎娘作者画布制）。

use super::container::Files;
use super::ini::{parse_ini, Section};
use super::{ini::decode_text, SsfError};
use serde::{Deserialize, Serialize};

/// 九宫格刀口（单轴）：`tile=false` 中段拉伸；`true` 中段平铺（渲染端限块数）。
/// `before/after` = 两端固定边宽（画布像素）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SsfStretch {
    pub tile: bool,
    pub before: i32,
    pub after: i32,
}

impl SsfStretch {
    /// 全拉伸（0,0,0）——真皮肤 `layout_vertical=0,0,0` 常见。
    pub const FULL: SsfStretch = SsfStretch {
        tile: false,
        before: 0,
        after: 0,
    };
}

/// 文字内边距（画布像素），顺序与 skin.ini `*_marge=top,bottom,left,right` 一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SsfInsets {
    pub top: i32,
    pub bottom: i32,
    pub left: i32,
    pub right: i32,
}

/// 装饰浮层（Sogou `custom0`/`comp_custom0`/`cand_custom0`）。
/// 一期只保留数据不渲染；`align` 十元组语义社区仅部分实证。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SsfDecoration {
    pub image: String,
    pub align: [i32; 10],
    /// ini 是否显式给了 `*_align`（未给=全 0）。
    pub explicit_alignment: bool,
}

/// 单个布局变体：背景图（可 APNG）+ 两轴刀口 + 编码行/候选行内边距 + 装饰。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SsfLayout {
    pub image: String,
    pub horizontal: SsfStretch,
    pub vertical: SsfStretch,
    /// 编码行内边距（pinyin_marge）。
    pub code: SsfInsets,
    /// 候选行内边距（zhongwen_marge）。
    pub candidates: SsfInsets,
    pub decorations: Vec<SsfDecoration>,
}

/// 六个布局变体（Scheme_H1/H2/V1/V2 展开）。
/// H2/V2 一节拆两变体：`pinyin_*` → *_code（编码窗），`zhongwen_*` → *_candidates（候选窗）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SsfSchemes {
    pub horizontal: Option<SsfLayout>,
    pub vertical: Option<SsfLayout>,
    pub horizontal_code: Option<SsfLayout>,
    pub horizontal_candidates: Option<SsfLayout>,
    pub vertical_code: Option<SsfLayout>,
    pub vertical_candidates: Option<SsfLayout>,
}

impl SsfSchemes {
    fn all(&self) -> [(&'static str, &Option<SsfLayout>); 6] {
        [
            ("horizontal", &self.horizontal),
            ("vertical", &self.vertical),
            ("horizontal_code", &self.horizontal_code),
            ("horizontal_candidates", &self.horizontal_candidates),
            ("vertical_code", &self.vertical_code),
            ("vertical_candidates", &self.vertical_candidates),
        ]
    }

    /// 变体选择回退链（虎娘 Definition::choose 语义）：
    /// 请求变体 → 同朝向完整窗 → 同朝向候选窗 → 反朝向完整窗 → 反朝向候选窗。
    pub fn choose(
        &self,
        vertical: bool,
        has_code: bool,
        has_candidates: bool,
    ) -> Option<&SsfLayout> {
        let requested = if !has_candidates {
            if vertical {
                &self.vertical_code
            } else {
                &self.horizontal_code
            }
        } else if !has_code {
            if vertical {
                &self.vertical_candidates
            } else {
                &self.horizontal_candidates
            }
        } else if vertical {
            &self.vertical
        } else {
            &self.horizontal
        };
        let (same_full, same_cand) = if vertical {
            (&self.vertical, &self.vertical_candidates)
        } else {
            (&self.horizontal, &self.horizontal_candidates)
        };
        let (other_full, other_cand) = if vertical {
            (&self.horizontal, &self.horizontal_candidates)
        } else {
            (&self.vertical, &self.vertical_candidates)
        };
        [requested, same_full, same_cand, other_full, other_cand]
            .into_iter()
            .flatten()
            .next()
    }

    /// 全部被引用的图片（含预览），去重升序——解包/缓存只保留这些。
    pub fn used_images(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .all()
            .iter()
            .filter_map(|(_, l)| l.as_ref())
            .flat_map(|l| {
                std::iter::once(l.image.clone())
                    .chain(l.decorations.iter().map(|d| d.image.clone()))
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

/// [Display]：字体/颜色/间距（画布单位）。颜色 0xAARRGGBB。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SsfDisplay {
    /// 画布字号（skin.ini font_size，虎娘自制皮肤为 200 量级；真皮肤 16~18）。
    pub font_size: f32,
    /// 中文字族；空 = 渲染端回退默认字体。
    pub font_ch: String,
    /// 英文字族；空 = 同 font_ch。
    pub font_en: String,
    pub code_color: u32,
    pub first_color: u32,
    pub text_color: u32,
    pub annotation_color: u32,
    pub candidate_spacing: f32,
    pub line_spacing: f32,
    pub character_spacing: f32,
}

impl Default for SsfDisplay {
    fn default() -> Self {
        // 默认色与虎娘一致（0xAARRGGBB）。
        SsfDisplay {
            font_size: 17.0,
            font_ch: String::new(),
            font_en: String::new(),
            code_color: 0xFF33_3333,
            first_color: 0xFF28_69B0,
            text_color: 0xFF33_3333,
            annotation_color: 0xFF66_6666,
            candidate_spacing: 0.0,
            line_spacing: 0.0,
            character_spacing: 0.0,
        }
    }
}

/// 解析结果。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SsfDefinition {
    pub name: String,
    pub author: String,
    /// 预览图归一路径（preview_comp，缺省回退 preview_square；无则空）。
    pub preview: String,
    pub display: SsfDisplay,
    pub schemes: SsfSchemes,
    /// 中文告警（坏字段降级明细，设置页展示）。
    pub warnings: Vec<String>,
}

// --------------------------------------------------------------------------------
// 解析
// --------------------------------------------------------------------------------

fn get<'a>(section: &'a Section, key: &str) -> Option<&'a str> {
    section
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// 逗号分隔整数：精确 count 个、各自 clamp 前值域 [min,max]。
/// 任何不合法 → None（调用方记告警并用默认值）。
fn numbers(text: &str, count: usize, min: i64, max: i64) -> Option<Vec<i32>> {
    let mut out = Vec::with_capacity(count);
    for part in text.split(',') {
        let part = part.trim();
        let mut chars = part.chars().peekable();
        let negative = matches!(chars.peek(), Some('-'));
        if matches!(chars.peek(), Some('+') | Some('-')) {
            chars.next();
        }
        let mut value: i64 = 0;
        let mut digits = 0;
        for c in chars {
            let d = c.to_digit(10)? as i64;
            value = value.checked_mul(10)?.checked_add(d)?;
            digits += 1;
            if value > 100_000_000 {
                return None;
            }
        }
        if digits == 0 {
            return None;
        }
        let value = if negative { -value } else { value };
        if value < min || value > max {
            return None;
        }
        out.push(value as i32);
    }
    (out.len() == count).then_some(out)
}

/// `0xBBGGRR` → 0xAARRGGBB（不透明）；非法 → None。
pub(super) fn color_bgr(text: &str) -> Option<u32> {
    let text = text.trim().to_ascii_lowercase();
    let hex = text.strip_prefix("0x")?;
    if hex.is_empty() || hex.len() > 8 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    let r = value & 0xFF;
    let g = (value >> 8) & 0xFF;
    let b = (value >> 16) & 0xFF;
    Some(0xFF00_0000 | (r << 16) | (g << 8) | b)
}

fn float_in(text: &str, min: f32, max: f32) -> Option<f32> {
    let v: f32 = text.trim().parse().ok()?;
    (v.is_finite() && v >= min && v <= max).then_some(v)
}

struct Ctx<'a> {
    files: &'a Files,
    warnings: &'a mut Vec<String>,
}

impl<'a> Ctx<'a> {
    fn warn(&mut self, text: impl Into<String>) {
        self.warnings.push(text.into());
    }

    /// 图片引用（归一化）→ 存在于条目表才返回 Some。`label` 用于告警文案。
    fn image(&mut self, reference: &str, label: &str) -> Option<String> {
        let Ok(normalized) = super::container::sanitize_path(reference) else {
            self.warn(format!("图片引用路径非法：{reference}"));
            return None;
        };
        if self.files.get(&normalized).is_some() {
            Some(normalized)
        } else {
            self.warn(format!("缺少{label}图片：{reference}"));
            None
        }
    }

    fn stretch(&mut self, section: &Section, key: &str) -> SsfStretch {
        match get(section, key) {
            None => SsfStretch::FULL,
            Some(v) => {
                match numbers(v, 3, 0, 8192) {
                    Some(n) => match n[0] {
                        0 => SsfStretch {
                            tile: false,
                            before: n[1],
                            after: n[2],
                        },
                        1 => SsfStretch {
                            tile: true,
                            before: n[1],
                            after: n[2],
                        },
                        other => {
                            self.warn(format!("已忽略无效伸缩参数：{key}={v}（mode={other}，仅支持 0 拉伸/1 平铺）"));
                            SsfStretch::FULL // 整键忽略，与告警文案一致
                        }
                    },
                    None => {
                        self.warn(format!("已忽略无效伸缩参数：{key}={v}"));
                        SsfStretch::FULL
                    }
                }
            }
        }
    }

    fn insets(&mut self, section: &Section, key: &str) -> Option<SsfInsets> {
        // marge/margin 双拼写兼容（社区皮肤两种拼法并存）。
        let alias = key
            .strip_suffix("marge")
            .map(|stem| format!("{stem}margin"));
        let value = get(section, key).or_else(|| alias.as_deref().and_then(|a| get(section, a)))?;
        match numbers(value, 4, 0, 8192) {
            Some(n) => Some(SsfInsets {
                top: n[0],
                bottom: n[1],
                left: n[2],
                right: n[3],
            }),
            None => {
                self.warn(format!("已忽略无效边距：{key}={value}"));
                Some(SsfInsets::default())
            }
        }
    }

    /// 装饰组：prefix 为 ""（custom）/ "comp_" / "cand_"。
    fn decorations(&mut self, section: &Section, prefix: &str) -> Vec<SsfDecoration> {
        let cnt_key = format!("{prefix}custom_cnt");
        let Some(cnt) = get(section, &cnt_key)
            .and_then(|v| numbers(v, 1, 0, 64))
            .map(|n| n[0] as usize)
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for i in 0..cnt {
            let image_key = format!("{prefix}custom{i}");
            let Some(reference) = get(section, &image_key) else {
                self.warn(format!("已跳过未定义装饰：{image_key}"));
                continue;
            };
            let Some(image) = self.image(reference, "装饰") else {
                continue;
            };
            let align_key = format!("{prefix}custom{i}_align");
            let (align, explicit) = match get(section, &align_key) {
                Some(v) => match numbers(v, 10, -8192, 8192) {
                    Some(n) => (n.try_into().unwrap(), true),
                    None => {
                        self.warn(format!("已忽略无效装饰对齐：{align_key}={v}"));
                        ([0; 10], false)
                    }
                },
                None => ([0; 10], false),
            };
            out.push(SsfDecoration {
                image,
                align,
                explicit_alignment: explicit,
            });
        }
        out
    }
}

/// 单窗变体（H1/V1）。
fn parse_single(ctx: &mut Ctx, section: &Section, image_key: &str) -> Option<SsfLayout> {
    let reference = get(section, image_key)?;
    let image = ctx.image(reference, "背景")?;
    let horizontal = ctx.stretch(section, "layout_horizontal");
    let vertical = ctx.stretch(section, "layout_vertical");
    let code = ctx.insets(section, "pinyin_marge").unwrap_or_default();
    let candidates = ctx.insets(section, "zhongwen_marge").unwrap_or_default();
    let decorations = ctx.decorations(section, "");
    Some(SsfLayout {
        image,
        horizontal,
        vertical,
        code,
        candidates,
        decorations,
    })
}

/// 分体变体（H2/V2 的 pinyin_* / zhongwen_* 半边）。
fn parse_split(
    ctx: &mut Ctx,
    section: &Section,
    prefix: &str,
    decor_prefix: &str,
) -> Option<SsfLayout> {
    let reference = get(section, &format!("{prefix}_pic"))?;
    let image = ctx.image(reference, "背景")?;
    let horizontal = ctx.stretch(section, &format!("{prefix}_layout_horizontal"));
    let vertical = ctx.stretch(section, &format!("{prefix}_layout_vertical"));
    let code = ctx
        .insets(section, &format!("{prefix}_marge"))
        .unwrap_or_default();
    let candidates = code;
    let decorations = ctx.decorations(section, decor_prefix);
    Some(SsfLayout {
        image,
        horizontal,
        vertical,
        code,
        candidates,
        decorations,
    })
}

/// skin.ini（Files["skin.ini"]）→ 定义。条目表里没有 skin.ini 时报错
/// （decode_archive 已保证存在，这里是独立调用时的防线）。
pub fn parse_definition(files: &Files) -> Result<SsfDefinition, SsfError> {
    let ini_bytes = files
        .get("skin.ini")
        .ok_or_else(|| SsfError::Message("SSF 缺少根 skin.ini".into()))?;
    let text = decode_text(ini_bytes)?;
    let ini = parse_ini(&text)?;
    let mut warnings: Vec<String> = ini
        .duplicates
        .iter()
        .map(|(s, k)| format!("重复键已取后者：[{s}] {k}"))
        .collect();
    let empty = Section::default();
    let general = ini.sections.get("general").unwrap_or(&empty);
    let display = ini.sections.get("display").unwrap_or(&empty);
    let h1 = ini.sections.get("scheme_h1").unwrap_or(&empty);
    let h2 = ini.sections.get("scheme_h2").unwrap_or(&empty);
    let v1 = ini.sections.get("scheme_v1").unwrap_or(&empty);
    let v2 = ini.sections.get("scheme_v2").unwrap_or(&empty);

    let mut ctx = Ctx {
        files,
        warnings: &mut warnings,
    };

    // [General]
    let mut definition = SsfDefinition {
        name: get(general, "skin_name")
            .map(|v| v.chars().take(120).collect())
            .unwrap_or_default(),
        author: get(general, "skin_author")
            .map(|v| v.chars().take(120).collect())
            .unwrap_or_default(),
        preview: String::new(),
        display: SsfDisplay::default(),
        schemes: SsfSchemes::default(),
        warnings: Vec::new(),
    };
    if definition.name.is_empty() {
        definition.name = "未命名皮肤".into();
    }
    for key in ["preview_comp", "preview_square"] {
        if let Some(reference) = get(general, key) {
            if let Some(normalized) = ctx.image(reference, "预览") {
                definition.preview = normalized;
                break;
            }
        }
    }

    // [Display]
    if let Some(v) = get(display, "font_size") {
        match float_in(v, 3.0, 200.0) {
            Some(size) => definition.display.font_size = size,
            None => ctx.warn(format!("已忽略无效字号：font_size={v}（取默认 17）")),
        }
    }
    for (key, target) in [("font_ch", 0), ("font_en", 1)] {
        if let Some(v) = get(display, key) {
            let trimmed: String = v.chars().take(64).collect();
            if target == 0 {
                definition.display.font_ch = trimmed;
            } else {
                definition.display.font_en = trimmed;
            }
        }
    }
    for (key, target) in [
        ("pinyin_color", 0),
        ("zhongwen_first_color", 1),
        ("zhongwen_color", 2),
        ("comphint_color", 3),
    ] {
        if let Some(v) = get(display, key) {
            match color_bgr(v) {
                Some(c) => match target {
                    0 => definition.display.code_color = c,
                    1 => definition.display.first_color = c,
                    2 => definition.display.text_color = c,
                    _ => definition.display.annotation_color = c,
                },
                None => ctx.warn(format!("已忽略无效颜色：{key}={v}")),
            }
        }
    }
    for (key, target) in [
        ("candidate_spacing", 0),
        ("line_spacing", 1),
        ("character_spacing", 2),
    ] {
        if let Some(v) = get(display, key) {
            match float_in(v, 0.0, 2000.0) {
                Some(n) => match target {
                    0 => definition.display.candidate_spacing = n,
                    1 => definition.display.line_spacing = n,
                    _ => definition.display.character_spacing = n,
                },
                None => ctx.warn(format!("已忽略无效间距：{key}={v}")),
            }
        }
    }

    // [Scheme_*] → 六变体
    definition.schemes.horizontal = parse_single(&mut ctx, h1, "pic");
    definition.schemes.vertical = parse_single(&mut ctx, v1, "pic");
    definition.schemes.horizontal_code = parse_split(&mut ctx, h2, "pinyin", "comp_");
    definition.schemes.horizontal_candidates = parse_split(&mut ctx, h2, "zhongwen", "cand_");
    definition.schemes.vertical_code = parse_split(&mut ctx, v2, "pinyin", "comp_");
    definition.schemes.vertical_candidates = parse_split(&mut ctx, v2, "zhongwen", "cand_");

    definition.warnings = warnings;
    if definition.schemes.all().iter().all(|(_, l)| l.is_none()) {
        return Err(SsfError::Message(
            "SSF has no supported candidate background".into(),
        ));
    }
    Ok(definition)
}
