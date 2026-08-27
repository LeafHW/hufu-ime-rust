//! 注释表：拼音注释、Unicode 分区注释、拆分（部件）提示、反查码表。

use std::collections::HashMap;
use std::path::Path;

use crate::parse::read_lines;

/// 字 → 注释（拼音 / Unicode 分区 / 拆分部件）。
#[derive(Debug, Default, Clone)]
pub struct AnnotationTable {
    map: HashMap<char, String>,
}

impl AnnotationTable {
    pub fn parse(lines: &[String]) -> Self {
        let mut map = HashMap::new();
        for line in lines {
            let t = line.trim_end();
            let t = t.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = t.split('\t').collect();
            if parts.len() < 2 {
                continue;
            }
            let ch = parts[0].trim();
            let note = parts[1].trim();
            if let Some(c) = ch.chars().next() {
                if !note.is_empty() {
                    map.insert(c, note.to_string());
                }
            }
        }
        AnnotationTable { map }
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        Ok(Self::parse(&read_lines(path)?))
    }

    pub fn get(&self, c: char) -> Option<&str> {
        self.map.get(&c).map(|s| s.as_str())
    }

    /// 给整词生成注释（逐字注释拼接，超过 3 字只注首尾）。
    pub fn annotate_word(&self, word: &str, max_chars: usize) -> String {
        let chars: Vec<char> = word.chars().collect();
        if chars.is_empty() {
            return String::new();
        }
        let take: Vec<char> = if chars.len() > max_chars {
            let mut v = vec![chars[0], chars[chars.len() - 1]];
            v.dedup();
            v
        } else {
            chars
        };
        take.iter()
            .filter_map(|c| self.get(*c))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// 反查码表：`词\t码`（如小鹤双拼反查、五笔画）。
/// 既是「码 → 词」的字典，也保留「词 → 码」用于回显主码。
#[derive(Debug, Default)]
pub struct ReverseTable {
    /// 码 → 候选词（按行序）
    by_code: HashMap<String, Vec<String>>,
    /// 词 → 反查码
    pub word_to_code: HashMap<String, String>,
}

impl ReverseTable {
    pub fn parse(lines: &[String]) -> Self {
        let mut t = ReverseTable::default();
        for line in lines {
            let l = line.trim_end();
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = l.split('\t').collect();
            if parts.len() < 2 {
                continue;
            }
            let word = parts[0].trim();
            let code = parts[1].trim();
            if word.is_empty() || code.is_empty() {
                continue;
            }
            t.by_code.entry(code.to_string()).or_default().push(word.to_string());
            t.word_to_code.entry(word.to_string()).or_insert(code.to_string());
        }
        t
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        Ok(Self::parse(&read_lines(path)?))
    }

    /// 反查：由反查码取词列表。
    pub fn lookup(&self, code: &str) -> &[String] {
        self.by_code.get(code).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// 词 → 反查码（候选注释用）。
    pub fn code_of(&self, word: &str) -> Option<&str> {
        self.word_to_code.get(word).map(|s| s.as_str())
    }

    pub fn len(&self) -> usize {
        self.word_to_code.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotation_tables() {
        let pinyin = AnnotationTable::parse(&[
            "的\tde".into(),
            "㐆\tyǐn yī".into(),
        ]);
        assert_eq!(pinyin.get('的'), Some("de"));
        assert_eq!(pinyin.get('㐆'), Some("yǐn yī"));
        assert_eq!(pinyin.annotate_word("的", 3), "de");

        let uni = AnnotationTable::parse(&["的\t基本".into(), "の\t平假名".into()]);
        assert_eq!(uni.get('の'), Some("平假名"));

        let split = AnnotationTable::parse(&["我\t丿扌戈".into(), "们\t亻门".into()]);
        assert_eq!(split.annotate_word("我们", 3), "丿扌戈 亻门");
        // 长词只注首尾
        assert_eq!(split.annotate_word("我们我们", 3), "丿扌戈 亻门");
    }

    #[test]
    fn reverse_table() {
        let t = ReverseTable::parse(&["的\tde".into(), "了\tle".into(), "我们\twm".into()]);
        assert_eq!(t.lookup("de"), &["的".to_string()]);
        assert_eq!(t.code_of("我们"), Some("wm"));
    }
}
