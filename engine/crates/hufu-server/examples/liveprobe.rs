//! session 全链路复现：Engine+Session 逐键 ueeyiahx，对比
//! sentence_candidates（应为 hits 序）与 state 实际候选序。
//! 用法: liveprobe <数据目录>

use hufu_engine::{Engine, Session};
use hufu_types::{KeyCode, KeyInput, Modifiers};

fn key(c: char) -> KeyInput {
    KeyInput { key: KeyCode::Char(c), modifiers: Modifiers::default(), is_press: true }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data_dir = std::path::PathBuf::from(&args[1]);
    let cfg = hufu_config::Config::load(&data_dir.join("config.json")).unwrap_or_default();
    let mut engine = Engine::new(&data_dir, cfg).expect("引擎构建");
    let ngram = data_dir.join(&engine.config.sentence.ngram_path);
    let dec = hufu_sentence::SentenceEngine::load(
        &ngram,
        engine.schema.dict.clone(),
        &engine.schema.supplement,
        engine.config.sentence.weights.clone(),
    )
    .expect("ngram");
    engine.set_sentence_decoder(Some(std::sync::Arc::new(dec)));

    let mut session = Session::new(true);
    for ch in "ueeyiahx".chars() {
        let out = engine.process_key(&mut session, key(ch));
        let commit = out.commit.clone().unwrap_or_default();
        let cs: Vec<String> = out
            .state
            .as_ref()
            .map(|s| s.candidates.iter().take(4).map(|c| c.text.clone()).collect())
            .unwrap_or_default();
        let raw = out.state.as_ref().map(|s| s.raw.clone()).unwrap_or_default();
        println!("键={ch} raw=[{raw}] commit=[{commit}] 候选={cs:?}");
    }
    // 对照：sentence_candidates 直读（decode_rich hits 序）
    let full = format!("{}{}", session.committed_raw, session.raw);
    let d = engine.sentence_decoder().expect("dec").decode_rich(&full);
    println!("── decode_rich({full}) hits 序:");
    for (i, h) in d.hits.iter().take(4).enumerate() {
        println!("  {}. {} score={:.3} max_rank={}", i + 1, h.text, h.score, h.max_rank);
    }
    // ── 重排请求验证：句首空语境应跳过；有文章尾巴应带语境 ──
    session.tail_context.clear();
    println!(
        "句首空语境 rerank_request = {:?}",
        engine.rerank_request(&session).map(|r| r.1)
    );
    session.tail_context = "房间里空气不流通，".into();
    println!(
        "带尾巴语境 rerank_request = {:?}",
        engine.rerank_request(&session).map(|r| r.1)
    );
}
