//! 迷你 TOML（平面 key = value 子集）—— 网络隔离期替代 toml crate。
//!
//! 只支持我们配置所需的平面标量：字符串/整数/浮点/布尔。足够 Config 便携持久化。
//! 解析规则对齐 TOML v1.0：`#` 注释、双引号字符串（含 \" \\ \n \t \r \u 转义）、
//! 裸整数/浮点/true/false。写出固定带注释头。

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// 解析平面 TOML 文本 → 键值表（原始标量已转成 Rust 字符串形式）
pub fn parse(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let Some((k, v)) = split_kv(line) else {
            continue;
        };
        let key = k.trim().to_string();
        let val = parse_value(v.trim());
        if !key.is_empty() {
            map.insert(key, val);
        }
    }
    map
}

/// 去掉行尾注释（不在字符串内的 #）
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_str = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_str = !in_str,
            b'\\' if in_str => i += 1, // 跳过转义字符
            b'#' if !in_str => return &line[..i],
            _ => {}
        }
        i += 1;
    }
    line
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut in_str = false;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => in_str = !in_str,
            b'=' if !in_str => return Some((&line[..i], &line[i + 1..])),
            _ => {}
        }
    }
    None
}

/// 解析单个值：字符串去引号解转义；其余原样（数字/布尔字符串）
fn parse_value(v: &str) -> String {
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        return unescape(&v[1..v.len() - 1]);
    }
    v.to_string()
}

/// 字符串转义解码（TOML 子集：\n \t \r \" \\ \uXXXX）
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('u') => {
                let hex: String = (0..4).filter_map(|_| chars.next()).collect();
                if let Ok(cp) = u32::from_str_radix(&hex, 16) {
                    if let Some(ch) = char::from_u32(cp) {
                        out.push(ch);
                    }
                }
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// 字符串转义编码
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// 值序列化：字符串加引号转义；其余裸写
pub fn render_value(v: &str) -> String {
    let needs_quote =
        v.parse::<i64>().is_err() && v.parse::<f64>().is_err() && v != "true" && v != "false";
    if needs_quote {
        format!("\"{}\"", escape(v))
    } else {
        v.to_string()
    }
}

/// 序列化键值表 → TOML 文本
pub fn to_string(map: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    for (k, v) in map {
        let _ = writeln!(out, "{} = {}", k.replace('=', "_"), render_value(v));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_types() {
        let m = parse(
            "# 注释行\nwin_x = 200\nfont_size = 30.5\nvisible = true\nname = \"疾风2\"\nempty = \"\"\nurl = \"https://a.b/\"",
        );
        assert_eq!(m["win_x"], "200");
        assert_eq!(m["font_size"], "30.5");
        assert_eq!(m["visible"], "true");
        assert_eq!(m["name"], "疾风2");
        assert_eq!(m["empty"], "");
        assert_eq!(m["url"], "https://a.b/");
    }

    #[test]
    fn comments_and_escapes() {
        let m = parse("a = \"x#y\" # 行尾注释\nb = 1 # = 2\nsig = \"a\\\"b\\\\c\\n\"");
        assert_eq!(m["a"], "x#y");
        assert_eq!(m["b"], "1");
        assert_eq!(m["sig"], "a\"b\\c\n");
    }

    #[test]
    fn roundtrip() {
        let mut m = BTreeMap::new();
        m.insert("win_x".into(), "200".into());
        m.insert("theme".into(), "暗夜·松烟".into());
        m.insert("material".into(), "Acrylic".into());
        let text = to_string(&m);
        let back = parse(&text);
        assert_eq!(back, m);
    }

    #[test]
    fn escape_unescape_inverse() {
        for s in ["普通", "a\"b\\c\nd\te", "中文\n换行", "\u{1}", ""] {
            assert_eq!(unescape(&escape(s)), s, "s={s:?}");
        }
    }

    #[test]
    fn chinese_and_unicode_escape() {
        let m = parse("k = \"\\u75be\\u98ce\"");
        assert_eq!(m["k"], "疾风"); // 75be=疾 98ce=风
    }

    #[test]
    fn malformed_lines_skipped() {
        let m = parse("no_equal_sign\n=empty_key\nk = v\n");
        assert_eq!(m.len(), 1);
        assert_eq!(m["k"], "v");
    }
}
