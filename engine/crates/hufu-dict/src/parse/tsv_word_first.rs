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
        let parts: Vec<&str> = t.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let mut word = parts[0].trim().to_string();
        let code = parts[1].trim().to_string();
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
}
