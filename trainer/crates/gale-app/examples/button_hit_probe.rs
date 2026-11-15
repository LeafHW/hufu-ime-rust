#![allow(dead_code)]
#[path = "../src/analysis.rs"]
mod analysis;
#[path = "../src/ctxmenu.rs"]
mod ctxmenu;
#[path = "../src/history.rs"]
mod history;
#[path = "../src/keyboard.rs"]
mod keyboard;
#[path = "../src/modals.rs"]
mod modals;
#[path = "../src/secondary_panels.rs"]
mod secondary_panels;
#[path = "../src/session.rs"]
mod session;
#[path = "../src/ui.rs"]
mod ui;
pub use analysis::{FullAnalysis, FullAnalysisEntry};
pub fn aux_marker(cmd: &str) -> modals::Action {
    modals::Action::Theme(format!("\u{e000}{cmd}"))
}
use gale_core::config::{Config, DisplayMode};
use gale_win::d2d::{Align, DrawCmd, FontKit, Frame};
use modals::{Action, Modal, Model};

fn label(frame: &Frame, hit: &modals::Hit) -> String {
    frame
        .cmds
        .iter()
        .filter_map(|cmd| {
            if let DrawCmd::TextRun { x, y, text, .. } = cmd {
                (*x >= hit.x && *x < hit.x + hit.w && *y >= hit.y && *y < hit.y + hit.h)
                    .then_some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn inspect(
    name: &str,
    frame: &Frame,
    hits: &[modals::Hit],
    fields: &[modals::FieldSpec],
    counts: &mut [usize; 6],
) {
    for h in hits
        .iter()
        .filter(|h| h.w > 0. && h.h > 0. && h.h <= 62. && h.action != Action::None)
    {
        counts[0] += 1;
        for (site, x, y) in [
            ("center", h.x + h.w / 2., h.y + h.h / 2.),
            ("tl", h.x + 1., h.y + 1.),
            ("tr", h.x + h.w - 1., h.y + 1.),
            ("bl", h.x + 1., h.y + h.h - 1.),
            ("br", h.x + h.w - 1., h.y + h.h - 1.),
        ] {
            if modals::hit(hits, x, y).as_ref() != Some(&h.action) {
                counts[1] += 1;
                println!(
                    "OCCLUDED {name} {site} label={:?} action={:?} got={:?} xy=({x},{y})",
                    label(frame, h),
                    h.action,
                    modals::hit(hits, x, y)
                );
            }
        }
        for field in fields {
            let iw = (h.x + h.w).min(field.x + field.w) - h.x.max(field.x);
            let ih = (h.y + h.h).min(field.y + field.h) - h.y.max(field.y);
            if iw > 0.5 && ih > 0.5 {
                counts[2] += 1;
                println!(
                    "NATIVE_OVERLAP {name} field={} label={:?} action={:?} intersection={iw}x{ih}",
                    field.id,
                    label(frame, h),
                    h.action
                );
            }
        }
    }
    let mut fonts = FontKit::new();
    let mut clips: Vec<(f32, f32, f32, f32)> = Vec::new();
    for cmd in &frame.cmds {
        match cmd {
            DrawCmd::PushClip { x, y, w, h } => clips.push((*x, *y, *w, *h)),
            DrawCmd::PopClip => {
                clips.pop();
            }
            DrawCmd::TextRun {
                x,
                y,
                h,
                text,
                font,
                align,
                ..
            } => {
                let width = fonts.text_width(*font, *h, text);
                let left = match align {
                    Align::Left => *x,
                    Align::Center => *x - width / 2.,
                    Align::Right => *x - width,
                };
                let center = (left + width / 2., *y + *h * 0.6);
                if !clips.iter().all(|(x, y, w, h)| {
                    center.0 >= *x && center.0 < *x + *w && center.1 >= *y && center.1 < *y + *h
                }) {
                    continue;
                }
                let owner = hits.iter().rev().find(|hit| {
                    hit.h > 0.
                        && hit.h <= 62.
                        && hit.action != Action::None
                        && *x >= hit.x
                        && *x < hit.x + hit.w
                        && *y >= hit.y
                        && *y < hit.y + hit.h
                });
                if let Some(owner) = owner {
                    counts[3] += 1;
                    if modals::hit(hits, center.0, center.1).as_ref() != Some(&owner.action) {
                        counts[4] += 1;
                        println!("LABEL_MISS {name} text={text:?} center={center:?} expected={:?} got={:?}",owner.action,modals::hit(hits,center.0,center.1));
                    }
                    if fields.iter().any(|field| {
                        center.0 >= field.x
                            && center.0 < field.x + field.w
                            && center.1 >= field.y
                            && center.1 < field.y + field.h
                    }) {
                        counts[5] += 1;
                        println!(
                            "LABEL_NATIVE {name} text={text:?} center={center:?} action={:?}",
                            owner.action
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let _com = gale_win::window::ComSta::new().unwrap();
    let cfg = Config::default();
    let theme = ui::theme("墨玉");
    let sess = session::Session::new(100);
    let mut counts = [0; 6];
    for (w, h) in [(640., 420.), (800., 600.), (1038., 662.), (1280., 800.)] {
        for minimal in [false, true] {
            for split in [false, true] {
                for state in 0..3 {
                    let mut cfg = cfg.clone();
                    cfg.minimal = minimal;
                    cfg.display_mode = if split {
                        DisplayMode::Split
                    } else {
                        DisplayMode::Copybook
                    };
                    let mut s = session::Session::new(100);
                    if state > 0 {
                        s.load_text("甲乙丙丁");
                    }
                    if state == 2 {
                        s.feed_input(
                            gale_core::compare::InputEvent::Commit {
                                chars: "甲乙丙丁".into(),
                            },
                            100,
                        );
                    }
                    for dock in [false, true] {
                        let mut m = Model::default();
                        m.analysis_dock = dock;
                        let mut a = ui::Animation::default();
                        let mut hs = Vec::new();
                        let f = ui::build(
                            &s,
                            &cfg,
                            &theme,
                            &m,
                            w,
                            h,
                            200,
                            0.,
                            &mut a,
                            &mut hs,
                            &[(0, "甲乙丙丁".into(), vec![36.; 4])],
                        );
                        inspect(&format!("main {w}x{h} minimal{minimal} split{split} state{state} dock{dock}"),&f,&hs,&[],&mut counts);
                        if dock {
                            let header = modals::analysis_dock_rect_for(&cfg, w, h);
                            assert!(
                                !a.split_divider
                                    .is_some_and(|(_, dy, _, dh)| dy + dh > header.1),
                                "DIVIDER_OVERLAP {w}x{h} {minimal} {split} {state} divider={:?} dock={header:?}",
                                a.split_divider
                            );
                        }
                        let mut f = Frame::new(w, h);
                        m.menu = Some(modals::CtxMenu {
                            x: w / 2.,
                            y: h / 2.,
                        });
                        hs.clear();
                        ctxmenu::draw(&mut f, &mut hs, &cfg, &m, &theme, w, h);
                        inspect(&format!("menu {w}x{h} minimal{minimal} split{split} state{state} dock{dock}"),&f,&hs,&[],&mut counts);
                    }
                }
            }
        }
        for modal in [
            Modal::Preferences,
            Modal::Rules,
            Modal::Online,
            Modal::Groups,
            Modal::More,
            Modal::Heatmap,
            Modal::Analysis,
            Modal::Confirm,
        ] {
            for variant in 0..if modal == Modal::Preferences {
                8
            } else if modal == Modal::Online {
                8
            } else if modal == Modal::Groups {
                6
            } else if modal == Modal::Rules {
                4
            } else if modal == Modal::Analysis {
                6
            } else {
                1
            } {
                for scroll in [0., 100., 300., 1000.] {
                    let mut sess = session::Session::new(100);
                    let mut model = Model::default();
                    model.open(modal);
                    model.panel_scroll = scroll;
                    model.groups = vec!["审计群".into()];
                    model.bound = Some(0);
                    if modal == Modal::Groups {
                        let n = match variant {
                            0 => 0,
                            1 | 2 => 1,
                            _ => 15,
                        };
                        model.groups = (0..n)
                            .map(|i| format!("审计群 {i} · 长名称验证按钮文字与选择标记"))
                            .collect();
                        model.bound = match variant {
                            2 => Some(0),
                            4 | 5 => Some(14),
                            _ => None,
                        };
                        if variant == 5 {
                            model.scroll = 1000.;
                        }
                    }
                    if modal == Modal::Rules && variant > 0 {
                        model.rules = cfg
                            .cleaning_preset
                            .rules
                            .iter()
                            .map(|rule| {
                                (
                                    rule.name.clone(),
                                    rule.enabled,
                                    !rule.safe,
                                    rule.pattern.clone(),
                                    rule.replacement.clone(),
                                )
                            })
                            .collect();
                        model.rule_selected = if variant == 3 {
                            model.rules.len().saturating_sub(1)
                        } else {
                            0
                        };
                        model.rule_save_preset = variant == 2;
                        if variant == 3 {
                            model.scroll = 1000.;
                        }
                    }
                    model
                        .fields
                        .insert("pref_tab".into(), variant.min(5).to_string());
                    if variant == 6 {
                        model.fields.insert("pref_tab".into(), "0".into());
                        model
                            .fields
                            .insert("pref_hint_view".into(), "colors".into());
                    }
                    if variant == 7 {
                        model.fields.insert("pref_tab".into(), "0".into());
                        model
                            .fields
                            .insert("pref_hint_view".into(), "colors".into());
                        model
                            .fields
                            .insert("hint_rank_colors".into(), "true".into());
                    }
                    if modal == Modal::Online {
                        let state = variant % 4;
                        model.online_login_expanded = state < 2;
                        model.online_register_mode = state == 1;
                        model.online_authenticated = state >= 2;
                        model.online_expired = state == 3;
                        model.online_busy = variant >= 4;
                        model.online_code_wait = if variant == 5 { 59 } else { 0 };
                        model.online_categories = vec![
                            ("all".into(), "全部".into()),
                            ("literature".into(), "散文".into()),
                        ];
                        model.online_shelf = vec!["书架文章".into()];
                    }
                    if modal == Modal::Analysis && variant > 0 {
                        sess.load_text("理论分析按钮审计");
                        match variant {
                            1 => {
                                model
                                    .fields
                                    .insert("analysis_loading".into(), "true".into());
                            }
                            2 => {
                                model
                                    .fields
                                    .insert("analysis_error".into(), "测试分析失败，请重试".into());
                            }
                            _ => {
                                let count = if variant == 4 { 1000 } else { 20 };
                                sess.full_analysis = Some(FullAnalysis {
                                    schema: "审计词库 · 长名称验证标题与操作按钮的边界".into(),
                                    max_code_length: 4,
                                    entries: (0..count)
                                        .map(|i| FullAnalysisEntry {
                                            word: if i % 5 == 0 {
                                                "缺字验证".into()
                                            } else {
                                                "理论编码".into()
                                            },
                                            code: (i % 5 != 0).then_some("abcd".into()),
                                            code_len: 4,
                                            rank: (i % 9 + 1) as u64,
                                            ..Default::default()
                                        })
                                        .collect(),
                                    ..Default::default()
                                });
                                model.analysis_dock_scroll = scroll;
                                if variant == 5 {
                                    model
                                        .fields
                                        .insert("analysis_view".into(), "heatmap".into());
                                }
                            }
                        }
                    }
                    let (_, _, pw, ph) = modals::panel_for_model(&model, w, h);
                    for pin in [None, Some((16., 24.))] {
                        let (fw, fh) = if pin.is_some() {
                            ((pw + 32.).min(w), (ph + 48.).min(h))
                        } else {
                            (w, h)
                        };
                        let mut frame = Frame::new(fw, fh);
                        let mut hits = Vec::new();
                        modals::draw(
                            &mut frame,
                            &mut hits,
                            &model,
                            &sess,
                            &cfg,
                            &theme,
                            fw,
                            fh,
                            0.,
                            pin.is_none(),
                            pin,
                        );
                        let mut fields = modals::fields(&model, fw, fh, pin);
                        if let Some(font) = modals::font_picker(&model, fw, fh, pin) {
                            fields.push(font);
                        }
                        inspect(
                            &format!(
                                "{w}x{h} {modal:?} v{variant} scroll{scroll} overlay{}",
                                pin.is_some()
                            ),
                            &frame,
                            &hits,
                            &fields,
                            &mut counts,
                        );
                    }
                }
            }
        }
        for minimal in [false, true] {
            for split in [false, true] {
                let mut cfg = cfg.clone();
                cfg.minimal = minimal;
                cfg.display_mode = if split {
                    DisplayMode::Split
                } else {
                    DisplayMode::Copybook
                };
                let mut model = Model::default();
                model.analysis_dock = true;
                let mut frame = Frame::new(w, h);
                let mut hits = Vec::new();
                modals::draw_analysis_dock_for(
                    &mut frame, &mut hits, &model, &sess, &theme, &cfg, w, h,
                );
                inspect(
                    &format!("{w}x{h} dock minimal{minimal} split{split}"),
                    &frame,
                    &hits,
                    &[],
                    &mut counts,
                );
                for target in ["analysis_copy", "analysis_view"] {
                    let hit = hits
                        .iter()
                        .find(|hit| hit.action == aux_marker(target))
                        .unwrap();
                    let (x, y) = (hit.x + hit.w / 2., hit.y + hit.h / 2.);
                    let (top, bottom) = ui::body_region(&cfg, &model, w, h, false);
                    let g = ui::split_geom(
                        &cfg,
                        (w - 96.).max(360.),
                        cfg.font_size,
                        cfg.font_size * (1. + cfg.line_gap),
                        w,
                        top,
                        bottom,
                    );
                    println!("DOCK {w}x{h} minimal{minimal} split{split} {target} xy=({x},{y}) hit={:?} ledger_grip={:?} divider={:?} collapsed={}",modals::hit(&hits,x,y),history::resize_grip_rect(&cfg,&model,w,h),(g.divider_y,g.divider_y+12.),g.collapsed);
                }
            }
        }
    }
    println!("TOTAL controls={} button_points={} occluded_points={} native_overlaps={} label_centers={} label_misses={} label_native={}",counts[0],counts[0]*5,counts[1],counts[2],counts[3],counts[4],counts[5]);
    assert_eq!(counts[1], 0, "button centers or corners are occluded");
    assert_eq!(counts[2], 0, "native fields overlap actionable controls");
    assert_eq!(
        counts[4], 0,
        "visible label centers miss their owner controls"
    );
    assert_eq!(counts[5], 0, "native fields cover actionable labels");
}
