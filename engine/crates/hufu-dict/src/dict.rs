//! 字典：精确检索 + 前缀树（补全 / 整句词法）+ 词→码索引（反查注释 / 造词）。

use crate::entry::{rank_cmp, DictEntry};
use std::collections::HashMap;

/// 前缀树节点（arena 数组布局）。
#[derive(Default)]
struct TrieNode {
    children: HashMap<char, u32>,
    /// 以该节点为终点的编码对应的条目下标
    entries: Vec<u32>,
}

/// 前缀树。
#[derive(Default)]
pub struct Trie {
    nodes: Vec<TrieNode>,
}

impl Trie {
    pub fn new() -> Self {
        Trie {
            nodes: vec![TrieNode::default()],
        }
    }

    pub fn insert(&mut self, code: &str, entry_idx: u32) {
        let mut cur = 0usize;
        for ch in code.chars() {
            let next = self.nodes[cur].children.get(&ch).copied();
            let next = match next {
                Some(n) => n as usize,
                None => {
                    let id = self.nodes.len() as u32;
                    self.nodes.push(TrieNode::default());
                    self.nodes[cur].children.insert(ch, id);
                    id as usize
                }
            };
            cur = next;
        }
        self.nodes[cur].entries.push(entry_idx);
    }

    /// 精确命中该编码的条目下标。
    pub fn exact(&self, code: &str) -> &[u32] {
        let mut cur = 0usize;
        for ch in code.chars() {
            match self.nodes[cur].children.get(&ch).copied() {
                Some(n) => cur = n as usize,
                None => return &[],
            }
        }
        &self.nodes[cur].entries
    }

    /// 是否存在以 `prefix` 开头的编码（含 prefix 本身为完整码）。
    /// 空串视为真。整句空码自动顶屏用它判断「余码是否还是正常码」。
    pub fn has_prefix(&self, prefix: &str) -> bool {
        let mut cur = 0usize;
        for ch in prefix.chars() {
            match self.nodes[cur].children.get(&ch).copied() {
                Some(n) => cur = n as usize,
                None => return false,
            }
        }
        true
    }

    /// 收集以 `prefix` 为前缀的所有编码（`(编码, 条目下标)`，编码短者优先）。
    /// `limit` 限制返回条数，防止 `a` 这类短前缀爆炸。
    pub fn completions(&self, prefix: &str, limit: usize) -> Vec<(String, u32)> {
        let mut cur = 0usize;
        for ch in prefix.chars() {
            match self.nodes[cur].children.get(&ch).copied() {
                Some(n) => cur = n as usize,
                None => return Vec::new(),
            }
        }
        let mut out = Vec::new();
        let mut stack = vec![(cur, prefix.to_string())];
        while let Some((node, code)) = stack.pop() {
            let node_ref = &self.nodes[node];
            for &idx in &node_ref.entries {
                if out.len() >= limit {
                    return out;
                }
                out.push((code.clone(), idx));
            }
            for (&ch, &child) in &node_ref.children {
                if out.len() >= limit {
                    return out;
                }
                stack.push((child as usize, format!("{code}{ch}")));
            }
        }
        out.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then(a.0.cmp(&b.0)));
        out
    }

    /// 从 `raw` 开头做最长匹配枚举：返回所有「raw 的前缀编码 → 条目」，
    /// 编码长度降序（长码优先，供整句解码枚举切分）。
    pub fn prefix_matches(&self, raw: &str) -> Vec<(usize, Vec<u32>)> {
        let mut result: Vec<(usize, Vec<u32>)> = Vec::new();
        let mut cur = 0usize;
        let mut len = 0usize;
        let mut chars = raw.chars();
        loop {
            if !self.nodes[cur].entries.is_empty() {
                result.push((len, self.nodes[cur].entries.clone()));
            }
            match chars.next() {
                Some(ch) => {
                    match self.nodes[cur].children.get(&ch).copied() {
                        Some(n) => {
                            cur = n as usize;
                            len += ch.len_utf8();
                        }
                        None => break,
                    }
                }
                None => break,
            }
        }
        result.sort_by(|a, b| b.0.cmp(&a.0));
        result
    }
}

/// 一部已加载的字典。
#[derive(Default)]
pub struct Dict {
    pub name: String,
    pub entries: Vec<DictEntry>,
    /// 精确编码 → 条目下标（按 rank 排序）
    by_code: HashMap<String, Vec<u32>>,
    trie: Trie,
    /// 词 → 编码列表（反查注释 / 造词）
    pub text_to_codes: HashMap<String, Vec<String>>,
}

impl Dict {
    pub fn new(name: impl Into<String>) -> Self {
        Dict {
            name: name.into(),
            ..Default::default()
        }
    }

    /// 从解析产物构建（自动按 权重↓、原序↑ 排序并建索引）。
    pub fn from_entries(name: impl Into<String>, entries: Vec<DictEntry>) -> Self {
        let mut dict = Dict::new(name);
        dict.load_entries(entries);
        dict
    }

    pub fn load_entries(&mut self, entries: Vec<DictEntry>) {
        self.entries = entries;
        self.rebuild();
    }

    /// 追加合并（import_tables：主表在前，导入表按声明顺序追加）。
    pub fn merge(&mut self, other: &Dict) {
        let base = self.entries.len() as u32;
        for e in &other.entries {
            let mut e = e.clone();
            e.seq += base;
            self.entries.push(e);
        }
        self.rebuild();
    }

    fn rebuild(&mut self) {
        self.by_code.clear();
        self.trie = Trie::new();
        self.text_to_codes.clear();
        let mut order: Vec<u32> = (0..self.entries.len() as u32).collect();
        order.sort_by(|&i, &j| crate::entry::rank_cmp(&self.entries[i as usize], &self.entries[j as usize]));
        for idx in order {
            let (code, text) = {
                let e = &self.entries[idx as usize];
                (e.code.clone(), e.text.clone())
            };
            self.by_code.entry(code.clone()).or_default().push(idx);
            self.trie.insert(&code, idx);
            let codes = self.text_to_codes.entry(text).or_default();
            if !codes.contains(&code) {
                codes.push(code);
            }
        }
    }

    /// 精确查询某编码的候选（已排序）。
    /// 是否存在以 `code` 开头的编码（含 code 为完整码；空串恒真）。
    /// 整句空码自动顶屏用它判断「余码是否仍是正常码」。
    pub fn trie_has_prefix(&self, code: &str) -> bool {
        self.trie.has_prefix(code)
    }

    pub fn lookup(&self, code: &str) -> Vec<&DictEntry> {
        self.by_code
            .get(code)
            .map(|v| v.iter().map(|&i| &self.entries[i as usize]).collect())
            .unwrap_or_default()
    }

    /// 前缀补全。
    pub fn completions(&self, prefix: &str, limit: usize) -> Vec<&DictEntry> {
        self.trie
            .completions(prefix, limit)
            .into_iter()
            .filter_map(|(_, idx)| self.entries.get(idx as usize))
            .collect()
    }

    /// 整句解码用：raw 前缀的编码匹配（长码优先）。
    pub fn prefix_matches(&self, raw: &str) -> Vec<(usize, Vec<u32>)> {
        self.trie.prefix_matches(raw)
    }

    /// 词 → 码（取最优码，即权重最高）。
    pub fn best_code_of(&self, text: &str) -> Option<&str> {
        self.text_to_codes.get(text).and_then(|codes| {
            codes
                .iter()
                .filter_map(|c| self.by_code.get(c).map(|v| (c, v)))
                .max_by(|(c1, v1), (c2, v2)| {
                    let e1 = &self.entries[*v1.first().unwrap_or(&0) as usize];
                    let e2 = &self.entries[*v2.first().unwrap_or(&0) as usize];
                    rank_cmp(e1, e2).then_with(|| c1.len().cmp(&c2.len()))
                })
                .map(|(c, _)| c.as_str())
        })
    }

    pub fn all_codes_of(&self, text: &str) -> &[String] {
        self.text_to_codes.get(text).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(code: &str, text: &str, weight: f64, seq: u32) -> DictEntry {
        let mut e = DictEntry::new(code, text, seq);
        e.weight = weight;
        e
    }

    #[test]
    fn lookup_order_and_merge() {
        let entries = vec![
            mk("a", "来", 99.0, 0),
            mk("a", "叉", 10.0, 1),
            mk("h", "道", 50.0, 2),
        ];
        let mut d = Dict::from_entries("t", entries);
        assert_eq!(d.lookup("a").iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), ["来", "叉"]);

        let d2 = Dict::from_entries(
            "import",
            vec![mk("a", "次要", 1.0, 0), mk("jd", "什么", 100.0, 1)],
        );
        d.merge(&d2);
        // 同码合并后：主表权重高的仍在前
        assert_eq!(
            d.lookup("a").iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
            ["来", "叉", "次要"]
        );
        assert_eq!(d.lookup("jd")[0].text, "什么");
        assert_eq!(d.best_code_of("什么"), Some("jd"));
    }

    #[test]
    fn completions_and_prefix() {
        let entries = vec![
            mk("t", "我", 5.0, 0),
            mk("tu", "们", 5.0, 1),
            mk("tuja", "我们", 5.0, 2),
        ];
        let d = Dict::from_entries("t", entries);
        let comp = d.completions("t", 10);
        assert!(comp.iter().any(|e| e.text == "们"));
        let pm = d.prefix_matches("tujax");
        // 长码优先：tuja 在 tu 之前
        let lens: Vec<usize> = pm.iter().map(|(l, _)| *l).collect();
        assert_eq!(lens, vec![4, 2, 1]);
    }

    #[test]
    fn trie_has_prefix() {
        let entries = vec![
            mk("t", "我", 5.0, 0),
            mk("tu", "们", 5.0, 1),
            mk("tuja", "我们", 5.0, 2),
        ];
        let d = Dict::from_entries("t", entries);
        // 真前缀（后续还有更长码）
        assert!(d.trie_has_prefix("t"));
        assert!(d.trie_has_prefix("tuj"));
        // 完整码本身也算
        assert!(d.trie_has_prefix("tuja"));
        // 无延续
        assert!(!d.trie_has_prefix("tujb"));
        assert!(!d.trie_has_prefix("x"));
        // 空串恒真
        assert!(d.trie_has_prefix(""));
    }
}
