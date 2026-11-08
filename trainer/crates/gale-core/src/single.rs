//! 单字/单词练习 —— 兼容 TypeSunny/Pain 的 `练单器/*.txt`（每行一个字/词），
//! 内置州州语料（Apache-2.0），种子化乱序分组（社群 10 字/组口径），错字本（州州连对制）。
//!
//! ## 语料来源（license 必须保留）
//! ` drills()` 内置五件字表内容取自 zhouzhou-typing（州州跟打器）
//! `newgdq/Resources/TXT/`，**Apache License 2.0**（`_research\drill-corpus\` 存档，
//! README.md 有来源/许可/再分发说明）。再分发时在关于页/NOTICE 注明
//! "练习语料源自 zhouzhou-typing（Apache-2.0）"并保留其 NOTICE 内容。
//!
//! ## 课程模型（stats-and-drill-models.md §4.1 社群共识规格）
//! - 语料阶梯：前500 → 中500 → 后500（=1500 常用字），词组前三百进阶；
//! - 分组：10 字/组（每 500 字 = 50 组），组内**种子化乱序**（种子可复现）；
//! - 门槛：击键 ≥ 阈值 **且** 键准 100%（无回改）才算过关；不达标打乱重打本组；
//! - 阶梯阈值：新手 2.5→3→3.5→4；前500 击键 ≥6 解锁中500，中500 ≥6 解锁后500。
//!
//! ## 双拼课程三坑（州州 SimpleCharTable.cs 踩坑记录，gale 双拼练习照此口径）
//! - 零声母音节按 y/w 折算（如 i→yi、u→wu、ü→yu），不按字面首字母；
//! - 多音字白名单（PolyChars 16 字）固定取高频音，避免方案歧义；
//! - 排除 ü 单独成音节的字（各双拼方案处理不一，无法统一映射）。
//!
//! ## 错字本（州州 ErrorBookRepository 连对制，§4.3）
//! 双表：`error_log`（明细，typed/title 溯源）+ `char_stat`（聚合，streak 毕业制）。
//! **不用 FSRS/SM-2**：打字技能是速度维持不是回忆，连对 5 次毕业的工程裁定够用。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::segment::Segment;
use crate::GaleResult;

/// 毕业所需连续打对次数（州州 MasterStreak = 5）
pub const MASTER_STREAK: u32 = 5;
/// 巩固段目标字数（州州 BtnGenPractice_Click 以 ~80 字为段长基准）
const PRACTICE_TARGET_CHARS: usize = 80;
/// 巩固段取高频错字上限（州州前 30 字）
const PRACTICE_TOP_CHARS: usize = 30;

// ─────────────────────────────────────────────────────────────
// 单字练习配置（整改8：乱序开关 + 段长，持久化 sidecar JSON）
// ─────────────────────────────────────────────────────────────

/// 段长下限（含）
pub const SINGLE_SEGMENT_MIN: u16 = 5;
/// 段长上限（含）
pub const SINGLE_SEGMENT_MAX: u16 = 100;
/// 段长默认值
pub const SINGLE_SEGMENT_DEFAULT: u16 = 20;

/// 单字练习用户配置。config.toml 归 gale-app 主配置（本 crate 不持有），
/// 该小节独立持久化在数据目录 `single.json`（与 online.json 同一 sidecar 口径）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SingleDrillConfig {
    /// 词表乱序（组内种子化洗牌；关=原序）
    #[serde(default = "default_single_shuffle")]
    pub shuffle: bool,
    /// 每段字数（clamp 5..=100）
    #[serde(default = "default_single_segment_chars")]
    pub segment_chars: u16,
    #[serde(default = "default_target_keystroke")]
    pub target_keystroke_x10: u16,
}

fn default_target_keystroke() -> u16 {
    30
}

fn default_single_shuffle() -> bool {
    true
}
fn default_single_segment_chars() -> u16 {
    SINGLE_SEGMENT_DEFAULT
}

impl Default for SingleDrillConfig {
    fn default() -> Self {
        Self {
            shuffle: true,
            segment_chars: SINGLE_SEGMENT_DEFAULT,
            target_keystroke_x10: default_target_keystroke(),
        }
    }
}

impl SingleDrillConfig {
    /// 段长夹取到合法区间（洗牌开关原样保留）。
    pub fn clamped(&self) -> Self {
        Self {
            shuffle: self.shuffle,
            target_keystroke_x10: self.target_keystroke_x10.clamp(10, 120),
            segment_chars: self
                .segment_chars
                .clamp(SINGLE_SEGMENT_MIN, SINGLE_SEGMENT_MAX),
        }
    }
    /// 读取 sidecar 配置：缺失/损坏/越界一律回退（clamped 后）合法值，
    /// 与主配置 load_or_create 的宽容口径一致，不因坏文件阻断练习。
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            .map(|c| c.clamped())
            .unwrap_or_default()
    }
    /// 持久化 sidecar 配置（小 JSON，直接覆写）。
    pub fn save(&self, path: &Path) -> GaleResult<()> {
        let bytes =
            serde_json::to_vec(self).map_err(|e| crate::GaleError::Format(e.to_string()))?;
        if bytes.len() > 4096 {
            return Err(crate::GaleError::Format("单字配置过大".into()));
        }
        std::fs::write(path, bytes)?;
        Ok(())
    }
    /// 由字表文本构建一次练单会话：乱序开→按段长做组内种子化洗牌
    /// （同 seed 可复现，重打本段重现同一段）；切段 = segment_chars。
    pub fn build_session(
        &self,
        title: &str,
        body: &str,
        word_len: usize,
        seed: u64,
    ) -> SingleSession {
        let c = self.clamped();
        let group = c.segment_chars.max(1) as usize;
        let items = parse_items(body, word_len);
        let (items, seed) = if c.shuffle {
            (shuffle_grouped(&items, group, seed), seed)
        } else {
            (items, 0)
        };
        SingleSession {
            title: title.to_string(),
            group_size: group,
            items,
            group_index: 0,
            seed,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// 内置语料（zhouzhou-typing Apache-2.0；_research\drill-corpus\ 存档核对）
// ─────────────────────────────────────────────────────────────

/// 内置字表内容分隔符：州州 Resources/TXT 的字表是**连续字流**（无换行），
/// 载入时按字符拆成单字条目；词组表同为连续流（2 字词连写）→ 按词长切。
/// 兼容：每行一字/词的 TypeSunny 风格 txt 也照常（`parse_items` 自动识别）。
pub fn parse_items(text: &str, word_len: usize) -> Vec<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    // TypeSunny 风格：多行且每行 ≤ word_len 字 → 逐行作条目
    if lines.len() > 1 && lines.iter().all(|l| l.chars().count() <= word_len) {
        return lines.into_iter().map(str::to_string).collect();
    }
    // 州州风格：连续字流 → 按定长切
    let chars: Vec<char> = lines.join("").chars().collect();
    chars
        .chunks(word_len)
        .filter(|c| !c.is_empty())
        .map(|c| c.iter().collect())
        .collect()
}

/// 内置字表（title, 内容）。内容与 `_research\drill-corpus\` 五件文件一致（首尾已核对）。
/// 字表是连续字流（500 字无换行），用 `parse_items(text, 1)` 拆字；词组表按 2 字切。
pub fn builtin_tables() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "常用单字前五百",
            include_str!("../../../drills/常用单字前五百.txt"),
        ),
        (
            "常用单字中五百",
            include_str!("../../../drills/常用单字中五百.txt"),
        ),
        (
            "常用单字后五百",
            include_str!("../../../drills/常用单字后五百.txt"),
        ),
        ("前1500单字", include_str!("../../../drills/前1500单字.txt")),
        (
            "常用词组前三百",
            include_str!("../../../drills/常用词组前三百.txt"),
        ),
    ]
}

/// 把内置字表落盘到 `练单器/` 目录（文件不存在时写入；存在不覆盖——用户可改）。
/// 返回写入的文件数。
pub fn seed_builtin_tables(dir: &Path) -> usize {
    let mut n = 0;
    if std::fs::create_dir_all(dir).is_err() {
        return 0;
    }
    for (name, content) in builtin_tables() {
        let p = dir.join(format!("{name}.txt"));
        if !p.exists() {
            if std::fs::write(&p, content).is_ok() {
                n += 1;
            }
        }
    }
    n
}

// ─────────────────────────────────────────────────────────────
// 种子化乱序（可复现；对齐 pain-hyper "乱序+种子" 配置口径）
// ─────────────────────────────────────────────────────────────

/// xorshift64* —— 确定性伪随机（无依赖、种子可复现；非加密用途）。
fn xorshift64(mut x: u64) -> u64 {
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    x.wrapping_mul(0x2545F4914F6CDD1D)
}

/// 种子化乱序：把 `items` 分成 `group_size` 一组，组内按 seed 洗牌（组序不变）。
/// 同一 (items, group_size, seed) 结果完全一致——重打本组用同 seed 重现同一段。
pub fn shuffle_grouped(items: &[String], group_size: usize, seed: u64) -> Vec<String> {
    let group_size = group_size.max(1);
    let mut out = Vec::with_capacity(items.len());
    for chunk in items.chunks(group_size) {
        let mut g: Vec<(u64, &String)> = chunk
            .iter()
            .enumerate()
            .map(|(i, s)| {
                // 每组重置种：组间互不干扰，组序稳定
                let mut h = seed ^ ((i as u64).wrapping_mul(0x9E3779B97F4A7C15));
                h = xorshift64(h | 1); // |1 防全零种子
                (h, s)
            })
            .collect();
        g.sort_by_key(|(h, _)| *h);
        out.extend(g.into_iter().map(|(_, s)| s.clone()));
    }
    out
}

// ─────────────────────────────────────────────────────────────
// 练单会话
// ─────────────────────────────────────────────────────────────

/// 一次练单会话：把字表按组切分，每组 `group_size` 个字，
/// 打完一组出成绩（蓝本：TypeSunny 练单器"每N字一组循环"）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SingleSession {
    pub title: String,
    pub group_size: usize,
    /// 所有待练条目（字或词；乱序后）
    pub items: Vec<String>,
    /// 当前组序号
    pub group_index: usize,
    /// 乱序种子（0 = 原序不乱序）
    #[serde(default)]
    pub seed: u64,
}

impl SingleSession {
    /// 从 练单器/*.txt 读取：每行一字/词的 TypeSunny 风格、或连续字流的州州风格都识别
    /// （单字流按 1 字切、2 字词流按 2 字切——按内容最长行启发：全 ≤1 字且无换行结构时按 1 切）。
    pub fn load(path: &Path, group_size: usize) -> GaleResult<Self> {
        let text = crate::library::read_text_auto(path)?;
        let items = Self::parse_text(&text);
        let title = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        Ok(Self {
            title,
            group_size: group_size.max(1),
            items,
            group_index: 0,
            seed: 0,
        })
    }

    /// 文本 → 条目：TypeSunny 行风格优先；连续流按最长行长度切（1=单字、2=词），
    /// 单行连续流默认按 1 字切（单字表场景占绝大多数）。
    fn parse_text(text: &str) -> Vec<String> {
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        // 多行且行长一致 → 按行长切；单行连续流 → 1 字（调用方可显式用 parse_items 覆盖）
        if lines.len() > 1 {
            let max_len = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1);
            let min_len = lines.iter().map(|l| l.chars().count()).min().unwrap_or(1);
            if max_len == min_len || max_len <= 2 {
                return parse_items(text, max_len.max(1));
            }
            return lines.into_iter().map(str::to_string).collect();
        }
        parse_items(text, 1)
    }

    /// 带乱序种子构建（社群口径：10 字/组、组内种子化乱序、重打同 seed 复现）
    pub fn load_seeded(path: &Path, group_size: usize, seed: u64) -> GaleResult<Self> {
        let mut s = Self::load(path, group_size)?;
        if seed != 0 {
            s.items = shuffle_grouped(&s.items, s.group_size, seed);
        }
        s.seed = seed;
        Ok(s)
    }

    /// 列出目录下所有字表名（不含扩展名）
    pub fn list_tables(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut ps: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().map(|e| e == "txt").unwrap_or(false))
                .collect();
            ps.sort();
            out = ps;
        }
        out
    }

    /// 当前组的跟打段（字与字之间不加空格：中文逐字比对；词表则按词组段）
    pub fn current_segment(&self) -> Segment {
        let start = self.group_index * self.group_size;
        let chunk: Vec<String> = self
            .items
            .iter()
            .skip(start)
            .take(self.group_size)
            .cloned()
            .collect();
        Segment {
            mark: format!("{}", self.group_index + 1),
            text: chunk.join(""),
        }
    }

    /// 总组数
    pub fn total_groups(&self) -> usize {
        self.items.len().div_ceil(self.group_size)
    }

    /// 完成当前组 → 推进；循环回绕（蓝本：练单器循环打）
    pub fn advance(&mut self) {
        self.group_index = (self.group_index + 1) % self.total_groups().max(1);
    }

    pub fn jump_to(&mut self, group: usize) {
        let total = self.total_groups().max(1);
        self.group_index = group.min(total - 1);
    }

    /// 本组是否达标（社群门槛：击键 ≥ 阈值 且 键准 100%）
    pub fn group_passed(&self, keystroke: f64, key_accuracy: f64, threshold: f64) -> bool {
        keystroke >= threshold && key_accuracy >= 100.0
    }
}

// ─────────────────────────────────────────────────────────────
// 错字本（州州 ErrorBookRepository：error_log + char_stat 双表，连对毕业制）
// ─────────────────────────────────────────────────────────────

/// 错字明细（州州 error_log 表对齐；typed/title 溯源列保留）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorLogEntry {
    /// 正确字
    pub correct: String,
    /// 打成了什么（混淆对分析码表问题极有价值：'地'→'的'）
    pub typed: String,
    /// 错误发生时刻（本地日期，yyyy-MM-dd；时间范围聚合用）
    pub when: String,
    /// 来源文章标题（错字溯源：这个字在哪些文里错）
    pub title: String,
}

/// 单字聚合统计（州州 char_stat 表对齐）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharStat {
    pub correct: String,
    pub typed_total: u64,
    pub wrong_total: u64,
    /// 连续打对次数；≥ MASTER_STREAK(5) 毕业淡出，错则归零
    pub streak: u32,
    /// 手动"标记已掌握"（错了会重新出现——双通道，州州同款）
    #[serde(default)]
    pub mastered: bool,
    /// 最近一次错误日期
    #[serde(default)]
    pub last_wrong: String,
}

/// 错字本（内存态 + JSON 持久化；州州 SQLite 双表的文件版同构）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ErrorBook {
    pub error_log: Vec<ErrorLogEntry>,
    pub char_stat: HashMap<String, CharStat>,
}

impl ErrorBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 JSON 文件载入（不存在给空本）
    pub fn open(path: &Path) -> Self {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(b) = serde_json::from_str::<ErrorBook>(&text) {
                return b;
            }
        }
        Self::new()
    }

    pub fn save(&self, path: &Path) -> GaleResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// 记一次错字（明细落 log + 聚合记错：streak 归零、mastered 撤销重现）
    pub fn record_wrong(&mut self, correct: &str, typed: &str, when: &str, title: &str) {
        self.error_log.push(ErrorLogEntry {
            correct: correct.to_string(),
            typed: typed.to_string(),
            when: when.to_string(),
            title: title.to_string(),
        });
        let e = self
            .char_stat
            .entry(correct.to_string())
            .or_insert_with(|| CharStat {
                correct: correct.to_string(),
                typed_total: 0,
                wrong_total: 0,
                streak: 0,
                mastered: false,
                last_wrong: String::new(),
            });
        e.wrong_total += 1;
        e.streak = 0; // 错一次归零（连对制核心）
        e.mastered = false; // 手动毕业被错误撤销，重新出现
        e.last_wrong = when.to_string();
    }

    /// 记一次打对（聚合 streak +1；**不打明细 log**——州州 error_log 只记错）
    pub fn record_right(&mut self, correct: &str) {
        let e = self
            .char_stat
            .entry(correct.to_string())
            .or_insert_with(|| CharStat {
                correct: correct.to_string(),
                typed_total: 0,
                wrong_total: 0,
                streak: 0,
                mastered: false,
                last_wrong: String::new(),
            });
        e.typed_total += 1;
        e.streak = e.streak.saturating_add(1);
    }

    /// 是否已毕业（streak ≥ 5 连对自动淡出）
    pub fn is_mastered(&self, correct: &str) -> bool {
        match self.char_stat.get(correct) {
            Some(s) => s.mastered || s.streak >= MASTER_STREAK,
            None => false,
        }
    }

    /// 手动标记已掌握（错了会重新出现——州州双通道）
    pub fn mark_mastered(&mut self, correct: &str, mastered: bool) {
        let e = self
            .char_stat
            .entry(correct.to_string())
            .or_insert_with(|| CharStat {
                correct: correct.to_string(),
                typed_total: 0,
                wrong_total: 0,
                streak: 0,
                mastered: false,
                last_wrong: String::new(),
            });
        e.mastered = mastered;
    }

    /// 未毕业错字清单：按总错次倒序（并列按字排序稳定输出）
    pub fn active_wrong_chars(&self) -> Vec<&CharStat> {
        let mut v: Vec<&CharStat> = self
            .char_stat
            .values()
            .filter(|s| s.wrong_total > 0 && !(s.mastered || s.streak >= MASTER_STREAK))
            .collect();
        v.sort_by(|a, b| {
            b.wrong_total
                .cmp(&a.wrong_total)
                .then(a.correct.cmp(&b.correct))
        });
        v
    }

    /// 生成巩固练习段（州州 BtnGenPractice_Click 公式原样）：
    /// 高频前 30 字 × repeat = clamp(ceil(80/n), 2, 6) 遍，乱序拼接 ≈80 字。
    /// 返回 (标题 "错字针对练习 · N字×M遍", 段文本, 所用种子)。
    pub fn build_practice(&self, seed: u64) -> Option<(String, String, u64)> {
        let top: Vec<String> = self
            .active_wrong_chars()
            .into_iter()
            .take(PRACTICE_TOP_CHARS)
            .map(|s| s.correct.clone())
            .collect();
        let n = top.len();
        if n == 0 {
            return None;
        }
        let repeat = ((PRACTICE_TARGET_CHARS + n - 1) / n).clamp(2, 6);
        let mut pool: Vec<String> = Vec::with_capacity(n * repeat);
        for _ in 0..repeat {
            pool.extend(top.iter().cloned());
        }
        // 洗牌（种子化可复现）：按 (seed, i) hash 排序
        let mut keyed: Vec<(u64, String)> = pool
            .into_iter()
            .enumerate()
            .map(|(i, s)| {
                let mut h = seed ^ ((i as u64).wrapping_mul(0x9E3779B97F4A7C15));
                h = xorshift64(h | 1);
                (h, s)
            })
            .collect();
        keyed.sort_by_key(|(h, _)| *h);
        let text: String = keyed.into_iter().map(|(_, s)| s).collect();
        let title = format!("错字针对练习 · {n}字×{repeat}遍");
        Some((title, text, seed))
    }

    /// 按来源文章标题分组溯源（州州错字浏览同款视图）：标题 → (字, 错次)
    pub fn by_title(&self) -> HashMap<&str, Vec<(&str, u64)>> {
        let mut out: HashMap<&str, HashMap<&str, u64>> = HashMap::new();
        for e in &self.error_log {
            *out.entry(e.title.as_str())
                .or_default()
                .entry(e.correct.as_str())
                .or_insert(0) += 1;
        }
        out.into_iter()
            .map(|(title, m)| {
                let mut v: Vec<(&str, u64)> = m.into_iter().collect();
                v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
                (title, v)
            })
            .collect()
    }

    /// 时间范围内错字数（本地日期前缀比较：日=yyyy-MM-dd，周/月=yyyy-MM，年=yyyy）
    pub fn wrong_count_since(&self, prefix: &str) -> u64 {
        self.error_log
            .iter()
            .filter(|e| e.when.starts_with(prefix))
            .count() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_table(dir: &Path, name: &str, lines: &[&str]) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), lines.join("\n")).unwrap();
    }

    #[test]
    fn load_and_group() {
        let dir = crate::test_util::tmp("single");
        let _ = std::fs::remove_dir_all(&dir);
        make_table(&dir, "前500.txt", &["的", "一", "是", "了", "不"]);
        let mut s = SingleSession::load(&dir.join("前500.txt"), 2).unwrap();
        assert_eq!(s.title, "前500");
        assert_eq!(s.total_groups(), 3);
        assert_eq!(s.current_segment().text, "的一");
        s.advance();
        assert_eq!(s.current_segment().text, "是了");
        s.advance();
        assert_eq!(s.current_segment().text, "不");
        s.advance(); // 回绕
        assert_eq!(s.current_segment().text, "的一");
        s.jump_to(99);
        assert_eq!(s.group_index, 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn word_table() {
        let dir = crate::test_util::tmp("single-w");
        let _ = std::fs::remove_dir_all(&dir);
        make_table(&dir, "常用词.txt", &["我们", "你们", "他们"]);
        let s = SingleSession::load(&dir.join("常用词.txt"), 10).unwrap();
        assert_eq!(s.current_segment().text, "我们你们他们");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── 内置语料（州州 Apache-2.0；_research\drill-corpus\ 锚点）──

    #[test]
    fn builtin_tables_five_and_sizes() {
        let tables = builtin_tables();
        assert_eq!(tables.len(), 5);
        for (name, content) in &tables {
            match *name {
                "常用单字前五百" | "常用单字中五百" | "常用单字后五百" => {
                    let items = parse_items(content, 1);
                    assert_eq!(items.len(), 500, "{name} 应 500 字");
                    assert!(
                        items.iter().all(|s| s.chars().count() == 1),
                        "{name} 应每行一字"
                    );
                }
                "前1500单字" => {
                    let items = parse_items(content, 1);
                    assert_eq!(items.len(), 1500, "{name} 应 1500 字");
                }
                "常用词组前三百" => {
                    // 实测 401 字连续流（README 记 300 词为约数；按 2 字切 = 200 词 + 尾 1 字）
                    let items = parse_items(content, 2);
                    assert_eq!(items.len(), 201, "{name} 应 401 字 → 200 词 + 1 尾字");
                    assert!(items.iter().all(|s| s.chars().count() <= 2));
                    // 尾条目是单字（401 奇数）
                    assert_eq!(items.last().unwrap().chars().count(), 1);
                }
                _ => panic!("未知表 {name}"),
            }
        }
        // 前500 首三 / 末字锚点（实测连续字流：的一了…统）
        let q500 = parse_items(tables[0].1, 1);
        assert_eq!(
            &q500[..3],
            &["的".to_string(), "一".to_string(), "了".to_string()]
        );
        assert_eq!(q500.last().map(String::as_str), Some("府"));
        // 词组表首词锚点（"一个没有我们…"→ 一个/没有/我们）
        let words = parse_items(tables[4].1, 2);
        assert_eq!(
            &words[..3],
            &["一个".to_string(), "没有".to_string(), "我们".to_string()]
        );
        // 末字锚点（README/实测：401 字尾字"乎"，"几乎"拆出）
        assert_eq!(words.last().unwrap(), "乎");
        // 前1500 = 前+中+后 拼接
        let all = parse_items(tables[3].1, 1);
        let concat: Vec<String> = [tables[0].1, tables[1].1, tables[2].1]
            .iter()
            .flat_map(|c| parse_items(c, 1))
            .collect();
        assert_eq!(all, concat);
    }

    #[test]
    fn seed_builtin_tables_writes_once() {
        let dir = crate::test_util::tmp("single-seed");
        let _ = std::fs::remove_dir_all(&dir);
        let n = seed_builtin_tables(&dir);
        assert_eq!(n, 5);
        // 第二次不覆盖（用户可能改过）
        std::fs::write(dir.join("常用单字前五百.txt"), "用户改写").unwrap();
        let n2 = seed_builtin_tables(&dir);
        assert_eq!(n2, 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("常用单字前五百.txt")).unwrap(),
            "用户改写"
        );
        // 落盘文件可正常载入（"用户改写" 4 字单行流 → 4 个单字条目）
        let s = SingleSession::load(&dir.join("常用单字后五百.txt"), 10).unwrap();
        assert_eq!(s.items.len(), 500);
        let s2 = SingleSession::load(&dir.join("常用单字前五百.txt"), 10).unwrap();
        assert_eq!(s2.items.len(), 4);
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── 种子化乱序 ──

    #[test]
    fn shuffle_reproducible_and_permutation() {
        let items: Vec<String> = (0..20).map(|i| format!("字{i}")).collect();
        let a = shuffle_grouped(&items, 10, 42);
        let b = shuffle_grouped(&items, 10, 42);
        assert_eq!(a, b, "同种子必须可复现");
        // 组序不变：前 10 个仍是 0..9 的某个排列
        let set_a: std::collections::BTreeSet<&String> = a[..10].iter().collect();
        let orig: std::collections::BTreeSet<&String> = items[..10].iter().collect();
        assert_eq!(set_a, orig);
        // 大表大概率真乱（20 项全原序概率 1/10!^2——确定性断言用具体种子）
        let c = shuffle_grouped(&items, 10, 7);
        assert_ne!(a, c, "不同种子应给出不同排列");
        // seed=0 语义在 load_seeded：不乱序
        let mut sorted = items.clone();
        sorted.sort();
        let _ = sorted;
    }

    #[test]
    fn seeded_session_replay() {
        let dir = crate::test_util::tmp("single-r");
        let _ = std::fs::remove_dir_all(&dir);
        // 20 字（6! 的排列空间足够大，乱序几乎必然 ≠ 原序）
        let chars: [&str; 20] = [
            "甲", "乙", "丙", "丁", "戊", "己", "庚", "辛", "壬", "癸", "子", "丑", "寅", "卯",
            "辰", "巳", "午", "未", "申", "酉",
        ];
        make_table(&dir, "t.txt", &chars);
        let s1 = SingleSession::load_seeded(&dir.join("t.txt"), 10, 99).unwrap();
        let s2 = SingleSession::load_seeded(&dir.join("t.txt"), 10, 99).unwrap();
        assert_eq!(s1.current_segment().text, s2.current_segment().text);
        assert_ne!(
            s1.items,
            chars.iter().map(|s| s.to_string()).collect::<Vec<_>>()
        );
        // 重打同 seed = 同段
        let again = SingleSession::load_seeded(&dir.join("t.txt"), 10, 99).unwrap();
        assert_eq!(again.current_segment().text, s1.current_segment().text);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn group_pass_threshold() {
        let s = SingleSession {
            title: "t".into(),
            group_size: 10,
            items: vec![],
            group_index: 0,
            seed: 0,
        };
        // 社群门槛：击键≥阈值 且 键准100%
        assert!(s.group_passed(6.0, 100.0, 5.0));
        assert!(!s.group_passed(6.0, 99.9, 5.0), "键准必须 100%");
        assert!(!s.group_passed(4.9, 100.0, 5.0), "击键必须达阈值");
    }

    // ── 错字本（州州连对制）──

    fn book_with_errors() -> ErrorBook {
        let mut b = ErrorBook::new();
        b.record_wrong("地", "的", "2026-09-28", "文章A");
        b.record_wrong("地", "的", "2026-09-28", "文章B");
        b.record_wrong("疆", "壃", "2026-09-27", "文章A");
        b
    }

    #[test]
    fn error_book_wrong_and_streak_reset() {
        let mut b = book_with_errors();
        assert_eq!(b.error_log.len(), 3);
        let st = &b.char_stat["地"];
        assert_eq!(st.wrong_total, 2);
        assert_eq!(st.streak, 0);
        // 连对 4 次 → 未毕业；第 5 次 → 毕业
        for i in 0..4 {
            b.record_right("地");
            assert!(!b.is_mastered("地"), "连对 {i} 次不应毕业");
        }
        b.record_right("地");
        assert!(b.is_mastered("地"), "连对 5 次应毕业");
        // 毕业后再错 → streak 归零重现
        b.record_wrong("地", "得", "2026-09-29", "文章C");
        assert!(!b.is_mastered("地"));
        assert_eq!(b.char_stat["地"].streak, 0);
        assert_eq!(b.char_stat["地"].last_wrong, "2026-09-29");
    }

    #[test]
    fn mastered_manual_and_revive() {
        let mut b = book_with_errors();
        b.mark_mastered("疆", true);
        assert!(b.is_mastered("疆"), "手动毕业生效");
        // 手动毕业后错 → 撤销毕业重现
        b.record_wrong("疆", "僵", "2026-09-29", "文章B");
        assert!(!b.is_mastered("疆"));
        // active 清单不再含疆？不——错后重现，应在清单
        let active: Vec<&str> = b
            .active_wrong_chars()
            .iter()
            .map(|s| s.correct.as_str())
            .collect();
        assert!(active.contains(&"疆"));
    }

    #[test]
    fn active_sorted_by_wrong_total() {
        let b = book_with_errors();
        let active = b.active_wrong_chars();
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].correct, "地", "错 2 次在前");
        assert_eq!(active[0].wrong_total, 2);
        assert_eq!(active[1].correct, "疆");
    }

    #[test]
    fn practice_generation_formula() {
        let mut b = ErrorBook::new();
        // 3 个错字 → repeat = clamp(ceil(80/3)=27, 2, 6) = 6 → 18 字段
        for c in ["甲", "乙", "丙"] {
            b.record_wrong(c, "x", "2026-09-28", "T");
        }
        let (title, text, seed) = b.build_practice(7).unwrap();
        assert_eq!(title, "错字针对练习 · 3字×6遍");
        assert_eq!(text.chars().count(), 18); // 3 字条目 × 6 遍 = 18 字
        assert_eq!(seed, 7);
        // 内容 = 甲乙丙 各 6 次
        for c in ["甲", "乙", "丙"] {
            assert_eq!(text.matches(c).count(), 6);
        }
        // 同种子可复现
        let (_, text2, _) = b.build_practice(7).unwrap();
        assert_eq!(text, text2);
        // 40 个错字 → 只取前 30 → repeat = clamp(ceil(80/30)=3,2,6) = 3 → 90 字
        let mut b2 = ErrorBook::new();
        for i in 0..40 {
            // 各错 40-i 次：保证"按错次倒序取前 30"的选取是确定性的（字00 最多）
            for _ in 0..(40 - i) {
                b2.record_wrong(&format!("字{i:02}"), "x", "2026-09-28", "T");
            }
        }
        let (t2, x2, _) = b2.build_practice(1).unwrap();
        assert_eq!(t2, "错字针对练习 · 30字×3遍");
        assert_eq!(x2.chars().count(), 270, "30 字 × 3 字条目 = 90 条 × 3 字");
        // 取的是错次最高的前 30：字00..字29 各 3 次，字30+ 不出现
        for i in 0..30 {
            let c = format!("字{i:02}");
            assert_eq!(x2.matches(c.as_str()).count(), 3, "{c} 应 3 次");
        }
        assert!(!x2.contains("字30"), "第 31 名不应入选");
        // 空本 → None
        assert!(ErrorBook::new().build_practice(1).is_none());
    }

    #[test]
    fn practice_respects_mastery() {
        let mut b = ErrorBook::new();
        b.record_wrong("甲", "x", "2026-09-28", "T");
        b.record_wrong("乙", "x", "2026-09-28", "T");
        // 甲毕业（连对5）
        for _ in 0..5 {
            b.record_right("甲");
        }
        let (_, text, _) = b.build_practice(3).unwrap();
        assert!(text.contains('乙'));
        assert!(!text.contains('甲'), "毕业字不应进巩固段");
    }

    #[test]
    fn wrong_by_title_tracing() {
        let b = book_with_errors();
        let by_title = b.by_title();
        let a = &by_title["文章A"];
        assert_eq!(a.len(), 2);
        assert_eq!(a[0], ("地", 1)); // 错次倒序（同 1 次按字序）
        assert_eq!(by_title["文章B"], vec![("地", 1)]);
        assert_eq!(b.wrong_count_since("2026-09-28"), 2);
        assert_eq!(b.wrong_count_since("2026-09"), 3);
        assert_eq!(b.wrong_count_since("2026"), 3);
        assert_eq!(b.wrong_count_since("2025"), 0);
    }

    #[test]
    fn error_book_persistence_roundtrip() {
        let dir = crate::test_util::tmp("errbook");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("error_book.json");
        let mut b = book_with_errors();
        b.record_right("疆");
        b.mark_mastered("疆", false);
        b.save(&p).unwrap();
        let b2 = ErrorBook::open(&p);
        assert_eq!(b2.error_log.len(), 3);
        assert_eq!(b2.char_stat["地"].wrong_total, 2);
        assert_eq!(b2.char_stat["疆"].streak, 1);
        assert_eq!(b2.char_stat["疆"].typed_total, 1);
        // 坏文件 → 空本不炸
        std::fs::write(&p, "not json").unwrap();
        assert_eq!(ErrorBook::open(&p).error_log.len(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── 整改8：乱序 + 段长配置 ──

    const DRILL_BODY: &str = "的一是了不在有和人这中大为上个国我以要他时来用们生到作地于出就分对成会可主发年动同工也能下过子说产种面而方后多定行学法所民得经十三之进着等部度家电力里如水化高自二理起小物现实加量都两体制机当使点从业本去了把性强应十开由己从进";

    fn count_chars(items: &[String]) -> std::collections::BTreeMap<char, usize> {
        let mut m = std::collections::BTreeMap::new();
        for it in items {
            for c in it.chars() {
                *m.entry(c).or_insert(0) += 1;
            }
        }
        m
    }

    #[test]
    fn drill_config_defaults_clamp_and_roundtrip() {
        let dir = crate::test_util::tmp("single-cfg");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 缺省文件 → 默认（shuffle=true, segment=20）
        let p = dir.join("single.json");
        let d = SingleDrillConfig::load(&p);
        assert_eq!(d, SingleDrillConfig::default());
        assert_eq!(d.segment_chars, SINGLE_SEGMENT_DEFAULT);
        // 越界 → clamp
        let raw = SingleDrillConfig {
            target_keystroke_x10: 30,
            shuffle: false,
            segment_chars: 999,
        };
        assert_eq!(raw.clamped().segment_chars, SINGLE_SEGMENT_MAX);
        let raw = SingleDrillConfig {
            target_keystroke_x10: 30,
            shuffle: true,
            segment_chars: 1,
        };
        assert_eq!(raw.clamped().segment_chars, SINGLE_SEGMENT_MIN);
        // 存取往返（落盘带 clamp）
        raw.save(&p).unwrap();
        assert_eq!(SingleDrillConfig::load(&p), raw.clamped());
        // 坏文件 → 回默认
        std::fs::write(&p, "{nope").unwrap();
        assert_eq!(SingleDrillConfig::load(&p), SingleDrillConfig::default());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn drill_build_shuffle_keeps_multiset_and_segment_lengths() {
        let cfg = SingleDrillConfig {
            target_keystroke_x10: 30,
            shuffle: true,
            segment_chars: 20,
        };
        let s = cfg.build_session("前500", DRILL_BODY, 1, 12345);
        assert_eq!(s.group_size, 20);
        assert_eq!(s.total_groups(), (DRILL_BODY.chars().count() + 19) / 20);
        // 无重复丢失：全量字符多重集与原表一致
        let original: Vec<char> = DRILL_BODY.chars().collect();
        let joined: String = s.items.concat();
        assert_eq!(
            count_chars(&s.items),
            count_chars(&original.iter().map(|c| c.to_string()).collect::<Vec<_>>())
        );
        assert_eq!(joined.chars().count(), DRILL_BODY.chars().count());
        // 段长：前 n-1 段恰 20 字，末段为余数
        let mut group = s.clone();
        for i in 0..group.total_groups() {
            group.group_index = i;
            let len = group.current_segment().text.chars().count();
            if i + 1 < group.total_groups() {
                assert_eq!(len, 20, "segment {i}");
            } else {
                assert_eq!(len, DRILL_BODY.chars().count() % 20, "tail segment");
            }
        }
        // 乱序确实生效且同 seed 复现
        let s2 = cfg.build_session("前500", DRILL_BODY, 1, 12345);
        assert_eq!(s.items, s2.items);
        let s3 = cfg.build_session("前500", DRILL_BODY, 1, 67890);
        assert_ne!(s.items, s3.items, "不同种子应产生不同排列");
        // 原表顺序下的段内容与洗牌后的段内容（组内）多重集一致
        let plain = SingleDrillConfig {
            target_keystroke_x10: 30,
            shuffle: false,
            segment_chars: 20,
        }
        .build_session("前500", DRILL_BODY, 1, 0);
        for i in 0..plain.total_groups() {
            let mut a = s.clone();
            let mut b = plain.clone();
            a.group_index = i;
            b.group_index = i;
            let av: Vec<char> = a.current_segment().text.chars().collect();
            let bv: Vec<char> = b.current_segment().text.chars().collect();
            let mut av2 = av.clone();
            let mut bv2 = bv.clone();
            av2.sort_unstable();
            bv2.sort_unstable();
            assert_eq!(av2, bv2, "组 {i} 成员集合一致（洗牌不换成员）");
        }
    }

    #[test]
    fn drill_build_no_shuffle_preserves_order_and_seed_zero() {
        let cfg = SingleDrillConfig {
            target_keystroke_x10: 30,
            shuffle: false,
            segment_chars: 7,
        };
        let s = cfg.build_session("t", DRILL_BODY, 1, 999);
        assert_eq!(s.seed, 0, "乱序关=原序（seed 0）");
        let expect: String = DRILL_BODY.chars().take(7).collect();
        assert_eq!(s.current_segment().text, expect);
    }
}
