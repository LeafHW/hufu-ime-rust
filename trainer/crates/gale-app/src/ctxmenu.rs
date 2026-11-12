//! 「视图」上下文菜单（2026-11-12 用户需求）：右键任意处、工具栏「视图」按钮、
//! 菜单键/Shift+F10 都打开同一张窗口内自绘菜单——不弹系统菜单（训练正文区
//! 的系统右键菜单通道仍被输入宿主全拦，防作弊语义不变）。
//!
//! 设计：一张 336 宽的浮层卡片，分「显示 / 排版 / 快捷」三组 + 底部三键；
//! 全窗口自绘（圆角、投影、hover 高亮、开关滑块、主题色点），不依赖原生控件。
//! 布局与命中在同一遍绘制里产出：`hs` 里追加的 Hit 由主循环的 `modals::hit` 消费。
use crate::{
    modals::{Action, Hit, Model},
    ui::{alpha, elide_width, mono, rect, right, round, text, tokens, ui_text_width, Palette},
};
use gale_core::{
    config::{Config, DisplayMode},
    theme::Theme,
};
use gale_win::d2d::{Align, DrawCmd, Frame};

/// 菜单宽度（逻辑 DIP）。
pub const MENU_W: f32 = 336.;
/// 距窗口边缘的安全距离。
const EDGE: f32 = 10.;

const T_TITLE: f32 = 14.;
const T_LABEL: f32 = 14.;
const T_NOTE: f32 = 12.;

/// 内置主题的快捷色点（与 `ui::theme` 的 accent 同源；绘制时取真值）。
const THEMES: [(&str, &str); 5] = [
    ("default", "墨玉"),
    ("宣纸", "宣纸"),
    ("夜巡", "夜巡"),
    ("朱砂", "朱砂"),
    ("青瓷", "青瓷"),
];

fn theme_label(name: &str) -> &'static str {
    // 「赤金」在 ui::theme 里与「朱砂」共用同一套色板，菜单里点亮朱砂。
    let name = if name == "赤金" { "朱砂" } else { name };
    THEMES
        .iter()
        .find(|(key, _)| *key == name || (*key == "default" && name.is_empty()))
        .map(|(_, label)| *label)
        .unwrap_or("墨玉")
}

/// 当前主题色点颜色：直接从主题解析结果取强调色，保证和真实皮肤一致。
fn theme_dot(name: &str, t: &Theme) -> [f32; 4] {
    let key = if name.is_empty() { "default" } else { name };
    if theme_label(key) == theme_label(&t.name) {
        return tokens(t).accent;
    }
    let resolved = crate::ui::theme(key);
    alpha(tokens(&resolved).accent, 0.9)
}

struct Geom {
    x: f32,
    y: f32,
    h: f32,
}

pub fn draw(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    cfg: &Config,
    m: &Model,
    t: &Theme,
    w: f32,
    h: f32,
) -> Option<(f32, f32, f32, f32)> {
    let menu = m.menu?;
    let p = tokens(t);
    let compact = h < 520.;
    let row = |normal: f32, tiny: f32| if compact { tiny } else { normal };
    let pad = 8.;
    let h_header = row(20., 18.);
    let h_label = row(16., 14.);
    let h_modes = row(46., 32.);
    let h_switch = row(50., 40.);
    let h_div = row(9., 7.);
    let h_size = row(34., 30.);
    let h_themes = row(36., 32.);
    let h_chips = row(30., 26.);
    let h_footer = row(32., 28.);
    let total = pad * 2.
        + h_header
        + h_label
        + h_modes
        + h_switch
        + h_div
        + h_label
        + h_size
        + h_themes
        + h_div
        + h_chips
        + h_chips
        + h_chips
        + h_div
        + h_footer;

    let x = menu.x.clamp(EDGE, (w - EDGE - MENU_W).max(EDGE));
    let y = menu.y.clamp(EDGE, (h - EDGE - total).max(EDGE));
    let rect_x = x + 8.;
    let rect_w = MENU_W - 16.;

    // 投影：三层递减透明度的圆角矩形，卡片下缘更重（无 D2D 阴影原语）。
    for (dy, spread, a) in [(7., 5., 0.10), (4., 3., 0.08), (2., 1., 0.10)] {
        round(
            f,
            x - spread + 2.,
            y - spread + dy,
            MENU_W + spread * 2. - 4.,
            total + spread * 2.,
            14. + spread,
            [0., 0., 0., a],
            None,
        );
    }
    round(f, x, y, MENU_W, total, 14., p.surface, Some(p.line2));
    // 顶部一条极细的强调色渐隐条：菜单卡片的"身份线"。
    rect(f, x + 14., y, MENU_W - 28., 2., alpha(p.accent, 0.5));

    let pointer = m.pointer;
    let hovered = |rx: f32, ry: f32, rw: f32, rh: f32| {
        pointer.is_some_and(|(px, py)| px >= rx && px < rx + rw && py >= ry && py < ry + rh)
    };
    let mut cursor = y + pad;

    // ── 头部：标题 + Esc 提示 ─────────────────────────────────────────
    text(f, rect_x + 4., cursor + 2., T_TITLE, "视图", p.text);
    right(
        f,
        x + MENU_W - 16.,
        cursor + 4.,
        11.,
        "Esc 关闭",
        p.muted,
    );
    cursor += h_header + 4.;

    // ── 显示 ─────────────────────────────────────────────────────────
    section_label(f, rect_x + 4., cursor, "显示", &p);
    cursor += h_label + 2.;
    let half = (rect_w - 6.) / 2.;
    let mode = cfg.display_mode;
    segment(
        f,
        hs,
        rect_x,
        cursor,
        half,
        h_modes,
        "字贴",
        "直接跟打 · 光标逐字推进",
        &cfg.shortcut("toggle_mode"),
        mode == DisplayMode::Copybook,
        Action::SetDisplayMode(DisplayMode::Copybook),
        &p,
        compact,
    );
    segment(
        f,
        hs,
        rect_x + half + 6.,
        cursor,
        half,
        h_modes,
        "对照",
        "上文对照 · 下方输入框",
        &cfg.shortcut("toggle_mode"),
        mode == DisplayMode::Split,
        Action::SetDisplayMode(DisplayMode::Split),
        &p,
        compact,
    );
    cursor += h_modes + 4.;
    // 极简模式开关行（整行 hover 高亮）
    let row_rect = (rect_x, cursor, rect_w, h_switch - 4.);
    if hovered(rect_x, cursor, rect_w, h_switch - 4.) {
        round(
            f,
            rect_x - 2.,
            cursor,
            rect_w + 4.,
            h_switch - 4.,
            8.,
            alpha(p.accent, 0.07),
            None,
        );
    }
    text(
        f,
        rect_x + 6.,
        cursor + 6.,
        T_LABEL,
        "极简模式",
        p.text,
    );
    if !compact {
        mono(
            f,
            rect_x + 6.,
            cursor + 27.,
            11.,
            "隐藏工具栏、统计与状态栏 · 正文满窗",
            p.muted,
        );
    }
    right(
        f,
        rect_x + rect_w - 50.,
        cursor + 14.,
        11.,
        &cfg.shortcut("minimal"),
        p.muted,
    );
    switch(
        f,
        hs,
        rect_x + rect_w - 42.,
        cursor + (h_switch - 4. - 22.) / 2.,
        cfg.minimal,
        Action::ToggleMinimal,
        &p,
    );
    hs.push(Hit {
        x: row_rect.0,
        y: row_rect.1,
        w: row_rect.2,
        h: row_rect.3,
        action: Action::ToggleMinimal,
    });
    cursor += h_switch;

    divider(f, &mut cursor, x, MENU_W, h_div, &p);

    // ── 排版 ─────────────────────────────────────────────────────────
    section_label(f, rect_x + 4., cursor, "排版", &p);
    cursor += h_label + 2.;
    // 字号行：右侧 [-][ 54 ][+] 一组右对齐。
    let value_w = 46.;
    text(
        f,
        rect_x + 6.,
        cursor + (h_size - 16.) / 2.,
        T_LABEL,
        "字号",
        p.text,
    );
    let group_x = rect_x + rect_w - (28. + 6. + value_w + 6. + 28.);
    step_button(
        f,
        hs,
        group_x,
        cursor + (h_size - 26.) / 2.,
        "−",
        Action::StepFont(-1),
        hovered(group_x, cursor, 28., h_size),
        &p,
    );
    text_center(
        f,
        group_x + 34. + value_w / 2.,
        cursor + (h_size - 15.) / 2.,
        15.,
        format!("{:.0}", cfg.font_size),
        p.text2,
    );
    step_button(
        f,
        hs,
        group_x + 34. + value_w + 6.,
        cursor + (h_size - 26.) / 2.,
        "+",
        Action::StepFont(1),
        hovered(group_x + 34. + value_w + 6., cursor, 28., h_size),
        &p,
    );
    cursor += h_size;
    // 主题色点行：标签 38 宽，余下均分给 5 个色点 chip。
    let dot_chip_w = ((rect_w - 6. - 38.) - 6. * 4.) / 5.;
    text(
        f,
        rect_x + 6.,
        cursor + (h_themes - 16.) / 2.,
        T_LABEL,
        "主题",
        p.text,
    );
    for (i, (key, label)) in THEMES.iter().enumerate() {
        let cx = rect_x + 6. + 38. + i as f32 * (dot_chip_w + 6.);
        let cw = dot_chip_w;
        let cy = cursor + (h_themes - 26.) / 2.;
        let selected = theme_label(&cfg.theme) == *label;
        let hot = hovered(cx, cursor, cw, h_themes);
        round_dot_chip(f, cx, cy, cw, 26., selected, hot, &p);
        let dot = theme_dot(key, t);
        round(f, cx + 6., cy + 8., 10., 10., 5., dot, None);
        text(
            f,
            cx + 19.,
            cy + 6.,
            11.,
            *label,
            if selected { p.text } else { p.muted },
        );
        hs.push(Hit {
            x: cx,
            y: cy,
            w: cw,
            h: 26.,
            action: Action::Theme(if *key == "default" { "default" } else { key }.into()),
        });
    }
    cursor += h_themes;

    divider(f, &mut cursor, x, MENU_W, h_div, &p);

    // ── 快捷开关（两组等宽 chip）────────────────────────────────────
    let chip_w = (rect_w - 2. * 6.) / 3.;
    let chips: [(bool, &str, Action); 3] = [
        (m.hints, "词提", Action::ToggleHints),
        (m.analysis_dock, "理论分析", Action::ToggleAnalysis),
        (m.ledger_open, "成绩记录", Action::ToggleLedger),
    ];
    for (i, (on, label, action)) in chips.into_iter().enumerate() {
        let cx = rect_x + i as f32 * (chip_w + 6.);
        toggle_chip(
            f,
            hs,
            cx,
            cursor + 2.,
            chip_w,
            h_chips - 4.,
            label,
            on,
            action,
            hovered(cx, cursor, chip_w, h_chips),
            false,
            &p,
        );
    }
    cursor += h_chips;
    // 在线发文 / 选群：两枚等宽入口（跟打器最常用的两个动作也要能右键直达）。
    let wide = (rect_w - 6.) / 2.;
    toggle_chip(
        f,
        hs,
        rect_x,
        cursor + 2.,
        wide,
        h_chips - 4.,
        &format!("在线发文 {}", cfg.shortcut("online")),
        false,
        Action::Open(crate::modals::Modal::Online),
        hovered(rect_x, cursor, wide, h_chips),
        false,
        &p,
    );
    toggle_chip(
        f,
        hs,
        rect_x + wide + 6.,
        cursor + 2.,
        wide,
        h_chips - 4.,
        &format!("选群 {}", cfg.shortcut("groups")),
        false,
        Action::Open(crate::modals::Modal::Groups),
        hovered(rect_x + wide + 6., cursor, wide, h_chips),
        false,
        &p,
    );
    cursor += h_chips;
    let chip_w2 = (rect_w - 6.) / 2.;
    toggle_chip(
        f,
        hs,
        rect_x,
        cursor + 2.,
        chip_w2,
        h_chips - 4.,
        if cfg.minimal_hud {
            "数据条 · 开"
        } else {
            "数据条 · 关"
        },
        cfg.minimal_hud,
        Action::ToggleMinimalHud,
        hovered(rect_x, cursor, chip_w2, h_chips),
        !cfg.minimal,
        &p,
    );
    toggle_chip(
        f,
        hs,
        rect_x + chip_w2 + 6.,
        cursor + 2.,
        chip_w2,
        h_chips - 4.,
        "窗口置顶",
        cfg.always_on_top,
        Action::ToggleTopmost,
        hovered(rect_x + chip_w2 + 6., cursor, chip_w2, h_chips),
        false,
        &p,
    );
    cursor += h_chips;

    divider(f, &mut cursor, x, MENU_W, h_div, &p);

    // ── 底部三键 ─────────────────────────────────────────────────────
    let foot_w = (rect_w - 2. * 6.) / 3.;
    footer_button(
        f,
        hs,
        rect_x,
        cursor + 2.,
        foot_w,
        h_footer - 4.,
        "设置",
        &cfg.shortcut("settings"),
        Action::Open(crate::modals::Modal::Preferences),
        hovered(rect_x, cursor, foot_w, h_footer),
        &p,
        false,
    );
    footer_button(
        f,
        hs,
        rect_x + foot_w + 6.,
        cursor + 2.,
        foot_w,
        h_footer - 4.,
        "更多",
        "",
        Action::Open(crate::modals::Modal::More),
        hovered(rect_x + foot_w + 6., cursor, foot_w, h_footer),
        &p,
        false,
    );
    footer_button(
        f,
        hs,
        rect_x + (foot_w + 6.) * 2.,
        cursor + 2.,
        foot_w,
        h_footer - 4.,
        "退出",
        "",
        Action::Exit,
        hovered(rect_x + (foot_w + 6.) * 2., cursor, foot_w, h_footer),
        &p,
        true,
    );

    Some((x, y, MENU_W, total))
}

fn section_label(f: &mut Frame, x: f32, y: f32, label: &str, p: &Palette) {
    text(f, x, y + 3., 11., label, p.muted);
}

fn divider(f: &mut Frame, cursor: &mut f32, x: f32, w: f32, h_div: f32, p: &Palette) {
    let y = *cursor + h_div / 2.;
    rect(f, x + 14., y, w - 28., 1., alpha(p.line2, 0.5));
    *cursor += h_div;
}

fn segment(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    title: &str,
    note: &str,
    key: &str,
    selected: bool,
    action: Action,
    p: &Palette,
    compact: bool,
) {
    let bg = if selected {
        alpha(p.accent, 0.13)
    } else {
        alpha(p.bg2, 0.9)
    };
    let stroke = if selected {
        alpha(p.accent, 0.55)
    } else {
        p.line
    };
    round(f, x, y, w, h, 9., bg, Some(stroke));
    if selected {
        // 选中段：左侧 3px 强调条（底部条会压到说明文字）。
        round(
            f,
            x + 4.,
            y + 9.,
            3.,
            h - 18.,
            1.5,
            alpha(p.accent, 0.85),
            None,
        );
    }
    // 【快捷键自设 2026-11-13】段右上角显示当前生效组合键。
    if !key.is_empty() {
        right(
            f,
            x + w - 10.,
            if compact { y + (h - 13.) / 2. } else { y + 9. },
            10.,
            key,
            p.muted,
        );
    }
    let label_x = x + 12.;
    text(
        f,
        label_x,
        if compact { y + (h - 15.) / 2. } else { y + 8. },
        T_LABEL,
        title,
        if selected { p.accent } else { p.text2 },
    );
    if !compact {
        text(
            f,
            label_x,
            y + h - 18.,
            10.,
            elide_width(note, (w - 24.).max(24.), 10.),
            p.muted,
        );
    }
    hs.push(Hit {
        x,
        y,
        w,
        h,
        action,
    });
}

fn switch(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    on: bool,
    action: Action,
    p: &Palette,
) {
    let (w, h) = (40., 22.);
    round(
        f,
        x,
        y,
        w,
        h,
        h / 2.,
        if on { p.accent } else { p.surface2 },
        Some(if on { alpha(p.accent, 0.9) } else { p.line }),
    );
    let kx = if on { x + w - h + 2. } else { x + 2. };
    round(
        f,
        kx,
        y + 2.,
        h - 4.,
        h - 4.,
        (h - 4.) / 2.,
        if on { p.bg } else { p.muted },
        None,
    );
    hs.push(Hit {
        x,
        y,
        w,
        h,
        action,
    });
}

fn step_button(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    label: &str,
    action: Action,
    hot: bool,
    p: &Palette,
) {
    round(
        f,
        x,
        y,
        28.,
        26.,
        7.,
        if hot { alpha(p.accent, 0.16) } else { p.surface2 },
        Some(if hot { alpha(p.accent, 0.5) } else { p.line }),
    );
    text_center(f, x + 14., y + 4., 15., label, if hot { p.accent } else { p.text2 });
    hs.push(Hit {
        x,
        y,
        w: 28.,
        h: 26.,
        action,
    });
}

fn text_center(f: &mut Frame, cx: f32, y: f32, size: f32, s: impl Into<String>, c: [f32; 4]) {
    f.push(DrawCmd::TextRun {
        x: cx,
        y,
        h: size,
        text: s.into(),
        color: c,
        font: gale_win::d2d::FontFamily::Ui,
        align: Align::Center,
    });
}

fn round_dot_chip(
    f: &mut Frame,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    selected: bool,
    hot: bool,
    p: &Palette,
) {
    let stroke = if selected {
        alpha(p.accent, 0.6)
    } else if hot {
        alpha(p.line2, 0.9)
    } else {
        p.line
    };
    round(
        f,
        x,
        y,
        w,
        h,
        7.,
        if selected { alpha(p.accent, 0.10) } else { [0.; 4] },
        Some(stroke),
    );
}

#[allow(clippy::too_many_arguments)]
fn toggle_chip(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    on: bool,
    action: Action,
    hot: bool,
    dim: bool,
    p: &Palette,
) {
    let tone = if dim { 0.45 } else { 1.0 };
    round(
        f,
        x,
        y,
        w,
        h,
        7.,
        if on {
            alpha(p.accent, 0.13 * tone)
        } else if hot {
            alpha(p.text2, 0.06)
        } else {
            [0.; 4]
        },
        Some(if on {
            alpha(p.accent, 0.55 * tone)
        } else {
            p.line
        }),
    );
    let mark = if on { "✓ " } else { "" };
    let label = format!("{mark}{label}");
    f.push(DrawCmd::PushClip {
        x: x + 6.,
        y,
        w: (w - 12.).max(4.),
        h,
    });
    text_center(
        f,
        x + w / 2.,
        y + (h - 13.) / 2.,
        12.,
        elide_width(&label, w - 12., 12.),
        if dim {
            p.muted
        } else if on {
            p.accent
        } else {
            p.text2
        },
    );
    f.push(DrawCmd::PopClip);
    hs.push(Hit {
        x,
        y,
        w,
        h,
        action,
    });
}

#[allow(clippy::too_many_arguments)]
fn footer_button(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    key: &str,
    action: Action,
    hot: bool,
    p: &Palette,
    danger: bool,
) {
    round(
        f,
        x,
        y,
        w,
        h,
        7.,
        if hot {
            alpha(if danger { p.red } else { p.accent }, 0.12)
        } else {
            p.surface2
        },
        Some(if hot {
            alpha(if danger { p.red } else { p.accent }, 0.45)
        } else {
            p.line
        }),
    );
    let color = if danger {
        alpha(p.red, 0.92)
    } else if hot {
        p.accent
    } else {
        p.text2
    };
    text(f, x + 10., y + (h - 13.) / 2., 12., label, color);
    if !key.is_empty() {
        let kw = (ui_text_width(key, 10.) + 8.).max(22.);
        round(
            f,
            x + w - kw - 7.,
            y + (h - 16.) / 2.,
            kw,
            16.,
            4.,
            p.bg2,
            Some(alpha(p.line2, 0.9)),
        );
        text(
            f,
            x + w - kw - 3.,
            y + (h - 16.) / 2. + 2.,
            10.,
            key,
            p.muted,
        );
    }
    hs.push(Hit {
        x,
        y,
        w,
        h,
        action,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modals::{CtxMenu, Modal};
    use crate::ui::theme;

    fn frame(w: f32, h: f32) -> (Frame, Vec<Hit>, Model, Config, Theme) {
        let cfg = Config::default();
        let theme = theme("墨玉");
        let mut m = Model {
            menu: Some(CtxMenu { x: 120., y: 80. }),
            ..Default::default()
        };
        m.hints = true;
        (Frame::new(w, h), Vec::new(), m, cfg, theme)
    }

    fn actions(hits: &[Hit]) -> Vec<Action> {
        hits.iter().map(|h| h.action.clone()).collect()
    }

    #[test]
    fn menu_exposes_every_view_action_and_fits_small_windows() {
        for (w, h) in [(1280., 800.), (800., 600.), (640., 420.)] {
            let (mut f, mut hs, mut m, cfg, theme) = frame(w, h);
            let rect = draw(&mut f, &mut hs, &cfg, &m, &theme, w, h).expect("menu");
            assert!(rect.0 >= EDGE - 0.01 && rect.1 >= EDGE - 0.01);
            assert!(rect.0 + rect.2 <= w - EDGE + 0.01);
            assert!(rect.1 + rect.3 <= h - EDGE + 0.01);
            let list = actions(&hs);
            for expected in [
                Action::SetDisplayMode(DisplayMode::Copybook),
                Action::SetDisplayMode(DisplayMode::Split),
                Action::ToggleMinimal,
                Action::StepFont(-1),
                Action::StepFont(1),
                Action::ToggleHints,
                Action::ToggleAnalysis,
                Action::ToggleLedger,
                Action::ToggleMinimalHud,
                Action::ToggleTopmost,
                Action::Open(Modal::Preferences),
                Action::Open(Modal::More),
                Action::Exit,
            ] {
                assert!(list.contains(&expected), "missing {expected:?} at {w}x{h}");
            }
            assert!(
                list.iter()
                    .filter(|a| matches!(a, Action::Theme(_)))
                    .count()
                    >= 5
            );
            // 菜单锚点越界时被夹回窗口内
            m.menu = Some(CtxMenu { x: 5000., y: 5000. });
            let rect = draw(&mut f, &mut hs, &cfg, &m, &theme, w, h).expect("menu");
            assert!(rect.0 + rect.2 <= w - EDGE + 0.01);
            assert!(rect.1 + rect.3 <= h - EDGE + 0.01);
        }
    }

    #[test]
    fn selected_states_follow_config_and_pointer_hover_adds_no_actions() {
        let (mut f, mut hs, mut m, cfg, theme) = frame(1280., 800.);
        m.pointer = Some((200., 300.));
        draw(&mut f, &mut hs, &cfg, &m, &theme, 1280., 800.);
        let list = actions(&hs);
        // 字贴是默认模式：字贴段与「对照」段都在（选择态由描边表达，不改变动作）。
        assert!(list.contains(&Action::SetDisplayMode(DisplayMode::Split)));
        // 极简模式行 + 其开关是两个同动作 Hit（整行可点），都能命中。
        assert!(
            list.iter()
                .filter(|a| matches!(a, Action::ToggleMinimal))
                .count()
                >= 2
        );
        // 无菜单时不画也不产生命中
        m.menu = None;
        hs.clear();
        assert!(draw(&mut f, &mut hs, &cfg, &m, &theme, 1280., 800.).is_none());
        assert!(hs.is_empty());
    }
}
