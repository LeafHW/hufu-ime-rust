//! 成绩单格式化 + records.jsonl 持久化。
//!
//! 成绩单格式对齐晴跟打 Pro 的 Score.Report 风格（疾风1代 README:21 记载的字段集合）：
//! 速度/有效速度/击键/键准/码长/字数/用时/错字/退格/回改/总键数。
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
    /// 输入法名（晴发文 leaderboard 字段 input_method；由输入层在结算时注入）
    #[serde(default)]
    pub input_method: String,
    /// 州州四分解（正常/卡顿/回改/错罚 秒 + avgPos + 理想速度）
    #[serde(default)]
    pub time_breakdown: crate::stats::TimeBreakdown,
    /// 州州击键速度 jj（键/秒）——jjC 九级评定的历史输入
    #[serde(default)]
    pub jj: f64,
    /// 版本（区分1代/2代记录）
    #[serde(default = "default_version")]
    pub version: String,
}

fn default_version() -> String {
    "2".into()
}

impl ScoreRecord {
    pub fn from_stats(
        stats: &SegmentStats,
        title: impl Into<String>,
        source: impl Into<String>,
        mark: impl Into<String>,
        para_index: usize,
        ts_unix: u64,
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
            accuracy: stats.accuracy,
            code_len: stats.code_len,
            corrections: stats.corrections,
            effective_speed: stats.effective_speed,
            strokes_per_sec: stats.strokes_per_sec,
            composing_keys: stats.composing_keys,
            accuracy_pro: stats.accuracy_pro,
            waste_codes: stats.waste_codes,
            commit_groups: stats.commit_groups,
            input_method: String::new(),
            time_breakdown: stats.time_breakdown,
            jj: stats.jj,
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
fn unix_to_date(ts: u64) -> String {
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

/// 成绩单（发成绩用，对齐晴跟打 Pro 报告串字段与双空格分隔）
pub fn format_score_report(r: &ScoreRecord) -> String {
    let mut s = String::new();
    // 段号项：文来 mark 含 "-"（如 "2-277101"）时晴 Pro 用 "段{mark}"，否则 "第{mark}段"
    let label = if r.mark.contains('-') {
        format!("段{}", r.mark)
    } else {
        format!("第{}段", r.mark)
    };
    let _ = write!(s, "{label}");
    // 速度：有错字时晴 Pro 输出 "罚后/裸速" 双数字
    if r.wrong > 0 {
        let _ = write!(
            s,
            "  速度{:.2}/{:.2}（有效/裸速，错一罚五）",
            r.effective_speed, r.speed
        );
    } else {
        let _ = write!(s, "  速度{:.2}", r.speed);
    }
    // 晴样例无内空格："击键5.12"（E2 勘误：去掉标签后空格与"键/秒"后缀）
    let _ = write!(s, "  击键{:.2}", r.strokes_per_sec);
    let _ = write!(s, "  键准{:.2}%", r.accuracy);
    let _ = write!(s, "  字准{:.2}%", r.character_accuracy());
    // 码长两位小数（jifeng 样例 "码长2.00"）
    let _ = write!(s, "  码长{:.2}", r.code_len);
    let _ = write!(s, "  字数{}", r.chars);
    // 用时裁到厘秒（jifeng 样例 "用时1:23.45"）
    let _ = write!(s, "  用时{}", fmt_duration(r.time_ms));
    let _ = write!(s, "  错字{}", r.wrong);
    let _ = write!(s, "  退格{}", r.backspaces);
    let _ = write!(s, "  回改{}", r.corrections);
    let _ = write!(s, "  总键数{}", r.keys);
    if let Some(meta) = &r.round {
        let _ = write!(
            s,
            "  {}",
            if meta.mode == crate::rules::TrainingMode::Race {
                "赛制"
            } else {
                "练习"
            }
        );
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
    let _ = writeln!(s, "  {}", crate::APP_NAME);
    s
}

/// 用时（晴 trim_timespan 口径，typesunny-stats.md:211）：
/// TimeSpan 文本裁到厘秒（去小数第3位起，**截断不四舍五入**）→ 去前导 "00:"。
/// 83.45s → "1:23.45"；723.45s → "12:03.45"；1h+ → "1:02:03.45"。
fn fmt_duration(ms: u64) -> String {
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
    let backup = backup_path(path);
    if path.exists() {
        if backup.exists() {
            std::fs::remove_file(&backup)?;
        }
        std::fs::rename(path, &backup)?;
    }
    if let Err(error) = std::fs::rename(&staged, path) {
        if !path.exists() && backup.exists() {
            let _ = std::fs::rename(&backup, path);
        }
        return Err(error);
    }
    Ok(())
}

/// 统计有效性门槛（E4）：字数 <5 的段不入汇总（测试段/误触段），原始记录保留。
pub const VALID_MIN_CHARS: usize = 5;

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
        ScoreRecord::from_stats(&s, "听的能力", "群文", "999", 0, 1_790_555_080)
    }

    #[test]
    fn report_format() {
        let r = sample_record();
        let text = format_score_report(&r);
        // 晴 Pro 双空格分隔 + 罚后/裸速双数字（有错字时）
        assert!(text.contains("第999段"), "{text}");
        assert!(text.contains("速度191.47/201.77"), "{text}");
        // E2 勘误：标签后无内空格、击键无"键/秒"后缀（晴样例 "击键5.12"）
        assert!(text.contains("击键3.57"), "{text}");
        assert!(text.contains("键准94.71%"), "{text}");
        assert!(text.contains("字准98.98%"), "{text}");
        assert!(text.contains("有效/裸速，错一罚五"), "{text}");
        // 码长两位小数（jifeng 样例 "码长2.00"）
        assert!(text.contains("码长1.06"), "{text}");
        assert!(text.contains("字数196"), "{text}");
        assert!(text.contains("错字2"), "{text}");
        assert!(text.contains("退格6"), "{text}");
        assert!(text.contains("回改3"), "{text}");
        assert!(text.contains("总键数208"), "{text}");
        // 用时厘秒截断（晴 trim_timespan）：58285ms → 0:58.28
        assert!(text.contains("用时0:58.28"), "{text}");
        // >10min 样式：723_456ms → 12:03.45
        let mut r10 = sample_record();
        r10.time_ms = 723_456;
        let t10 = format_score_report(&r10);
        assert!(t10.contains("用时12:03.45"), "{t10}");
        // ≥1h 样式：3_723_456ms → 1:02:03.45
        let mut rh = sample_record();
        rh.time_ms = 3_723_456;
        let th = format_score_report(&rh);
        assert!(th.contains("用时1:02:03.45"), "{th}");
        // 双空格分隔（晴 Pro 口径）
        assert!(text.contains("  速度"), "{text}");
        assert!(text.contains("  键准"), "{text}");
        // 不再含偏离格式
        assert!(!text.contains("键/秒"), "{text}");
        assert!(!text.contains("码长 1."), "{text}");
        let mut first_wrong = r;
        first_wrong.chars = 1;
        first_wrong.wrong = 1;
        first_wrong.accuracy = 100.0;
        let text = format_score_report(&first_wrong);
        assert!(text.contains("键准100.00%  字准0.00%"), "{text}");
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
    fn report_format_no_wrong_single_speed() {
        // 无错字：速度单数字；文来段号 mark 含 "-" 用 "段2-277101" 格式
        let mut st = StatsEngine::new();
        let cmp = Comparer::new("十个字十个字十个");
        for _ in 0..14 {
            st.on_key(&crate::stats::KeyEvent {
                vk: 0x41,
                composing: false,
            });
        }
        let s = st.finalize(&cmp, 10_000);
        let r = ScoreRecord::from_stats(&s, "文来", "文来", "2-277101", 0, 1_790_555_080);
        let text = format_score_report(&r);
        assert!(text.starts_with("段2-277101"), "{text}");
        // 无错字：速度是单数字（没有 "/"）
        let speed_zone = &text[text.find("速度").unwrap()..text.find("击键").unwrap()];
        assert!(!speed_zone.contains('/'), "{text}");
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
        assert!(
            (lb.key_accuracy - 94.71).abs() < 0.05,
            "{}",
            lb.key_accuracy
        );
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
        assert_eq!(all[0].version, "2");
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
