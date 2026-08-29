//! 复刻 server 装配：Schema::load + SentenceEngine::load，验证 eyiahx/窒闷
//! 是否进入解码格子。用法: decprobe <数据目录> <ngram路径>

use hufu_engine::SentenceDecoder;


fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data_dir = std::path::PathBuf::from(&args[1]);
    let ngram = std::path::PathBuf::from(&args[2]);
    let schema_dir = data_dir.join("码表").join("虎整句");

    let schema = hufu_dict::schema::Schema::load(&schema_dir).expect("方案加载失败");
    println!("码表条目数: {}", schema.dict.entries.len());
    let hits: Vec<String> = schema
        .dict
        .lookup("eyiahx")
        .iter()
        .map(|e| e.text.clone())
        .collect();
    println!("lookup(eyiahx) = {hits:?}");
    println!(
        "prefix_matches(eyiahx) = {:?}",
        schema
            .dict
            .prefix_matches("eyiahx")
            .iter()
            .map(|(l, v)| (l.to_string(), v.len()))
            .collect::<Vec<_>>()
    );
    let supp_hit = schema
        .supplement
        .entries
        .iter()
        .find(|e| e.word == "窒闷")
        .map(|e| e.weight)
        .unwrap_or(-1.0);
    println!("supplement 窒闷 权重 = {supp_hit}");

    // 与用户 config.json 相同的权重
    let weights = hufu_config::SentenceWeights {
        beam_width: 200,
        candidate_limit: 20,
        max_raw_length: 128,
        rank_penalty: 0.03,
        emitted_character_reward: 2.0,
        isolation_threshold: 3000,
        isolation_lambda: 2.0,
        confidence: 0.995,
        dict_bias: 1.0,
        supplement_baseline: 9.0,
        supplement_scale: 2.0,
        supplement_maximum: 16.0,
    };
    let dict = schema.dict.clone();
    let eng = hufu_sentence::SentenceEngine::load(&ngram, dict, &schema.supplement, weights)
        .expect("引擎装配失败");
    for raw in ["ueeyiahx"] {
        let dec = eng.decode_rich(raw);
        println!("== decode_rich({raw}) hits ==");
        for (i, h) in dec.hits.iter().take(6).enumerate() {
            println!("{}. {}  score={:.3} conf={:.3} max_rank={} seg=[{}]", i + 1, h.text, h.score, h.confidence, h.max_rank, h.segmented);
        }
        println!("-- early_hits --");
        for (i, h) in dec.early_hits.iter().take(4).enumerate() {
            println!("{}. {}  conf={:.3} score={:.3} max_rank={}", i + 1, h.text, h.confidence, h.score, h.max_rank);
        }
    }
}