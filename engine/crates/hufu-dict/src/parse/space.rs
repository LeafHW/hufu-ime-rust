//! 虎整句格式：`码 词1 词2 词3`（空格分隔，顺序即优先级）。
//!
//! 该格式是引擎就绪形态：符号行（`/jm ぁ あ`）、快符行（`;f “`）、
//! 功能词（`/jc {加词}`）与正码行统一解析。

use super::{RawTable, TableMeta};
use crate::entry::DictEntry;

pub fn parse(lines: &[String]) -> RawTable {
    let meta = TableMeta::default();
    let mut rows = Vec::new();
    let mut seq = 0u32;
    for line in lines {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let mut it = t.split_whitespace();
        let Some(code) = it.next() else { continue };
        for word in it {
            if word.is_empty() {
                continue;
            }
            rows.push(DictEntry::new(code, word, seq));
            seq += 1;
        }
    }
    RawTable { meta, rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_table() {
        let lines: Vec<String> = vec![
            "/a ǎ á ǎ à".into(),
            "/chu ǔ ú".into(),
            ";f “".into(),
            "t 我 我们".into(),
            "a 来 那个".into(),
            "aaaa 魑魅魍魉 卍 卐".into(),
            "/jc {加词}".into(),
        ];
        let t = parse(&lines);
        assert_eq!(t.rows.len(), 15);
        assert_eq!(t.rows[7].code, "t");
        assert_eq!(t.rows[7].text, "我");
        assert_eq!(t.rows[8].text, "我们");
        assert_eq!(t.rows[10].text, "那个");
        assert_eq!(t.rows[14].text, "{加词}");
    }
}
