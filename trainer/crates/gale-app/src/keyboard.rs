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
    // 【审计 V03】键集合只决定「画哪些键」。键帽矩形、字号与是否降级全部交给
    // 纯函数 heat_layout()：它先缩键帽/字号（字号 ≥10、键帽高 ≥18），640 宽小窗
    // 里两行「键名 + 次数」放不下时降级成热区行，绝不再靠 PushClip 硬裁文字。
    let heat = heat_layout(w.max(0.), h.max(0.), &keys, &counts);
    let p = tokens(t);
    for cap in &heat.caps {
        let n = cap.count;
        let ratio = if summary.max == 0 {
            0.
        } else {
            (n as f32 / summary.max as f32).sqrt()
        };
        // 【审计 V03】降级行里的「其余 N 键」是说明格，不参与热力配色。
        let fill = if cap.rest {
            p.surface2
        } else {
            mix(p.surface2, p.accent, ratio * 0.86)
        };
        let luminance = fill[0] * 0.2126 + fill[1] * 0.7152 + fill[2] * 0.0722;
        let fg = if cap.rest || n == 0 {
            p.text2
        } else if luminance > 0.53 {
            [0.08, 0.10, 0.10, 1.]
        } else {
            [0.98, 0.98, 0.98, 1.]
        };
        round(
            f,
            x + cap.x,
            y + cap.y,
            cap.w,
            cap.h,
            4.,
            fill,
            Some(alpha(p.line, 0.8)),
        );
        match cap.align {
            HeatAlign::Center => {
                // 两行共用同一条中心线；block 在布局阶段已保证装得进键帽
                // （含 HEAT_PAD_Y 的上下留白），所以不会被裁。
                let block = if cap.show_count {
                    cap.label_size + HEAT_LINE_GAP + cap.count_size
                } else {
                    cap.label_size
                };
                let top = y + cap.y + ((cap.h - block) / 2.).max(0.);
                f.push(DrawCmd::PushClip {
                    x: x + cap.x,
                    y: y + cap.y,
                    w: cap.w,
                    h: cap.h,
                });
                centered(
                    f,
                    x + cap.x + cap.w / 2.,
                    top,
                    cap.label_size,
                    cap.label.clone(),
                    fg,
                );
                if cap.show_count {
                    centered(
                        f,
                        x + cap.x + cap.w / 2.,
                        top + cap.label_size + HEAT_LINE_GAP,
                        cap.count_size,
                        cap.count_text.clone(),
                        fg,
                    );
                }
                f.push(DrawCmd::PopClip);
            }
            HeatAlign::Split => {
                // 【审计 V03】降级行：键名左对齐、次数右对齐，同一行排开，
                // 不叠行也就不会出现上下挤压。
                let label_y = y + cap.y + ((cap.h - cap.label_size) / 2.).max(0.);
                left_text(
                    f,
                    x + cap.x + HEAT_PAD_X,
                    label_y,
                    cap.label_size,
                    cap.label.clone(),
                    fg,
                );
                if cap.show_count {
                    let count_y = y + cap.y + ((cap.h - cap.count_size) / 2.).max(0.);
                    right_text(
                        f,
                        x + cap.x + cap.w - HEAT_PAD_X,
                        count_y,
                        cap.count_size,
                        cap.count_text.clone(),
                        fg,
                    );
                }
            }
        }
    }
    summary
}
// ─────────────────────── 【审计 V03】热力图自适应布局 ───────────────────────
// 旧实现把键帽尺寸与字号写成两条互不相干的常量规则：ux/uy 按可用宽高各自做
// 比例、字号写死 11~14，画完再靠 PushClip 硬裁。小窗（640×420 一类最小窗）
// 里两行「键名 + 次数」高过键帽、5 位以上的次数也超出键帽宽，被裁掉后既看
// 不清也读不到。现在把「可用宽高 + 键集合 → 键帽矩形 / 字号 / 是否降级」抽成
// 纯函数 heat_layout()，渲染与单测共用同一套数字：字号与键帽尺寸互相约束，
// 放不下就降级成「只列有数据的热区键 + 其余合并成一格」，不再让文字被裁。

/// 【审计 V03】字号下限：键名与次数都不小于 10px（审计验收口径）。
const HEAT_MIN_FONT: f32 = 10.;
/// 【审计 V03】键帽高度下限：低于 18px 时两行文字必然挤压，直接降级。
const HEAT_MIN_CAP_H: f32 = 18.;
/// 【审计 V03】键帽内文字左右留白。
const HEAT_PAD_X: f32 = 3.;
/// 【审计 V03】键帽内文字上下留白。
const HEAT_PAD_Y: f32 = 1.;
/// 【审计 V03】键帽内两行文字之间的行距。
const HEAT_LINE_GAP: f32 = 1.;

/// 【审计 V03】热力图的两种呈现形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeatMode {
    /// 正常键盘网格：每个键帽两行（键名在上、次数在下）。
    Grid,
    /// 降级：网格在最小字号/最小键帽高下仍放不下两行文字，
    /// 改为一行一个热区键（键名左、次数右），其余键合并成最后一格说明。
    Rows,
}

/// 【审计 V03】文字对齐：Center=键帽内两行居中；Split=降级行的单行左右分列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeatAlign {
    Center,
    Split,
}

/// 【审计 V03】一个热力格子的最终绘制规格（坐标相对可用矩形左上角）。
#[derive(Debug, Clone, PartialEq)]
struct HeatCap {
    vk: u8,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: String,
    label_size: f32,
    count: u32,
    count_text: String,
    count_size: f32,
    show_count: bool,
    align: HeatAlign,
    /// 降级行的「其余 N 键」说明格：不参与热力配色。
    rest: bool,
}

/// 【审计 V03】一次布局的完整结果：矩形/字号/是否降级，全部可单测断言。
#[derive(Debug, Clone)]
struct HeatLayout {
    mode: HeatMode,
    caps: Vec<HeatCap>,
    /// 网格统一字号（逐键最多缩到 HEAT_MIN_FONT，不会大于这两个值）。
    label_size: f32,
    count_size: f32,
    /// 1 单位键帽的实际尺寸与间距（降级行时为 0）。
    cap_w: f32,
    cap_h: f32,
    gap: f32,
    degraded: bool,
}

/// 【审计 V03】给定可用宽高与键集合，算出每个格子的矩形、字号与是否降级。
/// 纯函数：不碰 Frame / Theme，单测可直接断言数字（640×420 下无重叠、字号 ≥10）。
fn heat_layout(w: f32, h: f32, keys: &[Key], counts: &[u32; 256]) -> HeatLayout {
    let empty = HeatLayout {
        mode: HeatMode::Rows,
        caps: Vec::new(),
        label_size: HEAT_MIN_FONT,
        count_size: HEAT_MIN_FONT,
        cap_w: 0.,
        cap_h: 0.,
        gap: 0.,
        degraded: true,
    };
    if keys.is_empty() || !w.is_finite() || !h.is_finite() || w <= 0. || h <= 0. {
        return empty;
    }
    // 归一化包围盒：F 行只有 0.82 个单位高，按 1 个单位参与计算，
    // 否则矮面板里 F 行会先被压成一条线。
    let (mut kx0, mut kx1, mut ky0, mut ky1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for key in keys {
        kx0 = kx0.min(key.x);
        kx1 = kx1.max(key.x + key.w);
        ky0 = ky0.min(key.y);
        ky1 = ky1.max(key.y + key.h);
    }
    let span_x = (kx1 - kx0).max(0.001);
    let span_y = (ky1 - ky0).max(0.001);
    // 宽高同比例缩放：键帽保持真实键盘的长宽比，剩余空间均匀留在两侧。
    let unit = (w / span_x).min(h / span_y);
    if !(unit > 0.) {
        return empty;
    }
    let gap = (unit * 0.08).clamp(2., 4.);
    let ox = (w - span_x * unit) / 2. - kx0 * unit;
    let oy = (h - span_y * unit) / 2. - ky0 * unit;
    // 字号从 14 起逐档下调，直到「最小键帽高装得下两行、且每格文字都装得下」；
    // 到 10px 还装不下才降级——绝不带着放不下的字号去绘制。
    for font in [14., 13., 12., 11., HEAT_MIN_FONT] {
        let count_size = (font - 2.).max(HEAT_MIN_FONT);
        let need = font + HEAT_LINE_GAP + count_size + HEAT_PAD_Y * 2.;
        let mut caps = Vec::with_capacity(keys.len());
        let mut ok = true;
        for key in keys {
            let cap_h = (if key.y <= 0.001 { 1. } else { key.h }) * unit - gap;
            let cap_w = key.w * unit - gap;
            if cap_h < HEAT_MIN_CAP_H || cap_h + 0.001 < need {
                ok = false;
                break;
            }
            let text_w = cap_w - HEAT_PAD_X * 2.;
            let Some((label, label_size)) = heat_label(key.label, text_w, font) else {
                ok = false;
                break;
            };
            let n = counts[key.vk as usize];
            let (count_text, show_count) = if n > 0 {
                let (text, fits) = heat_count_text(n, text_w, count_size);
                if !fits {
                    ok = false;
                    break;
                }
                (text, true)
            } else {
                (String::new(), false)
            };
            caps.push(HeatCap {
                vk: key.vk,
                x: ox + key.x * unit + gap / 2.,
                y: oy + key.y * unit + gap / 2.,
                w: cap_w.max(0.),
                h: cap_h.max(0.),
                label,
                label_size,
                count: n,
                count_text,
                count_size,
                show_count,
                align: HeatAlign::Center,
                rest: false,
            });
        }
        if ok && caps.len() == keys.len() {
            return HeatLayout {
                mode: HeatMode::Grid,
                caps,
                label_size: font,
                count_size,
                cap_w: unit - gap,
                cap_h: unit - gap,
                gap,
                degraded: false,
            };
        }
    }
    heat_rows(w, h, keys, counts)
}

/// 【审计 V03】降级形态：只列有数据的热区键（键名 + 次数一行一格），
/// 其余键合并成最后一格「其余 N 键」；行高不够先压行高再降字号，始终 ≥10px。
fn heat_rows(w: f32, h: f32, keys: &[Key], counts: &[u32; 256]) -> HeatLayout {
    let gap = 4.;
    let min_row = HEAT_MIN_FONT + 2.;
    let mut row_h = 22.;
    let mut rows_fit = ((h + gap) / (row_h + gap)).floor().max(0.) as usize;
    if rows_fit == 0 && h >= min_row {
        // 极矮：把行高压到可用高度，至少留一行。
        row_h = h;
        rows_fit = 1;
    }
    let mut layout = HeatLayout {
        mode: HeatMode::Rows,
        caps: Vec::new(),
        label_size: HEAT_MIN_FONT,
        count_size: HEAT_MIN_FONT,
        cap_w: 0.,
        cap_h: 0.,
        gap,
        degraded: true,
    };
    if rows_fit == 0 {
        return layout;
    }
    let (label_size, count_size) = if row_h >= 20. {
        (12., 12.)
    } else if row_h >= 15. {
        (11., 11.)
    } else {
        (HEAT_MIN_FONT, HEAT_MIN_FONT)
    };
    layout.label_size = label_size;
    layout.count_size = count_size;
    // 热区键：有数据的键按次数降序，最热的排最上面。
    let mut hot: Vec<(u32, &Key)> = keys
        .iter()
        .filter(|key| counts[key.vk as usize] > 0)
        .map(|key| (counts[key.vk as usize], key))
        .collect();
    hot.sort_by(|a, b| b.0.cmp(&a.0));
    let (shown, _folded) = if hot.len() <= rows_fit {
        (hot.len(), 0)
    } else if rows_fit >= 2 {
        // 留最后一格给「其余 N 键」。
        (rows_fit - 1, hot.len() - (rows_fit - 1))
    } else {
        (rows_fit.min(hot.len()), 0)
    };
    let rest_keys = keys.len().saturating_sub(shown);
    let mut rest_total = 0u32;
    for entry in hot.iter().skip(shown) {
        rest_total = rest_total.saturating_add(entry.0);
    }
    let label_w = (w * 0.62 - HEAT_PAD_X * 2.).max(1.);
    let count_w = (w * 0.38 - HEAT_PAD_X * 2.).max(1.);
    let mut rows: Vec<(String, f32, u32, String, bool, bool)> = Vec::new();
    if hot.is_empty() {
        // 完全没有数据：给一行说明，避免整块空白。
        let label = "本段暂无按键数据";
        let (text, size) = heat_label(label, label_w, label_size)
            .unwrap_or_else(|| (crate::ui::elide_width(label, label_w, HEAT_MIN_FONT), HEAT_MIN_FONT));
        rows.push((text, size, 0, String::new(), false, true));
    } else {
        for (n, key) in hot.iter().take(shown) {
            let (label, size) = heat_label(key.label, label_w, label_size).unwrap_or_else(|| {
                (crate::ui::elide_width(key.label, label_w, HEAT_MIN_FONT), HEAT_MIN_FONT)
            });
            let (count_text, show_count) = heat_count_text(*n, count_w, count_size);
            rows.push((label, size, *n, count_text, show_count, false));
        }
        if rest_keys > 0 && rows.len() < rows_fit {
            let label = format!("其余 {rest_keys} 键");
            let (text, size) = heat_label(&label, label_w, label_size)
                .unwrap_or_else(|| (crate::ui::elide_width(&label, label_w, HEAT_MIN_FONT), HEAT_MIN_FONT));
            let (count_text, show_count) = heat_count_text(rest_total, count_w, count_size);
            rows.push((text, size, rest_total, count_text, show_count, true));
        }
    }
    // 行数少时整体垂直居中，不顶在面板最上面。
    let rows_n = rows.len().max(1) as f32;
    let total_h = rows_n * row_h + (rows_n - 1.) * gap;
    let oy = ((h - total_h) / 2.).max(0.);
    for (i, (label, label_size, n, count_text, show_count, rest)) in rows.into_iter().enumerate() {
        layout.caps.push(HeatCap {
            vk: 0,
            x: 0.,
            y: oy + i as f32 * (row_h + gap),
            w,
            h: row_h,
            label,
            label_size,
            count: n,
            count_text,
            count_size,
            show_count,
            align: HeatAlign::Split,
            rest,
        });
    }
    layout
}

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
/// 【审计 V03】保守字宽：键盘自带的 em 因子与全局 ui_text_width 取较大者，
/// 两个口径都装得下才算装得下（渲染字体的实际 advance 介于两者之间）。
fn heat_text_width(text: &str, size: f32) -> f32 {
    label_width(text, size).max(crate::ui::ui_text_width(text, size))
}

/// 【审计 V03】挑一个「装得进 max_w 且字号不小于 10px」的键名：字号从大到小，
/// 每档先试全称再试短写与两字写法；返回 None 表示怎么缩都装不下（交给降级）。
fn heat_label(label: &str, max_w: f32, max_size: f32) -> Option<(String, f32)> {
    if max_w <= 0. {
        return None;
    }
    let variants = [label, short_label(label), ultra_label(label)];
    let mut size = max_size.max(HEAT_MIN_FONT);
    while size >= HEAT_MIN_FONT - 0.001 {
        for variant in variants {
            if heat_text_width(variant, size) <= max_w {
                return Some((variant.to_string(), size));
            }
        }
        size -= 1.;
    }
    None
}

/// 【审计 V03】次数文本：先试原数字，装不下再退到 12.3k / 12k / 1.2万 / 1万 / 1.2M。
fn heat_count_text(n: u32, max_w: f32, size: f32) -> (String, bool) {
    let mut candidates = vec![n.to_string()];
    if n >= 1_000 {
        candidates.push(format!("{:.1}k", n as f32 / 1_000.));
        candidates.push(format!("{}k", n / 1_000));
    }
    if n >= 10_000 {
        candidates.push(format!("{:.1}万", n as f32 / 10_000.));
        candidates.push(format!("{}万", n / 10_000));
    }
    if n >= 1_000_000 {
        candidates.push(format!("{:.1}M", n as f32 / 1_000_000.));
        candidates.push(format!("{}M", n / 1_000_000));
    }
    for text in candidates {
        if heat_text_width(&text, size) <= max_w {
            return (text, true);
        }
    }
    (String::new(), false)
}

/// 键名短写（沿用原有缩写表，不引入新写法）。
fn short_label(label: &str) -> &str {
    match label {
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
    }
}
/// 键名两字写法（更窄的键帽才用）。
fn ultra_label(label: &str) -> &str {
    match label {
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
        _ => short_label(label),
    }
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
/// 【审计 V03】降级行左侧键名（左对齐）。
fn left_text(f: &mut Frame, x: f32, y: f32, size: f32, text: String, color: [f32; 4]) {
    f.push(DrawCmd::TextRun {
        x,
        y,
        h: size,
        text,
        color,
        font: FontFamily::Ui,
        align: Align::Left,
    });
}
/// 【审计 V03】降级行右侧次数（右对齐）。
fn right_text(f: &mut Frame, x: f32, y: f32, size: f32, text: String, color: [f32; 4]) {
    f.push(DrawCmd::TextRun {
        x,
        y,
        h: size,
        text,
        color,
        font: FontFamily::Ui,
        align: Align::Right,
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
    /// 主键区键集合（draw_main / 物理热力图面板实际使用的键集）。
    fn main_keys() -> Vec<Key> {
        layout().into_iter().filter(|k| k.x < 15.2).collect()
    }
    /// 紧凑形态的键集合（draw 在窄窗里丢掉 F 行与右侧功能键后的键集）。
    fn compact_keys() -> Vec<Key> {
        layout()
            .into_iter()
            .filter(|k| k.y >= 1.2 && k.x < 15.2)
            .collect()
    }
    /// 逐键给一个 1..=max 的次数，模拟真实热力图（每个键都有数据）。
    fn busy_counts(keys: &[Key], max: u32) -> [u32; 256] {
        let mut counts = [0u32; 256];
        for (i, key) in keys.iter().enumerate() {
            counts[key.vk as usize] = 1 + (i as u32 * 37) % max.max(1);
        }
        counts
    }
    /// 【审计 V03】布局契约：格子不越出可用矩形、互不重叠、字号 ≥10、
    /// 每格文字宽度装得下、两行高度装得下——任何尺寸都必须成立。
    fn assert_heat_contract(layout: &HeatLayout, w: f32, h: f32) {
        assert!(
            !layout.caps.is_empty(),
            "{w}×{h} 应至少产出一个可读格子"
        );
        let mut rects = Vec::new();
        for cap in &layout.caps {
            assert!(cap.x >= -0.01 && cap.y >= -0.01, "{:?} 越左/上界", cap.label);
            assert!(
                cap.x + cap.w <= w + 0.01,
                "{w}×{h} 的 {:?} 越右界",
                cap.label
            );
            assert!(
                cap.y + cap.h <= h + 0.01,
                "{w}×{h} 的 {:?} 越下界",
                cap.label
            );
            assert!(
                cap.label_size >= HEAT_MIN_FONT - 0.001,
                "{:?} 字号 {} 低于 10px 下限",
                cap.label,
                cap.label_size
            );
            let text_w = cap.w - HEAT_PAD_X * 2. + 0.01;
            assert!(
                heat_text_width(&cap.label, cap.label_size) <= text_w,
                "{w}×{h} 的 {:?} 键名 {:.2}px 装不进 {:.2}px 键帽",
                cap.label,
                heat_text_width(&cap.label, cap.label_size),
                cap.w
            );
            match cap.align {
                HeatAlign::Center => {
                    if cap.show_count {
                        assert!(cap.count_size >= HEAT_MIN_FONT - 0.001);
                        assert!(
                            heat_text_width(&cap.count_text, cap.count_size) <= text_w,
                            "{w}×{h} 的 {:?} 次数 {:?} 装不进 {:.2}px 键帽",
                            cap.label,
                            cap.count_text,
                            cap.w
                        );
                        let block = cap.label_size + HEAT_LINE_GAP + cap.count_size;
                        assert!(
                            block <= cap.h - HEAT_PAD_Y * 2. + 0.01,
                            "{w}×{h} 的 {:?} 两行 {:.2}px 高过 {:.2}px 键帽",
                            cap.label,
                            block,
                            cap.h
                        );
                    } else {
                        assert!(
                            cap.label_size <= cap.h - HEAT_PAD_Y * 2. + 0.01,
                            "{w}×{h} 的 {:?} 单行也高过键帽",
                            cap.label
                        );
                    }
                    if layout.mode == HeatMode::Grid {
                        assert!(
                            cap.h >= HEAT_MIN_CAP_H - 0.001,
                            "{w}×{h} 的 {:?} 键帽高 {:.2} 低于 18px 下限",
                            cap.label,
                            cap.h
                        );
                    }
                }
                HeatAlign::Split => {
                    // 左键名 + 右次数不得在水平方向相撞。
                    let label_w = heat_text_width(&cap.label, cap.label_size);
                    let count_w = if cap.show_count {
                        heat_text_width(&cap.count_text, cap.count_size)
                    } else {
                        0.
                    };
                    assert!(
                        label_w + count_w + HEAT_PAD_X * 2. <= cap.w + 0.01,
                        "{w}×{h} 降级行 {:?} 的键名与次数相撞",
                        cap.label
                    );
                }
            }
            rects.push((cap.x, cap.y, cap.w, cap.h, cap.label.clone()));
        }
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                let dx = (a.0 + a.2).min(b.0 + b.2) - a.0.max(b.0);
                let dy = (a.1 + a.3).min(b.1 + b.3) - a.1.max(b.1);
                assert!(
                    dx <= 0.01 || dy <= 0.01,
                    "{w}×{h} 下 {:?} 与 {:?} 重叠",
                    a.4,
                    b.4
                );
            }
        }
    }
    #[test]
    fn complete_keyboard_and_counts_fit_compact_visible_rectangles() {
        let theme = crate::ui::theme("墨玉");
        for (w, h) in [(712., 366.), (552., 222.)] {
            let keys = compact_keys();
            let counts = busy_counts(&keys, 99);
            let layout = heat_layout(w, h, &keys, &counts);
            assert_eq!(layout.mode, HeatMode::Grid, "{w}×{h} 紧凑键盘应保持网格");
            assert_heat_contract(&layout, w, h);
            let mut frame = Frame::new(w, h);
            draw(&mut frame, &theme, 0., 0., w, h, |vk| counts[vk as usize]);
            let text: Vec<_> = frame
                .cmds
                .iter()
                .filter_map(|command| match command {
                    DrawCmd::TextRun { y, h, text, .. } => Some((*y, *h, text.clone())),
                    _ => None,
                })
                .collect();
            // 每格一行键名，有次数的再加一行次数：文字数必须与布局一一对应。
            let expected: usize = layout
                .caps
                .iter()
                .map(|cap| 1 + usize::from(cap.show_count))
                .sum();
            assert_eq!(
                text.len(),
                expected,
                "draw() 的文字必须与热力布局一一对应（{w}×{h}）"
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
        // 键名仍按可用宽度选（宽键给全称、窄键给短写/两字写）。
        assert_eq!(heat_label("Backspace", 54., 14.).unwrap().0, "Bksp");
        assert_eq!(heat_label("Home", 40., 14.).unwrap().0, "Home");
        assert_eq!(heat_label("Home", 30., 14.).unwrap().0, "Hm");
        assert!(heat_label("Num", 24., 14.).unwrap().0.chars().count() <= 3);
        assert!(heat_label("PgUp", 24., 14.).unwrap().0.chars().count() <= 3);
    }
    /// 【审计 V03】640×420 最小主窗的物理热力图：面板 608×372，
    /// draw_heatmap 交给键盘的可用矩形是 552×222（x+28、y+76、pw-56、ph-76-74）。
    /// 断言实际算出的键帽尺寸/字号，并验证无重叠、无裁切。
    #[test]
    fn keyboard_heat_layout_640x420_keeps_labels_and_counts_legible() {
        let keys = main_keys();
        let counts = busy_counts(&keys, 997);
        let layout = heat_layout(552., 222., &keys, &counts);
        assert_eq!(layout.mode, HeatMode::Grid, "640×420 不应降级");
        assert!(!layout.degraded);
        assert_eq!(layout.caps.len(), keys.len());
        assert_eq!(layout.label_size, 14., "640×420 键名字号");
        assert_eq!(layout.count_size, 12., "640×420 次数号");
        assert!(
            (layout.cap_w - 32.94).abs() < 0.05,
            "640×420 的 1 单位键帽宽 {}",
            layout.cap_w
        );
        assert!(
            (layout.cap_h - 32.94).abs() < 0.05,
            "640×420 的 1 单位键帽高 {}",
            layout.cap_h
        );
        assert!((layout.gap - 2.86).abs() < 0.05, "640×420 间距 {}", layout.gap);
        assert_heat_contract(&layout, 552., 222.);
        // 再矮（例如 640×360 逻辑窗口 → 键盘可用高 162，单位缩到 ≈26px）时
        // 一格宽只剩 ≈24px，F10/F11 这类三字键名在 10px 下也放不下，按设计降级成
        // 单行热区行，而不是把两行文字硬塞进矮键帽里裁掉。
        let tall = heat_layout(552., 162., &keys, &counts);
        assert_eq!(tall.mode, HeatMode::Rows, "552×162 应降级为热区行");
        assert!(tall.degraded);
        assert_heat_contract(&tall, 552., 162.);
    }
    /// 【审计 V03】分析模态/800×600 等真实尺寸都留在网格形态，且契约成立。
    #[test]
    fn keyboard_heat_layout_keeps_grid_across_real_panel_rects() {
        let keys = main_keys();
        let counts = busy_counts(&keys, 9_999);
        for (w, h) in [(552., 222.), (712., 294.), (552., 170.), (712., 350.), (924., 458.)] {
            let layout = heat_layout(w, h, &keys, &counts);
            assert_eq!(layout.mode, HeatMode::Grid, "{w}×{h} 不应降级");
            assert_heat_contract(&layout, w, h);
        }
        // 640×420 的分析模态把键盘放进 552×170：字号退到 12/10 号，仍两行可见。
        let analysis = heat_layout(552., 170., &keys, &counts);
        assert_eq!(analysis.label_size, 12., "552×170 键名字号");
        assert_eq!(analysis.count_size, 10., "552×170 次数号");
    }
    /// 【审计 V03】6 位次数必须缩写成短写法（12px 下 6 位数宽 41.8px，
    /// 装不进 33px 键帽），不能靠 PushClip 硬裁。
    #[test]
    fn keyboard_heat_layout_abbreviates_long_counts_instead_of_clipping() {
        let keys = main_keys();
        let mut counts = [0u32; 256];
        for key in &keys {
            counts[key.vk as usize] = 987_654;
        }
        let layout = heat_layout(552., 222., &keys, &counts);
        assert_eq!(layout.mode, HeatMode::Grid);
        let mut abbreviated = 0;
        for cap in &layout.caps {
            assert!(cap.show_count, "{:?} 必须显示次数", cap.label);
            // 无论缩写与否，次数文本都必须真装得进这一格（不是靠 PushClip 硬裁）。
            let text_w = cap.w - HEAT_PAD_X * 2.;
            assert!(
                heat_text_width(&cap.count_text, cap.count_size) <= text_w + 0.001,
                "{:?} 的次数 {:?} 宽 {} 超格宽 {}",
                cap.label,
                cap.count_text,
                heat_text_width(&cap.count_text, cap.count_size),
                text_w
            );
            // 一个单位左右的窄键帽（≈33px）装不下 6 位数，必须已缩写；
            // Tab/空格等宽键帽装得下原数字，保留精确值不算裁切。
            if cap.w < 45. {
                assert_ne!(cap.count_text, "987654", "{:?} 的 6 位次数必须缩写", cap.label);
                abbreviated += 1;
            }
        }
        assert!(abbreviated >= 30, "窄键帽应普遍缩写，实测 {abbreviated} 格");
        assert_heat_contract(&layout, 552., 222.);
    }
    /// 【审计 V03】键帽太矮/太窄时降级成「热区键行 + 其余 N 键」，
    /// 一行一个键名与次数，不叠行所以不会互相挤压或被裁。
    #[test]
    fn keyboard_heat_layout_degrades_to_hot_rows_before_clipping() {
        let keys = main_keys();
        let counts = busy_counts(&keys, 997);
        for (w, h) in [(552., 120.), (552., 22.), (150., 200.), (96., 60.)] {
            let layout = heat_layout(w, h, &keys, &counts);
            assert_eq!(layout.mode, HeatMode::Rows, "{w}×{h} 应降级为热区行");
            assert!(layout.degraded);
            assert_heat_contract(&layout, w, h);
            assert!(
                layout
                    .caps
                    .iter()
                    .all(|cap| cap.align == HeatAlign::Split),
                "{w}×{h} 降级后应全部走单行分列"
            );
        }
    }
    /// 【审计 V03】降级后热区键按次数降序、其余键合并成最后一格「其余 N 键」。
    #[test]
    fn keyboard_heat_layout_rows_fold_the_rest_into_one_cell() {
        let keys = main_keys();
        let counts = busy_counts(&keys, 997);
        let layout = heat_layout(552., 120., &keys, &counts);
        let shown: Vec<u32> = layout
            .caps
            .iter()
            .filter(|cap| !cap.rest)
            .map(|cap| cap.count)
            .collect();
        assert!(shown.len() >= 2, "至少展示两个热区键");
        assert!(
            shown.windows(2).all(|w| w[0] >= w[1]),
            "热区键必须按次数降序：{shown:?}"
        );
        let rest = layout.caps.last().expect("应有其余键说明格");
        assert!(rest.rest, "最后一格应是「其余 N 键」说明格");
        assert!(rest.label.starts_with("其余 "), "实测 {:?}", rest.label);
        let total_keys: u32 = keys.len() as u32;
        let shown_keys: u32 = layout.caps.iter().filter(|cap| !cap.rest).count() as u32;
        assert!(
            rest.label.contains(&(total_keys - shown_keys).to_string()),
            "「其余 N 键」的 N 必须等于未展示的键数，实测 {:?}",
            rest.label
        );
    }
    /// 【审计 V03】任意尺寸扫描：布局要么网格要么降级，两种形态都必须满足契约
    /// （不越界、不重叠、字号 ≥10、文字装得下）。
    #[test]
    fn keyboard_heat_layout_never_overlaps_or_clips_anywhere() {
        let keys = main_keys();
        let mut counts = busy_counts(&keys, 99);
        // 再加几个极端次数，覆盖缩短写法的路径。
        counts[0x20] = 1_234_567;
        counts[0x0D] = 65_535;
        for w in [96., 150., 240., 320., 480., 552., 712., 924., 1_200.] {
            for h in [16., 22., 40., 80., 120., 170., 222., 294., 458.] {
                let layout = heat_layout(w, h, &keys, &counts);
                if layout.caps.is_empty() {
                    // 极端窄小（<10px 高）允许完全省略，但也只能省略。
                    assert!(h < 16. || w < 64., "{w}×{h} 不应空布局");
                    continue;
                }
                assert_heat_contract(&layout, w, h);
            }
        }
    }
    /// 【审计 V03】draw_main 在 640×420 面板矩形里绘制时，所有裁剪框与文字
    /// 都落在传入矩形内（渲染层不会把键名/次数裁到面板外）。
    #[test]
    fn keyboard_draw_main_640x420_stays_inside_the_given_rect() {
        let theme = crate::ui::theme("墨玉");
        let (px, py, pw, ph) = (44., 100., 552., 222.);
        let mut frame = Frame::new(640., 420.);
        let summary = draw_main(
            &mut frame,
            &theme,
            px,
            py,
            pw,
            ph,
            |vk| 1 + u32::from(vk) % 997,
        );
        assert!(summary.total > 0 && summary.max > 0);
        let keys = main_keys();
        let mut texts = 0;
        for command in &frame.cmds {
            match command {
                DrawCmd::PushClip { x, y, w, h } => {
                    assert!(
                        *x >= px - 0.01 && *y >= py - 0.01 && x + w <= px + pw + 0.01 && y + h <= py + ph + 0.01,
                        "裁剪框越出键盘矩形：{x},{y},{w},{h}"
                    );
                }
                DrawCmd::TextRun { y, h, .. } => {
                    assert!(
                        *y >= py - 0.01 && y + h <= py + ph + 0.01,
                        "文字越出键盘矩形：{y}+{h}"
                    );
                    texts += 1;
                }
                _ => {}
            }
        }
        assert!(
            texts >= keys.len(),
            "每个键至少一行键名，实测 {texts} 行 / {} 键",
            keys.len()
        );
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
