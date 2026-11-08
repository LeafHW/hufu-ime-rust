//! Quiet Room: design tokens, native text, and distinct article / drill layouts.
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
pub const FONT_HINT: f32 = 13.0;
/// 面板/头部主标签字号（标题、面板名）。
pub const FONT_LABEL: f32 = 14.0;
/// 赛制下字提行的"停手才显示"门限（毫秒，晴口径 B：赛制词提全关、字提停手 5s 显示）。
pub const ZITI_IDLE_MS: f64 = 5000.;

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
pub struct Palette {
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
pub fn tokens(t: &Theme) -> Palette {
    let light = matches!(t.name.as_str(), "宣纸" | "青瓷");
    Palette {
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
    // 浅色主题三态底换浅色系；词提编码/下划线跟各主题强调色、角标跟打错色。
    if t.name == "宣纸" || t.name == "青瓷" {
        t.set("对打未打底色", Color(0, 0, 0, 11)); // rgba(0,0,0,0.045)
        t.set("对打打对底色", Color(43, 111, 78, 36)); // rgba(43,111,78,0.14)
        t.set("对打打错底色", Color(168, 60, 46, 41)); // rgba(168,60,46,0.16)
    }
    let acc = Color::from_hex_no_hash(accent).unwrap();
    let bad = Color::from_hex_no_hash(wrong).unwrap();
    t.set("词提编码色", acc);
    t.set("词提角标色", bad);
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
    let bw = (w - 64.).min(if single { 940. } else { 920. }).max(180.);
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
// The chrome uses the resolved theme, including user color overrides.
fn draw_chrome(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    sess: &Session,
    cfg: &Config,
    m: &Model,
    t: &Theme,
    w: f32,
    h: f32,
) {
    let p = tokens(t);
    rect(f, 0., 0., w, h, p.bg);
    rect(f, 0., 0., w, 55., alpha(p.text, 0.018));
    line(f, 0., 55., w, p.line);
    round(
        f,
        22.,
        14.,
        26.,
        26.,
        7.,
        alpha(p.accent, 0.035),
        Some(alpha(p.accent, 0.35)),
    );
    for yy in [19., 35.] {
        rect(f, 30., yy, 10., 1., p.accent);
    }
    for yy in [21., 32.] {
        rect(f, 31., yy, 1., 3., p.accent);
        rect(f, 38., yy, 1., 3., p.accent);
    }
    for (dx, dy) in [(0., 0.), (1., 1.), (2., 2.), (3., 3.)] {
        rect(f, 31. + dx, 24. + dy, 1., 1., p.accent);
        rect(f, 38. - dx, 24. + dy, 1., 1., p.accent);
        rect(f, 31. + dx, 31. - dy, 1., 1., p.accent);
        rect(f, 38. - dx, 31. - dy, 1., 1., p.accent);
    }
    text(f, 57., 18., 13., "虎符", p.text);
    text(f, 94., 20., FONT_HINT, "跟打", p.muted);
    rect(f, 132., 22., 1., 13., p.line2);
    text(
        f,
        149.,
        19.,
        FONT_LABEL,
        format!(
            "{}  ·  第 {} {}",
            elide(&sess.title, if w < 1000. { 11 } else { 18 }),
            sess.index + 1,
            if sess.source == SessionSource::Single {
                "组"
            } else {
                "段"
            }
        ),
        p.text2,
    );
    if w > 900. && matches!(m.modal, Modal::Result | Modal::Report) {
        let cx = if w > 1150. { 410. } else { 345. };
        chip(
            f,
            hs,
            cx,
            8.,
            92.,
            if m.modal == Modal::Result {
                "单段结算"
            } else {
                "整场赛报"
            },
            Action::None,
            &p,
            true,
        );
        chip(
            f,
            hs,
            cx + 98.,
            8.,
            62.,
            if m.race { "赛制" } else { "练习" },
            Action::None,
            &p,
            false,
        );
        text(
            f,
            cx,
            35.,
            FONT_HINT,
            if m.race {
                "词提按规则关闭 · 成绩按赛制记录"
            } else {
                "练习成绩 · 保留规则与输入依据"
            },
            p.muted,
        );
    } else if w > 900. {
        let cx = if w > 1150. { 410. } else { 345. };
        chip(
            f,
            hs,
            cx,
            8.,
            72.,
            if m.race {
                "赛制"
            } else if sess.source == SessionSource::Single {
                "单字练习"
            } else {
                "群文练习"
            },
            Action::ToggleRace,
            &p,
            true,
        );
        chip(
            f,
            hs,
            cx + 78.,
            8.,
            78.,
            if sess.source == SessionSource::Single {
                if m.hints {
                    "字提·开"
                } else {
                    "字提·关"
                }
            } else {
                "自定义规则"
            },
            if sess.source == SessionSource::Single {
                Action::ToggleHints
            } else {
                Action::Open(Modal::Rules)
            },
            &p,
            sess.source == SessionSource::Single && m.hints,
        );
        chip(
            f,
            hs,
            cx + 162.,
            8.,
            74.,
            if sess.source == SessionSource::Single {
                "居中行"
            } else if m.race {
                "词提·锁"
            } else if m.hints {
                "词提·开"
            } else {
                "词提·关"
            },
            if sess.source == SessionSource::Single {
                Action::None
            } else {
                Action::ToggleHints
            },
            &p,
            false,
        );
        text(
            f,
            cx,
            35.,
            FONT_HINT,
            if sess.source == SessionSource::Single {
                "字提编码下显 · 尾码方点 · 选重以上标位次标记"
            } else if m.race {
                "词提按规则关闭 · 开打后冻结规则"
            } else if m.hints {
                "• 词提已开启 · 自定义规则（非赛制）"
            } else {
                "自定义规则 · 首键计时"
            },
            p.muted,
        );
    }
    if w > 1100. {
        text(
            f,
            w - 239.,
            20.,
            FONT_HINT,
            format!(
                "● {} · {}",
                t.name,
                if matches!(t.name.as_str(), "宣纸" | "青瓷") {
                    "浅色"
                } else {
                    "深色"
                }
            ),
            p.muted,
        );
    }
    for (i, (label, a)) in [
        ("—", Action::Minimize),
        ("□", Action::Maximize),
        ("×", Action::Exit),
    ]
    .into_iter()
    .enumerate()
    {
        let xx = w - 96. + i as f32 * 28.;
        text(f, xx + 7., 18., 13., label, p.muted);
        hs.push(Hit {
            x: xx,
            y: 8.,
            w: 28.,
            h: 38.,
            action: a,
        });
    }
    rect(f, 0., h - 37., w, 37., alpha(p.bg2, 0.8));
    line(f, 0., h - 37., w, p.line);
    round(f, 22., h - 20., 4., 4., 2., p.accent, None);
    let sx = (w / 2. - 154.).max(if w > 1000. { 360. } else { 230. });
    text(
        f,
        34.,
        h - 25.,
        FONT_HINT,
        elide(
            if m.paused {
                "已暂停 · 返回窗口继续"
            } else {
                &m.status
            },
            (((sx - 46.) / FONT_HINT) as usize).max(6),
        ),
        p.muted,
    );
    let result = m.modal == Modal::Result;
    let report = m.modal == Modal::Report;
    let actions: Vec<(&str, &str, Action)> = if result {
        vec![
            ("Enter", "下一段", Action::Next),
            ("R", "重打", Action::Restart),
            ("C", "复制", Action::CopyScore),
            ("S", "复制发群", Action::SendScore),
        ]
    } else if report {
        vec![
            ("C", "复制整份", Action::CopyRace),
            ("S", "复制发群", Action::CopyRace),
            ("E", "导出", Action::ExportImage),
            ("Esc", "关闭", Action::Close),
        ]
    } else {
        vec![
            ("F2", "载文", Action::Open(Modal::Load)),
            ("F6", "群", Action::Open(Modal::Groups)),
            ("F7", "外观", Action::Open(Modal::Appearance)),
            ("F8", "在线", Action::Open(Modal::Online)),
            ("F9", "规则", Action::Open(Modal::Rules)),
        ]
    };
    let spacing = if result || report { 96. } else { 61. };
    for (i, (key, label, a)) in actions.into_iter().enumerate() {
        let xx = sx + i as f32 * spacing;
        let kw = if key == "Enter" { 35. } else { 22. };
        round(
            f,
            xx,
            h - 27.,
            kw,
            16.,
            4.,
            alpha(p.text, 0.015),
            Some(p.line2),
        );
        mono(f, xx + 4., h - 24., 8.5, key, p.text2);
        text(f, xx + kw + 5., h - 25., FONT_HINT, label, p.muted);
        hs.push(Hit {
            x: xx,
            y: h - 34.,
            w: spacing - 2.,
            h: 32.,
            action: a,
        });
    }
    if w > 1100. {
        right(
            f,
            w - 22.,
            h - 25.,
            FONT_HINT,
            format!(
                "今日 {} 字   |   {} / {} 字",
                cfg.today_chars,
                sess.comparer.pos,
                sess.current_segment().text.chars().count()
            ),
            p.muted,
        );
    }
}
fn rail(f: &mut Frame, sess: &Session, p: &Palette, x: f32, y: f32, bw: f32) {
    let n = sess.total_segments().max(1);
    let visible = n.min(8);
    let first = if n > visible {
        sess.index.saturating_sub(3).min(n - visible)
    } else {
        0
    };
    let gap = 15.;
    let sw = (bw - gap * (visible - 1) as f32) / visible as f32;
    for i in first..first + visible {
        let xx = x + (i - first) as f32 * (sw + gap);
        let done = sess.results.get(i).is_some_and(Option::is_some);
        let on = i == sess.index;
        let color = if on { p.accent } else { p.muted };
        text(
            f,
            xx,
            y,
            FONT_HINT,
            format!(
                "{} · {}{}",
                i + 1,
                elide(&sess.segments[i].mark, ((sw - 46.) / FONT_HINT) as usize),
                if done { " ✓" } else { "" }
            ),
            color,
        );
        rect(f, xx, y + 19., sw, if on { 3. } else { 2. }, p.line);
        let progress = if done {
            1.
        } else if on {
            sess.comparer.pos as f32 / sess.current_segment().text.chars().count().max(1) as f32
        } else {
            0.
        };
        rect(
            f,
            xx,
            y + 19.,
            sw * progress,
            2.,
            alpha(p.accent, if on { 1. } else { 0.38 }),
        );
        if on {
            round(
                f,
                xx + sw * progress - 3.,
                y + 17.,
                7.,
                7.,
                3.5,
                p.accent,
                None,
            );
        }
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
    if matches!(m.modal, Modal::Result | Modal::Report) {
        return f;
    }
    // 词不拆行（P2）：main 的 wrap_text 是纯像素宽贪心、会切断词，且只在
    // 段/字号/宽度变化时重算（词提是异步到达的）。这里每帧按词边界把跨行
    // 词整体移到后一行；词内字符推进量取自原行的 DirectWrite 实测位置，
    // 下划线/编码提示随新行位置自动跟随。单字练习的 words 全是 len=1，
    // rewrap_words 走快路径返回 None。
    let owned_lines = if m.words.is_empty() {
        None
    } else {
        gale_win::d2d::rewrap_words(
            lines,
            &m.words
                .iter()
                .map(|z| (z.start, z.len))
                .collect::<Vec<_>>(),
        )
    };
    let lines: &[(usize, String, Vec<f32>)] = owned_lines
        .as_ref()
        .map(|v| v.as_slice())
        .unwrap_or(lines);
    let p = tokens(t);
    let single = sess.source == SessionSource::Single;
    let (bw, fs, lh) = body_metrics(sess, cfg, w);
    let x = (w - bw) / 2.;
    let pos = sess.comparer.pos;
    let live = sess.live_stats(now_ms);
    let count = sess.current_segment().text.chars().count();
    let elapsed = sess.elapsed_ms(now_ms) as f64 / 1000.;
    let top;
    // 顶栏两行式（整改 6）：第一行 = 段号/进度（FONT_LABEL+1）· 计时 · 当前速度
    // （accent 伪粗）；第二行 = 错字/退格/废码/码长/键准小指标（FONT_HINT、竖线
    // 分隔、等距分组）。窄窗（cw<720）自动省略低优先级的码长/键准。
    let head1 = 78.;
    let head2 = 104.;
    let para_title_size = FONT_LABEL + 1.;
    // 当前速度：accent + 伪粗（0.6px 偏移二次描画；D2D 无独立粗体）。
    let mut speed_mono = |f: &mut Frame, x: f32, y: f32, size: f32, s: &str, c: [f32; 4]| {
        mono(f, x + 0.6, y + 0.4, size, s, c);
        mono(f, x, y, size, s, c);
    };
    // 第二行指标组：label + 等宽值水平排布，组间竖线分隔。
    let metric_row = |f: &mut Frame, x: f32, y: f32, w: f32, p: &Palette, items: &[(String, String, bool)]| {
        let n = items.iter().filter(|z| z.2).count().max(1);
        let pitch = (w / n as f32).min(128.);
        let mut i = 0;
        for (label, value, shown) in items {
            if !shown {
                continue;
            }
            let gx = x + i as f32 * pitch;
            if i > 0 {
                rect(f, gx - 14., y - 11., 1., 16., p.line);
            }
            text(f, gx, y, FONT_HINT, label, p.muted);
            mono(f, gx + 40., y, FONT_HINT, value, p.text2);
            i += 1;
        }
    };
    if single {
        text(
            &mut f,
            x,
            head1,
            para_title_size,
            format!(
                "第 {} / {} 组 · {} · 单字练习",
                sess.index + 1,
                sess.total_segments(),
                elide(&sess.title, 18)
            ),
            p.text2,
        );
        right(
            &mut f,
            x + bw - 96.,
            head1 + 2.,
            FONT_HINT,
            format!("{:02}:{:04.1}", (elapsed / 60.) as u64, elapsed % 60.),
            p.muted,
        );
        speed_mono(&mut f, x + bw, head1 - 4., 22., &format!("{:.1}", live.para_speed), p.accent);
        right(&mut f, x + bw, head1 + 18., FONT_HINT, "字/分", p.muted);
        metric_row(
            &mut f,
            x,
            head2,
            bw.min(720.),
            &p,
            &[
                ("错字".into(), sess.comparer.wrong.to_string(), true),
                ("退格".into(), sess.comparer.backspaces.to_string(), true),
                ("废码".into(), sess.stats.waste_codes.to_string(), true),
                (
                    "码长".into(),
                    format!("{:.2}", live.para_code_len),
                    bw >= 720.,
                ),
                (
                    "键准".into(),
                    format!("{:.1}%", live.para_accuracy),
                    bw >= 720.,
                ),
            ],
        );
        top = 145.;
        let center = top + ((h - 127. - top) / 2.) - fs / 2.;
        for (seg, yy) in [
            (sess.index.checked_sub(1), center - lh),
            (
                if sess.index + 1 < sess.total_segments() {
                    Some(sess.index + 1)
                } else {
                    None
                },
                center + lh,
            ),
        ] {
            if let Some(i) = seg {
                let value = elide(&sess.segments[i].text, 20);
                body_text(
                    &mut f,
                    x,
                    yy,
                    fs,
                    value,
                    alpha(p.text2, if i < sess.index { 0.20 } else { 0.55 }),
                );
            }
        }
        line(&mut f, x, center - lh - 31., bw, alpha(p.accent, 0.15));
        text(
            &mut f,
            x,
            center - lh - 45.,
            FONT_HINT,
            "居中行 · 逐字编码",
            p.muted,
        );
        if gale_core::rules::hints_effective(m.hints, m.race) {
            let seal_x = x + bw - 90.;
            round(
                &mut f,
                seal_x,
                center - 50.,
                46.,
                74.,
                4.,
                alpha(p.red, 0.025),
                Some(alpha(p.red, 0.82)),
            );
            round(
                &mut f,
                seal_x + 4.,
                center - 46.,
                38.,
                66.,
                2.,
                [0.; 4],
                Some(alpha(p.red, 0.45)),
            );
            body_text(&mut f, seal_x + 12., center - 42., 21., "字", p.red);
            body_text(&mut f, seal_x + 12., center - 19., 21., "提", p.red);
        }
    } else {
        text(
            &mut f,
            x,
            head1,
            para_title_size,
            format!(
                "第 {} 段 · {} / {} 字 · {} · {}",
                sess.index + 1,
                pos,
                count,
                elide(&sess.current_segment().mark, 14),
                if m.race {
                    "赛制"
                } else {
                    "自定义"
                }
            ),
            p.text2,
        );
        right(
            &mut f,
            x + bw - 96.,
            head1 + 2.,
            FONT_HINT,
            format!(
                "{:02}:{:04.1}",
                (elapsed / 60.) as u64,
                elapsed % 60.
            ),
            p.muted,
        );
        speed_mono(&mut f, x + bw, head1 - 4., 22., &format!("{:.1}", live.para_speed), p.accent);
        right(&mut f, x + bw, head1 + 18., FONT_HINT, "字/分", p.muted);
        metric_row(
            &mut f,
            x,
            head2,
            bw.min(720.),
            &p,
            &[
                (
                    "错字".into(),
                    sess.comparer.wrong.to_string(),
                    true,
                ),
                ("退格".into(), sess.comparer.backspaces.to_string(), true),
                ("废码".into(), sess.stats.waste_codes.to_string(), true),
                (
                    "码长".into(),
                    format!("{:.2}", live.para_code_len),
                    bw >= 720.,
                ),
                (
                    "键准".into(),
                    format!("{:.1}%", live.para_accuracy),
                    bw >= 720.,
                ),
            ],
        );
        rail(&mut f, sess, &p, x, 128., bw);
        top = 200.;
    }
    let bottom = (h - 127.).max(top + fs + 20.);
    let active = lines.iter().rposition(|l| l.0 <= pos).unwrap_or(0);
    let rows = if single {
        3
    } else {
        ((bottom - top) / lh).floor().max(1.) as usize
    };
    let first = if single {
        active.saturating_sub(1)
    } else {
        active
            .saturating_sub(1)
            .min(lines.len().saturating_sub(rows))
    };
    let stage_y = if single {
        (top + (bottom - top) / 2. - fs / 2. - (active - first) as f32 * lh).max(top)
    } else {
        top
    };
    f.push(DrawCmd::PushClip {
        x: x - 14.,
        y: top - 12.,
        w: bw + 28.,
        h: (bottom - top + 28.).max(1.),
    });
    let hint_cur = m
        .words
        .iter()
        .position(|z| z.start.saturating_add(z.len) > pos)
        .unwrap_or(m.words.len());
    for (li, (offset, line_text, widths)) in lines.iter().enumerate().skip(first).take(rows) {
        let yy = stage_y + (li - first) as f32 * lh;
        let xx = x;
        let dim = if single {
            if li < active {
                0.2
            } else if li > active {
                0.55
            } else {
                1.
            }
        } else {
            1.
        };
        let mut dx = 0.;
        if cfg.shake_on_wrong && !m.reduced_motion && now - anim.wrong_at < 0.3 {
            let z = (now - anim.wrong_at).max(0.) as f32;
            dx = (z * 72.).sin() * 6. * (1. - z / 0.3).max(0.);
        }
        // 对打三态字符底（整改 4）：未打=极淡灰、打对=淡绿、打错=红。
        // 高=行高、宽=字符宽+2px、圆角 3px，先画底再画字符（字符色逻辑不变）。
        for (j, _ch) in line_text.chars().enumerate() {
            let bg = match sess.comparer.states.get(offset + j) {
                Some(CharState::Correct) => p.bg_done,
                Some(CharState::Wrong) => p.bg_wrong,
                _ => p.bg_pending,
            };
            let a = widths.get(j).copied().unwrap_or(0.);
            let b = widths.get(j + 1).copied().unwrap_or(a + fs);
            round(
                &mut f,
                xx + dx + a - 1.,
                yy,
                (b - a + 2.).max(4.),
                lh,
                3.,
                bg,
                None,
            );
        }
        // Word glow and drill-character focus sit behind the actual DirectWrite line.
        if gale_core::rules::hints_effective(m.hints, m.race) && cfg.hint_style != 2 {
            if single {
                if li == active {
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
            } else if let Some(z) = m.words.get(hint_cur) {
                if z.start >= *offset && z.start < offset + line_text.chars().count() {
                    let start = widths.get(z.start - offset).copied().unwrap_or(0.);
                    let end = widths
                        .get((z.start - offset + z.len).min(widths.len() - 1))
                        .copied()
                        .unwrap_or(bw);
                    // 词提底色（整改 9）：默认全透明不画；自定义非透明时画
                    // 单层衬底 + 下划线色细描边（替代原三层 accent 微光晕）。
                    if p.hint_bg[3] > 0.004 {
                        round(
                            &mut f,
                            xx + start - 12.,
                            yy - 6.,
                            end - start + 24.,
                            fs + 12.,
                            8.,
                            p.hint_bg,
                            Some(alpha(p.hint_underline, 0.25)),
                        );
                    }
                }
            }
        }
        let mut spans = Vec::new();
        let mut u16pos = 0;
        for (j, ch) in line_text.chars().enumerate() {
            let c = match sess.comparer.states.get(offset + j) {
                Some(CharState::Correct) => p.text,
                Some(CharState::Wrong) => p.red,
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
        if gale_core::rules::hints_effective(m.hints, m.race) && cfg.hint_style != 2 {
            for (zi, z) in m.words.iter().enumerate() {
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
                let show = if single || cfg.hint_style == 1 {
                    li == active
                } else {
                    zi >= hint_cur && zi < hint_cur + 3 && z.len > 1
                };
                if z.start >= pos && !single {
                    line(
                        &mut f,
                        wx + 1.,
                        yy + fs + 4.,
                        (ww - 2.).max(1.),
                        alpha(p.hint_underline, if show { 0.3 } else { 0.07 }),
                    );
                }
                if !show {
                    continue;
                }
                if let Some(code) = &z.code {
                    let hx = wx + (ww - code.chars().count() as f32 * 6.3).max(0.) / 2.;
                    let code_color = alpha(p.hint_code, if z.start <= pos { 1. } else { 0.55 });
                    // 隔词交替加粗（晴口径 B：相邻词的编码下显第一词粗、第二词常规循环）：
                    // D2D 无独立粗体 Mono，用 0.6px 偏移二次描画模拟。
                    if zi % 2 == 0 {
                        mono(&mut f, hx + 0.6, yy + fs + 9., 10.5, code.clone(), code_color);
                    }
                    mono(&mut f, hx, yy + fs + 9., 10.5, code, code_color);
                    if z.rank > 1 {
                        mono(
                            &mut f,
                            hx + code.len() as f32 * 6.3,
                            yy + fs + 5.,
                            7.,
                            z.rank.to_string(),
                            p.hint_badge,
                        );
                    }
                } else if !z.tail.is_empty() && z.tail != "_" {
                    // 尾码圆点角标：与完整编码下显互斥（有下显不画），且
                    // tail=="_"（空格上屏）不画。
                    round(
                        &mut f,
                        wx + ww - 3.,
                        yy + 1.,
                        3.5,
                        3.5,
                        0.5,
                        [0.; 4],
                        Some(alpha(p.hint_badge, 0.6)),
                    );
                }
            }
        }
    }
    f.push(DrawCmd::PopClip);
    let ly = if single {
        (top + (bottom - top) / 2. - fs / 2. + lh + fs + 34.).min(h - 128.)
    } else {
        bottom + 34.
    };
    text(
        &mut f,
        x,
        ly,
        FONT_HINT,
        if single {
            "字提示色"
        } else {
            "词提示色"
        },
        p.muted,
    );
    // 词提配色图例（整改 9）：四色全部来自主题（编码下显/角标尾码/下划线/底色）。
    for (i, (name, c, hollow)) in [
        ("编码下显", p.hint_code, false),
        ("角标 · 选重位次", p.hint_badge, false),
        ("下划线", p.hint_underline, false),
        ("词底 · 默认透明", p.hint_bg, true),
    ]
    .into_iter()
    .enumerate()
    {
        let xx = x + 70. + i as f32 * 108.;
        if hollow && c[3] <= 0.004 {
            round(&mut f, xx, ly + 4., 5., 5., 1., [0.; 4], Some(p.muted));
        } else {
            round(&mut f, xx, ly + 4., 5., 5., 1., c, None);
        }
        text(&mut f, xx + 10., ly, FONT_HINT, name, p.muted);
    }
    if single {
        line(&mut f, x, h - 75., bw, p.line);
        body_text(
            &mut f,
            x,
            h - 63.,
            FONT_HINT,
            format!("「{}」", elide(&sess.current_segment().text, 18)),
            p.muted,
        );
        let total = sess.total_segments();
        let dots = total.min(18);
        let first = sess.index.saturating_sub(8).min(total.saturating_sub(dots));
        let dx = x + bw - 160. - dots as f32 * 8.;
        for (j, i) in (first..first + dots).enumerate() {
            round(
                &mut f,
                dx + j as f32 * 8.,
                h - 57.,
                if i == sess.index { 5. } else { 4. },
                4.,
                2.,
                if i == sess.index {
                    p.accent
                } else {
                    alpha(p.muted, 0.35)
                },
                None,
            );
        }
        right(
            &mut f,
            x + bw,
            h - 63.,
            FONT_HINT,
            format!("第 {}/{} 组", sess.index + 1, total),
            p.muted,
        );
    }
    // 字提行口径（research-digest §B 晴口径）：赛制=词提全关（hints_effective==false），
    // 字提是唯一允许的提示形态，但需**停手 5 秒**才显示（开打前不显示，防起跑泄题）；
    // 练习=字提跟词提同开同关、立即显示。
    let idle_ms = (now - anim.last_input).max(0.) * 1000.;
    let ziti_visible = if m.race {
        sess.started_ms.is_some() && idle_ms >= ZITI_IDLE_MS
    } else {
        gale_core::rules::hints_effective(m.hints, false)
    };
    if ziti_visible {
        if let Some(z) = m.words.get(hint_cur) {
            right(
                &mut f,
                x + bw,
                ly,
                FONT_HINT,
                format!(
                    "当前{}  {}  ·  {}{}",
                    if single { "字" } else { "词" },
                    z.text,
                    z.code.clone().unwrap_or_else(|| "—".into()),
                    if z.tail == "_" {
                        " · 空格上屏"
                    } else {
                        ""
                    }
                ),
                p.text2,
            );
        } else if !m.status.is_empty() && w > 1000. {
            right(
                &mut f,
                x + bw,
                ly,
                FONT_HINT,
                if m.race {
                    "字提需当前虎符服务支持"
                } else if m.hints {
                    "提示需当前虎符服务支持"
                } else {
                    "提示已关闭"
                },
                p.muted,
            );
        }
    } else if !m.status.is_empty() && w > 1000. && !m.race {
        right(
            &mut f,
            x + bw,
            ly,
            FONT_HINT,
            if m.hints {
                "提示需当前虎符服务支持"
            } else {
                "提示已关闭"
            },
            p.muted,
        );
    }
    f
}
