//! 通用 IO 与格式嗅探。

pub mod native;
pub mod rime;
pub mod space;
pub mod tsv_word_first;

use crate::entry::DictEntry;
use encoding_rs::GBK;
use std::path::Path;

/// 解析出的表：元信息 + 行条目（seq 已按行序赋值）。
pub struct RawTable {
    pub meta: TableMeta,
    pub rows: Vec<DictEntry>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableMeta {
    pub name: String,
    pub version: String,
    /// by_weight: 按权重列排序；original: 保持文件原序
    pub sort: Option<String>,
    pub imports: Vec<String>,
    pub encoder_rules: Vec<EncoderRule>,
    pub use_preset_vocabulary: bool,
}

/// Rime encoder 构词规则。
#[derive(Debug, Clone, PartialEq)]
pub struct EncoderRule {
    pub min_len: usize,
    pub max_len: usize,
    /// 形如 `AaAbBaBb`：A/B/C…第 1/2/3 字，Z 末字；小写字母表该字第 n 码
    pub formula: String,
}

/// 检测到的表格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableFormat {
    /// HuFu 原生 TSV：`码\t词\t权重?`
    Native,
    /// Rime dict.yaml
    RimeYaml,
    /// 多多：`---config@` 头 + `词\t码`
    Duoduo,
    /// 无头 `词\t码`（QQ五笔等）
    WordFirstTsv,
    /// 虎整句：`码 词1 词2`
    SpaceCodeWords,
}

/// 读文件为行列表：UTF-8（含 BOM）优先，失败回退 GBK；统一去 CRLF。
pub fn read_lines(path: &Path) -> std::io::Result<Vec<String>> {
    let bytes = std::fs::read(path)?;
    Ok(decode_lines(&bytes))
}

pub fn decode_lines(bytes: &[u8]) -> Vec<String> {
    let text = decode_text(bytes);
    text.lines().map(|l| l.trim_end_matches('\r').to_string()).collect()
}

pub fn decode_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let (cow, _, had_errors) = GBK.decode(bytes);
            if had_errors {
                String::from_utf8_lossy(bytes).into_owned()
            } else {
                cow.into_owned()
            }
        }
    }
}

fn contains_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c,
            '\u{2E80}'..='\u{2FDF}'   // 部首/康熙
            | '\u{3000}'..='\u{303F}' // CJK 标点
            | '\u{31C0}'..='\u{31EF}'
            | '\u{3400}'..='\u{4DBF}' // 扩 A
            | '\u{4E00}'..='\u{9FFF}' // 基本
            | '\u{F900}'..='\u{FAFF}' // 兼容
            | '\u{20000}'..='\u{2FFFF}'
        )
    })
}

/// 嗅探格式：看前若干有效行。
pub fn sniff_format(lines: &[String]) -> TableFormat {
    let mut data_checked = false;
    for line in lines.iter().take(64) {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t.starts_with("---config@") {
            return TableFormat::Duoduo;
        }
        if t == "---" || t.starts_with("...") {
            return TableFormat::RimeYaml;
        }
        if t.starts_with("#hufu-dict") {
            return TableFormat::Native;
        }
        // YAML 键值行（无 Tab）→ Rime 头（部分文件省略 `---` 起始符）
        if !t.contains('\t') && is_yaml_kv(t) {
            return TableFormat::RimeYaml;
        }
        if !data_checked {
            // 无头格式：依据首条数据行判断
            if t.contains('\t') {
                let first = t.split('\t').next().unwrap_or("");
                return if contains_cjk(first) {
                    TableFormat::WordFirstTsv
                } else {
                    TableFormat::Native
                };
            } else if t.split_whitespace().count() >= 2 {
                return TableFormat::SpaceCodeWords;
            }
            data_checked = true;
        }
    }
    TableFormat::Native
}

/// 形如 `name: tiger` / `sort: by_weight` 的 YAML 键值行。
fn is_yaml_kv(t: &str) -> bool {
    match t.split_once(':') {
        Some((k, v)) => {
            !k.is_empty()
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
                && (v.trim().is_empty() || !v.starts_with('\t'))
        }
        None => false,
    }
}

/// 按嗅探结果解析。
pub fn parse_auto(lines: &[String]) -> RawTable {
    match sniff_format(lines) {
        TableFormat::Native => crate::parse::native::parse(lines),
        TableFormat::RimeYaml => crate::parse::rime::parse(lines),
        TableFormat::Duoduo | TableFormat::WordFirstTsv => crate::parse::tsv_word_first::parse(lines),
        TableFormat::SpaceCodeWords => crate::parse::space::parse(lines),
    }
}

/// 从文件直接解析（自动格式）。
pub fn parse_file(path: &Path) -> std::io::Result<RawTable> {
    let lines = read_lines(path)?;
    Ok(parse_auto(&lines))
}

pub fn parse_weight(s: &str) -> f64 {
    s.trim().parse::<f64>().unwrap_or(0.0)
}
