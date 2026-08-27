//! 探针：直查某方案某码的字典层候选（诊断重复来源）。
//! 用法：cargo run -p hufu-dict --example probe -- <schema_dir> <code>

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| ".".into());
    let code = args.next().unwrap_or_else(|| "a".into());
    let schema = hufu_dict::schema::Schema::load(std::path::Path::new(&dir)).unwrap();
    println!("dict.len = {}", schema.dict.len());
    let raw = schema.dict.lookup(&code);
    println!("dict.lookup({code}) = {} 条:", raw.len());
    for e in raw {
        println!("  [dict] {} -> {} (w={})", e.code, e.text, e.weight);
    }
    let merged = schema.candidates(&code);
    println!("schema.candidates({code}) = {} 条:", merged.len());
    for e in &merged {
        println!("  [merged] {} -> {} pinned={}", e.code, e.text, e.pinned);
    }
    println!("user_dict.entries = {} 条", schema.user_dict.entries.len());
    for e in schema.user_dict.entries.iter().take(5) {
        println!("  [user] {} -> {}", e.code, e.text);
    }
}
