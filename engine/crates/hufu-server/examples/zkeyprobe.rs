//! 【z 键 4 选两景·实机复核 2026-10-18】用真实数据目录（狂练方案 +
//! 用户 config：z→select4、/→text:、）逐键复现两个需求场景：
//! ① 码表词候选不足 4 时按 z = 顶屏首选 + z 菜单（重复上屏/撤回）；
//! ② ; 引导态按 z = 快符 ;z=·（不被 select4 截走）。
//! 用法: cargo run --release -p hufu-server --example zkeyprobe -- <数据目录>

use hufu_engine::{Engine, Session};
use hufu_types::{KeyCode, KeyInput, Modifiers};

fn key(c: char) -> KeyInput {
    KeyInput {
        key: KeyCode::Char(c),
        modifiers: Modifiers::default(),
        is_press: true,
    }
}

fn show(tag: &str, out: &hufu_types::KeyOutcome, s: &Session) {
    let cands: Vec<String> = s.candidates.iter().map(|c| c.text.clone()).collect();
    println!(
        "{tag}: commit=[{}] back={} raw=[{}] 候选={cands:?}",
        out.commit.clone().unwrap_or_default(),
        out.back,
        s.raw
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data_dir = std::path::PathBuf::from(&args[1]);
    let cfg = hufu_config::Config::load(&data_dir.join("config.json")).unwrap_or_default();
    println!(
        "方案={} z→{:?} /→{:?} 顿号直出={} ;引导={}",
        cfg.schema.current,
        cfg.keymap.map.get("z"),
        cfg.keymap.map.get("/"),
        cfg.input.slash_dunhao,
        cfg.input.semicolon_guide
    );
    let mut engine = Engine::new(&data_dir, cfg).expect("引擎构建");
    println!(
        "z_symbol_domain={} sentence_active={}",
        engine.schema.dict.z_symbol_domain,
        engine.sentence_active()
    );

    // 找一个恰好 3 候选的码（越界前提）
    let mut code3 = String::new();
    let mut word3 = String::new();
    'outer: for e in engine.schema.dict.entries.iter() {
        if e.code.len() == 3 && !e.code.starts_with('z') && e.code.chars().all(|c| c.is_ascii_lowercase())
        {
            let n = engine.schema.candidates(&e.code).len();
            if n == 3 {
                code3 = e.code.clone();
                word3 = engine.schema.candidates(&e.code)[0].text.clone();
                break 'outer;
            }
        }
    }
    println!("三候选样本码：{code3} → 首选[{word3}]");
    if code3.is_empty() {
        println!("（码表没有 3 候选码——用 2 候选码对照越界）");
    }

    // ① 三候选码 + z：顶屏首选 + z 菜单
    if !code3.is_empty() {
        let mut s = Session::new(true);
        for c in code3.chars() {
            engine.process_key(&mut s, key(c));
        }
        println!("  打码 {code3} 后候选数={}", s.candidates.len());
        let out = engine.process_key(&mut s, key('z'));
        show("① 按 z", &out, &s);
        let ok1 = out.commit.as_deref() == Some(word3.as_str())
            && s.raw == "z"
            && s.candidates.len() == 2;
        println!("  ① 判定：{}\n", if ok1 { "PASS ✓" } else { "FAIL ✗" });
    }

    // ② ;z=· 快符
    {
        let mut s = Session::new(true);
        let o1 = engine.process_key(&mut s, key(';'));
        println!("  按 ; raw=[{}] commit=[{}]", s.raw, o1.commit.clone().unwrap_or_default());
        let out = engine.process_key(&mut s, key('z'));
        show("② 再按 z", &out, &s);
        let ok2 = out.commit.as_deref() == Some("·") && s.raw.is_empty();
        println!("  ② 判定：{}\n", if ok2 { "PASS ✓" } else { "FAIL ✗" });
    }

    // ③ z 符号域内再按 z：两种正确结局，由数据定夺——
    //    有 zz… 编码（狂练 410 个 zz* 码）→ 编码延续优先，raw="zz"（打
    //    zzbd=「」类）；无 zz 编码（单测夹具）→ select4 越界 → {重复上屏}
    //    展开顶屏 + z 菜单再来。两者都是「延续优先于选重」的同一哲学。
    {
        // 公开面探测：主码表 zz 前缀（狂练 410 个 zz* 码）；快符无 zz 码
        let has_zz = !engine.schema.dict.completions("zz", 1).is_empty();
        println!("  （本码表 zz 有延续编码={has_zz}）");
        engine.push_commit_history("上一条");
        let mut s = Session::new(true);
        engine.process_key(&mut s, key('z'));
        let out = engine.process_key(&mut s, key('z'));
        show("③ z 域再按 z", &out, &s);
        let ok3 = if has_zz {
            out.commit.is_none() && s.raw == "zz"
        } else {
            out.commit.as_deref() == Some("上一条") && s.raw == "z"
        };
        println!("  ③ 判定：{}", if ok3 { "PASS ✓" } else { "FAIL ✗" });
    }
}
