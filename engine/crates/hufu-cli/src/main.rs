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
        // 【真机重排对比 2026-09-05】逐句输出 v2 口径（整句虎规则）码串：
        // probe 真机 HTTP 逐键模拟用，保证和 tbench 同打法。
        "codes" => {
            let dir = args.get(2).expect("用法: codes <方案目录> <语料>");
            let corpus = args.get(3).expect("用法: codes <方案目录> <语料>");
            let schema = Schema::load(Path::new(dir)).expect("方案加载失败");
            let text = std::fs::read_to_string(corpus).expect("语料读取失败");
            for s in text.lines().filter(|l| !l.trim().is_empty()) {
                let raw: String = s.chars().map(|c| real_code_of(&schema, c)).collect::<Vec<_>>().concat();
                println!("{raw}");
            }
        }
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
    // 【整句虎规则 2026-09-05】整句录入下一简字一律打 2 码全码（「中」
    // 打 dg 不打 d，用户确认的录入规则）。此前实现取 rank 最优的 ≥2
    // 码——被测试码表的 rank 重排带偏（新表把一简字最优排到 3-4 码，
    // tbench 跟着打 3-4 码，+39% 键是口径偏移不是录入负担）。现固定
    // 「存在 2 码则打 2 码，无 2 码才取更长的 rank 最优」。对照旧行为
    // 设 HUFU_BENCH_RAW_RANK=1。
    static RAW_RANK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let raw_rank = *RAW_RANK.get_or_init(|| {
        std::env::var("HUFU_BENCH_RAW_RANK").map(|v| v == "1").unwrap_or(false)
    });
    let picked = if raw_rank {
        // 对照口径：rank 最优的 ≥2 码
        schema.dict.best_code_and_rank(&ch.to_string(), 2)
    } else if schema.dict.has_one_code_first(&ch.to_string()) {
        // 26 一简字：强制 2 码全码（「中」打 dg 不打 d）
        schema
            .dict
            .best_code_and_rank_exact(&ch.to_string(), 2)
            .or_else(|| schema.dict.shortest_first_choice(&ch.to_string()))
    } else {
        // 其余字：最短首选码优先（似=jvj、抑=ubz、揭=uon）；全无首选
        // 码的字打全码+选重锁（势=uk;）
        schema.dict.shortest_first_choice(&ch.to_string())
    };
    match picked {
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

/// 【权重扫描 2026-09-07】bench 系命令的配置：默认值 + HUFU_W_<字段>
/// 环境变量覆盖（f64/usize 按字段类型解析）。零编译扫描整句权重用。
/// 例：HUFU_W_CONFIDENCE=0.95 HUFU_W_EMITTED_CHARACTER_REWARD=4 tbench …
fn bench_config() -> Config {
    let mut cfg = Config::default();
    let w = &mut cfg.sentence.weights;
    macro_rules! w_f64 {
        ($($f:ident),* $(,)?) => { $( if let Ok(v) = std::env::var(concat!("HUFU_W_", stringify!($f))) {
            if let Ok(n) = v.parse::<f64>() { w.$f = n; }
        } )* }
    }
    macro_rules! w_usize {
        ($($f:ident),* $(,)?) => { $( if let Ok(v) = std::env::var(concat!("HUFU_W_", stringify!($f))) {
            if let Ok(n) = v.parse::<usize>() { w.$f = n; }
        } )* }
    }
    w_f64!(rank_penalty, emitted_character_reward, isolation_lambda,
           confidence, dict_bias, supplement_baseline, supplement_scale,
           supplement_maximum);
    w_usize!(beam_width, candidate_limit, max_raw_length, isolation_threshold);
    cfg
}

fn cmd_bench(dir: &str, corpus: &str, ngram: Option<String>) {
    let t0 = Instant::now();
    let schema = Schema::load(Path::new(dir)).expect("方案加载失败");
    let cfg = bench_config();
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
        { let mut w = cfg.sentence.weights.clone(); w.digit_codes = schema.dict.digit_coded; w },
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
    let cfg = bench_config();
    let dec = hufu_sentence::SentenceEngine::load(
        Path::new(ngram),
        schema.dict.clone(),
        &schema.supplement,
        { let mut w = cfg.sentence.weights.clone(); w.digit_codes = schema.dict.digit_coded; w },
    )
    .expect("ngram 装载失败");
    // 【用户词注入 2026-09-06】对齐 server：用户词参与整句词图（调试用）
    {
        let mut seen = std::collections::HashSet::new();
        let words: Vec<(String, String)> = schema
            .user_dict
            .entries
            .iter()
            .filter(|e| !e.code.is_empty() && !e.text.is_empty())
            .filter(|e| seen.insert((e.code.clone(), e.text.clone())))
            .map(|e| (e.code.clone(), e.text.clone()))
            .collect();
        eprintln!("注入用户词 {} 条", words.len());
        hufu_engine::SentenceDecoder::set_user_words(&dec, &words);
    }
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
    let cfg = bench_config();
    let mut engine = Engine::with_schema_dir(Path::new(dir), bench_config())
        .expect("引擎初始化失败");
    if ngram != "-" {
        let dec = hufu_sentence::SentenceEngine::load(
            Path::new(ngram),
            schema.dict.clone(),
            &schema.supplement,
            { let mut w = cfg.sentence.weights.clone(); w.digit_codes = schema.dict.digit_coded; w },
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
    let cfg = bench_config();
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
            { let mut w = cfg.sentence.weights.clone(); w.digit_codes = schema.dict.digit_coded; w },
        )
        .expect("ngram 装载失败");
        engine.set_sentence_decoder(Some(std::sync::Arc::new(dec)));
    }
    // 【rerank AB 2026-09-07】HUFU_RERANK_GGUF=<路径>：B 组——句末收尾
    // 空格前做一次神经重排（真机「打完停顿确认」场景的忠实子集：
    // request→score→cache→refresh，engine 侧消费含选重深度约束）。
    // 引擎 cfg 同步开 rerank（rerank_request 的出活开关）。native
    // llama.cpp 优先（与真机同引擎），失败退纯 Rust（约慢 40 倍）。
    let rerank_gguf = std::env::var("HUFU_RERANK_GGUF").ok().filter(|s| !s.is_empty());
    if rerank_gguf.is_some() {
        engine.config.sentence.rerank.enabled = true;
    }
    let reranker: Option<hufu_rerank::Reranker> = rerank_gguf.as_ref().and_then(|p| {
        hufu_rerank::Reranker::load(p).ok()
    });
    let native_scorer: Option<hufu_rerank::native::NativeScorer> = rerank_gguf.as_ref().and_then(|p| {
        hufu_rerank::native::NativeScorer::try_new(&[], std::path::Path::new(p))
    });
    if rerank_gguf.is_some() {
        let engine_kind = if native_scorer.is_some() { "native(llama.cpp)" } else { "rust(fallback)" };
        println!("[RERANK] {engine_kind}");
    }
    let sents: Vec<String> = std::io::BufReader::new(std::fs::File::open(corpus).expect("语料打开失败"))
        .lines()
        .map(|l| l.unwrap().trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let mut exact = 0usize;
    let mut total = 0usize;
    let mut early_commits = 0usize; // 提前上屏总次数（句中 commit，不含收尾）
    let mut early_chars = 0u64; // 提前上屏总字数（手感：每次几个字）
    let mut early_max = 0usize; // 单次提前上屏最大字数（最长免空格串）
    // 【残留码长 2026-09-05】提前上屏事件瞬间 raw 缓冲剩余键数——上屏
    // 后用户还需继续打/组句的负担，越短越跟手。
    let mut early_resid = 0usize;
    let mut total_chars = 0u64; // 全文总字数（覆盖率分母）
    let mut sent_events: Vec<usize> = Vec::new(); // 每句上屏事件数（提前+收尾，越少越一气呵成）
    let mut total_rerank_fired = 0usize; // 【rerank B 组】句中停顿重排实际触发次数
    let mut total_rerank_changed = 0usize; // 【rerank B 组】qwen 首选≠引擎原序首选的次数
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
        let mut sent_events_this = 0usize;
        // 【rerank B 组 v2】句中停顿点：打满 60% 编码时同步重排一次
        //（真机收益场景——句中停顿时 raw 尚长、Sentence 候选 ≥2；
        // 句末收尾时顶功已推空 raw，request 恒 None，测不出重排）。
        let raw_len = raw.chars().count();
        let pause_at = raw_len * 6 / 10;
        let mut keys_done = 0usize;
        for ch in raw.chars() {
            if line_end_w > 0 {
                let col = sess.committed_raw.chars().count() + sess.raw.chars().count();
                sess.line_end_hint = col % line_end_w >= line_end_w.saturating_sub(3);
            }
            let tk = Instant::now();
            let out = engine.process_key(&mut sess, KeyInput::char_key(ch));
            key_us.push(tk.elapsed().as_micros() as u64);
            keys_done += 1;
            if let Some(c) = out.commit {
                committed.push_str(&c);
                early_commits += 1;
                early_chars += c.chars().count() as u64;
                early_max = early_max.max(c.chars().count());
                early_resid += sess.raw.chars().count();
                sent_events_this += 1;
            }
            // 句中停顿重排（同步：等结果落地再继续打）
            if rerank_gguf.is_some() && keys_done == pause_at {
                if let Some((key, ctx, texts)) = engine.rerank_request(&sess) {
                    let first_before = texts.first().cloned();
                    let scores: Vec<f64> = if let Some(ns) = &native_scorer {
                        ns.score(&ctx, &texts)
                    } else if let Some(rr) = &reranker {
                        rr.score(&ctx, &texts)
                    } else {
                        Vec::new()
                    };
                    if scores.len() == texts.len() && texts.len() >= 2 {
                        let mut order: Vec<(f64, String)> =
                            scores.into_iter().zip(texts.into_iter()).collect();
                        order.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                        let new_texts: Vec<String> = order.into_iter().map(|(_, t)| t).collect();
                        // 分歧统计：qwen 首选 vs 引擎原序首选
                        total_rerank_fired += 1;
                        if new_texts.first() != first_before.as_ref() {
                            total_rerank_changed += 1;
                        }
                        engine
                            .rerank_cache
                            .lock()
                            .unwrap()
                            .insert(key, new_texts);
                        engine.refresh_rerank(&mut sess);
                    }
                }
            }
        }
        // 【rerank B 组】句末停顿重排：打完整句编码、收尾空格上屏前，
        // 按真机管线（rerank_request→score→cache→refresh）换序一次。
        if rerank_gguf.is_some() {
            if let Some((key, ctx, texts)) = engine.rerank_request(&sess) {
                let scores: Vec<f64> = if let Some(ns) = &native_scorer {
                    ns.score(&ctx, &texts)
                } else if let Some(rr) = &reranker {
                    rr.score(&ctx, &texts)
                } else {
                    Vec::new()
                };
                if scores.len() == texts.len() && texts.len() >= 2 {
                    let mut order: Vec<(f64, String)> =
                        scores.into_iter().zip(texts.into_iter()).collect();
                    order.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                    let new_texts: Vec<String> = order.into_iter().map(|(_, t)| t).collect();
                    engine
                        .rerank_cache
                        .lock()
                        .unwrap()
                        .insert(key, new_texts);
                    engine.refresh_rerank(&mut sess);
                }
            }
        }
        // 收尾：空格上屏剩余（延迟也计入）
        let tk = Instant::now();
        let out = engine.process_key(&mut sess, KeyInput::char_key(' '));
        key_us.push(tk.elapsed().as_micros() as u64);
        if let Some(c) = out.commit {
            committed.push_str(&c);
            sent_events_this += 1; // 收尾空格也是一次上屏事件
        }
        sent_events.push(sent_events_this);
        }
        total_ms.push(t1.elapsed().as_millis());
        total_chars += s.chars().count() as u64;
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
    if rerank_gguf.is_some() {
        println!("[RERANK] 句中停顿重排触发 {total_rerank_fired} 次 · qwen 首选与引擎原序分歧 {total_rerank_changed} 次（{}/{:.0}%）",
            total_rerank_changed,
            total_rerank_changed as f64 / total_rerank_fired.max(1) as f64 * 100.0);
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
    // 手感综合评估：提前上屏的字数视角——每次上屏平均几个字、全文
    // 有多大比例的字是免空格提前落地的（覆盖率=少按空格的真实比例）。
    // 残留码长：提前上屏瞬间 raw 缓冲剩余键数（还要继续打/组句的量）。
    println!(
        "手感： 每次提前上屏平均 {:.2} 字（最长 {} 字）  提前上屏覆盖 {:.1}%（{} 字 / 全文 {} 字）  上屏后残留平均 {:.2} 键",
        early_chars as f64 / early_commits.max(1) as f64,
        early_max,
        early_chars as f64 / total_chars.max(1) as f64 * 100.0,
        early_chars,
        total_chars,
        early_resid as f64 / early_commits.max(1) as f64
    );
    // 「几次上屏打完一句」分布（含收尾空格共 1 次，越小越一气呵成）：
    // 1 次=整句全靠句尾空格一次落地；N 次=中途 N-1 次提前+收尾。
    {
        let mut ev = sent_events.clone();
        ev.sort_unstable();
        let sn = ev.len().max(1);
        let q = |x: usize| ev[(sn * x / 100).min(sn - 1)];
        let zero = ev.iter().filter(|&&e| e <= 1).count();
        let two = ev.iter().filter(|&&e| e <= 2).count();
        let three = ev.iter().filter(|&&e| e <= 3).count();
        let avg_ev = ev.iter().sum::<usize>() as f64 / sn as f64;
        let avg_sent_chars = total_chars as f64 / sn as f64;
        println!(
            "句子节奏： 平均 {:.2} 次上屏/句（句均 {:.1} 字，每 {:.1} 字一次）  分位 p25={} p50={} p75={} p95={} max={}  ≤1次 {:.0}%  ≤2次 {:.0}%  ≤3次 {:.0}%",
            avg_ev,
            avg_sent_chars,
            avg_sent_chars / avg_ev.max(0.01),
            q(25),
            q(50),
            q(75),
            q(95),
            ev.last().copied().unwrap_or(0),
            zero as f64 / sn as f64 * 100.0,
            two as f64 / sn as f64 * 100.0,
            three as f64 / sn as f64 * 100.0
        );
    }
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
