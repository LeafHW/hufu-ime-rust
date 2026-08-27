//! early 调试：打印 syftuu 的 early_hits
use hufu_config::SentenceWeights;
use hufu_dict::schema::Schema;
use hufu_sentence::SentenceEngine;
use std::path::Path;
fn main() {
    let schema = Schema::load(Path::new("E:/DSH-KF/hufu/hufu-data/dictionaries/虎整句")).unwrap();
    let engine = SentenceEngine::load(
        Path::new("E:/DSH-KF/hufu/hufu-data/models/sentence-ngram.bin"),
        schema.dict.clone(),
        &schema.supplement,
        SentenceWeights::default(),
    ).unwrap();
    for raw in ["syftu", "syftuu", "syftuuu"] {
        let dec = hufu_engine::SentenceDecoder::decode_rich(&engine, raw);
        println!("raw={raw}: hits={} early={}", dec.hits.len(), dec.early_hits.len());
        for h in dec.hits.iter().take(4) { println!("  hit  {} conf={:.3} maxr={}", h.text, h.confidence, h.max_rank); }
        for h in dec.early_hits.iter().take(6) { println!("  earl {} conf={:.3}", h.text, h.confidence); }
    }
}