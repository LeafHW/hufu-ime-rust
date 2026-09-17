//! 二十六修诊断·引擎级逐键实验：复刻 server 装配（含提前上屏/顶屏/锁），
//! 逐键喂完整编码串，dump 每键后的 raw/已上屏/候选首5。
//! 用法: keydbg <数据目录> <ngram路径> <编码串>
use hufu_engine::Session;
use hufu_types::{KeyCode, KeyInput, Modifiers};

fn key(c: char) -> KeyInput {
    KeyInput {
        key: KeyCode::Char(c),
        modifiers: Modifiers::default(),
        is_press: true,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data_dir = std::path::PathBuf::from(&args[1]);
    let ngram = std::path::PathBuf::from(&args[2]);
    let full = args[3].clone();
    let schema_dir = data_dir.join("码表").join("虎整句");

    let mut cfg = hufu_config::Config::default();
    // 对齐部署 config.json 关键项
    cfg.sentence.enabled = true;
    cfg.sentence.auto_enable = true;
    cfg.sentence.early_commit = true;
    cfg.sentence.early_need = 3;
    cfg.sentence.ngram_path = ngram.clone().into_os_string().into_string().unwrap();
    cfg.input.max_code_length = 4;
    cfg.input.auto_push = true;
    cfg.sentence.weights.beam_width = 200;
    cfg.sentence.weights.candidate_limit = 20;

    let mut eng = hufu_engine::Engine::with_schema_dir(&schema_dir, cfg)
        .expect("引擎装配失败");
    // with_schema_dir 指向数据根（码表/ 的父目录）
    let schema = hufu_dict::schema::Schema::load(&schema_dir).expect("方案加载失败");
    let dec = hufu_sentence::SentenceEngine::load(
        &ngram,
        schema.dict.clone(),
        &schema.supplement,
        hufu_config::SentenceWeights {
            beam_width: 200,
            candidate_limit: 20,
            max_raw_length: 128,
            rank_penalty: 0.03,
            emitted_character_reward: 2.0,
            isolation_threshold: 3000,
            isolation_lambda: 2.0,
            confidence: 0.99,
            dict_bias: 1.0,
            supplement_baseline: 9.0,
            supplement_scale: 2.0,
            supplement_maximum: 32.0,
            digit_codes: false,
        },
    )
    .expect("解码器装配失败");
    eng.set_sentence_decoder(Some(std::sync::Arc::new(dec)));

    let mut s = Session::new(true);
    let mut committed_all = String::new();
    for ch in full.chars() {
        let out = eng.process_key(&mut s, key(ch));
        if let Some(c) = out.commit.as_deref() {
            committed_all.push_str(c);
        }
        let st = eng.state(&s);
        let cands: Vec<String> = st
            .candidates
            .iter()
            .take(5)
            .map(|c| c.text.clone())
            .collect();
        println!(
            "{ch}  raw={:<12} c_raw={:<8} c_txt={:<8} out_commit={:<10} 总上屏={:<16} cands={}",
            s.raw,
            s.committed_raw,
            s.committed_text,
            out.commit.as_deref().unwrap_or("-"),
            committed_all,
            cands.join("|")
        );
    }
    // 末尾空格收尾
    let out = eng.process_key(&mut s, key(' '));
    if let Some(c) = out.commit.as_deref() {
        committed_all.push_str(c);
    }
    println!("空格后总上屏: {committed_all}");
}
