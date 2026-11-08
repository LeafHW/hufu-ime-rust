//! Persistent per-attempt ledger; collapsed by default and independent of loaded text.
use crate::{
    modals::{Action, Hit, Modal, Model},
    session::{Phase, Session},
    ui::*,
};
use gale_core::{config::Config, theme::Theme};
use gale_win::d2d::Frame;

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
    let p = tokens(t);
    let left = 24.;
    let width = w - 48.;
    let ledger_top = if m.history_expanded {
        h - 244.
    } else {
        h - 78.
    };
    if sess.phase == Phase::Done {
        let y = ledger_top - 48.;
        let summary = sess
            .results
            .get(sess.current_index())
            .and_then(Option::as_ref)
            .map(|r| {
                format!(
                    "本段完成  {:.1} 字/分 · 码长 {:.2} · 键准 {:.1}%",
                    r.speed, r.code_len, r.accuracy
                )
            })
            .unwrap_or_else(|| "本段已完成".into());
        text(
            f,
            left,
            y + 10.,
            14.,
            elide(&summary, ((w - 340.) / 14.).max(8.) as usize),
            p.text2,
        );
        button(f, hs, w - 306., y, 90., "下一段", Action::Next, t, true);
        button(f, hs, w - 210., y, 82., "重打", Action::Restart, t, false);
        button(
            f,
            hs,
            w - 122.,
            y,
            98.,
            "本段详情",
            Action::Open(Modal::Report),
            t,
            false,
        );
    }
    round(
        f,
        left,
        ledger_top,
        width,
        if m.history_expanded { 198. } else { 34. },
        8.,
        p.surface,
        Some(p.line),
    );
    text(
        f,
        left + 12.,
        ledger_top + 8.,
        15.,
        format!(
            "{} 成绩记录 · 共 {} 段",
            if m.history_expanded { "▾" } else { "▸" },
            m.history.len()
        ),
        p.text2,
    );
    hs.push(Hit {
        x: left,
        y: ledger_top,
        w: if m.history_expanded {
            (width - 240.).max(220.)
        } else {
            width - 12.
        },
        h: 34.,
        action: crate::aux_marker("history_toggle"),
    });
    if !m.history_expanded {
        if let Some(r) = m.history.last().filter(|_| w >= 900.) {
            let char_acc = if r.chars == 0 {
                0.
            } else {
                100. * (r.chars.saturating_sub(r.wrong)) as f64 / r.chars as f64
            };
            right(
                f,
                left + width - 14.,
                ledger_top + 9.,
                14.,
                format!(
                    "最近 {:.1} 字/分 · 字准 {:.1}% · 点击展开",
                    r.speed, char_acc
                ),
                p.muted,
            );
        }
        return;
    }
    let columns: &[(&str, f32)] = if w >= 1040. {
        &[
            ("段号 / 文章", 0.),
            ("速度", 0.37),
            ("字准", 0.47),
            ("键准", 0.56),
            ("码长", 0.65),
            ("词率", 0.73),
            ("选重", 0.82),
            ("重打", 0.90),
        ]
    } else {
        &[
            ("段号 / 文章", 0.),
            ("速度", 0.44),
            ("键准", 0.59),
            ("码长", 0.73),
            ("重打", 0.87),
        ]
    };
    for (label, ratio) in columns {
        text(
            f,
            left + 12. + ratio * (width - 24.),
            ledger_top + 40.,
            14.,
            *label,
            p.muted,
        );
    }
    line(f, left + 12., ledger_top + 63., width - 24., p.line);
    let offset = m.history_scroll.min(m.history.len().saturating_sub(1));
    for (row, (i, r)) in m
        .history
        .iter()
        .enumerate()
        .rev()
        .skip(offset)
        .take(4)
        .enumerate()
    {
        let y = ledger_top + 71. + row as f32 * 31.;
        if row % 2 == 0 {
            rect(f, left + 5., y - 4., width - 10., 29., alpha(p.text, 0.025));
        }
        let segment = if r.mark.is_empty() {
            format!("第{}段", r.para_index + 1)
        } else {
            r.mark.clone()
        };
        let valid = if r.is_valid { "" } else { " · 练习" };
        text(
            f,
            left + 12.,
            y,
            14.,
            elide(
                &format!("{} · {}{}", segment, r.title, valid),
                ((width * if w >= 1040. { 0.35 } else { 0.41 }) / 14.) as usize,
            ),
            p.text2,
        );
        let at = |ratio: f32| left + 12. + ratio * (width - 24.);
        let word = r
            .word_rate
            .map(|n| format!("{n:.1}%"))
            .unwrap_or_else(|| "—".into());
        let char_acc = if r.chars == 0 {
            0.
        } else {
            100. * (r.chars.saturating_sub(r.wrong)) as f64 / r.chars as f64
        };
        let vals = if w >= 1040. {
            vec![
                (0.37, format!("{:.1}", r.speed)),
                (0.47, format!("{char_acc:.1}%")),
                (0.56, format!("{:.1}%", r.accuracy)),
                (0.65, format!("{:.2}", r.code_len)),
                (0.73, word),
                (0.82, r.select_count.to_string()),
                (0.90, r.retype_count.to_string()),
            ]
        } else {
            vec![
                (0.44, format!("{:.1}", r.speed)),
                (0.59, format!("{:.1}%", r.accuracy)),
                (0.73, format!("{:.2}", r.code_len)),
                (0.87, r.retype_count.to_string()),
            ]
        };
        for (ratio, value) in vals {
            mono(f, at(ratio), y, 14., value, p.text2);
        }
        hs.push(Hit {
            x: left,
            y: y - 4.,
            w: width,
            h: 29.,
            action: crate::aux_marker(&format!("history_open:{i}")),
        });
    }
    if m.history.is_empty() {
        text(
            f,
            left + 14.,
            ledger_top + 87.,
            14.,
            "打完一段后自动记录，换文后仍可查看。",
            p.muted,
        );
    }
    if m.history.len() > 4 {
        button(
            f,
            hs,
            w - 220.,
            ledger_top + 3.,
            90.,
            "较新",
            crate::aux_marker("history_step:-4"),
            t,
            false,
        );
        button(
            f,
            hs,
            w - 124.,
            ledger_top + 3.,
            90.,
            "较早",
            crate::aux_marker("history_step:4"),
            t,
            false,
        );
    }
}
