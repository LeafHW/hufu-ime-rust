//! 复现 Qwen 神经重排对 ueeyiahx 候选的打分。用法: rerankprobe <gguf路径>

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rr = hufu_rerank::Reranker::load(&args[1]).expect("重排模型加载");
    let cands: Vec<String> = vec![
        "的窒闷".into(),
        "拖至闷".into(),
        "的也盘心".into(),
        "拖乿心".into(),
    ];
    for ctx in ["", "房间空气", "屋里"] {
        let scores = rr.score(ctx, &cands);
        println!("── ctx=[{ctx}] ──");
        let mut order: Vec<(f64, &String)> = scores.iter().copied().zip(cands.iter()).collect();
        order.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        for (i, (s, t)) in order.iter().enumerate() {
            println!("  {}. {t}  score={s:.4}", i + 1);
        }
    }
}
