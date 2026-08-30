//! 共享类型：平台无关的按键、候选、会话状态。
//!
//! 平台层（TSF / IMK）把原生按键事件映射为 [`KeyInput`]，
//! 引擎返回 [`KeyOutcome`]，平台层据此上屏文字、更新组合串与候选窗。

use serde::{Deserialize, Serialize};

/// 平台无关按键。可打印字符直接用 `Char`，功能键用枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyCode {
    Char(char),
    Space,
    Enter,
    Backspace,
    Tab,
    Escape,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    CapsLock,
    ShiftLeft,
    ShiftRight,
    CtrlLeft,
    CtrlRight,
    AltLeft,
    AltRight,
    Meta,
    F(u8),
}

impl KeyCode {
    pub fn as_char(&self) -> Option<char> {
        match self {
            KeyCode::Char(c) => Some(*c),
            KeyCode::Space => Some(' '),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
    pub caps: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyInput {
    pub key: KeyCode,
    #[serde(default)]
    pub modifiers: Modifiers,
    #[serde(default = "default_true")]
    pub is_press: bool,
}

fn default_true() -> bool {
    true
}

impl KeyInput {
    pub fn char_key(c: char) -> Self {
        KeyInput {
            key: KeyCode::Char(c),
            modifiers: Modifiers::default(),
            is_press: true,
        }
    }
}

/// 候选来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CandidateKind {
    Dict,
    Sentence,
    Symbol,
    Reverse,
    UserWord,
    Command,
    English,
    History,
    Calculator,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub text: String,
    /// 命中（或生成）该候选的编码
    pub code: String,
    /// 候选右侧注解（拼音 / 拆分 / Unicode 分区 / 反查提示等）
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub weight: f64,
    pub source: CandidateKind,
    /// 置顶（固定）候选，不参与调序隐藏
    #[serde(default)]
    pub pinned: bool,
    /// 实际上屏文本与显示不同（多多 `显示=>输出`、剪贴板变量等）
    #[serde(default)]
    pub commit_override: Option<String>,
}

impl Candidate {
    pub fn new(text: impl Into<String>, code: impl Into<String>, source: CandidateKind) -> Self {
        Candidate {
            text: text.into(),
            code: code.into(),
            comment: String::new(),
            weight: 0.0,
            source,
            pinned: false,
            commit_override: None,
        }
    }

    /// 上屏时应输出的文本
    pub fn commit_text(&self) -> &str {
        self.commit_override.as_deref().unwrap_or(&self.text)
    }
}

/// 引擎对一次按键的处理结果。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct KeyOutcome {
    /// 是否吞掉该键（false = 交给系统直通）
    pub consumed: bool,
    /// 需要立即上屏的文本（可与会话状态同时存在：顶功上屏后继续组合）
    #[serde(default)]
    pub commit: Option<String>,
    /// 提交前需回删的已上屏字符数（数字后「1.」再按 . → 回删半角点换「。」）
    #[serde(default)]
    pub back: u8,
    /// 最新的会话状态（组合串 / 候选 / 辅助提示）
    pub state: Option<SessionState>,
    /// 提示音标签（sound.enabled 时由引擎填充：key/select/commit/page）
    #[serde(default)]
    pub sound: Option<String>,
}

impl KeyOutcome {
    pub fn passthrough() -> Self {
        KeyOutcome {
            consumed: false,
            commit: None,
            back: 0,
            state: None,
            sound: None,
        }
    }
    pub fn consumed(state: SessionState) -> Self {
        KeyOutcome {
            consumed: true,
            commit: None,
            back: 0,
            state: Some(state),
            sound: None,
        }
    }
    pub fn commit(text: impl Into<String>, state: SessionState) -> Self {
        KeyOutcome {
            consumed: true,
            commit: Some(text.into()),
            back: 0,
            state: Some(state),
            sound: None,
        }
    }
}

/// 会话状态快照：前端据此渲染编码串与候选窗。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionState {
    /// 原始编码（用户已敲入、未上屏部分）
    pub raw: String,
    /// 展示用 preedit（可能与 raw 不同：编码伪装、内嵌样式）
    pub preedit: String,
    /// 候选列表（当前页）
    pub candidates: Vec<Candidate>,
    /// 当前页码（0 起）
    pub page: usize,
    /// 总页数
    pub page_count: usize,
    /// ↑↓ 高亮候选（当前页内 0 起；默认 0=首选）
    #[serde(default)]
    pub selected: usize,
    /// 辅助提示（反查提示〔拼音〕、命令帮助、错误提示等）
    #[serde(default)]
    pub aux: String,
    /// 当前输入模式
    pub mode: InputMode,
    /// 中英状态（用于托盘/状态胶囊显示）
    pub chinese: bool,
    /// 全角
    pub full_shape: bool,
    /// 中文态使用英文标点
    pub ascii_punct: bool,
    /// 是否处于反查模式
    #[serde(default)]
    pub reverse_mode: bool,
    /// 候选窗是否显示编码行（TigerClaw「候选窗显示编码」）
    #[serde(default = "default_true")]
    pub show_code: bool,
    /// 候选窗是否显示序号（candidates.show_index）
    #[serde(default = "default_true")]
    pub show_index: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum InputMode {
    #[default]
    Normal,
    /// `` ` `` 反查
    Reverse,
    /// `\` 命令命名空间
    Command,
    /// 取词/造词模式
    AddWord,
}

impl SessionState {
    pub fn idle() -> Self {
        SessionState::default()
    }
    pub fn is_idle(&self) -> bool {
        self.raw.is_empty() && self.preedit.is_empty() && self.candidates.is_empty()
    }
}
