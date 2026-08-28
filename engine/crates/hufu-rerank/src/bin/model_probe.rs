//! 模型替换探针：对任意 GGUF 直接跑 Reranker::load + 一次真实打分，
//! 回答「换个模型文件能不能直接用」。用法：
//!   model_probe <gguf路径> [测试句]
//! 输出：加载耗时、模型结构、tokenizer、打分结果。

use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("用法: model_probe <gguf路径> [测试句]");
        std::process::exit(2);
    }
    let path = &args[1];
    let text = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "今天天气真不错".to_string());

    println!("── 模型探针 ──");
    println!("文件: {path}");
    let meta = std::fs::metadata(path);
    match &meta {
        Ok(m) => println!("大小: {:.1} MB", m.len() as f64 / 1048576.0),
        Err(e) => {
            println!("✗ 文件不可读: {e}");
            std::process::exit(1);
        }
    }

    let t0 = Instant::now();
    match hufu_rerank::Reranker::load(path) {
        Ok(r) => {
            println!("加载: ✓ {:.2}s", t0.elapsed().as_secs_f64());
            println!("结构: hidden={} layers={}", r.m.cfg.hidden, r.m.cfg.layers);
            let t1 = Instant::now();
            let (sum, mean, eot_lp, total) = r.score_debug("今天天气", &text);
            println!("打分: ✓ {:.1}s  ({text})", t1.elapsed().as_secs_f64());
            println!("  sum={sum:.3} mean={mean:.3} logP(eot)={eot_lp:.3} total={total:.3}");
            // 第二次更快（页缓存+已热）
            let t2 = Instant::now();
            let _ = r.score_debug("我们明天", &text);
            println!("二次打分: {:.1}s", t2.elapsed().as_secs_f64());
            println!("结论: ✓ 此模型可直接替换使用");
        }
        Err(e) => {
            println!("加载: ✗ {} ({:.2}s)", e, t0.elapsed().as_secs_f64());
            println!("结论: ✗ 不能直接用（须先转 Qwen3 架构 q8_0/F32 GGUF）");
            std::process::exit(1);
        }
    }
}
