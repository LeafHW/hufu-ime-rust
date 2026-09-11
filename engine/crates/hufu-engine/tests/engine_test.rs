//! 引擎状态机集成测试。

use hufu_config::Config;
use hufu_engine::{Engine, SentenceDecoder, SentenceHit, Session};
use hufu_types::{Candidate, KeyInput};
use std::sync::Arc;

fn setup() -> (Engine, Session, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("hufu-engine-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // 与 Config::default 的 schema.dir（"码表"）保持一致——1c94901
    // 改 default 目录名后 fixture 未同步，Engine::new 找不到方案目录
    let dict_dir = dir.join("码表").join("虎码单字");
    std::fs::create_dir_all(&dict_dir).unwrap();
    std::fs::write(
        dict_dir.join("tiger.dict.yaml"),
        "---\nname: tiger\nsort: by_weight\n...\n\
         我\tt\t900\n\
         你\tt\t450\n\
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
            sum_rank: 1,
            exact: true,
            word_ends: Vec::new(),
            segmented: raw.to_string(),
            partial: false,
            implicit2: false,
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

    // a 的次选走 `;`：`a;` 是「那个」的编码，仍延续。新组段显式清缓冲
    // （旧死路顶功语义里 u;+a 靠死路推屏自然分字；新语义死路=清屏，
    // 残留 raw 会吞掉新首键）
    session.clear();
    engine.process_key(&mut session, key('a'));
    let out = engine.process_key(&mut session, key(';'));
    assert_eq!(out.commit, None);
    let cands = out.state.unwrap().candidates;
    let dbg: Vec<&str> = cands.iter().map(|c| c.text.as_str()).collect();
    assert!(
        cands.iter().any(|c| c.text == "那个"),
        "a; 应组出「那个」，实际候选: {dbg:?}"
    );
}

#[test]
fn dinggong_push() {
    // 顶功（语义定版 2026-08-31）：死路键【不】顶屏——只有超过最大
    // 码长（第 max+1 键）才顶首选。一简 a(来) 后跟死路 z：不上屏。
    // 【空码码长=max+1 2026-09-11】2-max-1 键的暂时空码不清（可能仍
    // 有解/用户词）；恰满 max 码死路也保留缓冲；第 max+1 键仍空码才
    // 清前 max 码、保留第 max+1 键为新起点。
    let (mut engine, mut session, _dir) = setup();
    engine.process_key(&mut session, key('a')); // 来
    let out = engine.process_key(&mut session, key('z')); // az 死路（2 键）
    assert_eq!(out.commit, None, "死路键不得自动上屏");
    let st = out.state.unwrap();
    assert_eq!(st.raw, "az", "不满码空码保留缓冲");
    // 满 4 码仍空码 → 不清（等第 max+1 键定夺）
    engine.process_key(&mut session, key('z'));
    let out4 = engine.process_key(&mut session, key('z')); // azzz 满码死路
    assert_eq!(out4.commit, None);
    assert_eq!(out4.state.unwrap().raw, "azzz", "满码空码不清（第 max+1 键才清）");
    // 第 5 键（max+1）仍空码 → 清前 4 码、保留第 5 键自身
    let out5 = engine.process_key(&mut session, key('q')); // azzzq 死路
    assert_eq!(out5.commit, None);
    assert_eq!(out5.state.unwrap().raw, "q", "第 max+1 键清前 max 码、保留自身");
}

#[test]
fn tab_navigate_mode() {
    // 【Tab 双模式 2026-09-08】tab_clear=false → Tab=选重导航：
    // 按一下高亮移到下一候选（同方向键↓），空格上屏选中项。
    let (mut engine, mut session, _dir) = setup();
    engine.config.input.tab_clear = false;
    engine.process_key(&mut session, key('t')); // t 前缀态：我(t)、我们(tuja) 多候选
    let st0 = engine.state(&session);
    assert!(st0.candidates.len() >= 2, "需多候选场景（t 前缀）");
    assert_eq!(st0.selected, 0, "初始高亮首选");
    let out = engine.process_key(&mut session, KeyInput { key: hufu_types::KeyCode::Tab, modifiers: hufu_types::Modifiers::default(), is_press: true });
    let st1 = out.state.unwrap();
    assert_eq!(st1.selected, 1, "Tab 高亮下一候选");
    // 空格上屏当前高亮（不再清屏）
    let out2 = engine.process_key(&mut session, key(' '));
    assert!(out2.commit.is_some(), "空格上屏选中候选");
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
    // 快符与 ; 引导共存：;a 仍是快符 ！（有符号延续），
    // 无延续的字符（如 ;j 若无映射）才打断回正常编码
    engine.process_key(&mut session, key(';'));
    let out = engine.process_key(&mut session, key('a'));
    assert_eq!(out.commit.as_deref(), Some("！"));
}

#[test]
fn slash_dunhao_and_symbols() {
    let (mut engine, mut session, _dir) = setup();
    // 【2026-09-06 双档·默认命名空间】默认不再直出：/ 进候选首位
    // =、（空格确认）；直出档（开直出）：空态 / = 、 直接上屏
    engine.process_key(&mut session, key('/'));
    let out = engine.process_key(&mut session, key(' '));
    assert_eq!(out.commit.as_deref(), Some("、"), "命名空间档默认：空格确认顿号");
    engine.config.input.slash_dunhao = true;
    let out = engine.process_key(&mut session, key('/'));
    assert_eq!(out.commit.as_deref(), Some("、"), "直出档空态 / 直出顿号");
    // 命名空间档（关直出）：/ 进候选首位=、，空格确认
    engine.config.input.slash_dunhao = false;
    engine.process_key(&mut session, key('/'));
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
    // ; 引导标点：候选 [：,；]；;+空格=：，;;=；直上
    engine.process_key(&mut session, key(';'));
    let st = engine.state(&session);
    assert_eq!(st.candidates[0].text, "：");
    assert_eq!(st.candidates[1].text, "；");
    let out = engine.process_key(&mut session, key(' '));
    assert_eq!(out.commit.as_deref(), Some("："));
    engine.process_key(&mut session, key(';'));
    let out = engine.process_key(&mut session, key(';'));
    assert_eq!(out.commit.as_deref(), Some("；"));
}

#[test]
fn reverse_lookup_mode() {
    let (mut engine, mut session, _dir) = setup();
    let dir = engine.data_dir.join("码表").join("虎码单字");
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

    // ≤4 码：码表候选合并进列表（整句短语可置前，Rime 菜单合并语义）
    engine.process_key(&mut session, key('t'));
    assert!(engine.state(&session).candidates.iter().any(|c| c.text == "我"));
    // 4 码满码同上
    for c in ['u', 'j', 'a'] {
        engine.process_key(&mut session, key(c)); // tuja = 我们
    }
    assert!(engine.state(&session).candidates.iter().any(|c| c.text == "我们"));
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

/// 【4 码内高频字优先 2026-09-11】当前实打编码 ≤4（含 4）时前 1500
/// 高频单字候选压多字候选置首（同码竞争中权重低也置先）；第 5 键起
///（超 4 码）回归正常权重排序。置顶（pinned）仍压过优先规则。
#[test]
fn freq1500_priority_within_four_codes() {
    let dir = std::env::temp_dir().join(format!("hufu-freq1500-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let dict_dir = dir.join("码表").join("虎码单字");
    std::fs::create_dir_all(&dict_dir).unwrap();
    std::fs::write(
        dict_dir.join("tiger.dict.yaml"),
        "---\nname: tiger\nsort: by_weight\n...\n\
         写组\twx\t9000\n\
         的\twx\t10\n\
         一\twx\t5\n\
         词组长码\twxyzx\t9000\n\
         是\twxyzx\t10\n",
    )
    .unwrap();
    let mut config = Config::default();
    config.schema.current = "虎码单字".into();
    let mut engine = Engine::new(&dir, config).unwrap();
    let mut session = Session::new(true);
    // wx（2 码 ≤4）：的/一（前 1500 字频）压过高权重「写组」，且字频
    // 序 的(排1) 在 一(排2) 前
    engine.process_key(&mut session, key('w'));
    let out = engine.process_key(&mut session, key('x'));
    let cands = out.state.unwrap().candidates;
    let texts: Vec<&str> = cands.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, vec!["的", "一", "写组"], "≤4 码高频字优先首选: {texts:?}");
    // wxyzx（5 码 >4）：正常权重——高权重「词组长码」在先
    engine.process_key(&mut session, key('y'));
    engine.process_key(&mut session, key('z'));
    let out5 = engine.process_key(&mut session, key('x'));
    let cands5 = out5.state.unwrap().candidates;
    let texts5: Vec<&str> = cands5.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts5, vec!["词组长码", "是"], ">4 码正常权重: {texts5:?}");
}

/// 【单字永不换序·词下沉 2026-09-11 二次修正】用户实测回归：kc 全码组
/// [寸,泥,⼨]（泥在 1500 表、寸不在）被平铺浮前错排成泥首位——形码同码
/// 单字组的官方序不容频表重排。正确语义：1500 单字只浮到「多字候选」
/// 之前（wfsi→征压「写止」词），锚后罕字不回退、已达标组零扰动。
#[test]
fn freq1500_singles_never_reorder_words_sink() {
    let dir = std::env::temp_dir().join(format!("hufu-freq1500-fix-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let dict_dir = dir.join("码表").join("虎码单字");
    std::fs::create_dir_all(&dict_dir).unwrap();
    std::fs::write(
        dict_dir.join("tiger.dict.yaml"),
        "---\nname: tiger\nsort: by_weight\n...\n\
         寸\tkc\t9000\n\
         泥\tkc\t10\n\
         ⼨\tkc\t5\n\
         写止\twfsi\t9000\n\
         征\twfsi\t10\n\
         𡧡\twfsi\t8\n\
         象\twx\t9000\n\
         彻底\twx\t8000\n\
         𧰼\twx\t7000\n",
    )
    .unwrap();
    let mut config = Config::default();
    config.schema.current = "虎码单字".into();
    let mut engine = Engine::new(&dir, config).unwrap();
    let type_raw = |engine: &mut Engine, raw: &str| {
        let mut session = Session::new(true);
        let mut out = None;
        for ch in raw.chars() {
            out = Some(engine.process_key(&mut session, key(ch)));
        }
        out.unwrap().state.unwrap().candidates
    };
    // kc：同码单字组官方序不动（泥在 1500 表也不浮过寸）
    let cands = type_raw(&mut engine, "kc");
    let texts: Vec<&str> = cands.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, vec!["寸", "泥", "⼨"], "单字组永不换序: {texts:?}");
    // wfsi：词「写止」下沉，征浮首，锚后罕字保位
    let cands = type_raw(&mut engine, "wfsi");
    let texts: Vec<&str> = cands.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, vec!["征", "写止", "𡧡"], "1500 单字压词、罕字不回退: {texts:?}");
    // wx：权重序 [象,彻底,𧰼]，象本就在首——整表不动
    let cands = type_raw(&mut engine, "wx");
    let texts: Vec<&str> = cands.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, vec!["象", "彻底", "𧰼"], "已达标组零扰动: {texts:?}");
}
