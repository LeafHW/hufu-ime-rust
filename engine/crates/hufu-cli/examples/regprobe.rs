//! 2 句回归排查（2026-09-28）：修1 开/关 两种姿态逐键跑同一 raw，打印
//! 每次提前上屏，对比锁死点。用法: regprobe <方案目录> <ngram> <raw> [want]
use hufu_config::Config;
use hufu_engine::{Engine, Session};
use hufu_types::KeyInput;
use std::path::Path;
use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("用法: regprobe <方案目录> <ngram> <raw> [want]");
        std::process::exit(1);
    }
    let (dir, ngram, raw) = (&args[1], &args[2], &args[3]);
    let want = args.get(4).cloned().unwrap_or_default();
    let schema = hufu_dict::schema::Schema::load(Path::new(dir)).expect("方案加载失败");
    let mut w = Config::default().sentence.weights.clone();
    w.digit_codes = schema.dict.digit_coded;
    let dec = Arc::new(
        hufu_sentence::SentenceEngine::load(Path::new(ngram), schema.dict.clone(), &schema.supplement, w)
            .expect("ngram 装载失败"),
    );
    for &(label, fix1) in &[("v166 修1关", "0"), ("fix1 修1开", "1")] {
        unsafe { std::env::set_var("HUFU_FIX1", fix1) };
        let mut cfg = Config::default();
        cfg.sentence.early_diverg_guard = true;
        let mut engine = Engine::with_schema_dir(Path::new(dir), cfg).expect("引擎初始化失败");
        engine.set_sentence_decoder(Some(dec.clone()));
        let mut sess = Session::new(true);
        let mut committed = String::new();
        let mut mids: Vec<String> = Vec::new();
        for ch in raw.chars() {
            let out = engine.process_key(&mut sess, KeyInput::char_key(ch));
            if let Some(c) = out.commit {
                mids.push(c.clone());
                committed.push_str(&c);
            }
        }
        let out = engine.process_key(&mut sess, KeyInput::char_key(' '));
        if let Some(c) = out.commit {
            committed.push_str(&c);
        }
        let ok = !want.is_empty() && committed == want;
        println!(
            "[{label}] 中途上屏: {}\n   最终: {committed}  {}",
            mids.join("|"),
            if want.is_empty() { String::new() } else if ok { "✓".into() } else { "✗".into() }
        );
    }
}
