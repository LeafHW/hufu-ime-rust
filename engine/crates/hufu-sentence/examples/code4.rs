//! 4码现切验证：nqbh → 真好
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
    for raw in ["nqbh"] {
        let dec = hufu_engine::SentenceDecoder::decode_rich(&engine, raw);
        println!("raw={raw}: hits={}", dec.hits.len());
        for h in dec.hits.iter().take(5) { println!("  {} score={:.2} seg={}", h.text, h.score, h.segmented); }
    }
}