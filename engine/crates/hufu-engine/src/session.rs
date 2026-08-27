//! 会话状态。

use crate::punct::PairState;
use hufu_types::{Candidate, InputMode};

/// 每个输入上下文（应用 / 焦点）一个会话。
#[derive(Debug, Clone)]
pub struct Session {
    /// 中/英状态
    pub chinese: bool,
    /// 原始编码缓冲
    pub raw: String,
    /// 当前候选页
    pub page: usize,
    /// 输入模式
    pub mode: InputMode,
    /// 当前完整候选列表
    pub candidates: Vec<Candidate>,
    /// 成对引号状态
    pub pair: PairState,
    /// 提前上屏提案连击：((文本, raw 消耗长度), 连击数)
    pub early_streak: Option<((String, usize), u32)>,
    /// 本次按键内联产生的上屏文本（顶功/唯一上屏），由 take_or_state 消费
    pub pending_commit: Option<String>,
}

impl Session {
    pub fn new(chinese: bool) -> Self {
        Session {
            chinese,
            raw: String::new(),
            page: 0,
            mode: InputMode::Normal,
            candidates: Vec::new(),
            pair: PairState::default(),
            early_streak: None,
            pending_commit: None,
        }
    }

    pub fn clear(&mut self) {
        self.raw.clear();
        self.candidates.clear();
        self.page = 0;
        self.mode = InputMode::Normal;
        self.early_streak = None;
        self.pending_commit = None;
    }

    pub fn is_idle(&self) -> bool {
        self.raw.is_empty() && self.candidates.is_empty() && self.mode == InputMode::Normal
    }
}
