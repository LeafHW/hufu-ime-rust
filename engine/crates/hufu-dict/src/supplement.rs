//! 补充语料：`词条 [权重]`（省略权重默认 1000），供整句引擎加分。

use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct SupplementEntry {
    pub word: String,
    pub weight: f64,
}

#[derive(Debug, Default, Clone)]
pub struct Supplement {
    pub entries: Vec<SupplementEntry>,
}

impl Supplement {
    pub fn parse(lines: &[String]) -> Self {
        let mut entries = Vec::new();
        for line in lines {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let mut it = t.split_whitespace();
            let Some(word) = it.next() else { continue };
            let weight = it
                .next()
                .and_then(|w| w.parse::<f64>().ok())
                .unwrap_or(1000.0);
            entries.push(SupplementEntry {
                word: word.to_string(),
                weight,
            });
        }
        Supplement { entries }
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        Ok(Self::parse(&crate::parse::read_lines(path)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_supplement() {
        let lines: Vec<String> = vec![
            "# 本文件用于提升整句中模型未收录的新词、流行词和个人常用词".into(),
            "恁强".into(),
            "赢麻了\t8000".into(),
        ];
        let s = Supplement::parse(&lines);
        assert_eq!(s.entries.len(), 2);
        assert_eq!(s.entries[0].word, "恁强");
        assert_eq!(s.entries[0].weight, 1000.0);
        assert_eq!(s.entries[1].weight, 8000.0);
    }
}
