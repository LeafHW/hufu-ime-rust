//! 引擎状态机集成测试。

use hufu_config::Config;
use hufu_engine::{Engine, SentenceDecoder, SentenceHit, Session};
use hufu_types::{Candidate, KeyInput};
use std::sync::Arc;

fn setup() -> (Engine, Session, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("hufu-engine-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let dict_dir = dir.join("dictionaries").join("虎码单字");
    std::fs::create_dir_all(&dict_dir).unwrap();
    std::fs::write(
        dict_dir.join("tiger.dict.yaml"),
        "---\nname: tiger\nsort: by_weight\n...\n\
         我\tt\t900\n\
         来\ta\t800\n\
         的\tu\t700\n\
         他\tje\t600\n\
         我们\ttuja\t500\n\
         那个\ta;\t400\n\
         底\tu;\t300\n",
    )
    .unwrap();
    std::fs::write(dict_dir.join("快符.txt"), "！\t;a\n。\t;b\n“\t;f\n").unwrap();
    std::fs::write(
        dict_dir.join("常用符号.txt"),
        "™\t/tm\n℃\t/ssd\n",
    )
    .unwrap();
    let mut config = Config::default();
    config.schema.current = "虎码单字".into();
    let engine = Engine::new(&dir, config).unwrap();
    let session = Session::new(true);
    (engine, session, dir)
}

/// 模拟整句解码器。
struct MockDecoder;
impl SentenceDecoder for MockDecoder {
    fn decode_rich(&self, raw: &str) -> std::sync::Arc<hufu_engine::SentenceDecode> {
        let hits = vec![SentenceHit {
            text: format!("整句[{raw}]"),
            score: -1.0,
            confidence: -1.0,
            max_rank: 1,
            word_ends: Vec::new(),
            segmented: raw.to_string(),
        }];
        std::sync::Arc::new(hufu_engine::SentenceDecode {
            hits,
            truncated: false,
            early_hits: Vec::new(),
            early_truncated: false,
        })
    }
}

fn key(c: char) -> KeyInput {
    KeyInput::char_key(c)
}

#[test]
fn type_and_select_first() {
    let (mut engine, mut session, _dir) = setup();
    let out = engine.process_key(&mut session, key('t'));
    assert!(out.consumed);
    assert!(out.commit.is_none());
    let st = out.state.unwrap();
    assert_eq!(st.raw, "t");
    assert_eq!(st.candidates[0].text, "我");

    let out = engine.process_key(&mut session, key(' '));
    assert_eq!(out.commit.as_deref(), Some("我"));
    assert!(out.state.unwrap().is_idle());
}

#[test]
fn second_and_third_select() {
    let (mut engine, mut session, _dir) = setup();
    engine.process_key(&mut session, key('u')); // 的 / 底
    let out = engine.process_key(&mut session, key(';'));
    // `u;` 是「底」的编码 → 编码延续优先于次选
    assert_eq!(out.commit, None);
    let st = out.state.unwrap();
    assert_eq!(st.candidates[0].text, "底");

    // a 的次选走 `;`：`a;` 是「那个」的编码，仍延续
    engine.process_key(&mut session, key('a'));
    let out = engine.process_key(&mut session, key(';'));
    assert_eq!(out.commit, None);
    assert_eq!(out.state.unwrap().candidates[0].text, "那个");
}

#[test]
fn dinggong_push() {
    // 顶功：一简 a(来) 后跟死路字符 → 自动上屏「来」，新码从新字符开始
    let (mut engine, mut session, _dir) = setup();
    engine.process_key(&mut session, key('a')); // 来
    let out = engine.process_key(&mut session, key('z')); // az 无任何延续 → 顶功
    assert_eq!(out.commit.as_deref(), Some("来"));
    let st = out.state.unwrap();
    assert_eq!(st.raw, "z");
}

#[test]
fn full_code_push_on_fifth() {
    // 超过最大码长（4）→ 顶屏首选
    let (mut engine, mut session, _dir) = setup();
    for c in ['t', 'u', 'j', 'a'] {
        engine.process_key(&mut session, key(c)); // tuja = 我们（满码）
    }
    let st = engine.state(&session);
    assert_eq!(st.candidates[0].text, "我们");
    let out = engine.process_key(&mut session, key('t')); // 第 5 码
    assert_eq!(out.commit.as_deref(), Some("我们"));
    assert_eq!(out.state.unwrap().raw, "t");
}

#[test]
fn quick_symbol_auto_commit() {
    let (mut engine, mut session, _dir) = setup();
    engine.process_key(&mut session, key(';'));
    let out = engine.process_key(&mut session, key('a'));
    assert_eq!(out.commit.as_deref(), Some("！"));
}

#[test]
fn slash_dunhao_and_symbols() {
    let (mut engine, mut session, _dir) = setup();
    // / 空格 → 顿号
    let out = engine.process_key(&mut session, key('/'));
    assert_eq!(out.state.unwrap().candidates[0].text, "、");
    let out = engine.process_key(&mut session, key(' '));
    assert_eq!(out.commit.as_deref(), Some("、"));
    // /tm → ™
    engine.process_key(&mut session, key('/'));
    engine.process_key(&mut session, key('t'));
    let out = engine.process_key(&mut session, key('m'));
    assert_eq!(out.commit.as_deref(), Some("™"));
}

#[test]
fn punct_fullwidth_and_pair() {
    let (mut engine, mut session, _dir) = setup();
    let out = engine.process_key(&mut session, key(','));
    assert_eq!(out.commit.as_deref(), Some("，"));
    // ' 属于编码字母表：空态首选全角左单引号，空格上屏
    let out = engine.process_key(&mut session, key('\''));
    assert!(out.commit.is_none());
    assert_eq!(out.state.unwrap().candidates[0].text, "‘");
    let out = engine.process_key(&mut session, key(' '));
    assert_eq!(out.commit.as_deref(), Some("‘"));
    // ; 空态首选全角分号
    engine.process_key(&mut session, key(';'));
    let st = engine.state(&session);
    assert_eq!(st.candidates[0].text, "；");
}

#[test]
fn reverse_lookup_mode() {
    let (mut engine, mut session, _dir) = setup();
    let dir = engine.data_dir.join("dictionaries").join("虎码单字");
    std::fs::write(dir.join("Bime_小鹤双拼反查.txt"), "我\two\n的\tde\n").unwrap();
    engine.schema = hufu_dict::Schema::load(&dir).unwrap();
    engine.process_key(&mut session, key('`'));
    let out = engine.process_key(&mut session, key('d'));
    engine.process_key(&mut session, key('e'));
    let st = engine.state(&session);
    assert_eq!(st.raw, "de");
    assert!(st.reverse_mode);
    let out = engine.process_key(&mut session, key(' '));
    assert_eq!(out.commit.as_deref(), Some("的"));
}

#[test]
fn sentence_mode_after_max_length() {
    let (mut engine, mut session, _dir) = setup();
    engine.set_sentence_decoder(Some(Arc::new(MockDecoder)));
    // 方案名不含「整句」且 auto_enable=true → 未激活
    assert!(!engine.sentence_active());
    engine.config.sentence.auto_enable = false;
    assert!(engine.sentence_active());

    // ≤4 码：走码表
    engine.process_key(&mut session, key('t'));
    assert_eq!(engine.state(&session).candidates[0].text, "我");
    // 4 码满码仍是码表
    for c in ['u', 'j', 'a'] {
        engine.process_key(&mut session, key(c)); // tuja = 我们
    }
    assert_eq!(engine.state(&session).candidates[0].text, "我们");
    // 第 5 码：整句接管，不顶功
    engine.process_key(&mut session, key('x'));
    let st = engine.state(&session);
    assert_eq!(st.raw, "tujax");
    assert_eq!(st.candidates[0].text, "整句[tujax]");
}

#[test]
fn mixed_input_uppercase() {
    let (mut engine, mut session, _dir) = setup();
    engine.process_key(&mut session, key('A'));
    engine.process_key(&mut session, key('B'));
    let out = engine.process_key(&mut session, key(' '));
    assert_eq!(out.commit.as_deref(), Some("AB"));
}

#[test]
fn shift_toggle_and_english_passthrough() {
    let (mut engine, mut session, _dir) = setup();
    let out = engine.process_key(
        &mut session,
        KeyInput {
            key: hufu_types::KeyCode::ShiftLeft,
            ..KeyInput::char_key(' ')
        },
    );
    assert!(out.consumed);
    assert!(!out.state.unwrap().chinese);
    // 英文态字符直通
    let out = engine.process_key(&mut session, key('a'));
    assert!(!out.consumed);
}

#[test]
fn enter_clear_and_escape() {
    let (mut engine, mut session, _dir) = setup();
    engine.process_key(&mut session, key('t'));
    let out = engine.process_key(
        &mut session,
        KeyInput {
            key: hufu_types::KeyCode::Enter,
            ..KeyInput::char_key(' ')
        },
    );
    assert!(out.consumed);
    assert!(out.commit.is_none());
    assert!(out.state.unwrap().is_idle());
}

/// 独立测试目录由各用例自行创建；此处仅保证重复运行不冲突
#[allow(dead_code)]
fn setup_with_dir() -> (Engine, Session, std::path::PathBuf) {
    setup()
}
