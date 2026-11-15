//! Quiet Room: responsive chrome, native text, and distinct article / drill layouts.
use crate::{
    modals::{Action, Hit, Modal, Model},
    session::{Session, SessionSource},
};
use gale_core::{
    compare::CharState,
    config::Config,
    theme::{Color, Theme},
};
use gale_win::d2d::{Align, DrawCmd, FontFamily, FontKit, Frame};
use std::cell::RefCell;

/// 辅助/描述性文字最小字号（说明行、图例、次要标签、状态行、卡片描述；
/// 2026-11-08 用户拍板"部分描述字比较小"，10-12 一律提到 13）。
/// 主标签与正文（cfg.font_size，默认 36）不随之变化；数字等宽小字（编码行）保持。
pub const FONT_HINT: f32 = 14.0;
/// 面板/头部主标签字号（标题、面板名）。
pub const FONT_LABEL: f32 = 14.0;
/// 底部状态行高度。
pub const STATUS_H: f32 = 28.;
/// 底部成绩栏高度。
pub const LEDGER_H: f32 = 36.;
/// 底部成绩栏的顶边 y（状态行之上，留 8px 间距）。
pub fn ledger_bar_y(h: f32) -> f32 {
    h - STATUS_H - 8. - LEDGER_H
}

pub fn rgba(c: Color) -> [f32; 4] {
    [
        c.0 as f32 / 255.,
        c.1 as f32 / 255.,
        c.2 as f32 / 255.,
        c.3 as f32 / 255.,
    ]
}
pub fn hex(s: &str) -> [f32; 4] {
    rgba(Color::from_hex_no_hash(s).unwrap())
}
pub fn alpha(mut c: [f32; 4], a: f32) -> [f32; 4] {
    c[3] *= a;
    c
}
/// 【审计 V12】按比例混合两个颜色的 RGB（alpha 取前者）。
///
/// 只做「已有主题色之间」的混合，不产生任何新的硬编码色值：当前输入层的字形色
/// = 强调色向窗体字体色靠拢，于是暗色主题上变亮、浅色主题上变暗，两种极性都
/// 比强调色本身更接近正文的文字色，与背景的对比度也更高（三主题实测见单测）。
pub fn mix(a: [f32; 4], b: [f32; 4], f: f32) -> [f32; 4] {
    let f = f.clamp(0., 1.);
    [
        a[0] + (b[0] - a[0]) * f,
        a[1] + (b[1] - a[1]) * f,
        a[2] + (b[2] - a[2]) * f,
        a[3],
    ]
}
fn run(
    f: &mut Frame,
    x: f32,
    y: f32,
    size: f32,
    s: impl Into<String>,
    c: [f32; 4],
    font: FontFamily,
    align: Align,
) {
    f.push(DrawCmd::TextRun {
        x,
        y,
        h: size,
        text: s.into(),
        color: c,
        font,
        align,
    });
}
pub fn text(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: [f32; 4]) {
    run(f, x, y, size, s, c, FontFamily::Ui, Align::Left)
}
pub fn mono(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: [f32; 4]) {
    run(f, x, y, size, s, c, FontFamily::Mono, Align::Left)
}
pub fn body_text(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: [f32; 4]) {
    run(f, x, y, size, s, c, FontFamily::Lxgw, Align::Left)
}
/// 【选重数字 2026-11-13】角标统一绘制：`hint_rank_bold` 叠半像素伪加粗，
/// `hint_rank_offset` 调整离字的远近（负=更远，正=更近）。
pub fn rank_digit(
    f: &mut Frame,
    cfg: &gale_core::config::Config,
    x: f32,
    y: f32,
    size: f32,
    s: &str,
    c: [f32; 4],
) {
    let y = y + cfg.hint_rank_offset;
    mono(f, x, y, size, s, c);
    if cfg.hint_rank_bold {
        mono(f, x + 0.9, y, size, s, c);
    }
}

/// 【字间距 2026-11-13】按额外字距重排换行结果：每行内累加 `advance + spacing`，
/// 放不下就就地折行，offsets/widths 保持与原文逐字对应。
pub fn flow_with_spacing(
    lines: &[(usize, String, Vec<f32>)],
    spacing: f32,
    bw: f32,
) -> Vec<(usize, String, Vec<f32>)> {
    if spacing <= 0.01 {
        return lines.to_vec();
    }
    let limit = bw.max(8.);
    let mut out: Vec<(usize, String, Vec<f32>)> = Vec::with_capacity(lines.len() + 8);
    for (offset, text, widths) in lines {
        if text.is_empty() {
            out.push((*offset, String::new(), vec![0.]));
            continue;
        }
        let mut buf = String::new();
        let mut pos = vec![0.];
        let mut used = 0.;
        let mut line_start = *offset;
        for (k, ch) in text.chars().enumerate() {
            let a = widths.get(k).copied().unwrap_or(0.);
            let b = widths.get(k + 1).copied().unwrap_or(a);
            let adv = (b - a).max(1.);
            let extra = if buf.is_empty() { 0. } else { spacing };
            if !buf.is_empty() && used + extra + adv > limit {
                out.push((
                    line_start,
                    std::mem::take(&mut buf),
                    std::mem::take(&mut pos),
                ));
                line_start = offset + k;
                pos = vec![0.];
                used = 0.;
            }
            let extra = if buf.is_empty() { 0. } else { spacing };
            used += extra + adv;
            buf.push(ch);
            pos.push(used);
        }
        if !buf.is_empty() {
            out.push((line_start, buf, pos));
        }
    }
    out
}

/// 字距 > 0 时逐字绘制（位置来自重排后的 widths）；= 0 仍走整行 StyledLine。
#[allow(clippy::too_many_arguments)]
fn styled_line(
    f: &mut Frame,
    x: f32,
    y: f32,
    w: f32,
    size: f32,
    text: &str,
    spans: Vec<(u32, u32, [f32; 4])>,
    color: [f32; 4],
    spacing: f32,
    widths: &[f32],
) {
    if spacing <= 0.01 {
        f.push(DrawCmd::StyledLine {
            x,
            y,
            w,
            size,
            text: text.to_string(),
            spans,
            color,
        });
        return;
    }
    let mut u16pos = 0u32;
    for (j, ch) in text.chars().enumerate() {
        let start = u16pos;
        let end = start + ch.len_utf16() as u32;
        u16pos = end;
        let mut c = color;
        for (s, l, sc) in &spans {
            if *s < end && *s + *l > start {
                c = *sc;
                break;
            }
        }
        let cx = x + widths.get(j).copied().unwrap_or(j as f32 * size);
        body_text(f, cx, y, size, ch.to_string(), c);
    }
}
pub fn right(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: [f32; 4]) {
    run(f, x, y, size, s, c, FontFamily::Ui, Align::Right)
}
pub fn rect(f: &mut Frame, x: f32, y: f32, w: f32, h: f32, c: [f32; 4]) {
    f.push(DrawCmd::Rect {
        x,
        y,
        w: w.max(0.),
        h: h.max(0.),
        color: c,
    });
}
pub fn round(
    f: &mut Frame,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    c: [f32; 4],
    stroke: Option<[f32; 4]>,
) {
    f.push(DrawCmd::RoundRect {
        x,
        y,
        w: w.max(0.),
        h: h.max(0.),
        r,
        color: c,
        stroke,
    });
}
pub fn line(f: &mut Frame, x: f32, y: f32, w: f32, c: [f32; 4]) {
    rect(f, x, y, w, 1., c)
}
/// A separate underbracket for each phrase; the upturned ends expose boundaries
/// even when adjacent words share a colour. Keep it below the text glyphs.
pub fn word_bracket_offset(size: f32) -> f32 {
    // WenKai's visible glyphs can extend below the nominal em box at large sizes.
    // 【六轮 r3】用户反馈括线偏下：0.24×（下限 10）收紧到 0.18×（下限 6），
    // 括线贴着字形上提；编码行 y=+offset+5 同步上移，行距更省。
    // 【十轮】用户仍觉偏下：再收到 0.10×（下限 4），括线紧贴字形底部；
    // 编码行/词下数字引用同一函数，整套提示带同步上移。
    // 【十轮 b】0.10× 过紧（用户反馈太上）：取中 0.14×（下限 5，上限 13）。
    // 【十轮 c】0.14× 仍略高（用户反馈再下一点点）：0.16×（下限 5.5，上限 13.5）。
    // 【十轮 d】0.16× 仍偏高（用户反馈还要再降）：0.17×（下限 5.75，上限 14）。
    (size * 0.17).clamp(5.75, 14.)
}
/// 编码行独立调整高低；括线仍保留用户校准的位置，也不参与正文行高。
pub fn hint_code_y(cfg: &Config, word_y: f32, font_size: f32) -> f32 {
    word_y + font_size + word_bracket_offset(font_size) + 2. + cfg.hint_code_offset
}

thread_local! {
    // Match the renderer's Consolas and fallback advances without constructing
    // layouts again on every frame. This kit only measures the composition row.
    static PREEDIT_FONTS: RefCell<FontKit> = RefCell::new(FontKit::new());
}

/// Keep a composition row inside the content area even before the first key.
/// Reserving it while the switch is enabled prevents a jump on composition start.
pub fn preedit_headroom(cfg: &Config) -> f32 {
    if cfg.show_preedit {
        // 【用户 2026-11-15】改用固定比例，不再跟着编码字号/高低算：首行上方
        // 留白 = 字体大小 × 0.5（半高）。组字行本身按 0.4 倍字号画，见
        // `preedit_size`，两者相加刚好是这个半行。
        cfg.font_size * 0.5
    } else {
        0.
    }
}

/// 组字行整块占位（含上下呼吸），用于滚动时为上方组字让出的间隙。
/// 与 `preedit_headroom` 分开：正文顶部留白收到「刚好放下组字行」，
/// 但行间让位仍按整块算，避免组字行压到上一行已完成的文字。
fn preedit_band(cfg: &Config) -> f32 {
    if cfg.show_preedit {
        // 【用户 2026-11-15】组字行字号固定为字号 × 2/5；整块占位再多留
        // 6px 呼吸与「编码高低」下压量，滚动让位才不会压到上一行已完成文字。
        preedit_size(cfg) + 6. + (-cfg.preedit_offset).max(0.)
    } else {
        0.
    }
}

fn preedit_row_gap(cfg: &Config, font_size: f32, line_h: f32) -> f32 {
    if cfg.show_preedit {
        (font_size + preedit_band(cfg) - line_h).max(0.)
    } else {
        0.
    }
}

/// 【用户 2026-11-15】键入编码（组字行）字号固定为字体大小的 2/5，
/// 不再读 `preedit_font_size`：设置里的编码字号只影响词提编码，不影响
/// 正在输入的这一行。
pub fn preedit_size(cfg: &Config) -> f32 {
    (cfg.font_size * 0.4).max(8.)
}

fn preedit_tail(fonts: &mut FontKit, text: &str, size: f32, available: f32) -> String {
    if fonts.text_width(FontFamily::Mono, size, text) <= available {
        return text.into();
    }
    let ellipsis = fonts.char_width(FontFamily::Mono, size, '…');
    let mut used = ellipsis;
    let mut start = text.len();
    for (at, ch) in text.char_indices().rev() {
        let advance = fonts.char_width(FontFamily::Mono, size, ch);
        if used + advance > available {
            break;
        }
        used += advance;
        start = at;
    }
    if start == text.len() {
        // At a narrow right edge the newest key is more useful than an ellipsis.
        return text
            .chars()
            .next_back()
            .map(String::from)
            .unwrap_or_default();
    }
    format!("…{}", &text[start..])
}

/// Composition grows rightward from the real caret. Long sentence codes retain
/// the latest tail instead of dropping every new key after character fourteen.
/// 【传统模式键入编码】把当前输入的编码画在当前字下方（编码行样式：
/// 小号、强调色降透明度），与文章里的词提编码同一种视觉语法。
fn draw_input_preedit(
    f: &mut Frame,
    cfg: &Config,
    preedit: &str,
    caret: (f32, f32),
    left: f32,
    right: f32,
    clip_bottom: f32,
    color: [f32; 4],
) {
    if preedit.is_empty() {
        return;
    }
    let size = (cfg.font_size * 0.4).max(10.);
    let want_y = caret.1 + cfg.font_size + 2.;
    let y = want_y.min(clip_bottom - size).max(caret.1 + 2.);
    // 跟随光标：横向位置与文字上方那套编码同一口径（贴着光标、贴右边时向左收缩）。
    let available = (right - left).max(1.);
    let (x, shown) = PREEDIT_FONTS.with(|fonts| {
        let mut fonts = fonts.borrow_mut();
        let last = preedit.chars().next_back().unwrap();
        let minimum = (fonts.char_width(FontFamily::Mono, size, last)
            + fonts.char_width(FontFamily::Mono, size, '…'))
        .min(available);
        let x = caret.0.clamp(left, (right - minimum).max(left));
        let shown = preedit_tail(&mut fonts, preedit, size, (right - x).max(1.));
        (x, shown)
    });
    f.push(DrawCmd::PushClip {
        x: left,
        y: 0.,
        w: available,
        h: f.size.h,
    });
    mono(f, x, y, size, shown, alpha(color, 0.85));
    f.push(DrawCmd::PopClip);
}

pub fn draw_preedit(
    f: &mut Frame,
    cfg: &Config,
    preedit: &str,
    caret: (f32, f32),
    left: f32,
    right: f32,
    color: [f32; 4],
) {
    if !cfg.show_preedit || preedit.is_empty() {
        return;
    }
    let size = preedit_size(cfg);
    let available = (right - left).max(1.);
    let (x, shown) = PREEDIT_FONTS.with(|fonts| {
        let mut fonts = fonts.borrow_mut();
        let last = preedit.chars().next_back().unwrap();
        let minimum = (fonts.char_width(FontFamily::Mono, size, last)
            + fonts.char_width(FontFamily::Mono, size, '…'))
        .min(available);
        let x = caret.0.clamp(left, (right - minimum).max(left));
        let shown = preedit_tail(&mut fonts, preedit, size, (right - x).max(1.));
        (x, shown)
    });
    f.push(DrawCmd::PushClip {
        x: left,
        y: 0.,
        w: available,
        h: f.size.h,
    });
    mono(
        f,
        x,
        // 编码高低仍然微调，但整行被夹在预留的那半行里，越界会贴回边界。
        (caret.1 - size + 4. + cfg.preedit_offset)
            .clamp(
                // 字号很小时留白（0.5×字号）会小于组字行高（0.4×字号 + 8 下限），
                // 两个边界可能反向；此时以「贴着正文首行」为准，绝不让 clamp 崩。
                (caret.1 - preedit_headroom(cfg)).min(caret.1 - size),
                caret.1 - size,
            ),
        size,
        shown,
        color,
    );
    f.push(DrawCmd::PopClip);
}
/// 【词提下行 r3 2026-11-14】r2 把括线兜底到整带底部，上翘端挖进编码字面
/// （用户：「编码和数字在下括号里面了」）。定谳分层：括线回到四轮校准的
/// 紧贴字脚位（fs+offset，tab 朝词），编码行整体让到括线**之下 +2**——
/// 链=文字 → 括线 → 编码/选重，编码永远在杯外。
pub fn word_bracket(f: &mut Frame, x: f32, y: f32, w: f32, size: f32, c: [f32; 4]) {
    let inset = (size * 0.14).clamp(2.5, 10.).min(w / 4.);
    let width = (w - 2. * inset).max(0.);
    if width == 0. {
        return;
    }
    let stroke = if size < 24. { 1.5 } else { 2. };
    let rise = (size * 0.10).clamp(3., 8.);
    let bottom = y + size + word_bracket_offset(size);
    rect(f, x + inset, bottom, width, stroke, c);
    rect(f, x + inset, bottom - rise, stroke, rise, c);
    rect(
        f,
        x + inset + width - stroke,
        bottom - rise,
        stroke,
        rise,
        c,
    );
}
pub fn elide(s: &str, n: usize) -> String {
    let v: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{v}…")
    } else {
        v
    }
}
/// Approximate advance width in DIPs for the UI font. CJK is full-width, ASCII
/// is a little over half-width; used only to keep the header title inside its
/// clip while leaving the "· 第 N 段" suffix intact.
pub fn ui_text_width(s: &str, size: f32) -> f32 {
    s.chars()
        .map(|c| {
            if c == ' ' {
                size * 0.33
            } else if c.is_ascii() {
                size * 0.56
            } else {
                size * 0.98
            }
        })
        .sum()
}
pub fn elide_width(s: &str, max: f32, size: f32) -> String {
    if ui_text_width(s, size) <= max {
        return s.into();
    }
    let mut out = String::new();
    let mut width = 0.;
    for ch in s.chars() {
        let cw = ui_text_width(&ch.to_string(), size);
        if width + cw + size * 0.9 > max {
            out.push('…');
            break;
        }
        out.push(ch);
        width += cw;
    }
    out
}
pub struct Palette {
    pub word_colors: [[f32; 4]; 6],
    pub hint_rank_colors: [[f32; 4]; 10],
    pub article_bg: [f32; 4],
    pub correct: [f32; 4],
    pub bg: [f32; 4],
    pub bg2: [f32; 4],
    pub surface: [f32; 4],
    pub surface2: [f32; 4],
    pub line: [f32; 4],
    pub line2: [f32; 4],
    pub text: [f32; 4],
    pub text2: [f32; 4],
    pub muted: [f32; 4],
    pub accent: [f32; 4],
    pub red: [f32; 4],
    /// 【审计 V17】green/copper/jade 不再是反馈色来源（对/错/提示一律经
    /// status_color() 取主题键），保留字段只为皮肤/主题投影兼容。
    #[allow(dead_code)]
    pub green: [f32; 4],
    #[allow(dead_code)]
    pub copper: [f32; 4],
    #[allow(dead_code)]
    pub jade: [f32; 4],
    /// 对打三态字符底（整改 4）：未打 / 打对 / 打错。
    pub bg_pending: [f32; 4],
    pub bg_done: [f32; 4],
    pub bg_wrong: [f32; 4],
    /// 未打文字色（2026-11-13 四轮补齐）：对照文章卡/字贴正文里未敲的字；
    /// 缺键回退次级文字色（旧 profile/皮肤兼容）。
    pub pending: [f32; 4],
    /// 词提四色（整改 9）：编码下显文字 / 角标与尾码点 / 下划线 / 词底。
    pub hint_code: [f32; 4],
    pub hint_badge: [f32; 4],
    pub hint_underline: [f32; 4],
    pub hint_bg: [f32; 4],
}
/// Semantic dictionary-word colors; punctuation and single characters stay neutral.
pub fn hint_word_tint(p: &Palette, text: &str, code: &str, _max_len: usize) -> Option<[f32; 4]> {
    crate::analysis::word_color_class(text, code).map(|class| p.word_colors[class as usize])
}

pub const HINT_RANK_COLOR_KEYS: [&str; 10] = [
    "词提1选色",
    "词提2选色",
    "词提3选色",
    "词提4选色",
    "词提5选色",
    "词提6选色",
    "词提7选色",
    "词提8选色",
    "词提9选色",
    "词提10选色",
];

/// Both color modes share the dictionary-word gate; only the color lookup changes.
pub fn hint_word_tint_for_rank(
    p: &Palette,
    text: &str,
    code: &str,
    rank: usize,
    max_len: usize,
    rank_mode: bool,
) -> Option<[f32; 4]> {
    if !rank_mode {
        return hint_word_tint(p, text, code, max_len);
    }
    crate::analysis::word_color_class(text, code)?;
    rank.checked_sub(1)
        .and_then(|i| p.hint_rank_colors.get(i))
        .copied()
}

/// 【审计 V12】「当前正在输入的字符」字形色的配方：强调色向窗体字体色混合的比例。
///
/// 取 0.35 的依据是三套内置主题实测（bg = 跟打区背景色）：
/// 当前输入色对 bg 的对比度 墨玉 11.7:1 / 宣纸 5.9:1 / 青瓷 5.7:1，全部 > WCAG AA
/// 的 4.5:1，且分别高于裸强调色的 9.9 / 3.6 / 4.1；同时与打对色、打错色、词提
/// 语义色的 RGB 通道距离都足够大（见 audit_v12_* 单测），不会撞色。
pub const CURRENT_INPUT_INK_MIX: f32 = 0.35;

/// 【审计 V12】当前输入字符脚下实心下划线的粗细（px）。

/// 【审计 V17】对照文章卡里「已定稿」文字的不透明度层级。
///
/// 只表达「这是参照物、不是我在打的那一格」，色值仍取打对字色——不再在栏位内
/// 临时换成别的颜色（旧实现是 alpha(打对色, 0.42) 的固定值，但同一状态在输入卡
/// 里却是 100% 打对色，同一个「打对」在不同栏位颜色不同）。
pub fn reference_settled_alpha() -> f32 {
    0.42
}

/// 【审计 V12】【审计 V17】正文里一个字的视觉角色。
///
/// 角色只由「比对状态 + 是否当前输入位」决定，与它画在文章对照卡 / 输入卡 / 字贴
/// 哪一栏无关。旧实现把颜色和栏位绑在一起（输入卡里所有非错字一律画成打对色、
/// 文章卡里打对色按栏位淡化），用户没法据此判断自己打到哪。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphRole {
    /// 当前正在输入的那一格（整行最明确的一层）
    Current,
    /// 打错
    Wrong,
    /// 打对
    Correct,
    /// 未打、但有词提语义色
    Hint,
    /// 未打的中性字
    Pending,
}

/// 【审计 V12】纯逻辑：状态 + 当前位 + 是否有词提色 → 视觉角色。
///
/// 当前位优先于状态：正在输入的那个字必须是整行最醒目的一层。错字的信息不靠
/// 字形色独扛——红色虚线波浪 + 「对打打错底色」两个状态通道仍然保留。
pub fn classify_glyph(state: Option<CharState>, current: bool, tinted: bool) -> GlyphRole {
    if current {
        return GlyphRole::Current;
    }
    match state {
        Some(CharState::Wrong) => GlyphRole::Wrong,
        Some(CharState::Correct) => GlyphRole::Correct,
        _ if tinted => GlyphRole::Hint,
        _ => GlyphRole::Pending,
    }
}

/// 【审计 V12】当前输入字符的样式：(字形色, 底色, 是否画实心下划线)。
///
/// 三个通道全部由主题键派生，不新增硬编码色值：
/// - 字形色 = 强调色向窗体字体色混合 [`CURRENT_INPUT_INK_MIX`]；
/// - 底色 = 强调色 16% 填充，给这一格一个「面」，与打对/打错底色分层不同；
/// - 实心下划线 = 字脚下方的横条（宽度由绘制处按格子宽给出），比词提括线更粗、更贴字、
///   不带两端上翘，因此即使当前字恰好落在词提词里也能一眼分出「当前位置」。
pub fn current_input_style(t: &Theme) -> ([f32; 4], [f32; 4], bool) {
    let p = tokens(t);
    (
        mix(p.accent, p.text, CURRENT_INPUT_INK_MIX),
        alpha(p.accent, 0.16),
        true,
    )
}

/// 【审计 V12】【审计 V17】字形色：角色 → 主题键色。
///
/// 正文三处绘制共用这一条取色路径，反馈色因此由状态决定（打对字色 / 打错字色 /
/// 未打文字色 / 词提语义色），而不是按栏位临时挑色。
pub fn glyph_color(
    p: &Palette,
    role: GlyphRole,
    tint: Option<[f32; 4]>,
    current: [f32; 4],
) -> [f32; 4] {
    match role {
        GlyphRole::Current => current,
        GlyphRole::Wrong => p.red,
        GlyphRole::Correct => p.correct,
        GlyphRole::Hint => tint.unwrap_or(p.pending),
        GlyphRole::Pending => p.pending,
    }
}

/// 【审计 V17】状态语义档位：反馈色按状态取，不再用 tokens 里 hex 硬编码的
/// green / copper，也不按「画在哪个 chrome 上」临时决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    /// 提示 / 等待（最弱一档）
    Hint,
    /// 中性次级（暂停、非活动态）
    Neutral,
    /// 进行中（练习中、跟打中）
    Active,
    /// 完成 / 正确
    Done,
}

/// 【审计 V17】档位 → 主题键：提示=发文区字体色、中性=次级文字色、
/// 进行中=强调色、完成=打对字色。四个键都来自 `tokens()`，内置主题与自定义皮肤
/// 走同一条路径；任一主题下「对」与「错」分别取打对字色 / 打错字色，语义不串。
pub fn status_color(p: &Palette, tone: StatusTone) -> [f32; 4] {
    match tone {
        StatusTone::Hint => p.muted,
        StatusTone::Neutral => p.text2,
        StatusTone::Active => p.accent,
        StatusTone::Done => p.correct,
    }
}

pub fn tokens(t: &Theme) -> Palette {
    let light = matches!(t.name.as_str(), "宣纸" | "青瓷" | "灰砖" | "青砖");
    Palette {
        word_colors: [
            t.colors
                .get("词提简码色")
                .copied()
                .map(rgba)
                .unwrap_or_else(|| hex(if light { "16805b" } else { "7bdbac" })),
            t.colors
                .get("词提二字三码色")
                .copied()
                .map(rgba)
                .unwrap_or_else(|| hex(if light { "4660ae" } else { "a5b8ff" })),
            t.colors
                .get("词提二字全码色")
                .copied()
                .map(rgba)
                .unwrap_or_else(|| hex(if light { "b45b08" } else { "ffae65" })),
            t.colors
                .get("词提三字全码色")
                .copied()
                .map(rgba)
                .unwrap_or_else(|| hex(if light { "8550b0" } else { "c69bf2" })),
            t.colors
                .get("词提四字全码色")
                .copied()
                .map(rgba)
                .unwrap_or_else(|| hex(if light { "216d91" } else { "7ecbfa" })),
            t.colors
                .get("词提一码色")
                .copied()
                .map(rgba)
                .unwrap_or_else(|| hex(if light { "c43e62" } else { "ff7f9f" })),
        ],
        hint_rank_colors: std::array::from_fn(|i| {
            const LIGHT: [&str; 10] = [
                "16805b", "4660ae", "b45b08", "c43e62", "8550b0", "216d91", "7a6b00", "0d7b76",
                "a83a91", "8a6037",
            ];
            const DARK: [&str; 10] = [
                "7bdbac", "a5b8ff", "ffae65", "ff7f9f", "c69bf2", "7ecbfa", "e3cd70", "6dd7cc",
                "f29bdd", "dcaf89",
            ];
            t.colors
                .get(HINT_RANK_COLOR_KEYS[i])
                .copied()
                .map(rgba)
                .unwrap_or_else(|| hex(if light { LIGHT[i] } else { DARK[i] }))
        }),
        article_bg: rgba(interface_bg(t)),
        correct: rgba(t.get("打对字色")),
        bg: rgba(interface_bg(t)),
        bg2: rgba(t.get("次级背景色")),
        surface: rgba(t.get("菜单背景色")),
        surface2: rgba(t.get("按钮背景色")),
        line: rgba(t.get("边框色")),
        line2: rgba(t.get("输入框边框色")),
        text: rgba(t.get("窗体字体色")),
        text2: rgba(t.get("次级文字色")),
        muted: rgba(t.get("发文区字体色")),
        accent: rgba(t.get("光标色")),
        red: rgba(t.get("打错字色")),
        green: hex(if light { "2b6f4e" } else { "63c08d" }),
        copper: hex(if light { "a8703e" } else { "cf9a63" }),
        jade: hex(if light { "4f8f79" } else { "7fbfa6" }),
        bg_pending: rgba(t.get("对打未打底色")),
        bg_done: rgba(t.get("对打打对底色")),
        bg_wrong: rgba(t.get("对打打错底色")),
        // 未打文字色缺键回退次级文字色（内置主题/旧 profile 未设该键）。
        pending: rgba(
            t.colors
                .get("未打文字色")
                .copied()
                .unwrap_or_else(|| t.get("次级文字色")),
        ),
        hint_code: rgba(t.get("词提编码色")),
        hint_badge: rgba(t.get("词提角标色")),
        hint_underline: rgba(t.get("词提下划线色")),
        hint_bg: rgba(t.get("词提底色")),
    }
}
/// 2026-11-13 用户拍板：「跟打区背景色」升格为整体界面底色——窗口基底、极简
/// 模式正文面、对照模式输入卡全部吃它；旧 profile/皮肤只设了「窗体背景色」时
/// 向后兼容取该值（内置主题两键同值，无视觉变化）。
pub fn interface_bg(t: &Theme) -> Color {
    t.colors
        .get("跟打区背景色")
        .copied()
        .unwrap_or_else(|| t.get("窗体背景色"))
}
/// 【用户反馈 2026-11-15】主题配色统一收口：内置主题 + 用户 color_overrides
/// 一次装好。散在各处的「theme() 再逐个 set」必须走这里，否则换主题后
/// 旧的绝对色值会把配色撕开（灰底 + 近黑卡面 + 黑字）。
pub fn theme_with_overrides(
    name: &str,
    overrides: &std::collections::BTreeMap<String, String>,
) -> Theme {
    let mut t = theme(name);
    apply_overrides(&mut t, overrides);
    t
}

/// 把 color_overrides 套到已有主题（跟随虎符皮肤也走这里），返回修过的项数。
pub fn apply_overrides(
    t: &mut Theme,
    overrides: &std::collections::BTreeMap<String, String>,
) -> usize {
    for (key, value) in overrides {
        if let Some(color) = Color::from_hex_no_hash(value) {
            t.set(key, color);
        }
    }
    harmonize(t, overrides)
}

/// 卡面与字色统一（只在用户**显式覆盖过**的项上动手，主题自带层次不动）：
/// 覆盖过的阅读色低于 3.0:1 时往窗体字体色拉，够不着再退纯白/纯黑，
/// 直到在本区与文章卡上都看得清。卡面本身不派生：主题是什么样就画什么样。
pub fn harmonize(
    t: &mut Theme,
    overrides: &std::collections::BTreeMap<String, String>,
) -> usize {
    let base = interface_bg(t);
    let mut fixed = 0;
    let card = t.get("次级背景色");
    for key in [
        "未打文字色",
        "打对字色",
        "次级文字色",
        "跟打区字体色",
        "窗体字体色",
    ] {
        if lift_to_readable(t, overrides, key, 3.0, card, base) {
            fixed += 1;
        }
    }
    for key in HINT_RANK_COLOR_KEYS {
        if lift_to_readable(t, overrides, key, 3.0, card, base) {
            fixed += 1;
        }
    }
    fixed
}

/// 【用户反馈 2026-11-16 + 全面排查】选色与重启装配对称化：调色盘选完深色
/// 当场生效，重启后却被 harmonize 抬亮——看起来像"颜色被改回去了"。
/// 此变体对**活主题**（跟随皮肤时基座也是皮肤派生值）跑与重启装配同一套
/// 全量 harmonize，再把被抬亮的键值写回 overrides：保存值与下次重启装配
/// 值严格一致；先前随背景变化而失效的保底键（跨键场景）一并校准落盘。
pub fn harmonize_persist(
    t: &mut Theme,
    overrides: &mut std::collections::BTreeMap<String, String>,
) -> usize {
    let fixed = harmonize(t, overrides);
    if fixed == 0 {
        return 0;
    }
    for key in [
        "未打文字色",
        "打对字色",
        "次级文字色",
        "跟打区字体色",
        "窗体字体色",
    ] {
        write_back_lifted(t, overrides, key);
    }
    for key in HINT_RANK_COLOR_KEYS {
        write_back_lifted(t, overrides, key);
    }
    fixed
}

/// 把 t 中已被保底抬亮的键值同步回 overrides（值未变=未被抬亮则跳过）。
fn write_back_lifted(
    t: &Theme,
    overrides: &mut std::collections::BTreeMap<String, String>,
    key: &str,
) {
    let Some(stored) = overrides.get(key).cloned() else {
        return;
    };
    let c = t.get(key);
    let hex = format!("{:02x}{:02x}{:02x}", c.0, c.1, c.2);
    if hex != stored {
        overrides.insert(key.to_string(), hex);
    }
}

fn lift_to_readable(
    t: &mut Theme,
    overrides: &std::collections::BTreeMap<String, String>,
    key: &str,
    floor: f64,
    card: Color,
    field: Color,
) -> bool {
    if !overrides.contains_key(key) {
        return false;
    }
    let cur = t.get(key);
    let ok = |c: Color| {
        gale_core::theme::contrast_ratio(c, card) >= floor
            && gale_core::theme::contrast_ratio(c, field) >= floor
    };
    if ok(cur) {
        return false;
    }
    // 往窗体字体色拉是真·主题内的统一；但底色是用户硬改的（浅主题 + 灰底）
    // 时，深色字体再拉也够不着对比度，于是再退到纯白/纯黑里能读的那个。
    let fg = t.get("窗体字体色");
    for target in [fg, Color(0xff, 0xff, 0xff, 255), Color(0, 0, 0, 255)] {
        let mut f = 0.0_f32;
        while f < 1.0 {
            f = (f + 0.08).min(1.0);
            let next = gale_core::theme::mix(cur, target, f);
            if ok(next) {
                t.set(key, next);
                return true;
            }
        }
    }
    false

}

/// Match editable color values to the resolved palette, including schema/skin defaults.
pub fn setting_color(t: &Theme, key: &str) -> [f32; 4] {
    let p = tokens(t);
    if let Some(i) = HINT_RANK_COLOR_KEYS.iter().position(|item| *item == key) {
        return p.hint_rank_colors[i];
    }
    match key {
        "词提简码色" => p.word_colors[0],
        "词提二字三码色" => p.word_colors[1],
        "词提二字全码色" => p.word_colors[2],
        "词提三字全码色" => p.word_colors[3],
        "词提四字全码色" => p.word_colors[4],
        "词提一码色" => p.word_colors[5],
        _ => rgba(t.get(key)),
    }
}
pub fn button(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    label: &str,
    a: Action,
    t: &Theme,
    selected: bool,
) {
    let p = tokens(t);
    round(
        f,
        x,
        y,
        w,
        32.,
        8.,
        if selected {
            alpha(p.accent, 0.12)
        } else {
            p.surface2
        },
        Some(if selected {
            alpha(p.accent, 0.4)
        } else {
            p.line
        }),
    );
    text(
        f,
        x + 10.,
        y + 8.,
        FONT_HINT,
        label,
        if selected { p.accent } else { p.text2 },
    );
    hs.push(Hit {
        x,
        y,
        w,
        h: 32.,
        action: a,
    });
}
pub fn primary_button(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    label: &str,
    a: Action,
    t: &Theme,
) {
    let p = tokens(t);
    round(f, x, y + 3., w, 32., 8., alpha(p.accent, 0.06), None);
    round(f, x, y, w, 32., 8., p.accent, Some(alpha(p.text, 0.18)));
    // 【审计 V01】前景色由主题按强调色亮度现算，不再硬编码 #211a10（浅色主题只有 3.7–4.2:1）。
    text(f, x + 12., y + 8., FONT_HINT, label, rgba(t.get("强调前景色")));
    hs.push(Hit {
        x,
        y,
        w,
        h: 32.,
        action: a,
    });
}
pub fn theme(name: &str) -> Theme {
    let (bg, bg2, surface, surface2, l, l2, fg, fg2, muted, accent, wrong) = match name {
        "宣纸" => (
            "f5f2ea", "efe9dd", "fffdf8", "f3efe5", "e2dcce", "d2c8b6", "211e18", "5d574c",
            "7d7466", "a8741a", "a83c2e",
        ),
        "夜巡" => (
            "0c1119", "101722", "141b26", "1a2331", "232d3d", "354257", "e9eef6", "b3bfd0",
            "8696ae", "9db8e0", "e0746a",
        ),
        "朱砂" | "赤金" => (
            "191112", "211719", "281c1e", "322225", "3d292c", "583a3e", "f5e9e3", "c9b3ac",
            "a28b85", "d96a52", "e18473",
        ),
        "青瓷" => (
            "eef2ec", "e5ece2", "f9fcf5", "e8eee3", "d4dfd0", "bacbb5", "25392f", "596d5f",
            "7a8e7e", "4f7f6a", "ad5146",
        ),
        // 【用户 2026-11-15】中性灰主题：灰底 + 墨黑字，与宣纸（暖米）、青瓷（冷绿）分开。
        // 二轮：用户嫌「青砖」太亮，改深一档的灰砖；旧名「青砖」仍作别名认。
        "灰砖" | "青砖" => (
            "404040", "4a4a4a", "444444", "505050", "262626", "202020", "000000", "000000",
            "1a1a1a", "6e5318", "8f3227",
        ),
        _ => (
            "131417", "17181c", "1a1d22", "20242a", "2a2e36", "383d47", "f0ede6", "c6c1b5",
            "9d978b", "e7b75c", "e06a5a",
        ),
    };
    let mut t = Theme::default();
    t.name = if name == "青砖" {
        "灰砖".into()
    } else if name == "default" || name.is_empty() {
        "墨玉".into()
    } else {
        name.into()
    };
    for (k, v) in [
        ("窗体背景色", bg),
        ("跟打区背景色", bg),
        ("次级背景色", bg2),
        ("菜单背景色", surface),
        ("按钮背景色", surface2),
        ("边框色", l),
        ("输入框边框色", l2),
        ("窗体字体色", fg),
        ("跟打区字体色", fg),
        ("打对字色", fg),
        ("次级文字色", fg2),
        ("发文区字体色", muted),
        ("光标色", accent),
        ("标题栏进度条颜色", accent),
        ("打错字色", wrong),
        ("打错色", wrong),
    ] {
        t.set(k, Color::from_hex_no_hash(v).unwrap());
    }
    // v2.1 视觉整改：三态底/词提四色的预设适配（用户 color_overrides 仍可覆盖）。
    // 浅色主题状态底使用浅色系；单字编码与位次使用中性文字色。
    if t.name == "宣纸" || t.name == "青瓷" || t.name == "灰砖" || t.name == "青砖" {
        t.set("对打未打底色", Color(0, 0, 0, 11)); // rgba(0,0,0,0.045)
        t.set("对打打对底色", Color(43, 111, 78, 36)); // rgba(43,111,78,0.14)
        t.set("对打打错底色", Color(168, 60, 46, 41)); // rgba(168,60,46,0.16)
    }
    let acc = Color::from_hex_no_hash(accent).unwrap();
    t.set("词提编码色", Color::from_hex_no_hash(fg2).unwrap());
    t.set("词提角标色", Color::from_hex_no_hash(fg2).unwrap());
    t.set("词提下划线色", acc);
    // 【灰砖】#404040 体系：底是深灰，但「待打内容」必须亮、词提必须可区分，
    // 所以十档词提用低饱和的十个可辨色相（不是同灰阶），已打内容沉到墨黑。
    if t.name == "灰砖" || t.name == "青砖" {
        let hexc = |v: &str| Color::from_hex_no_hash(v).unwrap();
        const RANKS: [&str; 10] = [
            "9ab4c8", "a8c08f", "c8b07a", "c8948a", "b49ac8",
            "8fc0bc", "c8a4bc", "a8a88f", "8fa8c8", "c2c0a0",
        ];
        for (i, key) in HINT_RANK_COLOR_KEYS.iter().enumerate() {
            t.set(key, hexc(RANKS[i]));
        }
        t.set("跟打区字体色", hexc("000000"));
        t.set("窗体字体色", hexc("000000"));
        t.set("未打文字色", hexc("000000"));
        t.set("词提编码色", hexc("c8c8c8"));
        t.set("词提角标色", hexc("d0d0d0"));
        t.set("词提下划线色", hexc("a08f70"));
        t.set("光标色", hexc("e0d6c0"));
        t.set("标题栏进度条颜色", hexc("a08f70"));
        t.set("打错色", hexc("c46a5a"));
        t.set("打错字色", hexc("c46a5a"));
        t.set("打对字色", hexc("000000"));
        t.set("次级文字色", hexc("9a9a9a"));
        t.set("发文区字体色", hexc("000000"));
        t.set("菜单字体色", hexc("000000"));
        t.set("按钮字体色", hexc("000000"));
        t.set("对打未打底色", Color(0, 0, 0, 18));
        t.set("对打打对底色", Color(126, 150, 132, 26));
        t.set("对打打错底色", Color(160, 108, 96, 30));
    }
    // 词提底色保持 Theme::default 的全透明（显式自定义时才会画底）。
    t
}
#[derive(Default)]
pub struct Animation {
    pub caret: Option<(f32, f32)>,
    /// 未平滑的目标光标（DIP）。IME 隐藏宿主/候选锚必须用它，不能跟动画值，
    /// 否则 125% 下宿主会随平滑插值在行内滑动，候选窗看起来一直上下移动。
    pub caret_target: Option<(f32, f32)>,
    pub last_input: f64,
    pub wrong_at: f64,
    pub correct_at: f64,
    /// 【对照模式 2026-11-12】光标所在行行高（DIP）——IME 组字窗/候选窗
    /// 锚点需要真实行高；字贴模式=文章行高，对照模式=输入卡行高。
    pub input_line_h: f32,
    /// 【对照模式 2026-11-12】分栏把手命中区（DIP）：主循环据此拖动分栏。
    pub split_divider: Option<(f32, f32, f32, f32)>,
    /// 【动效 2026-11-15】光标缓动：起点 (x, y) 与起始秒数；None=已到位。
    pub caret_anim: Option<(f32, f32, f64)>,
    /// 【动效 2026-11-15】上次光标目标变化的时刻，用来按打字节奏自适应缓动时长。
    pub caret_since: f64,
    pub caret_dur: f64,
    /// 当前这格光标缓动时长（秒），按打字节奏在 60ms / 160ms 之间取。
    /// 【动效 2026-11-15】换行滚动的起始位移（行数）与起始秒数；0=已到位。
    pub scroll_from: f32,
    pub scroll_at: f64,
    /// 上一次的首行序号，用来发现「换行导致整屏上移」。
    pub scroll_first: Option<usize>,
}
impl Animation {
    pub fn active(&self, n: f64) -> bool {
        n - self.last_input < 0.35
            || n - self.wrong_at < 0.3
            || n - self.correct_at < 0.42
            || self.caret_anim.is_some()
            || self.scroll_from.abs() > 0.001
    }
}
/// 三次缓出：0..1 → 0..1，前快后慢，用于光标与滚动动效。
fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    1. - (1. - t) * (1. - t) * (1. - t)
}

/// 【动效 2026-11-15】光标平滑：目标一变就从当前位置按 130ms 缓出追过去，
/// 由 `Animation::active` 保证动画期间持续出帧；关掉动效时直接落位。
fn smooth_caret(
    anim: &mut Animation,
    target: (f32, f32),
    now: f64,
    enabled: bool,
    line_h: f32,
) -> (f32, f32) {
    if !enabled {
        anim.caret = Some(target);
        anim.caret_anim = None;
        anim.caret_target = Some(target);
        return target;
    }
    let moved = match anim.caret_target {
        Some((tx, ty)) => (tx - target.0).abs() > 0.5 || (ty - target.1).abs() > 0.5,
        None => true,
    };
    // 【动效 2026-11-15】换行时目标跨行：光标直接落到新行行首，不从上一行行尾
    // 斜着飞过去（那样看着像被拖过去的）。同行的横向移动仍然走缓动。
    if let Some((_, cur_y)) = anim.caret {
        if (cur_y - target.1).abs() >= line_h * 0.5 {
            anim.caret = Some(target);
            anim.caret_anim = None;
            anim.caret_target = Some(target);
            return target;
        }
    }
    if moved {
        // 【动效 2026-11-15】按打字节奏切换：间隔 < 150ms（约 400 字/分以上）直接落位，
        // 高速连打时任何缓动都只会让光标永远慢一格；慢打时用 160ms 缓出。
        let gap = now - anim.caret_since;
        anim.caret_since = now;
        // 时长跟着节奏走：永远只用「下一个键到来前」的一半时间，保证下一键落键时光标
        // 已经到位、不会越追越远；慢打时上限 160ms。
        anim.caret_dur = if gap > 0. && gap < 0.40 {
            (gap * 0.5).clamp(0.04, 0.16)
        } else {
            0.16
        };
        let from = anim.caret.unwrap_or(target);
        anim.caret_anim = Some((from.0, from.1, now));
    }
    let caret = match anim.caret_anim {
        Some((fx, fy, t0)) => {
            let p = ease_out(((now - t0) / anim.caret_dur.max(0.01)) as f32);
            if p >= 1. {
                anim.caret_anim = None;
                target
            } else {
                (fx + (target.0 - fx) * p, fy + (target.1 - fy) * p)
            }
        }
        None => target,
    };
    anim.caret = Some(caret);
    anim.caret_target = Some(target);
    caret
}

/// 【动效 2026-11-15】换行滚动：首行序号一跳就把旧位置记下来，
/// 用 160ms 缓出滑到新位置；返回当前还差多少行的位移。
fn scroll_offset(anim: &mut Animation, first: usize, now: f64, enabled: bool) -> f32 {
    if !enabled {
        anim.scroll_from = 0.;
        anim.scroll_first = Some(first);
        return 0.;
    }
    if let Some(prev) = anim.scroll_first {
        if prev != first {
            let delta = first as f32 - prev as f32;
            let remain = if anim.scroll_from.abs() > 0.001 {
                anim.scroll_from * (1. - ease_out(((now - anim.scroll_at) / 0.28) as f32))
            } else {
                0.
            };
            anim.scroll_from = remain + delta;
            anim.scroll_at = now;
        }
    }
    anim.scroll_first = Some(first);
    if anim.scroll_from.abs() <= 0.001 {
        anim.scroll_from = 0.;
        return 0.;
    }
    let p = ease_out(((now - anim.scroll_at) / 0.28) as f32);
    if p >= 1. {
        anim.scroll_from = 0.;
        0.
    } else {
        anim.scroll_from * (1. - p)
    }
}

pub fn body_metrics(sess: &Session, cfg: &Config, w: f32) -> (f32, f32, f32) {
    let single = sess.source == SessionSource::Single;
    // 对照模式的上下两张卡用满窗口宽度（左右各 12px 边），不再套字贴的 1000 上限，
    // 否则卡片左右会空出大片。
    // 字贴模式同样跟随窗口宽度：原先 960/1000 的隐形上限会让宽窗口左右空出一大截，
    // 用户拉宽窗口时正文反而停在 1000px（2026-11-14 用户反馈）。宽屏行宽按用户设置
    // 原样铺满，不设人为阅读宽度上限。
    // 对照模式：左右各留 ~36px 包边（太贴边会显得局促），极简下收窄到 16px。
    let bw = if cfg.display_mode.is_split() && !cfg.minimal {
        (w - 96.).max(360.).min(1600.)
    } else if cfg.display_mode.is_split() {
        (w - 56.).max(360.).min(1600.)
    } else {
        (w - 64.).max(180.)
    };
    let fs = (if single {
        cfg.font_size * 42. / 36.
    } else {
        cfg.font_size
    })
    .clamp(10., 100.);
    (
        bw,
        fs,
        fs * if single {
            2.08
        } else {
            // 行距范围 0.10..=3.00（2026-11-13 四轮：用户要求最低 0.1；
            // 上限与配置解析 line_gap -0.90..=2.0 对齐）。
            (1. + cfg.line_gap).clamp(0.10, 3.0)
        },
    )
}
fn chip(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    s: &str,
    a: Action,
    p: &Palette,
    on: bool,
) {
    round(
        f,
        x,
        y,
        w,
        22.,
        11.,
        if on { alpha(p.accent, 0.10) } else { [0.; 4] },
        Some(if on { alpha(p.accent, 0.35) } else { p.line }),
    );
    text(
        f,
        x + 9.,
        y + 4.,
        FONT_HINT,
        s,
        if on { p.accent } else { p.muted },
    );
    hs.push(Hit {
        x,
        y,
        w,
        h: 22.,
        action: a,
    });
}
// The chrome uses the resolved theme, including user overrides.
fn toolbar_width(label: &str, key: &str) -> f32 {
    let text_w = ui_text_width(label, FONT_HINT);
    (text_w
        + if key.is_empty() {
            0.
        } else {
            toolbar_key_width(key) + 6.
        }
        + 24.)
        .max(72.)
}
fn toolbar_key_width(key: &str) -> f32 {
    (ui_text_width(key, 12.) + 8.).max(28.)
}
#[allow(clippy::too_many_arguments)]
fn toolbar_button(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    label: &str,
    key: &str,
    action: Action,
    t: &Theme,
    selected: bool,
) -> f32 {
    let p = tokens(t);
    let w = toolbar_width(label, key);
    let key_w = toolbar_key_width(key);
    round(
        f,
        x,
        y,
        w,
        32.,
        8.,
        if selected {
            alpha(p.accent, 0.12)
        } else {
            p.surface2
        },
        Some(if selected {
            alpha(p.accent, 0.42)
        } else {
            p.line
        }),
    );
    f.push(DrawCmd::PushClip {
        x: x + 12.,
        y: y + 8.,
        w: (w - if key.is_empty() { 24. } else { key_w + 24. }).max(8.),
        h: 20.,
    });
    text(
        f,
        x + 12.,
        y + 8.,
        FONT_HINT,
        label,
        if selected { p.accent } else { p.text2 },
    );
    f.push(DrawCmd::PopClip);
    if !key.is_empty() {
        round(
            f,
            x + w - key_w - 10.,
            y + 7.,
            key_w,
            18.,
            4.,
            p.bg2,
            Some(alpha(p.line2, 0.8)),
        );
        text(f, x + w - key_w - 6., y + 8., 12., key, p.muted);
    }
    hs.push(Hit {
        x,
        y,
        w,
        h: 32.,
        action,
    });
    x + w
}
// The chrome uses the resolved theme, including user overrides.
#[allow(clippy::too_many_arguments)]
fn draw_chrome(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    sess: &Session,
    cfg: &Config,
    m: &Model,
    t: &Theme,
    w: f32,
    h: f32,
    now_ms: u64,
) {
    let p = tokens(t);
    // 极简模式：整张 chrome 收起，只留顶部静默带（两种正文模式通用）。
    if cfg.minimal {
        draw_minimal_chrome(f, hs, sess, cfg, m, t, w, h, now_ms);
        return;
    }
    rect(f, 0., 0., w, h, p.bg);

    rect(f, 0., 0., w, 48., p.bg2);
    line(f, 0., 48., w, p.line);
    body_text(f, 22., 11., 22., "符", p.accent);
    text(f, 53., 16., FONT_LABEL, "虎符 · 跟打", p.text2);
    rect(f, 143., 16., 1., 17., p.line2);
    f.push(DrawCmd::PushClip {
        x: 158.,
        y: 8.,
        w: (w - 294.).max(40.),
        h: 32.,
    });
    text(
        f,
        158.,
        16.,
        FONT_LABEL,
        elide_width(&sess.title, (w - 298.).max(40.), FONT_LABEL),
        p.muted,
    );
    f.push(DrawCmd::PopClip);
    for (i, (label, action)) in [
        ("—", Action::Minimize),
        ("□", Action::Maximize),
        ("×", Action::Exit),
    ]
    .into_iter()
    .enumerate()
    {
        let xx = w - 114. + i as f32 * 36.;
        text(f, xx + 11., 15., 14., label, p.muted);
        hs.push(Hit {
            x: xx,
            y: 4.,
            w: 36.,
            h: 40.,
            action,
        });
    }
    // 本地载入直接执行剪贴板动作；设置紧邻理论分析，不再经过载入面板。
    let y = 60.;
    let wide = w >= 960.;
    let compact = w < 800.;
    // 快捷键标签跟随自设（Ctrl+V 是固定编辑键）。
    let key_online = cfg.shortcut("online");
    let key_groups = cfg.shortcut("groups");
    let key_read = cfg.shortcut("read_group");
    let key_restart = cfg.shortcut("restart");
    let key_analysis = cfg.shortcut("analysis");
    let key_settings = cfg.shortcut("settings");
    let left = [
        (
            "从剪贴板载入",
            if wide { "Ctrl+V" } else { "" },
            Action::LoadClipboard,
            false,
        ),
        (
            "在线发文",
            if wide { key_online.as_str() } else { "" },
            Action::Open(Modal::Online),
            false,
        ),
        (
            "选群",
            if wide { key_groups.as_str() } else { "" },
            Action::Open(Modal::Groups),
            false,
        ),
        (
            "读取群文",
            if wide { key_read.as_str() } else { "" },
            Action::ReadGroup,
            false,
        ),
        (
            "重打",
            if wide { key_restart.as_str() } else { "" },
            Action::Restart,
            false,
        ),
    ];
    let mut xx = 24.;
    for (label, key, action, selected) in left {
        if compact && matches!(action, Action::Open(Modal::Groups) | Action::Restart) {
            continue;
        }
        xx = toolbar_button(f, hs, xx, y, label, key, action, t, selected) + 8.;
    }
    // 窄窗优先保留高频入口：<1080 时「更多」收进「视图」菜单页脚（同一组动作
    // 也在菜单里，F7/F9 等快捷键不变），避免与新增的「视图」按钮挤在一起。
    let show_more = w >= 1080.;
    let more_w = if show_more {
        toolbar_width("更多", "")
    } else {
        0.
    };
    let settings_w = toolbar_width("设置", key_settings.as_str());
    let analysis_w = toolbar_width("理论分析", key_analysis.as_str());
    // 【视图菜单 2026-11-12】工具栏入口：与右键菜单同一张卡片（按钮下缘展开）。
    let view_w = toolbar_width("视图", "");
    let more_gap = if show_more { more_w + 8. } else { 0. };
    let settings_x = w - 24. - more_gap - view_w - 8. - settings_w;
    let view_x = settings_x + settings_w + 8.;
    let analysis_x = settings_x - 8. - analysis_w;
    toolbar_button(
        f,
        hs,
        analysis_x,
        y,
        "理论分析",
        key_analysis.as_str(),
        Action::ToggleAnalysis,
        t,
        m.analysis_dock,
    );
    toolbar_button(
        f,
        hs,
        settings_x,
        y,
        "设置",
        key_settings.as_str(),
        Action::Open(Modal::Preferences),
        t,
        m.modal == Modal::Preferences,
    );
    toolbar_button(
        f,
        hs,
        view_x,
        y,
        "视图",
        "",
        Action::OpenMenuAt(view_x, y + 40.),
        t,
        m.menu.is_some(),
    );
    if show_more {
        toolbar_button(
            f,
            hs,
            w - 24. - more_w,
            y,
            "更多",
            "",
            Action::Open(Modal::More),
            t,
            false,
        );
    }
    line(f, 0., 104., w, p.line);
    // 底部状态行（成绩栏由 history.rs 绘制在它上方）。
    rect(f, 0., h - STATUS_H, w, STATUS_H, p.bg2);
    line(f, 0., h - STATUS_H, w, p.line);
    round(f, 20., h - STATUS_H + 11., 4., 4., 2., p.muted, None);
    let target = m
        .bound
        .and_then(|i| m.groups.get(i))
        .map(|name| format!("目标群 · {}", elide(name, 18)))
        .unwrap_or_else(|| "未选择群聊".into());
    let group_name = m.bound.and_then(|i| m.groups.get(i));
    let notice = if m.status.is_empty() || m.status.starts_with("就绪") {
        target
    } else if group_name
        .map(|name| m.status.contains(name.as_str()))
        .unwrap_or(false)
    {
        // 已绑定/已切到「群名」这类状态不再和目标群前缀重复。
        m.status.clone()
    } else {
        format!("{target} · {}", m.status)
    };
    f.push(DrawCmd::PushClip {
        x: 32.,
        y: h - STATUS_H + 4.,
        w: (w - 218.).max(100.),
        h: 22.,
    });
    text(
        f,
        32.,
        h - STATUS_H + 7.,
        FONT_HINT,
        elide_width(&notice, (w - 224.).max(100.), FONT_HINT),
        p.muted,
    );
    f.push(DrawCmd::PopClip);
    right(
        f,
        w - 24.,
        h - STATUS_H + 7.,
        FONT_HINT,
        if sess.current_segment().text.is_empty() {
            "等待载入文章"
        } else if m.paused {
            "已暂停 · 按键继续"
        } else if sess.phase == crate::session::Phase::Done {
            "本段完成"
        } else if sess.started_ms.is_some() {
            "练习中"
        } else {
            "首键开始计时"
        },
        p.muted,
    );
}
/// Effective article line height. 【五轮 2026-11-13】行距是用户拍板的：
/// 行高完全由 `fs * (1 + line_gap)`（config 已 clamp 到 0.10..=3.00 倍）决定。
/// 【2026-11-14 审计】去掉旧的 `max(fs + 6)` 地板：它让界面标称的 0.10 倍在
/// 正文里悄悄变成 ~1.05–1.17 倍，且与对照模式输入卡的行高不一致。行距压到
/// 字形以下时按用户设置重叠绘制——不替用户设上限/下限。
pub fn line_height_for_anchor(cfg: &Config, fs: f32, base: f32, hints_visible: bool) -> f32 {
    let _ = (hints_visible, cfg, fs);
    base
}

// ════════════════ 视图体系（2026-11-12）：极简模式 / 对照分栏 ════════════════
//
// 用户需求：① 「传统（对照）模式」——上方发文区对照、下方独立输入框跟打；
// ② 「极简模式」——红框里的工具栏/统计/底栏全部收起，正文满窗，两种模式通用；
// ③ 右键菜单集中常用设置；④ 任意模式下左键按住都能拖动窗口（见 main.rs）。

/// 【极简模式】顶部静默带：2px 进度线 + 状态行 + 一行完整数据（与常规统计带同指标）。
pub const MINIMAL_ROW_H: f32 = 26.;
/// 对照模式：文章卡与输入卡之间的分栏把手区高度。
pub const SPLIT_GAP: f32 = 12.;

/// 极简顶栏高度：数据行可关（`minimal_hud=false` 时只留状态行）。
pub fn minimal_band_h(cfg: &Config) -> f32 {
    if cfg.minimal_hud {
        76.
    } else {
        34.
    }
}

/// 极简模式正文起点（顶栏 + 8px 呼吸）。
pub fn minimal_top(cfg: &Config) -> f32 {
    minimal_band_h(cfg) + 8.
}

/// 兼容常量：默认（带数据行）极简正文起点。
pub const MINIMAL_TOP: f32 = 84.;

/// 极简模式右上角「窗口控制胶囊」矩形（常驻半透明，悬停提亮）。
pub fn minimal_controls_rect(w: f32) -> (f32, f32, f32, f32) {
    (w - 12. - 140., 5., 140., 26.)
}

/// 【顶部留白】常规模式统计带（含分隔线）的底边：正文带从这里起算，
/// 不再各自留 216/236 的空档——首行上方留白统一由 preedit_headroom 给（字体半高）。
pub fn chrome_band_bottom(cfg: &Config, h: f32) -> f32 {
    if cfg.minimal {
        return minimal_top(cfg);
    }
    if cfg.display_mode.is_split() && h < 560. {
        146.
    } else {
        187.
    }
}

/// 正文带的上下边界：极简模式满窗；常规模式按标题栏/工具栏/统计带/底栏让位。
pub fn body_region(cfg: &Config, m: &Model, w: f32, h: f32, single: bool) -> (f32, f32) {
    if cfg.minimal {
        let top = minimal_top(cfg);
        // 极简模式没有底栏：成绩面板/理论分析都是浮层，正文照常让位。
        let bottom = if m.analysis_dock {
            crate::modals::analysis_dock_rect_for(cfg, w, h).1 - 8.
        } else if m.ledger_open {
            crate::history::panel_rect_for(cfg, m, w, h).1 - 8.
        } else {
            h - 16.
        };
        return (top, bottom.max(top + 24.));
    }
    // 对照模式在矮窗口把统计带压成一行（见 build 里的紧凑统计条），正文带提前；
    // 上下各留 8px 呼吸，两张卡不贴统计带/底栏。
    // 【顶部留白】正文带贴住统计带底边 + 8px 呼吸；首行上方的半行留白由
    // preedit_headroom 统一给，不再按模式各自留固定空档（旧值 236/216/156）。
    let mut top = if m.analysis_dock && h < 700. {
        100.
    } else {
        chrome_band_bottom(cfg, h) + 8.
    };
    let bottom = if m.analysis_dock {
        crate::modals::analysis_dock_rect_for(cfg, w, h).1 - 8.
    } else if m.ledger_open {
        crate::history::panel_rect_for(cfg, m, w, h).1 - 8.
    } else {
        ledger_bar_y(h) - 8.
    };
    // A large composition row is part of the content, even while empty. In a
    // short window use one statistics row rather than cropping that content.
    let required = cfg.font_size
        + 7.
        + preedit_headroom(cfg)
        + if cfg.display_mode.is_split() { 38. } else { 0. };
    if !(m.analysis_dock && h < 700.) && bottom - top < required {
        top = 126.;
    }
    (top, bottom)
}
/// 对照模式几何：文章卡 / 分栏把手 / 输入卡。文章区占比取 `cfg.split_ratio`
///（拖动把手实时改），按整行吸附文章卡高度；极矮窗口文章区收起，保证输入卡至少一行。
#[derive(Debug, Clone, Copy)]
pub struct SplitGeom {
    pub x: f32,
    pub pane_w: f32,
    pub top: f32,
    pub article_h: f32,
    pub divider_y: f32,
    pub input_y: f32,
    pub input_h: f32,
    pub line_h: f32,
    /// 窗口太矮：文章对照区收起，只留输入卡（header 里有提示）。
    pub collapsed: bool,
}

/// 【传统模式输入区】输入卡头部（「输入 0 / 243 字」+ 分隔线）高度。
pub fn input_header_h() -> f32 {
    30.
}

/// 【传统模式输入区】输入卡正文首行文字盒顶。
/// 卡片被拖到最低（只容一行）时上下居中，避免字底被卡底切掉一点；
/// 卡片更高时仍按 8px 内边距贴顶，多行排版口径不变。
fn input_line_top(clip_top: f32, content_bottom: f32, line_h: f32, fs: f32) -> f32 {
    let available = (content_bottom - clip_top).max(fs);
    if available < line_h * 1.6 {
        clip_top + ((available - fs) * 0.5).max(0.)
    } else {
        clip_top + 8.
    }
}

/// 【传统模式输入区】光标竖条的（起笔偏移, 高度）。
/// 常态比文字盒多出 7px（上下各露出一点，视觉上跟字贴模式一致）；
/// 卡片只容一行时收紧到正好落在文字盒内，否则底边被卡口切掉一点。
fn input_caret_span(fs: f32, thin: bool) -> (f32, f32) {
    if thin {
        (2., (fs - 2.).max(2.))
    } else {
        (3., fs + 4.)
    }
}

pub fn split_geom(
    cfg: &Config,
    bw: f32,
    fs: f32,
    lh: f32,
    w: f32,
    top: f32,
    bottom: f32,
) -> SplitGeom {
    let x = (w - bw) / 2. - 12.;
    let pane_w = bw + 24.;
    let span = (bottom - top).max(48.);
    // 2026-11-13 二轮：输入卡行高同样吃 cfg.line_gap——此前固定 1.34 倍，
    // 行距调到最小输入区也不动（用户反馈「还是太大」的根因）。
    // 三轮：口径与 body_metrics 一致（负行距紧凑排版）；四轮：下限放到 0.10 倍。
    let line_h = fs * (1. + cfg.line_gap).clamp(0.10, 3.0);
    // A single article row needs a full glyph, not a trailing empty line pitch.
    let article_min = (fs + 20.).min(span * 0.62);
    // 【传统模式输入区】最低只到「一行字」的高度（header 30 + 一行 + 8 内边距），
    // 不再按 line_h + 编码留白抬到两行多——用户要求最低就是当前字号的一行。
    let input_min = (input_header_h() + fs + 8.).min(span * 0.6);
    let avail = (span - SPLIT_GAP).max(40.);
    let ratio = if cfg.split_ratio.is_finite() {
        // 2026-11-13 用户拍板：不设人为限制，拖到哪里算哪里（0.15..=0.97，
        // 下限只防把手粘到窗口顶，input_min/max_input 各自兜物理一行）。
        cfg.split_ratio.clamp(0.15, 0.97)
    } else {
        0.58
    };
    if avail < article_min + input_min {
        return SplitGeom {
            x,
            pane_w,
            top,
            article_h: 0.,
            divider_y: top,
            input_y: top,
            input_h: (bottom - top).max(24.),
            line_h,
            collapsed: true,
        };
    }
    let max_input = (avail - article_min).max(1.);
    let input_h = (avail * (1. - ratio)).clamp(input_min.min(max_input), max_input);
    let mut article_h = (avail - input_h).max(20.);
    // 整行吸附：多余高度让给输入卡，文章卡不会留一整块空白。
    // 2026-11-13 用户二轮反馈：输入卡已被拖到一行附近（≤ input_min+一行）时不吸附——
    // 反哺会把拖出来的高度吃回去甚至顶高，薄区拖到哪里算哪里；文章卡零头只是卡内留白。
    let rows = ((article_h - 20.) / lh).floor().max(1.);
    let snapped = rows * lh + 20.;
    if article_h - snapped > lh * 0.35 && input_h > input_min + line_h {
        article_h = snapped;
    }
    let divider_y = top + article_h;
    let input_y = divider_y + SPLIT_GAP;
    SplitGeom {
        x,
        pane_w,
        top,
        article_h,
        divider_y,
        input_y,
        input_h: (bottom - input_y).max(24.),
        line_h,
        collapsed: false,
    }
}

/// 极简模式常驻小动作：在线发文 / 选群 / 理论分析 / 成绩记录（右侧），
/// 与右上角窗口控制胶囊同排；统计行在下一行，指标与常规统计带一致。
fn draw_minimal_chrome(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    sess: &Session,
    cfg: &Config,
    m: &Model,
    t: &Theme,
    w: f32,
    h: f32,
    now_ms: u64,
) {
    let p = tokens(t);
    rect(f, 0., 0., w, h, p.bg);
    let band = minimal_band_h(cfg);
    line(f, 0., band, w, alpha(p.line, 0.85));
    let count = sess.current_segment().text.chars().count();
    let pos = sess.comparer.pos.min(count);
    rect(f, 0., 0., w, 2., alpha(p.line, 0.65));
    if count > 0 {
        rect(
            f,
            0.,
            0.,
            (w * pos as f32 / count as f32).clamp(0., w),
            2.,
            p.accent,
        );
    }
    // ── 第一行：状态（左）+ 四个常驻动作 + 窗口控制（右）──────────────
    let controls = minimal_controls_rect(w);
    let mut rx = controls.0 - 10.;
    for (label, action) in [
        ("成绩记录", Action::ToggleLedger),
        ("理论分析", Action::ToggleAnalysis),
        ("选群", Action::Open(crate::modals::Modal::Groups)),
        ("在线发文", Action::Open(crate::modals::Modal::Online)),
    ] {
        let bw = ui_text_width(label, 11.) + 16.;
        rx -= bw;
        let hot = m
            .pointer
            .is_some_and(|(px, py)| px >= rx && px < rx + bw && py >= 5. && py < 27.);
        if hot {
            round(f, rx, 5., bw, 22., 6., alpha(p.accent, 0.13), None);
        }
        text(
            f,
            rx + 8.,
            9.,
            11.,
            label,
            if hot { p.accent } else { p.muted },
        );
        hs.push(Hit {
            x: rx,
            y: 5.,
            w: bw,
            h: 22.,
            action,
        });
        rx -= 8.;
    }
    draw_minimal_controls(f, hs, m, t, w);
    // 【审计 V17】状态点/状态词按状态取主题键（提示/中性/进行中/完成四档），
    // 不再用 tokens 里 hex 硬编码的 copper / green——绿铜语义色在不同主题下
    // 既可能与打对/打错撞色，也不随自定义皮肤变化。
    let (dot, word) = if count == 0 {
        (status_color(&p, StatusTone::Hint), "等待载入文章")
    } else if m.paused {
        (status_color(&p, StatusTone::Neutral), "已暂停")
    } else if sess.phase == crate::session::Phase::Done {
        (status_color(&p, StatusTone::Done), "本段完成")
    } else if sess.started_ms.is_some() {
        (status_color(&p, StatusTone::Active), "练习中")
    } else {
        (status_color(&p, StatusTone::Hint), "首键开始计时")
    };
    round(f, 20., 14., 5., 5., 2.5, dot, None);
    let progress = if count == 0 {
        String::new()
    } else {
        format!(
            "第 {} / {} {} · {} / {} 字 · {word}",
            sess.index + 1,
            sess.total_segments(),
            if sess.source == SessionSource::Single {
                "组"
            } else {
                "段"
            },
            pos,
            count,
        )
    };
    let label = if m.status.contains("TSF") {
        m.status.clone()
    } else if progress.is_empty() {
        word.into()
    } else {
        progress
    };
    text(
        f,
        30.,
        7.5,
        13.,
        elide_width(&label, (rx - 34.).max(80.), 13.),
        p.muted,
    );
    // ── 第二行：与常规统计带一致的完整指标 ──────────────────────────
    if cfg.minimal_hud {
        let header = sess.header_stats(now_ms);
        let speed = format!("{:.1}", header.speed);
        mono(f, 20., 34., 24., speed.clone(), p.accent);
        let speed_w = ui_text_width(&speed, 24.);
        text(f, 20. + speed_w + 6., 44., 12., "字/分", p.muted);
        let elapsed = header.elapsed_ms as f64 / 1000.;
        let duration = format!("{:02}:{:04.1}", (elapsed / 60.) as u64, elapsed % 60.);
        let seg_start: usize = sess
            .segments
            .iter()
            .take(sess.index)
            .map(|s| s.text.chars().count())
            .sum();
        let code_at = if m.char_codes_whole {
            seg_start + pos
        } else if m.char_codes_seg == sess.index {
            pos
        } else {
            usize::MAX
        };
        let next_code = m
            .char_codes
            .get(code_at)
            .cloned()
            .flatten()
            .map(|cc| {
                if cc.rank > 1 && !cc.tail.is_empty() {
                    format!("{} {}", cc.code, cc.tail)
                } else {
                    cc.code
                }
            })
            .unwrap_or_else(|| "—".into());
        let mut items = vec![
            ("击键/秒", format!("{:.2}", header.keys_per_sec)),
            ("码长", format!("{:.2}", header.code_len)),
            ("键准", format!("{:.1}%", header.accuracy)),
            ("错字", header.wrong.to_string()),
            ("用时", duration),
            ("编码", next_code),
        ];
        if w < 760. {
            items.remove(1);
        }
        let start = 20. + speed_w + 52.;
        let end = w - 20.;
        let pitch = ((end - start) / items.len() as f32).max(72.);
        for (i, (label, value)) in items.iter().enumerate() {
            let x = start + i as f32 * pitch;
            text(f, x, 33., 12.5, *label, p.muted);
            mono(f, x, 48., 15., value, p.text2);
        }
    }
}

/// 极简模式右上角窗口控制胶囊：[视图][—][□][×]。
/// 常驻半透明（极简也不能没有关窗口/换模式的入口），悬停提亮。
fn draw_minimal_controls(f: &mut Frame, hs: &mut Vec<Hit>, m: &Model, t: &Theme, w: f32) {
    let p = tokens(t);
    let (x, y, cw, ch) = minimal_controls_rect(w);
    let hovered = m
        .pointer
        .is_some_and(|(px, py)| px >= x && px <= x + cw && py >= y && py <= y + ch);
    round(
        f,
        x,
        y,
        cw,
        ch,
        9.,
        alpha(p.surface, if hovered { 0.97 } else { 0.6 }),
        Some(alpha(p.line, if hovered { 0.95 } else { 0.55 })),
    );
    let cells = [
        ("view", Action::OpenMenuAt(x + 4., y + ch + 6.)),
        ("min", Action::Minimize),
        ("max", Action::Maximize),
        ("close", Action::Exit),
    ];
    for (i, (kind, action)) in cells.into_iter().enumerate() {
        let cx = x + 4. + i as f32 * 33.;
        let cy = y + 4.;
        let (cw2, ch2) = (33., ch - 8.);
        let hot = m
            .pointer
            .is_some_and(|(px, py)| px >= cx && px < cx + cw2 && py >= cy && py < cy + ch2);
        if hot {
            round(
                f,
                cx,
                cy,
                cw2,
                ch2,
                6.,
                alpha(if kind == "close" { p.red } else { p.accent }, 0.14),
                None,
            );
        }
        let color = if hot {
            if kind == "close" {
                p.red
            } else {
                p.accent
            }
        } else {
            p.text2
        };
        let mid = cy + ch2 / 2.;
        match kind {
            "view" => {
                for k in 0..3 {
                    let ly = mid - 5. + k as f32 * 5.;
                    rect(f, cx + 11., ly, 11., 1.2, color);
                    let kx = cx + 11. + if k == 1 { 6. } else { 2. };
                    rect(f, kx, ly - 1.2, 2.4, 3.6, color);
                }
            }
            "min" => rect(f, cx + 11., mid, 11., 1.2, color),
            "max" => round(f, cx + 11., mid - 5., 11., 10., 2., [0.; 4], Some(color)),
            _ => {
                run(
                    f,
                    cx + cw2 / 2.,
                    mid - 7.,
                    13.,
                    "×",
                    color,
                    FontFamily::Ui,
                    Align::Center,
                );
            }
        }
        hs.push(Hit {
            x: cx,
            y: cy,
            w: cw2,
            h: ch2,
            action,
        });
    }
}
/// 对照模式：上文章卡（对照）+ 分栏把手 + 下输入卡（实打字）。
/// 卡面色（2026-11-13 语义对调修复）：输入卡=跟打区背景色（用户在设置里
/// 唯一可自定义的卡面底色，跟打区在哪它就该在哪），文章卡=次级背景色；
/// 字贴模式的正文区同样吃跟打区背景色——两种模式的「跟打面」一致受控。
#[allow(clippy::too_many_arguments)]
fn draw_split_body(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    sess: &Session,
    cfg: &Config,
    m: &Model,
    t: &Theme,
    w: f32,
    h: f32,
    bw: f32,
    fs: f32,
    lh: f32,
    lines: &[(usize, String, Vec<f32>)],
    input_lines: &[(usize, String, Vec<f32>)],
    now: f64,
    anim: &mut Animation,
) {
    let p = tokens(t);
    let single = sess.source == SessionSource::Single;
    let (top, bottom) = body_region(cfg, m, w, h, single);
    let g = split_geom(cfg, bw, fs, lh, w, top, bottom);
    let spacing = cfg.letter_spacing.clamp(0., 12.);
    anim.split_divider = if g.collapsed {
        None
    } else {
        Some((g.x, g.divider_y, g.pane_w, SPLIT_GAP))
    };
    anim.input_line_h = g.line_h;
    let word_hints = m.hints;
    let pos = sess.comparer.pos;
    let inner_x = g.x + 12.;
    // 【审计 V12】当前输入层的三个通道一次算好，文章对照卡与输入卡共用同一配方。
    let (cur_color, cur_bg, _) = current_input_style(t);
    if !g.collapsed {
        // ── 文章对照卡 ─────────────────────────────────────────────────
        round(
            f,
            g.x,
            g.top,
            g.pane_w,
            g.article_h,
            12.,
            p.bg2,
            Some(alpha(p.line, 0.85)),
        );
        let rows = ((g.article_h - 20.) / lh).floor().max(1.) as usize;
        let active = lines.iter().rposition(|l| l.0 <= pos).unwrap_or(0);
        // 只有 ≥2 行时才留一行上下文；否则必须显示当前行（否则光标不可见）。
        let context = if rows >= 2 { 1 } else { 0 };
        let first = active
            .saturating_sub(context)
            .min(lines.len().saturating_sub(rows));
        // 【动效 2026-11-15】对照模式文章卡翻行同样缓出，别一跳到下一屏。
        // scroll_offset 返回的是「行数」，这里必须乘行高换成像素，否则位移只有 1px。
        let scroll_dy =
            scroll_offset(anim, first, now, !(m.reduced_motion || !cfg.smooth_caret)) * lh;
        f.push(DrawCmd::PushClip {
            x: g.x + 1.,
            y: g.top + 1.,
            w: g.pane_w - 2.,
            h: (g.article_h - 2.).max(1.),
        });
        for (li, (offset, line_text, widths)) in lines.iter().enumerate().skip(first).take(rows) {
            let yy = g.top + 10. + (li - first) as f32 * lh + scroll_dy;
            let mut spans = Vec::new();
            let mut u16pos = 0u32;
            for (j, ch) in line_text.chars().enumerate() {
                let char_at = offset + j;
                let state = sess.comparer.states.get(char_at);
                let tint = (word_hints && cfg.hint_color_words)
                    .then(|| {
                        let at = m
                            .words
                            .partition_point(|word| word.start + word.len <= char_at);
                        let word = m.words.get(at).filter(|word| word.start <= char_at)?;
                        hint_word_tint_for_rank(
                            &p,
                            &word.text,
                            word.code.as_deref()?,
                            word.rank,
                            m.max_code_length,
                            cfg.hint_rank_colors,
                        )
                    })
                    .flatten();
                // 【审计 V12】当前输入位单列一层：字形色 + 底块 + 实心下划线三通道，
                // 不再和其他字形色抢同一套颜色；【审计 V17】状态→角色→主题键色。
                let role = classify_glyph(state.copied(), char_at == pos, tint.is_some());
                let c = if role == GlyphRole::Correct {
                    // 对照卡「已定稿」只淡化不透明度表达参照层级，色值仍是打对字色。
                    alpha(p.correct, reference_settled_alpha())
                } else {
                    glyph_color(&p, role, tint, cur_color)
                };
                if char_at == pos {
                    let a = widths.get(j).copied().unwrap_or(0.);
                    let b = widths.get(j + 1).copied().unwrap_or(a + fs);
                    round(
                        f,
                        inner_x + a - 1.,
                        yy + 2.,
                        (b - a).max(4.) + 2.,
                        fs + 4.,
                        4.,
                        cur_bg,
                        Some(alpha(p.accent, 0.30)),
                    );
                }
                spans.push((u16pos, ch.len_utf16() as u32, c));
                u16pos += ch.len_utf16() as u32;
                if state == Some(&CharState::Wrong) {
                    let a = widths.get(j).copied().unwrap_or(0.);
                    let b = widths.get(j + 1).copied().unwrap_or(a + fs);
                    let mut px = inner_x + a + 3.;
                    while px < inner_x + b - 2. {
                        rect(f, px, yy + fs + 3., 2., 1., p.red);
                        px += 4.;
                    }
                }
            }
            styled_line(
                f, inner_x, yy, bw, fs, line_text, spans, p.text, spacing, widths,
            );
            // 【八轮重做】词提独立于行距：不再有空间门槛/缩号/退让——括线、编码行、
            // 选重数字一律在行内**原位**按设置字号原样绘制，行距紧时与下一行文字
            // 重叠也照画（独立层，不与文字行抢高度，也不再挪去别处）。
            if word_hints {
                for z in &m.words {
                    if z.start + z.len <= pos
                        || z.start < *offset
                        || z.start >= offset + line_text.chars().count()
                    {
                        continue;
                    }
                    let local = z.start - offset;
                    let wx = inner_x + widths.get(local).copied().unwrap_or(0.);
                    let ww = widths
                        .get((local + z.len).min(widths.len() - 1))
                        .copied()
                        .unwrap_or(bw)
                        - widths.get(local).copied().unwrap_or(0.);
                    if !z.text.chars().any(char::is_alphanumeric) {
                        continue;
                    }
                    if let Some(code) = &z.code {
                        let tint = cfg
                            .hint_color_words
                            .then(|| {
                                hint_word_tint_for_rank(
                                    &p,
                                    &z.text,
                                    code,
                                    z.rank,
                                    m.max_code_length,
                                    cfg.hint_rank_colors,
                                )
                            })
                            .flatten();
                        if crate::analysis::is_multi_word(&z.text) {
                            let underline = tint
                                .map(|color| alpha(color, 0.62))
                                .unwrap_or(p.hint_underline);
                            word_bracket(f, wx, yy, ww, fs, underline);
                        }
                        let hint_size = cfg.hint_font_size;
                        let shown_code = if cfg.hint_show_code {
                            code.as_str()
                        } else {
                            ""
                        };
                        let rank = if cfg.hint_show_rank && z.rank > 1 {
                            z.rank.to_string()
                        } else {
                            String::new()
                        };
                        // 选重数：编码行内原位（角标开关开启时画右上角标）。
                        let rank_in_code_row = rank.is_empty() || !cfg.hint_rank_corner;
                        let hint_advance = hint_size * 0.6;
                        let code_width = shown_code.chars().count() as f32 * hint_advance;
                        let rank_size = hint_size * 0.78;
                        let rank_width = if rank_in_code_row {
                            rank.chars().count() as f32 * rank_size * 0.6
                        } else {
                            0.
                        };
                        let code_rank_gap = if code_width > 0. && rank_width > 0. {
                            2.
                        } else {
                            0.
                        };
                        let hx = wx + (ww - code_width - code_rank_gap - rank_width).max(0.) / 2.;
                        if !shown_code.is_empty() {
                            // 【词提下行 r3 2026-11-14】括线回校准紧贴位（fs+offset），
                            // 编码行让到括线之下 +2——编码永远在杯外。
                            mono(
                                f,
                                hx,
                                hint_code_y(cfg, yy, fs),
                                hint_size,
                                shown_code,
                                alpha(tint.unwrap_or(p.hint_code), 1.),
                            );
                        }
                        if !rank.is_empty() {
                            let rank_color = if cfg.hint_rank_colors {
                                tint.unwrap_or(p.hint_badge)
                            } else {
                                p.hint_badge
                            };
                            if cfg.hint_rank_corner {
                                // 【词提下行 2026-11-14】角标改词右下角（下标式）。
                                let corner_size = (fs * 0.52).max(9.);
                                let corner_w = rank.chars().count() as f32 * corner_size * 0.6;
                                rank_digit(
                                    f,
                                    cfg,
                                    wx + ww - corner_w - 1.,
                                    yy + fs - corner_size * 0.62,
                                    corner_size,
                                    &rank,
                                    rank_color,
                                );
                            } else {
                                rank_digit(
                                    f,
                                    cfg,
                                    hx + code_width + code_rank_gap,
                                    hint_code_y(cfg, yy, fs) + (hint_size - rank_size),
                                    rank_size,
                                    &rank,
                                    rank_color,
                                );
                            }
                        }
                    }
                }
            }
        }
        f.push(DrawCmd::PopClip);

        // ── 分栏把手：hover/拖动时提亮 ─────────────────────────────────
        let divider_hot = m.pointer.is_some_and(|(px, py)| {
            px >= g.x && px <= g.x + g.pane_w && py >= g.divider_y && py < g.divider_y + SPLIT_GAP
        });
        round(
            f,
            (w - 40.) / 2.,
            g.divider_y + SPLIT_GAP / 2. - 2.,
            40.,
            4.,
            2.,
            if divider_hot {
                p.accent
            } else {
                alpha(p.line2, 0.85)
            },
            None,
        );
    }

    // ── 输入卡 ─────────────────────────────────────────────────────
    let done = sess.phase == crate::session::Phase::Done;
    let typing = sess.started_ms.is_some() && !m.paused && !done;
    let border = if done {
        // 【审计 V17】完成态边框取「完成」档状态色（打对字色），不再用 hex 硬编码的 green。
        alpha(status_color(&p, StatusTone::Done), 0.75)
    } else if typing {
        alpha(p.accent, 0.35)
    } else {
        p.line2
    };
    round(
        f,
        g.x,
        g.input_y,
        g.pane_w,
        g.input_h,
        12.,
        p.article_bg,
        Some(border),
    );
    let pad_x = 12.;
    let input_header = if g.collapsed && g.input_h < fs + 7. + preedit_headroom(cfg) + 38. {
        0.
    } else {
        30.
    };
    if input_header > 0. {
        let header_y = g.input_y + 8.;
        text(f, g.x + pad_x, header_y + 1., 11., "输入", p.muted);
        let count = sess.current_segment().text.chars().count();
        mono(
            f,
            g.x + pad_x + 30.,
            header_y,
            11.,
            format!("{pos} / {count} 字"),
            alpha(p.accent, 0.85),
        );
        if done {
            let speed = sess
                .results
                .get(sess.current_index())
                .and_then(Option::as_ref)
                .map(|r| r.speed)
                .unwrap_or(0.);
            right(
                f,
                g.x + g.pane_w - pad_x,
                header_y + 1.,
                11.,
                format!("本段完成 · {speed:.1} 字/分"),
                status_color(&p, StatusTone::Done),
            );
        } else if g.collapsed {
            // 窗口太矮：文章对照区收起，输入卡独占（header 里说明原因）。
            right(
                f,
                g.x + g.pane_w - pad_x,
                header_y + 1.,
                11.,
                "对照区已收起 · 拉大窗口恢复",
                p.muted,
            );
        }
        line(
            f,
            g.x + pad_x,
            g.input_y + 26.,
            g.pane_w - 2. * pad_x,
            alpha(p.line, 0.8),
        );
    }
    let clip_top = g.input_y + input_header;
    let content_bottom = g.input_y + g.input_h - if input_header == 0. { 1. } else { 8. };
    let text_inset = (g.line_h - fs) / 2.;
    // 【传统模式输入区】卡被压到最低（只容一行）时，按可视线盒上下居中；
    // 否则字底会被卡底切掉一点（用户：「下面的会挡一点点，应该是没有上下居中的原因」）。
    let content_top = input_line_top(clip_top, content_bottom, g.line_h, fs);
    // 卡只有一行高时，光标竖条（常态 fs+4 且自 +3 起笔）会比可见文字盒多出 7px，
    // 底边被卡口切掉（用户：「下面的会挡一点点」）。此处按紧口径画，竖条正好落在卡内。
    let thin_input = (content_bottom - clip_top) < g.line_h * 1.6;
    let row_top = content_top - text_inset;
    // 框底常留一行空白：当前行固定在倒数第二行，文字行向上堆叠、超出后滚动。
    let base_y = (content_bottom - g.line_h * 2.).max(row_top);
    let row_gap = preedit_row_gap(cfg, fs, g.line_h);
    let capacity = (((content_bottom - g.line_h) - row_top - row_gap) / g.line_h)
        .floor()
        .max(1.) as usize;
    let rows = capacity.min(input_lines.len().max(1));
    let first = input_lines.len().saturating_sub(rows);
    let visible = input_lines.len().saturating_sub(first).min(rows);
    let gap = if visible > 1 { row_gap } else { 0. };
    let start_y = base_y - (visible.saturating_sub(1)) as f32 * g.line_h - gap;
    f.push(DrawCmd::PushClip {
        x: g.x + 1.,
        y: content_top - 2.,
        w: g.pane_w - 2.,
        h: (content_bottom - content_top + 4.).max(1.),
    });
    let mut caret_at = (inner_x, base_y + text_inset);
    for (i, (offset, line_text, widths)) in input_lines.iter().enumerate().skip(first).take(rows) {
        let yy = start_y
            + (i - first) as f32 * g.line_h
            + text_inset
            + if i == input_lines.len() - 1 { gap } else { 0. };
        let mut spans = Vec::new();
        let mut u16pos = 0u32;
        for (j, ch) in line_text.chars().enumerate() {
            let char_at = offset + j;
            // 【审计 V17】反馈色按状态取：输入卡不再把「未打/待定」也算成打对色。
            // 【审计 V12】当前位单独一层（光标通常落在已打文本末尾，此时只有光标）。
            let role = classify_glyph(
                sess.comparer.states.get(char_at).copied(),
                char_at == pos,
                false,
            );
            let c = glyph_color(&p, role, None, cur_color);
            spans.push((u16pos, ch.len_utf16() as u32, c));
            u16pos += ch.len_utf16() as u32;
            if char_at == pos {
                let a = widths.get(j).copied().unwrap_or(0.);
                let b = widths.get(j + 1).copied().unwrap_or(a + fs);
                round(
                    f,
                    inner_x + a - 1.,
                    yy + 2.,
                    (b - a).max(4.) + 2.,
                    fs + 4.,
                    4.,
                    cur_bg,
                    Some(alpha(p.accent, 0.30)),
                );
            }
            if sess.comparer.states.get(char_at) == Some(&CharState::Wrong) {
                let a = widths.get(j).copied().unwrap_or(0.);
                let b = widths.get(j + 1).copied().unwrap_or(a + fs);
                let mut px = inner_x + a + 3.;
                while px < inner_x + b - 2. {
                    rect(f, px, yy + fs + 3., 2., 1., p.red);
                    px += 4.;
                }
            }
        }
        styled_line(
            f, inner_x, yy, bw, fs, line_text, spans, p.pending, spacing, widths,
        );
        if i == input_lines.len() - 1 {
            let col = pos
                .saturating_sub(*offset)
                .min(widths.len().saturating_sub(1));
            caret_at = (inner_x + widths.get(col).copied().unwrap_or(0.), yy);
        }
    }
    let (caret_x, caret_y) = caret_at;
    // 绘制光标（平滑跟随，与字贴模式同一套动效语义）。
    let target = (caret_x, caret_y);
    // 【动效 2026-11-15】缓动跟随（130ms 缓出），换行也不再闪现。
    let caret = smooth_caret(
            anim,
            target,
            now,
            !(m.reduced_motion || !cfg.smooth_caret),
            g.line_h,
        );
    let caret_y_target = target.1;
    // 【审计 V17】完成态光标取「完成」档状态色（打对字色），与完成态边框/摘要同源。
    let caret_color = if done {
        status_color(&p, StatusTone::Done)
    } else {
        p.accent
    };
    let (caret_dy, caret_h) = input_caret_span(fs, thin_input);
    rect(f, caret.0 - 4., caret.1 + caret_dy, 2., caret_h, caret_color);
    // 【传统模式键入编码】编码行画在当前字的正下方（与字贴模式编码行同一语法），
    // 不再浮在文字上方；下方放不下时贴住裁剪区底边，保证可见。
    draw_input_preedit(
        f,
        cfg,
        &sess.preedit,
        (caret.0, caret_y_target),
        inner_x,
        inner_x + bw,
        content_bottom,
        p.accent,
    );
    f.push(DrawCmd::PopClip);
    // 完成态（极简）：输入卡内浮现完成摘要，不用底栏。
    if done && cfg.minimal {
        let record = sess
            .results
            .get(sess.current_index())
            .and_then(Option::as_ref);
        let summary = record
            .map(|r| format!("本段完成  {:.1} 字/分 · 键准 {:.1}%", r.speed, r.accuracy))
            .unwrap_or_else(|| "本段完成".into());
        let pill_w = (ui_text_width(&summary, 13.) + 28.).min(g.pane_w - 24.);
        let px = g.x + (g.pane_w - pill_w) / 2.;
        let py = g.input_y + g.input_h / 2. - 16.;
        round(
            f,
            px,
            py,
            pill_w,
            32.,
            9.,
            alpha(p.surface, 0.96),
            Some(alpha(status_color(&p, StatusTone::Done), 0.45)),
        );
        text(f, px + 14., py + 9., 13., summary, p.text);
        text(
            f,
            px + 14.,
            py + 40.,
            11.,
            &format!("Enter 下一段 · {} 重打", cfg.shortcut("restart")),
            p.muted,
        );
    }
    let _ = hs;
}

/// 兼容入口：不需要对照输入行的调用方（测试/旧示例）走这里。
pub fn build(
    sess: &Session,
    cfg: &Config,
    t: &Theme,
    m: &Model,
    w: f32,
    h: f32,
    now_ms: u64,
    now: f64,
    anim: &mut Animation,
    hs: &mut Vec<Hit>,
    lines: &[(usize, String, Vec<f32>)],
) -> Frame {
    build_with_input(sess, cfg, t, m, w, h, now_ms, now, anim, hs, lines, &[])
}

/// 正文渲染主入口。`input_lines` = 用户已打字符的换行结果（对照模式输入卡用；
/// 字贴模式忽略）。输入行由主程序用真实 Renderer::wrap_text 量出，这里只消费。
#[allow(clippy::too_many_arguments)]
pub fn build_with_input(
    sess: &Session,
    cfg: &Config,
    t: &Theme,
    m: &Model,
    w: f32,
    h: f32,
    now_ms: u64,
    now: f64,
    anim: &mut Animation,
    hs: &mut Vec<Hit>,
    lines: &[(usize, String, Vec<f32>)],
    input_lines: &[(usize, String, Vec<f32>)],
) -> Frame {
    let mut f = Frame::new(w, h);
    hs.clear();
    draw_chrome(&mut f, hs, sess, cfg, m, t, w, h, now_ms);
    let p = tokens(t);
    if sess.current_segment().text.is_empty() {
        if cfg.minimal {
            f.push(DrawCmd::TextRun {
                x: w / 2.,
                y: 158.,
                h: 22.,
                text: "载入文章后开始跟打".into(),
                color: p.text,
                font: FontFamily::Ui,
                align: Align::Center,
            });
            f.push(DrawCmd::TextRun {
                x: w / 2.,
                y: 196.,
                h: FONT_HINT,
                text: format!(
                    "Ctrl+V 从剪贴板载入 · {} 在线取文 · {} 读取群文 · 右键打开视图菜单",
                    cfg.shortcut("online"),
                    cfg.shortcut("read_group")
                ),
                color: p.muted,
                font: FontFamily::Ui,
                align: Align::Center,
            });
            return f;
        }
        if cfg.display_mode.is_split() {
            // 对照模式空态：两张占位卡说明「上对照、下输入」。
            let (top, bottom) = body_region(cfg, m, w, h, false);
            let (bw, fs, lh) = body_metrics(sess, cfg, w);
            let g = split_geom(cfg, bw, fs, lh, w, top, bottom);
            round(
                &mut f,
                g.x,
                g.top,
                g.pane_w,
                g.article_h,
                12.,
                p.bg2,
                Some(alpha(p.line, 0.8)),
            );
            text(
                &mut f,
                g.x + 20.,
                g.top + 16.,
                18.,
                "载入文章后开始跟打",
                p.text,
            );
            text(
                &mut f,
                g.x + 20.,
                g.top + 44.,
                FONT_HINT,
                &format!(
                    "上方显示对照文章，下方输入框跟打。Ctrl+V 载入 · {} 在线取文 · {} 读取群文",
                    cfg.shortcut("online"),
                    cfg.shortcut("read_group")
                ),
                p.muted,
            );
            round(
                &mut f,
                g.x,
                g.input_y,
                g.pane_w,
                g.input_h,
                12.,
                p.article_bg,
                Some(p.line2),
            );
            text(&mut f, g.x + 16., g.input_y + 10., 11., "输入", p.muted);
            text(
                &mut f,
                g.x + 16.,
                g.input_y + 34.,
                15.,
                "这里将显示你打出的字",
                alpha(p.text2, 0.5),
            );
            return f;
        }
        text(&mut f, 32., 136., 20., "载入文章后开始跟打", p.text);
        text(
            &mut f,
            32.,
            176.,
            FONT_HINT,
            "点击「从剪贴板载入」或按 Ctrl+V，也可在线取文、读取群文。",
            p.muted,
        );
        // 【审计 V18】空态以前只有一句「按 Ctrl+V」，没有可点的起点：补两个入口，
        // 主按钮走剪贴板载入，次按钮直接开在线发文弹窗。
        if h > 320. {
            primary_button(&mut f, hs, 32., 200., 148., "从剪贴板载入", Action::LoadClipboard, t);
            button(&mut f, hs, 192., 200., 104., "在线取文", Action::Open(Modal::Online), t, false);
        }
        if !(m.analysis_dock && h < 520.) {
            crate::history::draw(&mut f, hs, sess, cfg, m, t, w, h);
        }
        if m.analysis_dock && !m.skip_analysis_dock && m.modal == Modal::None {
            crate::modals::draw_analysis_dock_for(&mut f, hs, m, sess, t, cfg, w, h);
        }
        return f;
    }
    let single = sess.source == SessionSource::Single;
    let word_hints = m.hints;
    let hints_visible = word_hints && (cfg.hint_show_code || cfg.hint_show_rank);
    let (bw, fs, base_lh) = body_metrics(sess, cfg, w);
    let lh = line_height_for_anchor(cfg, fs, base_lh, hints_visible);
    let x = (w - bw) / 2.;
    let pos = sess.comparer.pos;
    let header = sess.header_stats(now_ms);
    let count = sess.current_segment().text.chars().count();
    let elapsed = header.elapsed_ms as f64 / 1000.;
    let compact_analysis = !cfg.minimal && m.analysis_dock && h < 700.;
    let minimal = cfg.minimal;
    let split = cfg.display_mode.is_split();
    let (top, bottom) = body_region(cfg, m, w, h, single);
    let compact_stats = !minimal && !compact_analysis && top == 126.;
    anim.input_line_h = lh;
    anim.split_divider = None;
    // 【字间距 2026-11-13】> 0 时按“字宽 + 字距”重排文章/输入行；所有下游位置
    //（着色、词提括线、光标、命中区）统一消费重排后的 widths。
    let spacing = cfg.letter_spacing.clamp(0., 12.);
    let lines_adj;
    let input_adj;
    let (lines, input_lines): (&[(usize, String, Vec<f32>)], &[(usize, String, Vec<f32>)]) =
        if spacing > 0. {
            lines_adj = flow_with_spacing(lines, spacing, bw);
            input_adj = flow_with_spacing(input_lines, spacing, bw);
            (&lines_adj, &input_adj)
        } else {
            (lines, input_lines)
        };
    if !minimal && !split {
        rect(
            &mut f,
            x - 12.,
            top - 8.,
            bw + 24.,
            (bottom - top + 16.).max(1.),
            p.article_bg,
        );
    }
    if !minimal && !compact_analysis {
        // 对照模式 + 矮窗口（<560）：统计带压成一行，把高度让给文章/输入两栏。
        if compact_stats {
            let duration = format!("{:02}:{:04.1}", (elapsed / 60.) as u64, elapsed % 60.);
            let summary = format!(
                "{:.1} 字/分 · 击键 {:.2} · 码长 {:.2} · 键准 {:.1}% · 错 {} · {duration} · {pos}/{count}",
                header.speed, header.keys_per_sec, header.code_len, header.accuracy, header.wrong,
            );
            text(
                &mut f,
                x,
                106.,
                13.,
                elide_width(&summary, bw, 13.),
                p.text2,
            );
        } else if split && h < 560. {
            let speed = format!("{:.1}", header.speed);
            mono(&mut f, x, 108., 22., speed.clone(), p.accent);
            let sw = ui_text_width(&speed, 22.);
            text(&mut f, x + sw + 5., 114., 11., "字/分", p.muted);
            let duration = format!("{:02}:{:04.1}", (elapsed / 60.) as u64, elapsed % 60.);
            let progress = format!(
                "第 {} / {} {} · {} / {} 字",
                sess.index + 1,
                sess.total_segments(),
                if single { "组" } else { "段" },
                pos,
                count,
            );
            let progress = elide_width(&progress, (bw - sw - 52.).max(80.), FONT_HINT);
            right(&mut f, x + bw, 106., FONT_HINT, progress, p.text2);
            let line = format!(
                "击键 {:.2} · 码长 {:.2} · 键准 {:.1}% · 错字 {} · 用时 {duration}",
                header.keys_per_sec, header.code_len, header.accuracy, header.wrong,
            );
            text(
                &mut f,
                x,
                132.,
                11.,
                elide_width(&line, (bw - 8.).max(80.), 11.),
                p.muted,
            );
            rect(&mut f, x, 145., bw, 2., p.line);
            rect(
                &mut f,
                x,
                145.,
                bw * (pos as f32 / count.max(1) as f32).clamp(0., 1.),
                2.,
                p.accent,
            );
        } else {
            let speed = format!("{:.1}", header.speed);
            let speed_size = 30_f32.min(112. / (speed.len().max(1) as f32 * 0.6));
            mono(&mut f, x, 117., speed_size, speed.clone(), p.accent);
            // 速度只占固定槽位：单位右对齐到槽右缘、指标列从槽外起算 ——
            // 速度位数变化（0.0 → 123.4）时整条统计带不会横跳。
            let hero_w = 196.;
            right(&mut f, x + hero_w - 6., 134., 13., "字/分", p.muted);
            let duration = format!("{:02}:{:04.1}", (elapsed / 60.) as u64, elapsed % 60.);
            let mut metrics = vec![
                ("击键/秒", format!("{:.2}", header.keys_per_sec)),
                ("码长", format!("{:.2}", header.code_len)),
                ("键准", format!("{:.1}%", header.accuracy)),
                ("错字", header.wrong.to_string()),
                ("用时", duration.clone()),
            ];
            if w < 760. {
                metrics.remove(1);
            }
            // 【下一个字编码 2026-11-12】光标处单字的独立编码（非词提配方）：
            // 全文坐标按段起点换算；rank>1 追加选重尾键。
            let seg_start: usize = sess
                .segments
                .iter()
                .take(sess.index)
                .map(|s| s.text.chars().count())
                .sum();
            let code_at = if m.char_codes_whole {
                seg_start + pos
            } else if m.char_codes_seg == sess.index {
                pos
            } else {
                usize::MAX
            };
            let next_code = m
                .char_codes
                .get(code_at)
                .cloned()
                .flatten()
                .map(|cc| {
                    if cc.rank > 1 && !cc.tail.is_empty() {
                        format!("{} {}", cc.code, cc.tail)
                    } else {
                        cc.code
                    }
                })
                .unwrap_or_else(|| "—".into());
            metrics.push(("编码", next_code));
            let metrics_x = x + hero_w;
            let metrics_w = (bw - hero_w).max(300.);
            let pitch = (metrics_w / metrics.len() as f32).max(84.);
            for (i, (label, value)) in metrics.iter().enumerate() {
                let xx = metrics_x + i as f32 * pitch;
                text(&mut f, xx, 116., FONT_HINT, *label, p.muted);
                mono(&mut f, xx, 138., 18., value, p.text2);
            }
            let progress = format!(
                "第 {} / {} {} · {} / {} 字",
                sess.index + 1,
                sess.total_segments(),
                if single { "组" } else { "段" },
                pos,
                count,
            );
            text(
                &mut f,
                x,
                164.,
                FONT_HINT,
                elide_width(
                    &progress,
                    if w < 900. || (single && split) {
                        bw - 116.
                    } else {
                        bw
                    },
                    FONT_HINT,
                ),
                p.muted,
            );
            rect(&mut f, x, 185., bw, 2., p.line);
            rect(
                &mut f,
                x,
                185.,
                bw * (pos as f32 / count.max(1) as f32).clamp(0., 1.),
                2.,
                p.accent,
            );
            if single {
                // 字贴模式字提开关在正文上方；对照模式挪到进度行右端，避免压文章卡。
                // 【顶部留白】字贴模式也把字提开关并入进度行右端，
                // 不再独占一行（旧位置 199 顶着正文首行，逼得正文带整体下移）。
                let (cy, cx) = (158., x + bw - 98.);
                chip(
                    &mut f,
                    hs,
                    cx,
                    cy,
                    98.,
                    if m.hints {
                        "字提 · 开"
                    } else {
                        "字提 · 关"
                    },
                    Action::ToggleHints,
                    &p,
                    m.hints,
                );
            }
        }
    }
    if split {
        draw_split_body(
            &mut f,
            hs,
            sess,
            cfg,
            m,
            t,
            w,
            h,
            bw,
            fs,
            lh,
            lines,
            input_lines,
            now,
            anim,
        );
    } else {
        let active = lines.iter().rposition(|l| l.0 <= pos).unwrap_or(0);
        let text_top = top + preedit_headroom(cfg).min((bottom - top - fs - 7.).max(0.));
        let row_gap = preedit_row_gap(cfg, fs, lh);
        let mut rows = ((bottom - text_top) / lh).floor().max(1.) as usize;
        let first_for = |rows: usize| {
            if single {
                active
                    .saturating_sub(rows / 2)
                    .min(lines.len().saturating_sub(rows))
            } else {
                active
                    .saturating_sub(rows.saturating_sub(1).min(1))
                    .min(lines.len().saturating_sub(rows))
            }
        };
        let mut first = first_for(rows);
        if active > first && row_gap > 0. {
            rows = ((bottom - text_top - row_gap) / lh).floor().max(1.) as usize;
            first = first_for(rows);
        }
        let gap = if active > first { row_gap } else { 0. };
        let visible_rows = lines.len().saturating_sub(first).min(rows);
        let stage_y = if single {
            text_top + ((bottom - text_top) - visible_rows as f32 * lh - gap).max(0.) / 2.
        } else {
            text_top
        };
        // 【动效 2026-11-15】换行时整屏不再瞬移：把首行差值缓出成位移。
        // scroll_offset 返回「行数」，乘行高换成像素（1 行 = lh），否则只有 1px 看不出。
        let scroll_dy =
            scroll_offset(anim, first, now, !(m.reduced_motion || !cfg.smooth_caret)) * lh;
        f.push(DrawCmd::PushClip {
            x: x - 14.,
            y: top - 4.,
            w: bw + 28.,
            h: (bottom - top + 4.).max(1.),
        });
        // 【审计 V12】当前输入层不再依赖「词提可见」：词提两类开关都关掉时也必须
        // 一眼看见自己打到哪。三个通道——强调色混合字形色、强调色底块、实心下划线。
        let (cur_color, cur_bg, _) = current_input_style(t);
        for (li, (offset, line_text, widths)) in lines.iter().enumerate().skip(first).take(rows) {
            let yy = stage_y
                + (li - first) as f32 * lh
                + scroll_dy
                + if li >= active { gap } else { 0. };
            let xx = x;
            // Single practice shows exactly one group (possibly wrapped onto two
            // rows). Every row of that group is current: do not grey the second row
            // like a neighbour line.
            let dim = 1.;
            let mut dx = 0.;
            if cfg.shake_on_wrong && !m.reduced_motion && now - anim.wrong_at < 0.3 {
                let z = (now - anim.wrong_at).max(0.) as f32;
                dx = (z * 72.).sin() * 6. * (1. - z / 0.3).max(0.);
            }
            // Continuous state fills share edges: no pending character tiles or grid.
            // 【热路径 2026-11-14】chars().count() 提出循环外，避免每轮 O(n) 重求值。
            let n = line_text.chars().count();
            let mut j = 0;
            while j < n {
                let state = sess.comparer.states.get(offset + j);
                let end = (j + 1..n)
                    .find(|k| sess.comparer.states.get(offset + k) != state)
                    .unwrap_or(n);
                let bg = match state {
                    Some(CharState::Correct) => Some(p.bg_done),
                    Some(CharState::Wrong) => Some(p.bg_wrong),
                    _ => cfg
                        .color_overrides
                        .contains_key("对打未打底色")
                        .then_some(p.bg_pending),
                };
                if let Some(bg) = bg {
                    let a = widths.get(j).copied().unwrap_or(0.);
                    let b = widths.get(end).copied().unwrap_or(a + fs);
                    rect(&mut f, xx + dx + a, yy, b - a, fs + 6., bg);
                }
                j = end;
            }
            if li == active {
                let ci = pos
                    .saturating_sub(*offset)
                    .min(widths.len().saturating_sub(1));
                let a = widths.get(ci).copied().unwrap_or(0.);
                let b = widths.get(ci + 1).copied().unwrap_or(a + fs);
                round(
                    &mut f,
                    xx + a - 1.,
                    yy + 2.,
                    (b - a).max(4.) + 2.,
                    fs + 4.,
                    4.,
                    cur_bg,
                    Some(alpha(p.accent, 0.30)),
                );
            }
            let mut spans = Vec::new();
            let mut u16pos = 0;
            for (j, ch) in line_text.chars().enumerate() {
                let char_at = offset + j;
                let tint = (word_hints && cfg.hint_color_words)
                    .then(|| {
                        let at = m
                            .words
                            .partition_point(|word| word.start + word.len <= char_at);
                        let word = m.words.get(at).filter(|word| word.start <= char_at)?;
                        hint_word_tint_for_rank(
                            &p,
                            &word.text,
                            word.code.as_deref()?,
                            word.rank,
                            m.max_code_length,
                            cfg.hint_rank_colors,
                        )
                    })
                    .flatten();
                // 【审计 V12】【审计 V17】状态 → 角色 → 主题键色，与画在哪一栏无关。
                let role = classify_glyph(
                    sess.comparer.states.get(char_at).copied(),
                    char_at == pos,
                    tint.is_some(),
                );
                let c = glyph_color(&p, role, tint, cur_color);
                spans.push((u16pos, ch.len_utf16() as u32, alpha(c, dim)));
                u16pos += ch.len_utf16() as u32;
                if sess.comparer.states.get(offset + j) == Some(&CharState::Wrong) {
                    let a = widths.get(j).copied().unwrap_or(0.);
                    let b = widths.get(j + 1).copied().unwrap_or(a + fs);
                    let mut px = xx + a + 3.;
                    while px < xx + b - 2. {
                        rect(&mut f, px, yy + fs + 3., 2., 1., p.red);
                        px += 4.;
                    }
                }
            }
            styled_line(
                &mut f,
                xx + dx,
                yy,
                bw,
                fs,
                line_text,
                spans,
                p.text,
                spacing,
                widths,
            );
            if li == active {
                let ci = pos
                    .saturating_sub(*offset)
                    .min(widths.len().saturating_sub(1));
                let target = (xx + widths[ci], yy);
                // 【动效 2026-11-15】缓动跟随（130ms 缓出），换行也不再闪现。
                let caret_on = !(m.reduced_motion || !cfg.smooth_caret);
                let caret = smooth_caret(anim, target, now, caret_on, lh);
                rect(
                    &mut f,
                    caret.0 - 4.,
                    caret.1 + 5.,
                    2.,
                    fs + 2.,
                    // 【审计 V17】光标不是打错反馈：单字模式曾借用打错红当光标，
                    // 与打错字撞色；光标一律取光标色（强调色），与「对/错」语义解耦。
                    p.accent,
                );
                draw_preedit(&mut f, cfg, &sess.preedit, target, xx, xx + bw, p.accent);
            }
            // 【八轮重做】词提独立于行距（与文章卡同口径）：不再有空间门槛/缩号/
            // 退让，括线、编码行、选重数字一律在行内原位按设置字号原样绘制，
            // 行距紧时与下一行重叠也照画。
            if word_hints {
                for z in &m.words {
                    if z.start + z.len <= pos
                        || z.start < *offset
                        || z.start >= offset + line_text.chars().count()
                    {
                        continue;
                    }
                    let local = z.start - offset;
                    let wx = xx + widths.get(local).copied().unwrap_or(0.);
                    let ww = widths
                        .get((local + z.len).min(widths.len() - 1))
                        .copied()
                        .unwrap_or(bw)
                        - widths.get(local).copied().unwrap_or(0.);
                    // Punctuation keeps its neutral article colour and, like F10,
                    // gets no code row of its own even if the dictionary lists a
                    // literal key for it.
                    if !z.text.chars().any(char::is_alphanumeric) {
                        continue;
                    }
                    if let Some(code) = &z.code {
                        let tint = cfg
                            .hint_color_words
                            .then(|| {
                                hint_word_tint_for_rank(
                                    &p,
                                    &z.text,
                                    code,
                                    z.rank,
                                    m.max_code_length,
                                    cfg.hint_rank_colors,
                                )
                            })
                            .flatten();
                        if crate::analysis::is_multi_word(&z.text) {
                            let underline = tint
                                .map(|color| alpha(color, 0.62))
                                .unwrap_or(p.hint_underline);
                            word_bracket(&mut f, wx, yy, ww, fs, underline);
                        }
                        let hint_size = cfg.hint_font_size;
                        let shown_code = if cfg.hint_show_code {
                            code.as_str()
                        } else {
                            ""
                        };
                        let rank = if cfg.hint_show_rank && z.rank > 1 {
                            z.rank.to_string()
                        } else {
                            String::new()
                        };
                        // 【选重数字角标 2026-11-11】hint_rank_corner：数字从
                        // 编码行移到词右上角（上标式角标），编码行只居中显示
                        // 编码本身，居中计算不再计入数字宽度。
                        let rank_in_code_row = rank.is_empty() || !cfg.hint_rank_corner;
                        let hint_advance = hint_size * 0.6;
                        let code_width = shown_code.chars().count() as f32 * hint_advance;
                        let rank_size = hint_size * 0.78;
                        let rank_width = if rank_in_code_row {
                            rank.chars().count() as f32 * rank_size * 0.6
                        } else {
                            0.
                        };
                        let code_rank_gap = if code_width > 0. && rank_width > 0. {
                            2.
                        } else {
                            0.
                        };
                        let hx = wx + (ww - code_width - code_rank_gap - rank_width).max(0.) / 2.;
                        let code_color = alpha(tint.unwrap_or(p.hint_code), dim);
                        if !shown_code.is_empty() {
                            // 【词提下行 r3 2026-11-14】括线回校准紧贴位（fs+offset），
                            // 编码行让到括线之下 +2——编码永远在杯外。
                            mono(
                                &mut f,
                                hx,
                                hint_code_y(cfg, yy, fs),
                                hint_size,
                                shown_code,
                                code_color,
                            );
                        }
                        if !rank.is_empty() {
                            let rank_color = alpha(
                                if cfg.hint_rank_colors {
                                    tint.unwrap_or(p.hint_badge)
                                } else {
                                    p.hint_badge
                                },
                                dim,
                            );
                            if cfg.hint_rank_corner {
                                // 【词提下行 2026-11-14】角标改词右下角（下标式）：
                                // 字号随正文缩放，右缘对齐词宽，下缘略探出词底。
                                let corner_size = (fs * 0.52).max(9.);
                                let corner_w = rank.chars().count() as f32 * corner_size * 0.6;
                                rank_digit(
                                    &mut f,
                                    cfg,
                                    wx + ww - corner_w - 1.,
                                    yy + fs - corner_size * 0.62,
                                    corner_size,
                                    &rank,
                                    rank_color,
                                );
                            } else {
                                rank_digit(
                                    &mut f,
                                    cfg,
                                    hx + code_width + code_rank_gap,
                                    hint_code_y(cfg, yy, fs) + (hint_size - rank_size),
                                    rank_size,
                                    &rank,
                                    rank_color,
                                );
                            }
                        }
                    }
                }
            }
        }
        f.push(DrawCmd::PopClip);
    }
    // 极简模式没有底栏，但成绩面板/理论分析停靠面板仍是浮层，照样画。
    if !(m.analysis_dock && !cfg.minimal && h < 520.) {
        crate::history::draw(&mut f, hs, sess, cfg, m, t, w, h);
    }
    if m.analysis_dock && !m.skip_analysis_dock && m.modal == Modal::None {
        crate::modals::draw_analysis_dock_for(&mut f, hs, m, sess, t, cfg, w, h);
    }
    // 极简字贴模式：本段完成时底部浮条（仍保留下一段/重打入口）。
    if minimal && !split && sess.phase == crate::session::Phase::Done {
        draw_minimal_done_bar(&mut f, hs, sess, t, w, h);
    }
    f
}

/// 极简模式 + 字贴：完成态底部浮条。对照模式的完成摘要画在输入卡里。
fn draw_minimal_done_bar(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    sess: &Session,
    t: &Theme,
    w: f32,
    h: f32,
) {
    let p = tokens(t);
    let record = sess
        .results
        .get(sess.current_index())
        .and_then(Option::as_ref);
    let summary = record
        .map(|r| format!("本段完成  {:.1} 字/分 · 键准 {:.1}%", r.speed, r.accuracy))
        .unwrap_or_else(|| "本段完成".into());
    let has_next = sess.current_index() + 1 < sess.total_segments();
    let bar_w = (ui_text_width(&summary, 14.) + 244.).min((w - 40.).max(260.));
    let x = (w - bar_w) / 2.;
    let y = (h - 60.).max(8.);
    round(
        f,
        x,
        y,
        bar_w,
        44.,
        12.,
        p.surface,
        Some(alpha(p.accent, 0.35)),
    );
    text(
        f,
        x + 16.,
        y + 14.,
        14.,
        elide_width(&summary, (bar_w - 220.).max(80.), 14.),
        p.text2,
    );
    let mut bx = x + bar_w - 192.;
    if has_next {
        button(f, hs, bx, y + 6., 84., "下一段", Action::Next, t, true);
        bx += 90.;
    }
    button(
        f,
        hs,
        bx,
        y + 6.,
        84.,
        "重打",
        Action::Restart,
        t,
        !has_next,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modals::HintWord;
    use gale_core::segment::{ParsedGroupText, Segment};
    use gale_core::theme::contrast_ratio;

    #[test]
    fn hundred_pixel_body_text_is_not_reduced_by_window_width() {
        let cfg = Config {
            font_size: 100.,
            ..Default::default()
        };
        let session = Session::new(100);
        for width in [640., 800., 1280.] {
            let (_, size, line_height) = body_metrics(&session, &cfg, width);
            assert_eq!(size, 100.);
            assert!(line_height >= 140.);
        }
    }

    fn fixture() -> (Session, Model, Config, Vec<(usize, String, Vec<f32>)>) {
        let mut sess = Session::new(100);
        sess.load_group(ParsedGroupText {
            title: "布局验收".into(),
            difficulty: None,
            segments: vec![Segment {
                mark: "1".into(),
                text: "甲乙丙丁戊己庚辛壬癸".into(),
            }],
        });
        let model = Model {
            hints: true,
            words: vec![HintWord {
                start: 0,
                len: 2,
                text: "甲乙".into(),
                code: Some("abcd".into()),
                rank: 2,
                tail: ";".into(),
            }],
            max_code_length: 4,
            ..Default::default()
        };
        let cfg = Config {
            font_size: 54.,
            ..Default::default()
        };
        let lines = vec![
            (
                0,
                "甲乙丙丁戊".into(),
                (0..=5).map(|i| i as f32 * 54.).collect(),
            ),
            (
                5,
                "己庚辛壬癸".into(),
                (0..=5).map(|i| i as f32 * 54.).collect(),
            ),
        ];
        (sess, model, cfg, lines)
    }

    #[test]
    fn header_renders_previous_metrics_after_loading_and_resets_on_the_first_key() {
        let (mut sess, model, cfg, _) = fixture();
        for i in 0..10 {
            sess.on_key(0x41 + i, 100 + i as u64 * 100);
        }
        sess.feed_input(
            gale_core::compare::InputEvent::Commit {
                chars: "错乙丙丁戊己庚辛壬癸".into(),
            },
            2100,
        );
        sess.finalize_current_at(2100, 1).unwrap();
        let draw_header = |sess: &Session, now| {
            let text = &sess.current_segment().text;
            let lines = vec![(
                0,
                text.clone(),
                (0..=text.chars().count()).map(|i| i as f32 * 36.).collect(),
            )];
            build(
                sess,
                &cfg,
                &theme("墨玉"),
                &model,
                1280.,
                800.,
                now,
                0.,
                &mut Animation::default(),
                &mut Vec::new(),
                &lines,
            )
            .cmds
            .into_iter()
            .filter_map(|cmd| match cmd {
                DrawCmd::TextRun { y, text, .. } if y == 117. || y == 138. => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>()
        };
        let previous = draw_header(&sess, 3100);
        assert_eq!(
            previous,
            ["300.0", "5.00", "1.00", "100.0%", "1", "00:02.0", "—"]
        );
        sess.load_book("新文章", "新文章甲乙丙丁戊己庚");
        assert_eq!(draw_header(&sess, 7100), previous);
        assert_eq!(
            sess.comparer.pos, 0,
            "the new text is ready despite the retained header"
        );
        sess.on_key(0x41, 8100);
        assert_eq!(
            draw_header(&sess, 9100),
            ["0.0", "1.00", "0.00", "100.0%", "0", "00:01.0", "—"]
        );
    }

    #[test]
    fn adjacent_words_have_separate_brackets_below_the_text_at_all_sizes() {
        let article = "好处坏处好的经常不要";
        let mut sess = Session::new(100);
        sess.load_book("相邻词组", article);
        let model = Model {
            hints: true,
            words: ["好处", "坏处", "好的", "经常", "不要"]
                .into_iter()
                .enumerate()
                .map(|(i, text)| HintWord {
                    start: i * 2,
                    len: 2,
                    text: text.into(),
                    code: Some("abcd".into()),
                    rank: 1,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let theme = theme("墨玉");
        let underline = tokens(&theme).hint_underline;
        // 【八轮定稿】词提不再参与行距计算，也没有退让/缩号函数（hint_strip_fits
        // 已删除）：括线/编码/选重数字在行内原位原尺寸绘制，行距紧时重叠也照画。
        // 这里只逐尺寸验证括线几何不被行距或字号改变。
        for size in [36., 100.] {
            let cfg = Config {
                font_size: size,
                hint_color_words: false,
                ..Default::default()
            };
            let lines = vec![(
                0,
                article.into(),
                (0..=10).map(|i| i as f32 * size).collect(),
            )];
            let frame = build(
                &sess,
                &cfg,
                &theme,
                &model,
                1280.,
                800.,
                0,
                0.,
                &mut Animation::default(),
                &mut Vec::new(),
                &lines,
            );
            let article_y = frame
                .cmds
                .iter()
                .find_map(|cmd| match cmd {
                    DrawCmd::StyledLine { y, .. } => Some(*y),
                    _ => None,
                })
                .unwrap();
            let rules: Vec<_> = frame
                .cmds
                .iter()
                .filter_map(|cmd| match cmd {
                    DrawCmd::Rect {
                        x, y, w, h, color, ..
                    } if *h <= 2.
                        && *color == underline
                        && *y == article_y + size + word_bracket_offset(size) =>
                    {
                        Some((*x, *w))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(rules.len(), 5);
            for pair in rules.windows(2) {
                assert!(pair[1].0 - (pair[0].0 + pair[0].1) >= 5. - 0.01);
            }
            let rise = (size * 0.10).clamp(3., 8.);
            let ends: Vec<_> = frame
                .cmds
                .iter()
                .filter_map(|cmd| match cmd {
                    DrawCmd::Rect {
                        x, y, w, h, color, ..
                    } if *w <= 2.
                        && *h == rise
                        && *color == underline
                        && *y == article_y + size + word_bracket_offset(size) - rise =>
                    {
                        Some((*x, *y, *w))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(ends.len(), 10, "each phrase needs two upturned ends");
            for (rule, ends) in rules.iter().zip(ends.chunks_exact(2)) {
                assert_eq!(ends[0].0, rule.0);
                assert_eq!(ends[1].0 + ends[1].2, rule.0 + rule.1);
                assert!(
                    ends[0].1 > article_y + size,
                    "brackets must clear the glyphs"
                );
            }
            assert!(frame.cmds.iter().any(|cmd| matches!(cmd,
                DrawCmd::StyledLine { text, .. } if text == article)));
            assert!(line_height_for_anchor(&cfg, size, size * 1.4, false) > size + 12.);
        }
        // 【八轮重做】词提独立于行距：不再有退让门槛——任何行距下括线与编码行
        // 都照画（行距紧时与下一行重叠是用户自己的取舍）。10 号字极限行高
        // （gap 2.0 → lh 30 / gap 1.0 → lh 20）括线与编码行都必须在场。
        {
            let size = 10.;
            let cfg = Config {
                font_size: size,
                hint_color_words: false,
                line_gap: 2.0,
                ..Default::default()
            };
            let lines = vec![(
                0,
                article.into(),
                (0..=10).map(|i| i as f32 * size).collect(),
            )];
            let frame = build(
                &sess,
                &cfg,
                &theme,
                &model,
                1280.,
                800.,
                0,
                0.,
                &mut Animation::default(),
                &mut Vec::new(),
                &lines,
            );
            // w==0 的退化矩形（分隔线残留）不算括线；只统计有宽度的下划线规则。
            let brackets = frame.cmds.iter().filter(|cmd| {
                matches!(cmd,
                DrawCmd::Rect { w, h, color, .. } if *w > 0. && *h <= 2. && *color == underline)
            });
            assert_eq!(brackets.count(), 5, "brackets always draw now");
            assert!(
                frame.cmds.iter().any(|cmd| matches!(cmd,
                DrawCmd::TextRun { text, .. } if text == "abcd")),
                "code row always draws now"
            );
        }
        {
            let size = 10.;
            let cfg = Config {
                font_size: size,
                hint_color_words: false,
                line_gap: 1.0,
                ..Default::default()
            };
            let lines = vec![(
                0,
                article.into(),
                (0..=10).map(|i| i as f32 * size).collect(),
            )];
            let frame = build(
                &sess,
                &cfg,
                &theme,
                &model,
                1280.,
                800.,
                0,
                0.,
                &mut Animation::default(),
                &mut Vec::new(),
                &lines,
            );
            let brackets = frame.cmds.iter().filter(|cmd| {
                matches!(cmd,
                DrawCmd::Rect { w, h, color, .. } if *w > 0. && *h <= 2. && *color == underline)
            });
            assert_eq!(
                brackets.count(),
                5,
                "brackets never yield, tight spacing overlaps by design"
            );
            assert!(frame.cmds.iter().any(|cmd| matches!(cmd,
                DrawCmd::TextRun { text, .. } if text == "abcd")));
        }
    }

    #[test]
    fn rank_digit_stays_in_code_row_at_full_size_regardless_of_line_gap() {
        let article = "好处";
        let mut sess = Session::new(100);
        sess.load_book("选重", article);
        let model = Model {
            hints: true,
            words: vec![HintWord {
                start: 0,
                len: 2,
                text: "好处".into(),
                code: Some("abcd".into()),
                rank: 3,
                ..Default::default()
            }],
            ..Default::default()
        };
        let theme = theme("墨玉");
        // 【词提下行 r3 2026-11-14】r2 括线兜底整带导致上翘端包住编码；定谳分层：
        // 括线回四轮校准紧贴字脚（fs+offset），编码行让到括线之下 +2。编码行
        // 与选重数字仍按设置字号原样画，不缩号；角标开关是唯一把数字挪到
        // 词角（右下角下标）的路径；文字上方只留键入编码（preedit）。
        for gap in [0.7, 0.5, 0.4, 0.1] {
            let cfg = Config {
                font_size: 36.,
                hint_color_words: false,
                line_gap: gap,
                ..Default::default()
            };
            let lines = vec![(0, article.into(), (0..=2).map(|i| i as f32 * 36.).collect())];
            let frame = build(
                &sess,
                &cfg,
                &theme,
                &model,
                1280.,
                800.,
                0,
                0.,
                &mut Animation::default(),
                &mut Vec::new(),
                &lines,
            );
            let article_y = frame
                .cmds
                .iter()
                .find_map(|cmd| match cmd {
                    DrawCmd::StyledLine { y, .. } => Some(*y),
                    _ => None,
                })
                .unwrap();
            assert!(
                frame.cmds.iter().any(|cmd| matches!(cmd,
                    DrawCmd::TextRun { text, y, h, .. }
                        if text == "3" && (*y - (article_y + 47.2)).abs() < 0.5 && (*h - 10.92).abs() < 0.01)),
                "gap {gap}: rank digit must sit below the word at full size (14*0.78)"
            );
            assert!(
                frame.cmds.iter().any(|cmd| matches!(cmd,
                    DrawCmd::TextRun { text, y, .. } if text == "abcd" && (*y - (article_y + 44.12)).abs() < 0.5)),
                "gap {gap}: code row must draw below the word at full size"
            );
        }
        // 角标开关是唯一把数字挪上词右上角的路径。
        {
            let cfg = Config {
                font_size: 36.,
                hint_color_words: false,
                line_gap: 0.4,
                hint_rank_corner: true,
                ..Default::default()
            };
            let lines = vec![(0, article.into(), (0..=2).map(|i| i as f32 * 36.).collect())];
            let frame = build(
                &sess,
                &cfg,
                &theme,
                &model,
                1280.,
                800.,
                0,
                0.,
                &mut Animation::default(),
                &mut Vec::new(),
                &lines,
            );
            let article_y = frame
                .cmds
                .iter()
                .find_map(|cmd| match cmd {
                    DrawCmd::StyledLine { y, .. } => Some(*y),
                    _ => None,
                })
                .unwrap();
            // 角标：corner_size = max(36*0.52, 9) = 18.72，y = 词底 - 18.72*0.62
            // （右下角下标式，随词提下行）。
            let corner = frame.cmds.iter().any(|cmd| {
                matches!(cmd,
                DrawCmd::TextRun { text, y, .. }
                    if text == "3" && (*y - (article_y + 24.39)).abs() < 0.5)
            });
            assert!(
                corner,
                "hint_rank_corner is the only path to the subscript corner"
            );
            let in_row = frame.cmds.iter().any(|cmd| {
                matches!(cmd,
                DrawCmd::TextRun { text, y, .. }
                    if text == "3" && (*y - (article_y + 47.2)).abs() < 0.5)
            });
            assert!(!in_row, "corner on → rank must leave the code row position");
            assert!(
                frame.cmds.iter().any(|cmd| matches!(cmd,
                    DrawCmd::TextRun { text, y, .. } if text == "abcd" && (*y - (article_y + 44.12)).abs() < 0.5)),
                "code row still draws below the word under the corner toggle"
            );
        }
    }

    #[test]
    fn article_word_hints_preserve_typing_states_and_leave_single_characters_neutral() {
        let (mut sess, model, cfg, lines) = fixture();
        sess.comparer.states[0] = CharState::Correct;
        sess.comparer.states[1] = CharState::Wrong;
        let theme = theme("墨玉");
        let p = tokens(&theme);
        // 【审计 V12】当前输入是压在状态色之上的独立一层：pos 处的字归「当前输入」，
        // 状态色（打对/打错）仍在同一行的其它字上各自成立。
        let current = current_input_style(&theme).0;
        let frame = build(
            &sess,
            &cfg,
            &theme,
            &model,
            800.,
            600.,
            0,
            0.,
            &mut Animation::default(),
            &mut Vec::new(),
            &lines,
        );
        let spans = frame
            .cmds
            .iter()
            .find_map(|cmd| match cmd {
                DrawCmd::StyledLine { spans, .. } => Some(spans),
                _ => None,
            })
            .unwrap();
        assert_eq!(spans[0].2, current);
        assert_eq!(spans[1].2, p.red);
        assert_eq!(spans[2].2, p.text2);
        assert!(frame.cmds.iter().any(|cmd| matches!(cmd,
            DrawCmd::TextRun { text, .. } if text == "abcd")));

        let (mut sess, mut model, cfg, lines) = fixture();
        // 【审计 V12】同上：当前位挪开，避免抢走词提首字的语义色。
        sess.comparer.pos = 4;
        let draw = |sess: &Session, model: &Model| {
            build(
                sess,
                &cfg,
                &theme,
                model,
                800.,
                600.,
                0,
                0.,
                &mut Animation::default(),
                &mut Vec::new(),
                &lines,
            )
        };
        let frame = draw(&sess, &model);
        let spans = frame
            .cmds
            .iter()
            .find_map(|cmd| match cmd {
                DrawCmd::StyledLine { spans, .. } => Some(spans),
                _ => None,
            })
            .unwrap();
        assert_eq!(spans[0].2, hint_word_tint(&p, "甲乙", "abcd", 4).unwrap());
        assert_eq!(spans[2].2, p.text2);
        model.hints = false;
        // 词提关闭后当前位仍单列一层（【审计 V12】），把光标移到段末，让本断言
        // 只覆盖「所有字都回到未打文字色」。
        sess.comparer.pos = 10;
        let frame = draw(&sess, &model);
        assert!(!frame.cmds.iter().any(|cmd| matches!(cmd,
            DrawCmd::TextRun { text, .. } if text == "abcd")));
        assert!(frame
            .cmds
            .iter()
            .filter_map(|cmd| match cmd {
                DrawCmd::StyledLine { spans, .. } => Some(spans),
                _ => None,
            })
            .flatten()
            .all(|span| span.2 == p.text2));
    }

    #[test]
    fn punctuation_keeps_neutral_color_and_never_gets_a_code_row() {
        let mut sess = Session::new(100);
        sess.load_book("标点", "甲，乙");
        let model = Model {
            hints: true,
            words: vec![
                HintWord {
                    start: 0,
                    len: 1,
                    text: "甲".into(),
                    code: Some("a".into()),
                    rank: 1,
                    tail: String::new(),
                },
                HintWord {
                    start: 1,
                    len: 1,
                    text: "，".into(),
                    code: Some(",".into()),
                    rank: 1,
                    tail: String::new(),
                },
                HintWord {
                    start: 2,
                    len: 1,
                    text: "乙".into(),
                    code: Some("b".into()),
                    rank: 1,
                    tail: String::new(),
                },
            ],
            max_code_length: 4,
            ..Default::default()
        };
        let cfg = Config {
            font_size: 36.,
            ..Default::default()
        };
        let theme = theme("墨玉");
        let p = tokens(&theme);
        let lines = vec![(0, "甲，乙".into(), vec![0., 36., 72., 108.])];
        let frame = build(
            &sess,
            &cfg,
            &theme,
            &model,
            800.,
            600.,
            0,
            0.,
            &mut Animation::default(),
            &mut Vec::new(),
            &lines,
        );
        assert!(!frame.cmds.iter().any(|cmd| matches!(cmd,
            DrawCmd::TextRun { text, .. } if text == ","
        )));
        let spans = frame
            .cmds
            .iter()
            .find_map(|cmd| match cmd {
                DrawCmd::StyledLine { spans, .. } => Some(spans),
                _ => None,
            })
            .unwrap();
        assert_eq!(spans[1].2, p.text2);
        assert!(frame.cmds.iter().any(|cmd| matches!(cmd,
            DrawCmd::TextRun { text, .. } if text == "a"
        )));
    }

    #[test]
    fn rank_color_mode_ignores_code_class_and_preserves_neutral_items() {
        for name in ["墨玉", "宣纸"] {
            let mut t = theme(name);
            let p = tokens(&t);
            for rank in 1..=10 {
                let expected = Some(p.hint_rank_colors[rank - 1]);
                for (word, code) in [("一简词", "a"), ("二三", "abc"), ("四字词组", "abcd")]
                {
                    assert_eq!(
                        hint_word_tint_for_rank(&p, word, code, rank, 4, true),
                        expected
                    );
                    assert_eq!(
                        hint_word_tint_for_rank(&p, word, code, rank, 4, false),
                        hint_word_tint(&p, word, code, 4)
                    );
                }
                for (word, code) in [("单", "a"), ("缺词", "")] {
                    assert_eq!(hint_word_tint_for_rank(&p, word, code, rank, 4, true), None);
                }
                assert_eq!(
                    setting_color(&t, HINT_RANK_COLOR_KEYS[rank - 1]),
                    expected.unwrap()
                );
            }
            for rank in [0, 11, usize::MAX] {
                assert_eq!(
                    hint_word_tint_for_rank(&p, "词组", "ab", rank, 4, true),
                    None
                );
            }
            for (i, color) in p.hint_rank_colors.iter().enumerate() {
                assert!(!p.hint_rank_colors[..i].contains(color));
            }
            t.set("词提3选色", gale_core::theme::Color::rgb(18, 58, 239));
            let custom = tokens(&t);
            assert_eq!(custom.hint_rank_colors[2], hex("123aef"));
            assert_eq!(setting_color(&t, "词提3选色"), hex("123aef"));
            assert_eq!(custom.word_colors, p.word_colors);
            assert_eq!(custom.hint_rank_colors[1], p.hint_rank_colors[1]);
        }
    }

    #[test]
    fn toolbar_stays_inside_small_windows_without_overlap() {
        let (sess, mut model, cfg, _) = fixture();
        for modal in [Modal::None, Modal::More] {
            model.modal = modal;
            for width in [640., 800., 1000., 1280.] {
                let mut hits = Vec::new();
                draw_chrome(
                    &mut Frame::new(width, 480.),
                    &mut hits,
                    &sess,
                    &cfg,
                    &model,
                    &theme("墨玉"),
                    width,
                    480.,
                    0,
                );
                let buttons: Vec<_> = hits.iter().filter(|h| h.y == 60.).collect();
                assert!(buttons.iter().any(|h| h.action == Action::LoadClipboard));
                assert!(buttons
                    .iter()
                    .any(|h| h.action == Action::Open(Modal::Online)));
                assert!(buttons.iter().any(|h| h.action == Action::ReadGroup));
                let analysis = buttons
                    .iter()
                    .find(|h| h.action == Action::ToggleAnalysis)
                    .unwrap();
                let settings = buttons
                    .iter()
                    .find(|h| h.action == Action::Open(Modal::Preferences))
                    .unwrap();
                assert!((analysis.x + analysis.w + 8. - settings.x).abs() < 0.01);
                for (i, a) in buttons.iter().enumerate() {
                    assert!(a.x >= 0. && a.x + a.w <= width);
                    for b in buttons.iter().skip(i + 1) {
                        assert!(a.x + a.w <= b.x || b.x + b.w <= a.x);
                    }
                }
            }
        }
    }

    #[test]
    fn empty_startup_and_analysis_show_guidance_without_sample_article() {
        let sess = Session::new(100);
        let cfg = Config::default();
        for (w, h) in [(640., 420.), (800., 600.)] {
            for analysis_dock in [false, true] {
                let model = Model {
                    analysis_dock,
                    ..Default::default()
                };
                let mut hits = Vec::new();
                let frame = build(
                    &sess,
                    &cfg,
                    &theme("墨玉"),
                    &model,
                    w,
                    h,
                    0,
                    0.,
                    &mut Animation::default(),
                    &mut hits,
                    &[],
                );
                assert!(hits.iter().any(|h| h.action == Action::LoadClipboard));
                assert!(!frame
                    .cmds
                    .iter()
                    .any(|cmd| matches!(cmd, DrawCmd::StyledLine { .. })));
                assert!(frame.cmds.iter().any(|cmd| matches!(cmd,
                    DrawCmd::TextRun { text, .. } if text == "载入文章后开始跟打")));
                assert_eq!(
                    frame.cmds.iter().any(|cmd| matches!(cmd,
                        DrawCmd::TextRun { text, .. } if text == "请先载入文章")),
                    analysis_dock
                );
            }
        }
    }

    #[test]
    fn single_current_row_stays_visible_at_large_font_in_small_window() {
        let (mut sess, model, cfg, lines) = fixture();
        sess.source = SessionSource::Single;
        sess.comparer.pos = 8;
        let mut anim = Animation::default();
        let frame = build(
            &sess,
            &cfg,
            &theme("墨玉"),
            &model,
            640.,
            480.,
            0,
            0.,
            &mut anim,
            &mut Vec::new(),
            &lines,
        );
        let (_, caret_y) = anim.caret.expect("current row has a caret");
        assert!(caret_y >= 236. && caret_y + cfg.font_size < 402.);
        assert!(frame.cmds.iter().any(|cmd| matches!(cmd,
            DrawCmd::StyledLine { text, .. } if text.contains('壬'))));
    }

    #[test]
    fn hundred_pixel_article_scrolls_to_current_row_in_small_window() {
        let (mut sess, model, mut cfg, mut lines) = fixture();
        cfg.font_size = 100.;
        sess.comparer.pos = 8;
        for (_, text, positions) in &mut lines {
            *positions = (0..=text.chars().count())
                .map(|i| i as f32 * 100.)
                .collect();
        }
        for (w, h) in [(640., 420.), (800., 600.), (1280., 800.)] {
            let mut anim = Animation::default();
            let frame = build(
                &sess,
                &cfg,
                &theme("墨玉"),
                &model,
                w,
                h,
                0,
                0.,
                &mut anim,
                &mut Vec::new(),
                &lines,
            );
            let (_, caret_y) = anim.caret.expect("current row must remain visible");
            let (top, bottom) = body_region(&cfg, &model, w, h, false);
            assert!(caret_y >= top && caret_y + 107. <= bottom);
            assert!(frame.cmds.iter().any(|cmd| matches!(cmd,
                DrawCmd::StyledLine { text, size, .. } if text.contains('壬') && *size == 100.)));
        }
    }

    #[test]
    fn compact_analysis_leaves_room_for_full_size_article_and_code_annotations() {
        let (sess, mut model, mut cfg, mut lines) = fixture();
        cfg.font_size = 100.;
        model.analysis_dock = true;
        for (_, text, positions) in &mut lines {
            *positions = (0..=text.chars().count())
                .map(|i| i as f32 * 100.)
                .collect();
        }
        for (w, h) in [(640., 420.), (800., 600.)] {
            let mut anim = Animation::default();
            let frame = build(
                &sess,
                &cfg,
                &theme("墨玉"),
                &model,
                w,
                h,
                0,
                0.,
                &mut anim,
                &mut Vec::new(),
                &lines,
            );
            let bottom = crate::modals::analysis_dock_rect(w, h).1 - 8.;
            // 【用户 2026-11-15】顶部留白改按字号 0.5 倍固定后，这里只保证当前行
            // 与整行编码都在分析条之上，不再引用旧的固定 107px 预算。
            assert!(anim.caret.unwrap().1 < bottom);
            // 【用户 2026-11-15】顶部留白改成字号的 0.5 倍后，640×420 + 100px 字
            // 这种极端档位放不下「整行编码 + 分析条」，但当前行与编码行起点仍必须在条上方，
            // 不能被分析条整块盖掉。
            assert!(
                frame.cmds.iter().any(|cmd| matches!(cmd,
                    DrawCmd::StyledLine { size, y, .. } if *size == 100. && y + size <= bottom
                )),
                "the current article row must sit above the dock"
            );
            // 640×420 + 100px 字 + 分析条：当前行必须整行在条上方；末行下方那条
            // 编码行属于「再缩就放不下」的极端档，允许被分析条压住（可见行必在裁剪内）。
            assert!(
                frame.cmds.iter().any(|cmd| matches!(cmd,
                    DrawCmd::TextRun { text, .. } if text == "abcd"
                )),
                "code annotations must still be emitted"
            );
        }
    }

    #[test]
    fn word_semantics_follow_six_classes_and_single_characters_stay_neutral() {
        let p = tokens(&theme("宣纸"));
        assert_eq!(hint_word_tint(&p, "单", "a", 4), None);
        // 含标点多字词现在正常分档（原版口径，2026-11-12 对齐）
        assert_eq!(hint_word_tint(&p, "……", "a", 4), Some(p.word_colors[5]));
        assert_eq!(hint_word_tint(&p, "词，词", "a", 4), Some(p.word_colors[5]));
        assert_eq!(hint_word_tint(&p, "词组", "", 4), None);
        assert_eq!(hint_word_tint(&p, "一码词", "a", 4), Some(p.word_colors[5]));
        assert_eq!(hint_word_tint(&p, "简词", "ab", 4), Some(p.word_colors[0]));
        assert_eq!(
            hint_word_tint(&p, "二字", "abcd", 4),
            Some(p.word_colors[2])
        );
        assert_eq!(
            hint_word_tint(&p, "三字词", "abcd", 4),
            Some(p.word_colors[3])
        );
        assert_eq!(
            hint_word_tint(&p, "四字词语", "abcd", 4),
            Some(p.word_colors[4])
        );
        assert_eq!(hint_word_tint(&p, "二字", "abc", 3), Some(p.word_colors[1]));
        assert_eq!(hint_word_tint(&p, "二字", "abc", 4), Some(p.word_colors[1]));
        assert_eq!(
            hint_word_tint(&p, "三字词", "abc", 3),
            Some(p.word_colors[0])
        );
        assert_eq!(
            p.word_colors,
            [
                hex("16805b"),
                hex("4660ae"),
                hex("b45b08"),
                hex("8550b0"),
                hex("216d91"),
                hex("c43e62"),
            ]
        );
        let p = tokens(&theme("墨玉"));
        assert_eq!(
            p.word_colors,
            [
                hex("7bdbac"),
                hex("a5b8ff"),
                hex("ffae65"),
                hex("c69bf2"),
                hex("7ecbfa"),
                hex("ff7f9f"),
            ]
        );
    }

    #[test]
    fn one_code_color_resolves_defaults_and_overrides_without_changing_legacy_colors() {
        for (name, one_default) in [("宣纸", "c43e62"), ("墨玉", "ff7f9f")] {
            let mut t = theme(name);
            t.set("词提简码色", Color::rgb(0x12, 0x34, 0x56));
            t.set("词提四字全码色", Color::rgb(0x65, 0x43, 0x21));
            let legacy = tokens(&t);
            assert_eq!(
                hint_word_tint(&legacy, "一码", "a", 4),
                Some(hex(one_default))
            );
            assert_eq!(
                hint_word_tint(&legacy, "简码", "ab", 4),
                Some(hex("123456"))
            );
            assert_eq!(
                hint_word_tint(&legacy, "长词全码", "abcd", 4),
                Some(hex("654321"))
            );

            t.set("词提一码色", Color::rgb(0x9a, 0x87, 0x65));
            let custom = tokens(&t);
            assert_eq!(setting_color(&t, "词提一码色"), hex("9a8765"));
            assert_eq!(hint_word_tint(&custom, "一码", "a", 4), Some(hex("9a8765")));
            assert_eq!(custom.word_colors[..5], legacy.word_colors[..5]);
        }
    }

    // ═══════════ 视图体系（2026-11-12）：极简模式 / 对照分栏 ═══════════

    #[test]
    fn letter_spacing_rewraps_without_losing_characters_and_draws_per_character() {
        let (sess, model, mut cfg, lines) = fixture();
        let spacing = 6.;
        let wrapped = flow_with_spacing(&lines, spacing, 600.);
        let original: usize = lines.iter().map(|(_, text, _)| text.chars().count()).sum();
        let rewrapped: usize = wrapped
            .iter()
            .map(|(_, text, _)| text.chars().count())
            .sum();
        assert_eq!(original, rewrapped, "spacing must not drop characters");
        assert!(
            wrapped.len() >= lines.len(),
            "spacing should only add lines"
        );
        // 带字距时逐字绘制：不再产生整行 StyledLine，但位置随字距右移。
        cfg.letter_spacing = spacing;
        let mut anim = Animation::default();
        let mut hits = Vec::new();
        let frame = build(
            &sess,
            &cfg,
            &theme("墨玉"),
            &model,
            1280.,
            800.,
            0,
            0.,
            &mut anim,
            &mut hits,
            &lines,
        );
        let styled = frame
            .cmds
            .iter()
            .filter(|c| matches!(c, DrawCmd::StyledLine { .. }))
            .count();
        assert_eq!(styled, 0, "spacing path draws per character");
        let caret = anim.caret.expect("caret");
        let (top, bottom) = body_region(&cfg, &model, 1280., 800., false);
        assert!(caret.1 >= top && caret.1 < bottom);
    }

    #[test]
    fn minimal_mode_keeps_the_ledger_as_an_overlay_without_the_bottom_bar() {
        let (sess, model, mut cfg, lines) = fixture();
        cfg.minimal = true;
        let m = Model {
            ledger_open: true,
            ..Default::default()
        };
        let (top, bottom) = body_region(&cfg, &m, 1280., 800., false);
        assert_eq!(top, minimal_top(&cfg));
        assert!(bottom < 800. - 16., "ledger must shrink the minimal body");
        let mut anim = Animation::default();
        let mut hits = Vec::new();
        let _ = build(
            &sess,
            &cfg,
            &theme("墨玉"),
            &m,
            1280.,
            800.,
            0,
            0.,
            &mut anim,
            &mut hits,
            &lines,
        );
        // 极简模式没有摘要条，但面板表头仍可点收起。
        assert!(hits.iter().any(|h| h.action == Action::ToggleLedger));
        assert!(!hits.iter().any(|h| matches!(h.action, Action::Next)));
    }
    #[test]
    fn minimal_mode_hides_all_chrome_and_starts_the_article_at_the_top_band() {
        let (mut sess, model, mut cfg, lines) = fixture();
        sess.comparer.pos = 2;
        cfg.minimal = true;
        let mut anim = Animation::default();
        let mut hits = Vec::new();
        let frame = build(
            &sess,
            &cfg,
            &theme("墨玉"),
            &model,
            1280.,
            800.,
            0,
            0.,
            &mut anim,
            &mut hits,
            &lines,
        );
        // 极简不再画整套 chrome，但保留四个常驻动作与窗口控制（2026-11-13 用户要求）。
        assert!(
            !hits.iter().any(|h| matches!(
                h.action,
                Action::LoadClipboard | Action::Restart | Action::Next
            )),
            "minimal mode must not keep the full toolbar"
        );
        assert!(hits.iter().any(|h| h.action == Action::ToggleLedger));
        assert!(hits.iter().any(|h| h.action == Action::ToggleAnalysis));
        assert!(hits.iter().any(|h| h.action == Action::Open(Modal::Groups)));
        assert!(hits.iter().any(|h| h.action == Action::Open(Modal::Online)));
        assert!(hits.iter().any(|h| h.action == Action::Exit));
        // 顶部静默带内绘制状态/指标，正文从 minimal_top 附近开始。
        assert!(frame
            .cmds
            .iter()
            .any(|c| matches!(c, DrawCmd::TextRun { y, .. } if *y < minimal_top(&cfg))));
        let line_y = frame
            .cmds
            .iter()
            .find_map(|c| match c {
                DrawCmd::StyledLine { y, .. } => Some(*y),
                _ => None,
            })
            .expect("article line");
        assert!(
            line_y >= minimal_top(&cfg) - 10. && line_y < minimal_top(&cfg) + 40.,
            "article should begin under the minimal band, got {line_y}"
        );
        let (top, bottom) = body_region(&cfg, &model, 1280., 800., false);
        assert_eq!(top, minimal_top(&cfg));
        assert_eq!(bottom, 800. - 16.);
    }

    #[test]
    fn split_mode_draws_two_panes_and_keeps_the_caret_inside_the_input_box() {
        let (mut sess, model, mut cfg, lines) = fixture();
        cfg.display_mode = gale_core::config::DisplayMode::Split;
        sess.feed_input(
            gale_core::compare::InputEvent::Commit {
                chars: "甲乙".into(),
            },
            100,
        );
        let typed: String = sess.comparer.typed.iter().collect();
        let input_lines = vec![(
            0usize,
            typed,
            (0..=2).map(|i| i as f32 * 54.).collect::<Vec<f32>>(),
        )];
        let mut anim = Animation::default();
        let mut hits = Vec::new();
        let frame = build_with_input(
            &sess,
            &cfg,
            &theme("墨玉"),
            &model,
            1280.,
            800.,
            0,
            0.,
            &mut anim,
            &mut hits,
            &lines,
            &input_lines,
        );
        let (top, bottom) = body_region(&cfg, &model, 1280., 800., false);
        let (bw, fs, base_lh) = body_metrics(&sess, &cfg, 1280.);
        let lh = line_height_for_anchor(&cfg, fs, base_lh, true);
        let g = split_geom(&cfg, bw, fs, lh, 1280., top, bottom);
        assert!(!g.collapsed);
        assert!(
            g.article_h > lh,
            "article pane shows at least one full line"
        );
        assert!(g.input_h > line_height_split(fs));
        // 至少两条 StyledLine：文章对照行 + 输入卡里的实打字行。
        let styled = frame
            .cmds
            .iter()
            .filter(|c| matches!(c, DrawCmd::StyledLine { .. }))
            .count();
        assert!(
            styled >= 2,
            "expected article and input lines, got {styled}"
        );
        let (caret_x, caret_y) = anim.caret.expect("split caret");
        assert!(
            caret_y >= g.input_y && caret_y < g.input_y + g.input_h,
            "caret must sit inside the input pane: caret=({caret_x},{caret_y}) pane=({},{}) h={}",
            g.input_y,
            g.input_y + g.input_h,
            g.input_h
        );
        assert!(caret_x >= g.x && caret_x <= g.x + g.pane_w);
        // 分栏把手命中区从 ui 层交给主循环做拖动。
        assert!(anim.split_divider.is_some());
    }

    fn line_height_split(fs: f32) -> f32 {
        (fs * 1.34).max(fs + 8.)
    }

    #[test]
    fn copybook_body_width_follows_window_instead_of_stopping_at_1000px() {
        let (sess, _, cfg, _) = fixture();
        // 1579 宽窗口：正文带应为 1515（左右各 32px 边），不能被旧的 960/1000 上限截住。
        let (bw, _, _) = body_metrics(&sess, &cfg, 1579.);
        assert!(
            (bw - (1579. - 64.)).abs() < 0.01,
            "wide copybook body must follow the window, got {bw}"
        );
        // 窄窗仍只留 64px 边，不再额外设上限。
        let (bw, _, _) = body_metrics(&sess, &cfg, 640.);
        assert!((bw - (640. - 64.)).abs() < 0.01, "narrow width got {bw}");
    }

    #[test]
    fn thin_input_card_centers_its_only_line_inside_the_visible_box() {
        // 最低高度 = 卡头 30 + 字号 48 + 8：可视线盒正好一行，字必须整行落在框内。
        let fs = 48.;
        let line_h = fs * 1.36;
        let clip_top = 100.;
        let tight = clip_top + fs;
        let top = input_line_top(clip_top, tight, line_h, fs);
        assert!(top >= clip_top, "tight: top={top} 不得高于可视线盒");
        assert!(top + fs <= tight + 0.01, "tight: 字底被切了 top={top}");
        // 再高 12px：上下居中（各让 6px），仍然整行可见。
        let loose = clip_top + fs + 12.;
        let top = input_line_top(clip_top, loose, line_h, fs);
        assert!((top - (clip_top + 6.)).abs() < 0.01, "loose: 应上下居中");
        assert!(top + fs <= loose + 0.01);
        // 多行卡：回到 8px 内边距贴顶，不再居中。
        let tall = clip_top + 240.;
        assert!((input_line_top(clip_top, tall, line_h, fs) - (clip_top + 8.)).abs() < 0.01);
    }

    #[test]
    fn caret_span_never_leaves_the_thin_input_box() {
        // 一行高的输入卡：光标竖条（起笔偏移 + 高度）必须正好落在文字盒 fs 之内，
        // 否则底边被卡口切掉（用户：「下面的会挡一点点」）。
        for fs in [14., 24., 36., 48., 100.] {
            let (dy, h) = input_caret_span(fs, true);
            assert!(dy + h <= fs + 0.01, "thin fs={fs}: 竖条 {dy}+{h} 超出文字盒");
            assert!(h >= 2., "thin fs={fs}: 竖条过矮 {h}");
        }
        // 常态留出上下各一点，视觉上与字贴模式一致。
        let (dy, h) = input_caret_span(48., false);
        assert_eq!((dy, h), (3., 52.));
    }

    #[test]
    fn split_mode_collapses_the_article_pane_before_it_crushes_the_input_box() {
        let (sess, model, mut cfg, lines) = fixture();
        cfg.display_mode = gale_core::config::DisplayMode::Split;
        let (bw, fs, base_lh) = body_metrics(&sess, &cfg, 640.);
        let lh = line_height_for_anchor(&cfg, fs, base_lh, false);
        // 【传统模式输入区】最低高度改成「一行字」后，640×420 已经放得下两栏，
        // 真正收起要再矮一档。
        let (top, bottom) = body_region(&cfg, &model, 640., 260., false);
        let g = split_geom(&cfg, bw, fs, lh, 640., top, bottom);
        assert!(g.collapsed, "h=260 span={} 应收起", bottom - top);
        assert_eq!(g.article_h, 0.);
        assert!(g.input_h >= (bottom - top) - 0.01);
        let (top420, bottom420) = body_region(&cfg, &model, 640., 420., false);
        let g420 = split_geom(&cfg, bw, fs, lh, 640., top420, bottom420);
        assert!(
            !g420.collapsed,
            "h=420 span={} 已放得下一行输入（input_h={}）",
            bottom420 - top420,
            g420.input_h
        );
        // 80×60 常规窗口不收起。
        let (top, bottom) = body_region(&cfg, &model, 800., 600., false);
        let g = split_geom(&cfg, bw, fs, lh, 800., top, bottom);
        assert!(!g.collapsed);
    }

    #[test]
    fn split_article_height_snaps_to_whole_text_lines() {
        let (sess, model, mut cfg, _) = fixture();
        cfg.display_mode = gale_core::config::DisplayMode::Split;
        let (bw, fs, base_lh) = body_metrics(&sess, &cfg, 1280.);
        let lh = line_height_for_anchor(&cfg, fs, base_lh, false);
        // 用 1400 高窗：输入卡行高随 line_gap 变大后，800 高会落进「薄区不吸附」
        // 豁免带（input_h 贴 input_min），吸附断言的前提（厚区）不成立。
        let (top, bottom) = body_region(&cfg, &model, 1280., 1400., false);
        let g = split_geom(&cfg, bw, fs, lh, 1280., top, bottom);
        assert!(!g.collapsed);
        let rows = (g.article_h - 20.) / lh;
        assert!(
            (rows - rows.floor()).abs() < 0.36 || rows < 1.,
            "article pane should end on a whole line, rows={rows}"
        );
    }

    #[test]
    fn split_ratio_drag_lands_exactly_where_dropped() {
        // 2026-11-13 用户二轮反馈：不设人为限制，拖到哪里算哪里。
        // 两个吃高度的机制都要拆：①比例上限 0.78/0.9 → 0.97；②整行吸附的零头
        // 反哺输入卡（薄区禁用）。断言：ratio 0.97 时输入卡 == 物理最小高
        // （input_min = 卡头 24+16+一行，无吸附加成），且显著矮于 0.78；
        // 极简/非极简同路径。
        let (sess, model, mut cfg, _) = fixture();
        cfg.display_mode = gale_core::config::DisplayMode::Split;
        let (bw, fs, base_lh) = body_metrics(&sess, &cfg, 1280.);
        let lh = line_height_for_anchor(&cfg, fs, base_lh, false);
        // 输入卡行高随 line_gap（与 split_geom 同口径，四轮下限 0.10 倍）。
        let line_h = fs * (1. + cfg.line_gap).clamp(0.10, 3.0);
        let (top, bottom) = body_region(&cfg, &model, 1280., 1400., false);
        let span = bottom - top;
        // 【传统模式输入区】最低 = 卡头 + 一行字 + 8px 内边距（不再叠编码留白）。
        let input_min = (input_header_h() + fs + 8.).min(span * 0.6);
        cfg.split_ratio = 0.97;
        let g97 = split_geom(&cfg, bw, fs, lh, 1280., top, bottom);
        assert!(!g97.collapsed);
        assert!(
            (g97.input_h - input_min).abs() <= 0.5,
            "thin-zone input must equal the physical one-line minimum: \
             input_h={} input_min={} span={} fs={} lh={}",
            g97.input_h,
            input_min,
            span,
            fs,
            lh
        );
        assert!(g97.input_h >= fs, "one typing line still fits");
        assert!(g97.article_h >= lh, "article pane keeps at least one line");
        cfg.split_ratio = 0.78;
        let g78 = split_geom(&cfg, bw, fs, lh, 1280., top, bottom);
        assert!(
            g97.input_h < g78.input_h * 0.8,
            "dragging to 0.97 must shrink well below the old 0.78 result: {} vs {}",
            g97.input_h,
            g78.input_h
        );
        // 极简模式同一几何路径：同样收敛到 input_min、不塌陷。
        cfg.minimal = true;
        cfg.split_ratio = 0.97;
        let (top, bottom) = body_region(&cfg, &model, 1280., 1400., false);
        let span = bottom - top;
        let input_min = (input_header_h() + fs + 8.).min(span * 0.6);
        let g = split_geom(&cfg, bw, fs, lh, 1280., top, bottom);
        assert!(!g.collapsed);
        assert!(
            (g.input_h - input_min).abs() <= 0.5,
            "minimal: input_h={} input_min={} span={}",
            g.input_h,
            input_min,
            span
        );
        assert!(g.input_h >= fs, "one typing line still fits");
    }

    #[test]
    fn minimal_top_band_shows_hud_and_keeps_window_controls_always_available() {
        let (mut sess, model, mut cfg, lines) = fixture();
        sess.comparer.pos = 3;
        cfg.minimal = true;
        assert_eq!(minimal_band_h(&cfg), 76.);
        assert_eq!(minimal_top(&cfg), 84.);
        let model_far = Model {
            pointer: Some((120., 400.)),
            hints: true,
            ..Default::default()
        };
        let mut hits = Vec::new();
        let frame = build(
            &sess,
            &cfg,
            &theme("墨玉"),
            &model_far,
            1280.,
            800.,
            14_000,
            14.,
            &mut Animation::default(),
            &mut hits,
            &lines,
        );
        let has = |text: &str| {
            frame
                .cmds
                .iter()
                .any(|c| matches!(c, DrawCmd::TextRun { text: t, .. } if t == text))
        };
        // 数据行与常规统计带同指标：速度 + 击键/码长/键准/错字/用时/编码。
        assert!(has("字/分"));
        assert!(has("击键/秒"));
        assert!(has("码长"));
        assert!(has("键准"));
        assert!(has("用时"));
        assert!(has("编码"));
        // 指针不在窗口控制区也保留四个动作与窗口控制入口。
        assert!(hits.iter().any(|h| h.action == Action::Exit));
        assert!(hits.iter().any(|h| h.action == Action::ToggleAnalysis));
        // 悬停在控制胶囊上：控件提亮但数据行仍在（不再让位）。
        let model_near = Model {
            pointer: Some((1240., 20.)),
            hints: true,
            ..Default::default()
        };
        let mut hits = Vec::new();
        let frame = build(
            &sess,
            &cfg,
            &theme("墨玉"),
            &model_near,
            1280.,
            800.,
            14_000,
            14.,
            &mut Animation::default(),
            &mut hits,
            &lines,
        );
        assert!(hits.iter().any(|h| h.action == Action::Minimize));
        assert!(hits.iter().any(|h| h.action == Action::Maximize));
        assert!(hits.iter().any(|h| h.action == Action::Exit));
        assert!(frame
            .cmds
            .iter()
            .any(|c| matches!(c, DrawCmd::TextRun { text, .. } if text == "字/分")));
    }

    #[test]
    fn line_gap_extremes_reach_full_range_without_render_clamp() {
        // 2026-11-13 三轮：行距全程 0.70..=3.00 倍（负 line_gap 放开）。
        // 四轮：下限再放到 0.10 倍（line_gap=-0.90）。
        let (sess, _, mut cfg, _) = fixture();
        cfg.line_gap = -0.90;
        let (_, fs, lh_floor) = body_metrics(&sess, &cfg, 1280.);
        assert!(
            (lh_floor - fs * 0.10).abs() <= 0.01,
            "line_gap=-0.9 → 行高==0.10 倍字号（四轮下限），got {lh_floor} vs fs {fs}"
        );
        cfg.line_gap = -0.30;
        let (_, fs, lh_min) = body_metrics(&sess, &cfg, 1280.);
        assert!(
            (lh_min - fs * 0.70).abs() <= 0.01,
            "line_gap=-0.3 → 行高==0.70 倍字号，got {lh_min} vs fs {fs}"
        );
        cfg.line_gap = 0.0;
        let (_, _, lh_one) = body_metrics(&sess, &cfg, 1280.);
        assert!(
            (lh_one - fs * 1.0).abs() <= 0.01,
            "line_gap=0 → 行高==字号（1.00 倍），got {lh_one} vs fs {fs}"
        );
        cfg.line_gap = 2.0;
        let (_, _, lh_max) = body_metrics(&sess, &cfg, 1280.);
        assert!(
            (lh_max - fs * 3.0).abs() <= 0.01,
            "line_gap=2 → 行高==3 倍字号，got {lh_max} vs {}",
            fs * 3.0
        );
        // 2026-11-14 审计：正文的有效行高必须与 body_metrics 完全一致，
        // 不能再被 line_height_for_anchor 里的 fs+6 地板悄悄抬高。
        for gap in [-0.90f32, -0.30, 0.0, 0.66, 2.0] {
            cfg.line_gap = gap;
            let (_, fs, base) = body_metrics(&sess, &cfg, 1280.);
            let effective = line_height_for_anchor(&cfg, fs, base, true);
            assert!(
                (effective - base).abs() <= 0.001,
                "line_gap={gap}: effective {effective} != base {base}"
            );
        }
    }

    #[test]
    fn split_cards_paint_input_pane_with_follow_color_and_article_with_secondary() {
        // 2026-11-13 语义对调修复：输入卡（真正跟打面）吃「跟打区背景色」，
        // 文章卡吃「次级背景色」——用户自定义跟打区背景色必须落在打字那张卡上。
        let (sess, model, mut cfg, lines) = fixture();
        cfg.display_mode = gale_core::config::DisplayMode::Split;
        let typed: String = sess.comparer.typed.iter().collect();
        let input_lines = vec![(0usize, typed, vec![0., 54.])];
        let mut anim = Animation::default();
        let mut hits = Vec::new();
        let t = theme("墨玉");
        let frame = build_with_input(
            &sess,
            &cfg,
            &t,
            &model,
            1280.,
            800.,
            0,
            0.,
            &mut anim,
            &mut hits,
            &lines,
            &input_lines,
        );
        let (top, bottom) = body_region(&cfg, &model, 1280., 800., false);
        let (bw, fs, base_lh) = body_metrics(&sess, &cfg, 1280.);
        let lh = line_height_for_anchor(&cfg, fs, base_lh, true);
        let g = split_geom(&cfg, bw, fs, lh, 1280., top, bottom);
        assert!(!g.collapsed);
        let follow = rgba(*t.colors.get("跟打区背景色").unwrap());
        let secondary = rgba(*t.colors.get("次级背景色").unwrap());
        let fill_at = |x: f32, y: f32, w: f32, h: f32| {
            frame
                .cmds
                .iter()
                .find_map(|c| match c {
                    DrawCmd::RoundRect {
                        x: cx,
                        y: cy,
                        w: cw,
                        h: ch,
                        color,
                        ..
                    } if (*cx - x).abs() < 0.5
                        && (*cy - y).abs() < 0.5
                        && (*cw - w).abs() < 0.5
                        && (*ch - h).abs() < 0.5 =>
                    {
                        Some(*color)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no RoundRect at ({x},{y},{w},{h})"))
        };
        assert_eq!(fill_at(g.x, g.input_y, g.pane_w, g.input_h), follow);
        assert_eq!(fill_at(g.x, g.top, g.pane_w, g.article_h), secondary);
        // 2026-11-13 二轮：「跟打区背景色」=整体界面底色——窗口基底也吃它。
        let base = frame.cmds.iter().find_map(|c| match c {
            DrawCmd::Rect {
                x,
                y,
                w: cw,
                h: ch,
                color,
            } if *x == 0. && *y == 0. && *cw == 1280. && *ch == 800. => Some(*color),
            _ => None,
        });
        assert_eq!(base, Some(follow), "window base must follow 跟打区背景色");
    }

    #[test]
    fn minimal_mode_window_base_uses_follow_color() {
        // 极简模式没有卡片，整个窗口就是跟打面——基底必须吃「跟打区背景色」，
        // 否则用户改色极简下无感（2026-11-13 二轮反馈的第二个根因）。
        let (sess, model, mut cfg, lines) = fixture();
        cfg.minimal = true;
        let mut anim = Animation::default();
        let mut hits = Vec::new();
        let t = theme("墨玉");
        let frame = build(
            &sess, &cfg, &t, &model, 1280., 800., 0, 0., &mut anim, &mut hits, &lines,
        );
        let follow = rgba(*t.colors.get("跟打区背景色").unwrap());
        let base = frame.cmds.iter().find_map(|c| match c {
            DrawCmd::Rect {
                x,
                y,
                w: cw,
                h: ch,
                color,
            } if *x == 0. && *y == 0. && *cw == 1280. && *ch == 800. => Some(*color),
            _ => None,
        });
        assert_eq!(
            base,
            Some(follow),
            "minimal window base must follow 跟打区背景色"
        );
    }

    #[test]
    fn split_input_card_line_height_follows_line_gap() {
        // 2026-11-13 二轮：输入卡行高不再固定 1.34 倍——行距调到最小输入区跟着压缩。
        // 三轮：负行距（0.70 倍）同样生效。四轮：下限 0.10 倍。
        let (sess, model, mut cfg, _) = fixture();
        cfg.display_mode = gale_core::config::DisplayMode::Split;
        let (bw, fs, base_lh) = body_metrics(&sess, &cfg, 1280.);
        let (top, bottom) = body_region(&cfg, &model, 1280., 900., false);
        cfg.line_gap = -0.90;
        let lh_f = line_height_for_anchor(&cfg, fs, base_lh, false);
        let gf = split_geom(&cfg, bw, fs, lh_f, 1280., top, bottom);
        assert!(
            (gf.line_h - fs * 0.10).abs() <= 0.01,
            "line_gap=-0.9 → 输入卡行高==0.10 倍字号（四轮下限），got {}",
            gf.line_h
        );
        cfg.line_gap = -0.30;
        let lh_n = line_height_for_anchor(&cfg, fs, base_lh, false);
        let gn = split_geom(&cfg, bw, fs, lh_n, 1280., top, bottom);
        assert!(
            (gn.line_h - fs * 0.70).abs() <= 0.01,
            "line_gap=-0.3 → 输入卡行高==0.70 倍字号，got {}",
            gn.line_h
        );
        cfg.line_gap = 0.0;
        let lh0 = line_height_for_anchor(&cfg, fs, base_lh, false);
        let g0 = split_geom(&cfg, bw, fs, lh0, 1280., top, bottom);
        assert!(
            (g0.line_h - fs).abs() <= 0.01,
            "line_gap=0 → 输入卡行高==字号，got {}",
            g0.line_h
        );
        cfg.line_gap = 2.0;
        let (bw, fs, base_lh) = body_metrics(&sess, &cfg, 1280.);
        let lh2 = line_height_for_anchor(&cfg, fs, base_lh, false);
        let g2 = split_geom(&cfg, bw, fs, lh2, 1280., top, bottom);
        assert!(
            (g2.line_h - fs * 3.0).abs() <= 0.01,
            "line_gap=2 → 输入卡行高==3 倍字号，got {}",
            g2.line_h
        );
    }

    #[test]
    fn tight_line_gap_draws_word_code_inline_at_full_size() {
        // 【词提下行 r3 2026-11-14】fs36/gap0.5 → lh54：行内空间再紧，词提也**不退让**、
        // 不吃行距——编码行画在括线之下（yy + fs + offset + 2，对照与字贴两模式同
        // 口径），括线保持四轮校准紧贴位；文字上方只留键入编码（preedit，跟随光标）。
        let (sess, model, mut cfg, lines) = fixture();
        cfg.font_size = 36.;
        cfg.hint_font_size = 30.;
        cfg.line_gap = 0.5;
        let typed: String = sess.comparer.typed.iter().collect();
        let input_lines = vec![(0usize, typed, vec![0., 36.])];
        let t = theme("墨玉");
        let mut anim = Animation::default();
        let mut hits = Vec::new();
        let frame = build_with_input(
            &sess,
            &cfg,
            &t,
            &model,
            1280.,
            800.,
            0,
            0.,
            &mut anim,
            &mut hits,
            &lines,
            &input_lines,
        );
        let article_y = frame
            .cmds
            .iter()
            .find_map(|c| match c {
                DrawCmd::StyledLine { y, .. } => Some(*y),
                _ => None,
            })
            .unwrap();
        let code_y = frame
            .cmds
            .iter()
            .find_map(|c| match c {
                DrawCmd::TextRun { y, text, .. } if text.as_str() == "abcd" => Some(*y),
                _ => None,
            })
            .expect("紧行距下编码行必须原样绘制（不再退让）");
        assert!(
            (code_y - (article_y + 44.12)).abs() < 0.5,
            "编码行画在括线之下（fs+offset+2）y={code_y}，文章行 y={article_y}"
        );
        let rank_h = frame.cmds.iter().find_map(|c| match c {
            DrawCmd::TextRun { h, text, y, .. } if text.as_str() == "2" && *y > article_y => {
                Some(*h)
            }
            _ => None,
        });
        let rank_h = rank_h.unwrap_or(0.);
        assert!(
            rank_h > 20.,
            "选重数字按设置字号全尺寸（≈23.4），不缩号不上角，got {rank_h}"
        );
        // 字贴（默认）模式同口径：紧行距下编码行同样在场。
        let frame2 = build(
            &sess, &cfg, &t, &model, 1280., 800., 0, 0., &mut anim, &mut hits, &lines,
        );
        assert!(
            frame2.cmds.iter().any(|c| matches!(c,
                DrawCmd::TextRun { text, .. } if text.as_str() == "abcd")),
            "字贴模式紧行距下编码行同样原样绘制"
        );
    }

    #[test]
    fn typed_code_grows_right_from_the_caret_and_keeps_the_latest_tail() {
        let mut cfg = Config::default();
        cfg.hint_font_size = 80.;
        cfg.preedit_font_size = 22.;
        cfg.preedit_offset = -8.;
        for caret_x in [4., 240.] {
            for code in ["a", "abcd", "abcdefghijklmnop"] {
                let mut frame = Frame::new(480., 240.);
                draw_preedit(&mut frame, &cfg, code, (caret_x, 120.), 4., 476., [1.; 4]);
                let DrawCmd::TextRun {
                    x,
                    y,
                    h,
                    align,
                    text,
                    ..
                } = frame
                    .cmds
                    .iter()
                    .find(|cmd| matches!(cmd, DrawCmd::TextRun { .. }))
                    .unwrap()
                else {
                    unreachable!()
                };
                assert!(matches!(align, Align::Left));
                // 【用户 2026-11-15】键入编码字号固定为字体大小的 2/5，不再跟随编码字号设置。
                assert_eq!(*h, preedit_size(&cfg), "键入编码字号 = 字体大小 × 2/5");
                assert!(
                    *y + *h <= 120. && *y >= 120. - preedit_headroom(&cfg),
                    "键入编码整行落在预留留白内（编码高低只在此区间内微调）: y={}", *y
                );
                assert_eq!(*x, caret_x, "编码不得被固定十四字符槽推离光标");
                assert_eq!(text, code, "有空间时必须完整显示，不能固定截到十四位");
            }
        }
        let code = "abcdefghijklmnopqrstuvwxyz0123456789最新尾码";
        for caret_x in [4., 240., 476.] {
            let mut frame = Frame::new(480., 240.);
            draw_preedit(&mut frame, &cfg, code, (caret_x, 120.), 4., 476., [1.; 4]);
            let DrawCmd::TextRun { x, text, .. } = frame
                .cmds
                .iter()
                .find(|cmd| matches!(cmd, DrawCmd::TextRun { .. }))
                .unwrap()
            else {
                unreachable!()
            };
            assert!(
                code.ends_with(text.trim_start_matches('…')),
                "保留正在输入的尾部"
            );
            assert!(text.ends_with('码'));
            PREEDIT_FONTS.with(|fonts| {
                let width =
                    fonts
                        .borrow_mut()
                        .text_width(FontFamily::Mono, preedit_size(&cfg), text);
                assert!(
                    *x >= 4. && *x + width <= 476.01,
                    "真实中英文测宽须在正文边界内"
                );
            });
        }
        cfg.show_preedit = false;
        let mut frame = Frame::new(480., 240.);
        draw_preedit(&mut frame, &cfg, "abcd", (240., 120.), 4., 476., [1.; 4]);
        assert!(frame.cmds.is_empty(), "关闭只隐藏键入编码，不依赖词提开关");
    }

    #[test]
    fn small_window_keeps_hundred_pixel_text_and_composition_visible_together() {
        use gale_core::compare::InputEvent;
        for mode in [
            gale_core::config::DisplayMode::Copybook,
            gale_core::config::DisplayMode::Split,
        ] {
            let (mut sess, model, mut cfg, mut lines) = fixture();
            cfg.display_mode = mode;
            cfg.font_size = 100.;
            cfg.preedit_font_size = 100.;
            cfg.preedit_offset = 0.;
            for (_, text, widths) in &mut lines {
                *widths = (0..=text.chars().count())
                    .map(|i| i as f32 * 100.)
                    .collect();
            }
            sess.feed_input(
                InputEvent::Commit {
                    chars: "甲".into()
                },
                100,
            );
            sess.set_preedit("abcd".into());
            let mut anim = Animation::default();
            let frame = build_with_input(
                &sess,
                &cfg,
                &theme("墨玉"),
                &model,
                640.,
                420.,
                0,
                0.,
                &mut anim,
                &mut Vec::new(),
                &lines,
                &[(0, "甲".into(), vec![0., 100.])],
            );
            let mut clips = vec![(0_f32, 420_f32)];
            let mut found = false;
            for cmd in &frame.cmds {
                match cmd {
                    DrawCmd::PushClip { y, h, .. } => {
                        let &(t, b) = clips.last().unwrap();
                        clips.push((t.max(*y), b.min(y + h)));
                    }
                    DrawCmd::PopClip => {
                        clips.pop();
                    }
                    DrawCmd::TextRun { text, y, h, .. }
                        if text == "abcd" && *h == preedit_size(&cfg) =>
                    {
                        let &(t, b) = clips.last().unwrap();
                        assert!(*y >= t && y + h <= b, "full composition must fit: {mode:?}");
                        found = true;
                    }
                    _ => {}
                }
            }
            assert!(found);
            let (_, caret_y) = anim.caret_target.unwrap();
            assert!(
                caret_y + 107. <= ledger_bar_y(420.) - 8.,
                "current text must fit below composition: {mode:?} caret={caret_y}"
            );
        }
    }

    /// 【动效 2026-11-15】换行时光标必须直接落到新行行首，不能从上一行行尾斜着飞过去；
    /// 同一行内的横移仍然走缓动。
    #[test]
    fn caret_snaps_on_line_wrap_but_eases_horizontally() {
        let mut anim = Animation::default();
        anim.caret = Some((200., 100.));
        anim.caret_target = Some((200., 100.));
        // 跨行：直接等于目标，不产生动画。
        let wrapped = smooth_caret(&mut anim, (8., 220.), 10.0, true, 120.);
        assert_eq!(wrapped, (8., 220.), "换行应直接落位");
        assert!(anim.caret_anim.is_none(), "换行不应留下动画状态");
        // 同行横移：第一帧仍在缓动途中，不立刻到目标。
        anim.caret = Some((8., 220.));
        anim.caret_target = Some((8., 220.));
        let first = smooth_caret(&mut anim, (48., 220.), 11.0, true, 120.);
        assert!((first.0 - 8.).abs() < 0.01, "第一帧仍应从原位置出发：{}", first.0);
        let mid = smooth_caret(&mut anim, (48., 220.), 11.08, true, 120.);
        assert!(mid.0 > 8. && mid.0 < 48., "80ms 后应在途中：{}", mid.0);
        assert!((mid.1 - 220.).abs() < 0.01, "同行横移不应改变行位");
    }

    /// 【动效 2026-11-15】翻页缓动：首行序号一跳先把整行位移补回去，
    /// 之后在 280ms 内缓缓收到 0（真实插值由 scroll_offset 直接断言）。
    #[test]
    fn scroll_offset_keeps_the_page_shift_then_eases_it_away() {
        let mut anim = Animation::default();
        assert_eq!(scroll_offset(&mut anim, 0, 10.0, true), 0.);
        let jumped = scroll_offset(&mut anim, 1, 10.5, true);
        assert!(
            (jumped - 1.0).abs() < 0.01,
            "首帧必须补回整行位移：{jumped}"
        );
        let mid = scroll_offset(&mut anim, 1, 10.63, true);
        assert!(mid > 0.02 && mid < 0.99, "中途应在缓动：{mid}");
        let end = scroll_offset(&mut anim, 1, 11.0, true);
        assert_eq!(end, 0., "280ms 后必须归零");
    }

    #[test]
    fn tight_rows_leave_composition_clear_of_the_previous_completed_text() {
        use gale_core::compare::InputEvent;
        for mode in [
            gale_core::config::DisplayMode::Copybook,
            gale_core::config::DisplayMode::Split,
        ] {
            for (size, offset) in [(40., -8.), (100., -20.), (30., 12.)] {
                let (mut sess, model, mut cfg, lines) = fixture();
                cfg.display_mode = mode;
                cfg.line_gap = -0.35;
                cfg.preedit_font_size = size;
                cfg.preedit_offset = offset;
                sess.feed_input(
                    InputEvent::Commit {
                        chars: "甲乙丙丁戊己".into(),
                    },
                    100,
                );
                sess.set_preedit("tail987".into());
                let input = vec![lines[0].clone(), (5, "己".into(), vec![0., 54.])];
                let frame = build_with_input(
                    &sess,
                    &cfg,
                    &theme("墨玉"),
                    &model,
                    1280.,
                    1400.,
                    0,
                    0.,
                    &mut Animation::default(),
                    &mut Vec::new(),
                    &lines,
                    &input,
                );
                let code_top = frame
                    .cmds
                    .iter()
                    .find_map(|cmd| match cmd {
                        DrawCmd::TextRun { text, y, .. } if text == "tail987" => Some(*y),
                        _ => None,
                    })
                    .expect("active composition must remain visible");
                // The split article and input both contain this line; the latter
                // is drawn last and is the preceding committed input row.
                let previous_bottom = frame
                    .cmds
                    .iter()
                    .filter_map(|cmd| match cmd {
                        DrawCmd::StyledLine { text, y, size, .. } if text == "甲乙丙丁戊" => {
                            Some(y + size + 6.)
                        }
                        _ => None,
                    })
                    .last()
                    .expect("keep the preceding row in this tall viewport");
                assert!(code_top >= previous_bottom,
                    "composition must clear previous text: mode={mode:?} size={size} offset={offset} code={code_top} previous={previous_bottom}");
                assert!(
                    !frame.cmds.iter().any(|cmd| matches!(cmd,
                    DrawCmd::TextRun { text, .. } if text == "abcd")),
                    "completed row must also release its code hints"
                );
            }
        }
    }

    #[test]
    fn first_line_composition_is_inside_its_content_clip_and_does_not_move_on_first_key() {
        for mode in [
            gale_core::config::DisplayMode::Copybook,
            gale_core::config::DisplayMode::Split,
        ] {
            for minimal in [false, true] {
                for (size, offset) in [(14., 0.), (30., -8.), (100., -20.), (30., 12.)] {
                    let (mut sess, model, mut cfg, lines) = fixture();
                    cfg.display_mode = mode;
                    cfg.minimal = minimal;
                    cfg.preedit_font_size = size;
                    cfg.preedit_offset = offset;
                    let input = vec![(0, String::new(), vec![0.])];
                    let mut anim = Animation::default();
                    let _ = build_with_input(
                        &sess,
                        &cfg,
                        &theme("墨玉"),
                        &model,
                        1280.,
                        1000.,
                        0,
                        0.,
                        &mut anim,
                        &mut Vec::new(),
                        &lines,
                        &input,
                    );
                    let before = anim.caret_target.unwrap();
                    sess.set_preedit("tail987".into());
                    let frame = build_with_input(
                        &sess,
                        &cfg,
                        &theme("墨玉"),
                        &model,
                        1280.,
                        1000.,
                        0,
                        0.,
                        &mut anim,
                        &mut Vec::new(),
                        &lines,
                        &input,
                    );
                    assert_eq!(anim.caret_target.unwrap(), before, "第一键不得把正文顶下来");
                    let mut clips = vec![(0_f32, 0_f32, 1280_f32, 1000_f32)];
                    let mut found = false;
                    for cmd in frame.cmds {
                        match cmd {
                            DrawCmd::PushClip { x, y, w, h } => {
                                let &(l, t, r, b) = clips.last().unwrap();
                                clips.push((l.max(x), t.max(y), r.min(x + w), b.min(y + h)));
                            }
                            DrawCmd::PopClip => {
                                clips.pop();
                            }
                            DrawCmd::TextRun { text, x, y, h, .. } if text == "tail987" => {
                                let &(l, t, r, b) = clips.last().unwrap();
                                assert!(x >= l && x < r && y >= t && y+h <= b,
                                    "首行编码必须整体可见 mode={mode:?} minimal={minimal} size={size} offset={offset} run=({x},{y},{h}) clip={:?}", clips.last());
                                if minimal {
                                    assert!(y >= minimal_band_h(&cfg));
                                }
                                found = true;
                            }
                            _ => {}
                        }
                    }
                    assert!(found);
                }
            }
        }
    }

    #[test]
    fn completed_word_hints_disappear_and_backspace_restores_them_in_both_modes() {
        use gale_core::compare::InputEvent;
        for mode in [
            gale_core::config::DisplayMode::Copybook,
            gale_core::config::DisplayMode::Split,
        ] {
            let (mut sess, mut model, mut cfg, lines) = fixture();
            cfg.display_mode = mode;
            cfg.hint_show_code = true;
            model.words.push(HintWord {
                start: 2,
                len: 2,
                text: "丙丁".into(),
                code: Some("wxyz".into()),
                rank: 3,
                tail: String::new(),
            });
            let render = |sess: &Session| {
                build_with_input(
                    sess,
                    &cfg,
                    &theme("墨玉"),
                    &model,
                    1280.,
                    900.,
                    0,
                    0.,
                    &mut Animation::default(),
                    &mut Vec::new(),
                    &lines,
                    &[(0, String::new(), vec![0.])],
                )
            };
            let has_code = |sess: &Session, value: &str| {
                render(sess)
                    .cmds
                    .iter()
                    .any(|cmd| matches!(cmd, DrawCmd::TextRun { text, .. } if text == value))
            };
            sess.feed_input(
                InputEvent::Commit {
                    chars: "甲".into()
                },
                100,
            );
            assert!(has_code(&sess, "abcd"), "当前未完成词保留提示");
            sess.feed_input(
                InputEvent::Commit {
                    chars: "错".into()
                },
                200,
            );
            assert!(!has_code(&sess, "abcd"), "已打过的词即使有错也清除编码");
            assert!(has_code(&sess, "wxyz"), "未打词不能一起被隐藏");
            sess.feed_input(InputEvent::Backspace, 300);
            assert!(has_code(&sess, "abcd"), "回改后提示自动恢复");
        }
    }

    #[test]
    fn hint_vertical_offset_moves_code_in_both_modes_without_moving_the_article() {
        let (sess, model, mut cfg, lines) = fixture();
        cfg.hint_font_size = 30.;
        cfg.line_gap = -0.9;
        let input_lines = vec![(0usize, String::new(), vec![0.])];
        for mode in [
            gale_core::config::DisplayMode::Copybook,
            gale_core::config::DisplayMode::Split,
        ] {
            cfg.display_mode = mode;
            let render = |cfg: &Config| {
                build_with_input(
                    &sess,
                    cfg,
                    &theme("墨玉"),
                    &model,
                    1280.,
                    800.,
                    0,
                    0.,
                    &mut Animation::default(),
                    &mut Vec::new(),
                    &lines,
                    &input_lines,
                )
            };
            cfg.hint_code_offset = 0.;
            let first = render(&cfg);
            cfg.hint_code_offset = 18.;
            let shifted = render(&cfg);
            let code = |frame: &Frame| {
                frame
                    .cmds
                    .iter()
                    .find_map(|cmd| match cmd {
                        DrawCmd::TextRun { text, y, h, .. } if text == "abcd" => Some((*y, *h)),
                        _ => None,
                    })
                    .expect("词提编码必须保持可见")
            };
            let article = |frame: &Frame| {
                frame
                    .cmds
                    .iter()
                    .find_map(|cmd| match cmd {
                        DrawCmd::StyledLine { y, .. } => Some(*y),
                        _ => None,
                    })
                    .unwrap()
            };
            assert_eq!(code(&first).1, 30.);
            assert_eq!(code(&shifted).1, 30.);
            assert!((code(&shifted).0 - code(&first).0 - 18.).abs() < 0.01);
            assert_eq!(
                article(&first),
                article(&shifted),
                "词提位置不参与正文行距分配"
            );
        }
    }

    /// 【审计 V12】【审计 V17】测试辅助：色值 → gale_core::theme::Color（走 WCAG 计算）。
    fn probe(rgb: [f32; 4]) -> Color {
        Color(
            (rgb[0] * 255.).round() as u8,
            (rgb[1] * 255.).round() as u8,
            (rgb[2] * 255.).round() as u8,
            255,
        )
    }
    /// 两个颜色的 RGB 通道距离（0..=765）：越小越容易被误认成同一个颜色。
    fn channel_distance(a: [f32; 4], b: [f32; 4]) -> f32 {
        ((a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()) * 255.
    }
    fn body_bg(t: &Theme) -> Color {
        *t.colors.get("跟打区背景色").expect("主题必须给出跟打区背景色")
    }

    /// 【审计 V12】「当前正在输入的字符」必须单独成层，三套内置主题逐一验证：
    /// 与打对/打错/裸强调色都不同色，且对背景的对比度高于裸强调色并达到 4.5:1。
    #[test]
    fn audit_v12_current_input_is_its_own_layer_in_every_builtin_theme() {
        for name in ["墨玉", "宣纸", "青瓷"] {
            let t = theme(name);
            let p = tokens(&t);
            let (current, chip, underline) = current_input_style(&t);
            let bg = body_bg(&t);
            // 视觉标记：淡底块 + 实心下划线。
            assert!(underline, "{name}: 当前输入层必须带下划线标记");
            assert!(chip[3] > 0. && chip[3] < 0.3, "{name}: 当前位底色应是淡底");
            // 与打对/打错/裸强调色互不相同——不是同色相换透明度。
            assert_ne!(probe(current), probe(p.correct), "{name}: 当前输入与打对撞色");
            assert_ne!(probe(current), probe(p.red), "{name}: 当前输入与打错撞色");
            assert_ne!(
                probe(current),
                probe(p.accent),
                "{name}: 当前输入与裸强调色相同，未形成独立一层"
            );
            // 三者之间足够远（避免「看着一样」）。
            assert!(
                channel_distance(current, p.correct) >= 100.,
                "{name}: 当前输入与打对通道距离过近 {:.0}",
                channel_distance(current, p.correct)
            );
            assert!(
                channel_distance(current, p.red) >= 90.,
                "{name}: 当前输入与打错通道距离过近 {:.0}",
                channel_distance(current, p.red)
            );
            // 当前输入层比裸强调色更醒目（对比度更高），且达到 AA 4.5:1。
            let current_ratio = contrast_ratio(probe(current), bg);
            assert!(
                current_ratio >= 4.5,
                "{name}: 当前输入色对背景只有 {current_ratio:.2}:1"
            );
            assert!(
                current_ratio > contrast_ratio(probe(p.accent), bg),
                "{name}: 当前输入层不比裸强调色更醒目"
            );
            // 【审计 V17】对/错两态按状态取主题键，且在对背景维度上仍可辨（≥4.5）。
            assert_eq!(p.correct, rgba(t.get("打对字色")), "{name}: 打对必须取打对字色");
            assert_eq!(p.red, rgba(t.get("打错字色")), "{name}: 打错必须取打错字色");
            assert!(contrast_ratio(probe(p.correct), bg) >= 4.5, "{name}: 打对色对比不足");
            assert!(contrast_ratio(probe(p.red), bg) >= 4.5, "{name}: 打错色对比不足");
        }
    }

    /// 【审计 V12】【审计 V17】纯逻辑：角色只由「状态 + 是否当前位」决定，与栏位无关；
    /// 反馈色/状态色一律取主题键，不再用 hex 硬编码的铜/绿。
    #[test]
    fn audit_v17_glyph_roles_follow_state_not_position() {
        let t = theme("墨玉");
        let p = tokens(&t);
        let current = current_input_style(&t).0;
        let tint = hex("ff6600");
        // 当前位优先于状态：正在输入的那一格永远是最明确的一层。
        assert_eq!(
            classify_glyph(Some(CharState::Wrong), true, true),
            GlyphRole::Current
        );
        assert_eq!(
            classify_glyph(Some(CharState::Correct), true, false),
            GlyphRole::Current
        );
        assert_eq!(
            classify_glyph(Some(CharState::Correct), false, false),
            GlyphRole::Correct
        );
        assert_eq!(
            classify_glyph(Some(CharState::Wrong), false, false),
            GlyphRole::Wrong
        );
        assert_eq!(
            classify_glyph(Some(CharState::Pending), false, true),
            GlyphRole::Hint
        );
        assert_eq!(classify_glyph(None, false, false), GlyphRole::Pending);
        assert_eq!(classify_glyph(None, false, true), GlyphRole::Hint);
        // 角色 → 主题键色：状态色不因栏位改变。
        assert_eq!(glyph_color(&p, GlyphRole::Current, None, current), current);
        assert_eq!(glyph_color(&p, GlyphRole::Wrong, Some(tint), current), p.red);
        assert_eq!(
            glyph_color(&p, GlyphRole::Correct, Some(tint), current),
            p.correct
        );
        assert_eq!(glyph_color(&p, GlyphRole::Hint, Some(tint), current), tint);
        assert_eq!(
            glyph_color(&p, GlyphRole::Pending, Some(tint), current),
            p.pending
        );
        // 状态档位 → 主题键（四档全部来自 tokens()）。
        assert_eq!(status_color(&p, StatusTone::Hint), p.muted);
        assert_eq!(status_color(&p, StatusTone::Neutral), p.text2);
        assert_eq!(status_color(&p, StatusTone::Active), p.accent);
        assert_eq!(status_color(&p, StatusTone::Done), p.correct);
        assert_ne!(status_color(&p, StatusTone::Done), p.green);
        assert_ne!(status_color(&p, StatusTone::Hint), p.copper);
    }

    /// 【审计 V12】字贴正文同屏四层（打对/打错/当前/未打）：当前位取当前输入色并画出
    /// 淡底块 + 实心下划线，且不改动邻居的状态色。
    #[test]
    fn audit_v12_copybook_marks_current_char_and_keeps_state_colors() {
        let (mut sess, model, cfg, lines) = fixture();
        // 【审计 V12】当前位放在行首：它压过状态色但只占一格，同屏仍能看到
        // 打错（第 2 格）、打对（第 3 格）、未打（第 4 格）和词提编码行。
        sess.comparer.states[0] = CharState::Correct;
        sess.comparer.states[1] = CharState::Wrong;
        sess.comparer.states[2] = CharState::Correct;
        sess.comparer.pos = 0;
        let t = theme("墨玉");
        let p = tokens(&t);
        let (current, chip, underline) = current_input_style(&t);
        assert!(underline);
        let frame = build(
            &sess,
            &cfg,
            &t,
            &model,
            800.,
            600.,
            0,
            0.,
            &mut Animation::default(),
            &mut Vec::new(),
            &lines,
        );
        let spans = frame
            .cmds
            .iter()
            .find_map(|cmd| match cmd {
                DrawCmd::StyledLine { spans, .. } => Some(spans),
                _ => None,
            })
            .unwrap();
        assert_eq!(spans[0].2, current, "当前位取当前输入色（压过词提色）");
        assert_eq!(spans[1].2, p.red, "打错仍是打错字色");
        assert_eq!(spans[2].2, p.correct, "打对仍是打对字色");
        assert_eq!(spans[3].2, p.pending, "邻居未打字符保持未打文字色");
        assert_eq!(spans[4].2, p.pending);
        // 当前位只保留淡底块（实心下划线已按用户要求移除）。
        assert!(
            frame.cmds.iter().any(|cmd| matches!(cmd,
                DrawCmd::RoundRect { color, .. } if *color == chip)),
            "当前位应画出强调色淡底块"
        );
        // 词提语义色仍在（甲乙有编码 abcd），四层颜色互不相同。
        assert!(frame.cmds.iter().any(|cmd| matches!(cmd,
            DrawCmd::TextRun { text, .. } if text == "abcd")));
        assert_ne!(current, p.correct);
        assert_ne!(current, p.red);
        assert_ne!(p.correct, p.red);
    }

    /// 【审计 V17】输入卡反馈色由状态决定：打错=打错字色、打对=打对字色、
    /// 未打/待定=未打文字色（旧实现把输入卡里所有非错字都画成打对色）。
    #[test]
    fn audit_v17_split_input_card_uses_state_colors_not_position_colors() {
        let (mut sess, model, mut cfg, lines) = fixture();
        cfg.display_mode = gale_core::config::DisplayMode::Split;
        sess.comparer.states[0] = CharState::Correct;
        sess.comparer.states[1] = CharState::Wrong;
        // pos 越过本行 → 第 3 个字符保持 Pending（不被算成「当前位」）。
        sess.comparer.pos = 3;
        let input_lines = vec![(0usize, "甲乙丙".to_string(), vec![0., 54., 108., 162.])];
        let t = theme("墨玉");
        let p = tokens(&t);
        let frame = build_with_input(
            &sess,
            &cfg,
            &t,
            &model,
            1280.,
            800.,
            0,
            0.,
            &mut Animation::default(),
            &mut Vec::new(),
            &lines,
            &input_lines,
        );
        let spans = frame
            .cmds
            .iter()
            .find_map(|cmd| match cmd {
                DrawCmd::StyledLine { text, spans, .. } if text == "甲乙丙" => Some(spans),
                _ => None,
            })
            .expect("输入卡应画出带 span 的正文行");
        assert_eq!(spans[0].2, p.correct, "打对取打对字色");
        assert_eq!(spans[1].2, p.red, "打错取打错字色");
        assert_eq!(spans[2].2, p.pending, "未打字符取未打文字色，不能算成打对");
    }


    #[test]
    fn overridden_palette_is_lifted_back_to_readable() {
        // 复刻用户现场：墨玉底改成中灰、打对/未打字色改成纯黑、词提 2 选改暗红。
        let mut o = std::collections::BTreeMap::new();
        o.insert("跟打区背景色".to_string(), "474747".to_string());
        o.insert("打对字色".to_string(), "000000".to_string());
        o.insert("未打文字色".to_string(), "000000".to_string());
        o.insert("词提2选色".to_string(), "410304".to_string());
        let t = theme_with_overrides("墨玉", &o);
        let card = t.get("次级背景色");
        let field = interface_bg(&t);
        let cr = gale_core::theme::contrast_ratio;
        assert!(cr(t.get("打对字色"), card) >= 3.0, "打对字色在文章卡上不可读");
        assert!(cr(t.get("未打文字色"), card) >= 3.0, "未打文字色在文章卡上不可读");
        assert!(cr(t.get("未打文字色"), field) >= 3.0, "未打文字色在输入卡上不可读");
        assert!(cr(t.get("词提2选色"), card) >= 3.0, "词提 2 选色不可读");
    }

    #[test]
    fn harmonize_persist_matches_restart_lift_and_writes_it_back() {
        // 复刻 2026-11-16 现场：灰砖 + 词提2选色=2d0000（对 #4a4a4a 卡面仅
        // 2.13:1）。当场全量保底后，覆盖表写回的就是保底值——双底 ≥3.0 且
        // 不等于原值；达标的 3 选 c8b07a 原样保留；写回值再装配一遍不再
        // 变化（== 重启装配严格一致）；跨键换深卡面后保底自动再校准。
        let mut o = std::collections::BTreeMap::new();
        o.insert("词提2选色".to_string(), "2d0000".to_string());
        o.insert("词提3选色".to_string(), "c8b07a".to_string());
        let mut t = theme_with_overrides("灰砖", &o);
        t.set("词提2选色", Color::from_hex_no_hash("2d0000").unwrap());
        t.set("词提3选色", Color::from_hex_no_hash("c8b07a").unwrap());
        let fixed = harmonize_persist(&mut t, &mut o);
        assert_eq!(fixed, 1, "只有深红一处需要保底");
        assert_ne!(o["词提2选色"], "2d0000", "深红在灰砖上不可读，保底值要写回覆盖表");
        assert_eq!(o["词提3选色"], "c8b07a", "达标色不动");
        let cr = gale_core::theme::contrast_ratio;
        let lc = Color::from_hex_no_hash(&o["词提2选色"]).unwrap();
        assert!(cr(lc, t.get("次级背景色")) >= 3.0, "抬亮后的色在文章卡上要可读");
        assert!(cr(lc, interface_bg(&t)) >= 3.0, "抬亮后的色在输入卡上要可读");
        // 幂等：写回后的覆盖表再走一遍装配+保底，值不再变化（== 重启装配）。
        let mut t2 = theme_with_overrides("灰砖", &o);
        let mut o2 = o.clone();
        assert_eq!(harmonize_persist(&mut t2, &mut o2), 0, "保底幂等");
        assert_eq!(o2, o, "写回值与重启装配严格一致");
        // 跨键：卡面换深后重新装配，先前保底的键要在新卡面上仍然可读。
        let mut o3 = o.clone();
        o3.insert("次级背景色".to_string(), "141414".to_string());
        let mut t3 = theme_with_overrides("灰砖", &o3);
        harmonize_persist(&mut t3, &mut o3);
        let c3 = Color::from_hex_no_hash(&o3["词提2选色"]).unwrap();
        assert!(
            cr(c3, t3.get("次级背景色")) >= 3.0,
            "换深卡面后词提 2 选色仍要可读（保底自动再校准）"
        );
    }

    #[test]
    fn gray_theme_keeps_ink_black_text_readable() {
        // 「灰砖」= 深灰底 #404040 + 墨黑字：用户明确要这个组合。
        // 中灰偏暗底上黑字对底色只有 2.0:1（低于 AA 4.5），只能保证可见与调性，
        // 字号越大越可读；这里按「不低于 2.0」把口径钉住，避免以后被无意提亮。
        let t = theme("灰砖");
        let cr = gale_core::theme::contrast_ratio;
        let paper = t.get("次级背景色");
        let field = interface_bg(&t);
        for key in ["跟打区字体色", "打对字色", "未打文字色"] {
            assert!(cr(t.get(key), paper) >= 2.0, "{} 在灰砖纸面上过淡", key);
            assert!(cr(t.get(key), field) >= 2.0, "{} 在灰砖底面上过淡", key);
        }
        for key in ["窗体字体色", "菜单字体色", "按钮字体色"] {
            assert!(
                cr(t.get(key), t.get("窗体背景色")) >= 2.0,
                "{} 在灰砖外框上过淡",
                key
            );
        }
        assert_ne!(t.get("次级背景色"), theme("宣纸").get("次级背景色"));
    }

    #[test]
    fn authored_palette_stays_untouched_without_overrides() {
        let t = theme_with_overrides("宣纸", &std::collections::BTreeMap::new());
        assert_eq!(
            t.get("次级背景色"),
            Color::from_hex_no_hash("efe9dd").unwrap()
        );
        assert_eq!(harmonize(&mut Theme::default(), &Default::default()), 0);
    }
}
