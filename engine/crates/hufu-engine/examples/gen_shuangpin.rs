//! 双拼反查表生成器（一次性工具，2026-09-07）。
//!
//! 用法：cargo run --release --example gen_shuangpin -- <PY_c.dict.yaml 路径> <输出目录>
//!
//! 从 Rime 全拼词典生成 微软双拼.txt / 自然码.txt 反查表（词<TAB>码）。
//! 键位来源：ulpb.app/schemes/microsoft 与 /schemes/ziranma（2026-09-07 抓取）。
//!
//! 规则要点：
//! - 声母：zh→V, ch→I, sh→U（两家相同）；单字母声母保持。
//! - 韵母键位两家几乎一致；差异：ing（微软=; 自然码=Y）、ü（微软=Y 自然码=V）、
//!   uai 两家都在 Y（自然码 Y=ing+uai 共键，微软 Y=uai+ü）。
//! - 零声母：微软=O+韵母键（a→oa, ai→ol…）；自然码=按音节形态
//!   （a→aa, ai→ai, ang→ah, e→ee, eng→eg…）。
//! - yu→yu、wu→wu 特判；y/w 作声母剥离后查韵母表。
//! - nü/lü（词典写作 nv/lv）：微软→ny/ly，自然码→nv/lv。
//! - ju/qu/xu 等声母后 u 保持 u（不按 ü 转键）。

use std::collections::BTreeMap;
use std::io::{BufRead, BufWriter, Write};

/// 两家共同的韵母键位（差异项由参数覆盖）。
fn finals_map(ms: bool) -> BTreeMap<&'static str, char> {
    let mut m: BTreeMap<&'static str, char> = BTreeMap::new();
    let kv: &[(&str, char)] = &[
        ("iu", 'q'), ("ia", 'w'), ("ua", 'w'), ("uan", 'r'), ("ue", 't'), ("üe", 't'),
        ("uo", 'o'), ("un", 'p'), ("iong", 's'), ("ong", 's'), ("iang", 'd'), ("uang", 'd'),
        ("en", 'f'), ("eng", 'g'), ("ang", 'h'), ("an", 'j'), ("ao", 'k'), ("ai", 'l'),
        ("ei", 'z'), ("ie", 'x'), ("iao", 'c'), ("ui", 'v'), ("ou", 'b'), ("in", 'n'),
        ("ian", 'm'),
        // 单韵母（与声母相拼时）
        ("a", 'a'), ("e", 'e'), ("i", 'i'), ("o", 'o'), ("u", 'u'), ("er", 'r'),
    ];
    for (k, v) in kv {
        m.insert(k, *v);
    }
    if ms {
        m.insert("ing", ';');
        m.insert("uai", 'y');
        m.insert("ü", 'y');
    } else {
        m.insert("ing", 'y');
        m.insert("uai", 'y');
        m.insert("ü", 'v');
    }
    m
}

/// 零声母音节表。
fn zero_map(ms: bool) -> BTreeMap<&'static str, &'static str> {
    if ms {
        // 微软：O + 韵母键（a→oa …）
        [
            ("a", "oa"), ("ai", "ol"), ("an", "oj"), ("ang", "oh"), ("ao", "ok"),
            ("e", "oe"), ("ei", "oz"), ("en", "of"), ("eng", "og"), ("er", "or"),
            ("o", "oo"), ("ou", "ob"),
        ]
        .into_iter()
        .collect()
    } else {
        // 自然码：按音节形态
        [
            ("a", "aa"), ("ai", "ai"), ("an", "an"), ("ang", "ah"), ("ao", "ao"),
            ("e", "ee"), ("ei", "ei"), ("en", "en"), ("eng", "eg"), ("er", "er"),
            ("o", "oo"), ("ou", "ou"),
        ]
        .into_iter()
        .collect()
    }
}

/// 单音节 → 双拼码。
fn conv(syl: &str, ms: bool) -> Option<String> {
    let fm = finals_map(ms);
    let zm = zero_map(ms);
    if let Some(v) = zm.get(syl) {
        return Some(v.to_string());
    }
    if syl == "yu" {
        return Some("yu".into());
    }
    if syl == "wu" {
        return Some("wu".into());
    }
    // nv / lv → ü 韵母特判（Rime 词典以 v 代 ü）
    if syl.len() == 2 && (syl.starts_with('n') || syl.starts_with('l')) && syl.ends_with('v') {
        let sm = &syl[..1];
        let key = fm.get("ü").copied().unwrap_or('v');
        return Some(format!("{}{}", sm, key));
    }
    // 声母剥离（zh/ch/sh 最长优先；y/w 作声母；单字母声母）
    let (sm, fin): (&str, &str) = if let Some(f) = syl.strip_prefix("zh") {
        ("v", f)
    } else if let Some(f) = syl.strip_prefix("ch") {
        ("i", f)
    } else if let Some(f) = syl.strip_prefix("sh") {
        ("u", f)
    } else {
        let first = syl.chars().next()?;
        match first {
            'b' | 'p' | 'm' | 'f' | 'd' | 't' | 'n' | 'l' | 'g' | 'k' | 'h' | 'j' | 'q'
            | 'x' | 'r' | 'z' | 'c' | 's' | 'y' | 'w' => {
                let idx = first.len_utf8();
                (&syl[..idx], &syl[idx..])
            }
            _ => return None,
        }
    };
    let key = fm.get(fin)?;
    Some(format!("{}{}", sm, key))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!(
            "用法: gen_shuangpin <PY_c.dict.yaml> <输出目录> <字频csv（Jun Da hanziDB）> <小鹤表（过滤蓝本）>"
        );
        std::process::exit(2);
    }
    let src = &args[1];
    let out_dir = &args[2];
    let freq_csv = &args[3];
    let xiaohe_tbl = &args[4];

    // 【2026-09-07 用户拍板】反查表以小鹤（Bime 成品 43.9 万词）为蓝本：
    // 只保留小鹤收录的词（微软双拼方案删除——无人使用；全拼/自然码
    // 原始生成各 82 万条体积过大）。表格式：词<TAB>码。
    let mut keep: std::collections::HashSet<String> = std::collections::HashSet::new();
    {
        let f = std::fs::File::open(xiaohe_tbl).expect("打开小鹤表失败");
        let br = std::io::BufReader::new(f);
        for line in br.lines() {
            let line = line.unwrap();
            let word = line.split('\t').next().unwrap_or("").trim();
            if !word.is_empty() {
                keep.insert(word.to_string());
            }
        }
        eprintln!("小鹤蓝本: {} 词", keep.len());
    }

    // 字频表（Jun Da 现代汉语字频，frequency_rank 列）：rank 越小越常用。
    // CSV 列：frequency_rank,character,pinyin,definition,...
    let mut rank: std::collections::HashMap<char, i64> = std::collections::HashMap::new();
    {
        let f = std::fs::File::open(freq_csv).expect("打开字频表失败");
        let br = std::io::BufReader::new(f);
        for line in br.lines().skip(1) {
            let line = line.unwrap();
            let mut it = line.split(',');
            let (Some(r), Some(c)) = (it.next(), it.next()) else {
                continue;
            };
            let (Ok(r), Some(ch)) = (r.trim().parse::<i64>(), c.trim().chars().next()) else {
                continue;
            };
            rank.entry(ch).or_insert(r);
        }
        eprintln!("字频表: {} 字", rank.len());
    }

    // 第一遍：收集全部合法音节（单字词条的 code 字段，空格分隔的每段）
    let f = std::fs::File::open(src).expect("打开词典失败");
    let br = std::io::BufReader::new(f);
    let mut in_body = false;
    let mut entries: Vec<(String, Vec<String>, i64)> = Vec::new(); // (词, [音节], 分数)
    for line in br.lines() {
        let line = line.unwrap();
        if !in_body {
            if line.trim_end() == "..." {
                in_body = true;
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let text = parts[0].trim();
        let code = parts[1].trim();
        if text.is_empty() || code.is_empty() {
            continue;
        }
        let weight: i64 = parts.get(2).and_then(|w| w.trim().parse().ok()).unwrap_or(0);
        let syls: Vec<String> = code
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        if syls.is_empty() {
            continue;
        }
        // 【排序分 2026-09-07】PY_c 的 weight 字段对单字质量差（「的」在
        // de 组垫底）。单字改用 Jun Da 字频 rank（1e9 基准，rank 越小分
        // 越高）；多字词沿用 PY_c weight（词权重可用），整体压在字之下
        // （反查先选字，词为补充）；词权重封顶避免压过字。
        // 【小鹤过滤】非蓝本收录词整条剔除（用户拍板瘦身）。
        let chars: Vec<char> = text.chars().collect();
        let score: i64 = if chars.len() == 1 {
            let r = rank.get(&chars[0]).copied().unwrap_or(200_000);
            1_000_000_000 - r * 10_000
        } else {
            weight.min(900_000_000)
        };
        if !keep.contains(text) {
            continue;
        }
        entries.push((text.to_string(), syls, score));
    }
    // 【候选序=行序】PY_c 词典生僻字排在前（权重低），直接输出会让反查
    // 候选生僻字优先。按权重降序稳定排序后再编码：常用字进前列。
    entries.sort_by(|a, b| b.2.cmp(&a.2));
    eprintln!("词条: {}（小鹤过滤+权重降序）", entries.len());

    for (name, ms) in [("自然码", false)] {
        let mut out = BufWriter::new(
            std::fs::File::create(format!("{out_dir}/{name}.txt")).expect("创建输出失败"),
        );
        let mut n = 0usize;
        let mut bad = 0usize;
        for (text, syls, _) in &entries {
            let mut code = String::new();
            let mut ok = true;
            for s in syls {
                match conv(s, ms) {
                    Some(c) => code.push_str(&c),
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                let _ = writeln!(out, "{}\t{}", text, code);
                n += 1;
            } else {
                bad += 1;
            }
        }
        out.flush().unwrap();
        eprintln!("{name}: 输出 {n} 条，跳过 {bad} 条");
    }

    // 全拼表（音节连写）：同一 entries 流，同一排序分。
    {
        let mut out = BufWriter::new(
            std::fs::File::create(format!("{out_dir}/全拼.txt")).expect("创建输出失败"),
        );
        let mut n = 0usize;
        for (text, syls, _) in &entries {
            let code: String = syls.concat();
            if code.is_empty() {
                continue;
            }
            let _ = writeln!(out, "{}\t{}", text, code);
            n += 1;
        }
        out.flush().unwrap();
        eprintln!("全拼: 输出 {n} 条");
    }
}
