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

/// 行列表写文件（UTF-8 带 BOM，CRLF）：码表导出用。带 BOM 的 UTF-8
/// 是记事本/VSCode/Notepad++ 全兼容形态（GB18030 无 BOM 在 VSCode 等
/// 默认 UTF-8 的编辑器里显示为乱码）；HuFu 自身 read_lines 首查
/// UTF-8 BOM，导出文件可直接再导入。
pub fn write_lines_gb18030(path: &Path, lines: &[String]) -> std::io::Result<()> {
    let mut bytes: Vec<u8> = vec![0xEF, 0xBB, 0xBF];
    let mut text = lines.join("\r\n");
    text.push_str("\r\n");
    bytes.extend_from_slice(text.as_bytes());
    std::fs::write(path, bytes)
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
    // 【无头 Tab 方向投票 2026-10-16】方向（词前 vs 码前）不再由首条
    // 数据行单行裁定：首行可能是纯拉丁词条（0私用.txt 首行
    // `ceui\tyixx`）或 CJK 区外符号（1符号.txt 首行 `℃\tzq`，℃=
    // U+2103 不在 contains_cjk 区间）——单行判反整个文件方向颠倒，
    // 词被当码、码被当词，打不出任何字（用户实锤「0私用 1符号读不
    // 到」）。改两列 CJK 票数投票（首列票多=词前；次列票多或平票=
    // 码前，保持旧默认）。
    let mut first_cjk = 0u32;
    let mut second_cjk = 0u32;
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
            // 无头格式：依据首条数据行判断分隔方式；方向延后投票裁定
            if t.contains('\t') {
                let mut cols = t.split('\t');
                let first = cols.next().unwrap_or("");
                let second = cols.next().unwrap_or("");
                if contains_cjk(first) {
                    first_cjk += 1;
                }
                if contains_cjk(second) {
                    second_cjk += 1;
                }
            } else if t.split_whitespace().count() >= 2 {
                // 【词前格式空格行 2026-09-05】整文件用空格分隔的词前
                // 格式（如「的 u3」「来 a8」）：首列是汉字 → 词在前码
                // 在后（多多类变体，无 ---config@ 头）；首列拉丁 → 虎
                // 整句「码 词1 词2」格式。空格分隔沿用单行判（虎整句
                // 主表巨大，首行汉字即可靠；Tab 才走投票）。
                let first = t.split_whitespace().next().unwrap_or("");
                return if contains_cjk(first) {
                    TableFormat::WordFirstTsv
                } else {
                    TableFormat::SpaceCodeWords
                };
            }
            data_checked = true;
        } else if data_checked && t.contains('\t') {
            // 后续数据行继续投票（前 64 行窗口内）
            let mut cols = t.split('\t');
            let first = cols.next().unwrap_or("");
            let second = cols.next().unwrap_or("");
            if contains_cjk(first) {
                first_cjk += 1;
            }
            if contains_cjk(second) {
                second_cjk += 1;
            }
            if first_cjk + second_cjk >= 8 {
                // 票数足够，提前裁定（大表不必扫满 64 行）
                break;
            }
        }
    }
    if data_checked {
        return if first_cjk > second_cjk {
            TableFormat::WordFirstTsv
        } else {
            TableFormat::Native
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 【无头 Tab 方向投票 2026-10-16】用户实锤：五笔目录「0私用」
    /// 「1符号」打不出——首行判向反了（纯拉丁词 / ℃ 区外符号）。
    #[test]
    fn sniff_vote_direction() {
        // 0私用.txt：首行词是纯拉丁（ceui），次行 U盘 携 CJK——词前
        let l = vec![
            "ceui\tyixx".to_string(),
            "U盘\tute".to_string(),
            "12345678\tjy".to_string(),
            "183006575@qq.com\tqq".to_string(),
        ];
        assert_eq!(sniff_format(&l), TableFormat::WordFirstTsv);
        // 1符号.txt：℃℉°<＝ 均 CJK 区外，「『《 在区间——票数胜出
        let l = vec![
            "℃\tzq".to_string(),
            "℉\tzq".to_string(),
            "°\tzq".to_string(),
            "<\tzw".to_string(),
            "＜\tzw".to_string(),
            "「\tze".to_string(),
            "『\tze".to_string(),
            "《\tzr".to_string(),
        ];
        assert_eq!(sniff_format(&l), TableFormat::WordFirstTsv);
        // 码前（虎整句导出）不受影响：次列 CJK 票多
        let l: Vec<String> = ["a\t工", "b\t式", "c\t以", "d\t动", "e\t对", "f\t地", "g\t一", "h\t上"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(sniff_format(&l), TableFormat::Native);
        // 单行词前（词 CJK）依旧可判
        let l = vec!["的\tu3".to_string()];
        assert_eq!(sniff_format(&l), TableFormat::WordFirstTsv);
        // 单行码前（码拉丁、词 CJK）依旧可判
        let l = vec!["aa\t工作".to_string()];
        assert_eq!(sniff_format(&l), TableFormat::Native);
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
