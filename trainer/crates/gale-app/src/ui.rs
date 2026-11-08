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
pub const FONT_HINT: f32 = 14.0;
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
    text(f, 57., 18., FONT_LABEL, "虎符", p.text);
    text(f, 94., 20., FONT_HINT, "跟打", p.muted);
    rect(f, 132., 22., 1., 13., p.line2);
    // The suffix ("· 第 N 段") belongs to the same width budget as the title;
    // clipping only the title let long article names run into the mode chips or
    // get chopped mid-glyph at the clip edge.
    let title_w = if w > 900. {
        if w > 1150. {
            247.
        } else {
            182.
        }
    } else {
        (w - 260.).max(20.)
    };
    f.push(DrawCmd::PushClip {
        x: 149.,
        y: 8.,
        w: title_w,
        h: 40.,
    });
    let title = &sess.title;
    let para = sess.index + 1;
    let unit = if sess.source == SessionSource::Single {
        "组"
    } else {
        "段"
    };
    let suffix = format!("  ·  第 {para} {unit}");
    let budget = ((title_w / FONT_LABEL) as usize)
        .saturating_sub(suffix.chars().count() + 1)
        .max(4);
    text(
        f,
        149.,
        19.,
        FONT_LABEL,
        format!("{}{}", elide(title, budget), suffix),
        p.text2,
    );
    f.push(DrawCmd::PopClip);
    if w > 900. && m.modal == Modal::Report {
        let cx = if w > 1150. { 410. } else { 345. };
        chip(
            f,
            hs,
            cx,
            8.,
            92.,
            "整场赛报",
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
            } else {
                // 【减法 2026-11-09】词提已下线：文章模式这枚芯片改作发文入口。
                "发文"
            },
            if sess.source == SessionSource::Single {
                Action::None
            } else {
                Action::PostArticle
            },
            &p,
            false,
        );
        // 【群三件套】读群文被打断/延迟时，在练习屏给出一键载入入口。
        if sess.source != SessionSource::Single && !m.pending_source.is_empty() {
            chip(
                f,
                hs,
                cx + 240.,
                8.,
                150.,
                "待读群文 · 载入",
                Action::AcceptDraft,
                &p,
                true,
            );
        }
        if sess.source == SessionSource::Single {
            // 单字练习屏快捷设置行（2026-11-09）：乱序/段长/达标击力直接在练习屏
            // 可调——此前只藏在文库弹窗，用户找不到入口。乱序与段长进下一段生效，
            // 达标阈值即时生效（结算判定读 single_cfg）。
            let shuffle = m.field("single_shuffle") == "true";
            let seg: u16 = m.field("single_seg").parse().unwrap_or(20);
            let target = m.field("single_target_keystroke");
            chip(
                f,
                hs,
                cx,
                30.,
                92.,
                if shuffle { "乱序 · 开" } else { "乱序 · 关" },
                crate::aux_marker("single_shuffle"),
                &p,
                shuffle,
            );
            chip(
                f,
                hs,
                cx + 98.,
                30.,
                108.,
                format!("每段 {seg} 字 ⟳").as_str(),
                crate::aux_marker("single_seg_cycle"),
                &p,
                false,
            );
            chip(
                f,
                hs,
                cx + 212.,
                30.,
                122.,
                format!("达标击力 {target} ⟳").as_str(),
                crate::aux_marker("single_target_cycle"),
                &p,
                false,
            );
            text(
                f,
                cx + 342.,
                35.,
                FONT_HINT,
                "乱序/段长下段生效 · 击力即时 · 乱序关=原序",
                p.muted,
            );
        } else {
            text(
                f,
                cx,
                35.,
                FONT_HINT,
                if m.race {
                    "词提按规则关闭 · 开打后冻结规则"
                } else if m.hints {
                    "• 词提已开启 · 自定义规则（非赛制）"
                } else {
                    "自定义规则 · 首键计时"
                },
                p.muted,
            );
        }
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
    // Left status dot used to sit on top of the first keycap.
    round(f, 8., h - 20., 4., 4., 2., p.accent, None);
    let sx = 20.;
    if w > 1250. {
        text(
            f,
            700.,
            h - 28.,
            FONT_HINT,
            elide(
                if m.paused {
                    "已暂停 · 返回窗口继续"
                } else {
                    &m.status
                },
                ((w - 1050.) / FONT_HINT).max(6.) as usize,
            ),
            p.muted,
        );
    }
    let report = m.modal == Modal::Report;
    let actions: Vec<(&str, &str, Action)> = if report {
        vec![
            ("C", "复制整份", Action::CopyRace),
            ("S", "复制发群", Action::CopyRace),
            ("E", "导出", Action::ExportImage),
            ("Esc", "关闭", Action::Close),
        ]
    } else {
        // 【减法定稿 2026-11-09】F2 载文退役：本地文库/单字进「文库」面板，
        // 成绩记录进「赛报」面板，全部按钮化，不再占热键。
        vec![
            ("F5", "选群", Action::Open(Modal::Groups)),
            ("F1", "设置", Action::Open(Modal::Preferences)),
            ("", "赛报", Action::Open(Modal::Report)),
            ("", "文库", Action::Open(Modal::Library)),
            ("F7", "外观", Action::Open(Modal::Appearance)),
            ("F8", "在线", Action::Open(Modal::Online)),
            ("F9", "规则", Action::Open(Modal::Rules)),
            ("F10", "分析", crate::aux_marker("show_full_analysis")),
        ]
    };
    let spacing = if report { 112. } else { ((w - 180.) / 8.).clamp(70., 106.) };
    for (i, (key, label, a)) in actions.into_iter().enumerate() {
        let xx = sx + i as f32 * spacing;
        let kw = if key == "Enter" {
            46.
        } else if key.chars().count() >= 3 {
            34.
        } else {
            29.
        };
        round(
            f,
            xx,
            h - 27.,
            kw,
            22.,
            4.,
            alpha(p.text, 0.015),
            Some(p.line2),
        );
        // Centre the key label inside its cap; the old fixed +3 offset left
        // F10/Enter labels visibly off-centre on narrow caps.
        f.push(DrawCmd::TextRun {
            x: xx + kw / 2.,
            y: h - 26.,
            h: 14.,
            text: key.to_string(),
            color: p.text2,
            font: FontFamily::Mono,
            align: Align::Center,
        });
        text(f, xx + kw + 5., h - 26., FONT_HINT, label, p.muted);
        hs.push(Hit {
            x: xx,
            y: h - 34.,
            w: spacing - 2.,
            h: 32.,
            action: a,
        });
    }
    if w > 1500. {
        right(
            f,
            w - 22.,
            h - 28.,
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
    if m.modal == Modal::Report {
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
            &m.words.iter().map(|z| (z.start, z.len)).collect::<Vec<_>>(),
        )
    };
    let lines: &[(usize, String, Vec<f32>)] =
        owned_lines.as_ref().map(|v| v.as_slice()).unwrap_or(lines);
    let p = tokens(t);
    if m.history_expanded && h < 680. && m.modal == Modal::None {
        text(&mut f, 24., 78., 16., "成绩记录 · 收起后返回正文", p.text2);
        crate::history::draw(&mut f, hs, sess, cfg, m, t, w, h);
        return f;
    }
    let hints_visible = gale_core::rules::hints_effective(m.hints, m.race) && cfg.hint_style != 2;
    let word_colors: Vec<Option<[f32; 4]>> = m
        .words
        .iter()
        .map(|word| {
            if hints_visible && cfg.hint_color_words {
                word.code.as_deref().and_then(|code| {
                    hint_word_tint(
                        &p,
                        &word.text,
                        code,
                        if m.max_code_length == 0 {
                            4
                        } else {
                            m.max_code_length
                        },
                    )
                })
            } else {
                None
            }
        })
        .collect();
    let single = sess.source == SessionSource::Single;
    let (bw, fs, base_lh) = body_metrics(sess, cfg, w);
    let lh = line_height_for_anchor(cfg, fs, base_lh, hints_visible);
    let x = (w - bw) / 2.;
    let pos = sess.comparer.pos;
    let live = sess.live_stats(now_ms);
    let count = sess.current_segment().text.chars().count();
    let elapsed = sess.elapsed_ms(now_ms) as f64 / 1000.;
    // 顶栏两行式（整改 6）：第一行 = 段号/进度（FONT_LABEL+1）· 计时 · 当前速度
    // （accent 伪粗）；第二行 = 错字/退格/废码/码长/键准小指标（FONT_HINT、竖线
    // 分隔、等距分组）。窄窗（cw<720）自动省略低优先级的码长/键准。
    let head1 = 78.;
    let head2 = 104.;
    let para_title_size = FONT_LABEL + 1.;
    // 当前速度：accent + 伪粗（0.6px 偏移二次描画；D2D 无独立粗体）。
    let speed_mono = |f: &mut Frame, x: f32, y: f32, size: f32, s: &str, c: [f32; 4]| {
        mono(f, x + 0.6, y + 0.4, size, s, c);
        mono(f, x, y, size, s, c);
    };
    // 第二行指标组：label + 等宽值水平排布，组间竖线分隔。
    let metric_row =
        |f: &mut Frame, x: f32, y: f32, w: f32, p: &Palette, items: &[(String, String, bool)]| {
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
                mono(f, gx + 40., y - 1., 16., value, p.text2);
                i += 1;
            }
        };
    let top = if single { 165. } else { 200. };
    let history_h = if m.history_expanded { 192. } else { 0. };
    let completion_h = if sess.phase == crate::session::Phase::Done {
        48.
    } else {
        0.
    };
    let bottom = (h - 127. - history_h - completion_h).max(top + fs + 20.);
    let ledger_top = h - 78. - history_h;
    // Single drills at the 800x600 minimum cannot fit a previous line, the
    // "居中行" label and the group footer without overlapping the metrics.
    // Keep the current line, drop the decoration.
    let tight_single = single && (bottom - top) < lh * 2.6 + 30.;
    // The article backdrop must be painted before faded neighbouring rows,
    // otherwise it covers the previous/next line in single-character drill.
    rect(
        &mut f,
        x - 14.,
        top - 12.,
        bw + 28.,
        (bottom - top + 28.).max(1.),
        p.article_bg,
    );
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
        speed_mono(
            &mut f,
            x + bw,
            head1 - 4.,
            22.,
            &format!("{:.1}", live.para_speed),
            p.accent,
        );
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
        text(
            &mut f,
            x,
            132.,
            14.,
            format!(
                "击键目标 ≥ {} 键/秒 · 当前 {:.2}{}",
                if m.field("single_target_keystroke").is_empty() {
                    "3.0".into()
                } else {
                    m.field("single_target_keystroke")
                },
                live.para_keys_per_sec,
                if sess.phase == crate::session::Phase::Done {
                    format!(" · {}", m.field("single_target_status"))
                } else {
                    String::new()
                }
            ),
            p.muted,
        );
        let center = top + ((h - 127. - top) / 2.) - fs / 2.;
        if !tight_single {
            line(&mut f, x, center - lh - 31., bw, alpha(p.accent, 0.15));
            text(
                &mut f,
                x,
                center - lh - 45.,
                FONT_HINT,
                "居中行 · 逐字编码",
                p.muted,
            );
        }
        if gale_core::rules::hints_effective(m.hints, m.race) {
            let seal_x = x + bw - 58.;
            let seal_y = top + 4.;
            round(
                &mut f,
                seal_x,
                seal_y,
                46.,
                74.,
                4.,
                alpha(p.red, 0.025),
                Some(alpha(p.red, 0.82)),
            );
            round(
                &mut f,
                seal_x + 4.,
                seal_y + 4.,
                38.,
                66.,
                2.,
                [0.; 4],
                Some(alpha(p.red, 0.45)),
            );
            body_text(&mut f, seal_x + 12., seal_y + 8., 21., "字", p.red);
            body_text(&mut f, seal_x + 12., seal_y + 31., 21., "提", p.red);
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
                if m.race { "赛制" } else { "自定义" }
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
        speed_mono(
            &mut f,
            x + bw,
            head1 - 4.,
            22.,
            &format!("{:.1}", live.para_speed),
            p.accent,
        );
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
        rail(&mut f, sess, &p, x, 128., bw);
    }
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
        if lines.len() > 1 {
            // A 20-character drill group can wrap to two rows. Center the
            // whole block; otherwise the second row lands on the neighbour.
            (top + (bottom - top - lines.len() as f32 * lh) / 2. - (active - first) as f32 * lh)
                .max(top)
        } else {
            (top + (bottom - top) / 2. - fs / 2. - (active - first) as f32 * lh).max(top)
        }
    } else if lines.len() <= rows {
        // Short articles sit in the optical centre of the practice surface
        // instead of hugging the header and leaving a large empty bottom.
        top + ((bottom - top) - lines.len() as f32 * lh).max(0.) / 2.
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
                _ => None,
            };
            if let Some(bg) = bg {
                let a = widths.get(j).copied().unwrap_or(0.);
                let b = widths.get(end).copied().unwrap_or(a + fs);
                rect(&mut f, xx + dx + a, yy, b - a, fs + 6., bg);
            }
            j = end;
        }
        // Word glow and drill-character focus sit behind the actual DirectWrite line.
        if hints_visible {
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
            } else if let Some(z) = m
                .words
                .get(hint_cur)
                .filter(|z| z.code.is_some() && z.len > 1)
            {
                if z.start >= *offset && z.start < offset + line_text.chars().count() {
                    let start = widths.get(z.start - offset).copied().unwrap_or(0.);
                    let end = widths
                        .get((z.start - offset + z.len).min(widths.len() - 1))
                        .copied()
                        .unwrap_or(bw);
                    // 词提底色（整改 9）：默认全透明不画；自定义非透明时画
                    // 单层衬底 + 下划线色细描边（替代原三层 accent 微光晕）。
                    if cfg.hint_word_bg && p.hint_bg[3] > 0.004 {
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
        let mut word_at = m
            .words
            .partition_point(|z| z.start.saturating_add(z.len) <= *offset);
        for (j, ch) in line_text.chars().enumerate() {
            let char_at = offset + j;
            while word_at < m.words.len()
                && m.words[word_at].start.saturating_add(m.words[word_at].len) <= char_at
            {
                word_at += 1;
            }
            let word_color = m
                .words
                .get(word_at)
                .filter(|z| z.start <= char_at)
                .and_then(|_| word_colors[word_at]);
            let c = match sess.comparer.states.get(char_at) {
                Some(CharState::Wrong) => p.red,
                Some(CharState::Correct) => {
                    if cfg.color_overrides.contains_key("打对字色") {
                        p.correct
                    } else {
                        word_color.unwrap_or(p.correct)
                    }
                }
                _ => word_color.unwrap_or(p.text2),
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
                let word_color = word_colors[zi];
                let hint_color = word_color.unwrap_or(p.text2);
                if cfg.hint_underline
                    && z.start >= pos
                    && !single
                    && crate::analysis::is_multi_word(&z.text)
                {
                    line(
                        &mut f,
                        wx + 1.,
                        yy + fs + 4.,
                        (ww - 2.).max(1.),
                        alpha(hint_color, 0.3),
                    );
                }
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
                    let code_color = alpha(word_color.unwrap_or(p.hint_code), dim);
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
                            alpha(word_color.unwrap_or(p.hint_badge), dim),
                        );
                    }
                }
            }
        }
    }
    f.push(DrawCmd::PopClip);
    let ly = if single {
        if m.history_expanded {
            bottom + 18.
        } else {
            (top + (bottom - top) / 2. - fs / 2. + lh + fs + 34.).min(ledger_top - 110.)
        }
    } else {
        bottom + 18.
    };
    if !tight_single {
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
        text(
            &mut f,
            x + 70.,
            ly,
            FONT_HINT,
            "词色按码长与词长区分 · F1 自定配色",
            p.muted,
        );
    }
    if single && !m.history_expanded && !tight_single {
        line(&mut f, x, ledger_top - 44., bw, p.line);
        body_text(
            &mut f,
            x,
            ledger_top - 32.,
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
                ledger_top - 12.,
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
            ledger_top - 32.,
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
    if ziti_visible && w >= 1060. {
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
                    if cfg.hint_show_code {
                        z.code.clone().unwrap_or_else(|| "—".into())
                    } else {
                        String::new()
                    },
                    if cfg.hint_show_rank && z.rank > 1 {
                        format!(" · 选重{}", z.rank)
                    } else if cfg.hint_show_code && z.tail == "_" {
                        " · 空格上屏".into()
                    } else {
                        String::new()
                    }
                ),
                word_colors
                    .get(hint_cur)
                    .copied()
                    .flatten()
                    .unwrap_or(p.text2),
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
    crate::history::draw(&mut f, hs, sess, cfg, m, t, w, h);
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modals::HintWord;
    use gale_core::segment::{ParsedGroupText, Segment};

    #[test]
    fn article_word_colors_and_full_hints_preserve_states_and_rule_gates() {
        let mut sess = Session::new(100);
        sess.load_group(ParsedGroupText {
            title: "配色验收".into(),
            difficulty: None,
            segments: vec![Segment {
                mark: "1".into(),
                text: "甲乙丙丁戊己庚辛壬癸".into(),
            }],
        });
        sess.comparer.states[0] = CharState::Correct;
        sess.comparer.states[1] = CharState::Wrong;
        let word = |start, len, text: &str, code: Option<&str>, rank, tail: &str| HintWord {
            start,
            len,
            text: text.into(),
            code: code.map(str::to_owned),
            rank,
            tail: tail.into(),
        };
        let mut model = Model {
            hints: true,
            words: vec![
                word(0, 2, "甲乙", Some("ab"), 1, "_"),
                word(2, 1, "丙", Some("c"), 2, ";"),
                word(3, 2, "丁戊", Some("de"), 2, ";"),
                word(5, 1, "己", None, 0, ""),
                word(6, 2, "庚辛", Some("ef"), 3, "'"),
                word(8, 2, "壬癸", Some("gh"), 1, "_"),
            ],
            ..Default::default()
        };
        let mut cfg = Config {
            font_size: 24.,
            ..Default::default()
        };
        let lines = vec![
            (
                0,
                "甲乙丙丁戊".into(),
                (0..=5).map(|i| i as f32 * 24.).collect(),
            ),
            (
                5,
                "己庚辛壬癸".into(),
                (0..=5).map(|i| i as f32 * 24.).collect(),
            ),
        ];
        let theme = theme("墨玉");
        let p = tokens(&theme);
        let render = |model: &Model, cfg: &Config| {
            build(
                &sess,
                cfg,
                &theme,
                model,
                1280.,
                800.,
                0,
                0.,
                &mut Animation::default(),
                &mut Vec::new(),
                &lines,
            )
        };
        let frame = render(&model, &cfg);
        let rows: Vec<_> = frame
            .cmds
            .iter()
            .filter_map(|cmd| match cmd {
                DrawCmd::StyledLine { text, spans, y, .. } => Some((text, spans, y)),
                _ => None,
            })
            .collect();
        assert_eq!(
            rows[0].1[0].2, p.word_colors[0],
            "correct multi-word text retains its word color"
        );
        assert_eq!(
            rows[0].1[1].2, p.red,
            "wrong state takes priority over word color"
        );
        assert_eq!(
            rows[0].1[2].2, p.text2,
            "single-character text stays neutral"
        );
        assert_eq!(
            rows[0].1[3].2, p.word_colors[0],
            "both abbreviated words share the semantic short-code color"
        );
        assert!(*rows[1].2 - *rows[0].2 >= cfg.font_size + 28.);
        let has_run = |frame: &Frame, value: &str| {
            frame
                .cmds
                .iter()
                .any(|cmd| matches!(cmd, DrawCmd::TextRun { text, .. } if text == value))
        };
        assert!(
            has_run(&frame, "gh"),
            "codes beyond the former current+next-two window are visible"
        );
        assert!(has_run(&frame, "2") && has_run(&frame, "3"));
        assert!(!has_run(&frame, ";") && !has_run(&frame, "'"));
        assert!(has_run(&frame, "F10"));
        cfg.hint_show_code = false;
        let rank_only = render(&model, &cfg);
        assert!(!has_run(&rank_only, "ab") && has_run(&rank_only, "2"));
        cfg.hint_show_rank = false;
        assert!(!has_run(&render(&model, &cfg), "2"));
        cfg.hint_show_code = true;
        cfg.hint_show_rank = true;
        cfg.color_overrides
            .insert("打对字色".into(), "ffffff".into());
        let overridden = render(&model, &cfg);
        assert!(overridden
            .cmds
            .iter()
            .any(|c| matches!(c,DrawCmd::StyledLine{spans,..} if spans[0].2==p.correct)));
        cfg.color_overrides.clear();
        cfg.hint_style = 1;
        assert!(
            has_run(&render(&model, &cfg), "gh"),
            "character hint style also covers following lines"
        );
        model.hints = false;
        assert!(!has_run(&render(&model, &cfg), "ab"));
        model.hints = true;
        model.race = true;
        assert!(!has_run(&render(&model, &cfg), "ab"));
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
