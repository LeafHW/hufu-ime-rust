//! 「的窒闷 vs 的至闷」ngram 分项探针。
//! 用法: cargo run -p hufu-sentence --example ngramprobe -- <模型路径>

use hufu_sentence::model::NgramModel;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .map(|s| std::path::PathBuf::from(s))
        .unwrap_or_else(|| {
            eprintln!("用法: ngramprobe <sentence-ngram.bin 路径>");
            std::process::exit(2);
        });
    let m = NgramModel::load(&path).expect("模型加载失败");

    let de = '的' as u32;
    let zhi_rare = '窒' as u32;
    let men = '闷' as u32;
    let zhi_freq = '至' as u32;
    let bos = hufu_sentence::model::BOS;
    let eos = hufu_sentence::model::EOS;

    println!("══ 字符统计 ══");
    for (name, cp) in [("至", zhi_freq), ("窒", zhi_rare), ("闷", men), ("的", de)] {
        println!(
            "{name} U+{cp:04X}  unigram={:.3e}  freq_rank={} (阈值 3000)",
            m.unigram_prob(cp),
            m.freq_rank(cp)
        );
    }

    println!("══ bigram 观测 ══");
    for (w, c, label) in [
        (de, zhi_freq, "的→至"),
        (de, zhi_rare, "的→窒"),
        (zhi_freq, men, "至→闷"),
        (zhi_rare, men, "窒→闷"),
    ] {
        println!(
            "{label}  has={}  P={:.3e}",
            m.has_bigram(w, c),
            m.bigram_prob(w, c)
        );
    }

    println!("══ 三元路径分（emit 段，不含公共项）══");
    // 路径 A: 的窒闷  路径 B: 的至闷（首字 P(的|BOS,BOS) 相同略）
    let a1 = m.trigram_prob(bos, de, zhi_rare); // P(窒|BOS,的)
    let b1 = m.trigram_prob(bos, de, zhi_freq); // P(至|BOS,的)
    let a2 = m.trigram_prob(de, zhi_rare, men); // P(闷|的,窒)
    let b2 = m.trigram_prob(de, zhi_freq, men); // P(闷|的,至)
    let a3 = m.trigram_prob(zhi_rare, men, eos); // P(EOS|窒,闷)
    let b3 = m.trigram_prob(zhi_freq, men, eos); // P(EOS|至,闷)
    println!("P(窒|BOS,的)={a1:.3e}   P(至|BOS,的)={b1:.3e}");
    println!("P(闷|的,窒)={a2:.3e}   P(闷|的,至)={b2:.3e}");
    println!("P(EOS|窒,闷)={a3:.3e}  P(EOS|至,闷)={b3:.3e}");

    let la = (a1.max(1e-12).ln() + a2.max(1e-12).ln() + a3.max(1e-12).ln()) as f64;
    let lb = (b1.max(1e-12).ln() + b2.max(1e-12).ln() + b3.max(1e-12).ln()) as f64;
    println!("ln 三元合计: 的窒闷={la:.4}  的至闷={lb:.4}  差={:.4}", la - lb);

    // 孤立生僻惩罚（isolation_threshold=3000, lambda=2.0）
    // 窒: rank>3000 且左右 bigram 任一观测到 → 无惩罚；都无 → -2.0
    let zhi_rare_iso_hit =
        m.has_bigram(de, zhi_rare) || m.has_bigram(zhi_rare, men);
    let iso_a = if m.freq_rank(zhi_rare) > 3000 && !zhi_rare_iso_hit {
        -2.0f64
    } else {
        0.0
    };
    println!(
        "孤立惩罚: 的窒闷={iso_a}（窒 rank={}，左右 bigram 命中={zhi_rare_iso_hit}）的至闷=0",
        m.freq_rank(zhi_rare)
    );
    println!(
        "══ 总差（A-B，正数=A 胜出）: {:.4} ══",
        la - lb + iso_a
    );
}
