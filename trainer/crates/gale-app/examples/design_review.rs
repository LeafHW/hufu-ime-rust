//! Development-only native D2D layout fixtures for the nine Quiet Room screens.
//!
//! Run from trainer/:
//! cargo run -p gale-app --example design_review --target x86_64-pc-windows-gnu -- <output-dir>
//!
//! This program creates one hidden render surface. It does not capture the desktop,
//! create an InputHost, connect to QQ/online services, or read/write user settings.
//! RichEdit values are painted with D2D at the real FieldSpec rectangles, so these
//! images verify layout and rendering, not TSF composition or native edit behavior.
#![allow(dead_code)]

#[path = "../src/modals.rs"]
mod modals;
#[path = "../src/secondary_panels.rs"]
mod secondary_panels;
#[path = "../src/session.rs"]
mod session;
#[path = "../src/ui.rs"]
mod ui;

// —— Shim（2026-11-08）——aux_marker / FullAnalysis 定义在 bin 根 main.rs，
// example 编译单元的 crate 根是本文件，看不到它们；此处镜像一份（字段与
// main.rs 严格一致；重构收口时若挪进共享模块可整体删除本段）。
const AUX_PREFIX: char = '\u{e000}';
pub fn aux_marker(cmd: &str) -> modals::Action {
    modals::Action::Theme(format!("{AUX_PREFIX}{cmd}"))
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct FullAnalysis {
    #[serde(default)]
    pub entries: Vec<FullAnalysisEntry>,
    #[serde(default)]
    pub total_code_len: u64,
    #[serde(default)]
    pub avg_code_len_per_char: f64,
    #[serde(default)]
    pub select_count: u64,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct FullAnalysisEntry {
    #[serde(default)]
    pub word: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub code_len: u64,
    #[serde(default)]
    pub rank: u64,
}

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
const WORD_DICT: &str = include_str!("../../../../assets/码表/虎码字词/tigress_ci.dict.yaml");
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
/// still gets hints from the real server; this tool does not bypass that contract.
fn dictionaries() -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for (dict, code_column) in [(CHAR_DICT, 1), (WORD_DICT, 2)] {
        for row in dict.lines() {
            let cols: Vec<&str> = row.split('\t').collect();
            if cols.len() <= code_column || cols[0].is_empty() {
                continue;
            }
            let code = cols[code_column];
            if code.is_empty() || !code.bytes().all(|b| b.is_ascii_lowercase()) {
                continue;
            }
            let word = cols[0].to_string();
            if out.get(&word).is_none_or(|old| code.len() < old.len()) {
                out.insert(word, code.to_string());
            }
        }
    }
    out
}

fn article(mode: TrainingMode) -> Session {
    let mut sess = Session::new(100);
    sess.set_rules(SessionRules {
        mode,
        word_hints: mode == TrainingMode::Practice,
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
            sess.on_key(0x41 + key, tick + key as u64 * 15);
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
            sess.on_key(0x08, commit_at + 1);
            sess.feed_input(InputEvent::Backspace, commit_at + 1);
            sess.set_composing(true);
            sess.on_key(0x41, commit_at + 2);
            sess.on_key(0x42, commit_at + 3);
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
    m.latest = Some(first);
    (sess, m)
}

fn model() -> Model {
    let mut m = Model::default();
    m.hints = true;
    m.reduced_motion = false;
    m.quote_relaxed = true;
    m.status = "虎码训练营 · 群赛第 38 期 · 布局 fixture".into();
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
    let mut draft = String::new();
    for (i, paragraph) in PARAGRAPHS.iter().enumerate() {
        draft.push_str("岳阳楼记 · 选段\n");
        draft.push_str(paragraph);
        draft.push_str(&format!("\n-----第{}段-晴发文\n", i + 1));
    }
    m.fields.insert("load_text".into(), draft);
    m
}

fn hints(sess: &Session, dict: &BTreeMap<String, String>, single: bool) -> Vec<HintWord> {
    let chars: Vec<char> = sess.current_segment().text.chars().collect();
    let mut out = Vec::new();
    if !single && sess.current_segment().text == PARAGRAPHS[1] {
        // Fixed phrase boundaries from S1. Codes still come from the real source
        // dictionaries; a greedy fixture tokenizer would incorrectly prefer 之大.
        let chunks = [
            "予观夫",
            "巴陵",
            "胜状",
            "，",
            "在",
            "洞庭",
            "一湖",
            "。",
            "衔",
            "远山",
            "，",
            "吞",
            "长江",
            "，",
            "浩浩",
            "汤汤",
            "，",
            "横",
            "无际",
            "涯",
            "；",
            "朝晖",
            "夕阴",
            "，",
            "气象",
            "万千",
            "。",
            "此则",
            "岳阳楼",
            "之",
            "大观",
            "也",
            "，",
            "前人",
            "之述",
            "备矣",
            "。",
            "然则",
            "北通",
            "巫峡",
            "，",
            "南极",
            "潇湘",
            "，",
            "迁客",
            "骚人",
            "，",
            "多会",
            "于此",
            "，",
            "览物",
            "之情",
            "，",
            "得无",
            "异乎",
            "？",
        ];
        assert_eq!(chunks.concat(), PARAGRAPHS[1]);
        let mut start = 0;
        for word in chunks {
            let len = word.chars().count();
            if !word.chars().all(|c| "，。；：？！、".contains(c)) {
                out.push(HintWord {
                    text: word.into(),
                    code: dict.get(word).cloned(),
                    tail: String::new(),
                    start,
                    len,
                    rank: 1,
                });
            }
            start += len;
        }
        return out;
    }
    let mut i = 0;
    while i < chars.len() {
        let mut len = 1;
        if !single {
            for size in (2..=4.min(chars.len() - i)).rev() {
                let word: String = chars[i..i + size].iter().collect();
                if dict.contains_key(&word) {
                    len = size;
                    break;
                }
            }
        }
        let word: String = chars[i..i + len].iter().collect();
        let code = dict.get(&word).cloned();
        // Tail markers are layout samples only; no input-method recipe is inferred.
        let tail = if single && code.as_ref().is_some_and(|c| c.len() == 4) {
            "_"
        } else {
            ""
        };
        out.push(HintWord {
            text: word,
            code,
            tail: tail.into(),
            start: i,
            len,
            rank: 1,
        });
        i += len;
    }
    out
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
                r.safe,
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

fn paint_field_fixtures(frame: &mut gale_win::d2d::Frame, m: &Model, t: &gale_core::theme::Theme) {
    let p = ui::tokens(t);
    for field in modals::fields(m, WIDTH, HEIGHT) {
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
            modals::wrap(&value, ((field.w - 12.) / 12.).max(1.) as usize)
        } else {
            vec![value]
        };
        for (row, line) in lines.into_iter().enumerate() {
            ui::text(
                frame,
                field.x + 6.,
                field.y + 3. + row as f32 * 21.,
                12.,
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
    // This affects this example's thread only. No user/system display setting changes.
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
    for screen in 1..=9 {
        let (mut sess, mut m) = practice_fixture();
        let mut cfg = Config {
            font_size: 36.,
            line_gap: 0.66,
            today_chars: 1284,
            smooth_caret: false,
            shake_on_wrong: false,
            motion_level: 2,
            ..Config::default()
        };
        let mut theme_name = "墨玉";
        let name = match screen {
            1 => "01-main-practice-fixture.bmp",
            2 => {
                sess = drill_fixture();
                theme_name = "宣纸";
                cfg.hint_style = 1;
                "02-single-practice-fixture.bmp"
            }
            3 => {
                m.open(Modal::Load);
                "03-load-panel-fixture.bmp"
            }
            4 => {
                m.open(Modal::Groups);
                "04-groups-fixture.bmp"
            }
            5 => {
                m.open(Modal::Appearance);
                "05-appearance-fixture.bmp"
            }
            6 | 7 => {
                sess = article(TrainingMode::Race);
                let done = if screen == 6 { 2 } else { 4 };
                for index in 0..done {
                    m.latest = Some(finish_segment(&mut sess, index));
                }
                m.race = true;
                m.hints = false;
                m.open(if screen == 6 {
                    Modal::Result
                } else {
                    Modal::Report
                });
                if screen == 6 {
                    "06-segment-result-fixture.bmp"
                } else {
                    "07-race-report-fixture.bmp"
                }
            }
            8 => {
                online_fixture(&mut m);
                m.open(Modal::Online);
                "08-online-fixture.bmp"
            }
            _ => {
                cleaning_fixture(&mut cfg, &mut m);
                m.open(Modal::Rules);
                "09-clean-rules-fixture.bmp"
            }
        };
        cfg.theme = theme_name.into();
        let frame_now = if screen == 2 { 1663 } else { NOW_MS };
        if screen == 2 {
            m.status = "单字练习 · 常用单字前五百 · 布局 fixture".into();
        }
        let theme = ui::theme(theme_name);
        m.words = hints(&sess, &dict, screen == 2);
        if screen == 2 {
            // Explicit visual state to exercise the rank and tail decorations;
            // it is not an assertion about the live candidate order.
            if let Some(word) = m.words.get_mut(4) {
                word.rank = 2;
                word.tail = "_".into();
            }
        }
        let (body_w, font_size, _) = ui::body_metrics(&sess, &cfg, WIDTH);
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
            WIDTH,
            HEIGHT,
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
            WIDTH,
            HEIGHT,
            frame_now as f64 / 1000.,
        );
        paint_field_fixtures(&mut frame, &m, &theme);
        renderer.draw_frame(&frame)?;
        renderer
            .save_bmp(&output.join(name))
            .map_err(std::io::Error::other)?;
        manifest.push(serde_json::json!({ "screen": screen, "file": name, "logical_size": [1280, 800], "pixel_size": [1280, 800], "theme": theme_name, "draw_commands": frame.cmds.len(), "hit_targets": hits.len(), "native_field_values_painted_by_d2d": true }));
        println!("S{screen}: {}", output.join(name).display());
    }
    std::fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "kind": "development-layout-fixture", "deterministic": true,
            "notice": "Native D2D layout fixtures only. RichEdit field values are D2D stand-ins. These images do not verify native keyboard, TSF composition, QQ, network, account, or persistence behavior.",
            "scores": "Generated from deterministic Session.on_key/feed_input/finalize_current_at calls; never saved to product data.",
            "screens": manifest,
        }))?,
    )?;
    std::fs::write(output.join("README.txt"),
        "虎符跟打器 · 开发期布局 fixture\n\n9 张 BMP 均由产品真实 Frame / Direct2D Renderer 在隐藏窗口中生成，1280×800、96 DPI。\n所有账号、群、文章进度和成绩均为确定性示例，仅供原稿逐屏视觉对照。\n成绩通过真实 Session key / commit / finalize 路径生成，不读写产品成绩文件。\n原生输入字段仅按 FieldSpec 坐标用 D2D 绘制示例值；没有创建 InputHost/RichEdit，不能替代实机 TSF、输入、焦点、滚动测试。\n没有抓取桌面、读取其他应用、访问网络或修改系统设置。\n")?;
    Ok(())
}
