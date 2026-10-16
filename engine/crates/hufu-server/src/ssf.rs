//! .ssf 皮肤 · server 侧落地（解包缓存 / 指纹 / 清单 / 枚举 / 管道载荷）。
//!
//! 职责边界（设计稿 §4.1）：容器与 skin.ini 解析在 `hufu_skin::ssf`
//! （M1，纯内存、预算/消毒全套）；本模块只管磁盘布局与出入海口：
//! - 解包产物落 `数据\皮肤缓存\{stem消毒}\{归一文件名}`（写盘前对每个
//!   条目名做二次安全校验——zip 穿越防御不信任上游）；
//! - `manifest.json` 记 {definition, fp, source{size,mtime}}——缓存命中
//!   判定 = 源 size+mtime 未变 && manifest 存在 && 所有 used_images 存在；
//! - 源指纹 fp = `"{size}-{sha256 前 16 hex}"`（只在解包时算一次，随后经
//!   manifest 复用——每次 skin op 重算会白读 64MB）。
//!
//! 【绝不 panic】server 崩了输入法就完了（任务红线）：所有解析/IO 失败
//! 一律 Err(字符串) 上抛，由列表/端点/管道各自降级显示或回退默认皮。
//! .ssf 完全用户自管理：不做任何内置/自装（法律红线：绝不内置搜狗皮肤）。

use std::path::{Path, PathBuf};

/// 皮肤目录 .ssf 数量上限（虎娘口径 256；与 *.json 混居目录里按扩展名
/// 过滤后计数，同名覆盖不算新增）。
pub const MAX_SSF_LISTED: usize = 256;

/// manifest 文件名（缓存目录内）。
const MANIFEST_FILE: &str = "manifest.json";

// ── 数据结构 ─────────────────────────────────────────────────────

/// 一个可用 .ssf 皮肤包（缓存目录 + 结构化定义 + 源指纹）。
pub struct SsfBundle {
    /// 皮肤 id（= 源文件名 stem）。
    pub id: String,
    /// 解包缓存目录（绝对路径，管道下发 cache_dir 用它）。
    pub cache_dir: PathBuf,
    /// skin.ini 结构化结果。
    pub def: hufu_skin::ssf::SsfDefinition,
    /// 源指纹 "{size}-{sha256 前 16 hex}"。
    pub fp: String,
}

/// 列表项（含解析失败信息——坏皮也列出，用户能看到哪里坏了）。
pub struct SsfSkinEntry {
    pub id: String,
    /// 正常= skin_name；解析失败= 文件名。
    pub name: String,
    pub author: String,
    /// 解析/解包错误（皮肤存在但不可用）。
    pub error: Option<String>,
    /// manifest.preview 非空（GET /api/skin/preview 可用）。
    pub preview: bool,
    /// skin.ini 坏字段降级提示（虎娘「尽力加载」口径）。
    pub warnings: Vec<String>,
    pub fp: String,
}

/// 缓存清单：definition + 源指纹 + 源文件快照（size/mtime 命中判定）。
#[derive(serde::Serialize, serde::Deserialize)]
struct Manifest {
    fp: String,
    source: ManifestSource,
    definition: hufu_skin::ssf::SsfDefinition,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ManifestSource {
    size: u64,
    /// mtime 纳秒（NTFS 100ns 粒度；比秒级判定精细，防「同秒重存」边角）。
    mtime: u64,
}

// ── 目录布局 ─────────────────────────────────────────────────────

/// .ssf 皮肤目录：与现有 *.json 同目录混居（list 时按扩展名分类）。
pub fn ssf_skins_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("皮肤")
}

/// 解包缓存根：`数据\皮肤缓存\`。
pub fn ssf_cache_root(data_dir: &Path) -> PathBuf {
    data_dir.join("皮肤缓存")
}

// ── 工具 ─────────────────────────────────────────────────────────

/// 源指纹：`"{size}-{sha256 前 16 hex}"`（小写 hex）。
pub fn source_fp(bytes: &[u8]) -> String {
    format!("{}-{}", bytes.len(), sha256_16hex(bytes))
}

fn sha256_16hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    let d = h.finalize();
    d[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// u32(0xAARRGGBB) → "#RRGGBBAA"（DLL color_f 按 #RRGGBBAA 序解析）。
/// 【契约修正 2026-12 Lead】不能 `format!("#{:08X}", c)` 原序输出——
/// 例：skin.ini `pinyin_color=0x79513b`(BGR) → u32=0xFF3B5179 →
/// 本函数 → "#3B5179FF"（不透明 A 在末尾）。
pub fn argb_to_rgba_hex(c: u32) -> String {
    let r = (c >> 16) & 0xFF;
    let g = (c >> 8) & 0xFF;
    let b = c & 0xFF;
    let a = (c >> 24) & 0xFF;
    format!("#{:02X}{:02X}{:02X}{:02X}", r, g, b, a)
}

/// 皮肤 id / 缓存目录名消毒：去 Windows 非法字符与控制符、去尾随点/
/// 空格、限 64 字符、保留 CJK；空结果回退 "ssf"；保留名加后缀。
pub fn sanitize_stem(s: &str) -> String {
    let mut out: String = s
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .take(64)
        .collect();
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }
    if out.is_empty() {
        out = "ssf".into();
    }
    if is_reserved_name(&out) {
        out.push('_');
    }
    out
}

fn is_reserved_name(s: &str) -> bool {
    let base = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    for pre in ["COM", "LPT"] {
        if let Some(num) = base.strip_prefix(pre) {
            if let Ok(n) = num.parse::<u8>() {
                if (1..=9).contains(&n) {
                    return true;
                }
            }
        }
    }
    false
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && sanitize_stem(id) == id
}

/// 条目落盘前的二次校验（M1 已消毒归一，这里防御性再验）：sanitize_path
/// 可重入且结果=自身；逐分量非空、非 `.`/`..`、非 Windows 保留名。
/// 不合法返回 None（调用方跳过该条目——zip 穿越绝不落盘）。
fn safe_rel_path(name: &str) -> Option<PathBuf> {
    let re = hufu_skin::ssf::sanitize_path(name).ok()?;
    if re != name {
        return None;
    }
    let mut p = PathBuf::new();
    for comp in name.split('/') {
        if comp.is_empty() || comp == "." || comp == ".." {
            return None;
        }
        if is_reserved_name(comp) {
            return None;
        }
        p.push(comp);
    }
    Some(p)
}

fn mtime_nanos_of(m: &std::fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn mtime_nanos(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| mtime_nanos_of(&m)).unwrap_or(0)
}

fn read_manifest(cache_dir: &Path) -> Option<Manifest> {
    let b = std::fs::read(cache_dir.join(MANIFEST_FILE)).ok()?;
    serde_json::from_slice(&b).ok()
}

/// 解包失败负缓存：(源路径, size, mtime, 错误)。源未变不重试——坏皮
/// 若无此缓存，DLL skin op 每 2.5s 例行轮询都会在 host 锁内重解
/// （≤64MB 读 + AES/zip 解码，卡全机打字）。源变了（重存/重导入）指纹
/// 不同自动失效；导入/删除/解包成功时主动清除。
/// Vec 而非 HashMap：static 初始化须 const（与 pipe.rs LAST_SKIN 同
/// 风格）；坏皮条目个位数，线性扫成本可忽略。
static UNBUNDLE_FAIL: std::sync::Mutex<Vec<(PathBuf, u64, u64, String)>> =
    std::sync::Mutex::new(Vec::new());

fn fail_cached(src: &Path, size: u64, mtime: u64) -> Option<String> {
    UNBUNDLE_FAIL.lock().ok().and_then(|m| {
        m.iter()
            .find(|(p, s, t, _)| p == src && *s == size && *t == mtime)
            .map(|(_, _, _, e)| e.clone())
    })
}

fn fail_remember(src: &Path, size: u64, mtime: u64, err: &str) {
    if let Ok(mut m) = UNBUNDLE_FAIL.lock() {
        m.retain(|(p, _, _, _)| p != src);
        m.push((src.to_path_buf(), size, mtime, err.to_string()));
    }
}

fn fail_clear(src: &Path) {
    if let Ok(mut m) = UNBUNDLE_FAIL.lock() {
        m.retain(|(p, _, _, _)| p != src);
    }
}

fn used_images_ok(cache_dir: &Path, def: &hufu_skin::ssf::SsfDefinition) -> bool {
    def.schemes
        .used_images()
        .iter()
        .all(|img| safe_rel_path(img).map(|p| cache_dir.join(p).is_file()).unwrap_or(false))
}

// ── 核心：解包与缓存 ─────────────────────────────────────────────

/// id → 源文件。
/// 【QPYS 留档 2026-10-16】QQ 皮肤支持已按用户要求摘除——.qpys 分支
/// 留档注释；重启时恢复：list 过滤、heal/select 存在性、import 判型
/// 三处 + hufu_skin::ssf::parse 嗅探（git log「QQ输入法皮肤」找全）。
fn source_for(data_dir: &Path, id: &str) -> Option<std::path::PathBuf> {
    let ssf = ssf_skins_dir(data_dir).join(format!("{id}.ssf"));
    // let qpys = ssf_skins_dir(data_dir).join(format!("{id}.qpys"));
    // if qpys.is_file() { return Some(qpys); }
    ssf.is_file().then_some(ssf)
}

/// 取皮肤（缓存优先）。源未变且 manifest+used_images 齐 → 直接用缓存；
/// 源变了 → 重解包；源文件缺失但缓存完整 → 降级用缓存（设计稿降级
/// 矩阵「运行中被删→缓存继续用」；启动期回退由 host.heal_ssf_skin_config
/// 负责，不走这里）。
pub fn bundle_for(data_dir: &Path, id: &str) -> Result<SsfBundle, String> {
    if !valid_id(id) {
        return Err(format!("皮肤 id 非法: {id:?}"));
    }
    let Some(src) = source_for(data_dir, id) else {
        // 源缺失：缓存降级（无 source 校验——能凑合渲染就行）
        let cache_dir = ssf_cache_root(data_dir).join(id);
        if let Some(man) = read_manifest(&cache_dir) {
            if used_images_ok(&cache_dir, &man.definition) {
                return Ok(SsfBundle {
                    id: id.to_string(),
                    cache_dir,
                    def: man.definition,
                    fp: man.fp,
                });
            }
        }
        return Err(format!("皮肤文件不存在: {id}"));
    };
    let cache_dir = ssf_cache_root(data_dir).join(id);
    match std::fs::metadata(&src) {
        Ok(m) => {
            let (size, mtime) = (m.len(), mtime_nanos_of(&m));
            if let Some(man) = read_manifest(&cache_dir) {
                if man.source.size == size
                    && man.source.mtime == mtime
                    && used_images_ok(&cache_dir, &man.definition)
                {
                    return Ok(SsfBundle {
                        id: id.to_string(),
                        cache_dir,
                        def: man.definition,
                        fp: man.fp,
                    });
                }
            }
            // 负缓存命中（源未变且上次解包失败）→ 直接报上次的错，
            // 不再在 host 锁内周期性重解 64MB 坏皮
            if let Some(e) = fail_cached(&src, size, mtime) {
                return Err(e);
            }
            unbundle(data_dir, &src)
        }
        Err(_) => {
            // 源缺失：缓存降级（无 source 校验——能凑合渲染就行）
            if let Some(man) = read_manifest(&cache_dir) {
                if used_images_ok(&cache_dir, &man.definition) {
                    return Ok(SsfBundle {
                        id: id.to_string(),
                        cache_dir,
                        def: man.definition,
                        fp: man.fp,
                    });
                }
            }
            Err(format!("皮肤文件不存在: {}", src.display()))
        }
    }
}

/// 解包 .ssf → 全条目落 `皮肤缓存\{stem消毒}\` + 写 manifest。
/// 任何失败返回 Err 字符串（绝不 panic——server 崩了输入法就完了）。
pub fn unbundle(data_dir: &Path, src: &Path) -> Result<SsfBundle, String> {
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "文件名非 UTF-8".to_string())?
        .to_string();
    let src_fp = (src.to_path_buf(), mtime_nanos(src));
    let bytes = std::fs::read(src).map_err(|e| format!("读取失败: {e}"))?;
    let r = unbundle_bytes(data_dir, &stem, src, &bytes);
    match &r {
        Ok(_) => fail_clear(src),
        Err(e) => fail_remember(src, bytes.len() as u64, src_fp.1, e),
    }
    r
}

/// unbundle 的内存版（导入路径直接用收到的字节；HTTP 显式动作不受
/// 负缓存约束——用户操作理应重试）。
fn unbundle_bytes(
    data_dir: &Path,
    stem: &str,
    src: &Path,
    bytes: &[u8],
) -> Result<SsfBundle, String> {
    if bytes.len() > hufu_skin::ssf::MAX_INPUT_BYTES {
        return Err(format!(
            "皮肤文件超过 {} MiB 输入上限",
            hufu_skin::ssf::MAX_INPUT_BYTES / (1 << 20)
        ));
    }
    // 一站式解析（Lead 要点 #1）：错误消息已是中文、可直接进设置页
    let (files, def) = hufu_skin::ssf::parse(bytes).map_err(|e| format!("{e}"))?;
    let cache_dir = ssf_cache_root(data_dir).join(sanitize_stem(stem));
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("缓存目录创建失败: {e}"))?;
    // 全条目落盘（写盘前逐条再验文件名安全——二次防御）
    let mut kept: Vec<String> = Vec::new();
    for (name, data) in files.iter() {
        let Some(rel) = safe_rel_path(name) else {
            eprintln!("ssf 条目名不安全，跳过落盘: {name:?}");
            continue;
        };
        let dest = cache_dir.join(&rel);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&dest, data).map_err(|e| format!("缓存写入失败（{name}）: {e}"))?;
        kept.push(rel.to_string_lossy().replace('\\', "/"));
    }
    // 新条目集之外的旧缓存清理（换版本防残留；manifest 马上重写故跳过）
    if let Ok(rd) = std::fs::read_dir(&cache_dir) {
        for e in rd.flatten() {
            let p = e.path();
            let rel = p
                .strip_prefix(&cache_dir)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            if rel == MANIFEST_FILE || kept.iter().any(|k| *k == rel) {
                continue;
            }
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            } else {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    let fp = source_fp(bytes);
    let man = Manifest {
        fp: fp.clone(),
        source: ManifestSource {
            size: bytes.len() as u64,
            mtime: mtime_nanos(src),
        },
        definition: def.clone(),
    };
    let body =
        serde_json::to_vec(&man).map_err(|e| format!("manifest 序列化失败: {e}"))?;
    std::fs::write(cache_dir.join(MANIFEST_FILE), body)
        .map_err(|e| format!("manifest 写入失败: {e}"))?;
    Ok(SsfBundle {
        id: stem.to_string(),
        cache_dir,
        def,
        fp,
    })
}

// ── 枚举 / 导入 / 预览 / 删除 ────────────────────────────────────

/// 枚举 皮肤\*.ssf（≤256，字母序）。每个文件尝试取可用包：缓存命中
/// 即便宜；首次/过期会解包建缓存。解析失败也列出（error 带原因）。
/// 【QPYS 留档】.qpys 已摘除（x.eq_ignore_ascii_case("qpys") 分支留档）。
pub fn list_ssf(data_dir: &Path) -> Vec<SsfSkinEntry> {
    let dir = ssf_skins_dir(data_dir);
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .map(|x| x.eq_ignore_ascii_case("ssf"))
                        .unwrap_or(false)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    paths.sort();
    let mut out = Vec::new();
    for p in paths.into_iter().take(MAX_SSF_LISTED) {
        let id = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if !valid_id(&id) {
            continue; // 文件名本身无法安全引用（保留名等）——不列
        }
        match bundle_for(data_dir, &id) {
            Ok(b) => out.push(SsfSkinEntry {
                name: if b.def.name.is_empty() { id.clone() } else { b.def.name.clone() },
                author: b.def.author.clone(),
                error: None,
                preview: thumb_source(&b.def).is_some(),
                warnings: b.def.warnings.clone(),
                id: b.id,
                fp: b.fp,
            }),
            Err(e) => out.push(SsfSkinEntry {
                name: id.clone(),
                author: String::new(),
                error: Some(e),
                preview: false,
                warnings: Vec::new(),
                id,
                fp: String::new(),
            }),
        }
    }
    out
}

/// 导入独立皮肤（.ssf / .qpys）：bytes 原样落 `皮肤\{name}.{ext}`（name
/// 消毒：?name= 优先、否则皮肤内 skin_name、再否则「导入皮肤」），随后
/// 解包建缓存。坏文件绝不落盘（先解后写）。返回 (id, warnings)。
pub fn import_ssf(
    data_dir: &Path,
    bytes: &[u8],
    qname: Option<&str>,
) -> Result<(String, Vec<String>), String> {
    if bytes.is_empty() {
        return Err("导入数据为空".into());
    }
    if bytes.len() > hufu_skin::ssf::MAX_INPUT_BYTES {
        return Err(format!(
            "皮肤超过 {} MiB 输入上限",
            hufu_skin::ssf::MAX_INPUT_BYTES / (1 << 20)
        ));
    }
    // 先解（不落盘）：坏 .ssf / 不支持的容器直接报错，不进皮肤目录
    let (_files, def) = hufu_skin::ssf::parse(bytes).map_err(|e| format!("{e}"))?;
    // 【QPYS 留档】QQ 判型已摘除：let ext = if is_qpys(bytes) {"qpys"} else {"ssf"};
    let ext = "ssf";
    let stem = match qname.map(str::trim).filter(|s| !s.is_empty()) {
        Some(n) => {
            // 去伪路径成分 + 去扩展名（浏览器文件名可能带路径/大小写后缀）
            let leaf = n.rsplit(['\\', '/']).next().unwrap_or(n);
            sanitize_stem(
                leaf.trim_end_matches(".qpys")
                    .trim_end_matches(".QPYS")
                    .trim_end_matches(".ssf")
                    .trim_end_matches(".SSF"),
            )
        }
        None => {
            if def.name.trim().is_empty() {
                sanitize_stem("导入皮肤")
            } else {
                sanitize_stem(&def.name)
            }
        }
    };
    let fname = format!("{stem}.{ext}");
    let dir = ssf_skins_dir(data_dir);
    let _ = std::fs::create_dir_all(&dir);
    // 上限校验（同名覆盖不算新增）
    let count = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    e.path()
                        .extension()
                        .map(|x| x.eq_ignore_ascii_case("ssf"))
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);
    if count >= MAX_SSF_LISTED && !dir.join(&fname).exists() {
        return Err(format!("皮肤目录 .ssf 已达上限 {MAX_SSF_LISTED}"));
    }
    let p = dir.join(&fname);
    // 新导入是显式用户动作：清负缓存，强制真解一遍
    fail_clear(&p);
    std::fs::write(&p, bytes).map_err(|e| format!("写入失败: {e}"))?;
    let b = unbundle(data_dir, &p)?;
    Ok((b.id, b.def.warnings.clone()))
}

/// 缩略图来源（Lead 要点 #4）：优先 manifest.preview（preview_comp/
/// preview_square）；皮肤无预览图（如虎娘 默认.ssf）→ 回退第一个
/// 存在变体的背景图（变体顺序与 SsfSchemes::all 一致，横排单窗优先）。
fn thumb_source(def: &hufu_skin::ssf::SsfDefinition) -> Option<String> {
    if !def.preview.is_empty() {
        return Some(def.preview.clone());
    }
    let s = &def.schemes;
    let first = [
        &s.horizontal,
        &s.vertical,
        &s.horizontal_code,
        &s.horizontal_candidates,
        &s.vertical_code,
        &s.vertical_candidates,
    ]
    .into_iter()
    .flatten()
    .next()?;
    Some(first.image.clone())
}

/// 预览图缓存文件路径（manifest.preview 指向的缓存内文件；空则回退
/// 首个变体背景图）。无可用图 / 文件缺失 / 皮肤不可用 → None（端点
/// 404）。
pub fn preview_file(data_dir: &Path, id: &str) -> Option<PathBuf> {
    if !valid_id(id) {
        return None;
    }
    let b = bundle_for(data_dir, id).ok()?;
    let name = thumb_source(&b.def)?;
    let rel = safe_rel_path(&name)?;
    let p = b.cache_dir.join(rel);
    p.is_file().then_some(p)
}

/// 【SSF mock 2026-10-16】图片白名单文件路径（仅该皮肤 used_images 内的
/// 归一名可读——防 ../ 穿越）。
pub fn image_file(data_dir: &Path, id: &str, f: &str) -> Option<PathBuf> {
    if !valid_id(id) {
        return None;
    }
    let b = bundle_for(data_dir, id).ok()?;
    let ok = b.def.preview == f || b.def.schemes.used_images().iter().any(|x| x == f);
    if !ok {
        return None;
    }
    let rel = safe_rel_path(f)?;
    let p = b.cache_dir.join(rel);
    p.is_file().then_some(p)
}

/// 【SSF mock 2026-10-16】设置页「真实候选窗」实时框元数据：选定朝向的
/// 布局（九宫格刀口/文字 insets）+ display（overrides 已 merge，色为
/// "#RRGGBB"）——UI 用 CSS border-image + 文字行复刻 DLL 渲染。
pub fn mock_meta(data_dir: &Path, id: &str, horizontal: bool) -> Result<serde_json::Value, String> {
    let b = bundle_for(data_dir, id)?;
    let d = &b.def;
    let ov = SsfOverrides::load(&b.cache_dir);
    let horizontal = ov.horizontal.unwrap_or(horizontal);
    let lay = d
        .schemes
        .choose(!horizontal, true, true)
        .or_else(|| d.schemes.choose(!horizontal, false, true))
        .ok_or_else(|| "皮肤无可用布局".to_string())?;
    let ins = |i: hufu_skin::ssf::SsfInsets| serde_json::json!([i.left, i.top, i.right, i.bottom]);
    let v = serde_json::json!({
        "name": d.name,
        "horizontal": horizontal,
        "image": lay.image,
        "img_url": format!("/api/skin/ssf-image?id={}&f={}", urlenc(id), urlenc(&lay.image)),
        "cut_h": [lay.horizontal.before, lay.horizontal.after],
        "cut_v": [lay.vertical.before, lay.vertical.after],
        "tile_h": lay.horizontal.tile,
        "tile_v": lay.vertical.tile,
        "insets_code": ins(lay.code),
        "insets_cand": ins(lay.candidates),
        "display": {
            "font_size": d.display.font_size,
            "font_ch": d.display.font_ch,
            "font_en": d.display.font_en,
            "code_color": ov.code_color.clone().unwrap_or_else(|| argb_to_rgba_hex(d.display.code_color)),
            "first_color": ov.first_color.clone().unwrap_or_else(|| argb_to_rgba_hex(d.display.first_color)),
            "text_color": ov.text_color.clone().unwrap_or_else(|| argb_to_rgba_hex(d.display.text_color)),
            "annotation_color": ov.annotation_color.clone().unwrap_or_else(|| argb_to_rgba_hex(d.display.annotation_color)),
            "candidate_spacing": d.display.candidate_spacing,
            "line_spacing": d.display.line_spacing,
            "character_spacing": d.display.character_spacing,
        },
        "font_pt": ov.font_pt.unwrap_or(16.0),
        "font_face": ov.font_face.clone().unwrap_or_default(),
        "show_index": ov.show_index,
    });
    Ok(v)
}

/// URL 查询参数编码（id/文件名是中文）。
fn urlenc(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 删除 .ssf/.qpys 源文件 + 缓存目录（独立皮肤用户自管理可删除；JSON
/// 皮肤不走此通道——恢复出厂语义只对官方 JSON 皮肤）。
pub fn remove_ssf(data_dir: &Path, id: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Err(format!("皮肤 id 非法: {id:?}"));
    }
    let Some(src) = source_for(data_dir, id) else {
        return Err(format!("皮肤 {id} 不存在"));
    };
    fail_clear(&src);
    std::fs::remove_file(&src).map_err(|e| format!("删除失败: {e}"))?;
    let _ = std::fs::remove_dir_all(ssf_cache_root(data_dir).join(id));
    Ok(())
}

// ── 用户覆盖设置（独立皮肤分页 2026-10-16）─────────────────────────
//
// .ssf 皮肤的颜色/布局/刀口全由皮肤文件决定，但用户层要能微调：字号
//（滚轮/设置页）、字体、横竖排、序号、四色覆盖。设置 per-skin 存缓存
// 目录 overrides.json——与皮肤同生命周期（删皮肤即删设置），不污染
// 全局 config。全部 Option：None=跟随皮肤/全局默认。

/// 独立皮肤的用户覆盖设置。
#[derive(serde::Serialize, serde::Deserialize, Clone, Default, PartialEq)]
#[serde(default)]
pub struct SsfOverrides {
    /// 候选字号 pt（滚轮/设置页调）。None=16 默认。
    pub font_pt: Option<f32>,
    /// 字体族覆盖。None=皮肤 display.font_ch。
    pub font_face: Option<String>,
    /// 横排=true。None=竖排（虎娘 layoutMode 6 默认语义）。
    pub horizontal: Option<bool>,
    /// 序号开关覆盖。None=全局 candidates.show_index。
    pub show_index: Option<bool>,
    /// 【候选高度 2026-10-16】候选区纵向偏移（屏幕像素，正=下移/负=上移，
    /// 仅独立皮肤）。透明皮肤元素的编码行与候选行视觉间距由皮肤位图
    /// 决定，此值整体挪动候选区起点补视觉缝隙。None=0（皮肤原布局）。
    /// DLL 侧换算：布局源域偏移 = 像素/缩放比，钳 ±500 后叠加。
    pub cand_y: Option<f32>,
    /// 【挂件随独立皮肤 2026-10-16】挂件配置快照（矢量皮肤 overlay 节
    /// 整棵：enabled/image/proc/布局）。None=无挂件。payload 注入顶层
    /// overlay——DLL parse_cfg 同键读取，零新消费代码。
    pub overlay: Option<serde_json::Value>,
    /// 四色覆盖（"#RRGGBBAA"）。None=皮肤原色。
    pub code_color: Option<String>,
    pub first_color: Option<String>,
    pub text_color: Option<String>,
    pub annotation_color: Option<String>,
}

const OVERRIDES_FILE: &str = "overrides.json";

impl SsfOverrides {
    /// 读皮肤覆盖设置（无文件/坏 JSON=全默认——设置坏不弄坏皮肤）。
    pub fn load(cache_dir: &Path) -> Self {
        let p = cache_dir.join(OVERRIDES_FILE);
        match std::fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, cache_dir: &Path) -> Result<(), String> {
        let p = cache_dir.join(OVERRIDES_FILE);
        let s = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&p, s).map_err(|e| format!("写入失败: {e}"))
    }

    /// 覆盖参与源指纹（改设置=DLL 皮肤指纹失效重拉——与皮肤文件变同口径）。
    /// overlay 含大图 base64（可达数 MB）——只进长度+哈希，原文不拼
    ///（指纹串每 2.5s 构造一次，MB 级拼接是纯浪费）。
    fn fp_tag(&self) -> String {
        let ov_tag = match &self.overlay {
            Some(v) => {
                let s = serde_json::to_string(v).unwrap_or_default();
                let mut h: u64 = 0xcbf29ce484222325;
                for b in s.as_bytes() {
                    h ^= *b as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
                format!("ov{}:{:016x}", s.len(), h)
            }
            None => "ov-".into(),
        };
        let mut s = serde_json::to_string(self).unwrap_or_default();
        // overlay 原文剔除（上面已换哈希标签）
        if let Some(v) = &self.overlay {
            let raw = serde_json::to_string(v).unwrap_or_default();
            if !raw.is_empty() && s.contains(&raw) {
                s = s.replace(&raw, "");
            }
        }
        format!("{s}|{ov_tag}")
    }
}

// ── 管道载荷 ─────────────────────────────────────────────────────

/// 构造 pipe skin op 下发的 ssf 皮肤对象（不含 anim 注入字段——pipe 侧
/// 与 JSON 皮肤同款注入）。返回 (skin 对象, 源指纹)。
///
/// 形状（DLL 按同形状消费；颜色已转 "#RRGGBBAA"）：
/// ```json
/// { "kind":"ssf", "name":"…", "author":"…",
///   "cache_dir":"…\\皮肤缓存\\{id}", "fp":"…",
///   "display":{ "font_size":…, "font_ch":"…", "font_en":"…",
///     "code_color":"#3B5179FF", "first_color":"…", "text_color":"…",
///     "annotation_color":"…", "candidate_spacing":…, "line_spacing":…,
///     "character_spacing":… },
///   "layouts": { "horizontal": {…SsfLayout…}, …仅存在变体… } }
/// ```
pub fn skin_payload(data_dir: &Path, id: &str) -> Result<(serde_json::Value, String), String> {
    let b = bundle_for(data_dir, id)?;
    let d = &b.def;
    // 【独立皮肤设置 2026-10-16】用户覆盖 merge 进载荷：display 四色、
    // layout 注入 font_point/font_face/horizontal/show_index——DLL 侧
    // 读取路径（layout_f / horizontal_early / show_index）与 JSON 皮肤
    // 同键，零新消费代码。None 字段不注入（皮肤原值/默认值生效）。
    let ov = SsfOverrides::load(&b.cache_dir);
    let code_color = ov.code_color.clone().unwrap_or_else(|| argb_to_rgba_hex(d.display.code_color));
    let first_color = ov.first_color.clone().unwrap_or_else(|| argb_to_rgba_hex(d.display.first_color));
    let text_color = ov.text_color.clone().unwrap_or_else(|| argb_to_rgba_hex(d.display.text_color));
    let annotation_color = ov.annotation_color.clone().unwrap_or_else(|| argb_to_rgba_hex(d.display.annotation_color));
    // 【霞鹜默认 2026-10-16】皮肤自带 font_ch 是雅黑缺省（搜狗皮肤
    // common.ini 惯例值=「没写字体」）时视为空——不注入 layout.
    // font_face，DLL 空默认走「装了霞鹜用霞鹜、没装回退雅黑」（用户
    // 拍板所有自带默认皮肤用霞鹜）。真要雅黑：设置页字体 override。
    let font_ch_eff = {
        let f = d.display.font_ch.trim();
        if f.is_empty() || f.eq_ignore_ascii_case("microsoft yahei ui") || f == "微软雅黑" {
            String::new()
        } else {
            f.to_string()
        }
    };
    let display = serde_json::json!({
        "font_size": d.display.font_size,
        "font_ch": font_ch_eff,
        "font_en": d.display.font_en,
        "code_color": code_color,
        "first_color": first_color,
        "text_color": text_color,
        "annotation_color": annotation_color,
        "candidate_spacing": d.display.candidate_spacing,
        "line_spacing": d.display.line_spacing,
        "character_spacing": d.display.character_spacing,
    });
    let mut layout = serde_json::Map::new();
    if let Some(pt) = ov.font_pt {
        layout.insert("font_point".into(), serde_json::json!(pt));
    }
    if let Some(ff) = &ov.font_face {
        if !ff.is_empty() {
            layout.insert("font_face".into(), serde_json::json!(ff));
        }
    }
    if let Some(h) = ov.horizontal {
        layout.insert("horizontal".into(), serde_json::json!(h));
    }
    if let Some(si) = ov.show_index {
        layout.insert("show_index".into(), serde_json::json!(si));
    }
    // 【候选高度 2026-10-16】像素值原样下发（布局源域换算在 DLL——
    // 那里有 scale），layout.cand_y 与 font_point 等同通道。
    if let Some(y) = ov.cand_y {
        layout.insert("cand_y".into(), serde_json::json!(y));
    }
    let mut sv = serde_json::json!({
        "kind": "ssf",
        "name": d.name,
        "author": d.author,
        "cache_dir": b.cache_dir.to_string_lossy(),
        "fp": b.fp,
        "display": display,
        "layouts": layout_variants(&d.schemes),
    });
    if !layout.is_empty() {
        sv["layout"] = serde_json::Value::Object(layout);
    }
    // 【挂件随独立皮肤 2026-10-16】overlay 快照注入顶层——DLL parse_cfg
    //（pointer("/skin/overlay") or 顶层 overlay）同键命中，与矢量皮肤
    // 完全同链路（sync/帧钟/代次推进零改动）。
    if let Some(ov) = &ov.overlay {
        if ov.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false) {
            sv["overlay"] = ov.clone();
        }
    }
    // 覆盖参与指纹：设置变=DLL 重拉重布局（滚轮字号立即生效的链路基础）
    let fp = format!("{}|{}", b.fp, ov.fp_tag());
    Ok((sv, fp))
}

/// layouts 只放存在变体；键=变体名（DLL 按排版逐级回退自己选）。
fn layout_variants(s: &hufu_skin::ssf::SsfSchemes) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    let mut put = |k: &str, l: Option<&hufu_skin::ssf::SsfLayout>| {
        if let Some(l) = l {
            if let Ok(v) = serde_json::to_value(l) {
                m.insert(k.to_string(), v);
            }
        }
    };
    put("horizontal", s.horizontal.as_ref());
    put("vertical", s.vertical.as_ref());
    put("horizontal_code", s.horizontal_code.as_ref());
    put("horizontal_candidates", s.horizontal_candidates.as_ref());
    put("vertical_code", s.vertical_code.as_ref());
    put("vertical_candidates", s.vertical_candidates.as_ref());
    m
}

// ── 测试 ─────────────────────────────────────────────────────────
// M1（hufu_skin::ssf）落地前本模块无法编译——测试随 Lead 完成后一并跑。

#[cfg(test)]
mod tests {
    use super::*;

    // ── 手写最小 stored（无压缩）ZIP：M1 明文 ZIP 路径的最小样本 ──

    fn crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let name_b = name.as_bytes();
            let crc = crc32(data);
            let off = out.len() as u32;
            // local file header
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&20u16.to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&0u16.to_le_bytes()); // method=stored
            out.extend_from_slice(&0u16.to_le_bytes()); // time
            out.extend_from_slice(&0u16.to_le_bytes()); // date
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // csize
            out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // usize
            out.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra len
            out.extend_from_slice(name_b);
            out.extend_from_slice(data);
            // central directory record
            central.extend_from_slice(b"PK\x01\x02");
            central.extend_from_slice(&20u16.to_le_bytes()); // version made by
            central.extend_from_slice(&20u16.to_le_bytes()); // version needed
            central.extend_from_slice(&0u16.to_le_bytes()); // flags
            central.extend_from_slice(&0u16.to_le_bytes()); // stored
            central.extend_from_slice(&0u16.to_le_bytes()); // time
            central.extend_from_slice(&0u16.to_le_bytes()); // date
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // extra
            central.extend_from_slice(&0u16.to_le_bytes()); // comment
            central.extend_from_slice(&0u16.to_le_bytes()); // disk start
            central.extend_from_slice(&0u16.to_le_bytes()); // inner attrs
            central.extend_from_slice(&0u32.to_le_bytes()); // outer attrs
            central.extend_from_slice(&off.to_le_bytes());
            central.extend_from_slice(name_b);
        }
        let cd_off = out.len() as u32;
        out.extend_from_slice(&central);
        // EOCD
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        out
    }

    fn tmp_data_dir(tag: &str) -> PathBuf {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("ssf-tests")
            .join(format!(
                "{tag}-{}-{:x}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    const INI: &str = "\u{feff}[General]\r\nskin_name=测试皮肤\r\nskin_author=虎符测试\r\npreview_comp=preview.png\r\n\
        [Display]\r\nfont_size=16\r\nfont_ch=微软雅黑\r\nfont_en=Segoe UI\r\n\
        pinyin_color=0x79513b\r\nzhongwen_color=0x624433\r\nzhongwen_first_color=0xa85326\r\ncomphint_color=0x927b68\r\n\
        [Scheme_H1]\r\npic=panel.png\r\nlayout_horizontal=0,90,90\r\nlayout_vertical=0,0,0\r\n\
        pinyin_marge=8,8,20,20\r\nzhongwen_marge=6,6,18,18\r\n";

    fn sample_zip() -> Vec<u8> {
        let png = [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3, 4];
        build_zip(&[
            ("skin.ini", INI.as_bytes()),
            ("panel.png", &png),
            ("preview.png", &png),
        ])
    }

    #[test]
    fn import_unbundle_cache_payload_roundtrip() {
        let data = tmp_data_dir("rt");
        let zip = sample_zip();
        let (id, _warnings) = import_ssf(&data, &zip, None).expect("导入成功");
        assert_eq!(id, "测试皮肤", "缺省文件名=皮肤内 skin_name 消毒");
        // 源文件 + 缓存布局
        assert!(ssf_skins_dir(&data).join("测试皮肤.ssf").is_file());
        let cache = ssf_cache_root(&data).join("测试皮肤");
        assert!(cache.join("skin.ini").is_file());
        assert!(cache.join("panel.png").is_file());
        assert!(cache.join("preview.png").is_file());
        assert!(cache.join(MANIFEST_FILE).is_file());
        // 二次取用（应命中缓存——不重解包也能出同值）
        let b = bundle_for(&data, &id).expect("缓存取用");
        assert_eq!(b.def.name, "测试皮肤");
        assert_eq!(b.def.author, "虎符测试");
        // 指纹格式 "{size}-{16 hex}"
        let (sz, hex) = b.fp.split_once('-').expect("指纹含 -");
        assert_eq!(sz.parse::<u64>().unwrap(), zip.len() as u64);
        assert_eq!(hex.len(), 16);
        // 管道载荷形状 + 颜色契约（0x79513b BGR → ARGB 0xFF3B5179 → #3B5179FF）
        let (sv, fp) = skin_payload(&data, &id).expect("载荷");
        assert_eq!(sv["kind"], "ssf");
        assert_eq!(sv["name"], "测试皮肤");
        assert_eq!(sv["display"]["code_color"], "#3B5179FF");
        assert!(
            sv["display"]["first_color"].as_str().unwrap().starts_with('#'),
            "first_color 已转 #RRGGBBAA"
        );
        // 【独立皮肤设置 2026-10-16】返回指纹=源指纹+overrides 串（设置
        // 变=DLL 重拉）；载荷 fp 仍=纯源指纹（位图缓存键——设置变不重
        // 解码位图）。断言改为前缀包含关系。
        assert!(fp.starts_with(&format!("{}|", sv["fp"].as_str().unwrap())), "返回指纹含载荷 fp 前缀: {fp}");
        assert!(sv["cache_dir"].as_str().unwrap().contains("皮肤缓存"));
        assert!(
            sv["layouts"]["horizontal"].is_object(),
            "horizontal 变体存在（pic+layout_horizontal）"
        );
        // 预览文件
        let pv = preview_file(&data, &id).expect("预览存在");
        assert!(pv.ends_with("preview.png"));
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn cache_invalidated_on_source_change() {
        let data = tmp_data_dir("rehash");
        let zip = sample_zip();
        let (id, _) = import_ssf(&data, &zip, None).unwrap();
        let fp0 = bundle_for(&data, &id).unwrap().fp;
        // 重写一个内容不同的包（同名覆盖）→ 指纹必须变
        let png2 = [9u8, 9, 9, 9];
        let zip2 = build_zip(&[
            ("skin.ini", INI.as_bytes()),
            ("panel.png", &png2),
        ]);
        // 手动隔开 mtime（NTFS 100ns 粒度下连写可能同戳——改字节确保 sha 变）
        std::fs::write(ssf_skins_dir(&data).join(format!("{id}.ssf")), &zip2).unwrap();
        let fp1 = bundle_for(&data, &id).unwrap().fp;
        assert_ne!(fp0, fp1, "内容变化 → sha 指纹变化");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn source_deleted_falls_back_to_cache_then_error() {
        let data = tmp_data_dir("del");
        let zip = sample_zip();
        let (id, _) = import_ssf(&data, &zip, None).unwrap();
        let src = ssf_skins_dir(&data).join(format!("{id}.ssf"));
        std::fs::remove_file(&src).unwrap();
        // 运行中被删 → 缓存继续用（设计稿降级矩阵）
        assert!(bundle_for(&data, &id).is_ok(), "缓存降级可用");
        // 缓存也删了 → 明确报错
        let _ = std::fs::remove_dir_all(ssf_cache_root(&data).join(&id));
        assert!(bundle_for(&data, &id).is_err());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn import_rejects_garbage_and_bad_names() {
        let data = tmp_data_dir("bad");
        assert!(import_ssf(&data, b"not an archive at all", None).is_err());
        // query 名带路径穿越 → 消毒成叶名
        let zip = sample_zip();
        let (id, _) = import_ssf(&data, &zip, Some("..\\..\\evil\\名字.ssf")).unwrap();
        assert_eq!(id, "名字");
        assert!(ssf_skins_dir(&data).join("名字.ssf").is_file());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn remove_ssf_deletes_source_and_cache() {
        let data = tmp_data_dir("rm");
        let zip = sample_zip();
        let (id, _) = import_ssf(&data, &zip, None).unwrap();
        remove_ssf(&data, &id).unwrap();
        assert!(!ssf_skins_dir(&data).join(format!("{id}.ssf")).exists());
        assert!(!ssf_cache_root(&data).join(&id).exists());
        assert!(remove_ssf(&data, &id).is_err(), "再删报不存在");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn list_marks_broken_skin_with_error() {
        let data = tmp_data_dir("broken");
        let dir = ssf_skins_dir(&data);
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("坏皮肤.ssf"), b"garbage-bytes-not-archiv").unwrap();
        let list = list_ssf(&data);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "坏皮肤");
        assert_eq!(list[0].name, "坏皮肤", "解析失败 name 回退文件名");
        assert!(list[0].error.is_some(), "error 带原因: {:?}", list[0].error);
        assert!(!list[0].preview);
        let _ = std::fs::remove_dir_all(&data);
    }

    /// 真实语料回归（与 hufu-skin M1 同款门控）：HUFU_SSF_CORPUS 指向
    /// 存放 .ssf 的目录时，对每个皮肤走完整 server 链——导入→缓存→
    /// 二次取用（走缓存）→管道载荷→预览回退。语料勿入库。
    #[test]
    fn real_corpus_server_pipeline() {
        let Ok(corpus) = std::env::var("HUFU_SSF_CORPUS") else {
            eprintln!("（未设 HUFU_SSF_CORPUS，跳过真实语料回归）");
            return;
        };
        let mut names: Vec<PathBuf> = std::fs::read_dir(&corpus)
            .expect("语料目录可读")
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .map(|x| x.eq_ignore_ascii_case("ssf"))
                    .unwrap_or(false)
            })
            .collect();
        names.sort();
        assert!(names.len() >= 5, "语料应含至少 5 个 .ssf（实测 10）");
        let data = tmp_data_dir("corpus");
        for p in &names {
            let bytes = std::fs::read(p).unwrap();
            let tag = p.file_stem().unwrap().to_string_lossy().into_owned();
            // 导入（query 名固定 latin 标签，避免语料中文名与消毒规则耦合）
            let (id, _w) = import_ssf(&data, &bytes, Some(&format!("{tag}.ssf")))
                .unwrap_or_else(|e| panic!("{tag} 导入失败: {e}"));
            // 缓存命中二次取用
            let b = bundle_for(&data, &id)
                .unwrap_or_else(|e| panic!("{tag} 缓存取用失败: {e}"));
            assert!(!b.def.name.is_empty());
            // 管道载荷形状
            let (sv, fp) = skin_payload(&data, &id)
                .unwrap_or_else(|e| panic!("{tag} 载荷失败: {e}"));
            assert_eq!(sv["kind"], "ssf");
            assert_eq!(sv["fp"], fp);
            assert!(
                sv["display"]["code_color"].as_str().unwrap().len() == 9,
                "{tag} 颜色已转 #RRGGBBAA"
            );
            assert!(
                sv["layouts"].as_object().map(|m| !m.is_empty()).unwrap_or(false),
                "{tag} 至少一个布局变体"
            );
            // 预览或背景回退至少一个可用
            assert!(
                preview_file(&data, &id).is_some(),
                "{tag} 预览/背景回退可用"
            );
        }
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn sanitize_stem_rules() {
        assert_eq!(sanitize_stem("蓝鲸雪语"), "蓝鲸雪语");
        assert_eq!(sanitize_stem("a/b\\c:d"), "abcd");
        assert_eq!(sanitize_stem("  . "), "ssf");
        assert_eq!(sanitize_stem("CON"), "CON_");
        assert_eq!(sanitize_stem("com1.png"), "com1.png_");
        let long = "x".repeat(100);
        assert_eq!(sanitize_stem(&long).len(), 64);
    }

    #[test]
    fn safe_rel_path_rejects_traversal() {
        assert!(safe_rel_path("skin.ini").is_some());
        assert!(safe_rel_path("a/panel.png").is_some());
        assert!(safe_rel_path("../evil").is_none());
        assert!(safe_rel_path("a/../../evil").is_none());
        assert!(safe_rel_path("con.ini").is_none(), "保留名");
        assert!(safe_rel_path("").is_none());
    }

    #[test]
    fn argb_hex_contract() {
        // Lead 契约修正实例：0x79513b(BGR) → ARGB 0xFF3B5179 → #3B5179FF
        assert_eq!(argb_to_rgba_hex(0xFF3B5179), "#3B5179FF");
        assert_eq!(argb_to_rgba_hex(0x00000000), "#00000000");
        assert_eq!(argb_to_rgba_hex(0x80102030), "#10203080");
    }

    #[test]
    fn source_fp_shape() {
        let a = source_fp(b"abc");
        let b = source_fp(b"abd");
        assert!(a.starts_with("3-") && a.len() == 18, "size-16hex: {a}");
        assert_ne!(a, b);
        assert_eq!(a, source_fp(b"abc"), "确定性");
    }
}
