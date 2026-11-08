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
    /// 字号（跟打区，DIP）
    pub font_size: f32,
    #[serde(default)]
    pub hint_style: u8,
    #[serde(default = "default_motion_level")]
    pub motion_level: u8,
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
            font_size: 36.0,
            hint_style: 0,
            motion_level: 2,
            material: Material::Solid,
            opacity: 100,
            seg_chars: 100,
            auto_next: 0,
            group_watch: false,
            group_id: String::new(),
            auto_send_score: false,
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
            font_size: get("font_size", &d.font_size.to_string())
                .parse()
                .unwrap_or(d.font_size),
            material: match get("material", "Solid").as_str() {
                "Acrylic" => Material::Acrylic,
                "Blur" => Material::Blur,
                _ => Material::Solid,
            },
            hint_style: get("hint_style", "0").parse::<u8>().unwrap_or(0).min(2),
            motion_level: get("motion_level", "2").parse::<u8>().unwrap_or(2).min(3),
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
            auto_send_score: get("auto_send_score", "false") == "true",
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
            cfg.font_size.clamp(12.0, 160.0)
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
        m.insert("font_size".into(), self.font_size.to_string());
        m.insert("hint_style".into(), self.hint_style.to_string());
        m.insert("motion_level".into(), self.motion_level.to_string());
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
}
