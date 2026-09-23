//! 二十六修诊断：前4000置顶泄漏排查。离线复刻 server 装配（虎整句+配置
//! 权重），逐前缀 decode_rich，dump 全部 hits 的 score/conf/rank/分段。
//! 用法: boostdbg <数据目录> <ngram路径> [full_raw]
//! 不给 raw 时默认跑 letsuefh; 与 vhhbvhhb。

use hufu_engine::SentenceDecoder;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data_dir = std::path::PathBuf::from(&args[1]);
    let ngram = std::path::PathBuf::from(&args[2]);
    let full = args.get(3).cloned().unwrap_or_else(|| "letsuefh;".to_string());
    let schema_dir = data_dir.join("码表").join("虎整句");

    // 与部署 config.json 一致的权重（beam_width=200）
    let weights = hufu_config::SentenceWeights {
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
        ..Default::default()
    };
    let schema = hufu_dict::schema::Schema::load(&schema_dir).expect("方案加载失败");
    let dict = schema.dict.clone();
    let eng = hufu_sentence::SentenceEngine::load(&ngram, dict, &schema.supplement, weights)
        .expect("引擎装配失败");

    // 词典基准（供对照）
    for code in ["lets", "vhhb", "le", "ts", "uefh;", "vxobi", "kek", "vh", "hb", "fmkle"] {
        let rows: Vec<String> = schema
            .dict
            .lookup(code)
            .iter()
            .map(|e| e.text.clone())
            .collect();
        println!("dict[{code}] = {rows:?}");
    }

    // 逐前缀（复用同一引擎 → 命中增量缓存路径，与真实打字一致）
    let chars: Vec<char> = full.chars().collect();
    for end in 1..=chars.len() {
        let raw: String = chars[..end].iter().collect();
        let dec = eng.decode_rich(&raw);
        println!("==== decode({raw}) hits={} truncated={}", dec.hits.len(), dec.truncated);
        for (i, h) in dec.hits.iter().take(10).enumerate() {
            println!(
                "  {}. {:<14} score={:>9.3} conf={:.3} max_rank={} sum_rank={} exact={} seg=[{}]",
                i + 1,
                h.text,
                h.score,
                h.confidence,
                h.max_rank,
                h.sum_rank,
                h.exact,
                h.segmented
            );
        }
        if dec.hits.len() > 10 {
            println!("  … 共 {} 条", dec.hits.len());
        }
    }
}
