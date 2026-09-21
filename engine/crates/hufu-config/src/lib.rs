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
    Clear,
    #[default]
    Switch,
    None,
}

impl Default for GeneralSection {
    fn default() -> Self {
        Self {
            autostart: true,
            hide_status_bar: false,
            follow_system_lang: true,
            shift_switch: true,
            ctrl_space_switch: true,
            // 【2026-10-09 默认回正】Caps 默认=切英文（用户定稿；此前
            // 误设 Clear）。老用户 config.json 里已显式写Clear 的不受
            // 影响（序列化值优先于默认）。
            caps_action: CapsAction::Switch,
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
    /// 超最大码长自动上屏：编码长度超过 max_code_length 时自动顶屏首选（设置界面显示名）
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
    /// `/`=顿时直出，屏蔽码表内/相关内容：开启后空态按 / 直接上屏
    /// 「、」（不进 / 符号命名空间）、有候选时首选+「、」；关闭时
    /// / 进符号命名空间（首位顿号需空格确认，继续 / 按数量出 /）
    pub slash_dunhao: bool,
    /// 【\=顿号直出 2026-09-08】true=空态按 \ 直接上屏「、」、有
    /// 候选时首选+「、」；false（默认）=打 \ 弹「、」候选（空格
    /// 确认，与 / 命名空间档的首选行为一致）。
    pub backslash_dunhao: bool,
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
            // 【默认关 2026-10-09 十四】用户拍板：空码自动清屏/回车清屏/
            // 中英混输默认关（新用户更接近传统输入法行为，避免「打着
            // 打着编码没了」「大小写字母混进来」的困惑）。
            auto_clear_empty: false,
            enter_clear: false,
            tab_clear: true,
            mixed_input: false,
            code_disguise: String::new(),
            show_code: true,
            hide_candidates: false,
            default_chinese: true,
            ascii_punct: false,
            // 【2026-09-06 用户拍板】默认命名空间档：/ 前缀功能
            //（/jc 加词、/jq 加权、/rq 日期等，见各方案 快符.txt）
            // 开箱即用；要「/ 一键出顿号」的用户手动勾选直出档。
            slash_dunhao: false,
            backslash_dunhao: false,
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
    /// 显示拼音注释（数据\注释\拼音.注释；无表时回退反查表 词→码）
    pub show_pinyin_comment: bool,
    /// 显示 unicode 注释（数据\注释\unicode.注释，如 [平假名]）
    pub show_unicode_comment: bool,
    /// 显示拆分
    pub show_split: bool,
    /// 拆分方案名（数据\拆分\<名>.拆分；空=关）
    pub split_scheme: String,
    /// 延时显示候选（毫秒，0=立即）
    pub delay_show_ms: u32,
    /// 延时展开注释与拆分（毫秒）
    pub delay_comment_ms: u32,
}

impl Default for CandidatesSection {
    fn default() -> Self {
        CandidatesSection {
            page_size: 4, // 【默认 4 2026-10-09 十四】用户拍板（原 5）
            paging_keys: "-=".into(),
            second_select: ';',
            third_select: '\'',
            custom_select_keys: Vec::new(),
            vertical: false,
            show_index: true,
            // 【2026-09-07 用户拍板】拼音注释默认关（此前默认开，用户
            // 反馈默认关）；unicode/拆分维持默认关。
            show_pinyin_comment: false,
            show_unicode_comment: false,
            show_split: false,
            split_scheme: "虎码".into(),
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
    /// 反查方案名（数据\拼音反查\<名>.txt；空=关；文件缺失回退方案目录旧表）
    pub scheme: String,
}

impl Default for ReverseSection {
    fn default() -> Self {
        ReverseSection {
            enabled: true,
            prefix: '`',
            scheme: "小鹤双拼".into(),
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
    /// 【三十六修·断供兜底】提前上屏锁定错误前缀后完整态候选池断供
    ///（以 committed_text 为前缀的候选耗尽），空格本会 clear() 把剩余
    /// raw 连同其文本静默丢弃（「丢字」根因）——开启时把断供视作强制
    /// 分段：丢弃 committed 约束、用剩余 raw 重开一段解码取首选上屏
    ///（不进学习）。serde 默认 true（虎爪 2026-09 同步语义的落地实现）。
    #[serde(default = "default_true")]
    pub empty_code_auto_commit: bool,
    /// 预留参数，当前未消费（虎爪同步占位）：断供顶屏后缓冲保留最少
    /// 编码键数。当前兜底只在空格时刻整体重开一段，无逐键顶屏路径。
    #[serde(default)]
    pub min_retained_raw: usize,
    /// 神经重排（llama.cpp 子进程）
    pub rerank: RerankSection,
    /// ngram 模型文件（用户数据目录相对路径）
    pub ngram_path: String,
    /// 【提前上屏证据窗 2026-09-06 定版】确认上屏所需证据键数。
    /// 万句终测定版 3：残留码长 4.86、字/次 1.29、准率 99.54%（三者
    /// 最高），配束宽 6000 上屏率 48.5%。HUFU_EARLY_NEED 环境变量
    /// 优先（bench 覆盖）。
    #[serde(default = "default_early_need")]
    pub early_need: usize,
    /// 【同码分歧护栏 2026-09-19】提前上屏提交前，若候选池内存在距池首
    /// 分差 ≤ early_diverg_gap、且文本在本次消耗跨度内与稳定前缀分歧的
    /// 活候选（上屏将摧毁其词内切分），本键不上屏——保住句尾/停顿重排
    /// （qwen）的换句余地。实例：gkzpuvjihujinkbky（却足以绊住流…）在
    /// v5 模型下「收拾」第 17 键锁死整句。缺省关；HUFU_EARLY_DIVERG_GUARD=1
    /// 环境变量等效开启（优先）。
    #[serde(default)]
    pub early_diverg_guard: bool,
    /// 护栏分界 Δ（活候选距池首的分差上限）。实证：同码孪生全程落后
    /// 2~7 分（拦），噪音候选（生僻字类）落后 ≥9 分（放）。默认 8.0；
    /// HUFU_EARLY_DIVERG_GAP 环境变量优先。
    #[serde(default = "default_diverg_gap")]
    pub early_diverg_gap: f64,
    /// 组句权重（全部可调）
    pub weights: SentenceWeights,
}

fn default_early_need() -> usize {
    3
}

fn default_diverg_gap() -> f64 {
    8.0
}

fn default_true() -> bool {
    true
}

impl Default for SentenceSection {
    fn default() -> Self {
        SentenceSection {
            enabled: true,
            auto_enable: true,
            early_commit: true,
            empty_code_auto_commit: true,
            min_retained_raw: 0,
            rerank: RerankSection::default(),
            // 【三十六修】默认路径对齐发行布局（模型\ 一级目录，与
            // 数据说明/设置页指引/装载计划探测一致）——旧 models/ 前缀
            // 使 Default 态永远 miss、全靠目录探测兜底（配合 /api/state
            // 的探测口径修复消除「有模型显示未安装」）。
            ngram_path: "模型/sentence-ngram.bin".into(),
            // 【回归 1.4.8 模型默认 2026-09-08】W1 束宽 30000 实测引发
            // 「越打越卡」（每键全量解码数百 ms，1.4.8 基线对照实锤）
            //——默认回 1.4.8 值（beam200/cl20/supp32），W1 档位
            // 保留在设置页预设里按需一键切换。
            // 【八十修·默认参数统一 2026-09-14】early_need 2→3：与
            // serde 缺省（default_early_need）、设置页「提前上屏（稳
            // 3 键）」、设置页 W_DEFAULTS 出厂值、打包源模板 config、
            // 本机实测调校值五方对齐——Default impl 是新装机首启
            // （config.json 不存在）与 tbench 基准的兜底，此前 2 与
            // 产品定版 3 不一致（bench 口径偏差来源之一）。
            early_need: 3,
            early_diverg_guard: false,
            early_diverg_gap: 8.0,
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
            model_path: "模型/sentence-qwen-q8.gguf".into(),
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
    /// 【数字编码 2026-09-05】码表用数字做编码字符（a8=来、u3=的
    /// 这类第二码位为数字的体系）时为 true：raw 里的数字保留为编码、
    /// 不解析成「选重第 N」。由引擎按码表内容自动填充（有无数字码
    /// 词条），非用户配置项。
    pub digit_codes: bool,
}

impl Default for SentenceWeights {
    fn default() -> Self {
        SentenceWeights {
            // 【回归 1.4.8 模型默认 2026-09-08】束宽 30000 在实机每键
            // 全量解码数百 ms（短句无增量门槛），跟打场景节奏被拖垮
            //——默认回 1.4.8 值。束宽增益（上屏率/成段出词）保留在
            // 设置页「上屏节奏」预设：想用 W1 一键切换。
            beam_width: 200,
            candidate_limit: 20,
            max_raw_length: 128,
            rank_penalty: 0.03,
            emitted_character_reward: 2.0,
            isolation_threshold: 3000,
            isolation_lambda: 2.0,
            confidence: 0.99,
            dict_bias: 1.0,
            supplement_baseline: 9.0,
            supplement_scale: 2.0,
            supplement_maximum: 32.0,
            digit_codes: false,
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
        self.whitelist.iter().any(|w| w.eq_ignore_ascii_case(exe))
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
    /// 【动效开关 2026-09-11】候选窗动效总开关（false=一切动效瞬跳）
    pub anim: bool,
    /// 【动效速度 2026-09-11】整体速度倍率（1.0=默认速度；0~2，0=瞬跳）
    pub anim_speed: f32,
    // 【特效退役 2026-09-22】commit_fx / stamp_font_scale（上屏特效）
    // 字段删除——旧配置文件多余键 serde 自动忽略（同 二十四修 口径）。
}

impl Default for AppearanceSection {
    fn default() -> Self {
        AppearanceSection {
            skin: "hufu-moyan".into(), // 【用户定稿】墨岩为默认皮肤
            font_family: String::new(),
            font_size: 17.6,
            status_capsule: true,
            anim: true,
            anim_speed: 1.0,
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
    /// 容错：跳过 UTF-8 BOM（PowerShell 系工具写的文件常带——serde_json
    /// 遇 BOM 报 "expected value at line 1 column 1"，曾致 server 起不来）。
    pub fn load(path: &Path) -> std::io::Result<Config> {
        let text = std::fs::read_to_string(path)?;
        let text = text.trim_start_matches('\u{feff}');
        let cfg: Config = serde_json::from_str(text)
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
        // 2026-09-08 默认回归 1.4.8 模型值（W1 30000 实测致越打越卡）
        assert_eq!(cfg.sentence.weights.beam_width, 200);
        assert_eq!(cfg.sentence.weights.candidate_limit, 20);
        // 八十修：默认 3 与 serde 缺省/设置页「稳 3 键」对齐
        assert_eq!(cfg.sentence.early_need, 3);

        // 部分 JSON：未给字段用默认值
        let partial = r#"{ "input": { "max_code_length": 5 } }"#;
        let cfg2: Config = serde_json::from_str(partial).unwrap();
        assert_eq!(cfg2.input.max_code_length, 5);
        assert_eq!(cfg2.input.auto_push, true);
        // 【默认 4 2026-10-09 十四】每页候选默认 5→4（用户拍板）
        assert_eq!(cfg2.candidates.page_size, 4);
        // 【默认关 2026-10-09 十四】空码清屏/回车清屏/中英混输默认关
        assert_eq!(cfg2.input.auto_clear_empty, false);
        assert_eq!(cfg2.input.enter_clear, false);
        assert_eq!(cfg2.input.mixed_input, false);
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
