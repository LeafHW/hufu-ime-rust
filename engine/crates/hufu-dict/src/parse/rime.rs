//! Rime `*.dict.yaml` 解析。
//!
//! 头部（YAML 前置段）支持：name/version/sort/columns/import_tables/encoder/rules/
//! use_preset_vocabulary；表体为 Tab 分列，列序由 columns 决定（默认 text,code,weight）。

use super::{parse_weight, EncoderRule, RawTable, TableMeta};
use crate::entry::DictEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Col {
    Text,
    Code,
    Weight,
    Stem,
}

fn col_of(s: &str) -> Option<Col> {
    match s.trim() {
        "text" => Some(Col::Text),
        "code" => Some(Col::Code),
        "weight" => Some(Col::Weight),
        "stem" => Some(Col::Stem),
        _ => None,
    }
}

/// 解析内联列表 `[a, b, c]` 或 `a,b`。
fn parse_inline_list(s: &str) -> Vec<String> {
    let s = s.trim();
    let s = s.strip_prefix('[').unwrap_or(s);
    let s = s.strip_suffix(']').unwrap_or(s);
    s.split(',')
        .map(|x| x.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
        .unwrap_or(s)
}

/// 极简 YAML 头解析（只为 dict.yaml 头部形状服务，非通用 YAML）。
struct Header {
    meta: TableMeta,
    columns: Vec<Col>,
}

fn parse_header(lines: &[String]) -> Header {
    let mut meta = TableMeta::default();
    let mut columns: Vec<Col> = Vec::new();
    let mut i = 0;
    let mut in_columns = false;
    let mut in_imports = false;
    let mut in_encoder_rules = false;
    let mut cur_rule: Option<(usize, usize, String)> = None;

    let flush_rule = |cur: &mut Option<(usize, usize, String)>, rules: &mut Vec<EncoderRule>| {
        if let Some((min, max, formula)) = cur.take() {
            rules.push(EncoderRule { min_len: min, max_len: max, formula });
        }
    };

    while i < lines.len() {
        let line = lines[i].trim_end().to_string();
        let t = line.trim();
        if t == "..." || t == "---" {
            i += 1;
            if t == "..." {
                break;
            }
            continue;
        }
        // 表体行（含 Tab 且不是 key: 形状）→ 头部结束
        if t.contains('\t') && !t.contains(": ") && !t.ends_with(':') {
            break;
        }
        if let Some(rest) = t.strip_prefix("- ") {
            // 列表项
            if let Some(v) = rest.strip_prefix("length_equal:") {
                let n: usize = v.trim().parse().unwrap_or(0);
                flush_rule(&mut cur_rule, &mut meta.encoder_rules);
                cur_rule = Some((n, n, String::new()));
            } else if let Some(v) = rest.strip_prefix("length_in_range:") {
                let list = parse_inline_list(v);
                let min: usize = list.first().and_then(|x| x.parse().ok()).unwrap_or(0);
                let max: usize = list.get(1).and_then(|x| x.parse().ok()).unwrap_or(min);
                flush_rule(&mut cur_rule, &mut meta.encoder_rules);
                cur_rule = Some((min, max, String::new()));
            } else if let Some(v) = rest.strip_prefix("formula:") {
                if let Some(r) = cur_rule.as_mut() {
                    r.2 = unquote(v).to_string();
                }
            } else if in_columns {
                if let Some(c) = col_of(rest) {
                    columns.push(c);
                }
            } else if in_imports {
                meta.imports.push(unquote(rest).to_string());
            }
        } else if let Some((key, value)) = t.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            in_columns = false;
            in_imports = false;
            in_encoder_rules = false;
            match key {
                "name" => meta.name = unquote(value).to_string(),
                "version" => meta.version = unquote(value).to_string(),
                "sort" => meta.sort = Some(unquote(value).to_string()),
                "use_preset_vocabulary" => meta.use_preset_vocabulary = value == "true",
                "formula" => {
                    if let Some(r) = cur_rule.as_mut() {
                        r.2 = unquote(value).to_string();
                    }
                }
                "columns" => {
                    columns = parse_inline_list(value).iter().filter_map(|c| col_of(c)).collect();
                    if columns.is_empty() {
                        in_columns = true;
                    }
                }
                "import_tables" => {
                    meta.imports = parse_inline_list(value);
                    if meta.imports.is_empty() {
                        in_imports = true;
                    }
                }
                "rules" => {
                    in_encoder_rules = true;
                }
                _ => {}
            }
        }
        i += 1;
    }
    flush_rule(&mut cur_rule, &mut meta.encoder_rules);
    if columns.is_empty() {
        columns = vec![Col::Text, Col::Code, Col::Weight];
    }
    Header { meta, columns }
}

pub fn parse(lines: &[String]) -> RawTable {
    let header = parse_header(lines);
    let meta = header.meta;
    let columns = header.columns;

    let mut rows = Vec::new();
    let mut seq = 0u32;
    let mut in_body = false;
    for line in lines {
        let t = line.trim_end();
        if !in_body {
            if t.trim() == "..." {
                in_body = true;
                continue;
            }
            // 有些表省略 `...`：遇到首条含 Tab 的数据行即进入表体
            if t.contains('\t') && !t.trim().contains(": ") && !t.trim().ends_with(':') && !t.trim_start().starts_with('#')
            {
                in_body = true;
                // fallthrough 处理该行
            } else {
                continue;
            }
        }
        let t = t.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = t.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let mut text = String::new();
        let mut code = String::new();
        let mut weight = 0.0f64;
        let mut stem = None;
        for (i, col) in columns.iter().enumerate() {
            let v = parts.get(i).copied().unwrap_or("").trim();
            match col {
                Col::Text => text = v.to_string(),
                Col::Code => code = v.to_string(),
                Col::Weight => weight = parse_weight(v),
                Col::Stem => stem = if v.is_empty() { None } else { Some(v.to_string()) },
            }
        }
        if text.is_empty() || code.is_empty() {
            continue;
        }
        let mut e = DictEntry::new(code, text, seq);
        e.weight = weight;
        e.stem = stem;
        rows.push(e);
        seq += 1;
    }

    // sort: by_weight 在构建 Dict 时统一处理；original 保持文件序
    RawTable { meta, rows }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_auto;

    #[test]
    fn tiger_style_3col() {
        let lines: Vec<String> = vec![
            "---".into(),
            "name: personal".into(),
            "version: \"2026.08.13\"".into(),
            "sort: by_weight".into(),
            "...".into(),
            "的\tu\t10359470".into(),
            "道\th\t10000000".into(),
            "什么\tj\t9950000".into(),
        ];
        let t = parse(&lines);
        assert_eq!(t.meta.name, "personal");
        assert_eq!(t.meta.sort.as_deref(), Some("by_weight"));
        assert_eq!(t.rows.len(), 3);
        assert_eq!(t.rows[0].text, "的");
        assert_eq!(t.rows[0].code, "u");
        assert_eq!(t.rows[0].weight, 10359470.0);
    }

    #[test]
    fn tigress_style_4col_with_encoder_and_imports() {
        let lines: Vec<String> = vec![
            "---".into(),
            "name: tigress".into(),
            "columns:".into(),
            "  - text".into(),
            "  - weight".into(),
            "  - code".into(),
            "  - stem".into(),
            "import_tables:".into(),
            "  - tigress_ci".into(),
            "  - tigress_simp_ci".into(),
            "encoder:".into(),
            "  rules:".into(),
            "    - length_equal: 2".into(),
            "      formula: \"AaAbBaBb\"".into(),
            "    - length_in_range: [4, 10]".into(),
            "      formula: \"AaBaCaZa\"".into(),
            "...".into(),
            "的\t10359470\tu\tun".into(),
            "的\t256\tuni\t".into(),
        ];
        let t = parse(&lines);
        assert_eq!(t.meta.imports, vec!["tigress_ci", "tigress_simp_ci"]);
        assert_eq!(t.meta.encoder_rules.len(), 2);
        assert_eq!(t.meta.encoder_rules[0].formula, "AaAbBaBb");
        assert_eq!(t.meta.encoder_rules[1].min_len, 4);
        assert_eq!(t.meta.encoder_rules[1].max_len, 10);
        assert_eq!(t.rows[0].text, "的");
        assert_eq!(t.rows[0].code, "u");
        assert_eq!(t.rows[0].weight, 10359470.0);
        assert_eq!(t.rows[0].stem.as_deref(), Some("un"));
        assert_eq!(t.rows[1].code, "uni");
        assert_eq!(t.rows[1].stem, None);
    }

    #[test]
    fn sentence_2col_no_weight() {
        // tiger_sentence.dict.yaml：text+code 两列，sort: original
        let lines: Vec<String> = vec![
            "---".into(),
            "name: tiger_sentence".into(),
            "sort: original".into(),
            "...".into(),
            "来\ta".into(),
            "那个\ta;".into(),
            "卍\taaaa;".into(),
        ];
        let t = parse_auto(&lines);
        assert_eq!(t.rows.len(), 3);
        assert_eq!(t.rows[1].text, "那个");
        assert_eq!(t.rows[1].code, "a;");
        assert_eq!(t.rows[1].weight, 0.0);
    }
}
