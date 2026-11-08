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
    round(f, x, y, 25., 19., 4., p.surface, Some(p.line2));
    mono(f, x + 5., y + 3., 8.5, s, p.muted);
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
        32.,
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
        y + 7.,
        w - 20.,
        11.,
        s,
        if primary { p.bg } else { p.text2 },
    );
    if a != Action::None {
        hs.push(Hit {
            x,
            y,
            w,
            h: 32.,
            action: a,
        });
    }
}

fn link(f: &mut Frame, hs: &mut Vec<Hit>, x: f32, y: f32, w: f32, s: &str, a: Action, t: &Theme) {
    let p = tokens(t);
    clipped_text(f, x, y + 4., w, 9., s, p.accent);
    if a != Action::None {
        hs.push(Hit {
            x,
            y,
            w,
            h: 24.,
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
        20.,
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
        y + 4.,
        w - 12.,
        8.5,
        s,
        if selected { p.accent } else { p.muted },
    );
    if a != Action::None {
        hs.push(Hit {
            x,
            y,
            w,
            h: 20.,
            action: a,
        });
    }
}

fn badge(f: &mut Frame, x: f32, y: f32, w: f32, s: &str, c: Ink) {
    round(f, x, y, w, 14., 3., alpha(c, 0.06), Some(alpha(c, 0.3)));
    text(f, x + 4., y + 2., 7., s, c);
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
            y: y - 4.,
            w: 32.,
            h: 23.,
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

fn segment_stamp(index: usize) -> String {
    let n = index.saturating_add(1);
    match n {
        1..=10 => format!(
            "{}段",
            ["一", "二", "三", "四", "五", "六", "七", "八", "九", "十"][n - 1]
        ),
        _ => format!("{n}段"),
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

fn main_area(
    f: &mut Frame,
    m: &Model,
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
    clip(f, x, y, pw, (ph - 54.).max(1.));
    (cx, y - m.panel_scroll.clamp(0., (650. - ph).max(0.)), cw)
}

fn stamp(f: &mut Frame, x: f32, y: f32, size: f32, mark: &str, complete: bool, t: &Theme) {
    let p = tokens(t);
    let points = [
        (x, y + size * 0.09),
        (x + size * 0.91, y),
        (x + size, y + size * 0.91),
        (x + size * 0.09, y + size),
    ];
    for i in 0..4 {
        stroke(f, points[i], points[(i + 1) % 4], p.red);
    }
    let inset = 6.;
    let inner = [
        (x + inset, y + inset + size * 0.09),
        (x + size * 0.91 - inset, y + inset),
        (x + size - inset, y + size * 0.91 - inset),
        (x + inset + size * 0.09, y + size - inset),
    ];
    for i in 0..4 {
        stroke(f, inner[i], inner[(i + 1) % 4], alpha(p.red, 0.65));
    }
    body_text(
        f,
        x + size * 0.23,
        y + size * 0.3,
        size * 0.25,
        elide(mark, 3),
        p.red,
    );
    text(
        f,
        x + size * 0.42,
        y + size * 0.66,
        8.,
        if complete { "完" } else { "止" },
        p.red,
    );
}

fn draw_result(
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
    let (cx, by, cw) = main_area(f, m, t, x, y, pw, ph);
    let Some(r) = &m.latest else {
        body_text(f, cx, by + 116., 28., "这一段，等待落笔", p.text);
        text(
            f,
            cx,
            by + 169.,
            12.,
            "还没有完成任何一段。载入一篇文字，按下首键开始。",
            p.muted,
        );
        end_clip(f);
        action(
            f,
            hs,
            cx,
            y + ph - 42.,
            118.,
            "载入文字  F2",
            Action::Open(Modal::Load),
            t,
            true,
        );
        return;
    };
    let complete = r.round.as_ref().is_none_or(|z| z.complete);
    let race = r
        .round
        .as_ref()
        .map_or(m.race, |z| z.mode == gale_core::rules::TrainingMode::Race);
    clipped_text(
        f,
        cx,
        by + 26.,
        cw * 0.72,
        9.,
        &format!(
            "单段结算  ─  {} · {}",
            segment_caption(r.para_index, &r.mark),
            r.title
        ),
        p.muted,
    );
    right(
        f,
        cx + cw,
        by + 26.,
        8.5,
        record_time(r),
        alpha(p.muted, 0.65),
    );
    let hero = format!("{:.1}", r.speed);
    let base_size = if cw > 700. { 92. } else { 68. };
    let big = base_size * (5. / hero.chars().count().max(5) as f32);
    mono(f, cx, by + 58. + (base_size - big) * 0.9, big, hero, p.text);
    text(
        f,
        cx + if cw > 700. { 258. } else { 194. },
        by + 126.,
        11.,
        "字/分",
        p.muted,
    );
    if cw > 640. {
        let ex = cx + if cw > 800. { 350. } else { 310. };
        rect(f, ex - 30., by + 87., 1., 65., p.line);
        text(f, ex, by + 87., 8., "有 效 速 度", p.muted);
        mono(
            f,
            ex,
            by + 105.,
            32.,
            format!("{:.1}", r.effective_speed),
            p.text2,
        );
        text(
            f,
            ex,
            by + 143.,
            8.,
            "错一罚五 · 下限为 0",
            alpha(p.muted, 0.75),
        );
        text(
            f,
            ex,
            by + 160.,
            8.5,
            format!("字准 {:.1}% · 未改错字 {}", r.character_accuracy(), r.wrong),
            if r.wrong > 0 { p.red } else { p.muted },
        );
    } else {
        text(
            f,
            cx,
            by + 145.,
            9.,
            format!("有效速度 {:.1} 字/分 · 错一罚五", r.effective_speed),
            p.muted,
        );
    }
    let rank = 1 + sess
        .results
        .iter()
        .flatten()
        .filter(|a| a.speed > r.speed)
        .count();
    let compared = sess.results.iter().flatten().count();
    let previous = r
        .para_index
        .checked_sub(1)
        .and_then(|i| sess.results.get(i))
        .and_then(Option::as_ref);
    let delta = previous.map(|a| format!("{:+.1} 较上一段", r.speed - a.speed));
    text(
        f,
        cx,
        by + if cw > 640. { 157. } else { 168. },
        9.,
        match delta {
            Some(d) => format!("{d}  ·  已结算段第 {rank} 快"),
            None if compared > 1 => format!("已结算 {compared} 段 · 本段第 {rank} 快"),
            None => "本场首个已结算段".into(),
        },
        if previous.is_some_and(|a| r.speed >= a.speed) {
            p.green
        } else {
            p.muted
        },
    );
    if cw > 460. {
        let sx = cx + cw - 111.;
        stamp(
            f,
            sx,
            by + 54.,
            102.,
            &segment_stamp(r.para_index),
            complete,
            t,
        );
        // P2 赛制标记（任务 6a）：core score::race_badge 单点取用（快照
        // RoundMetadata 优先；旧记录无快照回退 m），禁止手写 “赛制/练习” 散点。
        let badge_txt = r
            .round
            .as_ref()
            .map(|z| race_badge(z.mode, z.word_hints))
            .unwrap_or_else(|| {
                race_badge(
                    if race {
                        TrainingMode::Race
                    } else {
                        TrainingMode::Practice
                    },
                    m.hints && !race,
                )
            });
        text(f, sx + 9., by + 163., 8., badge_txt, p.muted);
        text(
            f,
            sx + 9.,
            by + 176.,
            7.5,
            if complete { "已结算" } else { "中途停止" },
            p.muted,
        );
    }
    let sy = by + 196.;
    line(f, cx, sy, cw, p.line);
    line(f, cx, sy + 65., cw, p.line);
    text(f, cx, sy + 19., 9., "本段速度曲线", p.muted);
    let has_trace = live_trace(sess, r);
    let (step, samples) = if has_trace {
        speed_samples(&sess.comparer.timing_events, r.time_ms)
    } else {
        (5000, Vec::new())
    };
    if samples.is_empty() {
        text(
            f,
            cx + 181.,
            sy + 27.,
            9.,
            "本条记录未保留上屏时间事件",
            alpha(p.muted, 0.75),
        );
    } else {
        let peak = samples.iter().copied().fold(0., f64::max);
        text(
            f,
            cx,
            sy + 36.,
            7.5,
            format!("{} 秒分窗 · 峰值 {:.1}", step / 1000, peak),
            alpha(p.muted, 0.7),
        );
        let gx = cx + if cw > 600. { 181. } else { 135. };
        let gw = cw - (gx - cx);
        let points: Vec<_> = samples
            .iter()
            .enumerate()
            .map(|(i, s)| {
                (
                    gx + i as f32 / samples.len().saturating_sub(1).max(1) as f32 * (gw - 2.),
                    sy + 55. - (*s / peak.max(1.)) as f32 * 41.,
                )
            })
            .collect();
        for pair in points.windows(2) {
            let dx = (pair[1].0 - pair[0].0).max(1.);
            rect(
                f,
                pair[0].0,
                pair[0].1,
                dx,
                sy + 58. - pair[0].1,
                alpha(p.accent, 0.035),
            );
            stroke(f, pair[0], pair[1], p.accent);
        }
        if let Some(&(px, py)) = points.last() {
            round(f, px - 2., py - 2., 4., 4., 2., p.accent, None);
        }
    }
    let stats = [
        (format!("{:.2}", r.strokes_per_sec), "击键 / 秒"),
        (format!("{:.1}%", r.accuracy), "键准 · 按键效率"),
        (format!("{:.2}", r.code_len), "码长"),
        (r.wrong.to_string(), "错字"),
        (duration(r.time_ms), "用时"),
        (r.backspaces.to_string(), "退格"),
        // P1-3 选重/标顶（任务 2）：字段同步进结算面板（账=20ms 配对）。
        (r.select_count.to_string(), "选重"),
        (r.ding_count.to_string(), "标顶"),
        (r.corrections.to_string(), "回改"),
        (r.chars.to_string(), "本段字数"),
    ];
    let cols = if cw > 720. { 8 } else { 4 };
    let gy = sy + 94.;
    for (i, (value, name)) in stats.iter().enumerate() {
        let cellw = cw / cols as f32;
        let xx = cx + (i % cols) as f32 * cellw;
        let yy = gy + (i / cols) as f32 * 57.;
        if i % cols > 0 {
            rect(f, xx - 15., yy + 2., 1., 35., p.line);
        }
        mono(f, xx, yy, if cw > 720. { 20. } else { 18. }, value, p.text2);
        text(f, xx, yy + 26., 8., *name, p.muted);
    }
    // 10 格：宽版 2 行 / 窄版 3 行（原 8 格各多出一行，下方卡片整体下移一行）。
    let extra_y = gy + if cols == 8 { 123. } else { 176. };
    line(f, cx, extra_y - 17., cw, p.line);
    clipped_text(
        f,
        cx,
        extra_y - 12.,
        cw,
        7.5,
        &format!(
            "字准 {:.1}% 按未改错字计算；键准采用晴 Pro 按键效率口径，不扣未改错字。",
            r.character_accuracy()
        ),
        if r.wrong > 0 { p.red } else { p.muted },
    );
    let slipw = (cw - 11.) * 0.575;
    card(f, cx, extra_y, slipw, 116., t);
    text(f, cx + 13., extra_y + 10., 9., "成绩单预览", p.text2);
    right(
        f,
        cx + slipw - 13.,
        extra_y + 11.,
        7.5,
        "复制后手动发群",
        p.muted,
    );
    let preview = [
        format!(
            "虎符跟打 · 单段结算（{}）",
            r.round
                .as_ref()
                .map(|z| race_badge(z.mode, z.word_hints))
                .unwrap_or_else(|| {
                    race_badge(
                        if race {
                            TrainingMode::Race
                        } else {
                            TrainingMode::Practice
                        },
                        m.hints && !race,
                    )
                })
        ),
        format!(
            "{} · {} / 共 {} 段 · {} 字",
            r.title,
            segment_caption(r.para_index, &r.mark),
            sess.total_segments(),
            r.chars
        ),
        format!(
            "速度 {:.1} · 有效 {:.1} · 键准 {:.1}% · 字准 {:.1}%",
            r.speed,
            r.effective_speed,
            r.accuracy,
            r.character_accuracy()
        ),
        format!(
            "击键 {:.2} · 码长 {:.2} · 用时 {} · 错字 {}",
            r.strokes_per_sec,
            r.code_len,
            duration(r.time_ms),
            r.wrong
        ),
    ];
    clip(f, cx + 13., extra_y + 28., slipw - 26., 82.);
    for (i, s) in preview.iter().enumerate() {
        mono(
            f,
            cx + 13.,
            extra_y + 31. + i as f32 * 17.,
            8.5,
            s,
            if i == 0 { p.text2 } else { p.muted },
        );
    }
    end_clip(f);
    let ex = cx + slipw + 11.;
    let ew = cw - slipw - 11.;
    card(f, ex, extra_y, ew, 116., t);
    text(f, ex + 13., extra_y + 10., 9., "本段留痕", p.text2);
    right(f, ex + ew - 13., extra_y + 11., 7.5, "错字与回改", p.muted);
    let mismatch = if has_trace {
        let target: Vec<char> = sess.comparer.text.chars().collect();
        sess.comparer
            .states
            .iter()
            .enumerate()
            .find_map(|(i, state)| {
                if *state == CharState::Wrong {
                    target
                        .get(i)
                        .zip(sess.comparer.typed.get(i))
                        .map(|(a, b)| format!("原「{a}」 → 实输「{b}」"))
                } else {
                    None
                }
            })
    } else {
        None
    };
    let evidence = [
        (
            r.wrong.to_string(),
            mismatch.unwrap_or_else(|| {
                if r.wrong == 0 {
                    "没有未改错字".into()
                } else {
                    "未保留逐字错字明细".into()
                }
            }),
        ),
        (r.backspaces.to_string(), "组字退格及取消编码折算".into()),
        (r.corrections.to_string(), "删除已上屏字符的回改次数".into()),
    ];
    for (i, (value, label)) in evidence.iter().enumerate() {
        let yy = extra_y + 29. + i as f32 * 26.;
        line(f, ex + 13., yy, ew - 26., p.line);
        mono(
            f,
            ex + 13.,
            yy + 5.,
            13.,
            value,
            if i == 0 && r.wrong > 0 {
                p.red
            } else {
                p.text2
            },
        );
        clipped_text(f, ex + 39., yy + 6., ew - 52., 8.5, label, p.muted);
    }
    let py = extra_y + 130.;
    // P1-4 LiveStats 接线（任务 5）：整场口径（已结算 ≥5 字硬门 + 当前段实时）
    // 与“本段”统计分列显示——卡片加高一档放整场实时行。
    card(f, cx, py, cw, 68., t);
    text(f, cx + 13., py + 8., 8.5, "整场位置", p.muted);
    let live = sess.live_stats_now();
    let done = sess
        .results
        .iter()
        .flatten()
        .filter(|z| z.round.as_ref().is_none_or(|v| v.complete))
        .count();
    right(
        f,
        cx + cw - 13.,
        py + 8.,
        7.5,
        format!(
            "第 {} / {} 段 · 已完成 {} 段",
            r.para_index + 1,
            sess.total_segments(),
            done
        ),
        p.muted,
    );
    let visible = sess.total_segments().min(24).max(1);
    let start = r
        .para_index
        .saturating_sub(visible.saturating_sub(1))
        .min(sess.total_segments().saturating_sub(visible));
    let cellw = (cw - 26.) / visible as f32;
    for i in 0..visible {
        let idx = start + i;
        let xx = cx + 13. + i as f32 * cellw;
        let item = sess.results.get(idx).and_then(Option::as_ref);
        rect(
            f,
            xx,
            py + 28.,
            cellw - 4.,
            3.,
            if idx == r.para_index {
                p.accent
            } else if item.is_some() {
                alpha(p.accent, 0.4)
            } else {
                p.line2
            },
        );
        if visible <= 8 {
            mono(
                f,
                xx,
                py + 37.,
                7.5,
                match item {
                    Some(z) => format!("{} · {:.1}", idx + 1, z.speed),
                    None => format!("{} · —", idx + 1),
                },
                if idx == r.para_index {
                    p.accent
                } else {
                    p.muted
                },
            );
        }
    }
    // P1-4 整场实时行（任务 5）：LiveStats 整场口径 = 已结算段（≥5 硬门）+
    // 当前段进行中——与本面板上方“本段”统计分列。
    clipped_text(
        f,
        cx + 13.,
        py + 52.,
        cw - 26.,
        8.,
        &format!(
            "整场实时 · 速度{:.1} · 击键{:.2} · 码长{:.2} · 键准{:.1}% · 累计{}字",
            live.total_speed, live.keys_per_sec, live.code_len, live.accuracy, live.done_chars
        ),
        p.muted,
    );
    end_clip(f);
    let ay = y + ph - 38.;
    let compact = cw <= 650.;
    action(
        f,
        hs,
        cx,
        ay,
        if compact { 116. } else { 138. },
        if compact {
            "复制成绩单"
        } else {
            "复制成绩单  C"
        },
        Action::CopyScore,
        t,
        true,
    );
    action(
        f,
        hs,
        cx + if compact { 124. } else { 147. },
        ay,
        if compact { 92. } else { 110. },
        "复制后发群",
        Action::SendScore,
        t,
        false,
    );
    action(
        f,
        hs,
        cx + if compact { 224. } else { 266. },
        ay,
        if compact { 80. } else { 96. },
        if compact {
            "重打  R"
        } else {
            "重打本段  R"
        },
        Action::Restart,
        t,
        false,
    );
    action(
        f,
        hs,
        cx + if compact { 312. } else { 371. },
        ay,
        if compact { 76. } else { 92. },
        "下一段  ↵",
        Action::Next,
        t,
        false,
    );
    link(
        f,
        hs,
        cx + cw - if compact { 90. } else { 125. },
        ay + 4.,
        if compact { 90. } else { 125. },
        if compact {
            "查看赛报  →"
        } else {
            "查看整场赛报  →"
        },
        Action::Open(Modal::Report),
        t,
    );
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
    let p = tokens(t);
    let (cx, by, cw) = main_area(f, m, t, x, y, pw, ph);
    let rows: Vec<_> = sess
        .results
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.as_ref().map(|r| (i, r)))
        .collect();
    if rows.is_empty() {
        body_text(f, cx, by + 116., 28., "赛报，等待第一段", p.text);
        text(
            f,
            cx,
            by + 169.,
            12.,
            "还没有完成任何一段 · 先打一段试试",
            p.muted,
        );
        end_clip(f);
        action(
            f,
            hs,
            cx,
            y + ph - 42.,
            118.,
            "载入文字  F2",
            Action::Open(Modal::Load),
            t,
            true,
        );
        return;
    }
    let agg = sess.aggregate();
    let character_accuracy = if agg.chars > 0 {
        agg.chars.saturating_sub(agg.wrong) as f64 / agg.chars as f64 * 100.
    } else {
        0.
    };
    let complete = agg.completed_segments == sess.total_segments();
    body_text(f, cx, by + 20., 27., "赛报", p.text);
    clipped_text(
        f,
        cx + 74.,
        by + 35.,
        cw * 0.52,
        10.,
        &format!("{} · {} 段", sess.title, sess.total_segments()),
        p.muted,
    );
    if cw > 750. {
        // P2 赛制标记（任务 6a）：core score::race_badge 单点取用——最新结算段
        // 的 RoundMetadata 快照优先（开打时冻结），无快照回退 m。
        let (mode, word_hints) = rows
            .iter()
            .max_by_key(|(_, r)| r.ts)
            .and_then(|(_, r)| r.round.as_ref())
            .map(|z| (z.mode, z.word_hints))
            .unwrap_or((
                if m.race {
                    TrainingMode::Race
                } else {
                    TrainingMode::Practice
                },
                m.hints && !m.race,
            ));
        right(
            f,
            cx + cw - 45.,
            by + 34.,
            8.5,
            format!(
                "{} · {} · {} / {} 段已结算",
                race_badge(mode, word_hints),
                rows.iter()
                    .max_by_key(|(_, r)| r.ts)
                    .map(|(_, r)| r.date.as_str())
                    .unwrap_or(""),
                rows.len(),
                sess.total_segments()
            ),
            p.muted,
        );
        badge(
            f,
            cx + cw - 32.,
            by + 30.,
            32.,
            if complete { "完赛" } else { "进行中" },
            if complete { p.red } else { p.muted },
        );
    }
    text(f, cx, by + 70., 8., "总 速 度", p.muted);
    let hero = format!("{:.1}", agg.speed);
    let hero_size = 58. * (5. / hero.chars().count().max(5) as f32);
    mono(
        f,
        cx,
        by + 87. + (58. - hero_size) * 0.9,
        hero_size,
        hero,
        p.text,
    );
    text(f, cx + 166., by + 127., 9., "字/分", p.muted);
    text(
        f,
        cx,
        by + 149.,
        8.,
        if complete {
            "整场已结算 · 按总字数 / 总用时"
        } else {
            "部分赛报 · 仅包含已结算段"
        },
        if complete { p.green } else { p.muted },
    );
    // P1-4 LiveStats 接线（任务 5）：当前段进行中时给出“整场含当前段”实时行
    //（与上方已结算聚合分列；结算完两者口径重合，不重复显示）。
    if sess.phase == Phase::Typing && !sess.recorded {
        let live = sess.live_stats_now();
        clipped_text(
            f,
            cx,
            by + 161.,
            cw,
            8.,
            &format!(
                "整场实时（含当前段）· 速度{:.1} · 击键{:.2} · 码长{:.2} · 键准{:.1}% · 累计{}字",
                live.total_speed, live.keys_per_sec, live.code_len, live.accuracy, live.done_chars
            ),
            p.accent,
        );
    }
    // P1-3 选重/标顶合计（任务 2）：与 agg 同一 <5 硬门口径，短段不进合计。
    let gated: Vec<&ScoreRecord> = rows
        .iter()
        .map(|(_, r)| *r)
        .filter(|r| r.chars >= VALID_MIN_CHARS)
        .collect();
    let select_total: u32 = gated.iter().map(|r| r.select_count).sum();
    let ding_total: u32 = gated.iter().map(|r| r.ding_count).sum();
    let mut summaries = vec![
        (format!("{:.1}", agg.effective_speed), "有效速度"),
        (duration(agg.time_ms), "总用时"),
        (agg.chars.to_string(), "总字数"),
        (format!("{:.1}%", agg.accuracy), "键准·按键效率"),
        (format!("{:.1}%", character_accuracy), "字准"),
        (agg.wrong.to_string(), "错字"),
    ];
    // 选重/标顶合计：窄面板（<620）放不下 8 格时省略，宽面板补齐。
    if cw > 620. {
        summaries.push((select_total.to_string(), "选重"));
        summaries.push((ding_total.to_string(), "标顶"));
    }
    let sx = cx + if cw > 800. { 228. } else { 207. };
    let sw =
        ((cw - (sx - cx) - if cw > 900. { 260. } else { 0. }) / summaries.len() as f32).max(52.);
    for (i, (value, label)) in summaries.iter().enumerate() {
        mono(
            f,
            sx + i as f32 * sw,
            by + 122.,
            if cw > 700. { 15. } else { 12. },
            value,
            p.text2,
        );
        clipped_text(
            f,
            sx + i as f32 * sw,
            by + 144.,
            sw - 6.,
            8.,
            label,
            p.muted,
        );
    }
    if cw > 900. {
        for (i, (idx, r)) in rows.iter().take(4).enumerate() {
            let xx = cx + cw - 216. + i as f32 * 54.;
            round(f, xx, by + 130., 48., 21., 10., p.bg, Some(p.line));
            mono(
                f,
                xx + 6.,
                by + 136.,
                7.5,
                format!("{} {:.0}", idx + 1, r.speed),
                p.text2,
            );
        }
    }
    line(f, cx, by + 174., cw, p.line);
    let chart_y = by + 187.;
    card(f, cx, chart_y, cw, 173., t);
    text(f, cx + 14., chart_y + 10., 9., "分段对比", p.text2);
    text(
        f,
        cx + 81.,
        chart_y + 11.,
        8.,
        "速度 · 从 0 起算 · 虚线为已结算段总速度",
        p.muted,
    );
    let page = (m.scroll as usize).min(rows.len().saturating_sub(1));
    let shown: Vec<_> = rows
        .iter()
        .skip(page)
        .take(((cw - 60.) / 60.).floor().clamp(1., 16.) as usize)
        .collect();
    let highest = rows.iter().map(|(_, r)| r.speed).fold(1., f64::max);
    let max = (highest / 50.).ceil().max(1.) * 50.;
    let plot_x = cx + 46.;
    let plot_w = cw - 61.;
    let plot_top = chart_y + 49.;
    let plot_h = 94.;
    for i in 0..=3 {
        let yy = plot_top + plot_h - i as f32 / 3. * plot_h;
        line(f, plot_x, yy, plot_w, alpha(p.line2, 0.5));
        right(
            f,
            plot_x - 10.,
            yy - 5.,
            7.5,
            format!("{:.0}", i as f64 / 3. * max),
            alpha(p.muted, 0.7),
        );
    }
    let mean_y = plot_top + plot_h - (agg.speed / max) as f32 * plot_h;
    for i in 0..(plot_w / 5.) as usize {
        rect(
            f,
            plot_x + i as f32 * 5.,
            mean_y,
            3.,
            1.,
            alpha(p.accent, 0.5),
        );
    }
    let fastest = rows
        .iter()
        .max_by(|a, b| a.1.speed.total_cmp(&b.1.speed))
        .map(|(i, _)| *i);
    let slowest = rows
        .iter()
        .min_by(|a, b| a.1.speed.total_cmp(&b.1.speed))
        .map(|(i, _)| *i);
    let steady = rows
        .iter()
        .max_by(|a, b| a.1.accuracy.total_cmp(&b.1.accuracy))
        .map(|(i, _)| *i);
    let bw = plot_w / shown.len().max(1) as f32;
    for (j, row) in shown.iter().enumerate() {
        let (idx, r) = **row;
        let xx = plot_x + j as f32 * bw + bw * 0.5;
        let bh = (r.speed / max) as f32 * plot_h;
        let color = if Some(idx) == fastest {
            p.accent
        } else if Some(idx) == slowest && rows.len() > 1 {
            p.red
        } else if Some(idx) == steady {
            p.jade
        } else {
            alpha(p.accent, 0.75)
        };
        let width = 42f32.min(bw * 0.45).max(7.);
        rect(
            f,
            xx - width / 2.,
            plot_top + plot_h - bh,
            width,
            bh,
            alpha(color, 0.82),
        );
        line(
            f,
            xx - width / 2.,
            plot_top + plot_h - bh,
            width,
            alpha(p.text, 0.45),
        );
        mono(
            f,
            xx - 19.,
            chart_y + 32.,
            9.,
            format!("{:.1}", r.speed),
            p.text2,
        );
        text(
            f,
            xx - 21.,
            chart_y + 151.,
            8.,
            format!(
                "{}{} · {}",
                if r.round.as_ref().is_some_and(|z| !z.complete) {
                    "止·"
                } else {
                    ""
                },
                idx + 1,
                elide(&r.mark, 4)
            ),
            p.muted,
        );
    }
    let bottom = by + 372.;
    let sidew = if cw > 900. {
        316.
    } else {
        (cw * 0.31).max(174.)
    };
    let tw = cw - sidew - 14.;
    card(f, cx, bottom, tw, 224., t);
    text(f, cx + 13., bottom + 10., 9., "逐段明细", p.text2);
    text(
        f,
        cx + 81.,
        bottom + 11.,
        7.5,
        "按段序 · 滚轮查看更多",
        p.muted,
    );
    let compact = tw < 560.;
    let headings: Vec<(&str, f32)> = if compact {
        vec![
            ("段", 0.02),
            ("字数", 0.28),
            ("速度", 0.46),
            ("键/字准%", 0.65),
            ("错字", 0.79),
            ("用时", 0.98),
        ]
    } else {
        vec![
            ("段", 0.02),
            ("字数", 0.26),
            ("速度", 0.39),
            ("击键", 0.50),
            ("码长", 0.61),
            ("键/字准%", 0.73),
            ("错字", 0.81),
            ("回改", 0.89),
            ("用时", 0.99),
        ]
    };
    let inner = tw - 26.;
    for (i, (label, z)) in headings.iter().enumerate() {
        if i == 0 {
            text(f, cx + 13., bottom + 33., 7.5, *label, p.muted);
        } else {
            right(f, cx + 13. + inner * *z, bottom + 33., 7.5, *label, p.muted);
        }
    }
    line(f, cx + 13., bottom + 49., inner, p.line2);
    for (j, (idx, r)) in rows.iter().skip(page).take(5).enumerate() {
        let yy = bottom + 55. + j as f32 * 25.;
        if Some(*idx) == fastest {
            rect(f, cx + 13., yy - 3., inner, 24., alpha(p.accent, 0.07));
        }
        clipped_text(
            f,
            cx + 13.,
            yy + 1.,
            inner * if compact { 0.23 } else { 0.20 },
            8.5,
            &format!(
                "{}{} · {}",
                if r.round.as_ref().is_some_and(|z| !z.complete) {
                    "中止 · "
                } else {
                    ""
                },
                idx + 1,
                r.mark
            ),
            p.text2,
        );
        let values = if compact {
            vec![
                r.chars.to_string(),
                format!("{:.1}", r.speed),
                format!("{:.1}/{:.1}", r.accuracy, r.character_accuracy()),
                r.wrong.to_string(),
                duration(r.time_ms),
            ]
        } else {
            vec![
                r.chars.to_string(),
                format!("{:.1}", r.speed),
                format!("{:.2}", r.strokes_per_sec),
                format!("{:.2}", r.code_len),
                format!("{:.1}/{:.1}", r.accuracy, r.character_accuracy()),
                r.wrong.to_string(),
                r.corrections.to_string(),
                duration(r.time_ms),
            ]
        };
        for (k, value) in values.iter().enumerate() {
            right(
                f,
                cx + 13. + inner * headings[k + 1].1,
                yy + 1.,
                8.5,
                value,
                p.text2,
            );
        }
        line(f, cx + 13., yy + 20., inner, p.line);
    }
    let total_y = bottom + 194.;
    line(f, cx + 13., total_y - 6., inner, p.line2);
    text(f, cx + 13., total_y, 8.5, "合计", p.muted);
    let totals = if compact {
        vec![
            agg.chars.to_string(),
            format!("{:.1}", agg.speed),
            format!("{:.1}/{:.1}", agg.accuracy, character_accuracy),
            agg.wrong.to_string(),
            duration(agg.time_ms),
        ]
    } else {
        vec![
            agg.chars.to_string(),
            format!("{:.1}", agg.speed),
            format!("{:.2}", agg.strokes_per_sec),
            format!("{:.2}", agg.code_len),
            format!("{:.1}/{:.1}", agg.accuracy, character_accuracy),
            agg.wrong.to_string(),
            agg.corrections.to_string(),
            duration(agg.time_ms),
        ]
    };
    for (k, value) in totals.iter().enumerate() {
        right(
            f,
            cx + 13. + inner * headings[k + 1].1,
            total_y,
            8.,
            value,
            p.muted,
        );
    }
    clipped_text(
        f,
        cx + 13.,
        bottom + 212.,
        inner,
        7.,
        "键/字准顺序：按键效率 / 文本正确率",
        p.muted,
    );
    let dx = cx + tw + 14.;
    for (i, (label, idx, color)) in [
        ("最快段", fastest, p.accent),
        ("最慢段", slowest, p.red),
        ("最高键准", steady, p.jade),
    ]
    .into_iter()
    .enumerate()
    {
        let yy = bottom + i as f32 * 32.;
        card(f, dx, yy, sidew, 27., t);
        text(f, dx + 10., yy + 8., 7.5, label, color);
        if let Some((n, r)) = rows.iter().find(|(n, _)| Some(*n) == idx) {
            clipped_text(
                f,
                dx + 73.,
                yy + 7.,
                sidew - 146.,
                9.,
                &if sidew > 240. {
                    format!(
                        "{}{} · {}",
                        if r.round.as_ref().is_some_and(|z| !z.complete) {
                            "中止 · "
                        } else {
                            ""
                        },
                        n + 1,
                        elide(&r.mark, 7)
                    )
                } else {
                    format!(
                        "{}{}段",
                        if r.round.as_ref().is_some_and(|z| !z.complete) {
                            "止·"
                        } else {
                            "第"
                        },
                        n + 1
                    )
                },
                p.text2,
            );
            right(
                f,
                dx + sidew - 10.,
                yy + 8.,
                8.,
                if i == 2 {
                    format!("{:.1}%", r.accuracy)
                } else {
                    format!("{:.1}", r.speed)
                },
                p.text2,
            );
        }
    }
    let dy = bottom + 98.;
    card(f, dx, dy, sidew, 126., t);
    text(f, dx + 10., dy + 9., 8., "错字与慢字", p.text2);
    right(f, dx + sidew - 10., dy + 10., 7., "当前段 · 估计", p.muted);
    let estimates = slow_estimates(sess);
    if estimates.is_empty() {
        paragraph(
            f,
            dx + 10.,
            dy + 40.,
            sidew - 20.,
            64.,
            9.,
            19.,
            "本段尚无可分析的上屏时间事件。",
            p.muted,
        );
    } else {
        let max_ms = estimates.iter().map(|z| z.average_ms).fold(1., f64::max);
        for (i, z) in estimates.iter().enumerate() {
            let yy = dy + 31. + i as f32 * 17.;
            body_text(f, dx + 10., yy - 1., 12., z.ch.to_string(), p.text2);
            let barx = dx + 33.;
            let barw = (sidew - 115.).max(20.);
            rect(f, barx, yy + 8., barw, 2., p.line);
            rect(
                f,
                barx,
                yy + 8.,
                barw * (z.average_ms / max_ms) as f32,
                2.,
                alpha(p.accent, 0.8),
            );
            right(
                f,
                dx + sidew - 29.,
                yy + 2.,
                7.5,
                format!("{:.2}s", z.average_ms / 1000.),
                p.muted,
            );
            if z.wrong > 0 {
                right(
                    f,
                    dx + sidew - 8.,
                    yy + 2.,
                    7.,
                    format!("错{}", z.wrong),
                    p.red,
                );
            } else if z.count > 1 {
                right(
                    f,
                    dx + sidew - 8.,
                    yy + 2.,
                    7.,
                    format!("×{}", z.count),
                    p.muted,
                );
            }
        }
    }
    text(
        f,
        dx + 10.,
        dy + 119.,
        6.5,
        "上屏组时长均摊，非逐字独立计时",
        alpha(p.muted, 0.75),
    );
    end_clip(f);
    let ay = y + ph - 38.;
    action(
        f,
        hs,
        cx,
        ay,
        148.,
        "复制整份成绩单  C",
        Action::CopyRace,
        t,
        true,
    );
    action(
        f,
        hs,
        cx + 157.,
        ay,
        110.,
        "复制后发群",
        Action::CopyRace,
        t,
        false,
    );
    action(
        f,
        hs,
        cx + 276.,
        ay,
        93.,
        "导出图片  E",
        Action::ExportImage,
        t,
        false,
    );
    if cw > 780. {
        right(
            f,
            cx + cw,
            ay + 13.,
            7.5,
            "含逐段明细 · 复制后确认目标群再手动发送",
            p.muted,
        );
    }
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
    line(f, x, y + 62., pw, p.line);
    round(
        f,
        x + 20.,
        y + 15.,
        32.,
        32.,
        9.,
        alpha(p.accent, 0.1),
        Some(alpha(p.accent, 0.35)),
    );
    text(f, x + 28., y + 23., 16., icon, p.accent);
    text(f, x + 64., y + 14., 14., title, p.text);
    clipped_text(f, x + 64., y + 35., pw - 180., 9.5, sub, p.muted);
    keycap(f, x + pw - 86., y + 22., key, t);
    text(f, x + pw - 43., y + 15., 22., "×", p.muted);
    hs.push(Hit {
        x: x + pw - 48.,
        y: y + 16.,
        w: 28.,
        h: 28.,
        action: Action::Close,
    });
}

fn online_side(pw: f32) -> f32 {
    if pw >= 800. {
        264.
    } else {
        (pw * 0.33).clamp(190., 264.)
    }
}
fn online_base(m: &Model) -> f32 {
    if m.online_expired {
        164.
    } else {
        130.
    }
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
        "从晴发文取文、浏览书架；Base URL 可改",
        "F8",
    );
    let left = online_side(pw);
    rect(f, x, y + 63., left, ph - 125., p.bg2);
    rect(f, x + left, y + 63., 1., ph - 125., p.line);
    let iw = left - 24.;
    card(f, x + 12., y + 74., iw, 48., t);
    round(
        f,
        x + 21.,
        y + 82.,
        32.,
        32.,
        8.,
        if m.online_authenticated {
            p.accent
        } else {
            p.line2
        },
        None,
    );
    body_text(
        f,
        x + 30.,
        y + 89.,
        15.,
        "晴",
        if m.online_authenticated {
            p.bg
        } else {
            p.muted
        },
    );
    let acct = if m.online_account.is_empty() {
        if m.online_authenticated {
            "已登录"
        } else {
            "未登录"
        }
    } else {
        &m.online_account
    };
    clipped_text(f, x + 62., y + 84., iw - 113., 11., acct, p.text2);
    clipped_text(
        f,
        x + 62.,
        y + 103.,
        iw - 124.,
        7.5,
        &cfg.wenlai_domain.trim_start_matches("https://"),
        p.muted,
    );
    badge(
        f,
        x + left - 60.,
        y + 103.,
        43.,
        if m.online_expired {
            "已过期"
        } else if m.online_authenticated {
            "已登录"
        } else {
            "未登录"
        },
        if m.online_expired {
            p.accent
        } else if m.online_authenticated {
            p.green
        } else {
            p.muted
        },
    );
    link(
        f,
        hs,
        x + left - 61.,
        y + 82.,
        42.,
        if m.online_login_expanded {
            "收起"
        } else if m.online_authenticated {
            "账号"
        } else {
            "登录"
        },
        Action::OnlineAccount,
        t,
    );
    if m.online_login_expanded {
        for (yy, height) in [(153., 28.), (205., 28.), (311., 30.)] {
            round(
                f,
                x + 15.,
                y + yy - 1.,
                left - 30.,
                height + 2.,
                5.,
                p.bg2,
                Some(p.line2),
            );
        }
        for (yy, label) in [
            (137., "用户名"),
            (189., "密码 · 不保存"),
            (293., "服务地址 · 仅 HTTPS"),
        ] {
            text(f, x + 16., y + yy, 8.5, label, p.muted);
        }
        // 整改2c：登录行三键（登录 | 注册账号 | 退出登录）——注册走
        // U+E000 标记动作，main.rs aux 解码后 ShellExecuteW 开站点账号入口
        action(
            f,
            hs,
            x + 16.,
            y + 245.,
            (iw - 20.) / 3.,
            "登录",
            Action::OnlineLogin,
            t,
            true,
        );
        action(
            f,
            hs,
            x + 22. + (iw - 20.) / 3.,
            y + 245.,
            (iw - 20.) / 3.,
            "注册账号",
            crate::aux_marker("online_register"),
            t,
            false,
        );
        action(
            f,
            hs,
            x + 28. + (iw - 20.) / 3. * 2.,
            y + 245.,
            (iw - 20.) / 3.,
            "退出登录",
            Action::OnlineLogout,
            t,
            false,
        );
        action(
            f,
            hs,
            x + 16.,
            y + 353.,
            iw - 8.,
            "应用服务地址",
            Action::OnlineServer,
            t,
            false,
        );
        paragraph(
            f,
            x + 16.,
            y + 405.,
            iw - 8.,
            90.,
            9.,
            20.,
            "登录后收起此页，可按分类、难度取文。\n切换服务后需要重新登录。",
            p.muted,
        );
    } else {
        if m.online_expired {
            round(
                f,
                x + 12.,
                y + 130.,
                iw,
                25.,
                6.,
                alpha(p.accent, 0.08),
                Some(alpha(p.accent, 0.3)),
            );
            clipped_text(
                f,
                x + 20.,
                y + 138.,
                iw - 55.,
                8.,
                "JWT 已过期 · 当前文保留",
                p.accent,
            );
            link(
                f,
                hs,
                x + left - 57.,
                y + 131.,
                39.,
                "登录",
                Action::OnlineAccount,
                t,
            );
        }
        let base = online_base(m);
        card(f, x + 12., y + base, iw, 36., t);
        text(f, x + 21., y + base + 6., 8., "Base URL  高级", p.muted);
        clipped_text(
            f,
            x + 21.,
            y + base + 21.,
            iw - 18.,
            7.5,
            &cfg.wenlai_domain,
            p.text2,
        );
        link(
            f,
            hs,
            x + left - 57.,
            y + base + 2.,
            39.,
            "修改",
            Action::OnlineAccount,
            t,
        );
        let fy = y + base + 48.;
        let active = m.online_authenticated && !m.online_expired;
        text(f, x + 12., fy, 8., "分类", p.muted);
        let cats: Vec<_> = m.online_categories.iter().enumerate().collect();
        if cats.is_empty() {
            text(
                f,
                x + 12.,
                fy + 18.,
                8.5,
                if active {
                    "分类加载中…"
                } else {
                    "登录后同步分类"
                },
                alpha(p.muted, 0.65),
            );
        } else {
            let cap = ((iw + 4.) / 52.).floor().max(1.) as usize;
            let selected = m.online_category.min(cats.len() - 1);
            let start = if selected >= cap {
                selected + 1 - cap
            } else {
                0
            };
            let visible = cap.min(cats.len());
            let bw = (iw - (visible.saturating_sub(1)) as f32 * 4.) / visible as f32;
            for (j, (idx, (_, name))) in cats.iter().skip(start).take(visible).enumerate() {
                chip(
                    f,
                    hs,
                    x + 12. + j as f32 * (bw + 4.),
                    fy + 15.,
                    bw,
                    &elide(name, 4),
                    *idx == m.online_category,
                    if active {
                        Action::OnlineCategory(*idx)
                    } else {
                        Action::None
                    },
                    t,
                );
            }
            if cats.len() > cap {
                link(
                    f,
                    hs,
                    x + left - 57.,
                    fy - 5.,
                    42.,
                    "更多 ›",
                    if active {
                        Action::OnlineCategory((selected + 1) % cats.len())
                    } else {
                        Action::None
                    },
                    t,
                );
            }
        }
        text(f, x + 12., fy + 43., 8., "难度", p.muted);
        let bw = (iw - 15.) / 6.;
        for (i, name) in ["淼", "水", "易", "普", "难", "虐"].into_iter().enumerate() {
            chip(
                f,
                hs,
                x + 12. + i as f32 * (bw + 3.),
                fy + 58.,
                bw,
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
        text(f, x + 12., fy + 86., 8., "字数", p.muted);
        if active {
            round(f, x + 13., fy + 100., 54., 25., 5., p.bg2, Some(p.line2));
        }
        if !active {
            round(f, x + 14., fy + 101., 52., 23., 5., p.surface, Some(p.line));
            let value = m.field("online_chars");
            mono(
                f,
                x + 20.,
                fy + 107.,
                9.5,
                if value.is_empty() {
                    cfg.seg_chars.to_string()
                } else {
                    elide(&value, 5)
                },
                p.muted,
            );
        }
        text(f, x + 76., fy + 107., 8., "字", p.muted);
        toggle(
            f,
            hs,
            x + left - 87.,
            fy + 104.,
            m.online_strict,
            if active {
                Action::OnlineStrict
            } else {
                Action::None
            },
            t,
        );
        text(f, x + left - 55., fy + 107., 7.5, "精确字数", p.muted);
        for (i, (name, dir)) in [("随机", 0), ("上一段", -1), ("下一段", 1)]
            .into_iter()
            .enumerate()
        {
            let bw = (iw - 8.) / 3.;
            chip(
                f,
                hs,
                x + 12. + i as f32 * (bw + 4.),
                fy + 130.,
                bw,
                name,
                false,
                if active {
                    Action::OnlineFetch(dir)
                } else {
                    Action::None
                },
                t,
            );
        }
        if !active {
            text(
                f,
                x + 12.,
                fy + 163.,
                8.,
                if m.online_expired {
                    "重新登录后可取文；已有正文仍可载入"
                } else {
                    "登录后可取文；已有正文仍可载入"
                },
                p.muted,
            );
        }
        // 整改2b：自动下一段（结算 1.5s 后 fetch_adjacent，赛制除外）——
        // 开关状态经 model.fields("online_auto_next") 透出，main.rs 启动时播种、
        // 标记动作回写；默认开，持久化在数据目录 online.json。
        let auto_next = m.field("online_auto_next") != "false";
        toggle(
            f,
            hs,
            x + left - 87.,
            fy + 160.,
            auto_next,
            crate::aux_marker("online_auto_next"),
            t,
        );
        text(f, x + left - 55., fy + 163., 7.5, "自动下一段", p.muted);
        let shelf_y = y + ph - 62. - 134.;
        line(f, x + 12., shelf_y, iw, p.line2);
        text(
            f,
            x + 12.,
            shelf_y + 9.,
            8.,
            format!("书架  {} 本", m.online_shelf.len()),
            p.muted,
        );
        link(
            f,
            hs,
            x + left - 59.,
            shelf_y + 2.,
            43.,
            "刷新",
            if active {
                Action::OnlineBooks
            } else {
                Action::None
            },
            t,
        );
        if m.online_shelf.is_empty() {
            paragraph(
                f,
                x + 17.,
                shelf_y + 38.,
                iw - 10.,
                55.,
                9.,
                19.,
                if active {
                    "书架为空，可先随机取文。"
                } else {
                    "登录后浏览在线书架"
                },
                p.muted,
            );
        } else {
            let count = 3usize;
            let offset = m.shelf_offset.min(m.online_shelf.len().saturating_sub(1));
            for (j, (idx, name)) in m
                .online_shelf
                .iter()
                .enumerate()
                .skip(offset)
                .take(count)
                .enumerate()
            {
                let yy = shelf_y + 27. + j as f32 * 24.;
                let selected = !m.online_preview.is_empty() && name.starts_with(&m.online_title);
                chip(
                    f,
                    hs,
                    x + 12.,
                    yy,
                    iw,
                    &elide(name, ((iw - 16.) / 9.).max(6.) as usize),
                    selected,
                    if active {
                        Action::OnlineShelf(idx)
                    } else {
                        Action::None
                    },
                    t,
                );
            }
            if m.online_shelf.len() > 3 {
                link(
                    f,
                    hs,
                    x + left - 72.,
                    shelf_y + 101.,
                    56.,
                    "下一页 ›",
                    Action::ShelfPage,
                    t,
                );
            }
        }
        text(
            f,
            x + 14.,
            shelf_y + 115.,
            7.,
            "读取在线进度 · 当前书架模式由服务端决定",
            alpha(p.muted, 0.7),
        );
    }
    let rx = x + left + 16.;
    let rw = pw - left - 32.;
    // The account expiry banner and request status occupy distinct locations so
    // identity, expired credentials and a retained article can coexist visibly.
    let request_status = if m.online_busy {
        "正在请求… · 已保留当前预览"
    } else if m.online_status.is_empty() {
        "就绪"
    } else {
        &m.online_status
    };
    clipped_text(f, rx + 2., y + 79., rw - 58., 7.5, request_status, p.muted);
    link(
        f,
        hs,
        rx + rw - 39.,
        y + 70.,
        39.,
        "重试",
        Action::OnlineFetch(0),
        t,
    );
    let doc_y = y + 96.;
    let doc_h = ph - 62. - doc_y + y - 33.;
    card(f, rx, doc_y, rw, doc_h, t);
    if m.online_preview.is_empty() {
        body_text(
            f,
            rx + 15.,
            doc_y + 24.,
            18.,
            if m.online_authenticated {
                "从书架或随机取文开始"
            } else {
                "登录后浏览在线书架"
            },
            p.text2,
        );
        paragraph(
            f,
            rx + 15.,
            doc_y + 66.,
            rw - 30.,
            100.,
            11.,
            23.,
            // 整改2a：预览步骤退役——取到的正文直接进跟打区，第一段起打
            "取到的正文会直接载入跟打区开始，无需预览。\\n\"自动下一段\"开启时，一段结算 1.5 秒后自动接下一段。",
            p.muted,
        );
    } else {
        body_text(
            f,
            rx + 15.,
            doc_y + 15.,
            18.,
            elide(&m.online_title, ((rw - 120.) / 18.).max(5.) as usize),
            p.text,
        );
        badge(f, rx + rw - 73., doc_y + 20., 55., "晴发文", p.muted);
        let meta = m
            .online_article
            .as_ref()
            .map(|a| {
                format!(
                    "{} 字  ·  分类 {}  ·  难度 {}",
                    a.text.chars().count(),
                    if a.category.is_empty() {
                        "未提供"
                    } else {
                        &a.category
                    },
                    a.difficulty.label()
                )
            })
            .unwrap_or_else(|| format!("{} 字 · 晴发文正文", m.online_preview.chars().count()));
        clipped_text(f, rx + 15., doc_y + 47., rw - 30., 7.5, &meta, p.muted);
        // 整改2a：正文不再重复预览（已在跟打区），此卡只保留取文元信息
        if let Some(a) = &m.online_article {
            let yy = doc_y + doc_h - 73.;
            text(f, rx + 15., yy, 7.5, "分段", p.muted);
            clipped_text(
                f,
                rx + 46.,
                yy - 1.,
                rw - 60.,
                8.,
                &if a.seg_mark.is_empty() {
                    "源站未提供段标".into()
                } else {
                    format!("第 {} 段", a.seg_mark)
                },
                p.text2,
            );
            text(f, rx + 15., yy + 23., 7., "本次取文位置", p.muted);
            if a.sort_num > 0 && a.end_sort_num >= a.sort_num {
                right(
                    f,
                    rx + rw - 15.,
                    yy + 23.,
                    7.5,
                    if a.sort_num == a.end_sort_num {
                        format!("第 {} 段 · 总段数未提供", a.sort_num)
                    } else {
                        format!("第 {} 至 {} 段", a.sort_num, a.end_sort_num)
                    },
                    p.muted,
                );
                rect(f, rx + 15., yy + 40., rw - 30., 2., p.line2);
                // end_sort_num is the last source segment included in this
                // fetch, not the book's total segment count. A percent bar would
                // falsely report every single-segment fetch as 100% complete.
            } else {
                right(f, rx + rw - 15., yy + 23., 7., "源站未提供总段数", p.muted);
            }
        }
        line(f, rx + 15., doc_y + doc_h - 23., rw - 30., p.line);
        text(
            f,
            rx + 15.,
            doc_y + doc_h - 15.,
            7.5,
            // 整改2a：正文在跟打区，此卡仅保留取文信息备查
            "晴发文 · 正文已在跟打区，此处仅存取文信息",
            p.muted,
        );
    }
    clipped_text(
        f,
        rx,
        y + ph - 85.,
        rw,
        7.5,
        "发文格式：[难度]书名 [字数N] · 正文 · -----第{mark}段-晴发文",
        p.muted,
    );
    line(f, x, y + ph - 62., pw, p.line);
    if pw > 820. {
        keycap(f, x + 20., y + ph - 39., "F8", t);
        text(
            f,
            x + 52.,
            y + ph - 34.,
            9.,
            "在线取文 · 断网保留当前文",
            p.muted,
        );
    }
    // 整改13：理论分析入口（打开整篇理论码长/选重分析模态）
    link(
        f,
        hs,
        x + pw - 320.,
        y + ph - 41.,
        60.,
        "理论分析",
        crate::aux_marker("show_full_analysis"),
        t,
    );
    // 整改12：当前段键盘热力图入口（最近结算段 VK 直方图）
    link(
        f,
        hs,
        x + pw - 250.,
        y + ph - 41.,
        60.,
        "热力图",
        crate::aux_marker("show_heatmap"),
        t,
    );
    // 整改2a：去预览步骤后两键合一——“仅载入/载入并开始”语义相同，保留
    // 单一“载入正文并开始”（重取文后手动回载入口）
    action(
        f,
        hs,
        x + pw - 119.,
        y + ph - 46.,
        99.,
        "载入正文并开始",
        Action::OnlineLoad,
        t,
        true,
    );
}

fn rules_side(pw: f32) -> f32 {
    if pw >= 1000. {
        348.
    } else {
        pw * 0.32
    }
}
fn rules_geometry(pw: f32, ph: f32) -> (f32, f32, f32, f32, f32, f32) {
    let side = rules_side(pw);
    let rw = pw - side - 32.;
    let labels = if rw > 500. { 104. } else { 82. };
    let test_y = 242.;
    let test_h = (ph - 62. - test_y - 72.).max(120.);
    (side, rw, labels, test_y, test_h, (rw - 44.) / 2.)
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
    let lines = wrap_lines(output, w, 12.);
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
            size: 12.,
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
        "▽",
        "文本清洗",
        "规则、测试与比对口径；保留原文，可随时还原",
        "F9",
    );
    let (side, rw, labels, test_y, test_h, cw) = rules_geometry(pw, ph);
    rect(f, x, y + 63., side, ph - 125., p.bg2);
    rect(f, x + side, y + 63., 1., ph - 125., p.line);
    text(
        f,
        x + 16.,
        y + 78.,
        8.,
        format!("清洗规则  {} 条", m.rules.len()),
        p.muted,
    );
    link(
        f,
        hs,
        x + side - 75.,
        y + 70.,
        62.,
        "恢复内置",
        Action::RuleReset,
        t,
    );
    let list_rows = ((ph - 62. - 118. - 139.) / 42.).floor().max(1.) as usize;
    let offset = (m.scroll as usize).min(m.rules.len().saturating_sub(1));
    for (j, (idx, (name, on, content, pattern, _))) in m
        .rules
        .iter()
        .enumerate()
        .skip(offset)
        .take(list_rows)
        .enumerate()
    {
        let yy = y + 107. + j as f32 * 42.;
        if idx == m.rule_selected {
            round(
                f,
                x + 10.,
                yy - 3.,
                side - 20.,
                39.,
                6.,
                p.surface2,
                Some(p.line),
            );
            rect(f, x + 10., yy + 2., 2., 29., p.accent);
        }
        text(f, x + 18., yy + 10., 10., "⠿", p.muted);
        toggle(f, hs, x + 34., yy + 9., *on, Action::RuleToggle(idx), t);
        clipped_text(f, x + 68., yy + 10., side - 165., 9.5, name, p.text2);
        let builtin = cfg
            .cleaning_preset
            .rules
            .get(idx)
            .is_some_and(|r| r.builtin);
        badge(
            f,
            x + side - 88.,
            yy + 10.,
            if builtin { 28. } else { 36. },
            if builtin { "内置" } else { "自定义" },
            if builtin { p.muted } else { p.accent },
        );
        if side > 300. {
            clipped_text(
                f,
                x + side - 51.,
                yy + 12.,
                40.,
                7.,
                &elide(pattern, 5),
                alpha(p.muted, 0.65),
            );
        }
        hs.push(Hit {
            x: x + 65.,
            y: yy - 2.,
            w: side - 76.,
            h: 39.,
            action: Action::RuleSelect(idx),
        });
        if *content && side < 270. {
            badge(f, x + side - 43., yy + 10., 28., "内容", p.red);
        }
    }
    let add_y = y + 107. + list_rows.min(m.rules.len().saturating_sub(offset)) as f32 * 42. + 5.;
    action(
        f,
        hs,
        x + 11.,
        add_y,
        side - 22.,
        "+ 新建规则",
        Action::RuleAdd,
        t,
        false,
    );
    let tools_y = y + ph - 62. - 99.;
    for (i, (name, a)) in [
        ("上移", Action::RuleUp),
        ("下移", Action::RuleDown),
        ("删除", Action::RuleDelete),
    ]
    .into_iter()
    .enumerate()
    {
        let bw = (side - 30.) / 3.;
        chip(
            f,
            hs,
            x + 10. + i as f32 * (bw + 5.),
            tools_y,
            bw,
            name,
            false,
            a,
            t,
        );
    }
    line(f, x + 11., tools_y + 34., side - 22., p.line2);
    text(f, x + 16., tools_y + 45., 7.5, "执行顺序", p.muted);
    paragraph(
        f,
        x + 16.,
        tools_y + 61.,
        side - 32.,
        35.,
        7.,
        14.,
        "从上到下执行，关闭的规则跳过。\n赛制仅允许可信内置安全规则。",
        alpha(p.muted, 0.78),
    );
    let rx = x + side + 16.;
    card(f, rx, y + 74., rw, 160., t);
    text(f, rx + 12., y + 84., 9., "规则编辑", p.text2);
    let selected = cfg.cleaning_preset.rules.get(m.rule_selected);
    if let Some(rule) = selected {
        badge(
            f,
            rx + 66.,
            y + 84.,
            if rule.builtin { 28. } else { 36. },
            if rule.builtin { "内置" } else { "自定义" },
            p.muted,
        );
        badge(
            f,
            rx + 106.,
            y + 84.,
            38.,
            if rule.safe { "安全" } else { "内容级" },
            if rule.safe { p.green } else { p.red },
        );
        right(
            f,
            rx + rw - 12.,
            y + 85.,
            7.,
            if rule.safe {
                "不会扩展赛制权限"
            } else {
                "会改变正文"
            },
            if rule.safe { p.muted } else { p.red },
        );
    }
    for (yy, label) in [
        (111., "名称"),
        (143., "正则 pattern"),
        (181., "替换 replacement"),
    ] {
        clipped_text(f, rx + 12., y + yy, labels - 18., 7.5, label, p.muted);
    }
    for yy in [105., 136., 174.] {
        round(
            f,
            rx + labels - 1.,
            y + yy - 1.,
            rw - labels - 10.,
            24.,
            5.,
            p.bg2,
            Some(p.line2),
        );
    }
    clipped_text(
        f,
        rx + labels,
        y + 163.,
        rw - labels - 12.,
        7.,
        "Rust regex 不支持反向引用 / 环视；捕获组替换使用 $1",
        p.accent,
    );
    line(f, rx + 12., y + 205., rw - 24., p.line);
    text(f, rx + 12., y + 217., 7.5, "作用域", p.muted);
    chip(
        f,
        hs,
        rx + 47.,
        y + 211.,
        54.,
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
        rx + 104.,
        y + 211.,
        64.,
        "保存预设",
        m.rule_save_preset,
        if m.rule_save_preset {
            Action::None
        } else {
            Action::RuleScopeToggle
        },
        t,
    );
    let safe = selected.is_some_and(|r| r.safe);
    if rw > 510. {
        text(f, rx + 187., y + 217., 7.5, "安全等级", p.muted);
        chip(
            f,
            hs,
            rx + 235.,
            y + 211.,
            43.,
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
            rx + 281.,
            y + 211.,
            54.,
            "内容级",
            !safe,
            if safe {
                Action::RuleContentToggle
            } else {
                Action::None
            },
            t,
        );
    }
    link(
        f,
        hs,
        rx + rw - 61.,
        y + 208.,
        49.,
        "保存规则",
        Action::RuleSave,
        t,
    );
    let ty = y + test_y;
    card(f, rx, ty, rw, test_h, t);
    text(f, rx + 12., ty + 10., 9., "测试", p.text2);
    let current = m.field("rule_test");
    let preview_available =
        !m.rule_test_source.is_empty() || !m.rule_preview.is_empty() || m.rule_hits > 0;
    let fresh = m.rule_test_source == current && preview_available;
    badge(
        f,
        rx + 39.,
        ty + 8.,
        72.,
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
        rx + rw - 84.,
        ty + 2.,
        72.,
        "预览整组  →",
        Action::RuleTest,
        t,
    );
    let col_y = ty + 32.;
    let col_h = test_h - 59.;
    round(f, rx + 12., col_y, cw, col_h, 6., p.bg2, Some(p.line));
    round(f, rx + 32. + cw, col_y, cw, col_h, 6., p.bg2, Some(p.line));
    text(f, rx + 20., col_y + 8., 7.5, "原文 · 可编辑", p.muted);
    text(
        f,
        rx + 40. + cw,
        col_y + 8.,
        7.5,
        "替换后 · 上次预览",
        p.muted,
    );
    text(f, rx + 17. + cw, col_y + col_h / 2., 12., "→", p.muted);
    if !m.rule_error.is_empty() {
        paragraph(
            f,
            rx + 40. + cw,
            col_y + 31.,
            cw - 16.,
            col_h - 42.,
            9.,
            18.,
            &m.rule_error,
            p.red,
        );
    } else if !preview_available {
        paragraph(
            f,
            rx + 40. + cw,
            col_y + 32.,
            cw - 16.,
            col_h - 44.,
            9.5,
            19.,
            "点击「预览整组」查看替换结果。\n预览不会写回正文。",
            p.muted,
        );
    } else if m.rule_preview.is_empty() {
        paragraph(
            f,
            rx + 40. + cw,
            col_y + 32.,
            cw - 16.,
            col_h - 44.,
            9.5,
            19.,
            "替换后为空文本。\n所有字符均被本组规则移除。",
            p.accent,
        );
    } else {
        result_paragraph(
            f,
            rx + 40. + cw,
            col_y + 31.,
            cw - 16.,
            col_h - 42.,
            &m.rule_test_source,
            &m.rule_preview,
            t,
        );
    }
    clipped_text(
        f,
        rx + 12.,
        ty + test_h - 17.,
        rw - 24.,
        7.,
        if !fresh && preview_available {
            "原文已修改，请重新预览 · 不会写回文章"
        } else {
            "仅在明确应用后改变正文；可随时「还原原文」"
        },
        if !fresh && preview_available {
            p.accent
        } else {
            p.muted
        },
    );
    let compare_y = ty + test_h + 8.;
    card(f, rx, compare_y, rw, 52., t);
    text(f, rx + 12., compare_y + 9., 9., "比对规则", p.text2);
    badge(f, rx + 64., compare_y + 8., 42., "不改正文", p.green);
    if rw > 510. {
        text(
            f,
            rx + 117.,
            compare_y + 10.,
            7.,
            "只放宽比对 · 原始文本始终保留",
            p.muted,
        );
    }
    toggle(
        f,
        hs,
        rx + rw - 39.,
        compare_y + 8.,
        m.quote_relaxed,
        Action::QuoteToggle,
        t,
    );
    let q = &cfg.session_rules.quotes;
    for (i, (name, on)) in [
        ("双引号 “ ”", q.double),
        ("单引号 ‘ ’", q.single),
        ("角括号「 」", q.corner),
    ]
    .into_iter()
    .enumerate()
    {
        let xx = rx + 12. + i as f32 * ((rw - 24.) / 3.);
        toggle(
            f,
            hs,
            xx,
            compare_y + 30.,
            on && m.quote_relaxed,
            Action::QuoteClass(i as u8),
            t,
        );
        clipped_text(
            f,
            xx + 32.,
            compare_y + 34.,
            (rw - 24.) / 3. - 34.,
            7.,
            name,
            p.muted,
        );
    }
    line(f, x, y + ph - 62., pw, p.line);
    if pw > 950. {
        keycap(f, x + 20., y + ph - 39., "F9", t);
        text(
            f,
            x + 52.,
            y + ph - 34.,
            8.5,
            "规则按顺序执行 · 赛制限制始终有效",
            p.muted,
        );
    }
    let start = x + pw - 344.;
    action(
        f,
        hs,
        start,
        y + ph - 46.,
        88.,
        "还原原文",
        Action::RuleRestore,
        t,
        false,
    );
    action(
        f,
        hs,
        start + 96.,
        y + ph - 46.,
        54.,
        "导入",
        Action::RuleImport,
        t,
        false,
    );
    action(
        f,
        hs,
        start + 158.,
        y + ph - 46.,
        54.,
        "导出",
        Action::RuleExport,
        t,
        false,
    );
    action(
        f,
        hs,
        start + 220.,
        y + ph - 46.,
        104.,
        "应用到当前文",
        Action::RuleApply,
        t,
        true,
    );
}

/// Coordinates shared with the native RichEdit host. The caller applies the same
/// body scrolling and clipping as it applies to the D2D cards around these fields.
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
        Modal::Online => {
            let side = online_side(pw);
            if m.online_login_expanded {
                add("username", x + 16., y + 153., side - 32., 28., false, false);
                add("password", x + 16., y + 205., side - 32., 28., false, true);
                add("server", x + 16., y + 311., side - 32., 30., false, false);
            } else if m.online_authenticated && !m.online_expired {
                add(
                    "online_chars",
                    x + 14.,
                    y + online_base(m) + 48. + 101.,
                    52.,
                    23.,
                    false,
                    false,
                );
            }
        }
        Modal::Rules => {
            let (side, rw, labels, test_y, test_h, cw) = rules_geometry(pw, ph);
            let rx = x + side + 16.;
            for (id, yy) in [
                ("rule_name", 105.),
                ("rule_pattern", 136.),
                ("rule_replacement", 174.),
            ] {
                add(
                    id,
                    rx + labels,
                    y + yy,
                    rw - labels - 12.,
                    22.,
                    false,
                    false,
                );
            }
            add(
                "rule_test",
                rx + 20.,
                y + test_y + 62.,
                cw - 16.,
                test_h - 102.,
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
        Modal::Result => draw_result(f, hs, m, sess, t, x, y, pw, ph),
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
            let fs = fields(&m, 0., 0., pw, 660.);
            let raw = fs.iter().find(|z| z.id == "rule_test").unwrap();
            let (side, rw, _, _, _, cw) = rules_geometry(pw, 660.);
            assert!(raw.x >= side && raw.x + raw.w <= side + 16. + 12. + cw);
            assert!(raw.w > 100. && rw > raw.w * 2.);
            assert!(raw.y + raw.h < 598.);
        }
    }

    #[test]
    fn selecting_active_scope_does_not_toggle_to_persistent_storage() {
        let cfg = Config::default();
        let t = crate::ui::theme("墨玉");
        for persistent in [false, true] {
            let mut m = Model::default();
            m.modal = Modal::Rules;
            m.rule_save_preset = persistent;
            let mut f = Frame::new(1040., 660.);
            let mut hs = Vec::new();
            draw_rules(&mut f, &mut hs, &m, &cfg, &t, 0., 0., 1040., 660.);
            let rx = 348. + 16.;
            let local = crate::modals::hit(&hs, rx + 55., 217.);
            let saved = crate::modals::hit(&hs, rx + 112., 217.);
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
        m.modal = Modal::Result;
        m.latest = Some(result);
        let mut f = Frame::new(1280., 800.);
        draw_result(
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
        assert!(text.iter().any(|s| s.contains("字准 0.0%")));
        assert!(text.iter().any(|s| s.contains("键准 · 按键效率")));
        assert_eq!(m.latest.as_ref().unwrap().accuracy, 100.);
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
        assert!(report_text.contains(&"字准"));
        assert!(report_text.contains(&"键准·按键效率"));
        assert!(report_text.contains(&"100.0/0.0"));
        assert_eq!(sess.aggregate().accuracy, 100.);
    }
}
