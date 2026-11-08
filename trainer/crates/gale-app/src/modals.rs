//! D2D panels with accessible native RichEdit fields supplied by the application.
use crate::{session::Session, ui::*};
use gale_core::{
    config::{Config, Material},
    score::ScoreRecord,
    theme::Theme,
};
use gale_win::d2d::{DrawCmd, Frame};
use serde::Deserialize;
use std::collections::BTreeMap;
#[path = "preferences.rs"]
mod preferences;
// Modal text always remains legible at the actual window size. Decorative
// geometry and the training article's configured size are independent.
fn text(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: [f32; 4]) {
    crate::ui::text(f, x, y, size.max(15.), s, c)
}
fn mono(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: [f32; 4]) {
    crate::ui::mono(f, x, y, size.max(15.), s, c)
}
fn right(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: [f32; 4]) {
    crate::ui::right(f, x, y, size.max(15.), s, c)
}
fn body_text(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: [f32; 4]) {
    crate::ui::body_text(f, x, y, size.max(15.), s, c)
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Modal {
    #[default]
    None,
    Groups,
    Appearance,
    Preferences,
    Report,
    Online,
    Rules,
    Confirm,
    /// 本段按键热力图（结算后，数据 Session::last_vk_histogram）。
    Heatmap,
    /// 文章理论分析（数据 Session::full_analysis，None=旧版服务）。
    FullAnalysis,
    /// 【减法 2026-11-09】原 F2 载文窗退役；本地文库 + 单字表 + TXT 收拢为
    /// 轻量文库面板，从主界面「文库」按钮进入。
    Library,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    None,
    Open(Modal),
    Close,
    Exit,
    Minimize,
    Maximize,
    ToggleRace,
    ToggleHints,
    Restart,
    Previous,
    Next,
    CopyScore,
    CopyRace,
    ExportImage,
    SendScore,
    /// 【群三件套】把成绩单直接发到绑定群（增强链：剪贴板→激活→校验→粘贴→回车）。
    SendScoreToGroup,
    /// 【群三件套】按晴发文格式把当前段/整篇发到绑定群。
    PostArticle,
    /// 【群三件套】切换「打完自动发成绩到群」（cfg.auto_send_score）。
    ToggleAutoSend,
    LoadClipboard,
    LoadFile,
    AcceptDraft,
    LoadBook(usize),
    Practice(usize),
    Theme(String),
    Material(Material),
    Font(i32),
    Motion,
    Accent(String),
    LineHeight(i32),
    MotionLevel(u8),
    HintStyle(u8),
    AppearanceReset,
    Folder,
    ScanGroups,
    SelectGroup(usize),
    ReadGroup,
    OnlineLogin,
    OnlineAccount,
    OnlineLogout,
    OnlineServer,
    OnlineFetch(i8),
    OnlineLoad,
    OnlineLoadOnly,
    OnlineBooks,
    OnlineCategory(usize),
    OnlineDifficulty,
    OnlineDifficultyTo(usize),
    OnlineStrict,
    OnlineShelf(usize),
    ShelfPage,
    RuleSelect(usize),
    RuleToggle(usize),
    RuleUp,
    RuleDown,
    RuleAdd,
    RuleDelete,
    RuleSave,
    RuleTest,
    RuleApply,
    RuleRestore,
    RuleReset,
    RuleContentToggle,
    RuleScopeToggle,
    RuleImport,
    RuleExport,
    QuoteToggle,
    QuoteClass(u8),
    ConfirmYes,
    ConfirmNo,
}
#[derive(Clone)]
pub struct Hit {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub action: Action,
}
pub fn hit(hs: &[Hit], x: f32, y: f32) -> Option<Action> {
    hs.iter()
        .rev()
        .find(|z| x >= z.x && x < z.x + z.w && y >= z.y && y < z.y + z.h)
        .map(|z| z.action.clone())
}
#[derive(Clone, Debug, Default, Deserialize)]
pub struct HintWord {
    pub text: String,
    pub code: Option<String>,
    #[serde(default)]
    pub tail: String,
    #[serde(default)]
    pub start: usize,
    #[serde(default)]
    pub len: usize,
    #[serde(default)]
    pub rank: usize,
}
#[derive(Default)]
pub struct Model {
    pub modal: Modal,
    pub panel_scroll: f32,
    pub scroll: f32,
    pub race: bool,
    pub hints: bool,
    pub reduced_motion: bool,
    pub paused: bool,
    pub status: String,
    pub words: Vec<HintWord>,
    pub max_code_length: usize,
    pub schema: String,
    pub schema_ver: String,
    pub load_tab: u8,
    pub load_editing: bool,
    pub pending_source: String,
    pub shelf_offset: usize,
    pub books: Vec<String>,
    pub groups: Vec<String>,
    pub bound: Option<usize>,
    pub online_status: String,
    pub online_title: String,
    pub online_preview: String,
    pub online_categories: Vec<(String, String)>,
    pub online_shelf: Vec<String>,
    pub online_category: usize,
    pub online_difficulty: usize,
    pub online_strict: bool,
    pub online_busy: bool,
    pub online_login_expanded: bool,
    /// 注册卡模式（展开的登录区二态：登录卡 / 注册卡，aux "online_register" 切换）。
    pub online_register_mode: bool,
    /// 注册验证码重发冷却剩余秒数（>0 时"发送验证码"按钮禁用并显示剩余秒；
    /// 由主循环按 reg_code_until 截止时刻每秒刷新）。
    pub online_code_wait: u16,
    pub online_authenticated: bool,
    pub online_expired: bool,
    pub online_account: String,
    pub online_article: Option<gale_core::wenlai::WenlaiArticle>,
    pub rule_save_preset: bool,
    pub rules: Vec<(String, bool, bool, String, String)>,
    pub rule_selected: usize,
    pub rule_preview: String,
    pub rule_test_source: String,
    pub rule_hits: usize,
    pub rule_error: String,
    pub quote_relaxed: bool,
    pub fields: BTreeMap<String, String>,
    pub latest: Option<ScoreRecord>,
    pub history: Vec<ScoreRecord>,
    pub history_expanded: bool,
    pub history_scroll: usize,
    pub confirm: String,
    pub pending: Option<Action>,
    pub toast: Option<(String, f64)>,
    pub modal_epoch: u64,
}
impl Model {
    pub fn open(&mut self, m: Modal) {
        self.modal = m;
        self.scroll = 0.;
        self.panel_scroll = 0.;
        self.modal_epoch += 1;
    }
    pub fn toast(&mut self, s: impl Into<String>, n: f64) {
        self.toast = Some((s.into(), n));
    }
    pub fn field(&self, id: &str) -> String {
        self.fields.get(id).cloned().unwrap_or_default()
    }
}
pub struct FieldSpec {
    pub id: &'static str,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub multi: bool,
    pub password: bool,
}
pub const PANEL_HEAD: f32 = 62.;
pub const PANEL_FOOT: f32 = 62.;

pub fn panel(m: Modal, w: f32, h: f32) -> (f32, f32, f32, f32) {
    if m == Modal::Report {
        return (0., 55., w, (h - 92.).max(160.));
    }
    let (ideal_w, ph): (f32, f32) = match m {
        Modal::Online => (960., 760.),
        Modal::Preferences => (960., 1040.),
        Modal::Groups => (940., 620.),
        Modal::Appearance => (1064., 668.),
        Modal::Rules => (1040., 900.),
        Modal::Confirm => (560., 300.),
        Modal::Heatmap => (1040., 620.),
        Modal::FullAnalysis => (1000., 690.),
        _ => (880., 596.),
    };
    let ph = if matches!(m, Modal::FullAnalysis | Modal::Heatmap) {
        panel_view(ph, h)
    } else {
        ph
    };
    let pw = ideal_w.min((w - 32.).max(200.));
    let view = panel_view(ph, h);
    ((w - pw) / 2., ((h - view) / 2.).max(16.), pw, ph)
}
fn panel_view(ph: f32, h: f32) -> f32 {
    ph.min((h - 48.).max(PANEL_HEAD + PANEL_FOOT + 48.))
}
pub fn fields(m: &Model, w: f32, h: f32) -> Vec<FieldSpec> {
    let (x, y, pw, ph) = panel(m.modal, w, h);
    let mut v = match m.modal {
        Modal::Online | Modal::Rules => crate::secondary_panels::fields(m, x, y, pw, ph),
        Modal::Preferences => preferences::fields(m, x, y, pw, ph),
        _ => Vec::new(),
    };
    let top = y
        + PANEL_HEAD
        + if m.modal == Modal::Preferences {
            50.
        } else {
            0.
        };
    let bottom = y + panel_view(ph, h) - PANEL_FOOT;
    let scroll = m.panel_scroll.clamp(0., (ph - panel_view(ph, h)).max(0.));
    for z in &mut v {
        z.y -= scroll;
        let end = (z.y + z.h).min(bottom);
        let right = (z.x + z.w).min(x + pw - 12.);
        z.y = z.y.max(top);
        z.x = z.x.max(x + 12.);
        z.w = (right - z.x).max(0.);
        z.h = (end - z.y).max(0.);
    }
    v
}
fn command_y(c: &DrawCmd) -> Option<f32> {
    match c {
        DrawCmd::Rect { y, .. }
        | DrawCmd::RoundRect { y, .. }
        | DrawCmd::Line { y, .. }
        | DrawCmd::Text { y, .. }
        | DrawCmd::TextRun { y, .. }
        | DrawCmd::StyledLine { y, .. }
        | DrawCmd::PushClip { y, .. }
        | DrawCmd::Glyphs { y, .. } => Some(*y),
        DrawCmd::PopClip | DrawCmd::BlurBackdrop { .. } => None,
    }
}
fn shift_command(c: &mut DrawCmd, dy: f32) {
    match c {
        DrawCmd::Rect { y, .. }
        | DrawCmd::RoundRect { y, .. }
        | DrawCmd::Line { y, .. }
        | DrawCmd::Text { y, .. }
        | DrawCmd::TextRun { y, .. }
        | DrawCmd::StyledLine { y, .. }
        | DrawCmd::PushClip { y, .. }
        | DrawCmd::Glyphs { y, .. } => *y -= dy,
        DrawCmd::PopClip | DrawCmd::BlurBackdrop { .. } => {}
    }
}
/// Keep header and footer fixed; scroll and clip the complete body, including
/// nested drawing clips. Hit targets and native editor rectangles use the same viewport.
fn fit_panel(
    f: &mut Frame,
    hits: &mut Vec<Hit>,
    start: usize,
    x: f32,
    y: f32,
    pw: f32,
    ph: f32,
    view: f32,
    scroll: f32,
    sticky: f32,
    content: f32,
) {
    // `sticky` keeps a band just below the header (for example the preference
    // tabs) out of the scrolling body so a scrolled panel never shows a
    // half-clipped tab row.
    let body_top = y + PANEL_HEAD + sticky;
    let body_end = y + ph - PANEL_FOOT;
    let visible_end = y + view - PANEL_FOOT;
    let body_view = (visible_end - body_top).max(1.);
    let (scroll_range, body_content) = if content > 0. {
        let body_content = (content - PANEL_HEAD - PANEL_FOOT).max(0.);
        ((body_content - body_view).max(0.), body_content)
    } else {
        ((ph - view).max(0.), (body_end - body_top).max(1.))
    };
    let scroll = scroll.clamp(0., scroll_range);
    let old = f.cmds.split_off(start);
    let mut body_clip = false;
    let mut original_clips = Vec::new();
    for (i, mut c) in old.into_iter().enumerate() {
        if i == 0 {
            if let DrawCmd::RoundRect { h, .. } = &mut c {
                *h = view;
            }
            f.push(c);
            continue;
        }
        let section = if matches!(c, DrawCmd::PopClip) {
            original_clips.pop().unwrap_or(0u8)
        } else {
            original_clips.last().copied().unwrap_or_else(|| {
                let cy = command_y(&c).unwrap_or(y);
                if cy >= body_end {
                    2
                } else if cy >= body_top {
                    1
                } else {
                    0
                }
            })
        };
        if body_clip && section != 1 {
            f.push(DrawCmd::PopClip);
            body_clip = false;
        }
        if !body_clip && section == 1 {
            f.push(DrawCmd::PushClip {
                x: x + 1.,
                y: body_top,
                w: pw - 2.,
                h: (visible_end - body_top).max(1.),
            });
            body_clip = true;
        }
        if matches!(c, DrawCmd::PushClip { .. }) {
            original_clips.push(section);
        }
        shift_command(
            &mut c,
            if section == 1 {
                scroll
            } else if section == 2 {
                ph - view
            } else {
                0.
            },
        );
        f.push(c);
    }
    if body_clip {
        f.push(DrawCmd::PopClip);
    }
    for z in hits.iter_mut() {
        if z.action == Action::None && z.x == x && z.y == y && z.w == pw {
            z.h = view;
        } else if z.y >= body_top && z.y < body_end {
            z.y -= scroll;
            let right = (z.x + z.w).min(x + pw);
            let bottom = (z.y + z.h).min(visible_end);
            z.x = z.x.max(x);
            z.w = (right - z.x).max(0.);
            z.y = z.y.max(body_top);
            z.h = (bottom - z.y).max(0.);
        } else if z.y >= body_end {
            z.y -= ph - view;
        }
    }
    if scroll_range > 0. {
        let track = body_view;
        let thumb = (track * track / body_content.max(1.)).clamp(12., track);
        // A 2 DIP hairline was easy to miss on the tall settings/online panels.
        rect(f, x + pw - 6., body_top, 4., track, [0.5, 0.5, 0.5, 0.10]);
        rect(
            f,
            x + pw - 6.,
            body_top + scroll / scroll_range * (track - thumb),
            4.,
            thumb,
            [0.78, 0.65, 0.38, 0.75],
        );
    }
}
pub(crate) fn label(f: &mut Frame, x: f32, y: f32, s: impl Into<String>, t: &Theme) {
    text(f, x, y, FONT_HINT, s, tokens(t).muted);
}
fn button_label(f: &mut Frame, x: f32, y: f32, w: f32, s: &str, c: [f32; 4]) {
    // Keep the readable font size even on narrow windows. The clip prevents a
    // long file or action name from painting over the neighbouring control.
    f.push(DrawCmd::PushClip {
        x,
        y,
        w: w.max(0.),
        h: 20.,
    });
    text(f, x, y, 15., s, c);
    f.push(DrawCmd::PopClip);
}
pub(crate) fn btn(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    s: &str,
    a: Action,
    t: &Theme,
    primary: bool,
) {
    let p = tokens(t);
    round(
        f,
        x,
        y,
        w,
        34.,
        8.,
        if primary { p.accent } else { p.surface2 },
        if primary { None } else { Some(p.line2) },
    );
    button_label(
        f,
        x + 11.,
        y + 7.,
        w - 22.,
        s,
        if primary {
            [0.13, 0.09, 0.035, 1.]
        } else {
            p.text2
        },
    );
    hs.push(Hit {
        x,
        y,
        w,
        h: 34.,
        action: a,
    });
}
fn small_btn(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    s: &str,
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
        34.,
        6.,
        if selected {
            alpha(p.accent, 0.1)
        } else {
            p.bg2
        },
        Some(if selected {
            alpha(p.accent, 0.5)
        } else {
            p.line
        }),
    );
    button_label(
        f,
        x + 8.,
        y + 7.,
        w - 16.,
        s,
        if selected { p.accent } else { p.muted },
    );
    hs.push(Hit {
        x,
        y,
        w,
        h: 34.,
        action: a,
    });
}
fn disabled(f: &mut Frame, x: f32, y: f32, w: f32, s: &str, t: &Theme) {
    let p = tokens(t);
    round(f, x, y, w, 34., 6., alpha(p.bg2, 0.8), Some(p.line));
    button_label(f, x + 7., y + 7., w - 14., s, alpha(p.muted, 0.5));
}
fn hit_box(hs: &mut Vec<Hit>, x: f32, y: f32, w: f32, h: f32, action: Action) {
    hs.push(Hit { x, y, w, h, action });
}
fn text_block(
    f: &mut Frame,
    x: f32,
    y: f32,
    w: f32,
    size: f32,
    line_h: f32,
    max_lines: usize,
    s: &str,
    color: [f32; 4],
) {
    // Wrap with the same clamped size the drawing helper uses; otherwise a
    // caller passing 13.5 wraps wider than the 15 DIP glyphs and overflows.
    let size = size.max(15.);
    for (i, l) in wrap(s, (w / size).max(1.) as usize)
        .into_iter()
        .take(max_lines)
        .enumerate()
    {
        body_text(f, x, y + i as f32 * line_h, size, l, color);
    }
}
fn icon(f: &mut Frame, kind: &str, x: f32, y: f32, size: f32, color: [f32; 4]) {
    let u = size / 18.;
    let clear = [0.; 4];
    let mut outline = |xx: f32, yy: f32, ww: f32, hh: f32, r: f32| {
        round(
            f,
            x + xx * u,
            y + yy * u,
            ww * u,
            hh * u,
            r * u,
            clear,
            Some(color),
        );
    };
    match kind {
        "group" => {
            outline(3., 1., 6., 6., 3.);
            outline(0., 9., 12., 8., 5.);
            outline(12., 4., 4., 5., 2.);
            outline(13., 11., 4., 6., 2.);
        }
        "palette" => {
            outline(0., 0., 18., 18., 9.);
            for (dx, dy) in [(5., 4.), (10., 3.), (13., 7.), (4., 9.)] {
                round(f, x + dx * u, y + dy * u, 2. * u, 2. * u, u, color, None);
            }
        }
        "folder" => {
            outline(0., 4., 18., 13., 2.);
            outline(1., 1., 7., 4., 1.);
        }
        "globe" => {
            outline(0., 0., 18., 18., 9.);
            outline(5., 0., 8., 18., 4.);
            line(f, x, y + 8.5 * u, size, color);
        }
        // 整改 12：键盘（两排键块 + 空格条）。
        "keyboard" => {
            for (kx, kw) in [(0., 3.), (4.5, 3.), (9., 3.), (13.5, 4.5)] {
                outline(kx, 0., kw, 4.5, 1.);
                outline(kx, 6.5, kw, 4.5, 1.);
            }
            outline(4.5, 13., 9., 4.5, 2.);
        }
        // 整改 13：柱状图（三根高低柱）。
        "chart" => {
            outline(0., 9., 4., 9., 1.);
            outline(6.5, 4., 4., 14., 1.);
            outline(13., 0., 4., 18., 1.);
        }
        _ => {
            outline(2., 2., 14., 16., 2.);
            outline(6., 0., 6., 4., 1.);
        }
    }
}
fn header_keycap(f: &mut Frame, x: f32, y: f32, key: &str, t: &Theme) {
    let p = tokens(t);
    round(f, x, y, 36., 28., 6., p.surface2, Some(p.line2));
    mono(f, x + 7., y + 4., 15., key, p.muted);
}
fn panel_header(f: &mut Frame, hs: &mut Vec<Hit>, m: Modal, x: f32, y: f32, pw: f32, t: &Theme) {
    let p = tokens(t);
    let (icon_name, title, desc, key) = match m {
        Modal::Library => (
            "book",
            "文库",
            "本地文库、单字表与 TXT；剪贴板 Ctrl+V 直接开打",
            "—",
        ),
        Modal::Groups => ("group", "群与通道", "从 QQ 群取赛文，把成绩带回去", "F5"),
        Modal::Appearance => ("palette", "外观", "主题、材质与词提；改动即时生效", "F7"),
        Modal::Preferences => (
            "palette",
            "跟打器设置",
            "词提显示、成绩单和自定义配色",
            "F1",
        ),
        Modal::Heatmap => (
            "keyboard",
            "本段按键热力图",
            "最近一次结算段的按键频次 · 滚轮关闭",
            "Esc",
        ),
        Modal::FullAnalysis => (
            "chart",
            "理论码长详情",
            "全文逐词编码 · 理论码长与选重 · 滚轮翻看",
            "F10",
        ),
        Modal::Confirm => ("!", "确认操作", "当前练习的内容将发生改变", "Esc"),
        _ => ("", "", "", ""),
    };
    round(
        f,
        x + 20.,
        y + 15.,
        32.,
        32.,
        9.,
        alpha(p.accent, 0.08),
        Some(alpha(p.accent, 0.35)),
    );
    icon(f, icon_name, x + 27., y + 22., 18., p.accent);
    text(f, x + 62., y + 10., 20., title, p.text);
    text(f, x + 62., y + 38., 15., desc, p.muted);
    header_keycap(f, x + pw - 104., y + 17., key, t);
    text(f, x + pw - 52., y + 13., 25., "×", p.muted);
    hit_box(hs, x + pw - 58., y + 10., 42., 42., Action::Close);
    line(f, x, y + PANEL_HEAD - 1., pw, p.line);
}
fn footer(f: &mut Frame, x: f32, y: f32, pw: f32, ph: f32, t: &Theme) {
    let p = tokens(t);
    rect(
        f,
        x + 1.,
        y + ph - PANEL_FOOT,
        pw - 2.,
        PANEL_FOOT - 1.,
        alpha(p.bg2, 0.6),
    );
    line(f, x, y + ph - PANEL_FOOT, pw, p.line);
}
fn draw_library(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    t: &Theme,
    x: f32,
    y: f32,
    pw: f32,
    ph: f32,
) {
    // 【减法 2026-11-09】原 F2 载文窗退役后的轻量文库面板：本地文库 +
    // 单字表 + TXT。剪贴板走 Ctrl+V 直载、群文走 F4、在线走 F8。
    let p = tokens(t);
    let rx = x + 24.;
    let rw = pw - 48.;
    text(f, rx, y + 84., FONT_HINT, "本地文库", p.text2);
    small_btn(
        f,
        hs,
        rx + rw - 88.,
        y + 76.,
        88.,
        "打开 TXT",
        Action::LoadFile,
        t,
        false,
    );
    round(f, rx, y + 104., rw, 112., 12., p.surface, Some(p.line));
    if m.books.is_empty() {
        body_text(f, rx + 18., y + 122., 19., "文库还没有文章", p.text);
        text_block(
            f,
            rx + 18.,
            y + 157.,
            rw - 36.,
            FONT_HINT,
            25.,
            2,
            "打开一个 TXT 文件，或把文本放进数据目录的文库文件夹。",
            p.muted,
        );
    } else {
        for (row, (i, title)) in m
            .books
            .iter()
            .enumerate()
            .skip(m.scroll.max(0.) as usize)
            .take(2)
            .enumerate()
        {
            small_btn(
                f,
                hs,
                rx + 14.,
                y + 119. + row as f32 * 39.,
                rw - 28.,
                &elide(title, ((rw - 50.) / 14.) as usize),
                Action::LoadBook(i),
                t,
                false,
            );
        }
        text(
            f,
            rx + 16.,
            y + 203.,
            14.,
            format!("共 {} 篇 · 滚轮浏览文库", m.books.len()),
            p.muted,
        );
    }
    text(f, rx, y + 246., 17., "单字与词组练习", p.text2);
    text(
        f,
        rx,
        y + 274.,
        14.,
        "乱序 / 段长 / 达标击力在单字练习屏顶部快捷条调整。",
        p.muted,
    );
    round(f, rx, y + 296., rw, 190., 12., p.surface, Some(p.line));
    for (i, (title, _)) in gale_core::single::builtin_tables().iter().enumerate() {
        let cw = (rw - 38.) / 2.;
        small_btn(
            f,
            hs,
            rx + 14. + (i % 2) as f32 * (cw + 10.),
            y + 310. + (i / 2) as f32 * 36.,
            cw,
            title,
            Action::Practice(i),
            t,
            false,
        );
    }
    footer(f, x, y, pw, ph, t);
    btn(
        f,
        hs,
        x + pw - 132.,
        y + ph - 50.,
        110.,
        "关闭",
        Action::Close,
        t,
        false,
    );
}
fn draw_groups(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    sess: &Session,
    cfg: &Config,
    t: &Theme,
    x: f32,
    y: f32,
    pw: f32,
    ph: f32,
) {
    let p = tokens(t);
    let side = 356f32.min(pw * 0.39).max(210.);
    let bh = ph - 124.;
    rect(f, x + 1., y + 62., side - 1., bh, p.bg2);
    rect(f, x + side, y + 62., 1., bh, p.line);
    text(
        f,
        x + 22.,
        y + 82.,
        FONT_HINT,
        format!("QQ 群 / 会话   {}", m.groups.len()),
        p.muted,
    );
    small_btn(
        f,
        hs,
        x + side - 76.,
        y + 76.,
        58.,
        "扫描",
        Action::ScanGroups,
        t,
        false,
    );
    if m.groups.is_empty() {
        body_text(f, x + 24., y + 142., 16., "还没有会话", p.text2);
        text_block(
            f,
            x + 24.,
            y + 182.,
            side - 48.,
            FONT_HINT,
            26.,
            5,
            "打开 QQ 和目标群后扫描。\n也可以手动复制赛文，Ctrl+V 直接开打。",
            p.muted,
        );
    } else {
        let avatar_colors = [
            p.accent,
            p.jade,
            [0.65, 0.49, 0.65, 1.],
            [0.45, 0.63, 0.81, 1.],
        ];
        for (row, (i, name)) in m
            .groups
            .iter()
            .enumerate()
            .skip(m.scroll.max(0.) as usize)
            .take(5)
            .enumerate()
        {
            let gy = y + 113. + row as f32 * 67.;
            let selected = m.bound == Some(i);
            if selected {
                round(
                    f,
                    x + 12.,
                    gy,
                    side - 24.,
                    64.,
                    10.,
                    p.surface2,
                    Some(p.line),
                );
                round(f, x + 12., gy + 5., 2., 54., 1., p.accent, None);
            }
            round(
                f,
                x + 22.,
                gy + 15.,
                34.,
                34.,
                9.,
                avatar_colors[i % 4],
                None,
            );
            body_text(
                f,
                x + 30.,
                gy + 19.,
                17.,
                name.chars().next().unwrap_or('群').to_string(),
                [0.08, 0.09, 0.08, 1.],
            );
            text(
                f,
                x + 66.,
                gy + 17.,
                FONT_HINT,
                elide(name, ((side - 90.) / FONT_HINT) as usize),
                p.text2,
            );
            text(
                f,
                x + 66.,
                gy + 39.,
                FONT_HINT,
                elide(
                    if selected {
                        if side < 260. {
                            "已选择"
                        } else {
                            "已选择 · 选群即切群"
                        }
                    } else if side < 260. {
                        "点击选择"
                    } else {
                        "QQ 会话 · 点击选择"
                    },
                    ((side - 90.) / FONT_HINT).max(4.) as usize,
                ),
                p.muted,
            );
            hit_box(hs, x + 12., gy, side - 24., 64., Action::SelectGroup(i));
        }
    }
    line(f, x + 22., y + ph - 108., side - 44., p.line);
    text(
        f,
        x + 22.,
        y + ph - 92.,
        FONT_HINT,
        "剪贴板通道始终可用",
        p.muted,
    );
    let rx = x + side + 20.;
    let rw = pw - side - 40.;
    let group = m.bound.and_then(|i| m.groups.get(i));
    round(f, rx, y + 80., 42., 42., 11., alpha(p.accent, 0.8), None);
    body_text(
        f,
        rx + 12.,
        y + 88.,
        19.,
        group
            .and_then(|s| s.chars().next())
            .unwrap_or('群')
            .to_string(),
        [0.1, 0.07, 0.03, 1.],
    );
    text(
        f,
        rx + 54.,
        y + 81.,
        16.,
        group
            .map(|s| elide(s, ((rw - 54.) / 16.) as usize))
            .unwrap_or_else(|| "选择一个会话".into()),
        p.text,
    );
    text(
        f,
        rx + 54.,
        y + 104.,
        FONT_HINT,
        if rw < 420. {
            "独立聊天窗选群即自动切换"
        } else {
            "独立聊天窗选群即自动切换；主面板条目需在 QQ 点开"
        },
        p.muted,
    );
    // Channel status belongs next to the session identity. `m.status` is the
    // general trainer status ("词提已就绪"), not the QQ channel; derive the dot
    // from the actual session list so F6 cannot claim a connected QQ with 0 rows.
    let qq_ready = !m.groups.is_empty();
    let bad = ["失败", "错误", "离线", "不可用", "未连接"]
        .iter()
        .any(|k| m.status.contains(k));
    let dot = if qq_ready {
        p.green
    } else if bad {
        p.red
    } else {
        p.muted
    };
    let channel = if qq_ready {
        format!("QQ 已连接 · {} 个会话", m.groups.len())
    } else if m.status.contains("UIA") || m.status.contains("未返回") {
        "已找到 QQ，但 UIA 未返回会话；重启 QQ 或打开群聊窗口后重试".into()
    } else {
        "尚未读取到 QQ 会话 · 点击扫描".into()
    };
    round(f, rx, y + 130., 6., 6., 3., dot, None);
    text(
        f,
        rx + 12.,
        y + 124.,
        14.,
        elide(
            &format!("通道 · {channel}"),
            ((rw - 16.) / 14.).max(6.) as usize,
        ),
        if bad {
            p.red
        } else if qq_ready {
            p.green
        } else {
            p.muted
        },
    );
    // Three metrics only: the selected session is already the header, repeating
    // it as a KPI read like a placeholder.
    round(f, rx, y + 146., rw, 58., 9., p.surface2, Some(p.line));
    let vals = [
        (
            "最近成绩",
            m.latest
                .as_ref()
                .map(|r| format!("{:.1}", r.speed))
                .unwrap_or_else(|| "—".into()),
        ),
        ("当前文档", format!("{} 段", sess.total_segments())),
        ("输入方式", "真实输入".into()),
    ];
    for (i, (l, v)) in vals.into_iter().enumerate() {
        let sx = rx + i as f32 * rw / 3.;
        if i > 0 {
            rect(f, sx, y + 146., 1., 58., p.line);
        }
        text(f, sx + 12., y + 158., 14., l, p.muted);
        mono(f, sx + 12., y + 176., 20., v, p.text2);
    }
    round(f, rx, y + 214., rw, 69., 10., p.surface, Some(p.line));
    body_text(f, rx + 12., y + 227., 16., "文", p.accent);
    text(
        f,
        rx + 44.,
        y + 226.,
        FONT_HINT,
        elide(&sess.title, ((rw - 64.) / FONT_HINT) as usize),
        p.text2,
    );
    text(
        f,
        rx + 44.,
        y + 250.,
        FONT_HINT,
        format!(
            "当前文档 · {} 段 · {} 字",
            sess.total_segments(),
            sess.segments
                .iter()
                .map(|s| s.text.chars().count())
                .sum::<usize>()
        ),
        p.muted,
    );
    round(f, rx, y + 296., rw, 118., 10., p.surface2, Some(p.line));
    text(f, rx + 12., y + 310., FONT_HINT, "UIA 读取通道", p.text2);
    text_block(
        f,
        rx + 12.,
        y + 334.,
        rw - 24.,
        FONT_HINT,
        22.,
        2,
        "从 QQ 当前界面读取赛文；不注入、不修改 QQ 进程。",
        p.muted,
    );
    round(
        f,
        rx + 12.,
        y + 372.,
        rw - 24.,
        34.,
        6.,
        alpha(p.accent, 0.055),
        Some(alpha(p.accent, 0.23)),
    );
    text_block(
        f,
        rx + 20.,
        y + 379.,
        rw - 40.,
        FONT_HINT,
        22.,
        2,
        "QQ 更新可能使 UIA 失效。失败时请手动复制，通过剪贴板载入。",
        p.accent,
    );
    // Reading order: users repeatedly ask what to do after opening QQ.
    round(f, rx, y + 426., rw, 58., 10., p.surface, Some(p.line));
    text(f, rx + 12., y + 438., 14., "群流程", p.muted);
    let narrow = rw < 520.;
    text(
        f,
        rx + 78.,
        y + 438.,
        14.,
        if narrow {
            "扫描 › 点选 › F4 读文 › 打完自动发"
        } else {
            "扫描  ›  点选（自动切群）  ›  F4 读文  ›  打完自动发成绩"
        },
        p.text2,
    );
    if !narrow {
        text(
            f,
            rx + 12.,
            y + 460.,
            14.,
            "发送前自动校验目标群；失败内容保留在剪贴板，绝不错发。",
            p.muted,
        );
    }
    // 【群三件套】自动发成绩开关（cfg.auto_send_score，默认开）
    round(
        f,
        rx,
        y + 494.,
        rw,
        40.,
        9.,
        if cfg.auto_send_score {
            alpha(p.accent, 0.07)
        } else {
            p.surface2
        },
        Some(if cfg.auto_send_score {
            alpha(p.accent, 0.3)
        } else {
            p.line
        }),
    );
    text(
        f,
        rx + 12.,
        y + 506.,
        14.,
        "打完自动发成绩到群",
        if cfg.auto_send_score { p.text } else { p.muted },
    );
    text(
        f,
        rx + 12.,
        y + 522.,
        FONT_HINT,
        if cfg.auto_send_score {
            "晴兼容成绩单 · 发送前校验目标群"
        } else {
            "关闭后仅复制成绩，可手动粘贴"
        },
        p.muted,
    );
    hit_box(hs, rx, y + 494., rw, 40., Action::ToggleAutoSend);
    footer(f, x, y, pw, ph, t);
    if pw > 740. {
        text(
            f,
            x + 20.,
            y + ph - 35.,
            FONT_HINT,
            "发文 = 把当前段按晴发文格式发到群，随后可 F4 读回",
            p.muted,
        );
    }
    let fy = y + ph - 47.;
    btn(
        f,
        hs,
        x + pw - 442.,
        fy,
        96.,
        "发成绩到群",
        Action::SendScoreToGroup,
        t,
        false,
    );
    btn(
        f,
        hs,
        x + pw - 330.,
        fy,
        72.,
        "发文",
        Action::PostArticle,
        t,
        false,
    );
    btn(
        f,
        hs,
        x + pw - 214.,
        fy,
        70.,
        "关闭",
        Action::Close,
        t,
        false,
    );
    btn(
        f,
        hs,
        x + pw - 136.,
        fy,
        116.,
        "读取当前群文",
        Action::ReadGroup,
        t,
        true,
    );
}
fn hex_color(s: &str) -> [f32; 4] {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return [0.9, 0.7, 0.35, 1.];
    }
    let c = |i| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or(128) as f32 / 255.;
    [c(0), c(2), c(4), 1.]
}
fn appearance_preview(
    f: &mut Frame,
    sess: &Session,
    m: &Model,
    cfg: &Config,
    t: &Theme,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) {
    let p = tokens(t);
    let sample: Vec<char> = sess.current_segment().text.chars().collect();
    if sample.is_empty() {
        text_block(
            f,
            x,
            y,
            w,
            18.,
            39.,
            4,
            "此则岳阳楼之大观也，前人之述备矣。",
            p.text2,
        );
        return;
    }
    let fs = (cfg.font_size / 2.).clamp(14., 32.);
    let hint_fs = cfg.hint_font_size.max(14.);
    let hints_visible = gale_core::rules::hints_effective(m.hints, m.race) && cfg.hint_style != 2;
    let lh = if hints_visible {
        (fs * (1. + cfg.line_gap) + 9.).max(fs + hint_fs + 11.)
    } else {
        fs * (1. + cfg.line_gap) + 9.
    };
    let word_colors: Vec<_> = m
        .words
        .iter()
        .map(|word| {
            (hints_visible && cfg.hint_color_words)
                .then(|| {
                    word.code.as_deref().and_then(|code| {
                        crate::ui::hint_word_tint(
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
                })
                .flatten()
        })
        .collect();
    let columns = (w / fs).floor().max(1.) as usize;
    let rows = (h / lh).floor().max(1.) as usize;
    let start = sess.comparer.pos.saturating_sub(6);
    let word_index = m
        .words
        .iter()
        .position(|z| z.start.saturating_add(z.len) > sess.comparer.pos)
        .unwrap_or(m.words.len());
    f.push(DrawCmd::PushClip { x, y, w, h });
    let mut offset = start;
    for row in 0..rows {
        if offset >= sample.len() {
            break;
        }
        let mut end = (offset + columns).min(sample.len());
        if end < sample.len()
            && end > offset + 1
            && "，。；：？！、）》】」』”’".contains(sample[end])
        {
            end -= 1;
        }
        let yy = y + row as f32 * lh;
        let current = sess.comparer.pos;
        if gale_core::rules::hints_effective(m.hints, m.race) && current >= offset && current < end
        {
            let highlight = if cfg.hint_style == 1 {
                (current, 1)
            } else {
                m.words
                    .get(word_index)
                    .map(|z| (z.start, z.len))
                    .unwrap_or((current, 1))
            };
            let xx = x + highlight.0.saturating_sub(offset) as f32 * fs;
            let ww = (highlight.1 as f32 * fs).min(x + w - xx);
            if cfg.hint_style == 2 {
                line(f, xx, yy + fs + 2., ww, p.accent);
            } else {
                round(
                    f,
                    xx - 2.,
                    yy + 1.,
                    ww + 4.,
                    fs + 6.,
                    4.,
                    alpha(p.accent, 0.1),
                    Some(alpha(p.accent, 0.24)),
                );
            }
            rect(
                f,
                x + current.saturating_sub(offset) as f32 * fs - 3.,
                yy + 2.,
                1.5,
                fs + 2.,
                p.accent,
            );
        }
        let mut spans = Vec::new();
        let mut utf16_pos = 0;
        let mut word_at = m
            .words
            .partition_point(|word| word.start.saturating_add(word.len) <= offset);
        for (i, ch) in sample[offset..end].iter().enumerate() {
            let char_at = offset + i;
            while word_at < m.words.len()
                && m.words[word_at].start.saturating_add(m.words[word_at].len) <= char_at
            {
                word_at += 1;
            }
            let tint = m
                .words
                .get(word_at)
                .filter(|word| word.start <= char_at)
                .and_then(|_| word_colors[word_at]);
            let color = match sess.comparer.states.get(char_at) {
                Some(gale_core::compare::CharState::Correct) => tint.unwrap_or(p.text),
                Some(gale_core::compare::CharState::Wrong) => p.red,
                _ => tint.unwrap_or(p.text2),
            };
            spans.push((utf16_pos, ch.len_utf16() as u32, color));
            utf16_pos += ch.len_utf16() as u32;
        }
        f.push(DrawCmd::StyledLine {
            x,
            y: yy,
            w,
            size: fs,
            text: sample[offset..end].iter().collect(),
            spans,
            color: p.text2,
        });
        if hints_visible {
            for (i, word) in m.words.iter().enumerate() {
                if word.start < offset || word.start >= end {
                    continue;
                }
                if let Some(code) = &word.code {
                    let c = word_colors[i].unwrap_or(p.text2);
                    let xx = x + (word.start - offset) as f32 * fs;
                    let hint_size = hint_fs;
                    if cfg.hint_show_code {
                        mono(f, xx, yy + fs + 5., hint_size, code, c);
                    }
                    if cfg.hint_show_rank && word.rank > 1 {
                        mono(
                            f,
                            xx + if cfg.hint_show_code {
                                code.chars().count() as f32 * hint_size * 0.6
                            } else {
                                0.
                            },
                            yy + fs + 2.,
                            hint_size,
                            word.rank.to_string(),
                            c,
                        );
                    }
                }
            }
        }
        offset = end;
    }
    f.push(DrawCmd::PopClip);
}
fn draw_appearance(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    sess: &Session,
    cfg: &Config,
    t: &Theme,
    x: f32,
    y: f32,
    pw: f32,
    ph: f32,
) {
    let p = tokens(t);
    let preview_w = 344f32.min(pw * 0.34).max(246.);
    let controls_w = pw - preview_w;
    let cx = x + 20.;
    let cw = controls_w - 40.;
    let px = x + controls_w;
    rect(f, px, y + 62., preview_w - 1., ph - 124., p.bg2);
    rect(f, px, y + 62., 1., ph - 124., p.line);
    text(f, cx, y + 79., FONT_HINT, "预设   5 套", p.muted);
    let preset_w = (cw - 36.) / 5.;
    for (i, n) in ["墨玉", "宣纸", "夜巡", "青瓷", "朱砂"].iter().enumerate() {
        let tx = cx + i as f32 * (preset_w + 9.);
        let tt = theme(n);
        let pt = tokens(&tt);
        let selected = cfg.theme == *n || (i == 0 && cfg.theme == "default");
        round(
            f,
            tx,
            y + 99.,
            preset_w,
            58.,
            8.,
            pt.bg,
            Some(if selected { p.accent } else { p.line }),
        );
        for (row, ratio) in [0.68, 0.44, 0.56].iter().enumerate() {
            round(
                f,
                tx + 10.,
                y + 110. + row as f32 * 7.,
                (preset_w - 20.) * ratio,
                2.,
                1.,
                alpha(pt.text2, 0.7 - row as f32 * 0.2),
                None,
            );
        }
        round(
            f,
            tx + preset_w - 14.,
            y + 143.,
            5.,
            5.,
            2.5,
            pt.accent,
            None,
        );
        text(
            f,
            tx,
            y + 164.,
            FONT_HINT,
            *n,
            if selected { p.accent } else { p.muted },
        );
        hit_box(hs, tx, y + 99., preset_w, 82., Action::Theme(n.to_string()));
    }
    round(f, cx, y + 190., cw, 96., 9., p.surface2, Some(p.line));
    text(f, cx + 12., y + 202., FONT_HINT, "跟随虎符皮肤", p.muted);
    small_btn(
        f,
        hs,
        cx + cw - 58.,
        y + 198.,
        46.,
        if cfg.theme == "跟随虎符" {
            "已开"
        } else {
            "开启"
        },
        Action::Theme("跟随虎符".into()),
        t,
        cfg.theme == "跟随虎符",
    );
    let swatches = [
        (p.bg, "背景"),
        (p.text, "文字"),
        (p.accent, "强调"),
        (p.red, "错字"),
        (p.line2, "边框"),
    ];
    for (i, (c, label)) in swatches.iter().enumerate() {
        let col = i % 3;
        let row = i / 3;
        let sx = cx + 12. + col as f32 * (cw - 24.) / 3.;
        let sy = y + 232. + row as f32 * 17.;
        round(f, sx, sy, 8., 8., 1.5, *c, Some(alpha(p.text, 0.2)));
        text(f, sx + 14., sy - 1., 14., *label, p.muted);
    }
    text(
        f,
        cx + 12.,
        y + 269.,
        FONT_HINT,
        "只映射跟打器配色，不修改虎符原皮肤",
        p.muted,
    );
    let half = (cw - 18.) / 2.;
    let col2 = cx + half + 18.;
    text(f, cx, y + 301., FONT_HINT, "强调色", p.muted);
    let colors = ["e7b75c", "7fbfa6", "e06a5a", "83abd9", "c3a1c4", "d4d0c6"];
    for (i, s) in colors.iter().enumerate() {
        let sx = cx + i as f32 * 25.;
        let c = hex_color(s);
        round(f, sx, y + 324., 19., 19., 9.5, p.bg2, Some(p.line2));
        round(f, sx + 4., y + 328., 11., 11., 5.5, c, None);
        if (c[0] - p.accent[0]).abs() + (c[1] - p.accent[1]).abs() + (c[2] - p.accent[2]).abs()
            < 0.06
        {
            round(
                f,
                sx - 2.,
                y + 322.,
                23.,
                23.,
                11.5,
                [0., 0., 0., 0.],
                Some(p.accent),
            );
        }
        hit_box(
            hs,
            sx - 2.,
            y + 322.,
            23.,
            23.,
            Action::Accent(s.to_string()),
        );
    }
    text(f, col2, y + 301., FONT_HINT, "材质（仅纯色可用）", p.muted);
    small_btn(
        f,
        hs,
        col2,
        y + 322.,
        48.,
        "纯色",
        Action::Material(Material::Solid),
        t,
        true,
    );
    disabled(f, col2 + 52., y + 322., 48., "磨砂", t);
    disabled(f, col2 + 104., y + 322., 48., "透明", t);
    text(
        f,
        col2,
        y + 363.,
        FONT_HINT,
        "磨砂 / 透明：原生输入兼容，已禁用",
        p.muted,
    );
    text(f, cx, y + 385., FONT_HINT, "动效强度   四档", p.muted);
    let motion_labels = if half < 190. {
        ["关", "轻", "中", "强"]
    } else {
        ["关", "轻", "标准", "丰富"]
    };
    for (i, s) in motion_labels.iter().enumerate() {
        small_btn(
            f,
            hs,
            cx + i as f32 * 43.,
            y + 408.,
            39.,
            s,
            Action::MotionLevel(i as u8),
            t,
            cfg.motion_level == i as u8,
        );
    }
    text(
        f,
        cx,
        y + 445.,
        FONT_HINT,
        if m.reduced_motion {
            "尊重系统减少动态"
        } else {
            "输入时触发动效"
        },
        p.muted,
    );
    text(f, col2, y + 385., FONT_HINT, "字号 / 行高", p.muted);
    let slider_w = (half - 146.).max(0.);
    for (row, (name, value, minus, plus)) in [
        (
            "字号",
            format!("{:.0}", cfg.font_size),
            Action::Font(-2),
            Action::Font(2),
        ),
        (
            "行高",
            format!("{:.2}", 1. + cfg.line_gap),
            Action::LineHeight(-1),
            Action::LineHeight(1),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let sy = y + 408. + row as f32 * 39.;
        text(f, col2, sy + 3., FONT_HINT, name, p.muted);
        let ratio = if row == 0 {
            (cfg.font_size - 24.) / 40.
        } else {
            cfg.line_gap / 1.2
        };
        if half >= 190. {
            let filled = slider_w * ratio.clamp(0., 1.);
            line(f, col2 + 34., sy + 11., slider_w, p.line2);
            rect(f, col2 + 34., sy + 11., filled, 2., p.accent);
            round(f, col2 + 30. + filled, sy + 7., 8., 8., 4., p.accent, None);
        }
        small_btn(f, hs, col2 + half - 108., sy, 32., "−", minus, t, false);
        small_btn(f, hs, col2 + half - 72., sy, 32., "+", plus, t, false);
        right(f, col2 + half, sy + 4., FONT_HINT, value, p.text2);
    }
    text(f, cx, y + 493., FONT_HINT, "词提样式", p.muted);
    text(f, cx + cw - 126., y + 497., 15., "详细设置 ›", p.accent);
    hit_box(
        hs,
        cx + cw - 130.,
        y + 490.,
        130.,
        30.,
        Action::Open(Modal::Preferences),
    );
    let hw = (cw - 8.) / 3.;
    for (i, s) in ["词级柔光", "字级编码", "极简下划线"].iter().enumerate() {
        small_btn(
            f,
            hs,
            cx + i as f32 * (hw + 4.),
            y + 516.,
            hw,
            s,
            Action::HintStyle(i as u8),
            t,
            cfg.hint_style == i as u8,
        );
    }
    text(
        f,
        cx,
        y + 561.,
        FONT_HINT,
        if m.race {
            // 晴口径（research-digest §B）：赛制只关词提；字提保留、停手 5s 后显示
            "赛制已关闭词提；字提停手 5 秒后显示"
        } else {
            "提示只用于练习，赛制会自动关闭词提"
        },
        p.muted,
    );
    let card_x = px + 14.;
    let card_w = preview_w - 28.;
    text(f, card_x, y + 79., FONT_HINT, "实时预览", p.muted);
    right(
        f,
        px + preview_w - 14.,
        y + 79.,
        FONT_HINT,
        &t.name,
        p.muted,
    );
    round(
        f,
        card_x,
        y + 100.,
        card_w,
        ph - 236.,
        11.,
        p.surface,
        Some(p.line),
    );
    text(
        f,
        card_x + 13.,
        y + 115.,
        FONT_HINT,
        format!("第 {} 段 · 排版示例", sess.current_index() + 1),
        p.muted,
    );
    appearance_preview(
        f,
        sess,
        m,
        cfg,
        t,
        card_x + 13.,
        y + 151.,
        card_w - 26.,
        198.,
    );
    line(f, card_x + 13., y + 360., card_w - 26., p.line);
    let preview_stats = m
        .latest
        .as_ref()
        .map(|r| {
            [
                format!("{:.1}", r.speed),
                format!("{:.1}%", r.accuracy),
                format!("{:.2}", r.code_len),
            ]
        })
        .unwrap_or_else(|| ["—".into(), "—".into(), "—".into()]);
    for (i, name) in ["字 / 分", "键准", "码长"].iter().enumerate() {
        let sx = card_x + 13. + i as f32 * (card_w - 26.) / 3.;
        mono(f, sx, y + 373., 14., &preview_stats[i], p.text2);
        text(f, sx, y + 395., FONT_HINT, *name, p.muted);
    }
    text(
        f,
        card_x + 13.,
        y + 421.,
        FONT_HINT,
        "最近已结算成绩；无成绩显示 —",
        p.muted,
    );
    line(f, card_x + 13., y + ph - 166., card_w - 26., p.line);
    text(
        f,
        card_x + 13.,
        y + ph - 151.,
        FONT_HINT,
        format!(
            "词提{} · {:.0}px · {:.2} · 纯色",
            if gale_core::rules::hints_effective(m.hints, m.race) {
                "开"
            } else {
                "关"
            },
            cfg.font_size,
            1. + cfg.line_gap
        ),
        p.muted,
    );
    for (i, (word, code, name)) in [
        ("简码", "ab", "简码"),
        ("二字", "abcd", "二字"),
        ("三个字", "abcd", "三字"),
        ("四字以上", "abcd", "四字+"),
    ]
    .iter()
    .enumerate()
    {
        let sx = card_x + i as f32 * card_w / 4.;
        let c = crate::ui::hint_word_tint(&p, word, code, 4).unwrap_or(p.text);
        rect(f, sx, y + ph - 122., 5., 5., c);
        text(f, sx + 10., y + ph - 127., 14., *name, c);
    }
    btn(
        f,
        hs,
        card_x,
        y + ph - 104.,
        (card_w - 8.) / 2.,
        "重置",
        Action::AppearanceReset,
        t,
        false,
    );
    btn(
        f,
        hs,
        card_x + (card_w + 8.) / 2.,
        y + ph - 104.,
        (card_w - 8.) / 2.,
        "应用并关闭",
        Action::Close,
        t,
        true,
    );
    footer(f, x, y, pw, ph, t);
    text(
        f,
        x + 20.,
        y + ph - 38.,
        FONT_HINT,
        "改动即时生效 · 详细设置在上方「词提样式」右侧",
        p.muted,
    );
    btn(
        f,
        hs,
        x + pw - 216.,
        y + ph - 47.,
        124.,
        "打开数据目录",
        Action::Folder,
        t,
        false,
    );
    btn(
        f,
        hs,
        x + pw - 84.,
        y + ph - 47.,
        64.,
        "完成",
        Action::Close,
        t,
        true,
    );
}
/// The actual and theoretical heatmaps share the same complete physical keyboard.
fn draw_heatmap(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    sess: &Session,
    t: &Theme,
    x: f32,
    y: f32,
    pw: f32,
    ph: f32,
) {
    let p = tokens(t);
    let hist = &sess.last_vk_histogram;
    let total: u32 = hist.values().sum();
    let max = hist.values().copied().max().unwrap_or(0);
    let keyboard_top = if ph >= 480. { 112. } else { 76. };
    if ph >= 480. {
        text(
            f,
            x + 28.,
            y + 78.,
            14.,
            if total == 0 {
                "本段暂无按键记录 · 完成一段后生成"
            } else {
                if pw - 56. < 760. {
                    "本段按键分布 · 窄窗仅显示主键区"
                } else {
                    "本段按键分布 · 含修饰键与数字区 · 不含界面快捷键"
                }
            },
            p.muted,
        );
    }
    crate::keyboard::draw(
        f,
        t,
        x + 28.,
        y + keyboard_top,
        pw - 56.,
        ph - keyboard_top - 74.,
        |key| hist.get(&key).copied().unwrap_or(0),
    );
    footer(f, x, y, pw, ph, t);
    text(
        f,
        x + 28.,
        y + ph - 49.,
        14.,
        format!("总计 {total} 次 · 单键最高 {max} 次"),
        p.muted,
    );
    text(
        f,
        x + 28.,
        y + ph - 29.,
        14.,
        "颜色越深，频次越高 · 左右修饰键分别计数",
        p.muted,
    );
    btn(
        f,
        hs,
        x + pw - 112.,
        y + ph - 47.,
        84.,
        "完成",
        Action::Close,
        t,
        true,
    );
}
fn analysis_font(m: &Model) -> f32 {
    m.field("analysis_font")
        .parse::<f32>()
        .ok()
        .filter(|n| n.is_finite())
        .unwrap_or(20.)
        .clamp(12., 36.)
}
fn analysis_auto4(m: &Model) -> bool {
    m.fields
        .get("analysis_auto4")
        .map(|v| v != "false")
        .unwrap_or(true)
}
fn analysis_is_heatmap(m: &Model) -> bool {
    m.fields
        .get("analysis_view")
        .map(|v| v == "heatmap")
        .unwrap_or(false)
}
fn analysis_char_width(ch: char, size: f32, mono_font: bool) -> f32 {
    if ch.is_ascii() {
        size * if mono_font { 0.63 } else { 0.88 }
    } else {
        size * 1.04
    }
}
fn analysis_text_width(s: &str, size: f32, mono_font: bool) -> f32 {
    s.chars()
        .map(|c| analysis_char_width(c, size, mono_font))
        .sum()
}
/// Conservative wrapping keeps the word and its key recipe in one aligned block.
/// A long dictionary entry can occupy several lines without clipping its encoding.
fn analysis_wrap(s: &str, width: f32, size: f32, mono_font: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let mut buf = String::new();
    let mut used = 0.;
    for ch in s.chars() {
        if ch == '\r' {
            continue;
        }
        if ch == '\n' {
            lines.push(std::mem::take(&mut buf));
            used = 0.;
            continue;
        }
        let cw = analysis_char_width(ch, size, mono_font);
        if !buf.is_empty() && used + cw > width.max(size) {
            lines.push(std::mem::take(&mut buf));
            used = 0.;
        }
        buf.push(ch);
        used += cw;
    }
    if !buf.is_empty() || lines.is_empty() {
        lines.push(buf);
    }
    lines
}
#[derive(Debug)]
struct AnalysisBlock {
    source_index: usize,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    words: Vec<String>,
    keys: Vec<String>,
    missing: bool,
}
fn analysis_blocks(fa: &crate::FullAnalysis, m: &Model, width: f32) -> (Vec<AnalysisBlock>, f32) {
    let fs = analysis_font(m);
    let ks = (fs * 0.68).max(14.);
    let available = width.max(fs * 2.);
    let gap = (fs * 0.56).max(9.);
    let mut x = 0.;
    let mut y = 0.;
    let mut row_height: f32 = 0.;
    let mut out = Vec::new();
    for (source_index, entry) in fa.entries.iter().enumerate() {
        if entry.word == "\n" || entry.word == "\r\n" {
            y += row_height.max(fs * 2.7);
            x = 0.;
            row_height = 0.;
            continue;
        }
        let recipe = crate::analysis::entry_keys(entry, analysis_auto4(m), fa.max_code_length);
        let missing = entry.code.is_none()
            && recipe.is_empty()
            && entry.word.chars().any(|c| !c.is_whitespace());
        let keys = if missing {
            "缺字".into()
        } else if entry.rank > 1 && entry.code.is_some() {
            format!(
                "{}{}",
                crate::analysis::display_keys(entry.code.as_deref().unwrap()),
                entry.rank
            )
        } else {
            crate::analysis::display_keys(&recipe)
        };
        let key_label = if keys.is_empty() { "—" } else { &keys };
        let word_width = analysis_text_width(&entry.word, fs, false);
        let key_width = analysis_text_width(key_label, ks, true);
        let block_width = word_width.max(key_width).max(fs * 0.7).min(available);
        if x > 0. && x + block_width > available {
            y += row_height;
            x = 0.;
            row_height = 0.;
        }
        let words = analysis_wrap(&entry.word, block_width, fs, false);
        let keys = analysis_wrap(key_label, block_width, ks, true);
        let block_height = words.len() as f32 * fs * 1.4 + keys.len() as f32 * ks * 1.4 + gap;
        out.push(AnalysisBlock {
            source_index,
            x,
            y,
            w: block_width,
            h: block_height,
            words,
            keys,
            missing,
        });
        row_height = row_height.max(block_height);
        x += block_width + gap;
        if entry.word.contains('\n') {
            y += row_height;
            x = 0.;
            row_height = 0.;
        }
    }
    (out, y + row_height)
}
fn analysis_body_height(ph: f32) -> f32 {
    (ph - 274.).max(24.)
}
pub fn analysis_scroll_max(sess: &Session, m: &Model, pw: f32, ph: f32) -> f32 {
    if analysis_is_heatmap(m) {
        return 0.;
    }
    sess.full_analysis
        .as_ref()
        .map(|fa| {
            let (_, height) = analysis_blocks(fa, m, (pw - 64.).max(80.));
            (height - analysis_body_height(ph)).max(0.)
        })
        .unwrap_or(0.)
}
fn analysis_button(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    label: &str,
    marker: &str,
    t: &Theme,
    selected: bool,
) {
    let p = tokens(t);
    round(
        f,
        x,
        y,
        w,
        34.,
        6.,
        if selected {
            alpha(p.accent, 0.13)
        } else {
            p.surface2
        },
        Some(if selected {
            alpha(p.accent, 0.55)
        } else {
            p.line2
        }),
    );
    text(
        f,
        x + 12.,
        y + 9.,
        13.,
        label,
        if selected { p.accent } else { p.text2 },
    );
    hs.push(Hit {
        x,
        y,
        w,
        h: 34.,
        action: crate::aux_marker(marker),
    });
}
/// Fixed header, summary and controls; only the complete article scrolls.
fn draw_full_analysis(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    sess: &Session,
    t: &Theme,
    x: f32,
    y: f32,
    pw: f32,
    ph: f32,
) {
    let p = tokens(t);
    let auto4 = analysis_auto4(m);
    let heatmap = analysis_is_heatmap(m);
    let body_y = y + 150.;
    let body_h = analysis_body_height(ph);
    let body_w = (pw - 64.).max(80.);
    let fs = analysis_font(m);
    text(f, x + 24., y + 72., 14., "【当前词库】", p.accent);
    if let Some(fa) = &sess.full_analysis {
        f.push(DrawCmd::PushClip {
            x: x + 128.,
            y: y + 68.,
            w: pw - 152.,
            h: 24.,
        });
        text(f, x + 128., y + 72., 13., &fa.schema, p.muted);
        f.push(DrawCmd::PopClip);
        right(f, x + pw - 24., y + 71., 14., "滚轮翻页", p.muted);
        let summary = crate::analysis::summarize(fa, auto4);
        let items = [
            ("码长", format!("{:.4}", summary.avg_code_len)),
            ("字数", summary.char_count.to_string()),
            ("选重", summary.select_count.to_string()),
            ("缺字", summary.missing_chars.to_string()),
            ("已知键数", summary.known_keys.to_string()),
        ];
        let step = (pw - 48.) / 5.;
        for (i, (label, value)) in items.iter().enumerate() {
            let sx = x + 24. + i as f32 * step;
            text(f, sx, y + 101., 13., *label, p.muted);
            mono(
                f,
                sx,
                y + 121.,
                18.,
                value,
                if i == 3 && summary.missing_chars > 0 {
                    p.red
                } else {
                    p.text
                },
            );
        }
        line(f, x + 24., y + 145., pw - 48., p.line);
        f.push(DrawCmd::PushClip {
            x: x + 24.,
            y: body_y,
            w: pw - 48.,
            h: body_h,
        });
        if heatmap {
            crate::keyboard::draw(f, t, x + 32., body_y + 6., body_w, body_h - 8., |key| {
                summary.key_counts.get(&key).copied().unwrap_or(0)
            });
        } else {
            let (blocks, total_h) = analysis_blocks(fa, m, body_w);
            let scroll = m.scroll.clamp(0., (total_h - body_h).max(0.));
            let ks = (fs * 0.68).max(14.);
            for block in blocks
                .iter()
                .filter(|b| b.y + b.h >= scroll && b.y < scroll + body_h)
            {
                let bx = x + 32. + block.x;
                let by = body_y + block.y - scroll;
                f.push(DrawCmd::PushClip {
                    x: bx,
                    y: by,
                    w: block.w + 2.,
                    h: block.h,
                });
                // Keep the semantic word-length colours here too: users read
                // the analysis page as a colour-coded copy of the practice text.
                let entry = &fa.entries[block.source_index];
                let color = if block.missing {
                    p.red
                } else {
                    let code = entry.code.as_deref().unwrap_or("");
                    // Single-character drills have no word-length category;
                    // colour them by code length so 理论 is still readable as a
                    // heat map instead of a wall of white glyphs.
                    if entry.word.chars().filter(|c| c.is_alphanumeric()).count() <= 1 {
                        let n = code.chars().count().max(1);
                        p.word_colors[(n - 1).min(p.word_colors.len() - 1)]
                    } else {
                        crate::ui::hint_word_tint(&p, &entry.word, code, fa.max_code_length)
                            .unwrap_or(p.text)
                    }
                };
                let key_color = color;
                for (i, word) in block.words.iter().enumerate() {
                    crate::ui::body_text(f, bx, by + i as f32 * fs * 1.4, fs, word, color);
                }
                let key_y = by + block.words.len() as f32 * fs * 1.4;
                for (i, keys) in block.keys.iter().enumerate() {
                    mono(f, bx, key_y + i as f32 * ks * 1.4, ks, keys, key_color);
                }
                f.push(DrawCmd::PopClip);
            }
            if total_h > body_h {
                let thumb = (body_h * body_h / total_h).clamp(16., body_h);
                rect(f, x + pw - 28., body_y, 3., body_h, alpha(p.text, 0.06));
                rect(
                    f,
                    x + pw - 28.,
                    body_y + scroll / (total_h - body_h) * (body_h - thumb),
                    3.,
                    thumb,
                    alpha(p.accent, 0.65),
                );
            }
        }
        f.push(DrawCmd::PopClip);
    } else {
        let loading = m.field("analysis_loading") == "true";
        let error = m.field("analysis_error");
        text(
            f,
            x + 32.,
            body_y + 8.,
            16.,
            if loading {
                "正在分析全文，请稍候…"
            } else if error.is_empty() {
                "当前文章尚未取得理论分析"
            } else {
                "理论分析未完成"
            },
            p.text2,
        );
        if !loading {
            let detail = if error.is_empty() {
                "可重新请求；加载成功后会在此显示全文及每词编码。"
            } else {
                &error
            };
            let lines = analysis_wrap(detail, body_w, 13., false);
            let max_lines = (((body_h - 70.) / 19.).floor().max(1.) as usize).min(4);
            for (i, line) in lines.iter().take(max_lines).enumerate() {
                text(
                    f,
                    x + 32.,
                    body_y + 36. + i as f32 * 19.,
                    13.,
                    line,
                    p.muted,
                );
            }
            analysis_button(
                f,
                hs,
                x + 32.,
                body_y + body_h - 34.,
                104.,
                "重新分析",
                "analysis_retry",
                t,
                false,
            );
        }
    }
    let fy = y + ph - 108.;
    line(f, x, fy - 9., pw, p.line);
    round(
        f,
        x + 24.,
        fy + 8.,
        16.,
        16.,
        3.,
        if auto4 { p.accent } else { p.surface2 },
        Some(p.line2),
    );
    if auto4 {
        text(f, x + 27., fy + 7., 13., "✓", p.bg);
    }
    text(f, x + 49., fy + 8., 15., "四码首选自动上屏", p.text2);
    text(
        f,
        x + 286.,
        fy + 8.,
        15.,
        if pw < 760. {
            "灰码 · 铜选 · 红缺"
        } else {
            "词色按词长 / 单字按码长 · 红=缺字"
        },
        p.muted,
    );
    hs.push(Hit {
        x: x + 20.,
        y: fy,
        w: 230.,
        h: 34.,
        action: crate::aux_marker("analysis_auto4"),
    });
    analysis_button(
        f,
        hs,
        x + pw - 192.,
        fy,
        168.,
        if heatmap {
            "返回全文编码"
        } else {
            "按键热力图"
        },
        "analysis_view",
        t,
        heatmap,
    );
    let sy = y + ph - 48.;
    text(f, x + 24., sy + 3., 13., "字号", p.muted);
    let slider_x = x + 70.;
    let slider_w = (pw - 430.).clamp(80., 250.);
    let ratio = (fs - 12.) / 24.;
    rect(f, slider_x, sy + 11., slider_w, 3., p.line2);
    rect(f, slider_x, sy + 11., slider_w * ratio, 3., p.accent);
    round(
        f,
        slider_x + slider_w * ratio - 5.,
        sy + 7.,
        10.,
        10.,
        5.,
        p.accent,
        None,
    );
    for size in 12..=36 {
        let center = slider_x + slider_w * (size - 12) as f32 / 24.;
        hs.push(Hit {
            x: center - slider_w / 48.,
            y: sy - 2.,
            w: slider_w / 24. + 1.,
            h: 34.,
            action: crate::aux_marker(&format!("analysis_font:{size}")),
        });
    }
    mono(
        f,
        slider_x + slider_w + 14.,
        sy + 3.,
        13.,
        format!("{fs:.0}"),
        p.text2,
    );
    analysis_button(
        f,
        hs,
        x + pw - 240.,
        sy - 3.,
        140.,
        "复制当前图片",
        "analysis_copy",
        t,
        false,
    );
    btn(
        f,
        hs,
        x + pw - 88.,
        sy - 3.,
        64.,
        "关闭",
        Action::Close,
        t,
        true,
    );
}

fn modal_content_height(m: &Model, pw: f32) -> f32 {
    match m.modal {
        Modal::Preferences => preferences::content_height(m, pw),
        Modal::Online => crate::secondary_panels::content_height_online(m),
        Modal::Rules => 900.,
        _ => 0.,
    }
}

pub fn draw(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    sess: &Session,
    cfg: &Config,
    t: &Theme,
    w: f32,
    h: f32,
    now: f64,
) {
    let p = tokens(t);
    if m.modal != Modal::None {
        let full_page = m.modal == Modal::Report;
        let (x, y, pw, ph) = panel(m.modal, w, h);
        if full_page {
            hs.retain(|hit| hit.y < 55. || hit.y >= h - 37.);
            rect(f, 0., 55., w, h - 92., p.bg);
            crate::secondary_panels::draw(f, hs, m, sess, cfg, t, x, y, pw, ph);
        } else {
            hs.clear();
            f.push(DrawCmd::BlurBackdrop { radius: 7. });
            rect(f, 0., 0., w, h, [0., 0., 0., 0.72]);
            hs.push(Hit {
                x: 0.,
                y: 0.,
                w,
                h,
                action: Action::Close,
            });
            let command_start = f.cmds.len();
            round(f, x, y, pw, ph, 16., p.surface, Some(p.line2));
            hs.push(Hit {
                x,
                y,
                w: pw,
                h: ph,
                action: Action::None,
            });
            if !matches!(m.modal, Modal::Online | Modal::Rules) {
                panel_header(f, hs, m.modal, x, y, pw, t);
            }
            match m.modal {
                Modal::Library => draw_library(f, hs, m, t, x, y, pw, ph),
                Modal::Groups => draw_groups(f, hs, m, sess, cfg, t, x, y, pw, ph),
                Modal::Appearance => draw_appearance(f, hs, m, sess, cfg, t, x, y, pw, ph),
                Modal::Preferences => preferences::draw(f, hs, m, t, x, y, pw, ph),
                Modal::Heatmap => draw_heatmap(f, hs, sess, t, x, y, pw, ph),
                Modal::FullAnalysis => draw_full_analysis(f, hs, m, sess, t, x, y, pw, ph),
                Modal::Online | Modal::Rules => {
                    crate::secondary_panels::draw(f, hs, m, sess, cfg, t, x, y, pw, ph)
                }
                Modal::Confirm => {
                    text_block(
                        f,
                        x + 24.,
                        y + 92.,
                        pw - 48.,
                        15.,
                        28.,
                        4,
                        &m.confirm,
                        p.text2,
                    );
                    footer(f, x, y, pw, ph, t);
                    btn(
                        f,
                        hs,
                        x + pw - 226.,
                        y + ph - 47.,
                        96.,
                        "取消",
                        Action::ConfirmNo,
                        t,
                        false,
                    );
                    btn(
                        f,
                        hs,
                        x + pw - 122.,
                        y + ph - 47.,
                        98.,
                        "继续",
                        Action::ConfirmYes,
                        t,
                        true,
                    );
                }
                _ => {}
            }
            if m.modal != Modal::FullAnalysis {
                fit_panel(
                    f,
                    hs,
                    command_start,
                    x,
                    y,
                    pw,
                    ph,
                    panel_view(ph, h),
                    m.panel_scroll,
                    if m.modal == Modal::Preferences {
                        50.
                    } else {
                        0.
                    },
                    modal_content_height(m, pw),
                );
            }
        }
    }
    if let Some((msg, at)) = &m.toast {
        if now - at < 3.6 {
            let tw = (msg.chars().count() as f32 * 13. + 36.).min(w - 40.);
            round(
                f,
                (w - tw) / 2.,
                h - 100.,
                tw,
                40.,
                9.,
                p.surface,
                Some(alpha(p.accent, 0.35)),
            );
            text(
                f,
                (w - tw) / 2. + 16.,
                h - 89.,
                FONT_HINT,
                elide(msg, ((tw - 32.) / FONT_HINT) as usize),
                p.text,
            );
        }
    }
}
pub fn wrap(s: &str, n: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in s.lines() {
        let mut buf = String::new();
        let mut count = 0;
        for c in line.chars() {
            buf.push(c);
            count += 1;
            if count >= n.max(1) {
                out.push(std::mem::take(&mut buf));
                count = 0;
            }
        }
        if !buf.is_empty() {
            out.push(buf);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn analysis_session(entries: usize) -> Session {
        let mut s = Session::new(100);
        s.full_analysis = Some(crate::FullAnalysis {
            schema: "测试词库".into(),
            max_code_length: 4,
            entries: (0..entries)
                .map(|i| crate::FullAnalysisEntry {
                    word: if i + 1 == entries {
                        "最后一词".into()
                    } else {
                        "理论编码".into()
                    },
                    code: Some("abcd".into()),
                    rank: 1,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
        s
    }
    #[test]
    fn physical_heatmap_uses_visible_height_and_keeps_footer_button_complete() {
        for (w, h) in [(800., 600.), (640., 420.)] {
            let mut model = Model::default();
            model.open(Modal::Heatmap);
            let (x, y, pw, ph) = panel(model.modal, w, h);
            assert!(ph <= h - 48.);
            let mut frame = Frame::new(w, h);
            let mut hits = Vec::new();
            draw_heatmap(
                &mut frame,
                &mut hits,
                &Session::new(100),
                &crate::ui::theme("墨玉"),
                x,
                y,
                pw,
                ph,
            );
            let close = hits.iter().find(|hit| hit.action == Action::Close).unwrap();
            assert_eq!(close.h, 34.);
            assert!(close.y >= 0. && close.y + close.h <= h);
            // The keyboard rectangle ends twelve pixels above the footer.
            let top = if ph >= 480. { 112. } else { 76. };
            assert!(y + top + (ph - top - 74.) <= y + ph - PANEL_FOOT);
        }
    }
    #[test]
    fn full_analysis_scroll_reaches_entries_beyond_old_limit_without_moving_controls() {
        let s = analysis_session(1600);
        let mut m = Model::default();
        m.open(Modal::FullAnalysis);
        let cfg = Config::default();
        let t = crate::ui::theme("墨玉");
        let (_, _, pw, ph) = panel(m.modal, 1280., 800.);
        let max = analysis_scroll_max(&s, &m, pw, ph);
        assert!(max > ph);
        let mut top = Frame::new(1280., 800.);
        let mut top_hits = Vec::new();
        draw(&mut top, &mut top_hits, &m, &s, &cfg, &t, 1280., 800., 0.);
        m.scroll = max;
        let mut bottom = Frame::new(1280., 800.);
        let mut bottom_hits = Vec::new();
        draw(
            &mut bottom,
            &mut bottom_hits,
            &m,
            &s,
            &cfg,
            &t,
            1280.,
            800.,
            0.,
        );
        assert!(bottom
            .cmds
            .iter()
            .any(|c| matches!(c, DrawCmd::TextRun { text, .. } if text == "最后一词")));
        let copy = crate::aux_marker("analysis_copy");
        let a = top_hits.iter().find(|h| h.action == copy).unwrap();
        let b = bottom_hits.iter().find(|h| h.action == copy).unwrap();
        assert_eq!((a.x, a.y, a.w, a.h), (b.x, b.y, b.w, b.h));
        assert!(bottom_hits.iter().all(|h| h.y >= 0. && h.y + h.h <= 800.));
    }
    #[test]
    fn full_analysis_fits_small_viewport_and_has_all_footer_actions() {
        let s = analysis_session(12);
        let mut m = Model::default();
        m.open(Modal::FullAnalysis);
        let (_, y, _, ph) = panel(m.modal, 640., 420.);
        assert!(y >= 0. && y + ph <= 420.);
        let mut f = Frame::new(640., 420.);
        let mut hits = Vec::new();
        draw(
            &mut f,
            &mut hits,
            &m,
            &s,
            &Config::default(),
            &crate::ui::theme("墨玉"),
            640.,
            420.,
            0.,
        );
        for marker in [
            "analysis_auto4",
            "analysis_view",
            "analysis_copy",
            "analysis_font:12",
            "analysis_font:36",
        ] {
            let h = hits
                .iter()
                .find(|h| h.action == crate::aux_marker(marker))
                .unwrap();
            assert!(h.x >= 0. && h.y >= 0. && h.x + h.w <= 640. && h.y + h.h <= 420.);
        }
    }
    #[test]
    fn analysis_wrap_keeps_long_word_and_recipe_in_bounded_blocks() {
        let mut s = analysis_session(1);
        let fa = s.full_analysis.as_mut().unwrap();
        fa.entries[0].word = "很长的词条".repeat(20);
        fa.entries[0].code = Some("abcdefghijklmnop".repeat(12));
        let (blocks, height) = analysis_blocks(fa, &Model::default(), 250.);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].words.concat(), fa.entries[0].word);
        assert_eq!(
            blocks[0].keys.concat(),
            crate::analysis::display_keys(&crate::analysis::entry_keys(&fa.entries[0], true, 4))
        );
        assert!(blocks[0].w <= 250. && height > 100.);
    }
    #[test]
    fn analysis_colors_only_words_and_shows_selection_rank_without_changing_key_counts() {
        let entry = |word: &str, code: Option<&str>, rank, tail: &str| crate::FullAnalysisEntry {
            word: word.into(),
            code: code.map(str::to_owned),
            rank,
            tail: tail.into(),
            ..Default::default()
        };
        let fa = crate::FullAnalysis {
            max_code_length: 4,
            entries: vec![
                entry("甲", Some("a"), 1, "_"),
                entry("词组", Some("ab"), 2, ";"),
                entry("，", None, 0, ""),
                entry("另一词", Some("cd"), 3, "'"),
                entry("缺字", None, 0, ""),
                entry("乙", Some("e"), 1, "_"),
                entry("最后词", Some("fg"), 1, "_"),
            ],
            ..Default::default()
        };
        let (blocks, _) = analysis_blocks(&fa, &Model::default(), 900.);
        let p = tokens(&crate::ui::theme("墨玉"));
        let colors: Vec<_> = blocks
            .iter()
            .map(|block| {
                let entry = &fa.entries[block.source_index];
                entry.code.as_deref().and_then(|code| {
                    crate::ui::hint_word_tint(&p, &entry.word, code, fa.max_code_length)
                })
            })
            .collect();
        let short = crate::ui::hint_word_tint(&p, "简码", "ab", 4).unwrap();
        assert_eq!(
            colors,
            vec![
                None,
                Some(short),
                None,
                Some(short),
                None,
                None,
                Some(short)
            ]
        );
        assert_eq!(blocks[0].keys.concat(), "a_");
        assert_eq!(blocks[1].keys.concat(), "ab2");
        assert_eq!(blocks[3].keys.concat(), "cd3");
        assert!(blocks[4].missing);
        let summary = crate::analysis::summarize(&fa, true);
        assert_eq!(summary.key_counts.get(&0xba), Some(&1));
        assert_eq!(summary.key_counts.get(&0xde), Some(&1));
        assert!(!summary.key_counts.contains_key(&b'2'));
        assert!(!summary.key_counts.contains_key(&b'3'));
    }
    #[test]
    fn appearance_preview_uses_all_word_hints_and_keeps_single_characters_uncolored() {
        let mut s = Session::new(100);
        s.load_book("预览", "甲乙丙丁戊己庚辛壬癸");
        let mut m = Model::default();
        m.hints = true;
        m.words = (0..5)
            .map(|i| HintWord {
                text: ["甲", "乙丙", "丁戊", "己庚", "辛壬癸"][i].into(),
                code: Some(format!("a{i}")),
                tail: ";".into(),
                start: [0, 1, 3, 5, 7][i],
                len: [1, 2, 2, 2, 3][i],
                rank: if i == 0 { 2 } else { 1 },
            })
            .collect();
        let cfg = Config::default();
        let t = crate::ui::theme("墨玉");
        let p = tokens(&t);
        let mut frame = Frame::new(800., 300.);
        appearance_preview(&mut frame, &s, &m, &cfg, &t, 0., 0., 700., 250.);
        let codes: Vec<_> = frame
            .cmds
            .iter()
            .filter_map(|cmd| match cmd {
                DrawCmd::TextRun { text, color, .. } if text.starts_with('a') => {
                    Some((text.as_str(), *color))
                }
                _ => None,
            })
            .collect();
        assert_eq!(codes.len(), 5);
        assert_eq!(codes[0].1, p.text2);
        assert_eq!(
            codes[1].1,
            crate::ui::hint_word_tint(&p, "乙丙", "a1", 4).unwrap()
        );
        assert_eq!(
            codes[4].1,
            crate::ui::hint_word_tint(&p, "辛壬癸", "a4", 4).unwrap()
        );
        assert!(frame.cmds.iter().any(|cmd| matches!(cmd,
            DrawCmd::TextRun { text, color, .. } if text == "2" && *color == p.text2)));
    }
    #[test]
    fn small_panel_keeps_primary_actions_visible() {
        let mut m = Model::default();
        m.open(Modal::Library);
        let s = Session::new(100);
        let cfg = Config::default();
        let t = crate::ui::theme("墨玉");
        let mut f = Frame::new(640., 420.);
        let mut hits = Vec::new();
        draw(&mut f, &mut hits, &m, &s, &cfg, &t, 640., 420., 0.);
        let close = hits.iter().find(|h| h.action == Action::Close).unwrap();
        assert!(close.y >= 0. && close.y + close.h <= 420.);
    }
    #[test]
    fn rules_editors_scroll_within_viewport() {
        let mut m = Model::default();
        m.open(Modal::Rules);
        m.panel_scroll = 288.;
        for field in fields(&m, 640., 420.) {
            assert!(field.h >= 0.);
            assert!(field.y >= 0.);
            if field.h > 0. {
                assert!(field.y + field.h <= 420.);
            }
        }
    }
    #[test]
    fn panel_scroll_preserves_nested_clips_and_footer_hits() {
        let mut f = Frame::new(640., 420.);
        round(&mut f, 16., 24., 608., 596., 16., [0.; 4], None);
        f.push(DrawCmd::PushClip {
            x: 30.,
            y: 120.,
            w: 500.,
            h: 300.,
        });
        text(&mut f, 40., 160., 12., "body", [1.; 4]);
        f.push(DrawCmd::PopClip);
        text(&mut f, 40., 577., 12., "footer", [1.; 4]);
        let mut hits = vec![Hit {
            x: 40.,
            y: 573.,
            w: 80.,
            h: 32.,
            action: Action::Close,
        }];
        fit_panel(
            &mut f, &mut hits, 0, 16., 24., 608., 596., 372., 100., 0., 0.,
        );
        let mut depth = 0;
        for c in &f.cmds {
            match c {
                DrawCmd::PushClip { .. } => depth += 1,
                DrawCmd::PopClip => {
                    depth -= 1;
                    assert!(depth >= 0);
                }
                _ => {}
            }
        }
        assert_eq!(depth, 0);
        assert!(hits[0].y + hits[0].h <= 420.);
    }
}
