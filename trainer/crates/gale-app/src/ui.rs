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
/// 赛制下字提行的"停手才显示"门限（毫秒，晴口径 B：赛制词提全关、字提停手 5s 显示）。
pub const ZITI_IDLE_MS: f64 = 5000.;
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
    pub word_colors: [[f32; 4]; 4],
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
pub fn hint_word_tint(p: &Palette, text: &str, code: &str, max_len: usize) -> Option<[f32; 4]> {
    if !crate::analysis::is_multi_word(text) {
        return None;
    }
    let n = text.chars().filter(|c| c.is_alphanumeric()).count();
    let kind = if code.chars().count() < max_len.max(1) {
        0
    } else {
        match n {
            2 => 1,
            3 => 2,
            _ => 3,
        }
    };
    Some(p.word_colors[kind])
}

pub fn tokens(t: &Theme) -> Palette {
    let light = matches!(t.name.as_str(), "宣纸" | "青瓷");
    Palette {
        word_colors: [
            t.colors
                .get("词提简码色")
                .copied()
                .map(rgba)
                .unwrap_or_else(|| hex(if light { "27754b" } else { "63c08d" })),
            t.colors
                .get("词提二字全码色")
                .copied()
                .map(rgba)
                .unwrap_or_else(|| hex(if light { "a45b16" } else { "db9b55" })),
            t.colors
                .get("词提三字全码色")
                .copied()
                .map(rgba)
                .unwrap_or_else(|| hex(if light { "2769a3" } else { "70a9e5" })),
            t.colors
                .get("词提四字全码色")
                .copied()
                .map(rgba)
                .unwrap_or_else(|| hex(if light { "b63c38" } else { "df7974" })),
        ],
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
    match key {
        "词提简码色" => p.word_colors[0],
        "词提二字全码色" => p.word_colors[1],
        "词提三字全码色" => p.word_colors[2],
        "词提四字全码色" => p.word_colors[3],
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
    .clamp(24., 64.)
    .min((bw / 9.).max(24.));
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
    (text_w + if key.is_empty() { 0. } else { 34. } + 24.).max(72.)
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
    round(
        f,
        x,
        y,
        w,
        32.,
        8.,
        if selected { alpha(p.accent, 0.12) } else { p.surface2 },
        Some(if selected { alpha(p.accent, 0.42) } else { p.line }),
    );
    f.push(DrawCmd::PushClip {
        x: x + 12.,
        y: y + 8.,
        w: (w - if key.is_empty() { 24. } else { 52. }).max(8.),
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
            x + w - 38.,
            y + 7.,
            28.,
            18.,
            4.,
            p.bg2,
            Some(alpha(p.line2, 0.8)),
        );
        text(f, x + w - 34., y + 8., 12., key, p.muted);
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
    // 常用操作栏：文库退出主工具栏，让位给「在线发文」；理论分析回到窗口内。
    let y = 60.;
    let wide = w >= 960.;
    let draft = !m.pending_source.is_empty();
    let left = [
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
            if draft { "载入待读群文" } else { "读取群文" },
            if wide { "F4" } else { "" },
            if draft {
                Action::AcceptDraft
            } else {
                Action::ReadGroup
            },
            draft,
        ),
        ("重打", if wide { "F3" } else { "" }, Action::Restart, false),
    ];
    let mut xx = 24.;
    for (label, key, action, selected) in left {
        xx = toolbar_button(f, hs, xx, y, label, key, action, t, selected) + 8.;
    }
    let more_w = toolbar_width("更多", "");
    let analysis_w = toolbar_width("理论分析", if wide { "F10" } else { "" });
    let analysis_x = w - 24. - more_w - 8. - analysis_w;
    toolbar_button(
        f,
        hs,
        analysis_x,
        y,
        "理论分析",
        if wide { "F10" } else { "" },
        Action::ToggleAnalysis,
        t,
        m.analysis_dock,
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
        if m.paused {
            "已暂停"
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
        base.max(fs + cfg.hint_font_size + 18.)
    } else {
        base
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
    let single = sess.source == SessionSource::Single;
    let hints_visible = single
        && gale_core::rules::hints_effective(m.hints, m.race)
        && (cfg.hint_show_code || cfg.hint_show_rank);
    let (bw, fs, base_lh) = body_metrics(sess, cfg, w);
    let lh = line_height_for_anchor(cfg, fs, base_lh, hints_visible);
    let x = (w - bw) / 2.;
    let pos = sess.comparer.pos;
    let live = sess.live_stats(now_ms);
    let count = sess.current_segment().text.chars().count();
    let elapsed = sess.elapsed_ms(now_ms) as f64 / 1000.;
    let top = if single { 236. } else { 208. };
    // 理论分析停靠面板打开时正文区让位；成绩面板是浮层，不改变正文高度。
    let bottom = if m.analysis_dock {
        crate::modals::analysis_dock_rect(w, h).1 - 8.
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
    let speed = format!("{:.1}", live.para_speed);
    let speed_size = 30_f32.min(112. / (speed.len().max(1) as f32 * 0.6));
    mono(&mut f, x, 117., speed_size, speed.clone(), p.accent);
    let speed_w = ui_text_width(&speed, speed_size);
    text(&mut f, x + speed_w + 7., 132., 12., "字/分", p.muted);
    let duration = format!("{:02}:{:04.1}", (elapsed / 60.) as u64, elapsed % 60.);
    let mut metrics = vec![
        ("击键/秒", format!("{:.2}", live.para_keys_per_sec)),
        ("码长", format!("{:.2}", live.para_code_len)),
        ("键准", format!("{:.1}%", live.para_accuracy)),
        ("错字", sess.comparer.wrong.to_string()),
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
        "第 {} / {} {} · {} / {} 字{}",
        sess.index + 1,
        sess.total_segments(),
        if single { "组" } else { "段" },
        pos,
        count,
        if m.race { " · 赛制" } else { "" }
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
        let shuffle = m.field("single_shuffle") == "true";
        let seg = m.field("single_seg").parse::<u16>().unwrap_or(20);
        let target = m.field("single_target_keystroke");
        let target = if target.is_empty() { "3.0" } else { &target };
        let mut xx = x;
        for (label, ww, action, on) in [
            (
                if shuffle {
                    "乱序 · 开".to_string()
                } else {
                    "乱序 · 关".to_string()
                },
                92.,
                crate::aux_marker("single_shuffle"),
                shuffle,
            ),
            (
                format!("每组 {seg} 字 ↻"),
                118.,
                crate::aux_marker("single_seg_cycle"),
                false,
            ),
            (
                format!("达标 {target} 键/秒 ↻"),
                162.,
                crate::aux_marker("single_target_cycle"),
                false,
            ),
        ] {
            chip(&mut f, hs, xx, 199., ww, &label, action, &p, on);
            xx += ww + 8.;
        }
        if !m.race {
            chip(
                &mut f,
                hs,
                xx,
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
        } else {
            text(&mut f, xx, 203., FONT_HINT, "赛制关闭字提", p.muted);
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
            .saturating_sub(1)
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
            let c = match sess.comparer.states.get(char_at) {
                Some(CharState::Wrong) => p.red,
                Some(CharState::Correct) => p.correct,
                _ => p.text2,
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
        if hints_visible {
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
                if let Some(code) = &z.code {
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
                    let hx = wx
                        + (ww - (shown_code.len() + rank.len()) as f32 * hint_advance).max(0.) / 2.;
                    let code_color = alpha(p.hint_code, dim);
                    if !shown_code.is_empty() {
                        mono(&mut f, hx, yy + fs + 9., hint_size, shown_code, code_color);
                    }
                    if !rank.is_empty() {
                        mono(
                            &mut f,
                            hx + shown_code.chars().count() as f32 * hint_advance,
                            yy + fs + 5.,
                            hint_size,
                            rank,
                            alpha(p.hint_badge, dim),
                        );
                    }
                }
            }
        }
    }
    f.push(DrawCmd::PopClip);
    crate::history::draw(&mut f, hs, sess, cfg, m, t, w, h);
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
                code: Some("stale-code".into()),
                rank: 2,
                tail: ";".into(),
            }],
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
    fn article_ignores_retired_hints_and_preserves_typing_states() {
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
        assert!(!frame.cmds.iter().any(|cmd| matches!(cmd,
            DrawCmd::TextRun { text, .. } if text == "stale-code")));
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
    fn word_semantics_follow_code_length_and_single_characters_stay_neutral() {
        let p = tokens(&theme("宣纸"));
        assert_eq!(hint_word_tint(&p, "单", "a", 4), None);
        assert_eq!(hint_word_tint(&p, "……", "a", 4), None);
        assert_eq!(hint_word_tint(&p, "简词", "ab", 4), Some(p.word_colors[0]));
        assert_eq!(
            hint_word_tint(&p, "二字", "abcd", 4),
            Some(p.word_colors[1])
        );
        assert_eq!(
            hint_word_tint(&p, "三字词", "abcd", 4),
            Some(p.word_colors[2])
        );
        assert_eq!(
            hint_word_tint(&p, "四字词语", "abcd", 4),
            Some(p.word_colors[3])
        );
        assert_eq!(hint_word_tint(&p, "二字", "abc", 3), Some(p.word_colors[1]));
    }
}
