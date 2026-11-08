//! 主题系统 —— 晴跟打 Pro `主题/*.txt` 兼容（键<TAB>十六进制色），缺省键自动派生。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

use crate::GaleResult;

/// 颜色（8位 RGBA，主题文件里是 RRGGBB）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color(pub u8, pub u8, pub u8, pub u8);

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color(r, g, b, 255)
    }
    pub fn from_hex_no_hash(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(Color(
            ((v >> 16) & 0xff) as u8,
            ((v >> 8) & 0xff) as u8,
            (v & 0xff) as u8,
            255,
        ))
    }
    /// 预乘 Alpha（D2D 用）
    pub fn premul(&self) -> (f32, f32, f32, f32) {
        let a = self.0 as f32 / 255.0;
        (
            self.0 as f32 / 255.0 * a,
            self.1 as f32 / 255.0 * a,
            self.2 as f32 / 255.0 * a,
            a,
        )
    }
}

/// 主题键名（晴跟打 Pro 主题文件的实际键，蓝本：疾风1代 README 主题格式）。
/// 「光标色」为 gale 扩展键（晴 Pro 无此键，光标跟强调色走）——主题文件
/// 可选写入；设置面板的配色覆盖同样写进 Theme.colors。
pub const THEME_KEYS: &[&str] = &[
    "窗体背景色",
    "窗体字体色",
    "边框色",
    "跟打区背景色",
    "跟打区字体色",
    "发文区字体色",
    "打对色",
    "打错色",
    "打对字色",
    "打错字色",
    "按钮背景色",
    "按钮字体色",
    "菜单背景色",
    "菜单字体色",
    "标题栏进度条颜色",
    "光标色",
    // gale 扩展（v2.1 视觉整改）：对打三态字符底色（未打/打对/打错）。
    // 值可带 alpha（旧格式 RRGGBB 仍兼容——写入时按不透明实色理解）。
    "对打未打底色",
    "对打打对底色",
    "对打打错底色",
    // gale 扩展（v2.1 视觉整改）：词提颜色组（编码下显/角标与尾码点/下划线/底色）。
    "词提编码色",
    "词提角标色",
    "词提下划线色",
    "词提底色",
];

/// 半透明默认底色常量（对打三态）。深色主题口径，浅色主题由 gale-app 侧覆盖。
pub const BODY_BG_PENDING_DARK: Color = Color(255, 255, 255, 10); // rgba(255,255,255,0.04)
pub const BODY_BG_DONE_DARK: Color = Color(96, 180, 96, 41); // rgba(96,180,96,0.16)
pub const BODY_BG_WRONG_DARK: Color = Color(220, 80, 80, 56); // rgba(220,80,80,0.22)
/// 词提底色默认：完全透明（不画底）。
pub const HINT_BG_NONE: Color = Color(0, 0, 0, 0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub colors: BTreeMap<String, Color>,
}

impl Default for Theme {
    fn default() -> Self {
        // 疾风暗夜松烟（疾风1代内置主题之一，视觉语言延续）
        let mut colors = BTreeMap::new();
        for (k, v) in [
            ("窗体背景色", "26262a"),
            ("窗体字体色", "e9e6e0"),
            ("边框色", "3a3a42"),
            ("跟打区背景色", "222226"),
            ("跟打区字体色", "f0ede7"),
            ("发文区字体色", "a9a8a4"),
            ("打对色", "33534a"),
            ("打错色", "9c3242"),
            // jifeng1 实拍：已打字近白 #E9E6E0；打错纯白凸显（主题未覆盖时的默认）
            ("打对字色", "e9e6e0"),
            ("打错字色", "ffffff"),
            ("按钮背景色", "323238"),
            ("按钮字体色", "dedcd6"),
            ("菜单背景色", "2d2d32"),
            ("菜单字体色", "e0ded9"),
            ("标题栏进度条颜色", "58a6f5"),
            // gale 扩展：光标色缺省 = 强调色同源（晴 Pro parity：光标跟进度条色）
            ("光标色", "58a6f5"),
            // gale 扩展（v2.1 视觉整改）：对打三态底 + 词提四色。
            // 三态底默认半透明（serde 按值拷贝，旧主题文件不含这些键时取此默认）；
            // 主题文件若写入 RRGGBB 则按不透明实色生效。
            ("对打未打底色", ""), // 占位：下方直接插 Color 常量（带 alpha）
            ("对打打对底色", ""),
            ("对打打错底色", ""),
            ("词提编码色", "e7b75c"),
            ("词提角标色", "e06a5a"),
            ("词提下划线色", "58a6f5"),
            ("词提底色", ""),
        ] {
            let v = match k {
                "对打未打底色" => Some(BODY_BG_PENDING_DARK),
                "对打打对底色" => Some(BODY_BG_DONE_DARK),
                "对打打错底色" => Some(BODY_BG_WRONG_DARK),
                "词提底色" => Some(HINT_BG_NONE),
                _ => Color::from_hex_no_hash(v),
            };
            if let Some(c) = v {
                colors.insert(k.into(), c);
            }
        }
        Self {
            name: "default".into(),
            colors,
        }
    }
}

impl Theme {
    /// 解析晴跟打 Pro 主题文件：每行 `键<TAB>RRGGBB`
    pub fn parse_file(name: &str, text: &str) -> Self {
        let mut base = Theme::default();
        base.name = name.to_string();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if let Some((k, v)) = line.split_once('\t') {
                if let Some(c) = Color::from_hex_no_hash(v.trim()) {
                    base.colors.insert(k.trim().to_string(), c);
                }
            }
        }
        base
    }

    pub fn load(dir: &Path, name: &str) -> GaleResult<Self> {
        let path = dir.join(format!("{name}.txt"));
        let text = std::fs::read_to_string(&path)?;
        Ok(Self::parse_file(name, &text))
    }

    /// 取色（缺省自动派生：打对字色→打对色提亮、打错字色→打错色提亮）。
    /// v2.1 扩展键的缺省派生：词提三色跟强调/打错色系、词提底=透明、
    /// 三态底=深色常量（防御旧序列化 Theme.colors 缺键时的取色兜底）。
    pub fn get(&self, key: &str) -> Color {
        if let Some(c) = self.colors.get(key) {
            return *c;
        }
        match key {
            "打对字色" => self.lighten(self.get("打对色"), 0.55),
            "打错字色" => self.lighten(self.get("打错色"), 0.55),
            "强调色" => self.get("标题栏进度条颜色"),
            "对打未打底色" => BODY_BG_PENDING_DARK,
            "对打打对底色" => BODY_BG_DONE_DARK,
            "对打打错底色" => BODY_BG_WRONG_DARK,
            "词提编码色" => self.get("强调色"),
            "词提角标色" => self.get("打错色"),
            "词提下划线色" => self.get("强调色"),
            "词提底色" => HINT_BG_NONE,
            _ => Color::rgb(200, 200, 200),
        }
    }

    /// 配色覆盖（设置面板「自定义颜色」写入口）：写入 Theme.colors 叠加在
    /// 已加载主题之上——主题切换会重载丢弃覆盖（这是预期：换主题=换整套配色）。
    pub fn set(&mut self, key: &str, c: Color) {
        self.colors.insert(key.to_string(), c);
    }

    /// 应用 config.color_overrides（设置面板持久化的自定义颜色）。
    /// 在 Theme::load 之后调用：非法值跳过（parse 已过滤，双保险）。
    pub fn apply_overrides(&mut self, overrides: &BTreeMap<String, String>) {
        for (k, v) in overrides {
            if let Some(c) = Color::from_hex_no_hash(v) {
                self.set(k, c);
            }
        }
    }

    fn lighten(&self, c: Color, f: f32) -> Color {
        let m = |v: u8| (v as f32 + (255.0 - v as f32) * f).clamp(0.0, 255.0) as u8;
        Color::rgb(m(c.0), m(c.1), m(c.2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sunny_theme() {
        let text = "窗体背景色\t26262a\n窗体字体色\te9e6e0\n打对色\t33534a\n打错色\t9c3242\n";
        let t = Theme::parse_file("test", text);
        assert_eq!(t.get("窗体背景色"), Color::rgb(0x26, 0x26, 0x2a));
        assert_eq!(t.get("打对色"), Color::rgb(0x33, 0x53, 0x4a));
    }

    #[test]
    fn derived_keys() {
        let t = Theme::default();
        let z = t.get("打对字色");
        assert!(z.1 > t.get("打对色").1, "打对字色应比打对色亮");
    }

    #[test]
    fn caret_key_and_override() {
        // 光标色扩展键：默认=强调色同源；set 覆盖后 get 直取覆盖值
        let mut t = Theme::default();
        assert_eq!(t.get("光标色"), t.get("标题栏进度条颜色"));
        t.set("光标色", Color::rgb(0x11, 0x22, 0x33));
        assert_eq!(t.get("光标色"), Color::rgb(0x11, 0x22, 0x33));
        // 主题文件里也可写光标色（parse_file 覆盖默认）
        let t2 = Theme::parse_file("t", "光标色\tff8800\n");
        assert_eq!(t2.get("光标色"), Color::rgb(0xff, 0x88, 0x00));
    }

    #[test]
    fn bad_lines_ignored() {
        let text = "乱行\n窗体背景色\tzzzz\n窗体字体色\t12345\n边框色\t3a3a42";
        let t = Theme::parse_file("t", text);
        assert_eq!(t.get("边框色"), Color::rgb(0x3a, 0x3a, 0x42));
        // 坏行被忽略，取缺省
        assert_eq!(t.get("窗体背景色"), Color::rgb(0x26, 0x26, 0x2a));
    }

    #[test]
    fn v21_visual_keys_default_and_derive() {
        // 对打三态底 + 词提四色：Default 即含全部键（旧主题配置兼容）。
        let t = Theme::default();
        assert_eq!(t.get("对打未打底色"), BODY_BG_PENDING_DARK);
        assert_eq!(t.get("对打打对底色"), BODY_BG_DONE_DARK);
        assert_eq!(t.get("对打打错底色"), BODY_BG_WRONG_DARK);
        assert_eq!(t.get("词提底色"), HINT_BG_NONE, "词提底色默认透明");
        assert_eq!(t.get("词提编码色"), Color::rgb(0xe7, 0xb7, 0x5c));
        // 旧序列化 Theme.colors 缺键时走派生：词提三色跟强调/打错色系。
        let mut bare = Theme::default();
        for k in [
            "对打未打底色",
            "对打打对底色",
            "对打打错底色",
            "词提编码色",
            "词提角标色",
            "词提下划线色",
            "词提底色",
        ] {
            bare.colors.remove(k);
        }
        assert_eq!(bare.get("词提编码色"), bare.get("强调色"));
        assert_eq!(bare.get("词提角标色"), bare.get("打错色"));
        assert_eq!(bare.get("词提底色"), HINT_BG_NONE);
        // 晴格式主题文件同样可写新键（RRGGBB 实色覆盖）。
        let t2 = Theme::parse_file("t", "词提编码色\tff8800\n对打打对底色\t3c7a3c\n");
        assert_eq!(t2.get("词提编码色"), Color::rgb(0xff, 0x88, 0x00));
        assert_eq!(t2.get("对打打对底色"), Color::rgb(0x3c, 0x7a, 0x3c));
        // 覆盖通道（color_overrides）与 THEME_KEYS 收录。
        assert!(THEME_KEYS.contains(&"词提编码色"));
        assert!(THEME_KEYS.contains(&"对打打错底色"));
    }
}
