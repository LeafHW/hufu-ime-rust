//! Shared complete physical keyboard for measured and theoretical heatmaps.
use crate::ui::{alpha, round, tokens};
use gale_core::theme::Theme;
use gale_win::d2d::{Align, DrawCmd, FontFamily, Frame};

/// Internal histogram key for extended VK_RETURN; ordinary Enter remains 0x0D.
pub const NUMPAD_ENTER: u8 = 0x0E;

#[derive(Debug, Clone, Copy, Default)]
pub struct KeyboardSummary {
    pub total: u64,
    pub max: u32,
}

#[derive(Debug, Clone, Copy)]
struct Key {
    label: &'static str,
    vk: u8,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

fn layout() -> Vec<Key> {
    let mut keys = Vec::with_capacity(105);
    let mut add = |label, vk, x, y, w, h| {
        keys.push(Key {
            label,
            vk,
            x,
            y,
            w,
            h,
        })
    };
    add("Esc", 0x1B, 0., 0., 1., 0.82);
    for (i, label) in [
        "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
    ]
    .into_iter()
    .enumerate()
    {
        let gap = (i / 4) as f32 * 0.5;
        add(label, 0x70 + i as u8, 2. + i as f32 + gap, 0., 1., 0.82);
    }
    // Print/Scroll/Pause get a wider cap so their full short names stay legible
    // instead of the old 3-character stumps (Prt/Scr/Brk).
    for (i, (label, vk)) in [("PrtSc", 0x2C), ("ScrLk", 0x91), ("Pause", 0x13)]
        .into_iter()
        .enumerate()
    {
        add(label, vk, 15.5 + i as f32 * 1.5, 0., 1.5, 0.82);
    }
    for (i, (label, vk)) in [
        ("` ~", 0xC0),
        ("1", 0x31),
        ("2", 0x32),
        ("3", 0x33),
        ("4", 0x34),
        ("5", 0x35),
        ("6", 0x36),
        ("7", 0x37),
        ("8", 0x38),
        ("9", 0x39),
        ("0", 0x30),
        ("- _", 0xBD),
        ("= +", 0xBB),
    ]
    .into_iter()
    .enumerate()
    {
        add(label, vk, i as f32, 1.2, 1., 1.);
    }
    add("Backspace", 0x08, 13., 1.2, 2., 1.);
    add("Tab", 0x09, 0., 2.2, 1.5, 1.);
    for (i, (label, vk)) in [
        ("Q", b'Q'),
        ("W", b'W'),
        ("E", b'E'),
        ("R", b'R'),
        ("T", b'T'),
        ("Y", b'Y'),
        ("U", b'U'),
        ("I", b'I'),
        ("O", b'O'),
        ("P", b'P'),
        ("[ {", 0xDB),
        ("] }", 0xDD),
    ]
    .into_iter()
    .enumerate()
    {
        add(label, vk, 1.5 + i as f32, 2.2, 1., 1.);
    }
    add("\\ |", 0xDC, 13.5, 2.2, 1.5, 1.);
    add("Caps", 0x14, 0., 3.2, 1.75, 1.);
    for (i, (label, vk)) in [
        ("A", b'A'),
        ("S", b'S'),
        ("D", b'D'),
        ("F", b'F'),
        ("G", b'G'),
        ("H", b'H'),
        ("J", b'J'),
        ("K", b'K'),
        ("L", b'L'),
        ("; :", 0xBA),
        ("' \"", 0xDE),
    ]
    .into_iter()
    .enumerate()
    {
        add(label, vk, 1.75 + i as f32, 3.2, 1., 1.);
    }
    add("Enter", 0x0D, 12.75, 3.2, 2.25, 1.);
    // Left/right are named explicitly: selection keys are often mapped to one
    // physical Shift/Ctrl/Alt/Win, so an identical label hid which side counted.
    // The ISO OEM102 key remains visible; a US keyboard simply leaves it at zero.
    add("LShift", 0xA0, 0., 4.2, 1.25, 1.);
    add("ISO", 0xE2, 1.25, 4.2, 1., 1.);
    for (i, (label, vk)) in [
        ("Z", b'Z'),
        ("X", b'X'),
        ("C", b'C'),
        ("V", b'V'),
        ("B", b'B'),
        ("N", b'N'),
        ("M", b'M'),
        (", <", 0xBC),
        (". >", 0xBE),
        ("/ ?", 0xBF),
    ]
    .into_iter()
    .enumerate()
    {
        add(label, vk, 2.25 + i as f32, 4.2, 1., 1.);
    }
    add("RShift", 0xA1, 12.25, 4.2, 2.75, 1.);
    for (label, vk, x, w) in [
        ("LCtrl", 0xA2, 0., 1.25),
        ("LWin", 0x5B, 1.25, 1.25),
        ("LAlt", 0xA4, 2.5, 1.25),
        ("Space", 0x20, 3.75, 6.25),
        ("RAlt", 0xA5, 10., 1.25),
        ("RWin", 0x5C, 11.25, 1.25),
        ("Menu", 0x5D, 12.5, 1.25),
        ("RCtrl", 0xA3, 13.75, 1.25),
    ] {
        add(label, vk, x, 5.2, w, 1.);
    }
    for (r, row) in [
        [("Ins", 0x2D), ("Home", 0x24), ("PgUp", 0x21)],
        [("Del", 0x2E), ("End", 0x23), ("PgDn", 0x22)],
    ]
    .into_iter()
    .enumerate()
    {
        for (i, (label, vk)) in row.into_iter().enumerate() {
            add(label, vk, 15.5 + i as f32, 1.2 + r as f32, 1., 1.);
        }
    }
    add("↑", 0x26, 16.5, 4.2, 1., 1.);
    for (i, (label, vk)) in [("←", 0x25), ("↓", 0x28), ("→", 0x27)]
        .into_iter()
        .enumerate()
    {
        add(label, vk, 15.5 + i as f32, 5.2, 1., 1.);
    }
    for (i, (label, vk)) in [("Num", 0x90), ("/", 0x6F), ("*", 0x6A), ("-", 0x6D)]
        .into_iter()
        .enumerate()
    {
        add(label, vk, 19. + i as f32, 1.2, 1., 1.);
    }
    for (r, row) in [
        [("7", 0x67), ("8", 0x68), ("9", 0x69)],
        [("4", 0x64), ("5", 0x65), ("6", 0x66)],
        [("1", 0x61), ("2", 0x62), ("3", 0x63)],
    ]
    .into_iter()
    .enumerate()
    {
        for (i, (label, vk)) in row.into_iter().enumerate() {
            add(label, vk, 19. + i as f32, 2.2 + r as f32, 1., 1.);
        }
    }
    add("+", 0x6B, 22., 2.2, 1., 2.);
    add("Enter", NUMPAD_ENTER, 22., 4.2, 1., 2.);
    add("0", 0x60, 19., 5.2, 2., 1.);
    add(".", 0x6E, 21., 5.2, 1., 1.);
    keys
}

/// Draws only the keyboard, so callers control their own title and legend.
/// Both HashMap and BTreeMap work through `|vk| counts.get(&vk).copied().unwrap_or(0)`.
/// Short names keep narrow keycaps readable; the caller supplies the complete
/// visible rectangle so the keyboard never scrolls behind a panel footer.
pub fn draw(
    f: &mut Frame,
    t: &Theme,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    count: impl Fn(u8) -> u32,
) -> KeyboardSummary {
    draw_impl(f, t, x, y, w, h, count, false)
}

/// 只画红色框里的主键盘区（Esc/F 行 + 字母数字区），不含右侧功能键与数字小键盘。
/// 用于「当前段按键热力图」：键帽上直接显示该键按下次数。
pub fn draw_main(
    f: &mut Frame,
    t: &Theme,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    count: impl Fn(u8) -> u32,
) -> KeyboardSummary {
    draw_impl(f, t, x, y, w, h, count, true)
}

fn draw_impl(
    f: &mut Frame,
    t: &Theme,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    count: impl Fn(u8) -> u32,
    main_only: bool,
) -> KeyboardSummary {
    let mut counts = [0u32; 256];
    for vk in 0..=255 {
        counts[vk] = count(vk as u8);
    }
    // Theoretical analysis has no physical side. Put its generic modifier on
    // the left key without showing the same aggregate twice.
    for (generic, left, right) in [(0x10, 0xA0, 0xA1), (0x11, 0xA2, 0xA3), (0x12, 0xA4, 0xA5)] {
        if counts[left] == 0 && counts[right] == 0 {
            counts[left] = counts[generic];
        }
        counts[generic] = 0;
    }
    let all_keys = layout();
    let summary = KeyboardSummary {
        total: counts.iter().map(|n| *n as u64).sum(),
        max: all_keys
            .iter()
            .map(|k| counts[k.vk as usize])
            .max()
            .unwrap_or(0),
    };
    // 640-wide windows cannot show 105 caps and keep their labels legible.
    // Drop the F-row and the nav/numpad cluster, then scale the main block up.
    let compact = !main_only && w < 760.;
    let keys: Vec<_> = if main_only {
        all_keys.into_iter().filter(|k| k.x < 15.2).collect()
    } else if compact {
        all_keys
            .into_iter()
            .filter(|k| k.y >= 1.2 && k.x < 15.2)
            .collect()
    } else {
        all_keys
    };
    let (mut kx0, mut kx1, mut ky0, mut ky1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for k in &keys {
        kx0 = kx0.min(k.x);
        kx1 = kx1.max(k.x + k.w);
        ky0 = ky0.min(k.y);
        ky1 = ky1.max(k.y + k.h);
    }
    let p = tokens(t);
    let ux = if main_only { w.max(0.) / 15.2 } else { w.max(0.) / 23. };
    let uy = h.max(0.) / 6.2;
    let mut gap = (ux * 0.08).clamp(2., 4.);
    let mut unit = 0.;
    let (mut ox, mut oy) = (x, y);
    if compact {
        unit = ((w - 12.) / (kx1 - kx0).max(1.))
            .min((h - 12.) / (ky1 - ky0).max(1.))
            .max(1.);
        ox = x + (w - (kx1 - kx0) * unit) / 2. - kx0 * unit;
        oy = y + (h - (ky1 - ky0) * unit) / 2. - ky0 * unit;
        gap = (unit * 0.08).clamp(2., 4.);
    }
    for key in keys {
        let (kx, ky, kw, kh) = if compact {
            (
                ox + key.x * unit,
                oy + key.y * unit,
                (key.w * unit - gap).max(0.),
                (key.h * unit - gap).max(0.),
            )
        } else {
            let kx = x + key.x * ux;
            let ky = y + key.y * uy;
            let key_height = if key.y == 0. && h < 260. { 1. } else { key.h };
            (
                kx,
                ky,
                (key.w * ux - gap).max(0.),
                (key_height * uy - gap).max(0.),
            )
        };
        let n = counts[key.vk as usize];
        let ratio = if summary.max == 0 {
            0.
        } else {
            (n as f32 / summary.max as f32).sqrt()
        };
        let fill = mix(p.surface2, p.accent, ratio * 0.86);
        let luminance = fill[0] * 0.2126 + fill[1] * 0.7152 + fill[2] * 0.0722;
        let fg = if n == 0 {
            p.text2
        } else if luminance > 0.53 {
            [0.08, 0.10, 0.10, 1.]
        } else {
            [0.98, 0.98, 0.98, 1.]
        };
        round(f, kx, ky, kw, kh, 4., fill, Some(alpha(p.line, 0.8)));
        let (label, mut label_size) = key_label(key.label, kw);
        if main_only {
            label_size = label_size.min(13.);
        }
        // Only draw a count when there is one, and stack the label above it so
        // both lines share the same centre line on every cap.
        let show_count = (main_only || key_show_count(kh) || compact) && n > 0;
        let count_size = if main_only { 11. } else if compact { 12. } else { 14. };
        let block = if show_count {
            label_size + 1. + count_size
        } else {
            label_size
        };
        let top = ky + ((kh - block) / 2.).max(0.);
        f.push(DrawCmd::PushClip {
            x: kx,
            y: ky,
            w: kw,
            h: kh,
        });
        centered(f, kx + kw / 2., top, label_size, label.into(), fg);
        if show_count {
            centered(
                f,
                kx + kw / 2.,
                top + label_size + 1.,
                count_size,
                n.to_string(),
                fg,
            );
        }
        f.push(DrawCmd::PopClip);
    }
    summary
}
fn key_show_count(key_h: f32) -> bool {
    key_h >= 40.
}
/// Approximate UI advance width; only used to choose between the full label and
/// a compact one. ASCII runs use the same 0.58 factor as the fixed-width chips.
/// Per-character advance estimate for the UI font. A flat factor either let
/// Num/Menu clip (too small) or abbreviated LShift needlessly (too large).
fn char_width(c: char, size: f32) -> f32 {
    let em = match c {
        ' ' => 0.38,
        'M' | 'W' => 0.92,
        'm' | 'w' => 0.78,
        'i' | 'l' | 'j' | 'f' | 't' | 'I' => 0.42,
        '[' | ']' | '(' | ')' | '!' | '.' | ',' | ';' | ':' | '\'' | '"' | '|' => 0.42,
        c if c.is_ascii_uppercase() => 0.68,
        c if c.is_ascii_lowercase() => 0.56,
        c if c.is_ascii_digit() => 0.58,
        _ => 0.6,
    };
    em * size
}
fn label_width(text: &str, size: f32) -> f32 {
    text.chars().map(|c| char_width(c, size)).sum()
}
fn fits(label: &str, width: f32, size: f32) -> bool {
    let pad = if label.chars().count() >= 3 { 5. } else { 8. };
    label_width(label, size) <= (width - pad).max(0.)
}
/// Pick the widest label that fits, shrinking to 13/12 DIP first, then falling
/// back to a compact spelling (and a two-letter form on 800/640 windows).
fn key_label(label: &str, width: f32) -> (&str, f32) {
    for size in [14., 13., 12.] {
        if fits(label, width, size) {
            return (label, size);
        }
    }
    let short = match label {
        "Backspace" => "Bksp",
        "LShift" => "LSh",
        "RShift" => "RSh",
        "LCtrl" => "LCt",
        "RCtrl" => "RCt",
        "Home" => "Hm",
        "PgUp" => "PgU",
        "PgDn" => "PgD",
        "Enter" => "Ent",
        "Menu" => "Men",
        "PrtSc" => "Prt",
        "ScrLk" => "Scr",
        "Pause" => "Pau",
        _ => label,
    };
    for size in [14., 13., 12.] {
        if fits(short, width, size) {
            return (short, size);
        }
    }
    let ultra = match label {
        "Backspace" => "Bk",
        "LShift" => "LS",
        "RShift" => "RS",
        "LCtrl" => "LC",
        "RCtrl" => "RC",
        "LAlt" => "LA",
        "RAlt" => "RA",
        "LWin" => "LW",
        "RWin" => "RW",
        "PgUp" => "PU",
        "PgDn" => "PD",
        "Enter" => "↵",
        "PrtSc" => "Pr",
        "ScrLk" => "Sc",
        "Pause" => "Pa",
        _ => short,
    };
    for size in [13., 12., 11.] {
        if fits(ultra, width, size) {
            return (ultra, size);
        }
    }
    (ultra, 11.)
}
fn centered(f: &mut Frame, x: f32, y: f32, size: f32, text: String, color: [f32; 4]) {
    f.push(DrawCmd::TextRun {
        x,
        y,
        h: size,
        text,
        color,
        font: FontFamily::Ui,
        align: Align::Center,
    });
}
fn mix(a: [f32; 4], b: [f32; 4], amount: f32) -> [f32; 4] {
    let mut out = [0.; 4];
    for i in 0..4 {
        out[i] = a[i] + (b[i] - a[i]) * amount;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_keyboard_and_counts_fit_compact_visible_rectangles() {
        assert_eq!(key_label("Backspace", 60.).0, "Bksp");
        let theme = crate::ui::theme("墨玉");
        for (w, h) in [(712., 366.), (552., 222.)] {
            let mut frame = Frame::new(w, h);
            draw(&mut frame, &theme, 0., 0., w, h, |_| 7);
            let text: Vec<_> = frame
                .cmds
                .iter()
                .filter_map(|command| match command {
                    DrawCmd::TextRun { y, h, text, .. } => Some((*y, *h, text.as_str())),
                    _ => None,
                })
                .collect();
            let compact = w < 760.;
            let expected: usize = layout()
                .iter()
                .filter(|k| !compact || (k.y >= 1.2 && k.x < 15.2))
                .map(|k| {
                    let kh = ((if k.y == 0. && h < 260. { 1. } else { k.h }) * (h / 6.2)
                        - ((w / 23.) * 0.08).clamp(2., 4.))
                    .max(0.);
                    1 + usize::from(compact || key_show_count(kh))
                })
                .sum();
            assert_eq!(
                text.len(),
                expected,
                "every key keeps its label (and count when it fits) at {w}×{h}"
            );
            for (y, font, label) in text {
                assert!(y >= 0. && y + font <= h + 0.01, "{label} outside {w}×{h}");
            }
            for command in &frame.cmds {
                if let DrawCmd::PushClip { x, y, w: cw, h: ch } = command {
                    assert!(*x >= 0. && *y >= 0. && x + cw <= w + 0.01 && y + ch <= h + 0.01);
                }
            }
        }
        assert_eq!(key_label("Home", 40.).0, "Home");
        assert_eq!(key_label("LShift", 50.).0, "LShift");
        assert!(key_label("Num", 26.).0.len() <= 3);
        assert!(key_label("PgUp", 26.).0.len() <= 3);
    }
    #[test]
    fn keyboard_has_unique_complete_nonoverlapping_physical_keys() {
        let keys = layout();
        let mut seen = std::collections::BTreeSet::new();
        for key in &keys {
            assert!(seen.insert(key.vk), "duplicate VK {}", key.vk);
            assert!(key.x >= 0. && key.y >= 0. && key.x + key.w <= 23.01 && key.y + key.h <= 6.21);
        }
        for vk in [
            0x1B,
            0x09,
            0x14,
            0xA0,
            0xA1,
            0xA2,
            0xA3,
            0xA4,
            0xA5,
            0x5B,
            0x5C,
            0x20,
            0x0D,
            0x08,
            0xE2,
            0x60,
            0x69,
            0x6B,
            0x6F,
            NUMPAD_ENTER,
        ] {
            assert!(seen.contains(&vk), "missing VK {vk}");
        }
        for vk in 0x70..=0x7B {
            assert!(seen.contains(&vk));
        }
        for (i, a) in keys.iter().enumerate() {
            for b in &keys[i + 1..] {
                let dx = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
                let dy = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
                assert!(
                    dx <= 0.001 || dy <= 0.001,
                    "overlap {} / {}",
                    a.label,
                    b.label
                );
            }
        }
    }
}
