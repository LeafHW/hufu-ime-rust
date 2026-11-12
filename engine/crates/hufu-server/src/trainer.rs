//! Trainer API. Hints read canonical scheme files and persisted adjustments.
//! File reloads, segmentation and candidate lookups never own the input lock.
use crate::{
    host::Host,
    http::{Request, Response},
};
use hufu_config::Config;
use hufu_dict::Schema;
use serde::{Deserialize, Serialize};
use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    hash::{Hash, Hasher},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Duration, Instant},
};
mod rules;
mod sources;

const MAX_CHARS: usize = 10_000;
/// 【整改13】全文理论分析仅对 ≤2000 字文章开放（逐词聚合预算护栏）。
const FULL_ANALYSIS_MAX_CHARS: usize = 2_000;
static ANALYZING: AtomicBool = AtomicBool::new(false);
struct CachedAnalysis {
    text: String,
    breaks: Vec<usize>,
    strategy: HintStrategy,
    required_spans: Vec<RequiredSpan>,
    dynamic_boundary: Option<usize>,
    route_mode: RouteMode,
    version: String,
    single: bool,
    /// 【整改13】full_analysis 是否随响应缓存（同文同版本但 include_full
    /// 不同的请求不得互相命中）。
    include_full: bool,
    response: Response,
    // Retain dictionary allocations while the pointer identity is in a cache key.
    // This prevents allocator reuse from making a reloaded dictionary look unchanged.
    _dict: Arc<hufu_dict::Dict>,
    _personal: Arc<hufu_dict::Dict>,
}
static CACHE: Mutex<Option<CachedAnalysis>> = Mutex::new(None);

/// Word display routes preserve candidate rank; their objective is deliberately
/// separate from the pending-candidate keyboard model used to annotate them.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum HintStrategy {
    #[default]
    Shortest,
    Phrases,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum RouteMode {
    #[default]
    Display,
    Theory,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RequiredSpan {
    start: usize,
    len: usize,
}

fn parse_route_options(
    value: &serde_json::Value,
    chars: usize,
    breaks: &[usize],
) -> Result<(HintStrategy, Vec<RequiredSpan>), &'static str> {
    let strategy = value
        .get("strategy")
        .map_or(Ok(HintStrategy::default()), |v| {
            serde_json::from_value(v.clone()).map_err(|_| "strategy 必须为 shortest 或 phrases")
        })?;
    let required: Vec<RequiredSpan> = value.get("required_spans").map_or(Ok(Vec::new()), |v| {
        serde_json::from_value(v.clone())
            .map_err(|_| "required_spans 必须为含 start、len 的字符范围数组")
    })?;
    let mut previous_end = 0;
    for span in &required {
        let end = span.start.checked_add(span.len).ok_or("锁定词范围越界")?;
        if span.len == 0
            || span.start < previous_end
            || end > chars
            || breaks
                .iter()
                .any(|&point| span.start < point && point < end)
        {
            return Err("锁定词范围必须依次排列、不重叠且不得跨段");
        }
        previous_end = end;
    }
    Ok((strategy, required))
}

fn parse_dynamic_boundary(
    value: Option<&serde_json::Value>,
    chars: usize,
) -> Result<Option<usize>, &'static str> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|&n| n <= chars)
            .map(Some)
            .ok_or("dynamic_boundary 必须为正文内的字符偏移"),
    }
}

fn valid_analysis_breaks(breaks: &[usize], chars: usize) -> bool {
    breaks.iter().all(|&point| point > 0 && point < chars)
        && breaks.windows(2).all(|pair| pair[0] < pair[1])
}

/// Break offsets count Unicode scalars, exactly like Word.start/len.
fn parse_analysis_breaks(
    value: Option<&serde_json::Value>,
    chars: usize,
) -> Result<Vec<usize>, &'static str> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let error = "breaks 必须为严格递增的段落字符偏移数组（0 < 偏移 < 字数）";
    let values = value.as_array().ok_or(error)?;
    if values.len() > chars.saturating_sub(1) {
        return Err(error);
    }
    let breaks = values
        .iter()
        .map(|point| {
            point
                .as_u64()
                .and_then(|point| usize::try_from(point).ok())
                .ok_or(error)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !valid_analysis_breaks(&breaks, chars) {
        return Err(error);
    }
    Ok(breaks)
}
struct Admission;
impl Drop for Admission {
    fn drop(&mut self) {
        ANALYZING.store(false, Ordering::Release);
    }
}

pub const DISABLED_MESSAGE: &str = "跟打器内置入口已关闭，请先在虎符设置的概览页启用";

/// try_lock 未拿到引擎锁的瞬时失败文案（tray 据此识别为瞬时并静默重试）。
pub const BUSY_MESSAGE: &str = "输入引擎忙，请稍后重试";

fn source_key(host: &Host) -> sources::SourceKey {
    sources::SourceKey {
        data_dir: host.data_dir.clone(),
        dir: host.engine.schema.dir.clone(),
        name: host.engine.schema.name.clone(),
    }
}

enum SnapshotError {
    Transient(&'static str),
    Unavailable(String),
}

impl SnapshotError {
    fn message(&self) -> &str {
        match self {
            Self::Transient(reason) => reason,
            Self::Unavailable(reason) => reason,
        }
    }
}

fn snapshot(host: &Mutex<Host>) -> Result<(Schema, Config, bool, u64), SnapshotError> {
    let (key, cfg, sentence, config_revision) = {
        let h = host
            .try_lock()
            .map_err(|_| SnapshotError::Transient(BUSY_MESSAGE))?;
        (
            source_key(&h),
            h.engine.config.clone(),
            h.engine.sentence_active(),
            crate::config_rev(),
        )
    };
    // The canonical source is the current scheme's files. In particular, a main
    // table edit must update hints while the IME protects an unfinished input,
    // and transient session learning must not continually reroute static hints.
    // First load, later reparses and old dictionary destruction own no Host lock.
    let source = sources::get(key.clone());
    {
        let h = host
            .try_lock()
            .map_err(|_| SnapshotError::Transient(BUSY_MESSAGE))?;
        if source_key(&h) != key || crate::config_rev() != config_revision {
            return Err(SnapshotError::Transient("方案或配置已变更，正在更新词提"));
        }
    }
    let source = source.map_err(SnapshotError::Unavailable)?;
    validate_source_size(&source.schema).map_err(SnapshotError::Unavailable)?;
    Ok((
        source.schema.as_ref().clone(),
        cfg,
        sentence,
        source.revision,
    ))
}

/// 用户词/调整快照门限：只防「快照大到让分析变慢」，不是语义上限。
/// 2026-11-13：UserAdjust 加了按码索引后 apply/added 不再全量扫日志；
/// 校验本身是 O(条目长度和) 的一次扫描（10MB 文本仍是毫秒级），因此放宽到
/// 10 万条 / 10MB（实测 3.8k 条调整 + 3.1k 用户词的方案可直接出词提）。
const SNAPSHOT_ENTRY_LIMIT: usize = 100_000;
const SNAPSHOT_BYTES_LIMIT: usize = 10 * 1024 * 1024;

fn validate_source_size(state: &Schema) -> Result<(), String> {
    if state.user_dict.entries.len() > SNAPSHOT_ENTRY_LIMIT
        || state.user_dict.weights.len() > SNAPSHOT_ENTRY_LIMIT
        || state.user_dict.hidden.len() > SNAPSHOT_ENTRY_LIMIT
        || state.adjust.operation_count() > SNAPSHOT_ENTRY_LIMIT
        || state.adjust.weights.len() > SNAPSHOT_ENTRY_LIMIT
    {
        return Err(format!(
            "用户词或调整过多（>{SNAPSHOT_ENTRY_LIMIT} 条），已跳过词提以保持输入响应；"
        ));
    }
    let bytes = state
        .user_dict
        .entries
        .iter()
        .map(|e| e.code.len().saturating_add(e.text.len()))
        .sum::<usize>()
        .saturating_add(state.adjust.snapshot_bytes())
        .saturating_add(
            state
                .user_dict
                .hidden
                .iter()
                .map(|(c, t)| c.len().saturating_add(t.len()))
                .sum::<usize>(),
        )
        .saturating_add(
            state
                .user_dict
                .weights
                .keys()
                .map(|(c, t)| c.len().saturating_add(t.len()))
                .sum::<usize>(),
        );
    if bytes > SNAPSHOT_BYTES_LIMIT {
        return Err(format!(
            "用户词快照过大（>{SNAPSHOT_BYTES_LIMIT} 字节），已跳过词提"
        ));
    }
    Ok(())
}

fn version(s: &Schema, cfg: &Config, sentence: bool) -> String {
    let mut h = DefaultHasher::new();
    s.name.hash(&mut h);
    (Arc::as_ptr(&s.dict) as usize).hash(&mut h);
    (Arc::as_ptr(&s.personal) as usize).hash(&mut h);
    s.adjust.to_lines().hash(&mut h);
    s.user_dict.to_lines().hash(&mut h);
    // 【补充语料词提 2026-11-11】语料内容变化必须失效词提缓存
    for e in &s.supplement.entries {
        e.word.hash(&mut h);
        e.weight.to_bits().hash(&mut h);
    }
    for e in &s.user_dict.entries {
        e.weight.to_bits().hash(&mut h);
    }
    for weights in [&s.user_dict.weights, &s.adjust.weights] {
        let mut sorted: Vec<_> = weights.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        for (key, value) in sorted {
            key.hash(&mut h);
            value.to_bits().hash(&mut h);
        }
    }
    // 【schema_ver 收敛 2026-11-08 深挖 P2】只哈希影响词提的配置节
    // （input/candidates/keymap/reverse/opencc——与 Analyzer 实际读取面一致）：
    // 改皮肤色/音效/trainer 开关等无关项不再无谓失效词提缓存。
    // JSON object order is stable (serde_json's default BTreeMap).
    for section in [
        serde_json::to_value(&cfg.input),
        serde_json::to_value(&cfg.candidates),
        serde_json::to_value(&cfg.keymap),
        serde_json::to_value(&cfg.reverse),
        serde_json::to_value(&cfg.opencc),
    ] {
        section.unwrap_or_default().to_string().hash(&mut h);
    }
    sentence.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn source_version(s: &Schema, cfg: &Config, sentence: bool, revision: u64) -> String {
    // Allocation addresses can be reused after A -> B -> A. The source cache's
    // monotonic revision gives each publication its own response identity.
    // Include the algorithm revision so a running trainer also invalidates its
    // routes after a server upgrade with unchanged dictionary files.
    format!("{}-{revision:x}-pagesel1-qt483", version(s, cfg, sentence))
}

fn select_action(action: &str) -> bool {
    action
        .strip_prefix("select")
        .and_then(|n| n.parse::<usize>().ok())
        .is_some_and(|n| (1..=10).contains(&n))
}

/// Windows physical VKs which may select a candidate in the active scheme.
/// This is an attribution hint, not a promise that a key is currently selectable:
/// code continuations and candidate availability remain owned by the live engine.
/// map_idle is deliberately excluded; the input has candidates when selecting.
fn selection_keys(cfg: &Config) -> Vec<u32> {
    let mut keys = std::collections::BTreeSet::new();
    for n in 0..10 {
        keys.insert(0x30 + n);
        keys.insert(0x60 + n);
    }
    let map = &cfg.keymap.map;
    for ch in [cfg.candidates.second_select, cfg.candidates.third_select]
        .into_iter()
        .chain(cfg.candidates.custom_select_keys.iter().copied())
    {
        let name = if ch == ' ' {
            "space".into()
        } else {
            ch.to_string()
        };
        if map.get(&name).is_some_and(|a| !select_action(a)) {
            continue;
        }
        if let Some(vk) = selection_char_vk(ch) {
            keys.insert(vk);
        }
    }
    for (name, action) in map {
        if !select_action(action) || matches!(name.as_str(), "shift" | "shiftleft" | "shiftright") {
            continue;
        }
        if let Some(vk) = selection_key_vk(name) {
            keys.insert(vk);
        }
    }
    // Engine lookup: side-specific Shift overrides the legacy generic binding.
    for (name, vk) in [("shiftleft", 0xA0), ("shiftright", 0xA1)] {
        if map
            .get(name)
            .or_else(|| map.get("shift"))
            .is_some_and(|a| select_action(a))
        {
            keys.insert(vk);
        }
    }
    keys.into_iter().collect()
}
fn selection_char_vk(ch: char) -> Option<u32> {
    Some(match ch {
        'a'..='z' => ch.to_ascii_uppercase() as u32,
        '0'..='9' => ch as u32,
        ' ' => 0x20,
        ';' => 0xBA,
        '=' => 0xBB,
        ',' => 0xBC,
        '-' => 0xBD,
        '.' => 0xBE,
        '/' => 0xBF,
        '`' => 0xC0,
        '[' => 0xDB,
        '\\' => 0xDC,
        ']' => 0xDD,
        '\'' => 0xDE,
        _ => return None,
    })
}
fn selection_key_vk(name: &str) -> Option<u32> {
    if name.chars().count() == 1 {
        return selection_char_vk(name.chars().next()?);
    }
    Some(match name {
        "space" => 0x20,
        "enter" => 0x0D,
        "tab" => 0x09,
        "backspace" => 0x08,
        "delete" => 0x2E,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "capslock" => 0x14,
        "numadd" => 0x6B,
        "numsub" => 0x6D,
        "nummul" => 0x6A,
        "numdiv" => 0x6F,
        "numdot" => 0x6E,
        "numenter" => 0x0E, // trainer's physical extended-Enter identity
        _ => {
            if let Some(n) = name
                .strip_prefix("num")
                .filter(|n| n.len() == 1)
                .and_then(|n| n.parse::<u32>().ok())
            {
                if n <= 9 {
                    return Some(0x60 + n);
                }
            }
            if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
                if (1..=12).contains(&n) && name == format!("f{n}") {
                    return Some(0x6F + n);
                }
            }
            return None;
        }
    })
}

pub fn executable() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|p| p.join("hufu-trainer.exe"))
        .filter(|p| p.is_file())
}

pub fn enabled(host: &Mutex<Host>) -> Result<bool, &'static str> {
    host.try_lock()
        .map(|h| h.engine.config.trainer.enabled)
        .map_err(|_| BUSY_MESSAGE)
}

pub fn open(host: &Mutex<Host>) -> Result<(), String> {
    if !enabled(host)? {
        return Err(DISABLED_MESSAGE.into());
    }
    let exe = executable().ok_or("跟打器尚未安装，请更新完整安装包")?;
    let mut cmd = std::process::Command::new(&exe); // zi-ok: verified sibling hufu-trainer.exe; no shell/interpreter
    cmd.current_dir(exe.parent().unwrap());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("无法打开跟打器：{e}"))
}

/// Win32 RegisterHotKey 修饰键位（与 tray.rs 手写 FFI 同值；Windows SDK 常量）。
pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;
pub const MOD_NOREPEAT: u32 = 0x4000;

/// 解析 trainer.hotkey 配置串 → (mods, vk)。
/// 格式：小写、加号分隔；修饰键 ctrl/alt/shift/win（至少一个，可组合、顺序任意）
/// + 主键 a-z / 0-9 / f1-f12。MOD_NOREPEAT 恒加（长按不重复触发）。
/// 空串或纯空白 = 禁用热键 → None；非法组合（裸主键/未知键名/缺主键）→ None。
/// 大小写宽容（手改 config.json 常见）。
pub fn parse_hotkey(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    if s.is_empty() {
        return None; // 空串=禁用
    }
    let binding = s.to_ascii_lowercase();
    let parts: Vec<&str> = binding
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() < 2 {
        return None; // 裸主键（无修饰键）会吞掉全局打字，拒绝注册
    }
    let (main, mods_str) = parts.split_last()?;
    let mut mods = MOD_NOREPEAT;
    let mut seen_any = false;
    for m in mods_str {
        let bit = match *m {
            "ctrl" | "control" => MOD_CONTROL,
            "alt" => MOD_ALT,
            "shift" => MOD_SHIFT,
            "win" | "meta" => MOD_WIN,
            _ => return None,
        };
        mods |= bit;
        seen_any = true;
    }
    if !seen_any {
        return None;
    }
    let vk = match main.chars().collect::<Vec<char>>()[..] {
        [c] if c.is_ascii_alphabetic() => c.to_ascii_uppercase() as u32,
        [c] if c.is_ascii_digit() => c as u32,
        _ => {
            let n: u32 = main.strip_prefix('f')?.parse().ok()?;
            (1..=12).contains(&n).then_some(0x6F + n)? // VK_F1=0x70 … VK_F12=0x7B
        }
    };
    Some((mods, vk))
}

pub fn route(host: &Mutex<Host>, req: &Request) -> Response {
    match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/api/trainer/open") => match open(host) {
            Ok(()) => Response::json(&serde_json::json!({"ok":true})),
            Err(e) => Response::err(if e == DISABLED_MESSAGE { 403 } else { 503 }, &e),
        },
        ("GET", "/api/trainer/status") => {
            // 【整改4 2026-11-08】整句模式词提已放开（码表编码口径），
            // 不再因 sentence_active 下发 hint_unavailable_reason；
            // 快照超限（用户词过大）仍是真实的词提不可用，保留。
            let (schema, schema_ver, enabled, max_code_length, sentence, unavailable, selections) =
                match snapshot(host) {
                    Ok((s, cfg, sentence, revision)) => (
                        s.name.clone(),
                        source_version(&s, &cfg, sentence, revision),
                        cfg.trainer.enabled,
                        cfg.input.max_code_length,
                        sentence,
                        None,
                        selection_keys(&cfg),
                    ),
                    Err(SnapshotError::Transient(reason)) => return Response::err(503, reason),
                    Err(SnapshotError::Unavailable(reason)) => {
                        // A dictionary snapshot limit disables hints, not the feature switch.
                        // Read only the small status fields without copying user dictionaries.
                        let h = match host.try_lock() {
                            Ok(h) => h,
                            Err(_) => return Response::err(503, &reason),
                        };
                        (
                            h.engine.schema.name.clone(),
                            String::new(),
                            h.engine.config.trainer.enabled,
                            h.engine.config.input.max_code_length,
                            h.engine.sentence_active(),
                            Some(reason),
                            selection_keys(&h.engine.config),
                        )
                    }
                };
            Response::json(
                &serde_json::json!({"enabled":enabled,"installed":executable().is_some(),
                "schema":schema,"schema_ver":schema_ver,
                "max_article_chars":MAX_CHARS,"max_code_length":max_code_length,
                "version":"0.1.0","hint_scope":"dictionary","sentence_active":sentence,
                "hint_unavailable_reason":unavailable,"selection_keys":selections,
                "route_rules":"quiettype-v1"}),
            )
        }
        ("POST", "/api/trainer/analyze") => {
            if req.body.len() > 128 * 1024 {
                return Response::err(413, "文章请求过大");
            }
            let v = req.json();
            let Some(text) = v.get("text").and_then(|v| v.as_str()) else {
                return Response::err(400, "需要 text 字符串");
            };
            let single = match v.get("single") {
                Some(value) => match value.as_bool() {
                    Some(single) => single,
                    None => return Response::err(400, "single 必须为布尔值"),
                },
                None => false,
            };
            // 【整改13 2026-11-08】include_full=true 时附全文理论码长/选重分析；
            // 仅文长 ≤2000 字时返回（预算护栏），否则静默省略 full_analysis 字段。
            let include_full = match v.get("include_full") {
                Some(value) => match value.as_bool() {
                    Some(b) => b,
                    None => return Response::err(400, "include_full 必须为布尔值"),
                },
                None => false,
            };
            // Theory requests describe dictionary recipes independently of the live
            // sentence decoder.  None preserves the ordinary word-hint contract.
            let theory_auto4 = match v.get("theory_auto4") {
                Some(value) => match value.as_bool() {
                    Some(auto4) => Some(auto4),
                    None => return Response::err(400, "theory_auto4 必须为布尔值"),
                },
                None => None,
            };
            let char_count = text.chars().count();
            if char_count > MAX_CHARS {
                return Response::err(413, "文章最多 10000 字");
            }
            let breaks = match parse_analysis_breaks(v.get("breaks"), char_count) {
                Ok(breaks) => breaks,
                Err(error) => return Response::err(400, error),
            };
            let (strategy, required_spans) = match parse_route_options(&v, char_count, &breaks) {
                Ok(options) => options,
                Err(error) => return Response::err(400, error),
            };
            if single && required_spans.iter().any(|span| span.len != 1) {
                return Response::err(400, "单字分析不能锁定多字词");
            }
            let dynamic_boundary =
                match parse_dynamic_boundary(v.get("dynamic_boundary"), char_count) {
                    Ok(boundary) => boundary,
                    Err(error) => return Response::err(400, error),
                };
            let route_mode: RouteMode = match v.get("route_mode") {
                None => RouteMode::Display,
                Some(value) => match serde_json::from_value(value.clone()) {
                    Ok(mode) => mode,
                    Err(_) => return Response::err(400, "route_mode 必须为 display 或 theory"),
                },
            };
            if route_mode == RouteMode::Theory && dynamic_boundary.is_some() {
                return Response::err(400, "理论分析使用完整正文，不接受动态词提边界");
            }
            if ANALYZING
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                return Response::err(429, "正在分析另一篇文章，请稍后重试");
            }
            let _admission = Admission;
            let started = Instant::now();
            let (s, cfg, sentence, revision) = match snapshot(host) {
                Ok(s) => s,
                Err(e) => return Response::err(503, e.message()),
            };
            let ver = source_version(&s, &cfg, sentence, revision);
            // Theory options change both recipe costs and DP segmentation.  Do not
            // consume or replace the ordinary hint cache with a theory response.
            if theory_auto4.is_none() {
                if let Some(cache) = CACHE.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
                    if cache.text == text
                        && cache.breaks == breaks
                        && cache.strategy == strategy
                        && cache.required_spans == required_spans
                        && cache.dynamic_boundary == dynamic_boundary
                        && cache.route_mode == route_mode
                        && cache.version == ver
                        && cache.single == single
                        && cache.include_full == include_full
                    {
                        return cache.response.clone();
                    }
                }
            }
            let mut analyzer = Analyzer::new(&s, &cfg, sentence && theory_auto4.is_none());
            analyzer.theory_auto4 = theory_auto4;
            analyzer.strategy = strategy;
            analyzer.required_spans = required_spans.clone();
            analyzer.dynamic_boundary = dynamic_boundary;
            analyzer.route_mode = route_mode;
            match analyzer.analyze_mode_with_breaks(text, started, single, &breaks) {
                Ok(words) => {
                    // 全文分析按词提分词口径逐词聚合；words 已在锁外算完，
                    // 这里只做算术聚合（同样在锁外）。rank>1 每词至多计 1 次选重。
                    let full_analysis = if include_full && char_count <= FULL_ANALYSIS_MAX_CHARS {
                        let mut entries = Vec::with_capacity(words.len());
                        let mut total_code_len = 0usize;
                        let mut select_count = 0usize;
                        for w in &words {
                            let code_len = w.hint.code.as_ref().map_or(0, |c| c.len());
                            total_code_len += code_len;
                            if w.hint.rank > 1 {
                                select_count += 1;
                            }
                            entries.push(serde_json::json!({
                                "word": w.text,
                                "code": w.hint.code,
                                "code_len": code_len,
                                "rank": w.hint.rank,
                                "tail": w.hint.tail,
                            }));
                        }
                        Some(serde_json::json!({
                            "entries": entries,
                            "total_code_len": total_code_len,
                            "avg_code_len_per_char": total_code_len as f64 / char_count as f64,
                            "select_count": select_count,
                        }))
                    } else {
                        None
                    };
                    let mut body = serde_json::json!({"schema":s.name,"schema_ver":ver,
                    "max_code_length":cfg.input.max_code_length,"hint_scope":"dictionary",
                    "sentence_active":sentence,"hint_unavailable_reason":null,
                    "single":single,"breaks":breaks,"words":words,"selection_keys":selection_keys(&cfg),
                    "strategy":strategy,"required_spans":required_spans,"dynamic_boundary":dynamic_boundary,"route_rules":"quiettype-v1",
                    "route_mode":route_mode,
                    "elapsed_ms":started.elapsed().as_secs_f64()*1000.0});
                    if let Some(fa) = full_analysis {
                        body["full_analysis"] = fa;
                    }
                    if let Some(auto4) = theory_auto4 {
                        body["theory_auto4"] = auto4.into();
                    }
                    let response = Response::json(&body);
                    if theory_auto4.is_none() {
                        *CACHE.lock().unwrap_or_else(|p| p.into_inner()) = Some(CachedAnalysis {
                            text: text.into(),
                            breaks,
                            strategy,
                            required_spans,
                            dynamic_boundary,
                            route_mode,
                            version: ver,
                            single,
                            include_full,
                            response: response.clone(),
                            _dict: s.dict.clone(),
                            _personal: s.personal.clone(),
                        });
                    }
                    response
                }
                Err(e) => Response::err(503, e),
            }
        }
        _ => Response::err(404, "not found"),
    }
}

#[derive(Clone, Serialize)]
struct Hint {
    code: Option<String>,
    rank: usize,
    tail: String,
}
impl Hint {
    fn missing() -> Self {
        Self {
            code: None,
            rank: 0,
            tail: String::new(),
        }
    }
}
#[derive(Serialize)]
struct CharHint {
    ch: char,
    #[serde(flatten)]
    hint: Hint,
}
#[derive(Serialize)]
struct Word {
    text: String,
    #[serde(flatten)]
    hint: Hint,
    level: &'static str,
    start: usize,
    len: usize,
    chars: Vec<CharHint>,
}

/// Test-only accounting for replaying standalone dictionary recipes.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
struct RouteCost {
    missing: usize,
    keys: usize,
    nonfirst: usize,
    tokens: usize,
    base: usize,
    singletons: usize,
    rank: usize,
}
#[cfg(test)]
impl RouteCost {
    fn token(text: &str, len: usize, hint: &Hint) -> Self {
        let literal = literal_key_count(text);
        Self {
            missing: usize::from(hint.code.is_none() && literal.is_none()) * len,
            keys: hint.code.as_ref().map_or_else(
                || literal.unwrap_or(0),
                |code| code.len() + hint.tail.chars().count(),
            ),
            nonfirst: usize::from(hint.code.is_some() && hint.rank > 1),
            tokens: 1,
            base: hint.code.as_ref().map_or(0, String::len),
            singletons: usize::from(len == 1),
            rank: hint.rank,
        }
    }
    fn then(self, rest: Self) -> Self {
        Self {
            missing: self.missing + rest.missing,
            keys: self.keys + rest.keys,
            nonfirst: self.nonfirst + rest.nonfirst,
            tokens: self.tokens + rest.tokens,
            base: self.base + rest.base,
            singletons: self.singletons + rest.singletons,
            rank: self.rank + rest.rank,
        }
    }
}

/// Literal punctuation stays in the route without being counted as a missing
/// dictionary character. This matches the trainer's theoretical physical keys
/// (Shift counts as a key); it does not invent a dictionary recipe for literals.
fn literal_key_count(text: &str) -> Option<usize> {
    if matches!(text, "……" | "——") {
        return Some(2);
    }
    text.chars().try_fold(0, |keys, ch| {
        let count = match ch {
            '\n' | '\r' => 0,
            '，' | '。' | '、' | '；' | '【' | '】' => 1,
            '：' | '！' | '？' | '（' | '）' | '“' | '”' | '《' | '》' | '￥' | '…' | '—' | '·' => {
                2
            }
            '‘' | '’' => 1,
            c if c.is_ascii() && !c.is_control() => {
                1 + usize::from(c.is_ascii_uppercase() || "~!@#$%^&*()_+{}|:\"<>?".contains(c))
            }
            _ => return None,
        };
        Some(keys + count)
    })
}

struct CachedWordLengths {
    dict: Weak<hufu_dict::Dict>,
    lengths: Arc<Vec<usize>>,
}
static WORD_LENGTHS: Mutex<Vec<CachedWordLengths>> = Mutex::new(Vec::new());

struct Analyzer<'a> {
    s: &'a Schema,
    cfg: &'a Config,
    sentence: bool,
    /// Explicit theoretical four-code auto commit; None follows runtime settings.
    theory_auto4: Option<bool>,
    strategy: HintStrategy,
    required_spans: Vec<RequiredSpan>,
    dynamic_boundary: Option<usize>,
    route_mode: RouteMode,
    extra: HashMap<String, Vec<String>>,
    extra_prefixes: HashSet<String>,
    /// 【补充语料词提 2026-11-11】(组合码, 词) 对：引擎把这些词注入整句
    /// 词图并用挂起机制让字词模式全码也能打出；词提必须同样可见，
    /// 否则正文里这些词永远没有提示（用户实测漏词）。
    supp_pairs: HashSet<(String, String)>,
    started: Instant,
    work: usize,
    exceeded: bool,
    #[cfg(test)]
    hints: HashMap<String, Hint>,
    candidates: HashMap<String, Vec<hufu_dict::DictEntry>>,
    route_hints: HashMap<String, Vec<(Hint, bool)>>,
}
impl<'a> Analyzer<'a> {
    fn new(s: &'a Schema, cfg: &'a Config, sentence: bool) -> Self {
        let mut extra: HashMap<String, Vec<String>> = HashMap::new();
        for (code, text) in s.adjust.added_words() {
            extra.entry(text).or_default().push(code);
        }
        for e in &s.user_dict.entries {
            extra
                .entry(e.text.clone())
                .or_default()
                .push(e.code.clone());
        }
        // 【补充语料词提 2026-11-11】组合码推导与 Engine::build_supplement_cache
        // 同口径：≥2 字、≤4 字、每字至多 4 码、每词至多 32 组合。
        // 单字不进（码表本身有单字，补充语料单字只是整句加权）。
        let mut supp_pairs: HashSet<(String, String)> = HashSet::new();
        for e in &s.supplement.entries {
            let chars: Vec<char> = e.word.chars().collect();
            if chars.len() < 2 || chars.len() > 4 {
                continue;
            }
            let mut combos: Vec<String> = vec![String::new()];
            for ch in chars {
                let codes = s.dict.codes_of(&ch.to_string());
                if codes.is_empty() {
                    combos.clear();
                    break;
                }
                let mut next = Vec::with_capacity(combos.len() * 4);
                for c in &combos {
                    for code in codes.iter().take(4) {
                        let mut merged = c.clone();
                        merged.push_str(code);
                        next.push(merged);
                    }
                }
                combos = next;
                if combos.len() > 32 {
                    combos.truncate(32);
                }
            }
            for c in combos {
                if supp_pairs.insert((c.clone(), e.word.clone())) {
                    extra.entry(e.word.clone()).or_default().push(c);
                }
            }
            if supp_pairs.len() > 20_000 {
                break;
            }
        }
        let mut extra_prefixes = HashSet::new();
        for codes in extra.values() {
            for code in codes {
                for (i, _) in code.char_indices().skip(1) {
                    extra_prefixes.insert(code[..i].to_string());
                }
                extra_prefixes.insert(code.clone());
            }
        }
        Self {
            s,
            cfg,
            sentence,
            theory_auto4: None,
            strategy: HintStrategy::default(),
            required_spans: Vec::new(),
            dynamic_boundary: None,
            route_mode: RouteMode::Display,
            extra,
            extra_prefixes,
            supp_pairs,
            started: Instant::now(),
            work: 0,
            exceeded: false,
            #[cfg(test)]
            hints: HashMap::new(),
            candidates: HashMap::new(),
            route_hints: HashMap::new(),
        }
    }
    fn mapped(&self, key: char, idle: bool) -> bool {
        let name = if key == ' ' {
            "space".into()
        } else {
            key.to_string()
        };
        let m = &self.cfg.keymap;
        if idle {
            m.map_idle.contains_key(&name) || m.map.contains_key(&name)
        } else {
            m.map.contains_key(&name)
        }
    }
    fn extends(&self, code: &str, key: char) -> bool {
        let probe = format!("{code}{key}");
        !self.s.dict.lookup(&probe).is_empty()
            || !self.s.dict.completions(&probe, 1).is_empty()
            || !self.s.personal.lookup(&probe).is_empty()
            || self.extra_prefixes.contains(&probe)
    }
    fn numeric_suffix_extends(&self, code: &str, key: char) -> bool {
        // on_char's ordinary digit path checks every raw suffix, even outside
        // sentence mode (e.g. ab + 4 remains code input when b4 exists).
        code.char_indices().any(|(i, _)| {
            let probe = format!("{}{key}", &code[i..]);
            !self.s.dict.lookup(&probe).is_empty() || !self.s.dict.completions(&probe, 1).is_empty()
        })
    }
    #[cfg(test)]
    fn tail(&self, code: &str, rank: usize, count: usize) -> Option<String> {
        self.selection_tail(code, rank, count, self.sentence, true)
    }
    fn selection_tail(
        &self,
        code: &str,
        rank: usize,
        count: usize,
        sentence: bool,
        allow_auto: bool,
    ) -> Option<String> {
        // The live z-domain also merges symbol tables and their continuations.
        // This dictionary-only snapshot cannot promise its displayed rank or auto commit.
        if self.s.dict.z_symbol_domain && code.starts_with('z') {
            return None;
        }
        if rank == 1 {
            if allow_auto && self.theory_auto4 == Some(true) && code.len() == 4 {
                return Some(String::new());
            }
            let auto = allow_auto
                && self.theory_auto4.is_none()
                && !sentence
                && count == 1
                && ((self.cfg.input.auto_select_unique
                    && code.len() >= 2
                    && code.len() == self.cfg.input.max_code_length)
                    || (self.s.dict.z_symbol_domain
                        && code.starts_with('z')
                        && code.len() >= 2
                        && !self.s.dict.has_longer(code)));
            if auto {
                return Some(String::new());
            }
            let c = &self.cfg.candidates;
            let space_select = if c.second_select == ' ' {
                2
            } else if c.third_select == ' ' {
                3
            } else {
                c.custom_select_keys
                    .iter()
                    .position(|&k| k == ' ')
                    .map_or(0, |i| i + 1)
            };
            let space_is_first = space_select == 1
                || space_select == 0
                    && !c.paging_keys.contains(' ')
                    && !self.cfg.input.alphabet.contains(' ');
            return (!self.mapped(' ', false)
                && self.cfg.reverse.prefix != ' '
                && space_is_first
                && !self.extends(code, ' '))
            .then(|| "_".into());
        }
        // The hint describes an explicit dictionary word, so sentence-mode rank
        // keys (which lock a segment without committing it) are not suggested.
        if sentence {
            return None;
        }
        if rank > 10 {
            // 【候选翻页可达 2026-11-12】引擎翻页键纯翻页（默认 -/=，
            // 2026-09-06 改判后不选定），第 11-20 名可「翻页×N + 页内选重键」
            // 打出；两页封顶控制 DP 枚举规模。翻页键本身必须对当前编码
            // 无延伸/映射冲突，且不得与选重键重叠。
            if rank > 20 {
                return None;
            }
            let c = &self.cfg.candidates;
            let paging = c.paging_keys.chars().find(|&p| {
                !(self.mapped(p, false)
                    || self.extends(code, p)
                    || p == self.cfg.reverse.prefix
                    || p.is_control()
                    || !p.is_ascii()
                    || p.is_ascii_uppercase()
                    || "~!@#$%^&*()_+{}|:\\\"<>?".contains(p)
                    || "_234567890".contains(p)
                    || p == c.second_select
                    || p == c.third_select
                    || c.custom_select_keys.contains(&p))
            })?;
            let mut tail = String::new();
            for _ in 0..(rank - 1) / 10 {
                tail.push(paging);
            }
            tail.push(self.selector_char(code, (rank - 1) % 10 + 1)?);
            return Some(tail);
        }
        self.selector_char(code, rank).map(|k| k.to_string())
    }

    /// 页内选重键（1..=10）：二选/三选/自定义优先，数字兜底，
    /// 与 on_rank_key 同序消解键位冲突；任一冲突即返回 None。
    /// 【候选翻页可达 2026-11-12】从 selection_tail 抽出，供第 11-20 名
    /// 翻页路径复用（页内位次 = (rank-1)%10+1）。
    fn selector_char(&self, code: &str, inpage: usize) -> Option<char> {
        let c = &self.cfg.candidates;
        let mut keys = Vec::new();
        if inpage == 2 {
            keys.push(c.second_select);
        }
        if inpage == 3 {
            keys.push(c.third_select);
        }
        if let Some(&k) = c.custom_select_keys.get(inpage - 1) {
            keys.push(k);
        }
        keys.push(if inpage == 10 {
            '0'
        } else {
            char::from_digit(inpage as u32, 10)?
        });
        keys.into_iter()
            .find(|&k| {
                if self.mapped(k, false)
                    || self.extends(code, k)
                    || k == self.cfg.reverse.prefix
                    || k.is_control()
                    || !k.is_ascii()
                    || k.is_ascii_uppercase()
                    || "~!@#$%^&*()_+{}|:\"<>?".contains(k)
                {
                    return false;
                }
                let explicit_select = k == c.second_select
                    || k == c.third_select
                    || c.custom_select_keys.contains(&k);
                if k.is_ascii_digit() && !explicit_select && self.numeric_suffix_extends(code, k) {
                    return false;
                }
                // Resolve conflicts with the same priority used by on_rank_key.
                let resolved = if k == c.second_select {
                    2
                } else if k == c.third_select {
                    3
                } else if let Some(i) = c.custom_select_keys.iter().position(|x| *x == k) {
                    i + 1
                } else if k.is_ascii_digit() {
                    if k == '0' {
                        10
                    } else {
                        (k as u8 - b'0') as usize
                    }
                } else {
                    0
                };
                resolved == inpage
                    && (!c.paging_keys.contains(k)
                        || k == c.second_select
                        || k == c.third_select
                        || c.custom_select_keys.contains(&k))
                    && (!self.cfg.input.alphabet.contains(k)
                        || k == c.second_select
                        || k == c.third_select
                        || c.custom_select_keys.contains(&k))
            })
    }
    fn budget(&mut self, cost: usize) -> bool {
        self.work = self.work.saturating_add(cost);
        if self.work > 500_000 || self.started.elapsed() > Duration::from_millis(250) {
            self.exceeded = true;
            false
        } else {
            true
        }
    }
    /// Immutable dictionary handles keep this metadata valid. Weak identities
    /// prevent allocator reuse while allowing replaced dictionaries to be freed.
    /// Build outside both the engine lock and this short-lived metadata lock.
    fn dictionary_lengths(
        &mut self,
        dict: &Arc<hufu_dict::Dict>,
    ) -> Result<Arc<Vec<usize>>, &'static str> {
        let identity = Arc::downgrade(dict);
        {
            let mut cache = WORD_LENGTHS.lock().unwrap_or_else(|p| p.into_inner());
            cache.retain(|entry| entry.dict.strong_count() > 0);
            if let Some(entry) = cache.iter().find(|entry| entry.dict.ptr_eq(&identity)) {
                return Ok(entry.lengths.clone());
            }
        }
        let mut lengths = HashSet::new();
        for (index, text) in dict.text_to_codes.keys().enumerate() {
            // Indexing only reads map keys, unlike a recipe lookup. Charge and
            // check the time budget in small batches rather than per candidate.
            if index % 64 == 0 && !self.budget(1) {
                return Err("词提分析预算已用尽，请缩短文章后重试");
            }
            let len = text.chars().take(MAX_CHARS + 1).count();
            if len > 0 && len <= MAX_CHARS {
                lengths.insert(len);
            }
        }
        let mut lengths: Vec<_> = lengths.into_iter().collect();
        lengths.sort_unstable();
        let lengths = Arc::new(lengths);
        let mut cache = WORD_LENGTHS.lock().unwrap_or_else(|p| p.into_inner());
        cache.push(CachedWordLengths {
            dict: identity,
            lengths: lengths.clone(),
        });
        Ok(lengths)
    }
    /// 编码可打性过滤（两模式共用）：反查前缀占用/超长/非编码字母表/
    /// 键位映射（自定义键位方案）/opencc 转换。整句模式同样保留这些
    /// 保守限制（整改4：用户只点名放开整句的"跳过"，未放开过滤）。
    fn code_typable(&self, code: &str) -> bool {
        !(code.is_empty()
            || (self.s.dict.z_symbol_domain && code.starts_with('z'))
            || code.contains(self.cfg.reverse.prefix)
            || code.len() > self.cfg.input.max_code_length.clamp(1, 16)
            || !code
                .chars()
                .all(|c| c.is_ascii_lowercase() && self.cfg.input.alphabet.contains(c))
            || code
                .chars()
                .enumerate()
                .any(|(i, c)| self.mapped(c, i == 0))
            || self.cfg.opencc.enabled
            || self.cfg.opencc.emoji)
    }
    #[cfg(test)]
    fn hint(&mut self, text: &str) -> Hint {
        if !self.budget(1) {
            return Hint::missing();
        }
        if let Some(h) = self.hints.get(text) {
            return h.clone();
        }
        let mut codes = Vec::new();
        for d in [&self.s.dict, &self.s.personal] {
            if let Some(cs) = d.text_to_codes.get(text) {
                codes.extend(cs.iter().cloned());
            }
        }
        if let Some(cs) = self.extra.get(text) {
            codes.extend(cs.iter().cloned());
        }
        codes.sort();
        codes.dedup();
        codes.truncate(256);
        // 【整改4 2026-11-08】整句模式词提放开（用户拍板）：hints=该词在码表中的
        // 编码 + 码表候选位次（信息展示口径，不依赖运行时候选——整句下运行时
        // 候选窗是整句预测，位次会错）。多编码取 rank 最优、同 rank 取更短码。
        // tail 沿用 sentence 语义（rank>1 不给选重键），且不因无 tail 拒绝编码。
        if self.sentence {
            let mut best = Hint::missing();
            let mut best_key = (usize::MAX, usize::MAX);
            for code in codes {
                if !self.budget(1) {
                    break;
                }
                if !self.code_typable(&code) {
                    continue;
                }
                let entries = self
                    .candidates
                    .entry(code.clone())
                    .or_insert_with(|| self.s.candidates(&code));
                let count = entries.len();
                let rank = entries
                    .iter()
                    .position(|e| e.text == text)
                    .map_or(0, |i| i + 1);
                let key = (if rank == 0 { usize::MAX } else { rank }, code.len());
                if key < best_key || (key == best_key && best.code.is_none()) {
                    best_key = key;
                    let tail = if rank > 0 {
                        self.tail(&code, rank, count)
                    } else {
                        None
                    }
                    .unwrap_or_default();
                    best = Hint {
                        code: Some(code),
                        rank,
                        tail,
                    };
                }
            }
            self.hints.insert(text.into(), best.clone());
            return best;
        }
        let mut best = Hint::missing();
        let mut best_cost = (usize::MAX, usize::MAX, usize::MAX, usize::MAX);
        for code in codes {
            if !self.budget(1) {
                break;
            }
            // Special namespaces and output conversions require their own
            // runtime state. Missing hints are preferable to a false recipe.
            if !self.code_typable(&code) {
                continue;
            }
            let entries = self
                .candidates
                .entry(code.clone())
                .or_insert_with(|| self.s.candidates(&code));
            // Dynamic/function rows can be filtered at runtime; do not infer
            // a displayed rank across them or a directed-output row.
            let Some(idx) = entries.iter().position(|e| e.text == text) else {
                continue;
            };
            let rank = idx + 1;
            // 【候选翻页可达 2026-11-12】第 11-20 名可翻页选出（两页封顶）
            if rank > 20
                || entries[..=idx].iter().any(|e| {
                    e.text.contains('{') || e.commit_override.as_ref().is_some_and(|v| v != &e.text)
                })
            {
                continue;
            }
            let count = entries.len();
            let Some(tail) = self.tail(&code, rank, count) else {
                continue;
            };
            let cost = (
                code.len() + tail.chars().count(),
                usize::from(rank > 1),
                code.len(),
                rank,
            );
            if cost < best_cost {
                best_cost = cost;
                best = Hint {
                    code: Some(code),
                    rank,
                    tail,
                };
            }
        }
        self.hints.insert(text.into(), best.clone());
        best
    }

    /// Keep every usable alternative until the display DP has compared the
    /// complete routes. Choosing one recipe here loses rank/base tie breakers.
    fn routing_hints(&mut self, text: &str) -> Vec<(Hint, bool)> {
        if let Some(hints) = self.route_hints.get(text) {
            return hints.clone();
        }
        if !self.budget(1) {
            return Vec::new();
        }
        let mut codes = Vec::new();
        for dict in [&self.s.dict, &self.s.personal] {
            if let Some(values) = dict.text_to_codes.get(text) {
                codes.extend(values.iter().cloned());
            }
        }
        if let Some(values) = self.extra.get(text) {
            codes.extend(values.iter().cloned());
        }
        codes.sort();
        codes.dedup();
        codes.truncate(256);
        let mut hints = Vec::new();
        for code in codes {
            if !self.budget(1) || !self.code_typable(&code) {
                continue;
            }
            let entries = self
                .candidates
                .entry(code.clone())
                .or_insert_with(|| self.s.candidates(&code));
            let Some(index) = entries.iter().position(|entry| entry.text == text) else {
                // 【补充语料词提 2026-11-11】补充语料词不在码表候选里
                //（引擎以整句词图/挂起路径打出）：按「唯一首选」合成提示，
                // 尾键走真实首选口径（空格上屏）；空格被映射/编码可延伸
                // 拿不到尾键时按 rank>1 同策略跳过（合成词打不完不如不提）。
                if self.supp_pairs.contains(&(code.clone(), text.to_string())) {
                    if let Some(selector) = self.selection_tail(&code, 1, 1, false, false) {
                        hints.push((
                            Hint {
                                code: Some(code.clone()),
                                rank: 1,
                                tail: selector,
                            },
                            true,
                        ));
                    }
                }
                continue;
            };
            let rank = index + 1;
            // page_size only limits the displayed rows. The engine accepts
            // direct rank keys up to ten even when fewer rows are visible;
            // ranks 11-20 are reachable through paging keys.
            if rank > 20
                || entries[..=index].iter().any(|entry| {
                    entry.text.contains('{')
                        || entry
                            .commit_override
                            .as_ref()
                            .is_some_and(|output| output != &entry.text)
                })
            {
                continue;
            }
            let count = entries.len();
            // This is the explicit selector. The pending-state DP also tries a
            // bare first candidate and decides whether it can be committed.
            let selector = self.selection_tail(&code, rank, count, false, false);
            if rank > 1 && selector.is_none() {
                continue;
            }
            hints.push((
                Hint {
                    code: Some(code),
                    rank,
                    tail: selector.unwrap_or_default(),
                },
                count == 1,
            ));
        }
        self.route_hints.insert(text.to_string(), hints.clone());
        hints
    }
    #[cfg(test)]
    fn analyze(&mut self, text: &str, started: Instant) -> Result<Vec<Word>, &'static str> {
        self.analyze_mode(text, started, false)
    }
    fn analyze_mode(
        &mut self,
        text: &str,
        started: Instant,
        single: bool,
    ) -> Result<Vec<Word>, &'static str> {
        self.analyze_mode_with_breaks(text, started, single, &[])
    }

    fn analyze_mode_with_breaks(
        &mut self,
        text: &str,
        started: Instant,
        single: bool,
        breaks: &[usize],
    ) -> Result<Vec<Word>, &'static str> {
        self.started = started;
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        if !valid_analysis_breaks(breaks, n) {
            return Err("段落字符偏移无效");
        }
        let mut lengths = vec![1];
        if !single {
            // Use real dictionary lengths, including phrases longer than six
            // characters. Sparse lengths avoid probing every intermediate size.
            let dict = self.s.dict.clone();
            let personal = self.s.personal.clone();
            lengths.extend(self.dictionary_lengths(&dict)?.iter().copied());
            lengths.extend(self.dictionary_lengths(&personal)?.iter().copied());
            lengths.extend(self.extra.keys().map(|word| word.chars().count()));
            lengths.push(2); // paired ellipsis/em dash use one shifted key
            lengths.retain(|&len| len > 0 && len <= n);
            lengths.sort_unstable();
            lengths.dedup();
        }
        let mut choices = vec![Vec::new(); n];
        for i in 0..n {
            if !self.budget(1) {
                return Err("词提分析预算已用尽，请缩短文章后重试");
            }
            let end = breaks
                .get(breaks.partition_point(|&point| point <= i))
                .copied()
                .unwrap_or(n);
            for &len in &lengths {
                if len > end - i {
                    break;
                }
                let surface: String = chars[i..i + len].iter().collect();
                for (hint, unique) in self.routing_hints(&surface) {
                    choices[i].push(rules::Choice { len, hint, unique });
                }
            }
        }
        let auto_four = self.theory_auto4.unwrap_or(true);
        let strategy = self.strategy;
        let mode = self.route_mode;
        let mut selectors: Vec<char> = "_234567890".chars().collect();
        selectors.extend([
            self.cfg.candidates.second_select,
            self.cfg.candidates.third_select,
        ]);
        selectors.extend(self.cfg.candidates.custom_select_keys.iter().copied());
        // 【候选翻页可达 2026-11-12】翻页键作用于挂起态的候选列表（纯翻页），
        // 挂起时不能充当下一词首键——并入 can_start 的拦截集合。
        selectors.extend(
            self.cfg
                .candidates
                .paging_keys
                .chars()
                .filter(|p| p.is_ascii() && !p.is_control()),
        );
        let required = self.required_spans.clone();
        let mut steps = rules::route(
            &chars,
            &choices,
            strategy,
            mode,
            auto_four,
            &selectors,
            &required,
            breaks,
            || self.budget(1),
        )?;
        if let Some(boundary) = self.dynamic_boundary.filter(|&point| point > 0) {
            if boundary > n {
                return Err("动态词提边界越界");
            }
            let base = steps;
            steps = Vec::new();
            for step in base {
                if step.start + step.len <= boundary {
                    steps.push(step);
                } else if step.start < boundary {
                    steps.push(rules::Step {
                        start: step.start,
                        len: boundary - step.start,
                        hint: Hint::missing(),
                    });
                }
            }
            let suffix_breaks: Vec<_> = breaks
                .iter()
                .copied()
                .filter(|&point| point > boundary)
                .map(|point| point - boundary)
                .collect();
            let suffix_locks: Vec<_> = required
                .iter()
                .filter(|span| span.start >= boundary)
                .map(|span| RequiredSpan {
                    start: span.start - boundary,
                    len: span.len,
                })
                .collect();
            for mut step in rules::route(
                &chars[boundary..],
                &choices[boundary..],
                strategy,
                mode,
                auto_four,
                &selectors,
                &suffix_locks,
                &suffix_breaks,
                || self.budget(1),
            )? {
                step.start += boundary;
                steps.push(step);
            }
        }
        let mut out = Vec::with_capacity(steps.len());
        for step in steps {
            let i = step.start;
            let len = step.len;
            let text: String = chars[i..i + len].iter().collect();
            let hint = step.hint;
            let level = if hint.code.is_none() {
                if chars[i].is_ascii_punctuation()
                    || chars[i].is_whitespace()
                    || "，。！？；：、‘’“”（）《》…—".contains(chars[i])
                {
                    "Punctuation"
                } else {
                    "SingleChar"
                }
            } else {
                match hint.code.as_ref().unwrap().len() {
                    // Keep the legacy level protocol for the other cases;
                    // only the verified v4.8.3 one-code word category is new.
                    1 if len >= 2 && text.chars().all(char::is_alphanumeric) => "one",
                    1 => "Jian1",
                    2 => "Jian2",
                    3 => "Jian3",
                    4 => "Ma4",
                    _ => "Normal",
                }
            };
            let char_hints = chars[i..i + len]
                .iter()
                .map(|&ch| {
                    let hint = self
                        .routing_hints(&ch.to_string())
                        .into_iter()
                        .min_by_key(|(hint, _)| {
                            (
                                hint.code.as_ref().map_or(usize::MAX, String::len),
                                hint.rank,
                            )
                        })
                        .map_or_else(Hint::missing, |(hint, _)| hint);
                    CharHint { ch, hint }
                })
                .collect();
            out.push(Word {
                text,
                hint,
                level,
                start: i,
                len,
                chars: char_hints,
            });
        }
        if self.exceeded {
            return Err("词提分析预算已用尽，请缩短文章后重试");
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hufu_dict::Dict;

    #[test]
    fn real_http_setting_export_import_persists_without_touching_practice_data() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        let dir = std::env::temp_dir().join(format!("hufu-trainer-http-{}", std::process::id()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let schema_dir = dir.join("码表/test");
        std::fs::create_dir_all(&schema_dir).unwrap();
        std::fs::write(
            schema_dir.join("test.dict.yaml"),
            "---\nname: test\nsort: by_weight\n...\n你\tjx\t800\n好\tbh\t800\n",
        )
        .unwrap();
        let mut cfg = Config::default();
        cfg.schema.current = "test".into();
        cfg.sentence.enabled = false;
        cfg.save(&dir.join("config.json")).unwrap();
        let progress = dir.join("trainer-progress.json");
        std::fs::write(&progress, b"preserved practice data").unwrap();
        let host = Arc::new(Mutex::new(Host::new(&dir).unwrap()));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            crate::http::serve_test_requests(listener, |req| crate::route(&host, req), 12);
        });
        let request = |method: &str, path: &str, value: &serde_json::Value| {
            let body = if method == "GET" {
                String::new()
            } else {
                value.to_string()
            };
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let head = format!("{method} {path} HTTP/1.1\r\nHost: {address}\r\nOrigin: http://{address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(body.as_bytes()).unwrap();
            let mut response = Vec::new();
            stream.read_to_end(&mut response).unwrap();
            let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            let header = String::from_utf8_lossy(&response[..split]);
            let status = header
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse::<u16>()
                .unwrap();
            (status, response[split + 4..].to_vec())
        };
        let json = |bytes: &[u8]| serde_json::from_slice::<serde_json::Value>(bytes).unwrap();
        let empty = serde_json::json!({});
        let (code, bytes) = request("GET", "/api/config", &empty);
        assert_eq!(code, 200);
        let mut settings = json(&bytes);
        settings["trainer"]["enabled"] = false.into();
        assert_eq!(request("POST", "/api/config", &settings).0, 200);
        assert!(
            !Config::load(&dir.join("config.json"))
                .unwrap()
                .trainer
                .enabled
        );
        let (code, status) = request("GET", "/api/trainer/status", &empty);
        assert_eq!(code, 200);
        assert_eq!(json(&status)["enabled"], false);
        assert_eq!(request("POST", "/api/trainer/open", &empty).0, 403);
        let (code, exported) = request("GET", "/api/settings_export", &empty);
        assert_eq!(code, 200);
        let mut bundle = json(&exported);
        assert_eq!(bundle["config"]["trainer"]["enabled"], false);
        bundle["config"]["trainer"]["enabled"] = true.into();
        assert_eq!(request("POST", "/api/settings_import", &bundle).0, 200);
        assert!(
            Config::load(&dir.join("config.json"))
                .unwrap()
                .trainer
                .enabled
        );
        let (_, bytes) = request("GET", "/api/config", &empty);
        let mut settings = json(&bytes);
        settings["trainer"]["enabled"] = false.into();
        assert_eq!(request("POST", "/api/config", &settings).0, 200);
        bundle["config"].as_object_mut().unwrap().remove("trainer");
        assert_eq!(request("POST", "/api/settings_import", &bundle).0, 200);
        let (_, bytes) = request("GET", "/api/config", &empty);
        assert_eq!(json(&bytes)["trainer"]["enabled"], false);
        assert!(
            !Config::load(&dir.join("config.json"))
                .unwrap()
                .trainer
                .enabled
        );
        assert_eq!(
            std::fs::read(&progress).unwrap(),
            b"preserved practice data"
        );
        let (code, html) = request("GET", "/next", &empty);
        assert_eq!(code, 200);
        assert_eq!(html, crate::NEXT_INDEX.as_bytes());
        let script = crate::NEXT_ASSETS
            .iter()
            .find(|(path, _, _)| path.ends_with(".js"))
            .unwrap()
            .0;
        let (code, script) = request("GET", script, &empty);
        assert_eq!(code, 200);
        assert!(String::from_utf8_lossy(&script).contains("启用跟打器"));
        server.join().unwrap();
    }

    // analyze 端点带进程级单飞锁（ANALYZING），cargo test 默认并行线程
    // 会让两个发 analyze 的测试互相撞 429——用共享锁把它们串行化。
    static ANALYZE_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 【候选翻页可达 2026-11-12】第 11-20 名经「翻页键 + 页内选重键」
    /// 打出（两页封顶）；第 21 名起依旧不可达。翻页键取配置里第一个
    /// 无冲突字符（默认 -/= 之一）。
    #[test]
    fn candidates_rank_eleven_to_twenty_reach_paging_tail() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let _analyze_gate = ANALYZE_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!("hufu-trainer-paging-{}", std::process::id()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let schema_dir = dir.join("码表/test");
        std::fs::create_dir_all(&schema_dir).unwrap();
        // 21 个同码字按权重降序 → 引擎候选 rank 1..21
        let chars = [
            "甲", "乙", "丙", "丁", "戊", "己", "庚", "辛", "壬", "癸", "子", "丑", "寅",
            "卯", "辰", "巳", "午", "未", "申", "酉", "戌",
        ];
        let mut dict = String::from("---\nname: test\nsort: by_weight\n...\n");
        for (i, ch) in chars.iter().enumerate() {
            dict.push_str(&format!("{ch}\tjq\t{}\n", 921 - i));
        }
        std::fs::write(schema_dir.join("test.dict.yaml"), &dict).unwrap();
        let mut cfg = Config::default();
        cfg.schema.current = "test".into();
        cfg.sentence.enabled = false;
        cfg.save(&dir.join("config.json")).unwrap();
        let host = Mutex::new(Host::new(&dir).unwrap());
        let request = |body| Request {
            method: "POST".into(),
            path: "/api/trainer/analyze".into(),
            query: Default::default(),
            headers: Default::default(),
            body: serde_json::to_vec(&body).unwrap(),
        };
        let call = |req: &Request| {
            let mut response = route(&host, req);
            for _ in 0..40 {
                if response.status != 503 {
                    return response;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
                response = route(&host, req);
            }
            response
        };
        let post = |body| {
            let response = call(&request(body));
            assert_eq!(
                response.status,
                200,
                "{}",
                String::from_utf8_lossy(&response.body)
            );
            serde_json::from_slice::<serde_json::Value>(&response.body).unwrap()
        };
        // rank 12：一页翻页 + 页内第 2 位（二选 ';'）
        let r12 = post(serde_json::json!({"text":"丑"}));
        assert_eq!(r12["words"][0]["rank"], 12);
        let tail = r12["words"][0]["tail"].as_str().unwrap();
        assert_eq!(tail.chars().count(), 2, "{tail}");
        assert!(tail.ends_with(';'), "rank12 inpage selector, got {tail}");
        assert!(
            "-=".contains(tail.chars().next().unwrap()),
            "rank12 paging prefix, got {tail}"
        );
        // rank 13：一页翻页 + 页内第 3 位
        let r13 = post(serde_json::json!({"text":"寅"}));
        assert_eq!(r13["words"][0]["rank"], 13);
        let tail13 = r13["words"][0]["tail"].as_str().unwrap();
        assert_eq!(tail13.chars().count(), 2, "{tail13}");
        assert!(
            "-=".contains(tail13.chars().next().unwrap()),
            "rank13 paging prefix, got {tail13}"
        );
        // rank 21：两页封顶之外 → 无码可提
        let r21 = post(serde_json::json!({"text":"戌"}));
        assert!(
            r21["words"][0]["code"].is_null(),
            "{}",
            serde_json::to_string_pretty(&r21["words"]).unwrap()
        );
    }

    #[test]
    fn trainer_setting_guards_open_and_single_analysis_has_separate_cache() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let _analyze_gate = ANALYZE_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let dir =
            std::env::temp_dir().join(format!("hufu-trainer-integration-{}", std::process::id()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let schema_dir = dir.join("码表/test");
        std::fs::create_dir_all(&schema_dir).unwrap();
        std::fs::write(schema_dir.join("test.dict.yaml"), "---\nname: test\nsort: by_weight\n...\n你好\tjxbh\t900\n伱\tjx\t900\n你\tjx\t800\n好\tbh\t800\n").unwrap();
        let mut cfg = Config::default();
        cfg.schema.current = "test".into();
        cfg.sentence.enabled = false;
        cfg.trainer.enabled = false;
        cfg.save(&dir.join("config.json")).unwrap();
        let progress = dir.join("trainer-progress.json");
        std::fs::write(&progress, b"preserved practice data").unwrap();
        let host = Mutex::new(Host::new(&dir).unwrap());
        let request = |method: &str, path: &str, body: serde_json::Value| Request {
            method: method.into(),
            path: path.into(),
            query: Default::default(),
            headers: Default::default(),
            body: serde_json::to_vec(&body).unwrap(),
        };
        // 词提码表缓存进程级共享：并行的其他 trainer 测试切换方案时，单次请求
        // 可能观察到设计的 503「已切换/忙」。此处按瞬态语义做有界重试再断言。
        let call = |req: &Request| {
            let mut response = route(&host, req);
            for _ in 0..40 {
                if response.status != 503 {
                    return response;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
                response = route(&host, req);
            }
            response
        };
        // Disabled must win even if no trainer executable is installed alongside the test binary.
        let response = call(&request("POST", "/api/trainer/open", serde_json::json!({})),
        );
        assert_eq!(response.status, 403);
        let status = call(&request("GET", "/api/trainer/status", serde_json::json!({})),
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&status.body).unwrap()["enabled"],
            false
        );
        {
            let mut h = host.lock().unwrap();
            let mut enabled = h.engine.config.clone();
            enabled.trainer.enabled = true;
            enabled
                .keymap
                .map
                .insert("capslock".into(), "select2".into());
            enabled.keymap.map.insert("shift".into(), "select3".into());
            h.apply_config(enabled, None).unwrap();
        }
        assert!(
            Config::load(&dir.join("config.json"))
                .unwrap()
                .trainer
                .enabled
        );
        assert_eq!(
            std::fs::read(&progress).unwrap(),
            b"preserved practice data"
        );

        let analyze = |single: bool| {
            let response = call(&request(
                    "POST",
                    "/api/trainer/analyze",
                    serde_json::json!({"text":"你好🙂", "single":single}),
                ),
            );
            assert_eq!(
                response.status,
                200,
                "{}",
                String::from_utf8_lossy(&response.body)
            );
            serde_json::from_slice::<serde_json::Value>(&response.body).unwrap()
        };
        let words = analyze(false);
        assert_eq!(words["words"][0]["text"], "你好");
        let selections = words["selection_keys"].as_array().unwrap();
        for vk in [0xBA, 0xDE, 0x14, 0xA0, 0xA1] {
            assert!(
                selections.contains(&serde_json::json!(vk)),
                "missing selection VK {vk}"
            );
        }
        let single = analyze(true);
        assert_eq!(single["words"].as_array().unwrap().len(), 3);
        assert_eq!(single["words"][0]["text"], "你");
        assert_eq!(single["words"][0]["code"], "jx");
        assert_eq!(single["words"][0]["rank"], 2);
        assert!(!single["words"][0]["tail"].as_str().unwrap().is_empty());
        assert_eq!(single["words"][2]["start"], 2);
        assert_eq!(single["words"][2]["len"], 1);
        assert!(single["words"][2]["code"].is_null());
        assert_eq!(analyze(false)["words"][0]["text"], "你好");
        let invalid = route(
            &host,
            &request(
                "POST",
                "/api/trainer/analyze",
                serde_json::json!({"text":"你好", "single":"true"}),
            ),
        );
        assert_eq!(invalid.status, 400);

        // The same text with different practice-round boundaries must never
        // reuse a segmentation that joins characters across those boundaries.
        for (breaks, split) in [
            (vec![], false),
            (vec![1], true),
            (vec![2], false),
            (vec![1, 2], true),
            (vec![], false),
        ] {
            let response = call(&request(
                    "POST",
                    "/api/trainer/analyze",
                    serde_json::json!({"text":"你好🙂", "breaks":breaks}),
                ),
            );
            assert_eq!(response.status, 200, "breaks={breaks:?}");
            let value: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
            assert_eq!(value["breaks"], serde_json::json!(breaks));
            assert_eq!(value["words"][0]["text"], if split { "你" } else { "你好" });
            let last = value["words"].as_array().unwrap().last().unwrap();
            assert_eq!(last["text"], "🙂");
            assert_eq!(last["start"], 2, "offsets count Unicode scalars");
        }
        let response = call(&request(
                "POST",
                "/api/trainer/analyze",
                serde_json::json!({"text":"你好🙂", "breaks":[1], "theory_auto4":true}),
            ),
        );
        assert_eq!(response.status, 200);
        let theory: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(theory["words"][0]["text"], "你");
        assert_eq!(theory["words"][1]["text"], "好");
        for breaks in [
            serde_json::json!(null),
            serde_json::json!("1"),
            serde_json::json!(1),
            serde_json::json!([0]),
            serde_json::json!([3]),
            serde_json::json!([4]),
            serde_json::json!([1, 1]),
            serde_json::json!([2, 1]),
            serde_json::json!([-1]),
            serde_json::json!([1.5]),
            serde_json::json!(["1"]),
            serde_json::json!([true]),
        ] {
            let response = call(&request(
                    "POST",
                    "/api/trainer/analyze",
                    serde_json::json!({"text":"你好🙂", "breaks":breaks}),
                ),
            );
            assert_eq!(response.status, 400, "invalid breaks={breaks}");
        }

        // New rule dimensions must be validated and echoed, including on the
        // ordinary response cache. Retraction to boundary zero restores base.
        for strategy in ["shortest", "phrases", "shortest"] {
            for boundary in [None, Some(1), Some(0), Some(2)] {
                let response = call(&request(
                        "POST",
                        "/api/trainer/analyze",
                        serde_json::json!({"text":"你好🙂","strategy":strategy,"dynamic_boundary":boundary}),
                    ),
                );
                assert_eq!(response.status, 200);
                let value: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
                assert_eq!(value["route_rules"], "quiettype-v1");
                assert_eq!(value["route_mode"], "display");
                assert_eq!(value["strategy"], strategy);
                assert_eq!(value["dynamic_boundary"], serde_json::json!(boundary));
                assert_eq!(
                    value["words"][0]["text"],
                    if boundary == Some(1) { "你" } else { "你好" }
                );
                assert_eq!(value["words"][0]["code"].is_null(), boundary == Some(1));
            }
        }
        let response = call(&request(
                "POST",
                "/api/trainer/analyze",
                serde_json::json!({
                    "text":"你好🙂","strategy":"phrases","route_mode":"theory","required_spans":[{"start":0,"len":1}]
                }),
            ),
        );
        assert_eq!(response.status, 200);
        let value: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(value["route_mode"], "theory");
        assert_eq!(value["words"][0]["text"], "你");
        assert_eq!(
            value["required_spans"],
            serde_json::json!([{"start":0,"len":1}])
        );
        for invalid in [
            serde_json::json!({"text":"你好","strategy":"old"}),
            serde_json::json!({"text":"你好","route_mode":"old"}),
            serde_json::json!({"text":"你好","dynamic_boundary":3}),
            serde_json::json!({"text":"你好","dynamic_boundary":1,"route_mode":"theory"}),
            serde_json::json!({"text":"你好","single":true,"required_spans":[{"start":0,"len":2}]}),
        ] {
            assert_eq!(
                route(&host, &request("POST", "/api/trainer/analyze", invalid)).status,
                400
            );
        }

        // Limits apply to the canonical persisted source, rather than transient
        // session learning. Settings must stay usable when a source is too large.
        for oversized_bytes in [false, true] {
            {
                let mut h = host.lock().unwrap();
                h.engine.config.trainer.enabled = !oversized_bytes;
            }
            let adjustments = if oversized_bytes {
                format!("{{删除}}a\t{}\n", "甲".repeat(10_500_000))
            } else {
                // 2026-11-13：快照门限 2,000 → 100,000（UserAdjust 加按码索引）。
                (0..100_001).map(|i| format!("{{删除}}a{i}\t甲\n")).collect()
            };
            std::fs::write(schema_dir.join("用户调整.txt"), adjustments).unwrap();
            let key = source_key(&host.lock().unwrap());
            sources::refresh_now(&key).unwrap();
            let response = call(&request("GET", "/api/trainer/status", serde_json::json!({})),
            );
            assert_eq!(response.status, 200);
            let status: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
            assert_eq!(status["enabled"], !oversized_bytes);
            assert!(status["installed"].is_boolean());
            assert!(status["hint_unavailable_reason"].is_string());
            assert!(
                status["selection_keys"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!(0x14)),
                "selection configuration remains available when dictionary hints exceed limits"
            );
            let response = call(&request(
                    "POST",
                    "/api/trainer/analyze",
                    serde_json::json!({"text":"你好"}),
                ),
            );
            assert_eq!(response.status, 503);
        }
        assert_eq!(
            route(
                &host,
                &request("POST", "/api/trainer/open", serde_json::json!({}))
            )
            .status,
            403
        );
    }

    #[test]
    fn persisted_edits_refresh_display_and_theory_without_reloading_the_composing_engine() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let _analyze_gate = ANALYZE_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let dir =
            std::env::temp_dir().join(format!("hufu-trainer-live-source-{}", std::process::id()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let schema_dir = dir.join("码表/test");
        std::fs::create_dir_all(&schema_dir).unwrap();
        let table_path = schema_dir.join("test.dict.yaml");
        let original = "---\nname: test\nsort: by_weight\n...\n你好\tab\t900\n您好\tab\t800\n你\tjx\t800\n好\tbh\t800\n您\tni\t800\n新\tx\t800\n词\tc\t800\n";
        std::fs::write(&table_path, original).unwrap();
        let mut cfg = Config::default();
        cfg.schema.current = "test".into();
        cfg.sentence.enabled = false;
        cfg.save(&dir.join("config.json")).unwrap();
        let host = Mutex::new(Host::new(&dir).unwrap());
        let (key, engine_dict) = {
            let mut h = host.lock().unwrap();
            h.session.raw = "unfinished".into();
            h.session.committed_text = "保留组字".into();
            (source_key(&h), h.engine.schema.dict.clone())
        };
        let request = |method: &str, path: &str, value: serde_json::Value| {
            route(
                &host,
                &Request {
                    method: method.into(),
                    path: path.into(),
                    query: Default::default(),
                    headers: Default::default(),
                    body: serde_json::to_vec(&value).unwrap(),
                },
            )
        };
        let status = || {
            let response = request("GET", "/api/trainer/status", serde_json::json!({}));
            assert_eq!(response.status, 200);
            serde_json::from_slice::<serde_json::Value>(&response.body).unwrap()
        };
        let analyze = |text: &str, mode: &str, explicit_auto4: bool| {
            let mut value = serde_json::json!({"text":text,"route_mode":mode,"include_full":true});
            if explicit_auto4 {
                value["theory_auto4"] = true.into();
            }
            let response = request("POST", "/api/trainer/analyze", value);
            assert_eq!(
                response.status,
                200,
                "text={text}, mode={mode}, explicit_auto4={explicit_auto4}: {}",
                String::from_utf8_lossy(&response.body)
            );
            serde_json::from_slice::<serde_json::Value>(&response.body).unwrap()
        };
        let assert_routes = |text: &str, expected: &[(&str, &str, u64)]| {
            let state = status();
            assert!(state["hint_unavailable_reason"].is_null(), "{state}");
            let ver = state["schema_ver"].as_str().unwrap();
            assert!(!ver.is_empty());
            // Exercise the ordinary response cache as well as the two explicit
            // models used by the trainer display and F10.
            for (mode, explicit) in [("display", false), ("display", true), ("theory", true)] {
                let value = analyze(text, mode, explicit);
                assert_eq!(value["schema_ver"], ver);
                let actual: Vec<_> = value["words"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|word| {
                        (
                            word["text"].as_str().unwrap(),
                            word["code"].as_str().unwrap(),
                            word["rank"].as_u64().unwrap(),
                        )
                    })
                    .collect();
                assert_eq!(actual, expected, "mode={mode}, explicit_auto4={explicit}");
            }
            ver.to_owned()
        };
        let first = assert_routes("你好", &[("你好", "ab", 1)]);
        assert_eq!(
            status()["schema_ver"],
            first,
            "unchanged sources stay stable"
        );
        {
            let mut h = host.lock().unwrap();
            h.engine.schema.user_dict.add_word("zz", "仅会话学习");
            h.engine.schema.adjust.pin("ab", "您好");
        }
        assert_eq!(
            status()["schema_ver"],
            first,
            "unpersisted session state is excluded"
        );

        let adjustment_path = schema_dir.join("用户调整.txt");
        std::fs::write(&adjustment_path, "{删除}ab\t你好\n").unwrap();
        sources::refresh_now(&key).unwrap();
        let deleted = assert_routes("你好", &[("你", "jx", 1), ("好", "bh", 1)]);
        assert_ne!(deleted, first);
        std::fs::write(
            &adjustment_path,
            "{删除}ab\t你好\n{添加}cd\t你好\n{添加}xy\t新词\n",
        )
        .unwrap();
        sources::refresh_now(&key).unwrap();
        let added = assert_routes("你好新词", &[("你好", "cd", 1), ("新词", "xy", 1)]);
        assert_ne!(added, deleted);

        std::fs::write(&adjustment_path, "{置顶}ab\t您好\n").unwrap();
        sources::refresh_now(&key).unwrap();
        let pinned = assert_routes("你好", &[("你好", "ab", 2)]);
        assert_routes("您好", &[("您好", "ab", 1)]);
        assert_ne!(pinned, added);
        std::fs::remove_file(&adjustment_path).unwrap();
        sources::refresh_now(&key).unwrap();
        let restored = assert_routes("你好", &[("你好", "ab", 1)]);
        assert_ne!(
            restored, first,
            "A -> B -> A must get a fresh source identity"
        );

        // Same-length main-table edits must also refresh while the live engine
        // retains its old dictionary to protect an unfinished composition.
        let edited = original.replace("你好\tab", "你好\ttt");
        assert_eq!(edited.len(), original.len());
        std::fs::write(&table_path, &edited).unwrap();
        sources::refresh_now(&key).unwrap();
        let rewritten = assert_routes("你好", &[("你好", "tt", 1)]);
        assert_ne!(rewritten, restored);

        std::fs::write(&table_path, "# temporarily no usable dictionary\n").unwrap();
        assert!(sources::refresh_now(&key).is_err());
        let unavailable = status();
        assert_eq!(unavailable["schema_ver"], "");
        assert!(unavailable["hint_unavailable_reason"].is_string());
        assert_eq!(
            request(
                "POST",
                "/api/trainer/analyze",
                serde_json::json!({"text":"你好"})
            )
            .status,
            503
        );
        std::fs::write(&table_path, original).unwrap();
        sources::refresh_now(&key).unwrap();
        let recovered = assert_routes("你好", &[("你好", "ab", 1)]);
        assert_ne!(recovered, rewritten);
        let h = host.lock().unwrap();
        assert!(Arc::ptr_eq(&h.engine.schema.dict, &engine_dict));
        assert_eq!(h.session.raw, "unfinished");
        assert_eq!(h.session.committed_text, "保留组字");
    }

    #[test]
    fn supplement_words_receive_hints() {
        // 【补充语料词提 2026-11-11】语料词由引擎注入整句词图并以挂起
        // 机制可打出，词提必须同样可见——此前 sources 剥离 supplement
        // 导致正文里这些词永远无提示（用户实测漏词：上屏/傲慢 等）。
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let _analyze_gate = ANALYZE_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let dir =
            std::env::temp_dir().join(format!("hufu-trainer-supp-{}", std::process::id()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let schema_dir = dir.join("码表/test");
        std::fs::create_dir_all(&schema_dir).unwrap();
        let table = "---\nname: test\nsort: by_weight\n...\n你\tjx\t800\n好\tbh\t800\n新\tx\t800\n词\tc\t800\n";
        std::fs::write(schema_dir.join("test.dict.yaml"), table).unwrap();
        std::fs::write(schema_dir.join("补充语料.txt"), "新词\t1000\n").unwrap();
        let mut cfg = Config::default();
        cfg.schema.current = "test".into();
        cfg.sentence.enabled = false;
        cfg.save(&dir.join("config.json")).unwrap();
        let host = Mutex::new(Host::new(&dir).unwrap());
        let response = route(
            &host,
            &Request {
                method: "POST".into(),
                path: "/api/trainer/analyze".into(),
                query: Default::default(),
                headers: Default::default(),
                body: serde_json::to_vec(&serde_json::json!({
                    "text": "新词",
                    "route_mode": "display",
                    "include_full": true
                }))
                .unwrap(),
            },
        );
        assert_eq!(
            response.status,
            200,
            "{}",
            String::from_utf8_lossy(&response.body)
        );
        let value: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        let words: Vec<(String, String)> = value["words"]
            .as_array()
            .unwrap()
            .iter()
            .map(|word| {
                (
                    word["text"].as_str().unwrap().to_string(),
                    word["code"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert!(
            words.iter().any(|(t, c)| t == "新词" && c == "xc"),
            "supplement word must carry its combo-code hint: {words:?}"
        );
    }

    #[test]
    fn selection_keys_follow_candidates_and_active_mapping_with_shift_overrides() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let mut cfg = Config::default();
        let defaults = selection_keys(&cfg);
        assert!(defaults.contains(&0xBA) && defaults.contains(&0xDE));
        assert!(!defaults.contains(&0x14) && !defaults.contains(&0xA0));
        cfg.candidates.custom_select_keys = vec![',', 'q'];
        cfg.keymap.map.insert(";".into(), "text:甲".into());
        cfg.keymap.map.insert("capslock".into(), "select4".into());
        cfg.keymap.map.insert("shift".into(), "select2".into());
        cfg.keymap.map.insert("shiftleft".into(), "switch".into());
        cfg.keymap.map.insert("numdot".into(), "select10".into());
        cfg.keymap.map.insert("f12".into(), "select3".into());
        cfg.keymap.map.insert("f01".into(), "select2".into());
        cfg.keymap.map.insert("escape".into(), "select2".into()); // fixed engine behavior
        cfg.keymap.map.insert("ctrl".into(), "select2".into()); // no engine keymap identity
        cfg.keymap.map.insert("/".into(), "select11".into()); // invalid action
        cfg.keymap.map_idle.insert("tab".into(), "select2".into());
        let active = selection_keys(&cfg);
        for vk in [0xBC, 0x51, 0x14, 0xA1, 0x6E, 0x7B] {
            assert!(active.contains(&vk), "missing {vk}");
        }
        for vk in [0xBA, 0xA0, 0x70, 0x1B, 0x11, 0xBF, 0x09] {
            assert!(!active.contains(&vk), "inactive key {vk}");
        }
        assert!(
            active.windows(2).all(|pair| pair[0] < pair[1]),
            "stable deduplicated API order"
        );
    }

    fn schema(entries: Vec<(&str, &str)>) -> Schema {
        let mut d = Dict::new("test");
        d.load_entries(
            entries
                .into_iter()
                .enumerate()
                .map(|(i, (c, t))| hufu_dict::DictEntry::new(c, t, i as u32))
                .collect(),
        );
        Schema {
            name: "test".into(),
            dir: Default::default(),
            dict: Arc::new(d),
            symbols: Default::default(),
            supplement: Default::default(),
            pinyin: None,
            unicode_block: None,
            split: None,
            reverse: None,
            reverse_path: None,
            reverse_rule: None,
            super_dict: Arc::new(Dict::new("")),
            personal: Arc::new(Dict::new("")),
            adjust: Default::default(),
            user_dict: Default::default(),
            encoder_rules: vec![],
        }
    }
    #[test]
    fn real_candidates_only_and_live_adjustments() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let mut s = schema(vec![("ab", "甲"), ("ab", "乙"), ("abcd", "甲乙")]);
        let c = Config::default();
        assert_eq!(Analyzer::new(&s, &c, false).hint("乙").rank, 2);
        s.adjust.remove("ab", "甲");
        assert_eq!(Analyzer::new(&s, &c, false).hint("乙").rank, 1);
        assert!(Analyzer::new(&s, &c, false).hint("甲").code.is_none());
        assert!(Analyzer::new(&s, &c, false).hint("甲甲乙").code.is_none());
    }

    #[test]
    fn direct_rank_recipes_replay_beyond_the_visible_candidate_page() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "hufu-trainer-small-page-replay-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let words = [
            "首项词", "二项词", "三项词", "四项词", "五项词", "六项词",
            "七项词", "八项词", "九项词", "十项词", "十一项词",
        ];
        let mut dict = "---\nname: test\nsort: by_weight\n...\n".to_string();
        for (index, word) in words.iter().enumerate() {
            dict.push_str(&format!("{word}\tab\t{}\n", 2000 - index));
        }
        std::fs::write(dir.join("test.dict.yaml"), dict).unwrap();
        for page_size in [1, 2, 5] {
            let mut cfg = Config::default();
            cfg.candidates.page_size = page_size;
            cfg.user.auto_frequency = false;
            cfg.user.log_adjust = false;
            let mut engine = hufu_engine::Engine::with_schema_dir(&dir, cfg).unwrap();
            assert!(!engine.sentence_active());
            for (index, word) in words[..10].iter().enumerate() {
                let hint = Analyzer::new(&engine.schema, &engine.config, false).hint(word);
                assert_eq!(hint.code.as_deref(), Some("ab"), "{page_size}/{word}");
                assert_eq!(hint.rank, index + 1);
                let expected_tail = match index + 1 {
                    1 => "_".to_string(),
                    2 => ";".to_string(),
                    3 => "'".to_string(),
                    10 => "0".to_string(),
                    rank => rank.to_string(),
                };
                assert_eq!(hint.tail, expected_tail);
                let mut session = hufu_engine::Session::new(true);
                let mut committed = String::new();
                for key in format!("ab{}", hint.tail.replace('_', " ")).chars() {
                    if let Some(text) = engine
                        .process_key(&mut session, hufu_types::KeyInput::char_key(key))
                        .commit
                    {
                        committed.push_str(&text);
                    }
                }
                assert_eq!(committed, *word, "{page_size}/rank {}", index + 1);
                for sentence in [false, true] {
                    for mode in [RouteMode::Display, RouteMode::Theory] {
                        let mut analyzer = Analyzer::new(&engine.schema, &engine.config, sentence);
                        analyzer.route_mode = mode;
                        let route = analyzer.analyze(word, Instant::now()).unwrap();
                        assert_eq!(route.len(), 1, "{page_size}/{word}/{mode:?}");
                        assert_eq!(route[0].text, *word);
                        assert_eq!(route[0].hint.code.as_deref(), Some("ab"));
                        assert_eq!(route[0].hint.rank, index + 1);
                    }
                }
            }
            // 【候选翻页可达 2026-11-12】rank 11-20 可「编码+翻页键+页内
            // 选重键」打出，且经引擎按键回放确实上屏。
            let eleventh = Analyzer::new(&engine.schema, &engine.config, false)
                .hint(words[10]);
            assert_eq!(eleventh.code.as_deref(), Some("ab"), "{page_size}");
            assert_eq!(eleventh.rank, 11, "{page_size}");
            assert_eq!(eleventh.tail.chars().count(), 2, "{page_size}");
            assert!(
                "-=".contains(eleventh.tail.chars().next().unwrap()),
                "{page_size}: {}",
                eleventh.tail
            );
            assert_eq!(eleventh.tail.chars().nth(1), Some('1'), "{page_size}");
            assert_eq!(
                Analyzer::new(&engine.schema, &engine.config, false)
                    .routing_hints(words[10])
                    .len(),
                1,
                "{page_size}: rank eleven is now reachable through paging"
            );
            let mut session = hufu_engine::Session::new(true);
            let mut committed = String::new();
            for key in format!("ab{}", eleventh.tail).chars() {
                if let Some(text) = engine
                    .process_key(&mut session, hufu_types::KeyInput::char_key(key))
                    .commit
                {
                    committed.push_str(&text);
                }
            }
            assert_eq!(committed, words[10], "{page_size}/rank 11 paging replay");
        }
    }

    #[test]
    fn one_level_marks_only_multi_character_alphanumeric_one_code_words() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let s = schema(vec![
            ("a", "一简词"),
            ("b", "甲"),
            ("c", "，，"),
            ("d", "甲，"),
            ("e", "𠮷甲"),
            ("ab", "二码词"),
        ]);
        let cfg = Config::default();
        for text in ["一简词", "𠮷甲"] {
            let words = Analyzer::new(&s, &cfg, false)
                .analyze(text, Instant::now())
                .unwrap();
            assert_eq!(words.len(), 1);
            assert_eq!(words[0].level, "one");
        }
        for text in ["甲", "，，", "甲，", "缺词"] {
            let words = Analyzer::new(&s, &cfg, false)
                .analyze(text, Instant::now())
                .unwrap();
            assert!(words.iter().all(|word| word.level != "one"), "{text}");
        }
        let words = Analyzer::new(&s, &cfg, false)
            .analyze("二码词", Instant::now())
            .unwrap();
        assert_eq!(
            words[0].level, "Jian2",
            "existing protocol stays compatible"
        );
        let words = Analyzer::new(&s, &cfg, false)
            .analyze("甲", Instant::now())
            .unwrap();
        assert_eq!(
            words[0].level, "Jian1",
            "single characters are never one words"
        );
    }
    #[test]
    fn unicode_positions_and_minimum_keys() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let s = schema(vec![("a", "甲"), ("b", "乙"), ("abcd", "甲乙")]);
        let c = Config::default();
        let mut a = Analyzer::new(&s, &c, false);
        a.route_mode = RouteMode::Theory;
        a.theory_auto4 = Some(false);
        let w = a.analyze("甲乙😀甲", Instant::now()).unwrap();
        assert_eq!(
            w.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            vec!["甲", "乙", "😀", "甲"]
        );
        assert_eq!(w[3].start, 3);
    }
    #[test]
    fn round_breaks_stop_words_without_changing_unicode_scalar_offsets() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let s = schema(vec![("a", "𠮷甲"), ("b", "𠮷"), ("c", "甲"), ("d", "乙")]);
        let c = Config::default();
        let words = Analyzer::new(&s, &c, false)
            .analyze("𠮷甲乙", Instant::now())
            .unwrap();
        assert_eq!(words[0].text, "𠮷甲");
        let words = Analyzer::new(&s, &c, false)
            .analyze_mode_with_breaks("𠮷甲乙", Instant::now(), false, &[1])
            .unwrap();
        assert_eq!(
            words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            ["𠮷", "甲", "乙"]
        );
        assert_eq!(
            words
                .iter()
                .map(|word| (word.start, word.len))
                .collect::<Vec<_>>(),
            [(0, 1), (1, 1), (2, 1)]
        );
    }

    #[test]
    fn dynamic_routes_rebuild_from_immutable_base_and_neutralize_partial_words() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let s = schema(vec![
            ("abcd", "𠮷甲乙"),
            ("bc", "甲乙"),
            ("a", "𠮷"),
            ("b", "甲"),
            ("c", "乙"),
        ]);
        let cfg = Config::default();
        let run = |boundary| {
            let mut analyzer = Analyzer::new(&s, &cfg, false);
            analyzer.dynamic_boundary = boundary;
            analyzer.analyze("𠮷甲乙", Instant::now()).unwrap()
        };
        let base = run(None);
        assert_eq!(
            base.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            ["𠮷甲乙"]
        );
        let split = run(Some(1));
        assert_eq!(
            split
                .iter()
                .map(|w| (w.start, w.len, w.text.as_str()))
                .collect::<Vec<_>>(),
            [(0, 1, "𠮷"), (1, 2, "甲乙")]
        );
        assert!(split[0].hint.code.is_none());
        assert_eq!(split[1].hint.code.as_deref(), Some("bc"));
        let partial = run(Some(2));
        assert_eq!(partial[0].text, "𠮷甲");
        assert!(
            partial[0].hint.code.is_none(),
            "a partially typed base word must lose its annotation"
        );
        assert_eq!(partial[1].text, "乙");
        for boundary in [Some(0), Some(3), None] {
            let restored = run(boundary);
            assert_eq!(restored.len(), 1);
            assert_eq!(restored[0].hint.code, base[0].hint.code);
        }
    }

    #[test]
    fn new_route_options_validate_unicode_spans_and_boundaries() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let options =
            serde_json::json!({"strategy":"phrases","required_spans":[{"start":1,"len":2}]});
        let (strategy, spans) = parse_route_options(&options, 4, &[]).unwrap();
        assert_eq!(strategy, HintStrategy::Phrases);
        assert_eq!(spans, [RequiredSpan { start: 1, len: 2 }]);
        for invalid in [
            serde_json::json!({"strategy":"legacy"}),
            serde_json::json!({"required_spans":[{"start":1,"len":0}]}),
            serde_json::json!({"required_spans":[{"start":1,"len":3},{"start":2,"len":1}]}),
            serde_json::json!({"required_spans":[{"start":usize::MAX,"len":1}]}),
        ] {
            assert!(parse_route_options(&invalid, 4, &[]).is_err());
        }
        assert!(parse_route_options(&options, 4, &[2]).is_err());
        assert_eq!(
            parse_dynamic_boundary(Some(&serde_json::json!(3)), 3),
            Ok(Some(3))
        );
        for invalid in [
            serde_json::json!(-1),
            serde_json::json!(1.2),
            serde_json::json!(4),
        ] {
            assert!(parse_dynamic_boundary(Some(&invalid), 3).is_err());
        }
    }
    #[test]
    fn route_uses_global_keys_then_selection_and_token_ties() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let analyze_theory = |s: &Schema, c: &Config, text: &str| {
            let mut analyzer = Analyzer::new(s, c, false);
            analyzer.route_mode = RouteMode::Theory;
            analyzer.analyze(text, Instant::now()).unwrap()
        };
        let c = Config::default();
        let s = schema(vec![
            ("a", "甲"),
            ("b", "乙丙"),
            ("c", "乙"),
            ("defg", "丙"),
            ("abcd", "甲乙"),
        ]);
        let words = analyze_theory(&s, &c, "甲乙丙");
        assert_eq!(
            words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            ["甲", "乙丙"],
            "a locally longer phrase must not hide the cheaper suffix route"
        );

        let s = schema(vec![
            ("a", "甲"),
            ("b", "乙"),
            ("abc", "别"),
            ("abc", "甲乙"),
        ]);
        let words = analyze_theory(&s, &c, "甲乙");
        assert_eq!(
            words.len(),
            2,
            "equal keys prefer no selection before fewer tokens"
        );
        assert!(words.iter().all(|w| w.hint.rank == 1));

        let s = schema(vec![("a", "甲"), ("b", "乙"), ("abc", "甲乙")]);
        let words = analyze_theory(&s, &c, "甲乙");
        assert_eq!(
            words.len(),
            1,
            "equal keys and selection prefer fewer tokens"
        );
        assert_eq!(words[0].text, "甲乙");
    }

    #[test]
    fn recipe_ties_prefer_first_candidate_with_real_auto_commit_tail() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let s = schema(vec![("abc", "别"), ("abc", "甲"), ("abcd", "甲")]);
        let mut c = Config::default();
        c.input.auto_select_unique = true;
        let hint = Analyzer::new(&s, &c, false).hint("甲");
        assert_eq!(hint.code.as_deref(), Some("abcd"));
        assert_eq!(hint.rank, 1);
        assert!(hint.tail.is_empty());
    }

    #[test]
    fn actual_word_lengths_include_long_unicode_and_added_phrases() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let phrase = "𠮷甲乙丙丁戊己庚";
        let mut s = schema(vec![("abcd", phrase), ("a", "乙")]);
        let c = Config::default();
        let words = Analyzer::new(&s, &c, false)
            .analyze(&format!("😀{phrase}，乙"), Instant::now())
            .unwrap();
        assert_eq!(words[1].text, phrase);
        assert_eq!((words[1].start, words[1].len), (1, 8));
        assert_eq!((words[2].start, words[3].start), (9, 10));

        let added = "天下没有不散的筵席";
        s.user_dict.add_word("abc", added);
        let words = Analyzer::new(&s, &c, false)
            .analyze(added, Instant::now())
            .unwrap();
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, added);

        let single = Analyzer::new(&s, &c, false)
            .analyze_mode(phrase, Instant::now(), true)
            .unwrap();
        assert_eq!(single.len(), phrase.chars().count());
        assert!(single.iter().all(|w| w.len == 1));
    }

    #[test]
    fn punctuation_and_unknown_scalars_preserve_route_and_physical_costs() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let s = schema(vec![("abc", "甲乙"), ("a", "𠮷")]);
        let c = Config::default();
        let input = "甲乙，……😀𠮷";
        let words = Analyzer::new(&s, &c, false)
            .analyze(input, Instant::now())
            .unwrap();
        assert_eq!(
            words.iter().map(|w| w.text.as_str()).collect::<String>(),
            input
        );
        assert_eq!(
            words.iter().map(|w| (w.start, w.len)).collect::<Vec<_>>(),
            [(0, 2), (2, 1), (3, 2), (5, 1), (6, 1)]
        );
        assert_eq!(words[1].level, "Punctuation");
        assert_eq!(words[2].level, "Punctuation");
        assert!(words[1].hint.code.is_none() && words[2].hint.code.is_none());
        let cost = words.iter().fold(RouteCost::default(), |cost, word| {
            cost.then(RouteCost::token(&word.text, word.len, &word.hint))
        });
        assert_eq!(
            cost.missing, 1,
            "only the emoji lacks a dictionary/literal route"
        );
        assert!(
            words[0].hint.tail.is_empty(),
            "comma commits the pending first candidate"
        );
        assert_eq!(cost.keys, 8, "abc, comma, Shift+6, a+space");
    }

    #[test]
    fn selection_conflicts_and_auto_commit() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let s = schema(vec![
            ("abcd", "甲"),
            ("ab", "乙"),
            ("ab", "丙"),
            ("abq", "丁"),
        ]);
        let mut c = Config::default();
        c.input.auto_select_unique = true;
        assert_eq!(Analyzer::new(&s, &c, false).hint("甲").tail, "");
        // 【整改4】整句模式词提放开：给码表编码（自动顶屏不再适用于整句，
        // tail 语义见 tail() 的 sentence 分支）。
        let sh = Analyzer::new(&s, &c, true).hint("甲");
        assert_eq!(sh.code.as_deref(), Some("abcd"));
        assert_eq!(sh.rank, 1);
        c.candidates.second_select = 'q';
        c.candidates.custom_select_keys = vec!['2'];
        // q extends the code and digit 2 is custom rank 1: no false rank 2 recipe.
        assert!(Analyzer::new(&s, &c, false).hint("丙").code.is_none());
        c.keymap.map.insert("a".into(), "clear".into());
        assert!(Analyzer::new(&s, &c, false).hint("甲").code.is_none());
    }
    #[test]
    fn theory_auto4_overrides_runtime_auto_but_preserves_space_legality() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let s = schema(vec![("ab", "甲"), ("abcd", "乙")]);
        let mut cfg = Config::default();
        cfg.input.auto_select_unique = true;
        cfg.input.max_code_length = 2;
        assert_eq!(Analyzer::new(&s, &cfg, false).hint("甲").tail, "");
        for auto4 in [false, true] {
            let mut theory = Analyzer::new(&s, &cfg, false);
            theory.theory_auto4 = Some(auto4);
            assert_eq!(
                theory.hint("甲").tail,
                "_",
                "two-code recipes must not inherit runtime unique auto-commit"
            );
        }
        cfg.input.max_code_length = 4;
        cfg.keymap.map.insert("space".into(), "clear".into());
        let mut manual = Analyzer::new(&s, &cfg, false);
        manual.theory_auto4 = Some(false);
        assert!(
            manual.hint("乙").code.is_none(),
            "mapped space cannot select"
        );
        let mut auto4 = Analyzer::new(&s, &cfg, false);
        auto4.theory_auto4 = Some(true);
        assert_eq!(auto4.hint("乙").tail, "");
        assert!(
            auto4.hint("甲").code.is_none(),
            "short codes still need legal space"
        );
    }

    #[test]
    fn theory_requests_resegment_without_sentence_tails_or_cache_contamination() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let _analyze_gate = ANALYZE_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        struct EmptyDecoder;
        impl hufu_engine::SentenceDecoder for EmptyDecoder {
            fn decode_rich(&self, _: &str) -> Arc<hufu_engine::SentenceDecode> {
                Arc::new(hufu_engine::SentenceDecode {
                    hits: Vec::new(),
                    truncated: false,
                    early_hits: Vec::new(),
                    early_truncated: false,
                })
            }
        }
        let dir =
            std::env::temp_dir().join(format!("hufu-trainer-theory-mode-{}", std::process::id()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let schema_dir = dir.join("码表/test");
        std::fs::create_dir_all(&schema_dir).unwrap();
        std::fs::write(
            schema_dir.join("test.dict.yaml"),
            "---\nname: test\nsort: by_weight\n...\n甲\ta\t900\n乙\tb\t900\n甲乙\tabcd\t900\n丙\tab\t800\n丁\tab\t700\n",
        )
        .unwrap();
        let mut cfg = Config::default();
        cfg.schema.current = "test".into();
        cfg.sentence.enabled = true;
        cfg.sentence.auto_enable = false;
        cfg.input.auto_select_unique = true;
        cfg.save(&dir.join("config.json")).unwrap();
        let mut host = Host::new(&dir).unwrap();
        host.engine
            .set_sentence_decoder(Some(Arc::new(EmptyDecoder)));
        assert!(host.engine.sentence_active());
        let host = Mutex::new(host);
        let request = |body| Request {
            method: "POST".into(),
            path: "/api/trainer/analyze".into(),
            query: Default::default(),
            headers: Default::default(),
            body: serde_json::to_vec(&body).unwrap(),
        };
        let post = |body| {
            let response = route(&host, &request(body));
            assert_eq!(
                response.status,
                200,
                "{}",
                String::from_utf8_lossy(&response.body)
            );
            serde_json::from_slice::<serde_json::Value>(&response.body).unwrap()
        };
        let ordinary = post(serde_json::json!({"text":"丁"}));
        assert_eq!(ordinary["sentence_active"], true);
        assert_eq!(ordinary["words"][0]["rank"], 2);
        assert_eq!(
            ordinary["words"][0]["tail"], ";",
            "new display rules preserve dictionary rank and selector in sentence mode"
        );
        assert!(ordinary.get("theory_auto4").is_none());
        for auto4 in [false, true] {
            let theory =
                post(serde_json::json!({"text":"丁", "theory_auto4":auto4,"route_mode":"theory"}));
            assert_eq!(theory["theory_auto4"], auto4);
            assert_eq!(theory["words"][0]["rank"], 2);
            assert_eq!(
                theory["words"][0]["tail"], ";",
                "theory keeps real rank-2 selector even when sentence mode is active"
            );
        }
        for auto4 in [false, true, false] {
            let theory = post(
                serde_json::json!({"text":"甲乙", "theory_auto4":auto4,"route_mode":"theory"}),
            );
            let words = theory["words"].as_array().unwrap();
            let texts: Vec<_> = words.iter().map(|w| w["text"].as_str().unwrap()).collect();
            if auto4 {
                assert_eq!(
                    texts,
                    vec!["甲乙"],
                    "four-code auto commit makes the phrase cheapest (tie prefers phrase)"
                );
                assert_eq!(words[0]["tail"], "");
            } else {
                assert_eq!(
                    texts,
                    vec!["甲", "乙"],
                    "manual four-code commit costs five keys, so DP must split into four-key route"
                );
                assert!(words.iter().all(|w| w["tail"] == "_"));
            }
        }
        assert_eq!(
            post(serde_json::json!({"text":"丁"})),
            ordinary,
            "theory requests must neither consume nor overwrite the ordinary hint cache"
        );
        assert_eq!(
            route(
                &host,
                &request(serde_json::json!({"text":"甲", "theory_auto4":"true"}))
            )
            .status,
            400
        );
    }
    /// 【键数平粘词优先 2026-11-12 用户改判】「怎么办？」@kol+选重（4 键，
    /// rank1）与「怎么办」@k+选重2键 +「？」↑/ 2键（4 键）平键时，粘词
    /// 胜出（rank 分量更低）；撤销 2026-11-11 的 +1 罚分。粘词仅在严格
    /// 更费键时输。
    #[test]
    fn glued_punctuation_word_wins_key_ties() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let s = schema(vec![
            ("k", "怎"),
            ("k", "怎么办"),
            ("kol", "怎么办？"),
        ]);
        let c = Config::default();
        let mut analyzer = Analyzer::new(&s, &c, false);
        analyzer.route_mode = RouteMode::Display;
        let route = analyzer.analyze("怎么办？", std::time::Instant::now()).unwrap();
        assert_eq!(route.len(), 1);
        assert_eq!(route[0].text, "怎么办？");
        assert_eq!(route[0].hint.code.as_deref(), Some("kol"));
        assert_eq!(route[0].hint.rank, 1);
    }

    #[test]
    fn paging_reaches_rank_eleven_but_not_twentyone() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let entries = (0..21).map(|i| ("a", format!("字{i}"))).collect::<Vec<_>>();
        let s = schema(entries.iter().map(|(c, t)| (*c, t.as_str())).collect());
        let c = Config::default();
        // 【候选翻页可达 2026-11-12】第 11 名有翻页配方；第 21 名两页封顶之外。
        let eleventh = Analyzer::new(&s, &c, false).hint("字10");
        assert_eq!(eleventh.code.as_deref(), Some("a"));
        assert_eq!(eleventh.rank, 11);
        assert_eq!(eleventh.tail.chars().count(), 2);
        assert!("-=".contains(eleventh.tail.chars().next().unwrap()));
        assert!(Analyzer::new(&s, &c, false).hint("字20").code.is_none());
    }
    #[test]
    fn z_domain_and_one_key_auto_commit_are_not_false_recipes() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let mut s = schema(vec![("za", "字"), ("zaaa", "词组"), ("a", "甲")]);
        Arc::get_mut(&mut s.dict).unwrap().z_symbol_domain = true;
        let mut c = Config::default();
        c.input.auto_select_unique = true;
        assert!(Analyzer::new(&s, &c, false).hint("字").code.is_none());
        for mode in [RouteMode::Display, RouteMode::Theory] {
            let mut analyzer = Analyzer::new(&s, &c, false);
            analyzer.theory_auto4 = Some(true);
            analyzer.route_mode = mode;
            let words = analyzer.analyze("字词组", Instant::now()).unwrap();
            assert!(words.iter().all(|word| word.hint.code.is_none()));
            assert!(words.iter().all(|word| word.len == 1));
        }
        c.input.max_code_length = 1;
        assert_eq!(Analyzer::new(&s, &c, false).hint("甲").tail, "_");
    }
    #[test]
    fn fractional_and_user_weights_invalidate_version() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let mut s = schema(vec![("ab", "甲")]);
        let c = Config::default();
        s.user_dict.add_word("ab", "乙");
        s.user_dict.entries[0].weight = 1.25;
        let first = version(&s, &c, false);
        s.user_dict.entries[0].weight = 1.75;
        assert_ne!(first, version(&s, &c, false));
        let first = version(&s, &c, false);
        s.user_dict.weights.insert(("ab".into(), "乙".into()), 1.1);
        assert_ne!(first, version(&s, &c, false));
    }
    #[test]
    fn version_tracks_hint_sections_only() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let s = schema(vec![("ab", "甲")]);
        let c = Config::default();
        let base = version(&s, &c, false);
        // 无关节：皮肤/外观、音效、通用习惯、trainer 开关、整句参数
        // （sentence 以独立布尔参与，不重复哈希节）——不应失效词提缓存。
        let mut unrelated = c.clone();
        unrelated.appearance.skin = "另一款皮肤".into();
        unrelated.appearance.font_size = 21.0;
        unrelated.sound.enabled = !unrelated.sound.enabled;
        unrelated.general.esc_undo = !unrelated.general.esc_undo;
        unrelated.trainer.enabled = !unrelated.trainer.enabled;
        unrelated.sentence.enabled = !unrelated.sentence.enabled;
        assert_eq!(base, version(&s, &unrelated, false));
        // 词提相关节：任一变化必须换 version（缓存失效）。
        let mut hint = c.clone();
        hint.input.max_code_length += 1;
        assert_ne!(base, version(&s, &hint, false));
        let mut hint = c.clone();
        hint.candidates.second_select = 'z';
        assert_ne!(base, version(&s, &hint, false));
        let mut hint = c.clone();
        hint.keymap.map.insert("a".into(), "clear".into());
        assert_ne!(base, version(&s, &hint, false));
        let mut hint = c.clone();
        hint.reverse.enabled = !hint.reverse.enabled;
        assert_ne!(base, version(&s, &hint, false));
        let mut hint = c.clone();
        hint.opencc.enabled = !hint.opencc.enabled;
        assert_ne!(base, version(&s, &hint, false));
        // sentence 标志独立参与（整句模式词提不可用）。
        assert_ne!(base, version(&s, &c, true));
    }
    #[test]
    fn prefix_index_and_internal_budget_are_bounded() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let mut s = schema(vec![("ab", "甲"), ("ab", "乙")]);
        s.user_dict.add_word("abqz", "词");
        let c = Config::default();
        let mut a = Analyzer::new(&s, &c, false);
        assert!(a.extends("ab", 'q'));
        assert!(!a.extends("ab", 'x'));
        a.work = 500_000;
        assert!(a.hint("甲").code.is_none());
        assert!(a.exceeded);
        let mut a = Analyzer::new(&s, &c, false);
        assert!(a
            .analyze("甲", Instant::now() - Duration::from_secs(1))
            .is_err());
    }

    #[test]
    fn advertised_recipes_replay_through_real_engine() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        // Exercise the same engine path used by TSF, including automatic commit,
        // explicit selection and code-continuation conflicts.
        let dir =
            std::env::temp_dir().join(format!("hufu-trainer-key-replay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        std::fs::write(
            dir.join("test.dict.yaml"),
            "---\nname: test\nsort: by_weight\n...\n甲\ta\t900\n乙\tab\t800\n丙\tab\t700\n丁\tab\t600\n戊\tabq\t500\n你好\tjxbh\t900\n你\tjx\t800\n好\tbh\t800\n",
        )
        .unwrap();
        for auto in [false, true] {
            for custom in [false, true] {
                let mut cfg = Config::default();
                cfg.input.auto_select_unique = auto;
                if custom {
                    cfg.candidates.second_select = 'q';
                    cfg.candidates.third_select = 'w';
                    cfg.candidates.custom_select_keys = vec!['1', '2', '3'];
                }
                let mut engine = hufu_engine::Engine::with_schema_dir(&dir, cfg).unwrap();
                for expected in ["甲", "乙", "丙", "丁", "戊", "你好", "你", "好"] {
                    let hint = Analyzer::new(&engine.schema, &engine.config, false).hint(expected);
                    let Some(code) = hint.code else { continue };
                    let recipe = format!("{code}{}", hint.tail.replace('_', " "));
                    let mut session = hufu_engine::Session::new(true);
                    let mut committed = String::new();
                    for key in recipe.chars() {
                        let result =
                            engine.process_key(&mut session, hufu_types::KeyInput::char_key(key));
                        if let Some(text) = result.commit {
                            committed.push_str(&text);
                        }
                    }
                    assert_eq!(
                        committed, expected,
                        "recipe {recipe:?}, auto={auto}, custom={custom}"
                    );
                    assert!(
                        session.raw.is_empty(),
                        "recipe left an active composition: {recipe:?}"
                    );
                }
                // Replay the new complete route too: punctuation may confirm
                // its preceding candidate, so standalone recipes are not enough.
                for expected in ["甲，你好", "丙，你好", "你好，你好"] {
                    let mut analyzer = Analyzer::new(&engine.schema, &engine.config, false);
                    analyzer.theory_auto4 = Some(auto);
                    let words = analyzer.analyze(expected, Instant::now()).unwrap();
                    let recipe: String = words
                        .iter()
                        .map(|word| match &word.hint.code {
                            Some(code) => format!("{code}{}", word.hint.tail.replace('_', " ")),
                            None if word.text == "，" => ",".into(),
                            None => panic!("unexpected missing route: {}", word.text),
                        })
                        .collect();
                    let mut session = hufu_engine::Session::new(true);
                    let mut committed = String::new();
                    for key in recipe.chars() {
                        if let Some(text) = engine
                            .process_key(&mut session, hufu_types::KeyInput::char_key(key))
                            .commit
                        {
                            committed.push_str(&text);
                        }
                    }
                    assert_eq!(
                        committed, expected,
                        "new route {recipe:?}, auto={auto}, custom={custom}"
                    );
                    assert!(session.raw.is_empty());
                }
            }
        }
    }

    #[test]
    fn sentence_phrase_merge_invalidates_even_dictionary_first_recipe() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        struct PhraseDecoder;
        impl hufu_engine::SentenceDecoder for PhraseDecoder {
            fn decode_rich(&self, raw: &str) -> Arc<hufu_engine::SentenceDecode> {
                Arc::new(hufu_engine::SentenceDecode {
                    hits: if raw == "mlwe" {
                        vec![hufu_engine::SentenceHit {
                            text: "两次".into(),
                            score: -7.0,
                            confidence: -7.0,
                            max_rank: 1,
                            sum_rank: 2,
                            exact: true,
                            word_ends: vec![(1, 2), (2, 4)],
                            segmented: "ml we".into(),
                            partial: false,
                            implicit2: false,
                        }]
                    } else {
                        Vec::new()
                    },
                    truncated: false,
                    early_hits: Vec::new(),
                    early_truncated: false,
                })
            }
        }
        let dir = std::env::temp_dir().join(format!(
            "hufu-trainer-sentence-replay-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        std::fs::write(
            dir.join("test.dict.yaml"),
            "---\nname: test\nsort: by_weight\n...\n𠓅\tmlwe\t900\n两\tml\t800\n次\twe\t800\n",
        )
        .unwrap();
        let mut cfg = Config::default();
        cfg.sentence.enabled = true;
        cfg.sentence.auto_enable = false;
        let mut engine = hufu_engine::Engine::with_schema_dir(&dir, cfg).unwrap();
        engine.set_sentence_decoder(Some(Arc::new(PhraseDecoder)));
        assert!(engine.sentence_active());
        assert_eq!(engine.schema.candidates("mlwe")[0].text, "𠓅");
        assert!(Analyzer::new(&engine.schema, &engine.config, false)
            .hint("𠓅")
            .code
            .is_some());
        let mut session = hufu_engine::Session::new(true);
        let mut committed = String::new();
        for key in "mlwe ".chars() {
            if let Some(text) = engine
                .process_key(&mut session, hufu_types::KeyInput::char_key(key))
                .commit
            {
                committed.push_str(&text);
            }
        }
        assert_eq!(
            committed, "两次",
            "real engine phrase merge overrides dictionary rank 1"
        );
        // 【整改4 2026-11-08】整句模式词提已放开：hints=码表编码（信息口径）。
        // 即便真实引擎按整句合并把 mlwe 打成「两次」，词提仍标注 𠓅→mlwe/两→ml/次→we
        // 的码表编码与位次（整句下位次是码表口径，非运行时候选口径）。
        let words = Analyzer::new(&engine.schema, &engine.config, true)
            .analyze("𠓅两次", Instant::now())
            .unwrap();
        let top: Vec<(String, String, usize)> = words
            .iter()
            .filter(|w| w.len > 1 || w.hint.code.is_some())
            .map(|w| {
                (
                    w.text.clone(),
                    w.hint.code.clone().unwrap_or_default(),
                    w.hint.rank,
                )
            })
            .collect();
        assert_eq!(
            top,
            vec![
                ("𠓅".into(), "mlwe".into(), 1),
                ("两".into(), "ml".into(), 1),
                ("次".into(), "we".into(), 1),
            ],
            "sentence hints carry dictionary codes"
        );
        // 整句下不给选重键（rank>1 词提的 tail 为空），这是维持的保守语义。
        assert!(words
            .iter()
            .all(|w| w.hint.tail.is_empty() || w.hint.rank <= 1));
    }

    #[test]
    fn reserved_prefix_and_digit_suffix_recipes_replay_as_nonselections() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "hufu-trainer-reserved-replay-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        std::fs::write(dir.join("test.dict.yaml"),
            "---\nname: test\nsort: by_weight\n...\n甲\tab\t900\n乙\tab\t800\n丙\tab\t700\n丁\tab\t600\n戊\tb4\t500\n").unwrap();
        let mut engine = hufu_engine::Engine::with_schema_dir(&dir, Config::default()).unwrap();
        assert!(Analyzer::new(&engine.schema, &engine.config, false)
            .hint("丁")
            .code
            .is_none());
        let mut session = hufu_engine::Session::new(true);
        for key in "ab4".chars() {
            let outcome = engine.process_key(&mut session, hufu_types::KeyInput::char_key(key));
            assert!(
                outcome.commit.is_none(),
                "numeric suffix continuation must not select rank 4"
            );
        }
        assert_eq!(session.raw, "ab4");
        engine.config.reverse.prefix = 'a';
        engine.config.reverse.enabled = false;
        assert!(Analyzer::new(&engine.schema, &engine.config, false)
            .hint("甲")
            .code
            .is_none());
        let mut session = hufu_engine::Session::new(true);
        let out = engine.process_key(&mut session, hufu_types::KeyInput::char_key('a'));
        assert_eq!(
            out.commit.as_deref(),
            Some("·"),
            "disabled reverse prefix still owns the key"
        );

        let mut cfg = Config::default();
        cfg.candidates.second_select = ' ';
        assert!(Analyzer::new(&engine.schema, &cfg, false)
            .hint("甲")
            .code
            .is_none());
        cfg = Config::default();
        cfg.candidates.second_select = '!';
        cfg.candidates.custom_select_keys = vec!['1', '2'];
        assert_ne!(
            Analyzer::new(&engine.schema, &cfg, false).hint("乙").tail,
            "!"
        );
    }

    #[test]
    fn parse_hotkey_accepts_legal_and_rejects_illegal() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        // 空串/纯空白 = 禁用热键
        assert_eq!(parse_hotkey(""), None);
        assert_eq!(parse_hotkey("   "), None);
        // 默认热键 ctrl+alt+g → MOD_CONTROL|MOD_ALT|MOD_NOREPEAT, 'G'=0x47
        assert_eq!(
            parse_hotkey("ctrl+alt+g"),
            Some((MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 0x47))
        );
        // 大小写宽容、修饰键顺序任意、win/meta 别名
        assert_eq!(
            parse_hotkey("Shift+Win+F5"),
            Some((MOD_SHIFT | MOD_WIN | MOD_NOREPEAT, 0x74))
        );
        assert_eq!(parse_hotkey("meta+z"), Some((MOD_WIN | MOD_NOREPEAT, 0x5A)));
        // 数字与 f12
        assert_eq!(parse_hotkey("alt+7"), Some((MOD_ALT | MOD_NOREPEAT, 0x37)));
        assert_eq!(
            parse_hotkey("shift+f12"),
            Some((MOD_SHIFT | MOD_NOREPEAT, 0x7B))
        );
        // 容忍多余空格与空段（手改 config 常见）
        assert_eq!(
            parse_hotkey(" ctrl + alt + g "),
            Some((MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 0x47))
        );
        // 非法：裸主键（无修饰键会吞掉全局打字）
        assert_eq!(parse_hotkey("g"), None);
        // 非法：只有修饰键（末段按主键解析必失败）
        assert_eq!(parse_hotkey("ctrl+alt"), None);
        assert_eq!(parse_hotkey("ctrl"), None);
        // 非法：未知修饰键 / 未知主键 / f13 越界 / 主键后多一段
        assert_eq!(parse_hotkey("ctrl+alt+zz"), None);
        assert_eq!(parse_hotkey("ctrl+alt+g+g"), None);
        assert_eq!(parse_hotkey("caps+g"), None);
        assert_eq!(parse_hotkey("ctrl+f13"), None);
        assert_eq!(parse_hotkey("ctrl+alt+space"), None);
    }

    #[test]
    fn real_http_full_analysis_numbers_and_gating() {
        let _serial = crate::GLOBALS_TEST_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let _analyze_gate = ANALYZE_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        let dir =
            std::env::temp_dir().join(format!("hufu-trainer-full-analysis-{}", std::process::id()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let schema_dir = dir.join("码表/test");
        std::fs::create_dir_all(&schema_dir).unwrap();
        // 你 jx 唯一（rank1）；ab 两词乙 rank1 / 丙 rank2（选重 1）；
        // 你好 jxbh（rank1，4 码）；字 z 无码（SingleChar，code_len 0）。
        std::fs::write(
            schema_dir.join("test.dict.yaml"),
            "---\nname: test\nsort: by_weight\n...\n你\tjx\t900\n好\tbh\t900\n乙\tab\t800\n丙\tab\t700\n你好\tjxbh\t950\n",
        )
        .unwrap();
        let mut cfg = Config::default();
        cfg.schema.current = "test".into();
        cfg.sentence.enabled = false;
        cfg.save(&dir.join("config.json")).unwrap();
        let host = Arc::new(Mutex::new(Host::new(&dir).unwrap()));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            crate::http::serve_test_requests(listener, |req| crate::route(&host, req), 8);
        });
        let post = |body: &str| {
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let head = format!("POST /api/trainer/analyze HTTP/1.1\r\nHost: {address}\r\nOrigin: http://{address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(body.as_bytes()).unwrap();
            let mut response = Vec::new();
            stream.read_to_end(&mut response).unwrap();
            let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            serde_json::from_slice::<serde_json::Value>(&response[split + 4..]).unwrap()
        };
        // 不带 include_full → 无 full_analysis 字段
        let v = post(r#"{"text":"你好乙丙"}"#);
        assert!(v.get("full_analysis").is_none());
        // include_full=true：词提分词口径 你好|乙|丙 → jxbh(4) + ab(2) + ab(2)
        let v = post(r#"{"text":"你好乙丙","include_full":true}"#);
        let fa = v["full_analysis"].clone();
        assert_eq!(fa["total_code_len"], 8, "4+2+2");
        assert_eq!(fa["select_count"], 1, "丙 rank2；乙 rank1 不计");
        assert_eq!(fa["avg_code_len_per_char"], 8.0 / 4.0);
        let entries = fa["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0]["word"], "你好");
        assert_eq!(entries[0]["code"], "jxbh");
        assert_eq!(entries[0]["code_len"], 4);
        assert_eq!(entries[0]["rank"], 1);
        assert_eq!(entries[2]["word"], "丙");
        assert_eq!(entries[2]["code"], "ab");
        assert_eq!(entries[2]["rank"], 2);
        // 无码字（字）计 0 码长、rank 0，不增加选重
        let v = post(r#"{"text":"乙字","include_full":true}"#);
        let fa = &v["full_analysis"];
        assert_eq!(fa["total_code_len"], 2);
        assert_eq!(fa["select_count"], 0);
        assert_eq!(fa["avg_code_len_per_char"], 1.0);
        let entries = fa["entries"].as_array().unwrap();
        assert_eq!(entries[1]["word"], "字");
        assert_eq!(entries[1]["code_len"], 0);
        assert_eq!(entries[1]["rank"], 0);
        // 超长（>2000 字）请求：正常词提，但省略 full_analysis
        let long = format!(
            "{{\"text\":\"{}\",\"include_full\":true}}",
            "乙".repeat(FULL_ANALYSIS_MAX_CHARS + 1)
        );
        let v = post(&long);
        assert!(v.get("full_analysis").is_none());
        assert!(v["words"].as_array().unwrap().len() > 0);
        drop(server);
    }
}
