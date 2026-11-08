//! 比对状态机 —— 跟打器的灵魂。
//!
//! 设计要点（对齐晴跟打 Pro 与疾风1代行为，蓝本：`_research/typesunny-stats.md`）：
//! - 用户输入以"上屏字符流"（IME commit，含整句）+ "退格事件"驱动，与具体输入法无关。
//! - 逐字符推进 `pos`；上屏字符与原文不匹配 → 错字计数 +1，光标仍然推进（晴跟打法）。
//! - Backspace：删除一个已上屏字符 → `pos` 回退，回改 +1；删除错字同时移除当前错字罚数。
//! - CRLF 与 CR 在训练坐标中统一为一个 LF，空格与其他字符原样保留。
//! - 段末判定：`pos >= text.chars().count()` 且最后一个字符已比对。
//!
//! 本模块是纯逻辑，全部行为由 `tests` 覆盖；任何改动先跑 `cargo test -p gale-core`。

use serde::{Deserialize, Serialize};

/// Structural line-ending equivalence only. Never trims spaces or blank lines.
pub fn normalize_newlines(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.as_bytes().contains(&b'\r') {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// 单个字符的比对结果
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CharState {
    /// 未打
    Pending,
    /// 打对
    Correct,
    /// 打错（当前停在错误状态；后续字符仍继续比对）
    Wrong,
}

/// 比对事件（由输入宿主翻译成该统一事件流）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputEvent {
    /// IME 上屏（commit）了一批字符（整句上屏 = 一次多字符）
    Commit { chars: String },
    /// 一次退格（删除一个已上屏字符）
    Backspace,
}

/// 跟打时间事件（州州 TypingSession.Report 事件流的 gale 同构物，
/// `_research/stats-and-drill-models.md` §3.2）：
/// 每次输入长度变化记一条，`dt_ms` = 距上一事件的毫秒数。
/// 这是四分解（正常/卡顿/回改/错罚）与逐字时长的数据底座。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimingEvent {
    /// 本次输入长度变化（上屏 n 字 = +n；一次退格 = −1）
    pub len_delta: i32,
    /// 距上一事件毫秒（首事件 = 0）
    pub dt_ms: u64,
}

/// 比对状态机
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comparer {
    #[serde(default)]
    pub quote_rules: crate::rules::QuoteRules,
    #[serde(default)]
    pub quote_relaxed: usize,
    #[serde(skip)]
    target_chars: Vec<char>,
    /// 原文（跟打目标）
    pub text: String,
    /// 当前光标（已上屏字符数，char 计数）
    pub pos: usize,
    /// 每个字符的状态
    pub states: Vec<CharState>,
    /// 每个位置实际输入的字符（打错时 UI 显示实输字；重打覆盖）
    #[serde(default)]
    pub typed: Vec<char>,
    /// 错字数（比对层口径：不匹配的上屏字符数）
    pub wrong: usize,
    /// 回改数（物理退格删除已上屏字符的次数，不论此前对错）
    pub corrections: usize,
    /// 退格总数
    pub backspaces: usize,
    /// 首字符上屏时间戳(ms, 由调用方注入) —— 已打字数>0 时有效
    pub started_at: Option<u64>,
    /// 完成时间戳(ms)
    pub finished_at: Option<u64>,
    /// 时间事件流（州州 TypingSession.Report 同构：每次输入长度变化 + 距上次事件毫秒）。
    /// 四分解/逐字时长/慢字分析的数据底座，结算时交给 StatsEngine。
    #[serde(default)]
    pub timing_events: Vec<TimingEvent>,
    /// 最近一次时间事件的绝对毫秒（dt_ms 计算锚点；运行态，不入 JSON）
    #[serde(skip)]
    pub last_event_ms: u64,
    /// 非 BMP 字符告警（E1：原文含 U+10000+ 字符——emoji/扩展B汉字等）。
    /// 只标记不拒载：gale 按 Rust char（Unicode 标量，非 BMP 也计 1）计字，与晴
    /// LengthInTextElements 的字素簇口径存在已知偏差，含此类字符时统计需带保留。
    #[serde(default)]
    pub has_non_bmp: bool,
}

impl Comparer {
    pub fn new(text: &str) -> Self {
        let normalized = normalize_newlines(text);
        let text = normalized.as_ref();
        let n = text.chars().count();
        Self {
            quote_rules: crate::rules::QuoteRules::default(),
            quote_relaxed: 0,
            target_chars: text.chars().collect(),
            text: text.to_string(),
            pos: 0,
            states: vec![CharState::Pending; n],
            typed: Vec::new(),
            wrong: 0,
            corrections: 0,
            backspaces: 0,
            started_at: None,
            finished_at: None,
            timing_events: Vec::new(),
            last_event_ms: 0,
            has_non_bmp: text.chars().any(|c| (c as u32) >= 0x1_0000),
        }
    }

    /// 是否已完成（所有字符都比对过）
    pub fn is_done(&self) -> bool {
        !self.states.is_empty() && self.pos >= self.states.len()
    }

    /// 喂入统一事件流。`now_ms` 由调用方注入（便于测试）。
    /// 返回本事件产生的上屏字符列表（供统计/渲染用）。
    /// 同时维护时间事件流（州州 Report 同构：长度变化 + 距上一事件毫秒）。
    pub fn feed(&mut self, ev: &InputEvent, now_ms: u64) -> Vec<char> {
        if self.target_chars.len() != self.states.len() {
            self.target_chars = self.text.chars().collect();
        }
        // 首事件先落 started_at，dt 锚它 → 首条 dt=0（州州 Report 首事件 TotalTime=0 同构）
        if self.started_at.is_none() {
            self.started_at = Some(now_ms);
        }
        let anchor = if self.timing_events.is_empty() {
            self.started_at.unwrap_or(now_ms)
        } else {
            self.last_event_ms
        };
        let dt = now_ms.saturating_sub(anchor);
        match ev {
            InputEvent::Commit { chars } => {
                let mut out = Vec::new();
                for c in normalize_newlines(chars).chars() {
                    if self.pos < self.states.len() {
                        let expected = self.target_chars[self.pos];
                        let st = if self.quote_rules.equivalent(c, expected) {
                            if c != expected {
                                self.quote_relaxed += 1;
                            }
                            CharState::Correct
                        } else {
                            self.wrong += 1;
                            CharState::Wrong
                        };
                        self.states[self.pos] = st;
                        self.pos += 1;
                        self.typed.push(c);
                        out.push(c);
                    }
                }
                if out.len() > 0 {
                    self.timing_events.push(TimingEvent {
                        len_delta: out.len() as i32,
                        dt_ms: dt,
                    });
                    self.last_event_ms = now_ms;
                }
                if self.is_done() && self.finished_at.is_none() {
                    self.finished_at = Some(now_ms);
                }
                out
            }
            InputEvent::Backspace => {
                self.backspaces += 1;
                if self.pos > 0 {
                    self.pos -= 1;
                    let was = self.states[self.pos];
                    self.corrections += 1;
                    if was == CharState::Wrong {
                        self.wrong = self.wrong.saturating_sub(1);
                    }
                    if self.typed.last().is_some_and(|c| {
                        *c != self.target_chars[self.pos]
                            && self.quote_rules.equivalent(*c, self.target_chars[self.pos])
                    }) {
                        self.quote_relaxed = self.quote_relaxed.saturating_sub(1);
                    }
                    self.states[self.pos] = CharState::Pending;
                    self.typed.pop();
                    self.timing_events.push(TimingEvent {
                        len_delta: -1,
                        dt_ms: dt,
                    });
                    self.last_event_ms = now_ms;
                }
                if !self.is_done() {
                    self.finished_at = None;
                }
                Vec::new()
            }
        }
    }

    /// 打对的字符数（用于实时"有效字数"）
    pub fn correct_chars(&self) -> usize {
        self.states
            .iter()
            .filter(|s| **s == CharState::Correct)
            .count()
    }

    /// 当前光标前最后一个字符（用于候选窗锚定等）
    pub fn current_char(&self) -> Option<char> {
        if self.pos == 0 {
            None
        } else {
            self.target_chars
                .get(self.pos - 1)
                .copied()
                .or_else(|| self.text.chars().nth(self.pos - 1))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correcting_wrong_char_removes_penalty_and_relaxed_quote_undoes() {
        let mut c = Comparer::new("“甲”");
        c.feed(
            &InputEvent::Commit {
                chars: "”错".into(),
            },
            100,
        );
        assert_eq!(c.wrong, 1);
        assert_eq!(c.quote_relaxed, 1);
        c.feed(&InputEvent::Backspace, 200);
        assert_eq!(c.wrong, 0);
        assert_eq!(c.corrections, 1);
        c.feed(&InputEvent::Backspace, 300);
        assert_eq!(c.quote_relaxed, 0);
        c.feed(
            &InputEvent::Commit {
                chars: "“甲”".into(),
            },
            400,
        );
        assert_eq!(c.wrong, 0);
        assert!(c.is_done());
    }

    fn feed_commit(c: &mut Comparer, s: &str) {
        c.feed(&InputEvent::Commit { chars: s.into() }, 0);
    }

    #[test]
    fn all_correct() {
        let mut c = Comparer::new("你好世界");
        feed_commit(&mut c, "你好");
        assert_eq!(c.pos, 2);
        assert_eq!(c.wrong, 0);
        feed_commit(&mut c, "世界");
        assert!(c.is_done());
        assert_eq!(c.correct_chars(), 4);
        assert_eq!(c.corrections, 0);
    }

    #[test]
    fn whole_sentence_commit() {
        // 整句上屏：一次 commit 多字符，逐字比对
        let mut c = Comparer::new("今天天气很好");
        feed_commit(&mut c, "今天天汽很好");
        assert!(c.is_done());
        assert_eq!(c.wrong, 1);
        assert_eq!(c.correct_chars(), 5);
    }

    #[test]
    fn wrong_then_backspace_is_correction_and_clears_current_wrong() {
        // 打错→退格：计一次物理回改，移除当前错误状态。
        let mut c = Comparer::new("abc");
        feed_commit(&mut c, "x"); // 打错
        assert_eq!(c.pos, 1);
        c.feed(&InputEvent::Backspace, 0);
        assert_eq!(c.pos, 0);
        assert_eq!(c.corrections, 1);
        assert_eq!(c.wrong, 0);
        assert_eq!(c.backspaces, 1);
    }

    #[test]
    fn correct_then_backspace_is_correction() {
        let mut c = Comparer::new("abc");
        feed_commit(&mut c, "ab");
        c.feed(&InputEvent::Backspace, 0);
        assert_eq!(c.pos, 1);
        assert_eq!(c.corrections, 1);
        feed_commit(&mut c, "bc");
        assert!(c.is_done());
        assert_eq!(c.wrong, 0);
    }

    #[test]
    fn over_commit_is_clamped() {
        let mut c = Comparer::new("ab");
        feed_commit(&mut c, "abcd");
        assert!(c.is_done());
        assert_eq!(c.pos, 2);
        assert_eq!(c.wrong, 0);
    }

    #[test]
    fn backspace_at_zero_noop() {
        let mut c = Comparer::new("ab");
        c.feed(&InputEvent::Backspace, 0);
        assert_eq!(c.pos, 0);
        assert_eq!(c.backspaces, 1);
    }

    #[test]
    fn restart_clears() {
        let mut c = Comparer::new("重打");
        feed_commit(&mut c, "重");
        let fresh = Comparer::new("重打");
        assert_eq!(fresh.pos, 0);
        assert_eq!(fresh.wrong, 0);
    }

    #[test]
    fn timing_marks() {
        let mut c = Comparer::new("甲乙");
        c.feed(
            &InputEvent::Commit {
                chars: "甲".into()
            },
            100,
        );
        assert_eq!(c.started_at, Some(100));
        c.feed(
            &InputEvent::Commit {
                chars: "乙".into()
            },
            200,
        );
        assert_eq!(c.finished_at, Some(200));
    }

    #[test]
    fn line_endings_share_one_scalar_coordinate() {
        let mut c = Comparer::new("甲\r\n\r乙\n");
        assert_eq!(c.text, "甲\n\n乙\n");
        feed_commit(&mut c, "甲\r\n\n乙\r");
        assert!(c.is_done());
        assert_eq!(c.wrong, 0);
        assert_eq!(c.pos, 5);
        c.feed(&InputEvent::Backspace, 10);
        assert_eq!(c.pos, 4);
        feed_commit(&mut c, "\n");
        assert!(c.is_done());
    }
}
