//! 码表解析探针：`cargo run -p hufu-dict --example loadprobe -- <文件>`
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).expect("用法: loadprobe <码表文件>");
    let t0 = Instant::now();
    let lines = hufu_dict::parse::read_lines(std::path::Path::new(&path)).expect("读取失败");
    let fmt = hufu_dict::parse::sniff_format(&lines);
    println!(
        "文件: {path}\n行数: {}  嗅探格式: {fmt:?}  读取耗时 {:?}",
        lines.len(),
        t0.elapsed()
    );
    let t1 = Instant::now();
    let table = hufu_dict::parse::parse_auto(&lines);
    println!("解析: {} 行  耗时 {:?}  meta={:?}", table.rows.len(), t1.elapsed(), table.meta);
    let n = table.rows.len().min(3);
    for e in &table.rows[..n] {
        println!("  样例: code='{}' text='{}' weight={}", e.code, e.text, e.weight);
    }
    let t2 = Instant::now();
    let dict = hufu_dict::Dict::from_entries("probe", table.rows);
    println!(
        "建索引: 耗时 {:?}  总条目 {}  lookup('a')={} lookup('u')={}",
        t2.elapsed(),
        dict.len(),
        dict.lookup("a").first().map(|e| e.text.clone()).unwrap_or_default(),
        dict.lookup("u").first().map(|e| e.text.clone()).unwrap_or_default(),
    );
}
