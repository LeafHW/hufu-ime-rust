//! Trainer API. Only the immutable dictionary handles and small user/config state
//! cross the engine lock; segmentation and candidate lookups run outside it.
use crate::{
    host::Host,
    http::{Request, Response},
};
use hufu_config::Config;
use hufu_dict::Schema;
use serde::Serialize;
use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    hash::{Hash, Hasher},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

const MAX_CHARS: usize = 10_000;
/// 【整改13】全文理论分析仅对 ≤2000 字文章开放（逐词聚合预算护栏）。
const FULL_ANALYSIS_MAX_CHARS: usize = 2_000;
static ANALYZING: AtomicBool = AtomicBool::new(false);
struct CachedAnalysis {
    text: String,
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
struct Admission;
impl Drop for Admission {
    fn drop(&mut self) {
        ANALYZING.store(false, Ordering::Release);
    }
}

pub const DISABLED_MESSAGE: &str = "跟打器内置入口已关闭，请先在虎符设置的概览页启用";

/// try_lock 未拿到引擎锁的瞬时失败文案（tray 据此识别为瞬时并静默重试）。
pub const BUSY_MESSAGE: &str = "输入引擎忙，请稍后重试";

fn snapshot(host: &Mutex<Host>) -> Result<(Schema, Config, bool), &'static str> {
    let h = host.try_lock().map_err(|_| BUSY_MESSAGE)?;
    let state = &h.engine.schema;
    if state.user_dict.entries.len() > 2000
        || state.user_dict.weights.len() > 2000
        || state.user_dict.hidden.len() > 2000
        || state.adjust.operation_count() > 2000
        || state.adjust.weights.len() > 2000
    {
        return Err("用户词或调整过多，已跳过词提以保持输入响应");
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
    if bytes > 512 * 1024 {
        return Err("用户词快照过大，已跳过词提");
    }
    let s = &h.engine.schema;
    // Deliberately do not clone annotations, reverse tables or supplement corpora.
    Ok((
        Schema {
            name: s.name.clone(),
            dir: s.dir.clone(),
            dict: s.dict.clone(),
            super_dict: s.super_dict.clone(),
            personal: s.personal.clone(),
            adjust: s.adjust.clone(),
            user_dict: s.user_dict.clone(),
            symbols: Default::default(),
            supplement: Default::default(),
            pinyin: None,
            unicode_block: None,
            split: None,
            reverse: None,
            reverse_path: None,
            reverse_rule: None,
            encoder_rules: Vec::new(),
        },
        h.engine.config.clone(),
        h.engine.sentence_active(),
    ))
}

fn version(s: &Schema, cfg: &Config, sentence: bool) -> String {
    let mut h = DefaultHasher::new();
    s.name.hash(&mut h);
    (Arc::as_ptr(&s.dict) as usize).hash(&mut h);
    (Arc::as_ptr(&s.personal) as usize).hash(&mut h);
    s.adjust.to_lines().hash(&mut h);
    s.user_dict.to_lines().hash(&mut h);
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
            let (schema, schema_ver, enabled, max_code_length, sentence, unavailable) =
                match snapshot(host) {
                    Ok((s, cfg, sentence)) => (
                        s.name.clone(),
                        version(&s, &cfg, sentence),
                        cfg.trainer.enabled,
                        cfg.input.max_code_length,
                        sentence,
                        None,
                    ),
                    Err(reason) => {
                        // A dictionary snapshot limit disables hints, not the feature switch.
                        // Read only the small status fields without copying user dictionaries.
                        let h = match host.try_lock() {
                            Ok(h) => h,
                            Err(_) => return Response::err(503, reason),
                        };
                        (
                            h.engine.schema.name.clone(),
                            String::new(),
                            h.engine.config.trainer.enabled,
                            h.engine.config.input.max_code_length,
                            h.engine.sentence_active(),
                            Some(reason),
                        )
                    }
                };
            Response::json(
                &serde_json::json!({"enabled":enabled,"installed":executable().is_some(),
                "schema":schema,"schema_ver":schema_ver,
                "max_article_chars":MAX_CHARS,"max_code_length":max_code_length,
                "version":"0.1.0","hint_scope":"dictionary","sentence_active":sentence,
                "hint_unavailable_reason":unavailable}),
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
            let char_count = text.chars().count();
            if char_count > MAX_CHARS {
                return Response::err(413, "文章最多 10000 字");
            }
            if ANALYZING
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                return Response::err(429, "正在分析另一篇文章，请稍后重试");
            }
            let _admission = Admission;
            let started = Instant::now();
            let (s, cfg, sentence) = match snapshot(host) {
                Ok(s) => s,
                Err(e) => return Response::err(503, e),
            };
            let ver = version(&s, &cfg, sentence);
            if let Some(cache) = CACHE.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
                if cache.text == text
                    && cache.version == ver
                    && cache.single == single
                    && cache.include_full == include_full
                {
                    return cache.response.clone();
                }
            }
            match Analyzer::new(&s, &cfg, sentence).analyze_mode(text, started, single) {
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
                    "single":single,"words":words,"elapsed_ms":started.elapsed().as_secs_f64()*1000.0});
                    if let Some(fa) = full_analysis {
                        body["full_analysis"] = fa;
                    }
                    let response = Response::json(&body);
                    *CACHE.lock().unwrap_or_else(|p| p.into_inner()) = Some(CachedAnalysis {
                        text: text.into(),
                        version: ver,
                        single,
                        include_full,
                        response: response.clone(),
                        _dict: s.dict.clone(),
                        _personal: s.personal.clone(),
                    });
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

struct Analyzer<'a> {
    s: &'a Schema,
    cfg: &'a Config,
    sentence: bool,
    extra: HashMap<String, Vec<String>>,
    extra_prefixes: HashSet<String>,
    started: Instant,
    work: usize,
    exceeded: bool,
    hints: HashMap<String, Hint>,
    candidates: HashMap<String, Vec<hufu_dict::DictEntry>>,
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
            extra,
            extra_prefixes,
            started: Instant::now(),
            work: 0,
            exceeded: false,
            hints: HashMap::new(),
            candidates: HashMap::new(),
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
    fn tail(&self, code: &str, rank: usize, count: usize) -> Option<String> {
        // The live z-domain also merges symbol tables and their continuations.
        // This dictionary-only snapshot cannot promise its displayed rank or auto commit.
        if self.s.dict.z_symbol_domain && code.starts_with('z') {
            return None;
        }
        if rank == 1 {
            let auto = !self.sentence
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
        if self.sentence {
            return None;
        }
        let c = &self.cfg.candidates;
        let mut keys = Vec::new();
        if rank == 2 {
            keys.push(c.second_select);
        }
        if rank == 3 {
            keys.push(c.third_select);
        }
        if let Some(&k) = c.custom_select_keys.get(rank - 1) {
            keys.push(k);
        }
        keys.push(if rank == 10 {
            '0'
        } else {
            char::from_digit(rank as u32, 10)?
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
                resolved == rank
                    && (!c.paging_keys.contains(k)
                        || k == c.second_select
                        || k == c.third_select
                        || c.custom_select_keys.contains(&k))
                    && (!self.cfg.input.alphabet.contains(k)
                        || k == c.second_select
                        || k == c.third_select
                        || c.custom_select_keys.contains(&k))
            })
            .map(|k| k.to_string())
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
    /// 编码可打性过滤（两模式共用）：反查前缀占用/超长/非编码字母表/
    /// 键位映射（自定义键位方案）/opencc 转换。整句模式同样保留这些
    /// 保守限制（整改4：用户只点名放开整句的"跳过"，未放开过滤）。
    fn code_typable(&self, code: &str) -> bool {
        !(code.is_empty()
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
        let mut best_cost = usize::MAX;
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
            if rank > self.cfg.candidates.page_size.clamp(1, 10)
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
            let cost = code.len() + usize::from(!tail.is_empty());
            if cost < best_cost || (cost == best_cost && rank < best.rank) {
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
        self.started = started;
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let mut costs = vec![usize::MAX / 2; n + 1];
        let mut ends = vec![1; n];
        costs[n] = 0;
        // Minimize unknown characters first, then actual key count. A longest
        // word need not be the cheapest executable route.
        for i in (0..n).rev() {
            if !self.budget(1) {
                return Err("分析超时，请缩短文章后重试");
            }
            // Single-character practice must not use a multi-character recipe.
            for len in 1..=if single { 1 } else { 6.min(n - i) } {
                let word: String = chars[i..i + len].iter().collect();
                let h = self.hint(&word);
                if len > 1 && h.code.is_none() {
                    continue;
                }
                let cost = h
                    .code
                    .as_ref()
                    .map(|c| c.len() + usize::from(!h.tail.is_empty()))
                    .unwrap_or(100_000)
                    + costs[i + len];
                if cost < costs[i] || (cost == costs[i] && len > ends[i]) {
                    costs[i] = cost;
                    ends[i] = len;
                }
            }
        }
        if self.exceeded {
            return Err("词提分析预算已用尽，请缩短文章后重试");
        }
        let mut out = Vec::new();
        let mut i = 0;
        while i < n {
            if !self.budget(1) {
                return Err("词提分析预算已用尽，请缩短文章后重试");
            }
            let len = ends[i].max(1);
            let text: String = chars[i..i + len].iter().collect();
            let hint = self.hint(&text);
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
                    1 => "Jian1",
                    2 => "Jian2",
                    3 => "Jian3",
                    4 => "Ma4",
                    _ => "Normal",
                }
            };
            let char_hints = chars[i..i + len]
                .iter()
                .map(|&ch| CharHint {
                    ch,
                    hint: self.hint(&ch.to_string()),
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
            i += len;
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

    #[test]
    fn trainer_setting_guards_open_and_single_analysis_has_separate_cache() {
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
        // Disabled must win even if no trainer executable is installed alongside the test binary.
        let response = route(
            &host,
            &request("POST", "/api/trainer/open", serde_json::json!({})),
        );
        assert_eq!(response.status, 403);
        let status = route(
            &host,
            &request("GET", "/api/trainer/status", serde_json::json!({})),
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&status.body).unwrap()["enabled"],
            false
        );
        {
            let mut h = host.lock().unwrap();
            let mut enabled = h.engine.config.clone();
            enabled.trainer.enabled = true;
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
            let response = route(
                &host,
                &request(
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

        // Large personal dictionaries may turn hints off, but settings must remain usable.
        for oversized_bytes in [false, true] {
            {
                let mut h = host.lock().unwrap();
                h.engine.config.trainer.enabled = !oversized_bytes;
                let user = &mut h.engine.schema.user_dict;
                user.hidden.clear();
                if oversized_bytes {
                    user.hidden.insert(("a".into(), "甲".repeat(180_000)));
                } else {
                    user.hidden
                        .extend((0..2001).map(|i| (format!("a{i}"), "甲".into())));
                }
            }
            let response = route(
                &host,
                &request("GET", "/api/trainer/status", serde_json::json!({})),
            );
            assert_eq!(response.status, 200);
            let status: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
            assert_eq!(status["enabled"], !oversized_bytes);
            assert!(status["installed"].is_boolean());
            assert!(status["hint_unavailable_reason"].is_string());
            let response = route(
                &host,
                &request(
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
        let mut s = schema(vec![("ab", "甲"), ("ab", "乙"), ("abcd", "甲乙")]);
        let c = Config::default();
        assert_eq!(Analyzer::new(&s, &c, false).hint("乙").rank, 2);
        s.adjust.remove("ab", "甲");
        assert_eq!(Analyzer::new(&s, &c, false).hint("乙").rank, 1);
        assert!(Analyzer::new(&s, &c, false).hint("甲").code.is_none());
        assert!(Analyzer::new(&s, &c, false).hint("甲甲乙").code.is_none());
    }
    #[test]
    fn unicode_positions_and_minimum_keys() {
        let s = schema(vec![("a", "甲"), ("b", "乙"), ("abcd", "甲乙")]);
        let c = Config::default();
        let mut a = Analyzer::new(&s, &c, false);
        let w = a.analyze("甲乙😀甲", Instant::now()).unwrap();
        assert_eq!(
            w.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            vec!["甲", "乙", "😀", "甲"]
        );
        assert_eq!(w[3].start, 3);
    }
    #[test]
    fn selection_conflicts_and_auto_commit() {
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
    fn inaccessible_page_is_not_a_hint() {
        let entries = (0..11).map(|i| ("a", format!("字{i}"))).collect::<Vec<_>>();
        let s = schema(entries.iter().map(|(c, t)| (*c, t.as_str())).collect());
        let c = Config::default();
        assert!(Analyzer::new(&s, &c, false).hint("字10").code.is_none());
    }
    #[test]
    fn z_domain_and_one_key_auto_commit_are_not_false_recipes() {
        let mut s = schema(vec![("za", "字"), ("a", "甲")]);
        Arc::get_mut(&mut s.dict).unwrap().z_symbol_domain = true;
        let mut c = Config::default();
        c.input.auto_select_unique = true;
        assert!(Analyzer::new(&s, &c, false).hint("字").code.is_none());
        c.input.max_code_length = 1;
        assert_eq!(Analyzer::new(&s, &c, false).hint("甲").tail, "_");
    }
    #[test]
    fn fractional_and_user_weights_invalidate_version() {
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
            }
        }
    }

    #[test]
    fn sentence_phrase_merge_invalidates_even_dictionary_first_recipe() {
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
        assert!(words.iter().all(|w| w.hint.tail.is_empty() || w.hint.rank <= 1));
    }

    #[test]
    fn reserved_prefix_and_digit_suffix_recipes_replay_as_nonselections() {
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
        assert_eq!(
            parse_hotkey("meta+z"),
            Some((MOD_WIN | MOD_NOREPEAT, 0x5A))
        );
        // 数字与 f12
        assert_eq!(
            parse_hotkey("alt+7"),
            Some((MOD_ALT | MOD_NOREPEAT, 0x37))
        );
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
        let _analyze_gate = ANALYZE_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        let dir = std::env::temp_dir().join(format!(
            "hufu-trainer-full-analysis-{}",
            std::process::id()
        ));
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
