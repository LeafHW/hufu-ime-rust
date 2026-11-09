//! Result, race report, online library and cleaning workbench from the Quiet Room design.
//!
//! These views render actual session/account data. Historical comparisons and character
//! timings are shown only when their provenance is available; design sample values are
//! deliberately never used as application data.
use crate::{
    modals::{Action, FieldSpec, Hit, Modal, Model},
    session::{Phase, Session},
    ui::*,
};
use gale_core::{
    compare::{CharState, TimingEvent},
    config::Config,
    rules::TrainingMode,
    score::{race_badge, ScoreRecord, VALID_MIN_CHARS},
    theme::Theme,
};
use gale_win::d2d::{DrawCmd, Frame};
use std::collections::BTreeMap;

type Ink = [f32; 4];

// Secondary windows use a 15 DIP floor: 14 DIP CJK read too small on the
// 960/1040 wide panels, and every view here is supporting text or controls.
fn text(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: Ink) {
    crate::ui::text(f, x, y, size.max(15.), s, c);
}
fn mono(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: Ink) {
    crate::ui::mono(f, x, y, size.max(15.), s, c);
}
fn body_text(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: Ink) {
    crate::ui::body_text(f, x, y, size.max(15.), s, c);
}
fn right(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: Ink) {
    crate::ui::right(f, x, y, size.max(15.), s, c);
}

fn card(f: &mut Frame, x: f32, y: f32, w: f32, h: f32, t: &Theme) {
    let p = tokens(t);
    round(f, x, y, w, h, 9., p.surface2, Some(p.line));
}

fn clip(f: &mut Frame, x: f32, y: f32, w: f32, h: f32) {
    f.push(DrawCmd::PushClip {
        x,
        y,
        w: w.max(0.),
        h: h.max(0.),
    });
}

fn end_clip(f: &mut Frame) {
    f.push(DrawCmd::PopClip);
}

fn clipped_text(f: &mut Frame, x: f32, y: f32, w: f32, size: f32, s: &str, c: Ink) {
    clip(f, x, y, w, size * 1.7);
    text(f, x, y, size, s, c);
    end_clip(f);
}

fn keycap(f: &mut Frame, x: f32, y: f32, s: &str, t: &Theme) {
    let p = tokens(t);
    round(f, x, y, 36., 28., 4., p.surface, Some(p.line2));
    mono(f, x + 7., y + 4., 14., s, p.muted);
}

fn action(
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
        36.,
        8.,
        if primary { p.accent } else { p.surface },
        Some(if primary {
            alpha(p.accent, 0.8)
        } else {
            p.line2
        }),
    );
    clipped_text(
        f,
        x + 12.,
        y + 8.,
        w - 20.,
        14.,
        s,
        if primary { p.bg } else { p.text2 },
    );
    if a != Action::None {
        hs.push(Hit {
            x,
            y,
            w,
            h: 36.,
            action: a,
        });
    }
}

fn link(f: &mut Frame, hs: &mut Vec<Hit>, x: f32, y: f32, w: f32, s: &str, a: Action, t: &Theme) {
    let p = tokens(t);
    clipped_text(f, x, y + 8., w, 14., s, p.accent);
    if a != Action::None {
        hs.push(Hit {
            x,
            y,
            w,
            h: 36.,
            action: a,
        });
    }
}

fn chip(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    s: &str,
    selected: bool,
    a: Action,
    t: &Theme,
) {
    let p = tokens(t);
    round(
        f,
        x,
        y,
        w,
        36.,
        5.,
        if selected {
            alpha(p.accent, 0.12)
        } else {
            p.surface
        },
        Some(if selected {
            alpha(p.accent, 0.36)
        } else {
            p.line
        }),
    );
    clipped_text(
        f,
        x + 7.,
        y + 8.,
        w - 12.,
        14.,
        s,
        if selected { p.accent } else { p.muted },
    );
    if a != Action::None {
        hs.push(Hit {
            x,
            y,
            w,
            h: 36.,
            action: a,
        });
    }
}

fn toggle(f: &mut Frame, hs: &mut Vec<Hit>, x: f32, y: f32, on: bool, a: Action, t: &Theme) {
    let p = tokens(t);
    round(
        f,
        x,
        y,
        26.,
        15.,
        7.5,
        if on { p.accent } else { p.line2 },
        None,
    );
    round(
        f,
        x + if on { 14. } else { 3. },
        y + 3.,
        9.,
        9.,
        4.5,
        if on { p.bg } else { p.muted },
        None,
    );
    if a != Action::None {
        hs.push(Hit {
            x: x - 3.,
            y: y - 9.,
            w: 32.,
            h: 36.,
            action: a,
        });
    }
}

/// Draw a short static segment using small filled pixels. DrawCmd::Line is an
/// axis-aligned divider, so it must not be used for the sparkline's sloping edges.
fn stroke(f: &mut Frame, from: (f32, f32), to: (f32, f32), c: Ink) {
    let n = ((to.0 - from.0).abs().max((to.1 - from.1).abs()) / 1.25)
        .ceil()
        .max(1.)
        .min(800.) as usize;
    for i in 0..=n {
        let z = i as f32 / n as f32;
        rect(
            f,
            from.0 + (to.0 - from.0) * z,
            from.1 + (to.1 - from.1) * z,
            1.4,
            1.4,
            c,
        );
    }
}

fn wrap_lines(s: &str, width: f32, size: f32) -> Vec<String> {
    let limit = (width / size).max(1.);
    let mut out = Vec::new();
    for paragraph in s.split('\n') {
        let mut line = String::new();
        let mut used = 0.;
        for ch in paragraph.chars() {
            let cw = if ch.is_ascii() { 0.57 } else { 1. };
            if used + cw > limit && !line.is_empty() {
                out.push(std::mem::take(&mut line));
                used = 0.;
            }
            line.push(ch);
            used += cw;
        }
        out.push(line);
    }
    out
}

fn paragraph(f: &mut Frame, x: f32, y: f32, w: f32, h: f32, size: f32, lh: f32, s: &str, c: Ink) {
    clip(f, x, y, w, h);
    let rows = (h / lh).ceil().max(0.) as usize;
    for (i, line) in wrap_lines(s, w, size).into_iter().take(rows).enumerate() {
        body_text(f, x, y + i as f32 * lh, size, line, c);
    }
    end_clip(f);
}

fn duration(ms: u64) -> String {
    let tenths = ms / 100;
    format!(
        "{:02}:{:02}.{}",
        tenths / 600,
        (tenths / 10) % 60,
        tenths % 10
    )
}

fn segment_caption(index: usize, mark: &str) -> String {
    let ordinal = index.saturating_add(1);
    if mark.is_empty() || mark == ordinal.to_string() {
        format!("第 {ordinal} 段")
    } else {
        format!("第 {ordinal} 段 · {mark}")
    }
}

fn record_time(r: &ScoreRecord) -> String {
    // ScoreRecord.date uses the record's own civil date. Do not mix it with a
    // guessed local UTC offset to manufacture a wall-clock timestamp.
    r.date.clone()
}

fn live_trace(sess: &Session, r: &ScoreRecord) -> bool {
    sess.recorded
        && r.para_index == sess.current_index()
        && r.title == sess.title
        && sess
            .results
            .get(r.para_index)
            .and_then(Option::as_ref)
            .is_some_and(|x| x.ts == r.ts)
}

/// Net committed characters in elapsed-time windows. Empty windows stay zero,
/// and Backspace reduces the window's net output instead of adding throughput.
fn speed_samples(events: &[TimingEvent], elapsed_ms: u64) -> (u64, Vec<f64>) {
    if events.is_empty() || elapsed_ms == 0 {
        return (5000, Vec::new());
    }
    let step = elapsed_ms
        .saturating_add(199)
        .checked_div(200)
        .unwrap_or(5000)
        .max(5000);
    let n = elapsed_ms
        .saturating_add(step - 1)
        .checked_div(step)
        .unwrap_or(1)
        .clamp(1, 200) as usize;
    let mut counts = vec![0i64; n];
    let mut at = 0u64;
    for e in events {
        at = at.saturating_add(e.dt_ms);
        let i = (at / step).min(n as u64 - 1) as usize;
        counts[i] = counts[i].saturating_add(e.len_delta as i64);
    }
    let samples = counts
        .into_iter()
        .enumerate()
        .map(|(i, chars)| {
            let span = elapsed_ms.saturating_sub(i as u64 * step).min(step).max(1);
            chars.max(0) as f64 * 60000. / span as f64
        })
        .collect();
    (step, samples)
}

#[derive(Debug)]
struct SlowEstimate {
    ch: char,
    average_ms: f64,
    count: usize,
    wrong: usize,
}

/// Commit groups have one timestamp, not one timestamp per character. Group time
/// is shared evenly and explicitly labelled as an estimate in the report.
fn slow_estimates(sess: &Session) -> Vec<SlowEstimate> {
    let target: Vec<char> = sess.comparer.text.chars().collect();
    let mut position = 0usize;
    let mut groups: BTreeMap<char, (f64, usize)> = BTreeMap::new();
    for e in &sess.comparer.timing_events {
        if e.len_delta > 0 {
            let n = e.len_delta as usize;
            let each = e.dt_ms as f64 / n as f64;
            for ch in target.iter().skip(position).take(n) {
                if !ch.is_whitespace() && e.dt_ms > 0 {
                    let item = groups.entry(*ch).or_default();
                    item.0 += each;
                    item.1 += 1;
                }
            }
            position = position.saturating_add(n).min(target.len());
        } else {
            position = position.saturating_sub(e.len_delta.unsigned_abs() as usize);
        }
    }
    let mut wrong = BTreeMap::<char, usize>::new();
    for (i, state) in sess.comparer.states.iter().enumerate() {
        if *state == CharState::Wrong {
            if let Some(ch) = target.get(i) {
                *wrong.entry(*ch).or_default() += 1;
            }
        }
    }
    let mut out: Vec<_> = groups
        .into_iter()
        .map(|(ch, (total, count))| SlowEstimate {
            ch,
            average_ms: total / count.max(1) as f64,
            count,
            wrong: wrong.get(&ch).copied().unwrap_or(0),
        })
        .collect();
    out.sort_by(|a, b| b.average_ms.total_cmp(&a.average_ms).then(a.ch.cmp(&b.ch)));
    out.truncate(5);
    out
}

pub fn session_report(m: &Model) -> bool {
    m.field("report_tab") == "session"
}

pub fn selected_report_record(m: &Model) -> Option<&ScoreRecord> {
    m.selected_history
        .and_then(|index| m.history.get(index))
        .or_else(|| m.history.last())
        .or(m.latest.as_ref())
}

fn report_columns(width: f32) -> (f32, bool, f32, usize) {
    let cw = (width - 48.).min(980.).max(160.);
    let wide = cw >= 720.;
    let list_w = if !wide {
        cw
    } else if cw >= 900. {
        300.
    } else {
        240.
    };
    (cw, wide, list_w, if wide { 6 } else { 3 })
}

/// Keep rendering, wheel scrolling and page buttons on the same page size.
pub fn report_visible_rows(m: &Model, width: f32) -> usize {
    if session_report(m) {
        if (width - 48.).min(980.) >= 720. {
            5
        } else {
            3
        }
    } else {
        report_columns(width).3
    }
}

pub fn report_scroll_cap(m: &Model, sess: &Session, width: f32) -> usize {
    let rows = if session_report(m) {
        sess.results.iter().flatten().count()
    } else {
        m.history.len()
    };
    rows.saturating_sub(report_visible_rows(m, width))
}

/// A narrow report stacks its detail below the list. Selecting a record should
/// bring that actual detail into view rather than leave the selection offscreen.
pub fn history_detail_scroll(m: &Model, sess: &Session, width: f32, viewport: f32) -> f32 {
    let (_, wide, _, _) = report_columns(width);
    let limit = report_visible_rows(m, width);
    if wide {
        return 0.;
    }
    let page = (m.scroll as usize).min(report_scroll_cap(m, sess, width));
    let shown = m.history.len().saturating_sub(page).min(limit);
    let detail_top = 80. + 34. + shown as f32 * 64. + 66.;
    (detail_top - 16.).min((report_content_height(m, sess, width) - viewport).max(0.))
}

/// The report has a compact empty state and only scrolls when its actual content needs it.
pub fn report_content_height(m: &Model, sess: &Session, width: f32) -> f32 {
    if session_report(m) {
        if sess.results.iter().all(Option::is_none) {
            320.
        } else {
            1328.
        }
    } else if selected_report_record(m).is_none() {
        320.
    } else {
        let (cw, wide, list_w, _) = report_columns(width);
        let limit = report_visible_rows(m, width);
        if wide {
            let detail_h: f32 = if cw - list_w - 24. >= 480. {
                420.
            } else {
                510.
            };
            let list_h = 34.
                + m.history.len().min(limit) as f32 * 64.
                + if m.history.len() > limit { 52. } else { 0. };
            80. + detail_h.max(list_h) + 80.
        } else {
            let detail_h = if cw >= 480. { 420. } else { 510. };
            80. + 34. + m.history.len().min(limit) as f32 * 64. + 66. + detail_h + 80.
        }
    }
}

fn main_area(
    f: &mut Frame,
    m: &Model,
    sess: &Session,
    t: &Theme,
    x: f32,
    y: f32,
    pw: f32,
    ph: f32,
) -> (f32, f32, f32) {
    let p = tokens(t);
    rect(f, x, y, pw, ph, p.bg);
    let cw = (pw - 48.).min(980.).max(160.);
    let cx = x + (pw - cw) / 2.;
    let height = report_content_height(m, sess, pw);
    let maximum = (height - ph).max(0.);
    let scroll = m.panel_scroll.clamp(0., maximum);
    if maximum > 0. {
        let viewport = (ph - 62.).max(1.);
        let track = (viewport - 42.).max(1.);
        let body = (height - 62.).max(viewport);
        let thumb = (track * viewport / body).clamp(18f32.min(track), track);
        round(f, x + pw - 10., y + 32., 3., track, 1.5, p.line, None);
        round(
            f,
            x + pw - 10.,
            y + 32. + scroll / maximum * (track - thumb),
            3.,
            thumb,
            1.5,
            alpha(p.accent, 0.7),
            None,
        );
    }
    clip(f, x, y, pw, (ph - 62.).max(1.));
    (cx, y - scroll, cw)
}

fn metric(f: &mut Frame, x: f32, y: f32, w: f32, value: &str, label: &str, t: &Theme) {
    let p = tokens(t);
    clipped_text(f, x, y, w, 25., value, p.text);
    clipped_text(f, x, y + 35., w, 14., label, p.muted);
}

fn result_badge(m: &Model, r: &ScoreRecord) -> &'static str {
    let (mode, hints) = r.round.as_ref().map(|r| (r.mode, r.word_hints)).unwrap_or((
        if m.race {
            TrainingMode::Race
        } else {
            TrainingMode::Practice
        },
        m.hints && !m.race,
    ));
    race_badge(mode, hints)
}

fn draw_report(
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
    let first_hit = hs.len();
    if session_report(m) {
        draw_session_report(f, hs, m, sess, t, x, y, pw, ph);
    } else {
        draw_history_report(f, hs, m, sess, t, x, y, pw, ph);
    }
    // Body controls scroll with the drawing clip; clipped rows cannot intercept
    // the toolbar or fixed report footer.
    for hit in hs.iter_mut().skip(first_hit) {
        let fixed_footer = matches!(
            hit.action,
            Action::Close | Action::CopyScore | Action::CopyRace | Action::ExportImage
        ) || (hit.y >= y + ph - 48.
            && hit.action == crate::aux_marker("report_history"));
        if !fixed_footer {
            let bottom = (hit.y + hit.h).min(y + ph - 62.);
            hit.y = hit.y.max(y);
            hit.h = (bottom - hit.y).max(0.);
        }
    }
}

fn report_heading(f: &mut Frame, hs: &mut Vec<Hit>, m: &Model, t: &Theme, x: f32, y: f32) {
    let p = tokens(t);
    text(f, x, y + 20., 24., "成绩", p.text);
    chip(
        f,
        hs,
        x + 100.,
        y + 14.,
        126.,
        "成绩记录",
        !session_report(m),
        crate::aux_marker("report_history"),
        t,
    );
    chip(
        f,
        hs,
        x + 236.,
        y + 14.,
        126.,
        "本轮汇总",
        session_report(m),
        crate::aux_marker("report_session"),
        t,
    );
}

fn draw_history_report(
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
    let (cx, by, cw) = main_area(f, m, sess, t, x, y, pw, ph);
    report_heading(f, hs, m, t, cx, by);
    let Some(record) = selected_report_record(m) else {
        text(f, cx, by + 140., 21., "还没有成绩记录", p.text2);
        text(
            f,
            cx,
            by + 180.,
            15.,
            "完成一段后会自动保存在这里，换文后仍可查看。",
            p.muted,
        );
        end_clip(f);
        return;
    };

    let (_, wide, list_w, _) = report_columns(pw);
    let limit = report_visible_rows(m, pw);
    let list_y = by + 80.;
    let page = (m.scroll as usize).min(report_scroll_cap(m, sess, pw));
    let shown = m.history.len().saturating_sub(page).min(limit);
    text(
        f,
        cx,
        list_y,
        16.,
        format!("全部记录 · {} 条", m.history.len()),
        p.text2,
    );
    for (row, (index, item)) in m
        .history
        .iter()
        .enumerate()
        .rev()
        .skip(page)
        .take(limit)
        .enumerate()
    {
        let yy = list_y + 34. + row as f32 * 64.;
        let selected = std::ptr::eq(item, record);
        if selected {
            round(
                f,
                cx,
                yy,
                list_w,
                58.,
                7.,
                alpha(p.accent, 0.10),
                Some(alpha(p.accent, 0.28)),
            );
        } else {
            line(f, cx + 10., yy + 58., list_w - 20., p.line);
        }
        clipped_text(
            f,
            cx + 12.,
            yy + 8.,
            list_w - 102.,
            15.,
            &item.title,
            if selected { p.accent } else { p.text2 },
        );
        right(
            f,
            cx + list_w - 12.,
            yy + 8.,
            16.,
            format!("{:.1}", item.speed),
            p.text2,
        );
        clipped_text(
            f,
            cx + 12.,
            yy + 32.,
            list_w - 20.,
            14.,
            &format!(
                "{} · 第 {} 段 · {} 字",
                item.date,
                item.para_index + 1,
                item.chars
            ),
            p.muted,
        );
        hs.push(Hit {
            x: cx,
            y: yy,
            w: list_w,
            h: 58.,
            action: Action::SelectHistory(index),
        });
    }
    let list_bottom = list_y + 34. + shown as f32 * 64.;
    if m.history.len() > limit {
        action(
            f,
            hs,
            cx,
            list_bottom + 8.,
            90.,
            "较新",
            if page > 0 {
                crate::aux_marker(&format!("history_step:-{limit}"))
            } else {
                Action::None
            },
            t,
            false,
        );
        action(
            f,
            hs,
            cx + 100.,
            list_bottom + 8.,
            90.,
            "较早",
            if page + shown < m.history.len() {
                crate::aux_marker(&format!("history_step:{limit}"))
            } else {
                Action::None
            },
            t,
            false,
        );
    }

    let dx = if wide { cx + list_w + 24. } else { cx };
    let dy = if wide { list_y } else { list_bottom + 66. };
    let dw = if wide { cw - list_w - 24. } else { cw };
    let cols = if dw >= 480. { 3 } else { 2 };
    let detail_h = if cols == 3 { 420. } else { 510. };
    card(f, dx, dy, dw, detail_h, t);
    let tx = dx + 20.;
    let tw = dw - 40.;
    clipped_text(f, tx, dy + 18., tw, 20., &record.title, p.text);
    clipped_text(
        f,
        tx,
        dy + 54.,
        tw,
        14.,
        &format!(
            "{} · {} · 第 {} 段 · {} 字",
            record_time(record),
            record.source,
            record.para_index + 1,
            record.chars
        ),
        p.muted,
    );
    let values = [
        (format!("{:.1}", record.speed), "速度 · 字/分"),
        (format!("{:.1}%", record.accuracy), "键准·按键效率"),
        (format!("{:.1}%", record.character_accuracy()), "字准"),
        (format!("{:.2}", record.strokes_per_sec), "击键 / 秒"),
        (format!("{:.2}", record.code_len), "码长"),
        (duration(record.time_ms), "用时"),
    ];
    for (i, (value, label)) in values.iter().enumerate() {
        metric(
            f,
            tx + (i % cols) as f32 * tw / cols as f32,
            dy + 98. + (i / cols) as f32 * 80.,
            tw / cols as f32 - 12.,
            value,
            label,
            t,
        );
    }
    let extra_y = dy + 98. + (values.len() / cols) as f32 * 80.;
    line(f, tx, extra_y + 2., tw, p.line);
    let word_rate = record
        .word_rate
        .map(|rate| format!("{rate:.1}%"))
        .unwrap_or_else(|| "—".into());
    paragraph(
        f,
        tx,
        extra_y + 18.,
        tw,
        62.,
        15.,
        26.,
        &format!(
            "总键 {} · 错字 {} · 退格 {} · 选重 {}\n词率 {} · 重打 {} 次",
            record.keys,
            record.wrong,
            record.backspaces,
            record.select_count,
            word_rate,
            record.retype_count
        ),
        p.text2,
    );
    if live_trace(sess, record) && !sess.last_vk_histogram.is_empty() {
        link(
            f,
            hs,
            tx,
            extra_y + 86.,
            tw,
            "查看本段按键热力图",
            crate::aux_marker("show_heatmap"),
            t,
        );
    } else {
        clipped_text(
            f,
            tx,
            extra_y + 96.,
            tw,
            14.,
            "此记录未保存按键分布。",
            p.muted,
        );
    }
    end_clip(f);
    let ay = y + ph - 48.;
    action(
        f,
        hs,
        cx,
        ay,
        142.,
        "复制这条成绩",
        Action::CopyScore,
        t,
        true,
    );
    action(
        f,
        hs,
        cx + 154.,
        ay,
        126.,
        "导出图片",
        Action::ExportImage,
        t,
        false,
    );
    if !wide {
        action(
            f,
            hs,
            cx + cw - 142.,
            ay,
            142.,
            "← 全部记录",
            crate::aux_marker("report_history"),
            t,
            false,
        );
    }
}

fn draw_session_report(
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
    let (cx, base_y, cw) = main_area(f, m, sess, t, x, y, pw, ph);
    report_heading(f, hs, m, t, cx, base_y);
    let by = base_y + 16.;
    let rows: Vec<_> = sess
        .results
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.as_ref().map(|r| (i, r)))
        .collect();
    if rows.is_empty() {
        text(
            f,
            cx,
            by + 90.,
            16.,
            "本轮还没有完成的段落。历史成绩可在「成绩记录」中查看。",
            p.muted,
        );
        end_clip(f);
        return;
    }
    let agg = sess.aggregate();
    let character_accuracy = if agg.chars > 0 {
        agg.chars.saturating_sub(agg.wrong) as f64 / agg.chars as f64 * 100.
    } else {
        0.
    };
    clipped_text(
        f,
        cx,
        by + 64.,
        cw,
        16.,
        &format!(
            "{} · 已完成 {} / {} 段",
            sess.title,
            agg.completed_segments,
            sess.total_segments()
        ),
        p.text2,
    );
    let newest = rows.iter().max_by_key(|(_, r)| r.ts).unwrap().1;
    text(
        f,
        cx,
        by + 96.,
        14.,
        format!(
            "{} · {} · 仅统计已结算且不少于 {} 字的段落",
            result_badge(m, newest),
            newest.date,
            VALID_MIN_CHARS
        ),
        p.muted,
    );
    let gated: Vec<_> = rows
        .iter()
        .map(|(_, r)| *r)
        .filter(|r| r.chars >= VALID_MIN_CHARS)
        .collect();
    let select_total: u32 = gated.iter().map(|r| r.select_count).sum();
    let ding_total: u32 = gated.iter().map(|r| r.ding_count).sum();
    let summaries = [
        (format!("{:.1}", agg.speed), "总速度 · 字/分"),
        (format!("{:.1}", agg.effective_speed), "有效速度"),
        (duration(agg.time_ms), "总用时"),
        (agg.chars.to_string(), "总字数"),
        (format!("{:.1}%", agg.accuracy), "键准·按键效率"),
        (format!("{:.1}%", character_accuracy), "字准"),
        (format!("{:.2}", agg.strokes_per_sec), "击键 / 秒"),
        (format!("{:.2}", agg.code_len), "平均码长"),
        (select_total.to_string(), "选重"),
        (ding_total.to_string(), "标顶"),
    ];
    let cols = if cw >= 720. { 5 } else { 3 };
    for (i, (value, label)) in summaries.iter().enumerate() {
        metric(
            f,
            cx + (i % cols) as f32 * cw / cols as f32,
            by + 150. + (i / cols) as f32 * 78.,
            cw / cols as f32 - 12.,
            value,
            label,
            t,
        );
    }
    let chart_y = by + if cols == 5 { 326. } else { 482. };
    card(f, cx, chart_y, cw, 176., t);
    text(f, cx + 16., chart_y + 14., 18., "分段速度对比", p.text2);
    text(
        f,
        cx + 16.,
        chart_y + 47.,
        14.,
        "纵轴从 0 起算；虚线为已结算段总速度。",
        p.muted,
    );
    let page = (m.scroll as usize).min(report_scroll_cap(m, sess, pw));
    let visible = report_visible_rows(m, pw);
    let shown: Vec<_> = rows.iter().skip(page).take(visible).collect();
    let highest = rows.iter().map(|(_, r)| r.speed).fold(1., f64::max);
    let max = (highest / 50.).ceil().max(1.) * 50.;
    let plot_x = cx + 64.;
    let plot_w = cw - 88.;
    let plot_y = chart_y + 94.;
    let plot_h = 60.;
    for i in 0..=2 {
        let yy = plot_y + plot_h - i as f32 * plot_h / 2.;
        line(f, plot_x, yy, plot_w, p.line);
        right(
            f,
            plot_x - 12.,
            yy - 8.,
            14.,
            format!("{:.0}", i as f64 * max / 2.),
            p.muted,
        );
    }
    let mean_y = plot_y + plot_h - (agg.speed / max) as f32 * plot_h;
    for i in 0..(plot_w / 8.) as usize {
        rect(f, plot_x + i as f32 * 8., mean_y, 4., 1., p.accent);
    }
    // Keep the mean label at the left end so it never collides with the last
    // bar's value label.
    text(
        f,
        plot_x + 6.,
        (mean_y - 18.).max(plot_y + 2.),
        14.,
        format!("均速 {:.1}", agg.speed),
        p.accent,
    );
    let bw = plot_w / shown.len().max(1) as f32;
    for (j, row) in shown.iter().enumerate() {
        let (idx, r) = **row;
        let xx = plot_x + (j as f32 + 0.5) * bw;
        let bh = (r.speed / max) as f32 * plot_h;
        rect(f, xx - 14., plot_y + plot_h - bh, 28., bh, p.accent);
        right(
            f,
            xx + 23.,
            chart_y + 72.,
            14.,
            format!("{:.1}", r.speed),
            p.text2,
        );
        right(
            f,
            xx + 26.,
            chart_y + 160.,
            14.,
            format!("第 {} 段", idx + 1),
            p.muted,
        );
    }
    let detail_y = chart_y + 192.;
    text(f, cx, detail_y, 18., "逐段明细", p.text2);
    text(
        f,
        cx,
        detail_y + 34.,
        14.,
        "键准 / 字准分别表示按键效率 / 文本正确率。",
        p.muted,
    );
    let count = report_visible_rows(m, pw);
    for (j, (idx, r)) in rows.iter().skip(page).take(count).enumerate() {
        let yy = detail_y + 76. + j as f32 * 68.;
        line(f, cx, yy - 10., cw, p.line);
        clipped_text(
            f,
            cx,
            yy,
            cw,
            16.,
            &format!(
                "第 {} 段 · {} · {} 字 · 速度 {:.1} 字/分 · {}",
                idx + 1,
                r.mark,
                r.chars,
                r.speed,
                if r.round.as_ref().is_some_and(|r| !r.complete) {
                    "中途停止"
                } else {
                    "已结算"
                }
            ),
            p.text2,
        );
        clipped_text(
            f,
            cx,
            yy + 28.,
            cw,
            14.,
            &format!(
                "键/字准 {:.1}/{:.1} · 击键 {:.2} · 码长 {:.2} · 错字 {} · 回改 {} · {}",
                r.accuracy,
                r.character_accuracy(),
                r.strokes_per_sec,
                r.code_len,
                r.wrong,
                r.corrections,
                duration(r.time_ms)
            ),
            p.muted,
        );
    }
    let tail_y = detail_y + 76. + count as f32 * 68.;
    text(f, cx, tail_y, 18., "当前段慢字估计", p.text2);
    let estimates = slow_estimates(sess);
    let estimate_text = if estimates.is_empty() {
        "当前段尚无可分析的上屏时间事件。".into()
    } else {
        estimates
            .iter()
            .map(|z| {
                format!(
                    "「{}」{:.2} 秒（{} 次，错 {} 次）",
                    z.ch,
                    z.average_ms / 1000.,
                    z.count,
                    z.wrong
                )
            })
            .collect::<Vec<_>>()
            .join(" · ")
    };
    paragraph(
        f,
        cx,
        tail_y + 34.,
        cw,
        62.,
        14.,
        24.,
        &estimate_text,
        p.muted,
    );
    text(
        f,
        cx,
        tail_y + 105.,
        14.,
        "上屏组时长按字符均摊，属于估计值；滚轮可查看更多已结算段。",
        p.muted,
    );
    if sess.phase == Phase::Typing && !sess.recorded {
        let live = sess.live_stats_now();
        text(
            f,
            cx,
            tail_y + 135.,
            14.,
            format!(
                "含当前段的实时统计：{} 字 · 速度 {:.1} · 击键 {:.2} · 键准 {:.1}%",
                live.done_chars, live.total_speed, live.keys_per_sec, live.accuracy
            ),
            p.accent,
        );
    }
    end_clip(f);
    let ay = y + ph - 48.;
    action(
        f,
        hs,
        cx,
        ay,
        174.,
        "复制整份成绩单",
        Action::CopyRace,
        t,
        true,
    );
    action(
        f,
        hs,
        cx + 186.,
        ay,
        142.,
        "导出图片",
        Action::ExportImage,
        t,
        false,
    );
}

fn overlay_header(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    t: &Theme,
    x: f32,
    y: f32,
    pw: f32,
    icon: &str,
    title: &str,
    sub: &str,
    key: &str,
) {
    let p = tokens(t);
    line(f, x + 16., y + 62., pw - 32., p.line);
    text(f, x + 22., y + 17., 23., icon, p.accent);
    text(f, x + 62., y + 10., 20., title, p.text);
    clipped_text(f, x + 62., y + 36., pw - 198., 14., sub, p.muted);
    keycap(f, x + pw - 102., y + 17., key, t);
    text(f, x + pw - 49., y + 13., 25., "×", p.muted);
    hs.push(Hit {
        x: x + pw - 58.,
        y: y + 10.,
        w: 42.,
        h: 42.,
        action: Action::Close,
    });
}

fn login_visible(m: &Model) -> bool {
    (!m.online_authenticated || m.online_expired) && m.online_login_expanded
}

fn online_form(pw: f32) -> (f32, f32) {
    let w = (pw - 64.).min(560.).max(120.);
    ((pw - w) / 2., w)
}

fn field_shell(f: &mut Frame, x: f32, y: f32, w: f32, label: &str, t: &Theme) {
    let p = tokens(t);
    text(f, x, y - 24., 14., label, p.muted);
    round(f, x - 1., y - 1., w + 2., 38., 6., p.bg2, Some(p.line2));
}

pub(crate) fn content_height_online(_m: &Model) -> f32 {
    700.
}

fn draw_online(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    cfg: &Config,
    t: &Theme,
    x: f32,
    y: f32,
    pw: f32,
    ph: f32,
) {
    let p = tokens(t);
    overlay_header(
        f,
        hs,
        t,
        x,
        y,
        pw,
        "◎",
        "在线发文",
        "选择文章后直接载入跟打区",
        "F8",
    );
    let ix = x + 24.;
    let iw = pw - 48.;
    let active = m.online_authenticated && !m.online_expired;
    let compact_card = login_visible(m);
    card(f, ix, y + 82., iw, if compact_card { 44. } else { 64. }, t);
    clipped_text(
        f,
        ix + 16.,
        if compact_card { y + 92. } else { y + 96. },
        iw - 250.,
        16.,
        if m.online_account.is_empty() {
            if active {
                "已登录"
            } else {
                "未登录"
            }
        } else {
            &m.online_account
        },
        p.text2,
    );
    if !compact_card {
        clipped_text(
            f,
            ix + 16.,
            y + 121.,
            iw - 250.,
            14.,
            if m.online_expired {
                "登录已过期，当前文章已保留"
            } else {
                &cfg.wenlai_domain
            },
            p.muted,
        );
    } else if m.online_expired {
        clipped_text(
            f,
            ix + 16.,
            y + 113.,
            iw - 250.,
            14.,
            "登录已过期，当前文章已保留",
            p.muted,
        );
    }
    if active {
        action(
            f,
            hs,
            ix + iw - 132.,
            if compact_card { y + 88. } else { y + 96. },
            116.,
            "退出登录",
            Action::OnlineLogout,
            t,
            false,
        );
    } else {
        action(
            f,
            hs,
            ix + iw - 132.,
            if compact_card { y + 88. } else { y + 96. },
            116.,
            if login_visible(m) {
                "收起账号"
            } else {
                "登录 / 注册"
            },
            Action::OnlineAccount,
            t,
            false,
        );
    }
    if login_visible(m) {
        let (fx, fw) = online_form(pw);
        let fx = x + fx;
        field_shell(f, fx, y + 154., fw, "用户名", t);
        field_shell(f, fx, y + 226., fw, "密码 · 不保存", t);
        if m.online_register_mode {
            field_shell(f, fx, y + 298., fw, "邮箱", t);
            field_shell(f, fx, y + 370., fw - 152., "邮箱验证码", t);
            let label = if m.online_code_wait > 0 {
                format!("重发 {} 秒", m.online_code_wait)
            } else {
                "发送验证码".into()
            };
            action(
                f,
                hs,
                fx + fw - 140.,
                y + 370.,
                140.,
                &label,
                if m.online_code_wait > 0 {
                    Action::None
                } else {
                    crate::aux_marker("online_reg_code")
                },
                t,
                false,
            );
            action(
                f,
                hs,
                fx,
                y + 426.,
                180.,
                "注册并登录",
                crate::aux_marker("online_reg_submit"),
                t,
                true,
            );
            link(
                f,
                hs,
                fx,
                y + 474.,
                160.,
                "‹ 返回登录",
                crate::aux_marker("online_register"),
                t,
            );
            paragraph(
                f,
                fx,
                y + 484.,
                fw,
                50.,
                14.,
                25.,
                "验证码每 60 秒可获取一次。\n注册成功后自动登录，随后可取文和浏览书架。",
                p.muted,
            );
        } else {
            action(
                f,
                hs,
                fx,
                y + 280.,
                150.,
                "登录",
                Action::OnlineLogin,
                t,
                true,
            );
            link(
                f,
                hs,
                fx + 170.,
                y + 280.,
                190.,
                "没有账号？注册",
                crate::aux_marker("online_register"),
                t,
            );
            field_shell(f, fx, y + 374., fw, "服务地址 · HTTPS", t);
            action(
                f,
                hs,
                fx,
                y + 426.,
                176.,
                "应用服务地址",
                Action::OnlineServer,
                t,
                false,
            );
            paragraph(
                f,
                fx,
                y + 514.,
                fw,
                50.,
                14.,
                25.,
                "登录后自动收起账号表单。\n切换服务地址后需要重新登录。",
                p.muted,
            );
        }
        paragraph(
            f,
            fx,
            y + 610.,
            fw,
            75.,
            14.,
            24.,
            &m.online_status,
            p.muted,
        );
    } else {
        clipped_text(
            f,
            ix,
            y + 164.,
            iw,
            20.,
            if m.online_title.is_empty() {
                "尚未取文"
            } else {
                &m.online_title
            },
            p.text,
        );
        let meta = m
            .online_article
            .as_ref()
            .map(|a| {
                let position = if a.sort_num > 0 && a.end_sort_num > a.sort_num {
                    format!("第 {} 至 {} 段", a.sort_num, a.end_sort_num)
                } else if a.sort_num > 0 {
                    format!("第 {} 段", a.sort_num)
                } else if !a.seg_mark.is_empty() {
                    format!("段标 {}", a.seg_mark)
                } else {
                    "源站未提供段号".into()
                };
                format!(
                    "{} 字 · 分类 {} · 难度 {} · {}",
                    a.text.chars().count(),
                    if a.category.is_empty() {
                        "未提供"
                    } else {
                        &a.category
                    },
                    a.difficulty.label(),
                    position
                )
            })
            .unwrap_or_else(|| "取文后，文章会直接进入跟打区。".into());
        clipped_text(f, ix, y + 196., iw, 14., &meta, p.muted);
        text(f, ix, y + 232., 14., "分类", p.muted);
        let cap = ((iw - 90.) / 104.).floor().max(1.) as usize;
        let selected = m
            .online_category
            .min(m.online_categories.len().saturating_sub(1));
        let start = selected.saturating_add(1).saturating_sub(cap);
        let visible = m.online_categories.len().min(cap);
        let bw = ((iw - 90.) / visible.max(1) as f32 - 8.).min(140.);
        for (j, (idx, (_, name))) in m
            .online_categories
            .iter()
            .enumerate()
            .skip(start)
            .take(visible)
            .enumerate()
        {
            chip(
                f,
                hs,
                ix + j as f32 * (bw + 8.),
                y + 258.,
                bw,
                &elide(name, (bw / 14.).floor().max(1.) as usize),
                idx == m.online_category,
                if active {
                    Action::OnlineCategory(idx)
                } else {
                    Action::None
                },
                t,
            );
        }
        if m.online_categories.is_empty() {
            text(
                f,
                ix,
                y + 267.,
                14.,
                if active {
                    "分类加载中…"
                } else {
                    "登录后同步分类"
                },
                p.muted,
            );
        } else if m.online_categories.len() > cap {
            link(
                f,
                hs,
                ix + iw - 74.,
                y + 258.,
                74.,
                "更多 ›",
                if active {
                    Action::OnlineCategory((selected + 1) % m.online_categories.len())
                } else {
                    Action::None
                },
                t,
            );
        }
        text(f, ix, y + 304., 14., "难度", p.muted);
        let dw = ((iw - 40.) / 6.).min(100.);
        for (i, name) in ["淼", "水", "易", "普", "难", "虐"].into_iter().enumerate() {
            chip(
                f,
                hs,
                ix + i as f32 * (dw + 8.),
                y + 330.,
                dw,
                name,
                m.online_difficulty == i,
                if active {
                    Action::OnlineDifficultyTo(i)
                } else {
                    Action::None
                },
                t,
            );
        }
        field_shell(f, ix, y + 400., 100., "取文字数", t);
        if !active {
            text(
                f,
                ix + 10.,
                y + 410.,
                14.,
                cfg.seg_chars.to_string(),
                p.muted,
            );
        }
        toggle(
            f,
            hs,
            ix + 130.,
            y + 410.,
            m.online_strict,
            if active {
                Action::OnlineStrict
            } else {
                Action::None
            },
            t,
        );
        text(f, ix + 168., y + 410., 14., "精确字数", p.muted);
        toggle(
            f,
            hs,
            ix + 300.,
            y + 410.,
            m.field("online_auto_next") != "false",
            crate::aux_marker("online_auto_next"),
            t,
        );
        text(f, ix + 338., y + 410., 14., "自动下一段", p.muted);
        let aw = ((iw - 24.) / 3.).min(190.);
        for (i, (name, dir)) in [("上一段", -1), ("随机取文", 0), ("下一段", 1)]
            .into_iter()
            .enumerate()
        {
            action(
                f,
                hs,
                ix + i as f32 * (aw + 12.),
                y + 458.,
                aw,
                name,
                if active {
                    Action::OnlineFetch(dir)
                } else {
                    Action::None
                },
                t,
                dir == 0,
            );
        }
        let status_text = if m.online_busy {
            "正在取文，当前文章已保留…"
        } else if !m.online_status.is_empty() {
            &m.online_status
        } else if active {
            "自动下一段开启后，结算 1.5 秒后接续；赛制除外。"
        } else {
            "登录后可取文和浏览书架。"
        };
        let status_color = if m.online_status.contains("失败")
            || m.online_status.contains("过期")
            || m.online_status.contains("错误")
        {
            p.red
        } else {
            p.muted
        };
        clipped_text(f, ix, y + 512., iw, 14., status_text, status_color);
        // Shelf as a compact 3-up grid: the old vertical rows pushed the
        // second/third book under the fixed footer at 1280x800.
        card(f, ix, y + 548., iw, 108., t);
        text(
            f,
            ix + 16.,
            y + 562.,
            18.,
            format!("在线书架 · {} 本", m.online_shelf.len()),
            p.text2,
        );
        link(
            f,
            hs,
            ix + iw - 176.,
            y + 552.,
            70.,
            "刷新",
            if active {
                Action::OnlineBooks
            } else {
                Action::None
            },
            t,
        );
        if m.online_shelf.len() > 3 {
            link(
                f,
                hs,
                ix + iw - 94.,
                y + 552.,
                80.,
                "下一页 ›",
                Action::ShelfPage,
                t,
            );
        }
        if m.online_shelf.is_empty() {
            text(
                f,
                ix + 16.,
                y + 604.,
                14.,
                if active {
                    "书架为空，可先随机取文。"
                } else {
                    "登录后显示书架。"
                },
                p.muted,
            );
        } else {
            let offset = m.shelf_offset.min(m.online_shelf.len().saturating_sub(1));
            let book_w = ((iw - 48.) / 3.).max(120.);
            for (j, (idx, name)) in m
                .online_shelf
                .iter()
                .enumerate()
                .skip(offset)
                .take(3)
                .enumerate()
            {
                chip(
                    f,
                    hs,
                    ix + 16. + j as f32 * (book_w + 8.),
                    y + 596.,
                    book_w,
                    &elide(name, ((book_w - 16.) / 15.).max(4.) as usize),
                    false,
                    if active {
                        Action::OnlineShelf(idx)
                    } else {
                        Action::None
                    },
                    t,
                );
            }
        }
    }
    line(f, x + 16., y + ph - 62., pw - 32., p.line);
    link(
        f,
        hs,
        ix,
        y + ph - 49.,
        110.,
        "理论分析",
        crate::aux_marker("show_full_analysis"),
        t,
    );
    link(
        f,
        hs,
        ix + 128.,
        y + ph - 49.,
        110.,
        "键盘热力图",
        crate::aux_marker("show_heatmap"),
        t,
    );
    action(
        f,
        hs,
        x + pw - 132.,
        y + ph - 49.,
        108.,
        "返回跟打",
        Action::Close,
        t,
        true,
    );
}

fn rules_side(pw: f32) -> f32 {
    (pw * 0.27).clamp(210., 280.)
}

fn rules_geometry(pw: f32, _ph: f32) -> (f32, f32, f32, f32, f32, f32) {
    let side = rules_side(pw);
    let rw = pw - side - 32.;
    (side, rw, 16., 452., 274., (rw - 44.) / 2.)
}
fn result_paragraph(
    f: &mut Frame,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    source: &str,
    output: &str,
    t: &Theme,
) {
    let p = tokens(t);
    let a: Vec<char> = source.chars().collect();
    let b: Vec<char> = output.chars().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let changed_end = b.len().saturating_sub(suffix);
    let lines = wrap_lines(output, w, 14.);
    let mut at = 0usize;
    clip(f, x, y, w, h);
    for (i, line) in lines
        .iter()
        .take((h / 21.).ceil().max(0.) as usize)
        .enumerate()
    {
        let mut spans = Vec::new();
        let mut u16_at = 0u32;
        for ch in line.chars() {
            if at >= prefix && at < changed_end {
                spans.push((u16_at, ch.len_utf16() as u32, p.accent));
            }
            at += 1;
            u16_at += ch.len_utf16() as u32;
        }
        f.push(DrawCmd::StyledLine {
            x,
            y: y + i as f32 * 21.,
            w,
            size: 14.,
            text: line.clone(),
            spans,
            color: p.text2,
        });
        // A wrapped paragraph consumes LF as well; account for source line breaks
        // when locating the changed range without ever slicing UTF-8 by byte index.
        if b.get(at) == Some(&'\n') {
            at += 1;
        }
    }
    end_clip(f);
}

fn draw_rules(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    cfg: &Config,
    t: &Theme,
    x: f32,
    y: f32,
    pw: f32,
    ph: f32,
) {
    let p = tokens(t);
    overlay_header(
        f,
        hs,
        t,
        x,
        y,
        pw,
        "≡",
        "文本清洗",
        "保留原文；预览确认后再应用",
        "F9",
    );
    let (side, rw, _, test_y, test_h, cw) = rules_geometry(pw, ph);
    text(
        f,
        x + 16.,
        y + 84.,
        18.,
        format!("规则 · {} 条", m.rules.len()),
        p.text2,
    );
    link(
        f,
        hs,
        x + side - 90.,
        y + 76.,
        76.,
        "恢复内置",
        Action::RuleReset,
        t,
    );
    let offset = (m.scroll as usize).min(m.rules.len().saturating_sub(1));
    let list_rows = 8;
    for (j, (idx, (name, on, content, _, _))) in m
        .rules
        .iter()
        .enumerate()
        .skip(offset)
        .take(list_rows)
        .enumerate()
    {
        let yy = y + 126. + j as f32 * 62.;
        if idx == m.rule_selected {
            round(
                f,
                x + 10.,
                yy - 5.,
                side - 20.,
                54.,
                6.,
                p.surface2,
                Some(p.line),
            );
        }
        toggle(f, hs, x + 18., yy + 15., *on, Action::RuleToggle(idx), t);
        clipped_text(f, x + 58., yy + 5., side - 72., 14., name, p.text2);
        text(
            f,
            x + 58.,
            yy + 29.,
            14.,
            if *content {
                "内容级"
            } else if cfg
                .cleaning_preset
                .rules
                .get(idx)
                .is_some_and(|r| r.builtin)
            {
                "内置安全规则"
            } else {
                "自定义规则"
            },
            p.muted,
        );
        hs.push(Hit {
            x: x + 54.,
            y: yy - 4.,
            w: side - 66.,
            h: 54.,
            action: Action::RuleSelect(idx),
        });
    }
    let add_y = y + 126. + list_rows.min(m.rules.len().saturating_sub(offset)) as f32 * 62.;
    action(
        f,
        hs,
        x + 12.,
        add_y + 8.,
        side - 24.,
        "+ 新建规则",
        Action::RuleAdd,
        t,
        false,
    );
    let bw = (side - 40.) / 3.;
    for (i, (label, a)) in [
        ("上移", Action::RuleUp),
        ("下移", Action::RuleDown),
        ("删除", Action::RuleDelete),
    ]
    .into_iter()
    .enumerate()
    {
        chip(
            f,
            hs,
            x + 12. + i as f32 * (bw + 8.),
            y + 690.,
            bw,
            label,
            false,
            a,
            t,
        );
    }
    paragraph(
        f,
        x + 16.,
        y + 748.,
        side - 32.,
        78.,
        14.,
        24.,
        "从上到下执行，关闭的规则跳过。\n赛制仅允许可信内置安全规则。",
        p.muted,
    );
    let rx = x + side + 16.;
    card(f, rx, y + 82., rw, 354., t);
    text(f, rx + 16., y + 96., 18., "规则编辑", p.text2);
    for (yy, label) in [(148., "名称"), (216., "正则表达式"), (284., "替换文本")] {
        field_shell(f, rx + 16., y + yy, rw - 32., label, t);
    }
    clipped_text(
        f,
        rx + 16.,
        y + 328.,
        rw - 32.,
        14.,
        "捕获组替换用 $1；不支持反向引用与环视。",
        p.muted,
    );
    text(f, rx + 16., y + 360., 14., "范围", p.muted);
    chip(
        f,
        hs,
        rx + 58.,
        y + 354.,
        92.,
        "仅本次",
        !m.rule_save_preset,
        if m.rule_save_preset {
            Action::RuleScopeToggle
        } else {
            Action::None
        },
        t,
    );
    chip(
        f,
        hs,
        rx + 158.,
        y + 354.,
        106.,
        "保存预设",
        m.rule_save_preset,
        if m.rule_save_preset {
            Action::None
        } else {
            Action::RuleScopeToggle
        },
        t,
    );
    let safe = cfg
        .cleaning_preset
        .rules
        .get(m.rule_selected)
        .is_some_and(|r| r.safe);
    text(f, rx + 16., y + 402., 14., "级别", p.muted);
    chip(
        f,
        hs,
        rx + 58.,
        y + 396.,
        70.,
        "安全",
        safe,
        if safe {
            Action::None
        } else {
            Action::RuleContentToggle
        },
        t,
    );
    chip(
        f,
        hs,
        rx + 136.,
        y + 396.,
        82.,
        "内容级",
        !safe,
        if safe {
            Action::RuleContentToggle
        } else {
            Action::None
        },
        t,
    );
    action(
        f,
        hs,
        rx + rw - 122.,
        y + 88.,
        106.,
        "保存规则",
        Action::RuleSave,
        t,
        true,
    );
    let ty = y + test_y;
    card(f, rx, ty, rw, test_h, t);
    text(f, rx + 16., ty + 15., 18., "测试", p.text2);
    clipped_text(
        f,
        rx + 76.,
        ty + 18.,
        rw - 238.,
        14.,
        &format!("命中 {} 处", m.rule_hits),
        if m.rule_error.is_empty() {
            p.accent
        } else {
            p.red
        },
    );
    link(
        f,
        hs,
        rx + rw - 142.,
        ty + 5.,
        126.,
        "预览整组 →",
        Action::RuleTest,
        t,
    );
    let col_y = ty + 66.;
    let col_h = test_h - 96.;
    round(f, rx + 12., col_y, cw, col_h, 6., p.bg2, Some(p.line));
    round(f, rx + 32. + cw, col_y, cw, col_h, 6., p.bg2, Some(p.line));
    text(f, rx + 20., col_y + 12., 14., "原文 · 可编辑", p.muted);
    text(f, rx + 40. + cw, col_y + 12., 14., "替换后", p.muted);
    let available = !m.rule_test_source.is_empty() || !m.rule_preview.is_empty() || m.rule_hits > 0;
    if !m.rule_error.is_empty() {
        paragraph(
            f,
            rx + 40. + cw,
            col_y + 44.,
            cw - 16.,
            col_h - 52.,
            14.,
            24.,
            &m.rule_error,
            p.red,
        );
    } else if !available {
        paragraph(
            f,
            rx + 40. + cw,
            col_y + 44.,
            cw - 16.,
            col_h - 52.,
            14.,
            24.,
            "点击「预览整组」查看替换结果。",
            p.muted,
        );
    } else if m.rule_preview.is_empty() {
        paragraph(
            f,
            rx + 40. + cw,
            col_y + 44.,
            cw - 16.,
            col_h - 52.,
            14.,
            24.,
            "替换后为空文本。",
            p.accent,
        );
    } else {
        result_paragraph(
            f,
            rx + 40. + cw,
            col_y + 44.,
            cw - 16.,
            col_h - 52.,
            &m.rule_test_source,
            &m.rule_preview,
            t,
        );
    }
    clipped_text(
        f,
        rx + 16.,
        ty + test_h - 23.,
        rw - 32.,
        14.,
        if available && m.rule_test_source != m.field("rule_test") {
            "原文已改，请重新预览；预览不写回正文。"
        } else {
            "预览不改正文，明确应用后才生效。"
        },
        p.muted,
    );
    let qy = y + 738.;
    card(f, rx, qy, rw, 86., t);
    text(f, rx + 16., qy + 12., 18., "引号宽松比对", p.text2);
    toggle(
        f,
        hs,
        rx + rw - 48.,
        qy + 17.,
        m.quote_relaxed,
        Action::QuoteToggle,
        t,
    );
    let q = &cfg.session_rules.quotes;
    for (i, (label, on)) in [
        ("双引号", q.double),
        ("单引号", q.single),
        ("角括号", q.corner),
    ]
    .into_iter()
    .enumerate()
    {
        let xx = rx + 16. + i as f32 * ((rw - 32.) / 3.);
        toggle(
            f,
            hs,
            xx,
            qy + 56.,
            on && m.quote_relaxed,
            Action::QuoteClass(i as u8),
            t,
        );
        text(f, xx + 34., qy + 56., 14., label, p.muted);
    }
    line(f, x + 16., y + ph - 62., pw - 32., p.line);
    let buttons = [
        ("还原原文", Action::RuleRestore, false),
        ("导入", Action::RuleImport, false),
        ("导出", Action::RuleExport, false),
        ("应用到当前文", Action::RuleApply, true),
    ];
    let bw = ((pw - 72.) / 4.).min(180.);
    for (i, (label, a, primary)) in buttons.into_iter().enumerate() {
        action(
            f,
            hs,
            x + 24. + i as f32 * (bw + 8.),
            y + ph - 49.,
            bw,
            label,
            a,
            t,
            primary,
        );
    }
}

/// The native editors share the exact geometry of their drawn field shells.
/// The outer panel transforms and clips these fields with the same body scroll.
pub fn fields(m: &Model, x: f32, y: f32, pw: f32, ph: f32) -> Vec<FieldSpec> {
    let mut out = Vec::new();
    let mut add = |id, x, y, w, h, multi, password| {
        out.push(FieldSpec {
            id,
            x,
            y,
            w,
            h,
            multi,
            password,
        })
    };
    match m.modal {
        Modal::Online if login_visible(m) => {
            let (fx, fw) = online_form(pw);
            let fx = x + fx;
            add("username", fx, y + 154., fw, 36., false, false);
            add("password", fx, y + 226., fw, 36., false, true);
            if m.online_register_mode {
                add("reg_email", fx, y + 298., fw, 36., false, false);
                add("reg_code", fx, y + 370., fw - 152., 36., false, false);
            } else {
                add("server", fx, y + 374., fw, 36., false, false);
            }
        }
        Modal::Online if m.online_authenticated && !m.online_expired => {
            add("online_chars", x + 24., y + 400., 100., 36., false, false);
        }
        Modal::Rules => {
            let (side, rw, _, test_y, test_h, cw) = rules_geometry(pw, ph);
            let rx = x + side + 16.;
            for (id, yy) in [
                ("rule_name", 148.),
                ("rule_pattern", 216.),
                ("rule_replacement", 284.),
            ] {
                add(id, rx + 16., y + yy, rw - 32., 36., false, false);
            }
            add(
                "rule_test",
                rx + 20.,
                y + test_y + 110.,
                cw - 16.,
                test_h - 148.,
                true,
                false,
            );
        }
        _ => {}
    }
    out
}
pub fn draw(
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
    match m.modal {
        Modal::Report => draw_report(f, hs, m, sess, t, x, y, pw, ph),
        Modal::Online => draw_online(f, hs, m, cfg, t, x, y, pw, ph),
        Modal::Rules => draw_rules(f, hs, m, cfg, t, x, y, pw, ph),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeline_uses_elapsed_windows_and_subtracts_backspace() {
        let events = [
            TimingEvent {
                len_delta: 10,
                dt_ms: 1000,
            },
            TimingEvent {
                len_delta: -2,
                dt_ms: 1000,
            },
            TimingEvent {
                len_delta: 5,
                dt_ms: 4000,
            },
        ];
        let (step, samples) = speed_samples(&events, 10000);
        assert_eq!(step, 5000);
        assert_eq!(samples, vec![96., 60.]);
        assert_eq!(speed_samples(&events, 0).1.len(), 0);
    }
    #[test]
    fn long_timeline_is_bounded_and_keeps_idle_windows() {
        let (_, samples) = speed_samples(
            &[TimingEvent {
                len_delta: 1,
                dt_ms: 1,
            }],
            86_400_000,
        );
        assert!(samples.len() <= 200);
        assert_eq!(samples.iter().filter(|s| **s > 0.).count(), 1);
    }
    #[test]
    fn cleaning_fields_fit_both_comparison_columns() {
        for pw in [640., 880., 1040.] {
            let mut m = Model::default();
            m.modal = Modal::Rules;
            let fs = fields(&m, 0., 0., pw, 900.);
            let raw = fs.iter().find(|z| z.id == "rule_test").unwrap();
            let (side, rw, _, _, _, cw) = rules_geometry(pw, 900.);
            assert!(raw.x >= side && raw.x + raw.w <= side + 16. + 12. + cw);
            assert!(raw.w > 100. && rw > raw.w * 2.);
            assert!(raw.y + raw.h < 726.);
            assert!(fs.iter().all(|field| field.w > 100. && field.h >= 34.));
        }
    }

    #[test]
    fn register_fields_leave_room_for_send_code_and_cooldown_disables_it() {
        let cfg = Config::default();
        let t = crate::ui::theme("墨玉");
        for pw in [640., 880.] {
            let mut m = Model::default();
            m.modal = Modal::Online;
            m.online_login_expanded = true;
            m.online_register_mode = true;
            let fs = fields(&m, 0., 0., pw, 760.);
            let ids: Vec<_> = fs.iter().map(|z| z.id).collect();
            assert_eq!(ids, vec!["username", "password", "reg_email", "reg_code"]);
            assert!(fs
                .iter()
                .all(|field| field.h >= 34. && field.w >= 150. && !field.multi));
            let code = fs.iter().find(|z| z.id == "reg_code").unwrap();
            let mut f = Frame::new(pw, 760.);
            let mut hs = Vec::new();
            draw_online(&mut f, &mut hs, &m, &cfg, &t, 0., 0., pw, 760.);
            let send = hs
                .iter()
                .find(|h| h.action == crate::aux_marker("online_reg_code"))
                .unwrap();
            assert!(code.x + code.w + 8. <= send.x);
            assert_eq!(code.y, send.y);
            assert!(hs
                .iter()
                .any(|h| h.action == crate::aux_marker("online_reg_submit")));
            assert!(hs
                .iter()
                .any(|h| h.action == crate::aux_marker("online_register")));
            m.online_code_wait = 59;
            hs.clear();
            draw_online(
                &mut Frame::new(pw, 760.),
                &mut hs,
                &m,
                &cfg,
                &t,
                0.,
                0.,
                pw,
                760.,
            );
            assert!(!hs
                .iter()
                .any(|h| h.action == crate::aux_marker("online_reg_code")));
        }
    }

    #[test]
    fn authenticated_online_page_hides_credentials_and_never_draws_article_body() {
        let cfg = Config::default();
        let t = crate::ui::theme("墨玉");
        let mut m = Model::default();
        m.modal = Modal::Online;
        m.online_login_expanded = true;
        let ids: Vec<_> = fields(&m, 0., 0., 880., 900.)
            .iter()
            .map(|z| z.id)
            .collect();
        assert_eq!(ids, vec!["username", "password", "server"]);
        let mut f = Frame::new(880., 900.);
        let mut hs = Vec::new();
        draw_online(&mut f, &mut hs, &m, &cfg, &t, 0., 0., 880., 900.);
        assert!(hs
            .iter()
            .any(|h| h.action == crate::aux_marker("online_register")));
        assert!(hs.iter().any(|h| h.action == Action::OnlineLogin));

        m.online_authenticated = true;
        m.online_preview = "正文哨兵：不能出现在在线面板中".into();
        let fs = fields(&m, 0., 0., 880., 900.);
        assert_eq!(fs.len(), 1);
        assert_eq!(fs[0].id, "online_chars");
        f.cmds.clear();
        hs.clear();
        draw_online(&mut f, &mut hs, &m, &cfg, &t, 0., 0., 880., 900.);
        assert!(!hs.iter().any(|h| h.action == Action::OnlineLoad));
        assert!(hs.iter().any(|h| h.action == Action::OnlineLogout));
        assert!(!f.cmds.iter().any(|c| matches!(c,
            DrawCmd::TextRun { text, .. } if text.contains("正文哨兵")
        )));
    }

    #[test]
    fn selecting_active_scope_does_not_toggle_to_persistent_storage() {
        let cfg = Config::default();
        let t = crate::ui::theme("墨玉");
        for persistent in [false, true] {
            let mut m = Model::default();
            m.modal = Modal::Rules;
            m.rule_save_preset = persistent;
            let mut f = Frame::new(1040., 900.);
            let mut hs = Vec::new();
            draw_rules(&mut f, &mut hs, &m, &cfg, &t, 0., 0., 1040., 900.);
            let rx = rules_side(1040.) + 16.;
            let local = crate::modals::hit(&hs, rx + 80., 372.);
            let saved = crate::modals::hit(&hs, rx + 200., 372.);
            assert_eq!(
                local,
                if persistent {
                    Some(Action::RuleScopeToggle)
                } else {
                    None
                }
            );
            assert_eq!(
                saved,
                if persistent {
                    None
                } else {
                    Some(Action::RuleScopeToggle)
                }
            );
        }
    }

    #[test]
    fn secondary_controls_are_readable_in_both_themes() {
        let cfg = Config::default();
        for name in ["墨玉", "宣纸"] {
            let t = crate::ui::theme(name);
            for modal in [Modal::Online, Modal::Rules] {
                let mut m = Model::default();
                m.modal = modal;
                let mut f = Frame::new(880., 900.);
                let mut hs = Vec::new();
                draw(
                    &mut f,
                    &mut hs,
                    &m,
                    &Session::new(100),
                    &cfg,
                    &t,
                    0.,
                    0.,
                    880.,
                    900.,
                );
                for cmd in &f.cmds {
                    match cmd {
                        DrawCmd::TextRun { h, text, .. } => {
                            assert!(*h >= 14., "{modal:?}: {text}");
                            assert!(!text.contains("\\n"), "literal newline escape: {text}");
                        }
                        DrawCmd::StyledLine { size, .. } => assert!(*size >= 14.),
                        _ => {}
                    }
                }
                assert!(hs.iter().all(|hit| hit.h >= 34.));
                let primary_label = if modal == Modal::Online {
                    "随机取文"
                } else {
                    "应用到当前文"
                };
                assert!(f.cmds.iter().any(|cmd| matches!(cmd,
                    DrawCmd::TextRun { text, color, .. }
                        if text == primary_label && *color == tokens(&t).bg
                )));
            }
        }
    }
    #[test]
    fn slow_estimates_share_commit_time_and_follow_backspace_position() {
        let mut sess = Session::new(100);
        sess.comparer = gale_core::compare::Comparer::new("甲乙丙");
        sess.comparer.timing_events = vec![
            TimingEvent {
                len_delta: 2,
                dt_ms: 2000,
            },
            TimingEvent {
                len_delta: -1,
                dt_ms: 1000,
            },
            TimingEvent {
                len_delta: 1,
                dt_ms: 3000,
            },
            TimingEvent {
                len_delta: 1,
                dt_ms: 1000,
            },
        ];
        let slow = slow_estimates(&sess);
        assert_eq!(slow[0].ch, '乙');
        assert_eq!(slow[0].count, 2);
        assert_eq!(slow[0].average_ms, 2000.);
        assert_eq!(
            slow.iter().find(|z| z.ch == '甲').unwrap().average_ms,
            1000.
        );
    }

    #[test]
    fn two_wrong_characters_show_text_accuracy_separately_from_pro_key_efficiency() {
        let mut sess = Session::new(100);
        sess.load_book("测试", "你好");
        sess.on_key(0x4A, 1000);
        sess.feed_input(
            gale_core::compare::InputEvent::Commit { chars: "j".into() },
            1001,
        );
        sess.on_key(0x4A, 2000);
        sess.feed_input(
            gale_core::compare::InputEvent::Commit { chars: "j".into() },
            2001,
        );
        let result = sess.finalize_current_at(2002, 1).unwrap();
        assert_eq!(result.keys, 2);
        assert_eq!(result.wrong, 2);
        assert_eq!(result.accuracy, 100.);
        assert_eq!(result.character_accuracy(), 0.);
        assert_eq!(result.effective_speed, 0.);
        let mut m = Model::default();
        m.latest = Some(result);
        // Older records have no word-rate provenance; preserve that absence
        // while still showing the stored numeric retry count.
        let legacy = m.latest.as_mut().unwrap();
        legacy.word_rate = None;
        legacy.retype_count = 2;
        let mut f = Frame::new(1280., 800.);
        draw_report(
            &mut f,
            &mut Vec::new(),
            &m,
            &sess,
            &crate::ui::theme("墨玉"),
            0.,
            55.,
            1280.,
            708.,
        );
        let text: Vec<_> = f
            .cmds
            .iter()
            .filter_map(|c| match c {
                DrawCmd::TextRun { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(f
            .cmds
            .iter()
            .all(|cmd| !matches!(cmd, DrawCmd::TextRun { h, .. } if *h < 14.)));
        assert!(text.iter().any(|s| s.contains("字准")));
        assert!(text.iter().any(|s| s.contains("键准")));
        assert_eq!(m.latest.as_ref().unwrap().accuracy, 100.);
        m.fields.insert("report_tab".into(), "session".into());
        let mut report = Frame::new(1280., 800.);
        draw_report(
            &mut report,
            &mut Vec::new(),
            &m,
            &sess,
            &crate::ui::theme("墨玉"),
            0.,
            55.,
            1280.,
            708.,
        );
        let report_text: Vec<_> = report
            .cmds
            .iter()
            .filter_map(|c| match c {
                DrawCmd::TextRun { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(report
            .cmds
            .iter()
            .all(|cmd| !matches!(cmd, DrawCmd::TextRun { h, .. } if *h < 14.)));
        assert!(report_text.contains(&"字准"));
        assert!(report_text.contains(&"键准·按键效率"));
        assert!(report_text.iter().any(|s| s.contains("100.0/0.0")));
        assert_eq!(sess.aggregate().accuracy, 100.);
    }

    #[test]
    fn historical_detail_uses_the_selected_record_and_never_current_key_distribution() {
        let mut sess = Session::new(100);
        sess.load_book("当前文章", "甲");
        sess.on_key(65, 10);
        // Production separately drains the host's physical-key queue into this
        // histogram; on_key only updates the score's statistical key counters.
        *sess.vk_histogram.entry(0x41).or_insert(0) += 1;
        sess.feed_input(
            gale_core::compare::InputEvent::Commit {
                chars: "甲".into()
            },
            20,
        );
        let current = sess.finalize_current_at(30, 2).unwrap();
        assert!(!sess.last_vk_histogram.is_empty());
        let mut older = current.clone();
        older.title = "历史文章".into();
        older.ts = 1;
        older.speed = 123.4;
        let mut m = Model::default();
        m.modal = Modal::Report;
        m.history = vec![older, current.clone()];
        m.latest = Some(current);
        m.selected_history = Some(0);
        let mut f = Frame::new(1280., 800.);
        let mut hits = Vec::new();
        draw_report(
            &mut f,
            &mut hits,
            &m,
            &sess,
            &crate::ui::theme("墨玉"),
            0.,
            108.,
            1280.,
            662.,
        );
        assert_eq!(selected_report_record(&m).unwrap().title, "历史文章");
        let detail_titles: Vec<_> = f
            .cmds
            .iter()
            .filter_map(|cmd| match cmd {
                DrawCmd::TextRun { x, text, .. } if *x > 478. => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(detail_titles.contains(&"历史文章"));
        assert!(!detail_titles.contains(&"当前文章"));
        assert!(!hits
            .iter()
            .any(|hit| hit.action == crate::aux_marker("show_heatmap")));
        assert_eq!(m.latest.as_ref().unwrap().title, "当前文章");
        for width in [800., 640.] {
            let viewport = 462.;
            m.panel_scroll = history_detail_scroll(&m, &sess, width, viewport);
            let mut frame = Frame::new(width, 600.);
            let mut hits = Vec::new();
            draw_report(
                &mut frame,
                &mut hits,
                &m,
                &sess,
                &crate::ui::theme("墨玉"),
                0.,
                108.,
                width,
                viewport,
            );
            let detail_title = frame
                .cmds
                .iter()
                .find_map(|cmd| match cmd {
                    DrawCmd::TextRun { text, x, y, h, .. }
                        if text == "历史文章" && *h >= 20. && (width < 768. || *x > 300.) =>
                    {
                        Some(*y)
                    }
                    _ => None,
                })
                .unwrap();
            assert!(
                detail_title >= 108. && detail_title < 108. + viewport - 62.,
                "selected detail should be visible at width {width}"
            );
            if width == 800. {
                assert_eq!(
                    m.panel_scroll, 0.,
                    "800px report keeps history beside detail"
                );
            } else {
                assert!(m.panel_scroll > 0.);
                assert!(hits
                    .iter()
                    .any(|hit| hit.action == crate::aux_marker("report_history")
                        && hit.y > 108. + viewport - 62.
                        && hit.h > 0.));
            }
        }
    }

    #[test]
    fn empty_report_is_compact_and_has_no_disabled_score_actions() {
        let mut m = Model::default();
        m.modal = Modal::Report;
        let sess = Session::new(100);
        assert!(report_content_height(&m, &sess, 1280.) < 400.);
        let mut f = Frame::new(1280., 800.);
        let mut hits = Vec::new();
        draw_report(
            &mut f,
            &mut hits,
            &m,
            &sess,
            &crate::ui::theme("墨玉"),
            0.,
            108.,
            1280.,
            662.,
        );
        assert!(!hits
            .iter()
            .any(|hit| matches!(hit.action, Action::CopyScore | Action::ExportImage)));
        assert!(f
            .cmds
            .iter()
            .any(|cmd| matches!(cmd, DrawCmd::TextRun { text, .. } if text == "还没有成绩记录")));
    }

    fn pagination_record() -> ScoreRecord {
        let mut sess = Session::new(100);
        sess.load_book("分页测试", "甲");
        sess.on_key(0x41, 10);
        sess.feed_input(
            gale_core::compare::InputEvent::Commit {
                chars: "甲".into()
            },
            20,
        );
        sess.finalize_current_at(30, 1).unwrap()
    }

    #[test]
    fn report_history_last_page_reaches_the_oldest_record_at_each_width() {
        let sess = Session::new(100);
        let record = pagination_record();
        for (width, total, visible, cap) in [(640., 4, 3, 1), (1280., 14, 6, 8)] {
            let mut m = Model::default();
            m.modal = Modal::Report;
            m.history = (0..total)
                .map(|i| {
                    let mut item = record.clone();
                    item.title = format!("历史记录 {i}");
                    item.ts = i as u64;
                    item
                })
                .collect();
            assert_eq!(report_visible_rows(&m, width), visible);
            assert_eq!(report_scroll_cap(&m, &sess, width), cap);
            m.scroll = cap as f32;
            let mut frame = Frame::new(width, 1038.);
            let mut hits = Vec::new();
            draw_report(
                &mut frame,
                &mut hits,
                &m,
                &sess,
                &crate::ui::theme("墨玉"),
                0.,
                108.,
                width,
                900.,
            );
            let visible_records: Vec<_> = hits
                .iter()
                .filter_map(|hit| match hit.action {
                    Action::SelectHistory(index) if hit.h > 0. => Some(index),
                    _ => None,
                })
                .collect();
            assert_eq!(visible_records, (0..visible).rev().collect::<Vec<_>>());
        }
    }

    #[test]
    fn report_summary_paginates_completed_records_instead_of_unfinished_segments() {
        let record = pagination_record();
        let mut sess = Session::new(100);
        sess.load_book("分页测试", "甲");
        sess.segments = vec![sess.current_segment().clone(); 12];
        sess.results = (0..12)
            .map(|index| (index % 2 == 0).then(|| record.clone()))
            .collect();
        let mut m = Model::default();
        m.modal = Modal::Report;
        m.fields.insert("report_tab".into(), "session".into());
        assert_eq!(report_visible_rows(&m, 640.), 3);
        assert_eq!(report_scroll_cap(&m, &sess, 640.), 3);
        assert_eq!(report_visible_rows(&m, 1280.), 5);
        assert_eq!(report_scroll_cap(&m, &sess, 1280.), 1);
        m.scroll = 1.;
        let mut frame = Frame::new(1280., 1466.);
        draw_report(
            &mut frame,
            &mut Vec::new(),
            &m,
            &sess,
            &crate::ui::theme("墨玉"),
            0.,
            108.,
            1280.,
            1328.,
        );
        let details: Vec<_> = frame
            .cmds
            .iter()
            .filter_map(|cmd| match cmd {
                DrawCmd::TextRun { text, .. } if text.contains("· 已结算") => {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(details.len(), 5);
        assert!(details.iter().any(|text| text.starts_with("第 11 段 ·")));
        assert!(!details.iter().any(|text| text.starts_with("第 1 段 ·")));
    }
}
