//! 配置持久化（平面 TOML 子集，exe 旁便携式）。
//!
//! 疾风1代 config.toml 字段超集；主题/材质三态（纯色/磨砂/毛玻璃）+ 群文监听 + 文来账号占位。
//! 网络隔离期用 mini_toml（自带平面 TOML 解析/序列化）替代 toml crate。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

use crate::mini_toml;
use crate::GaleResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Material {
    /// 纯色
    Solid,
    /// 磨砂玻璃（Acrylic）
    Acrylic,
    /// 毛玻璃（Blur，半透明+模糊）
    Blur,
}

impl Default for Material {
    fn default() -> Self {
        Material::Solid
    }
}

impl Material {
    pub fn label(self) -> &'static str {
        match self {
            Material::Solid => "纯色",
            Material::Acrylic => "磨砂",
            Material::Blur => "透明",
        }
    }
}

/// Display routes follow either the shortest base code or the longest phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HintStrategy {
    Phrases,
    #[default]
    #[serde(other)]
    Shortest,
}
impl HintStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shortest => "shortest",
            Self::Phrases => "phrases",
        }
    }
    pub fn from_str_lossy(value: &str) -> Self {
        match value {
            "phrases" => Self::Phrases,
            _ => Self::Shortest,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub session_rules: crate::rules::SessionRules,
    #[serde(default)]
    pub cleaning_preset: crate::rules::CleaningPreset,
    /// 窗口
    pub win_x: i32,
    pub win_y: i32,
    pub win_w: i32,
    pub win_h: i32,
    /// 新版位置用屏幕物理坐标；旧配置仍按逻辑坐标迁移。
    #[serde(default)]
    pub win_position_physical: bool,
    /// 字号（跟打区，DIP）
    pub font_size: f32,
    /// 正文字体家族；空串 = 自动选择霞鹜文楷，未安装时回退微软雅黑。
    #[serde(default)]
    pub font_family: String,
    #[serde(default)]
    pub hint_style: u8,
    #[serde(default = "default_motion_level")]
    pub motion_level: u8,
    #[serde(default = "yes")]
    pub hint_show_code: bool,
    #[serde(default = "yes")]
    pub hint_show_rank: bool,
    #[serde(default = "yes")]
    pub hint_color_words: bool,
    /// 按候选序号为词组配色；关闭时沿用六档词色，旧配置默认关闭。
    #[serde(default)]
    pub hint_rank_colors: bool,
    #[serde(default)]
    pub hint_strategy: HintStrategy,
    #[serde(default)]
    pub hint_dynamic: bool,
    #[serde(default)]
    pub hint_underline: bool,
    #[serde(default)]
    pub hint_word_bg: bool,
    #[serde(default = "hint_font_default")]
    pub hint_font_size: f32,
    #[serde(default = "yes")]
    pub score_auto_copy: bool,
    #[serde(default)]
    pub score_template: String,
    #[serde(default = "score_fields_default")]
    pub score_fields: String,

    /// 窗口材质
    pub material: Material,
    /// 背景不透明度 10-100
    pub opacity: u8,
    /// 每段目标字数（智能切段）
    pub seg_chars: usize,
    /// 自动跳段（0=手动空格/回车跳段）
    pub auto_next: u8,
    /// 群文监听（剪贴板轮询）
    pub group_watch: bool,
    /// 群选择（QQ 群号/名）
    pub group_id: String,
    /// 自动发送成绩
    pub auto_send_score: bool,
    /// 成绩签名
    pub score_signature: String,
    /// 主题名（主题/ 下文件名，缺省 default）
    pub theme: String,
    /// 自定义颜色覆盖（键=主题键名 打对色/打错色/跟打区背景色/窗体背景色/光标色…，
    /// 值=RRGGBB 十六进制）。载入主题后叠加；空=全跟主题走。
    /// 持久化在 config.toml：color_打对色 = "33534a"。
    /// 新增词色键（如词提一码色）缺省不写覆盖，旧四/五档自定义值仍独立保留。
    pub color_overrides: BTreeMap<String, String>,
    /// 今日字数记忆（date+chars）
    pub today: String,
    pub today_chars: u64,
    // ── 疾风1代动效开关（typing_view.rs 同名能力，默认全开） ──
    /// 平滑光标
    pub smooth_caret: bool,
    /// 光标速度（指数平滑系数）
    pub caret_speed: f32,
    /// 光标闪烁
    pub caret_blink: bool,
    /// 聚焦变暗（当前字之后按距离渐隐）
    pub focus_dim: bool,
    /// 打对闪光
    pub pop_on_correct: bool,
    /// 打错抖动
    pub shake_on_wrong: bool,
    /// 行距（行高的额外倍数）
    pub line_gap: f32,
    /// 段末自动跳段延迟（ms，0=手动）
    pub auto_next_delay_ms: u32,
    /// 文来（晴发文系统）账号占位——本期只存不连
    pub wenlai_username: String,
    pub wenlai_domain: String,
}

fn yes() -> bool {
    true
}
fn hint_font_default() -> f32 {
    14.
}
fn score_fields_default() -> String {
    crate::scorecard::STANDARD_SCORE_FIELDS.to_string()
}

fn default_motion_level() -> u8 {
    2
}

impl Default for Config {
    fn default() -> Self {
        Self {
            session_rules: crate::rules::SessionRules::default(),
            cleaning_preset: crate::rules::CleaningPreset::default(),
            win_x: 200,
            win_y: 200,
            win_w: 1280,
            win_h: 800,
            win_position_physical: false,
            font_size: 36.0,
            font_family: String::new(),
            hint_style: 0,
            motion_level: 2,
            hint_show_code: true,
            hint_show_rank: true,
            hint_color_words: true,
            hint_rank_colors: false,
            hint_strategy: HintStrategy::Shortest,
            hint_dynamic: false,
            hint_underline: false,
            hint_word_bg: false,
            hint_font_size: 14.,
            score_auto_copy: true,
            score_template: String::new(),
            score_fields: score_fields_default(),
            material: Material::Solid,
            opacity: 100,
            seg_chars: 100,
            auto_next: 0,
            group_watch: false,
            group_id: String::new(),
            auto_send_score: true,
            score_signature: "虎符跟打".into(),
            theme: "default".into(),
            color_overrides: BTreeMap::new(),
            today: String::new(),
            today_chars: 0,
            smooth_caret: true,
            caret_speed: 12.0,
            caret_blink: true,
            focus_dim: true,
            pop_on_correct: true,
            shake_on_wrong: true,
            line_gap: 0.66,
            auto_next_delay_ms: 0,
            wenlai_username: String::new(),
            wenlai_domain: "https://qingfawen.fcxxz.com/".into(),
        }
    }
}

impl Config {
    fn read_saved(path: &Path) -> GaleResult<Self> {
        use std::io::Read;
        let mut text = String::new();
        std::fs::File::open(path)?
            .take(256 * 1024 + 1)
            .read_to_string(&mut text)?;
        if text.len() > 256 * 1024 {
            return Err(crate::GaleError::Format("配置文件超过 256 KiB".into()));
        }
        if text.trim().is_empty() {
            return Err(crate::GaleError::Format("配置文件为空或已截断".into()));
        }
        Self::from_toml_text(&text)
    }
    pub fn load_or_create(path: &Path) -> GaleResult<Self> {
        let backup = crate::score::backup_path(path);
        if path.try_exists()? {
            match Self::read_saved(path) {
                Ok(cfg) => Ok(cfg),
                Err(primary) => Self::read_saved(&backup).or(Err(primary)),
            }
        } else if backup.try_exists()? {
            Self::read_saved(&backup)
        } else {
            let cfg = Self::default();
            cfg.save(path)?;
            Ok(cfg)
        }
    }

    pub fn from_toml_text(text: &str) -> GaleResult<Self> {
        let map = mini_toml::parse(text);
        if !text.trim().is_empty() && map.is_empty() {
            return Err(crate::GaleError::Format("无法解析配置文件".into()));
        }
        let d = Config::default();
        let get = |k: &str, cur: &str| -> String {
            map.get(k).cloned().unwrap_or_else(|| cur.to_string())
        };
        let mut cfg = Config {
            win_x: get("win_x", &d.win_x.to_string())
                .parse()
                .unwrap_or(d.win_x),
            win_y: get("win_y", &d.win_y.to_string())
                .parse()
                .unwrap_or(d.win_y),
            win_w: get("win_w", &d.win_w.to_string())
                .parse()
                .unwrap_or(d.win_w),
            win_h: get("win_h", &d.win_h.to_string())
                .parse()
                .unwrap_or(d.win_h),
            win_position_physical: get("win_position_physical", "false") == "true",
            font_size: get("font_size", &d.font_size.to_string())
                .parse()
                .unwrap_or(d.font_size),
            font_family: get("font_family", "").trim().to_string(),
            material: match get("material", "Solid").as_str() {
                "Acrylic" => Material::Acrylic,
                "Blur" => Material::Blur,
                _ => Material::Solid,
            },
            hint_style: get("hint_style", "0").parse::<u8>().unwrap_or(0).min(2),
            motion_level: get("motion_level", "2").parse::<u8>().unwrap_or(2).min(3),
            hint_show_code: get("hint_show_code", "true") == "true",
            hint_show_rank: get("hint_show_rank", "true") == "true",
            hint_color_words: get("hint_color_words", "true") == "true",
            hint_rank_colors: get("hint_rank_colors", "false") == "true",
            hint_strategy: HintStrategy::from_str_lossy(&get("hint_strategy", "shortest")),
            hint_dynamic: get("hint_dynamic", "false") == "true",
            hint_underline: get("hint_underline", "false") == "true",
            hint_word_bg: get("hint_word_bg", "false") == "true",
            hint_font_size: get("hint_font_size", "14")
                .parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .unwrap_or(14.)
                .clamp(14., 28.),
            score_auto_copy: get("score_auto_copy", "true") == "true",
            score_template: get("score_template", ""),
            score_fields: get("score_fields", &d.score_fields),
            opacity: get("opacity", &d.opacity.to_string())
                .parse()
                .unwrap_or(d.opacity),
            seg_chars: get("seg_chars", &d.seg_chars.to_string())
                .parse()
                .unwrap_or(d.seg_chars),
            auto_next: get("auto_next", &d.auto_next.to_string())
                .parse()
                .unwrap_or(d.auto_next),
            group_watch: get("group_watch", "false") == "true",
            group_id: get("group_id", &d.group_id),
            auto_send_score: get("auto_send_score", "true") == "true",
            score_signature: get("score_signature", &d.score_signature),
            theme: get("theme", &d.theme),
            color_overrides: map
                .iter()
                .filter(|(k, _)| k.starts_with("color_"))
                .filter_map(|(k, v)| {
                    let key = k.strip_prefix("color_")?.to_string();
                    if crate::theme::Color::from_hex_no_hash(v).is_some() {
                        Some((key, v.clone()))
                    } else {
                        None
                    }
                })
                .collect(),
            today: get("today", &d.today),
            today_chars: get("today_chars", "0").parse().unwrap_or(0),
            smooth_caret: get("smooth_caret", "true") == "true",
            caret_speed: get("caret_speed", &d.caret_speed.to_string())
                .parse()
                .unwrap_or(d.caret_speed),
            caret_blink: get("caret_blink", "true") == "true",
            focus_dim: get("focus_dim", "true") == "true",
            pop_on_correct: get("pop_on_correct", "true") == "true",
            shake_on_wrong: get("shake_on_wrong", "true") == "true",
            line_gap: get("line_gap", &d.line_gap.to_string())
                .parse()
                .unwrap_or(d.line_gap),
            auto_next_delay_ms: get("auto_next_delay_ms", &d.auto_next_delay_ms.to_string())
                .parse()
                .unwrap_or(d.auto_next_delay_ms),
            wenlai_username: get("wenlai_username", &d.wenlai_username),
            wenlai_domain: get("wenlai_domain", &d.wenlai_domain),
            session_rules: serde_json::from_str::<crate::rules::SessionRules>(&get(
                "session_rules_json",
                "{}",
            ))
            .map_err(|e| crate::GaleError::Format(format!("规则配置损坏：{e}")))?
            .normalized(),
            cleaning_preset: crate::rules::CleaningPreset::import_json(&get(
                "cleaning_preset_json",
                "{\"rules\":[]}",
            ))
            .map_err(crate::GaleError::Format)?,
        };
        // 用户要求：成绩单不再出现「字准」。清掉旧配置里已保存的字准 token/字段。
        cfg.score_template = crate::scorecard::strip_character_accuracy(&cfg.score_template);
        cfg.score_template = crate::scorecard::upgrade_builtin_template(&cfg.score_template);
        cfg.score_fields = cfg
            .score_fields
            .split(',')
            .map(str::trim)
            .filter(|key| !key.is_empty() && *key != "character_accuracy")
            .collect::<Vec<_>>()
            .join(",");
        cfg.score_fields = crate::scorecard::upgrade_standard_fields(&cfg.score_fields);
        // Migrate only the uncustomized appearance defaults of the first preview.
        if !map.contains_key("design_version")
            && cfg.win_w == 1100
            && cfg.win_h == 680
            && cfg.font_size == 64.0
            && (cfg.line_gap - 0.22).abs() < 0.001
        {
            cfg.win_w = 1280;
            cfg.win_h = 800;
            cfg.font_size = 36.0;
            cfg.line_gap = 0.66;
        }
        cfg.opacity = cfg.opacity.clamp(10, 100);
        cfg.seg_chars = cfg.seg_chars.clamp(10, 1000);
        cfg.font_size = if cfg.font_size.is_finite() {
            cfg.font_size.clamp(10.0, 100.0)
        } else {
            d.font_size
        };
        cfg.line_gap = if cfg.line_gap.is_finite() {
            cfg.line_gap.clamp(0.0, 2.0)
        } else {
            d.line_gap
        };
        cfg.caret_speed = if cfg.caret_speed.is_finite() {
            cfg.caret_speed.clamp(1.0, 60.0)
        } else {
            d.caret_speed
        };
        cfg.win_w = cfg.win_w.clamp(640, 7680);
        cfg.win_h = cfg.win_h.clamp(480, 4320);
        cfg.auto_next = cfg.auto_next.min(1);
        cfg.auto_next_delay_ms = cfg.auto_next_delay_ms.min(10000);
        Ok(cfg)
    }

    pub fn to_toml_text(&self) -> String {
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        m.insert("design_version".into(), "2".into());
        m.insert("win_x".into(), self.win_x.to_string());
        m.insert("win_y".into(), self.win_y.to_string());
        m.insert("win_w".into(), self.win_w.to_string());
        m.insert("win_h".into(), self.win_h.to_string());
        m.insert(
            "win_position_physical".into(),
            self.win_position_physical.to_string(),
        );
        m.insert("font_size".into(), self.font_size.to_string());
        m.insert("font_family".into(), self.font_family.clone());
        m.insert("hint_style".into(), self.hint_style.to_string());
        m.insert("motion_level".into(), self.motion_level.to_string());
        m.insert("hint_strategy".into(), self.hint_strategy.as_str().into());
        for (key, value) in [
            ("hint_show_code", self.hint_show_code),
            ("hint_show_rank", self.hint_show_rank),
            ("hint_color_words", self.hint_color_words),
            ("hint_rank_colors", self.hint_rank_colors),
            ("hint_dynamic", self.hint_dynamic),
            ("hint_underline", self.hint_underline),
            ("hint_word_bg", self.hint_word_bg),
            ("score_auto_copy", self.score_auto_copy),
        ] {
            m.insert(key.into(), value.to_string());
        }
        m.insert("hint_font_size".into(), self.hint_font_size.to_string());
        m.insert("score_template".into(), self.score_template.clone());
        m.insert("score_fields".into(), self.score_fields.clone());
        m.insert("material".into(), format!("{:?}", self.material));
        m.insert("opacity".into(), self.opacity.to_string());
        m.insert("seg_chars".into(), self.seg_chars.to_string());
        m.insert("auto_next".into(), self.auto_next.to_string());
        m.insert(
            "group_watch".into(),
            (if self.group_watch { "true" } else { "false" }).into(),
        );
        m.insert("group_id".into(), self.group_id.clone());
        m.insert(
            "auto_send_score".into(),
            (if self.auto_send_score {
                "true"
            } else {
                "false"
            })
            .into(),
        );
        m.insert("score_signature".into(), self.score_signature.clone());
        m.insert("theme".into(), self.theme.clone());
        for (k, v) in &self.color_overrides {
            m.insert(format!("color_{k}"), v.clone());
        }
        m.insert("today".into(), self.today.clone());
        m.insert("today_chars".into(), self.today_chars.to_string());
        m.insert(
            "smooth_caret".into(),
            (if self.smooth_caret { "true" } else { "false" }).into(),
        );
        m.insert("caret_speed".into(), self.caret_speed.to_string());
        m.insert(
            "caret_blink".into(),
            (if self.caret_blink { "true" } else { "false" }).into(),
        );
        m.insert(
            "focus_dim".into(),
            (if self.focus_dim { "true" } else { "false" }).into(),
        );
        m.insert(
            "pop_on_correct".into(),
            (if self.pop_on_correct { "true" } else { "false" }).into(),
        );
        m.insert(
            "shake_on_wrong".into(),
            (if self.shake_on_wrong { "true" } else { "false" }).into(),
        );
        m.insert("line_gap".into(), self.line_gap.to_string());
        m.insert(
            "auto_next_delay_ms".into(),
            self.auto_next_delay_ms.to_string(),
        );
        m.insert("wenlai_username".into(), self.wenlai_username.clone());
        m.insert("wenlai_domain".into(), self.wenlai_domain.clone());
        m.insert(
            "session_rules_json".into(),
            serde_json::to_string(&self.session_rules).unwrap_or_default(),
        );
        m.insert(
            "cleaning_preset_json".into(),
            serde_json::to_string(&self.cleaning_preset).unwrap_or_default(),
        );
        mini_toml::to_string(&m)
    }

    pub fn save(&self, path: &Path) -> GaleResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        self.cleaning_preset
            .validate()
            .map_err(crate::GaleError::Format)?;
        let text = self.to_toml_text();
        if text.len() > 256 * 1024 {
            return Err(crate::GaleError::Format("配置文件超过 256 KiB".into()));
        }
        crate::score::durable_replace(path, text.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_strategy_and_dynamic_preferences_roundtrip_with_safe_legacy_defaults() {
        let old = Config::from_toml_text("").unwrap();
        assert_eq!(old.hint_strategy, HintStrategy::Shortest);
        assert!(!old.hint_dynamic);
        let cfg = Config {
            hint_strategy: HintStrategy::Phrases,
            hint_dynamic: true,
            ..Default::default()
        };
        let loaded = Config::from_toml_text(&cfg.to_toml_text()).unwrap();
        assert_eq!(loaded.hint_strategy, HintStrategy::Phrases);
        assert!(loaded.hint_dynamic);
        assert_eq!(
            Config::from_toml_text("hint_strategy = \"unknown\"")
                .unwrap()
                .hint_strategy,
            HintStrategy::Shortest
        );
        assert_eq!(
            serde_json::from_str::<HintStrategy>("\"unknown\"").unwrap(),
            HintStrategy::Shortest
        );
        assert_eq!(
            serde_json::to_string(&HintStrategy::Phrases).unwrap(),
            "\"phrases\""
        );
    }

    #[test]
    fn initial_preview_appearance_migrates_but_custom_font_survives() {
        let old = "win_w = 1100\nwin_h = 680\nfont_size = 64\nline_gap = 0.22\n";
        let cfg = Config::from_toml_text(old).unwrap();
        assert_eq!((cfg.win_w, cfg.win_h, cfg.font_size), (1280, 800, 36.));
        assert!((cfg.line_gap - 0.66).abs() < 0.001);
        let custom =
            Config::from_toml_text(&old.replace("font_size = 64", "font_size = 48")).unwrap();
        assert_eq!(custom.font_size, 48.);
        let saved = Config::from_toml_text(&cfg.to_toml_text()).unwrap();
        assert_eq!((saved.hint_style, saved.motion_level), (0, 2));
    }

    #[test]
    fn roundtrip() {
        let dir = crate::test_util::tmp("cfg");
        let p = dir.join("config.toml");
        let mut cfg = Config::default();
        cfg.material = Material::Acrylic;
        cfg.group_id = "75237860".into();
        cfg.score_signature = "打打你的 \"跟打\" v2".into();
        cfg.save(&p).unwrap();
        let cfg2 = Config::load_or_create(&p).unwrap();
        assert_eq!(cfg2.material, Material::Acrylic);
        assert_eq!(cfg2.group_id, "75237860");
        assert_eq!(cfg2.score_signature, "打打你的 \"跟打\" v2");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_legacy_and_bad_values() {
        // 老配置缺字段 → 默认值补齐；坏数字 → 回退默认
        let cfg = Config::from_toml_text("win_x = abc\ntheme = \"松烟\"\n").unwrap();
        assert_eq!(cfg.win_x, 200);
        assert_eq!(cfg.theme, "松烟");
        // 越界夹紧
        let cfg = Config::from_toml_text("opacity = 5\nseg_chars = 99999\n").unwrap();
        assert_eq!(cfg.opacity, 10);
        assert_eq!(cfg.seg_chars, 1000);
    }

    /// jifeng1 实拍权威默认值（界面组解剖定值）
    #[test]
    fn quiet_room_design_defaults() {
        let d = Config::default();
        assert_eq!(d.font_size, 36.0);
        assert_eq!(d.line_gap, 0.66);
        assert_eq!(d.caret_speed, 12.0);
        assert_eq!(d.auto_next_delay_ms, 0);
    }

    #[test]
    fn selected_font_and_full_size_range_survive_config_roundtrip() {
        let cfg = Config::from_toml_text("font_family = \"霞鹜文楷 GB Screen\"\nfont_size = 100\n")
            .unwrap();
        let loaded = Config::from_toml_text(&cfg.to_toml_text()).unwrap();
        assert_eq!(loaded.font_family, "霞鹜文楷 GB Screen");
        assert_eq!(loaded.font_size, 100.);
        assert_eq!(
            Config::from_toml_text("font_size = 999").unwrap().font_size,
            100.
        );
        assert_eq!(
            Config::from_toml_text("font_size = 1").unwrap().font_size,
            10.
        );
        assert!(Config::from_toml_text("").unwrap().font_family.is_empty());
    }

    /// 旧 config.toml（jifeng1 时代的旧默认值已写盘）缺字段时读新默认：
    /// 只有文件里显式写出的字段覆盖，其余（含这批新权威值）走 default。
    #[test]
    fn legacy_toml_missing_fields_fall_back_to_new_defaults() {
        // 模拟疾风1代风格的旧配置：只写了几个字段，含一个旧默认值 30
        // （snake_* 等已删键在旧文件里出现 = 未知键忽略，不炸不残留）
        let cfg = Config::from_toml_text(
            "win_x = 100\nfont_size = 30\nsnake = true\nsnake_before = 10\n",
        )
        .unwrap();
        assert_eq!(cfg.win_x, 100); // 显式写的生效
        assert_eq!(cfg.font_size, 30.0); // 显式写的生效（用户改过就尊重）
                                         // 缺的字段全部落新默认
        assert_eq!(cfg.line_gap, 0.66);
        assert_eq!(cfg.caret_speed, 12.0);
        assert_eq!(cfg.auto_next_delay_ms, 0);
        // 空文件 = 全默认
        let empty = Config::from_toml_text("").unwrap();
        assert_eq!(empty.font_size, 36.0);
        assert_eq!(empty.auto_next_delay_ms, 0);
    }

    #[test]
    fn material_labels() {
        // 三态标签与设置面板三横排一致（原 next() 循环切换已随「切换」chip 删除）
        assert_eq!(Material::Solid.label(), "纯色");
        assert_eq!(Material::Acrylic.label(), "磨砂");
        assert_eq!(Material::Blur.label(), "透明");
    }

    #[test]
    fn damaged_rules_recover_backup_and_nonfinite_layout_is_safe() {
        let dir = crate::test_util::tmp("cfg-backup");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.toml");
        let mut config = Config::default();
        config.group_id = "kept".into();
        config.save(&path).unwrap();
        config.save(&path).unwrap();
        std::fs::write(&path, "session_rules_json = \"broken\"\n").unwrap();
        assert_eq!(Config::load_or_create(&path).unwrap().group_id, "kept");
        // Byte damage, truncation and a missing primary all recover the same backup.
        for bad in [Vec::new(), vec![0xff, 0xfe], vec![b'a'; 256 * 1024 + 1]] {
            std::fs::write(&path, bad).unwrap();
            assert_eq!(Config::load_or_create(&path).unwrap().group_id, "kept");
        }
        std::fs::remove_file(&path).unwrap();
        assert_eq!(Config::load_or_create(&path).unwrap().group_id, "kept");
        let c = Config::from_toml_text(
            "font_size = NaN\nline_gap = inf\ncaret_speed = -inf\nwin_w = -1",
        )
        .unwrap();
        assert!(c.font_size.is_finite() && c.line_gap.is_finite() && c.caret_speed.is_finite());
        assert_eq!(c.win_w, 640);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn color_overrides_roundtrip_and_sanitize() {
        // 合法 RRGGBB 保留；非法值丢弃；未知前缀键不进覆盖表
        let cfg = Config::from_toml_text(
            "color_打对色 = \"33534a\"\ncolor_打错色 = \"zzzzzz\"\nother = 1\n",
        )
        .unwrap();
        assert_eq!(cfg.color_overrides.len(), 1);
        assert_eq!(
            cfg.color_overrides.get("打对色").map(|s| s.as_str()),
            Some("33534a")
        );
        // roundtrip：写盘再读回
        let dir = crate::test_util::tmp("cfgcolor");
        let p = dir.join("config.toml");
        cfg.save(&p).unwrap();
        let cfg2 = Config::load_or_create(&p).unwrap();
        assert_eq!(
            cfg2.color_overrides.get("打对色").map(|s| s.as_str()),
            Some("33534a")
        );
        std::fs::remove_dir_all(&dir).ok();
    }
    #[test]
    fn word_color_overrides_keep_legacy_keys_and_roundtrip_one_code_color() {
        // Legacy four-color profiles must retain their exact overrides while
        // leaving the new one-code color to the current theme's default.
        let legacy = Config::from_toml_text(
            "color_词提简码色 = \"112233\"\n\
             color_词提二字全码色 = \"223344\"\n\
             color_词提三字全码色 = \"334455\"\n\
             color_词提四字全码色 = \"445566\"\n",
        )
        .unwrap();
        assert_eq!(legacy.color_overrides.len(), 4);
        assert!(!legacy.color_overrides.contains_key("词提一码色"));
        assert_eq!(
            serde_json::from_str::<Config>(&serde_json::to_string(&legacy).unwrap())
                .unwrap()
                .color_overrides,
            legacy.color_overrides
        );

        let mut five = legacy.clone();
        five.color_overrides
            .insert("词提二字三码色".into(), "556677".into());
        let old_five = Config::from_toml_text(&five.to_toml_text()).unwrap();
        assert_eq!(old_five.color_overrides, five.color_overrides);
        assert!(!old_five.color_overrides.contains_key("词提一码色"));
        let mut six = old_five;
        six.color_overrides
            .insert("词提一码色".into(), "c43e62".into());
        let reloaded = Config::from_toml_text(&six.to_toml_text()).unwrap();
        assert_eq!(reloaded.color_overrides, six.color_overrides);
        assert_eq!(
            serde_json::from_str::<Config>(&serde_json::to_string(&six).unwrap())
                .unwrap()
                .color_overrides,
            six.color_overrides
        );
        for (key, value) in legacy.color_overrides {
            assert_eq!(reloaded.color_overrides.get(&key), Some(&value));
        }
        let invalid_one = Config::from_toml_text(
            "color_词提一码色 = \"invalid\"\ncolor_词提简码色 = \"112233\"\n",
        )
        .unwrap();
        assert!(!invalid_one.color_overrides.contains_key("词提一码色"));
        assert_eq!(invalid_one.color_overrides["词提简码色"], "112233");
    }
    #[test]
    fn candidate_rank_colors_are_opt_in_and_roundtrip_with_custom_colors() {
        let legacy = Config::from_toml_text("hint_color_words = true").unwrap();
        assert!(!legacy.hint_rank_colors);
        let mut snapshot = serde_json::to_value(&legacy).unwrap();
        snapshot.as_object_mut().unwrap().remove("hint_rank_colors");
        assert!(
            !serde_json::from_value::<Config>(snapshot)
                .unwrap()
                .hint_rank_colors
        );

        let cfg = Config::from_toml_text(
            "hint_rank_colors = true\ncolor_词提3选色 = \"123aef\"\ncolor_词提10选色 = \"456abc\"\n",
        ).unwrap();
        assert!(cfg.hint_rank_colors);
        let loaded = Config::from_toml_text(&cfg.to_toml_text()).unwrap();
        assert!(loaded.hint_rank_colors);
        assert_eq!(loaded.color_overrides, cfg.color_overrides);
        assert_eq!(loaded.color_overrides["词提3选色"], "123aef");
        assert!(loaded.hint_color_words);
    }

    #[test]
    fn trainer_preferences_preserve_multiline_templates_and_independent_switches() {
        let mut cfg = Config::default();
        cfg.hint_show_code = false;
        cfg.hint_show_rank = true;
        cfg.hint_word_bg = true;
        cfg.hint_font_size = 22.;
        cfg.score_auto_copy = false;
        cfg.score_template = "第{segment}段\n速度 {speed} \"quote\"".into();
        cfg.score_fields = "segment,word_rate,retype".into();
        cfg.color_overrides
            .insert("词提三字全码色".into(), "123aef".into());
        cfg.color_overrides
            .insert("词提二字三码色".into(), "456abc".into());
        let loaded = Config::from_toml_text(&cfg.to_toml_text()).unwrap();
        assert!(!loaded.hint_show_code && loaded.hint_show_rank && loaded.hint_word_bg);
        assert_eq!(loaded.hint_font_size, 22.);
        assert!(!loaded.score_auto_copy);
        assert_eq!(loaded.score_template, cfg.score_template);
        assert_eq!(loaded.score_fields, cfg.score_fields);
        assert_eq!(loaded.color_overrides, cfg.color_overrides);
        assert_eq!(
            Config::from_toml_text("hint_font_size = 99")
                .unwrap()
                .hint_font_size,
            28.
        );
    }
}
#[test]
fn physical_window_position_roundtrips_and_legacy_config_keeps_old_coordinate_mode() {
    let old = Config::from_toml_text("win_x = -1000\nwin_y = 40").unwrap();
    assert!(!old.win_position_physical);
    let mut cfg = old;
    cfg.win_position_physical = true;
    cfg.win_w = 960;
    let restored = Config::from_toml_text(&cfg.to_toml_text()).unwrap();
    assert!(restored.win_position_physical);
    assert_eq!(restored.win_x, -1000);
    assert_eq!(restored.win_w, 960);
}
