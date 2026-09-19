//! 「绊住流 vs 收拾主」同码分叉 ngram 探针（两模型对比）。
//! 用法: cargo run -p hufu-sentence --release --example banprobe -- <模型路径>

use hufu_sentence::model::NgramModel;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .map(|s| std::path::PathBuf::from(s))
        .unwrap_or_else(|| {
            eprintln!("用法: banprobe <模型路径>");
            std::process::exit(2);
        });
    let m = NgramModel::load(&path).expect("模型加载失败");
    let c = |s: &str| s.chars().next().unwrap() as u32;
    let (zu, yi, ban, zhu, liu, sha, shou, shi, zhu2) = (
        c("足"), c("以"), c("绊"), c("住"), c("流"), c("沙"), c("收"), c("拾"), c("主"),
    );

    println!("══ unigram ══");
    for name in ["绊", "住", "流", "收", "拾", "主"] {
        let cp = c(name);
        println!("{name}  P1={:.3e}  freq_rank={}", m.unigram_prob(cp), m.freq_rank(cp));
    }

    println!("══ bigram 观测 ══");
    for (w, x, label) in [
        (yi, ban, "以→绊"), (ban, zhu, "绊→住"), (zhu, liu, "住→流"),
        (yi, shou, "以→收"), (shou, shi, "收→拾"), (shi, zhu2, "拾→主"),
        (zhu2, liu, "主→流"), (liu, sha, "流→沙"),
    ] {
        println!("{label}  has={}  P={:.3e}", m.has_bigram(w, x), m.bigram_prob(w, x));
    }

    println!("══ trigram 路径分（emit 段）══");
    // A: …以 绊 住 流 沙…   B: …以 收 拾 主 流 沙…
    let (a1, a2, a3, a4) = (
        m.trigram_prob(zu, yi, ban),  // P(绊|足,以)
        m.trigram_prob(yi, ban, zhu), // P(住|以,绊)
        m.trigram_prob(ban, zhu, liu),// P(流|绊,住)
        m.trigram_prob(zhu, liu, sha),// P(沙|住,流)
    );
    let (b1, b2, b3, b4, b5) = (
        m.trigram_prob(zu, yi, shou),   // P(收|足,以)
        m.trigram_prob(yi, shou, shi),  // P(拾|以,收)
        m.trigram_prob(shou, shi, zhu2),// P(主|收,拾)
        m.trigram_prob(shi, zhu2, liu), // P(流|拾,主)
        m.trigram_prob(zhu2, liu, sha), // P(沙|主,流)
    );
    println!("A: P(绊|足,以)={a1:.3e} P(住|以,绊)={a2:.3e} P(流|绊,住)={a3:.3e} P(沙|住,流)={a4:.3e}");
    println!("B: P(收|足,以)={b1:.3e} P(拾|以,收)={b2:.3e} P(主|收,拾)={b3:.3e} P(流|拾,主)={b4:.3e} P(沙|主,流)={b5:.3e}");
    let la: f64 = [a1, a2, a3, a4].iter().map(|p| (*p).max(1e-12) as f64).map(f64::ln).sum();
    let lb: f64 = [b1, b2, b3, b4, b5].iter().map(|p| (*p).max(1e-12) as f64).map(f64::ln).sum();
    // emitted_character_reward：每字 +reward（默认 4.0）→ B 多发一字
    println!("ln 合计 A(4字)={la:.4}  B(5字)={lb:.4}  差(A-B)={:.4}  （B 多 1 字，每字 emit 奖励另计）", la - lb);
}
