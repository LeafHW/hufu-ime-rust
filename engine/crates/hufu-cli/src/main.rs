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
        "code" => cmd_code(
            args.get(2).expect("用法: code <方案目录> <句子>"),
            args.get(3).expect("用法: code <方案目录> <句子>"),
        ),
        "query" => {
            let mut rest: Vec<String> = args[2..].to_vec();
            let mut show_early = false;
            if rest.first().map(|s| s.as_str()) == Some("-e") {
                show_early = true;
                rest.remove(0);
            }
            cmd_query(
                rest.get(0).expect("用法: query [-e] <方案目录> <ngram> <raw...>"),
                rest.get(1).expect("用法: query [-e] <方案目录> <ngram> <raw...>"),
                &rest[2..],
                show_early,
            );
        }
        "cands" => cmd_cands(
            args.get(2).expect("用法: cands <方案目录> <ngram> <raw...>"),
            args.get(3).expect("用法: cands <方案目录> <ngram> <raw...>"),
            &args[4..],
        ),
        "tbench" => cmd_tbench(
            args.get(2).expect("用法: tbench <方案目录> <语料> <ngram路径> [延迟输出]"),
            args.get(3).expect("用法: tbench <方案目录> <语料> <ngram路径> [延迟输出]"),
            args.get(4).expect("用法: tbench <方案目录> <语料> <ngram路径> [延迟输出]"),
            args.get(5).cloned(),
        ),
        _ => {
            println!("hufu-cli 命令：");
            println!("  check   <方案目录>   加载方案并输出统计与样例候选");
            println!("  convert <输入> <输出> 任意支持格式 → HuFu 原生 TSV");
            println!("  repl    <方案目录>   逐字符模拟输入（q 退出，BS 退格，SP 空格）");
            println!("  bench   <方案目录> <语料> [ngram] 整句质量基准（exact 率 + 逐句解码耗时）");
            println!("  code    <方案目录> <句子>   逐字 best_code_of 展示（bench 同款打法）");
            println!("  query   [-e] <方案目录> <ngram> <raw...>  解码候选（分数/名次/切分）");
            println!("  cands   <方案目录> <ngram> <raw...>  逐键 session 候选框（真实 UI 所见）");
        }
    }
}

/// 整句质量基准：语料每句 → 逐字 best_code_of 拼编码 → SentenceEngine
/// 解码 → top1 与原句比对 exact。_beam 调档前后的回归护栏
/// （历史基准：100 句 exact 92.93%）。
/// 整句真实打法：最优码（≥2 码）+ 名次锁键（engine RankLocks 同款：
/// 位次 1 无锁、2=';'、3='\''、4..9=数字、10='0'）。
fn real_code_of(schema: &Schema, ch: char) -> String {
    // 【无锁打法】HUFU_BENCH_NOLOCK=1：只打最优码不取名次锁——模拟
    // 真实单字用户「打码+空格取首选」行为。用于全码表排序扫描：
    // 码表序第一名的字打自己的码+空格，首选必须是自己（否则即
    // 「唬/嘶」类前缀态霸首 bug 残留）。
    static NO_LOCK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let no_lock = *NO_LOCK.get_or_init(|| {
        std::env::var("HUFU_BENCH_NOLOCK").map(|v| v == "1").unwrap_or(false)
    });
    match schema.dict.best_code_and_rank(&ch.to_string(), 2) {
        Some((code, rank)) => {
            if no_lock {
                return code;
            }
            let lock = match rank {
                1 => String::new(),
                2 => ";".into(),
                3 => "'".into(),
                4..=9 => rank.to_string(),
                10 => "0".into(),
                _ => String::new(),
            };
            format!("{code}{lock}")
        }
        None => String::new(),
    }
}

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
    let mut code_exact = 0usize; // 码级：输出句重新编码 == 原编码（同码句算对）
    let mut total = 0usize;
    let mut decode_ms: Vec<u128> = Vec::new();
    let dump_fail: usize = std::env::var("BENCH_DUMP_FAIL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut dumped = 0usize;
    for s in &sents {
        // 逐字取码（best_code_of：最短/最优码）拼整句编码
        let mut raw = String::new();
        for ch in s.chars() {
            // 整句真实打法：一简 2 码全码 + 同码字名次锁（ag; → 装）
            let code = real_code_of(&schema, ch);
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
                code_exact += 1;
            } else {
                // 码级判定：输出句逐字真实编码与原编码一致 = 同码句
                let raw2: String = top.text.chars().map(|c| real_code_of(&schema, c)).collect::<Vec<_>>().concat();
                if raw2 == raw {
                    code_exact += 1;
                }
                if dumped < dump_fail {
                    dumped += 1;
                    println!("[FAIL] 原: {s}");
                    println!("       码: {raw}");
                    println!("       出: {}{}", top.text, if raw2 == raw { "（同码句）" } else { "" });
                }
            }
        }
    }
    let avg = decode_ms.iter().sum::<u128>() as f64 / decode_ms.len().max(1) as f64;
    let p95_idx = (decode_ms.len() * 95 / 100).min(decode_ms.len().saturating_sub(1));
    let mut sorted = decode_ms.clone();
    sorted.sort();
    let p95 = sorted.get(p95_idx).copied().unwrap_or(0);
    println!(
        "句数 {total}  exact {}/{} = {:.2}%  码级 {}/{} = {:.2}%  解码 avg {:.1}ms  p95 {}ms  max {}ms",
        exact,
        total,
        exact as f64 / total as f64 * 100.0,
        code_exact,
        total,
        code_exact as f64 / total as f64 * 100.0,
        avg,
        p95,
        sorted.last().copied().unwrap_or(0)
    );
}

/// 逐字展示：整句真实打法（≥2 码 + 同码字名次锁）。
fn cmd_code(dir: &str, sentence: &str) {
    let schema = Schema::load(Path::new(dir)).expect("方案加载失败");
    let mut raw = String::new();
    for ch in sentence.chars() {
        let code = real_code_of(&schema, ch);
        println!("{ch} → {code}{}", if code.is_empty() { "（无码）" } else { "" });
        raw.push_str(&code);
    }
    println!("整句编码: {raw}");
}

/// 解码候选透视：完整候选（score/confidence/max_rank/segmented），
/// -e 附带提前上屏不完全尾候选。码表直查对照一并输出。
fn cmd_query(dir: &str, ngram: &str, raws: &[String], show_early: bool) {
    let schema = Schema::load(Path::new(dir)).expect("方案加载失败");
    let cfg = Config::default();
    let dec = hufu_sentence::SentenceEngine::load(
        Path::new(ngram),
        schema.dict.clone(),
        &schema.supplement,
        cfg.sentence.weights.clone(),
    )
    .expect("ngram 装载失败");
    for raw in raws {
        println!("━━ raw = {raw}");
        let rich = hufu_engine::SentenceDecoder::decode_rich(&dec, raw);
        for (i, h) in rich.hits.iter().take(10).enumerate() {
            println!(
                "  {:>2}. {}  score={:>8.3} conf={:>8.3} rank={}  {}",
                i + 1,
                h.text,
                h.score,
                h.confidence,
                h.max_rank,
                h.segmented
            );
        }
        if show_early {
            println!("  ── early（不完全尾）:");
            for h in rich.early_hits.iter().take(5) {
                println!(
                "     {}  conf={:>8.3} rank={}  {}",
                h.text, h.confidence, h.max_rank, h.segmented
            );
            }
        }
    }
}

/// session 级候选框透视：逐键喂入，每键后打印候选框（真实 UI 同源）。
fn cmd_cands(dir: &str, ngram: &str, raws: &[String]) {
    let schema = Schema::load(Path::new(dir)).expect("方案加载失败");
    let cfg = Config::default();
    let mut engine = Engine::with_schema_dir(Path::new(dir), Config::default())
        .expect("引擎初始化失败");
    if ngram != "-" {
        let dec = hufu_sentence::SentenceEngine::load(
            Path::new(ngram),
            schema.dict.clone(),
            &schema.supplement,
            cfg.sentence.weights.clone(),
        )
        .expect("ngram 装载失败");
        engine.set_sentence_decoder(Some(std::sync::Arc::new(dec)));
    }
    for raw in raws {
        println!("━━ raw = {raw}");
        let mut sess = Session::new(true);
        for ch in raw.chars() {
            let out = engine.process_key(&mut sess, KeyInput::char_key(ch));
            if let Some(c) = out.commit {
                println!("   [commit] {c}");
            }
            let show: Vec<String> =
                sess.candidates.iter().take(6).map(|c| c.text.clone()).collect();
            println!("   {ch} → [{}]", show.join(" "));
        }
    }
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

/// 逐键打字基准：真实整句打法（一简 2 码 + 同码锁）逐键喂引擎，
/// 统计 准率 / 提前上屏次数 / 每键触达延迟（击键→解码可用）。
/// lat_out 可选：每键延迟 µs 逐行写文件（多进程分片汇总用）。
fn cmd_tbench(dir: &str, corpus: &str, ngram: &str, lat_out: Option<String>) {
    let schema = Schema::load(Path::new(dir)).expect("方案加载失败");
    let cfg = Config::default();
    // ngram == "-"：纯码表模式（不装整句解码器、不调模型不重排）——
    // 顶功码表 + 选重锁单字打法，测纯查表路径触达。
    // 纯码表基准同时关学习（auto_frequency/log_adjust）：逐句调频会
    // 改变后续句的候选序（实测「装」上屏后 ag 首选被顶、「鬼」句打错），
    // 基准必须测裸码表能力。
    let no_lock_scan = std::env::var("HUFU_BENCH_NOLOCK")
        .map(|v| v == "1")
        .unwrap_or(false);
    let bench_cfg = if ngram == "-" {
        let mut c = Config::default();
        c.user.auto_frequency = false;
        c.user.log_adjust = false;
        c
    } else if no_lock_scan {
        // 无锁整句扫描同样关学习：扫描的逐字上屏若写调频，会改变后续
        // 字的候选序，污染判据（回放亦然——跑前须删 user-adjust.log）。
        let mut c = Config::default();
        c.user.auto_frequency = false;
        c.user.log_adjust = false;
        c
    } else {
        Config::default()
    };
    let mut engine = Engine::with_schema_dir(Path::new(dir), bench_cfg).expect("引擎初始化失败");
    if ngram != "-" {
        let dec = hufu_sentence::SentenceEngine::load(
            Path::new(ngram),
            schema.dict.clone(),
            &schema.supplement,
            cfg.sentence.weights.clone(),
        )
        .expect("ngram 装载失败");
        engine.set_sentence_decoder(Some(std::sync::Arc::new(dec)));
    }
    let sents: Vec<String> = std::io::BufReader::new(std::fs::File::open(corpus).expect("语料打开失败"))
        .lines()
        .map(|l| l.unwrap().trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let mut exact = 0usize;
    let mut total = 0usize;
    let mut early_commits = 0usize; // 提前上屏总次数（句中 commit，不含收尾）
    let mut total_ms: Vec<u128> = Vec::new();
    let mut key_us: Vec<u64> = Vec::new(); // 每键触达延迟（µs）
    let dump: usize = std::env::var("BENCH_DUMP_FAIL").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut dumped = 0usize;
    // 【行尾模拟 A/B】HUFU_BENCH_LINE_END=W（W>0 启用）：模拟 TSF 侧行尾
    // 检测——组段尾（已上屏编码+剩余编码的列位置，按每行 W 个编码字符
    // 折行）进入行尾 3 字区即置 line_end_hint。与真机行为同构：真实检测
    // 为 caret 距窗口右缘 <56px（≈2-3 字）。对照组不设该变量（恒 false）。
    let line_end_w: usize = std::env::var("HUFU_BENCH_LINE_END")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if no_lock_scan {
        println!("[NOLOCK 无锁扫描] 只测引擎真实候选序（schema.candidates）第一名==本字的码（整句模式覆盖生僻下沉路径）");
    }
    for s in &sents {
        let raw: String = s.chars().map(|c| real_code_of(&schema, c)).collect::<Vec<_>>().concat();
        if raw.is_empty() {
            continue;
        }
        total += 1;
        let mut sess = Session::new(true);
        let mut committed = String::new();
        let t1 = Instant::now();
        if ngram == "-" || no_lock_scan {
            // 纯码表单字打法 / 无锁整句扫描：逐字打码，顶功自动推字；
            // 未被顶出的字补一次空格强上（真实单字打法用户的行为）。
            // 连续喂整句码流在纯码表模式不可用——无整句解码，码流会
            // 粘连成多字词/长码生僻字（实测 出: 𤁨咆绶 vs 10 字原句）。
            // 无锁扫描必须逐字（单字独立成段才能验证「打自己的码+空格
            // =上自己」），且在整句模式（带模型）下运行——覆盖 rare_rescue
            // 生僻下沉路径：纯码表无模型 rare_hint 恒假，测不出「踹/起」
            // 「唬/嘶」类整句霸首 bug（2026-09-04「踹→起」的教训）。
            let per_char: Vec<String> = s
                .chars()
                .filter_map(|c| {
                    // 无锁扫描判据＝引擎真实序：schema.candidates(c)[0]==字
                    //（best_code_and_rank 的 rank 按 weight 序，与显示序
                    // 不一致，不能用作「第一名」判据）。
                    if no_lock_scan {
                        let ch = c.to_string();
                        let codes = schema.dict.codes_of(&ch);
                        return codes.into_iter().find(|code| {
                            schema
                                .candidates(code)
                                .first()
                                .map(|e| e.text == ch)
                                .unwrap_or(false)
                        });
                    }
                    Some(real_code_of(&schema, c))
                })
                .collect();
            for code in per_char {
                if code.is_empty() {
                    continue;
                }
                for ch in code.chars() {
                    let tk = Instant::now();
                    let out = engine.process_key(&mut sess, KeyInput::char_key(ch));
                    key_us.push(tk.elapsed().as_micros() as u64);
                    if let Some(c) = out.commit {
                        committed.push_str(&c);
                        early_commits += 1;
                    }
                }
                if !sess.raw.is_empty() {
                    let tk = Instant::now();
                    let out = engine.process_key(&mut sess, KeyInput::char_key(' '));
                    key_us.push(tk.elapsed().as_micros() as u64);
                    if let Some(c) = out.commit {
                        committed.push_str(&c);
                    }
                }
            }
        } else {
        for ch in raw.chars() {
            if line_end_w > 0 {
                let col = sess.committed_raw.chars().count() + sess.raw.chars().count();
                sess.line_end_hint = col % line_end_w >= line_end_w.saturating_sub(3);
            }
            let tk = Instant::now();
            let out = engine.process_key(&mut sess, KeyInput::char_key(ch));
            key_us.push(tk.elapsed().as_micros() as u64);
            if let Some(c) = out.commit {
                committed.push_str(&c);
                early_commits += 1;
            }
        }
        // 收尾：空格上屏剩余（延迟也计入）
        let tk = Instant::now();
        let out = engine.process_key(&mut sess, KeyInput::char_key(' '));
        key_us.push(tk.elapsed().as_micros() as u64);
        if let Some(c) = out.commit {
            committed.push_str(&c);
        }
        }
        total_ms.push(t1.elapsed().as_millis());
        if committed == *s {
            exact += 1;
        } else if dumped < dump {
            dumped += 1;
            println!("[FAIL] 原: {s}");
            println!("       码: {raw}");
            println!("       出: {committed}");
        }
    }
    if let Some(path) = lat_out {
        std::fs::write(&path, key_us.iter().map(|u| u.to_string()).collect::<Vec<_>>().join("\n"))
            .expect("延迟文件写出失败");
    }
    let mut sorted = key_us.clone();
    sorted.sort_unstable();
    let n = sorted.len().max(1);
    let p = |q: usize| sorted[(n * q / 100).min(n - 1)];
    let avg = total_ms.iter().sum::<u128>() as f64 / total_ms.len().max(1) as f64;
    println!(
        "句数 {total}  准率 {}/{} = {:.2}%  提前上屏 {} 次（平均 {:.2} 次/句）  键 {}  触达延迟 p50 {}µs p95 {}µs avg {}µs max {}µs",
        exact,
        total,
        exact as f64 / total as f64 * 100.0,
        early_commits,
        early_commits as f64 / total.max(1) as f64,
        key_us.len(),
        p(50),
        p(95),
        key_us.iter().sum::<u64>() as f64 / n as f64,
        sorted.last().copied().unwrap_or(0)
    );
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
