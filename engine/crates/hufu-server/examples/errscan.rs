//! 同类错案自查扫描：1000 句逐字二简转码 → 整句解码 → 首选 vs 期望，
//! 剥掉公共前后缀取差异区，按「差异词是否在码表/是否可被补充语料救」归类。
//! 用法: errscan <语料.txt> [首N句]

use hufu_engine::Engine;
use std::collections::HashMap;
use std::io::BufRead;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let corpus = &args[1];
    let limit: usize = args.get(2).map(|s| s.parse().unwrap_or(1000)).unwrap_or(1000);
    let data_dir = std::path::PathBuf::from(r"E:\DSH-KF\hufu\hufu-data");
    let cfg = hufu_config::Config::load(&data_dir.join("config.json")).unwrap_or_default();
    let mut engine = Engine::new(&data_dir, cfg).expect("引擎构建");
    let ngram = data_dir.join(&engine.config.sentence.ngram_path);
    let dec = hufu_sentence::SentenceEngine::load(
        &ngram,
        engine.schema.dict.clone(),
        &engine.schema.supplement,
        engine.config.sentence.weights.clone(),
    )
    .expect("ngram");
    engine.set_sentence_decoder(Some(std::sync::Arc::new(dec)));

    let sentences: Vec<String> = std::io::BufReader::new(
        std::fs::File::open(corpus).expect("语料打开"),
    )
    .lines()
    .map(|l| l.unwrap_or_default())
    .filter(|l| !l.is_empty())
    .take(limit)
    .collect();

    // 二简转码（与 sentence_bench 同规则）
    let mut code_cache: HashMap<char, Option<String>> = HashMap::new();
    let mut wrong = 0usize;
    let mut untypeable = 0usize;
    // 差异区统计: (期望差异串, 实际差异串) → 次数
    let mut diffs: HashMap<(String, String), usize> = HashMap::new();
    // 期望差异串是否是码表词条（多字词存在）
    let mut in_dict: HashMap<String, bool> = HashMap::new();

    for s in &sentences {
        let mut raw = String::with_capacity(s.len() * 3);
        let mut ok = true;
        for ch in s.chars() {
            let pick = code_cache.entry(ch).or_insert_with(|| {
                let mut buf = [0u8; 4];
                let t: &str = ch.encode_utf8(&mut buf);
                let codes = engine.schema.dict.all_codes_of(t);
                // 熟练用户策略：句中一简非法（段跨≥2），取「该字为首选」
                // 的最短 ≥2 码（长句 n>4 只取 rank1，二简名次 2 的字走全码），
                // 否则退最短 ≥2 码，再退任何码。
                let mut by_len: Vec<&String> = codes.iter().collect();
                by_len.sort_by_key(|c| c.chars().count());
                let mut rank1: Option<String> = None;
                let mut ge2: Option<String> = None;
                for c in by_len {
                    if c.chars().count() < 2 {
                        continue;
                    }
                    if engine.schema.dict.lookup(c).first().map(|e| e.text.as_str())
                        == Some(t)
                    {
                        rank1 = Some(c.clone());
                        break;
                    }
                    if ge2.is_none() {
                        ge2 = Some(c.clone());
                    }
                }
                rank1.or(ge2).or_else(|| codes.first().cloned())
            });
            match pick {
                Some(c) => raw.push_str(c),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            untypeable += 1;
            continue;
        }
        let got = engine
            .sentence_decoder()
            .map(|d| d.decode(&raw))
            .and_then(|mut v| if v.is_empty() { None } else { Some(v.remove(0)) });
        let Some(top) = got else { continue };
        if top.text == *s {
            continue;
        }
        wrong += 1;
        // 剥公共前后缀
        let e: Vec<char> = s.chars().collect();
        let g: Vec<char> = top.text.chars().collect();
        let mut p = 0;
        while p < e.len() && p < g.len() && e[p] == g[p] {
            p += 1;
        }
        let mut q = 0;
        while q < e.len() - p && q < g.len() - p && e[e.len() - 1 - q] == g[g.len() - 1 - q] {
            q += 1;
        }
        let de: String = e[p..e.len() - q].iter().collect();
        let dg: String = g[p..g.len() - q].iter().collect();
        if de.is_empty() || dg.is_empty() {
            continue;
        }
        // 全长差异太长的句子不参与词统计（整句崩坏另算）
        if de.chars().count() <= 4 {
            *diffs.entry((de.clone(), dg)).or_insert(0) += 1;
            let has = engine.schema.dict.text_to_codes.contains_key(&de);
            in_dict.insert(de, has);
        }    }

    let mut ranked: Vec<_> = diffs.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1));
    println!("句数={} 错={} 缺字跳过={}", sentences.len(), wrong, untypeable);
    println!("══ 差异区 Top 40（期望段 → 实际段 ×次数 〔码表有词条?〕）══");
    for ((de, dg), n) in ranked.iter().take(40) {
        let has = in_dict.get(de).copied().unwrap_or(false);
        println!("{de} → {dg}  ×{n}  〔词条:{has}〕");
    }
}
