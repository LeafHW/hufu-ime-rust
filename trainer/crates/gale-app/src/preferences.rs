//! Detailed trainer preferences. The application owns persistence and validation;
//! this view supplies native editable fields and explicit, named actions.
use super::{
    body_text, btn, button_label, hit_box, mono, round, small_btn, text, text_block, Action,
    FieldSpec, Hit, Model,
};
use crate::ui::{alpha, line, rect, tokens};
use gale_core::scorecard::{format_score_template, SCORE_FIELDS, SCORE_PRESETS};
use gale_core::theme::Theme;
use gale_win::d2d::{Align, DrawCmd, FontFamily, Frame};

const COLORS: &[(&str, &str, &str)] = &[
    ("词提简码色", "简码词", "63c08d"),
    ("词提二字全码色", "二字全码", "db9b55"),
    ("词提三字全码色", "三字全码", "70a9e5"),
    ("词提四字全码色", "四字以上全码", "df7974"),
    ("跟打区背景色", "正文背景", "111315"),
    ("打对字色", "打对文字", "e7e4db"),
    ("对打打对底色", "打对底色", "173c2b"),
    ("打错字色", "打错文字", "e36b65"),
    ("对打打错底色", "打错底色", "4b2021"),
    ("词提编码色", "单字编码", "d0c9b9"),
    ("词提角标色", "选重数字", "d0c9b9"),
    ("词提下划线色", "词下划线", "c9a766"),
    ("词提底色", "词底色", "222a27"),
];
fn field_label(label: &str) -> &str {
    label.split('（').next().unwrap_or(label)
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
fn tab(m: &Model) -> u8 {
    m.field("pref_tab").parse::<u8>().unwrap_or(0).min(2)
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
fn color_columns(pw: f32) -> usize {
    if pw >= 800. {
        4
    } else {
        3
    }
}
fn color_edit_y(pw: f32) -> f32 {
    let rows = COLORS.len().div_ceil(color_columns(pw)) as f32;
    184. + (rows - 1.) * 43. + 35. + 30.
}
fn field_chip(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    s: &str,
    selected: bool,
    action: Action,
    t: &Theme,
) {
    let p = tokens(t);
    round(
        f,
        x,
        y,
        w,
        30.,
        6.,
        if selected {
            alpha(p.accent, 0.10)
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
        y + 5.,
        w - 16.,
        s,
        if selected { p.accent } else { p.muted },
    );
    hs.push(Hit {
        x,
        y,
        w,
        h: 30.,
        action,
    });
}
fn score_columns(pw: f32) -> usize {
    if pw >= 800. {
        5
    } else {
        4
    }
}
/// Template editor sits below the output-field grid (the common task), so the
/// grid stays visible at 800x600 instead of starting under the fixed footer.
fn score_editor_y(pw: f32) -> f32 {
    let rows = SCORE_FIELDS.len().div_ceil(score_columns(pw)) as f32;
    let grid_end = 290. + (rows - 1.) * 34. + 30.;
    grid_end + 20. + 28. + 40. + 20.
}
fn centered_text(f: &mut Frame, cx: f32, y: f32, size: f32, s: &str, c: [f32; 4]) {
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
/// A live sample of the hint rules. The old hint page ended with a short
/// paragraph and roughly 120 DIP of dead space, so changing a toggle gave no
/// visible feedback until the next practice.
fn hint_preview(f: &mut Frame, m: &Model, t: &Theme, x: f32, y: f32, pw: f32) {
    let p = tokens(t);
    round(f, x + 24., y, pw - 48., 102., 7., p.surface2, Some(p.line));
    text(f, x + 38., y + 12., 14., "实时预览", p.text);
    text(
        f,
        x + 112.,
        y + 12.,
        14.,
        "按当前开关与颜色绘制，改完立即生效",
        p.muted,
    );
    let show_code = enabled(m, "hint_show_code", true);
    let show_rank = enabled(m, "hint_show_rank", true);
    let color_words = enabled(m, "hint_color_words", true);
    let underline = enabled(m, "hint_underline", false);
    let word_bg = enabled(m, "hint_word_bg", false);
    let hint_size = m
        .field("hint_font_size")
        .parse::<f32>()
        .unwrap_or(14.)
        .clamp(14., 28.)
        .min(18.);
    let bg = parse_color(&color_hex(m, "词提底色", "222a27")).unwrap_or(p.hint_bg);
    let cell = ((pw - 96.) / 4.).max(80.);
    let samples = [
        ("简码词", "abc", "词提简码色", "63c08d"),
        ("二字", "abcd", "词提二字全码色", "db9b55"),
        ("三个字", "abcd", "词提三字全码色", "70a9e5"),
        ("四字以上", "abcd", "词提四字以上全码色", "df7974"),
    ];
    for (i, (word, code, key, fallback)) in samples.iter().enumerate() {
        let sx = x + 40. + i as f32 * cell;
        let color = if color_words {
            parse_color(&color_hex(m, key, fallback)).unwrap_or(p.text2)
        } else {
            p.text2
        };
        let wy = y + 38.;
        if word_bg {
            round(
                f,
                sx - 8.,
                wy - 6.,
                92.,
                42.,
                6.,
                bg,
                Some(alpha(p.hint_underline, 0.25)),
            );
        }
        body_text(f, sx, wy, 20., *word, color);
        if underline {
            line(f, sx, wy + 25., 72., alpha(color, 0.35));
        }
        if show_code {
            mono(f, sx, y + 64., hint_size, *code, color);
        }
        if show_rank {
            let code_w = code.chars().count() as f32 * hint_size * 0.6;
            mono(f, sx + code_w + 2., y + 60., hint_size, "2", color);
        }
    }
}
fn row(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    t: &Theme,
    x: f32,
    y: f32,
    w: f32,
    key: &str,
    title: &str,
    desc: &str,
    default: bool,
) {
    let p = tokens(t);
    round(f, x, y, w, 56., 7., p.surface2, Some(p.line));
    button_label(f, x + 14., y + 7., w - 110., title, p.text);
    button_label(f, x + 14., y + 29., w - 110., desc, p.muted);
    let on = enabled(m, key, default);
    small_btn(
        f,
        hs,
        x + w - 76.,
        y + 11.,
        60.,
        if on { "开启" } else { "关闭" },
        crate::aux_marker(&format!("pref_toggle:{key}")),
        t,
        on,
    );
}
pub(super) fn fields(m: &Model, x: f32, y: f32, pw: f32, _ph: f32) -> Vec<FieldSpec> {
    match tab(m) {
        1 => vec![FieldSpec {
            id: "score_template",
            x: x + 24.,
            y: y + score_editor_y(pw) + 10.,
            w: pw - 48.,
            h: 76.,
            multi: true,
            password: false,
        }],
        2 => vec![FieldSpec {
            id: "pref_color_hex",
            x: x + 24.,
            y: y + color_edit_y(pw) + 26.,
            w: (pw - 320.).max(120.),
            h: 38.,
            multi: false,
            password: false,
        }],
        _ => Vec::new(),
    }
}
/// Height of the currently selected preference page, used to decide whether a
/// scrollbar is meaningful. Without it every tab showed a scrollbar even when
/// the hints page fit on screen.
pub(super) fn content_height(m: &Model, pw: f32) -> f32 {
    match tab(m) {
        0 => 690.,
        1 => 930.,
        _ => color_edit_y(pw) + 106. + 168. + 16.,
    }
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
    let p = tokens(t);
    // Compact tab strip: three full-width cards made the dialog look like a
    // toolbar and wasted a whole row. The strip stays inside the sticky band
    // (see modals::fit_panel) so scrolling never reveals a half-cut tab.
    let tab_w = 96.;
    let tab_gap = 6.;
    for (i, title) in ["词提", "成绩单", "颜色"].iter().enumerate() {
        let tx = x + 24. + i as f32 * (tab_w + tab_gap);
        let on = tab(m) == i as u8;
        round(
            f,
            tx,
            y + 77.,
            tab_w,
            34.,
            7.,
            if on { alpha(p.accent, 0.10) } else { p.bg2 },
            Some(if on { alpha(p.accent, 0.55) } else { p.line }),
        );
        if on {
            rect(f, tx + 9., y + 108., tab_w - 18., 2., p.accent);
        }
        centered_text(
            f,
            tx + tab_w / 2.,
            y + 84.,
            15.,
            title,
            if on { p.accent } else { p.muted },
        );
        hs.push(Hit {
            x: tx,
            y: y + 77.,
            w: tab_w,
            h: 34.,
            action: crate::aux_marker(&format!("pref_tab:{i}")),
        });
    }
    match tab(m) {
        0 => {
            text(f, x + 24., y + 125., 16., "提示内容与显示方式", p.text);
            let rows = [
                (
                    "hint_show_code",
                    "显示编码",
                    "在词下方显示当前词库编码",
                    true,
                ),
                (
                    "hint_show_rank",
                    "显示选重数字",
                    "使用候选位次数字，单字保持中性色",
                    true,
                ),
                (
                    "hint_color_words",
                    "多字词着色",
                    "简码绿 / 二字全码橙 / 三字蓝 / 四字以上红",
                    true,
                ),
                ("hint_underline", "词下划线", "显示词边界下划线", false),
                ("hint_word_bg", "词底色", "使用颜色页指定的词底色", false),
            ];
            for (i, (key, title, desc, default)) in rows.iter().enumerate() {
                row(
                    f,
                    hs,
                    m,
                    t,
                    x + 24.,
                    y + 154. + i as f32 * 58.,
                    pw - 48.,
                    key,
                    title,
                    desc,
                    *default,
                );
            }
            text(f, x + 24., y + 459., 15., "编码与选重提示字号", p.text);
            let size = m.field("hint_font_size");
            mono(
                f,
                x + 226.,
                y + 457.,
                16.,
                if size.is_empty() { "14" } else { &size },
                p.accent,
            );
            small_btn(
                f,
                hs,
                x + 298.,
                y + 452.,
                44.,
                "−",
                crate::aux_marker("pref_step:hint_font_size:-1"),
                t,
                false,
            );
            small_btn(
                f,
                hs,
                x + 350.,
                y + 452.,
                44.,
                "+",
                crate::aux_marker("pref_step:hint_font_size:1"),
                t,
                false,
            );
            text_block(f, x + 24., y + 496., pw - 48., 14., 22., 2,
                "词提只提供当前词库的静态编码；赛制规则优先。单字与标点不着词色，错字始终使用打错文字色。", p.muted);
            hint_preview(f, m, t, x, y + 550., pw);
        }
        1 => {
            row(
                f,
                hs,
                m,
                t,
                x + 24.,
                y + 120.,
                pw - 48.,
                "score_auto_copy",
                "打完自动复制成绩",
                "完成当前段时复制成绩文本",
                true,
            );
            row(
                f,
                hs,
                m,
                t,
                x + 24.,
                y + 184.,
                pw - 48.,
                "auto_send_score",
                "打完自动发成绩到群",
                "绑定 QQ 群后，完成一段即自动发送晴兼容成绩单",
                true,
            );
            let editor_y = y + score_editor_y(pw) + 10.;
            let template_y = editor_y - 98.;
            let presets_y = editor_y - 60.;
            let mode_y = editor_y - 20.;
            let save_y = template_y;
            text(
                f,
                x + 24.,
                y + 255.,
                14.,
                "勾选输出字段 · 点击即切换为字段模式",
                p.text,
            );
            text(f, x + 24., template_y, 16., "成绩单模板", p.text);
            for (i, (id, label)) in [
                ("standard", "标准"),
                ("compact", "简洁"),
                ("classic", SCORE_PRESETS[0].name),
                ("detailed", SCORE_PRESETS[1].name),
                ("custom", "自由编辑"),
            ]
            .iter()
            .enumerate()
            {
                small_btn(
                    f,
                    hs,
                    x + 24. + i as f32 * ((pw - 80.) / 5. + 8.),
                    presets_y,
                    (pw - 80.) / 5.,
                    label,
                    crate::aux_marker(&format!("pref_score_preset:{id}")),
                    t,
                    m.field("score_preset") == *id,
                );
            }
            let preset = m.field("score_preset");
            let fields_mode = m.field("score_template").trim().is_empty();
            let mode = if fields_mode {
                if preset.is_empty() {
                    "字段模式（自定义勾选）"
                } else {
                    "标准字段"
                }
            } else {
                match preset.as_str() {
                    "compact" => "简洁模板",
                    "classic" => "经典模板",
                    "detailed" => "完整模板",
                    _ => "自定义模板",
                }
            };
            text(
                f,
                x + 24.,
                mode_y,
                14.,
                format!("当前：{mode} · 自由模板优先；点击下方字段会切回字段模式。"),
                p.muted,
            );
            round(f, x + 23., editor_y, pw - 46., 76., 5., p.bg, Some(p.line2));
            btn(
                f,
                hs,
                x + pw - 142.,
                save_y,
                118.,
                "保存模板",
                crate::aux_marker("pref_save_template"),
                t,
                true,
            );
            let active = m.field("score_fields");
            let columns = if pw >= 800. { 5 } else { 4 };
            let cw = (pw - 48. - (columns - 1) as f32 * 7.) / columns as f32;
            for (i, (key, title)) in SCORE_FIELDS.iter().enumerate() {
                let on = active
                    .split(',')
                    .any(|item| canonical_field(item.trim()) == *key);
                field_chip(
                    f,
                    hs,
                    x + 24. + (i % columns) as f32 * (cw + 7.),
                    y + 290. + (i / columns) as f32 * 34.,
                    cw,
                    &format!("{} {}", if on { "✓" } else { "□" }, field_label(title)),
                    on,
                    crate::aux_marker(&format!("pref_field:{key}")),
                    t,
                );
            }
            let legend_y = editor_y + 86.;
            text_block(f, x + 24., legend_y, pw - 48., 14., 22., 3,
                "占位符：{speed}={速度}，{character_accuracy}={字准}，{key_accuracy}={键准}，{strokes}={击键}。\n字准/键准需自行加 %；{word_rate} 已含 %；{{ 与 }} 输出花括号。", p.muted);
            let preview_y = legend_y + 76.;
            round(
                f,
                x + 24.,
                preview_y,
                pw - 48.,
                86.,
                7.,
                p.surface2,
                Some(p.line),
            );
            text(
                f,
                x + 38.,
                preview_y + 11.,
                14.,
                "自由模板预览 · 最近一次成绩",
                p.text,
            );
            let template = m.field("score_template");
            let preview = if template.trim().is_empty() {
                "当前为字段模式；填写自由模板后可在此预览。".to_string()
            } else if let Some(record) = &m.latest {
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
            } else {
                "完成一段跟打后，这里会按模板展示实际成绩。".into()
            };
            text_block(
                f,
                x + 38.,
                preview_y + 37.,
                pw - 76.,
                14.,
                22.,
                2,
                &preview,
                p.text2,
            );
        }
        _ => {
            text(f, x + 24., y + 125., 16., "任意颜色 · RRGGBB", p.text);
            text(
                f,
                x + 24.,
                y + 151.,
                14.,
                "选一项后输入六位十六进制颜色；默认语义：简码绿 / 二字全码橙 / 三字全码蓝 / 四字以上红。",
                p.muted,
            );
            let columns = color_columns(pw);
            let cw = (pw - 48. - (columns - 1) as f32 * 8.) / columns as f32;
            for (i, (key, label, fallback)) in COLORS.iter().enumerate() {
                let cx = x + 24. + (i % columns) as f32 * (cw + 8.);
                let cy = y + 184. + (i / columns) as f32 * 43.;
                let selected = selected_color(m) == *key;
                round(
                    f,
                    cx,
                    cy,
                    cw,
                    35.,
                    6.,
                    if selected {
                        alpha(p.accent, 0.1)
                    } else {
                        p.surface2
                    },
                    Some(if selected { p.accent } else { p.line }),
                );
                let hex = color_hex(m, key, fallback);
                round(
                    f,
                    cx + 8.,
                    cy + 8.,
                    18.,
                    18.,
                    3.,
                    parse_color(&hex).unwrap_or(p.text2),
                    Some(p.line2),
                );
                button_label(f, cx + 34., cy + 7., cw - 42., label, p.text2);
                hit_box(
                    hs,
                    cx,
                    cy,
                    cw,
                    35.,
                    crate::aux_marker(&format!("pref_color_select:{key}")),
                );
            }
            let selected = selected_color(m);
            let edit_y = y + color_edit_y(pw);
            text(
                f,
                x + 24.,
                edit_y,
                15.,
                format!("正在编辑：{selected}"),
                p.text,
            );
            round(
                f,
                x + 23.,
                edit_y + 26.,
                (pw - 318.).max(122.),
                40.,
                5.,
                p.bg,
                Some(p.line2),
            );
            let input = m.field("pref_color_hex");
            round(
                f,
                x + pw - 273.,
                edit_y + 28.,
                60.,
                36.,
                5.,
                parse_color(&input).unwrap_or(p.surface2),
                Some(p.line2),
            );
            btn(
                f,
                hs,
                x + pw - 196.,
                edit_y + 30.,
                172.,
                "应用此颜色",
                crate::aux_marker("pref_apply_colors"),
                t,
                true,
            );
            text(
                f,
                x + 24.,
                edit_y + 78.,
                14.,
                if parse_color(&input).is_some() {
                    "格式有效 · 应用后即时生效并保存"
                } else {
                    "请输入六位颜色值，例如 63C08D；也可带 # 前缀。"
                },
                p.muted,
            );
            let preview_y = edit_y + 106.;
            round(
                f,
                x + 24.,
                preview_y,
                pw - 48.,
                168.,
                7.,
                p.bg,
                Some(p.line),
            );
            let max_code = if m.max_code_length == 0 {
                4
            } else {
                m.max_code_length
            };
            let full_code = "abcd"
                .chars()
                .cycle()
                .take(max_code.min(16))
                .collect::<String>();
            let short_code = "abc"
                .chars()
                .cycle()
                .take(max_code.saturating_sub(1).clamp(1, 15))
                .collect::<String>();
            let sample_width = (pw - 80.) / 5.;
            for (i, (sample, code)) in [
                ("字", "a"),
                ("简码词", short_code.as_str()),
                ("二字", full_code.as_str()),
                ("三个字", full_code.as_str()),
                ("四字以上", full_code.as_str()),
            ]
            .iter()
            .enumerate()
            {
                let sx = x + 40. + i as f32 * sample_width;
                let c = crate::ui::hint_word_tint(&p, sample, code, max_code).unwrap_or(p.text);
                body_text(f, sx, preview_y + 34., 22., *sample, c);
                mono(
                    f,
                    sx,
                    preview_y + 70.,
                    14.,
                    *code,
                    if i == 0 { p.hint_code } else { c },
                );
            }
            text_block(f, x + 40., preview_y + 106., pw - 80., 14., 23., 2,
                &format!("当前方案最大码长为 {max_code}：编码不足 {max_code} 码的多字词用简码色；满码词按二字、三字、四字以上分色。单字与标点使用正文色。"), p.muted);
        }
    }
    line(f, x, y + ph - 62., pw, p.line);
    text(
        f,
        x + 24.,
        y + ph - 38.,
        14.,
        "设置独立保存 · 不更改输入法配色 · 滚轮查看更多",
        p.muted,
    );
    btn(
        f,
        hs,
        x + pw - 108.,
        y + ph - 47.,
        84.,
        "完成",
        Action::Close,
        t,
        true,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_color_input_accepts_every_rgb_value_and_rejects_bad_shapes() {
        assert_eq!(
            parse_color("#123aef"),
            Some([18. / 255., 58. / 255., 239. / 255., 1.])
        );
        assert!(parse_color("12xyef").is_none());
        assert!(parse_color("abcd").is_none());
    }
    #[test]
    fn native_fields_are_bound_to_the_selected_preference_page() {
        let mut m = Model::default();
        m.fields.insert("pref_tab".into(), "1".into());
        assert_eq!(fields(&m, 0., 0., 960., 740.)[0].id, "score_template");
        m.fields.insert("pref_tab".into(), "2".into());
        assert_eq!(fields(&m, 0., 0., 960., 740.)[0].id, "pref_color_hex");
    }
}
