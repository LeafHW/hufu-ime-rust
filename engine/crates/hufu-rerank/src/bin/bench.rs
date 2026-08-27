//! 重排基准：加载模型，多组前文×候选打分，验证排序正确性与耗时。
//! 用法：rerank-bench [模型路径]

use hufu_rerank::Reranker;
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        "E:\\DSH-KF\\TigerClaw\\sentence\\Models\\sentence-qwen-q8.gguf".into()
    });
    println!("加载 {path} ...");
    let t0 = Instant::now();
    let r = match Reranker::load(&path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("加载失败: {e}");
            std::process::exit(1);
        }
    };
    println!("加载完成 {}ms", t0.elapsed().as_millis());

    let cases: &[(&str, &[&str])] = &[
        ("", &["两次", "𠓅", "𰧓", "兩次"]),
        ("他来过", &["两次", "𠓅"]),
        ("我去过", &["两次", "𰧓"]),
        ("这个方案需要讨论", &["两次", "𠓅"]),
        ("明天开会", &["两次", "俩次", "二回"]),
    ];
    for (ctx, cands) in cases {
        let t = Instant::now();
        let cs: Vec<String> = cands.iter().map(|s| s.to_string()).collect();
        let scores = r.score(ctx, &cs);
        let ms = t.elapsed().as_millis();
        let mut pairs: Vec<(f64, &str)> = scores.iter().copied().zip(cands.iter().copied()).collect();
        pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        let line: Vec<String> = pairs.iter().map(|(s, c)| format!("{c}={s:.3}")).collect();
        println!("ctx[{ctx}] {ms}ms → {} 最优:{}", line.join(" "), pairs[0].1);
    }
}
