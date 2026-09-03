//! 多多 / QQ五笔格式：`词\t码`（词在前），行序即优先级。
//!
//! - 头部：`---config@属性=值`（忽略内容，仅识别为多多标志）
//! - 词尾 `#固` → 固定置顶
//! - 词内 `显示=>输出` → 显示与上屏分离

use super::{RawTable, TableMeta};
use crate::entry::DictEntry;

pub fn parse(lines: &[String]) -> RawTable {
    let meta = TableMeta::default();
    let mut rows = Vec::new();
    let mut seq = 0u32;
    for line in lines {
        let t = line.trim_end();
        let t = t.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t.starts_with("---config@") {
            continue;
        }
        // 【词前格式空格行 2026-09-05】部分导出（如「的  u3」）用空格
        // 分隔词与码。无 TAB 时按空白分：首段=词、次段=码；多余段忽略
        // （兼容权重列等）。空段跳过同 TAB 路径。
        let (word_raw, code) = match t.split_once('\t') {
            Some((w, c)) => (w.trim().to_string(), c.trim().to_string()),
            None => {
                let mut it = t.split_whitespace();
                let w = it.next().unwrap_or("").to_string();
                let c = it.next().unwrap_or("").to_string();
                (w, c)
            }
        };
        let mut word = word_raw;
        if word.is_empty() || code.is_empty() {
            continue;
        }
        let mut pinned = false;
        if let Some(stripped) = word.strip_suffix("#固") {
            word = stripped.to_string();
            pinned = true;
        }
        let (text, commit_override) = match word.split_once("=>") {
            Some((disp, out)) => (disp.to_string(), Some(out.to_string())),
            None => (word, None),
        };
        let mut e = DictEntry::new(code, text, seq);
        e.pinned = pinned;
        e.commit_override = commit_override;
        rows.push(e);
        seq += 1;
    }
    RawTable { meta, rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duoduo_variants() {
        let lines: Vec<String> = vec![
            "---config@码表分类=主码-系统码表|主码-用户码表|主码-次显码表".into(),
            "---config@码表别名=常用字词|用户码表|生僻字".into(),
            "的\tu".into(),
            "他\tje".into(),
            "fjj#固\tfjj".into(),
            "电话号码=>10086\tdhhm".into(),
            "恭恭敬敬\taaaa".into(),
        ];
        let t = parse(&lines);
        assert_eq!(t.rows.len(), 5);
        assert_eq!(t.rows[0].text, "的");
        assert_eq!(t.rows[0].code, "u");
        assert_eq!(t.rows[2].text, "fjj");
        assert!(t.rows[2].pinned);
        assert_eq!(t.rows[3].text, "电话号码");
        assert_eq!(t.rows[3].commit_override.as_deref(), Some("10086"));
        assert_eq!(t.rows[4].text, "恭恭敬敬");
    }

    /// 【词前空格式 2026-09-05】「的  u3」（词+双空格+码）无 TAB 行——
    /// 数字编码体系的码表导出常见（a8=来、u3=的 的全码行）。必须与
    /// TAB 行混排解析，且整文件纯空格分隔时嗅探为词前格式而非虎
    /// 整句「码 词」格式。
    #[test]
    fn word_first_space_lines() {
        // TAB 与空格行混排
        let lines: Vec<String> = vec![
            "的\tu".into(),
            "的  u3".into(),
            "来\ta".into(),
            "来  a8".into(),
            "比\tvv".into(),
            "如  b8".into(),
        ];
        let t = parse(&lines);
        assert_eq!(t.rows.len(), 6);
        assert!(t.rows.iter().any(|e| e.text == "的" && e.code == "u3"));
        assert!(t.rows.iter().any(|e| e.text == "来" && e.code == "a8"));
        assert!(t.rows.iter().any(|e| e.text == "如" && e.code == "b8"));

        // 整文件纯空格分隔：嗅探必须是词前格式（首列汉字），不是虎格式
        let pure: Vec<String> = vec!["的 u".into(), "来 a".into(), "了 r".into(), "了 r8".into()];
        assert_eq!(
            crate::parse::sniff_format(&pure),
            crate::parse::TableFormat::WordFirstTsv
        );
        let t2 = parse(&pure);
        assert_eq!(t2.rows.len(), 4);
        assert_eq!(t2.rows[0].code, "u");
        assert_eq!(t2.rows[3].code, "r8");

        // 虎整句格式（首列拉丁）不受影响
        let tiger: Vec<String> = vec!["jd 斗 鬥".into(), "aaaa 卍 卐".into()];
        assert_eq!(
            crate::parse::sniff_format(&tiger),
            crate::parse::TableFormat::SpaceCodeWords
        );

        // 数字编码检测：含数字码词条 → digit_coded
        let dict = crate::Dict::from_entries("t".to_string(), t.rows.clone());
        assert!(dict.digit_coded);
        // 虎码类（无数字码）→ 不置位
        let plain = crate::Dict::from_entries(
            "t".to_string(),
            vec![
                crate::entry::DictEntry::new("u".to_string(), "的".to_string(), 0),
                crate::entry::DictEntry::new("jd".to_string(), "斗".to_string(), 1),
            ],
        );
        assert!(!plain.digit_coded);
    }
}
