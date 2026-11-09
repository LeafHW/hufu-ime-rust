//! F1 跟打器设置：左侧四页导航 + 右侧内容区（设计规格 §6）。
//!
//! 面板几何由 `modals` 提供（960×752，与 1280×800 视口等高，内容区
//! `y+62 .. y+ph-62`，左侧导航 208）；
//! 本模块只负责排版、绘制与原生编辑框坐标。`pref_*` 动作语义保持与 `main.rs`
//! 一致：`pref_tab:{0..3}`、`pref_toggle:{key}`、`pref_step:hint_font_size:±1`、
//! `pref_field:{key}`、`pref_score_preset:{id}`、`pref_save_template`、
//! `pref_color_select:{key}`、`pref_apply_colors`，以及 `Action::Folder`。
use super::{hit_box, wrap, Action, FieldSpec, Hit, Model, PANEL_FOOT, PANEL_HEAD};
use crate::ui::{
    alpha, body_text, elide_width, line, mono, rect, right, round, text, tokens, ui_text_width,
    Palette,
};
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
    ("练习", "单字提示与字号"),
    ("成绩单", "自动复制、字段与模板"),
    ("颜色与提示", "跟打配色与提示样式"),
    ("数据与关于", "存储位置与版本"),
];

const COLORS: &[(&str, &str, &str)] = &[
    ("跟打区背景色", "跟打区背景", "111315"),
    ("对打未打底色", "未打底色", "111315"),
    ("打对字色", "打对文字", "e7e4db"),
    ("对打打对底色", "打对底色", "173c2b"),
    ("打错字色", "打错文字", "e36b65"),
    ("对打打错底色", "打错底色", "4b2021"),
    ("词提编码色", "单字编码", "d0c9b9"),
    ("词提角标色", "选重数字", "d0c9b9"),
];

// 练习页
const PRACTICE_SWITCH_Y: f32 = 48.;
const PRACTICE_SWITCH_H: f32 = 56.;
const PRACTICE_SWITCH_PITCH: f32 = 64.;
const PRACTICE_SIZE_LABEL_Y: f32 = 176.;
const PRACTICE_SIZE_Y: f32 = 196.;
const PRACTICE_SIZE_H: f32 = 56.;
const PRACTICE_PREVIEW_Y: f32 = 264.;
const PRACTICE_PREVIEW_H: f32 = 150.;

// 成绩单页
const SCORE_G1_LABEL_Y: f32 = 24.;
const SCORE_G1_Y: f32 = 44.;
const SCORE_G1_ROW_H: f32 = 40.;
const SCORE_G2_LABEL_Y: f32 = 132.;
const SCORE_CHIPS_Y: f32 = 154.;
const SCORE_CHIP_PITCH: f32 = 32.;
const SCORE_CHIP_H: f32 = 30.;
const SCORE_CHIP_COLUMNS: usize = 4;
const SCORE_G3_TITLE_Y: f32 = 384.;
const SCORE_SAVE_Y: f32 = 382.;
const SCORE_PRESETS_Y: f32 = 418.;
const SCORE_PRESETS_H: f32 = 30.;
const SCORE_EDITOR_Y: f32 = 454.;
const SCORE_EDITOR_H: f32 = 76.;
const SCORE_PREVIEW_Y: f32 = 536.;
const SCORE_PREVIEW_H: f32 = 80.;

// 颜色页
const COLOR_LABEL_Y: f32 = 30.;
const COLOR_ROWS_Y: f32 = 52.;
const COLOR_ROW_H: f32 = 34.;
const COLOR_ROW_GAP: f32 = 6.;
const COLOR_HEX_LABEL_Y: f32 = 370.;
const COLOR_HEX_Y: f32 = 390.;
const COLOR_HEX_H: f32 = 38.;
const COLOR_HEX_W: f32 = 220.;
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
    COLORS
        .iter()
        .find(|(key, _, _)| *key == value)
        .map(|v| v.0)
        .unwrap_or(COLORS[0].0)
}
fn selected_color_label(m: &Model) -> &'static str {
    let value = selected_color(m);
    COLORS
        .iter()
        .find(|(key, _, _)| *key == value)
        .map(|v| v.1)
        .unwrap_or(COLORS[0].1)
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
        "race" => "赛制标记",
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
/// 颜色值输入框（内容区坐标；绘制与原生 FieldSpec 共用同一组常量）。
fn color_field_rect(bx: f32, by: f32) -> (f32, f32, f32, f32) {
    (bx, by + COLOR_HEX_Y, COLOR_HEX_W, COLOR_HEX_H)
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
fn primary(f: &mut Frame, hs: &mut Vec<Hit>, x: f32, y: f32, w: f32, label: &str, a: Action, t: &Theme) {
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
fn step_button(f: &mut Frame, hs: &mut Vec<Hit>, x: f32, y: f32, label: &str, a: Action, t: &Theme) {
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
    note(f, x + 16., y + desc_dy, w - 16. - 16. - 44. - 12., desc, p.muted);
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
            round(
                f,
                ix,
                iy + 8.,
                3.,
                NAV_ITEM_H - 16.,
                1.5,
                p.accent,
                None,
            );
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
    page_title(
        f,
        x,
        y,
        w,
        "单字练习提示",
        "编码与选重只出现在单字练习",
        &p,
    );
    section_label(f, x, y + 30., "提示开关", &p);
    for (i, (key, title, desc)) in [
        (
            "hint_show_code",
            "显示单字编码",
            "在练习字下方显示当前码表编码",
        ),
        (
            "hint_show_rank",
            "显示选重数字",
            "显示候选位次，辅助练习选重",
        ),
    ]
    .iter()
    .enumerate()
    {
        let cy = y + PRACTICE_SWITCH_Y + i as f32 * PRACTICE_SWITCH_PITCH;
        round(
            f,
            x,
            cy,
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
            cy,
            w,
            PRACTICE_SWITCH_H,
            key,
            title,
            desc,
            true,
            8.,
            31.,
        );
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
    right(f, x + w - 16. - 84., sy + 15., 20., format!("{:.0}", hint_size(m)), p.accent);
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
    // 预览卡：字 / 编码 / 选重三例，脚注收进卡内。
    let py = y + PRACTICE_PREVIEW_Y;
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
    note(f, x + w - 16. - 160., py + 14., 160., "仅单字练习显示", p.muted);
    let show_code = enabled(m, "hint_show_code", true);
    let show_rank = enabled(m, "hint_show_rank", true);
    let size = hint_size(m);
    let cell = (w - 32.) / 3.;
    for (i, (code, caption)) in [("a", "单码"), ("ab", "两码"), ("abc", "三码")]
        .iter()
        .enumerate()
    {
        let sx = x + 16. + i as f32 * cell;
        body_text(f, sx, py + 42., 24., "字", p.text);
        if show_code {
            mono(f, sx, py + 72., size, *code, p.hint_code);
        }
        if show_rank {
            let code_w = if show_code {
                code.chars().count() as f32 * size * 0.6
            } else {
                0.
            };
            mono(f, sx + code_w + 2., py + 68., size, "2", p.hint_badge);
        }
        note(f, sx, py + 100., cell - 8., caption, p.muted);
    }
    note(
        f,
        x + 16.,
        py + 122.,
        w - 32.,
        "文章练习不显示编码与选重提示。",
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
    page_title(f, x, y, w, "成绩单", "打完自动执行的行为与输出内容", &p);

    // 1. 自动行为：两行开关，不放进字段网格。
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

    // 2. 输出内容：4 列字段 chips。
    section_label(f, x, y + SCORE_G2_LABEL_Y, "输出内容", &p);
    let hint_w = (w - 140.).min(340.);
    note(
        f,
        x + w - hint_w,
        y + SCORE_G2_LABEL_Y,
        hint_w,
        "自由模板优先于勾选；点字段会切回字段模式",
        p.muted,
    );
    let active = m.field("score_fields");
    for (i, (key, title)) in SCORE_FIELDS.iter().enumerate() {
        let (cx, cy, cw) = score_chip_rect(x, y + SCORE_CHIPS_Y, w, i);
        let on = active
            .split(',')
            .any(|item| canonical_field(item.trim()) == *key);
        field_chip(
            f,
            hs,
            cx,
            cy,
            cw,
            chip_label(key, title),
            on,
            crate::aux_marker(&format!("pref_field:{key}")),
            &p,
        );
    }

    // 3. 模板：5 段预设 + 多行编辑框 + 保存 + 最近成绩预览。
    text(f, x, y + SCORE_G3_TITLE_Y, T_TITLE, "成绩单模板", p.text);
    note(
        f,
        x + 104.,
        y + SCORE_G3_TITLE_Y + 3.,
        240.,
        &format!("当前：{}", score_mode(m)),
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
    let preset = m.field("score_preset");
    let presets = [
        ("standard", "标准".to_string()),
        ("compact", "简洁".to_string()),
        ("classic", SCORE_PRESETS[0].name.to_string()),
        ("detailed", SCORE_PRESETS[1].name.to_string()),
        ("custom", "自由编辑".to_string()),
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
    let (ex, ey, ew, eh) = score_editor_rect(x, y, w);
    round(f, ex, ey, ew, eh, 6., p.bg, Some(p.line2));

    let py = y + SCORE_PREVIEW_Y;
    round(
        f,
        x,
        py,
        w,
        SCORE_PREVIEW_H,
        8.,
        p.surface2,
        Some(p.line),
    );
    text(f, x + 16., py + 8., T_NOTE, "最近一次成绩预览", p.text);
    let per_line = (((w - 32.) / T_NOTE) as usize).max(8);
    let lines = wrap(&score_preview_text(m), per_line);
    let total = lines.len();
    for (i, row) in lines.into_iter().take(2).enumerate() {
        let row = if total > 2 && i == 1 {
            format!(
                "{}…",
                row.chars().take(per_line.saturating_sub(1)).collect::<String>()
            )
        } else {
            row
        };
        text(f, x + 16., py + 25. + i as f32 * 17., T_NOTE, row, p.text2);
    }
    note(
        f,
        x + 16.,
        py + 61.,
        w - 32.,
        "占位符 {speed} 速度 · {character_accuracy} 字准 · {key_accuracy} 键准 · {strokes} 击键；字准/键准需自行加 %",
        p.muted,
    );
}

// ------------------------------------------------------------------ 颜色页

fn colors_page(f: &mut Frame, hs: &mut Vec<Hit>, m: &Model, t: &Theme, x: f32, y: f32, w: f32) {
    let p = tokens(t);
    page_title(f, x, y, w, "颜色与提示", "跟打区三态配色与单字提示样式", &p);
    section_label(f, x, y + COLOR_LABEL_Y, "跟打区配色", &p);
    let selected = selected_color(m);
    for (i, (key, label, fallback)) in COLORS.iter().enumerate() {
        let ry = color_row_y(y, i);
        let on = selected == *key;
        round(
            f,
            x,
            ry,
            w,
            COLOR_ROW_H,
            7.,
            if on {
                alpha(p.accent, 0.08)
            } else {
                p.surface2
            },
            Some(if on { p.accent } else { p.line }),
        );
        let hex = color_hex(m, key, fallback);
        round(
            f,
            x + 10.,
            ry + 7.,
            20.,
            20.,
            5.,
            parse_color(&hex).unwrap_or(p.text2),
            Some(p.line2),
        );
        text(f, x + 40., ry + 7., T_LABEL, *label, if on { p.accent } else { p.text });
        right(
            f,
            x + w - 12.,
            ry + 8.,
            T_LABEL,
            format!("#{}", hex.to_uppercase()),
            if on { p.text2 } else { p.muted },
        );
        hit_box(
            hs,
            x,
            ry,
            w,
            COLOR_ROW_H,
            crate::aux_marker(&format!("pref_color_select:{key}")),
        );
    }
    // 自定义颜色值：原生输入框 + 色块 + 应用按钮。
    let (fx, fy, fw, fh) = color_field_rect(x, y);
    section_label(f, x, y + COLOR_HEX_LABEL_Y, "颜色值（#RRGGBB）", &p);
    let edit_w = (w - 170.).min(300.);
    note(
        f,
        x + w - edit_w,
        y + COLOR_HEX_LABEL_Y,
        edit_w,
        &format!("正在编辑：{}", selected_color_label(m)),
        p.muted,
    );
    round(f, fx, fy, fw, fh, 6., p.bg, Some(p.line2));
    let input = m.field("pref_color_hex");
    let swatch = parse_color(&input).unwrap_or(p.surface2);
    round(f, fx + fw + 12., fy, 40., fh, 6., swatch, Some(p.line2));
    let bw = 104.;
    primary(
        f,
        hs,
        x + w - bw,
        fy + 1.,
        bw,
        "应用颜色",
        crate::aux_marker("pref_apply_colors"),
        t,
    );
    note(
        f,
        x,
        y + COLOR_HINT_Y,
        w,
        if parse_color(&input).is_some() {
            "格式有效 · 应用后即时生效并保存"
        } else {
            "请输入六位十六进制，例如 63C08D；可带 # 前缀"
        },
        p.muted,
    );
    // 三态预览：未打 / 打对 / 打错 + 编码。
    let py = y + COLOR_PREVIEW_Y;
    round(
        f,
        x,
        py,
        w,
        COLOR_PREVIEW_H,
        8.,
        p.article_bg,
        Some(p.line),
    );
    text(f, x + 16., py + 12., T_LABEL, "跟打区三态预览", p.text2);
    let cell = (w - 32.) / 3.;
    for (i, (label, color, bg)) in [
        ("未打", p.text, p.bg_pending),
        ("打对", p.correct, p.bg_done),
        ("打错", p.red, p.bg_wrong),
    ]
    .iter()
    .enumerate()
    {
        let sx = x + 16. + i as f32 * cell;
        text(f, sx, py + 40., T_NOTE, *label, p.muted);
        round(f, sx, py + 60., 72., 34., 5., *bg, None);
        body_text(f, sx + 10., py + 64., 22., "示例", *color);
        if i == 0 {
            mono(f, sx, py + 102., T_LABEL, "abc", p.hint_code);
            mono(f, sx + 32., py + 98., T_LABEL, "2", p.hint_badge);
        }
    }
    note(
        f,
        x + 16.,
        py + 130.,
        w - 32.,
        "三态颜色同时作用于正文与单字提示；编码/选重取「词提」两色",
        p.muted,
    );
}

// ------------------------------------------------------------------ 数据页

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
        format!("虎符跟打器 {}", env!("CARGO_PKG_VERSION")),
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

// ---------------------------------------------------------------------- 页脚

fn footer(f: &mut Frame, hs: &mut Vec<Hit>, t: &Theme, x: f32, y: f32, pw: f32, ph: f32) {
    let p = tokens(t);
    let top = y + ph - PANEL_FOOT;
    rect(f, x + 1., top, pw - 2., PANEL_FOOT - 1., alpha(p.bg2, 0.6));
    line(f, x, top, pw, p.line);
    text(f, x + PAD, top + 24., T_NOTE, "更改即时保存", p.muted);
    primary(f, hs, x + pw - PAD - 96., top + 13., 96., "完成", Action::Close, t);
}

// ---------------------------------------------------------------- 对外接口

/// 原生编辑框：成绩单模板与颜色值输入。坐标与 `draw` 共用同一组常量。
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
        2 => {
            let (fx, fy, fw, fh) = color_field_rect(bx, by);
            vec![FieldSpec {
                id: "pref_color_hex",
                x: fx,
                y: fy,
                w: fw,
                h: fh,
                multi: false,
                password: false,
            }]
        }
        _ => Vec::new(),
    }
}

/// 固定面板高度：四页切换不跳动，短窗口整页滚动。
pub(super) fn content_height(_m: &Model, _pw: f32) -> f32 {
    CONTENT_HEIGHT
}

pub(super) fn draw(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
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
        2 => colors_page(f, hs, m, t, bx, by, bw),
        _ => data_page(f, hs, t, bx, by, bw),
    }
    footer(f, hs, t, x, y, pw, ph);
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
        draw(&mut f, &mut hs, m, &t, x, y, pw, ph);
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
        let v = fields(&m, x, y, pw, ph);
        assert_eq!(v[0].id, "pref_color_hex");
        assert!(!v[0].multi);
        m.fields.insert("pref_tab".into(), "3".into());
        let (x, y, pw, ph) = panel_for_model(&m, 1280., 800.);
        assert!(fields(&m, x, y, pw, ph).is_empty());
    }

    #[test]
    fn native_editor_rectangles_match_the_drawn_boxes() {
        for (w, h, tab) in [
            (1280., 800., 1u8),
            (800., 600., 1),
            (1280., 800., 2),
            (800., 600., 2),
        ] {
            let m = preference_model(tab);
            let (x, y, pw, ph) = panel_for_model(&m, w, h);
            let t = crate::ui::theme("墨玉");
            let mut f = Frame::new(w, h);
            let mut hs = Vec::new();
            draw(&mut f, &mut hs, &m, &t, x, y, pw, ph);
            let field = fields(&m, x, y, pw, ph).remove(0);
            assert!(field.w > 0. && field.h > 0.);
            assert!(
                drawn_round_rect(&f, field.x, field.y, field.w, field.h),
                "tab={tab} {w}x{h}: 原生框与绘制矩形不一致"
            );
        }
    }

    #[test]
    fn content_height_is_fixed_so_the_panel_never_jumps_between_pages() {
        for tab in 0..=3u8 {
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
        assert!(COLOR_PREVIEW_Y + COLOR_PREVIEW_H <= BODY_VISIBLE_800 - 12.);
        assert!(PRACTICE_PREVIEW_Y + PRACTICE_PREVIEW_H <= BODY_VISIBLE_800);
        assert!(DATA_CARD2_Y + DATA_CARD_H <= BODY_VISIBLE_800);
        // 800×600 靠滚动可达：任何一页都不超过滚动上限。
        for end in [
            PRACTICE_PREVIEW_Y + PRACTICE_PREVIEW_H,
            SCORE_PREVIEW_Y + SCORE_PREVIEW_H,
            COLOR_PREVIEW_Y + COLOR_PREVIEW_H,
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
                hs.iter().any(|h| &h.action == a && h.y >= y && h.y + h.h <= visible_end)
            };
            for i in 0..4 {
                assert!(
                    inside(&hs, &crate::aux_marker(&format!("pref_tab:{i}"))),
                    "800×600 tab={tab}: 导航项 {i} 不在首屏"
                );
            }
            let anchors: Vec<Action> = match tab {
                0 => vec![
                    crate::aux_marker("pref_toggle:hint_show_code"),
                    crate::aux_marker("pref_toggle:hint_show_rank"),
                    crate::aux_marker("pref_step:hint_font_size:1"),
                ],
                1 => vec![
                    crate::aux_marker("pref_toggle:score_auto_copy"),
                    crate::aux_marker("pref_field:segment"),
                    crate::aux_marker("pref_save_template"),
                ],
                2 => vec![
                    crate::aux_marker(&format!("pref_color_select:{}", COLORS[0].0)),
                    crate::aux_marker(&format!("pref_color_select:{}", COLORS[2].0)),
                ],
                _ => vec![Action::Folder],
            };
            for a in anchors {
                assert!(
                    inside(&hs, &a),
                    "800×600 tab={tab}: 主控件 {a:?} 不在首屏"
                );
            }
            let _ = (x, pw);
        }
    }

    #[test]
    fn score_field_chips_are_four_columns_and_clear_of_the_template_section() {
        let rows = SCORE_FIELDS.len().div_ceil(SCORE_CHIP_COLUMNS);
        let last = SCORE_CHIPS_Y + (rows as f32 - 1.) * SCORE_CHIP_PITCH + SCORE_CHIP_H;
        assert!(
            last <= SCORE_G3_TITLE_Y,
            "字段 chips 压到模板区：{last} > {}",
            SCORE_G3_TITLE_Y
        );
        let (x0, y0, cw) = score_chip_rect(0., 0., 704., 0);
        let (x1, y1, _) = score_chip_rect(0., 0., 704., 1);
        assert_eq!(y0, y1);
        assert!((x1 - (x0 + cw + 8.)).abs() < 0.01);
        assert!((cw * 4. + 3. * 8. - 704.).abs() < 0.01);
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
        for key in ["hint_show_code", "hint_show_rank"] {
            assert!(hs
                .iter()
                .any(|h| h.action == crate::aux_marker(&format!("pref_toggle:{key}"))));
        }
        for delta in ["-1", "1"] {
            assert!(hs
                .iter()
                .any(|h| h.action == crate::aux_marker(&format!("pref_step:hint_font_size:{delta}"))));
        }
        // 成绩单：两行开关、全部字段 chip、5 个预设、保存。
        let hs = hit_targets(&preference_model(1), 1280., 800.);
        for key in ["score_auto_copy", "auto_send_score"] {
            assert!(hs
                .iter()
                .any(|h| h.action == crate::aux_marker(&format!("pref_toggle:{key}"))));
        }
        for (key, _) in SCORE_FIELDS {
            let action = crate::aux_marker(&format!("pref_field:{key}"));
            assert_eq!(
                hs.iter().filter(|h| h.action == action).count(),
                1,
                "字段 {key} 的 chip 缺失或重复"
            );
        }
        for id in ["standard", "compact", "classic", "detailed", "custom"] {
            assert!(hs
                .iter()
                .any(|h| h.action == crate::aux_marker(&format!("pref_score_preset:{id}"))));
        }
        assert!(hs
            .iter()
            .any(|h| h.action == crate::aux_marker("pref_save_template")));
        // 颜色页：8 个颜色行 + 应用。
        let hs = hit_targets(&preference_model(2), 1280., 800.);
        for (key, _, _) in COLORS {
            assert!(hs
                .iter()
                .any(|h| h.action == crate::aux_marker(&format!("pref_color_select:{key}"))));
        }
        assert_eq!(
            hs.iter()
                .filter(|h| h.action == crate::aux_marker("pref_apply_colors"))
                .count(),
            1
        );
        // 数据页：打开目录。
        let hs = hit_targets(&preference_model(3), 1280., 800.);
        assert!(hs.iter().any(|h| h.action == Action::Folder));
    }

    #[test]
    fn preference_hit_targets_do_not_overlap_at_supported_sizes() {
        for tab in 0..=3u8 {
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
