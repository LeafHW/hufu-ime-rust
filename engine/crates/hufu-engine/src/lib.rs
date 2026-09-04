//! hufu-engine —— 平台无关的输入法会话引擎。
//!
//! 状态机：按键 →（切换键 / 标点 / 反查 / 命令 / 编码追加与顶功 /
//! 候选生成 / 选重翻页）→ KeyOutcome。

// 整句短语压前的「真词地板」（对数得分，f64）：
// 实测校准——真词短语 两次(mlwe)≈-7.72 / 真好(nqbh)≈-8.09；
// 垃圾切分 午王(ennw)≈-21.5。取 -12 分层：强短语压生僻表项置前
// （Rime 同拍），弱切分不压词典精确匹配（框@ennw 居首，虎爪/Rime 同）。
const SENT_PHRASE_FRONT_FLOOR: f64 = -12.0;

pub mod dynamic;
pub mod punct;
pub mod session;

pub use punct::PairState;
pub use session::{EarlyHistory, Session};

use hufu_config::Config;
use hufu_dict::annotation::ReverseTable;
use hufu_dict::entry::DictEntry;
use hufu_dict::schema::Schema;
use hufu_types::{
    Candidate, CandidateKind, InputMode, KeyCode, KeyInput, KeyOutcome, SessionState,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 单条整句解码命中（对齐 Rime emit 结果）。
#[derive(Debug, Clone)]
pub struct SentenceHit {
    pub text: String,
    /// 排序分（含名次惩罚 / EOS / 孤立惩罚）
    pub score: f64,
    /// 置信分（同文本聚合质量 + EOS - 孤立惩罚）
    pub confidence: f64,
    /// 全路径最大码表名次（排序首位键）
    pub max_rank: usize,
    /// 全路径各段码表名次总和（选重深度）：无锁候选各段名次之和，
    /// 「们服」=1+1=2、「舒服」=2+1=3。rerank 换序约束用：无锁时
    /// 深于原首选的候选不得被重排提前（要打「舒服」规范打法是 ja2vz
    /// 锁第 2 选——见 2026-09-05 用户实测 舒服/坏人/势力 三案例）。
    pub sum_rank: usize,
    /// 全路径每段精确对应实打编码：段词条码表码长==消耗键数（无前
    /// 缀扩展）且名次 1（或被锁钉住=用户打了选重键）。无锁短码候选
    /// 过滤用（2026-09-05 用户规则：「选重的数字、词锁的 ; 都是编码
    /// 的一部分——没打就不要出现在候选里」：javaz 只该有「们服」与
    /// 整码字，「舒服」(ja2)「们改变」(javz;) 不出现；打 javz; 或
    /// ja2vz 时才出现）。
    pub exact: bool,
    /// 词边界：(累计字数, base 消耗位置)
    pub word_ends: Vec<(usize, usize)>,
    /// 分段显示（空格分隔编码段）
    pub segmented: String,
    /// 不完全尾候选（尾部编码未完成，text 为 raw 的真前缀产物）：
    /// 录入中间态候选框用（正在打的词），完整态恒 false。
    pub partial: bool,
}

/// 整句解码结果（含提前上屏置信源）。
pub struct SentenceDecode {
    /// 完整解码命中（max_rank, score 排序）
    pub hits: Vec<SentenceHit>,
    /// 完整解码是否被 beam 截断（置信不可信）
    pub truncated: bool,
    /// 不完全尾候选（尾部未成码时把前缀视为完整句；confidence 排序）
    pub early_hits: Vec<SentenceHit>,
    /// 不完全尾是否截断
    pub early_truncated: bool,
}

/// 整句解码器接口（由 hufu-sentence 实现，可注入替换）。
pub trait SentenceDecoder: Send + Sync {
    /// 富解码：raw（含选重后缀）→ 命中 + 提前上屏置信源。
    fn decode_rich(&self, raw: &str) -> std::sync::Arc<SentenceDecode>;
    /// 字是否生僻（模型字频名次超阈值）——候选框生僻下沉判据。
    /// 默认 false（无模型实现时不下沉）。
    fn rare_hint(&self, _ch: char) -> bool {
        false
    }
    /// 组句：raw（含选重后缀）→ 已排序候选（默认由富解码派生）。
    fn decode(&self, raw: &str) -> Vec<Candidate> {
        self.decode_rich(raw)
            .hits
            .iter()
            .map(|h| {
                let mut c = Candidate::new(h.text.clone(), raw.to_string(), CandidateKind::Sentence);
                c.weight = h.score;
                c
            })
            .collect()
    }
}

/// 【Shift 标点 2026-09-05】US 键盘 Shift 形态（基础键 → Shift 字符）。
/// TSF 层传基础键名+shift=true（Shift+, → key=","），引擎侧转成 shift
/// 形态字符再走标点映射：shift+,→<→《、shift+'→"→“、shift+/→?→？、
/// shift+1→!→！。
fn shift_form(c: char) -> Option<char> {
    Some(match c {
        ',' => '<',
        '.' => '>',
        '/' => '?',
        ';' => ':',
        '\'' => '"',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        '`' => '~',
        '=' => '+',
        '-' => '_',
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        _ => return None,
    })
}

/// 置信前缀提案（Rime confidence_proposal）：软最大前缀质量占比 ≥ 阈值的最长真前缀。
/// 返回 (提案, 份额)。
pub fn confidence_proposal(cands: &[&SentenceHit], threshold: f64) -> (String, f64) {
    if cands.is_empty() {
        return (String::new(), 0.0);
    }
    let max_score = cands
        .iter()
        .map(|c| c.confidence)
        .fold(f64::NEG_INFINITY, f64::max);
    let total: f64 = cands.iter().map(|c| (c.confidence - max_score).exp()).sum();
    let mut prefix_mass: Vec<(Vec<char>, f64)> = Vec::new();
    for c in cands {
        let weight = (c.confidence - max_score).exp();
        let chars: Vec<char> = c.text.chars().collect();
        if chars.len() < 2 {
            continue;
        }
        let mut prefix: Vec<char> = Vec::new();
        for _l in 1..chars.len() {
            // 真前缀（不含全长）
            prefix.push(chars[_l - 1]);
            if let Some((_, m)) = prefix_mass.iter_mut().find(|(p, _)| *p == prefix) {
                *m += weight;
            } else {
                prefix_mass.push((prefix.clone(), weight));
            }
        }
    }
    let mut proposal: Vec<char> = Vec::new();
    let mut share = 0.0;
    for (p, m) in &prefix_mass {
        let s = m / total;
        if s >= threshold && p.len() > proposal.len() {
            proposal = p.clone();
            share = s;
        }
    }
    (proposal.into_iter().collect(), share)
}

/// 证据史公共前缀（Rime common_history_prefix）。
fn common_history_prefix(history: &[crate::session::EarlyHistory]) -> String {
    if history.is_empty() {
        return String::new();
    }
    let mut common: Vec<char> = history[0].proposal.chars().collect();
    for e in history.iter().skip(1) {
        let chars: Vec<char> = e.proposal.chars().collect();
        let mut matched = 0usize;
        while matched < common.len()
            && matched < chars.len()
            && common[matched] == chars[matched]
        {
            matched += 1;
        }
        common.truncate(matched);
        if common.is_empty() {
            return String::new();
        }
    }
    common.into_iter().collect()
}

/// 前缀在证据史中的一致消耗长度（最少 2 键、全部同值；Rime stable_history_raw_length）。
fn stable_history_raw_length(history: &[crate::session::EarlyHistory], text: &str) -> usize {
    if text.is_empty() || history.len() < 2 {
        return 0;
    }
    let mut stable = 0usize;
    for e in history {
        let raw_length = e
            .raw_lengths
            .iter()
            .find(|(p, _)| p == text)
            .map(|(_, l)| *l)
            .unwrap_or(0);
        if raw_length == 0 {
            return 0;
        }
        if stable == 0 {
            stable = raw_length;
        } else if stable != raw_length {
            return 0;
        }
    }
    stable
}

/// 为提案构建 前缀→orig消耗 映射（Rime raw_lengths_for_proposal）。
fn build_raw_lengths(
    cands: &[&SentenceHit],
    full_raw: &str,
    digit_coded: bool,
    is_code_prefix: &dyn Fn(&str) -> bool,
) -> Vec<(String, usize)> {
    let parsed = if digit_coded {
        parse_rank_locks_keep_digits(full_raw, is_code_prefix)
    } else {
        parse_rank_locks(full_raw)
    };
    let base_len = parsed.base.chars().count();
    let full_len = full_raw.chars().count();
    let mut out: Vec<(String, usize)> = Vec::new();
    for h in cands {
        let mut cum = 0usize;
        let text_all: Vec<char> = h.text.chars().collect();
        for (chars_cum, base_end) in &h.word_ends {
            cum = *chars_cum;
            if cum == 0 || cum > text_all.len() {
                continue;
            }
            let prefix: String = text_all[..cum].iter().collect();
            if out.iter().any(|(p, _)| *p == prefix) {
                continue;
            }
            let orig = if *base_end >= base_len {
                full_len
            } else {
                parsed.orig_of_base[*base_end]
            };
            out.push((prefix, orig));
        }
    }
    out
}

/// 「写入编码选重」解析结果（整句模式选重后缀）。
#[derive(Debug, Clone)]
pub struct RankLocks {
    /// 去掉选重后缀后的纯编码
    pub base: String,
    /// (段结束位置, 1 起名次)：该位置必须有段恰好结束于此并取该名次。
    /// 段起点由解码器决定（;只锁它前面的那个词段，不锁整个字母流）。
    pub locks: Vec<(usize, usize)>,
    /// base 第 i 个字符在原始 raw 中的字符下标（提前上屏 consumed 映射用）
    pub orig_of_base: Vec<usize>,
}

impl RankLocks {
    pub fn has_locks(&self) -> bool {
        !self.locks.is_empty()
    }
}

/// 解析选重后缀：raw 中紧跟在编码块（[a-z]+）之后的
/// `;`(第2) `'`(第3) `2-9`(第N) `0`(第10) 在该处设段名次锁。
/// 其余字符原样保留在 base 中（lenient）。连续后缀只取第一个。
pub fn parse_rank_locks(raw: &str) -> RankLocks {
    let chars: Vec<char> = raw.chars().collect();
    let mut base = String::new();
    let mut locks = Vec::new();
    let mut orig_of_base = Vec::new();
    let mut in_run = false; // 是否处于字母块中（后缀仅跟在字母后有效）
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let rank = match c {
            ';' => Some(2),
            '\'' => Some(3),
            '1'..='9' => Some(c.to_digit(10).unwrap() as usize),
            '0' => Some(10),
            _ => None,
        };
        if let Some(r) = rank {
            if in_run {
                // 在当前 base 末尾设段名次锁
                locks.push((base.chars().count(), r));
                // 连续后缀：跳过（只取第一个）
                while i + 1 < chars.len() && matches!(chars[i + 1], ';' | '\'' | '0'..='9') {
                    i += 1;
                }
                in_run = false;
                i += 1;
                continue;
            }
        }
        in_run = c.is_ascii_lowercase();
        base.push(c);
        orig_of_base.push(i);
        i += 1;
    }
    RankLocks {
        base,
        locks,
        orig_of_base,
    }
}

/// 【数字编码 2026-09-05】数字编码表的锁解析：raw 里的数字按「码表
/// 延续」逐个判定——到该数字为止的前缀在码表有延续（如 a8、u3 的
/// 8/3）则保留为编码字符；无延续（如 ve; 锁转成的内部数字 ve2）则
/// 仍做「选重第 N」锁。分号/单引号词锁恒为锁。is_code_prefix 由
/// 调用方传入（码表前缀查询）。
pub fn parse_rank_locks_keep_digits(
    raw: &str,
    is_code_prefix: &dyn Fn(&str) -> bool,
) -> RankLocks {
    let chars: Vec<char> = raw.chars().collect();
    let mut base = String::new();
    let mut locks = Vec::new();
    let mut orig_of_base = Vec::new();
    let mut in_run = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let rank = match c {
            ';' => Some(2),
            '\'' => Some(3),
            '0'..='9' => {
                // 数字：码表延续 → 编码字符（保留）；否则锁。
                // base+c 是当前累计前缀（含更早保留的数字，如 r8 后再打数字）。
                let probe = format!("{}{c}", base);
                if is_code_prefix(&probe) {
                    None
                } else {
                    Some(c.to_digit(10).unwrap() as usize).filter(|&r| r != 1)
                }
            }
            _ => None,
        };
        if let Some(r) = rank {
            if in_run {
                locks.push((base.chars().count(), r));
                while i + 1 < chars.len() && matches!(chars[i + 1], ';' | '\'' | '0'..='9') {
                    // 连续后缀跳过前，先看下一个数字是否码表延续（是则停，
                    // 后续数字可能是编码，如 ve2 锁后跟 8 属新段编码）
                    let nc = chars[i + 1];
                    if nc.is_ascii_digit() {
                        let probe = format!("{}{nc}", base);
                        if is_code_prefix(&probe) {
                            break;
                        }
                    }
                    i += 1;
                }
                in_run = false;
                i += 1;
                continue;
            }
        }
        in_run = c.is_ascii_lowercase() || c.is_ascii_digit();
        base.push(c);
        orig_of_base.push(i);
        i += 1;
    }
    RankLocks {
        base,
        locks,
        orig_of_base,
    }
}

/// 引擎：配置 + 当前方案 + 可选整句解码器。
pub struct Engine {
    pub config: Config,
    pub schema: Schema,
    /// 可用方案名列表
    pub schemas: Vec<String>,
    /// 用户数据目录
    pub data_dir: PathBuf,
    sentence: Option<Arc<dyn SentenceDecoder>>,
    /// 神经重排结果缓存：key=committed_raw+raw → 有序候选文本
    /// Arc 共享给 server 重排线程写入
    pub rerank_cache: Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>>,
    /// 单次按键内的提示音标签提示（select/page 覆盖默认 key/commit）
    sound_hint: Option<&'static str>,
    /// 上一次提交文本（跨 session，进程级）——{重复上屏} 的回放源
    pub last_commit: String,
    /// OpenCC 转换表（opencc.enabled 时懒加载）
    opencc: Option<hufu_dict::OpenCc>,
    opencc_emoji: Option<hufu_dict::OpenCc>,
    opencc_loaded: bool,
}

impl Engine {
    /// 【数字编码】按当前码表选择锁解析：数字编码表按码表延续逐位
    /// 判定数字是编码还是选重锁；普通表数字一律选重锁。
    pub fn parse_locks(&self, raw: &str) -> RankLocks {
        if self.schema.dict.digit_coded {
            let dict = &self.schema.dict;
            // 整体或任意后缀是词条/前缀都算编码延续（跨段：vvb8 的
            // b8=如）。锁数字（ve2：e2/2 均无）不命中。
            let is_code = |p: &str| {
                let cs: Vec<char> = p.chars().collect();
                (1..=cs.len()).any(|j| {
                    let s: String = cs[cs.len() - j..].iter().collect();
                    !dict.lookup(&s).is_empty() || !dict.completions(&s, 1).is_empty()
                })
            };
            parse_rank_locks_keep_digits(raw, &is_code)
        } else {
            parse_rank_locks(raw)
        }
    }

    pub fn new(data_dir: &Path, config: Config) -> std::io::Result<Engine> {
        // 【性能插桩】词典装载分解（与 server 侧 startup-trace 配套）
        let t0 = std::time::Instant::now();
        let mark = |label: &str| {
            use std::io::Write;
            let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(r"C:\ProgramData\HuFu\diag\startup-trace.txt")
            {
                let _ = writeln!(f, "  engine/{label}: {}ms", t0.elapsed().as_millis());
            }
        };
        let dict_root = data_dir.join(&config.schema.dir);
        let current = dict_root.join(&config.schema.current);
        let mut schema = Schema::load(&current)?;
        mark("schema_load");
        let rev_name = config.reverse.table.trim().to_string();
        let rev_dir = schema.dir.clone();
        let mut schemas = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dict_root) {
            for e in rd.flatten() {
                if e.path().is_dir() {
                    if let Some(n) = e.file_name().to_str() {
                        schemas.push(n.to_string());
                    }
                }
            }
        }
        let mut engine = Engine {
            config,
            schema,
            schemas,
            data_dir: data_dir.to_path_buf(),
            sentence: None,
            rerank_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            sound_hint: None,
            last_commit: String::new(),
            opencc: None,
            opencc_emoji: None,
            opencc_loaded: false,
        };
        // 反查表覆盖（config.reverse.table）——【性能】懒加载：只设
        // 路径不读文件（见 Schema::reverse 注释）；与方案内自动探测
        // 同名时天然去重（一个路径只装一次）。
        if !rev_name.is_empty() {
            let p = rev_dir.join(&rev_name);
            if p.exists() {
                engine.schema.reverse = None;
                engine.schema.reverse_path = Some(p);
            }
        }
        mark("reverse+done");
        Ok(engine)
    }

    /// 直接从方案目录构建引擎（CLI / 测试用，无 dictionaries/ 包装）。
    pub fn with_schema_dir(schema_dir: &Path, config: Config) -> std::io::Result<Engine> {
        let mut schema = Schema::load(schema_dir)?;
        let rev_dir = schema.dir.clone();
        let rev_name = config.reverse.table.trim().to_string();
        let name = schema.name.clone();
        let mut engine = Engine {
            config,
            schemas: vec![name],
            data_dir: schema_dir
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf(),
            schema,
            sentence: None,
            rerank_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            sound_hint: None,
            last_commit: String::new(),
            opencc: None,
            opencc_emoji: None,
            opencc_loaded: false,
        };
        if !rev_name.is_empty() {
            let p = rev_dir.join(&rev_name);
            if p.exists() {
                // 【性能】懒加载（同 Engine::new）
                engine.schema.reverse = None;
                engine.schema.reverse_path = Some(p);
            }
        }
        Ok(engine)
    }

    /// 注入整句解码器。
    pub fn set_sentence_decoder(&mut self, dec: Option<Arc<dyn SentenceDecoder>>) {
        self.sentence = dec;
    }

    pub fn sentence_decoder(&self) -> Option<&Arc<dyn SentenceDecoder>> {
        self.sentence.as_ref()
    }

    /// 反查表覆盖：config.reverse.table 指定方案目录内文件名时优先加载
    /// （未指定或加载失败 → 保持按文件名含「反查」的自动探测结果）。
    /// 【性能】懒加载：只换路径，真正装载见 ensure_reverse。
    fn apply_reverse_override(&self, schema: &mut Schema) {
        let name = self.config.reverse.table.trim();
        if name.is_empty() {
            return;
        }
        let p = schema.dir.join(name);
        if p.exists() {
            schema.reverse = None;
            schema.reverse_path = Some(p);
        }
    }

    /// 【性能】反查表按需装载：Schema::load 只记路径（冷启动省 ~700ms
    /// 文本解析），首次进入反查模式或 server 后台预热线程调用本方法
    /// 真正装载。装完置 None 路径防重复。
    pub fn ensure_reverse(&mut self) {
        if self.schema.reverse.is_some() {
            return;
        }
        let Some(p) = self.schema.reverse_path.clone() else {
            return;
        };
        let t0 = std::time::Instant::now();
        match ReverseTable::load(&p) {
            Ok(rt) => {
                eprintln!(
                    "反查表已装载（懒加载 {:.0}ms）: {}",
                    t0.elapsed().as_millis(),
                    p.display()
                );
                self.schema.reverse = Some(rt);
                self.schema.reverse_path = None;
            }
            Err(e) => eprintln!("反查表 {} 装载失败: {e}", p.display()),
        }
    }

    /// 切换方案。同时记录「最近方案对」供 Ctrl+M 往返切换。
    pub fn switch_schema(&mut self, name: &str) -> std::io::Result<()> {
        // 空名防御：dir.join("") = 码表根目录本身，Schema::load 会把
        // 所有方案子目录当一个方案读（实测挂死 30s+，Ctrl+M 卡死根源；
        // recent_pair 污染出空端时 target 即空名）。
        if name.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "empty schema name",
            ));
        }
        let old = self.config.schema.current.clone();
        let dir = self.data_dir.join(&self.config.schema.dir).join(name);
        let mut schema = Schema::load(&dir)?;
        self.apply_reverse_override(&mut schema);
        self.schema = schema;
        self.config.schema.current = name.to_string();
        // 只记录两端皆非空的方案对（启动期 old 为空时不污染——
        // 否则 Ctrl+M 的 target 会解析出空名）
        if old != name && !old.is_empty() {
            self.config.schema.recent_pair = Some((old, name.to_string()));
        }
        Ok(())
    }

    /// 重新加载用户数据（用户词/用户调整），码表主体不重载。
    pub fn reload_user_data(&mut self) {
        let dir = self.schema.dir.clone();
        let uw = dir.join("用户词.txt");
        if let Ok(ud) = hufu_dict::user::UserDict::load(&uw) {
            self.schema.user_dict = ud;
        }
        let adj = dir.join("用户调整.txt");
        if let Ok(a) = hufu_dict::user::UserAdjust::load(&adj) {
            self.schema.adjust = a;
        }
    }

    /// 方案是否启用整句。
    pub fn sentence_active(&self) -> bool {
        if !self.config.sentence.enabled || self.sentence.is_none() {
            return false;
        }
        if self.config.sentence.auto_enable {
            self.schema.name.contains("整句")
        } else {
            true
        }
    }

    /// 处理一次按键。
    pub fn process_key(&mut self, session: &mut Session, key: KeyInput) -> KeyOutcome {
        if !key.is_press {
            return KeyOutcome::passthrough();
        }
        self.sound_hint = None;
        // 神经重排结果已到（停顿期间异步算完）→ 先应用再处理本键，
        // 空格/选重等不改变 raw 的操作即拿到新顺序
        self.apply_rerank(session);
        let mut out = self.process_key_inner(session, key);
        // 提示音标签（前端按数据目录 sounds/<tag>.wav 播放）
        if self.config.sound.enabled && out.consumed {
            let tag = self.sound_hint.unwrap_or(if out.commit.is_some() {
                "commit"
            } else {
                "key"
            });
            out.sound = Some(tag.to_string());
        }
        out
    }

    fn process_key_inner(&mut self, session: &mut Session, key: KeyInput) -> KeyOutcome {
        let m = key.modifiers;
        if m.ctrl && !m.alt {
            if let Some(c) = key.key.as_char() {
                if c == ' ' && self.config.general.ctrl_space_switch {
                    session.chinese = !session.chinese;
                    session.clear();
                    return KeyOutcome::consumed(self.state(session));
                }
                if c == 'm' && !m.shift && self.config.general.switch_recent_schema {
                    // 目标：最近方案对的另一端；从未成对时（recent_pair=None）
                    // 取方案列表中首个非当前方案 —— 保证 Ctrl+M 首次即可用。
                    let target = match self.config.schema.recent_pair.clone() {
                        Some((a, b)) => Some(if self.config.schema.current == a { b } else { a }),
                        None => self
                            .schemas
                            .iter()
                            .find(|s| **s != self.config.schema.current)
                            .cloned(),
                    };
                    // 空名防御：recent_pair 可能被历史 bug 污染出空端
                    // （启动期 current="" 时的切换），空目标直接放弃。
                    if let Some(t) = target.filter(|t| !t.is_empty()) {
                        if t != self.config.schema.current && self.switch_schema(&t).is_ok() {
                            session.clear();
                            return KeyOutcome::consumed(self.state(session));
                        }
                    }
                }
                // Ctrl+Shift+数字：置顶当前页第 N 候选
                if m.shift {
                    if let Some(n) = c.to_digit(10) {
                        let idx = if n == 0 { 9 } else { (n - 1) as usize };
                        return self.op_pin_candidate(session, idx);
                    }
                }
            }
            // Ctrl+Delete：软删当前页首选
            if key.key == KeyCode::Delete && self.config.user.allow_delete_word {
                return self.op_hide_candidate(session, 0);
            }
            return KeyOutcome::passthrough();
        }
        if m.alt || m.meta {
            return KeyOutcome::passthrough();
        }

        // Caps
        if key.key == KeyCode::CapsLock {
            match self.config.general.caps_action {
                hufu_config::CapsAction::Clear => {
                    if !session.raw.is_empty() {
                        session.clear();
                        return KeyOutcome::consumed(self.state(session));
                    }
                }
                hufu_config::CapsAction::Switch => {
                    session.chinese = !session.chinese;
                    session.clear();
                    return KeyOutcome::consumed(self.state(session));
                }
                hufu_config::CapsAction::None => {}
            }
            return KeyOutcome::passthrough();
        }

        // Shift 单击切换中英（有编码时不处理）
        if matches!(key.key, KeyCode::ShiftLeft | KeyCode::ShiftRight)
            && self.config.general.shift_switch
        {
            if session.raw.is_empty() {
                session.chinese = !session.chinese;
                session.pair.reset();
                return KeyOutcome::consumed(self.state(session));
            }
            return KeyOutcome::passthrough();
        }

        match key.key {
            KeyCode::Backspace => self.on_backspace(session),
            KeyCode::Escape => {
                if !session.raw.is_empty() || session.mode != InputMode::Normal {
                    session.clear();
                    KeyOutcome::consumed(self.state(session))
                } else {
                    KeyOutcome::passthrough()
                }
            }
            KeyCode::Enter => {
                if session.raw.is_empty() {
                    return KeyOutcome::passthrough();
                }
                if self.config.input.enter_clear {
                    session.clear();
                    KeyOutcome::consumed(self.state(session))
                } else {
                    let raw = std::mem::take(&mut session.raw);
                    session.candidates.clear();
                    KeyOutcome::commit(raw, self.state(session))
                }
            }
            KeyCode::Tab => {
                if session.raw.is_empty() {
                    return KeyOutcome::passthrough();
                }
                if self.config.input.tab_clear {
                    session.clear();
                    KeyOutcome::consumed(self.state(session))
                } else {
                    self.on_page(session, 1)
                }
            }
            KeyCode::Up => self.on_updown(session, -1),
            KeyCode::Down => self.on_updown(session, 1),
            KeyCode::PageDown => {
                if session.raw.is_empty() {
                    KeyOutcome::passthrough()
                } else {
                    self.on_page(session, 1)
                }
            }
            KeyCode::PageUp => {
                if session.raw.is_empty() {
                    KeyOutcome::passthrough()
                } else {
                    self.on_page(session, -1)
                }
            }
            KeyCode::Char(c) => self.on_char(session, c, m.shift),
            KeyCode::Space => self.on_char(session, ' ', false),
            KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End | KeyCode::Delete => {
                if !session.raw.is_empty() {
                    KeyOutcome::consumed(self.state(session))
                } else {
                    KeyOutcome::passthrough()
                }
            }
            _ => KeyOutcome::passthrough(),
        }
    }

    fn on_backspace(&mut self, session: &mut Session) -> KeyOutcome {
        if session.raw.is_empty() {
            return KeyOutcome::passthrough();
        }
        session.raw.pop();
        // 退格打断提前上屏证据（Rime），但保留已提交前缀
        session.early_history.clear();
        if session.raw.is_empty() {
            session.clear();
        } else {
            self.refresh_candidates(session);
        }
        KeyOutcome::consumed(self.state(session))
    }

    fn on_char(&mut self, session: &mut Session, c: char, shift: bool) -> KeyOutcome {
        if !session.chinese {
            return KeyOutcome::passthrough();
        }

        // 反查模式
        if session.mode == InputMode::Reverse {
            return self.on_reverse_char(session, c);
        }

        // 命令模式
        if session.mode == InputMode::Command {
            return self.on_command_char(session, c);
        }

        if session.raw.is_empty() {
            // 反查引导
            if c == self.config.reverse.prefix && self.config.reverse.enabled {
                session.mode = InputMode::Reverse;
                return KeyOutcome::consumed(self.state(session));
            }
            // 命令命名空间（Shift+\ = ｜ 符号，不进命令——2026-09-06
            // 符号自查：空态 Shift+\ 误入命令模式导致 ｜ 打不出）
            if c == '\\' && !shift {
                session.mode = InputMode::Command;
                session.raw = "\\".into();
                self.refresh_candidates(session);
                return KeyOutcome::consumed(self.state(session));
            }
            // 【Shift 标点 2026-09-05】TSF 传基础键+shift=true（Shift+,→
            // key=","），空态 Shift+标点/数字转 US 键盘 shift 形态字符走
            // 标点映射：shift+,→<→《、shift+'→"→“（智能配对）、shift+/
            // →?→？、shift+1→!→！。放在 '/' 符号命名空间之前（否则
            // shift+/ 被顿号分支先吃）。有编码态不受影响。
            if shift && session.raw.is_empty() {
                if let Some(sf) = shift_form(c) {
                    if let Some((text, back)) = self.punct_output(session, sf) {
                        let mut o = KeyOutcome::commit(text, self.state(session));
                        o.back = back;
                        return o;
                    }
                }
            }
            // '/' 符号命名空间（首选顿号，继续输入进入 /xx 符号）
            if c == '/'
                && (self.config.input.slash_dunhao || self.has_continuation_prefix("/"))
            {
                session.raw.push(c);
                self.refresh_candidates(session);
                return KeyOutcome::consumed(self.state(session));
            }
            // 编码字符
            if self.config.input.is_alphabet_char(c) && !shift {
                session.raw.push(c);
                self.after_append(session);
                return self.take_or_state(session);
            }
            // 大写字母：混合输入缓冲（不限长混输）
            if self.config.input.mixed_input && c.is_ascii_uppercase() {
                session.raw.push(c);
                self.refresh_candidates(session);
                return KeyOutcome::consumed(self.state(session));
            }
            // 空态数字：直通（系统原生半角上屏），但记入跨句尾巴——
            // 数字后的标点半角化（1.5 / 3.14 / 2,500）依赖 tail 判
            // 「上一个已上屏字符是 ASCII 数字」；不记则判不出。
            if c.is_ascii_digit() {
                session.tail_context.push(c);
                return KeyOutcome::passthrough();
            }
            // 标点
            if let Some((text, back)) = self.punct_output(session, c) {
                let mut o = KeyOutcome::commit(text, self.state(session));
                o.back = back;
                return o;
            }
            return KeyOutcome::passthrough();
        }

        // —— 有编码态 ——
        // 「;;」→；直接上屏（; 引导标点）。Shift+; 例外：那是「：」，
        // 落到下面 Shift 形态拦截段处理（或 ; 引导清缓冲后空态输出）。
        if c == ';' && !shift && session.raw == ";" && self.config.input.semicolon_guide {
            session.clear();
            return KeyOutcome::commit("；".to_string(), self.state(session));
        }
        // 其他字符：有快符/符号延续（;x…）则继续组快符，
        // 无延续才打断「;」引导清缓冲重入（空格留给首选上屏）
        if session.raw == ";"
            && self.config.input.semicolon_guide
            && c != ' '
            && !self.has_continuation_prefix(&format!(";{c}"))
        {
            session.clear();
            return self.on_char(session, c, shift);
        }
        // 【翻页/顶字复用键 2026-09-06】-（——）/ =（+）候选可翻时翻页，
        // 翻不动时直接顶屏（首选+符号形态上屏，编码态标点顶字同款语义）。
        // Shift 形态（—— / +）与基键同方向。须在 Shift 标点拦截之前。
        if (c == '-' || c == '=') && !session.candidates.is_empty() {
            let (dir, sym) = if c == '-' {
                if shift {
                    (-1, "——".to_string())
                } else {
                    (-1, "-".to_string())
                }
            } else if shift {
                (1, "+".to_string())
            } else {
                (1, "=".to_string())
            };
            let page_size = self.config.candidates.page_size.max(1);
            let pages = (session.candidates.len() + page_size - 1) / page_size;
            let can = if dir < 0 {
                session.page > 0
            } else {
                session.page + 1 < pages
            };
            if can {
                return self.on_page(session, dir);
            }
            // 不可翻：顶字（当前页首选——翻页后顶的是所在页首字）
            let ps = page_size as usize;
            let idx = (session.page as usize * ps).min(session.candidates.len().saturating_sub(1));
            let first = session.candidates[idx].commit_text().to_string();
            session.clear();
            return KeyOutcome::commit(format!("{first}{sym}"), self.state(session));
        }
        // 【Shift 标点 2026-09-06】有编码态同空态（a18f89c 的空态修复漏了
        // 这条路径）：TSF 传基础键+shift=true，Shift+标点/数字先转 US 键盘
        // shift 形态再走标点映射——打 d 出候选「中」后 Shift+, 应上屏
        // 「中《」而非「中，」。置于选重/翻页/数字选重之前：Shift+1 是
        // 「！」不是数字选重、Shift+; 不是二选键。语义同编码态标点顶字
        //（提交首选后输出标点）。
        if shift {
            if let Some(sf) = shift_form(c) {
                if let Some((text, back)) = self.punct_output(session, sf) {
                    let first = session
                        .candidates
                        .first()
                        .map(|x| x.commit_text().to_string())
                        .unwrap_or_default();
                    session.clear();
                    let mut o = KeyOutcome::commit(format!("{first}{text}"), self.state(session));
                    o.back = back;
                    return o;
                }
            }
        }
        let extends = self.has_continuation_prefix(&format!("{}{c}", session.raw));
        // 选重键（不构成编码延续时才作为选重）
        if !extends {
            if c == self.config.candidates.second_select
                || c == self.config.candidates.third_select
            {
                return self.on_rank_key(session, c);
            }
        }
        // 编码字符（含死路：交给 after_append 处理顶功/清屏）
        if self.config.input.is_alphabet_char(c) && !shift {
            session.raw.push(c);
            self.after_append(session);
            return self.take_or_state(session);
        }
        // 混合输入：大写继续追加
        if self.config.input.mixed_input && c.is_ascii_uppercase() {
            session.raw.push(c);
            self.refresh_candidates(session);
            return KeyOutcome::consumed(self.state(session));
        }
        // 选重键
        if c == self.config.candidates.second_select || c == self.config.candidates.third_select
        {
            return self.on_rank_key(session, c);
        }
        // 翻页键（其余配置翻页键：保持纯翻页）
        if self.config.candidates.paging_keys.contains(c) && !(c == '-' || c == '=') {
            let idx = self
                .config
                .candidates
                .paging_keys
                .chars()
                .position(|x| x == c)
                .unwrap_or(0);
            let dir = if idx == 0 { -1 } else { 1 };
            return self.on_page(session, dir);
        }
        // 数字选重
        if let Some(n) = c.to_digit(10) {
            let _ = n;
            // 【数字编码 2026-09-05】raw+数字构成码表延续（如 a8=来、
            // u3=的）时数字是编码字符，不当选重——数字做第二码位的
            // 码表体系。无延续（虎码类）保持数字选重不变。
            // 【跨段延续】整句流里数字常与前一段尾字符组成词条
            //（如 比|vv + 如|b8 连打：按到 8 时 raw=vvb，整体 vvb8
            // 无词条，但后缀 b8=如 是词条）——数字对 raw 的任意
            // 后缀构成词条/前缀即当编码字符，交给整句解码切分。
            let digit_extends = extends || {
                let raw = &session.raw;
                (1..=raw.chars().count()).any(|k| {
                    let start = raw.chars().count() - k;
                    let suffix: String = raw.chars().skip(start).collect();
                    let probe = format!("{}{c}", suffix);
                    !self.schema.dict.lookup(&probe).is_empty()
                        || !self.schema.dict.completions(&probe, 1).is_empty()
                })
            };
            if digit_extends {
                session.raw.push(c);
                self.after_append(session);
                return self.take_or_state(session);
            }
            return self.on_rank_key(session, c);
        }
        // 空格首选
        if c == ' ' {
            return self.select_first(session);
        }
        // 编码态标点：顶字（提交首选后输出标点）
        if let Some((punct, back)) = self.punct_output(session, c) {
            if !session.candidates.is_empty() {
                let first = session.candidates[0].commit_text().to_string();
                session.clear();
                let mut o = KeyOutcome::commit(format!("{first}{punct}"), self.state(session));
                o.back = back;
                return o;
            }
            session.clear();
            let mut o = KeyOutcome::commit(punct, self.state(session));
            o.back = back;
            return o;
        }
        if c.is_ascii_alphanumeric() {
            return KeyOutcome::consumed(self.state(session));
        }
        KeyOutcome::passthrough()
    }

    /// 标点输出（全角/半角/引号配对/数字后点半角化）。
    /// 返回 (文本, 提交前回删数)——back>0 用于「1.」再按 . 替换为「。」。
    fn punct_output(&mut self, session: &mut Session, c: char) -> Option<(String, u8)> {
        if !c.is_ascii_punctuation() {
            return None;
        }
        if self.config.input.ascii_punct {
            return Some((c.to_string(), 0));
        }
        // 数字后的标点半角化（对齐 Rime/虎爪）：已上屏尾是 ASCII 数字时，
        // . 与 , 直通半角（1.5 / 3.14 / 2,500）；尾恰是刚直通的半角 . 时
        // 再按 . → 回删替换为全角句号（「1.」后想打中文句号的通道）。
        let tail_last = session.tail_context.chars().last();
        if c == '.' || c == ',' {
            if tail_last == Some('.') && c == '.' {
                // 上一键刚直通半角点：本键语义为中文句号，回删替换
                return Some(("。".into(), 1));
            }
            if let Some(t) = tail_last {
                if t.is_ascii_digit() {
                    return Some((c.to_string(), 0));
                }
            }
        }
        if self.config.punct.pair_brackets {
            if let Some(q) = session.pair.quote(c) {
                return Some((q.to_string(), 0));
            }
        } else if c == '\'' || c == '"' {
            return Some((c.to_string(), 0));
        }
        punct::to_full_width_punct(c).map(|s| (s, 0))
    }

    /// 编码追加后的顶功 / 自动上屏 / 快符唯一上屏判定。
    fn after_append(&mut self, session: &mut Session) {
        let prev_cands = session.candidates.clone(); // 追加前 raw 的候选
        let c = session.raw.chars().last().unwrap_or(' ');
        self.refresh_candidates(session);
        let raw = session.raw.clone();
        let len = raw.chars().count();
        let max_len = self.config.input.max_code_length;
        let has_upper = raw.chars().any(|x| x.is_ascii_uppercase());

        // 快符 / 符号：唯一候选立即上屏（auto_select_pattern ^;\w+ 语义，至少两码）
        if (raw.starts_with(';') || raw.starts_with('/'))
            && len >= 2
            && session.candidates.len() == 1
        {
            self.commit_first_inline(session);
            return;
        }

        // 满码唯一上屏
        if len == max_len
            && self.config.input.auto_select_unique
            && session.candidates.len() == 1
        {
            self.commit_first_inline(session);
            return;
        }

        // 整句方案：超过最大码长后由整句解码器接管（不顶功、不清屏）；
        // 整句模式下死路同样不顶屏——编码留在缓冲区交给解码器组句
        let sentence_mode = self.sentence_active();
        let sentence_takeover = sentence_mode && len > max_len;

        // 顶功：仅超长顶屏（第 max+1 键，即最大码长 4 时的第 5 键）。
        // 【语义定版】死路（新码无延续）不顶——此前实现 3 码全码字后
        // 接死路键也自动上屏（如 kog+x 直接顶「涅」），与「打第五码才
        // 上屏」的顶功定义不符（用户实测拍板）。死路走下方空码清屏
        // 分支（auto_clear_empty 开则清缓冲重打，关则留空码由退格/空格
        // 处理）。
        let dead_end = session.candidates.is_empty() && !self.has_continuation(&raw);
        let over_length = len > max_len;
        if over_length && !sentence_mode && self.config.input.auto_push && !has_upper
        {
            if let Some(first) = prev_cands.first().cloned() {
                // 提交追加前 raw 的首选，新 raw 从刚输入的字符重新开始
                self.learn(&first);
                session.clear();
                session.raw = c.to_string();
                self.refresh_candidates(session);
                session.pending_commit = Some(first.commit_text().to_string());
                return;
            }
            // 追加前也无候选：空码处理
            if dead_end && self.config.input.auto_clear_empty {
                session.clear();
            }
            return;
        }

        // 空码自动清屏（既无精确也无前缀，且未开启顶功短路；整句模式保留缓冲）
        if dead_end && !sentence_mode && self.config.input.auto_clear_empty && !has_upper {
            session.clear();
        }

        // 提前上屏：整句接管/带锁/已有前缀时逐键评估（Rime 在 push_input 后立即评估）
        if sentence_takeover || self.parsed_has_locks(session) || !session.committed_raw.is_empty() {
            self.try_early_commit(session);
            if session.pending_commit.is_some() {
                // 前缀已上屏、raw 缩为剩余 → 重刷候选
                self.refresh_candidates(session);
            }
        }
    }

    /// raw 是否带选重锁。
    fn parsed_has_locks(&self, session: &Session) -> bool {
        self.parse_locks(&session.raw).has_locks()
    }

    /// 选重键分流（;/'/数字）。
    /// 整句模式：写入编码选重——后缀进 raw 锁定该段解释，继续组句不上屏
    /// （TigerClaw/Rime 语义，提前上屏规则另行接管）。
    /// 非整句：立即选重上屏。
    fn on_rank_key(&mut self, session: &mut Session, c: char) -> KeyOutcome {
        if self.sentence_active()
            && session.mode == InputMode::Normal
            && !session.raw.is_empty()
            && !session.raw.starts_with([';', '/', '\\'])
            && session.raw.chars().all(|x| {
                x.is_ascii_lowercase() || matches!(x, ';' | '\'' | '0'..='9')
            })
        {
            self.sound_hint = Some("select");
            // 名次基准 = 候选框显示序（置顶/用户词参与排序）。
            // 目标块 = 当前编码的尾部最长有效码（;只锁它前面的那个词段，
            // 如 syftuuu; 的块是 uu 而非整个字母流）。
            // 解码器名次 = 码表原序 → 换算后写入，保证锁到用户看到的那个词。
            let disp_rank: usize = match c {
                x if x == self.config.candidates.second_select => 2,
                x if x == self.config.candidates.third_select => 3,
                x => {
                    let n = x.to_digit(10).unwrap_or(1);
                    if n == 0 {
                        10
                    } else {
                        n as usize
                    }
                }
            };
            let base_now = self.parse_locks(&session.raw).base;
            let chunk: Option<String> = (1..=4usize).rev().find_map(|len| {
                if base_now.chars().count() < len {
                    return None;
                }
                let cand: String = base_now
                    .chars()
                    .skip(base_now.chars().count() - len)
                    .collect();
                if cand.chars().all(|k| k.is_ascii_lowercase())
                    && self.schema.candidates(&cand).len() >= disp_rank
                {
                    Some(cand)
                } else {
                    None
                }
            });
            // 锁只认码表位次。用户词（/jc 加的）不在码表原序里——换算
            // 不到锁位次时，不能把原键字符塞进 raw（ae+3 会变成死码
            // ae3，被解码器再当「码表第 3 名」锁，2026-09-06 用户实测
            // 出乛）。单段纯净态（无前段无锁）直接提交该词；流中场景
            // 维持原 fallback。
            let picked: Option<String> = chunk
                .as_ref()
                .and_then(|ch| {
                    self.schema
                        .candidates(ch)
                        .get(disp_rank - 1)
                        .map(|p| p.text.clone())
                });
            let suffix = chunk
                .and_then(|ch| {
                    let pk = picked.clone()?;
                    self.schema
                        .dict
                        .lookup(&ch)
                        .into_iter()
                        .position(|e| e.text == pk)
                        .map(|i| i + 1)
                })
                .and_then(|file_rank| {
                    if (2..=10).contains(&file_rank) {
                        Some(if file_rank == 10 {
                            '0'
                        } else {
                            char::from_digit(file_rank as u32, 10).unwrap()
                        })
                    } else {
                        None
                    }
                });
            let had_locks = self.parsed_has_locks(session);
            if suffix.is_none() {
                if let Some(pk) = picked {
                    if session.committed_raw.is_empty() {
                        // 单段纯净态或已有锁的改选：选中即上屏（用户词/
                        // 置顶项无锁位次；2026-09-06 锁态改选用户词也直接
                        // 上屏，不再把原键字符塞进 raw 成死码）
                        session.clear();
                        return KeyOutcome::commit(pk, self.state(session));
                    }
                }
            }
            // 【锁态重锁 2026-09-06】raw 已带锁时按数字 = 改锁（去掉旧锁
            // 换新名次），不追加——原实现连按 4567890 会堆出 ae3465678
            // 死码串（换算后的码表名次逐个追加，候选恒为首个锁产物）。
            // suffix 算不出（picked 超候选）时锁态忽略本键。
            if had_locks {
                if let Some(sf) = suffix {
                    let keep = base_now.chars().count();
                    session.raw.truncate(keep);
                    session.raw.push(sf);
                    self.refresh_candidates(session);
                    self.try_early_commit(session);
                    if session.pending_commit.is_some() {
                        self.refresh_candidates(session);
                    }
                    return self.take_or_state(session);
                }
                return KeyOutcome::consumed(self.state(session));
            }
            session.raw.push(suffix.unwrap_or(c));
            self.refresh_candidates(session);
            // 选重后缀也是一「键」：评估提前上屏（Rime 在 push_input 后统一评估）
            if self.parsed_has_locks(session) || !session.committed_raw.is_empty() {
                self.try_early_commit(session);
                if session.pending_commit.is_some() {
                    self.refresh_candidates(session);
                }
            }
            return self.take_or_state(session);
        }
        // 非整句：按名次立即选重上屏
        let idx = match c {
            x if x == self.config.candidates.second_select => 1,
            x if x == self.config.candidates.third_select => 2,
            x => {
                let n = x.to_digit(10).unwrap_or(1);
                if n == 0 {
                    9
                } else {
                    (n - 1) as usize
                }
            }
        };
        self.select_candidate(session, idx)
    }

    /// raw 是否还有编码延续（前缀树或符号表）。
    fn has_continuation(&self, raw: &str) -> bool {
        if !self.schema.dict.completions(raw, 1).is_empty() {
            return true;
        }
        if raw.starts_with(';') || raw.starts_with('/') {
            let map = self.schema.symbols.merge_code_map();
            return map.keys().any(|k| k.starts_with(raw));
        }
        false
    }

    /// 某串是否为某编码（或符号码）的前缀。
    fn has_continuation_prefix(&self, s: &str) -> bool {
        if !self.schema.dict.completions(s, 1).is_empty() {
            return true;
        }
        if s.starts_with(';') || s.starts_with('/') {
            let map = self.schema.symbols.merge_code_map();
            return map.keys().any(|k| k.starts_with(s) || k == s);
        }
        false
    }

    /// 【满码判定】编码是否存在严格更长的码表条目。注意 completions
    /// 含自身条目——必须过滤长度；结果以 code 为前缀、长度 ≥ code、
    /// 排序后自身居首，limit=2 足够抓到「更长」。
    fn has_longer_code(&self, code: &str) -> bool {
        self.schema
            .dict
            .completions(code, 2)
            .iter()
            .any(|e| e.code.chars().count() > code.chars().count())
    }

    /// 内联提交首选（顶功 / 唯一上屏）：置 pending_commit，由 take_or_state 消费。
    fn commit_first_inline(&mut self, session: &mut Session) {
        if session.candidates.is_empty() {
            return;
        }
        let first = session.candidates[0].clone();
        if !first.text.starts_with('{') {
            self.learn(&first);
        }
        let mut text = first.commit_text().to_string();
        if text.starts_with('{') {
            text = self.resolve_dynamic(&text);
        }
        session.clear();
        session.pending_commit = Some(text);
    }

    /// 提前上屏（Rime try_early_commit 逐行移植）：
    /// 置信前缀提案 + 3 键证据史公共前缀 → 增量上屏，编码留在上下文继续组句。
    fn try_early_commit(&mut self, session: &mut Session) {
        if !self.config.sentence.early_commit || session.early_suspended {
            session.early_history.clear();
            session.line_end_hint = false;
            return;
        }
        // 行尾瞬态（单键有效）：确认键数 2→1，组段早一步缩短
        let line_end = std::mem::take(&mut session.line_end_hint);
        let live = session.raw.clone();
        if live.chars().count() + session.committed_raw.chars().count() <= 3 {
            session.early_history.clear();
            return;
        }
        let dec = match &self.sentence {
            Some(d) => d.clone(),
            None => return,
        };
        let full = format!("{}{}", session.committed_raw, live);
        let dec = dec.decode_rich(&full);
        // 不完全尾优先作置信源（Rime early_commit_uses_incomplete_tail）
        let (src, truncated) = if !dec.early_hits.is_empty() {
            (&dec.early_hits[..], dec.early_truncated)
        } else {
            (&dec.hits[..], dec.truncated)
        };
        if truncated {
            session.early_history.clear();
            return;
        }
        let committed_text = session.committed_text.clone();
        let cands: Vec<&SentenceHit> = src
            .iter()
            .filter(|h| committed_text.is_empty() || h.text.starts_with(&committed_text))
            .collect();
        if cands.is_empty() {
            session.early_history.clear();
            return;
        }

        let (proposal, proposal_share) =
            confidence_proposal(&cands, self.config.sentence.weights.confidence);
        if proposal.is_empty()
            || proposal.chars().count() <= committed_text.chars().count()
        {
            session.early_history.clear();
            return;
        }

        // 证据史：须逐键延伸（full = 上一证据 + 1 字符）
        let extends = match session.early_history.last() {
            Some(e) => {
                full.chars().count() == e.full_raw.chars().count() + 1
                    && full.starts_with(&e.full_raw)
            }
            None => false,
        };
        if !extends {
            session.early_history.clear();
        }
        let raw_lengths = {
            let dict = &self.schema.dict;
            let is_code = |p: &str| {
                let cs: Vec<char> = p.chars().collect();
                (1..=cs.len()).any(|j| {
                    let s: String = cs[cs.len() - j..].iter().collect();
                    !dict.lookup(&s).is_empty() || !dict.completions(&s, 1).is_empty()
                })
            };
            build_raw_lengths(&cands, &full, dict.digit_coded, &is_code)
        };
        session.early_history.push(EarlyHistory {
            proposal: proposal.clone(),
            full_raw: full.clone(),
            raw_lengths,
            strong: proposal_share >= 0.999, // Rime STRONG_SHARE（0.9999→0.99999 曾反向调严；
            // 实测 v5 下提前上屏偏保守，回 0.999 提高积极性）
        });
        while session.early_history.len() > 3 {
            session.early_history.remove(0);
        }

        // 观察窗口按证据强度自适应：强证据 2 键确认；普通证据 2 键
        // （原 3 键双保险——实测 v5 下偏保守，统一 2 键提高积极性）。
        // 行尾（组段逼近窗口右缘）1 键即确认——commit 的仍是同一置信
        // 前缀（confidence 软最大占比 ≥0.99 不变），只是早一键落地，
        // 让组段尽快缩回一行内。
        // 【证据窗参数】HUFU_EARLY_NEED（默认 2）：确认上屏所需证据键
        // 数。3 = 更保守（第三键仍同提案才落地）——上屏更晚更少、残
        // 留码更长（2026-09-05 档位实验）。行尾仍 1 键。
        static NEED_K: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let need_k = *NEED_K.get_or_init(|| {
            std::env::var("HUFU_EARLY_NEED")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(2)
        });
        let need = if line_end { 1 } else { need_k };
        if session.early_history.len() < need {
            return;
        }

        // 3 键公共前缀 + 一致的消耗长度
        let mut stable = common_history_prefix(&session.early_history);
        let mut consumed = stable_history_raw_length(&session.early_history, &stable);
        while stable.chars().count() > committed_text.chars().count() && consumed == 0 {
            stable = stable.chars().take(stable.chars().count() - 1).collect();
            consumed = stable_history_raw_length(&session.early_history, &stable);
        }
        if consumed == 0 {
            return;
        }
        let committed_raw_len = session.committed_raw.chars().count();
        if consumed <= committed_raw_len || consumed > full.chars().count() {
            return;
        }
        let mut delta: String = stable.chars().skip(committed_text.chars().count()).collect();
        if delta.chars().count() < 1 || live.chars().count() < 2 {
            return;
        }
        // 【残码门槛实验】HUFU_EARLY_MIN_RESID：上屏后 raw 剩余键数
        // （live - consumed）低于该值时——不是丢弃上屏机会（那样次数
        // 腰斩，2026-09-05 用户实测 3.5→4.0 档 6.44→3.52 次/句），而是
        // 截短上屏前缀（尾字留在缓冲继续攒，剩余≥门槛），上屏照常发
        // 生、每次少上几字。未设置或 0 = 无门槛（当前行为）。
        static MIN_RESID: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
        let min_resid = *MIN_RESID.get_or_init(|| {
            std::env::var("HUFU_EARLY_MIN_RESID")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0)
        });
        if min_resid > 0.0 {
            let live_len = live.chars().count();
            let floor = min_resid as usize;
            while (live_len.saturating_sub(consumed)) < floor
                && stable.chars().count() > committed_text.chars().count() + 1
            {
                stable = stable.chars().take(stable.chars().count() - 1).collect();
                consumed = stable_history_raw_length(&session.early_history, &stable);
                if consumed == 0 || consumed <= committed_raw_len {
                    break;
                }
            }
            if consumed == 0 || consumed <= committed_raw_len {
                return;
            }
            delta = stable.chars().skip(committed_text.chars().count()).collect();
            if delta.chars().count() < 1 {
                return;
            }
        }

        // 提交：committed 前缀增长，live raw 缩为剩余
        session.committed_text = stable;
        let full_chars: Vec<char> = full.chars().collect();
        session.committed_raw = full_chars[..consumed].iter().collect();
        session.raw = full_chars[consumed..].iter().collect();
        session.early_history.clear();
        session.pending_commit = Some(delta);
    }

    /// refresh_candidates 的无早屏递归体。
    fn refresh_candidates_inner(&mut self, session: &mut Session) {
        let entries = self.schema.candidates(&session.raw);
        if !entries.is_empty() {
            session.candidates = entries.iter().map(|e| self.entry_to_candidate(e)).collect();
            return;
        }
        let map = self.schema.symbols.merge_code_map();
        if let Some(list) = map.get(&session.raw) {
            session.candidates = list
                .iter()
                .map(|s| {
                    let mut c =
                        Candidate::new(s.text.clone(), s.code.clone(), CandidateKind::Symbol);
                    c.weight = s.weight;
                    c
                })
                .collect();
        }
    }

    fn take_or_state(&mut self, session: &mut Session) -> KeyOutcome {
        if let Some(text) = session.pending_commit.take() {
            KeyOutcome::commit(text, self.state(session))
        } else {
            KeyOutcome::consumed(self.state(session))
        }
    }

    /// 空格首选 / 空码处理（↑↓ 移动过则上屏高亮项）。
    fn select_first(&mut self, session: &mut Session) -> KeyOutcome {
        if !session.candidates.is_empty() {
            let idx = session
                .selected
                .min(session.candidates.len().saturating_sub(1));
            return self.select_candidate_abs(session, idx);
        }
        // 空码：大写混合输入直接上屏原串，否则清屏
        if session.raw.chars().any(|c| c.is_ascii_uppercase()) {
            let raw = std::mem::take(&mut session.raw);
            session.clear();
            return KeyOutcome::commit(raw, self.state(session));
        }
        session.clear();
        KeyOutcome::consumed(self.state(session))
    }

    /// 选第 idx 个候选（当前页内，0 起）。
    fn select_candidate(&mut self, session: &mut Session, idx: usize) -> KeyOutcome {
        let page_size = self.config.candidates.page_size.max(1);
        let start = session.page * page_size;
        self.select_candidate_abs(session, start + idx)
    }

    /// 选绝对下标候选。
    fn select_candidate_abs(&mut self, session: &mut Session, idx: usize) -> KeyOutcome {
        let pick = session.candidates.get(idx).cloned();
        if let Some(cand) = pick {
            self.sound_hint = Some("select");
            // 动态/功能词（{日期} 等）不进用户词学习——字面标记不是词
            if !cand.text.starts_with('{') {
                self.learn(&cand);
            }
            let mut text = cand.commit_text().to_string();
            if text.starts_with('{') {
                text = self.resolve_dynamic(&text);
            }
            session.clear();
            return KeyOutcome::commit(text, self.state(session));
        }
        KeyOutcome::consumed(self.state(session))
    }

    /// ↑↓ 高亮移动（Rime 悬浮窗选重）：逐候选移动、跨页跟随；浏览即暂停提前上屏。
    fn on_updown(&mut self, session: &mut Session, dir: i32) -> KeyOutcome {
        if session.raw.is_empty() {
            return KeyOutcome::passthrough();
        }
        // 组句中即使无候选也吞键，避免方向键落入应用移动光标
        if session.candidates.is_empty() {
            return KeyOutcome::consumed(self.state(session));
        }
        // 方向键浏览 = 暂停提前上屏直至整句结束（Rime 语义）
        session.early_suspended = true;
        session.early_history.clear();
        let len = session.candidates.len();
        let cur = session.selected.min(len - 1);
        let next = if dir < 0 {
            cur.saturating_sub(1)
        } else {
            (cur + 1).min(len - 1)
        };
        if next == cur {
            return KeyOutcome::consumed(self.state(session));
        }
        session.selected = next;
        // 跨页跟随高亮
        let page_size = self.config.candidates.page_size.max(1);
        session.page = next / page_size;
        self.sound_hint = Some("page");
        KeyOutcome::consumed(self.state(session))
    }

    fn on_page(&mut self, session: &mut Session, dir: i32) -> KeyOutcome {
        // 用户翻页浏览 = 暂停提前上屏直至整句结束（Rime 语义）
        if dir != 0 {
            session.early_suspended = true;
            session.early_history.clear();
        }
        let page_size = self.config.candidates.page_size.max(1);
        let pages = (session.candidates.len() + page_size - 1) / page_size;
        if pages <= 1 {
            return KeyOutcome::consumed(self.state(session));
        }
        self.sound_hint = Some("page");
        let cur = session.page as i32;
        let next = if dir < 0 {
            if cur == 0 {
                pages - 1
            } else {
                (cur - 1) as usize
            }
        } else {
            ((cur + 1) as usize) % pages
        };
        session.page = next;
        KeyOutcome::consumed(self.state(session))
    }

    /// 反查模式按键。
    fn on_reverse_char(&mut self, session: &mut Session, c: char) -> KeyOutcome {
        if session.raw.is_empty() {
            if c.is_ascii_lowercase() || c == '\'' {
                session.raw.push(c);
                self.refresh_candidates(session);
                return KeyOutcome::consumed(self.state(session));
            }
            if c == ' ' || c == self.config.reverse.prefix {
                return KeyOutcome::consumed(self.state(session));
            }
            session.mode = InputMode::Normal;
            return KeyOutcome::consumed(self.state(session));
        }
        match c {
            ' ' => self.select_first(session),
            _ if c.is_ascii_lowercase() || c == '\'' => {
                session.raw.push(c);
                self.refresh_candidates(session);
                KeyOutcome::consumed(self.state(session))
            }
            // 数字选重 / 翻页（与普通模式一致；反查候选多时翻页查看）
            _ if c.is_ascii_digit() && c != '0' => {
                let page_size = self.config.candidates.page_size.max(1);
                let start = session.page * page_size;
                let pick = session.candidates.get(start + (c as usize - '1' as usize)).cloned();
                match pick {
                    Some(cand) => {
                        let text = cand.commit_text().to_string();
                        session.clear();
                        KeyOutcome::commit(text, self.state(session))
                    }
                    None => KeyOutcome::consumed(self.state(session)),
                }
            }
            _ if self.config.candidates.paging_keys.contains(c) => {
                let dir = if self.config.candidates.paging_keys.find(c)
                    >= Some(self.config.candidates.paging_keys.len() / 2)
                {
                    1
                } else {
                    -1
                };
                self.on_page(session, dir)
            }
            _ => {
                session.clear();
                KeyOutcome::consumed(self.state(session))
            }
        }
    }

    /// `\` 命令模式：动态变量（含 \n数字 → 中文）与工具命令。
    fn on_command_char(&mut self, session: &mut Session, c: char) -> KeyOutcome {
        if c == ' ' || c == '\\' {
            return self.select_first(session);
        }
        // 命令空间收任意非空白字符（calc 表达式符号 / \w 造词中文）；
        // 上限 18 字符
        if !c.is_whitespace() && session.raw.chars().count() < 18 {
            session.raw.push(c);
            self.refresh_candidates(session);
            return KeyOutcome::consumed(self.state(session));
        }
        session.clear();
        KeyOutcome::consumed(self.state(session))
    }

    /// 置顶当前页第 idx 候选（Ctrl+Shift+N / 设置界面）。持久化到用户调整日志。
    pub fn op_pin_candidate(&mut self, session: &mut Session, idx: usize) -> KeyOutcome {
        let page_size = self.config.candidates.page_size.max(1);
        let start = session.page * page_size;
        let pick = session.candidates.get(start + idx).cloned();
        let Some(cand) = pick else {
            return KeyOutcome::consumed(self.state(session));
        };
        self.adjust_pin(&cand.code, &cand.text);
        let keep_raw = session.raw.clone();
        session.clear();
        session.raw = keep_raw;
        self.refresh_candidates(session);
        KeyOutcome::consumed(self.state(session))
    }

    /// 软删当前页第 idx 候选（Ctrl+Delete / 设置界面）。
    pub fn op_hide_candidate(&mut self, session: &mut Session, idx: usize) -> KeyOutcome {
        let page_size = self.config.candidates.page_size.max(1);
        let start = session.page * page_size;
        let pick = session.candidates.get(start + idx).cloned();
        let Some(cand) = pick else {
            return KeyOutcome::consumed(self.state(session));
        };
        self.adjust_hide(&cand.code, &cand.text);
        let keep_raw = session.raw.clone();
        session.clear();
        session.raw = keep_raw;
        self.refresh_candidates(session);
        KeyOutcome::consumed(self.state(session))
    }

    /// 按 code+word 置顶（内存 + 追加日志）。
    pub fn adjust_pin(&mut self, code: &str, word: &str) {
        self.schema.adjust.pin(code, word);
        self.append_adjust_log("{置顶}", code, word);
    }

    /// 按 code+word 软删（内存 + 追加日志）。
    pub fn adjust_hide(&mut self, code: &str, word: &str) {
        self.schema.adjust.remove(code, word);
        self.append_adjust_log("{删除}", code, word);
    }

    /// 追加一行到 用户调整.txt。
    fn append_adjust_log(&self, op: &str, code: &str, word: &str) {
        use std::io::Write;
        let path = self.schema.dir.join("用户调整.txt");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{op}{code}\t{word}");
        }
    }

    /// OpenCC 滤镜：为前若干候选追加繁体/emoji 变体（Rime simplifier 语义）。
    fn apply_opencc(&mut self, session: &mut Session) {
        let cfg = self.config.opencc.clone();
        if !cfg.enabled {
            return;
        }
        if !self.opencc_loaded {
            let dir = self.data_dir.join("转换词典");
            // 本数据集只有台版单字表 STCharacters_Tu（无标准 STCharacters，缺文件自动跳过）
            let t = if cfg.to_traditional {
                hufu_dict::OpenCc::load_dir(
                    &dir,
                    &["STPhrases", "STCharacters", "STCharacters_Tu"],
                )
            } else {
                hufu_dict::OpenCc::load_dir(&dir, &["TSPhrases", "TSCharacters"])
            };
            self.opencc = if t.is_empty() { None } else { Some(t) };
            let em = hufu_dict::OpenCc::load_dir_full(&dir, &["emoji"]);
            self.opencc_emoji = if em.is_empty() { None } else { Some(em) };
            self.opencc_loaded = true;
        }
        let base: Vec<Candidate> = session.candidates.iter().take(3).cloned().collect();
        // 【2026-09-05 定稿】简→繁=替换式（Rime simplifier 语义，用户拍板）：
        // 直接转换候选文本，显示与上屏都是繁体——「打简出繁」；候选顺序
        // 不动：无简繁之分的字（中/你）保持原样原位（「中」仍是首选
        // 「中」，不会冒出「哪個」压顶；此前「置顶追加变体」被用户否决：
        // d 键首选变「哪個」）。emoji 仍为追加变体（原本语义）。
        if cfg.to_traditional {
            if let Some(t) = &self.opencc {
                for cand in session.candidates.iter_mut() {
                    let conv = t.convert(&cand.text);
                    if conv != cand.text {
                        cand.text = conv;
                    }
                }
            }
        }
        let mut variants: Vec<Candidate> = Vec::new();
        for cand in &base {
            if cfg.emoji {
                if let Some(em) = &self.opencc_emoji {
                    let v = em.convert(&cand.text);
                    if v != cand.text && v.chars().count() > cand.text.chars().count() {
                        let mut c = cand.clone();
                        c.text = v;
                        c.comment = "😊".into();
                        variants.push(c);
                    }
                }
            }
        }
        let n = variants.len();
        session.candidates.append(&mut variants);
        let _ = n;
    }

    /// 用户学习：自动调频 + 可选调整日志（user-adjust.log，log_adjust=true 时记录）。
    /// 提交收口的动态/功能词解析：`{日期}`族 → 实时展开；`{重复上屏}`
    /// → last_commit（进程级，server 侧每键收口更新）；`{加词}`/
    /// `{隐藏候选}` → 原样透传（DLL 拦截处理）；未知 `{x}` 原样。
    fn resolve_dynamic(&self, text: &str) -> String {
        if !text.starts_with('{') || !text.ends_with('}') || text.len() < 3 {
            return text.to_string();
        }
        let tag = &text[1..text.len() - 1];
        if tag == "重复上屏" {
            return self.last_commit.clone();
        }
        if let Some(v) = dynamic::expand(tag) {
            return v;
        }
        text.to_string()
    }

    fn learn(&mut self, cand: &Candidate) {
        if self.config.user.auto_frequency {
            self.schema.user_dict.add_word(&cand.code, &cand.text);
        }
        if self.config.user.log_adjust {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let line = format!(
                "{secs}\t{}\t{}\t{:?}\n",
                cand.code, cand.text, cand.source
            );
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.data_dir.join("user-adjust.log"))
                .and_then(|mut f| {
                    use std::io::Write;
                    f.write_all(line.as_bytes())
                });
        }
    }

    /// 重建候选列表（含整句模式切换）。
    /// 整句候选显示：全上下文解码（committed ++ live），只显示已提交文本之后的剩余。
    /// 组一个重排请求：(key, 前文, 前 top_k 个整句候选文本)。
    /// 无整句候选或候选不足 2 时返回 None。
    pub fn rerank_request(
        &self,
        session: &Session,
    ) -> Option<(String, String, Vec<String>)> {
        if !self.config.sentence.rerank.enabled {
            return None;
        }
        if session.raw.is_empty() {
            return None;
        }
        let key = format!("{}{}", session.committed_raw, session.raw);
        let top = self.config.sentence.rerank.top_k.max(2);
        let texts: Vec<String> = session
            .candidates
            .iter()
            .filter(|c| c.source == CandidateKind::Sentence && !c.partial)
            .take(top)
            .map(|c| c.text.clone())
            .collect();
        if texts.len() < 2 {
            return None;
        }
        // 语境 = 文章尾巴 + 句内已上屏前缀。句首无语境时填「。」伪句首
        // 语境继续重排（而非跳过）：空上下文裸跑 Qwen 会把 ngram 正确序
        // 翻掉（实测 ueeyiahx 空 ctx 拖乿心 -33.31 > 的窒闷 -34.19），
        // 但 ctx="。" 时模型获得「新句开始」信号，判序大幅正确——
        // 实测 agkadklecbsy：阖口而不言 -28.26 vs 痔问而不言 -40.26
        // （ngram 字模型把高频字病句排成语前面，句首用户实测投诉位）。
        let mut ctx = session.tail_context.clone();
        ctx.push_str(&session.committed_text);
        if ctx.trim().is_empty() {
            ctx = "。".to_string();
        }
        Some((key, ctx, texts))
    }

    /// 轮询读入口：先应用已到达的重排缓存（无按键副作用）再取 state——
    /// 候选窗停顿期轮询由此立即看到换序后的新首选。
    pub fn refresh_rerank(&self, session: &mut Session) {
        self.apply_rerank(session);
    }

    /// 应用神经重排缓存：同 key 时按缓存顺序重排 Sentence 类候选（稳定，缺席者保持相对顺序靠后）。
    fn apply_rerank(&self, session: &mut Session) {        if session.candidates.is_empty() {
            return;
        }
        let cache = match self.rerank_cache.lock() {
            Ok(c) => c,
            Err(_) => return,
        };
        if cache.is_empty() {
            return;
        }
        let key = format!("{}{}", session.committed_raw, session.raw);
        let Some(order) = cache.get(&key) else {
            return;
        };
        let rank: std::collections::HashMap<&String, usize> =
            order.iter().enumerate().map(|(i, t)| (t, i)).collect();
        // 只重排 Sentence 类子序列，其他类候选位置不动
        let mut idxs: Vec<usize> = Vec::new();
        for (i, c) in session.candidates.iter().enumerate() {
            if c.source == CandidateKind::Sentence {
                idxs.push(i);
            }
        }
        if idxs.len() < 2 {
            return;
        }
        // 【选重深度约束】（2026-09-05 用户实测 舒服/坏人/势力 三案例）：
        // 无锁时候选各段码表名次和（sum_rank）是「用户付出的选重代价」
        // ——纯字母码的经济契约是「每段取码表首选」（ja=们，舒 是 ja2，
        // 要打「舒服」规范打法 ja2vz）。rerank 换序不得把深度超过原
        // 首选的候选提到前面——Qwen 觉得「舒服/坏人/势力」是高频词也
        // 不行：那是在替用户付出他没同意的选重代价。
        let depth_map: std::collections::HashMap<String, usize> = self
            .sentence
            .as_ref()
            .map(|dec| {
                dec.decode_rich(&key)
                    .hits
                    .iter()
                    .map(|h| (h.text.clone(), h.sum_rank))
                    .collect()
            })
            .unwrap_or_default();
        let base_depth = depth_map.get(&session.candidates[idxs[0]].text).copied();
        let mut sorted: Vec<usize> = idxs.clone();
        // 【过程态硬防御】partial（未消耗全部 raw 的前缀形态）恒沉底：
        // 即使旧缓存（过滤前产生的 Qwen 顺序）含过程态文本也不得提前
        // ——缺席的完整态次沉，完整态间按 Qwen 顺序自由重排。
        // 【深度约束】完整态中 sum_rank 超过原首位深度者沉到缺席者
        // 之前（仍在候选可见，只是不占前）。无深度数据（decode 未返）
        // 时不约束。
        sorted.sort_by_key(|&i| {
            let c = &session.candidates[i];
            if c.partial {
                usize::MAX
            } else if let (Some(base), Some(&d)) = (base_depth, depth_map.get(&c.text)) {
                if d > base {
                    usize::MAX - 1
                } else {
                    rank.get(&c.text).copied().unwrap_or(usize::MAX - 2)
                }
            } else {
                rank.get(&c.text).copied().unwrap_or(usize::MAX - 2)
            }
        });
        if sorted == idxs {
            return;
        }
        let subs: Vec<Candidate> = sorted
            .iter()
            .map(|&i| session.candidates[i].clone())
            .collect();
        for (slot, cand) in idxs.into_iter().zip(subs) {
            session.candidates[slot] = cand;
        }
    }

    fn sentence_candidates(
        &self,
        session: &Session,
        dec: &dyn SentenceDecoder,
    ) -> Vec<Candidate> {
        let full = format!("{}{}", session.committed_raw, session.raw);
        let rich = dec.decode_rich(&full);
        let committed_text = session.committed_text.clone();
        let skip = committed_text.chars().count();
        // 【进行态（partial）不进候选】（2026-09-05 定版）：partial=前段
        // 精确+尾键进行中（消耗不满 raw），无论码表域还是整句域都不显
        // 示——用户规则：候选=与实打编码精确对应的完成态组合。历轮案
        // 例：倩（码 jav）在 javz 时刻、uaegq 时刻的「打干」「打干都」
        //（含一简尾段）、pvlc 时刻的「踹」——进行态一律退出候选框；
        // 要上屏走顶功/继续打完整码。2026-09-03「它存 wvn 提示它」的
        // 中间态并入已被此规则取代（3943a9b 踹修复同方向收口）。
        let has_locks = self.parse_locks(&session.raw).has_locks();
        let mut cands: Vec<Candidate> = Vec::new();
        // 【无锁短码候选=精确对应】（2026-09-05 用户规则）：live raw ≤
        // 最大码长且无锁=码表域——候选只留每段精确对应实打编码的完整
        // 态（段词条码表码长==消耗键数、名次 1）：javz 只出「们服」与
        // 整码字，「舒服」(ja2 的 2 没打)「们改变」(javz; 的 ; 没打)
        // 不出——打 javz;（5 键进整句域）或 ja2vz（锁域）时才出现。
        // exact 集为空时回退全显（不破坏可见性，如全生僻码）。
        let live_len = session.raw.chars().count();
        let dict_domain =
            !has_locks && live_len > 0 && live_len <= self.config.input.max_code_length;
        let mut exact_cands: Vec<Candidate> = Vec::new();
        let mut inexact_cands: Vec<Candidate> = Vec::new();
        for h in rich.hits.iter() {
            if !committed_text.is_empty() && !h.text.starts_with(&committed_text) {
                continue;
            }
            let text: String = h
                .text
                .chars()
                .skip(committed_text.chars().count())
                .collect();
            if text.is_empty() {
                continue;
            }
            let mut c = Candidate::new(text, session.raw.clone(), CandidateKind::Sentence);
            c.weight = h.score;
            if h.exact {
                exact_cands.push(c);
            } else {
                inexact_cands.push(c);
            }
        }
        if dict_domain && !exact_cands.is_empty() {
            // 码表域：只显精确对应项；exact 全空（无精确组合，如全生
            // 僻码 wvn）回退全显保持可见性
            cands = exact_cands;
        } else {
            cands = exact_cands;
            cands.extend(inexact_cands);
        }
        // 完整态构成整个候选列表（partial 已全局退出，见上）。
        cands
    }

    fn refresh_candidates(&mut self, session: &mut Session) {
        session.candidates.clear();
        session.page = 0;
        session.selected = 0;
        if session.raw.is_empty() {
            return;
        }
        // 命令模式：动态变量候选
        if session.mode == InputMode::Command {
            session.candidates = self.command_candidates(&session.raw);
            return;
        }
        // 反查模式（懒装载：首次使用或后台预热时装表）
        if session.mode == InputMode::Reverse {
            self.ensure_reverse();
            session.candidates = self.reverse_candidates(&session.raw);
            return;
        }

        // 「;」引导标点候选：;+空格=：、;;=；直上
        if self.config.input.semicolon_guide
            && session.mode == InputMode::Normal
            && session.raw == ";"
        {
            session.candidates = vec![
                Candidate::new("：".to_string(), ";".to_string(), CandidateKind::Symbol),
                Candidate::new("；".to_string(), ";".to_string(), CandidateKind::Symbol),
            ];
            return;
        }

        let parsed = self.parse_locks(&session.raw);
        let raw_len = session.raw.chars().count();
        // 整句接管：超长、带选重锁（≤4 码 + 锁时也组句），或已有提前上屏前缀
        let sentence_mode = self.sentence_active()
            && (raw_len > self.config.input.max_code_length
                || parsed.has_locks()
                || !session.committed_raw.is_empty());
        if sentence_mode {
            if let Some(dec) = &self.sentence {
                let cands = self.sentence_candidates(session, dec.as_ref());
                if !cands.is_empty() {
                    session.candidates = cands;
                    self.apply_rerank(session);
                    return;
                }
                // 解码器无产物（如锁无法匹配任何段）：回退常规路径
            }
        }

        // 常规：精确码候选 + 整句短语合并
        // （Rime 菜单合并语义：ngram 多字首选短语压生僻表项置前，
        //   mlwe → 两次 先于 𠓅/𰧓，与 Rime 实测同拍；其余句子候选去重补后）
        let entries = self.schema.candidates(&session.raw);
        if !entries.is_empty() {
            // 【中间态候选 2026-09-03】录入中途且码表精确候选被生僻字
            // 霸屏时（打「它存」到 wvn：码表只有 徴/𡦺）：early 前缀态
            //（正在打的词，如「它」）压最前、生僻码表候选下沉。
            // 码表首选非生僻时（srsr→常常）**不动序**——码表精确首选
            // 语义保持（虎爪回归案例 zhh→虎），early 也不压前。
            let rare_rescue = !parsed.has_locks()
                && self.has_continuation(&session.raw)
                && self
                    .sentence
                    .as_ref()
                    .map(|dec| {
                        entries.iter().any(|e| e.text.chars().any(|ch| dec.rare_hint(ch)))
                    })
                    .unwrap_or(false);
            if rare_rescue {
                let dec = self.sentence.as_ref().unwrap();
                // 【前缀态不进候选】（踹修复链，虎爪/Rime 对齐）：打出
                // 延伸键（pvl 的 l、dzht 的 t）即表达「要延伸」，前码字
                //（「起」pv、「唬」dzh）不再出现在候选里——虎爪/Rime 同
                // 款语义。前码字要上屏该在打它自己的码时空格（pv+空格
                // =起），打了延伸键再要前码字=退格。前缀态候选是历代
                // 霸首 bug 的种子（partial 提权/沉底防御复杂度全由此生）。
                let mut normal = Vec::new();
                let mut rare = Vec::new();
                // 【精确码不下沉】（踹修复）：码表精确候选若正是当前
                // raw 的精确条目（打出的码有精确匹配=用户明确意图，如
                // dzht=嘶、pvl=踹——即使存在更长码 pvlc/pvle，打在这个
                // 码上就是要这个字，继续打才延伸），不判生僻下沉——否则
                // 2/3 键前缀态（「唬」dzh、「起」pv）经 early 压前霸占首
                // 位，空格上屏上错字。未打到的更长码的生僻候选维持下沉
                //（码表全生僻时 wvn=[徴,𡦺] 照常全上）。
                let raw_exact = !self.schema.dict.lookup(&session.raw).is_empty();
                for e in &entries {
                    let c = self.entry_to_candidate(e);
                    if !raw_exact && e.text.chars().any(|ch| dec.rare_hint(ch)) {
                        rare.push(c);
                    } else {
                        normal.push(c);
                    }
                }
                session.candidates = normal;
                session.candidates.extend(rare);
            } else {
                session.candidates =
                    entries.iter().map(|e| self.entry_to_candidate(e)).collect();
            }
            self.apply_opencc(session);
            if !sentence_mode && self.sentence_active() {
                if let Some(dec) = &self.sentence {
                    let full = format!("{}{}", session.committed_raw, session.raw);
                    let d = dec.decode_rich(&full);
                    let cmt = session.committed_text.clone();
                    let skip = cmt.chars().count();
                    let mut phrase: Vec<Candidate> = Vec::new();
                    let mut rest: Vec<Candidate> = Vec::new();
                    for h in d.hits.iter() {
                        if !cmt.is_empty() && !h.text.starts_with(&cmt) {
                            continue;
                        }
                        let text: String = h.text.chars().skip(skip).collect();
                        if text.is_empty() {
                            continue;
                        }
                        // 【码表域 exact 过滤】（2026-09-05 uksl 案例）：此
                        // 常规路径（码表有精确条目，如 uksl=抛）此前没有
                        // sentence_candidates 的过滤，势力（uk2+sl）热发
                        // 生 类从 rest 漏进候选。码表域（无锁且 ≤ 最大码
                        // 长）短语合并同样只收精确对应项。
                        let live_n = session.raw.chars().count();
                        let dom = !parsed.has_locks()
                            && live_n > 0
                            && live_n <= self.config.input.max_code_length;
                        if dom && !h.exact {
                            continue;
                        }
                        let multi = text.chars().count() >= 2;
                        let mut cand =
                            Candidate::new(text, session.raw.clone(), CandidateKind::Sentence);
                        cand.weight = h.score;
                        // 短语压前须过「真词地板」：强短语（真词，如 两次
                        // mlwe≈-7.7 / 真好 nqbh≈-8.1）仍压生僻表项置前
                        //（与 Rime 同拍）；弱切分产物（如 ennw 解出「午王」
                        // ≈-21.5，非词）不再压词典精确匹配——「框」应居首
                        //（虎爪/Rime 实测均首选）。弱项回落到词典之后补位。
                        if h.max_rank == 1 && multi && h.score > SENT_PHRASE_FRONT_FLOOR {
                            phrase.push(cand);
                        } else {
                            rest.push(cand);
                        }
                    }
                    if !phrase.is_empty() || !rest.is_empty() {
                        // session.candidates 已含（码表 normal + early 前缀
                        // 态 + 生僻沉底）——phrase 真词压最前，其余原序补后
                        let mut merged = phrase;
                        for existing in session.candidates.drain(..) {
                            if !merged.iter().any(|m| m.text == existing.text) {
                                merged.push(existing);
                            }
                        }
                        for r in rest {
                            if !merged.iter().any(|m| m.text == r.text) {
                                merged.push(r);
                            }
                        }
                        merged.truncate(20);
                        session.candidates = merged;
                        self.apply_rerank(session);
                    }
                }
            }
            return;
        }
        // 词表无此码（如 nqbh 真好）：整句解码器现切（Rime lua_translator 同源行为，
        // n≤4 全名次参与，nq|bh 两段即出「真好」）
        if self.sentence_active() {
            if let Some(dec) = &self.sentence {
                let cands = self.sentence_candidates(session, dec.as_ref());
                if !cands.is_empty() {
                    session.candidates = cands;
                    self.apply_rerank(session);
                    return;
                }
            }
        }
        // 符号表回退
        let map = self.schema.symbols.merge_code_map();
        if let Some(list) = map.get(&session.raw) {
            session.candidates = list
                .iter()
                .map(|s| {
                    let mut c =
                        Candidate::new(s.text.clone(), s.code.clone(), CandidateKind::Symbol);
                    c.weight = s.weight;
                    c
                })
                .collect();
        }
        // 空态标点首选：/ → 顿号、; → 全角分号、' → 左单引号
        if session.candidates.is_empty() {
            let fallback = match session.raw.as_str() {
                "/" if self.config.input.slash_dunhao => Some("、"),
                ";" => Some("；"),
                "'" => Some("‘"),
                _ => None,
            };
            if let Some(t) = fallback {
                session
                    .candidates
                    .push(Candidate::new(t, session.raw.clone(), CandidateKind::Symbol));
            }
        }
    }

    /// 命令命名空间候选：动态变量（真实值）与工具命令。
    fn command_candidates(&self, raw: &str) -> Vec<Candidate> {
        let name = raw.trim_start_matches('\\');
        let mut out = Vec::new();

        // \n<数字> → 中文数字（小写）；\N<数字> → 大写
        if let Some(num) = name.strip_prefix('n').or_else(|| name.strip_prefix('N')) {
            if let Some(cn) = dynamic::number_to_chinese(num, name.starts_with('N')) {
                out.push(Candidate::new(
                    cn,
                    format!("\\{name}"),
                    CandidateKind::Command,
                ));
            }
        }

        let commands: Vec<(&str, String)> = vec![
            ("date", dynamic::date_string()),
            ("date2", dynamic::date_string_iso()),
            ("time", dynamic::time_string()),
            ("time2", dynamic::time_short()),
            ("week", dynamic::week_string()),
        ];
        for (k, v) in &commands {
            if k.starts_with(name) {
                out.push(Candidate::new(v.clone(), format!("\\{k}"), CandidateKind::Command));
            }
        }

        // \calc<表达式> → 实时求值
        if let Some(expr) = name.strip_prefix("calc") {
            if expr.is_empty() {
                out.push(Candidate::new(
                    "＝计算器：\\calc(1+2)*3".to_string(),
                    "\\calc".to_string(),
                    CandidateKind::Command,
                ));
            } else if let Some(v) = dynamic::calc(expr) {
                let shown = format!("＝{}", dynamic::fmt_num(v));
                let mut c = Candidate::new(shown, format!("\\calc{expr}"), CandidateKind::Command);
                c.commit_override = Some(dynamic::fmt_num(v));
                out.push(c);
            } else {
                out.push(Candidate::new(
                    "＝表达式无效".to_string(),
                    format!("\\calc{expr}"),
                    CandidateKind::Command,
                ));
            }
        } else if name == "c" {
            // calc 前缀提示
            out.push(Candidate::new(
                "＝计算器：\\calc(1+2)*3".to_string(),
                "\\calc".to_string(),
                CandidateKind::Command,
            ));
        }

        // \w<词> → Rime encoder 规则造词（构码 + 注释显示编码）
        if let Some(word) = name.strip_prefix('w') {
            if !word.is_empty() {
                if let Some(code) = self.encode_word(word) {
                    let mut c =
                        Candidate::new(word.to_string(), format!("\\w{word}"), CandidateKind::Command);
                    c.comment = code.clone();
                    c.commit_override = Some(word.to_string());
                    // 直接给候选码，选词时 learn() 自动入用户词库
                    c.code = code;
                    out.push(c);
                } else {
                    out.push(Candidate::new(
                        "造词失败：字无编码或无匹配规则".to_string(),
                        format!("\\w{word}"),
                        CandidateKind::Command,
                    ));
                }
            }
        }
        out
    }

    /// Rime encoder 造词：formula 形如 `AaAbBaBb`（大写=第几个字，Z=末字；
    /// 小写=该字第几码）。返回第一个完全可构的规则结果。
    fn encode_word(&self, word: &str) -> Option<String> {
        let chars: Vec<char> = word.chars().collect();
        if self.schema.encoder_rules.is_empty() || chars.is_empty() {
            return None;
        }
        // 每字首选码
        let codes: Vec<Option<String>> = chars
            .iter()
            .map(|c| self.schema.best_code_of(&c.to_string()))
            .collect();
        for rule in &self.schema.encoder_rules {
            if chars.len() < rule.min_len || chars.len() > rule.max_len {
                continue;
            }
            let mut code = String::new();
            let mut ok = true;
            let f: Vec<char> = rule.formula.chars().collect();
            let mut i = 0;
            while i + 1 < f.len() {
                let (up, low) = (f[i], f[i + 1]);
                let idx = if up == 'Z' || up == 'z' {
                    chars.len() - 1
                } else {
                    (up as u8 - b'A') as usize
                };
                let code_pos = (low as u8 - b'a') as usize;
                match codes.get(idx).and_then(|c| c.as_ref()) {
                    Some(cc) => match cc.chars().nth(code_pos) {
                        Some(ch) => code.push(ch),
                        None => {
                            ok = false;
                            break;
                        }
                    },
                    None => {
                        ok = false;
                        break;
                    }
                }
                i += 2;
            }
            if ok && !code.is_empty() {
                return Some(code);
            }
        }
        None
    }

    /// 反查候选（反查表 + 主码注释）。
    fn reverse_candidates(&self, raw: &str) -> Vec<Candidate> {
        let Some(rev) = &self.schema.reverse else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for word in rev.lookup(raw).iter().take(20) {
            let mut c = Candidate::new(word.clone(), raw.to_string(), CandidateKind::Reverse);
            if let Some(code) = self.schema.best_code_of(word) {
                c.comment = code;
            }
            out.push(c);
        }
        out
    }

    /// DictEntry → Candidate（含注释）。
    fn entry_to_candidate(&self, e: &DictEntry) -> Candidate {
        let mut c = Candidate::new(e.text.clone(), e.code.clone(), CandidateKind::Dict);
        c.weight = e.weight;
        c.pinned = e.pinned;
        c.commit_override = e.commit_override.clone();
        c.comment = self.annotate(&e.text);
        c
    }

    /// 注释：拆分 / 拼音 / Unicode 分区（按配置与可用性）。
    pub fn annotate(&self, word: &str) -> String {
        let mut parts: Vec<String> = Vec::new();
        // 【2026-09-05 修复】拆分/注释原为 else-if 互斥——开拆分时注释
        // 被短路（用户实测「显示注释不生效」）。两开关独立生效：
        // 拆分（部件提示）与拼音注释可同显，均受各自开关控制。
        if self.config.candidates.show_split {
            if let Some(sp) = &self.schema.split {
                let s = sp.annotate_word(word, 2);
                if !s.is_empty() {
                    parts.push(s);
                }
            }
        }
        if self.config.candidates.show_comment {
            if let Some(py) = &self.schema.pinyin {
                let s = py.annotate_word(word, 2);
                if !s.is_empty() {
                    parts.push(s);
                }
            } else if let Some(rev) = &self.schema.reverse {
                // 【2026-09-05】无拼音注释表时回退反查表（词→码）——
                // Bime 反查表数据已有，「显示注释」开箱即用（整词
                // 优先，miss 则前 2 字逐字拼）。
                let s = rev
                    .code_of(word)
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| {
                        word.chars()
                            .take(2)
                            .filter_map(|ch| rev.code_of(&ch.to_string()))
                            .collect::<Vec<_>>()
                            .join(" ")
                    });
                if !s.is_empty() {
                    parts.push(s);
                }
            }
        }
        if let Some(uni) = &self.schema.unicode_block {
            if let Some(fc) = word.chars().next() {
                if let Some(u) = uni.get(fc) {
                    if u != "基本" {
                        parts.push(format!("[{u}]"));
                    }
                }
            }
        }
        parts.join(" ")
    }

    /// 会话状态快照。
    /// 【锁名次显示还原 2026-09-06】回显层把尾部数字锁（码表名次）还原为
    /// 「候选框显示名次」：按 4 选「码表第 3」回显 ae4（与按键一致），而非
    /// 内部锁值 ae3——用户词插入使显示序与码表序错位，用户实测按 4567890
    /// 回显 ae3465678 观感错乱。仅还原尾部锁；中置锁与 ;/' 锁保持原样
    /// （;/' 的锁形即按键形态，本就一致）。Session.raw 真值不动（解码、
    /// 基准、测试断言均用真值）。
    fn display_raw(&self, session: &Session) -> String {
        let raw = &session.raw;
        if raw.is_empty() || !self.sentence_active() {
            return raw.clone();
        }
        let parsed = self.parse_locks(raw);
        let base = &parsed.base;
        if base.is_empty() || raw.len() <= base.len() {
            return raw.clone();
        }
        let tail = &raw[base.len()..];
        let mut chars = tail.chars();
        let lock_c = match (chars.next(), chars.next()) {
            (Some(c), None) => c,
            _ => return raw.clone(),
        };
        if !lock_c.is_ascii_digit() {
            return raw.clone();
        }
        let file_rank = if lock_c == '0' {
            10
        } else {
            lock_c.to_digit(10).unwrap_or(0) as usize
        };
        if !(2..=10).contains(&file_rank) {
            return raw.clone();
        }
        // 锁到的词（码表原序第 file_rank）→ 在候选显示序里的名次
        let word = match self.schema.dict.lookup(base).iter().nth(file_rank - 1) {
            Some(e) => e.text.clone(),
            None => return raw.clone(),
        };
        let disp = match self.schema.candidates(base).iter().position(|c| c.text == word) {
            Some(i) => i + 1,
            None => return raw.clone(),
        };
        if !(2..=10).contains(&disp) {
            return raw.clone();
        }
        let disp_c = if disp == 10 {
            '0'
        } else {
            char::from_digit(disp as u32, 10).unwrap()
        };
        format!("{}{}", base, disp_c)
    }

    pub fn state(&self, session: &Session) -> SessionState {
        let page_size = self.config.candidates.page_size.max(1);
        let pages = (session.candidates.len() + page_size - 1) / page_size;
        let start = (session.page * page_size).min(session.candidates.len());
        let end = (start + page_size).min(session.candidates.len());
        let mut preedit = self.display_raw(session);
        if !self.config.input.code_disguise.is_empty() && !preedit.is_empty() {
            preedit = format!("{}{}", self.config.input.code_disguise, preedit);
        }
        // 【动态候选实时显示 2026-09-06】{日期}族候选显示实时值（用户
        // 拍板：不要字面标记要实时）；{重复上屏} 显示上次上屏内容（还
        // 没上过屏则不显示该候选）；{加词}/{隐藏候选} 显示功能提示。
        // 只改显示层（session.candidates 原样）——选中提交走
        // resolve_dynamic，与显示一致。
        let shown: Vec<Candidate> = session.candidates[start..end]
            .iter()
            .filter_map(|c| {
                let mut c = c.clone();
                if c.text.starts_with('{') && c.text.ends_with('}') && c.text.len() > 2 {
                    let tag = &c.text[1..c.text.len() - 1];
                    if tag == "重复上屏" {
                        if self.last_commit.is_empty() {
                            return None;
                        }
                        c.text = self.last_commit.clone();
                    } else if tag == "加词" {
                        c.text = "＋加词".into();
                    } else if tag == "隐藏候选" {
                        c.text = "－隐藏候选".into();
                    } else if let Some(v) = dynamic::expand(tag) {
                        c.text = v;
                    }
                }
                Some(c)
            })
            .collect();
        SessionState {
            raw: self.display_raw(session),
            preedit,
            candidates: shown,
            page: session.page,
            page_count: pages,
            // 高亮（页内下标）：↑↓ 已同步页码，越界兜底 0
            selected: if session.selected >= start && session.selected < end {
                session.selected - start
            } else {
                0
            },
            aux: match session.mode {
                // 【2026-09-05】反查提示精简为「〔反查〕」——TSF 端把它与
                // raw 拼接成编码行「〔反查〕 ni」，全程提示反查态。
                InputMode::Reverse => "〔反查〕".into(),
                InputMode::Command => "〔命令〕".into(),
                _ => String::new(),
            },
            mode: session.mode,
            chinese: session.chinese,
            full_shape: self.config.punct.full_shape,
            ascii_punct: self.config.input.ascii_punct,
            reverse_mode: session.mode == InputMode::Reverse,
            show_code: self.config.input.show_code,
            show_index: self.config.candidates.show_index,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hufu_types::Modifiers;

    fn key(c: char) -> KeyInput {
        KeyInput {
            key: KeyCode::Char(c),
            modifiers: Modifiers::default(),
            is_press: true,
        }
    }

    fn ctrl_shift(c: char) -> KeyInput {
        KeyInput {
            key: KeyCode::Char(c),
            modifiers: Modifiers {
                ctrl: true,
                shift: true,
                ..Default::default()
            },
            is_press: true,
        }
    }

    /// 【数字编码锁消歧 2026-09-05】数字编码码表（a8=来、u3=的、
    /// b8=如）下 raw 数字的编码/锁二义：与任意后缀组成词条 → 编码
    /// 字符（跨段：vvb8 的 b8）；无词条 → 选重锁（ve; 转的内部
    /// 数字 ve2）。固化该行为。
    #[test]
    fn digit_code_locks_disambiguation() {
        let codes = ["b8", "a8", "u3", "r8", "vv", "vvb", "vvbn", "qpu", "ldl"];
        // 与 Engine::parse_locks 同款后缀枚举闭包
        let is_code = |p: &str| {
            let cs: Vec<char> = p.chars().collect();
            (1..=cs.len()).any(|j| {
                let s: String = cs[cs.len() - j..].iter().collect();
                codes.contains(&s.as_str())
            })
        };
        // 跨段：vvb8 的 8 与后缀 b 组成 b8（如）→ 保留编码
        let r = parse_rank_locks_keep_digits("vvb8", &is_code);
        assert_eq!(r.base, "vvb8");
        assert!(r.locks.is_empty(), "vvb8 的 8 是编码（b8=如），不应成锁");
        // 锁数字：ve2（ve; 的内部表达）——e2/2 均非词条 → rank 2 锁
        let r2 = parse_rank_locks_keep_digits("ve2", &is_code);
        assert_eq!(r2.base, "ve");
        assert_eq!(r2.locks, vec![(2, 2)], "ve2 的 2 应为选重锁");
        // 整句流：zlr8（错了）的 r8=了 是词条 → 保留
        let r3 = parse_rank_locks_keep_digits("zlr8", &is_code);
        assert_eq!(r3.base, "zlr8");
        assert!(r3.locks.is_empty());
        // 词锁分号恒锁（数字编码表下不变）
        let r4 = parse_rank_locks_keep_digits("qpu;", &is_code);
        assert_eq!(r4.base, "qpu");
        assert_eq!(r4.locks, vec![(3, 2)]);
        // 普通表行为不变：jd2 数字一律锁
        let r5 = parse_rank_locks("jd2");
        assert_eq!(r5.base, "jd");
        assert_eq!(r5.locks, vec![(2, 2)]);
    }

    #[test]
    fn rank_locks_parse() {
        // 无锁：base 原样
        let p = parse_rank_locks("jdtuja");
        assert_eq!(p.base, "jdtuja");
        assert!(p.locks.is_empty());        assert_eq!(p.orig_of_base.len(), 6);

        // 尾锁：jd + 2（锁在 base 终点 2，段起点由解码器决定）
        let p = parse_rank_locks("jd2");
        assert_eq!(p.base, "jd");
        assert_eq!(p.locks, vec![(2, 2)]);
        assert_eq!(p.orig_of_base, vec![0, 1]);

        // 中置锁 + 续打
        let p = parse_rank_locks("jd2tuja");
        assert_eq!(p.base, "jdtuja");
        assert_eq!(p.locks, vec![(2, 2)]);
        assert_eq!(p.orig_of_base, vec![0, 1, 3, 4, 5, 6], "t 在原 raw 下标 3");

        // 用户实例：syftuuu;w;jgfd → 让我看看怎么个事
        let p = parse_rank_locks("syftuuu;w;jgfd");
        assert_eq!(p.base, "syftuuuwjgfd");
        assert_eq!(p.locks, vec![(7, 2), (8, 2)], "; 只锁其前词段终点");

        // 分号/引号/0
        let p = parse_rank_locks("jd;");
        assert_eq!(p.locks[0].1, 2);
        let p = parse_rank_locks("jd'");
        assert_eq!(p.locks[0].1, 3);
        let p = parse_rank_locks("jd0");
        assert_eq!(p.locks[0].1, 10);
        // 连续后缀只取第一个
        let p = parse_rank_locks("jd2;");
        assert_eq!(p.locks.len(), 1);
        assert_eq!(p.base, "jd");
        // 两段各自锁
        let p = parse_rank_locks("jd2tu'");
        assert_eq!(p.base, "jdtu");
        assert_eq!(p.locks, vec![(2, 2), (4, 3)]);
        // 前导符号不锁
        let p = parse_rank_locks(";");
        assert_eq!(p.base, ";");
        assert!(p.locks.is_empty());
    }

    /// mock 整句解码器：识别 raw 中的锁，返回「锁文+余量」形态候选。
    struct MockDec;
    impl SentenceDecoder for MockDec {
        fn decode_rich(&self, raw: &str) -> std::sync::Arc<SentenceDecode> {
            let p = parse_rank_locks(raw);
            let hits = if p.base.is_empty() {
                Vec::new()
            } else {
                let rank = p.locks.first().map(|l| l.1).unwrap_or(1);
                let text = format!("锁{}+{}", rank, p.base);
                vec![SentenceHit {
                    text,
                    score: -1.0,
                    confidence: -1.0,
                    max_rank: 1,
                    sum_rank: 1,
                    exact: true,
                    word_ends: Vec::new(),
                    segmented: p.base.clone(),
                    partial: false,
                }]
            };
            std::sync::Arc::new(SentenceDecode {
                hits,
                truncated: false,
                early_hits: Vec::new(),
                early_truncated: false,
            })
        }
    }

    #[test]
    fn sentence_rank_key_locks_not_commits() {
        // 整句方案名带「整句」+ auto_enable → mock 解码器生效
        let dir = std::env::temp_dir().join(format!("hufu-eng-整句lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("main.txt"),
            "#hufu-dict v1 name=整句测试\njd\t人\njd\t什么\n",
        )
        .unwrap();
        let mut cfg = hufu_config::Config::default();
        cfg.sentence.enabled = true;
        cfg.sentence.auto_enable = true;
        let mut eng = Engine::with_schema_dir(&dir, cfg).unwrap();
        eng.set_sentence_decoder(Some(std::sync::Arc::new(MockDec)));

        let mut s = Session::new(true);
        eng.process_key(&mut s, key('j'));
        eng.process_key(&mut s, key('d'));
        // 整句模式下按 2：写入编码选重，不上屏，候选首为锁定结果
        let out = eng.process_key(&mut s, key('2'));
        assert!(out.commit.is_none(), "整句选重不应立即上屏（实际 {:?}）", out.commit);
        assert!(out.consumed);
        let st = eng.state(&s);
        assert_eq!(st.candidates[0].text, "锁2+jd", "候选首 = 锁定名次2");
        assert_eq!(st.raw, "jd2");
        // 继续打字：锁保留、组句继续
        let _ = eng.process_key(&mut s, key('t'));
        let st = eng.state(&s);
        assert_eq!(st.raw, "jd2t");
        assert_eq!(st.candidates[0].text, "锁2+jdt");
        // 空格才上屏
        let out = eng.process_key(&mut s, key(' '));
        assert_eq!(out.commit.unwrap(), "锁2+jdt");

        // 分号锁第 2：名次换算后写入（此处字典序第2=什么 → 后缀 '2'）
        let mut s2 = Session::new(true);
        eng.process_key(&mut s2, key('j'));
        eng.process_key(&mut s2, key('d'));
        let out = eng.process_key(&mut s2, key(';'));
        assert!(out.commit.is_none());
        let st = eng.state(&s2);
        assert_eq!(st.candidates[0].text, "锁2+jd");
        assert_eq!(st.raw, "jd2", "分号归一为码表名次后缀");
    }

    #[test]
    fn nonsentence_rank_key_still_commits() {
        // 非整句（方案名不含「整句」）：数字选重立即上屏（原行为）
        let (mut eng, _dir) = test_engine("nsl");
        let mut s = Session::new(true);
        eng.process_key(&mut s, key('j'));
        eng.process_key(&mut s, key('d'));
        let out = eng.process_key(&mut s, key('2'));
        assert_eq!(out.commit.unwrap(), "到的", "非整句数字选重立即上屏第 2");
    }

    // 【用户词选重 2026-09-06】整句方案锁式选重只认码表位次：/jc 加
    // 的用户词不在码表原序 → 换算不到锁位次，原实现退化把按键字符塞
    // 进 raw（ae+3 → 死码 ae3 被解码器再当「码表第 3」锁，用户实测
    // 出乛）。修复：单段纯净态（无前段无锁）选中即上屏该词。
    #[test]
    fn user_word_rank_select_commits() {
        let (mut eng, dir) = test_engine("uwr");
        // 整句引擎 + 用户词 jd 第 3 位（码表 [就,到的,加] → [就,到的,新,加]）
        std::fs::write(
            dir.join("用户词.txt"),
            "#hufu-dict v1 name=user_words\njd\t新\t1\tp3\n",
        )
        .unwrap();
        eng.reload_user_data();
        // 整句引擎：方案名 t 不含「整句」——锁式选重只在整句开。构造
        // 整句引擎：直接改 sentence 装载条件不现实，改用 schema 目录
        // 名含「整句」再建一个。
        let dir2 = std::env::temp_dir().join(format!("hufu-eng-整句uwrs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir2);
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(
            dir2.join("main.txt"),
            "#hufu-dict v1 name=整句测试\na\t啊\naa\t阿\njd\t就\njd\t到的\njd\t加\n",
        )
        .unwrap();
        std::fs::write(
            dir2.join("用户词.txt"),
            "#hufu-dict v1 name=user_words\njd\t新\t1\tp3\n",
        )
        .unwrap();
        let mut cfg2 = hufu_config::Config::default();
        cfg2.sentence.enabled = true;
        cfg2.sentence.auto_enable = true;
        let mut eng2 = Engine::with_schema_dir(&dir2, cfg2).unwrap();
        eng2.set_sentence_decoder(Some(std::sync::Arc::new(MockDec)));
        assert!(eng2.sentence_active(), "整句模式生效");
        let mut s = Session::new(true);
        eng2.process_key(&mut s, key('j'));
        eng2.process_key(&mut s, key('d'));
        // 显示序：MockDec 首选压首 + 码表 [就,到的,新(p3),加]
        let st = eng2.state(&s);
        let texts: Vec<String> = st.candidates.iter().map(|c| c.text.clone()).collect();
        assert!(
            texts.contains(&"新".to_string()),
            "p3 用户词进候选: {texts:?}"
        );
        // 按 3：不再变死码 jd3 出「加」（码表第 3），而是直接上屏「新」
        let out = eng2.process_key(&mut s, key('3'));
        assert_eq!(out.commit.unwrap(), "新", "用户词无锁位次 → 单段态直接上屏");
        assert!(s.raw.is_empty(), "上屏后缓冲清空");

        // 码表词选重不受影响：按 2（到的，码表第 2）→ 锁式：raw=jd2、
        // 不上屏，候选首=锁定名次（MockDec 的锁产物），空格才上屏
        let mut s2 = Session::new(true);
        eng2.process_key(&mut s2, key('j'));
        eng2.process_key(&mut s2, key('d'));
        let out2 = eng2.process_key(&mut s2, key('2'));
        assert!(out2.commit.is_none(), "码表词锁式选重不立即上屏");
        assert_eq!(s2.raw, "jd2", "码表名次 2 写入锁");
        let _ = std::fs::remove_dir_all(&dir2);
    }

    // 【翻页/顶字复用键 2026-09-06】-（——）/ =（+）候选可翻时翻页，
    // 翻不动时直接顶屏（首选+符号形态）。一页装满（pages=1）时按 -
    // 或 = 都不可翻 → 顶字；多页时 = 翻下页、- 翻上页；翻页后顶字。
    #[test]
    fn page_or_commit_keys() {
        let (mut eng, _dir) = test_engine("pok");
        // jd 3 个候选（page_size 4）→ 一页装满，无翻页空间
        let mut s = Session::new(true);
        eng.process_key(&mut s, key('j'));
        eng.process_key(&mut s, key('d'));
        // 按 -：无上页 → 顶字「就-」
        let out = eng.process_key(&mut s, key('-'));
        assert_eq!(out.commit.unwrap(), "就-", "一页满时 - 顶字");
        assert!(s.raw.is_empty());
        // 按 =：无下页 → 顶字「就=」
        let mut s2 = Session::new(true);
        eng.process_key(&mut s2, key('j'));
        eng.process_key(&mut s2, key('d'));
        let out2 = eng.process_key(&mut s2, key('='));
        assert_eq!(out2.commit.unwrap(), "就=", "一页满时 = 顶字");

        // Shift 形态顶字：—— / +
        let mut s3 = Session::new(true);
        eng.process_key(&mut s3, key('j'));
        eng.process_key(&mut s3, key('d'));
        let out3 = eng.process_key(&mut s3, {
            let mut k = key('-');
            k.modifiers.shift = true;
            k
        });
        assert_eq!(out3.commit.unwrap(), "就——", "Shift+- 顶字 ——");

        // 多页场景：临时把 page_size 调 2（jd 3 候选 → 2 页）
        let mut s4 = Session::new(true);
        eng.config.candidates.page_size = 2;
        eng.process_key(&mut s4, key('j'));
        eng.process_key(&mut s4, key('d'));
        let out4 = eng.process_key(&mut s4, key('='));
        assert!(out4.commit.is_none(), "= 有下页时翻页不上屏");
        assert_eq!(s4.page, 1, "翻到第 2 页");
        // 到末页再按 = → 顶字（首选=当前页首「加」）
        let out5 = eng.process_key(&mut s4, key('='));
        assert_eq!(out5.commit.unwrap(), "加=", "末页再 = 顶字");
        // - 回翻：第 2 页按 - 回第 1 页
        let mut s5 = Session::new(true);
        eng.process_key(&mut s5, key('j'));
        eng.process_key(&mut s5, key('d'));
        eng.process_key(&mut s5, key('='));
        let out6 = eng.process_key(&mut s5, key('-'));
        assert!(out6.commit.is_none(), "- 有上页时翻页不上屏");
        assert_eq!(s5.page, 0, "翻回第 1 页");
    }

    // 【锁态重锁与回显还原 2026-09-06】用户词插入使显示序≠码表序：
    // jd 显示 [就,到的,新(p3),加]——按 4（加=码表第3）旧实现追加锁字符
    // 成 jd3，再按数字逐个追加换算名次堆出死码串、候选恒首个锁产物。
    // 修复：锁态按数字=重锁（换名次不追加）；回显层把尾锁还原为显示
    // 名次（按 4 回显 jd4）。码表 jd=[就,到的,加]，用户词 新 p3。
    #[test]
    fn lock_relock_and_display() {
        let (mut eng, dir) = test_engine("整句锁显");
        std::fs::write(
            dir.join("用户词.txt"),
            "#hufu-dict v1 name=user_words\njd\t新\t1\tp3\n",
        )
        .unwrap();
        eng.reload_user_data();
        eng.config.sentence.enabled = true;
        eng.config.sentence.auto_enable = true;
        eng.set_sentence_decoder(Some(Arc::new(MockDec)));
        // 按 3（新，用户词）直接上屏
        let mut s = Session::new(true);
        eng.process_key(&mut s, key('j'));
        eng.process_key(&mut s, key('d'));
        let out = eng.process_key(&mut s, key('3'));
        assert_eq!(out.commit.unwrap(), "新", "用户词选重直接上屏");
        // 按 4（加=码表第3）→ 锁式 raw=jd3，回显 jd4（显示名次还原）
        let mut s2 = Session::new(true);
        eng.process_key(&mut s2, key('j'));
        eng.process_key(&mut s2, key('d'));
        let out2 = eng.process_key(&mut s2, key('4'));
        assert!(out2.commit.is_none(), "码表词锁式不上屏");
        assert_eq!(s2.raw, "jd3", "内部锁值=码表名次 3");
        assert_eq!(out2.state.as_ref().unwrap().raw, "jd4", "回显还原=显示名次 4");
        // 锁态再按 3（新，用户词）→ 改选直接上屏（不堆死码）
        let out3 = eng.process_key(&mut s2, key('3'));
        assert_eq!(out3.commit.unwrap(), "新", "锁态改选用户词直接上屏");
        // 锁态再按数字（码表词）→ 重锁（替换名次，不追加）
        let mut s3 = Session::new(true);
        eng.process_key(&mut s3, key('j'));
        eng.process_key(&mut s3, key('d'));
        eng.process_key(&mut s3, key('4')); // raw=jd3
        let out4 = eng.process_key(&mut s3, key('2')); // 到的=码表2
        assert!(out4.commit.is_none());
        assert_eq!(s3.raw, "jd2", "锁态重锁：raw 仍单锁（码表名次2），非 jd32");
        // 连按数字不堆死码：再按 4 → 仍 jd3
        let out5 = eng.process_key(&mut s3, key('4'));
        assert_eq!(s3.raw, "jd3", "连按数字重锁，raw 恒单锁");
        assert_eq!(out5.state.as_ref().unwrap().raw, "jd4", "回显随按键还原");
    }

    fn test_engine(tag: &str) -> (Engine, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("hufu-eng-dyn-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("main.txt"),
            "#hufu-dict v1 name=t\na\t啊\naa\t阿\njd\t就\njd\t到的\njd\t加\n",
        )
        .unwrap();
        let cfg = hufu_config::Config::default();
        let eng = Engine::with_schema_dir(&dir, cfg).unwrap();
        (eng, dir)
    }

    /// 【过程态防御】神经重排不得把 partial（未消耗全部 raw 的前缀态）
    /// 候选提到完整态之前——真实场景：qlagy 时 Qwen 把 4 键「老鬼」
    /// 提到 5 键「老痒」前霸首（2026-09-04 用户实测）。rerank_request
    /// 必须过滤 partial；缺席于重排序的 partial 在 apply 时自然靠后。
    /// 【选重深度约束】无锁时 rerank 不得把 sum_rank 更深的候选提到
    /// 原首选之前（javz 首选 们服 sum=2，Qwen 偏爱词「舒服」sum=3
    /// ——要打舒服规范打法 ja2vz）。2026-09-05 用户实测三案例。
    struct DepthMock;
    impl SentenceDecoder for DepthMock {
        fn decode_rich(&self, raw: &str) -> std::sync::Arc<SentenceDecode> {
            let hits = if raw == "javz" {
                vec![
                    SentenceHit { text: "们服".into(), score: -5.0, confidence: -5.0, max_rank: 1, sum_rank: 2, exact: true, word_ends: vec![(1,2),(2,4)], segmented: "ja vz".into(), partial: false },
                    SentenceHit { text: "舒服".into(), score: -5.5, confidence: -5.5, max_rank: 2, sum_rank: 3, exact: true, word_ends: vec![(1,2),(2,4)], segmented: "ja vz".into(), partial: false },
                    SentenceHit { text: "们改变".into(), score: -6.0, confidence: -6.0, max_rank: 1, sum_rank: 3, exact: true, word_ends: vec![(1,2),(3,4)], segmented: "ja vz".into(), partial: false },
                ]
            } else {
                Vec::new()
            };
            std::sync::Arc::new(SentenceDecode { hits, truncated: false, early_hits: Vec::new(), early_truncated: false })
        }
    }

    #[test]
    fn rerank_never_lifts_deeper_rank() {
        let dir = std::env::temp_dir().join("hufu-eng-depth");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut eng = Engine::with_schema_dir(&dir, Config::default()).unwrap();
        eng.set_sentence_decoder(Some(std::sync::Arc::new(DepthMock)));
        let mut s = Session::new(true);
        s.raw = "javz".into();
        s.candidates = vec![
            Candidate::new("们服", "javz", CandidateKind::Sentence),
            Candidate::new("舒服", "javz", CandidateKind::Sentence),
            Candidate::new("们改变", "javz", CandidateKind::Sentence),
        ];
        // Qwen 把「舒服」排第一（词频偏爱）
        {
            let mut cache = eng.rerank_cache.lock().unwrap();
            cache.insert("javz".into(), vec!["舒服".into(), "们改变".into(), "们服".into()]);
        }
        eng.refresh_rerank(&mut s);
        let texts: Vec<&str> = s.candidates.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts[0], "们服", "原首选（深度 2）必须保持首位: {texts:?}");
        assert!(texts.iter().position(|t| *t == "舒服").unwrap() > 0, "深度 3 的舒服不得被 Qwen 提到首位: {texts:?}");
    }

    #[test]
    fn rerank_never_lifts_partial() {
        let dir = std::env::temp_dir().join("hufu-eng-partial");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut eng = Engine::with_schema_dir(&dir, Config::default()).unwrap();
        // 造候选列表：完整态在前、过程态在后（qlagy 语义）
        let mut s = Session::new(true);
        s.raw = "y".into();
        s.committed_raw = "qlag".into();
        let mut full = Candidate::new("老痒", "y", CandidateKind::Sentence);
        full.partial = false;
        let full2 = Candidate::new("老痒痒", "y", CandidateKind::Sentence);
        let mut part = Candidate::new("老鬼", "y", CandidateKind::Sentence);
        part.partial = true;
        s.candidates = vec![full, full2, part];
        // 1) rerank_request 必须排除 partial：texts 只含完整态
        eng.config.sentence.rerank.enabled = true;
        let req = eng.rerank_request(&s);
        assert!(req.is_some(), "完整态≥2 应派发");
        if let Some((_, _, texts)) = req {
            assert!(!texts.contains(&"老鬼".to_string()), "partial 不得进重排: {texts:?}");
        }
        // 2) 缓存塞入「老鬼在前」的被污染顺序（模拟修复前的旧缓存
        //    残留）——apply 后老鬼仍沉底（partial 恒 MAX），完整态间
        //    按 Qwen 顺序自由重排（老痒痒可居首）。
        {
            let mut cache = eng.rerank_cache.lock().unwrap();
            cache.insert("qlagy".into(), vec!["老鬼".into(), "老痒痒".into()]);
        }
        eng.refresh_rerank(&mut s);
        let pos_lg = s.candidates.iter().position(|c| c.text == "老鬼").unwrap();
        assert_eq!(pos_lg, s.candidates.len() - 1, "过程态恒沉底: {:?}", s.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>());
        assert!(s.candidates[0].text != "老鬼", "过程态不可居首");
    }

    #[test]
    fn dynamic_date_week() {
        let (mut eng, _dir) = test_engine("date");
        let mut s = Session::new(true);
        eng.process_key(&mut s, key('\\'));
        eng.process_key(&mut s, key('d'));
        let _ = eng.process_key(&mut s, key('a'));
        let snap = eng.state(&s); let texts: Vec<String> = snap.candidates.iter().map(|c| c.text.clone()).collect();
        assert!(texts.iter().any(|t| t.contains('年') && t.contains('月')), "{texts:?}");
        // 星期
        let (mut eng2, _d2) = test_engine("week");
        let mut s2 = Session::new(true);
        eng2.process_key(&mut s2, key('\\'));
        eng2.process_key(&mut s2, key('w'));
        let _ = eng2.process_key(&mut s2, key('e'));
        let snap = eng2.state(&s2);
        let texts: Vec<String> = snap.candidates.iter().map(|c| c.text.clone()).collect();
        assert!(texts.iter().any(|t| t.starts_with("星期")), "{texts:?}");
    }

    #[test]
    fn dynamic_number() {
        let (mut eng, _dir) = test_engine("num");
        let mut s = Session::new(true);
        eng.process_key(&mut s, key('\\'));
        for c in "n12345".chars() {
            eng.process_key(&mut s, key(c));
        }
        let snap = eng.state(&s); let texts: Vec<String> = snap.candidates.iter().map(|c| c.text.clone()).collect();
        assert!(texts.iter().any(|t| t == &"一万二千三百四十五".to_string()), "{texts:?}");
        // 上屏
        let out = eng.process_key(&mut s, key(' '));
        assert_eq!(out.commit.unwrap(), "一万二千三百四十五");

        // 大写
        let (mut eng2, _d2) = test_engine("num2");
        let mut s2 = Session::new(true);
        eng2.process_key(&mut s2, key('\\'));
        for c in "N1234".chars() {
            eng2.process_key(&mut s2, key(c));
        }
        let snap = eng2.state(&s2); let texts: Vec<String> = snap.candidates.iter().map(|c| c.text.clone()).collect();
        assert!(texts.iter().any(|t| t == &"壹仟贰佰叁拾肆".to_string()), "{texts:?}");
    }

    // 【码表动态变量 2026-09-06】{日期}族上屏展开（候选显示保留字面标记）、
    // {重复上屏} 回放 last_commit、{加词} 原样透传（DLL 拦截弹窗）。
    #[test]
    fn dict_dynamic_tags() {
        let dir = std::env::temp_dir().join(format!("hufu-eng-dynv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("码表.txt"),
            "a 啊\nz3 {重复上屏}\n/jc {加词}\n/rq {日期} {日期-}\n",
        )
        .unwrap();
        let mut eng = Engine::with_schema_dir(&dir, Config::default()).unwrap();
        let mut s = Session::new(true);

        // /rq：候选显示实时日期值（非字面标记），上屏同值
        for c in "/rq".chars() {
            eng.process_key(&mut s, key(c));
        }
        let snap = eng.state(&s);
        assert!(
            snap.candidates
                .iter()
                .any(|c| c.text.contains('年') && c.text.contains('月') && c.text.contains('日')),
            "候选显示实时值: {:?}",
            snap.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>()
        );
        let out = eng.process_key(&mut s, key(' '));
        let t = out.commit.unwrap();
        assert!(t.contains('年') && t.contains('月') && t.contains('日'), "上屏展开: {t}");

        // z3：重复上屏回放 last_commit（host 收口更新，测试直设）
        eng.last_commit = "重复我".into();
        for c in "z3".chars() {
            eng.process_key(&mut s, key(c));
        }
        let out = eng.process_key(&mut s, key(' '));
        assert_eq!(out.commit.unwrap(), "重复我", "z3 = 上次上屏内容");

        // /jc：加词指令原样透传给 DLL（唯一候选 → 末键自动顶字，
        // 无需空格确认）
        let mut last: Option<String> = None;
        for c in "/jc".chars() {
            last = eng.process_key(&mut s, key(c)).commit;
        }
        assert_eq!(last.unwrap(), "{加词}", "加词指令透传");
    }

    #[test]
    fn pin_and_hide_via_keys() {
        let (mut eng, _dir) = test_engine("pin");
        let mut s = Session::new(true);
        // jd → 就/到的/加；Ctrl+Shift+2 置顶第 2 个（到的）
        eng.process_key(&mut s, key('j'));
        eng.process_key(&mut s, key('d'));
        let st = eng.state(&s);
        assert_eq!(st.candidates[0].text, "就");

        let _ = eng.process_key(&mut s, ctrl_shift('2'));
        let st = eng.state(&s);
        assert_eq!(st.candidates[0].text, "到的", "置顶后『到的』应在首位");

        // 日志落盘 + 回放等价
        let dir = eng.schema.dir.clone();
        let log = std::fs::read_to_string(dir.join("用户调整.txt")).unwrap();
        assert!(log.contains("{置顶}jd\t到的"), "{log}");
        let adj = hufu_dict::user::UserAdjust::parse(
            &log.lines().map(|l| l.to_string()).collect::<Vec<_>>(),
        );
        let base = eng.schema.dict.lookup("jd").into_iter().cloned().collect::<Vec<_>>();
        let out = adj.apply("jd", &base);
        assert_eq!(out[0].text, "到的");

        // Ctrl+Delete 软删首选（到的）→ 首选回到 就
        let mut s3 = Session::new(true);
        eng.process_key(&mut s3, key('j'));
        eng.process_key(&mut s3, key('d'));
        let del = KeyInput {
            key: KeyCode::Delete,
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            is_press: true,
        };
        let _ = eng.process_key(&mut s3, del);
        let st = eng.state(&s3);
        assert_eq!(st.candidates[0].text, "就", "软删后『就』应回到首位");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn calc_command() {
        let (mut eng, dir) = test_engine("calc");
        let mut s = Session::new(true);
        eng.process_key(&mut s, key('\\'));
        for c in "calc(1+2)*3".chars() {
            eng.process_key(&mut s, key(c));
        }
        let snap = eng.state(&s);
        assert!(snap.candidates.iter().any(|c| c.text.contains('9')), "{:?}",
            snap.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>());
        // 上屏是纯数值
        let o = eng.process_key(&mut s, key(' '));
        assert_eq!(o.commit.unwrap(), "9");
        // 无效表达式
        let mut s2 = Session::new(true);
        eng.process_key(&mut s2, key('\\'));
        for c in "calc1+".chars() {
            eng.process_key(&mut s2, key(c));
        }
        let snap = eng.state(&s2);
        assert!(snap.candidates.iter().any(|c| c.text.contains("无效")), "提示无效");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn word_making_encoder() {
        // Rime encoder：length_equal 2 → formula AaBa（两字词=各取首码）
        let dir = std::env::temp_dir().join(format!("hufu-eng-wm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("tiger.dict.yaml"),
            "---\nname: t\nsort: by_weight\nencoder:\n  rules:\n    - length_equal: 2\n      formula: AaBa\n...\n就\tjd\n不\tbh\n",
        )
        .unwrap();
        let cfg = hufu_config::Config::default();
        let mut eng = Engine::with_schema_dir(&dir, cfg).unwrap();
        let mut s = Session::new(true);
        eng.process_key(&mut s, key('\\'));
        for c in "w就就".chars() {
            eng.process_key(&mut s, key(c));
        }
        let snap = eng.state(&s);
        let cand = snap.candidates.iter().find(|c| c.commit_override.is_some());
        assert!(cand.is_some(), "应有造词候选");
        let c = cand.unwrap();
        assert_eq!(c.comment, "jj", "两字各取首码: {}", c.comment);
        // 选中 → 上屏词、编码=构码（learn 入库）
        let o = eng.process_key(&mut s, key(' '));
        assert_eq!(o.commit.unwrap(), "就就");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn opencc_variants() {
        // fixture：ST 词组/单字 + emoji 表
        let dir = std::env::temp_dir().join(format!("hufu-eng-oc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let schema = dir.join("schema");
        std::fs::create_dir_all(&schema).unwrap();
        std::fs::create_dir_all(dir.join("转换词典")).unwrap();
        std::fs::write(schema.join("main.txt"), "#hufu-dict v1 name=t\nh\t后\nhq\t后来\n").unwrap();
        std::fs::write(dir.join("转换词典").join("STPhrases.txt"), "后来\t後來\n").unwrap();
        std::fs::write(dir.join("转换词典").join("STCharacters.txt"), "后\t後\n来\t來\n").unwrap();
        std::fs::write(dir.join("转换词典").join("emoji.txt"), "后\t后 👑\n").unwrap();
        let mut cfg = hufu_config::Config::default();
        cfg.opencc.enabled = true;
        cfg.opencc.to_traditional = true;
        let mut eng = Engine::with_schema_dir(&schema, cfg).unwrap();

        // 单字：后 → 後 变体
        let mut s = Session::new(true);
        eng.process_key(&mut s, key('h'));
        let snap = eng.state(&s);
        let texts: Vec<String> = snap.candidates.iter().map(|c| c.text.clone()).collect();
        assert!(texts.contains(&"後".to_string()), "繁体变体: {texts:?}");

        // 词组：后来 → 後來
        let mut s2 = Session::new(true);
        eng.process_key(&mut s2, key('h'));
        eng.process_key(&mut s2, key('q'));
        let snap = eng.state(&s2);
        let texts: Vec<String> = snap.candidates.iter().map(|c| c.text.clone()).collect();
        assert!(texts.contains(&"後來".to_string()), "词组繁体: {texts:?}");

        // emoji 变体
        eng.config.opencc.emoji = true;
        eng.opencc_loaded = false; // 重载表
        let mut s3 = Session::new(true);
        eng.process_key(&mut s3, key('h'));
        let snap = eng.state(&s3);
        let texts: Vec<String> = snap.candidates.iter().map(|c| c.text.clone()).collect();
        assert!(texts.iter().any(|t| t.contains('👑')), "emoji 变体: {texts:?}");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sound_tags() {
        let (mut eng, dir) = test_engine("snd");
        eng.config.sound.enabled = true;
        let mut s = Session::new(true);
        let o = eng.process_key(&mut s, key('a'));
        assert_eq!(o.sound.as_deref(), Some("key"));
        let o = eng.process_key(&mut s, key(' '));
        assert_eq!(o.sound.as_deref(), Some("select"), "空格首选上屏");
        let mut s2 = Session::new(true);
        eng.process_key(&mut s2, key('a'));
        let o = eng.process_key(&mut s2, key('1'));
        assert_eq!(o.sound.as_deref(), Some("select"), "数字选词");
        // 关闭 → 无标签
        eng.config.sound.enabled = false;
        let mut s4 = Session::new(true);
        let o = eng.process_key(&mut s4, key('a'));
        assert_eq!(o.sound, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 【Shift 标点 2026-09-06】有编码态 Shift+标点/数字转 US 键盘 shift
    /// 形态顶字：a 出候选「啊」后 Shift+, → 「啊《」（此前误出「啊，」
    /// ——a18f89c 只修了空态，漏了有编码态标点顶字路径）。Shift+1 →
    /// 「啊！」不再当数字选重；Shift+. → 「啊」+「」」。; 引导态
    /// Shift+; → 清缓冲空态输出「：」。
    #[test]
    fn shift_punct_with_composition() {
        let (mut eng, dir) = test_engine("shiftpunct");
        // 有编码态：a → 候选「啊」，Shift+, 顶字上屏「啊《」
        let mut s = Session::new(true);
        eng.process_key(&mut s, key('a'));
        assert!(!s.raw.is_empty(), "先组成编码态");
        let o = eng.on_char(&mut s, ',', true);
        assert_eq!(o.commit.as_deref(), Some("啊《"), "Shift+, 必须出《: {:?}", o.commit);
        assert!(s.raw.is_empty(), "顶字后缓冲清空");
        // Shift+. → 「啊》」
        let mut s2 = Session::new(true);
        eng.process_key(&mut s2, key('a'));
        let o = eng.on_char(&mut s2, '.', true);
        assert_eq!(o.commit.as_deref(), Some("啊》"));
        // Shift+1 → 「啊！」：不再当数字选重（否则会提交第 1 候选而不带标点）
        let mut s3 = Session::new(true);
        eng.process_key(&mut s3, key('a'));
        let o = eng.on_char(&mut s3, '1', true);
        assert_eq!(o.commit.as_deref(), Some("啊！"), "Shift+1 必须出！: {:?}", o.commit);
        // Shift+/ → 「啊？」（顿号分支不得先吃 shift 变体）
        let mut s4 = Session::new(true);
        eng.process_key(&mut s4, key('a'));
        let o = eng.on_char(&mut s4, '/', true);
        assert_eq!(o.commit.as_deref(), Some("啊？"));
        // 空态回归（a18f89c 行为不变）：Shift+, → 《
        let mut s5 = Session::new(true);
        let o = eng.on_char(&mut s5, ',', true);
        assert_eq!(o.commit.as_deref(), Some("《"));
        // ; 引导态：raw=";" 时 Shift+; → 清缓冲空态输出「：」（不走 ;; 分支）
        let mut s6 = Session::new(true);
        eng.process_key(&mut s6, key(';'));
        let o = eng.on_char(&mut s6, ';', true);
        assert_eq!(o.commit.as_deref(), Some("："), "Shift+; 在 ; 引导态应出：: {:?}", o.commit);
        // 无 shift 的普通标点行为不变：a + , → 「啊，」
        let mut s7 = Session::new(true);
        eng.process_key(&mut s7, key('a'));
        let o = eng.on_char(&mut s7, ',', false);
        assert_eq!(o.commit.as_deref(), Some("啊，"), "无 shift 行为不变");
        let _ = std::fs::remove_dir_all(dir);
    }
}
