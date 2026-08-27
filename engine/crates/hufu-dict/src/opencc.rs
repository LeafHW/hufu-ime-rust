//! OpenCC 文本转换表（txt 对照：`源\t目标1 目标2…`）。
//!
//! 词组表优先、单字表兜底的贪心最长匹配（Rime simplifier 语义）。
//! 多目标取首个；无命中原样返回。

use std::collections::HashMap;
use std::path::Path;

#[derive(Default)]
pub struct OpenCc {
    /// 词组（长词优先匹配）
    phrases: HashMap<String, String>,
    /// 单字
    chars: HashMap<String, String>,
}

impl OpenCc {
    /// 加载多个表文件。词组行（>1 字）进 phrases，单字进 chars。
    /// 多目标取首个空格分隔词。
    pub fn load(files: &[std::path::PathBuf]) -> OpenCc {
        OpenCc::load_impl(files, false)
    }

    /// emoji 表加载：目标保留整串（`词\t词+emoji`，含空格）。
    pub fn load_full(files: &[std::path::PathBuf]) -> OpenCc {
        OpenCc::load_impl(files, true)
    }

    fn load_impl(files: &[std::path::PathBuf], full_dst: bool) -> OpenCc {
        let mut t = OpenCc::default();
        for f in files {
            if let Ok(content) = std::fs::read_to_string(f) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    let mut it = line.split('\t');
                    let (Some(src), Some(dsts)) = (it.next(), it.next()) else {
                        continue;
                    };
                    let first = if full_dst {
                        dsts.trim()
                    } else {
                        dsts.split_whitespace().next().unwrap_or("")
                    };
                    if first.is_empty() {
                        continue;
                    }
                    if src.chars().count() > 1 {
                        t.phrases.insert(src.to_string(), first.to_string());
                    } else {
                        t.chars.insert(src.to_string(), first.to_string());
                    }
                }
            }
        }
        t
    }

    /// 是否一个表都没加载成功。
    pub fn is_empty(&self) -> bool {
        self.phrases.is_empty() && self.chars.is_empty()
    }

    /// 贪心最长匹配转换（词组表优先，其次单字表，无命中原样）。
    pub fn convert(&self, text: &str) -> String {
        if self.is_empty() {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0usize;
        while i < chars.len() {
            let mut matched = false;
            // 最长词组尝试（上限 16 字）
            let max = (i + 16).min(chars.len());
            for end in (i + 2..=max).rev() {
                let seg: String = chars[i..end].iter().collect();
                if let Some(v) = self.phrases.get(&seg) {
                    out.push_str(v);
                    i = end;
                    matched = true;
                    break;
                }
            }
            if matched {
                continue;
            }
            let one: String = chars[i..i + 1].iter().collect();
            match self.chars.get(&one) {
                Some(v) => out.push_str(v),
                None => out.push_str(&one),
            }
            i += 1;
        }
        out
    }

    /// 表目录存在则按用途加载。
    pub fn load_dir(dir: &Path, tables: &[&str]) -> OpenCc {
        let files: Vec<std::path::PathBuf> = tables
            .iter()
            .map(|name| dir.join(format!("{name}.txt")))
            .collect();
        OpenCc::load(&files)
    }

    /// emoji 目录加载（整目标模式）。
    pub fn load_dir_full(dir: &Path, tables: &[&str]) -> OpenCc {
        let files: Vec<std::path::PathBuf> = tables
            .iter()
            .map(|name| dir.join(format!("{name}.txt")))
            .collect();
        OpenCc::load_full(&files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn emoji_full_destination() {
        let tmp = std::env::temp_dir().join(format!("hufu-opencc-em-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        write(&tmp, "emoji.txt", "后\t后 👑\n日新月异\t日新月异 🌞❤🌙👆\n");
        // 普通模式：只取首个词 → 丢 emoji
        let t1 = OpenCc::load(&[tmp.join("emoji.txt")]);
        assert_eq!(t1.convert("后"), "后");
        // 整目标模式：保留 词+emoji
        let t2 = OpenCc::load_full(&[tmp.join("emoji.txt")]);
        assert_eq!(t2.convert("后"), "后 👑");
        assert_eq!(t2.convert("日新月异"), "日新月异 🌞❤🌙👆");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn phrase_first_conversion() {
        let tmp = std::env::temp_dir().join(format!("hufu-opencc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        write(
            &tmp,
            "ph.txt",
            "一丝不挂\t一絲不掛\n皇后\t皇后\n",
        );
        write(&tmp, "ch.txt", "后\t後\n丝\t絲\n挂\t掛\n一\t一\n不\t不\n心\t心\n愿\t願\n");
        let t = OpenCc::load(&[tmp.join("ph.txt"), tmp.join("ch.txt")]);
        assert_eq!(t.convert("一丝不挂"), "一絲不掛", "词组优先");
        assert_eq!(t.convert("后来"), "後來".replace('來', "来"), "单字转换：后→後，来无映射保持");
        assert_eq!(t.convert("皇后"), "皇后", "皇后词组不转后");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
