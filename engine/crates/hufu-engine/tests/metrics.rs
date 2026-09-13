//! 千句五指标对比工具（准率/上屏率/均上屏次数/残留码长/触达）。
//! 口径：
//!   准率     = 首选全对句数/句数（整句累计文本==目标）
//!   字准     = 1 - 编辑距离/总字数
//!   上屏率   = 提前上屏字数 / 最终累计上屏总字数（流式覆盖）
//!   均上屏次数 = 提前上屏发生总次数 / 句数
//!   残留码长 = 空格键按下时 live raw 的平均字符数
//!   触达     = 至少发生一次提前上屏的句子占比
use std::collections::HashMap;

fn key(c: char) -> hufu_types::KeyInput {
    hufu_types::KeyInput {
        key: hufu_types::KeyCode::Char(c),
        modifiers: hufu_types::Modifiers::default(),
        is_press: true,
    }
}

fn sp() -> hufu_types::KeyInput {
    hufu_types::KeyInput {
        key: hufu_types::KeyCode::Space,
        modifiers: hufu_types::Modifiers::default(),
        is_press: true,
    }
}

fn lev(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[test]
fn metrics_1k() {
    let version = std::env::var("HF_BENCH_VER").unwrap_or_else(|_| "current".into());
    let data_dir = std::path::Path::new(r"E:\DSH-KF\hufu\hufu-data");
    let corpus = std::fs::read_to_string(r"E:\DSH-KF\语料\test_sentences_1k.txt").unwrap();
    let cfg = hufu_config::Config::load(&data_dir.join("config.json")).unwrap_or_default();
    let mut engine = hufu_engine::Engine::new(data_dir, cfg).expect("引擎构建");
    let ngram = data_dir.join(&engine.config.sentence.ngram_path);
    let dec = hufu_sentence::SentenceEngine::load(
        &ngram,
        engine.schema.dict.clone(),
        &engine.schema.supplement,
        engine.config.sentence.weights.clone(),
    )
    .expect("ngram 加载");
    engine.set_sentence_decoder(Some(std::sync::Arc::new(dec)));

    let mut tr: HashMap<char, String> = HashMap::new();
    let code_of = |ch: char,
                   tr: &mut HashMap<char, String>,
                   eng: &hufu_engine::Engine|
     -> Option<String> {
        if let Some(v) = tr.get(&ch) {
            return Some(v.clone());
        }
        let s: String = ch.to_string();
        let codes = eng.schema.dict.all_codes_of(&s);
        let pick = codes
            .iter()
            .find(|c| c.chars().count() == 2)
            .or_else(|| codes.iter().find(|c| c.chars().count() >= 2))
            .cloned()
            .or_else(|| codes.first().cloned());
        if let Some(p) = &pick {
            tr.insert(ch, p.clone());
        }
        pick
    };

    let (mut n, mut exact, mut char_err, mut char_total) = (0usize, 0usize, 0usize, 0usize);
    let (mut commits_total, mut early_chars, mut out_chars) = (0usize, 0usize, 0usize);
    let (mut resid_sum, mut touched) = (0usize, 0usize);

    for line in corpus.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let mut codes = String::new();
        let mut ok = true;
        for ch in t.chars() {
            match code_of(ch, &mut tr, &engine) {
                Some(c) => codes.push_str(&c),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        let mut session = hufu_engine::Session::new(true);
        let mut committed = String::new();
        let mut commits = 0usize;
        let mut ech = 0usize;
        for c in codes.chars() {
            let out = engine.process_key(&mut session, key(c));
            if let Some(t) = &out.commit {
                commits += 1;
                ech += t.chars().count();
                committed.push_str(t);
            }
        }
        let resid = session.raw.chars().count();
        let out = engine.process_key(&mut session, sp());
        if let Some(t) = &out.commit {
            committed.push_str(t);
        }
        n += 1;
        if &committed == t {
            exact += 1;
        }
        char_err += lev(&committed, t);
        char_total += t.chars().count();
        commits_total += commits;
        early_chars += ech;
        out_chars += committed.chars().count();
        resid_sum += resid;
        if commits > 0 {
            touched += 1;
        }
    }

    println!(
        "HF_METRICS|{}|{}|{:.4}|{:.4}|{:.4}|{:.4}|{:.4}|{:.4}",
        version,
        n,
        exact as f64 / n as f64,
        1.0 - char_err as f64 / char_total as f64,
        early_chars as f64 / out_chars.max(1) as f64,
        commits_total as f64 / n as f64,
        resid_sum as f64 / n as f64,
        touched as f64 / n as f64,
    );
}