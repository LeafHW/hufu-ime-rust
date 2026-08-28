//! hufu-skin —— 皮肤模型（JSON）。
//!
//! 颜色角色与 weasel `preset_color_schemes` 字段一一对应（0xAABBGGRR），
//! 另有布局参数与「材质」（纯色/半透明/毛玻璃/玻璃边框），由各平台前端实现：
//! Windows: DWM Acrylic/Mica + DirectComposition；macOS: NSVisualEffectView。

use serde::{Deserialize, Serialize};
use std::path::Path;

/// RGBA 颜色（serde 兼容 0xAABBGGRR 整数与 #RRGGBBAA 字符串）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Color(#[serde(with = "color_serde")] pub [u8; 4]); // r, g, b, a

mod color_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    /// 序列化为 "#RRGGBBAA"；反序列化兼容 "#RRGGBB(AA)" 与 weasel 整数
    /// 0xAABBGGRR / 0xBBGGRR（≤0xFFFFFF 视为不透明）。
    pub fn serialize<S: Serializer>(c: &[u8; 4], s: S) -> Result<S::Ok, S::Error> {
        let [r, g, b, a] = c;
        format!("#{:02X}{:02X}{:02X}{:02X}", r, g, b, a).serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 4], D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        match &v {
            serde_json::Value::String(t) => {
                let t = t.trim_start_matches('#');
                if t.len() == 6 || t.len() == 8 {
                    let mut buf = [0u8; 4];
                    let mut ok = true;
                    for i in 0..t.len() / 2 {
                        let byte = u8::from_str_radix(&t[i * 2..i * 2 + 2], 16);
                        match byte {
                            Ok(b) => buf[i] = b,
                            Err(_) => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if !ok {
                        return Err(<D::Error as serde::de::Error>::custom("非法颜色字符串"));
                    }
                    if t.len() == 6 {
                        buf[3] = 0xFF;
                    }
                    Ok(buf)
                } else {
                    Err(<D::Error as serde::de::Error>::custom("颜色须为 #RRGGBB 或 #RRGGBBAA"))
                }
            }
            serde_json::Value::Number(n) => {
                let v = n.as_u64().ok_or_else(|| <D::Error as serde::de::Error>::custom("颜色数值溢出"))? as u32;
                // weasel 0xAABBGGRR
                let a = if (v >> 24) == 0 { 0xFF } else { (v >> 24) as u8 };
                Ok([v as u8, (v >> 8) as u8, (v >> 16) as u8, a])
            }
            _ => Err(<D::Error as serde::de::Error>::custom("颜色须为字符串或整数")),
        }
    }
}

/// 材质效果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Material {
    /// 不透明纯色
    #[default]
    Solid,
    /// 半透明（自定义不透明度）
    Translucent,
    /// 毛玻璃磨砂（Acrylic / vibrancy behindWindow）
    Frosted,
    /// 玻璃（Mica / underWindowBackground + 细边框高光）
    Glass,
}

/// 候选窗材质参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaterialConfig {
    pub kind: Material,
    /// 背景色与不透明度（translucent/frosted 时作 tint）
    pub tint: Color,
    /// 整体透明度（0=全透明 1=不透明，作用于候选窗底色）
    pub opacity: f32,
    /// 磨砂暗化修正
    pub darken: f32,
    /// 噪点强度（0–1，磨砂质感；已弃用，保留兼容旧皮肤）
    pub noise: f32,
    /// 玻璃边框宽度（px）
    pub border_width: f32,
    /// 玻璃边框色
    pub border_color: Color,
}

impl Default for MaterialConfig {
    fn default() -> Self {
        MaterialConfig {
            kind: Material::Frosted,
            tint: Color([28, 28, 30, 0xCC]),
            opacity: 1.0,
            darken: 0.0,
            noise: 0.0,
            border_width: 1.0,
            border_color: Color([255, 255, 255, 0x33]),
        }
    }
}

/// 颜色角色全集（与 weasel 对齐 + 扩展）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Colors {
    pub back_color: Color,
    pub border_color: Color,
    pub text_color: Color,
    pub preedit_back_color: Color,
    pub candidate_text_color: Color,
    pub candidate_back_color: Color,
    pub candidate_shadow_color: Color,
    pub comment_text_color: Color,
    pub label_color: Color,
    pub hilited_text_color: Color,
    pub hilited_back_color: Color,
    pub hilited_candidate_text_color: Color,
    pub hilited_candidate_back_color: Color,
    pub hilited_candidate_label_color: Color,
    pub hilited_comment_text_color: Color,
    pub hilited_label_color: Color,
    pub hilited_mark_color: Color,
    pub shadow_color: Color,
    pub hilited_shadow_color: Color,
    pub hilited_candidate_shadow_color: Color,
    /// 扩展：macOS 状态胶囊
    pub capsule_text_color: Color,
    pub capsule_back_color: Color,
}

impl Default for Colors {
    fn default() -> Self {
        // 迷雾（深色玻璃）默认
        Colors {
            back_color: Color([32, 32, 34, 0xE6]),
            border_color: Color([255, 255, 255, 0x26]),
            text_color: Color([0xE8, 0xE8, 0xEA, 0xFF]),
            preedit_back_color: Color([32, 32, 34, 0x00]),
            candidate_text_color: Color([0xE8, 0xE8, 0xEA, 0xFF]),
            candidate_back_color: Color([32, 32, 34, 0x00]),
            candidate_shadow_color: Color([0, 0, 0, 0x00]),
            comment_text_color: Color([0x9A, 0x9A, 0xA0, 0xFF]),
            label_color: Color([0xC9, 0xC9, 0xC9, 0xFF]),
            hilited_text_color: Color([0xFF, 0xFF, 0xFF, 0xFF]),
            hilited_back_color: Color([32, 32, 34, 0x00]),
            hilited_candidate_text_color: Color([0xFF, 0xFF, 0xFF, 0xFF]),
            hilited_candidate_back_color: Color([64, 64, 70, 0xFF]),
            hilited_candidate_label_color: Color([0xFF, 0xD7, 0x5E, 0xFF]),
            hilited_comment_text_color: Color([0xC9, 0xC9, 0xC9, 0xFF]),
            hilited_label_color: Color([0xFF, 0xD7, 0x5E, 0xFF]),
            hilited_mark_color: Color([0xFF, 0xD7, 0x5E, 0xFF]),
            shadow_color: Color([0, 0, 0, 0x59]),
            hilited_shadow_color: Color([0, 0, 0, 0x59]),
            hilited_candidate_shadow_color: Color([0, 0, 0, 0x00]),
            capsule_text_color: Color([0xE8, 0xE8, 0xEA, 0xFF]),
            capsule_back_color: Color([32, 32, 34, 0xCC]),
        }
    }
}

/// 布局参数（与 weasel layout 对齐）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub horizontal: bool,
    pub inline_preedit: bool,
    pub font_face: String,
    pub font_point: f32,
    pub label_font_point: f32,
    pub label_format: String,
    pub corner_radius: f32,
    pub hilited_corner_radius: f32,
    pub border_width: f32,
    pub margin_x: f32,
    pub margin_y: f32,
    pub spacing: f32,
    pub candidate_spacing: f32,
    pub hilite_spacing: f32,
    pub hilite_padding: f32,
    pub line_spacing: f32,
    pub min_width: f32,
    /// 候选窗总宽（px；0=按内容自适应留待后续）
    pub width: f32,
    pub shadow_radius: f32,
    pub shadow_offset_x: f32,
    pub shadow_offset_y: f32,
    pub mark_text: String,
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            horizontal: false,
            inline_preedit: true,
            font_face: String::new(),
            font_point: 14.5,
            label_font_point: 11.5,
            label_format: "%s.".into(),
            corner_radius: 8.0,
            hilited_corner_radius: 6.0,
            border_width: 1.0,
            margin_x: 8.0,
            margin_y: 5.0,
            spacing: 6.0,
            candidate_spacing: 4.0,
            hilite_spacing: 4.0,
            hilite_padding: 4.0,
            line_spacing: 3.0,
            min_width: 120.0,
            width: 250.0,
            shadow_radius: 4.0,
            shadow_offset_x: 0.0,
            shadow_offset_y: 2.0,
            mark_text: "·".into(),
        }
    }
}

/// 一套皮肤。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Skin {
    pub id: String,
    pub name: String,
    pub author: String,
    pub dark: bool,
    pub colors: Colors,
    pub layout: Layout,
    pub material: MaterialConfig,
}

impl Default for Skin {
    fn default() -> Self {
        Skin {
            id: "hufu-default".into(),
            name: "迷雾".into(),
            author: "HuFu".into(),
            dark: true,
            colors: Colors::default(),
            layout: Layout::default(),
            material: MaterialConfig::default(),
        }
    }
}

impl Skin {
    pub fn load(path: &Path) -> std::io::Result<Skin> {
        let text = std::fs::read_to_string(path)?;
        // 容忍 UTF-8 BOM（PowerShell/记事本保存会带；带 BOM 曾让皮肤静默失效）
        let text = text.trim_start_matches('\u{feff}');
        serde_json::from_str(text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(path, text.as_bytes())
    }

    /// 从 weasel/squirrel 的 `preset_color_schemes/<id>` JSON 片段导入配色。
    pub fn from_weasel_colors(id: &str, scheme: &serde_json::Value) -> Option<Skin> {
        let mut skin = Skin::default();
        skin.id = format!("weasel-{id}");
        skin.name = scheme
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(id)
            .to_string();
        let colors = &mut skin.colors;
        let mut set = |field: &str, slot: &mut Color| {
            if let Some(v) = scheme.get(field).and_then(|v| v.as_u64()) {
                let raw = v as u32;
                let a = if (raw >> 24) == 0 && raw <= 0xFFFFFF { 0xFF } else { (raw >> 24) as u8 };
                *slot = Color([raw as u8, (raw >> 8) as u8, (raw >> 16) as u8, a]);
            }
        };
        set("back_color", &mut colors.back_color);
        set("border_color", &mut colors.border_color);
        set("text_color", &mut colors.text_color);
        set("preedit_back_color", &mut colors.preedit_back_color);
        set("candidate_text_color", &mut colors.candidate_text_color);
        set("candidate_back_color", &mut colors.candidate_back_color);
        set("comment_text_color", &mut colors.comment_text_color);
        set("label_color", &mut colors.label_color);
        set("hilited_text_color", &mut colors.hilited_text_color);
        set("hilited_back_color", &mut colors.hilited_back_color);
        set("hilited_candidate_text_color", &mut colors.hilited_candidate_text_color);
        set("hilited_candidate_back_color", &mut colors.hilited_candidate_back_color);
        set("hilited_candidate_label_color", &mut colors.hilited_candidate_label_color);
        set("hilited_comment_text_color", &mut colors.hilited_comment_text_color);
        set("hilited_label_color", &mut colors.hilited_label_color);
        set("hilited_mark_color", &mut colors.hilited_mark_color);
        set("shadow_color", &mut colors.shadow_color);
        // 依据背景亮度判断深浅色，选材质
        let [r, g, b, _] = colors.back_color.0;
        let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        skin.dark = lum < 128.0;
        skin.material.tint = colors.back_color;
        Some(skin)
    }

    /// 导出 weasel patch 片段（写回小狼毫用）。
    pub fn to_weasel_patch(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        let conv = |c: &Color| -> u64 {
            let [r, g, b, a] = c.0;
            if a == 0xFF {
                ((b as u64) << 16) | ((g as u64) << 8) | r as u64
            } else {
                ((a as u64) << 24) | ((b as u64) << 16) | ((g as u64) << 8) | r as u64
            }
        };
        let c = &self.colors;
        m.insert("name".into(), serde_json::json!(self.name));
        m.insert("author".into(), serde_json::json!(self.author));
        m.insert("back_color".into(), serde_json::json!(conv(&c.back_color)));
        m.insert("border_color".into(), serde_json::json!(conv(&c.border_color)));
        m.insert("text_color".into(), serde_json::json!(conv(&c.text_color)));
        m.insert("candidate_text_color".into(), serde_json::json!(conv(&c.candidate_text_color)));
        m.insert("comment_text_color".into(), serde_json::json!(conv(&c.comment_text_color)));
        m.insert("label_color".into(), serde_json::json!(conv(&c.label_color)));
        m.insert("hilited_text_color".into(), serde_json::json!(conv(&c.hilited_text_color)));
        m.insert(
            "hilited_candidate_text_color".into(),
            serde_json::json!(conv(&c.hilited_candidate_text_color)),
        );
        m.insert(
            "hilited_candidate_back_color".into(),
            serde_json::json!(conv(&c.hilited_candidate_back_color)),
        );
        m.insert(
            "hilited_candidate_label_color".into(),
            serde_json::json!(conv(&c.hilited_candidate_label_color)),
        );
        m.insert("hilited_comment_text_color".into(), serde_json::json!(conv(&c.hilited_comment_text_color)));
        m.insert("hilited_label_color".into(), serde_json::json!(conv(&c.hilited_label_color)));
        m.insert("hilited_mark_color".into(), serde_json::json!(conv(&c.hilited_mark_color)));
        serde_json::Value::Object(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_weasel_roundtrip() {
        // 序列化为 #RRGGBBAA
        let v = serde_json::to_value(Colors::default().label_color).unwrap();
        assert_eq!(v, serde_json::json!("#C9C9C9FF"));
        // weasel 整数 0x30C9C9C9：BGR 反转 + alpha 高位
        let c: Color = serde_json::from_value(serde_json::json!(0x30C9C9C9)).unwrap();
        assert_eq!(c.0, [201, 201, 201, 48]);
        // 透明色无损往返
        let t = Colors::default().preedit_back_color;
        let back: Color = serde_json::from_value(serde_json::to_value(t).unwrap()).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn weasel_import() {
        let scheme = serde_json::json!({
            "name": "win11dark",
            "back_color": 0x303030,
            "candidate_text_color": 0xFFFFFF,
            "hilited_candidate_back_color": 0x202020,
            "label_color": 0xC9C9C9
        });
        let skin = Skin::from_weasel_colors("win11dark", &scheme).unwrap();
        assert_eq!(skin.name, "win11dark");
        assert!(skin.dark);
        let patch = skin.to_weasel_patch();
        assert_eq!(patch["back_color"], serde_json::json!(0x303030));
        assert_eq!(patch["label_color"], serde_json::json!(0xC9C9C9));
    }

    #[test]
    fn skin_save_load() {
        let dir = std::env::temp_dir().join(format!("hufu-skin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("skin.json");
        let skin = Skin::default();
        skin.save(&p).unwrap();
        let skin2 = Skin::load(&p).unwrap();
        assert_eq!(skin, skin2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
