//! 提前上屏频率统计：逐键模拟（与 sentence_bench 同法），统计
//! ①空格前发生的提前上屏事件 ②每次平均字数 ③提前上屏覆盖字数占比
//! ④平均每句事件数 ⑤平均每键事件率。用法: commitfreq <语料.txt> [数据目录]

use hufu_engine::Engine;
use hufu_types::{KeyCode, KeyInput, Modifiers};
use std::io::BufRead;

fn key(c: char) -> KeyInput {
    KeyInput { key: KeyCode::Char(c), modifiers: Modifiers::default(), is_press: true }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let corpus = &args[1];
    // 数据目录：第二个参数可选，缺省取当前目录下的 hufu-data
    let data_dir = args
        .get(2)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("hufu-data"));
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

    let sentences: Vec<String> = std::io::BufReader::new(std::fs::File::open(corpus).expect("语料"))
        .lines()
        .map(|l| l.unwrap_or_default())
        .filter(|l| !l.is_empty())
        .collect();

    // 二简转码（与 bench 同规则：句中 ≥2 码）
    let mut code_cache: std::collections::HashMap<char, Option<String>> =
        std::collections::HashMap::new();
    let mut sentences_codes: Vec<(String, String)> = Vec::new();
    for s in &sentences {
        let mut raw = String::with_capacity(s.len() * 3);
        let mut ok = true;
        for ch in s.chars() {
            let pick = code_cache.entry(ch).or_insert_with(|| {
                let mut buf = [0u8; 4];
                let t: &str = ch.encode_utf8(&mut buf);
                let codes = engine.schema.dict.all_codes_of(t);
                codes
                    .iter()
                    .filter(|c| c.chars().count() >= 2)
                    .min_by_key(|c| c.chars().count())
                    .cloned()
                    .or_else(|| codes.first().cloned())
            });
            match pick {
                Some(c) => raw.push_str(c),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            sentences_codes.push((s.clone(), raw));
        }
    }

    let mut n_sent = 0usize;
    let mut events = 0usize;
    let mut event_chars = 0usize;
    let mut total_chars = 0usize;
    let mut total_keys = 0usize;
    let mut sent_with_event = 0usize;

    for (_s, codes) in &sentences_codes {
        let mut session = hufu_engine::Session::new(true);
        let keys = codes.chars().count();
        let mut had_event = false;
        let mut early_chars = 0usize;
        for c in codes.chars() {
            let out = engine.process_key(&mut session, key(c));
            if let Some(t) = &out.commit {
                events += 1;
                event_chars += t.chars().count();
                early_chars += t.chars().count();
                had_event = true;
            }
        }
        // 空格收尾（尾段上屏不计入「提前」）
        let out = engine.process_key(
            &mut session,
            KeyInput { key: KeyCode::Space, modifiers: Modifiers::default(), is_press: true },
        );
        let tail = out.commit.map(|t| t.chars().count()).unwrap_or(0);
        n_sent += 1;
        total_chars += early_chars + tail;
        total_keys += keys;
        if had_event {
            sent_with_event += 1;
        }
    }

    println!("句数={n_sent} 总键数={total_keys} 总字数={total_chars}");
    println!("提前上屏事件={events} （平均 {}/句, {:.2}/百键）", if n_sent > 0 { events / n_sent } else { 0 }, events as f64 * 100.0 / total_keys as f64);
    println!("事件平均字数={:.2}", if events > 0 { event_chars as f64 / events as f64 } else { 0.0 });
    println!("提前上屏字数占比={:.2}%", event_chars as f64 * 100.0 / total_chars as f64);
    println!("发生过提前上屏的句子占比={:.2}%", sent_with_event as f64 * 100.0 / n_sent as f64);
}
