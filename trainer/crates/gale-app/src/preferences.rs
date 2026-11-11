//! F1 跟打器设置：左侧四页导航 + 右侧内容区（词提/成绩单/外观与颜色/数据）。
//!
//! 面板几何由 `modals` 提供（960×752，与 1280×800 视口等高，内容区
//! `y+62 .. y+ph-62`，左侧导航 208）；
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
use gale_core::config::Config;
use gale_core::scorecard::{format_score_template, SCORE_FIELDS, SCORE_PRESETS};
use gale_core::theme::Theme;
use gale_win::d2d::{Align, DrawCmd, FontFamily, Frame};
use std::path::PathBuf;

// ------------------------------------------------------------------ 规格常量

/// 左侧导航宽度（规格 §6）。
const NAV_W: f32 = 208.;
/// 内容区左右内边距。
const PAD: f32 = 24.;
const NAV_ITEM_H: f32 = 40.;
const NAV_ITEM_GAP: f32 = 6.;
const NAV_INSET: f32 = 12.;
/// 面板固定高度：四页切换不跳动，短窗口整页滚动。
/// 取 752 = 1280×800 下 `fit_panel` 的 view（h−48），面板与视口等高，
/// 不会出现「内容本来放得下却恒画 8px 滚动条」的错觉。
const CONTENT_HEIGHT: f32 = 752.;
/// 1280×800 下首屏可见的内容高度（面板 view=752，头脚各 62）。
const BODY_VISIBLE_800: f32 = 752. - PANEL_HEAD - PANEL_FOOT;
/// 滚动可达的内容高度上限（面板高 − 头脚；与 1280×800 首屏一致）。
const BODY_BUDGET: f32 = CONTENT_HEIGHT - PANEL_HEAD - PANEL_FOOT;

/// 页标题 16；正文标签 15；说明 13；数字等宽。
const T_TITLE: f32 = 16.;
const T_LABEL: f32 = 15.;
const T_NOTE: f32 = 13.;

const PAGES: [(&str, &str); 4] = [
    ("词提", "提示开关、策略与配色"),
    ("成绩单", "自动复制与成绩单样式"),
    ("外观与颜色", "主题、排版、动效与配色"),
    ("数据与关于", "存储位置与版本"),
];

const COLORS: &[(&str, &str, &str)] = &[
    ("跟打区背景色", "跟打区背景", "111315"),
    ("对打未打底色", "未打底色", "111315"),
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

// 词提页
const PRACTICE_SWITCH_Y: f32 = 72.;
const PRACTICE_SWITCH_H: f32 = 40.;
const PRACTICE_SWITCH_PITCH: f32 = 44.;
const PRACTICE_STRATEGY_Y: f32 = 298.;
const PRACTICE_DYNAMIC_Y: f32 = 358.;
const PRACTICE_SIZE_LABEL_Y: f32 = 406.;
const PRACTICE_SIZE_Y: f32 = 424.;
const PRACTICE_SIZE_H: f32 = 48.;
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
const SCORE_EDITOR_Y: f32 = 332.;
const SCORE_EDITOR_H: f32 = 76.;
const SCORE_PREVIEW_Y: f32 = 464.;
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

// -------------------------------------------------------------------- 取值层

fn tab(m: &Model) -> u8 {
    m.field("pref_tab").parse::<u8>().unwrap_or(0).min(3)
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
    m.fields.insert("pref_tab".into(), page.min(3).to_string());
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
    m.field("hint_font_size")
        .parse::<f32>()
        .unwrap_or(14.)
        .clamp(14., 28.)
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
fn nav_item_y(y: f32, index: usize) -> f32 {
    y + PANEL_HEAD + 16. + index as f32 * (NAV_ITEM_H + NAV_ITEM_GAP)
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

// -------------------------------------------------------------------- 左侧导航

fn nav(f: &mut Frame, hs: &mut Vec<Hit>, m: &Model, t: &Theme, x: f32, y: f32) {
    let p = tokens(t);
    // 竖分隔线：rect 而非 line，长度覆盖整个可视内容区。
    rect(
        f,
        x + NAV_W,
        y + PANEL_HEAD,
        1.,
        (CONTENT_HEIGHT - PANEL_HEAD - PANEL_FOOT).max(0.),
        p.line,
    );
    for (i, (title, desc)) in PAGES.iter().enumerate() {
        let iy = nav_item_y(y, i);
        let ix = x + NAV_INSET;
        let iw = NAV_W - 2. * NAV_INSET;
        let on = tab(m) == i as u8;
        if on {
            round(f, ix, iy, iw, NAV_ITEM_H, 8., alpha(p.accent, 0.08), None);
            round(f, ix, iy + 8., 3., NAV_ITEM_H - 16., 1.5, p.accent, None);
        }
        text(
            f,
            ix + 14.,
            iy + 4.,
            T_LABEL,
            *title,
            if on { p.accent } else { p.text2 },
        );
        note(f, ix + 14., iy + 22., iw - 24., desc, p.muted);
        hit_box(
            hs,
            ix,
            iy,
            iw,
            NAV_ITEM_H,
            crate::aux_marker(&format!("pref_tab:{i}")),
        );
    }
}

// -------------------------------------------------------------------- 练习页

fn practice_page(f: &mut Frame, hs: &mut Vec<Hit>, m: &Model, t: &Theme, x: f32, y: f32, w: f32) {
    let p = tokens(t);
    page_title(f, x, y, w, "词提", "正文与 F10 按当前码表分析", &p);
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
        cy + 25.,
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
    for (i, (key, title, desc)) in [
        ("hint_show_code", "显示编码", "在字词下方显示当前码表编码"),
        (
            "hint_show_rank",
            "显示选重数字",
            "首选不标数字，从第二候选起显示",
        ),
        (
            "hint_rank_corner",
            "选重数字角标",
            "数字显示在词的右上角；关闭时随编码行显示",
        ),
        (
            "hint_color_words",
            "词组着色",
            "仅词组着色，单字与标点保持普通字色",
        ),
    ]
    .iter()
    .enumerate()
    {
        let cy = y + PRACTICE_SWITCH_Y + (i + 1) as f32 * PRACTICE_SWITCH_PITCH;
        round(f, x, cy, w, PRACTICE_SWITCH_H, 8., p.surface2, Some(p.line));
        toggle_row(
            f,
            hs,
            m,
            t,
            x,
            cy,
            w,
            PRACTICE_SWITCH_H,
            key,
            title,
            desc,
            true,
            4.,
            23.,
        );
    }
    let locked = m.field("hint_settings_locked") == "true";
    let strategy_y = y + PRACTICE_STRATEGY_Y;
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
    let dynamic_y = y + PRACTICE_DYNAMIC_Y;
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
        4.,
        23.,
    );
    if locked {
        hs.truncate(route_hits);
        rect(f, x, strategy_y + 20., w, 30., alpha(p.bg, 0.35));
        rect(f, x, dynamic_y, w, PRACTICE_SWITCH_H, alpha(p.bg, 0.35));
    }
    // 字号步进：数值 + − / + 一组 32 高按钮。
    section_label(f, x, y + PRACTICE_SIZE_LABEL_Y, "提示字号", &p);
    let sy = y + PRACTICE_SIZE_Y;
    round(f, x, sy, w, PRACTICE_SIZE_H, 8., p.surface2, Some(p.line));
    text(f, x + 16., sy + 8., T_LABEL, "编码与选重字号", p.text);
    note(
        f,
        x + 16.,
        sy + 31.,
        w - 16. - 16. - 118.,
        "范围 14–28，默认 14",
        p.muted,
    );
    right(
        f,
        x + w - 16. - 84.,
        sy + 15.,
        20.,
        format!("{:.0}", hint_size(m)),
        p.accent,
    );
    step_button(
        f,
        hs,
        x + w - 16. - 72.,
        sy + 12.,
        "−",
        crate::aux_marker("pref_step:hint_font_size:-1"),
        t,
    );
    step_button(
        f,
        hs,
        x + w - 16. - 32.,
        sy + 12.,
        "+",
        crate::aux_marker("pref_step:hint_font_size:1"),
        t,
    );
    hint_preview(f, m, t, x, y + PRACTICE_PREVIEW_Y, w);
}

fn hint_preview(f: &mut Frame, m: &Model, t: &Theme, x: f32, py: f32, w: f32) {
    let p = tokens(t);
    round(
        f,
        x,
        py,
        w,
        PRACTICE_PREVIEW_H,
        8.,
        p.surface2,
        Some(p.line),
    );
    text(f, x + 16., py + 12., T_LABEL, "提示预览", p.text);
    note(f, x + w - 16. - 80., py + 14., 80., "规则示例", p.muted);
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
    let examples: &[(&str, &str, usize, &str)] = if rank_mode {
        &rank_examples
    } else {
        &class_examples
    };
    let cell = (w - 32.) / examples.len() as f32;
    let size = hint_size(m).min((cell - 10.) / 3.);
    for (i, (word, code, rank, caption)) in examples.iter().enumerate() {
        let sx = x + 16. + i as f32 * cell;
        let tint = color_words
            .then(|| crate::ui::hint_word_tint_for_rank(&p, word, code, *rank, max_len, rank_mode))
            .flatten();
        body_text(f, sx, py + 42., 24., *word, tint.unwrap_or(p.text2));
        if m.hints && word.chars().count() > 1 {
            crate::ui::word_bracket(
                f,
                sx,
                py + 42.,
                word.chars().count() as f32 * 24.,
                24.,
                tint.unwrap_or(p.hint_underline),
            );
        }
        if show_code {
            mono(f, sx, py + 81., size, *code, tint.unwrap_or(p.hint_code));
        }
        if show_rank && *rank > 1 {
            // 【选重数字角标 2026-11-11】与正文渲染同口径：角标模式把
            // 数字画到词右上角，否则跟在编码后面。
            if enabled(m, "hint_rank_corner", false) {
                let ds = 24. * 0.52;
                let dw = rank.to_string().chars().count() as f32 * ds * 0.6;
                let word_w = word.chars().count() as f32 * 24.;
                mono(
                    f,
                    sx + word_w - dw - 1.,
                    py + 42. - ds * 0.38,
                    ds,
                    rank.to_string(),
                    tint.unwrap_or(p.hint_badge),
                );
            } else {
                let code_w = if show_code {
                    code.chars().count() as f32 * size * 0.6
                } else {
                    0.
                };
                mono(
                    f,
                    sx + code_w + 2.,
                    py + 77.,
                    size,
                    rank.to_string(),
                    tint.unwrap_or(p.hint_badge),
                );
            }
        }
        note(f, sx, py + 100., cell - 8., caption, p.muted);
    }
    note(
        f,
        x + 16.,
        py + 122.,
        w - 32.,
        if rank_mode {
            "相同候选序号同色；单字、标点不着色。"
        } else {
            "正文与 F10 共用词类配色；单字、标点不着色。"
        },
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
        round(f, cx, y + 50., core_w, 60., 7., p.surface2, Some(p.line));
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
                    y + 78. + line as f32 * 16.,
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
        21.,
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
        21.,
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

fn data_page(f: &mut Frame, hs: &mut Vec<Hit>, t: &Theme, x: f32, y: f32, w: f32) {
    let p = tokens(t);
    page_title(f, x, y, w, "数据与关于", "存储位置、版本与影响范围", &p);
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

fn footer(f: &mut Frame, hs: &mut Vec<Hit>, t: &Theme, x: f32, y: f32, pw: f32, ph: f32) {
    let p = tokens(t);
    let top = y + ph - PANEL_FOOT;
    rect(f, x + 1., top, pw - 2., PANEL_FOOT - 1., alpha(p.bg2, 0.6));
    line(f, x, top, pw, p.line);
    text(f, x + PAD, top + 24., T_NOTE, "更改即时保存", p.muted);
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
pub(super) fn content_height(_m: &Model, _pw: f32) -> f32 {
    CONTENT_HEIGHT
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
    nav(f, hs, m, t, x, y);
    let (bx, by, bw) = (body_x(x), body_y(y), body_w(pw));
    match tab(m) {
        0 => practice_page(f, hs, m, t, bx, by, bw),
        1 => score_page(f, hs, m, t, bx, by, bw),
        2 => appearance_page(f, hs, m, sess, cfg, t, bx, by, bw),
        _ => data_page(f, hs, t, bx, by, bw),
    }
    footer(f, hs, t, x, y, pw, ph);
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
    section_label(f, x, y + 32., "主题", &p);
    let theme_w = (w - 4. * 8.) / 5.;
    for (i, name) in ["墨玉", "宣纸", "夜巡", "青瓷", "朱砂"].iter().enumerate() {
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
    let typography_w = (w - 24.) / 2.;
    for (row, (name, value, minus, plus)) in [
        (
            "字号 10–100",
            format!("{:.0} px", cfg.font_size),
            Action::Font(-2),
            Action::Font(2),
        ),
        (
            "行距",
            format!("{:.2}", 1. + cfg.line_gap),
            Action::LineHeight(-1),
            Action::LineHeight(1),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let rx = x + row as f32 * (typography_w + 24.);
        let ry = y + 178.;
        if typography_w < 220. {
            // At 640px the column is 156px wide: stack label and value in
            // its left 76px, leaving the two buttons their full width.
            text(f, rx, ry, T_NOTE, name, p.text2);
            mono(f, rx, ry + 17., 14., value, p.text);
        } else {
            text(f, rx, ry + 8., T_NOTE, name, p.text2);
            mono(f, rx + typography_w - 132., ry + 8., 14., value, p.text);
        }
        step_button(f, hs, rx + typography_w - 76., ry, "−", minus, t);
        step_button(f, hs, rx + typography_w - 36., ry, "+", plus, t);
    }
    section_label(f, x, y + 216., "动效强度", &p);
    let motion_w = (w - 3. * 8.) / 4.;
    for (i, name) in ["关闭", "轻柔", "标准", "丰富"].iter().enumerate() {
        seg_button(
            f,
            hs,
            x + i as f32 * (motion_w + 8.),
            y + 238.,
            motion_w,
            30.,
            name,
            cfg.motion_level == i as u8,
            Action::MotionLevel(i as u8),
            &p,
        );
    }
    line(f, x, y + 278., w, p.line);
    section_label(f, x, y + 290., "跟打区背景与三态配色", &p);
    color_controls(f, hs, m, t, x, y, w);
    // 三态预览（紧凑）
    let py = y + 568.;
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
    for (i, (key, label, fallback)) in page_colors(m).iter().enumerate() {
        let rx = x + (i % 2) as f32 * (col_w + 12.);
        let ry = y + 312. + (i / 2) as f32 * 30.;
        let on = selected == *key;
        round(
            f,
            rx,
            ry,
            col_w,
            26.,
            6.,
            p.surface2,
            Some(if on { p.accent } else { p.line }),
        );
        let hex = color_hex(m, key, fallback);
        round(
            f,
            rx + 8.,
            ry + 5.,
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
            26.,
            crate::aux_marker(&format!("pref_color_choose:{key}")),
        );
    }
    note(
        f,
        x,
        y + 416.,
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
        "正文与 F10 同步",
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
    fn system_font_picker_stays_inside_the_appearance_page_body() {
        for (w, h) in [(1280., 800.), (800., 600.)] {
            let mut m = preference_model(2);
            let field = crate::modals::font_picker(&m, w, h).unwrap();
            let hs = hit_targets(&m, w, h);
            let reset = hs
                .iter()
                .find(|hit| hit.action == Action::FontDefault)
                .unwrap();
            assert!(field.w >= 40. && field.h == 32.);
            assert!(field.x + field.w <= reset.x - 8.);
            if h == 600. {
                m.panel_scroll = 80.;
                let scrolled = crate::modals::font_picker(&m, w, h).unwrap();
                assert_eq!(scrolled.y, field.y - 80.);
                m.panel_scroll = 160.;
                assert!(crate::modals::font_picker(&m, w, h).is_none());
            }
            select_page(&mut m, 1);
            assert!(crate::modals::font_picker(&m, w, h).is_none());
            m.modal = crate::modals::Modal::None;
            assert!(crate::modals::font_picker(&m, w, h).is_none());
        }
    }

    #[test]
    fn content_height_is_fixed_so_the_panel_never_jumps_between_pages() {
        for tab in 0..=4u8 {
            let m = preference_model(tab);
            assert_eq!(content_height(&m, 960.), CONTENT_HEIGHT);
            assert_eq!(panel_for_model(&m, 800., 600.).3, CONTENT_HEIGHT);
            assert_eq!(panel_for_model(&m, 1280., 800.).3, CONTENT_HEIGHT);
        }
        // 短窗口下面板缩小到可用视口，靠整页滚动可达。
        assert!(panel_for_model(&preference_model(0), 800., 600.).3 > 600. - 48.);
        // 1280×800：面板高 == 视口 view（h−48）→ scroll range 0，不画多余滚动条。
        let ph = panel_for_model(&preference_model(0), 1280., 800.).3;
        assert_eq!(ph, 800. - 48.);
        assert_eq!(BODY_VISIBLE_800, BODY_BUDGET);
    }

    #[test]
    fn preference_tabs_are_clamped_to_the_fourth_page() {
        let mut m = preference_model(0);
        m.fields.insert("pref_tab".into(), "9".into());
        assert_eq!(tab(&m), 3);
        let hs = hit_targets(&m, 1280., 800.);
        assert!(hs.iter().any(|h| h.action == Action::Folder));
        m.fields.insert("pref_tab".into(), "x".into());
        assert_eq!(tab(&m), 0);
    }

    #[test]
    fn every_page_fits_the_first_screen_at_1280x800() {
        // 最底部的两页留出 ≥12px 呼吸空间，避免卡片下沿压在页脚分割线上。
        assert!(SCORE_PREVIEW_Y + SCORE_PREVIEW_H <= BODY_VISIBLE_800 - 12.);
        // 外观与颜色页末端：三态预览 568+48，留 12px。
        assert!(568. + 48. <= BODY_VISIBLE_800 - 12.);
        assert!(PRACTICE_PREVIEW_Y + PRACTICE_PREVIEW_H <= BODY_VISIBLE_800);
        assert!(DATA_CARD2_Y + DATA_CARD_H <= BODY_VISIBLE_800);
        // 800×600 靠滚动可达：任何一页都不超过滚动上限。
        for end in [
            PRACTICE_PREVIEW_Y + PRACTICE_PREVIEW_H,
            SCORE_PREVIEW_Y + SCORE_PREVIEW_H,
            568. + 48.,
            DATA_CARD2_Y + DATA_CARD_H,
        ] {
            assert!(end <= BODY_BUDGET);
        }
    }

    /// 800×600 首屏（body 可视 428px）必须能点到导航与每页的主控件。
    #[test]
    fn first_screen_fits_the_nav_and_each_page_main_control_at_800x600() {
        for tab in 0..=3u8 {
            let m = preference_model(tab);
            let (x, y, pw, ph) = panel_for_model(&m, 800., 600.);
            let view = ph.min(600. - 48.);
            let visible_end = y + view - PANEL_FOOT;
            let hs = hit_targets(&m, 800., 600.);
            let inside = |hs: &[Hit], a: &Action| {
                hs.iter()
                    .any(|h| &h.action == a && h.y >= y && h.y + h.h <= visible_end)
            };
            for i in 0..4 {
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
                _ => vec![Action::Folder],
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
        for tab in 0..=3u8 {
            let hs = hit_targets(&preference_model(tab), 1280., 800.);
            for i in 0..4 {
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
        // 数据页：打开目录。
        let hs = hit_targets(&preference_model(3), 1280., 800.);
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
            practice_page(&mut f, &mut hs, &m, &crate::ui::theme("墨玉"), 0., 0., w);
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
            practice_page(&mut f, &mut Vec::new(), m, &t, 0., 0., 336.);
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
                        font: FontFamily::Mono,
                        ..
                    } => Some(text.clone()),
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
                ("字号 10–100", "100 px", Action::Font(-2), Action::Font(2)),
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
        }
    }

    #[test]
    fn preference_hit_targets_do_not_overlap_at_supported_sizes() {
        for tab in 0..=4u8 {
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
}
