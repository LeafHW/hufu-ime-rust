//! 会话状态。

use crate::punct::PairState;
use hufu_types::{Candidate, InputMode};

/// 提前上屏证据键（Rime history 条目）：提案 + 完整 raw + 前缀→消耗映射。
#[derive(Debug, Clone)]
pub struct EarlyHistory {
    pub proposal: String,
    pub full_raw: String,
    /// (前缀文本, 消耗的 orig 字符数)
    pub raw_lengths: Vec<(String, usize)>,
    /// 强证据：提案份额 ≥ 0.9999（异议路径合计质量 < 0.01%）
    pub strong: bool,
}

/// 空码顶屏挂起态（虎爪 SentenceEmptyCodePending）：断供时捕获的
/// 强首选先挂起，等保留量攒够（min_retained_raw）或豁免解除再顶。
#[derive(Debug, Clone)]
pub struct EmptyCodePending {
    /// 追加前捕获的首选全文（含此前已提交部分）
    pub text: String,
    /// 捕获时的 committed_text（校验用，变了就作废）
    pub cmt: String,
    /// 顶屏消耗的 full 键数（base 长度；其后留缓冲）
    pub base: usize,
}

/// 每个输入上下文（应用 / 焦点）一个会话。
#[derive(Debug, Clone)]
pub struct Session {
    /// 中/英状态
    pub chinese: bool,
    /// 原始编码缓冲（活的未提交部分；提前上屏后仅剩剩余码）
    pub raw: String,
    /// 当前候选页
    pub page: usize,
    /// ↑↓ 高亮候选（绝对下标；每次重刷归 0）
    pub selected: usize,
    /// 输入模式
    pub mode: InputMode,
    /// 当前完整候选列表
    pub candidates: Vec<Candidate>,
    /// 成对引号状态
    pub pair: PairState,
    /// 提前上屏：已上屏编码前缀（含选重后缀字符；Rime committed_raw）
    pub committed_raw: String,
    /// 提前上屏：已上屏文本（Rime committed_text）
    pub committed_text: String,
    /// 提前上屏证据史（最近 3 键）
    pub early_history: Vec<EarlyHistory>,
    /// 空码顶屏挂起态（断供持续时跨键保留，见 after_append）
    pub empty_pending: Option<EmptyCodePending>,
    /// 用户翻页/选字后暂停提前上屏，直至整句提交
    pub early_suspended: bool,
    /// 本次按键内联产生的上屏文本（顶功/唯一上屏/提前上屏增量），由 take_or_state 消费
    pub pending_commit: Option<String>,
    /// 跨句文章尾巴（最近上屏文本的尾部，整句提交后保留，焦点切换时清空）。
    /// 神经重排在句首（committed_text 为空）时以它作语境，
    /// 避免空上下文下 Qwen 乱序（实测空 ctx 时 拖乿心 反超 的窒闷）。
    pub tail_context: String,
    /// 【行尾瞬态】前端检测到组段逼近窗口右缘（软换行边界）：本次按键
    /// 的提前上屏确认从 2 键放宽到 1 键——组段早一步缩短，减少 composition
    /// 跨软换行期应用 caret 视觉滞留（「光标停在上一行结尾」）。瞬态单键
    /// 有效：try_early_commit 开头消费。
    pub line_end_hint: bool,
}

impl Session {
    pub fn new(chinese: bool) -> Self {
        Session {
            chinese,
            raw: String::new(),
            page: 0,
            selected: 0,
            mode: InputMode::Normal,
            candidates: Vec::new(),
            pair: PairState::default(),
            committed_raw: String::new(),
            committed_text: String::new(),
            early_history: Vec::new(),
            empty_pending: None,
            early_suspended: false,
            pending_commit: None,
            tail_context: String::new(),
            line_end_hint: false,
        }
    }

    pub fn clear(&mut self) {
        self.raw.clear();
        self.candidates.clear();
        self.page = 0;
        self.selected = 0;
        self.mode = InputMode::Normal;
        self.committed_raw.clear();
        self.committed_text.clear();
        self.early_history.clear();
        self.empty_pending = None;
        self.early_suspended = false;
        self.pending_commit = None;
    }

    /// 清提前上屏瞬态（保留已提交前缀）。
    pub fn early_reset(&mut self) {
        self.early_history.clear();
    }

    pub fn is_idle(&self) -> bool {
        self.raw.is_empty() && self.candidates.is_empty() && self.mode == InputMode::Normal
    }
}
