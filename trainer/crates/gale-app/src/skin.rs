//! Adapt HuFu's current color palette to a readable training surface.
//! Network calls belong on the application's background worker.
use gale_core::theme::{Color, Theme};
use serde_json::Value;

pub fn fetch(port: u16) -> Result<Theme, String> {
    let list = crate::local_json(port, "/api/skins", None)?;
    if list["current_kind"].as_str() == Some("ssf") {
        return Err("当前为贴图皮肤，跟打器保留静室配色；可选择独立主题".into());
    }
    from_value(&crate::local_json(port, "/api/skin", None)?)
}

fn color(v: &Value) -> Option<Color> {
    if let Some(s) = v.as_str() {
        let s = s.strip_prefix('#').unwrap_or(s);
        if !s.is_ascii() || !matches!(s.len(), 6 | 8) {
            return None;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        let a = if s.len() == 8 {
            u8::from_str_radix(&s[6..8], 16).ok()?
        } else {
            255
        };
        Some(Color(r, g, b, a))
    } else {
        let n = u32::try_from(v.as_u64()?).ok()?;
        Some(Color(
            n as u8,
            (n >> 8) as u8,
            (n >> 16) as u8,
            if n >> 24 == 0 { 255 } else { (n >> 24) as u8 },
        ))
    }
}
fn blend(a: Color, b: Color, amount: f64) -> Color {
    let ch = |x: u8, y: u8| (x as f64 * (1.0 - amount) + y as f64 * amount).round() as u8;
    Color(ch(a.0, b.0), ch(a.1, b.1), ch(a.2, b.2), 255)
}
fn opaque(c: Color, background: Color) -> Color {
    blend(background, c, c.3 as f64 / 255.0)
}
fn luminance(c: Color) -> f64 {
    let linear = |n: u8| {
        let n = n as f64 / 255.0;
        if n <= 0.04045 {
            n / 12.92
        } else {
            ((n + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(c.0) + 0.7152 * linear(c.1) + 0.0722 * linear(c.2)
}
fn contrast(a: Color, b: Color) -> f64 {
    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
fn readable(c: Color, bg: Color, min: f64) -> Color {
    let c = opaque(c, bg);
    if contrast(c, bg) >= min {
        return c;
    }
    let light = Color::rgb(250, 248, 243);
    let dark = Color::rgb(20, 22, 26);
    let target = if contrast(light, bg) > contrast(dark, bg) {
        light
    } else {
        dark
    };
    for i in 1..=20 {
        let candidate = blend(c, target, i as f64 / 20.0);
        if contrast(candidate, bg) >= min {
            return candidate;
        }
    }
    target
}

pub fn from_value(v: &Value) -> Result<Theme, String> {
    let colors = v
        .get("colors")
        .and_then(Value::as_object)
        .ok_or("虎符皮肤缺少颜色数据")?;
    let dark = v["dark"].as_bool().unwrap_or(true);
    let base = if dark {
        Color::rgb(19, 20, 23)
    } else {
        Color::rgb(245, 242, 234)
    };
    let get = |key: &str| colors.get(key).and_then(color);
    let bg = opaque(get("back_color").ok_or("虎符皮肤背景颜色无效")?, base);
    let fallback = if luminance(bg) < 0.35 {
        Color::rgb(240, 237, 230)
    } else {
        Color::rgb(33, 30, 24)
    };
    let fg = readable(
        get("candidate_text_color")
            .or_else(|| get("text_color"))
            .unwrap_or(fallback),
        bg,
        4.5,
    );
    let accent = readable(
        get("hilited_mark_color")
            .or_else(|| get("hilited_candidate_back_color"))
            .unwrap_or(Color::rgb(198, 167, 102)),
        bg,
        3.0,
    );
    let muted = readable(
        get("comment_text_color").unwrap_or(blend(bg, fg, 0.62)),
        bg,
        3.0,
    );
    let surface = blend(bg, fg, 0.065);
    let wrong = readable(Color::rgb(202, 107, 93), bg, 3.0);
    let mut t = Theme::default();
    t.name = format!("跟随虎符 · {}", v["name"].as_str().unwrap_or("当前皮肤"));
    for (key, value) in [
        ("窗体背景色", bg),
        ("跟打区背景色", bg),
        ("菜单背景色", surface),
        ("按钮背景色", surface),
        ("边框色", surface),
        ("窗体字体色", fg),
        ("跟打区字体色", fg),
        ("菜单字体色", fg),
        ("按钮字体色", fg),
        ("打对字色", fg),
        ("打对色", fg),
        ("发文区字体色", muted),
        ("光标色", accent),
        ("标题栏进度条颜色", accent),
        ("打错字色", wrong),
        ("打错色", wrong),
    ] {
        t.set(key, value);
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bad_unicode_color_is_rejected_without_panicking() {
        assert!(color(&serde_json::json!("あい")).is_none());
        assert!(color(&serde_json::json!("#GG1133")).is_none());
        assert_eq!(
            color(&serde_json::json!(0x112233)),
            Some(Color(0x33, 0x22, 0x11, 255))
        );
    }
    #[test]
    fn transparent_or_low_contrast_skin_stays_readable() {
        let t = from_value(
            &serde_json::json!({"dark":true,"colors":{"back_color":"#00000000",
            "candidate_text_color":"#10101010","hilited_mark_color":"#00000000"}}),
        )
        .unwrap();
        assert!(contrast(t.get("窗体字体色"), t.get("窗体背景色")) >= 4.5);
        assert!(contrast(t.get("光标色"), t.get("窗体背景色")) >= 3.0);
    }
}
