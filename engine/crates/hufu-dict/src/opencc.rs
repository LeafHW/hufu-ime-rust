//! OpenCC 文本转换表（txt 对照：`源\t目标1 目标2…`）。
//!
//! 词组表优先、单字表兜底的贪心最长匹配（Rime simplifier 语义）。
//! convert 多目标取首个；无命中原样返回。单字全变体另见
//! [`OpenCc::char_variants`]（一简多繁候选展开用）。

use std::collections::HashMap;
use std::path::Path;

#[derive(Default)]
pub struct OpenCc {
    /// 词组（长词优先匹配）
    phrases: HashMap<String, String>,
    /// 单字
    chars: HashMap<String, String>,
    /// 【一简多繁 2026-10-17 用户需求】单字全变体（表序保留）：简体发
    /// 在繁体有 髮（头发）/發（发财）等多字形，此前加载只留首个、其余
    /// 全丢——单字候选无法全显。词组转换仍用 chars（取首个）+ phrases
    /// 消歧（头发→頭髮），不受此表影响。
    char_variants: HashMap<String, Vec<String>>,
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
                        // 【一简多繁 2026-10-17】非整目标模式（转换表）单字
                        // 行保留全部目标（表序）；emoji 整目标模式不适用
                        //（目标含空格是语义，不拆）。
                        if !full_dst {
                            let vs: Vec<String> = dsts
                                .split_whitespace()
                                .map(|s| s.to_string())
                                .collect();
                            if !vs.is_empty() {
                                t.char_variants.insert(src.to_string(), vs);
                            }
                        }
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

    /// 单字全变体（表序，`发\t發 髮` → [發, 髮]）。无条目 None；
    /// 单目标也返回（长度 1，调用方自行判断要不要展开）。
    pub fn char_variants(&self, ch: &str) -> Option<&[String]> {
        self.char_variants.get(ch).map(|v| v.as_slice())
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

    /// 【一简多繁 2026-10-17】单字全变体保留：`发\t發 髮` 两个都要在
    /// （此前加载即丢、单字候选永远只见首个）。convert 行为不变（首
    /// 个）——词组消歧（头发→頭髮）不受影响。
    #[test]
    fn char_variants_multi_traditional() {
        let tmp = std::env::temp_dir().join(format!("hufu-opencc-mv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        write(&tmp, "ch.txt", "发\t發 髮\n后\t後 后\n头\t頭\n财\t財\n钱\t錢\n");
        write(&tmp, "ph.txt", "头发\t頭髮\n发财\t發財\n");
        let t = OpenCc::load(&[tmp.join("ph.txt"), tmp.join("ch.txt")]);
        // 全变体（表序，含表内原字——「后」在繁体 皇后 亦用）
        assert_eq!(
            t.char_variants("发"),
            Some(&["發".to_string(), "髮".to_string()][..])
        );
        assert_eq!(
            t.char_variants("后"),
            Some(&["後".to_string(), "后".to_string()][..])
        );
        // 单目标：长度 1
        assert_eq!(t.char_variants("钱"), Some(&["錢".to_string()][..]));
        // 无条目
        assert_eq!(t.char_variants("虎"), None);
        // convert 语义不变：首个 + 词组消歧
        assert_eq!(t.convert("发"), "發");
        assert_eq!(t.convert("头发"), "頭髮");
        assert_eq!(t.convert("发财"), "發財");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
