//! Development-only native D2D layout fixtures for trainer screens, themes and sizes.
//!
//! Run from trainer/:
//! cargo run -p gale-app --example design_review --target x86_64-pc-windows-gnu -- <output-dir>
//!
//! This program creates one hidden render surface. It does not capture the desktop,
//! create an InputHost, connect to QQ/online services, or read/write user settings.
//! RichEdit values are painted with D2D at the real FieldSpec rectangles, so these
//! images verify layout and rendering, not TSF composition or native edit behavior.
#![allow(dead_code)]

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

// The fixture uses the same analysis model and display rules as the application.
const AUX_PREFIX: char = '\u{e000}';
pub fn aux_marker(cmd: &str) -> modals::Action {
    modals::Action::Theme(format!("{AUX_PREFIX}{cmd}"))
}

#[path = "../src/analysis.rs"]
mod analysis;
pub use analysis::{FullAnalysis, FullAnalysisEntry};

use gale_core::{
    compare::InputEvent,
    config::Config,
    rules::{clean_body, CleaningPreset, CleaningRule, SessionRules, TrainingMode},
    score::ScoreRecord,
    segment::{ParsedGroupText, Segment},
    single::{builtin_tables, parse_items, SingleSession},
    wenlai::{Difficulty, WenlaiArticle},
};
use gale_win::{
    d2d::{DrawCmd, Renderer},
    window::{ComSta, MainWindow},
};
use modals::{HintWord, Modal, Model};
use session::Session;
use std::{collections::BTreeMap, error::Error, path::PathBuf};
use windows::Win32::UI::{
    HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_UNAWARE},
    WindowsAndMessaging::DestroyWindow,
};

const WIDTH: f32 = 1280.;
const HEIGHT: f32 = 800.;
const NOW_MS: u64 = 14_300;
const UNIX_FIXTURE: u64 = 1_794_008_400;
const CHAR_DICT: &str = include_str!("../../../../assets/码表/虎码单字/tiger.dict.yaml");
const PARAGRAPHS: [&str; 4] = [
    "庆历四年春，滕子京谪守巴陵郡。越明年，政通人和，百废具兴。乃重修岳阳楼，增其旧制，刻唐贤今人诗赋于其上。属予作文以记之。",
    "予观夫巴陵胜状，在洞庭一湖。衔远山，吞长江，浩浩汤汤，横无际涯；朝晖夕阴，气象万千。此则岳阳楼之大观也，前人之述备矣。然则北通巫峡，南极潇湘，迁客骚人，多会于此，览物之情，得无异乎？",
    "若夫淫雨霏霏，连月不开，阴风怒号，浊浪排空；日星隐曜，山岳潜形；商旅不行，樯倾楫摧；薄暮冥冥，虎啸猿啼。登斯楼也，则有去国怀乡，忧谗畏讥，满目萧然，感极而悲者矣。",
    "嗟夫！予尝求古仁人之心，或异二者之为，何哉？不以物喜，不以己悲；居庙堂之高则忧其民，处江湖之远则忧其君。是进亦忧，退亦忧。然则何时而乐耶？其必曰：先天下之忧而忧，后天下之乐而乐乎！噫！微斯人，吾谁与归？",
];

struct HiddenWindow(MainWindow);
impl Drop for HiddenWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0.hwnd());
        }
    }
}

/// Dictionary data is compiled only into this development example. The product
/// still gets single-character hints from the real server.
fn dictionaries() -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for row in CHAR_DICT.lines() {
        let cols: Vec<&str> = row.split('\t').collect();
        if cols.len() <= 1 || cols[0].chars().count() != 1 {
            continue;
        }
        let code = cols[1];
        if code.is_empty() || !code.bytes().all(|b| b.is_ascii_lowercase()) {
            continue;
        }
        let word = cols[0].to_string();
        if out.get(&word).is_none_or(|old| code.len() < old.len()) {
            out.insert(word, code.to_string());
        }
    }
    out
}

fn article(mode: TrainingMode) -> Session {
    let mut sess = Session::new(100);
    sess.set_rules(SessionRules {
        mode,
        word_hints: false,
        ..SessionRules::default()
    })
    .expect("fixture rules");
    sess.load_group(ParsedGroupText {
        title: "岳阳楼记".into(),
        difficulty: Some("水".into()),
        segments: ["记事", "大观", "悲喜", "议论"]
            .into_iter()
            .zip(PARAGRAPHS)
            .map(|(mark, text)| Segment {
                mark: mark.into(),
                text: text.into(),
            })
            .collect(),
    });
    sess
}

/// Exercise the same key/commit path as the application, at deterministic times.
/// One wrong character can remain, and one earlier character is corrected with a
/// real physical Backspace plus InputEvent::Backspace.
fn fixture_key(sess: &mut Session, vk: u32, tick: u64) {
    sess.on_key(vk, tick);
    // The application records physical heatmap events separately from score
    // keys. Feed this same synthetic event to both paths before finalization.
    match vk & 0xff {
        0 | 0xe5 | 0xff => {}
        key => *sess.vk_histogram.entry(key as u8).or_insert(0) += 1,
    }
}

fn type_fixture(
    sess: &mut Session,
    chars: usize,
    duration_ms: u64,
    wrong_at: Option<usize>,
    correction_at: Option<usize>,
) -> u64 {
    let target: Vec<char> = sess.current_segment().text.chars().take(chars).collect();
    let count = target.len().max(1);
    for (i, ch) in target.into_iter().enumerate() {
        let tick = 1000 + (i as u64 * duration_ms / count as u64);
        sess.set_composing(true);
        for key in 0..(if i % 3 == 0 { 3 } else { 2 }) {
            fixture_key(sess, 0x41 + key, tick + key as u64 * 15);
        }
        let typed = if wrong_at == Some(i) || correction_at == Some(i) {
            if ch == '庭' {
                '廷'
            } else {
                '错'
            }
        } else {
            ch
        };
        let commit_at = 1000 + ((i + 1) as u64 * duration_ms / count as u64);
        sess.feed_input(
            InputEvent::Commit {
                chars: typed.to_string(),
            },
            commit_at,
        );
        if correction_at == Some(i) && i + 1 < count {
            fixture_key(sess, 0x08, commit_at + 1);
            sess.feed_input(InputEvent::Backspace, commit_at + 1);
            sess.set_composing(true);
            fixture_key(sess, 0x41, commit_at + 2);
            fixture_key(sess, 0x42, commit_at + 3);
            sess.feed_input(
                InputEvent::Commit {
                    chars: ch.to_string(),
                },
                commit_at + 4,
            );
        }
    }
    1000 + duration_ms
}

fn finish_segment(sess: &mut Session, index: usize) -> ScoreRecord {
    sess.jump(index);
    let chars = sess.current_segment().text.chars().count();
    let desired_speed = [198.4, 224.8, 216.2, 219.1][index];
    let duration = (chars as f64 / desired_speed * 60_000.).round() as u64;
    let end = type_fixture(sess, chars, duration, Some(10), Some(5));
    let mut record = sess
        .finalize_current_at(end, UNIX_FIXTURE + index as u64 * 40)
        .expect("completed fixture score");
    record.input_method = "虎符".into();
    sess.results[index] = Some(record.clone());
    record
}

fn practice_fixture() -> (Session, Model) {
    let mut sess = article(TrainingMode::Practice);
    let first = finish_segment(&mut sess, 0);
    sess.jump(1);
    type_fixture(&mut sess, 48, NOW_MS - 1000, Some(10), Some(5));
    let mut m = model();
    m.history.push(first.clone());
    m.latest = Some(first);
    (sess, m)
}

fn full_analysis_fixture(sess: &mut Session) {
    let words: [(&str, &str, u64, u64); 24] = [
        ("岳阳楼记", "yylj", 4, 1),
        ("庆历四年春", "qlsn", 4, 1),
        ("滕子京", "tzj", 3, 1),
        ("谪守巴陵郡", "zsbl", 4, 1),
        ("越明年", "ymn", 3, 1),
        ("政通人和", "ztrh", 4, 1),
        ("百废具兴", "bfjx", 4, 1),
        ("乃重修", "ncx", 3, 1),
        ("岳阳楼", "yyl", 3, 1),
        ("增其旧制", "zqjz", 4, 1),
        ("刻唐贤今人诗赋于其上", "ktx", 4, 1),
        ("属予作文以记之", "sywz", 4, 1),
        ("予观夫巴陵胜状", "ygfb", 4, 1),
        ("在洞庭一湖", "zdty", 4, 1),
        ("衔远山", "xys", 3, 1),
        ("吞长江", "tcj", 3, 1),
        ("浩浩汤汤", "hhtt", 4, 1),
        ("横无际涯", "hwjy", 4, 1),
        ("朝晖夕阴", "zhxy", 4, 1),
        ("气象万千", "qxwq", 4, 1),
        ("此则", "cz", 2, 1),
        ("岳阳楼之大观也", "yylz", 4, 1),
        ("前人之述备矣", "qrzs", 4, 1),
        ("然则北通巫峡", "rzbt", 4, 1),
    ];
    // Enough entries to exercise real page overflow and the scroll thumb.
    let entries = (0..180)
        .map(|i| {
            let (word, code, code_len, rank) = words[i % words.len()];
            analysis::FullAnalysisEntry {
                word: word.into(),
                code: Some(code.into()),
                code_len,
                rank: if i % 7 == 0 {
                    2
                } else if i % 11 == 0 {
                    3
                } else {
                    rank
                },
                tail: if i % 7 == 0 {
                    "1".into()
                } else {
                    String::new()
                },
            }
        })
        .collect::<Vec<_>>();
    let total: u64 = entries.iter().map(|e| e.code_len).sum();
    sess.full_analysis = Some(analysis::FullAnalysis {
        entries,
        total_code_len: total,
        avg_code_len_per_char: 2.37,
        select_count: 3,
        schema: "虎码 · 标准".into(),
        schema_ver: "2026-11".into(),
        max_code_length: 4,
    });
}

fn model() -> Model {
    let mut m = Model::default();
    m.hints = false;
    m.reduced_motion = false;
    m.quote_relaxed = true;
    m.status = "已绑定 虎码训练营".into();
    m.schema = "tiger".into();
    m.schema_ver = "2026.08.14".into();
    m.groups = ["虎码训练营", "五笔养老院", "跟打·夜场", "古文观止"]
        .into_iter()
        .map(str::to_string)
        .collect();
    m.bound = Some(0);
    m.books = [
        "岳阳楼记",
        "滕王阁序",
        "逍遥游",
        "核舟记",
        "醉翁亭记",
        "兰亭集序",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    m
}

fn single_hints(sess: &Session, dict: &BTreeMap<String, String>) -> Vec<HintWord> {
    sess.current_segment()
        .text
        .chars()
        .enumerate()
        .map(|(i, ch)| {
            let word = ch.to_string();
            let code = dict.get(&word).cloned();
            // Tail markers are layout samples only; no input-method recipe is inferred.
            let tail = if code.as_ref().is_some_and(|c| c.len() == 4) {
                "_"
            } else {
                ""
            };
            HintWord {
                text: word,
                code,
                tail: tail.into(),
                start: i,
                len: 1,
                rank: 1,
            }
        })
        .collect()
}

fn drill_fixture() -> Session {
    let tables = builtin_tables();
    let (title, text) = tables[0];
    let drill = SingleSession {
        title: title.into(),
        group_size: 18,
        items: parse_items(text, 1),
        group_index: 0,
        seed: 0,
    };
    let mut sess = Session::new(18);
    sess.load_single(drill);
    sess.jump(2);
    type_fixture(&mut sess, 2, 663, None, None);
    sess
}

fn online_fixture(m: &mut Model) {
    m.online_authenticated = true;
    m.online_expired = true;
    m.online_account = "墨池".into();
    m.online_status = "网络失败 · 已保留当前文《岳阳楼记》".into();
    m.online_title = "岳阳楼记".into();
    m.online_preview = PARAGRAPHS[1].into();
    m.online_categories = ["古文", "诗词", "现代", "全部"]
        .into_iter()
        .map(|s| (s.into(), s.into()))
        .collect();
    m.online_shelf = [
        "岳阳楼记 · 水 · 第 2 段",
        "滕王阁序 · 普 · 第 1 段",
        "逍遥游 · 难 · 第 3 段",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    m.online_difficulty = 1;
    m.online_article = Some(WenlaiArticle {
        title: "岳阳楼记".into(),
        difficulty: Difficulty::Shui,
        text: PARAGRAPHS[1].into(),
        seg_mark: "2-4".into(),
        difficulty_score: 1.84,
        book_id: "fixture-yueyanglou".into(),
        sort_num: 2,
        category: "古文".into(),
        end_sort_num: 2,
        end_chars: "乎？".into(),
        start_chars: "予观".into(),
    });
    m.fields
        .insert("server".into(), "https://qingfawen.fcxxz.com".into());
    m.fields.insert("username".into(), "fixture-account".into());
    m.fields.insert("password".into(), "fixture-only".into());
    m.fields.insert("online_chars".into(), "800".into());
}

fn cleaning_fixture(cfg: &mut Config, m: &mut Model) {
    cfg.cleaning_preset = CleaningPreset::builtin();
    cfg.cleaning_preset.rules.extend([
        CleaningRule {
            id: "fixture-repeat".into(),
            name: "重复标点（组）".into(),
            pattern: "，{2,}".into(),
            replacement: "，".into(),
            enabled: true,
            safe: false,
            builtin: false,
        },
        CleaningRule {
            id: "fixture-book".into(),
            name: "书名号归一".into(),
            pattern: "〈([^〉]*)〉".into(),
            replacement: "《$1》".into(),
            enabled: true,
            safe: false,
            builtin: false,
        },
    ]);
    m.rules = cfg
        .cleaning_preset
        .rules
        .iter()
        .map(|r| {
            (
                r.name.clone(),
                r.enabled,
                !r.safe,
                r.pattern.clone(),
                r.replacement.clone(),
            )
        })
        .collect();
    m.rule_selected = 3;
    m.rule_save_preset = true;
    let selected = &cfg.cleaning_preset.rules[m.rule_selected];
    m.fields.insert("rule_name".into(), selected.name.clone());
    m.fields
        .insert("rule_pattern".into(), selected.pattern.clone());
    m.fields
        .insert("rule_replacement".into(), selected.replacement.clone());
    let raw = "\u{FEFF}“明月”　照着小楼，山河—–无恙。";
    let result = clean_body(raw, &cfg.cleaning_preset, TrainingMode::Practice)
        .expect("fixture cleaning rules");
    m.fields.insert("rule_test".into(), raw.into());
    m.rule_test_source = result.raw_text;
    m.rule_preview = result.cleaned_text;
    m.rule_hits = result.hits.iter().map(|(_, n)| n).sum();
}

fn seed_preferences(cfg: &Config, m: &mut Model, theme: &gale_core::theme::Theme) {
    for (key, value) in [
        ("hint_show_code", cfg.hint_show_code),
        ("hint_show_rank", cfg.hint_show_rank),
        ("score_auto_copy", cfg.score_auto_copy),
        ("auto_send_score", cfg.auto_send_score),
    ] {
        m.fields.insert(key.into(), value.to_string());
    }
    m.fields
        .insert("hint_font_size".into(), cfg.hint_font_size.to_string());
    m.fields
        .insert("score_fields".into(), cfg.score_fields.clone());
    m.fields
        .insert("score_template".into(), cfg.score_template.clone());
    m.fields.insert("score_preset".into(), "detailed".into());
    m.fields
        .insert("pref_color_key".into(), "跟打区背景色".into());
    for key in [
        "跟打区背景色",
        "对打未打底色",
        "打对字色",
        "对打打对底色",
        "打错字色",
        "对打打错底色",
        "词提编码色",
        "词提角标色",
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
    m.fields
        .insert("pref_color_hex".into(), m.field("color_跟打区背景色"));
}

fn completed_fixture(m: &mut Model, mode: TrainingMode, done: usize) -> Session {
    let mut sess = article(mode);
    for index in 0..done {
        m.latest = Some(finish_segment(&mut sess, index));
    }
    m.race = mode == TrainingMode::Race;
    m.hints = false;
    m.history = sess.results.iter().flatten().cloned().collect();
    sess
}

fn history_fixture(m: &mut Model, _session_view: bool, select_older: bool) -> Session {
    let sess = completed_fixture(m, TrainingMode::Practice, 4);
    let mut previous = Session::new(100);
    previous.load_group(ParsedGroupText {
        title: "醉翁亭记".into(),
        difficulty: None,
        segments: vec![Segment {
            mark: "1".into(),
            text: "环滁皆山也。其西南诸峰，林壑尤美，望之蔚然而深秀者，琅琊也。山行六七里，渐闻水声潺潺，而泻出于两峰之间者，酿泉也。".into(),
        }],
    });
    let chars = previous.current_segment().text.chars().count();
    let end = type_fixture(&mut previous, chars, 18_200, Some(12), Some(4));
    let mut old_record = previous
        .finalize_current_at(end, UNIX_FIXTURE - 86_400)
        .expect("completed history fixture");
    old_record.input_method = "虎符".into();
    m.history.insert(0, old_record);
    m.selected_history = Some(if select_older { 0 } else { m.history.len() - 1 });
    m.ledger_open = true;
    m.ledger_scroll = if select_older {
        crate::history::record_offset(m, 0)
    } else {
        0.
    };
    sess
}

fn paint_field_fixtures(
    frame: &mut gale_win::d2d::Frame,
    m: &Model,
    t: &gale_core::theme::Theme,
    width: f32,
    height: f32,
) {
    let p = ui::tokens(t);
    for field in modals::fields(m, width, height) {
        if field.h <= 0. || field.w <= 0. {
            continue;
        }
        let value = if field.password {
            "••••••••".into()
        } else {
            m.field(field.id)
        };
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
        for (row, line) in lines.into_iter().enumerate() {
            ui::text(
                frame,
                field.x + 6.,
                field.y + 5. + row as f32 * 24.,
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
        .ok_or("Usage: design_review <output-directory>")?;
    std::fs::create_dir_all(&output)?;
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE);
    }
    let _com = ComSta::new()?;
    let window = HiddenWindow(MainWindow::create(
        "HuFu design fixture (hidden)",
        WIDTH as i32,
        HEIGHT as i32,
    )?);
    let scale = gale_win::window::dpi_scale();
    if (scale - 1.).abs() > 0.001 {
        return Err(
            format!("Fixture expected a 96-DPI render thread; actual scale={scale}").into(),
        );
    }
    let mut renderer = Renderer::new(window.0.hwnd, WIDTH as u32, HEIGHT as u32)?;
    let dict = dictionaries();
    let mut manifest = Vec::new();
    for screen in 1..=71 {
        let (mut sess, mut m) = practice_fixture();
        let mut cfg = Config {
            font_size: 54.,
            line_gap: 0.66,
            today_chars: 1284,
            smooth_caret: false,
            shake_on_wrong: false,
            motion_level: 2,
            ..Config::default()
        };
        let (width, height) = match screen {
            21..=25
            | 29
            | 34..=38
            | 40
            | 48
            | 49
            | 52
            | 55
            | 57
            | 58
            | 61
            | 63
            | 65..=67
            | 69
            | 70 => (640., 420.),
            26..=28 | 30 | 39 | 41..=47 | 51 | 54 | 56 | 59 | 60 | 71 => (800., 600.),
            _ => (WIDTH, HEIGHT),
        };
        let mut theme_name = "墨玉";
        let name = match screen {
            1 => "01-main-practice-dark.bmp",
            2 => {
                sess = drill_fixture();
                theme_name = "宣纸";
                "02-single-practice-light.bmp"
            }
            3 | 16 => {
                m.open(Modal::Library);
                if screen == 16 {
                    theme_name = "宣纸";
                    "16-library-light.bmp"
                } else {
                    "03-library-dark.bmp"
                }
            }
            4 => {
                m.open(Modal::Groups);
                "04-groups-dark.bmp"
            }
            5 => {
                m.open(Modal::Preferences);
                m.fields.insert("pref_tab".into(), "3".into());
                "05-preferences-appearance-dark.bmp"
            }
            6 | 7 | 19 | 29 => {
                let done = if screen == 7 { 4 } else { 2 };
                sess = completed_fixture(&mut m, TrainingMode::Race, done);
                if screen == 7 {
                    m.ledger_open = true;
                    m.selected_history = None;
                    m.history = sess.results.iter().flatten().cloned().collect();
                }
                match screen {
                    6 => "06-race-completed-dark.bmp",
                    7 => "07-ledger-session-dark.bmp",
                    19 => {
                        theme_name = "宣纸";
                        "19-race-completed-light.bmp"
                    }
                    _ => "29-race-completed-small-640x420.bmp",
                }
            }
            8 | 17 => {
                online_fixture(&mut m);
                if screen == 17 {
                    theme_name = "宣纸";
                    m.online_expired = false;
                }
                m.open(Modal::Online);
                if screen == 8 {
                    "08-online-expired-dark.bmp"
                } else {
                    "17-online-active-light.bmp"
                }
            }
            9 => {
                cleaning_fixture(&mut cfg, &mut m);
                m.open(Modal::Rules);
                "09-clean-rules-dark.bmp"
            }
            10..=12 | 18 | 24 | 27 => {
                m.open(Modal::Preferences);
                let tab = match screen {
                    10 => 0,
                    11 | 24 => 1,
                    _ => 2,
                };
                m.fields.insert("pref_tab".into(), tab.to_string());
                cfg.score_template = gale_core::scorecard::preset("detailed").template.into();
                if screen == 18 {
                    theme_name = "宣纸";
                }
                if screen == 24 {
                    m.panel_scroll = 300.;
                }
                match screen {
                    10 => "10-preferences-practice-dark.bmp",
                    11 => "11-preferences-scores-dark.bmp",
                    12 => "12-preferences-colors-dark.bmp",
                    18 => "18-preferences-colors-light.bmp",
                    24 => "24-preferences-scores-small-640x420-scrolled.bmp",
                    _ => "27-preferences-colors-small-800x600.bmp",
                }
            }
            13..=15 | 25 | 28 => {
                sess = history_fixture(&mut m, screen == 15, matches!(screen, 13 | 25 | 28));
                if screen == 15 {
                    m.selected_history = None;
                }
                match screen {
                    13 => "13-ledger-history-older-dark.bmp",
                    14 => "14-ledger-history-current-dark.bmp",
                    15 => "15-ledger-history-collapsed-dark.bmp",
                    25 => "25-ledger-history-small-640x420.bmp",
                    _ => "28-ledger-history-small-800x600.bmp",
                }
            }
            20 | 30 => {
                sess = completed_fixture(&mut m, TrainingMode::Practice, 2);
                m.open(Modal::Heatmap);
                theme_name = "宣纸";
                if screen == 20 {
                    "20-full-keyboard-light.bmp"
                } else {
                    "30-full-keyboard-light-small-800x600.bmp"
                }
            }
            21..=23 | 26 => {
                online_fixture(&mut m);
                m.online_authenticated = false;
                m.online_expired = false;
                m.online_login_expanded = true;
                m.online_register_mode = screen != 21;
                m.fields
                    .insert("reg_email".into(), "fixture@example.invalid".into());
                m.fields.insert("reg_code".into(), "123456".into());
                m.online_status = if screen == 21 {
                    "仅为离线登录布局样本，不发起网络请求。"
                } else {
                    "仅为离线注册布局样本，不发送验证码。"
                }
                .into();
                m.open(Modal::Online);
                if screen == 23 {
                    m.panel_scroll = 260.;
                }
                match screen {
                    21 => "21-online-login-small-640x420.bmp",
                    22 => "22-online-register-small-640x420-top.bmp",
                    23 => "23-online-register-small-640x420-scrolled.bmp",
                    _ => "26-online-register-small-800x600.bmp",
                }
            }
            31 => {
                m.open(Modal::Library);
                m.books.clear();
                "31-library-empty-dark.bmp"
            }
            32 | 33 | 37 => {
                full_analysis_fixture(&mut sess);
                m.analysis_dock = true;
                if screen == 33 {
                    m.fields.insert("analysis_view".into(), "heatmap".into());
                    "33-analysis-dock-heatmap-dark.bmp"
                } else if screen == 37 {
                    "37-analysis-dock-small-640x420.bmp"
                } else {
                    "32-analysis-dock-dark.bmp"
                }
            }
            34 => {
                m.open(Modal::Groups);
                "34-groups-small-640x420.bmp"
            }
            35 => {
                m.open(Modal::Preferences);
                m.fields.insert("pref_tab".into(), "3".into());
                "35-preferences-appearance-small-640x420.bmp"
            }
            36 => {
                cleaning_fixture(&mut cfg, &mut m);
                m.open(Modal::Rules);
                "36-rules-small-640x420.bmp"
            }
            38 | 39 => {
                m.open(Modal::Library);
                if screen == 39 {
                    "39-library-small-800x600.bmp"
                } else {
                    "38-library-small-640x420.bmp"
                }
            }
            40 => {
                sess = completed_fixture(&mut m, TrainingMode::Practice, 2);
                m.open(Modal::Heatmap);
                "40-heatmap-small-640x420.bmp"
            }
            41 => {
                sess = drill_fixture();
                theme_name = "宣纸";
                "41-single-practice-800x600-light.bmp"
            }
            42 => {
                theme_name = "宣纸";
                "42-main-practice-800x600-light.bmp"
            }
            43 | 44 => {
                m.open(Modal::Preferences);
                let tab = if screen == 43 { 1 } else { 4 };
                m.fields.insert("pref_tab".into(), tab.to_string());
                cfg.score_template = gale_core::scorecard::preset("detailed").template.into();
                if screen == 43 {
                    "43-preferences-scores-small-800x600.bmp"
                } else {
                    "44-preferences-data-small-800x600.bmp"
                }
            }
            45 => {
                m.open(Modal::Preferences);
                m.fields.insert("pref_tab".into(), "3".into());
                "45-preferences-appearance-small-800x600.bmp"
            }
            46 => {
                online_fixture(&mut m);
                m.online_authenticated = false;
                m.online_expired = false;
                m.online_login_expanded = true;
                m.online_status = "仅为离线登录布局样本，不发起网络请求。".into();
                m.open(Modal::Online);
                "46-online-login-small-800x600.bmp"
            }
            47 => {
                cleaning_fixture(&mut cfg, &mut m);
                m.open(Modal::Rules);
                "47-rules-small-800x600.bmp"
            }
            48 => "48-main-practice-small-640x420.bmp",
            49 => {
                sess = drill_fixture();
                "49-single-practice-small-640x420.bmp"
            }
            50..=52 | 68 | 69 => {
                let done = if screen >= 68 { 4 } else { 2 };
                sess = completed_fixture(&mut m, TrainingMode::Practice, done);
                if screen == 51 {
                    theme_name = "宣纸";
                }
                match screen {
                    50 => "50-main-completed-1280x800.bmp",
                    51 => "51-main-completed-800x600-light.bmp",
                    52 => "52-main-completed-640x420.bmp",
                    68 => "68-main-final-segment-completed-1280x800.bmp",
                    _ => "69-main-final-segment-completed-640x420.bmp",
                }
            }
            53..=55 | 66 => {
                if screen == 66 {
                    sess = drill_fixture();
                }
                m.open(Modal::More);
                match screen {
                    53 => "53-more-1280x800.bmp",
                    54 => "54-more-800x600.bmp",
                    55 => "55-more-640x420.bmp",
                    _ => "66-more-single-640x420.bmp",
                }
            }
            56 | 62 | 63 => {
                if screen == 62 {
                    m.groups.clear();
                    m.bound = None;
                } else if screen == 63 {
                    m.bound = Some(m.groups.len() + 3);
                }
                m.open(Modal::Groups);
                match screen {
                    56 => "56-groups-800x600.bmp",
                    62 => "62-groups-empty-1280x800.bmp",
                    _ => "63-groups-invalid-selection-640x420.bmp",
                }
            }
            57 | 58 | 67 => {
                m.open(Modal::Preferences);
                let tab = match screen {
                    57 => 0,
                    58 => 2,
                    _ => 1,
                };
                m.fields.insert("pref_tab".into(), tab.to_string());
                cfg.score_template = gale_core::scorecard::preset("detailed").template.into();
                match screen {
                    57 => "57-preferences-practice-small-640x420.bmp",
                    58 => "58-preferences-colors-small-640x420.bmp",
                    _ => "67-preferences-scores-small-640x420-top.bmp",
                }
            }
            59..=61 | 70 | 71 => {
                sess = history_fixture(&mut m, matches!(screen, 60 | 61), true);
                match screen {
                    59 => "59-ledger-history-800x600.bmp",
                    60 => "60-ledger-session-800x600.bmp",
                    61 => "61-ledger-session-640x420.bmp",
                    70 => "70-ledger-history-640x420-selected-detail.bmp",
                    _ => "71-ledger-history-800x600-selected-detail.bmp",
                }
            }
            64 => {
                sess = article(TrainingMode::Practice);
                m.latest = None;
                m.history.clear();
                "64-main-ready-1280x800.bmp"
            }
            65 => {
                sess = article(TrainingMode::Practice);
                m.latest = None;
                m.history.clear();
                m.ledger_open = true;
                "65-ledger-history-empty-640x420.bmp"
            }
            _ => unreachable!(),
        };
        if m.ledger_open {
            m.ledger_scroll = m.ledger_scroll.min(crate::history::scroll_max(&m, height));
        }
        cfg.theme = theme_name.into();
        let single = matches!(&sess.source, session::SessionSource::Single);
        let frame_now = if single { 1663 } else { NOW_MS };
        if single {
            m.status = "单字练习 · 常用单字前五百".into();
            m.hints = true;
        }
        let theme = ui::theme(theme_name);
        seed_preferences(&cfg, &mut m, &theme);
        m.words = if single {
            single_hints(&sess, &dict)
        } else {
            Vec::new()
        };
        m.max_code_length = 4;
        if single {
            if let Some(word) = m.words.get_mut(4) {
                word.rank = 2;
                word.tail = "_".into();
            }
        }
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
            frame_now,
            frame_now as f64 / 1000.,
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
            frame_now as f64 / 1000.,
        );
        paint_field_fixtures(&mut frame, &m, &theme, width, height);
        renderer.draw_frame(&frame)?;
        renderer
            .save_bmp(&output.join(name))
            .map_err(std::io::Error::other)?;
        let field_manifest: Vec<_> = modals::fields(&m, width, height).into_iter().map(|field|
            serde_json::json!({"id":field.id,"rect":[field.x,field.y,field.w,field.h],"multiline":field.multi,"password":field.password})
        ).collect();
        manifest.push(serde_json::json!({
            "screen":screen,"file":name,"logical_size":[width,height],"pixel_size":[width as u32,height as u32],
            "theme":theme_name,"modal":format!("{:?}",m.modal),"phase":format!("{:?}",sess.phase),
            "font_size":cfg.font_size,"single_character_hints":single,"report_tab":m.field("report_tab"),"ledger_open":m.ledger_open,"analysis_dock":m.analysis_dock,"ledger_scroll":m.ledger_scroll,"analysis_scroll":m.analysis_dock_scroll,
            "panel_scroll":m.panel_scroll,"list_scroll":m.scroll,"selected_history":m.selected_history,"history_records":m.history.len(),
            "draw_commands":frame.cmds.len(),"hit_targets":hits.len(),
            "visible_native_field_rectangles":field_manifest,"native_field_values_painted_by_d2d":true
        }));
        println!("S{screen}: {}", output.join(name).display());
    }
    std::fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "kind":"development-layout-fixture","deterministic":true,"dpi":96,"screen_count":manifest.len(),
            "notice":"Actual product Frame/Direct2D layout fixtures. RichEdit field values are D2D stand-ins; these images do not verify native keyboard, TSF composition, QQ, network, account, or persistence behavior.",
            "scores":"Generated from deterministic Session.on_key/feed_input/finalize_current_at calls; never saved to product data.",
            "screens":manifest,
        }))?,
    )?;
    std::fs::write(output.join("README.txt"), format!(
        "虎符跟打器 · 开发期布局 fixture\n\n{} 张 BMP 使用产品真实 ui::build、modals::draw、Frame 与 Direct2D Renderer，在单个隐藏窗口内生成。\n覆盖 1280×800、800×600 与 640×420，96 DPI；正文统一使用用户字号 54。\n核心场景覆盖文章、单字练习、完成与末段完成、更多菜单、选群、外观、设置三个页签，以及成绩记录/本轮汇总。\n另保留深浅主题、在线登录注册与滚动、清洗规则、完整键盘、理论码长分析与本地载文等辅助场景。\n成绩记录同时包含《岳阳楼记》《醉翁亭记》，区分所选旧记录与本轮最新成绩；小窗口额外覆盖滚动后的详情、群聊空态与失效选择。文章不创建编码提示，单字练习从真实单字码表生成提示。\nmanifest.json 逐图记录主题、尺寸、字号、模态、滚动、历史选择、绘制指令数量和原生字段矩形。\n账号、群、文章进度与成绩均为确定性样本；成绩通过真实 Session key / commit / finalize 路径生成，不读写产品成绩文件。\n原生输入字段按 FieldSpec 坐标用 D2D 绘制样本值；未创建 InputHost/RichEdit，不能替代真实 TSF、输入、焦点与滚动验证。\n没有抓取桌面、读取其他应用、访问网络或修改系统设置。\n", manifest.len()))?;
    Ok(())
}
