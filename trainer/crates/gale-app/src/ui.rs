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
use gale_win::d2d::{Align, DrawCmd, FontFamily, Frame};

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
fn word_bracket_offset(size: f32) -> f32 {
    // WenKai's visible glyphs can extend below the nominal em box at large sizes.
    (size * 0.24).clamp(10., 24.)
}
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
    pub green: [f32; 4],
    pub copper: [f32; 4],
    pub jade: [f32; 4],
    /// 对打三态字符底（整改 4）：未打 / 打对 / 打错。
    pub bg_pending: [f32; 4],
    pub bg_done: [f32; 4],
    pub bg_wrong: [f32; 4],
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

pub fn tokens(t: &Theme) -> Palette {
    let light = matches!(t.name.as_str(), "宣纸" | "青瓷");
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
        article_bg: rgba(t.get("跟打区背景色")),
        correct: rgba(t.get("打对字色")),
        bg: rgba(t.get("窗体背景色")),
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
        hint_code: rgba(t.get("词提编码色")),
        hint_badge: rgba(t.get("词提角标色")),
        hint_underline: rgba(t.get("词提下划线色")),
        hint_bg: rgba(t.get("词提底色")),
    }
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
    text(f, x + 12., y + 8., FONT_HINT, label, hex("211a10"));
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
        _ => (
            "131417", "17181c", "1a1d22", "20242a", "2a2e36", "383d47", "f0ede6", "c6c1b5",
            "9d978b", "e7b75c", "e06a5a",
        ),
    };
    let mut t = Theme::default();
    t.name = if name == "default" || name.is_empty() {
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
    if t.name == "宣纸" || t.name == "青瓷" {
        t.set("对打未打底色", Color(0, 0, 0, 11)); // rgba(0,0,0,0.045)
        t.set("对打打对底色", Color(43, 111, 78, 36)); // rgba(43,111,78,0.14)
        t.set("对打打错底色", Color(168, 60, 46, 41)); // rgba(168,60,46,0.16)
    }
    let acc = Color::from_hex_no_hash(accent).unwrap();
    t.set("词提编码色", Color::from_hex_no_hash(fg2).unwrap());
    t.set("词提角标色", Color::from_hex_no_hash(fg2).unwrap());
    t.set("词提下划线色", acc);
    // 词提底色保持 Theme::default 的全透明（显式自定义时才会画底）。
    t
}
#[derive(Default)]
pub struct Animation {
    pub caret: Option<(f32, f32)>,
    pub last_input: f64,
    pub wrong_at: f64,
    pub correct_at: f64,
}
impl Animation {
    pub fn active(&self, n: f64) -> bool {
        n - self.last_input < 0.35 || n - self.wrong_at < 0.3 || n - self.correct_at < 0.42
    }
}
pub fn body_metrics(sess: &Session, cfg: &Config, w: f32) -> (f32, f32, f32) {
    let single = sess.source == SessionSource::Single;
    let bw = (w - 64.).min(if single { 960. } else { 1000. }).max(180.);
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
            (1. + cfg.line_gap).clamp(1.4, 2.2)
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
fn draw_chrome(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    sess: &Session,
    _cfg: &Config,
    m: &Model,
    t: &Theme,
    w: f32,
    h: f32,
) {
    let p = tokens(t);
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
    let left = [
        (
            "从剪贴板载入",
            if wide { "Ctrl+V" } else { "" },
            Action::LoadClipboard,
            false,
        ),
        (
            "在线发文",
            if wide { "F8" } else { "" },
            Action::Open(Modal::Online),
            false,
        ),
        (
            "选群",
            if wide { "F5" } else { "" },
            Action::Open(Modal::Groups),
            false,
        ),
        (
            "读取群文",
            if wide { "F4" } else { "" },
            Action::ReadGroup,
            false,
        ),
        ("重打", if wide { "F3" } else { "" }, Action::Restart, false),
    ];
    let mut xx = 24.;
    for (label, key, action, selected) in left {
        if compact && matches!(action, Action::Open(Modal::Groups) | Action::Restart) {
            continue;
        }
        xx = toolbar_button(f, hs, xx, y, label, key, action, t, selected) + 8.;
    }
    let more_w = toolbar_width("更多", "");
    let settings_w = toolbar_width("设置", "F1");
    let analysis_w = toolbar_width("理论分析", "F10");
    let settings_x = w - 24. - more_w - 8. - settings_w;
    let analysis_x = settings_x - 8. - analysis_w;
    toolbar_button(
        f,
        hs,
        analysis_x,
        y,
        "理论分析",
        "F10",
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
        "F1",
        Action::Open(Modal::Preferences),
        t,
        m.modal == Modal::Preferences,
    );
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
/// Effective article line height. Only an actually visible hint row (codes or
/// selection digits) may grow the line; turning both off should reclaim the
/// space instead of keeping an empty code strip.
pub fn line_height_for_anchor(cfg: &Config, fs: f32, base: f32, hints_visible: bool) -> f32 {
    if hints_visible && (cfg.hint_show_code || cfg.hint_show_rank) {
        // Reserve the bracket and its code row together, including small fonts.
        base.max(fs + word_bracket_offset(fs) + cfg.hint_font_size + 10.)
    } else {
        base.max(fs + word_bracket_offset(fs) + 6.)
    }
}
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
    let mut f = Frame::new(w, h);
    hs.clear();
    draw_chrome(&mut f, hs, sess, cfg, m, t, w, h);
    let p = tokens(t);
    if sess.current_segment().text.is_empty() {
        text(&mut f, 32., 136., 20., "载入文章后开始跟打", p.text);
        text(
            &mut f,
            32.,
            172.,
            FONT_HINT,
            "点击「从剪贴板载入」或按 Ctrl+V，也可在线取文、读取群文。",
            p.muted,
        );
        if !(m.analysis_dock && h < 520.) {
            crate::history::draw(&mut f, hs, sess, cfg, m, t, w, h);
        }
        if m.analysis_dock && m.modal == Modal::None {
            crate::modals::draw_analysis_dock(&mut f, hs, m, sess, t, w, h);
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
    let compact_analysis = m.analysis_dock && h < 700.;
    let top = if compact_analysis {
        100.
    } else if single {
        236.
    } else {
        208.
    };
    // 理论分析停靠面板打开时正文区让位；成绩面板是浮层，不改变正文高度。
    let bottom = if m.analysis_dock {
        crate::modals::analysis_dock_rect(w, h).1 - 8.
    } else if m.ledger_open {
        crate::history::panel_rect_for(m, w, h).1 - 8.
    } else {
        ledger_bar_y(h) - 8.
    };
    rect(
        &mut f,
        x - 12.,
        top - 8.,
        bw + 24.,
        (bottom - top + 16.).max(1.),
        p.article_bg,
    );
    if !compact_analysis {
        let speed = format!("{:.1}", header.speed);
        let speed_size = 30_f32.min(112. / (speed.len().max(1) as f32 * 0.6));
        mono(&mut f, x, 117., speed_size, speed.clone(), p.accent);
        let speed_w = ui_text_width(&speed, speed_size);
        text(&mut f, x + speed_w + 7., 132., 12., "字/分", p.muted);
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
        let metrics_x = x + (speed_w + 74.).max(196.);
        let metrics_w = (bw - (metrics_x - x)).max(300.);
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
            elide_width(&progress, if w < 900. { bw - 116. } else { bw }, FONT_HINT),
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
            chip(
                &mut f,
                hs,
                x,
                199.,
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
    let active = lines.iter().rposition(|l| l.0 <= pos).unwrap_or(0);
    let rows = ((bottom - top) / lh).floor().max(1.) as usize;
    let first = if single {
        active
            .saturating_sub(rows / 2)
            .min(lines.len().saturating_sub(rows))
    } else {
        active
            .saturating_sub(rows.saturating_sub(1).min(1))
            .min(lines.len().saturating_sub(rows))
    };
    let visible_rows = lines.len().saturating_sub(first).min(rows);
    let stage_y = if single {
        top + ((bottom - top) - visible_rows as f32 * lh).max(0.) / 2.
    } else {
        top
    };
    f.push(DrawCmd::PushClip {
        x: x - 14.,
        y: top - 4.,
        w: bw + 28.,
        h: (bottom - top + 4.).max(1.),
    });
    for (li, (offset, line_text, widths)) in lines.iter().enumerate().skip(first).take(rows) {
        let yy = stage_y + (li - first) as f32 * lh;
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
        let mut j = 0;
        while j < line_text.chars().count() {
            let state = sess.comparer.states.get(offset + j);
            let end = (j + 1..line_text.chars().count())
                .find(|k| sess.comparer.states.get(offset + k) != state)
                .unwrap_or(line_text.chars().count());
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
        if hints_visible && li == active {
            let ci = pos
                .saturating_sub(*offset)
                .min(widths.len().saturating_sub(1));
            let cx = xx + widths[ci];
            round(
                &mut f,
                cx - 1.,
                yy + 3.,
                fs + 2.,
                fs + 5.,
                4.,
                alpha(p.accent, 0.10),
                Some(alpha(p.accent, 0.22)),
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
            let c = match sess.comparer.states.get(char_at) {
                Some(CharState::Wrong) => p.red,
                Some(CharState::Correct) => p.correct,
                _ => tint.unwrap_or(p.text2),
            };
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
        f.push(DrawCmd::StyledLine {
            x: xx + dx,
            y: yy,
            w: bw,
            size: fs,
            text: line_text.clone(),
            spans,
            color: p.text,
        });
        if li == active {
            let ci = pos
                .saturating_sub(*offset)
                .min(widths.len().saturating_sub(1));
            let target = (xx + widths[ci], yy);
            let caret = if m.reduced_motion || !cfg.smooth_caret {
                target
            } else {
                let old = anim.caret.unwrap_or(target);
                if (old.1 - target.1).abs() > lh * 1.5 {
                    target
                } else {
                    (
                        old.0 + (target.0 - old.0) * 0.55,
                        old.1 + (target.1 - old.1) * 0.55,
                    )
                }
            };
            anim.caret = Some(caret);
            rect(
                &mut f,
                caret.0 - 4.,
                caret.1 + 5.,
                2.,
                fs + 2.,
                if single { p.red } else { p.accent },
            );
            if !sess.preedit.is_empty() {
                round(
                    &mut f,
                    caret.0,
                    yy + fs + 18.,
                    (sess.preedit.chars().count() as f32 * 9. + 12.).min(bw),
                    22.,
                    4.,
                    p.surface2,
                    Some(p.line),
                );
                mono(
                    &mut f,
                    caret.0 + 5.,
                    yy + fs + 21.,
                    12.,
                    elide(&sess.preedit, 32),
                    p.accent,
                );
            }
        }
        if word_hints {
            for z in &m.words {
                if z.start < *offset || z.start >= offset + line_text.chars().count() {
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
                    let hint_advance = hint_size * 0.6;
                    let code_width = shown_code.chars().count() as f32 * hint_advance;
                    let rank_size = hint_size * 0.78;
                    let rank_width = rank.chars().count() as f32 * rank_size * 0.6;
                    let code_rank_gap = if code_width > 0. && rank_width > 0. {
                        2.
                    } else {
                        0.
                    };
                    let hx = wx + (ww - code_width - code_rank_gap - rank_width).max(0.) / 2.;
                    let code_color = alpha(tint.unwrap_or(p.hint_code), dim);
                    if !shown_code.is_empty() {
                        mono(
                            &mut f,
                            hx,
                            yy + fs + word_bracket_offset(fs) + 5.,
                            hint_size,
                            shown_code,
                            code_color,
                        );
                    }
                    if !rank.is_empty() {
                        mono(
                            &mut f,
                            hx + code_width + code_rank_gap,
                            yy + fs + word_bracket_offset(fs) + 1.,
                            rank_size,
                            rank,
                            alpha(
                                if cfg.hint_rank_colors {
                                    tint.unwrap_or(p.hint_badge)
                                } else {
                                    p.hint_badge
                                },
                                dim,
                            ),
                        );
                    }
                }
            }
        }
    }
    f.push(DrawCmd::PopClip);
    if !(m.analysis_dock && h < 520.) {
        crate::history::draw(&mut f, hs, sess, cfg, m, t, w, h);
    }
    if m.analysis_dock && m.modal == Modal::None {
        crate::modals::draw_analysis_dock(&mut f, hs, m, sess, t, w, h);
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modals::HintWord;
    use gale_core::segment::{ParsedGroupText, Segment};

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
            ["300.0", "5.00", "1.00", "100.0%", "1", "00:02.0"]
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
            ["0.0", "1.00", "0.00", "100.0%", "0", "00:01.0"]
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
        for size in [10., 36., 100.] {
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
    }

    #[test]
    fn article_word_hints_preserve_typing_states_and_leave_single_characters_neutral() {
        let (mut sess, model, cfg, lines) = fixture();
        sess.comparer.states[0] = CharState::Correct;
        sess.comparer.states[1] = CharState::Wrong;
        let theme = theme("墨玉");
        let p = tokens(&theme);
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
        assert_eq!(spans[0].2, p.correct);
        assert_eq!(spans[1].2, p.red);
        assert_eq!(spans[2].2, p.text2);
        assert!(frame.cmds.iter().any(|cmd| matches!(cmd,
            DrawCmd::TextRun { text, .. } if text == "abcd")));

        let (sess, mut model, cfg, lines) = fixture();
        let draw = |model: &Model| {
            build(
                &sess,
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
        let frame = draw(&model);
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
        let frame = draw(&model);
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
                for (word, code) in [("单", "a"), ("……", "ab"), ("词，词", "a"), ("缺词", "")]
                {
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
            assert!(caret_y >= 208. && caret_y + 107. <= ledger_bar_y(h) - 8.);
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
            assert!(anim.caret.unwrap().1 + 107. < bottom);
            assert!(
                frame.cmds.iter().any(|cmd| matches!(cmd,
                    DrawCmd::TextRun { text, y, h, .. } if text == "abcd" && y + h <= bottom
                )),
                "the whole hint row must fit above the dock"
            );
        }
    }

    #[test]
    fn word_semantics_follow_six_classes_and_single_characters_stay_neutral() {
        let p = tokens(&theme("宣纸"));
        assert_eq!(hint_word_tint(&p, "单", "a", 4), None);
        assert_eq!(hint_word_tint(&p, "……", "a", 4), None);
        assert_eq!(hint_word_tint(&p, "词，词", "a", 4), None);
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
}
