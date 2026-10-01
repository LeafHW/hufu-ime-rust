//! 方案目录整装计时探针：Schema::load 全流程耗时 + 主词典/用户词条目数。
//! 用法: cargo run -p hufu-dict --example schemaload -- <方案目录>
use std::time::Instant;

fn main() {
    let dir = std::env::args().nth(1).expect("用法: schemaload <方案目录>");
    let t0 = Instant::now();
    match hufu_dict::Schema::load(std::path::Path::new(&dir)) {
        Ok(s) => {
            println!(
                "加载完成: 耗时 {:?}\n  主词典条目: {}\n  用户词条目: {}\n  补充语料: {:?}",
                t0.elapsed(),
                s.dict.len(),
                s.user_dict.entries.len(),
                s.supplement,
            );
            // 逐码路径探针：candidates() 每键成本与产出
            for code in ["a", "wq", "aikt", "zzzz"] {
                let t = Instant::now();
                let c = s.candidates(code);
                let texts: Vec<&str> = c.iter().take(5).map(|e| e.text.as_str()).collect();
                println!(
                    "  candidates('{code}'): {:?} 返回 {} 条, 前5={:?}",
                    t.elapsed(),
                    c.len(),
                    texts
                );
            }
        }
        Err(e) => println!("加载失败: {e}（耗时 {:?}）", t0.elapsed()),
    }
}
