//! 整句录入 A/B 压测：ngram 基线 vs ngram+Qwen 神经重排。
//!
//! 录入规则（整句虎）：逐字全码连打（一简字用 2 码全码），
//! 一句打完才按空格上屏；提前上屏的前缀由引擎自行提交并累计。
//!
//! 并行：每工作线程独立引擎副本（thread_local），分阶段建池、用完即散释放内存。
//!
//! 用法：
//!   sentence-bench <语料.txt> [--arm A|B|AB] [--sample N] [--model path]
//!                  [--wa N] [--wb N] [--out report.md]

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use hufu_types::{KeyCode, KeyInput, Modifiers};
use rayon::prelude::*;

fn key(c: char) -> KeyInput {
    KeyInput {
        key: KeyCode::Char(c),
        modifiers: Modifiers::default(),
        is_press: true,
    }
}

fn space() -> KeyInput {
    KeyInput {
        key: KeyCode::Space,
        modifiers: Modifiers::default(),
        is_press: true,
    }
}

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
/// 全线程共享一份 ngram 解码器（省 16×214MB 副本）
static SDEC: OnceLock<Option<std::sync::Arc<dyn hufu_engine::SentenceDecoder>>> = OnceLock::new();

thread_local! {
    static ENGINE: RefCell<hufu_engine::Engine> = RefCell::new(build_engine());
}

fn build_engine() -> hufu_engine::Engine {
    let data_dir = DATA_DIR.get().unwrap().clone();
    let mut config = hufu_config::Config::load(&data_dir.join("config.json")).unwrap_or_default();
    // 压测客观性：关用户学习（避免跨句污染）
    config.user.auto_frequency = false;
    config.user.log_adjust = false;
    let mut engine = hufu_engine::Engine::new(&data_dir, config).expect("引擎构建");
    if let Some(dec) = SDEC.get().and_then(|d| d.clone()) {
        engine.set_sentence_decoder(Some(dec));
    } else {
        let ngram = data_dir.join(&engine.config.sentence.ngram_path);
        let dec = hufu_sentence::SentenceEngine::load(
            &ngram,
            engine.schema.dict.clone(),
            &engine.schema.supplement,
            engine.config.sentence.weights.clone(),
        )
        .expect("ngram 加载");
        engine.set_sentence_decoder(Some(std::sync::Arc::new(dec)));
    }
    engine
}

/// 句子 → 每字录入码（一简/单码字升级为 2 码全码；字典无码返回 None）
struct Transcriber {
    cache: HashMap<char, Option<String>>,
}

impl Transcriber {
    fn new() -> Self {
        Transcriber {
            cache: HashMap::new(),
        }
    }
    fn code_of(&mut self, ch: char, eng: &hufu_engine::Engine) -> Option<String> {
        if let Some(v) = self.cache.get(&ch) {
            return v.clone();
        }
        let mut buf = [0u8; 4];
        let s: &str = ch.encode_utf8(&mut buf);
        let codes = eng.schema.dict.all_codes_of(s);
        let pick = codes
            .iter()
            .find(|c| c.chars().count() == 2)
            .or_else(|| codes.iter().find(|c| c.chars().count() >= 2))
            .cloned()
            .or_else(|| codes.first().cloned());
        self.cache.insert(ch, pick.clone());
        pick
    }
    fn transcribe(&mut self, s: &str, eng: &hufu_engine::Engine) -> Option<String> {
        let mut out = String::with_capacity(s.len() * 4);
        for ch in s.chars() {
            match self.code_of(ch, eng) {
                Some(c) => out.push_str(&c),
                None => return None,
            }
        }
        Some(out)
    }
}

/// 编辑距离（字符级）
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

#[derive(Default, Clone)]
struct Stats {
    n: usize,
    exact: usize,
    char_err: usize,
    char_total: usize,
    /// 按句长分桶：(全对句数, 句数)
    buckets: Vec<(usize, usize)>,
    key_total: usize,
    ms_total: f64,
}

impl Stats {
    fn bucket_idx(len: usize) -> usize {
        match len {
            4..=9 => 0,
            10..=15 => 1,
            16..=21 => 2,
            _ => 3,
        }
    }
    fn add(&mut self, typed: &str, target: &str, keys: usize) {
        self.n += 1;
        let b = Self::bucket_idx(target.chars().count());
        while self.buckets.len() <= b {
            self.buckets.push((0, 0));
        }
        self.buckets[b].1 += 1;
        if typed == target {
            self.exact += 1;
            self.buckets[b].0 += 1;
        }
        self.char_err += lev(typed, target);
        self.char_total += target.chars().count();
        self.key_total += keys;
    }
    fn exact_rate(&self) -> f64 {
        if self.n == 0 { 0.0 } else { self.exact as f64 / self.n as f64 }
    }
    fn char_acc(&self) -> f64 {
        if self.char_total == 0 { 0.0 } else { 1.0 - self.char_err as f64 / self.char_total as f64 }
    }
    /// Wilson 95% CI
    fn wilson(&self) -> (f64, f64) {
        let n = self.n as f64;
        if n == 0.0 { return (0.0, 0.0); }
        let p = self.exact_rate();
        let z = 1.96;
        let d = 1.0 + z * z / n;
        let c = p + z * z / (2.0 * n);
        let h = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt();
        (((c - h) / d).max(0.0), ((c + h) / d).min(1.0))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let corpus = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"E:\DSH-KF\语料\test_sentences_50k.txt"));
    let mut arm = String::from("AB");
    let mut sample_b: usize = 2000;
    let mut wa: usize = 8;
    let mut _wb: usize = 2; // 已由全局池替代（保留参数兼容）
    let mut out_path = PathBuf::from(r"E:\DSH-KF\hufu\docs\benchmark-qwen-vs-ngram.md");
    let mut i = 2;
    while i + 1 < args.len() + 1 && i < args.len() {
        match args[i].as_str() {
            "--arm" if i + 1 < args.len() => arm = args[i + 1].clone(),
            "--sample" if i + 1 < args.len() => sample_b = args[i + 1].parse().unwrap_or(2000),
            "--wa" if i + 1 < args.len() => wa = args[i + 1].parse().unwrap_or(8),
            "--wb" if i + 1 < args.len() => _wb = args[i + 1].parse().unwrap_or(2),
            "--out" if i + 1 < args.len() => out_path = PathBuf::from(args[i + 1].clone()),
            _ => {}
        }
        i += 2;
    }

    DATA_DIR.set(PathBuf::from(r"E:\DSH-KF\hufu\hufu-data")).unwrap();
    let data_dir = DATA_DIR.get().unwrap().clone();
    // 先建共享 ngram 解码器（全线程复用一份，省 16×214MB）
    {
        let cfg = hufu_config::Config::load(&data_dir.join("config.json")).unwrap_or_default();
        let ngram = data_dir.join(&cfg.sentence.ngram_path);
        let probe = hufu_engine::Engine::new(&data_dir, cfg).expect("引擎构建");
        let dec = hufu_sentence::SentenceEngine::load(
            &ngram,
            probe.schema.dict.clone(),
            &probe.schema.supplement,
            probe.config.sentence.weights.clone(),
        )
        .expect("ngram 加载");
        let _ = SDEC.set(Some(std::sync::Arc::new(dec)));
        drop(probe);
    }
    // 主线程引擎：转码与元信息
    let (schema_name, ngram_disp, model_rel) = {
        let e = build_engine();
        let ngram = data_dir.join(&e.config.sentence.ngram_path);
        (
            e.schema.name.clone(),
            ngram.display().to_string(),
            e.config.sentence.rerank.model_path.clone(),
        )
    };

    // 语料载入 + 转码
    let sentences: Vec<String> = BufReader::new(std::fs::File::open(&corpus).expect("语料打开"))
        .lines()
        .map(|l| l.unwrap_or_default())
        .filter(|l| !l.is_empty())
        .collect();
    let mut tr = Transcriber::new();
    ENGINE.with(|e| {
        let eng = e.borrow();
        let mut typed_able = 0usize;
        let mut untypeable = 0usize;
        let mut cc: Vec<(String, String)> = Vec::with_capacity(sentences.len());
        for s in &sentences {
            match tr.transcribe(s, &eng) {
                Some(c) => {
                    cc.push((s.clone(), c));
                    typed_able += 1;
                }
                None => untypeable += 1,
            }
        }
        eprintln!("语料 {} 句：{}；可录入 {} 句，字典缺字剔除 {} 句", sentences.len(), corpus.display(), typed_able, untypeable);
        CORPUS.with(|c| *c.borrow_mut() = cc);
    });
    let corpus_codes: Vec<(String, String)> = CORPUS.with(|c| c.borrow().clone());
    let typed_able = corpus_codes.len();
    let untypeable = sentences.len() - typed_able;

    // B 臂抽样（固定种子 LCG 洗牌取前 N，保持原序）
    let sample_idx: Vec<usize> = if sample_b >= typed_able {
        (0..typed_able).collect()
    } else {
        let mut state: u64 = 0x9E3779B97F4A7C15;
        let mut idx: Vec<usize> = (0..typed_able).collect();
        for j in (1..idx.len()).rev() {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let r = (state >> 33) as usize % (j + 1);
            idx.swap(j, r);
        }
        idx.truncate(sample_b);
        let mut s = idx;
        s.sort_unstable();
        s
    };

    let do_a = arm.contains('A');
    let do_b = arm.contains('B');
    let b_set: std::collections::HashSet<usize> = sample_idx.iter().copied().collect();

    // ── A 臂：ngram 基线（全量，wa 线程） ──
    let mut sa = Stats::default();
    let mut sa_paired = Stats::default();
    let mut a_ms = 0f64;
    let mut results_a: Vec<(usize, String)> = Vec::new();
    // A 结果磁盘缓存（同语料可复用，重跑 B 不必重打 5 万句）
    let cache_path = std::env::temp_dir().join(format!(
        "hufu-bench-a-{}.jsonl",
        corpus
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(24)
            .collect::<String>()
    ));
    let mut a_cached = false;
    if do_a {
        if cache_path.exists() {
            let mut loaded = Vec::new();
            if let Ok(f) = std::fs::File::open(&cache_path) {
                for line in BufReader::new(f).lines().map_while(Result::ok) {
                    if let Some((i, t)) = line.split_once('\t') {
                        if let Ok(i) = i.parse::<usize>() {
                            loaded.push((i, t.to_string()));
                        }
                    }
                }
            }
            if loaded.len() == typed_able {
                eprintln!("A 臂命中缓存 {} 句：{}", loaded.len(), cache_path.display());
                results_a = loaded;
                a_cached = true;
            } else {
                eprintln!("A 缓存数量不符（{}≠{}），重跑", loaded.len(), typed_able);
            }
        }
        if !a_cached {
        eprintln!("A 臂启动：{} 句 × {} 线程", typed_able, wa);
        let done = AtomicUsize::new(0);
        let t0 = Instant::now();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(wa).build().unwrap();
        results_a = pool.install(|| {
            corpus_codes
                .par_iter()
                .enumerate()
                .map(|(i, (_t, codes))| {
                    let typed = ENGINE.with(|e| run_sentence(&mut e.borrow_mut(), codes, None));
                    let d = done.fetch_add(1, Ordering::Relaxed);
                    if d % 5000 == 4999 {
                        eprintln!("A {}/{} ({:.0}ms/句 均摊)", d + 1, typed_able, t0.elapsed().as_secs_f64() * 1000.0 / (d + 1) as f64);
                    }
                    (i, typed)
                })
                .collect()
        });
        drop(pool); // 释放各线程引擎副本内存
        a_ms = t0.elapsed().as_secs_f64() * 1000.0 / typed_able as f64;
        // 写缓存
        if let Ok(mut f) = std::fs::File::create(&cache_path) {
            use std::io::Write as _;
            for (i, t) in &results_a {
                let _ = writeln!(f, "{i}\t{t}");
            }
            eprintln!("A 结果已缓存：{}", cache_path.display());
        }
        }
        for (i, typed) in &results_a {
            let (target, codes) = &corpus_codes[*i];
            let keys = codes.chars().count();
            sa.add(typed, target, keys);
            if b_set.contains(i) {
                sa_paired.add(typed, target, keys);
            }
        }
        eprintln!("A 臂完成：exact={:.2}% 字准={:.3}%", sa.exact_rate() * 100.0, sa.char_acc() * 100.0);
    }

    // ── B 臂：ngram + Qwen 重排（抽样；句串行 + 候选 5 路并行走全局池） ──
    // 注意：不能用 scoped 小池或 16 句嵌套并行 —— 前者把 gemm 困在小池单核，
    // 后者因堆/缓存争用实测掉到 ~2.4 核。句串行 + 候选并行（主线程发起）最稳。
    let mut sb = Stats::default();
    let mut b_ms = 0f64;
    let mut rerank_applicable = 0usize;
    let mut model_disp = String::new();
    let mut results_b: Vec<(usize, String)> = Vec::new();
    if do_b {
        let mp = if PathBuf::from(&model_rel).is_absolute() {
            PathBuf::from(&model_rel)
        } else {
            data_dir.join(&model_rel)
        };
        model_disp = mp.display().to_string();
        eprintln!("B 臂启动：{} 句（句串行 + 候选并行，全局池 {} 核）；模型 {}", sample_idx.len(), rayon::current_num_threads(), model_disp);
        let t0 = Instant::now();
        let rr = std::sync::Arc::new(hufu_rerank::Reranker::load(&model_disp).expect("模型加载"));
        eprintln!("模型加载完成 {:.1}s", t0.elapsed().as_secs_f64());
        let applicable = AtomicUsize::new(0);
        let applicable = AtomicUsize::new(0);
        let n_total = sample_idx.len();
        let mut done: usize = 0;
        for &i in &sample_idx {
            let (_target, codes) = &corpus_codes[i];
            let typed = ENGINE.with(|e| {
                let e2 = &mut *e.borrow_mut();
                e2.rerank_cache.lock().unwrap().clear();
                let mut session = hufu_engine::Session::new(true);
                let mut committed = String::new();
                for c in codes.chars() {
                    let out = e2.process_key(&mut session, key(c));
                    if let Some(t) = &out.commit {
                        committed.push_str(t);
                    }
                }
                if let Some((k, ctx, cands)) = e2.rerank_request(&session) {
                    // 句串行 + 候选 5 路并行（主线程发起 → 全局池），gemm 块可被空闲核偷取。
                    // 实测 16 句嵌套并行反而因堆/缓存争用掉到 ~2.4 核，此结构最稳。
                    let scores: Vec<f64> = cands
                        .par_iter()
                        .map(|c| rr.score(&ctx, std::slice::from_ref(c))[0])
                        .collect();
                    let mut order: Vec<(f64, String)> =
                        scores.into_iter().zip(cands.iter().cloned()).collect();
                    order.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap_or(std::cmp::Ordering::Equal));
                    e2.rerank_cache
                        .lock()
                        .unwrap()
                        .insert(k, order.into_iter().map(|(_, t)| t).collect());
                    applicable.fetch_add(1, Ordering::Relaxed);
                }
                let out = e2.process_key(&mut session, space());
                if let Some(t) = &out.commit {
                    committed.push_str(t);
                }
                committed
            });
            results_b.push((i, typed));
            done += 1;
            if done % 100 == 0 {
                eprintln!("B {}/{} ({:.0}ms/句 均摊)", done, n_total, t0.elapsed().as_secs_f64() * 1000.0 / done as f64);
            }
        }
        b_ms = t0.elapsed().as_secs_f64() * 1000.0 / sample_idx.len() as f64;
        rerank_applicable = applicable.load(Ordering::Relaxed);
        for (i, typed) in &results_b {
            let (target, codes) = &corpus_codes[*i];
            sb.add(typed, target, codes.chars().count());
        }
        eprintln!("B 臂完成：exact={:.2}% 字准={:.3}%", sb.exact_rate() * 100.0, sb.char_acc() * 100.0);
    }

    // ── 报告 ──
    let mut r = String::new();
    r.push_str("# 整句录入 A/B 压测：ngram 基线 vs +Qwen3-0.6B 神经重排\n\n");
    r.push_str(&format!(
        "- 语料：`{}`（{} 句，可录入 {}，字典缺字剔除 {}）\n",
        corpus.display(),
        sentences.len(),
        typed_able,
        untypeable
    ));
    r.push_str("- 录入规则：逐字全码连打（一简字取 2 码全码），一句打完才空格上屏；提前上屏前缀由引擎提交并累计\n");
    r.push_str("- 压测期关闭用户词学习与调整日志（客观基线）\n");
    r.push_str(&format!(
        "- 方案：{}；ngram：`{}`\n",
        schema_name, ngram_disp
    ));
    if do_b {
        r.push_str(&format!("- 重排模型：`{}`（Q8_0，top_k=5，停顿后空格前一次性重排）\n\n", model_disp));
    } else {
        r.push('\n');
    }

    r.push_str("## 结果\n\n");
    r.push_str("| 臂 | 句数 | 首选全对率 | 95% CI | 字准确率 | 均耗时/句 |\n|---|---|---|---|---|---|\n");
    if do_a {
        let (lo, hi) = sa.wilson();
        let a_ms_disp = if a_cached { "缓存".to_string() } else { format!("{:.1}ms", a_ms) };
        r.push_str(&format!(
            "| A ngram 基线（全量） | {} | {:.2}% | [{:.2}%, {:.2}%] | {:.3}% | {} |\n",
            sa.n,
            sa.exact_rate() * 100.0,
            lo * 100.0,
            hi * 100.0,
            sa.char_acc() * 100.0,
            a_ms_disp
        ));
    }
    if do_b {
        let (lo, hi) = sb.wilson();
        r.push_str(&format!(
            "| B ngram+Qwen 重排（抽样） | {} | {:.2}% | [{:.2}%, {:.2}%] | {:.3}% | {:.0}ms |\n",
            sb.n,
            sb.exact_rate() * 100.0,
            lo * 100.0,
            hi * 100.0,
            sb.char_acc() * 100.0,
            b_ms
        ));
        let (lo, hi) = sa_paired.wilson();
        r.push_str(&format!(
            "| A 同子集（配对） | {} | {:.2}% | [{:.2}%, {:.2}%] | {:.3}% | - |\n",
            sa_paired.n,
            sa_paired.exact_rate() * 100.0,
            lo * 100.0,
            hi * 100.0,
            sa_paired.char_acc() * 100.0
        ));
    }
    if do_b {
        r.push_str(&format!(
            "\n**配对差（B − A 同子集）：{:+.2} 个百分点**（B 全对 {} / A 全对 {}，共 {} 句）\n\n",
            (sb.exact_rate() - sa_paired.exact_rate()) * 100.0,
            sb.exact,
            sa_paired.exact,
            sb.n
        ));
        r.push_str(&format!(
            "- 重排实际介入：{}/{} 句（其余句整句候选 <2 或无候选差）\n",
            rerank_applicable, sb.n
        ));
        // 翻转统计：B 把非首选句改成首选（打捞）与反例（拖累）
        let map_a: HashMap<usize, String> = results_a.iter().cloned().collect();
        let map_b: HashMap<usize, String> = results_b.iter().cloned().collect();
        let mut rescued = 0usize;
        let mut broken = 0usize;
        for &i in &sample_idx {
            if let (Some(ta), Some(tb)) = (map_a.get(&i), map_b.get(&i)) {
                let target = &corpus_codes[i].0;
                let a_ok = ta == target;
                let b_ok = tb == target;
                if !a_ok && b_ok {
                    rescued += 1;
                }
                if a_ok && !b_ok {
                    broken += 1;
                }
            }
        }
        r.push_str(&format!(
            "- 打捞（A 错 → B 对）：{} 句；拖累（A 对 → B 错）：{} 句\n",
            rescued, broken
        ));
    }

    r.push_str("\n## 按句长分桶（首选全对率）\n\n| 句长 | A 全量 | B 抽样 |\n|---|---|---|\n");
    let names = ["4-9 字", "10-15 字", "16-21 字", "22-30 字"];
    for (b, name) in names.iter().enumerate() {
        let a = sa.buckets.get(b).copied().unwrap_or((0, 0));
        let bb = sb.buckets.get(b).copied().unwrap_or((0, 0));
        let ar = if a.1 > 0 { format!("{:.2}% ({}/{})", a.0 as f64 / a.1 as f64 * 100.0, a.0, a.1) } else { "-".into() };
        let br = if bb.1 > 0 { format!("{:.2}% ({}/{})", bb.0 as f64 / bb.1 as f64 * 100.0, bb.0, bb.1) } else { "-".into() };
        r.push_str(&format!("| {name} | {ar} | {br} |\n"));
    }

    if do_a {
        r.push_str(&format!(
            "\n## 效率\n\n- A 臂均 {:.1}ms/句（含逐键 ngram 解码 + 空格上屏，{} 并行线程均摊后）\n",
            a_ms,
            wa
        ));
    }
    if do_b {
        r.push_str(&format!(
            "- B 臂均 {:.0}ms/句（Qwen3-0.6B Q8 每句 5 候选各全前向；全局池 {} 核句级+候选级并行均摊后）\n",
            b_ms,
            std::env::var("NUMBER_OF_PROCESSORS").unwrap_or_else(|_| "?".into())
        ));
        r.push_str(&format!(
            "- B 抽样 {} 句（固定种子洗牌，配对比较）；折算全量 5 万句纯 CPU 估约 {:.0} 小时，不在本轮执行\n",
            sample_b,
            b_ms * 50000.0 / 1000.0 / 3600.0
        ));
    }

    let _ = std::fs::create_dir_all(out_path.parent().unwrap_or(std::path::Path::new(".")));
    let mut f = std::fs::File::create(&out_path).expect("报告创建");
    let _ = f.write_all(r.as_bytes());
    eprintln!("报告已写：{}", out_path.display());
    println!("{r}");
}

thread_local! {
    static CORPUS: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// 打一句：逐码按键 → （可选）停顿重排 → 空格上屏。返回累计上屏文本。
fn run_sentence(
    engine: &mut hufu_engine::Engine,
    codes: &str,
    rerank: Option<(&hufu_rerank::Reranker, &AtomicUsize)>,
) -> String {
    let mut session = hufu_engine::Session::new(true);
    let mut committed = String::new();
    for c in codes.chars() {
        let out = engine.process_key(&mut session, key(c));
        if let Some(t) = &out.commit {
            committed.push_str(t);
        }
    }
    if let Some((rr, applicable)) = rerank {
        engine.rerank_cache.lock().unwrap().clear();
        if let Some((k, ctx, cands)) = engine.rerank_request(&session) {
            let scores = rr.score(&ctx, &cands);
            let mut order: Vec<(f64, String)> = scores.into_iter().zip(cands.iter().cloned()).collect();
            order.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            engine
                .rerank_cache
                .lock()
                .unwrap()
                .insert(k, order.into_iter().map(|(_, t)| t).collect());
            applicable.fetch_add(1, Ordering::Relaxed);
        }
    }
    let out = engine.process_key(&mut session, space());
    if let Some(t) = &out.commit {
        committed.push_str(t);
    }
    committed
}
