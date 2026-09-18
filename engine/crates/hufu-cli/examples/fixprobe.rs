//! 「却足以绊住流沙的舌尖」修复方案验证探针（2026-09-19）。
//! 场景矩阵：
//!   A 默认（提前上屏开，无重排）        —— 现网行为，预期错句
//!   B 提前上屏关 + 句末 qwen 重排       —— 验证重排能否选对
//!   C 提前上屏开 + 句末重排             —— 验证第17键锁死后重排救不回
//! 另附 qwen 对两句的直接分项打分。
//! 用法: fixprobe <方案目录> <v5ngram> <qwen.gguf>

use hufu_config::Config;
use hufu_engine::{Engine, Session};
use hufu_types::KeyInput;
use std::path::Path;
use std::sync::Arc;

const RAW: &str = "gkzpuvjihujinkbkytueasymd";
const WANT: &str = "却足以绊住流沙的舌尖";
const WRONG: &str = "却足以收拾主流沙的舌尖";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, ngram, gguf) = (
        args.get(1).expect("用法: fixprobe <方案目录> <v5ngram> <qwen.gguf> [raw want]"),
        args.get(2).expect("用法: fixprobe <方案目录> <v5ngram> <qwen.gguf> [raw want]"),
        args.get(3).expect("用法: fixprobe <方案目录> <v5ngram> <qwen.gguf> [raw want]"),
    );
    // 可选第 4/5 参：自定义句子（raw 期望句），用于抽验其它同码错句
    let (raw, want): (&str, &str) = match (args.get(4), args.get(5)) {
        (Some(r), Some(w)) => (r, w),
        _ => (RAW, WANT),
    };
    let guard = std::env::var("HUFU_EARLY_DIVERG_GUARD").map(|v| v == "1").unwrap_or(false);
    println!("句子: {want}  raw: {raw}  护栏: {}", if guard { "开" } else { "关" });
    let t0 = std::time::Instant::now();

    // 解码器共享（ngram 573MB mmap 只装一次）
    let schema = hufu_dict::schema::Schema::load(Path::new(dir)).expect("方案加载失败");
    let mut w = Config::default().sentence.weights.clone();
    w.digit_codes = schema.dict.digit_coded;
    let dec = Arc::new(
        hufu_sentence::SentenceEngine::load(Path::new(ngram), schema.dict.clone(), &schema.supplement, w)
            .expect("ngram 装载失败"),
    );
    // qwen（native 优先，纯 Rust 兜底）
    let native = hufu_rerank::native::NativeScorer::try_new(&[], Path::new(gguf));
    let rust_rr = if native.is_none() { hufu_rerank::Reranker::load(gguf).ok() } else { None };
    let engine_kind = if native.is_some() { "native(llama.cpp)" } else if rust_rr.is_some() { "rust" } else { "无!!" };
    println!("[加载] {:.0}ms  rerank={engine_kind}", t0.elapsed().as_millis());

    // ── qwen 直接打分：ctx="。" 下两句的分项（默认句专用；自定义句由场景 B/C 端到端验证）──
    if args.len() <= 4 {
        let score = |c: &str| -> (f64, f64, f64, f64) {
            if let Some(ns) = &native { let s = ns.score("。", &[c.to_string()]); return (s[0], f64::NAN, f64::NAN, s[0]); }
            if let Some(rr) = &rust_rr { return rr.score_debug("。", c); }
            (f64::NAN, f64::NAN, f64::NAN, f64::NAN)
        };
        let (a_sum, _, _, a_tot) = score(WANT);
        let (b_sum, _, _, b_tot) = score(WRONG);
        println!("[qwen] 对  | {WANT}  sum={a_sum:.3} total={a_tot:.3}");
        println!("[qwen] 错  | {WRONG}  sum={b_sum:.3} total={b_tot:.3}");
        println!("[qwen] 判定: {}", if a_tot > b_tot { "✓ 选对句" } else { "✗ 仍选错句" });
    }

    // ── 场景矩阵：每场景独立 Engine（隔离学习写回串扰）+ 独立 Session ──
    let run = |tag: &str, early: bool, rerank: bool, fire_rerank: bool, guard: bool| {
        let mut cfg = Config::default();
        cfg.sentence.early_commit = early;
        cfg.sentence.rerank.enabled = rerank;
        cfg.sentence.early_diverg_guard = guard;
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
        let mid_txt = if mids.is_empty() { "（无）".to_string() } else { mids.join("|") };
        // 句末重排（打完 25 键、空格收尾前）
        let cand_before: Vec<String> = sess.candidates.iter().take(3).map(|c| c.text.clone()).collect();
        if fire_rerank {
            if let Some((key, ctx, texts)) = engine.rerank_request(&sess) {
                let scores: Vec<f64> = if let Some(ns) = &native {
                    ns.score(&ctx, &texts)
                } else if let Some(rr) = &rust_rr {
                    rr.score(&ctx, &texts)
                } else {
                    Vec::new()
                };
                if scores.len() == texts.len() && texts.len() >= 2 {
                    let mut order: Vec<(f64, String)> = scores.into_iter().zip(texts.into_iter()).collect();
                    order.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                    let new_texts: Vec<String> = order.into_iter().map(|(_, t)| t).collect();
                    engine.rerank_cache.lock().unwrap().insert(key, new_texts);
                    engine.refresh_rerank(&mut sess);
                }
            }
        }
        let cand_after: Vec<String> = sess.candidates.iter().take(3).map(|c| c.text.clone()).collect();
        let out = engine.process_key(&mut sess, KeyInput::char_key(' '));
        if let Some(c) = out.commit {
            committed.push_str(&c);
        }
        let ok = committed == want;
        println!(
            "[{tag}] 提前上屏={early} 句末重排={fire_rerank} 护栏={guard}  中途上屏: {mid_txt}\n      句尾候选(前3): {cand_before:?} → 重排后: {cand_after:?}\n      最终: {committed}  {}",
            if ok { "✓✓ 正确" } else { "✗ 错" }
        );
    };

    run("A 现网默认  ", true, false, false, false);
    run("B 关提前+重排", false, true, true, false);
    run("C 开提前+重排", true, true, true, false);
    run("D 护栏+重排  ", true, true, true, true);
}
