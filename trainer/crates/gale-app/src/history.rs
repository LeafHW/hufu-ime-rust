//! 底栏成绩账本：常驻一条摘要，点开在原窗口内联展开，不再有独立成绩页。
//!
//! 面板向上展开覆盖正文下缘，行 = 一条历史成绩；点行内联手风琴展开该条详情，
//! 详情里的「复制成绩 / 按键热力图」只对最近一次结算段可用（历史记录不保存按键分布）。
use crate::{
    modals::{Action, Hit, Modal, Model},
    session::{Phase, Session},
    ui::*,
};
use gale_core::{config::Config, theme::Theme};
use gale_win::d2d::{DrawCmd, Frame};

/// 面板高度：窗口的 46%，夹在 260–360。
pub fn panel_height(h: f32) -> f32 {
    (h * 0.46).clamp(260., 360.)
}
pub fn panel_rect(w: f32, h: f32) -> (f32, f32, f32, f32) {
    let ph = panel_height(h);
    let bottom = ledger_bar_y(h) - 8.;
    (24., bottom - ph, (w - 48.).max(320.), ph)
}
/// 内容少时面板收短，避免大片空白；内容多时到上限后滚动。
pub fn panel_rect_for(m: &Model, w: f32, h: f32) -> (f32, f32, f32, f32) {
    let content: f32 = (0..m.history.len()).map(|i| block_h(m, i)).sum();
    let ph = (56. + content + 12.).clamp(200., panel_height(h));
    let bottom = ledger_bar_y(h) - 8.;
    (24., bottom - ph, (w - 48.).max(320.), ph)
}
const ROW_H: f32 = 44.;
const DETAIL_H: f32 = 108.;
fn segment_label(r: &gale_core::score::ScoreRecord) -> String {
    // 记录里的 mark 是源文段号（可能几十万），对用户没有意义；
    // 这里始终显示本轮的第几段（para_index 是会话内段序）。
    format!("第 {} 段", r.para_index + 1)
}

fn fmt_speed(r: &gale_core::score::ScoreRecord) -> String {
    if r.speed.is_finite() && r.speed >= 0. && r.speed <= 1000. {
        format!("{:.1}", r.speed)
    } else {
        "—".into()
    }
}

fn fmt_accuracy(r: &gale_core::score::ScoreRecord) -> String {
    let value = r.character_accuracy();
    if value.is_finite() {
        format!("{:.1}%", value.clamp(0., 100.))
    } else {
        "—".into()
    }
}

fn block_h(m: &Model, idx: usize) -> f32 {
    ROW_H
        + if m.selected_history == Some(idx) {
            DETAIL_H
        } else {
            0.
        }
}
/// 滚动上限（DIP）：内容总高 - 列表可视高。
pub fn scroll_max(m: &Model, h: f32) -> f32 {
    let (_, _, _, ph) = panel_rect_for(m, w_for_scroll(), h);
    let view = (ph - 56. - 12.).max(1.);
    let content: f32 = (0..m.history.len()).map(|i| block_h(m, i)).sum();
    (content - view).max(0.)
}
/// 某条记录在列表内容里的 y 偏移（显示顺序：最新在上）。
fn w_for_scroll() -> f32 {
    1280.
}
pub fn record_offset(m: &Model, idx: usize) -> f32 {
    (idx + 1..m.history.len())
        .rev()
        .map(|i| block_h(m, i))
        .sum()
}

pub fn draw(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    sess: &Session,
    _cfg: &Config,
    m: &Model,
    t: &Theme,
    w: f32,
    h: f32,
) {
    if m.modal != Modal::None {
        return;
    }
    if m.ledger_open {
        draw_panel(f, hs, sess, m, t, w, h);
    }
    draw_bar(f, hs, sess, m, t, w, h);
}

fn draw_bar(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    sess: &Session,
    m: &Model,
    t: &Theme,
    w: f32,
    h: f32,
) {
    let p = tokens(t);
    let (x, y, bw) = (24., ledger_bar_y(h), (w - 48.).max(200.));
    if sess.phase == Phase::Done {
        let record = sess
            .results
            .get(sess.current_index())
            .and_then(Option::as_ref);
        let has_next = sess.current_index() + 1 < sess.total_segments();
        let summary = record
            .map(|r| format!("本段完成  {:.1} 字/分 · 键准 {:.1}%", r.speed, r.accuracy))
            .unwrap_or_else(|| "本段已完成".into());
        let controls_w = if has_next { 292. } else { 196. };
        text(
            f,
            x,
            y + 10.,
            14.,
            elide(
                &summary,
                ((bw - controls_w - 18.) / 14.).max(6.) as usize,
            ),
            p.text2,
        );
        let mut bx = w - 24. - controls_w;
        if has_next {
            button(f, hs, bx, y, 90., "下一段", Action::Next, t, true);
            bx += 96.;
        }
        button(f, hs, bx, y, 82., "重打", Action::Restart, t, !has_next);
        if let Some(idx) = m.history.len().checked_sub(1) {
            button(
                f,
                hs,
                bx + 88.,
                y,
                98.,
                "查看成绩",
                Action::SelectHistory(idx),
                t,
                false,
            );
        }
        return;
    }
    round(f, x, y, bw, LEDGER_H, 8., p.surface, Some(p.line));
    text(
        f,
        x + 14.,
        y + 9.,
        14.,
        format!("成绩记录 · {} 条", m.history.len()),
        p.text2,
    );
    if let Some(r) = m.history.last() {
        if bw >= 560. {
            right(
                f,
                x + bw - 34.,
                y + 10.,
                13.,
                format!(
                    "最近 {:.1} 字/分 · 字准 {:.1}%",
                    r.speed,
                    r.character_accuracy()
                ),
                p.muted,
            );
        }
    } else if bw >= 420. {
        right(
            f,
            x + bw - 34.,
            y + 10.,
            13.,
            "打完一段会自动记录",
            p.muted,
        );
    }
    right(
        f,
        x + bw - 14.,
        y + 6.,
        16.,
        if m.ledger_open { "⌄" } else { "›" },
        p.muted,
    );
    hs.push(Hit {
        x,
        y,
        w: bw,
        h: LEDGER_H,
        action: Action::ToggleLedger,
    });
}

fn draw_panel(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    sess: &Session,
    m: &Model,
    t: &Theme,
    w: f32,
    h: f32,
) {
    let p = tokens(t);
    let (x, y, pw, ph) = panel_rect_for(m, w, h);
    round(f, x, y, pw, ph, 12., p.surface, Some(p.line2));
    // header
    text(f, x + 20., y + 13., 16., "成绩记录", p.text);
    let done = sess.results.iter().flatten().count();
    let avg = if done == 0 {
        0.
    } else {
        sess.results
            .iter()
            .flatten()
            .map(|r| r.speed)
            .sum::<f64>()
            / done as f64
    };
    let summary = if done == 0 {
        "本轮还没有完成的段落".to_string()
    } else {
        format!("本轮 {done} 段 · 平均 {avg:.1} 字/分")
    };
    text(f, x + 96., y + 15., 13., summary, p.muted);
    text(f, x + pw - 36., y + 9., 20., "×", p.muted);
    hs.push(Hit {
        x: x + pw - 44.,
        y: y + 6.,
        w: 34.,
        h: 32.,
        action: Action::ToggleLedger,
    });
    line(f, x + 16., y + 44., pw - 32., p.line);
    let list_top = y + 52.;
    let list_bottom = y + ph - 10.;
    let view = (list_bottom - list_top).max(1.);
    if m.history.is_empty() {
        text(
            f,
            x + 20.,
            list_top + 24.,
            15.,
            "还没有成绩记录",
            p.text2,
        );
        text(
            f,
            x + 20.,
            list_top + 50.,
            13.,
            "完成一段跟打后，成绩会自动记在这里。",
            p.muted,
        );
        return;
    }
    let max = scroll_max(m, h);
    let scroll = m.ledger_scroll.clamp(0., max);
    let mut cy = 0.;
    let mut first_visible = true;
    for idx in (0..m.history.len()).rev() {
        let bh = block_h(m, idx);
        let top = cy - scroll;
        if top + bh > 0. && top < view {
            let ry = list_top + top;
            draw_row(f, hs, sess, m, t, x, ry, pw, idx, &mut first_visible, list_top, list_bottom);
        }
        cy += bh;
    }
    if max > 0. {
        let thumb = (view * view / (view + max)).clamp(20., view);
        rect(f, x + pw - 12., list_top, 5., view, alpha(p.text, 0.07));
        rect(
            f,
            x + pw - 12.,
            list_top + scroll / max * (view - thumb),
            5.,
            thumb,
            alpha(p.accent, 0.65),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    sess: &Session,
    m: &Model,
    t: &Theme,
    x: f32,
    ry: f32,
    pw: f32,
    idx: usize,
    first_visible: &mut bool,
    list_top: f32,
    list_bottom: f32,
) {
    let p = tokens(t);
    let Some(r) = m.history.get(idx) else { return };
    let expanded = m.selected_history == Some(idx);
    let selected = expanded;
    if selected {
        round(
            f,
            x + 12.,
            ry - 2.,
            pw - 24.,
            ROW_H - 2.,
            7.,
            alpha(p.accent, 0.07),
            None,
        );
    }
    let pair_w = (pw - 24.).max(240.);
    let row_text = format!(
        "{}  {}  ·  {}  ·  {} 字",
        if r.date.is_empty() { "记录" } else { &r.date },
        r.title,
        segment_label(r),
        r.chars
    );
    f.push(DrawCmd::PushClip {
        x: x + 22.,
        y: ry + 7.,
        w: pair_w - 236.,
        h: 20.,
    });
    text(
        f,
        x + 22.,
        ry + 8.,
        14.,
        elide_width(&row_text, pair_w - 240., 14.),
        p.text2,
    );
    f.push(DrawCmd::PopClip);
    mono(
        f,
        x + pw - 198.,
        ry + 8.,
        14.,
        fmt_speed(r),
        p.text2,
    );
    right(
        f,
        x + pw - 26.,
        ry + 9.,
        13.,
        format!("字准 {}", fmt_accuracy(r)),
        p.muted,
    );
    right(
        f,
        x + pw - 20.,
        ry + 8.,
        13.,
        if expanded { "⌃" } else { "⌄" },
        p.muted,
    );
    if *first_visible {
        *first_visible = false;
    }
    hs.push(Hit {
        x: x + 12.,
        y: ry,
        w: pw - 24.,
        h: ROW_H,
        action: Action::SelectHistory(idx),
    });
    if !expanded {
        line(f, x + 20., ry + ROW_H - 1., pw - 40., alpha(p.line, 0.55));
        return;
    }
    // 详情：两行指标 + 操作按钮。只在可视区内绘制，避免盖住 header/footer。
    let dy = ry + ROW_H;
    if dy >= list_top && dy < list_bottom {
        let mut fy = dy + 4.;
        let cols = [
            ("速度", format!("{:.1}", r.speed)),
            ("键准", format!("{:.1}%", r.accuracy)),
            ("字准", format!("{:.1}%", r.character_accuracy())),
            ("码长", format!("{:.2}", r.code_len)),
            ("用时", duration(r.time_ms)),
        ];
        for (i, (label, value)) in cols.iter().enumerate() {
            let cx = x + 28. + i as f32 * ((pw - 90.) / 5.);
            text(f, cx, fy, 12., *label, p.muted);
            mono(f, cx + 34., fy - 1., 14., value, p.text2);
        }
        fy += 24.;
        let secondary = format!(
            "总键 {}  ·  错字 {}  ·  退格 {}  ·  选重 {}  ·  重打 {}  ·  有效速度 {:.1}",
            r.keys,
            r.wrong,
            r.backspaces,
            r.select_count,
            r.retype_count,
            r.effective_speed
        );
        f.push(DrawCmd::PushClip {
            x: x + 28.,
            y: fy,
            w: pw - 56.,
            h: 20.,
        });
        text(f, x + 28., fy, 12., secondary, p.muted);
        f.push(DrawCmd::PopClip);
        let latest = idx + 1 == m.history.len();
        let mut bx = x + 28.;
        let by = fy + 24.;
        if by + 28. <= list_bottom {
            button(f, hs, bx, by, 92., "复制成绩", Action::CopyScore, t, true);
            bx += 100.;
            if latest && !sess.last_vk_histogram.is_empty() {
                button(
                    f,
                    hs,
                    bx,
                    by,
                    104.,
                    "按键热力图",
                    crate::aux_marker("show_heatmap"),
                    t,
                    false,
                );
            }
        }
    }
    line(f, x + 20., ry + ROW_H, pw - 40., alpha(p.line, 0.4));
}

fn duration(ms: u64) -> String {
    let secs = ms as f64 / 1000.;
    format!("{:02}:{:04.1}", (secs / 60.) as u64, secs % 60.)
}
