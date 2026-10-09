//! 符号系统：快符、常用符号（`/xx` 分类符号）、一简符号、动态变量。
//!
//! 数据来源（虎爪同名文件）：
//! - `快符.txt`：`符号\t;字母`；功能段 `功能\t/rq\t1000`
//! - `常用符号.txt`：`符号\t/缩写`，`#分组` 注释
//! - `一简符号.txt`：`符号\t一简码\t0`

use std::collections::HashMap;

/// 一个符号映射条目。
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolEntry {
    /// 触发编码（`;a`、`/tm`、一简码 `a`）
    pub code: String,
    /// 输出内容（符号本身或动态变量 `{日期}`、功能项 `{加词}`）
    pub text: String,
    /// 权重（决定同码多符号时的顺序，默认 1000）
    pub weight: f64,
}

/// 全部符号表集合。
#[derive(Debug, Default, Clone)]
pub struct SymbolTables {
    /// `;x` 快符
    pub quick: HashMap<String, Vec<SymbolEntry>>,
    /// `/xx` 分类符号
    pub slash: HashMap<String, Vec<SymbolEntry>>,
    /// 一简符号（开启后单字母码直接出符号）
    pub simple: HashMap<String, Vec<SymbolEntry>>,
}

impl SymbolTables {
    /// 解析 `快符.txt`。
    pub fn parse_quick(lines: &[String]) -> HashMap<String, Vec<SymbolEntry>> {
        parse_symbol_lines(lines)
    }

    /// 解析 `常用符号.txt`。
    pub fn parse_slash(lines: &[String]) -> HashMap<String, Vec<SymbolEntry>> {
        parse_symbol_lines(lines)
    }

    /// 解析 `一简符号.txt`（`符号\t一简码\t0`）。
    pub fn parse_simple(lines: &[String]) -> HashMap<String, Vec<SymbolEntry>> {
        parse_symbol_lines(lines)
    }

    /// 合并多个符号文件（`/jm`、`/a` 这类与正码共用命名空间的行也收进来）。
    pub fn merge_code_map(&self) -> HashMap<String, Vec<SymbolEntry>> {
        let mut all: HashMap<String, Vec<SymbolEntry>> = HashMap::new();
        for group in [&self.quick, &self.slash, &self.simple] {
            for (code, entries) in group {
                all.entry(code.clone()).or_default().extend(entries.iter().cloned());
            }
        }
        all
    }
}

/// 通用解析：`符号\t编码[ \t权重]` → 编码 → [条目]。
fn parse_symbol_lines(lines: &[String]) -> HashMap<String, Vec<SymbolEntry>> {
    let mut map: HashMap<String, Vec<SymbolEntry>> = HashMap::new();
    for line in lines {
        let t = line.trim();
        // 【# 可作符号 2026-10-09】用户实锤「# 无法快符输出」：行 `#\t;3`
        // 被 # 注释规则整行吞掉。带 Tab 的 # 行是数据（第一列=符号 #），
        // 无 Tab 的 # 行才是注释。
        if t.is_empty() || (t.starts_with('#') && !t.contains('\t')) {
            continue;
        }
        let parts: Vec<&str> = t.split('\t').map(|p| p.trim()).collect();
        if parts.len() < 2 {
            continue;
        }
        let text = parts[0];
        let code = parts[1];
        if text.is_empty() || code.is_empty() {
            continue;
        }
        let weight = parts
            .get(2)
            .and_then(|w| w.parse::<f64>().ok())
            .unwrap_or(1000.0);
        map.entry(code.to_string()).or_default().push(SymbolEntry {
            code: code.to_string(),
            text: text.to_string(),
            weight,
        });
    }
    for entries in map.values_mut() {
        entries.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_symbols() {
        let lines: Vec<String> = vec![
            "# 快符，可以用分号及斜杠".into(),
            "！\t;a".into(),
            "。\t;b".into(),
            "“\t;f".into(),
        ];
        let m = SymbolTables::parse_quick(&lines);
        assert_eq!(m.get(";a").unwrap()[0].text, "！");
        assert_eq!(m.get(";f").unwrap()[0].text, "“");
    }

    #[test]
    fn slash_symbols_with_weight() {
        let lines: Vec<String> = vec![
            "#分组".into(),
            "™\t/tm".into(),
            "℃\t/dui\t50".into(),
        ];
        let m = SymbolTables::parse_slash(&lines);
        assert_eq!(m.get("/tm").unwrap()[0].text, "™");
        assert_eq!(m.get("/dui").unwrap()[0].weight, 50.0);
    }

    // 【# 可作符号 2026-10-09】`#\t;3` 是数据行不是注释（用户实锤
    // 「# 无法快符输出」）；无 Tab 的 # 行仍是注释。
    #[test]
    fn hash_symbol_data_line() {
        let lines: Vec<String> = vec![
            "# 快符注释行".into(),
            "#\t;3".into(),
            "×\t;4".into(),
        ];
        let m = SymbolTables::parse_quick(&lines);
        assert_eq!(m.get(";3").unwrap()[0].text, "#", "# 快符行不被注释吞掉");
        assert_eq!(m.get(";4").unwrap()[0].text, "×");
        assert!(m.values().flatten().all(|e| e.text != "快符注释行"));
    }
}
