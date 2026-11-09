//! A compact ledger entry point; complete history and details share one report view.
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
    let ledger_top = h - 78.;
    if sess.phase == Phase::Done {
        let y = ledger_top;
        let record = sess
            .results
            .get(sess.current_index())
            .and_then(Option::as_ref);
        let has_next = sess.current_index() + 1 < sess.total_segments();
        let controls_w = if has_next { 282. } else { 186. };
        let summary = record
            .map(|r| format!("本段完成  {:.1} 字/分 · 键准 {:.1}%", r.speed, r.accuracy))
            .unwrap_or_else(|| "本段已完成".into());
        text(
            f,
            left,
            y + 10.,
            14.,
            elide(
                &summary,
                ((width - controls_w - 18.) / 14.).max(6.) as usize,
            ),
            p.text2,
        );
        let mut bx = w - 24. - controls_w;
        if has_next {
            button(f, hs, bx, y, 90., "下一段", Action::Next, t, true);
            bx += 96.;
        }
        button(f, hs, bx, y, 82., "重打", Action::Restart, t, !has_next);
        let detail = record
            .and_then(|r| {
                m.history.iter().rposition(|h| {
                    h.ts == r.ts && h.title == r.title && h.para_index == r.para_index
                })
            })
            .map(Action::SelectHistory)
            .unwrap_or(Action::Open(Modal::Report));
        button(f, hs, bx + 88., y, 98., "本段详情", detail, t, false);
        return;
    }
    round(f, left, ledger_top, width, 34., 8., p.surface, Some(p.line));
    text(
        f,
        left + 12.,
        ledger_top + 8.,
        15.,
        format!("成绩记录 · {} 条", m.history.len()),
        p.text2,
    );
    if let Some(r) = m.history.last().filter(|_| w >= 760.) {
        right(
            f,
            left + width - 34.,
            ledger_top + 9.,
            14.,
            format!(
                "最近 {:.1} 字/分 · 字准 {:.1}%",
                r.speed,
                r.character_accuracy()
            ),
            p.muted,
        );
    } else if m.history.is_empty() && w >= 600. {
        right(
            f,
            left + width - 34.,
            ledger_top + 9.,
            14.,
            "打完一段后自动记录",
            p.muted,
        );
    }
    right(f, left + width - 12., ledger_top + 6., 18., "›", p.muted);
    hs.push(Hit {
        x: left,
        y: ledger_top,
        w: width,
        h: 34.,
        action: Action::Open(Modal::Report),
    });
}
