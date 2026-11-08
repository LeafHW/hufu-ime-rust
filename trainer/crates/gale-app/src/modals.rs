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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Modal {
    #[default]
    None,
    Load,
    Groups,
    Appearance,
    Result,
    Report,
    Online,
    Rules,
    Confirm,
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
    LoadClipboard,
    LoadFile,
    LoadText,
    AcceptDraft,
    LoadBook(usize),
    Practice(usize),
    LoadTab(u8),
    LoadEdit,
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
    if matches!(m, Modal::Result | Modal::Report) {
        return (0., 55., w, (h - 92.).max(160.));
    }
    let (ideal_w, ph): (f32, f32) = match m {
        Modal::Load | Modal::Online => (880., 596.),
        Modal::Groups => (940., 620.),
        Modal::Appearance => (1064., 668.),
        Modal::Rules => (1040., 660.),
        Modal::Confirm => (560., 300.),
        _ => (880., 596.),
    };
    let pw = ideal_w.min((w - 32.).max(200.));
    let view = panel_view(ph, h);
    ((w - pw) / 2., ((h - view) / 2.).max(16.), pw, ph)
}
fn panel_view(ph: f32, h: f32) -> f32 {
    ph.min((h - 48.).max(PANEL_HEAD + PANEL_FOOT + 48.))
}
fn load_side(pw: f32) -> f32 {
    232f32.min(pw * 0.29).max(154.)
}
pub fn fields(m: &Model, w: f32, h: f32) -> Vec<FieldSpec> {
    let (x, y, pw, ph) = panel(m.modal, w, h);
    let mut v = match m.modal {
        Modal::Load if m.load_tab == 0 && m.load_editing => {
            let side = load_side(pw);
            vec![FieldSpec {
                id: "load_text",
                x: x + side + 38.,
                y: y + 204.,
                w: (pw - side - 76.).max(80.),
                h: ph - 344.,
                multi: true,
                password: false,
            }]
        }
        Modal::Online | Modal::Rules => crate::secondary_panels::fields(m, x, y, pw, ph),
        _ => Vec::new(),
    };
    let top = y + PANEL_HEAD;
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
) {
    let body_top = y + PANEL_HEAD;
    let body_end = y + ph - PANEL_FOOT;
    let visible_end = y + view - PANEL_FOOT;
    let scroll = scroll.clamp(0., (ph - view).max(0.));
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
    if view < ph {
        let track = (visible_end - body_top).max(1.);
        let thumb = (track * track / (body_end - body_top)).clamp(12., track);
        rect(f, x + pw - 5., body_top, 2., track, [0.5, 0.5, 0.5, 0.12]);
        rect(
            f,
            x + pw - 5.,
            body_top + scroll / (ph - view) * (track - thumb),
            2.,
            thumb,
            [0.78, 0.65, 0.38, 0.7],
        );
    }
}
pub(crate) fn label(f: &mut Frame, x: f32, y: f32, s: impl Into<String>, t: &Theme) {
    text(f, x, y, FONT_HINT, s, tokens(t).muted);
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
        32.,
        8.,
        if primary { p.accent } else { p.surface2 },
        if primary { None } else { Some(p.line2) },
    );
    text(
        f,
        x + 11.,
        y + 8.,
        FONT_HINT,
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
        h: 32.,
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
        25.,
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
    text(
        f,
        x + 8.,
        y + 5.,
        FONT_HINT,
        s,
        if selected { p.accent } else { p.muted },
    );
    hs.push(Hit {
        x,
        y,
        w,
        h: 25.,
        action: a,
    });
}
fn disabled(f: &mut Frame, x: f32, y: f32, w: f32, s: &str, t: &Theme) {
    let p = tokens(t);
    round(f, x, y, w, 25., 6., alpha(p.bg2, 0.8), Some(p.line));
    text(f, x + 7., y + 5., FONT_HINT, s, alpha(p.muted, 0.5));
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
        _ => {
            outline(2., 2., 14., 16., 2.);
            outline(6., 0., 6., 4., 1.);
        }
    }
}
fn panel_header(f: &mut Frame, hs: &mut Vec<Hit>, m: Modal, x: f32, y: f32, pw: f32, t: &Theme) {
    let p = tokens(t);
    let (icon_name, title, desc, key) = match m {
        Modal::Load => (
            "clipboard",
            "载文",
            "从剪贴板、群文、本地文库或在线取一篇",
            "F2",
        ),
        Modal::Groups => ("group", "群与通道", "从 QQ 群取赛文，把成绩带回去", "F6"),
        Modal::Appearance => ("palette", "外观", "主题、材质与词提；改动即时生效", "F7"),
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
    text(f, x + 64., y + 13., FONT_LABEL, title, p.text);
    text(f, x + 64., y + 35., FONT_HINT, desc, p.muted);
    small_btn(
        f,
        hs,
        x + pw - 86.,
        y + 20.,
        30.,
        key,
        Action::None,
        t,
        false,
    );
    text(f, x + pw - 42., y + 16., 24., "×", p.muted);
    hit_box(hs, x + pw - 48., y + 12., 36., 38., Action::Close);
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
fn draw_load(
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
    let side = load_side(pw);
    let body_h = ph - PANEL_HEAD - PANEL_FOOT;
    rect(f, x + 1., y + PANEL_HEAD, side - 1., body_h, p.bg2);
    rect(f, x + side, y + PANEL_HEAD, 1., body_h, p.line);
    text(f, x + 22., y + 80., FONT_HINT, "来源", p.muted);
    let draft = m.field("load_text");
    let sources = [
        (
            0u8,
            "剪贴板",
            format!("{} 字 · 可编辑", draft.chars().count()),
            "clipboard",
        ),
        (
            2,
            "群文",
            if m.groups.is_empty() {
                "连接 QQ 读取当前群文".into()
            } else {
                format!("{} 个会话", m.groups.len())
            },
            "group",
        ),
        (
            1,
            "本地文库",
            format!("{} 篇 · 本地文本", m.books.len()),
            "folder",
        ),
        (3, "在线", "晴发文 · 在线书架".into(), "globe"),
    ];
    for (i, (tab, title, sub, icon_name)) in sources.iter().enumerate() {
        let sy = y + 100. + i as f32 * 55.;
        if m.load_tab == *tab {
            round(
                f,
                x + 13.,
                sy,
                side - 26.,
                52.,
                9.,
                p.surface2,
                Some(p.line),
            );
            round(f, x + 13., sy + 4., 2., 44., 1., p.accent, None);
        }
        icon(
            f,
            icon_name,
            x + 25.,
            sy + 13.,
            17.,
            if m.load_tab == *tab {
                p.accent
            } else {
                p.muted
            },
        );
        text(f, x + 51., sy + 9., FONT_HINT, *title, p.text2);
        text(
            f,
            x + 51.,
            sy + 30.,
            FONT_HINT,
            elide(sub, ((side - 72.) / FONT_HINT) as usize),
            p.muted,
        );
        right(f, x + side - 25., sy + 17., 12., "›", p.muted);
        hit_box(hs, x + 13., sy, side - 26., 52., Action::LoadTab(*tab));
    }
    line(f, x + 22., y + ph - 138., side - 44., p.line);
    text(f, x + 22., y + ph - 124., FONT_HINT, "当前文档", p.muted);
    text(
        f,
        x + 22.,
        y + ph - 105.,
        FONT_HINT,
        elide(&sess.title, ((side - 44.) / FONT_HINT) as usize),
        p.text2,
    );
    let rx = x + side + 20.;
    let rw = pw - side - 40.;
    let top = y + 80.;
    match m.load_tab {
        0 => {
            let group = m.bound.and_then(|i| m.groups.get(i));
            let chip_w = (rw - 220.).clamp(130., 230.);
            round(f, rx, top, chip_w, 30., 15., p.surface2, Some(p.line));
            round(f, rx + 4., top + 4., 22., 22., 6., p.accent, None);
            body_text(
                f,
                rx + 10.,
                top + 6.,
                12.,
                group
                    .and_then(|s| s.chars().next())
                    .unwrap_or('文')
                    .to_string(),
                [0.13, 0.09, 0.035, 1.],
            );
            text(
                f,
                rx + 34.,
                top + 8.,
                FONT_HINT,
                group
                    .map(|s| elide(s, ((chip_w - 50.) / FONT_HINT) as usize))
                    .unwrap_or_else(|| "剪贴板 / 自定义".into()),
                p.text2,
            );
            small_btn(
                f,
                hs,
                rx + rw - 178.,
                top + 3.,
                98.,
                "读取剪贴板",
                Action::LoadClipboard,
                t,
                false,
            );
            small_btn(
                f,
                hs,
                rx + rw - 72.,
                top + 3.,
                72.,
                if m.load_editing {
                    "预览"
                } else if draft.trim().is_empty() {
                    "开始编辑"
                } else {
                    "编辑"
                },
                Action::LoadEdit,
                t,
                m.load_editing,
            );
            let parsed = if draft.contains("-----第") {
                gale_core::segment::parse_group_text(&draft)
            } else {
                gale_core::segment::ParsedGroupText {
                    title: "自定义文字".into(),
                    difficulty: None,
                    segments: gale_core::segment::split_smart(&draft, cfg.seg_chars),
                }
            };
            let char_count: usize = parsed.segments.iter().map(|s| s.text.chars().count()).sum();
            round(f, rx, y + 121., rw, ph - 224., 12., p.surface, Some(p.line));
            let title = if m.load_editing {
                "编辑原文".to_string()
            } else if draft.trim().is_empty() {
                "等待一篇文字".into()
            } else {
                parsed.title.clone()
            };
            body_text(
                f,
                rx + 18.,
                y + 138.,
                19.,
                elide(&title, ((rw - 36.) / 19.) as usize),
                p.text,
            );
            text(
                f,
                rx + 18.,
                y + 169.,
                FONT_HINT,
                if m.load_editing {
                    "保留标题和段标；纯文本按设置字数切段".into()
                } else if draft.trim().is_empty() {
                    "剪贴板 / 自定义文字".into()
                } else {
                    format!(
                        "{} 段  ·  {} 字  ·  首键计时",
                        parsed.segments.len(),
                        char_count
                    )
                },
                p.muted,
            );
            if m.load_editing {
                // Native editor FieldSpec starts at y+204, below title and metadata.
                round(
                    f,
                    rx + 17.,
                    y + 203.,
                    rw - 34.,
                    ph - 342.,
                    6.,
                    p.bg2,
                    Some(p.line),
                );
            } else if draft.trim().is_empty() {
                body_text(f, rx + 18., y + 217., 17., "把想练的文字放在这里", p.text2);
                text_block(f, rx + 18., y + 259., rw - 36., FONT_HINT, 27., 4,
                    "按 Ctrl+V 读取剪贴板，或点击“开始编辑”直接输入。\n本地文库支持打开 TXT；在线来源可从晴发文取文。", p.muted);
                btn(
                    f,
                    hs,
                    rx + 18.,
                    y + 352.,
                    110.,
                    "开始编辑",
                    Action::LoadEdit,
                    t,
                    false,
                );
                btn(
                    f,
                    hs,
                    rx + 138.,
                    y + 352.,
                    110.,
                    "打开 TXT",
                    Action::LoadFile,
                    t,
                    false,
                );
            } else {
                let preview = parsed
                    .segments
                    .iter()
                    .take(2)
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                text_block(
                    f,
                    rx + 18.,
                    y + 205.,
                    rw - 36.,
                    13.5,
                    27.3,
                    6,
                    &preview,
                    p.text2,
                );
                text(f, rx + 18., y + ph - 170., FONT_HINT, "分段", p.muted);
                let max_chips = (((rw - 60.) / 104.).floor() as usize).clamp(1, 5);
                for (i, segment) in parsed.segments.iter().take(max_chips).enumerate() {
                    let sx = rx + 50. + i as f32 * 104.;
                    round(
                        f,
                        sx,
                        y + ph - 174.,
                        96.,
                        22.,
                        5.,
                        if i == 0 { alpha(p.accent, 0.08) } else { p.bg2 },
                        Some(if i == 0 { alpha(p.accent, 0.4) } else { p.line }),
                    );
                    text(
                        f,
                        sx + 7.,
                        y + ph - 170.,
                        FONT_HINT,
                        format!(
                            "{}·{}字",
                            elide(&segment.mark, 3),
                            segment.text.chars().count()
                        ),
                        if i == 0 { p.accent } else { p.muted },
                    );
                }
            }
            line(f, rx + 18., y + ph - 132., rw - 36., p.line);
            text(
                f,
                rx + 18.,
                y + ph - 119.,
                FONT_HINT,
                if m.load_editing {
                    "正在编辑原文 · 点击预览查看分段"
                } else {
                    "剪贴板 · 保留标题与段标"
                },
                p.muted,
            );
            if !m.pending_source.is_empty() {
                small_btn(
                    f,
                    hs,
                    rx + rw - 138.,
                    y + ph - 139.,
                    120.,
                    "显示待读文字",
                    Action::AcceptDraft,
                    t,
                    false,
                );
            }
        }
        1 => {
            text(f, rx, top + 3., FONT_HINT, "本地文库", p.text2);
            small_btn(
                f,
                hs,
                rx + rw - 88.,
                top,
                88.,
                "打开 TXT",
                Action::LoadFile,
                t,
                false,
            );
            round(f, rx, y + 121., rw, 210., 12., p.surface, Some(p.line));
            if m.books.is_empty() {
                body_text(f, rx + 18., y + 146., 19., "文库还没有文章", p.text);
                text_block(
                    f,
                    rx + 18.,
                    y + 184.,
                    rw - 36.,
                    FONT_HINT,
                    26.,
                    3,
                    "打开一个 TXT 文件，或把文本放进数据目录的文库文件夹。",
                    p.muted,
                );
            } else {
                for (row, (i, title)) in m
                    .books
                    .iter()
                    .enumerate()
                    .skip(m.scroll.max(0.) as usize)
                    .take(5)
                    .enumerate()
                {
                    small_btn(
                        f,
                        hs,
                        rx + 14.,
                        y + 136. + row as f32 * 35.,
                        rw - 28.,
                        &elide(title, ((rw - 50.) / 10.) as usize),
                        Action::LoadBook(i),
                        t,
                        false,
                    );
                }
            }
            text(f, rx, y + 347., FONT_HINT, "单字练习", p.text2);
            text(
                f,
                rx + 76.,
                y + 349.,
                FONT_HINT,
                "五份内置字表 · 居中行字提",
                p.muted,
            );
            for (i, (title, _)) in gale_core::single::builtin_tables().iter().enumerate() {
                let cw = (rw - 12.) / 3.;
                small_btn(
                    f,
                    hs,
                    rx + (i % 3) as f32 * (cw + 6.),
                    y + 371. + (i / 3) as f32 * 33.,
                    cw,
                    title,
                    Action::Practice(i),
                    t,
                    false,
                );
            }
        }
        2 => {
            let bound = m.bound.and_then(|i| m.groups.get(i));
            round(f, rx, top, rw, 31., 15., p.surface2, Some(p.line));
            text(
                f,
                rx + 12.,
                top + 8.,
                FONT_HINT,
                bound
                    .map(|s| elide(s, (((rw - 24.) / FONT_HINT) as usize).max(8)))
                    .unwrap_or_else(|| "尚未选择 QQ 会话".into()),
                p.text2,
            );
            round(f, rx, y + 126., rw, ph - 230., 12., p.surface, Some(p.line));
            body_text(f, rx + 18., y + 145., 19., "从正在打开的群里取文", p.text);
            text_block(
                f,
                rx + 18.,
                y + 187.,
                rw - 36.,
                FONT_HINT,
                26.,
                4,
                "先在 QQ 中打开目标群，再读取当前群文。也可以手动复制赛文，回到剪贴板来源粘贴。",
                p.text2,
            );
            btn(
                f,
                hs,
                rx + 18.,
                y + 299.,
                122.,
                "群与通道 F6",
                Action::Open(Modal::Groups),
                t,
                false,
            );
            btn(
                f,
                hs,
                rx + 150.,
                y + 299.,
                122.,
                "读取当前群文",
                Action::ReadGroup,
                t,
                false,
            );
            text_block(
                f,
                rx + 18.,
                y + 352.,
                rw - 36.,
                FONT_HINT,
                26.,
                3,
                "UIA 依赖 QQ 界面结构；不能自动切群。读取失败时可使用剪贴板。",
                p.muted,
            );
        }
        _ => {
            round(f, rx, y + 121., rw, ph - 224., 12., p.surface, Some(p.line));
            body_text(f, rx + 22., y + 145., 23., "晴发文 · 在线书架", p.text);
            text_block(
                f,
                rx + 22.,
                y + 195.,
                rw - 44.,
                13.,
                27.,
                4,
                "按分类、难度和字数取文。登录后可浏览书架，断网时已载入的正文仍可继续练习。",
                p.text2,
            );
            btn(
                f,
                hs,
                rx + 22.,
                y + 300.,
                150.,
                "打开在线发文 F8",
                Action::Open(Modal::Online),
                t,
                true,
            );
        }
    }
    footer(f, x, y, pw, ph, t);
    if pw > 700. {
        text(
            f,
            x + 20.,
            y + ph - 36.,
            FONT_HINT,
            "Ctrl V 读取剪贴板 · Esc 返回",
            p.muted,
        );
    }
    let fy = y + ph - 47.;
    btn(
        f,
        hs,
        x + pw - 292.,
        fy,
        78.,
        "清洗",
        Action::Open(Modal::Rules),
        t,
        false,
    );
    btn(
        f,
        hs,
        x + pw - 206.,
        fy,
        66.,
        "关闭",
        Action::Close,
        t,
        false,
    );
    let (label, action) = match m.load_tab {
        2 => ("读取当前群文", Action::ReadGroup),
        3 => ("在线取文", Action::Open(Modal::Online)),
        1 => ("打开 TXT", Action::LoadFile),
        _ => ("载入并开始  ↵", Action::LoadText),
    };
    btn(f, hs, x + pw - 132., fy, 112., label, action, t, true);
}
fn draw_groups(
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
            "打开 QQ 和目标群后扫描。\n也可以手动复制赛文，按 F2 载入。",
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
                if selected {
                    "已选择 · 请在 QQ 手动打开"
                } else {
                    "QQ 会话 · 点击选择"
                },
                p.muted,
            );
            hit_box(hs, x + 12., gy, side - 24., 64., Action::SelectGroup(i));
        }
    }
    line(f, x + 22., y + ph - 108., side - 44., p.line);
    text(f, x + 22., y + ph - 92., FONT_HINT, "剪贴板通道始终可用", p.muted);
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
        y + 83.,
        14.,
        group
            .map(|s| elide(s, ((rw - 54.) / 14.) as usize))
            .unwrap_or_else(|| "选择一个会话".into()),
        p.text,
    );
    text(
        f,
        rx + 54.,
        y + 107.,
        FONT_HINT,
        "只读取当前打开会话；不会自动切群",
        p.muted,
    );
    round(f, rx, y + 138., rw, 51., 9., p.surface2, Some(p.line));
    let vals = [
        (
            "会话",
            if group.is_some() {
                "已选择".to_string()
            } else {
                "待选择".into()
            },
        ),
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
        let sx = rx + i as f32 * rw / 4.;
        if i > 0 {
            rect(f, sx, y + 138., 1., 51., p.line);
        }
        mono(f, sx + 10., y + 149., 13., v, p.text2);
        text(f, sx + 10., y + 169., FONT_HINT, l, p.muted);
    }
    round(f, rx, y + 201., rw, 69., 10., p.surface, Some(p.line));
    body_text(f, rx + 12., y + 214., 16., "文", p.accent);
    text(
        f,
        rx + 44.,
        y + 213.,
        FONT_HINT,
        elide(&sess.title, ((rw - 64.) / FONT_HINT) as usize),
        p.text2,
    );
    text(
        f,
        rx + 44.,
        y + 237.,
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
    round(f, rx, y + 282., rw, 126., 10., p.surface2, Some(p.line));
    text(f, rx + 12., y + 296., FONT_HINT, "UIA 读取通道", p.text2);
    text_block(
        f,
        rx + 12.,
        y + 320.,
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
        y + 359.,
        rw - 24.,
        35.,
        6.,
        alpha(p.accent, 0.055),
        Some(alpha(p.accent, 0.23)),
    );
    text_block(
        f,
        rx + 20.,
        y + 366.,
        rw - 40.,
        FONT_HINT,
        24.,
        2,
        "QQ 更新可能使 UIA 失效。失败时请手动复制，通过剪贴板载入。",
        p.accent,
    );
    line(f, rx, y + ph - 139., rw, p.line);
    text(f, rx, y + ph - 126., FONT_HINT, "通道状态", p.muted);
    text(
        f,
        rx,
        y + ph - 105.,
        FONT_HINT,
        elide(&m.status, ((rw / FONT_HINT) as usize).max(8)),
        p.text2,
    );
    footer(f, x, y, pw, ph, t);
    if pw > 740. {
        text(
            f,
            x + 20.,
            y + ph - 35.,
            FONT_HINT,
            "发成绩时先复制，再确认目标群并粘贴",
            p.muted,
        );
    }
    let fy = y + ph - 47.;
    btn(
        f,
        hs,
        x + pw - 330.,
        fy,
        108.,
        "复制本段成绩",
        Action::CopyScore,
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
    let fs = (cfg.font_size / 2.).clamp(12., 32.);
    let lh = fs * (1. + cfg.line_gap) + 9.;
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
        if gale_core::rules::hints_effective(m.hints, m.race) && current >= offset && current < end {
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
        for (i, ch) in sample[offset..end].iter().enumerate() {
            let color = match sess.comparer.states.get(offset + i) {
                Some(gale_core::compare::CharState::Correct) => p.text,
                Some(gale_core::compare::CharState::Wrong) => p.red,
                _ => p.text2,
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
        if gale_core::rules::hints_effective(m.hints, m.race) && cfg.hint_style != 2 {
            for (i, word) in m.words.iter().enumerate() {
                let visible = if cfg.hint_style == 1 {
                    true
                } else {
                    i >= word_index && i < word_index + 3 && word.len > 1
                };
                if !visible || word.start < offset || word.start >= end {
                    continue;
                }
                if let Some(code) = &word.code {
                    let c = if word.rank > 1 {
                        p.red
                    } else {
                        match code.len() {
                            1 => p.accent,
                            2 => p.copper,
                            3 => p.jade,
                            _ => p.muted,
                        }
                    };
                    let xx = x + (word.start - offset) as f32 * fs;
                    mono(f, xx, yy + fs + 5., 7.5, code, c);
                    if word.rank > 1 {
                        mono(
                            f,
                            xx + code.len() as f32 * 4.5,
                            yy + fs + 2.,
                            6.,
                            word.rank.to_string(),
                            p.red,
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
        (p.bg, "back_color"),
        (p.text, "text_color"),
        (p.accent, "hilited_back"),
        (p.red, "hilited_label"),
        (p.line2, "border_color"),
    ];
    for (i, (c, label)) in swatches.iter().enumerate() {
        let col = i % 3;
        let row = i / 3;
        let sx = cx + 12. + col as f32 * (cw - 24.) / 3.;
        let sy = y + 232. + row as f32 * 17.;
        round(f, sx, sy, 8., 8., 1.5, *c, Some(alpha(p.text, 0.2)));
        mono(f, sx + 14., sy - 1., 7.5, *label, p.muted);
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
    text(f, col2, y + 301., FONT_HINT, "材质   三态", p.muted);
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
        y + 356.,
        FONT_HINT,
        "当前构建使用纯色，保证原生输入兼容",
        p.muted,
    );
    text(f, cx, y + 385., FONT_HINT, "动效强度   四档", p.muted);
    for (i, s) in ["关", "轻", "标准", "丰富"].iter().enumerate() {
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
            "系统减少动态已开启，优先于此处设置"
        } else {
            "动效只在输入、出错或打开面板时触发"
        },
        p.muted,
    );
    text(f, col2, y + 385., FONT_HINT, "字号 / 行高", p.muted);
    let slider_w = (half - 128.).max(30.);
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
        let sy = y + 412. + row as f32 * 32.;
        text(f, col2, sy + 3., FONT_HINT, name, p.muted);
        line(f, col2 + 34., sy + 11., slider_w, p.line2);
        let ratio = if row == 0 {
            (cfg.font_size - 24.) / 40.
        } else {
            cfg.line_gap / 1.2
        };
        let filled = slider_w * ratio.clamp(0., 1.);
        rect(f, col2 + 34., sy + 11., filled, 2., p.accent);
        round(f, col2 + 30. + filled, sy + 7., 8., 8., 4., p.accent, None);
        small_btn(f, hs, col2 + half - 88., sy, 23., "−", minus, t, false);
        small_btn(f, hs, col2 + half - 62., sy, 23., "+", plus, t, false);
        right(f, col2 + half, sy + 4., FONT_HINT, value, p.text2);
    }
    text(f, cx, y + 483., FONT_HINT, "词提样式", p.muted);
    let hw = (cw - 8.) / 3.;
    for (i, s) in ["词级柔光", "字级编码", "极简下划线"].iter().enumerate() {
        small_btn(
            f,
            hs,
            cx + i as f32 * (hw + 4.),
            y + 505.,
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
        y + 544.,
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
    right(f, px + preview_w - 14., y + 79., FONT_HINT, &t.name, p.muted);
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
        "最近已结算成绩；没有成绩时显示 —",
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
    for (i, (c, name)) in [
        (p.accent, "1简"),
        (p.copper, "2简"),
        (p.jade, "3简"),
        (p.muted, "4码"),
        (p.red, "选重"),
    ]
    .iter()
    .enumerate()
    {
        let sx = card_x + i as f32 * card_w / 5.;
        rect(f, sx, y + ph - 122., 4., 4., *c);
        text(f, sx + 8., y + ph - 126., 7.5, *name, p.muted);
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
        y + ph - 36.,
        FONT_HINT,
        "F7 随时呼出 · 只修改跟打器外观",
        p.muted,
    );
    btn(
        f,
        hs,
        x + pw - 194.,
        y + ph - 47.,
        102.,
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
        let full_page = matches!(m.modal, Modal::Result | Modal::Report);
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
                Modal::Load => draw_load(f, hs, m, sess, cfg, t, x, y, pw, ph),
                Modal::Groups => draw_groups(f, hs, m, sess, t, x, y, pw, ph),
                Modal::Appearance => draw_appearance(f, hs, m, sess, cfg, t, x, y, pw, ph),
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
            );
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
    #[test]
    fn small_panel_keeps_primary_actions_visible() {
        let mut m = Model::default();
        m.open(Modal::Load);
        m.load_editing = true;
        let s = Session::new(100);
        let cfg = Config::default();
        let t = crate::ui::theme("墨玉");
        let mut f = Frame::new(640., 420.);
        let mut hits = Vec::new();
        draw(&mut f, &mut hits, &m, &s, &cfg, &t, 640., 420., 0.);
        let load = hits.iter().find(|h| h.action == Action::LoadText).unwrap();
        assert!(load.y >= 0. && load.y + load.h <= 420.);
        let fields = fields(&m, 640., 420.);
        assert!(fields[0].h > 60.);
        assert!(fields[0].y + fields[0].h <= load.y);
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
            action: Action::LoadText,
        }];
        fit_panel(&mut f, &mut hits, 0, 16., 24., 608., 596., 372., 100.);
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
