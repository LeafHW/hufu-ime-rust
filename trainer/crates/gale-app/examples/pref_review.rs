//! 开发期 F1 设置页布局 fixture：六页 × 1280×800 / 800×600 / 640×420。
//!
//! 运行（trainer 目录）：
//! cargo run -p gale-app --example pref_review --target x86_64-pc-windows-gnu -- <输出目录>
//!
//! 只创建一个隐藏渲染窗口，不抓桌面、不读写用户设置、不联网。原生编辑框
//! （成绩单模板、颜色值）用 D2D 按真实 FieldSpec 矩形绘制样本值，因此这里验证
//! 的是排版与坐标，不验证 TSF 输入与焦点。
#![allow(dead_code)]

#[path = "../src/analysis.rs"]
mod analysis;
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

const AUX_PREFIX: char = '\u{e000}';

pub fn aux_marker(cmd: &str) -> modals::Action {
    modals::Action::Theme(format!("{AUX_PREFIX}{cmd}"))
}

use gale_core::{
    config::Config,
    rules::{SessionRules, TrainingMode},
    score::ScoreRecord,
    segment::{ParsedGroupText, Segment},
};
use gale_win::{
    d2d::{DrawCmd, Renderer},
    window::{ComSta, MainWindow},
};
use modals::{Modal, Model};
use std::error::Error;
use std::path::PathBuf;
use windows::Win32::UI::{
    HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_UNAWARE},
    WindowsAndMessaging::DestroyWindow,
};

const WIDTH: f32 = 1280.;
const HEIGHT: f32 = 800.;
const NOW_MS: u64 = 14_300;
const ARTICLE: &str = "庆历四年春，滕子京谪守巴陵郡。越明年，政通人和，百废具兴。乃重修岳阳楼，增其旧制，刻唐贤今人诗赋于其上。";

struct HiddenWindow(MainWindow);
impl Drop for HiddenWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0.hwnd());
        }
    }
}

fn article_session() -> session::Session {
    let mut sess = session::Session::new(100);
    sess.set_rules(SessionRules {
        mode: TrainingMode::Practice,
        word_hints: false,
        ..SessionRules::default()
    })
    .expect("fixture rules");
    sess.load_group(ParsedGroupText {
        title: "岳阳楼记".into(),
        difficulty: Some("水".into()),
        segments: vec![Segment {
            mark: "1".into(),
            text: ARTICLE.into(),
        }],
    });
    sess
}

fn latest_score() -> ScoreRecord {
    serde_json::from_str(
        r#"{"ts":1794008400,"date":"2026-11-09","title":"岳阳楼记","source":"群文","mark":"1","para_index":0,"chars":196,"keys":208,"backspaces":6,"wrong":2,"time_ms":58285,"speed":201.77,"accuracy":94.71,"code_len":1.06,"word_rate":86.2,"select_count":3,"ding_count":1,"corrections":1,"strokes_per_sec":3.57,"accuracy_pro":94.71,"effective_speed":196.7,"input_method":"虎符"}"#,
    )
    .expect("fixture record")
}

fn seed_preferences(cfg: &Config, m: &mut Model, theme: &gale_core::theme::Theme) {
    for (key, value) in [
        ("hint_show_code", cfg.hint_show_code),
        ("hint_show_rank", cfg.hint_show_rank),
        ("show_preedit", cfg.show_preedit),
        ("score_auto_copy", cfg.score_auto_copy),
        ("auto_send_score", false),
    ] {
        m.fields.insert(key.into(), value.to_string());
    }
    m.fields
        .insert("hint_font_size".into(), cfg.hint_font_size.to_string());
    for (key, value) in [
        ("hint_code_offset", cfg.hint_code_offset),
        ("hint_rank_offset", cfg.hint_rank_offset),
        ("preedit_font_size", cfg.preedit_font_size),
        ("preedit_offset", cfg.preedit_offset),
    ] {
        m.fields.insert(key.into(), value.to_string());
    }
    m.fields.insert(
        "score_fields".into(),
        "segment,speed,key_accuracy,character_accuracy,code_len,keys,wrong,backspaces,select_count,retype_count,chars,time"
            .into(),
    );
    m.fields.insert(
        "score_template".into(),
        gale_core::scorecard::SCORE_PRESETS[1].template.into(),
    );
    m.fields.insert("score_preset".into(), "detailed".into());
    m.fields
        .insert("pref_color_key".into(), "跟打区背景色".into());
    for (key, _, _) in [
        ("跟打区背景色", "", ""),
        ("对打未打底色", "", ""),
        ("打对字色", "", ""),
        ("对打打对底色", "", ""),
        ("打错字色", "", ""),
        ("对打打错底色", "", ""),
        ("词提编码色", "", ""),
        ("词提角标色", "", ""),
    ] {
        let color = ui::setting_color(theme, key);
        m.fields.insert(
            format!("color_{key}"),
            format!(
                "{:02x}{:02x}{:02x}",
                (color[0] * 255.).round() as u8,
                (color[1] * 255.).round() as u8,
                (color[2] * 255.).round() as u8
            ),
        );
    }
    m.fields.insert(
        "pref_color_hex".into(),
        m.field("color_跟打区背景色"),
    );
}

fn paint_field_fixtures(
    frame: &mut gale_win::d2d::Frame,
    m: &Model,
    t: &gale_core::theme::Theme,
    width: f32,
    height: f32,
) {
    let p = ui::tokens(t);
    for field in modals::fields(m, width, height, None) {
        if field.h <= 0. || field.w <= 0. {
            continue;
        }
        let value = m.field(field.id);
        ui::rect(frame, field.x, field.y, field.w, field.h, p.bg2);
        frame.push(DrawCmd::PushClip {
            x: field.x,
            y: field.y,
            w: field.w,
            h: field.h,
        });
        let lines = if field.multi {
            modals::wrap(&value, ((field.w - 12.) / 15.).max(1.) as usize)
        } else {
            vec![value]
        };
        for (row, line) in lines.into_iter().take(3).enumerate() {
            ui::text(
                frame,
                field.x + 6.,
                field.y + 5. + row as f32 * 20.,
                15.,
                line,
                p.text2,
            );
        }
        frame.push(DrawCmd::PopClip);
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("Usage: pref_review <output-directory>")?;
    std::fs::create_dir_all(&output)?;
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE);
    }
    let _com = ComSta::new()?;
    let window = HiddenWindow(MainWindow::create(
        "HuFu pref fixture (hidden)",
        WIDTH as i32,
        HEIGHT as i32,
    )?);
    let scale = gale_win::window::dpi_scale();
    if (scale - 1.).abs() > 0.001 {
        return Err(format!("Fixture expected a 96-DPI thread; actual scale={scale}").into());
    }
    let mut renderer = Renderer::new(window.0.hwnd, WIDTH as u32, HEIGHT as u32)?;

    let mut manifest = Vec::new();
    let mut shots: Vec<(f32, f32, u8, f32, &str)> = Vec::new();
    for (width, height) in [(WIDTH, HEIGHT), (800., 600.)] {
        for tab in 0..=5u8 {
            shots.push((width, height, tab, 0., ""));
        }
    }
    for (width, height) in [(WIDTH, HEIGHT), (800., 600.)] {
        for tab in 0..=5u8 {
            // 紧凑面板在大屏也可能需要滚动，不能只验首屏。
            shots.push((width, height, tab, 10_000., "-scrolled"));
        }
    }
    for tab in 0..=5u8 {
        // 640×420 是最小窗，design_review 也会出；这里先确认 F1 不破版。
        shots.push((640., 420., tab, 0., ""));
        shots.push((640., 420., tab, 10_000., "-scrolled"));
    }
    shots.push((WIDTH, HEIGHT, 0, 230., "-hint-large"));
    shots.push((640., 420., 0, 10_000., "-hint-large"));
    shots.push((WIDTH, HEIGHT, 3, 0., "-preedit-custom"));
    shots.push((WIDTH, HEIGHT, 3, 0., "-preedit-large"));
    shots.push((640., 420., 3, 0., "-preedit-large"));
    shots.push((WIDTH, HEIGHT, 3, 0., "-preedit-hidden"));
    for (width, height, tab, scroll, suffix) in shots {
        {
            let mut sess = article_session();
            let mut cfg = Config {
                font_size: 54.,
                line_gap: 0.66,
                smooth_caret: false,
                shake_on_wrong: false,
                ..Config::default()
            };
            cfg.theme = "墨玉".into();
            if suffix == "-hint-large" {
                cfg.hint_font_size = 100.;
                cfg.hint_code_offset = 18.;
                cfg.hint_rank_offset = -6.;
            }
            if suffix == "-preedit-custom" {
                cfg.preedit_font_size = 26.;
                cfg.preedit_offset = -8.;
            }
            if suffix == "-preedit-large" { cfg.preedit_font_size = 100.; }
            if suffix == "-preedit-hidden" { cfg.show_preedit = false; }
            let theme = ui::theme("墨玉");
            let mut m = Model::default();
            m.hints = true;
            m.open(Modal::Preferences);
            m.fields.insert("pref_tab".into(), tab.to_string());
            m.latest = Some(latest_score());
            seed_preferences(&cfg, &mut m, &theme);
            let name = format!(
                "{}-{}x{}{}.bmp",
                tab_name(tab),
                width as u32,
                height as u32,
                suffix
            );
            m.panel_scroll = scroll.min(modals::panel_scroll_max(&m, width, height, height));
            renderer.resize_lossy(width as u32, height as u32);
            let (body_w, font_size, _) = ui::body_metrics(&sess, &cfg, width);
            let lines = renderer.wrap_text(&sess.current_segment().text, font_size, body_w);
            let mut animation = ui::Animation {
                wrong_at: -100.,
                correct_at: -100.,
                ..ui::Animation::default()
            };
            let mut hits = Vec::new();
            let mut frame = ui::build(
                &sess,
                &cfg,
                &theme,
                &m,
                width,
                height,
                NOW_MS,
                NOW_MS as f64 / 1000.,
                &mut animation,
                &mut hits,
                &lines,
            );
            modals::draw(
                &mut frame,
                &mut hits,
                &m,
                &sess,
                &cfg,
                &theme,
                width,
                height,
                NOW_MS as f64 / 1000.,
                false,
                None,
            );
            paint_field_fixtures(&mut frame, &m, &theme, width, height);
            renderer.draw_frame(&frame)?;
            renderer
                .save_bmp(&output.join(&name))
                .map_err(std::io::Error::other)?;
            let field_manifest: Vec<_> = modals::fields(&m, width, height, None)
                .into_iter()
                .map(|field| {
                    serde_json::json!({
                        "id": field.id,
                        "rect": [field.x, field.y, field.w, field.h],
                        "multiline": field.multi
                    })
                })
                .collect();
            manifest.push(serde_json::json!({
                "tab": tab,
                "tab_name": tab_name(tab),
                "file": name,
                "logical_size": [width, height],
                "panel_scroll": m.panel_scroll,
                "show_preedit":cfg.show_preedit,
                "preedit_font_size":cfg.preedit_font_size,
                "preedit_offset":cfg.preedit_offset,
                "panel": modals::panel_for_model(&m, width, height),
                "native_fields": field_manifest,
                "hits": hits.len(),
                "draw_commands": frame.cmds.len(),
            }));
            println!("tab {tab} {width}x{height}: {}", output.join(&name).display());
        }
    }
    std::fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "kind": "development-layout-fixture",
            "dpi": 96,
            "screens": manifest,
            "notice": "F1 设置六页在 1280×800、800×600 与 640×420 下的真实 Frame/D2D 布局，含顶端/滚动到底及键入提示开关、100px字号；原生编辑框样本值由 D2D 按 FieldSpec 绘制，不验证 TSF 输入。"
        }))?,
    )?;
    Ok(())
}

fn tab_name(tab: u8) -> &'static str {
    match tab {
        0 => "practice",
        1 => "score",
        2 => "colors",
        3 => "interface",
        4 => "data",
        _ => "shortcuts",
    }
}
