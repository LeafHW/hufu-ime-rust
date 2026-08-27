//! HuFu 原生格式：TSV，`码\t词\t[权重\t[stem]]`。
//!
//! 头部：`#hufu-dict v1 name=... [version=...]`；`#` 注释。
//! 用作平台交换 / 导出格式，与虎整句空格格式等价但允许携带权重。

use super::{parse_weight, RawTable, TableMeta};
use crate::entry::DictEntry;

pub fn parse(lines: &[String]) -> RawTable {
    let mut meta = TableMeta::default();
    let mut rows = Vec::new();
    let mut seq = 0u32;
    for line in lines {
        let t = line.trim_end();
        let trimmed = t.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("#hufu-dict") {
            for kv in trimmed.split_whitespace().skip(2) {
                if let Some((k, v)) = kv.split_once('=') {
                    match k {
                        "name" => meta.name = v.to_string(),
                        "version" => meta.version = v.to_string(),
                        _ => {}
                    }
                }
            }
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = t.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let code = parts[0].trim().to_string();
        let text = parts[1].trim().to_string();
        if code.is_empty() || text.is_empty() {
            continue;
        }
        let mut e = DictEntry::new(code, text, seq);
        if let Some(w) = parts.get(2) {
            e.weight = parse_weight(w);
        }
        if let Some(s) = parts.get(3) {
            let s = s.trim();
            if !s.is_empty() {
                e.stem = Some(s.to_string());
            }
        }
        rows.push(e);
        seq += 1;
    }
    RawTable { meta, rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_roundtrip_shape() {
        let lines: Vec<String> = vec![
            "#hufu-dict v1 name=test version=1".into(),
            "u\t的\t1000000".into(),
            "t\t我\t900000\tw".into(),
            "aaaa\t魑魅魍魉".into(),
        ];
        let t = parse(&lines);
        assert_eq!(t.meta.name, "test");
        assert_eq!(t.rows.len(), 3);
        assert_eq!(t.rows[1].stem.as_deref(), Some("w"));
        assert_eq!(t.rows[2].weight, 0.0);
    }
}
