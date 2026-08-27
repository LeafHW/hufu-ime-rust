//! 整句真实数据探针：
//! `cargo run -p hufu-sentence --release --example sentenceprobe -- <模型bin> <方案目录> <raw> [raw2...]`
use hufu_config::SentenceWeights;
use hufu_dict::schema::Schema;
use hufu_sentence::SentenceEngine;
use std::path::Path;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args.next().expect("模型 bin 路径");
    let schema_dir = args.next().expect("方案目录");
    let raws: Vec<String> = args.collect();
    let t0 = Instant::now();
    let schema = Schema::load(Path::new(&schema_dir)).expect("方案加载失败");
    println!("码表加载: {:?}（{} 条）", t0.elapsed(), schema.dict.len());
    let t1 = Instant::now();
    let engine = SentenceEngine::load(
        Path::new(&model),
        schema.dict.clone(),
        &schema.supplement,
        SentenceWeights::default(),
    )
    .expect("模型加载失败");
    println!("模型加载: {:?}（unigram {} 条）", t1.elapsed(), engine.model.uni_count());
    for raw in &raws {
        let t = Instant::now();
        let dec = hufu_engine::SentenceDecoder::decode_rich(&engine, raw);
        let out: Vec<&str> = dec.hits.iter().map(|h| h.text.as_str()).collect();
        println!(
            "\nraw='{}' → {:?}  （{:?}，候选 {}）",
            raw,
            &out[..out.len().min(8)],
            t.elapsed(),
            out.len()
        );
        let top: Vec<&hufu_engine::SentenceHit> = dec.hits.iter().take(8).collect();
        let (proposal, _share) = hufu_engine::confidence_proposal(&top, 0.995);
        if !proposal.is_empty() {
            println!("  置信前缀提案: '{proposal}'（完整候选源）");
        }
        if !dec.early_hits.is_empty() {
            let etop: Vec<&hufu_engine::SentenceHit> = dec.early_hits.iter().take(8).collect();
            let (eproposal, _eshare) = hufu_engine::confidence_proposal(&etop, 0.995);
            let early: Vec<&str> = dec.early_hits.iter().map(|h| h.text.as_str()).take(4).collect();
            println!("  不完全尾候选: {:?} 提案: '{eproposal}'", early);
        }
    }
}
