//! 索引构建分步计时：`cargo run -p hufu-dict --example indextime -- <文件>`
use hufu_dict::entry::rank_cmp;
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).expect("用法: indextime <码表文件>");
    let lines = hufu_dict::parse::read_lines(std::path::Path::new(&path)).unwrap();
    let table = hufu_dict::parse::parse_auto(&lines);
    let entries = table.rows;
    println!("条目: {}", entries.len());

    let t = Instant::now();
    let mut order: Vec<u32> = (0..entries.len() as u32).collect();
    order.sort_by(|&i, &j| rank_cmp(&entries[i as usize], &entries[j as usize]));
    println!("排序: {:?}", t.elapsed());

    let t = Instant::now();
    let mut by_code: std::collections::HashMap<String, Vec<u32>> = Default::default();
    for idx in &order {
        let code = entries[*idx as usize].code.clone();
        by_code.entry(code).or_default().push(*idx);
    }
    println!("by_code: {:?}  键数 {}", t.elapsed(), by_code.len());

    let t = Instant::now();
    let mut nodes: Vec<std::collections::HashMap<char, u32>> = vec![Default::default()];
    for idx in &order {
        let code = entries[*idx as usize].code.clone();
        let mut cur = 0usize;
        for ch in code.chars() {
            let next = nodes[cur].get(&ch).copied();
            let next = match next {
                Some(n) => n as usize,
                None => {
                    let id = nodes.len() as u32;
                    nodes.push(Default::default());
                    nodes[cur].insert(ch, id);
                    id as usize
                }
            };
            cur = next;
        }
    }
    println!("trie: {:?}  节点数 {}", t.elapsed(), nodes.len());

    let t = Instant::now();
    let mut t2c: std::collections::HashMap<String, Vec<String>> = Default::default();
    for idx in &order {
        let e = &entries[*idx as usize];
        let v = t2c.entry(e.text.clone()).or_default();
        if !v.contains(&e.code) {
            v.push(e.code.clone());
        }
    }
    println!("text_to_codes: {:?}  词条数 {}", t.elapsed(), t2c.len());
}
