//! 底栏成绩账本：点「成绩记录」后为一张向下展开的分段成绩表，不再有独立成绩页。
//!
//! 结构：表头（点它收起）+ 9 列表格（用时/重打/段号/速度/击键/码长/键准/打词/字数）。
//! 表格占底部区域，正文区向上让位；数据直接取 ScoreRecord，不做额外加工。
use crate::{
    modals::{Action, Hit, Modal, Model},
    session::{Phase, Session},
    ui::*,
};
use gale_core::{config::Config, score::ScoreRecord, theme::Theme};
use gale_win::d2d::{Align, DrawCmd, FontFamily, Frame};

/// 收起时的摘要条高度。
pub const BAR_H: f32 = 36.;
/// 表格行高。
const ROW_H: f32 = 32.;
/// 表头高度（列名）。
const HEAD_H: f32 = 26.;
/// 面板底部留白。
const PAD_BOTTOM: f32 = 8.;

/// 面板底边距离窗口底的距离：常规=状态行上方；极简模式没有状态行，贴底 12px。
pub fn bottom_inset(cfg: &Config) -> f32 {
    if cfg.minimal {
        12.
    } else {
        STATUS_H + 8.
    }
}

fn visible_rows(cfg: &Config, m: &Model, h: f32) -> usize {
    let ph = if cfg.ledger_panel_h > 0. {
        cfg.ledger_panel_h
    } else {
        (h * 0.55).max(180.)
    };
    let body = (ph - BAR_H - HEAD_H - PAD_BOTTOM).max(ROW_H);
    (body / ROW_H).floor().clamp(1., 14.) as usize
}

/// 展开后的面板高度：用户拖过就用记忆高度，否则按成绩条数自动收短。
pub fn panel_height(cfg: &Config, m: &Model, h: f32) -> f32 {
    if cfg.ledger_panel_h > 0. {
        return cfg.ledger_panel_h.clamp(120., (h - 140.).max(160.));
    }
    let rows = m.history.len().min(visible_rows(cfg, m, h)).max(1) as f32;
    (BAR_H + HEAD_H + rows * ROW_H + PAD_BOTTOM).clamp(120., (h - 140.).max(200.))
}

/// 【2026-11-13】成绩面板/摘要条的左右边界：对照模式下与两张卡对齐
/// （非极简卡边 36px、极简 16px），字贴模式保持原 24px 内距。
fn horizontal_bounds(cfg: &Config, w: f32) -> (f32, f32) {
    if cfg.display_mode.is_split() {
        let bw = if cfg.minimal {
            (w - 56.).max(360.).min(1600.)
        } else {
            (w - 96.).max(360.).min(1600.)
        };
        (((w - bw) / 2.) - 12., bw + 24.)
    } else {
        (24., (w - 48.).max(320.))
    }
}

pub fn panel_rect_for(cfg: &Config, m: &Model, w: f32, h: f32) -> (f32, f32, f32, f32) {
    let ph = panel_height(cfg, m, h);
    let bottom = h - bottom_inset(cfg);
    let (x, pw) = horizontal_bounds(cfg, w);
    (x, bottom - ph, pw, ph)
}

/// 面板上沿 8px 作为拖动把手（拖高拖低），主循环据此进入拖动。
pub fn resize_grip_rect(
    cfg: &Config,
    m: &Model,
    w: f32,
    h: f32,
) -> Option<(f32, f32, f32, f32)> {
    if !m.ledger_open || m.modal != Modal::None {
        return None;
    }
    let (x, y, pw, _) = panel_rect_for(cfg, m, w, h);
    Some((x, y - 2., pw, 8.))
}

/// 收起状态下的摘要条（仅常规模式；极简模式不画底栏），横向与面板同界。
pub fn bar_rect(cfg: &Config, w: f32, h: f32) -> (f32, f32, f32, f32) {
    let bottom = h - STATUS_H - 8.;
    let (x, bw) = horizontal_bounds(cfg, w);
    (x, bottom - BAR_H, bw.max(200.), BAR_H)
}

pub fn scroll_max(cfg: &Config, m: &Model, h: f32) -> f32 {
    let rows = m.history.len() as f32;
    let visible = visible_rows(cfg, m, h) as f32;
    ((rows - visible).max(0.) * ROW_H).max(0.)
}

pub fn record_offset(m: &Model, idx: usize) -> f32 {
    // 列表按最新在上：第 idx 条（m.history 顺序）的距离 = (len-1-idx) 行。
    (m.history.len().saturating_sub(1 + idx) as f32) * ROW_H
}

pub fn draw(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    sess: &Session,
    cfg: &Config,
    m: &Model,
    t: &Theme,
    w: f32,
    h: f32,
) {
    if m.modal != Modal::None {
        return;
    }
    if m.ledger_open {
        draw_table(f, hs, sess, cfg, m, t, w, h);
    } else if !cfg.minimal {
        draw_bar(f, hs, sess, cfg, m, t, w, h);
    }
}

fn draw_bar(
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
    let (x, y, bw, bh) = bar_rect(cfg, w, h);
    if sess.phase == Phase::Done {
        // 完成态与空闲态同一张底栏：同背景、同 14px 内距、同 1px 垂直基准。
        round(f, x, y, bw, bh, 8., p.surface, Some(p.line));
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
            x + 14.,
            y + 9.,
            14.,
            elide(
                &summary,
                ((bw - controls_w - 46.) / 14.).max(6.) as usize,
            ),
            p.text2,
        );
        let mut bx = x + bw - 14. - controls_w;
        if has_next {
            button(f, hs, bx, y + 2., 90., "下一段", Action::Next, t, true);
            bx += 96.;
        }
        button(f, hs, bx, y + 2., 82., "重打", Action::Restart, t, !has_next);
        if let Some(idx) = m.history.len().checked_sub(1) {
            button(
                f,
                hs,
                bx + 88.,
                y + 2.,
                98.,
                "查看成绩",
                Action::SelectHistory(idx),
                t,
                false,
            );
        }
        return;
    }
    round(f, x, y, bw, bh, 8., p.surface, Some(p.line));
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
                format!("最近 {} 字/分", fmt_speed(r)),
                p.muted,
            );
        }
    }
    right(f, x + bw - 14., y + 7., 16., "⌄", p.muted);
    hs.push(Hit {
        x,
        y,
        w: bw,
        h: bh,
        action: Action::ToggleLedger,
    });
}

const COLUMNS: [(&str, f32); 9] = [
    ("用时", 0.10),
    ("重打", 0.07),
    ("段号", 0.11),
    ("速度", 0.10),
    ("击键", 0.09),
    ("码长", 0.09),
    ("键准", 0.10),
    ("打词", 0.09),
    ("字数", 0.08),
];

fn cell_text(r: &ScoreRecord) -> [String; 9] {
    let seg = r.mark.trim().to_string();
    [
        duration(r.time_ms),
        r.retype_count.to_string(),
        seg,
        fmt_speed(r),
        format!("{:.2}", r.strokes_per_sec),
        format!("{:.2}", r.code_len),
        format!("{:.1}%", r.accuracy.clamp(0., 100.)),
        r.word_rate
            .map(|v| format!("{v:.0}%"))
            .unwrap_or_else(|| "—".into()),
        r.chars.to_string(),
    ]
}

fn draw_table(
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
    let (x, y, pw, ph) = panel_rect_for(cfg, m, w, h);
    round(f, x, y, pw, ph, 10., p.surface, Some(p.line2));
    // 上沿拖动把手：hover 时主程序把光标换成上下箭头，拖动改面板高度。
    round(
        f,
        x + pw / 2. - 18.,
        y + 3.,
        36.,
        4.,
        2.,
        alpha(p.line2, 0.9),
        None,
    );
    // 表头（可点收起）
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
    text(f, x + 16., y + 10., 15., "成绩记录", p.text);
    text(
        f,
        x + 86.,
        y + 12.,
        12.,
        format!("{} 条 · {}", m.history.len(), summary),
        p.muted,
    );
    right(f, x + pw - 16., y + 9., 15., "收起 ⌃", p.muted);
    hs.push(Hit {
        x,
        y,
        w: pw,
        h: BAR_H,
        action: Action::ToggleLedger,
    });
    if sess.phase == Phase::Done {
        let has_next = sess.current_index() + 1 < sess.total_segments();
        let mut rx = x + pw - 76.;
        if has_next {
            button(f, hs, rx - 90., y + 3., 84., "下一段", Action::Next, t, true);
            rx -= 98.;
        }
        button(f, hs, rx - 76., y + 3., 70., "重打", Action::Restart, t, false);
    }
    line(f, x + 12., y + BAR_H, pw - 24., p.line);

    let left = x + 16.;
    let inner = pw - 32.;
    let total_w: f32 = COLUMNS.iter().map(|c| c.1).sum();
    let mut col_x = [0f32; 9];
    let mut col_w = [0f32; 9];
    let mut cx = left;
    for (i, (_, weight)) in COLUMNS.iter().enumerate() {
        col_x[i] = cx;
        col_w[i] = inner * weight / total_w;
        cx += col_w[i];
    }
    for (i, (name, _)) in COLUMNS.iter().enumerate() {
        f.push(DrawCmd::PushClip {
            x: col_x[i] + 2.,
            y: y + BAR_H + 5.,
            w: col_w[i] - 4.,
            h: 18.,
        });
        f.push(DrawCmd::TextRun {
            x: col_x[i] + col_w[i] - 6.,
            y: y + BAR_H + 5.,
            h: 13.,
            text: (*name).into(),
            color: p.muted,
            font: FontFamily::Ui,
            align: Align::Right,
        });
        f.push(DrawCmd::PopClip);
    }
    let list_top = y + BAR_H + HEAD_H;
    let list_h = ph - BAR_H - HEAD_H - PAD_BOTTOM;
    let max = scroll_max(cfg, m, h);
    let scroll = m.ledger_scroll.clamp(0., max);
    f.push(DrawCmd::PushClip {
        x,
        y: list_top,
        w: pw,
        h: list_h.max(1.),
    });
    let mut row_y = list_top - scroll;
    for idx in (0..m.history.len()).rev() {
        if row_y + ROW_H > list_top && row_y < list_top + list_h {
            let r = &m.history[idx];
            if idx + 1 == m.history.len() {
                rect(f, x + 16., row_y, pw - 32., ROW_H, alpha(p.accent, 0.07));
            }
            let cells = cell_text(r);
            for i in 0..9 {
                let color = if i == 3 { p.text } else { p.text2 };
                f.push(DrawCmd::PushClip {
                    x: col_x[i] + 2.,
                    y: row_y + 8.,
                    w: col_w[i] - 4.,
                    h: 20.,
                });
                f.push(DrawCmd::TextRun {
                    x: col_x[i] + col_w[i] - 6.,
                    y: row_y + 8.,
                    h: 13.,
                    text: cells[i].clone(),
                    color,
                    font: FontFamily::Mono,
                    align: Align::Right,
                });
                f.push(DrawCmd::PopClip);
            }
            line(f, x + 16., row_y + ROW_H - 1., pw - 32., alpha(p.line, 0.4));
        }
        row_y += ROW_H;
    }
    f.push(DrawCmd::PopClip);
    if max > 0. {
        let track = list_h.max(1.);
        let content = m.history.len() as f32 * ROW_H;
        let thumb = (track * track / content.max(1.)).clamp(18., track);
        rect(f, x + pw - 8., list_top, 4., track, alpha(p.text, 0.06));
        rect(
            f,
            x + pw - 8.,
            list_top + scroll / max * (track - thumb),
            4.,
            thumb,
            alpha(p.accent, 0.65),
        );
    }
}

fn fmt_speed(r: &ScoreRecord) -> String {
    if r.speed.is_finite() && r.speed >= 0. && r.speed <= 1000. {
        format!("{:.1}", r.speed)
    } else {
        "—".into()
    }
}

fn duration(ms: u64) -> String {
    let secs = ms as f64 / 1000.;
    format!("{}:{:04.1}", (secs / 60.) as u64, secs % 60.)
}


#[cfg(test)]
mod tests {
    use super::*;
    use gale_core::config::DisplayMode;

    /// 【2026-11-13】成绩面板/摘要条的左右边界必须与对照模式两张卡对齐：
    /// 非极简 36px 包边、极简 16px；字贴模式保持 24px 内距。
    #[test]
    fn ledger_panel_aligns_with_split_cards_in_both_chrome_modes() {
        let m = Model {
            ledger_open: true,
            ..Default::default()
        };
        let w = 1300.;
        for minimal in [false, true] {
            let mut cfg = Config::default();
            cfg.display_mode = DisplayMode::Split;
            cfg.minimal = minimal;
            let (x, _, pw, _) = panel_rect_for(&cfg, &m, w, 800.);
            let bw = if minimal {
                (w - 56.).max(360.).min(1600.)
            } else {
                (w - 96.).max(360.).min(1600.)
            };
            let card_x = (w - bw) / 2. - 12.;
            assert!((x - card_x).abs() < 0.01, "panel x {x} vs card {card_x}");
            assert!((pw - (bw + 24.)).abs() < 0.01, "panel w {pw}");
        }
        let cfg = Config::default();
        let (x, _, pw, _) = panel_rect_for(&cfg, &m, w, 800.);
        assert_eq!(x, 24.);
        assert_eq!(pw, w - 48.);
        // 摘要条与面板同界
        let (bx, _, bwidth, _) = bar_rect(&cfg, w, 800.);
        assert_eq!(bx, 24.);
        assert_eq!(bwidth, w - 48.);
        let mut split = Config::default();
        split.display_mode = DisplayMode::Split;
        let (bx, _, bwidth, _) = bar_rect(&split, w, 800.);
        assert_eq!(bx, 36.);
        assert_eq!(bwidth, w - 72.);
    }
}