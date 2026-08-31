//! hufu-cli —— 码表转换 / 方案检查 / 引擎 REPL。

use hufu_config::Config;
use hufu_dict::schema::Schema;
use hufu_engine::{Engine, Session};
use hufu_types::KeyInput;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("help");
    match cmd {
        "check" => cmd_check(args.get(2).expect("用法: check <方案目录>")),
        "convert" => cmd_convert(
            args.get(2).expect("用法: convert <输入文件> <输出文件>"),
            args.get(3).expect("用法: convert <输入文件> <输出文件>"),
        ),
        "repl" => cmd_repl(args.get(2).expect("用法: repl <方案目录>")),
        "bench" => cmd_bench(
            args.get(2).expect("用法: bench <方案目录> <语料> [ngram路径]"),
            args.get(3).expect("用法: bench <方案目录> <语料> [ngram路径]"),
            args.get(4).map(|s| s.to_string()),
        ),
        _ => {
            println!("hufu-cli 命令：");
            println!("  check   <方案目录>   加载方案并输出统计与样例候选");
            println!("  convert <输入> <输出> 任意支持格式 → HuFu 原生 TSV");
            println!("  repl    <方案目录>   逐字符模拟输入（q 退出，BS 退格，SP 空格）");
            println!("  bench   <方案目录> <语料> [ngram] 整句质量基准（exact 率 + 逐句解码耗时）");
        }
    }
}

/// 整句质量基准：语料每句 → 逐字 best_code_of 拼编码 → SentenceEngine
/// 解码 → top1 与原句比对 exact。_beam 调档前后的回归护栏
/// （历史基准：100 句 exact 92.93%）。
fn cmd_bench(dir: &str, corpus: &str, ngram: Option<String>) {
    let t0 = Instant::now();
    let schema = Schema::load(Path::new(dir)).expect("方案加载失败");
    let cfg = Config::default();
    let ngram_path = ngram
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("模型/sentence-ngram.bin"));
    println!(
        "方案 {} 条目 {}（{:.0}ms）ngram={}",
        schema.name,
        schema.dict.len(),
        t0.elapsed().as_millis(),
        ngram_path.display()
    );
    let dec = match hufu_sentence::SentenceEngine::load(
        &ngram_path,
        schema.dict.clone(),
        &schema.supplement,
        cfg.sentence.weights.clone(),
    ) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("ngram 装载失败: {e}");
            return;
        }
    };
    let sents: Vec<String> = std::io::BufReader::new(
        std::fs::File::open(corpus).expect("语料打开失败"),
    )
    .lines()
    .map(|l| l.unwrap().trim().to_string())
    .filter(|l| !l.is_empty())
    .collect();
    let mut exact = 0usize;
    let mut total = 0usize;
    let mut decode_ms: Vec<u128> = Vec::new();
    for s in &sents {
        // 逐字取码（best_code_of：最短/最优码）拼整句编码
        let mut raw = String::new();
        for ch in s.chars() {
            let code = schema.best_code_of(&ch.to_string()).unwrap_or_default();
            raw.push_str(&code);
        }
        if raw.is_empty() {
            continue;
        }
        total += 1;
        let t1 = Instant::now();
        let hits = hufu_engine::SentenceDecoder::decode(&dec, &raw);
        decode_ms.push(t1.elapsed().as_millis());
        if let Some(top) = hits.first() {
            if top.text == *s {
                exact += 1;
            }
        }
    }
    let avg = decode_ms.iter().sum::<u128>() as f64 / decode_ms.len().max(1) as f64;
    let p95_idx = (decode_ms.len() * 95 / 100).min(decode_ms.len().saturating_sub(1));
    let mut sorted = decode_ms.clone();
    sorted.sort();
    let p95 = sorted.get(p95_idx).copied().unwrap_or(0);
    println!(
        "句数 {total}  exact {}/{} = {:.2}%  解码 avg {:.1}ms  p95 {}ms  max {}ms",
        exact,
        total,
        exact as f64 / total as f64 * 100.0,
        avg,
        p95,
        sorted.last().copied().unwrap_or(0)
    );
}

fn cmd_check(dir: &str) {
    let t0 = Instant::now();
    let schema = Schema::load(Path::new(dir)).expect("方案加载失败");
    let dt = t0.elapsed();
    println!("方案: {}", schema.name);
    println!("条目: {}（加载耗时 {:.0} ms）", schema.dict.len(), dt.as_millis());
    println!(
        "符号: 快符 {} 组 / 斜杠 {} 组 / 一简 {} 组",
        schema.symbols.quick.len(),
        schema.symbols.slash.len(),
        schema.symbols.simple.len()
    );
    println!(
        "注释: 拼音 {} / 分区 {} / 拆分 {} / 反查 {}",
        schema.pinyin.as_ref().map(|t| t.len()).unwrap_or(0),
        schema.unicode_block.as_ref().map(|t| t.len()).unwrap_or(0),
        schema.split.as_ref().map(|t| t.len()).unwrap_or(0),
        schema.reverse.as_ref().map(|t| t.len()).unwrap_or(0),
    );
    println!("补充语料: {} 条", schema.supplement.entries.len());
    println!("用户词: {} 条", schema.user_dict.entries.len());
    println!("---- 样例候选 ----");
    for code in ["a", "u", "t", "jd", "aaaa", "tuja", ";a", "/tm"] {
        let cands = schema.candidates(code);
        let show: Vec<String> = cands
            .iter()
            .take(6)
            .map(|e| e.text.clone())
            .collect();
        println!("  {code:<6} → [{}]", show.join(" "));
    }
}

fn cmd_convert(input: &str, output: &str) {
    let table = hufu_dict::parse::parse_file(Path::new(input)).expect("解析失败");
    let mut out = String::new();
    out.push_str(&format!(
        "#hufu-dict v1 name={} version={}\n",
        if table.meta.name.is_empty() { "converted" } else { &table.meta.name },
        table.meta.version
    ));
    for e in &table.rows {
        out.push_str(&e.code);
        out.push('\t');
        out.push_str(&e.text);
        if e.weight != 0.0 {
            out.push('\t');
            out.push_str(&(e.weight as i64).to_string());
        }
        out.push('\n');
    }
    std::fs::write(output, out).expect("写出失败");
    println!("已转换 {} 条 → {}", table.rows.len(), output);
}

fn cmd_repl(dir: &str) {
    let config = Config::default();
    let mut engine = Engine::with_schema_dir(Path::new(dir), config).expect("引擎初始化失败");
    let _ = &mut engine;
    let mut session = Session::new(true);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    println!("HuFu REPL —— 输入编码字符逐键模拟；'q' 退出；'BS' 退格；'SP' 空格；其余按字面。");
    for line in stdin.lock().lines() {
        let line = line.unwrap();
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "q" {
            break;
        }
        let keys: Vec<KeyInput> = if line == "SP" {
            vec![KeyInput::char_key(' ')]
        } else if line == "BS" {
            vec![KeyInput {
                key: hufu_types::KeyCode::Backspace,
                ..KeyInput::char_key(' ')
            }]
        } else {
            line.chars().map(KeyInput::char_key).collect()
        };
        for k in keys {
            let out = engine.process_key(&mut session, k);
            let st = out.state.clone().unwrap_or_default();
            render(out.consumed, out.commit.as_deref().unwrap_or(""), st);
        }
        stdout.flush().ok();
    }
}

fn render(consumed: bool, commit: &str, st: hufu_types::SessionState) {
    let cands: Vec<String> = st
        .candidates
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{}.{}{}", i + 1, c.text, if c.comment.is_empty() { String::new() } else { format!("({})", c.comment) }))
        .collect();
    println!(
        "[{}] raw='{}' aux='{}' 页 {}/{} | {}",
        if consumed { "吞" } else { "通" },
        st.raw,
        st.aux,
        st.page + 1,
        st.page_count.max(1),
        cands.join(" ")
    );
    if !commit.is_empty() {
        println!("    ↑ 上屏: {commit}");
    }
}
