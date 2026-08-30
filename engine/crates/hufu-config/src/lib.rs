//! hufu-config —— 全局设置模型（JSON）。
//!
//! 覆盖虎爪 config.txt 的全部设置语义 + Rime tiger_base 的关键参数 +
//! 整句引擎权重。设置界面直接读写本模型，不暴露 yaml/lua。

use serde::{Deserialize, Serialize};
use std::path::Path;

/// 根配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct Config {
    pub general: GeneralSection,
    pub schema: SchemaSection,
    pub input: InputSection,
    pub candidates: CandidatesSection,
    pub reverse: ReverseSection,
    pub sentence: SentenceSection,
    pub punct: PunctSection,
    pub clipboard: ClipboardSection,
    pub appearance: AppearanceSection,
    pub sound: SoundSection,
    pub opencc: OpenCcSection,
    pub user: UserSection,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            general: GeneralSection::default(),
            schema: SchemaSection::default(),
            input: InputSection::default(),
            candidates: CandidatesSection::default(),
            reverse: ReverseSection::default(),
            sentence: SentenceSection::default(),
            punct: PunctSection::default(),
            clipboard: ClipboardSection::default(),
            appearance: AppearanceSection::default(),
            sound: SoundSection::default(),
            opencc: OpenCcSection::default(),
            user: UserSection::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSection {
    /// 开机自启（由安装器/托盘管理）
    pub autostart: bool,
    /// 隐藏状态栏
    pub hide_status_bar: bool,
    /// 自动跟随系统输入语言
    pub follow_system_lang: bool,
    /// Shift 切换中英
    pub shift_switch: bool,
    /// Ctrl+空格切换中英
    pub ctrl_space_switch: bool,
    /// Caps 行为：clear（清屏）/ switch（切英文）
    pub caps_action: CapsAction,
    /// 最近方案对（Ctrl+M 来回切换）
    pub switch_recent_schema: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CapsAction {
    #[default]
    Clear,
    Switch,
    None,
}

impl Default for GeneralSection {
    fn default() -> Self {
        GeneralSection {
            autostart: true,
            hide_status_bar: false,
            follow_system_lang: true,
            shift_switch: true,
            ctrl_space_switch: true,
            caps_action: CapsAction::Clear,
            switch_recent_schema: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SchemaSection {
    /// 码表根目录（相对用户数据目录）
    pub dir: String,
    /// 当前方案名（目录名）
    pub current: String,
    /// 最近方案对
    pub recent_pair: Option<(String, String)>,
}

impl Default for SchemaSection {
    fn default() -> Self {
        SchemaSection {
            dir: "码表".into(),
            // 新装机默认整句方案（产品主打；虎码单字为单字精简模式，
            // 老用户 config.json 里显式保存不受影响）
            current: "虎整句".into(),
            recent_pair: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputSection {
    /// 编码字母表（参与编码的字符集），虎码默认 27 码元
    pub alphabet: String,
    /// 最大码长
    pub max_code_length: usize,
    /// 顶功：编码长度超过 max_code_length 时自动顶屏首选
    pub auto_push: bool,
    /// 满码且唯一候选时自动上屏
    pub auto_select_unique: bool,
    /// 空码时自动清屏
    pub auto_clear_empty: bool,
    /// 回车清屏
    pub enter_clear: bool,
    /// Tab 清屏
    pub tab_clear: bool,
    /// 中英文不限长混合输入（保留大小写）
    pub mixed_input: bool,
    /// 编码伪装前缀
    pub code_disguise: String,
    /// 候选窗显示编码
    pub show_code: bool,
    /// 隐藏候选窗（盲打）
    pub hide_candidates: bool,
    /// 默认中文
    pub default_chinese: bool,
    /// 中文态使用英文标点
    pub ascii_punct: bool,
    /// 无编码时 `/` 输出顿号
    pub slash_dunhao: bool,
    /// 无编码时「;」引导标点：;+空格=：、;;=；直上
    pub semicolon_guide: bool,
    /// 数字键参与整句选重
    pub digits_in_sentence: bool,
}

impl Default for InputSection {
    fn default() -> Self {
        InputSection {
            alphabet: ";'zyxwvutsrqponmlkjihgfedcba".into(),
            max_code_length: 4,
            auto_push: true,
            auto_select_unique: false,
            auto_clear_empty: true,
            enter_clear: true,
            tab_clear: true,
            mixed_input: true,
            code_disguise: String::new(),
            show_code: true,
            hide_candidates: false,
            default_chinese: true,
            ascii_punct: false,
            slash_dunhao: true,
            semicolon_guide: true,
            digits_in_sentence: true,
        }
    }
}

impl InputSection {
    pub fn is_alphabet_char(&self, c: char) -> bool {
        self.alphabet.contains(c)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CandidatesSection {
    pub page_size: usize,
    /// 翻页键（字符序列，逐字符）
    pub paging_keys: String,
    /// 次选键
    pub second_select: char,
    /// 三选键
    pub third_select: char,
    /// 自定义选重键（1..10 位对应的按键）
    pub custom_select_keys: Vec<char>,
    /// 竖排候选
    pub vertical: bool,
    /// 显示候选序号
    pub show_index: bool,
    /// 显示注释
    pub show_comment: bool,
    /// 显示拆分
    pub show_split: bool,
    /// 延时显示候选（毫秒，0=立即）
    pub delay_show_ms: u32,
    /// 延时展开注释与拆分（毫秒）
    pub delay_comment_ms: u32,
}

impl Default for CandidatesSection {
    fn default() -> Self {
        CandidatesSection {
            page_size: 5,
            paging_keys: "-=".into(),
            second_select: ';',
            third_select: '\'',
            custom_select_keys: Vec::new(),
            vertical: false,
            show_index: true,
            show_comment: true,
            show_split: false,
            delay_show_ms: 0,
            delay_comment_ms: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReverseSection {
    pub enabled: bool,
    /// 反查引导前缀
    pub prefix: char,
    /// 反查表文件名（方案目录内）
    pub table: String,
}

impl Default for ReverseSection {
    fn default() -> Self {
        ReverseSection {
            enabled: true,
            prefix: '`',
            table: "Bime_小鹤双拼反查.txt".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SentenceSection {
    /// 整句输入总开关
    pub enabled: bool,
    /// 方案名含「整句」时自动启用
    pub auto_enable: bool,
    /// 提前上屏
    pub early_commit: bool,
    /// 神经重排（llama.cpp 子进程）
    pub rerank: RerankSection,
    /// ngram 模型文件（用户数据目录相对路径）
    pub ngram_path: String,
    /// 组句权重（全部可调）
    pub weights: SentenceWeights,
}

impl Default for SentenceSection {
    fn default() -> Self {
        SentenceSection {
            enabled: true,
            auto_enable: true,
            early_commit: true,
            rerank: RerankSection::default(),
            ngram_path: "models/sentence-ngram.bin".into(),
            weights: SentenceWeights::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RerankSection {
    pub enabled: bool,
    /// 神经重排模型（用户数据目录相对路径或绝对路径；留空自动探测）
    pub model_path: String,
    /// 重排候选数
    pub top_k: usize,
    /// 去抖毫秒（停顿后才开始打分）
    pub debounce_ms: u64,
    /// 兼容保留（llama.cpp 子进程时代字段，现无用）
    pub endpoint: String,
    pub timeout_ms: u64,
}

impl Default for RerankSection {
    fn default() -> Self {
        RerankSection {
            enabled: true,
            model_path: "models/sentence-qwen-q8.gguf".into(),
            top_k: 5,
            debounce_ms: 350,
            endpoint: "127.0.0.1:0".into(),
            timeout_ms: 500,
        }
    }
}

/// 整句组句权重（与 Rime tiger_sentence.lua / 虎爪 对齐的默认值）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SentenceWeights {
    pub beam_width: usize,
    pub candidate_limit: usize,
    pub max_raw_length: usize,
    /// 未显式选重时的码表名次惩罚系数（× ln(名次)）
    pub rank_penalty: f64,
    /// 每输出一个字的奖励（鼓励多出字）
    pub emitted_character_reward: f64,
    /// 字频排名超过该值的字视为孤立生僻
    pub isolation_threshold: usize,
    /// 孤立生僻惩罚
    pub isolation_lambda: f64,
    /// 提前上屏置信阈值（候选前缀质量占比）
    pub confidence: f64,
    /// 码表候选与整句候选融合时，码表首选的加成
    pub dict_bias: f64,
    /// 补充语料奖励基准
    pub supplement_baseline: f64,
    /// 补充语料权重缩放：reward = baseline + scale × ln(w/1000)
    pub supplement_scale: f64,
    /// 补充语料奖励上限
    pub supplement_maximum: f64,
}

impl Default for SentenceWeights {
    fn default() -> Self {
        SentenceWeights {
            beam_width: 200,
            candidate_limit: 20,
            max_raw_length: 128,
            rank_penalty: 0.03,
            emitted_character_reward: 2.0,
            isolation_threshold: 3000,
            isolation_lambda: 2.0,
            confidence: 0.995,
            dict_bias: 1.0,
            supplement_baseline: 9.0,
            supplement_scale: 2.0,
            supplement_maximum: 16.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PunctSection {
    /// 全角标点
    pub full_shape: bool,
    /// 成对标点自动配对
    pub pair_brackets: bool,
}

impl Default for PunctSection {
    fn default() -> Self {
        PunctSection {
            full_shape: true,
            pair_brackets: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardSection {
    /// 剪贴板上屏
    pub enabled: bool,
    /// 进程白名单（exe 名）
    pub whitelist: Vec<String>,
}

impl Default for ClipboardSection {
    fn default() -> Self {
        ClipboardSection {
            enabled: false,
            whitelist: Vec::new(),
        }
    }
}

impl ClipboardSection {
    /// 白名单判定：空名单=全部进程允许；否则按 exe 名（大小写不敏感）匹配。
    pub fn allows(&self, exe: &str) -> bool {
        if self.whitelist.is_empty() {
            return true;
        }
        self.whitelist
            .iter()
            .any(|w| w.eq_ignore_ascii_case(exe))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSection {
    /// 当前皮肤 id
    pub skin: String,
    /// 候选字体族（空 = 平台默认）
    pub font_family: String,
    /// 候选字号
    pub font_size: f32,
    /// 显示状态胶囊
    pub status_capsule: bool,
}

impl Default for AppearanceSection {
    fn default() -> Self {
        AppearanceSection {
            skin: "hufu-default".into(),
            font_family: String::new(),
            font_size: 17.6,
            status_capsule: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundSection {
    pub enabled: bool,
    /// 0–100
    pub volume: u8,
}

impl Default for SoundSection {
    fn default() -> Self {
        SoundSection {
            enabled: false,
            volume: 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenCcSection {
    /// 启用转换（繁体候选追加）
    pub enabled: bool,
    /// 简→繁（STCharacters/STPhrases）；false 则繁→简（TS 表）
    pub to_traditional: bool,
    /// emoji 注解候选（emoji.txt）
    pub emoji: bool,
}

impl Default for OpenCcSection {
    fn default() -> Self {
        OpenCcSection {
            enabled: false,
            to_traditional: true,
            emoji: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserSection {
    /// 选词后自动调频
    pub auto_frequency: bool,
    /// 自动记录用户调整日志
    pub log_adjust: bool,
    /// 允许 Ctrl+Delete 软删候选
    pub allow_delete_word: bool,
}

impl Default for UserSection {
    fn default() -> Self {
        UserSection {
            auto_frequency: true,
            log_adjust: true,
            allow_delete_word: true,
        }
    }
}

impl Config {
    /// 从 JSON 文件加载（缺省字段取默认值）。
    pub fn load(path: &Path) -> std::io::Result<Config> {
        let text = std::fs::read_to_string(path)?;
        let cfg: Config = serde_json::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(cfg)
    }

    /// 原子保存（tmp + rename）。
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text.as_bytes())?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrip_and_partial_load() {
        let cfg = Config::default();
        assert_eq!(cfg.input.max_code_length, 4);
        assert_eq!(cfg.sentence.weights.beam_width, 200);

        // 部分 JSON：未给字段用默认值
        let partial = r#"{ "input": { "max_code_length": 5 } }"#;
        let cfg2: Config = serde_json::from_str(partial).unwrap();
        assert_eq!(cfg2.input.max_code_length, 5);
        assert_eq!(cfg2.input.auto_push, true);
        assert_eq!(cfg2.candidates.page_size, 5);
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("hufu-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.json");
        let mut cfg = Config::default();
        cfg.schema.current = "虎整句".into();
        cfg.sentence.weights.beam_width = 80;
        cfg.save(&p).unwrap();
        let cfg2 = Config::load(&p).unwrap();
        assert_eq!(cfg, cfg2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
