//! 成绩单格式化 + records.jsonl 持久化。
//!
//! 成绩单格式对齐晴跟打 Pro 的 Score.Report 逐字节口径（research-digest.md §C.3/C.4；
//! 疾风1代 README:21 记载的字段集合）：
//! 速度(罚后/裸速双数字，无括注)/击键/键准(晴精确式)/码长/字数/难度(有数据时)/
//! 用时(厘秒截断)/错字/退格/废码/选重/标顶/回改/总键数/赛制标记/签名。
//! 双空格分隔、标签与数值间无空格、**不含尾换行**（消费方自行加）。
//!
//! records.jsonl 每行一个 JSON（疾风1代字段超集，可读疾风1代旧文件）：
//! {"ts":1790555080,"date":"2026-09-28","title":"...","source":"群文","mark":"999",
//!  "para_index":0,"chars":196,"keys":208,"backspaces":6,"wrong":2,"time_ms":58285,
//!  "speed":201.77,"accuracy":94.71,"code_len":1.0612}

use crate::stats::SegmentStats;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
static JOURNAL_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static FILE_REPLACE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 一条成绩记录（= jsonl 一行）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreRecord {
    #[serde(default)]
    pub round: Option<crate::rules::RoundMetadata>,
    /// unix 秒
    pub ts: u64,
    /// YYYY-MM-DD
    pub date: String,
    pub title: String,
    /// 来源：群文/本地/文库/练单/自定义
    pub source: String,
    /// 段标记（发文段号/书名段号）
    pub mark: String,
    pub para_index: usize,
    // —— 统计字段（与疾风1代同名同义）——
    pub chars: usize,
    pub keys: u32,
    pub backspaces: u32,
    pub wrong: usize,
    pub time_ms: u64,
    pub speed: f64,
    /// 键准 %（**产线口径** = 晴 Pro 精确式，from_stats 直接取 accuracy_pro，
    /// 下限/除零处理见 stats.rs finalize；不再有简化式回退路径）
    pub accuracy: f64,
    pub code_len: f64,
    // —— 2代扩展字段（旧文件缺省可读）——
    #[serde(default)]
    pub corrections: usize,
    /// 有效速度（晴 Pro 错一罚五口径：Speed − Wrong×5/分钟）
    #[serde(default)]
    pub effective_speed: f64,
    #[serde(default)]
    pub strokes_per_sec: f64,
    #[serde(default)]
    pub composing_keys: u32,
    /// 晴 Pro 精确键准 %（(Hit−Corr−Backs×2)/(字数+Corr)×字数/Hit）
    #[serde(default)]
    pub accuracy_pro: f64,
    /// 废码次数（组字被取消）
    #[serde(default)]
    pub waste_codes: u32,
    /// 上屏键组数（并击口径）
    #[serde(default)]
    pub commit_groups: u32,
    /// 标顶次数（顶屏键完成提交，20ms 键-提交配对；成绩单 `标顶N`）。旧记录缺省 0。
    #[serde(default)]
    pub ding_count: u32,
    /// 选重次数（候选选重键完成提交，20ms 键-提交配对；成绩单 `选重N`）。旧记录缺省 0。
    #[serde(default)]
    pub select_count: u32,
    /// 打词率 %（0–100）：按上屏组的非标点字符归类，回改会撤销被删字符。
    /// None 表示旧记录或恢复快照缺少上屏归类数据，不能用 0 冒充。
    #[serde(default)]
    pub word_rate: Option<f64>,
    /// 同一正文第几次重打，首打为 0；以开打而非完成计次。
    #[serde(default)]
    pub retype_count: u32,
    /// 输入法名（晴发文 leaderboard 字段 input_method；由输入层在结算时注入）
    #[serde(default)]
    pub input_method: String,
    /// 州州四分解（正常/卡顿/回改/错罚 秒 + avgPos + 理想速度）
    #[serde(default)]
    pub time_breakdown: crate::stats::TimeBreakdown,
    /// 州州击键速度 jj（键/秒）——jjC 九级评定的历史输入
    #[serde(default)]
    pub jj: f64,
    /// 有效性（晴 E4 双门槛，见 SegmentStats::is_valid）：字数≥10 且 用时≥10s。
    /// false 的记录进"练习总字数"、不进"有效均速/均键准"类指标（UI 消费）；
    /// 与字数<5 聚合硬门（VALID_MIN_CHARS）语义分开。旧记录缺省 false。
    #[serde(default)]
    pub is_valid: bool,
    /// 难度（跟打文《难度》头 / 文来难度；有则进成绩单 `难度{label}({score:.2})`）。
    /// 旧记录缺省 None。
    #[serde(default)]
    pub difficulty: Option<DifficultyTag>,
    /// 版本（区分1代/2代记录）
    #[serde(default = "default_version")]
    pub version: String,
}

/// 难度标签（成绩单 `难度普(1.84)` 口径：label + 可选数值分）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DifficultyTag {
    /// 难度名（淼/水/易/普/难/虐 等单字）
    pub label: String,
    /// 数值分（晴样本括号部分）；文来只给标签时为 None → 成绩单只写 `难度{label}`
    #[serde(default)]
    pub score: Option<f64>,
}

impl DifficultyTag {
    /// 解析载文链的难度串：`"普 1.84"` → (普, 1.84)；`"普"` → (普, None)；空/坏 → None。
    pub fn parse(text: &str) -> Option<Self> {
        let mut it = text.split_whitespace();
        let label = it.next()?.trim().to_string();
        if label.is_empty() {
            return None;
        }
        let score = it
            .next()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite());
        Some(Self { label, score })
    }
}

fn default_version() -> String {
    "2".into()
}

impl ScoreRecord {
    /// All score sending paths share this gate, including historical records.
    /// A legacy zero-error completed round also proves its ending was correct.
    pub fn can_send_score(&self) -> bool {
        self.chars > 0
            && self.round.as_ref().is_some_and(|round| {
                round.complete && round.last_char_correct.unwrap_or(self.wrong == 0)
            })
    }

    /// 结算构造（产线路径）：accuracy **直接取晴精确式** accuracy_pro（消除新增调用点
    /// 静默回退简化式的路径——简化对照式在 SegmentStats.accuracy_simple，不进本结构）。
    /// `difficulty` 为载文链难度（Session 侧持有；无则 None）。
    #[allow(clippy::too_many_arguments)]
    pub fn from_stats(
        stats: &SegmentStats,
        title: impl Into<String>,
        source: impl Into<String>,
        mark: impl Into<String>,
        para_index: usize,
        ts_unix: u64,
        difficulty: Option<DifficultyTag>,
    ) -> Self {
        let date = unix_to_date(ts_unix);
        Self {
            round: None,
            ts: ts_unix,
            date,
            title: title.into(),
            source: source.into(),
            mark: mark.into(),
            para_index,
            chars: stats.chars,
            keys: stats.keys,
            backspaces: stats.backspaces,
            wrong: stats.wrong,
            time_ms: stats.elapsed_ms,
            speed: stats.speed,
            accuracy: stats.accuracy_pro,
            code_len: stats.code_len,
            corrections: stats.corrections,
            effective_speed: stats.effective_speed,
            strokes_per_sec: stats.strokes_per_sec,
            composing_keys: stats.composing_keys,
            accuracy_pro: stats.accuracy_pro,
            waste_codes: stats.waste_codes,
            commit_groups: stats.commit_groups,
            ding_count: stats.ding_count,
            select_count: stats.select_count,
            word_rate: None,
            retype_count: 0,
            input_method: String::new(),
            time_breakdown: stats.time_breakdown,
            jj: stats.jj,
            is_valid: stats.is_valid,
            difficulty,
            version: default_version(),
        }
    }

    /// 晴发文 leaderboard 字段视图（字段名对齐 qingfawen API，上报免转换）。
    /// speed=有效速度（错一罚五），keystroke=击键，code_length=码长，
    /// key_accuracy=键准%，word_rate=打词率（gale 未采时 None）。
    pub fn to_leaderboard(&self) -> LeaderboardScore {
        LeaderboardScore {
            speed: self.effective_speed,
            raw_speed: self.speed,
            keystroke: self.strokes_per_sec,
            code_length: self.code_len,
            key_accuracy: self.accuracy,
            wrong: self.wrong as u64,
            word_count: self.chars as u64,
            time_ms: self.time_ms,
            total_keys: self.keys as u64,
            backspaces: self.backspaces as u64,
            corrections: self.corrections as u64,
            input_method: self.input_method.clone(),
            mark: self.mark.clone(),
        }
    }

    /// Text accuracy measures remaining mismatches independently of key efficiency.
    pub fn character_accuracy(&self) -> f64 {
        if self.chars == 0 {
            return 0.0;
        }
        self.chars.saturating_sub(self.wrong) as f64 / self.chars as f64 * 100.0
    }
}

/// 晴发文（qingfawen）leaderboard 成绩载荷——字段名与 API 对齐（上报免转换）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaderboardScore {
    /// 速度（有效，错一罚五）字/分
    pub speed: f64,
    /// 裸速度 字/分
    pub raw_speed: f64,
    /// 击键 键/秒
    pub keystroke: f64,
    /// 码长 键/字
    pub code_length: f64,
    /// 键准 %（0-100）
    pub key_accuracy: f64,
    /// 错字数
    pub wrong: u64,
    /// 字数
    pub word_count: u64,
    /// 用时 ms
    pub time_ms: u64,
    /// 总键数
    pub total_keys: u64,
    /// 退格
    pub backspaces: u64,
    /// 回改
    pub corrections: u64,
    /// 输入法名（如 "五笔""虎码"）
    pub input_method: String,
    /// 段标记（文来 mark）
    pub mark: String,
}

/// unix 秒 → 本地日期字符串（无 chrono：用纯手写换算，测试覆盖）。
/// 日期按东八区（跟打器用户群全部为中国时区；疾风1代记录即按北京时间写）。
/// （P2 今日汇总接线公开：main.rs 用它把当天 records 过滤进 day_summary。）
pub fn unix_to_date(ts: u64) -> String {
    // days since epoch（+8h 偏移到东八区）→ civil date（Howard Hinnant 算法）
    let days = ((ts + 8 * 3600) / 86400) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 赛制标记（成绩单 / 赛报 / UI 面板统一取用，避免 core 与 secondary_panels 各写一份漂移）：
/// 赛制 → `【赛制】`；练习 → `【非赛制】`，且开了词提再附 `[词提开]`
/// （虎魄写法，research-digest §B：带提示的成绩必须可一眼甄别）。
/// 赛制模式下 word_hints 恒 false（SessionRules::normalized 强制），mode 优先。
pub fn race_badge(mode: crate::rules::TrainingMode, word_hints: bool) -> &'static str {
    use crate::rules::TrainingMode;
    match mode {
        TrainingMode::Race => "【赛制】",
        TrainingMode::Practice if word_hints => "【非赛制】[词提开]",
        TrainingMode::Practice => "【非赛制】",
    }
}

/// 成绩单（发成绩用，对齐晴跟打 Pro 报告串逐字节口径，research-digest.md §C.3/C.4）：
/// - 各项**双空格**连接、标签与数值间无空格（`击键5.12`）、末项恒为签名（APP_NAME）；
/// - 速度：无错字 `速度{F2}`；有错字 `速度{罚后}/{裸速}` 双数字——**无数值括注**
///   （晴 ParseReportLine/社群脚本按 token 解析，括注会错位，P1-1）；
/// - 字准**不进成绩单**（晴字段集无此项，P1-2；字准留在 character_accuracy()/UI）；
/// - 废码紧跟退格（晴默认成绩顺序 Score.cs:254：…退格,废码,选重…）；
/// - 选重/标顶紧跟废码（标顶位序为推断——晴样本无标顶字段，按同伴项放选重后）；
/// - 难度（有数据时）紧跟字数（样本 `难度普(1.84)`）；无数据不输出该字段；
/// - **不含尾换行**：单条复制与整场赛报逐行拼接（消费方加 '\n'）都不会出现空行（P1-5）。
pub fn format_score_report(r: &ScoreRecord) -> String {
    let mut s = String::new();
    // 段号项：文来 mark 含 "-"（如 "2-277101"）时晴 Pro 用 "段{mark}"，否则 "第{mark}段"
    let label = if r.mark.contains('-') {
        format!("段{}", r.mark)
    } else {
        format!("第{}段", r.mark)
    };
    let _ = write!(s, "{label}");
    // 速度：有错字时晴 Pro 输出 "罚后/裸速" 双数字（无括注）
    if r.wrong > 0 {
        let _ = write!(s, "  速度{:.2}/{:.2}", r.effective_speed, r.speed);
    } else {
        let _ = write!(s, "  速度{:.2}", r.speed);
    }
    // 晴样例无内空格："击键5.12"（E2 勘误：去掉标签后空格与"键/秒"后缀）
    let _ = write!(s, "  击键{:.2}", r.strokes_per_sec);
    let _ = write!(s, "  键准{:.2}%", r.accuracy);
    // 码长两位小数（jifeng 样例 "码长2.00"）
    let _ = write!(s, "  码长{:.2}", r.code_len);
    let _ = write!(s, "  字数{}", r.chars);
    // 难度（有数据才输出；样本序位：字数之后）
    if let Some(d) = &r.difficulty {
        match d.score {
            Some(v) => {
                let _ = write!(s, "  难度{}({:.2})", d.label, v);
            }
            None => {
                let _ = write!(s, "  难度{}", d.label);
            }
        }
    }
    // 用时裁到厘秒（jifeng 样例 "用时1:23.45"）
    let _ = write!(s, "  用时{}", fmt_duration(r.time_ms));
    let _ = write!(s, "  错字{}", r.wrong);
    let _ = write!(s, "  退格{}", r.backspaces);
    // 废码（晴默认顺序紧跟退格；数据段 SegmentStats.waste_codes 罚账见 stats.rs）
    let _ = write!(s, "  废码{}", r.waste_codes);
    // 选重/标顶（P1-3 20ms 配对账）：晴顺序 …退格,废码,选重…（Score.cs:254）；
    // 标顶样本未见独立字段——按选重同伴项紧跟选重输出【推断位，2026-11-08】。
    let _ = write!(s, "  选重{}", r.select_count);
    let _ = write!(s, "  标顶{}", r.ding_count);
    let _ = write!(s, "  回改{}", r.corrections);
    let _ = write!(s, "  总键数{}", r.keys);
    if let Some(meta) = &r.round {
        let _ = write!(s, "  {}", race_badge(meta.mode, meta.word_hints));
        if !meta.complete {
            let _ = write!(s, "·未完成");
        }
        if meta.cleaned {
            let _ = write!(s, "·清洗");
        }
        if meta.pasted {
            let _ = write!(s, "·粘贴");
        }
        // Quote tolerance is an explicitly requested common rule; preserve the group's
        // score template while keeping the exact relaxed count in the local record.
    }
    let _ = write!(s, "  {}", crate::APP_NAME);
    s
}

/// 用时（晴 trim_timespan 口径，typesunny-stats.md:211）：
/// TimeSpan 文本裁到厘秒（去小数第3位起，**截断不四舍五入**）→ 去前导 "00:"。
/// 83.45s → "1:23.45"；723.45s → "12:03.45"；1h+ → "1:02:03.45"。
pub(crate) fn fmt_duration(ms: u64) -> String {
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let sec = (ms % 60_000) / 1000;
    let cs = (ms % 1000) / 10; // 厘秒（截断）
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}.{cs:02}")
    } else {
        format!("{m}:{sec:02}.{cs:02}") // m<10 自然无前导零，对齐晴"去前导 00:"
    }
}

/// records.jsonl 仓库（便携式，exe 旁 统计/records.jsonl）
pub struct ScoreStore {
    path: PathBuf,
}

impl ScoreStore {
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            path: dir.join("records.jsonl"),
        })
    }

    pub fn append(&self, r: &ScoreRecord) -> std::io::Result<()> {
        append_json_line(&self.path, r)
    }

    /// 读取全部（跳过坏行；兼容疾风1代旧记录：缺省字段用 serde default 补齐）
    pub fn load_all(&self) -> Vec<ScoreRecord> {
        use std::io::{BufRead, BufReader, Read};
        let Ok(file) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        let Ok(metadata) = file.metadata() else {
            return Vec::new();
        };
        // Read a fixed snapshot of the file, not an ever-growing concurrent append.
        // A partial UTF-8 scalar can damage one line without hiding older history.
        let mut reader = BufReader::with_capacity(64 * 1024, file.take(metadata.len()));
        let mut records = Vec::new();
        let mut line = Vec::new();
        let mut oversized = false;
        const MAX_SCORE_LINE: usize = 1024 * 1024;
        loop {
            let Ok(bytes) = reader.fill_buf() else { break };
            if bytes.is_empty() {
                break;
            }
            let newline = bytes.iter().position(|&b| b == b'\n');
            let used = newline.map_or(bytes.len(), |i| i + 1);
            let body_len = newline.unwrap_or(bytes.len());
            if !oversized {
                if line.len().saturating_add(body_len) <= MAX_SCORE_LINE {
                    line.extend_from_slice(&bytes[..body_len]);
                } else {
                    // Drain the rest of this damaged row with fixed memory, then
                    // continue at the next record rather than truncating history.
                    line.clear();
                    oversized = true;
                }
            }
            reader.consume(used);
            if newline.is_some() {
                if !oversized {
                    if let Ok(record) = serde_json::from_slice(&line) {
                        records.push(record);
                    }
                }
                line.clear();
                oversized = false;
            }
        }
        if !oversized && !line.is_empty() {
            if let Ok(record) = serde_json::from_slice(&line) {
                records.push(record);
            }
        }
        records
    }

    /// 读取**有效**记录（E4：字数≥5 才入统计汇总——晴 MinWordCount=5，typesunny-stats.md:261
    /// ArticleLog.cs:18/:312）。原始 records.jsonl 不拒写：append 照收，仅汇总层过滤。
    pub fn load_valid(&self) -> Vec<ScoreRecord> {
        self.load_all()
            .into_iter()
            .filter(|r| r.chars >= VALID_MIN_CHARS)
            .collect()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Durable single-writer journal. A truncated last line from a power loss is isolated
/// before appending the next record, so one damaged record cannot consume its successor.
pub fn append_json_line(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let _guard = JOURNAL_WRITE_LOCK
        .lock()
        .map_err(|_| std::io::Error::other("记录写入锁异常"))?;
    use std::io::{Read, Seek, SeekFrom, Write};
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(path)?;
    if file.metadata()?.len() > 0 {
        file.seek(SeekFrom::End(-1))?;
        let mut last = [0u8];
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            file.write_all(b"\n")?;
        }
    }
    file.write_all(&line)?;
    file.write_all(b"\n")?;
    file.sync_data()
}

pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".bak");
    PathBuf::from(name)
}
/// Portable replace with a synced staging file and one recoverable previous copy.
/// Readers may use `.bak` if interrupted between the two renames on Windows.
pub fn durable_replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let _guard = FILE_REPLACE_LOCK
        .lock()
        .map_err(|_| std::io::Error::other("配置写入锁异常"))?;
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".tmp-{}", std::process::id()));
    let staged = PathBuf::from(name);
    let mut file = std::fs::File::create(&staged)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    // A fixed .bak can be held by an editor/AV/another agent; when that
    // happens the old code failed the whole save with "Access denied".
    // Use a per-process backup name if the canonical one cannot be replaced,
    // and fall back to an in-place write if the new file cannot be renamed in.
    let canonical = backup_path(path);
    let backup = if !canonical.exists() {
        canonical
    } else if std::fs::remove_file(&canonical).is_ok() {
        canonical
    } else {
        let mut name = path.as_os_str().to_os_string();
        name.push(format!(".bak-{}", std::process::id()));
        PathBuf::from(name)
    };
    let had_original = path.exists();
    let moved = if had_original {
        std::fs::rename(path, &backup).is_ok()
    } else {
        true
    };
    if moved {
        match std::fs::rename(&staged, path) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if !path.exists() && backup.exists() {
                    let _ = std::fs::rename(&backup, path);
                }
                // Some filter drivers deny rename-over but allow truncate.
                if let Ok(mut direct) = std::fs::File::create(path) {
                    if direct.write_all(bytes).is_ok() && direct.sync_all().is_ok() {
                        let _ = std::fs::remove_file(&staged);
                        return Ok(());
                    }
                }
                return Err(error);
            }
        }
    }
    // Rename to backup failed (locked backup or target). Try a direct write.
    if let Ok(mut direct) = std::fs::File::create(path) {
        if direct.write_all(bytes).is_ok() && direct.sync_all().is_ok() {
            let _ = std::fs::remove_file(&staged);
            return Ok(());
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        format!("无法写入 {}（目标与备份均被占用）", path.display()),
    ))
}

/// 统计有效性门槛（E4）：字数 <5 的段不入汇总（测试段/误触段），原始记录保留。
pub const VALID_MIN_CHARS: usize = 5;

/// 有效成绩双门槛（晴口径，P1-4）：字数≥10 且 用时≥10s 才算"有效段"
/// （进有效均速/均键准类指标）。与 VALID_MIN_CHARS 硬门语义分开：
/// 硬门 = 直接不进任何聚合；is_valid = 进练习字数、不进"有效"指标（UI 消费）。
pub const VALID_EFFECTIVE_CHARS: usize = 10;
pub const VALID_EFFECTIVE_MS: u64 = 10_000;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::Comparer;
    use crate::stats::StatsEngine;

    fn sample_record() -> ScoreRecord {
        let mut st = StatsEngine::new();
        let mut cmp = Comparer::new(&"字".repeat(196));
        for _ in 0..202 {
            st.on_key(&crate::stats::KeyEvent {
                vk: 0x41,
                composing: false,
            });
        }
        for _ in 0..6 {
            st.on_key(&crate::stats::KeyEvent {
                vk: 0x08,
                composing: false,
            });
        }
        cmp.wrong = 2;
        cmp.backspaces = 6;
        // 该样例回改=3（疾风1代真实记录），直接设
        cmp.corrections = 3;
        cmp.pos = cmp.states.len();
        st.composition_backspaces = cmp.backspaces as u32;
        let s = st.finalize(&cmp, 58_285);
        ScoreRecord::from_stats(&s, "听的能力", "群文", "999", 0, 1_790_555_080, None)
    }

    #[test]
    fn score_sending_requires_proven_completion_and_correct_ending() {
        let mut r = sample_record();
        assert!(
            !r.can_send_score(),
            "legacy records with no completion metadata are unproven"
        );
        r.round = Some(crate::rules::RoundMetadata {
            complete: true,
            last_char_correct: Some(true),
            ..Default::default()
        });
        assert!(r.can_send_score(), "earlier wrong characters are allowed");
        r.round.as_mut().unwrap().last_char_correct = Some(false);
        assert!(!r.can_send_score());
        r.wrong = 0;
        assert!(
            !r.can_send_score(),
            "explicit ending evidence takes precedence"
        );
        r.round.as_mut().unwrap().last_char_correct = Some(true);
        r.round.as_mut().unwrap().complete = false;
        assert!(!r.can_send_score(), "a correct partial suffix is not completion");
    }

    #[test]
    fn legacy_history_only_allows_completed_zero_error_scores() {
        let mut r = sample_record();
        r.round = Some(crate::rules::RoundMetadata {
            complete: true,
            ..Default::default()
        });
        let mut old = serde_json::to_value(&r).unwrap();
        old["round"]
            .as_object_mut()
            .unwrap()
            .remove("last_char_correct");
        let mut history: ScoreRecord = serde_json::from_value(old).unwrap();
        assert!(
            !history.can_send_score(),
            "legacy errors do not prove the ending"
        );
        history.wrong = 0;
        assert!(
            history.can_send_score(),
            "complete plus zero errors proves the ending"
        );
        history.round.as_mut().unwrap().complete = false;
        assert!(!history.can_send_score(), "Esc records remain ineligible");
    }

    /// P1-1/P1-2/P1-5 + 全串等值：有错字 → 罚后/裸速双数字、无括注、无「字准」、单行无尾换行。
    /// 键准走产线精确式：(208−3−6×2)/(196+3)×196/208 = 91.39%。
    #[test]
    fn report_full_string_with_wrong_dual_speed_no_parenthetical() {
        let r = sample_record();
        let text = format_score_report(&r);
        assert_eq!(
            text,
            "第999段  速度191.47/201.77  击键3.57  键准91.39%  码长1.06  \
             字数196  用时0:58.28  错字2  退格6  废码0  选重0  标顶0  回改3  总键数208  虎符跟打"
        );
        // 协议硬断言：无数值括注（晴 ParseReportLine 按 token 解析）、无字准字段
        assert!(!text.contains('（'));
        assert!(!text.contains("字准"));
        // P1-5：单行、无尾换行（消费方自行加 '\n'）
        assert!(!text.contains('\n'));
        assert!(text.ends_with(crate::APP_NAME));

        // 用时样式变体（>10min / ≥1h）仍为全串等值：其余字段为结算快照值，只有用时变
        let mut r10 = sample_record();
        r10.time_ms = 723_456;
        assert_eq!(
            format_score_report(&r10),
            "第999段  速度191.47/201.77  击键3.57  键准91.39%  码长1.06  \
             字数196  用时12:03.45  错字2  退格6  废码0  选重0  标顶0  回改3  总键数208  虎符跟打"
        );
        let mut rh = sample_record();
        rh.time_ms = 3_723_456;
        assert_eq!(
            format_score_report(&rh),
            "第999段  速度191.47/201.77  击键3.57  键准91.39%  码长1.06  \
             字数196  用时1:02:03.45  错字2  退格6  废码0  选重0  标顶0  回改3  总键数208  虎符跟打"
        );
    }

    /// 全串等值：全对（无错字）→ 速度单数字；文来 mark 含 "-" 用 "段2-277101" 格式。
    #[test]
    fn report_full_string_no_wrong_single_speed() {
        let mut st = StatsEngine::new();
        let mut cmp = Comparer::new("十个字十个字十个");
        cmp.pos = cmp.states.len(); // 8 字全打完（全对）
        for _ in 0..14 {
            st.on_key(&crate::stats::KeyEvent {
                vk: 0x41,
                composing: false,
            });
        }
        let s = st.finalize(&cmp, 10_000);
        let r = ScoreRecord::from_stats(&s, "文来", "文来", "2-277101", 0, 1_790_555_080, None);
        assert_eq!(
            format_score_report(&r),
            "段2-277101  速度48.00  击键1.40  键准100.00%  码长1.75  \
             字数8  用时0:10.00  错字0  退格0  废码0  选重0  标顶0  回改0  总键数14  虎符跟打"
        );
    }

    /// 赛制标记三态（P1-3）：【赛制】/【非赛制】/【非赛制】[词提开]；后缀 ·未完成·清洗·粘贴 保持原样。
    #[test]
    fn report_full_string_race_badges() {
        fn meta(
            mode: crate::rules::TrainingMode,
            word_hints: bool,
            complete: bool,
        ) -> crate::rules::RoundMetadata {
            crate::rules::RoundMetadata {
                mode,
                text_hash: String::new(),
                raw_text_hash: String::new(),
                rules_hash: String::new(),
                quote_relaxed: 0,
                cleaned: false,
                pasted: false,
                complete,
                last_char_correct: None,
                word_hints,
            }
        }
        use crate::rules::TrainingMode;
        let mut r = sample_record();
        // 赛制（word_hints 快照即便为 true 也被 mode 压制）
        r.round = Some(meta(TrainingMode::Race, true, true));
        assert!(format_score_report(&r).ends_with("总键数208  【赛制】  虎符跟打"));
        // 练习 + 词提关
        r.round = Some(meta(TrainingMode::Practice, false, true));
        assert!(format_score_report(&r).ends_with("总键数208  【非赛制】  虎符跟打"));
        // 练习 + 词提开
        r.round = Some(meta(TrainingMode::Practice, true, true));
        assert!(format_score_report(&r).ends_with("总键数208  【非赛制】[词提开]  虎符跟打"));
        // 练习 + 词提开 + 未完成 + 清洗 + 粘贴（后缀直接接在 badge 后）
        r.round = Some(meta(TrainingMode::Practice, true, false));
        r.round.as_mut().unwrap().cleaned = true;
        r.round.as_mut().unwrap().pasted = true;
        assert!(format_score_report(&r)
            .ends_with("总键数208  【非赛制】[词提开]·未完成·清洗·粘贴  虎符跟打"));
        // race_badge 单点取用（secondary_panels 同款标记走这里，防两边漂移）
        assert_eq!(race_badge(TrainingMode::Race, false), "【赛制】");
        assert_eq!(
            race_badge(TrainingMode::Practice, true),
            "【非赛制】[词提开]"
        );
        assert_eq!(race_badge(TrainingMode::Practice, false), "【非赛制】");
    }

    /// 选重/标顶（P1-3 20ms 配对账进单）：注入 Ding×2 + Select×1 →
    /// 成绩单含 `选重1  标顶2`，序位=废码之后、回改之前；无数据恒输出 0（解析器稳定）。
    #[test]
    fn report_full_string_select_and_ding() {
        let mut r = sample_record();
        r.select_count = 1;
        r.ding_count = 2;
        assert!(
            format_score_report(&r).contains("废码0  选重1  标顶2  回改3  总键数208"),
            "{}",
            format_score_report(&r)
        );
        // 缺省（旧记录/未配对）恒输出双 0，字段位置稳定不漂移
        let zero = sample_record();
        assert!(format_score_report(&zero).contains("退格6  废码0  选重0  标顶0  回改3"));
    }

    /// 废码（P1 补字段，序位紧跟退格）与难度（有数据时紧跟字数，样本 `难度普(1.84)`）。
    #[test]
    fn report_full_string_waste_codes_and_difficulty() {
        let mut r = sample_record();
        r.waste_codes = 3;
        assert_eq!(
            format_score_report(&r),
            "第999段  速度191.47/201.77  击键3.57  键准91.39%  码长1.06  \
             字数196  用时0:58.28  错字2  退格6  废码3  选重0  标顶0  回改3  总键数208  虎符跟打"
        );
        // 难度带数值分（群文《难度》头 "普 1.84"）
        r.difficulty = Some(DifficultyTag::parse("普 1.84").unwrap());
        assert_eq!(
            format_score_report(&r),
            "第999段  速度191.47/201.77  击键3.57  键准91.39%  码长1.06  \
             字数196  难度普(1.84)  用时0:58.28  错字2  退格6  废码3  选重0  标顶0  回改3  总键数208  虎符跟打"
        );
        // 文来路径只有标签（main 侧只传 label）：`难度普`，无数值括号
        r.difficulty = Some(DifficultyTag::parse("普").unwrap());
        assert!(format_score_report(&r).contains("字数196  难度普  用时"));
        // DifficultyTag 解析边界
        assert!(DifficultyTag::parse("").is_none());
        assert!(DifficultyTag::parse("   ").is_none());
        assert_eq!(
            DifficultyTag::parse("普 1.84"),
            Some(DifficultyTag {
                label: "普".into(),
                score: Some(1.84)
            })
        );
        assert_eq!(
            DifficultyTag::parse("难"),
            Some(DifficultyTag {
                label: "难".into(),
                score: None
            })
        );
    }

    /// P2 键准下沉：from_stats 的 accuracy 直接是晴精确式（产线），无简化式回退路径。
    #[test]
    fn from_stats_uses_pro_accuracy_as_production_value() {
        let r = sample_record();
        // 精确式手算：(208−3−12)/(196+3)×196/208 = 91.3896…
        assert!((r.accuracy - 91.3896).abs() < 0.01, "{}", r.accuracy);
        assert_eq!(r.accuracy, r.accuracy_pro);
        // 与疾风1代简化式（94.71）和字准（98.98）明确区分
        assert!((r.accuracy - 94.71).abs() > 1.0);
        assert!((r.accuracy - r.character_accuracy()).abs() > 1.0);
        // 有效性：196 字 / 58.285s → 有效段
        assert!(r.is_valid);
        assert!(!r.difficulty.is_some());
    }

    #[test]
    fn journal_truncated_tail_cannot_consume_next_score() {
        let dir = crate::test_util::tmp("truncated-score");
        let _ = std::fs::remove_dir_all(&dir);
        let store = ScoreStore::open(&dir).unwrap();
        store.append(&sample_record()).unwrap();
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(store.path())
            .unwrap();
        file.write_all(b"{\"ts\":123").unwrap();
        drop(file);
        store.append(&sample_record()).unwrap();
        assert_eq!(store.load_all().len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_utf8_and_oversized_damage_do_not_hide_valid_history() {
        let dir = crate::test_util::tmp("score-utf8-damage");
        let store = ScoreStore::open(&dir).unwrap();
        let mut record = sample_record();
        store.append(&record).unwrap();
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(store.path())
            .unwrap();
        file.write_all(b"{\"title\":\"\xe4\xbd").unwrap();
        drop(file);
        assert_eq!(
            store.load_all().len(),
            1,
            "a torn UTF-8 tail must not hide the first record"
        );
        record.ts += 1;
        store.append(&record).unwrap();
        assert_eq!(
            store.load_all().len(),
            2,
            "next append isolates the damaged tail"
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(store.path())
            .unwrap();
        file.write_all(&vec![0xff; 1024 * 1024 + 1]).unwrap();
        file.write_all(b"\n").unwrap();
        drop(file);
        record.ts += 1;
        store.append(&record).unwrap();
        assert_eq!(
            store.load_all().len(),
            3,
            "oversized damage must not consume following records"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn leaderboard_view_fields() {
        let r = sample_record();
        let lb = r.to_leaderboard();
        // 晴发文 leaderboard 字段名对齐（上报免转换）
        assert!((lb.speed - 191.47).abs() < 0.05, "{}", lb.speed);
        assert!((lb.raw_speed - 201.77).abs() < 0.05);
        assert!((lb.keystroke - 3.57).abs() < 0.01, "{}", lb.keystroke);
        assert!((lb.code_length - 1.0612).abs() < 0.001);
        // 键准 = 产线精确式（from_stats 下沉后与 r.accuracy 同源）
        assert!(
            (lb.key_accuracy - 91.39).abs() < 0.05,
            "{}",
            lb.key_accuracy
        );
        assert_eq!(lb.key_accuracy, r.accuracy);
        assert_eq!(lb.wrong, 2);
        assert_eq!(lb.word_count, 196);
        assert_eq!(lb.total_keys, 208);
        assert_eq!(lb.backspaces, 6);
        // JSON 序列化含晴 API 字段名
        let j = serde_json::to_string(&lb).unwrap();
        assert!(j.contains("\"keystroke\""), "{j}");
        assert!(j.contains("\"code_length\""), "{j}");
        assert!(j.contains("\"key_accuracy\""), "{j}");
        assert!(j.contains("\"input_method\""), "{j}");
    }

    #[test]
    fn jsonl_roundtrip_and_legacy_compat() {
        let dir = crate::test_util::tmp("score");
        let store = ScoreStore::open(&dir).unwrap();
        store.append(&sample_record()).unwrap();
        // 疾风1代旧记录（真实字段，无 corrections 等）
        let legacy = r#"{"ts":1790555080,"date":"2026-09-28","title":"旧","source":"群文","mark":"999","para_index":0,"chars":196,"keys":208,"backspaces":6,"wrong":2,"time_ms":58285,"speed":201.76,"accuracy":94.71,"code_len":1.0612}"#;
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(store.path())
            .unwrap();
        writeln!(f, "{legacy}").unwrap();
        let all = store.load_all();
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].corrections, 0); // 旧记录缺省可读
        assert!(!all[1].is_valid); // 旧记录缺省 false
        assert!(all[1].difficulty.is_none());
        assert_eq!(all[0].version, "2");
        assert!(all[0].is_valid); // 196字/58s：有效段
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn valid_threshold_filters_short_segments() {
        // E4：字数<5 不入统计汇总（晴 MinWordCount=5），但 append 原始保留
        let dir = crate::test_util::tmp("score_valid");
        let store = ScoreStore::open(&dir).unwrap();
        store.append(&sample_record()).unwrap(); // chars=196，有效
        let mut tiny = sample_record();
        tiny.chars = 4; // 测试段
        store.append(&tiny).unwrap();
        let mut edge = sample_record();
        edge.chars = 5; // 恰好门槛：有效（晴 <= 边界为 TotalWords<5 丢，即 5 留）
        store.append(&edge).unwrap();
        assert_eq!(store.load_all().len(), 3); // 原始全保留
        let valid = store.load_valid();
        assert_eq!(valid.len(), 2); // 只滤掉 <5
        assert!(valid.iter().all(|r| r.chars >= 5));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unix_date_math() {
        assert_eq!(unix_to_date(0), "1970-01-01");
        assert_eq!(unix_to_date(1_790_555_080), "2026-09-28"); // 疾风1代记录同日
        assert_eq!(unix_to_date(1_709_164_800), "2024-02-29"); // 闰日
                                                               // 世纪年 2100 不闰：2月只有28天
        assert_eq!(unix_to_date(4_102_444_800), "2100-01-01"); // 2100-01-01 00:00 UTC
        assert_eq!(unix_to_date(4_107_542_400), "2100-03-01"); // 2100-02-28 的次日
        assert_eq!(unix_to_date(1_735_689_599), "2025-01-01"); // 2024-12-31 23:59:59 UTC → +8h = 东八区元旦
        assert_eq!(unix_to_date(1_735_660_799), "2024-12-31"); // 东八区 31 日 23:59:59
        assert_eq!(unix_to_date(1_735_660_800), "2025-01-01"); // 东八区元旦 00:00:00 整
    }
}
