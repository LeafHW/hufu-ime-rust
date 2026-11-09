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
    Preferences,
    Online,
    Rules,
    Confirm,
    /// 本段按键热力图（结算后，数据 Session::last_vk_histogram）。
    Heatmap,
    /// 【减法 2026-11-09】原 F2 载文窗退役；本地文库 + 单字表 + TXT 收拢为
    /// 轻量「载入」面板，从「更多」进入；主工具栏位让给「在线发文」。
    Library,
    /// Less frequent actions, kept out of the practice toolbar.
    More,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    None,
    Open(Modal),
    Close,
    /// 底栏成绩面板开关（主窗口内联展开）。
    ToggleLedger,
    /// 主窗口底部理论分析停靠面板开关（F10）。
    ToggleAnalysis,
    /// 打开 F1 设置并定位到「外观」页（F7）。
    OpenAppearanceSettings,
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
    SelectHistory(usize),
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
    pub selected_history: Option<usize>,
    pub confirm: String,
    pub pending: Option<Action>,
    pub toast: Option<(String, f64)>,
    pub modal_epoch: u64,
    /// 底栏「成绩记录」内联面板是否展开（替代旧成绩整页）。
    pub ledger_open: bool,
    /// 成绩面板滚动偏移（DIP）。
    pub ledger_scroll: f32,
    /// 主窗口底部「理论分析」停靠面板是否展开（替代旧 F10 模态）。
    pub analysis_dock: bool,
    /// 理论分析停靠面板滚动偏移（DIP）。
    pub analysis_dock_scroll: f32,
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
    let (ideal_w, ph): (f32, f32) = match m {
        Modal::Online => (920., 740.),
        Modal::Preferences => (960., 760.),
        Modal::Groups => (720., 520.),
        Modal::Library => (760., 460.),
        Modal::More => (640., 460.),
        Modal::Rules => (1040., 900.),
        Modal::Confirm => (560., 300.),
        Modal::Heatmap => (1040., 620.),
        _ => (880., 596.),
    };
    let ph = if matches!(
        m,
        Modal::Heatmap | Modal::Groups | Modal::Library | Modal::More
    ) {
        panel_view(ph, h)
    } else {
        ph
    };
    let pw = ideal_w.min((w - 32.).max(200.));
    let view = panel_view(ph, h);
    ((w - pw) / 2., ((h - view) / 2.).max(16.), pw, ph)
}
/// Field placement and panel rendering must use the same tab-dependent bounds.
pub fn panel_for_model(m: &Model, w: f32, h: f32) -> (f32, f32, f32, f32) {
    let (x, _y, pw, ph) = panel(m.modal, w, h);
    if m.modal == Modal::Groups {
        let rows = m.groups.len().clamp(2, 8) as f32;
        let ph = (62. + 48. + rows * 44. + 96.).clamp(320., 520.);
        return (x, ((h - panel_view(ph, h)) / 2.).max(16.), pw, ph);
    }
    if m.modal == Modal::Online {
        let ph = crate::secondary_panels::content_height_online(m).min(ph);
        return (x, ((h - panel_view(ph, h)) / 2.).max(16.), pw, ph);
    }
    let y = _y;
    if m.modal == Modal::Preferences {
        let ph = preferences::content_height(m, pw).min(ph);
        return (x, ((h - panel_view(ph, h)) / 2.).max(16.), pw, ph);
    }
    (x, y, pw, ph)
}
fn panel_view(ph: f32, h: f32) -> f32 {
    ph.min((h - 48.).max(PANEL_HEAD + PANEL_FOOT + 48.))
}
pub fn fields(m: &Model, w: f32, h: f32) -> Vec<FieldSpec> {
    let (x, y, pw, ph) = panel_for_model(m, w, h);
    let mut v = match m.modal {
        Modal::Online | Modal::Rules => crate::secondary_panels::fields(m, x, y, pw, ph),
        Modal::Preferences => preferences::fields(m, x, y, pw, ph),
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
    if key.is_empty() {
        return;
    }
    let p = tokens(t);
    round(f, x, y, 36., 28., 6., p.surface2, Some(p.line2));
    mono(f, x + 7., y + 4., 15., key, p.muted);
}
fn panel_header(f: &mut Frame, hs: &mut Vec<Hit>, m: Modal, x: f32, y: f32, pw: f32, t: &Theme) {
    let p = tokens(t);
    let (icon_name, title, desc, key) = match m {
        Modal::Library => ("book", "载入", "本地 TXT、单字练习与剪贴板", ""),
        Modal::More => ("folder", "更多", "工具、设置与数据", ""),
        Modal::Groups => ("group", "选择群聊", "选中后读取群文或发送成绩", "F5"),
        Modal::Preferences => ("palette", "跟打器设置", "练习、成绩、配色、外观与数据", "F1"),
        Modal::Heatmap => (
            "keyboard",
            "本段按键热力图",
            "当前段每个按键的按下次数 · 滚轮关闭",
            "Esc",
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
    let p = tokens(t);
    let left = x + 24.;
    let column = (pw - 68.) / 2.;
    let right_col = left + column + 20.;
    text(f, left, y + 84., FONT_HINT, "载入文章", p.text2);
    small_btn(
        f,
        hs,
        left,
        y + 112.,
        column,
        "打开 TXT 文件",
        Action::LoadFile,
        t,
        false,
    );
    small_btn(
        f,
        hs,
        left,
        y + 156.,
        column,
        "从剪贴板载入（Ctrl+V）",
        Action::LoadClipboard,
        t,
        false,
    );
    text_block(
        f,
        left,
        y + 206.,
        column,
        FONT_HINT,
        24.,
        2,
        "本地文章列表已取消；TXT 与剪贴板载入后直接进入跟打区。",
        p.muted,
    );
    rect(f, x + pw / 2., y + 80., 1., ph - 160., p.line);
    text(f, right_col, y + 84., FONT_HINT, "单字与词组练习", p.text2);
    for (i, (title, _)) in gale_core::single::builtin_tables().iter().enumerate() {
        small_btn(
            f,
            hs,
            right_col,
            y + 118. + i as f32 * 36.,
            column,
            title,
            Action::Practice(i),
            t,
            false,
        );
    }
    footer(f, x, y, pw, ph, t);
    text(
        f,
        left,
        y + ph - 39.,
        FONT_HINT,
        "Ctrl+V 可直接载入文本 · 单字练习从右侧选择",
        p.muted,
    );
    btn(
        f,
        hs,
        x + pw - 112.,
        y + ph - 48.,
        88.,
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
    _sess: &Session,
    cfg: &Config,
    t: &Theme,
    x: f32,
    y: f32,
    pw: f32,
    ph: f32,
) {
    let p = tokens(t);
    let left = x + 24.;
    let right_x = x + pw - 24.;
    let list_w = right_x - left;
    let list_top = y + 110.;
    let list_bottom = y + ph - 96.;
    let row_pitch = 44.;
    let rows = (((list_bottom - list_top) / row_pitch).floor() as usize).max(1);
    text(
        f,
        left,
        y + 76.,
        FONT_HINT,
        format!("QQ 群聊 · {}", m.groups.len()),
        p.muted,
    );
    small_btn(
        f,
        hs,
        right_x - 64.,
        y + 68.,
        64.,
        "刷新",
        Action::ScanGroups,
        t,
        false,
    );
    if let Some(bound) = m.bound.filter(|_| !m.groups.is_empty()) {
        small_btn(
            f,
            hs,
            right_x - 64. - 8. - 88.,
            y + 68.,
            88.,
            "取消选择",
            Action::SelectGroup(bound),
            t,
            false,
        );
    }
    if m.groups.is_empty() {
        text(f, left, y + 150., 17., "没有发现群聊", p.text2);
        text_block(
            f,
            left,
            y + 184.,
            list_w,
            FONT_HINT,
            26.,
            3,
            "先打开 QQ 群聊列表后刷新；也可复制群文，回到主界面按 Ctrl+V 载入。",
            p.muted,
        );
    } else {
        let max_scroll = m.groups.len().saturating_sub(rows);
        let offset = (m.scroll as usize).min(max_scroll);
        for (row, (i, name)) in m
            .groups
            .iter()
            .enumerate()
            .skip(offset)
            .take(rows)
            .enumerate()
        {
            let ry = list_top + row as f32 * row_pitch;
            let selected = m.bound == Some(i);
            round(
                f,
                left,
                ry,
                list_w,
                40.,
                7.,
                if selected {
                    alpha(p.accent, 0.10)
                } else {
                    alpha(p.surface2, 0.55)
                },
                Some(if selected {
                    alpha(p.accent, 0.45)
                } else {
                    p.line
                }),
            );
            f.push(DrawCmd::PushClip {
                x: left + 12.,
                y: ry + 9.,
                w: list_w - 52.,
                h: 24.,
            });
            text(f, left + 12., ry + 10., 15., name.clone(), p.text2);
            f.push(DrawCmd::PopClip);
            if selected {
                right(f, right_x - 12., ry + 9., 16., "✓", p.accent);
            }
            hs.push(Hit {
                x: left,
                y: ry,
                w: list_w,
                h: 40.,
                action: Action::SelectGroup(i),
            });
        }
        if max_scroll > 0 {
            let track = list_bottom - list_top;
            let thumb = (track * rows as f32 / m.groups.len() as f32).clamp(18., track);
            rect(f, right_x + 6., list_top, 3., track, alpha(p.text, 0.06));
            rect(
                f,
                right_x + 6.,
                list_top + offset as f32 / max_scroll as f32 * (track - thumb),
                3.,
                thumb,
                alpha(p.accent, 0.55),
            );
        }
    }
    let bad_status = ["失败", "错误", "离线", "未找到", "无法", "重试"]
        .iter()
        .any(|word| m.status.contains(word));
    if bad_status && !m.status.is_empty() {
        text_block(
            f,
            left,
            y + ph - 88.,
            list_w,
            FONT_HINT,
            20.,
            1,
            &m.status,
            p.red,
        );
    }
    footer(f, x, y, pw, ph, t);
    // 左下：自动发成绩复选框（原右侧大卡片的内容，按用户要求移到这里）。
    let cb_y = y + ph - 50.;
    round(
        f,
        left,
        cb_y,
        18.,
        18.,
        4.,
        if cfg.auto_send_score { p.accent } else { p.bg2 },
        Some(p.line2),
    );
    if cfg.auto_send_score {
        text(f, left + 3., cb_y - 2., 14., "✓", p.bg);
    }
    text(
        f,
        left + 28.,
        cb_y - 1.,
        FONT_HINT,
        "打完一段自动发成绩",
        p.text2,
    );
    text(
        f,
        left + 28.,
        cb_y + 22.,
        13.,
        "发送前会校验目标群",
        p.muted,
    );
    hit_box(hs, left, cb_y - 6., 250., 46., Action::ToggleAutoSend);
    let read_x = right_x - 128.;
    let post_x = read_x - 86.;
    if m.bound.and_then(|i| m.groups.get(i)).is_some() {
        btn(
            f,
            hs,
            post_x,
            y + ph - 52.,
            78.,
            "发文",
            Action::PostArticle,
            t,
            false,
        );
        btn(
            f,
            hs,
            read_x,
            y + ph - 52.,
            128.,
            "读取群文",
            Action::ReadGroup,
            t,
            true,
        );
    } else {
        disabled(f, post_x, y + ph - 52., 78., "发文", t);
        disabled(f, read_x, y + ph - 52., 128., "读取群文", t);
    }
}
fn draw_more(
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
    let left = x + 24.;
    let col_w = (pw - 64.) / 2.;
    let col2 = left + col_w + 16.;
    let row_h = 46.;
    let mut left_items: Vec<(String, String, Action, String)> = vec![
        (
            "载入与单字".into(),
            "TXT、剪贴板与单字练习".into(),
            Action::Open(Modal::Library),
            "".into(),
        ),
        (
            "从剪贴板载入".into(),
            "整篇文章直接进入跟打区".into(),
            Action::LoadClipboard,
            "Ctrl+V".into(),
        ),
    ];
    if sess.source != crate::session::SessionSource::Single
        && m.bound.and_then(|i| m.groups.get(i)).is_some()
    {
        left_items.push((
            "发送当前文章".into(),
            "按晴发文格式发到目标群".into(),
            Action::PostArticle,
            "F2".into(),
        ));
    }
    left_items.push((
        if m.race {
            "切换到练习模式".into()
        } else {
            "切换到赛制模式".into()
        },
        "赛制会关闭提示、粘贴与失焦暂停".into(),
        Action::ToggleRace,
        "".into(),
    ));
    let right_items: Vec<(String, String, Action, String)> = vec![
        (
            "跟打器设置".into(),
            "单字提示、成绩单与配色".into(),
            Action::Open(Modal::Preferences),
            "F1".into(),
        ),
        (
            "清洗规则".into(),
            "正则清洗、原文与结果预览".into(),
            Action::Open(Modal::Rules),
            "F9".into(),
        ),
        (
            "打开数据目录".into(),
            "成绩、配置与文章缓存".into(),
            Action::Folder,
            "".into(),
        ),
    ];
    // 左栏：载入 + 本轮；右栏：设置。行高 46，节标题 20。
    let mut ly = y + 74.;
    text(f, left, ly, FONT_HINT, "载入与发送", p.muted);
    ly += 20.;
    for (title, desc, action, key) in &left_items {
        draw_more_row(f, hs, left, ly, col_w, title, desc, action.clone(), key, t);
        ly += row_h;
    }
    let mut ry = y + 74.;
    text(f, col2, ry, FONT_HINT, "设置与数据", p.muted);
    ry += 20.;
    for (title, desc, action, key) in &right_items {
        draw_more_row(f, hs, col2, ry, col_w, title, desc, action.clone(), key, t);
        ry += row_h;
    }
    footer(f, x, y, pw, ph, t);
    text(
        f,
        left,
        y + ph - 39.,
        FONT_HINT,
        "快捷键在主窗口直接可用",
        p.muted,
    );
    btn(
        f,
        hs,
        x + pw - 112.,
        y + ph - 48.,
        88.,
        "关闭",
        Action::Close,
        t,
        false,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_more_row(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    title: &str,
    desc: &str,
    action: Action,
    key: &str,
    t: &Theme,
) {
    let p = tokens(t);
    line(f, x, y + 44., w, alpha(p.line, 0.65));
    f.push(DrawCmd::PushClip {
        x,
        y: y + 3.,
        w: w - 46.,
        h: 22.,
    });
    text(f, x, y + 4., 15., title, p.text2);
    f.push(DrawCmd::PopClip);
    f.push(DrawCmd::PushClip {
        x,
        y: y + 23.,
        w: w - 46.,
        h: 20.,
    });
    text(f, x, y + 23., 13., desc, p.muted);
    f.push(DrawCmd::PopClip);
    if !key.is_empty() {
        let kw = (ui_text_width(key, 12.) + 16.).max(38.);
        round(f, x + w - kw, y + 10., kw, 22., 5., p.surface2, Some(p.line2));
        text(f, x + w - kw + 8., y + 13., 12., key, p.muted);
    } else {
        text(f, x + w - 16., y + 12., 15., "›", p.muted);
    }
    hs.push(Hit {
        x,
        y,
        w,
        h: 45.,
        action,
    });
}
pub(super) fn appearance_preview(
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
    let sample: Vec<char> = sess
        .current_segment()
        .text
        .chars()
        .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
        .collect();
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
    let hints_visible = sess.source == crate::session::SessionSource::Single
        && gale_core::rules::hints_effective(m.hints, m.race);
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
        if hints_visible && current >= offset && current < end {
            let xx = x + current.saturating_sub(offset) as f32 * fs;
            let ww = fs.min(x + w - xx);
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
    let hist = if sess.vk_histogram.is_empty() {
        &sess.last_vk_histogram
    } else {
        &sess.vk_histogram
    };
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
                "本段暂无按键记录 · 开始跟打后这里显示每个按键的次数"
            } else {
                "本段按键热力图 · 主键区 · 键帽上的数字是该键按下次数"
            },
            p.muted,
        );
    }
    crate::keyboard::draw_main(
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
        "颜色越深，次数越多 · 不统计界面快捷键与按键自动重复",
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
pub fn analysis_scroll_max(sess: &Session, m: &Model, w: f32, h: f32) -> f32 {
    let (_, _, pw, dh) = analysis_dock_rect(w, h);
    let body_h = (dh - 96.).max(24.);
    sess.full_analysis
        .as_ref()
        .map(|fa| {
            let (_, height) = analysis_blocks(fa, m, (pw - 40.).max(80.));
            (height - body_h).max(0.)
        })
        .unwrap_or(0.)
}
/// 主窗口底部「理论分析」停靠面板的几何：紧贴成绩栏上方，宽 = 窗口 - 48。
pub fn analysis_dock_height(h: f32) -> f32 {
    (h * 0.36).clamp(216., 300.)
}
pub fn analysis_dock_rect(w: f32, h: f32) -> (f32, f32, f32, f32) {
    let dh = analysis_dock_height(h);
    let bottom = crate::ui::ledger_bar_y(h) - 8.;
    (24., bottom - dh, (w - 48.).max(220.), dh)
}
fn analysis_chip(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    label: &str,
    action: Action,
    t: &Theme,
    on: bool,
) {
    let p = tokens(t);
    round(
        f,
        x,
        y,
        w,
        26.,
        6.,
        if on { alpha(p.accent, 0.12) } else { p.surface2 },
        Some(if on { alpha(p.accent, 0.5) } else { p.line }),
    );
    text(
        f,
        x + 9.,
        y + 5.,
        13.,
        label,
        if on { p.accent } else { p.text2 },
    );
    hs.push(Hit {
        x,
        y,
        w,
        h: 26.,
        action,
    });
}
fn analysis_icon_button(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    s: f32,
    label: &str,
    action: Action,
    t: &Theme,
) {
    let p = tokens(t);
    round(f, x, y, s, 26., 6., p.surface2, Some(p.line));
    text(f, x + s / 2. - 5., y + 4., 15., label, p.text2);
    hs.push(Hit {
        x,
        y,
        w: s,
        h: 26.,
        action,
    });
}
/// 主窗口内嵌理论分析：不是模态，正文区让位，数据仍走 Session::full_analysis。
pub(crate) fn draw_analysis_dock(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    sess: &Session,
    t: &Theme,
    w: f32,
    h: f32,
) {
    let (x, y, pw, dh) = analysis_dock_rect(w, h);
    let p = tokens(t);
    round(f, x, y, pw, dh, 12., p.surface, Some(p.line2));
    let auto4 = analysis_auto4(m);
    let heatmap = analysis_is_heatmap(m);
    let fs = analysis_font(m);
    text(f, x + 20., y + 13., 16., "理论码长分析", p.text);
    let mut cx = x + 20. + 118.;
    if let Some(fa) = &sess.full_analysis {
        f.push(DrawCmd::PushClip {
            x: cx,
            y: y + 15.,
            w: (pw - 360.).max(60.),
            h: 20.,
        });
        text(f, cx, y + 15., 12., &fa.schema, p.muted);
        f.push(DrawCmd::PopClip);
    }
    // 右侧控制：字号 − n + · 四码首选 · 视图 · 关闭
    let close_x = x + pw - 44.;
    analysis_icon_button(f, hs, close_x, y + 8., 26., "×", Action::ToggleAnalysis, t);
    let view_w = 74.;
    let view_x = close_x - 8. - view_w;
    analysis_chip(
        f,
        hs,
        view_x,
        y + 8.,
        view_w,
        if heatmap { "全文编码" } else { "热力图" },
        crate::aux_marker("analysis_view"),
        t,
        heatmap,
    );
    let auto_w = 92.;
    let auto_x = view_x - 10. - auto_w;
    analysis_chip(
        f,
        hs,
        auto_x,
        y + 8.,
        auto_w,
        "四码首选",
        crate::aux_marker("analysis_auto4"),
        t,
        auto4,
    );
    let plus_x = auto_x - 10. - 26.;
    let minus_x = plus_x - 4. - 30. - 26.;
    let minus = crate::aux_marker(&format!("analysis_font:{}", (fs as i32 - 1).clamp(12, 36)));
    let plus = crate::aux_marker(&format!("analysis_font:{}", (fs as i32 + 1).clamp(12, 36)));
    analysis_icon_button(f, hs, minus_x, y + 8., 26., "−", minus, t);
    mono(f, minus_x + 30., y + 12., 13., format!("{fs:.0}"), p.text2);
    analysis_icon_button(f, hs, plus_x, y + 8., 26., "+", plus, t);
    // 汇总行
    if let Some(fa) = &sess.full_analysis {
        let summary = crate::analysis::summarize(fa, auto4);
        let items = [
            ("码长", format!("{:.4}", summary.avg_code_len)),
            ("字数", summary.char_count.to_string()),
            ("选重", summary.select_count.to_string()),
            ("缺字", summary.missing_chars.to_string()),
            ("已知键", summary.known_keys.to_string()),
        ];
        let step = ((pw - 40.) / 5.).max(96.);
        for (i, (label, value)) in items.iter().enumerate() {
            let sx = x + 20. + i as f32 * step;
            text(f, sx, y + 46., 12., *label, p.muted);
            mono(
                f,
                sx + 46.,
                y + 42.,
                16.,
                value,
                if i == 3 && summary.missing_chars > 0 {
                    p.red
                } else {
                    p.text
                },
            );
        }
    }
    line(f, x + 20., y + 74., pw - 40., p.line);
    let body_y = y + 84.;
    let body_h = (dh - 96.).max(24.);
    let body_w = (pw - 40.).max(120.);
    if let Some(fa) = &sess.full_analysis {
        if heatmap {
            // 理论热力：按全文词条的编码统计每个键的理论使用次数（不是实际按键）。
            let summary = crate::analysis::summarize(fa, auto4);
            f.push(DrawCmd::PushClip {
                x: x + 20.,
                y: body_y,
                w: body_w,
                h: body_h,
            });
            crate::keyboard::draw_main(f, t, x + 24., body_y + 2., body_w - 8., body_h - 6., |key| {
                summary.key_counts.get(&key).copied().unwrap_or(0)
            });
            f.push(DrawCmd::PopClip);
        } else {
            let (blocks, total_h) = analysis_blocks(fa, m, body_w);
            let scroll = m.analysis_dock_scroll.clamp(0., (total_h - body_h).max(0.));
            let ks = (fs * 0.68).max(14.);
            f.push(DrawCmd::PushClip {
                x: x + 20.,
                y: body_y,
                w: body_w,
                h: body_h,
            });
            for block in blocks
                .iter()
                .filter(|b| b.y + b.h >= scroll && b.y < scroll + body_h)
            {
                let bx = x + 20. + block.x;
                let by = body_y + block.y - scroll;
                let entry = &fa.entries[block.source_index];
                let color = if block.missing {
                    p.red
                } else {
                    let code = entry.code.as_deref().unwrap_or("");
                    if entry.word.chars().filter(|c| c.is_alphanumeric()).count() <= 1 {
                        let n = code.chars().count().max(1);
                        p.word_colors[(n - 1).min(p.word_colors.len() - 1)]
                    } else {
                        crate::ui::hint_word_tint(&p, &entry.word, code, fa.max_code_length)
                            .unwrap_or(p.text)
                    }
                };
                f.push(DrawCmd::PushClip {
                    x: bx,
                    y: by,
                    w: block.w + 2.,
                    h: block.h,
                });
                for (i, word) in block.words.iter().enumerate() {
                    crate::ui::body_text(f, bx, by + i as f32 * fs * 1.4, fs, word, color);
                }
                let key_y = by + block.words.len() as f32 * fs * 1.4;
                for (i, keys) in block.keys.iter().enumerate() {
                    mono(f, bx, key_y + i as f32 * ks * 1.4, ks, keys, color);
                }
                f.push(DrawCmd::PopClip);
            }
            if total_h > body_h {
                let thumb = (body_h * body_h / total_h).clamp(18., body_h);
                rect(f, x + pw - 14., body_y, 5., body_h, alpha(p.text, 0.07));
                rect(
                    f,
                    x + pw - 14.,
                    body_y + scroll / (total_h - body_h) * (body_h - thumb),
                    5.,
                    thumb,
                    alpha(p.accent, 0.7),
                );
            }
            f.push(DrawCmd::PopClip);
        }
    } else {
        let loading = m.field("analysis_loading") == "true";
        let error = m.field("analysis_error");
        text(
            f,
            x + 24.,
            body_y + 8.,
            15.,
            if loading {
                "正在分析当前文章…"
            } else if error.is_empty() {
                "当前文章尚未取得理论分析"
            } else {
                "理论分析未完成"
            },
            p.text2,
        );
        if !loading {
            let detail = if error.is_empty() {
                "重新请求后，这里会显示全文逐词编码与理论码长。".to_string()
            } else {
                error
            };
            let lines = analysis_wrap(&detail, body_w, 13., false);
            for (i, line) in lines.iter().take(2).enumerate() {
                text(f, x + 24., body_y + 36. + i as f32 * 19., 13., line, p.muted);
            }
            btn(
                f,
                hs,
                x + 24.,
                body_y + body_h - 38.,
                104.,
                "重新分析",
                crate::aux_marker("analysis_retry"),
                t,
                false,
            );
        }
    }
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
        let (x, y, pw, ph) = panel_for_model(m, w, h);
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
            Modal::More => draw_more(f, hs, m, sess, t, x, y, pw, ph),
            Modal::Groups => draw_groups(f, hs, m, sess, cfg, t, x, y, pw, ph),
            Modal::Preferences => preferences::draw(f, hs, m, sess, cfg, t, x, y, pw, ph),
            Modal::Heatmap => draw_heatmap(f, hs, sess, t, x, y, pw, ph),
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
            0.,
            modal_content_height(m, pw),
        );
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
    fn analysis_dock_scrolls_body_without_moving_header_controls() {
        let s = analysis_session(1600);
        let mut m = Model::default();
        m.analysis_dock = true;
        let t = crate::ui::theme("墨玉");
        let max = analysis_scroll_max(&s, &m, 1280., 800.);
        assert!(max > 0.);
        let mut top = Frame::new(1280., 800.);
        let mut top_hits = Vec::new();
        draw_analysis_dock(&mut top, &mut top_hits, &m, &s, &t, 1280., 800.);
        m.analysis_dock_scroll = max;
        let mut bottom = Frame::new(1280., 800.);
        let mut bottom_hits = Vec::new();
        draw_analysis_dock(&mut bottom, &mut bottom_hits, &m, &s, &t, 1280., 800.);
        assert!(bottom
            .cmds
            .iter()
            .any(|c| matches!(c, DrawCmd::TextRun { text, .. } if text == "最后一词")));
        let close = Action::ToggleAnalysis;
        let a = top_hits.iter().find(|h| h.action == close).unwrap();
        let b = bottom_hits.iter().find(|h| h.action == close).unwrap();
        assert_eq!((a.x, a.y, a.w, a.h), (b.x, b.y, b.w, b.h));
        assert!(bottom_hits
            .iter()
            .all(|h| h.x >= 0. && h.y >= 0. && h.x + h.w <= 1280. && h.y + h.h <= 800.));
    }

    #[test]
    fn analysis_dock_fits_small_viewport_and_has_all_controls() {
        let s = analysis_session(12);
        let mut m = Model::default();
        m.analysis_dock = true;
        let (_, y, _, dh) = analysis_dock_rect(640., 420.);
        assert!(y >= 0. && y + dh <= 420.);
        let mut f = Frame::new(640., 420.);
        let mut hits = Vec::new();
        draw_analysis_dock(
            &mut f,
            &mut hits,
            &m,
            &s,
            &crate::ui::theme("墨玉"),
            640.,
            420.,
        );
        for action in [
            Action::ToggleAnalysis,
            crate::aux_marker("analysis_auto4"),
            crate::aux_marker("analysis_view"),
            crate::aux_marker("analysis_font:19"),
            crate::aux_marker("analysis_font:21"),
        ] {
            let h = hits.iter().find(|h| h.action == action).unwrap();
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
    fn appearance_preview_keeps_hints_only_for_single_practice() {
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
        let mut cfg = Config::default();
        // A retired style value must not make the surviving single hints inaccessible.
        cfg.hint_style = 2;
        let t = crate::ui::theme("墨玉");
        let p = tokens(&t);
        let mut frame = Frame::new(800., 300.);
        appearance_preview(&mut frame, &s, &m, &cfg, &t, 0., 0., 700., 250.);
        assert!(!frame.cmds.iter().any(|cmd| matches!(cmd,
            DrawCmd::TextRun { text, .. } if text.starts_with('a'))));
        s.source = crate::session::SessionSource::Single;
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
    fn small_library_keeps_every_practice_table_and_file_action_visible() {
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
        for action in std::iter::once(Action::LoadFile)
            .chain((0..gale_core::single::builtin_tables().len()).map(Action::Practice))
        {
            let hit = hits.iter().find(|hit| hit.action == action).unwrap();
            assert!(hit.h == 34. && hit.x >= 0. && hit.y >= 0.);
            assert!(hit.x + hit.w <= 640. && hit.y + hit.h <= 420.);
        }
    }
    #[test]
    fn group_actions_require_a_live_target_and_fit_the_small_window() {
        let s = Session::new(100);
        let cfg = Config::default();
        let t = crate::ui::theme("墨玉");
        for (w, h) in [(640., 420.), (640., 480.), (800., 600.)] {
            for bound in [None, Some(99), Some(0)] {
                let mut m = Model::default();
                m.open(Modal::Groups);
                m.groups = vec!["测试群".into()];
                m.bound = bound;
                let mut frame = Frame::new(w, h);
                let mut hits = Vec::new();
                draw(&mut frame, &mut hits, &m, &s, &cfg, &t, w, h, 0.);
                for action in [Action::ReadGroup, Action::PostArticle] {
                    let hit = hits.iter().find(|hit| hit.action == action);
                    assert_eq!(hit.is_some(), bound == Some(0));
                    if let Some(hit) = hit {
                        assert_eq!(hit.h, 34.);
                        assert!(hit.x >= 0. && hit.y >= 0.);
                        assert!(hit.x + hit.w <= w && hit.y + hit.h <= h);
                    }
                }
                let auto = hits
                    .iter()
                    .find(|hit| hit.action == Action::ToggleAutoSend)
                    .unwrap();
                assert_eq!(auto.h, 46.);
                assert!(auto.y + auto.h <= h);
            }
        }
    }
    #[test]
    fn more_preserves_secondary_actions_without_overlap_in_both_modes() {
        let cfg = Config::default();
        let t = crate::ui::theme("墨玉");
        for (w, h) in [(640., 420.), (640., 480.), (800., 600.)] {
            let mut s = Session::new(100);
            let mut m = Model::default();
            m.open(Modal::More);
            m.groups = vec!["测试群".into()];
            m.bound = Some(0);
            for single in [false, true] {
                s.source = if single {
                    crate::session::SessionSource::Single
                } else {
                    crate::session::SessionSource::Custom
                };
                let mut frame = Frame::new(w, h);
                let mut hits = Vec::new();
                draw(&mut frame, &mut hits, &m, &s, &cfg, &t, w, h, 0.);
                for action in [
                    Action::Open(Modal::Library),
                    Action::Open(Modal::Preferences),
                    Action::Open(Modal::Rules),
                    Action::ToggleRace,
                    Action::Folder,
                ] {
                    assert!(hits.iter().any(|hit| hit.action == action));
                }
                assert_eq!(
                    hits.iter().any(|hit| hit.action == Action::PostArticle),
                    !single
                );
                let menu: Vec<_> = hits
                    .iter()
                    .filter(|hit| hit.action != Action::None && hit.action != Action::Close)
                    .collect();
                for (i, hit) in menu.iter().enumerate() {
                    assert!(hit.h > 0. && hit.h <= 45.);
                    assert!(hit.x >= 0. && hit.y >= 0.);
                    assert!(hit.x + hit.w <= w && hit.y + hit.h <= h);
                    for other in menu.iter().skip(i + 1) {
                        assert!(
                            hit.x + hit.w <= other.x
                                || other.x + other.w <= hit.x
                                || hit.y + hit.h <= other.y
                                || other.y + other.h <= hit.y
                        );
                    }
                }
            }
        }
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
