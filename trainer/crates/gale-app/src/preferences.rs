//! F1 跟打器设置：左侧四页导航 + 右侧内容区（词提/成绩单/外观与颜色/数据）。
//!
//! 面板几何由 `modals` 提供（默认840×660，内容按实际高度滚动），
//! `y+62 .. y+ph-62` 为可视内容区，左侧导航176；
//! 本模块只负责排版、绘制与原生编辑框坐标。`pref_*` 动作语义保持与 `main.rs`
//! 一致：`pref_tab:{0..3}`、`pref_toggle:{key}`、`pref_step:hint_font_size:±1`、
//! `pref_field:{key}`、`pref_score_preset:{id}`、`pref_save_template`、
//! `pref_color_choose:{key}`（系统调色盘），以及 `Action::Folder`。
use super::{hit_box, wrap, Action, FieldSpec, Hit, Model, PANEL_FOOT, PANEL_HEAD};
use crate::session::Session;
use crate::ui::{
    alpha, body_text, elide_width, line, mono, rect, right, round, text, tokens, ui_text_width,
    Palette,
};
use gale_core::config::{Config, DisplayMode};
use gale_core::scorecard::{format_score_template, SCORE_FIELDS, SCORE_PRESETS};
use gale_core::theme::Theme;
use gale_win::d2d::{Align, DrawCmd, FontFamily, Frame};
use std::path::PathBuf;

// ------------------------------------------------------------------ 规格常量

/// 左侧导航宽度（规格 §6）。
pub(super) const NAV_W: f32 = 176.;
/// 内容区左右内边距。
const PAD: f32 = 24.;
const NAV_ITEM_H: f32 = 40.;
const NAV_ITEM_GAP: f32 = 6.;
const NAV_INSET: f32 = 12.;
/// 常规内容的逻辑高度，独立于外层面板的紧凑视口。
/// 字号预览保持原尺寸，超出视口时由 `fit_panel` 滚动承接。
const CONTENT_HEIGHT: f32 = 752.;
/// 1280×800 下首屏可见的内容高度（面板 view=752，头脚各 62）。
const BODY_VISIBLE_800: f32 = 752. - PANEL_HEAD - PANEL_FOOT;
/// 滚动可达的内容高度上限（面板高 − 头脚；与 1280×800 首屏一致）。
const BODY_BUDGET: f32 = CONTENT_HEIGHT - PANEL_HEAD - PANEL_FOOT;

/// 页标题 16；正文标签 15；说明 13；数字等宽。
const T_TITLE: f32 = 16.;
const T_LABEL: f32 = 15.;
const T_NOTE: f32 = 13.;

/// 六页：2026-11-13 新增「快捷键」（录制式自设 + 恢复默认）。
const PAGES: [(&str, &str); 6] = [
    ("词提", "提示开关、策略与配色"),
    ("成绩单", "自动复制与成绩单样式"),
    ("外观与颜色", "主题、排版、动效与配色"),
    ("界面与显示", "对照模式、极简与窗口"),
    ("数据与关于", "存储位置与版本"),
    ("快捷键", "录制按键、恢复默认"),
];

const COLORS: &[(&str, &str, &str)] = &[
    ("跟打区背景色", "跟打区背景", "111315"),
    ("对打未打底色", "未打底色", "111315"),
    // 未打文字色（2026-11-13 四轮）：对照文章卡/字贴正文未敲的字。
    ("未打文字色", "未打文字", "d0c9b9"),
    ("打对字色", "打对文字", "e7e4db"),
    ("对打打对底色", "打对底色", "173c2b"),
    ("打错字色", "打错文字", "e36b65"),
    ("对打打错底色", "打错底色", "4b2021"),
];
const HINT_COLORS: &[(&str, &str, &str)] = &[
    ("词提编码色", "单字编码", "d0c9b9"),
    ("词提角标色", "选重数字", "d0c9b9"),
    ("词提一码色", "多字一码", "ff7f9f"),
    ("词提简码色", "简码词", "7bdbac"),
    ("词提二字三码色", "二字三码", "a5b8ff"),
    ("词提二字全码色", "二字全码", "ffae65"),
    ("词提三字全码色", "三字全码", "c69bf2"),
    ("词提四字全码色", "长词全码", "7ecbfa"),
    ("词提下划线色", "词组括线", "58a6f5"),
];
const RANK_COLORS: &[(&str, &str, &str)] = &[
    ("词提1选色", "1 选", "7bdbac"),
    ("词提2选色", "2 选", "a5b8ff"),
    ("词提3选色", "3 选", "ffae65"),
    ("词提4选色", "4 选", "ff7f9f"),
    ("词提5选色", "5 选", "c69bf2"),
    ("词提6选色", "6 选", "7ecbfa"),
    ("词提7选色", "7 选", "e3cd70"),
    ("词提8选色", "8 选", "6dd7cc"),
    ("词提9选色", "9 选", "f29bdd"),
    ("词提10选色", "10 选", "dcaf89"),
    ("词提编码色", "单字编码", "d0c9b9"),
    ("词提角标色", "选重数字", "d0c9b9"),
    ("词提下划线色", "词组括线", "58a6f5"),
];

/// 设置页三处分页会用到的全部颜色键（外观 / 词提配色 / 选重分页）。
///
/// `sync_preferences` 依此重建 `model.fields["color_*"]`，色块读取的就是这些字段。
/// 颜色键分散在 `COLORS` / `HINT_COLORS` / `RANK_COLORS` 三份清单里，曾经另有一份
/// 手写的 `PREFERENCE_COLORS` 只覆盖旧键——漏掉「未打文字色」和词提配色键，结果
/// 正文颜色已改、设置页色块却还显示 fallback 默认色（2026-11-14 用户反馈）。
/// 这里统一去重导出，后续新增颜色只改一份清单即可。
pub(crate) fn all_color_keys() -> Vec<&'static str> {
    let mut keys: Vec<&'static str> = Vec::new();
    for (key, _, _) in COLORS
        .iter()
        .chain(HINT_COLORS.iter())
        .chain(RANK_COLORS.iter())
    {
        if !keys.contains(key) {
            keys.push(*key);
        }
    }
    keys
}

// 词提页
const PRACTICE_SWITCH_Y: f32 = 72.;
const PRACTICE_SWITCH_H: f32 = 36.;
const PRACTICE_SWITCH_PITCH: f32 = 40.;
const PRACTICE_STRATEGY_Y: f32 = 274.;
const PRACTICE_DYNAMIC_Y: f32 = 332.;
const PRACTICE_SIZE_LABEL_Y: f32 = 376.;
const PRACTICE_SIZE_Y: f32 = 394.;
const PRACTICE_SIZE_H: f32 = 84.;
const PRACTICE_PREVIEW_Y: f32 = 484.;
const PRACTICE_PREVIEW_H: f32 = 144.;

// 成绩单页
const SCORE_G1_LABEL_Y: f32 = 124.;
const SCORE_G1_Y: f32 = 144.;
const SCORE_G1_ROW_H: f32 = 40.;
const SCORE_G2_LABEL_Y: f32 = 132.;
const SCORE_CHIPS_Y: f32 = 154.;
const SCORE_CHIP_PITCH: f32 = 32.;
const SCORE_CHIP_H: f32 = 30.;
const SCORE_CHIP_COLUMNS: usize = 4;
const SCORE_G3_TITLE_Y: f32 = 306.;
const SCORE_SAVE_Y: f32 = 304.;
const SCORE_PRESETS_Y: f32 = 258.;
const SCORE_PRESETS_H: f32 = 30.;
const SCORE_EDITOR_Y: f32 = 344.;
const SCORE_EDITOR_H: f32 = 76.;
const SCORE_PREVIEW_Y: f32 = 476.;
const SCORE_PREVIEW_H: f32 = 134.;

// 颜色页
const COLOR_LABEL_Y: f32 = 30.;
const COLOR_ROWS_Y: f32 = 52.;
const COLOR_ROW_H: f32 = 34.;
const COLOR_ROW_GAP: f32 = 6.;
const COLOR_HINT_Y: f32 = 432.;
const COLOR_PREVIEW_Y: f32 = 456.;
const COLOR_PREVIEW_H: f32 = 160.;

// 数据页
const DATA_CARD1_Y: f32 = 36.;
const DATA_CARD_H: f32 = 92.;
const DATA_CARD2_Y: f32 = 144.;
/// 练字记录卡（数据页第三张卡）。
const DATA_PRACTICE_Y: f32 = 252.;
const DATA_PRACTICE_H: f32 = 128.;

// -------------------------------------------------------------------- 取值层

fn tab(m: &Model) -> u8 {
    m.field("pref_tab").parse::<u8>().unwrap_or(0).min(5)
}
fn enabled(m: &Model, key: &str, default: bool) -> bool {
    m.fields.get(key).map(|v| v == "true").unwrap_or(default)
}
fn selected_color(m: &Model) -> &'static str {
    let value = m.field("pref_color_key");
    page_colors(m)
        .iter()
        .find(|(key, _, _)| *key == value)
        .map(|v| v.0)
        .unwrap_or(page_colors(m)[0].0)
}
fn selected_color_label(m: &Model) -> &'static str {
    let value = selected_color(m);
    page_colors(m)
        .iter()
        .find(|(key, _, _)| *key == value)
        .map(|v| v.1)
        .unwrap_or(page_colors(m)[0].1)
}
fn hint_colors_view(m: &Model) -> bool {
    tab(m) == 0 && m.field("pref_hint_view") == "colors"
}
fn page_colors(m: &Model) -> &'static [(&'static str, &'static str, &'static str)] {
    if tab(m) == 0 {
        if enabled(m, "hint_rank_colors", false) {
            RANK_COLORS
        } else {
            HINT_COLORS
        }
    } else {
        COLORS
    }
}
pub(crate) fn color_allowed(m: &Model, key: &str) -> bool {
    (tab(m) == 2 || hint_colors_view(m)) && page_colors(m).iter().any(|c| c.0 == key)
}
/// Remember a color from the visible group for the palette and preview.
pub(crate) fn sync_color_editor(m: &mut Model) {
    let key = selected_color(m);
    let hex = m.field(&format!("color_{key}"));
    m.fields.insert("pref_color_key".into(), key.into());
    m.fields.insert("pref_color_hex".into(), hex);
}
pub(crate) fn select_page(m: &mut Model, page: u8) {
    m.fields.insert("pref_tab".into(), page.min(5).to_string());
    refresh_page(m);
}
pub(crate) fn select_hint_view(m: &mut Model, view: &str) {
    m.fields.insert("pref_hint_view".into(), view.into());
    refresh_page(m);
}
fn refresh_page(m: &mut Model) {
    sync_color_editor(m);
    m.panel_scroll = 0.;
    m.modal_epoch += 1;
}
fn color_hex(m: &Model, key: &str, fallback: &str) -> String {
    m.fields
        .get(&format!("color_{key}"))
        .cloned()
        .unwrap_or_else(|| fallback.into())
}
fn parse_color(hex: &str) -> Option<[f32; 4]> {
    let value = hex.trim().trim_start_matches('#');
    if value.len() != 6 || !value.is_ascii() {
        return None;
    }
    let n = u32::from_str_radix(value, 16).ok()?;
    Some([
        ((n >> 16) & 255) as f32 / 255.,
        ((n >> 8) & 255) as f32 / 255.,
        (n & 255) as f32 / 255.,
        1.,
    ])
}
fn hint_size(m: &Model) -> f32 {
    typography_value(m, "hint_font_size", "hint_font", 14.).clamp(10., 100.)
}
fn typography_value(m: &Model, key: &str, editor: &str, fallback: f32) -> f32 {
    let value = if m.field("pref_num_field") == editor {
        m.field("pref_num_buf")
    } else {
        m.field(key)
    };
    value
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .unwrap_or(fallback)
}
fn typography_preview(m: &Model, cfg: &Config) -> Config {
    let mut preview = cfg.clone();
    preview.hint_font_size =
        typography_value(m, "hint_font_size", "hint_font", cfg.hint_font_size).clamp(10., 100.);
    preview.hint_code_offset = typography_value(
        m,
        "hint_code_offset",
        "hint_code_offset",
        cfg.hint_code_offset,
    )
    .clamp(-100., 100.);
    preview.hint_rank_offset = typography_value(
        m,
        "hint_rank_offset",
        "hint_rank_offset",
        cfg.hint_rank_offset,
    )
    .clamp(-100., 100.);
    preview.preedit_font_size = typography_value(
        m,
        "preedit_font_size",
        "preedit_font",
        cfg.preedit_font_size,
    )
    .clamp(10., 100.);
    preview.preedit_offset =
        typography_value(m, "preedit_offset", "preedit_offset", cfg.preedit_offset)
            .clamp(-100., 100.);
    preview.show_preedit = enabled(m, "show_preedit", cfg.show_preedit);
    preview
}
/// 大字号预览原样绘制并扩大滚动内容，避免“数值改了但预览仍被缩小”。
fn hint_preview_metrics(cfg: &Config) -> (f32, f32) {
    let word_top = 42. + (-cfg.hint_code_offset).max(0.) + (-cfg.hint_rank_offset).max(0.);
    let code_bottom = crate::ui::hint_code_y(cfg, word_top, 24.)
        + cfg.hint_font_size
        + cfg.hint_rank_offset.max(0.);
    (
        word_top,
        PRACTICE_PREVIEW_H.max((word_top + 36.).max(code_bottom) + 48.),
    )
}
fn field_label(label: &str) -> &str {
    label.split('（').next().unwrap_or(label)
}
/// 字段 chip 的短标签：4 列网格下 6 字以上会截断，改用等义短名。
fn chip_label<'a>(key: &str, title: &'a str) -> &'a str {
    match key {
        "legacy_report" => "经典成绩",
        "retype_label" => "首打/重打",
        _ => field_label(title),
    }
}
fn canonical_field(key: &str) -> &str {
    match key {
        "accuracy" => "key_accuracy",
        "select" => "select_count",
        "retype" => "retype_count",
        "backspace" => "backspaces",
        _ => key,
    }
}
/// 数据目录文案：与 `main::data_dir` 同口径（fixture 里没有主程序函数）。
fn data_dir_text() -> String {
    if let Some(v) = std::env::var_os("HUFU_TRAINER_DATA") {
        return PathBuf::from(v).to_string_lossy().into_owned();
    }
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if let Some(value) = args
        .windows(2)
        .find(|a| a[0].to_string_lossy() == "--data")
        .map(|a| &a[1])
    {
        return PathBuf::from(value).to_string_lossy().into_owned();
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
        .join("HuFu")
        .join("trainer")
        .to_string_lossy()
        .into_owned()
}

// ------------------------------------------------------------------ 几何工具

fn body_x(x: f32) -> f32 {
    x + NAV_W + PAD
}
fn body_w(pw: f32) -> f32 {
    (pw - NAV_W - 2. * PAD).max(160.)
}
fn body_y(y: f32) -> f32 {
    y + PANEL_HEAD
}
/// 成绩单模板编辑框（内容区坐标；绘制与原生 FieldSpec 共用同一组常量）。
fn score_editor_rect(bx: f32, by: f32, bw: f32) -> (f32, f32, f32, f32) {
    (bx, by + SCORE_EDITOR_Y, bw, SCORE_EDITOR_H)
}
fn font_picker_rect(bx: f32, by: f32, bw: f32) -> (f32, f32, f32, f32) {
    (bx + 44., by + 142., (bw - 164.).max(40.), 32.)
}
fn score_chip_rect(x: f32, y: f32, w: f32, index: usize) -> (f32, f32, f32) {
    let gap = 8.;
    let cw = (w - (SCORE_CHIP_COLUMNS - 1) as f32 * gap) / SCORE_CHIP_COLUMNS as f32;
    (
        x + (index % SCORE_CHIP_COLUMNS) as f32 * (cw + gap),
        y + (index / SCORE_CHIP_COLUMNS) as f32 * SCORE_CHIP_PITCH,
        cw,
    )
}
fn color_row_y(y: f32, index: usize) -> f32 {
    y + COLOR_ROWS_Y + index as f32 * (COLOR_ROW_H + COLOR_ROW_GAP)
}
// ------------------------------------------------------------------ 绘制小件

fn centered(f: &mut Frame, cx: f32, y: f32, size: f32, s: &str, c: [f32; 4]) {
    f.push(DrawCmd::TextRun {
        x: cx,
        y,
        h: size,
        text: s.to_string(),
        color: c,
        font: FontFamily::Ui,
        align: Align::Center,
    });
}
/// 13 号说明行：超出宽度用省略号截断，绝不压到右侧控件。
fn note(f: &mut Frame, x: f32, y: f32, w: f32, s: &str, c: [f32; 4]) {
    text(f, x, y, T_NOTE, elide_width(s, w.max(8.), T_NOTE), c);
}
fn primary(
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
    round(f, x, y, w, 36., 8., p.accent, Some(alpha(p.text, 0.18)));
    centered(f, x + w / 2., y + 10., 15., label, [0.13, 0.09, 0.035, 1.]);
    hit_box(hs, x, y, w, 36., a);
}
fn outline_button(
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
    round(f, x, y, w, 32., 8., p.bg2, Some(p.line2));
    centered(f, x + w / 2., y + 8., 15., label, p.text2);
    hit_box(hs, x, y, w, 32., a);
}
fn step_button(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    label: &str,
    a: Action,
    t: &Theme,
) {
    let p = tokens(t);
    round(f, x, y, 32., 32., 8., p.bg2, Some(p.line2));
    centered(f, x + 16., y + 7., 18., label, p.text2);
    hit_box(hs, x, y, 32., 32., a);
}
/// 【数值直输 2026-11-13】可点数值框：常态显示格式化值，编辑态高亮描边
/// 并显示缓冲+光标下划线；点击发出 pref_num_edit marker 进入编辑。
fn value_box(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    p: &Palette,
    fid: &str,
    x: f32,
    y: f32,
    w: f32,
    value: &str,
) {
    let editing = m.field("pref_num_field") == fid;
    // 【审计 V08】数值框和只读标签看起来没差别，用户不知道能点进去改。
    // 复用已有的指针通道做悬停线索：浅底高亮 + 强调色描边 + 框内底部细线。
    // 三种都是纯绘制，不改 Hit（仍是 pref_num_edit:{fid}）与点击逻辑。
    let hot = !editing && pointer_over(m, x, y, w, 26.);
    let shown = if editing {
        format!("{}_", m.field("pref_num_buf"))
    } else {
        value.to_string()
    };
    round(
        f,
        x,
        y,
        w,
        26.,
        6.,
        if hot { alpha(p.accent, 0.10) } else { p.bg2 },
        Some(if editing || hot { p.accent } else { p.line2 }),
    );
    if hot {
        // 下划线压在框底内侧（14 号数字排到 y+19），既是「可输入」的可视线索，
        // 又不会与数值文字抢位置。
        rect(f, x + 5., y + 23., (w - 10.).max(0.), 1., alpha(p.accent, 0.7));
    }
    centered(f, x + w / 2., y + 5., 14., &shown, p.text);
    hit_box(
        hs,
        x,
        y,
        w,
        26.,
        crate::aux_marker(&format!("pref_num_edit:{fid}")),
    );
}
/// 带直接输入框的紧凑步进项；数字与加减按钮共用同一配置键。
fn number_control(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    t: &Theme,
    x: f32,
    y: f32,
    w: f32,
    label: &str,
    key: &str,
    editor: &str,
    value: f32,
) {
    let p = tokens(t);
    let short = if w < 214. {
        label
            .trim_start_matches("编码")
            .trim_start_matches("键入")
            .trim_start_matches("数字")
    } else {
        label
    };
    text(f, x, y + 11., T_NOTE, short, p.text2);
    let gx = x + w - 124.;
    step_button(
        f,
        hs,
        gx,
        y + 2.,
        "−",
        crate::aux_marker(&format!("pref_step:{key}:-1")),
        t,
    );
    value_box(
        f,
        hs,
        m,
        &p,
        editor,
        gx + 38.,
        y + 5.,
        48.,
        &format!("{value:.0}"),
    );
    step_button(
        f,
        hs,
        gx + 92.,
        y + 2.,
        "+",
        crate::aux_marker(&format!("pref_step:{key}:1")),
        t,
    );
}
fn switch(f: &mut Frame, hs: &mut Vec<Hit>, x: f32, y: f32, on: bool, a: Action, p: &Palette) {
    let w = 44.;
    let h = 24.;
    round(
        f,
        x,
        y,
        w,
        h,
        12.,
        if on { p.accent } else { p.line2 },
        Some(if on { alpha(p.accent, 0.9) } else { p.line }),
    );
    let knob = 18.;
    let kx = if on { x + w - knob - 3. } else { x + 3. };
    round(
        f,
        kx,
        y + 3.,
        knob,
        knob,
        9.,
        if on { p.text } else { p.muted },
        None,
    );
    hit_box(hs, x - 6., y - 6., w + 12., h + 12., a);
}
/// 一行开关：标题 15 + 说明 13 + 右侧开关；卡片由调用方画。
#[allow(clippy::too_many_arguments)]
fn toggle_row(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    t: &Theme,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    key: &str,
    title: &str,
    desc: &str,
    default: bool,
    title_dy: f32,
    desc_dy: f32,
) {
    let p = tokens(t);
    text(f, x + 16., y + title_dy, T_LABEL, title, p.text);
    note(
        f,
        x + 16.,
        y + desc_dy,
        w - 16. - 16. - 44. - 12.,
        desc,
        p.muted,
    );
    switch(
        f,
        hs,
        x + w - 16. - 44.,
        y + (h - 24.) / 2.,
        enabled(m, key, default),
        crate::aux_marker(&format!("pref_toggle:{key}")),
        &p,
    );
}
fn seg_button(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    selected: bool,
    a: Action,
    p: &Palette,
) {
    round(
        f,
        x,
        y,
        w,
        h,
        7.,
        if selected {
            alpha(p.accent, 0.12)
        } else {
            p.bg2
        },
        Some(if selected {
            alpha(p.accent, 0.55)
        } else {
            p.line
        }),
    );
    let label = elide_width(label, w - 12., T_LABEL);
    centered(
        f,
        x + w / 2.,
        y + (h - T_LABEL) / 2.,
        T_LABEL,
        &label,
        if selected { p.accent } else { p.muted },
    );
    hit_box(hs, x, y, w, h, a);
}
fn field_chip(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    label: &str,
    selected: bool,
    a: Action,
    p: &Palette,
) {
    round(
        f,
        x,
        y,
        w,
        SCORE_CHIP_H,
        6.,
        if selected {
            alpha(p.accent, 0.12)
        } else {
            p.bg2
        },
        Some(if selected {
            alpha(p.accent, 0.55)
        } else {
            p.line
        }),
    );
    let (bx, by) = (x + 10., y + 9.);
    round(
        f,
        bx,
        by,
        12.,
        12.,
        3.,
        if selected { p.accent } else { [0.; 4] },
        Some(if selected { p.accent } else { p.line2 }),
    );
    if selected {
        centered(f, bx + 6., by - 2., 12., "✓", [0.13, 0.09, 0.035, 1.]);
    }
    let label = elide_width(label, w - 40., T_LABEL);
    text(
        f,
        x + 30.,
        y + 5.,
        T_LABEL,
        label,
        if selected { p.accent } else { p.text2 },
    );
    hit_box(hs, x, y, w, SCORE_CHIP_H, a);
}
fn page_title(f: &mut Frame, x: f32, y: f32, w: f32, title: &str, desc: &str, p: &Palette) {
    text(f, x, y, T_TITLE, title, p.text);
    let tw = ui_text_width(title, T_TITLE);
    text(
        f,
        x + tw + 12.,
        y + 3.,
        T_NOTE,
        elide_width(desc, (w - tw - 12.).max(40.), T_NOTE),
        p.muted,
    );
}
fn section_label(f: &mut Frame, x: f32, y: f32, label: &str, p: &Palette) {
    text(f, x, y, T_NOTE, label, p.muted);
}

// ------------------------------------------------- 审计 V08 / V09 / V13 共享小件

/// 【审计 V09】窄窗阈值：正文列宽度 < 536 DIP 视为窄窗。
///
/// 面板宽 = 正文列 + 左侧导航 176 + 两侧内边距 48，所以 536 等价于
/// 「面板 < 760 DIP」（窗口宽约 < 792 DIP）。默认 840 面板的正文列是 616，
/// 800×600 窗口是 544，都不触发；只有真窄窗才收紧行距。
/// 之所以换算到正文列：页面函数只拿到正文列宽 `w`，而改全局 geometry
/// （modals 的 panel/panel_view）不在本次允许范围内。
const COMPACT_BODY_W: f32 = 536.;

/// 【审计 V09】是否按紧凑密度排版。
fn compact_width(w: f32) -> bool {
    w < COMPACT_BODY_W
}

/// 【审计 V08】数值框悬停判定。
///
/// 指针位置来自主循环每帧写入的 `Model::pointer`（逻辑坐标）。正文在共享的
/// `fit_panel` 里整体上移了 `panel_scroll`，命中热区也是同一个偏移平移的；
/// 所以这里必须减掉同一个偏移，悬停框才与点击落点重合。
fn pointer_over(m: &Model, x: f32, y: f32, w: f32, h: f32) -> bool {
    let Some((px, py)) = m.pointer else {
        return false;
    };
    let top = y - m.panel_scroll;
    px >= x && px <= x + w && py >= top && py <= top + h
}

/// 【审计 V08】「可直接输入」一次性文字提示（11 号小字，右缘对齐 `right_x`）。
///
/// 鼠标形状（手型/文本光标）由 main.rs 的窗口层决定，本模块够不着；
/// 所以可输入性必须由页面自己说清：数值框悬停时给视觉线索，
/// 每组数值旁边再给一行小字，两者都不依赖光标。
fn direct_input_hint(f: &mut Frame, right_x: f32, y: f32, p: &Palette) {
    right(f, right_x, y, 11., "可直接输入", p.muted);
}

/// 【审计 V13】主题名口径：配置没设过主题时存的是 "default"，
/// 但用户看到的是首个内置主题「墨玉」；状态行必须显示同一个名字，
/// 否则「当前 default」会和下面高亮的「墨玉」按钮对不上。
fn theme_display_name(theme: &str) -> &str {
    if theme == "default" {
        "墨玉"
    } else {
        theme
    }
}

/// 【审计 V13】外观页口径的颜色覆盖项数：只数 `COLORS` 清单里的键。
///
/// `main.rs` 的 `Action::AppearanceReset` 只 retain 以「词提」开头的键，
/// 也就是「恢复默认外观」不会重置词提配色；若把词提色也算进 N，
/// 用户按一次按钮会发现只清掉一部分，比不显示更困惑。
fn appearance_override_count(cfg: &Config) -> usize {
    COLORS
        .iter()
        .filter(|(key, _, _)| cfg.color_overrides.contains_key(*key))
        .count()
}

/// 【审计 V13】覆盖数文案：0 项返回 None（不占位），避免常态出现「已覆盖 0 项」。
fn override_text(n: usize) -> Option<String> {
    (n > 0).then(|| format!("已覆盖 {n} 项颜色"))
}

/// 【审计 V09】「可滚动」提示的显隐条件。
///
/// 只在「面板被窗口裁短」（`panel_h > view_h`，即真小窗）且「还没滚动过」时出现一次；
/// 用户一动滚轮就收掉，不做常驻闪烁。
///
/// 注意不能用「正文总高 > 可视正文高」当条件：默认 840×660 面板下六页正文都是
/// 752，比 536 的可视正文高，那样提示会在默认窗口上一直挂着，反而变成噪音。
/// 换用「面板本身是否被裁短」这个口径，恰好只在小窗（窗口高 < 面板高 + 48）成立。
fn needs_scroll_hint(panel_h: f32, view_h: f32, scroll: f32) -> bool {
    panel_h > view_h + 0.5 && scroll < 0.5
}

// -------------------------------------------------------------------- 左侧导航

fn nav(f: &mut Frame, hs: &mut Vec<Hit>, m: &Model, t: &Theme, x: f32, y: f32, body_h: f32) {
    let p = tokens(t);
    // 矮窗仍固定导航；压缩条目并省去副标题，保住六个页面的入口。
    let compact = body_h < PAGES.len() as f32 * (NAV_ITEM_H + NAV_ITEM_GAP) + 26.;
    let item_h = if compact { 32. } else { NAV_ITEM_H };
    let pitch = ((body_h - 32. - item_h) / (PAGES.len() - 1) as f32)
        .clamp(item_h + 2., NAV_ITEM_H + NAV_ITEM_GAP);
    // 竖分隔线：rect 而非 line，长度覆盖整个可视内容区。
    rect(f, x + NAV_W, y + PANEL_HEAD, 1., body_h, p.line);
    for (i, (title, desc)) in PAGES.iter().enumerate() {
        let iy = y + PANEL_HEAD + 16. + i as f32 * pitch;
        let ix = x + NAV_INSET;
        let iw = NAV_W - 2. * NAV_INSET;
        let on = tab(m) == i as u8;
        if on {
            round(f, ix, iy, iw, item_h, 8., alpha(p.accent, 0.08), None);
            round(f, ix, iy + 8., 3., item_h - 16., 1.5, p.accent, None);
        }
        text(
            f,
            ix + 14.,
            iy + if compact { 7. } else { 4. },
            T_LABEL,
            *title,
            if on { p.accent } else { p.text2 },
        );
        if !compact {
            note(f, ix + 14., iy + 22., iw - 24., desc, p.muted);
        }
        hit_box(
            hs,
            ix,
            iy,
            iw,
            item_h,
            crate::aux_marker(&format!("pref_tab:{i}")),
        );
    }
}

// -------------------------------------------------------------------- 练习页

fn practice_page(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    cfg: &Config,
    t: &Theme,
    x: f32,
    y: f32,
    w: f32,
) {
    let p = tokens(t);
    page_title(f, x, y, w, "词提", "正文与理论分析按当前码表分析", &p);
    let choice_w = (w - 8.) / 2.;
    for (i, (id, label)) in [("settings", "提示设置"), ("colors", "词提配色")]
        .iter()
        .enumerate()
    {
        seg_button(
            f,
            hs,
            x + i as f32 * (choice_w + 8.),
            y + 30.,
            choice_w,
            30.,
            label,
            hint_colors_view(m) == (*id == "colors"),
            crate::aux_marker(&format!("pref_hint_view:{id}")),
            &p,
        );
    }
    if hint_colors_view(m) {
        round(f, x, y + 72., w, 44., 8., p.surface2, Some(p.line));
        toggle_row(
            f,
            hs,
            m,
            t,
            x,
            y + 72.,
            w,
            44.,
            "hint_rank_colors",
            "选重颜色模式",
            "默认关闭；开启后只按候选序号为词组着色",
            false,
            5.,
            25.,
        );
        color_controls(f, hs, m, t, x, y, w);
        hint_color_preview(f, m, t, x, y + 372., w);
        return;
    }
    // The main switch uses session rules; the other three are display choices.
    let cy = y + PRACTICE_SWITCH_Y;
    round(f, x, cy, w, PRACTICE_SWITCH_H, 8., p.surface2, Some(p.line));
    text(f, x + 16., cy + 5., T_LABEL, "启用词提", p.text);
    note(
        f,
        x + 16.,
        cy + 21.,
        w - 88.,
        "按当前虎符码表识别正文词组",
        p.muted,
    );
    switch(
        f,
        hs,
        x + w - 60.,
        cy + (PRACTICE_SWITCH_H - 24.) / 2.,
        m.hints,
        Action::ToggleHints,
        &p,
    );
    // 【审计 V09】窄窗密度：四行开关行距 40→34、行高 36→32（行距必须 ≥ 行高）。
    // 省下的 24 DIP 在后面各段统一减掉，页面顶部与首屏锚点不变。
    let compact = compact_width(w);
    let switch_h = if compact { 34. } else { PRACTICE_SWITCH_H };
    let switch_pitch = if compact { 38. } else { PRACTICE_SWITCH_PITCH };
    let (switch_title_dy, switch_desc_dy) = if compact { (2., 18.) } else { (4., 20.) };
    let tightened = if compact {
        4. * (PRACTICE_SWITCH_PITCH - switch_pitch) + 6.
    } else {
        0.
    };
    for (i, (key, title, desc, default)) in [
        // 【默认值对齐 2026-11-14】fallback 与 config.rs 默认一致（旧配置缺键时
        // UI 开关态与实际行为不漂移；hint_rank_corner 默认关）。
        (
            "hint_show_code",
            "显示编码",
            "在字词下方显示当前码表编码",
            true,
        ),
        (
            "hint_show_rank",
            "显示选重数字",
            "首选不标数字，从第二候选起显示",
            true,
        ),
        (
            "hint_rank_corner",
            "选重数字角标",
            "数字显示在词的右下角；关闭时随编码行显示",
            false,
        ),
        (
            "hint_color_words",
            "词组着色",
            "仅词组着色，单字与标点保持普通字色",
            true,
        ),
    ]
    .iter()
    .enumerate()
    {
        let cy = y + PRACTICE_SWITCH_Y + (i + 1) as f32 * switch_pitch;
        round(f, x, cy, w, switch_h, 8., p.surface2, Some(p.line));
        toggle_row(
            f,
            hs,
            m,
            t,
            x,
            cy,
            w,
            switch_h,
            key,
            title,
            desc,
            *default,
            switch_title_dy,
            switch_desc_dy,
        );
    }
    let locked = m.field("hint_settings_locked") == "true";
    let strategy_y = y + PRACTICE_STRATEGY_Y - tightened;
    section_label(f, x, strategy_y, "路由策略", &p);
    note(
        f,
        x + 100.,
        strategy_y,
        w - 100.,
        if locked {
            "本轮已锁定，重打后可修改"
        } else {
            "先比较实际键数，再按策略取舍"
        },
        p.muted,
    );
    let route_hits = hs.len();
    let choice_w = (w - 8.) / 2.;
    for (i, (id, label)) in [("shortest", "最短编码"), ("phrases", "词组优先")]
        .iter()
        .enumerate()
    {
        seg_button(
            f,
            hs,
            x + i as f32 * (choice_w + 8.),
            strategy_y + 20.,
            choice_w,
            30.,
            label,
            if *id == "shortest" {
                m.field("hint_strategy") != "phrases"
            } else {
                m.field("hint_strategy") == "phrases"
            },
            crate::aux_marker(&format!("pref_strategy:{id}")),
            &p,
        );
    }
    let dynamic_y = y + PRACTICE_DYNAMIC_Y - tightened;
    round(
        f,
        x,
        dynamic_y,
        w,
        PRACTICE_SWITCH_H,
        8.,
        p.surface2,
        Some(p.line),
    );
    toggle_row(
        f,
        hs,
        m,
        t,
        x,
        dynamic_y,
        w,
        PRACTICE_SWITCH_H,
        "hint_dynamic",
        "动态词提",
        "按正确进度重算后续词提",
        false,
        5.,
        21.,
    );
    if locked {
        hs.truncate(route_hits);
        rect(f, x, strategy_y + 20., w, 30., alpha(p.bg, 0.35));
        rect(f, x, dynamic_y, w, PRACTICE_SWITCH_H, alpha(p.bg, 0.35));
    }
    let size_label_y = y + PRACTICE_SIZE_LABEL_Y - tightened;
    section_label(
        f,
        x,
        size_label_y,
        "词提编码 · 数字样式（点击数值直接输入）",
        &p,
    );
    // 【审计 V08】「能直接输入」这件事埋在一长串标题里没人会读；本行数值框
    // 旁边再放一次 11 号小字，与数值框同屏，悬停高亮则补足「具体哪个框」。
    direct_input_hint(f, x + w, size_label_y, &p);
    let sy = y + PRACTICE_SIZE_Y - tightened;
    round(f, x, sy, w, PRACTICE_SIZE_H, 8., p.surface2, Some(p.line));
    let col_w = (w - 48.) / 2.;
    let left = x + 16.;
    let right = x + 32. + col_w;
    let preview = typography_preview(m, cfg);
    number_control(
        f,
        hs,
        m,
        t,
        left,
        sy,
        col_w,
        "编码字号",
        "hint_font_size",
        "hint_font",
        preview.hint_font_size,
    );
    number_control(
        f,
        hs,
        m,
        t,
        right,
        sy,
        col_w,
        "编码高低",
        "hint_code_offset",
        "hint_code_offset",
        preview.hint_code_offset,
    );
    text(f, left, sy + 53., T_NOTE, "数字加粗", p.text2);
    switch(
        f,
        hs,
        left + col_w - 44.,
        sy + 50.,
        enabled(m, "hint_rank_bold", false),
        crate::aux_marker("pref_toggle:hint_rank_bold"),
        &p,
    );
    number_control(
        f,
        hs,
        m,
        t,
        right,
        sy + 42.,
        col_w,
        "数字高低",
        "hint_rank_offset",
        "hint_rank_offset",
        preview.hint_rank_offset,
    );
    hint_preview(f, m, cfg, t, x, y + PRACTICE_PREVIEW_Y - tightened, w);
}

fn hint_preview(f: &mut Frame, m: &Model, cfg: &Config, t: &Theme, x: f32, py: f32, w: f32) {
    let p = tokens(t);
    let preview = typography_preview(m, cfg);
    let cfg = &preview;
    let (word_top, preview_h) = hint_preview_metrics(cfg);
    round(f, x, py, w, preview_h, 8., p.surface2, Some(p.line));
    text(f, x + 16., py + 12., T_LABEL, "词提实际字号预览", p.text);
    let show_code = m.hints && enabled(m, "hint_show_code", true);
    let show_rank = m.hints && enabled(m, "hint_show_rank", true);
    let color_words = m.hints && enabled(m, "hint_color_words", true);
    let max_len = if m.max_code_length == 0 {
        4
    } else {
        m.max_code_length
    };
    let rank_mode = enabled(m, "hint_rank_colors", false);
    let rank_examples = [
        ("词组", "abcd", 1, "1 选"),
        ("词组", "abcd", 2, "2 选"),
        ("词组", "abcd", 3, "3 选"),
        ("词组", "abcd", 4, "4 选"),
    ];
    let class_examples = [
        ("的", "u", 1, "单字 · 首选"),
        ("简明", "a", 1, "一码词 · 首选"),
        ("来来来", "aaah", 2, "三字词 · 选重"),
    ];
    let large_example = [("词组", "abcd", 2, "词组 · 选重")];
    let size = cfg.hint_font_size;
    let mut examples: &[(&str, &str, usize, &str)] = if rank_mode {
        &rank_examples
    } else {
        &class_examples
    };
    // 不把预览偷偷缩号；大字号改为展示一个完整示例，垂直方向由整页滚动容纳。
    if size * 3. + 8. > (w - 32.) / examples.len() as f32 {
        examples = &large_example;
    }
    let cell = (w - 32.) / examples.len() as f32;
    for (i, (word, code, rank, caption)) in examples.iter().enumerate() {
        let word_w = word.chars().count() as f32 * 24.;
        let sx = x + 16. + i as f32 * cell + (cell - word_w) / 2.;
        let wy = py + word_top;
        let tint = color_words
            .then(|| crate::ui::hint_word_tint_for_rank(&p, word, code, *rank, max_len, rank_mode))
            .flatten();
        body_text(f, sx, wy, 24., *word, tint.unwrap_or(p.text2));
        if m.hints && word.chars().count() > 1 {
            crate::ui::word_bracket(f, sx, wy, word_w, 24., tint.unwrap_or(p.hint_underline));
        }
        let code_w = if show_code {
            code.chars().count() as f32 * size * 0.6
        } else {
            0.
        };
        let corner = enabled(m, "hint_rank_corner", false);
        let rank_size = size * 0.78;
        let rank_text = rank.to_string();
        let inline_rank = show_rank && *rank > 1 && !corner;
        let rank_w = if inline_rank {
            rank_text.chars().count() as f32 * rank_size * 0.6
        } else {
            0.
        };
        let gap = if code_w > 0. && rank_w > 0. { 2. } else { 0. };
        let hx = sx + (word_w - code_w - gap - rank_w) / 2.;
        let code_y = crate::ui::hint_code_y(cfg, wy, 24.);
        if show_code {
            mono(f, hx, code_y, size, *code, tint.unwrap_or(p.hint_code));
        }
        if show_rank && *rank > 1 {
            let rank_color = tint.unwrap_or(p.hint_badge);
            if corner {
                let ds = (24_f32 * 0.52).max(9.);
                let dw = rank_text.chars().count() as f32 * ds * 0.6;
                crate::ui::rank_digit(
                    f,
                    cfg,
                    sx + word_w - dw - 1.,
                    wy + 24. - ds * 0.62,
                    ds,
                    &rank_text,
                    rank_color,
                );
            } else {
                crate::ui::rank_digit(
                    f,
                    cfg,
                    hx + code_w + gap,
                    code_y + size - rank_size,
                    rank_size,
                    &rank_text,
                    rank_color,
                );
            }
        }
        note(
            f,
            x + 16. + i as f32 * cell,
            py + preview_h - 39.,
            cell - 8.,
            caption,
            p.muted,
        );
    }
    note(
        f,
        x + 16.,
        py + preview_h - 20.,
        w - 32.,
        "高低：负数上移，正数下移；键入编码另在「界面与显示」设置。",
        p.muted,
    );
}

// ------------------------------------------------------------------ 成绩单页

fn score_mode(m: &Model) -> String {
    let template = m.field("score_template");
    let preset = m.field("score_preset");
    if template.trim().is_empty() {
        "字段模式".into()
    } else {
        match preset.as_str() {
            "standard" => "标准字段".into(),
            "compact" => "简洁模板".into(),
            "classic" => "经典模板".into(),
            "detailed" => "完整模板".into(),
            _ => "自定义模板".into(),
        }
    }
}

fn score_preview_text(m: &Model) -> String {
    let template = m.field("score_template");
    if template.trim().is_empty() {
        return "当前为字段模式；填写自由模板后可在此预览。".into();
    }
    match &m.latest {
        Some(record) => {
            let signature = m.field("score_signature");
            format_score_template(
                record,
                &template,
                if signature.is_empty() {
                    gale_core::APP_NAME
                } else {
                    &signature
                },
            )
        }
        None => "完成一段跟打后，这里会按模板展示实际成绩。".into(),
    }
}

fn score_page(f: &mut Frame, hs: &mut Vec<Hit>, m: &Model, t: &Theme, x: f32, y: f32, w: f32) {
    let p = tokens(t);
    page_title(f, x, y, w, "成绩单", "自动行为、样式与最近一次预览", &p);
    section_label(f, x, y + 30., "核心数据 · 标准与简洁均保留", &p);
    let core_w = (w - 24.) / 4.;
    for (i, (label, token)) in [
        ("速度", "{speed}"),
        ("键准 %", "{key_accuracy}"),
        ("击键", "{strokes}"),
        ("码长", "{code_len}"),
    ]
    .iter()
    .enumerate()
    {
        let cx = x + i as f32 * (core_w + 8.);
        round(f, cx, y + 50., core_w, 66., 7., p.surface2, Some(p.line));
        text(f, cx + 8., y + 58., T_NOTE, *label, p.muted);
        let value = m
            .latest
            .as_ref()
            .map(|r| format_score_template(r, token, ""))
            .unwrap_or_else(|| "—".into());
        let size = if core_w < 100. { 16. } else { 20. };
        let available = core_w - 16.;
        if value.chars().count() as f32 * size * 0.6 > available && value.contains('/') {
            let (penalized, raw) = value.split_once('/').unwrap();
            for (line, value) in [penalized.to_string(), format!("/{raw}")]
                .iter()
                .enumerate()
            {
                let fit = 14f32.min(available / (value.chars().count().max(1) as f32 * 0.6));
                mono(
                    f,
                    cx + 8.,
                    y + 80. + line as f32 * 16.,
                    fit,
                    value,
                    p.accent,
                );
            }
        } else {
            let fit = size.min(available / (value.chars().count().max(1) as f32 * 0.6));
            mono(f, cx + 8., y + 80., fit, value, p.accent);
        }
    }

    section_label(f, x, y + SCORE_G1_LABEL_Y, "自动行为", &p);
    let g1 = y + SCORE_G1_Y;
    round(
        f,
        x,
        g1,
        w,
        SCORE_G1_ROW_H * 2.,
        8.,
        p.surface2,
        Some(p.line),
    );
    line(f, x + 12., g1 + SCORE_G1_ROW_H, w - 24., p.line);
    toggle_row(
        f,
        hs,
        m,
        t,
        x,
        g1,
        w,
        SCORE_G1_ROW_H,
        "score_auto_copy",
        "打完自动复制成绩",
        "完成当前段时复制成绩文本",
        true,
        4.,
        23.,
    );
    toggle_row(
        f,
        hs,
        m,
        t,
        x,
        g1 + SCORE_G1_ROW_H,
        w,
        SCORE_G1_ROW_H,
        "auto_send_score",
        "打完自动发成绩到群",
        "绑定 QQ 群后，完成一段即自动发送晴兼容成绩单",
        true,
        4.,
        23.,
    );

    section_label(f, x, y + 236., "成绩单样式", &p);
    let preset = m.field("score_preset");
    let presets = [
        ("standard", "标准"),
        ("compact", "简洁"),
        ("classic", "经典"),
        ("detailed", "完整"),
        ("custom", "自由编辑"),
    ];
    let gap = 8.;
    let pw = (w - (presets.len() - 1) as f32 * gap) / presets.len() as f32;
    for (i, (id, label)) in presets.iter().enumerate() {
        seg_button(
            f,
            hs,
            x + i as f32 * (pw + gap),
            y + SCORE_PRESETS_Y,
            pw,
            SCORE_PRESETS_H,
            label,
            preset == *id,
            crate::aux_marker(&format!("pref_score_preset:{id}")),
            &p,
        );
    }

    text(f, x, y + SCORE_G3_TITLE_Y, T_TITLE, "成绩单模板", p.text);
    note(
        f,
        x + 96.,
        y + SCORE_G3_TITLE_Y + 3.,
        w - 220.,
        "自由编辑后点击保存",
        p.muted,
    );
    outline_button(
        f,
        hs,
        x + w - 96.,
        y + SCORE_SAVE_Y,
        96.,
        "保存模板",
        crate::aux_marker("pref_save_template"),
        t,
    );
    let (ex, ey, ew, eh) = score_editor_rect(x, y, w);
    round(f, ex, ey, ew, eh, 6., p.bg, Some(p.line2));
    note(
        f,
        x,
        ey + eh + 10.,
        w,
        "速度 {speed} · 键准 {key_accuracy}",
        p.muted,
    );
    note(
        f,
        x,
        ey + eh + 30.,
        w,
        "击键 {strokes} · 码长 {code_len}",
        p.muted,
    );

    let py = y + SCORE_PREVIEW_Y;
    round(f, x, py, w, SCORE_PREVIEW_H, 8., p.surface2, Some(p.line));
    text(f, x + 16., py + 9., T_NOTE, "最近一次成绩预览", p.text);
    let per_line = (((w - 32.) / T_NOTE) as usize).max(8);
    let lines = wrap(&score_preview_text(m), per_line);
    let total = lines.len();
    for (i, row) in lines.into_iter().take(3).enumerate() {
        let row = if total > 3 && i == 2 {
            format!(
                "{}…",
                row.chars()
                    .take(per_line.saturating_sub(1))
                    .collect::<String>()
            )
        } else {
            row
        };
        text(f, x + 16., py + 30. + i as f32 * 20., T_NOTE, row, p.text2);
    }
    note(
        f,
        x + 16.,
        py + SCORE_PREVIEW_H - 18.,
        w - 32.,
        "自由编辑时改的是最终文本；数字由成绩记录填充。",
        p.muted,
    );
}

fn data_page(f: &mut Frame, hs: &mut Vec<Hit>, cfg: &Config, t: &Theme, x: f32, y: f32, w: f32) {
    let p = tokens(t);
    page_title(f, x, y, w, "数据与关于", "存储位置、练字记录与版本", &p);
    // 数据目录
    let c1 = y + DATA_CARD1_Y;
    round(f, x, c1, w, DATA_CARD_H, 8., p.surface2, Some(p.line));
    section_label(f, x + 16., c1 + 12., "数据目录", &p);
    let dir = data_dir_text();
    let btn_w = 128.;
    mono(
        f,
        x + 16.,
        c1 + 36.,
        T_LABEL,
        elide_width(&dir, w - 16. - 16. - btn_w - 16., T_LABEL),
        p.text2,
    );
    outline_button(
        f,
        hs,
        x + w - 16. - btn_w,
        c1 + 32.,
        btn_w,
        "打开数据目录",
        Action::Folder,
        t,
    );
    // 版本
    let c2 = y + DATA_CARD2_Y;
    round(f, x, c2, w, DATA_CARD_H, 8., p.surface2, Some(p.line));
    section_label(f, x + 16., c2 + 12., "版本", &p);
    text(
        f,
        x + 16.,
        c2 + 36.,
        T_LABEL,
        format!("虎符跟打器 {}", release_version()),
        p.text,
    );
    note(
        f,
        x + 16.,
        c2 + 58.,
        w - 32.,
        "设置只影响跟打器，不改变输入法皮肤与候选窗。",
        p.muted,
    );
    // 练字记录：按实际敲入的字计（含错字/重打），随 config.toml 一起云同步。
    let c3 = y + DATA_PRACTICE_Y;
    round(f, x, c3, w, DATA_PRACTICE_H, 8., p.surface2, Some(p.line));
    section_label(f, x + 16., c3 + 12., "练字记录", &p);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let today = gale_core::score::unix_to_date(now);
    let day7 = gale_core::score::unix_to_date(now.saturating_sub(6 * 86_400));
    let day30 = gale_core::score::unix_to_date(now.saturating_sub(29 * 86_400));
    let today_chars = cfg.practice_days.get(&today).copied().unwrap_or(0);
    let stats = [
        ("今日", today_chars),
        ("近 7 天", cfg.practice_sum_since(&day7)),
        ("近 30 天", cfg.practice_sum_since(&day30)),
        ("总计", cfg.practice_total),
    ];
    let col_w = (w - 32.) / stats.len() as f32;
    for (i, (label, chars)) in stats.iter().enumerate() {
        let cx = x + 16. + i as f32 * col_w;
        text(f, cx, c3 + 38., T_NOTE, *label, p.muted);
        text(f, cx, c3 + 58., 24., format_chars(*chars), p.text);
    }
    note(
        f,
        x + 16.,
        c3 + 96.,
        w - 32.,
        "按实际敲入的字计（含错字与重打），随设置一起云同步。",
        p.muted,
    );
}

/// 字数展示：超过 1 万用「万」缩写，保留一位小数；不足一万用千分位。
fn format_chars(n: u64) -> String {
    if n >= 10_000 {
        format!("{:.1} 万", n as f64 / 10_000.)
    } else {
        let s = n.to_string();
        let mut out = String::new();
        for (i, ch) in s.chars().enumerate() {
            if i > 0 && (s.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(ch);
        }
        out
    }
}

fn release_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("version.txt")))
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|version| version.trim().to_owned())
            .filter(|version| !version.is_empty())
            .unwrap_or_else(|| "开发版".to_owned())
    })
}

// ---------------------------------------------------------------------- 页脚

fn footer(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    t: &Theme,
    x: f32,
    y: f32,
    pw: f32,
    ph: f32,
    scrollable: bool,
) {
    let p = tokens(t);
    let top = y + ph - PANEL_FOOT;
    rect(f, x + 1., top, pw - 2., PANEL_FOOT - 1., alpha(p.bg2, 0.6));
    line(f, x, top, pw, p.line);
    text(f, x + PAD, top + 24., T_NOTE, "更改即时保存", p.muted);
    // 【审计 V09】小窗滚动提示：面板被窗口裁短（`needs_scroll_hint`）时，
    // 页脚左侧「更改即时保存」与右侧「完成」之间的空档补一句「可滚动 · 滚轮下翻」；
    // 页脚带在 fit_panel 里属固定段（不随滚动上移），所以用户滚一下就收掉，
    // 不做常驻闪烁。右侧细滚动条由共享的 fit_panel 已画好（4 DIP 轨道 + 滑块），
    // 本文件不再重复画第二根。
    if scrollable {
        right(
            f,
            x + pw - PAD - 96. - 12.,
            top + 24.,
            T_NOTE,
            "可滚动 · 滚轮下翻",
            p.text2,
        );
    }
    primary(
        f,
        hs,
        x + pw - PAD - 96.,
        top + 13.,
        96.,
        "完成",
        Action::Close,
        t,
    );
}

// ---------------------------------------------------------------- 对外接口

/// 原生编辑框仅用于成绩单模板；颜色由系统调色盘处理。
pub(super) fn fields(m: &Model, x: f32, y: f32, pw: f32, _ph: f32) -> Vec<FieldSpec> {
    let (bx, by, bw) = (body_x(x), body_y(y), body_w(pw));
    match tab(m) {
        1 => {
            let (fx, fy, fw, fh) = score_editor_rect(bx, by, bw);
            vec![FieldSpec {
                id: "score_template",
                x: fx,
                y: fy,
                w: fw,
                h: fh,
                multi: true,
                password: false,
            }]
        }
        _ => Vec::new(),
    }
}

pub(super) fn font_picker(m: &Model, x: f32, y: f32, pw: f32) -> Option<FieldSpec> {
    if tab(m) != 2 {
        return None;
    }
    let (x, y, w, h) = font_picker_rect(body_x(x), body_y(y), body_w(pw));
    Some(FieldSpec {
        id: "body_font",
        x,
        y,
        w,
        h,
        multi: false,
        password: false,
    })
}

/// 固定面板高度：四页切换不跳动，短窗口整页滚动。
pub(super) fn content_height(m: &Model, pw: f32) -> f32 {
    if tab(m) == 0 && !hint_colors_view(m) {
        let preview = typography_preview(m, &Config::default());
        CONTENT_HEIGHT
            .max(PRACTICE_PREVIEW_Y + hint_preview_metrics(&preview).1 + PANEL_HEAD + PANEL_FOOT)
    } else if tab(m) == 5 {
        CONTENT_HEIGHT.max(shortcut_body_height(body_w(pw)) + PANEL_HEAD + PANEL_FOOT)
    } else {
        CONTENT_HEIGHT
    }
}

/// 面板高度保持稳定，滚动范围只覆盖当前页实际内容，短页不允许滚成空白。
pub(super) fn scroll_content_height(m: &Model, pw: f32) -> f32 {
    match tab(m) {
        2 => content_height(m, pw)
            .max(PANEL_HEAD + 568. + appearance_extra_row(body_w(pw)) + 48. + 12. + PANEL_FOOT),
        4 => PANEL_HEAD + DATA_PRACTICE_Y + DATA_PRACTICE_H + 16. + PANEL_FOOT,
        5 => PANEL_HEAD + shortcut_body_height(body_w(pw)) + PANEL_FOOT,
        _ => content_height(m, pw),
    }
}

pub(super) fn draw(
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
    let body_h = (super::panel_view(ph, f.size.h) - PANEL_HEAD - PANEL_FOOT).max(0.);
    nav(f, hs, m, t, x, y, body_h);
    let (bx, by, bw) = (body_x(x), body_y(y), body_w(pw));
    // 大字号预览可超过面板的固定高度；整个页面仍属于正文，不能按原始 y
    // 被 fit_panel 误判成固定页脚。外层可视裁切负责滚动后的显示范围。
    f.push(DrawCmd::PushClip {
        x: bx,
        y: by,
        w: bw,
        h: (content_height(m, pw).max(scroll_content_height(m, pw)) - PANEL_HEAD - PANEL_FOOT)
            .max(1.),
    });
    match tab(m) {
        0 => practice_page(f, hs, m, cfg, t, bx, by, bw),
        1 => score_page(f, hs, m, t, bx, by, bw),
        2 => appearance_page(f, hs, m, sess, cfg, t, bx, by, bw),
        3 => interface_page(f, hs, m, cfg, t, bx, by, bw),
        4 => data_page(f, hs, cfg, t, bx, by, bw),
        _ => shortcuts_page(f, hs, m, cfg, t, bx, by, bw),
    }
    f.push(DrawCmd::PopClip);
    // 【审计 V09】「可滚动」提示的判定口径：面板是否被窗口裁短（`ph > panel_view`）。
    // 默认 840×660 面板能整块显示，不提示；窗口变矮、面板被裁时提示一次，
    // 用户滚动后（panel_scroll > 0）自动收掉。
    let view_h = super::panel_view(ph, f.size.h);
    footer(
        f,
        hs,
        t,
        x,
        y,
        pw,
        ph,
        needs_scroll_hint(ph, view_h, m.panel_scroll),
    );
}

// -------------------------------------------------------------- 快捷键页

const SHORTCUT_ROW_H: f32 = 42.;
const SHORTCUT_TOP: f32 = 58.;
const SHORTCUT_CHIP_W: f32 = 132.;

fn shortcut_columns(w: f32) -> usize {
    let label_w = gale_core::config::SHORTCUT_DEFAULTS
        .iter()
        .map(|(_, label, _)| ui_text_width(label, T_LABEL))
        .fold(0_f32, f32::max);
    if w >= 2. * (label_w + 12. + SHORTCUT_CHIP_W) + 12. {
        2
    } else {
        1
    }
}

fn shortcut_body_height(w: f32) -> f32 {
    let rows = gale_core::config::SHORTCUT_DEFAULTS
        .len()
        .div_ceil(shortcut_columns(w)) as f32;
    SHORTCUT_TOP + rows * shortcut_row_h(w) + 12. + 20. + T_NOTE + 16.
}

/// 【审计 V09】快捷键行距：窄窗 42→36（键位框 28 高 + 2 偏移，仍留 6 DIP 空隙）。
///
/// 行距同时决定 `shortcut_body_height`，两处必须共用同一函数——否则滚动高度
/// 与真实排版差一行，末行会被页脚盖住（或底部多出一截空白）。
fn shortcut_row_h(w: f32) -> f32 {
    if compact_width(w) {
        36.
    } else {
        SHORTCUT_ROW_H
    }
}

/// 【快捷键 2026-11-13】一页录制式自设：点键位框→按新组合键；右上角恢复默认。
fn shortcuts_page(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    cfg: &Config,
    t: &Theme,
    x: f32,
    y: f32,
    w: f32,
) {
    let p = tokens(t);
    page_title(
        f,
        x,
        y,
        w,
        "快捷键",
        "点键位后按下新组合键；录制中按 Esc 取消",
        &p,
    );
    // 恢复默认（右上角）
    let reset_w = 96.;
    let reset_x = x + w - reset_w;
    round(f, reset_x, y + 26., reset_w, 30., 8., p.bg2, Some(p.line2));
    centered(
        f,
        reset_x + reset_w / 2.,
        y + 33.,
        T_NOTE,
        "恢复默认",
        p.text2,
    );
    hit_box(
        hs,
        reset_x,
        y + 26.,
        reset_w,
        30.,
        crate::aux_marker("pref_shortcut:reset"),
    );
    section_label(f, x, y + 32., "命令快捷键", &p);

    let recording = m.field("pref_recording");
    let cols = shortcut_columns(w);
    let gap = 12.;
    let col_w = (w - (cols - 1) as f32 * gap) / cols as f32;
    let row_h = shortcut_row_h(w);
    let top = y + SHORTCUT_TOP;
    for (index, (id, label, _)) in gale_core::config::SHORTCUT_DEFAULTS.iter().enumerate() {
        let col = index % cols;
        let row = index / cols;
        let cx = x + col as f32 * (col_w + gap);
        let cy = top + row as f32 * row_h;
        let chip_w = SHORTCUT_CHIP_W;
        let chip_h = 28.;
        let chip_x = cx + col_w - chip_w;
        let chip_y = cy + 2.;
        text(f, cx, cy + 6., T_LABEL, *label, p.text2);
        let active = recording == *id;
        round(
            f,
            chip_x,
            chip_y,
            chip_w,
            chip_h,
            7.,
            if active { p.accent } else { p.bg2 },
            Some(if active { p.accent } else { p.line2 }),
        );
        let shown = if active {
            "按下组合键…".to_string()
        } else {
            cfg.shortcut(id)
        };
        let key_size = (14. * (chip_w - 12.) / ui_text_width(&shown, 14.).max(1.)).clamp(10., 14.);
        centered(
            f,
            chip_x + chip_w / 2.,
            chip_y + 6.,
            key_size,
            &shown,
            if active { p.bg } else { p.text },
        );
        hit_box(
            hs,
            chip_x,
            chip_y,
            chip_w,
            chip_h,
            crate::aux_marker(&format!("pref_shortcut:record:{id}")),
        );
    }
    let rows = gale_core::config::SHORTCUT_DEFAULTS.len().div_ceil(cols) as f32;
    let note_y = top + rows * row_h + 12.;
    note(
        f,
        x,
        note_y,
        w,
        "Enter / 空格 / Ctrl+V / Ctrl+Z / Ctrl+Y 是打字与编辑固定键，不参与自设；",
        p.muted,
    );
    note(
        f,
        x,
        note_y + 20.,
        w,
        "快捷键需为 F1–F24 或包含 Ctrl / Alt，重复会提示占用。",
        p.muted,
    );
}

// -------------------------------------------------------------- 界面与显示页

/// 迷你模式预览：字贴=满窗正文；对照=上文章块 + 下输入框。
fn mode_preview(f: &mut Frame, p: &Palette, x: f32, y: f32, w: f32, h: f32, mode: DisplayMode) {
    round(f, x, y, w, h, 6., p.bg, Some(p.line));
    rect(f, x + 6., y + 6., w - 12., 3., p.line2);
    match mode {
        DisplayMode::Copybook => {
            for i in 0..3 {
                let lw = w - 16. - if i == 1 { 12. } else { 0. };
                rect(
                    f,
                    x + 8.,
                    y + 16. + i as f32 * 9.,
                    lw,
                    3.,
                    if i == 0 { p.accent } else { p.text2 },
                );
            }
        }
        DisplayMode::Split => {
            for i in 0..2 {
                rect(f, x + 8., y + 15. + i as f32 * 7., w - 16., 3., p.text2);
            }
            round(
                f,
                x + 6.,
                y + 31.,
                w - 12.,
                h - 37.,
                4.,
                p.bg2,
                Some(p.line2),
            );
            rect(f, x + 10., y + 36., 10., 2.5, p.accent);
        }
    }
}

fn interface_page(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    cfg: &Config,
    t: &Theme,
    x: f32,
    y: f32,
    w: f32,
) {
    let p = tokens(t);
    // 【审计 V09】窄窗密度：模式卡片到分栏卡片、以及三张开关卡之间的竖向留白
    // 各收 8 DIP（138→130 / 88→80 / 60→52）。卡片本身尺寸不动，保证卡片内的
    // 迷你预览与标题仍有呼吸；收紧后同屏能多看到一张卡，少滚一次。
    let compact = compact_width(w);
    let (to_ratio, to_list, card_gap, tail_gap) = if compact {
        (130., 80., 52., 56.)
    } else {
        (138., 88., 60., 64.)
    };
    page_title(f, x, y, w, "界面与显示", "键入编码、正文模式与窗口", &p);
    // 键入编码与码表词提提示是两个独立功能，入口放在本页首屏。
    let preedit_y = y + 30.;
    round(f, x, preedit_y, w, 102., 8., p.surface2, Some(p.line));
    toggle_row(
        f,
        hs,
        m,
        t,
        x,
        preedit_y,
        w,
        44.,
        "show_preedit",
        "显示键入编码",
        "文字上方显示正在敲的编码，与下方词提提示独立",
        true,
        4.,
        24.,
    );
    let preview = typography_preview(m, cfg);
    let col_w = (w - 48.) / 2.;
    number_control(
        f,
        hs,
        m,
        t,
        x + 16.,
        preedit_y + 50.,
        col_w,
        "键入字号",
        "preedit_font_size",
        "preedit_font",
        preview.preedit_font_size,
    );
    number_control(
        f,
        hs,
        m,
        t,
        x + 32. + col_w,
        preedit_y + 50.,
        col_w,
        "键入高低",
        "preedit_offset",
        "preedit_offset",
        preview.preedit_offset,
    );
    // 【审计 V08】「键入字号 / 键入高低」也是「数值 + 加减」行，同样支持点数值直输；
    // 卡片左下角补同一条小字（与外观页排版行一致的口径）。之所以左对齐而不像外观页
    // 那样右对齐：这两行的 + 按钮就压在卡片右缘，右对齐的小字会顶到按钮下沿；
    // 竖向放在数值框/步进按钮（底 y+114）与卡片下沿（y+132）之间。
    text(f, x + 16., preedit_y + 88., 11., "可直接输入", p.muted);
    // 模式二选一卡片（带迷你预览，选中 accent 描边 + 勾）
    let card_w = (w - 10.) / 2.;
    let cards_y = y + 140.;
    for (i, (mode, title, desc)) in [
        (
            DisplayMode::Copybook,
            "字贴模式",
            "发文区直接跟打，光标逐字推进",
        ),
        (
            DisplayMode::Split,
            "对照模式",
            "上方文章对照，下方输入框跟打",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let cx = x + i as f32 * (card_w + 10.);
        let selected = cfg.display_mode == mode;
        round(
            f,
            cx,
            cards_y,
            card_w,
            118.,
            10.,
            if selected {
                alpha(p.accent, 0.07)
            } else {
                p.surface2
            },
            Some(if selected {
                alpha(p.accent, 0.6)
            } else {
                p.line
            }),
        );
        text(
            f,
            cx + 16.,
            cards_y + 14.,
            T_LABEL,
            title,
            if selected { p.accent } else { p.text },
        );
        note(
            f,
            cx + 16.,
            cards_y + 36.,
            (card_w - 130.).max(60.),
            desc,
            p.muted,
        );
        mode_preview(f, &p, cx + card_w - 100., cards_y + 56., 84., 50., mode);
        if selected {
            round(
                f,
                cx + card_w - 28.,
                cards_y + 12.,
                16.,
                16.,
                8.,
                p.accent,
                None,
            );
            text(f, cx + card_w - 25., cards_y + 12.5, 11., "✓", p.bg);
        }
        hit_box(
            hs,
            cx,
            cards_y,
            card_w,
            118.,
            crate::aux_marker(&format!("pref_mode:{}", mode.as_str())),
        );
    }
    // 对照模式分栏比例（紧跟模式卡片：与模式选择语义相邻，也保住 800×600 首屏锚点）
    let s_ratio = cards_y + to_ratio;
    round(f, x, s_ratio, w, 76., 8., p.surface2, Some(p.line));
    text(
        f,
        x + 16.,
        s_ratio + 19.,
        T_LABEL,
        "对照模式文章区高度",
        p.text,
    );
    let group_w = 32. + 6. + 40. + 6. + 32.;
    let gx = x + w - 16. - group_w;
    step_button(
        f,
        hs,
        gx,
        s_ratio + 10.,
        "−",
        crate::aux_marker("pref_step:split_ratio:-1"),
        t,
    );
    centered(
        f,
        gx + 34. + 20.,
        s_ratio + 19.,
        14.,
        &format!("{:.0}%", cfg.split_ratio * 100.),
        p.accent,
    );
    step_button(
        f,
        hs,
        gx + 80.,
        s_ratio + 10.,
        "+",
        crate::aux_marker("pref_step:split_ratio:1"),
        t,
    );
    note(
        f,
        x + 16.,
        s_ratio + 44.,
        (w - 32. - group_w - 16.).max(80.),
        "直接在窗口里上下拖动中间把手也可以调整；其余高度留给输入框",
        p.muted,
    );
    // 极简模式（两种正文模式通用）
    let s1 = s_ratio + to_list;
    round(f, x, s1, w, 52., 8., p.surface2, Some(p.line));
    toggle_row(
        f,
        hs,
        m,
        t,
        x,
        s1,
        w,
        52.,
        "minimal",
        "极简模式",
        "隐藏标题栏、工具栏、统计带与底栏，两种正文模式通用",
        false,
        8.,
        28.,
    );
    let s2 = s1 + card_gap;
    round(f, x, s2, w, 52., 8., p.surface2, Some(p.line));
    toggle_row(
        f,
        hs,
        m,
        t,
        x,
        s2,
        w,
        52.,
        "minimal_hud",
        "极简数据条",
        "极简模式顶部显示速度、键准与进度；关闭后只剩正文",
        true,
        8.,
        28.,
    );
    let s3 = s2 + card_gap;
    round(f, x, s3, w, 52., 8., p.surface2, Some(p.line));
    toggle_row(
        f,
        hs,
        m,
        t,
        x,
        s3,
        w,
        52.,
        "always_on_top",
        "窗口置顶",
        "跟打时保持在其它窗口之上（右键菜单也可切换）",
        false,
        8.,
        28.,
    );
    note(
        f,
        x,
        s3 + tail_gap,
        w,
        &format!(
            "右键打开视图菜单 · {} 极简 · {} 切换对照",
            cfg.shortcut("minimal"),
            cfg.shortcut("toggle_mode")
        ),
        p.muted,
    );
}

const APPEARANCE_TYPOGRAPHY_Y: f32 = 178.;
/// 窄排数值框高 26px、距标签顶 12px；次行再留 4px 间隔，避免跨行命中。
const APPEARANCE_NARROW_ROW_PITCH: f32 = 52.;

fn appearance_extra_row(w: f32) -> f32 {
    if (w - 32.) / 3. >= 224. {
        0.
    } else {
        APPEARANCE_NARROW_ROW_PITCH + 12.
    }
}

/// 外观与颜色页：原 F7 外观 + 原「颜色与提示」合并。
#[allow(clippy::too_many_arguments)]
fn appearance_page(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    sess: &Session,
    cfg: &Config,
    t: &Theme,
    x: f32,
    y: f32,
    w: f32,
) {
    let p = tokens(t);
    // 【审计 V09】窄窗（面板 <760 DIP，正文列 <536）收紧分组间留白 6 DIP；
    // 配色行距由 color_controls 内部按同一阈值 30→26 收紧。
    let compact = compact_width(w);
    let sec = if compact { 6. } else { 0. };
    page_title(
        f,
        x,
        y,
        w - 120.,
        "外观与颜色",
        "主题、排版、动效与三态配色",
        &p,
    );
    outline_button(
        f,
        hs,
        x + w - 108.,
        y + 2.,
        108.,
        "恢复默认外观",
        Action::AppearanceReset,
        t,
    );
    // 【审计 V13】「恢复默认外观」到底会动哪些东西，按钮文案说不清：按钮正下方
    // 补一条 11 号旁注，和右侧主题状态行同屏，按下去之前就能对上账。
    // （y+36 起、11 号高，落在按钮下沿 y+34 与主题按钮 y+50 之间。）
    let reset_note = "会重置字号/字距/主题与颜色";
    let reset_note_w = ui_text_width(reset_note, 11.);
    right(f, x + w, y + 36., 11., reset_note, p.muted);
    section_label(f, x, y + 32., "主题", &p);
    // 【审计 V13】主题行右侧补状态：当前主题名（含「跟随虎符」这类动态名）用
    // 强调色，「已覆盖 N 项颜色」用次级文字色；N=0 时不画。右界给上面的旁注
    // 留位，两段文字同屏不打架；宽度不够时按剩余宽度截断。
    let status_x = x + 34.;
    let status_limit = (x + w - reset_note_w - 10.).max(status_x);
    let theme_name = theme_display_name(&cfg.theme);
    let name_text = format!("当前 {theme_name}");
    text(f, status_x, y + 32., T_NOTE, &name_text, p.accent);
    if let Some(ov) = override_text(appearance_override_count(cfg)) {
        let ov_x = status_x + ui_text_width(&name_text, T_NOTE) + 10.;
        let room = status_limit - ov_x;
        if room >= 24. {
            text(
                f,
                ov_x,
                y + 32.,
                T_NOTE,
                elide_width(&ov, room, T_NOTE),
                p.text2,
            );
        }
    }
    let themes = ["墨玉", "宣纸", "夜巡", "青瓷", "朱砂", "灰砖"];
    let theme_w = (w - (themes.len() as f32 - 1.) * 8.) / themes.len() as f32;
    for (i, name) in themes.iter().enumerate() {
        let bx = x + i as f32 * (theme_w + 8.);
        let pt = tokens(&crate::ui::theme(name));
        let selected = cfg.theme == *name || (i == 0 && cfg.theme == "default");
        round(
            f,
            bx,
            y + 50.,
            theme_w,
            34.,
            7.,
            pt.bg,
            Some(if selected { p.accent } else { p.line2 }),
        );
        centered(f, bx + theme_w / 2., y + 58., T_LABEL, name, pt.text);
        hit_box(
            hs,
            bx,
            y + 50.,
            theme_w,
            34.,
            Action::Theme(name.to_string()),
        );
    }
    let follow = cfg.theme == "跟随虎符";
    text(f, x, y + 96., T_LABEL, "跟随虎符皮肤", p.text2);
    note(
        f,
        x,
        y + 118.,
        w - 130.,
        "开启后跟随虎符当前皮肤；关闭则使用上面的主题。",
        p.muted,
    );
    outline_button(
        f,
        hs,
        x + w - 92.,
        y + 90.,
        92.,
        if follow { "已开启" } else { "开启" },
        Action::Theme("跟随虎符".into()),
        t,
    );
    line(f, x, y + 134., w, p.line);
    text(f, x, y + 150., T_LABEL, "字体", p.text2);
    let font_name = if cfg.font_family.is_empty() {
        format!("默认 · {}", gale_win::d2d::detected_cjk_family())
    } else {
        let actual = gale_win::d2d::resolved_cjk_family(&cfg.font_family);
        if actual == cfg.font_family {
            actual
        } else {
            format!("{}（已回退 {}）", cfg.font_family, actual)
        }
    };
    text(
        f,
        x + 44.,
        y + 150.,
        T_NOTE,
        elide_width(&font_name, (w - 164.).max(40.), T_NOTE),
        p.text,
    );
    outline_button(
        f,
        hs,
        x + w - 108.,
        y + 142.,
        108.,
        "默认霞鹜",
        Action::FontDefault,
        t,
    );
    // 排版：字号 / 行距 / 字间距。宽窗口三列；窄窗口两列 + 字间距独占一行，
    // 标签与数值同一左轴、步进按钮贴右缘，避免窄列里文字压按钮。
    // 【数值直输 2026-11-13】点数值框直接输入（数字/退格，Enter 提交 Esc 取消），
    // 加减按钮仍保留；三列需要每列 ≥224px：标签+数值框+两个 32px 按钮才互不挤。
    let three = appearance_extra_row(w) == 0.;
    let typography_w = if three {
        (w - 32.) / 3.
    } else {
        (w - 24.) / 2.
    };
    let mut typo = vec![
        (
            "font",
            "字号".to_string(),
            format!("{:.0} px", cfg.font_size),
            Action::Font(-2),
            Action::Font(2),
        ),
        (
            "line",
            "行距".to_string(),
            format!("{:.2}", 1. + cfg.line_gap),
            Action::LineHeight(-1),
            Action::LineHeight(1),
        ),
    ];
    if three {
        typo.push((
            "letter",
            "字间距".to_string(),
            format!("{:.0} px", cfg.letter_spacing),
            crate::aux_marker("pref_step:letter_spacing:-1"),
            crate::aux_marker("pref_step:letter_spacing:1"),
        ));
    }
    let pitch = if three { 16. } else { 24. };
    for (row, (fid, name, value, minus, plus)) in typo.into_iter().enumerate() {
        let rx = x + row as f32 * (typography_w + pitch);
        let ry = y + APPEARANCE_TYPOGRAPHY_Y;
        // 标签一律短名（范围由直输时的 clamp 兜底；列宽 224px 放不下「标签+范围+数值框+双按钮」）。
        if three {
            text(f, rx, ry + 8., T_NOTE, &name, p.text2);
            value_box(f, hs, m, &p, fid, rx + typography_w - 144., ry, 64., &value);
        } else {
            // 窄列：标签在上、数值框在下（左轴对齐），按钮贴右缘不压字。
            text(f, rx, ry, T_NOTE, &name, p.text2);
            value_box(f, hs, m, &p, fid, rx, ry + 19., 64., &value);
        }
        step_button(f, hs, rx + typography_w - 76., ry, "−", minus, t);
        step_button(f, hs, rx + typography_w - 36., ry, "+", plus, t);
    }
    let dy = if three {
        0.
    } else {
        // 窄窗口：字间距独占一行，插在两列排版行下方（短名标签，同窄列口径）。
        let ry = y + APPEARANCE_TYPOGRAPHY_Y + APPEARANCE_NARROW_ROW_PITCH;
        // 与上方窄列同口径：标签顶对齐 ry、数值框从 ry+12 起。
        // （旧版标签用 ry+8、框用 ry+12，框会盖住标签下半行。）
        text(f, x, ry, T_NOTE, "字间距", p.text2);
        value_box(
            f,
            hs,
            m,
            &p,
            "letter",
            x,
            ry + 19.,
            64.,
            &format!("{:.0} px", cfg.letter_spacing),
        );
        step_button(
            f,
            hs,
            x + typography_w - 76.,
            ry,
            "−",
            crate::aux_marker("pref_step:letter_spacing:-1"),
            t,
        );
        step_button(
            f,
            hs,
            x + typography_w - 36.,
            ry,
            "+",
            crate::aux_marker("pref_step:letter_spacing:1"),
            t,
        );
        appearance_extra_row(w)
    };
    // 【审计 V08】数值直输的可发现性：光标形状由 main.rs 的窗口层管，本模块改不了；
    // 所以在这组数值下方补一行 11 号小字，和悬停高亮配合，明说不靠鼠标形状也能发现。
    // 三列时排版行底约 y+210，窄窗还有独占的「字间距」行（底 y+252），提示分开放。
    let hint_y = y + if three { 212. } else { 254. };
    direct_input_hint(f, x + w, hint_y, &p);
    // 小字占 18 DIP：其后各分组整体下移，避免小字贴住「动效强度」标题。
    // 窄窗再按【审计 V09】减 6 DIP（分组标题上下留白收紧）。
    let hint_dy = 18.;
    let stack = hint_dy - sec;
    section_label(f, x, y + 216. + dy + stack, "动效强度", &p);
    let motion_w = (w - 3. * 8.) / 4.;
    for (i, name) in ["关闭", "轻柔", "标准", "丰富"].iter().enumerate() {
        seg_button(
            f,
            hs,
            x + i as f32 * (motion_w + 8.),
            y + 238. + dy + stack,
            motion_w,
            30.,
            name,
            cfg.motion_level == i as u8,
            Action::MotionLevel(i as u8),
            &p,
        );
    }
    line(f, x, y + 278. + dy + stack, w, p.line);
    section_label(f, x, y + 290. + dy + stack, "跟打区背景与三态配色", &p);
    // 【审计 V13】配色清单沿用原有绘制；【审计 V09】窄窗行距在 color_controls 内收紧。
    color_controls(f, hs, m, t, x, y + dy + stack, w);
    // 三态预览（紧凑）
    let py = y + 568. + dy;
    round(f, x, py, w, 48., 8., p.article_bg, Some(p.line));
    text(f, x + 12., py + 5., T_NOTE, "跟打区三态预览", p.text2);
    for (i, (label, color, bg)) in [
        ("未打", p.text, p.bg_pending),
        ("打对", p.correct, p.bg_done),
        ("打错", p.red, p.bg_wrong),
    ]
    .iter()
    .enumerate()
    {
        let sx = x + 12. + i as f32 * (w - 24.) / 3.;
        text(f, sx, py + 27., T_NOTE, *label, p.muted);
        round(f, sx + 34., py + 24., 24., 20., 4., *bg, None);
        body_text(f, sx + 39., py + 24., 14., "字", *color);
    }
}

fn color_controls(f: &mut Frame, hs: &mut Vec<Hit>, m: &Model, t: &Theme, x: f32, y: f32, w: f32) {
    if hint_colors_view(m) {
        hint_color_controls(f, hs, m, t, x, y, w);
        return;
    }
    let p = tokens(t);
    let selected = selected_color(m);
    let col_w = (w - 12.) / 2.;
    // 【审计 V09】窄窗配色清单行距 30→28、行高 26→24：窗口只有 600 多 DIP 宽时
    // 七项颜色堆到屏幕外最多，收紧后滚动距离更短；仍留 4 DIP 间隙，
    // 也保证两个色块命中区不重叠（Hit 相撞会让点击选错颜色）。
    let (row_pitch, row_h) = if compact_width(w) {
        (28., 24.)
    } else {
        (30., 26.)
    };
    for (i, (key, label, fallback)) in page_colors(m).iter().enumerate() {
        let rx = x + (i % 2) as f32 * (col_w + 12.);
        let ry = y + 312. + (i / 2) as f32 * row_pitch;
        let on = selected == *key;
        round(
            f,
            rx,
            ry,
            col_w,
            row_h,
            6.,
            p.surface2,
            Some(if on { p.accent } else { p.line }),
        );
        let hex = color_hex(m, key, fallback);
        round(
            f,
            rx + 8.,
            ry + if row_h > 24. { 5. } else { 4. },
            24.,
            16.,
            4.,
            parse_color(&hex).unwrap_or(p.text2),
            Some(p.line2),
        );
        text(
            f,
            rx + 42.,
            ry + 6.,
            T_NOTE,
            elide_width(label, col_w - 76., T_NOTE),
            if on { p.accent } else { p.text },
        );
        right(f, rx + col_w - 10., ry + 6., T_NOTE, "›", p.muted);
        hit_box(
            hs,
            rx,
            ry,
            col_w,
            row_h,
            crate::aux_marker(&format!("pref_color_choose:{key}")),
        );
    }
    // 颜色行数由 COLORS 决定（2026-11-13 新增「未打文字色」后为 7 行）。
    // 提示语必须跟着最后一行下移，否则会压在末行色块上。
    let rows = page_colors(m).len().div_ceil(2) as f32;
    note(
        f,
        x,
        y + 312. + rows * row_pitch + 8.,
        w,
        "点击色块打开调色盘，选好后即时生效并保存。",
        p.muted,
    );
}

fn hint_word_color_key(key: &str) -> bool {
    matches!(
        key,
        "词提一码色"
            | "词提简码色"
            | "词提二字三码色"
            | "词提二字全码色"
            | "词提三字全码色"
            | "词提四字全码色"
    )
}

/// Word-class colors and generic annotation colors form two visual groups. The
/// flat two-column list made the six classes and the three helper colors look
/// equally important; the groups also keep the rank palette compact enough to
/// fit beside its preview on the narrow page.
fn hint_color_controls(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    t: &Theme,
    x: f32,
    y: f32,
    w: f32,
) {
    let p = tokens(t);
    note(
        f,
        x + w - 156.,
        y + 130.,
        156.,
        "点击色块打开调色盘",
        p.muted,
    );
    let mut cy = y + 152.;
    if enabled(m, "hint_rank_colors", false) {
        // Ten ranks fit as two clean rows at the normal panel width; the
        // narrow page keeps four columns so the labels never truncate.
        let rank_cols = if w >= 600. { 5 } else { 4 };
        cy = color_group(
            f,
            hs,
            m,
            t,
            x,
            cy,
            w,
            "候选序号配色",
            &RANK_COLORS[..10],
            rank_cols,
        );
        cy += 6.;
        color_group(f, hs, m, t, x, cy, w, "通用提示色", &RANK_COLORS[10..], 3);
    } else {
        let words: Vec<_> = HINT_COLORS
            .iter()
            .copied()
            .filter(|(key, _, _)| hint_word_color_key(key))
            .collect();
        let meta: Vec<_> = HINT_COLORS
            .iter()
            .copied()
            .filter(|(key, _, _)| !hint_word_color_key(key))
            .collect();
        cy = color_group(f, hs, m, t, x, cy, w, "六档词色", &words, 3);
        cy += 6.;
        color_group(f, hs, m, t, x, cy, w, "通用提示色", &meta, 3);
    }
    note(f, x, y + 344., w, "选好颜色后即时生效并保存。", p.muted);
}

fn color_group(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    t: &Theme,
    x: f32,
    mut y: f32,
    w: f32,
    title: &str,
    items: &[(&str, &str, &str)],
    cols: usize,
) -> f32 {
    let p = tokens(t);
    let cols = cols.max(1);
    rect(f, x, y + 2., 2., 11., alpha(p.accent, 0.72));
    text(f, x + 9., y, T_NOTE, title, p.text2);
    y += 20.;
    let gap = 8.;
    let col_w = (w - (cols - 1) as f32 * gap) / cols as f32;
    let selected = selected_color(m);
    for (i, (key, label, fallback)) in items.iter().enumerate() {
        let rx = x + (i % cols) as f32 * (col_w + gap);
        let ry = y + (i / cols) as f32 * 34.;
        let on = selected == *key;
        round(
            f,
            rx,
            ry,
            col_w,
            30.,
            6.,
            if on {
                alpha(p.accent, 0.05)
            } else {
                p.surface2
            },
            Some(if on { alpha(p.accent, 0.8) } else { p.line }),
        );
        let hex = color_hex(m, key, fallback);
        round(
            f,
            rx + 8.,
            ry + 6.,
            22.,
            18.,
            4.,
            parse_color(&hex).unwrap_or(p.text2),
            Some(alpha(p.line2, 0.9)),
        );
        let show_hex = col_w >= 168.;
        let label_w = col_w - 38. - if show_hex { 64. } else { 6. };
        text(
            f,
            rx + 38.,
            ry + 8.,
            T_NOTE,
            elide_width(label, label_w.max(24.), T_NOTE),
            if on { p.accent } else { p.text },
        );
        if show_hex {
            let hex_w = hex.chars().count() as f32 * 11. * 0.6;
            mono(
                f,
                rx + col_w - 8. - hex_w,
                ry + 9.,
                11.,
                hex,
                if on { alpha(p.accent, 0.85) } else { p.muted },
            );
        }
        hit_box(
            hs,
            rx,
            ry,
            col_w,
            30.,
            crate::aux_marker(&format!("pref_color_choose:{key}")),
        );
    }
    y + ((items.len().max(1) + cols - 1) / cols) as f32 * 34.
}

/// Full color preview for the palette page: six word classes, or all ten
/// candidate ranks. Unlike the compact hint-toggle preview this intentionally
/// shows the configured colors even while word hints are switched off, because
/// this page is the color editor.
fn hint_color_preview(f: &mut Frame, m: &Model, t: &Theme, x: f32, py: f32, w: f32) {
    let p = tokens(t);
    let rank_mode = enabled(m, "hint_rank_colors", false);
    let one_row = if rank_mode { w >= 680. } else { w >= 600. };
    let h = if one_row { 140. } else { 200. };
    round(f, x, py, w, h, 8., p.surface2, Some(p.line));
    text(f, x + 16., py + 12., T_LABEL, "配色预览", p.text);
    note(
        f,
        x + w - 16. - 110.,
        py + 14.,
        110.,
        "与理论分析同步",
        p.muted,
    );
    let show_code = enabled(m, "hint_show_code", true);
    let show_rank = enabled(m, "hint_show_rank", true);
    let max_len = if m.max_code_length == 0 {
        4
    } else {
        m.max_code_length
    };
    let class_examples = [
        ("一码词", "a", 1, "一简词"),
        ("简码", "ab", 1, "简码"),
        ("三码", "abc", 1, "二字三码"),
        ("全码", "abcd", 1, "二字全码"),
        ("三字词", "abcd", 1, "三字全码"),
        ("四字词语", "abcd", 1, "长词全码"),
    ];
    let rank_examples: [(&str, &str, usize, String); 10] = std::array::from_fn(|i| {
        let rank = i + 1;
        ("词组", "abcd", rank, format!("{rank} 选"))
    });
    let examples: Vec<(&str, &str, usize, String)> = if rank_mode {
        rank_examples.to_vec()
    } else {
        class_examples
            .iter()
            .map(|(word, code, rank, caption)| (*word, *code, *rank, (*caption).to_string()))
            .collect()
    };
    let cols = if one_row {
        examples.len()
    } else if rank_mode {
        5
    } else {
        3
    };
    let cell = (w - 32.) / cols as f32;
    let row_h = 66.;
    let size = 22.;
    let code_size = hint_size(m).min(16.);
    for (i, (word, code, rank, caption)) in examples.into_iter().enumerate() {
        let sx = x + 16. + (i % cols) as f32 * cell;
        let sy = py + 36. + (i / cols) as f32 * row_h;
        let caption_w = ui_text_width(&caption, 11.);
        text(
            f,
            sx + (cell - caption_w).max(0.) / 2.,
            sy,
            11.,
            &caption,
            if rank_mode { p.text2 } else { p.muted },
        );
        let word_y = sy + 15.;
        let tint = crate::ui::hint_word_tint_for_rank(&p, word, code, rank, max_len, rank_mode);
        let word_w = ui_text_width(word, size);
        let wx = sx + (cell - word_w).max(0.) / 2.;
        body_text(f, wx, word_y, size, word, tint.unwrap_or(p.text2));
        if crate::analysis::is_multi_word(word) {
            let underline = tint
                .map(|color| alpha(color, 0.62))
                .unwrap_or(p.hint_underline);
            // 【词提下行 r3】word_bracket 恢复 6 参：括线自算校准紧贴位。
            crate::ui::word_bracket(f, wx, word_y, word_w, size, underline);
        }
        let code_w = if show_code {
            code.chars().count() as f32 * code_size * 0.6
        } else {
            0.
        };
        let rank_size = code_size * 0.78;
        let rank_w = if show_rank && rank > 1 {
            rank.to_string().chars().count() as f32 * rank_size * 0.6
        } else {
            0.
        };
        let gap = if code_w > 0. && rank_w > 0. { 2. } else { 0. };
        let hx = sx + (cell - code_w - gap - rank_w).max(0.) / 2.;
        if show_code {
            mono(
                f,
                hx,
                word_y + size + 15.,
                code_size,
                code,
                alpha(tint.unwrap_or(p.hint_code), 0.92),
            );
        }
        if show_rank && rank > 1 {
            mono(
                f,
                hx + code_w + gap,
                word_y + size + 11.,
                rank_size,
                rank.to_string(),
                alpha(tint.unwrap_or(p.hint_badge), 0.95),
            );
        }
    }
    note(
        f,
        x + 16.,
        py + h - 20.,
        w - 32.,
        "配色作用于词组；单字与标点保持普通字色。",
        p.muted,
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::modals::{panel_for_model, Modal};

    fn preference_model(tab: u8) -> Model {
        let mut m = Model::default();
        m.open(Modal::Preferences);
        m.fields.insert("pref_tab".into(), tab.to_string());
        m
    }

    fn hit_targets(m: &Model, w: f32, h: f32) -> Vec<Hit> {
        let (x, y, pw, ph) = panel_for_model(m, w, h);
        let t = crate::ui::theme("墨玉");
        let mut f = Frame::new(w, h);
        let mut hs = Vec::new();
        draw(
            &mut f,
            &mut hs,
            m,
            &Session::new(100),
            &Config::default(),
            &t,
            x,
            y,
            pw,
            ph,
        );
        hs
    }

    fn drawn_round_rect(f: &Frame, x: f32, y: f32, w: f32, h: f32) -> bool {
        f.cmds.iter().any(|c| match c {
            DrawCmd::RoundRect {
                x: cx,
                y: cy,
                w: cw,
                h: ch,
                ..
            } => {
                (cx - x).abs() < 0.01
                    && (cy - y).abs() < 0.01
                    && (cw - w).abs() < 0.01
                    && (ch - h).abs() < 0.01
            }
            _ => false,
        })
    }

    #[test]
    fn custom_color_input_accepts_every_rgb_value_and_rejects_bad_shapes() {
        assert_eq!(
            parse_color("#123aef"),
            Some([18. / 255., 58. / 255., 239. / 255., 1.])
        );
        assert!(parse_color("63c08d").is_some());
        assert!(parse_color("12xyef").is_none());
        assert!(parse_color("abcd").is_none());
        assert!(parse_color("").is_none());
    }

    #[test]
    fn all_color_keys_covers_every_page_without_duplicates() {
        let keys = all_color_keys();
        for expected in [
            "未打文字色",
            "跟打区背景色",
            "词提简码色",
            "词提下划线色",
            "词提1选色",
            "词提10选色",
        ] {
            assert!(keys.contains(&expected), "missing color key {expected}");
        }
        // 三份清单有重复键（词提编码/角标/下划线、跟打区背景色），必须去重。
        let mut seen = std::collections::BTreeSet::new();
        for k in &keys {
            assert!(seen.insert(*k), "duplicate color key {k}");
        }
    }

    #[test]
    fn native_fields_are_bound_to_the_selected_preference_page() {
        let mut m = preference_model(0);
        let (x, y, pw, ph) = panel_for_model(&m, 1280., 800.);
        assert!(fields(&m, x, y, pw, ph).is_empty());
        m.fields.insert("pref_tab".into(), "1".into());
        let (x, y, pw, ph) = panel_for_model(&m, 1280., 800.);
        let v = fields(&m, x, y, pw, ph);
        assert_eq!(v[0].id, "score_template");
        assert!(v[0].multi);
        m.fields.insert("pref_tab".into(), "2".into());
        let (x, y, pw, ph) = panel_for_model(&m, 1280., 800.);
        assert!(fields(&m, x, y, pw, ph).is_empty());
        m.fields.insert("pref_tab".into(), "3".into());
        let (x, y, pw, ph) = panel_for_model(&m, 1280., 800.);
        assert!(fields(&m, x, y, pw, ph).is_empty());
        select_page(&mut m, 0);
        select_hint_view(&mut m, "colors");
        assert!(fields(&m, x, y, pw, ph).is_empty());
        select_hint_view(&mut m, "settings");
        assert!(fields(&m, x, y, pw, ph).is_empty());
    }

    #[test]
    fn native_editor_rectangles_match_the_drawn_boxes() {
        for (w, h, tab) in [(1280., 800., 1u8), (800., 600., 1)] {
            let m = preference_model(tab);
            let (x, y, pw, ph) = panel_for_model(&m, w, h);
            let t = crate::ui::theme("墨玉");
            let mut f = Frame::new(w, h);
            let mut hs = Vec::new();
            draw(
                &mut f,
                &mut hs,
                &m,
                &Session::new(100),
                &Config::default(),
                &t,
                x,
                y,
                pw,
                ph,
            );
            let field = fields(&m, x, y, pw, ph).remove(0);
            assert!(field.w > 0. && field.h > 0.);
            assert!(
                drawn_round_rect(&f, field.x, field.y, field.w, field.h),
                "tab={tab} {w}x{h}: 原生框与绘制矩形不一致"
            );
        }
    }

    #[test]
    fn native_fields_never_cover_preference_actions_while_scrolling() {
        let t = crate::ui::theme("墨玉");
        for (w, h) in [(1280., 800.), (840., 660.), (800., 600.), (640., 420.)] {
            for tab in 0..=5 {
                let mut m = preference_model(tab);
                let max = crate::modals::panel_scroll_max(&m, w, h, h);
                for scroll in [0., max * 0.5, max] {
                    m.panel_scroll = scroll;
                    let mut frame = Frame::new(w, h);
                    let mut hits = Vec::new();
                    crate::modals::draw(
                        &mut frame,
                        &mut hits,
                        &m,
                        &Session::new(100),
                        &Config::default(),
                        &t,
                        w,
                        h,
                        0.,
                        false,
                        None,
                    );
                    let mut native = crate::modals::fields(&m, w, h, None);
                    native.extend(crate::modals::font_picker(&m, w, h, None));
                    for field in native.iter().filter(|field| field.w > 0. && field.h > 0.) {
                        for button in hits
                            .iter()
                            .filter(|hit| hit.action != Action::None && hit.w > 0. && hit.h > 0.)
                        {
                            let overlap_w = (field.x + field.w).min(button.x + button.w)
                                - field.x.max(button.x);
                            let overlap_h = (field.y + field.h).min(button.y + button.h)
                                - field.y.max(button.y);
                            assert!(overlap_w <= 0. || overlap_h <= 0.,
                                "native {} covers {:?}: tab={tab} {w}x{h} scroll={scroll} overlap={overlap_w}x{overlap_h}",
                                field.id, button.action);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn system_font_picker_stays_inside_the_appearance_page_body() {
        for (w, h) in [(1280., 800.), (800., 600.)] {
            let mut m = preference_model(2);
            let field = crate::modals::font_picker(&m, w, h, None).unwrap();
            let hs = hit_targets(&m, w, h);
            let reset = hs
                .iter()
                .find(|hit| hit.action == Action::FontDefault)
                .unwrap();
            assert!(field.w >= 40. && field.h == 32.);
            assert!(field.x + field.w <= reset.x - 8.);
            if h == 600. {
                m.panel_scroll = 80.;
                let scrolled = crate::modals::font_picker(&m, w, h, None).unwrap();
                assert_eq!(scrolled.y, field.y - 80.);
                m.panel_scroll = 160.;
                assert!(crate::modals::font_picker(&m, w, h, None).is_none());
            }
            select_page(&mut m, 1);
            assert!(crate::modals::font_picker(&m, w, h, None).is_none());
            m.modal = crate::modals::Modal::None;
            assert!(crate::modals::font_picker(&m, w, h, None).is_none());
        }
    }

    #[test]
    fn content_height_is_fixed_so_the_panel_never_jumps_between_pages() {
        for tab in 0..=5u8 {
            let m = preference_model(tab);
            assert_eq!(content_height(&m, 960.), CONTENT_HEIGHT);
            assert_eq!(panel_for_model(&m, 800., 600.).3, 660.);
            assert_eq!(panel_for_model(&m, 1280., 800.).3, 660.);
        }
        // 短窗口下面板缩小到可用视口，靠整页滚动可达。
        assert!(panel_for_model(&preference_model(0), 800., 600.).3 > 600. - 48.);
        // 紧凑面板与逻辑内容高度独立；长页可滚动，不能为少滚一次把整窗撑大。
        let ph = panel_for_model(&preference_model(0), 1280., 800.).3;
        assert_eq!(ph, 660.);
        assert!(crate::modals::panel_scroll_max(&preference_model(0), 1280., 800., 800.) > 0.);
        assert_eq!(BODY_VISIBLE_800, BODY_BUDGET);
    }

    #[test]
    fn preference_tabs_are_clamped_to_the_sixth_page() {
        let mut m = preference_model(0);
        m.fields.insert("pref_tab".into(), "9".into());
        assert_eq!(tab(&m), 5);
        let hs = hit_targets(&m, 1280., 800.);
        assert!(hs
            .iter()
            .any(|h| h.action == crate::aux_marker("pref_shortcut:reset")));
        // 数据与关于页仍是第 5 个（下标 4），Folder 入口保留。
        let data = preference_model(4);
        assert!(hit_targets(&data, 1280., 800.)
            .iter()
            .any(|h| h.action == Action::Folder));
        m.fields.insert("pref_tab".into(), "x".into());
        assert_eq!(tab(&m), 0);
    }

    #[test]
    fn every_page_fits_its_scrollable_content_budget() {
        // 保留完整字号预览及底部呼吸空间；紧凑视口由滚动承接这份内容预算。
        assert!(SCORE_PREVIEW_Y + SCORE_PREVIEW_H <= BODY_VISIBLE_800 - 12.);
        // 外观与颜色页末端：三态预览 568+48，留 12px。
        assert!(568. + 48. <= BODY_VISIBLE_800 - 12.);
        assert!(PRACTICE_PREVIEW_Y + PRACTICE_PREVIEW_H <= BODY_VISIBLE_800);
        assert!(DATA_CARD2_Y + DATA_CARD_H <= BODY_VISIBLE_800);
        // 数据页第三张「练字记录」卡（2026-11-14）必须完整落在首屏内。
        assert!(DATA_PRACTICE_Y + DATA_PRACTICE_H <= BODY_VISIBLE_800 - 12.);
        assert_eq!(format_chars(0), "0");
        assert_eq!(format_chars(999), "999");
        assert_eq!(format_chars(1_284), "1,284");
        assert_eq!(format_chars(12_345), "1.2 万");
        assert_eq!(format_chars(1_234_567), "123.5 万");
        // 界面与显示页末端：分栏比例卡 348+76，底部提示 ~440+13。
        assert!(453. <= BODY_VISIBLE_800 - 12.);
        // 800×600 靠滚动可达：任何一页都不超过滚动上限。
        for end in [
            PRACTICE_PREVIEW_Y + PRACTICE_PREVIEW_H,
            SCORE_PREVIEW_Y + SCORE_PREVIEW_H,
            568. + 48.,
            453.,
            DATA_CARD2_Y + DATA_CARD_H,
            DATA_PRACTICE_Y + DATA_PRACTICE_H,
        ] {
            assert!(end <= BODY_BUDGET);
        }
    }

    /// 800×600 首屏（body 可视 428px）必须能点到导航与每页的主控件。
    #[test]
    fn first_screen_fits_the_nav_and_each_page_main_control_at_800x600() {
        for tab in 0..=5u8 {
            let m = preference_model(tab);
            let (x, y, pw, ph) = panel_for_model(&m, 800., 600.);
            let view = ph.min(600. - 48.);
            let visible_end = y + view - PANEL_FOOT;
            let hs = hit_targets(&m, 800., 600.);
            let inside = |hs: &[Hit], a: &Action| {
                hs.iter()
                    .any(|h| &h.action == a && h.y >= y && h.y + h.h <= visible_end)
            };
            for i in 0..6 {
                assert!(
                    inside(&hs, &crate::aux_marker(&format!("pref_tab:{i}"))),
                    "800×600 tab={tab}: 导航项 {i} 不在首屏"
                );
            }
            let anchors: Vec<Action> = match tab {
                0 => vec![
                    Action::ToggleHints,
                    crate::aux_marker("pref_toggle:hint_show_code"),
                    crate::aux_marker("pref_toggle:hint_show_rank"),
                    crate::aux_marker("pref_toggle:hint_color_words"),
                    crate::aux_marker("pref_strategy:shortest"),
                    crate::aux_marker("pref_strategy:phrases"),
                    crate::aux_marker("pref_toggle:hint_dynamic"),
                ],
                1 => vec![
                    crate::aux_marker("pref_toggle:score_auto_copy"),
                    crate::aux_marker("pref_score_preset:standard"),
                    crate::aux_marker("pref_save_template"),
                ],
                2 => vec![
                    Action::Theme("宣纸".into()),
                    Action::Font(2),
                    Action::MotionLevel(2),
                    crate::aux_marker(&format!("pref_color_choose:{}", COLORS[0].0)),
                ],
                3 => vec![
                    crate::aux_marker("pref_mode:split"),
                    crate::aux_marker("pref_mode:copybook"),
                    crate::aux_marker("pref_toggle:minimal"),
                    crate::aux_marker("pref_step:split_ratio:1"),
                ],
                4 => vec![Action::Folder],
                _ => vec![
                    crate::aux_marker("pref_shortcut:reset"),
                    crate::aux_marker("pref_shortcut:record:settings"),
                    crate::aux_marker("pref_shortcut:record:analysis"),
                ],
            };
            for a in anchors {
                assert!(inside(&hs, &a), "800×600 tab={tab}: 主控件 {a:?} 不在首屏");
            }
            let _ = (x, pw);
        }
    }

    #[test]
    fn preference_pages_expose_every_existing_action() {
        // 左侧导航在四页都常驻。
        for tab in 0..=5u8 {
            let hs = hit_targets(&preference_model(tab), 1280., 800.);
            for i in 0..6 {
                assert!(hs
                    .iter()
                    .any(|h| h.action == crate::aux_marker(&format!("pref_tab:{i}"))));
            }
            assert!(hs.iter().any(|h| h.action == Action::Close));
        }
        // 练习
        let hs = hit_targets(&preference_model(0), 1280., 800.);
        assert!(hs.iter().any(|h| h.action == Action::ToggleHints));
        for key in [
            "hint_show_code",
            "hint_show_rank",
            "hint_color_words",
            "hint_dynamic",
        ] {
            assert!(hs
                .iter()
                .any(|h| h.action == crate::aux_marker(&format!("pref_toggle:{key}"))));
        }
        for delta in ["-1", "1"] {
            assert!(hs.iter().any(
                |h| h.action == crate::aux_marker(&format!("pref_step:hint_font_size:{delta}"))
            ));
        }
        // 成绩单：两行开关、全部字段 chip、5 个预设、保存。
        let hs = hit_targets(&preference_model(1), 1280., 800.);
        for key in ["score_auto_copy", "auto_send_score"] {
            assert!(hs
                .iter()
                .any(|h| h.action == crate::aux_marker(&format!("pref_toggle:{key}"))));
        }
        for id in ["standard", "compact", "classic", "detailed", "custom"] {
            assert!(hs
                .iter()
                .any(|h| h.action == crate::aux_marker(&format!("pref_score_preset:{id}"))));
        }
        assert!(hs
            .iter()
            .any(|h| h.action == crate::aux_marker("pref_save_template")));
        for id in ["standard", "compact", "classic", "detailed", "custom"] {
            assert!(hs
                .iter()
                .any(|h| h.action == crate::aux_marker(&format!("pref_score_preset:{id}"))));
        }
        assert!(hs
            .iter()
            .any(|h| h.action == crate::aux_marker("pref_save_template")));
        // 外观与颜色页：主题、字号、动效与跟打区三态。
        let hs = hit_targets(&preference_model(2), 1280., 800.);
        for a in [
            Action::Theme("宣纸".into()),
            Action::Font(2),
            Action::FontDefault,
            Action::MotionLevel(2),
            Action::AppearanceReset,
        ] {
            assert!(hs.iter().any(|h| h.action == a));
        }
        for (key, _, _) in COLORS {
            assert!(hs
                .iter()
                .any(|h| h.action == crate::aux_marker(&format!("pref_color_choose:{key}"))));
        }
        assert_eq!(
            hs.iter()
                .filter(|h| h.action == crate::aux_marker("pref_apply_colors"))
                .count(),
            0
        );
        // 界面与显示页：双模式、极简、置顶与分栏比例。
        let hs = hit_targets(&preference_model(3), 1280., 800.);
        for a in [
            crate::aux_marker("pref_mode:copybook"),
            crate::aux_marker("pref_mode:split"),
            crate::aux_marker("pref_toggle:minimal"),
            crate::aux_marker("pref_toggle:minimal_hud"),
            crate::aux_marker("pref_toggle:always_on_top"),
        ] {
            assert!(hs.iter().any(|h| h.action == a), "界面页缺少 {a:?}");
        }
        for delta in ["-1", "1"] {
            assert!(hs
                .iter()
                .any(|h| h.action == crate::aux_marker(&format!("pref_step:split_ratio:{delta}"))));
        }
        // 数据页：打开目录。
        let hs = hit_targets(&preference_model(4), 1280., 800.);
        assert!(hs.iter().any(|h| h.action == Action::Folder));
    }

    #[test]
    fn active_round_locks_strategy_and_dynamic_without_hiding_display_controls() {
        let mut m = preference_model(0);
        let actions = [
            crate::aux_marker("pref_strategy:shortest"),
            crate::aux_marker("pref_strategy:phrases"),
            crate::aux_marker("pref_toggle:hint_dynamic"),
        ];
        let hs = hit_targets(&m, 640., 600.);
        assert!(actions
            .iter()
            .all(|action| hs.iter().any(|hit| &hit.action == action)));
        m.fields
            .insert("hint_settings_locked".into(), "true".into());
        let hs = hit_targets(&m, 640., 600.);
        assert!(actions
            .iter()
            .all(|action| hs.iter().all(|hit| &hit.action != action)));
        assert!(hs
            .iter()
            .any(|hit| hit.action == crate::aux_marker("pref_toggle:hint_show_code")));
    }

    #[test]
    fn word_colors_open_the_palette_and_fit_both_modes_on_the_narrow_page() {
        let mut m = preference_model(0);
        select_hint_view(&mut m, "colors");
        let w = 336.;
        for rank_mode in [false, true] {
            m.fields
                .insert("hint_rank_colors".into(), rank_mode.to_string());
            let mut f = Frame::new(w, BODY_BUDGET);
            let mut hs = Vec::new();
            practice_page(
                &mut f,
                &mut hs,
                &m,
                &Config::default(),
                &crate::ui::theme("墨玉"),
                0.,
                0.,
                w,
            );
            for (key, _, _) in page_colors(&m) {
                let hit = hs
                    .iter()
                    .find(|hit| {
                        hit.action == crate::aux_marker(&format!("pref_color_choose:{key}"))
                    })
                    .unwrap();
                assert!(hit.x >= 0. && hit.x + hit.w <= w);
                assert!(hit.y >= 152. && hit.y + hit.h <= 380.);
            }
            assert!(fields(&m, 0., 0., 640., CONTENT_HEIGHT).is_empty());
            assert!(f.cmds.iter().all(|cmd| !matches!(cmd, DrawCmd::TextRun {text, ..} if text.contains("RRGGBB") || text.starts_with('#'))));
        }
    }

    #[test]
    fn switching_color_pages_rebinds_the_editor_and_keeps_colors_in_their_page() {
        let mut m = preference_model(2);
        m.fields
            .insert("color_跟打区背景色".into(), "123456".into());
        m.fields.insert("color_词提编码色".into(), "654321".into());
        sync_color_editor(&mut m);
        assert_eq!(m.field("pref_color_key"), "跟打区背景色");
        assert_eq!(m.field("pref_color_hex"), "123456");
        let epoch = m.modal_epoch;
        m.panel_scroll = 200.;
        select_page(&mut m, 0);
        select_hint_view(&mut m, "colors");
        assert_eq!(m.field("pref_color_key"), "词提编码色");
        assert_eq!(m.field("pref_color_hex"), "654321");
        assert_eq!(m.panel_scroll, 0.);
        assert_eq!(m.modal_epoch, epoch + 2);
        let hints = hit_targets(&m, 1280., 800.);
        let appearance = hit_targets(&preference_model(2), 1280., 800.);
        for (colors, own, other) in [
            (HINT_COLORS, &hints, &appearance),
            (COLORS, &appearance, &hints),
        ] {
            for (key, _, _) in colors {
                let action = crate::aux_marker(&format!("pref_color_choose:{key}"));
                assert!(own.iter().any(|hit| hit.action == action));
                assert!(other.iter().all(|hit| hit.action != action));
            }
        }
        assert!(color_allowed(&m, "词提下划线色"));
        assert!(!color_allowed(&m, "跟打区背景色"));
        select_page(&mut m, 2);
        assert_eq!(m.field("pref_color_key"), "跟打区背景色");
        assert_eq!(m.field("pref_color_hex"), "123456");
        assert!(!color_allowed(&m, "词提一码色"));
    }

    #[test]
    fn practice_preview_matches_word_colors_and_candidate_rank_rules() {
        let t = crate::ui::theme("墨玉");
        let p = tokens(&t);
        let mut m = preference_model(0);
        m.hints = true;
        m.max_code_length = 4;
        let render = |m: &Model| {
            let mut f = Frame::new(640., 600.);
            practice_page(
                &mut f,
                &mut Vec::new(),
                m,
                &Config::default(),
                &t,
                0.,
                0.,
                336.,
            );
            f
        };
        let body_color = |f: &Frame, word: &str| {
            f.cmds
                .iter()
                .find_map(|cmd| match cmd {
                    DrawCmd::TextRun {
                        text,
                        color,
                        font: FontFamily::Lxgw,
                        ..
                    } if text == word => Some(*color),
                    _ => None,
                })
                .unwrap()
        };
        let mono_texts = |f: &Frame| {
            f.cmds
                .iter()
                .filter_map(|cmd| match cmd {
                    DrawCmd::TextRun {
                        text,
                        y,
                        font: FontFamily::Mono,
                        ..
                    } if *y > PRACTICE_PREVIEW_Y - 24. => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let f = render(&m);
        assert_eq!(body_color(&f, "的"), p.text2);
        assert_eq!(
            body_color(&f, "简明"),
            crate::ui::hint_word_tint(&p, "简明", "a", 4).unwrap()
        );
        assert_eq!(
            body_color(&f, "来来来"),
            crate::ui::hint_word_tint(&p, "来来来", "aaah", 4).unwrap()
        );
        assert_eq!(mono_texts(&f), ["u", "a", "aaah", "2"]);
        m.fields.insert("hint_show_code".into(), "false".into());
        assert_eq!(mono_texts(&render(&m)), ["2"]);
        m.fields.insert("hint_show_rank".into(), "false".into());
        m.fields.insert("hint_color_words".into(), "false".into());
        let f = render(&m);
        assert!(mono_texts(&f).is_empty());
        assert_eq!(body_color(&f, "简明"), p.text2);
        m.fields.clear();
        m.hints = false;
        let f = render(&m);
        assert!(mono_texts(&f).is_empty());
        assert_eq!(body_color(&f, "来来来"), p.text2);
    }

    #[test]
    fn hint_size_display_follows_config_range_not_legacy_cap() {
        // 七轮：显示口径与 config 解析同域（10..=100），不再被旧 28 上限卡在界面上。
        let mut m = preference_model(0);
        assert_eq!(hint_size(&m), 14.);
        m.fields.insert("hint_font_size".into(), "99".into());
        assert_eq!(hint_size(&m), 99.);
        m.fields.insert("hint_font_size".into(), "2".into());
        assert_eq!(hint_size(&m), 10.);
    }

    #[test]
    fn narrow_practice_page_keeps_four_switches_and_size_controls_reachable() {
        let m = preference_model(0);
        let hs = hit_targets(&m, 640., 600.);
        for action in [
            Action::ToggleHints,
            crate::aux_marker("pref_toggle:hint_show_code"),
            crate::aux_marker("pref_toggle:hint_show_rank"),
            crate::aux_marker("pref_toggle:hint_color_words"),
            crate::aux_marker("pref_step:hint_font_size:-1"),
            crate::aux_marker("pref_step:hint_font_size:1"),
        ] {
            let hit = hs.iter().find(|h| h.action == action).unwrap();
            assert!(hit.x >= 0. && hit.x + hit.w <= 640.);
        }
        assert!(PRACTICE_PREVIEW_Y + PRACTICE_PREVIEW_H <= BODY_BUDGET);
    }

    /// 回归：新增第 7 个色块后，末行色块与提示语必须分行（2026-11-13）。
    #[test]
    fn color_hint_stays_below_the_last_color_row() {
        let m = preference_model(2);
        let t = crate::ui::theme("墨玉");
        for width in [336., 704., 912.] {
            let mut f = Frame::new(width, 752.);
            let mut hs = Vec::new();
            appearance_page(
                &mut f,
                &mut hs,
                &m,
                &Session::new(100),
                &Config::default(),
                &t,
                0.,
                0.,
                width,
            );
            let last = page_colors(&m).last().unwrap().0.clone();
            let row = hs
                .iter()
                .find(|h| h.action == crate::aux_marker(&format!("pref_color_choose:{last}")))
                .unwrap();
            let note_y = f
                .cmds
                .iter()
                .filter_map(|cmd| match cmd {
                    DrawCmd::TextRun { y, text, .. } if text.contains("点击色块打开调色盘") => {
                        Some(*y)
                    }
                    _ => None,
                })
                .fold(f32::INFINITY, f32::min);
            assert!(
                row.y + row.h + 2. <= note_y,
                "width={width}: last row ends at {}, hint starts at {note_y}",
                row.y + row.h
            );
        }
    }

    /// 回归：窄窗「字间距」标签不能盖住自身数值，也不能落入上一行点击区。
    #[test]
    fn narrow_letter_spacing_label_sits_above_its_value_box() {
        let m = preference_model(2);
        let t = crate::ui::theme("墨玉");
        let cfg = Config::default();
        for width in [336., 384., 600.] {
            let mut f = Frame::new(width, 752.);
            let mut hs = Vec::new();
            appearance_page(
                &mut f,
                &mut hs,
                &m,
                &Session::new(100),
                &cfg,
                &t,
                0.,
                0.,
                width,
            );
            let text_box = |label: &str| {
                f.cmds
                    .iter()
                    .find_map(|cmd| match cmd {
                        DrawCmd::TextRun { x, y, h, text, .. } if text == label => {
                            Some((*x, *y, ui_text_width(text, *h), *h))
                        }
                        _ => None,
                    })
                    .unwrap()
            };
            let label = text_box("字间距");
            let value = text_box("0 px");
            assert!(
                label.1 + label.3 <= value.1 + 0.5,
                "width={width}: label ends at {}, value starts at {}",
                label.1 + label.3,
                value.1
            );
            for hit in hs.iter().filter(|hit| {
                hit.action == crate::aux_marker("pref_num_edit:font")
                    || hit.action == crate::aux_marker("pref_num_edit:line")
            }) {
                let overlaps = label.0 < hit.x + hit.w
                    && label.0 + label.2 > hit.x
                    && label.1 < hit.y + hit.h
                    && label.1 + label.3 > hit.y;
                assert!(
                    !overlaps,
                    "width={width}: 字间距标签落入上一行数值框 {:?}",
                    hit.action
                );
            }
        }
    }

    #[test]
    fn typography_labels_values_and_buttons_do_not_overlap_in_narrow_columns() {
        let m = preference_model(2);
        let t = crate::ui::theme("墨玉");
        let mut cfg = Config::default();
        cfg.font_size = 100.;
        cfg.line_gap = 2.2;
        for width in [336., 704.] {
            let mut f = Frame::new(width, 752.);
            let mut hs = Vec::new();
            appearance_page(
                &mut f,
                &mut hs,
                &m,
                &Session::new(100),
                &cfg,
                &t,
                0.,
                0.,
                width,
            );
            let text_box = |label: &str| {
                f.cmds
                    .iter()
                    .find_map(|cmd| match cmd {
                        // Align::Center 的 x 是中心点（见 centered()）：换算成左上角盒。
                        DrawCmd::TextRun {
                            x,
                            y,
                            h,
                            text,
                            align: Align::Center,
                            ..
                        } if text == label => {
                            let w = ui_text_width(text, *h);
                            Some((*x - w / 2., *y, w, *h))
                        }
                        DrawCmd::TextRun { x, y, h, text, .. } if text == label => {
                            Some((*x, *y, ui_text_width(text, *h), *h))
                        }
                        _ => None,
                    })
                    .unwrap()
            };
            let intersects = |a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)| {
                a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
            };
            for (label, value, minus, plus) in [
                ("字号", "100 px", Action::Font(-2), Action::Font(2)),
                (
                    "行距",
                    "3.20",
                    Action::LineHeight(-1),
                    Action::LineHeight(1),
                ),
            ] {
                let label_box = text_box(label);
                let value_box = text_box(value);
                assert!(
                    !intersects(label_box, value_box),
                    "width={width}: {label}/{value}"
                );
                for action in [minus, plus] {
                    let hit = hs.iter().find(|hit| hit.action == action).unwrap();
                    let button = (hit.x, hit.y, hit.w, hit.h);
                    assert!(
                        !intersects(label_box, button),
                        "width={width}: {label}/{action:?}"
                    );
                    assert!(
                        !intersects(value_box, button),
                        "width={width}: {value}/{action:?}"
                    );
                }
            }
            // 【数值直输 2026-11-13】三个数值框点击目标都在视口内
            // （窄窗字间距独占一行同样有点击框）。
            for marker in [
                "pref_num_edit:font",
                "pref_num_edit:line",
                "pref_num_edit:letter",
            ] {
                let action = crate::aux_marker(marker);
                let hit = hs.iter().find(|h| h.action == action).unwrap();
                assert!(
                    hit.x >= 0. && hit.x + hit.w <= width,
                    "width={width}: {marker} hit {}..{}",
                    hit.x,
                    hit.x + hit.w
                );
            }
        }
    }

    /// 【2026-11-13】提示预览的选重数字必须实时跟随「高低/加粗」设置，
    /// 且示例在格内水平居中（此前是写死高度、左对齐）。
    #[test]
    fn hint_preview_rank_digit_follows_offset_and_bold_settings() {
        let mut m = preference_model(0);
        m.hints = true;
        m.fields.insert("hint_show_rank".into(), "true".into());
        m.fields.insert("hint_show_code".into(), "true".into());
        m.fields.insert("hint_rank_corner".into(), "false".into());
        let render = |cfg: &Config, m: &Model| {
            let mut f = Frame::new(960., 752.);
            let mut hs = Vec::new();
            practice_page(
                &mut f,
                &mut hs,
                m,
                cfg,
                &crate::ui::theme("墨玉"),
                0.,
                0.,
                960.,
            );
            f
        };
        let rank_y = |f: &Frame| {
            f.cmds
                .iter()
                .find_map(|cmd| match cmd {
                    DrawCmd::TextRun {
                        text,
                        y,
                        font: FontFamily::Mono,
                        ..
                    } if text == "2" && *y > PRACTICE_PREVIEW_Y => Some(*y),
                    _ => None,
                })
                .expect("preview rank digit")
        };
        let count_rank = |f: &Frame| {
            f.cmds
                .iter()
                .filter(|c| {
                    matches!(c, DrawCmd::TextRun { text, font: FontFamily::Mono, .. } if text == "2")
                })
                .count()
        };
        let mut cfg = Config::default();
        cfg.hint_rank_offset = 0.;
        let base = render(&cfg, &m);
        cfg.hint_rank_offset = 8.;
        let shifted = render(&cfg, &m);
        assert!(
            (rank_y(&shifted) - rank_y(&base) - 8.).abs() < 0.01,
            "offset must move the digit live: {} -> {}",
            rank_y(&base),
            rank_y(&shifted)
        );
        assert_eq!(count_rank(&base), 1);
        cfg.hint_rank_bold = true;
        let bold = render(&cfg, &m);
        assert_eq!(count_rank(&bold), 2, "bold draws an offset copy");
    }
    #[test]
    fn hint_number_inputs_preview_the_entered_size_and_offset_without_shrinking() {
        let mut m = preference_model(0);
        m.hints = true;
        m.fields.insert("pref_num_field".into(), "hint_font".into());
        m.fields.insert("pref_num_buf".into(), "99".into());
        let cfg = Config::default();
        let preview = typography_preview(&m, &cfg);
        assert_eq!(preview.hint_font_size, 99.);
        assert!(
            content_height(&m, 960.) > CONTENT_HEIGHT,
            "大字号预览应可滚动，不缩小字体"
        );
        let mut frame = Frame::new(960., 1000.);
        hint_preview(
            &mut frame,
            &m,
            &cfg,
            &crate::ui::theme("墨玉"),
            0.,
            0.,
            656.,
        );
        assert!(frame.cmds.iter().any(|cmd| matches!(cmd,
            DrawCmd::TextRun { text, h, .. } if text == "abcd" && *h == 99.
        )));
        m.fields
            .insert("pref_num_field".into(), "hint_code_offset".into());
        m.fields.insert("pref_num_buf".into(), "18".into());
        assert_eq!(typography_preview(&m, &cfg).hint_code_offset, 18.);
        assert_eq!(
            typography_preview(&m, &cfg).hint_font_size,
            cfg.hint_font_size
        );
    }

    #[test]
    fn code_hint_and_typed_code_number_controls_are_distinct_and_reachable() {
        for (tab, markers) in [
            (
                0,
                vec![
                    "pref_num_edit:hint_font",
                    "pref_num_edit:hint_code_offset",
                    "pref_num_edit:hint_rank_offset",
                ],
            ),
            (
                3,
                vec![
                    "pref_toggle:show_preedit",
                    "pref_num_edit:preedit_font",
                    "pref_num_edit:preedit_offset",
                ],
            ),
        ] {
            for width in [640., 800., 1280.] {
                let hs = hit_targets(&preference_model(tab), width, 800.);
                for marker in &markers {
                    let h = hs
                        .iter()
                        .find(|h| h.action == crate::aux_marker(marker))
                        .expect(marker);
                    assert!(h.x >= 0. && h.x + h.w <= width, "{marker} width={width}");
                }
            }
        }
    }
    #[test]
    fn shortcut_labels_have_clear_space_before_every_key_chip_in_narrow_windows() {
        for (w, h) in [(640., 420.), (800., 600.), (1280., 800.)] {
            let model = preference_model(5);
            let (x, y, pw, ph) = panel_for_model(&model, w, h);
            let mut frame = Frame::new(w, h);
            let mut hits = Vec::new();
            draw(
                &mut frame,
                &mut hits,
                &model,
                &Session::new(100),
                &Config::default(),
                &crate::ui::theme("墨玉"),
                x,
                y,
                pw,
                ph,
            );
            for (id, label, _) in gale_core::config::SHORTCUT_DEFAULTS {
                let key = hits
                    .iter()
                    .find(|hit| {
                        hit.action == crate::aux_marker(&format!("pref_shortcut:record:{id}"))
                    })
                    .unwrap();
                let label_x = frame
                    .cmds
                    .iter()
                    .find_map(|cmd| match cmd {
                        DrawCmd::TextRun { text, x, .. } if text == label => Some(*x),
                        _ => None,
                    })
                    .unwrap();
                assert!(
                    label_x + ui_text_width(label, T_LABEL) + 12. <= key.x + 0.01,
                    "{label} must remain readable before its key chip at {w}x{h}"
                );
            }
        }
    }

    #[test]
    fn preference_hit_targets_do_not_overlap_at_supported_sizes() {
        for tab in 0..=5u8 {
            for (w, h) in [(1280., 800.), (800., 600.)] {
                let hs = hit_targets(&preference_model(tab), w, h);
                for (i, a) in hs.iter().enumerate() {
                    assert!(a.w > 0. && a.h > 0., "tab={tab} {w}x{h} 出现零面积热区");
                    for b in hs.iter().skip(i + 1) {
                        let overlap = a.x < b.x + b.w
                            && b.x < a.x + a.w
                            && a.y < b.y + b.h
                            && b.y < a.y + a.h;
                        assert!(
                            !overlap,
                            "tab={tab} {w}x{h}: {:?} 与 {:?} 热区重叠",
                            a.action, b.action
                        );
                    }
                }
            }
        }
    }

    // -------------------------------------------------【审计 V08/V09/V13】单测
    //
    // 三条审计的难点都在「判定口径」而不在绘制：主题名映射、覆盖计数口径、
    // 窄窗阈值、可滚动提示的显隐、悬停命中与滚动的偏移关系。这些都是纯数值/
    // 纯字符串逻辑，抽成函数后必须直接测，否则回归只在肉眼可见处暴露。

    /// V13：配置未设主题时存 "default"，用户看到的是首个内置主题「墨玉」。
    #[test]
    fn theme_status_shows_the_name_the_user_actually_sees() {
        assert_eq!(theme_display_name("default"), "墨玉");
        assert_eq!(theme_display_name("宣纸"), "宣纸");
        // 「跟随虎符」是动态主题名，必须原样透出，不能落回任何内置默认值。
        assert_eq!(theme_display_name("跟随虎符"), "跟随虎符");
    }

    /// V13：覆盖计数只数「恢复默认外观」真会清掉的那批键。
    #[test]
    fn override_count_matches_what_appearance_reset_actually_clears() {
        let mut cfg = Config::default();
        assert_eq!(appearance_override_count(&cfg), 0);
        cfg.color_overrides
            .insert(COLORS[0].0.to_string(), "#123456".to_string());
        cfg.color_overrides
            .insert(COLORS[3].0.to_string(), "#654321".to_string());
        assert_eq!(appearance_override_count(&cfg), 2);
        // 词提配色以「词提」开头，AppearanceReset 会 retain 它们 → 不能计入 N。
        cfg.color_overrides
            .insert("词提一码色".to_string(), "#abcdef".to_string());
        assert_eq!(appearance_override_count(&cfg), 2);
        // 未知键（历史遗留/手改进配置）同样不计入，避免提示数字与实际不符。
        cfg.color_overrides
            .insert("不存在的颜色".to_string(), "#000000".to_string());
        assert_eq!(appearance_override_count(&cfg), 2);
    }

    /// V13：N=0 时不占位（不出现「已覆盖 0 项颜色」这种噪音）。
    #[test]
    fn override_text_is_absent_when_nothing_is_overridden() {
        assert_eq!(override_text(0), None);
        assert_eq!(override_text(1).as_deref(), Some("已覆盖 1 项颜色"));
        assert_eq!(override_text(7).as_deref(), Some("已覆盖 7 项颜色"));
    }

    /// V09：窄窗阈值等价于「面板 < 760 DIP」，默认 840 面板（正文列 616）
    /// 与 800×600 窗口（正文列 544）都不得触发，否则默认窗口也被当窄窗。
    #[test]
    fn compact_density_only_kicks_in_below_the_760_dip_panel() {
        assert!(!compact_width(616.)); // 默认 840 面板
        assert!(!compact_width(544.)); // 800×600 窗口
        assert!(!compact_width(COMPACT_BODY_W)); // 边界值本身仍按常规密度
        assert!(compact_width(535.9));
        assert!(compact_width(384.)); // 640×420 窗口
    }

    /// V09：快捷键行距与滚动高度必须同源，否则末行会被页脚盖住或多出空白。
    #[test]
    fn shortcut_row_height_tightens_with_compact_density() {
        assert_eq!(shortcut_row_h(704.), SHORTCUT_ROW_H);
        assert!(shortcut_row_h(384.) < shortcut_row_h(704.));
        // 键位框 28 高 + 2 偏移 = 30，紧凑行距仍要留出空隙，否则上下行热区相接。
        assert!(shortcut_row_h(384.) >= 30. + 4.);
        // 滚动高度必须用同一个行距算，否则滚动范围与实际排版脱节。
        // 注意窄窗列数更少、行数更多，绝对高度不一定更小 —— 只能比「公式是否同源」。
        for w in [384f32, 704.] {
            let rows = gale_core::config::SHORTCUT_DEFAULTS
                .len()
                .div_ceil(shortcut_columns(w)) as f32;
            let expect = SHORTCUT_TOP + rows * shortcut_row_h(w) + 12. + 20. + T_NOTE + 16.;
            assert!(
                (shortcut_body_height(w) - expect).abs() < 0.01,
                "{w}: body height {} should follow row height {expect}",
                shortcut_body_height(w)
            );
        }
    }

    /// V09：悬停判定必须减掉 panel_scroll，才能和 fit_panel 平移后的命中区对齐。
    #[test]
    fn hover_detection_follows_the_scroll_offset() {
        let mut m = Model::default();
        let (x, y, w, h) = (100., 200., 48., 26.);
        // 未滚动：指针在框内命中、框外不命中。
        m.pointer = Some((120., 210.));
        assert!(pointer_over(&m, x, y, w, h));
        m.pointer = Some((120., 100.));
        assert!(!pointer_over(&m, x, y, w, h));
        // 指针缺失（例如触控/键盘操作）时一律不命中，避免误亮。
        m.pointer = None;
        assert!(!pointer_over(&m, x, y, w, h));
        // 正文上移 150 后，同一个逻辑 y 在屏幕上的位置也上移 150：
        // 指针停在 210 时框已经跑到 y=50..76（指针不在其上），停在 60 时才算悬停。
        m.panel_scroll = 150.;
        m.pointer = Some((120., 60.));
        assert!(pointer_over(&m, x, y, w, h));
        m.pointer = Some((120., 210.));
        assert!(!pointer_over(&m, x, y, w, h));
    }

    /// V09：「可滚动」提示只在「面板被窗口裁短 + 尚未滚动」时出现一次。
    #[test]
    fn scroll_hint_only_shows_before_the_first_scroll() {
        // 默认 840×660 面板整块显示：不提示（正文仍需轻微滚动也不提示，避免噪音）。
        assert!(!needs_scroll_hint(660., 660., 0.));
        // 800×600：面板被裁到 552，提示。
        assert!(needs_scroll_hint(660., 552., 0.));
        // 已经滚过：收掉（不做常驻闪烁）。
        assert!(!needs_scroll_hint(660., 552., 12.));
        // 浮点误差不该让提示误显/误隐。
        assert!(!needs_scroll_hint(660., 659.8, 0.));
        assert!(needs_scroll_hint(660., 659.2, 0.));
    }

    /// V09：小窗（800×600 外观页）确实会在页脚画出「可滚动」提示，
    /// 默认 1280×800（面板 840×660 能整块显示）不画。
    #[test]
    fn footer_renders_the_scroll_hint_only_when_the_panel_is_clipped() {
        let hint_shown = |w: f32, h: f32| {
            let m = preference_model(2);
            let (x, y, pw, ph) = panel_for_model(&m, w, h);
            let t = crate::ui::theme("墨玉");
            let mut f = Frame::new(w, h);
            let mut hs = Vec::new();
            draw(
                &mut f,
                &mut hs,
                &m,
                &Session::new(100),
                &Config::default(),
                &t,
                x,
                y,
                pw,
                ph,
            );
            f.cmds.iter().any(|cmd| match cmd {
                DrawCmd::TextRun { text, .. } => text.contains("可滚动"),
                _ => false,
            })
        };
        assert!(!hint_shown(1280., 800.), "默认窗口外观页不该提示可滚动");
        assert!(hint_shown(800., 600.), "800×600 外观页应提示可滚动");
    }
}
