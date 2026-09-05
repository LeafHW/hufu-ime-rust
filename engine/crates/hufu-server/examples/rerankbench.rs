//! 【rerank AB 2026-09-07 v2】有无 Qwen 神经重排对首选准率的影响。
//! 贴近真机停顿场景的口径：每句拆两半——前半视为已上屏（rerank 上文
//! ctx），后半打码 decode 出候选：
//!   A = 引擎原序 top1
//!   B = Reranker.score(前半句上文, top5) 重排后 top1
//! 与后半原句比对 exact。
//! 用法: rerankbench <方案目录> <ngram路径> <gguf路径> <语料> [N]

use hufu_dict::Schema;
use hufu_engine::SentenceDecoder;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = &args[1];
    let ngram = &args[2];
    let gguf = &args[3];
    let corpus = &args[4];
    let limit: usize = args
        .get(5)
        .map(|s| s.parse().unwrap_or(usize::MAX))
        .unwrap_or(usize::MAX);

    let schema = Schema::load(Path::new(dir)).expect("方案加载失败");
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
        digit_codes: false,
    };
    let dict = schema.dict.clone();
    let eng = hufu_sentence::SentenceEngine::load(
        Path::new(ngram),
        dict,
        &schema.supplement,
        weights,
    )
    .expect("ngram 加载失败");
    let rr = hufu_rerank::Reranker::load(gguf).expect("重排模型加载失败");

    let text = std::fs::read_to_string(corpus).expect("语料读取失败");
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.trim().len() >= 8) // 至少 8 字才能拆两半
        .take(limit)
        .collect();
    eprintln!("句子数: {}", lines.len());

    let mut a_exact = 0usize;
    let mut b_exact = 0usize;
    let mut both_wrong = 0usize;
    let mut rerank_wins = 0usize;
    let mut rerank_loss = 0usize;
    let mut rerank_ms = 0f64;

    for (idx, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let mid = chars.len() / 2;
        let ctx: String = chars[..mid].iter().collect();
        let target: String = chars[mid..].iter().collect();
        let mut raw = String::new();
        for ch in target.chars() {
            if let Some(c) = best_code(&schema, ch) {
                raw.push_str(&c);
            }
        }
        if raw.is_empty() {
            continue;
        }
        let dec = eng.decode_rich(&raw);
        let texts: Vec<String> = dec.hits.iter().map(|h| h.text.clone()).collect();
        if texts.is_empty() {
            continue;
        }
        let a_top = texts[0].clone();
        let top_n = texts.len().min(5);
        let t0 = std::time::Instant::now();
        let scores = rr.score(&ctx, &texts[..top_n]);
        rerank_ms += t0.elapsed().as_secs_f64() * 1000.0;
        let mut order: Vec<(f64, usize)> = scores.iter().copied().zip(0..top_n).collect();
        order.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        let b_top = texts[order[0].1].clone();

        let a_ok = a_top == target;
        let b_ok = b_top == target;
        a_exact += a_ok as usize;
        b_exact += b_ok as usize;
        if !a_ok && b_ok {
            rerank_wins += 1;
        }
        if a_ok && !b_ok {
            rerank_loss += 1;
        }
        if !a_ok && !b_ok {
            both_wrong += 1;
        }
        if (idx + 1) % 500 == 0 {
            let n = idx + 1;
            eprintln!(
                "进度 {n}/{}  A={:.2}%  B={:.2}%  胜{}负{}  重排均值 {:.1}ms/句",
                lines.len(),
                a_exact as f64 / n as f64 * 100.0,
                b_exact as f64 / n as f64 * 100.0,
                rerank_wins,
                rerank_loss,
                rerank_ms / n as f64
            );
        }
    }
    let n = lines.len();
    println!("═══ rerank AB v2（前半=上文，后半=decode 段）· {n} 句 ═══");
    println!(
        "A 无重排 exact   {}/{} = {:.2}%",
        a_exact,
        n,
        a_exact as f64 / n as f64 * 100.0
    );
    println!(
        "B 有重排 exact   {}/{} = {:.2}%",
        b_exact,
        n,
        b_exact as f64 / n as f64 * 100.0
    );
    println!("重排救回（A错B对）: {rerank_wins}   重排弄丢（A对B错）: {rerank_loss}");
    println!(
        "双错: {both_wrong}   重排耗时均值: {:.1}ms/句（纯 Rust；真机 llama.cpp 约 81ms/2候选）",
        rerank_ms / n as f64
    );
}

/// 每字最优码（≥2 码，无锁口径）。
fn best_code(schema: &Schema, ch: char) -> Option<String> {
    let mut best: Option<(f64, String)> = None;
    for e in &schema.dict.entries {
        if e.text.chars().count() == 1 && e.text.chars().next() == Some(ch) {
            let cl = e.code.chars().count();
            if cl < 2 {
                continue;
            }
            let better = match &best {
                None => true,
                Some((w, c)) => {
                    e.weight > *w
                        || (e.weight == *w && e.code.chars().count() < c.chars().count())
                }
            };
            if better {
                best = Some((e.weight, e.code.clone()));
            }
        }
    }
    best.map(|(_, c)| c)
}
