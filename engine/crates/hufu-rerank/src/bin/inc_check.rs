//! 增量解码一致性检查：同一 42 键 raw，全量单次解码 vs 逐键追加（增量链）
//! 的结果必须一致。定位 hufu-sentence 增量 resume 的回归。

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data_dir = PathBuf::from(
        args.get(1)
            .cloned()
            .unwrap_or_else(|| r"D:\HUFJ\HuFu虎符输入法-安装包\数据".to_string()),
    );
    let config = hufu_config::Config::load(&data_dir.join("config.json")).unwrap_or_default();
    let engine = hufu_engine::Engine::new(&data_dir, config).expect("引擎构建");
    let ngram = data_dir.join(&engine.config.sentence.ngram_path);
    let dec = hufu_sentence::SentenceEngine::load(
        &ngram,
        engine.schema.dict.clone(),
        &engine.schema.supplement,
        engine.config.sentence.weights.clone(),
    )
    .expect("ngram 加载");

    let raw = "geaenwlcghxirlwddsyftuuuwwjjgffddgeeaennww";

    // 全量基线（新引擎，单次）
    let full = dec.decode_to_strings(raw);
    println!("全量({}键): {} 个候选", raw.chars().count(), full.len());
    for t in full.iter().take(3) {
        println!("  full: {t}");
    }

    // 增量链：新引擎，逐键 decode_to_strings（走 decode_cached 增量路径）
    let dec2 = hufu_sentence::SentenceEngine::load(
        &ngram,
        engine.schema.dict.clone(),
        &engine.schema.supplement,
        engine.config.sentence.weights.clone(),
    )
    .expect("ngram 加载2");
    let mut inc_last: Vec<String> = Vec::new();
    let mut timings: Vec<(usize, u128)> = Vec::new();
    for l in 1..=raw.chars().count() {
        let prefix: String = raw.chars().take(l).collect();
        let t0 = std::time::Instant::now();
        inc_last = dec2.decode_to_strings(&prefix);
        timings.push((l, t0.elapsed().as_millis()));
    }
    println!("增量逐键耗时: {:?}", timings);
    println!("增量({}键): {} 个候选", raw.chars().count(), inc_last.len());
    for t in inc_last.iter().take(3) {
        println!("  inc : {t}");
    }

    if full == inc_last {
        println!("一致 ✓");
    } else {
        println!("不一致 ✗（增量 bug）");
        // 二分定位首个分叉长度
        let dec3 = hufu_sentence::SentenceEngine::load(
            &ngram,
            engine.schema.dict.clone(),
            &engine.schema.supplement,
            engine.config.sentence.weights.clone(),
        )
        .expect("ngram 加载3");
        let mut prev_ok = 0usize;
        for l in 1..=raw.chars().count() {
            let prefix: String = raw.chars().take(l).collect();
            let step = dec3.decode_to_strings(&prefix);
            // 基线引擎必须打断缓存（先解无关码）才能拿到真全量
            let _ = dec.decode_to_strings("qq");
            let base_full = dec.decode_to_strings(&prefix);
            if step == base_full {
                prev_ok = l;
            } else {
                println!("首个分叉: {l} 键 [{prefix}]（此前 {prev_ok} 键一致）");
                println!("  增量: {}", step.first().map(|s| s.as_str()).unwrap_or("(空)"));
                println!("  全量: {}", base_full.first().map(|s| s.as_str()).unwrap_or("(空)"));
                break;
            }
        }
    }
}
