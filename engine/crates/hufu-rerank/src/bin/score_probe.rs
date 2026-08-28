//! 打分探针：score-probe <ctx> <候选1> <候选2> ...
//! 输出三种度量对比：sum(cand) / mean(cand) / P(EOT|ctx+cand) 与 sum+eot。

use hufu_rerank::Reranker;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: score-probe <ctx> <cand1> <cand2> ...");
        std::process::exit(1);
    }
    let ctx = args[1].clone();
    let cands: Vec<String> = args[2..].to_vec();
    let model = std::env::var("HUFU_MODEL").unwrap_or_else(|_| {
        r"E:\DSH-KF\hufu\hufu-data\models\sentence-qwen-q8.gguf".into()
    });
    let rr = Reranker::load(&model).expect("模型加载");
    for c in &cands {
        let (sum, mean, eot, sum_eot) = rr.score_debug(&ctx, c);
        println!(
            "{c}\tsum={sum:.3}\tmean={mean:.3}\teot={eot:.3}\tsum+eot={sum_eot:.3}"
        );
    }
}
