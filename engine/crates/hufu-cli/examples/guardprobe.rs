//! 「灵珑天狠狠的」护栏失效定位探针（2026-09-28）。
//! 逐键复放 raw，打印每次提前上屏提交时的：delta、stable、consumed、
//! 候选池（early_hits / hits 各前 6：text, conf, partial, word_ends），
//! 以及护栏判定输入（leader_conf、各活候选距池首分差、分歧位）。
//! 用法: guardprobe <方案目录> <v5ngram> <raw>

use hufu_config::Config;
use hufu_engine::{Engine, Session};
use hufu_types::KeyInput;
use std::path::Path;
use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, ngram) = (
        args.get(1).expect("用法: guardprobe <方案目录> <ngram> <raw>"),
        args.get(2).expect("用法: guardprobe <方案目录> <ngram> <raw>"),
    );
    let raw = args.get(3).cloned().unwrap_or_else(|| "bchntlfmmigmigue".into());

    let schema = hufu_dict::schema::Schema::load(Path::new(dir)).expect("方案加载失败");
    let mut w = Config::default().sentence.weights.clone();
    w.digit_codes = schema.dict.digit_coded;
    let dec = Arc::new(
        hufu_sentence::SentenceEngine::load(Path::new(ngram), schema.dict.clone(), &schema.supplement, w)
            .expect("ngram 装载失败"),
    );

    let mut cfg = Config::default();
    cfg.sentence.early_commit = true;
    cfg.sentence.early_diverg_guard = true; // 护栏开着观察
    let mut engine = Engine::with_schema_dir(Path::new(dir), cfg).expect("引擎初始化失败");
    engine.set_sentence_decoder(Some(dec.clone()));
    let mut sess = Session::new(true);

    // 解码器直查：每个前缀的 early_hits / hits 池（不经过引擎会话过滤）
    let full_raw = raw.clone();
    let chars: Vec<char> = full_raw.chars().collect();
    for i in 1..=chars.len() {
        let prefix: String = chars[..i].iter().collect();
        let out = engine.process_key(&mut sess, KeyInput::char_key(chars[i - 1]));
        let committed_now = out.commit.clone().unwrap_or_default();
        let dec_rich = engine
            .sentence_decoder()
            .map(|d| d.decode_rich(&format!("{}{}", sess.committed_raw, sess.raw)));
        if let Some(dr) = dec_rich {
            println!("═══ 键{} '{}' prefix={} ═══", i, chars[i - 1], prefix);
            if !committed_now.is_empty() {
                println!("  ★ 提前上屏 delta='{}'（{} 字） committed_text='{}' 剩raw='{}'",
                    committed_now,
                    committed_now.chars().count(),
                    sess.committed_text,
                    sess.raw);
            }
            let show = |tag: &str, v: &Vec<hufu_engine::SentenceHit>| {
                if !v.is_empty() {
                    let leader = v.iter().map(|h| h.confidence).fold(f64::NEG_INFINITY, f64::max);
                    let mut lines: Vec<String> = v.iter().take(6).map(|h| {
                        format!("      '{}' conf={:.2} Δ距首={:.2} partial={} word_ends={:?}",
                            h.text, h.confidence, leader - h.confidence, h.partial, h.word_ends)
                    }).collect();
                    lines.insert(0, format!("    [{}] 池首conf={:.2} 条数={}", tag, leader, v.len()));
                    for l in lines { println!("{l}"); }
                }
            };
            show("early_hits(不完全尾)", &dr.early_hits);
            show("hits(完整态)", &dr.hits);
        }
    }
    let out = engine.process_key(&mut sess, KeyInput::char_key(' '));
    println!("═══ 空格收尾 ═══");
    println!("  最终上屏: {}{}", sess.committed_text, out.commit.unwrap_or_default());
}
