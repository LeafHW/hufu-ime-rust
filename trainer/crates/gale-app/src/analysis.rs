//! Dictionary analysis shared by the live panel and native layout fixtures.
use std::collections::BTreeMap;

/// A word earns the bracket and tint when it spans at least two characters,
/// matching the original trainer: punctuation glued into a routed word (such
/// as 「怎么办？」) still gets its underline and colour class. Single
/// characters never do. 【含标点词显示对齐 2026-11-12】
pub fn is_multi_word(text: &str) -> bool {
    text.chars().count() >= 2
}

/// The six prompt colors describe word length and base-code length, without
/// selection keys or commit tails. Both the article and F10 use this classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordColorClass {
    Short = 0,
    Two3 = 1,
    Two4 = 2,
    Three = 3,
    Long = 4,
    One = 5,
}
pub fn word_color_class(text: &str, code: &str) -> Option<WordColorClass> {
    if code.is_empty() || !is_multi_word(text) {
        return None;
    }
    let chars = text.chars().count();
    let keys = code.chars().count();
    Some(if keys == 1 {
        WordColorClass::One
    } else if keys <= 2 || (chars >= 3 && keys == 3) {
        WordColorClass::Short
    } else if chars == 2 && keys == 3 {
        WordColorClass::Two3
    } else if chars == 2 {
        WordColorClass::Two4
    } else if chars == 3 {
        WordColorClass::Three
    } else {
        WordColorClass::Long
    })
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct FullAnalysis {
    #[serde(default)]
    pub entries: Vec<FullAnalysisEntry>,
    #[serde(default)]
    pub total_code_len: u64,
    #[serde(default)]
    pub avg_code_len_per_char: f64,
    #[serde(default)]
    pub select_count: u64,
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub schema_ver: String,
    #[serde(default = "default_max")]
    pub max_code_length: usize,
}
fn default_max() -> usize {
    4
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct FullAnalysisEntry {
    #[serde(default)]
    pub word: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub code_len: u64,
    #[serde(default)]
    pub rank: u64,
    #[serde(default)]
    pub tail: String,
}

/// 【下一个字编码 2026-11-12】单字码：该字独立成词时的显示路由配方。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CharCode {
    pub code: String,
    pub rank: usize,
    pub tail: String,
}

/// 解析 single=true 的 analyze 响应为与正文逐字对齐的单字码表；
/// 词库无码的字为 None。响应必须恰好逐字覆盖（多字词条目报错）。
pub fn response_char_codes(
    text: &str,
    value: &serde_json::Value,
) -> Result<Vec<Option<CharCode>>, String> {
    let entries = response_entries(text, value)?;
    let mut chars = text.chars();
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(ch) = chars.next() else {
            return Err("单字码表与正文长度不一致".into());
        };
        if entry.word.chars().count() != 1 || entry.word.chars().next() != Some(ch) {
            return Err("单字码响应包含多字词条目".into());
        }
        out.push(
            entry
                .code
                .clone()
                .filter(|c| !c.is_empty())
                .map(|code| CharCode {
                    code,
                    rank: entry.rank as usize,
                    tail: entry.tail.clone(),
                }),
        );
    }
    if chars.next().is_some() {
        return Err("单字码表与正文长度不一致".into());
    }
    Ok(out)
}

pub struct AnalysisSummary {
    pub char_count: u64,
    pub missing_chars: u64,
    pub known_keys: u64,
    pub select_count: u64,
    pub avg_code_len: f64,
    pub key_counts: BTreeMap<u8, u32>,
}

/// Standard punctuation uses its physical key, including Shift where needed.
fn literal_keys(text: &str) -> Option<String> {
    if text == "……" {
        return Some("^".into());
    }
    if text == "——" {
        return Some("_".into());
    }
    text.chars()
        .map(|ch| {
            Some(match ch {
                '\n' | '\r' => "".into(),
                '，' => ",".into(),
                '。' => ".".into(),
                '、' => "/".into(),
                '；' => ";".into(),
                '：' => ":".into(),
                '！' => "!".into(),
                '？' => "?".into(),
                '（' => "(".into(),
                '）' => ")".into(),
                '“' | '”' => "\"".into(),
                '‘' | '’' => "'".into(),
                '《' => "<".into(),
                '》' => ">".into(),
                '【' => "[".into(),
                '】' => "]".into(),
                '￥' => "$".into(),
                '…' => "^".into(),
                '—' => "_".into(),
                '·' => "~".into(),
                c if c.is_ascii() && !c.is_control() => c.to_string(),
                _ => return None,
            })
        })
        .collect::<Option<Vec<String>>>()
        .map(|parts| parts.concat())
}

pub fn entry_keys(entry: &FullAnalysisEntry, _auto4: bool, _max_code_length: usize) -> String {
    let Some(code) = &entry.code else {
        return literal_keys(&entry.word).unwrap_or_default();
    };
    // The route solver knows whether following punctuation commits a pending
    // candidate. Reconstructing tails here would change both the route and F10.
    format!("{}{}", code, entry.tail.replace('_', " "))
}

fn physical_key(ch: char) -> Option<(u8, bool)> {
    if ch.is_ascii_alphabetic() {
        return Some((ch.to_ascii_uppercase() as u8, ch.is_ascii_uppercase()));
    }
    if ch.is_ascii_digit() {
        return Some((ch as u8, false));
    }
    Some(match ch {
        ' ' => (0x20, false),
        '`' => (0xC0, false),
        '~' => (0xC0, true),
        '-' => (0xBD, false),
        '_' => (0xBD, true),
        '=' => (0xBB, false),
        '+' => (0xBB, true),
        '[' => (0xDB, false),
        '{' => (0xDB, true),
        ']' => (0xDD, false),
        '}' => (0xDD, true),
        '\\' => (0xDC, false),
        '|' => (0xDC, true),
        ';' => (0xBA, false),
        ':' => (0xBA, true),
        '\'' => (0xDE, false),
        '"' => (0xDE, true),
        ',' => (0xBC, false),
        '<' => (0xBC, true),
        '.' => (0xBE, false),
        '>' => (0xBE, true),
        '/' => (0xBF, false),
        '?' => (0xBF, true),
        '!' => (b'1', true),
        '@' => (b'2', true),
        '#' => (b'3', true),
        '$' => (b'4', true),
        '%' => (b'5', true),
        '^' => (b'6', true),
        '&' => (b'7', true),
        '*' => (b'8', true),
        '(' => (b'9', true),
        ')' => (b'0', true),
        _ => return None,
    })
}

pub fn display_keys(keys: &str) -> String {
    keys.chars()
        .map(|ch| {
            if ch == ' ' {
                return "_".into();
            }
            let Some((vk, true)) = physical_key(ch) else {
                return ch.to_string();
            };
            let base = match vk {
                0xC0 => '`',
                0xBD => '-',
                0xBB => '=',
                0xDB => '[',
                0xDD => ']',
                0xDC => '\\',
                0xBA => ';',
                0xDE => '\'',
                0xBC => ',',
                0xBE => '.',
                0xBF => '/',
                vk => (vk as char).to_ascii_lowercase(),
            };
            format!("↑{base}")
        })
        .collect()
}

pub fn summarize(fa: &FullAnalysis, auto4: bool) -> AnalysisSummary {
    let mut out = AnalysisSummary {
        char_count: 0,
        missing_chars: 0,
        known_keys: 0,
        select_count: 0,
        avg_code_len: 0.,
        key_counts: BTreeMap::new(),
    };
    for entry in &fa.entries {
        let chars = entry
            .word
            .chars()
            .filter(|c| !matches!(c, '\n' | '\r'))
            .count() as u64;
        out.char_count += chars;
        if entry.code.is_none() && literal_keys(&entry.word).is_none() {
            out.missing_chars += chars;
            continue;
        }
        out.select_count += u64::from(entry.code.is_some() && entry.rank > 1);
        for ch in entry_keys(entry, auto4, fa.max_code_length).chars() {
            if let Some((vk, shift)) = physical_key(ch) {
                *out.key_counts.entry(vk).or_default() += 1;
                out.known_keys += 1;
                if shift {
                    *out.key_counts.entry(0x10).or_default() += 1;
                    out.known_keys += 1;
                }
            }
        }
    }
    if out.char_count > 0 {
        out.avg_code_len = out.known_keys as f64 / out.char_count as f64;
    }
    out
}

/// Check every returned span, including punctuation, before displaying recipes.
pub fn response_entries(
    text: &str,
    value: &serde_json::Value,
) -> Result<Vec<FullAnalysisEntry>, String> {
    let chars: Vec<_> = text.chars().collect();
    let words = value["words"].as_array().ok_or("服务未返回词库分析")?;
    let mut pos = 0;
    let mut out: Vec<FullAnalysisEntry> = Vec::with_capacity(words.len());
    for word in words {
        let start = word["start"].as_u64().ok_or("分析缺少位置")? as usize;
        let len = word["len"].as_u64().ok_or("分析缺少长度")? as usize;
        let surface = word["text"].as_str().ok_or("分析缺少正文")?;
        if start != pos
            || len == 0
            || start + len > chars.len()
            || !chars[start..start + len]
                .iter()
                .copied()
                .eq(surface.chars())
        {
            return Err("分析与当前正文不一致，请重试".into());
        }
        let code = word["code"].as_str().map(str::to_owned);
        if code.is_none()
            && matches!(surface, "…" | "—")
            && out
                .last()
                .is_some_and(|prev| prev.code.is_none() && prev.word == surface)
        {
            out.last_mut().unwrap().word.push_str(surface);
        } else {
            out.push(FullAnalysisEntry {
                word: surface.into(),
                code_len: code.as_ref().map_or(0, |c| c.len() as u64),
                code,
                rank: word["rank"].as_u64().unwrap_or(0),
                tail: word["tail"].as_str().unwrap_or("").into(),
            });
        }
        pos += len;
    }
    if pos != chars.len() {
        return Err("分析未覆盖全文，请重试".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn totals_include_confirm_selection_shift_and_missing() {
        let fa = FullAnalysis {
            entries: vec![
                FullAnalysisEntry {
                    word: "你好".into(),
                    code: Some("abcd".into()),
                    rank: 1,
                    ..Default::default()
                },
                FullAnalysisEntry {
                    word: "世界".into(),
                    code: Some("lq".into()),
                    rank: 2,
                    tail: "2".into(),
                    ..Default::default()
                },
                FullAnalysisEntry {
                    word: "？".into(),
                    ..Default::default()
                },
                FullAnalysisEntry {
                    word: "🦀".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let a = summarize(&fa, true);
        assert_eq!(
            (a.char_count, a.missing_chars, a.known_keys, a.select_count),
            (6, 1, 9, 1)
        );
        assert_eq!(a.key_counts[&0x10], 1);
        assert_eq!(summarize(&fa, false).known_keys, 9);
        let mut explicit_confirm = fa.clone();
        explicit_confirm.entries[0].tail = "_".into();
        assert_eq!(summarize(&explicit_confirm, false).known_keys, 10);
    }
    #[test]
    fn executable_recipe_preserves_server_pending_and_confirmation_tails() {
        for (code, rank, tail, keys) in [
            ("ab", 1, "", "ab"),
            ("abcd", 1, "", "abcd"),
            ("abcd", 1, "_", "abcd "),
            ("abcde", 1, "_", "abcde "),
            ("abc", 2, ";", "abc;"),
        ] {
            let entry = FullAnalysisEntry {
                word: "词组".into(),
                code: Some(code.into()),
                rank,
                tail: tail.into(),
                ..Default::default()
            };
            for auto4 in [false, true] {
                assert_eq!(entry_keys(&entry, auto4, 4), keys);
            }
        }
    }
    #[test]
    fn char_codes_parse_single_response_per_char() {
        let v = serde_json::json!({"words":[
            {"text":"怎","start":0,"len":1,"code":"jq","rank":1,"tail":"_"},
            {"text":"呢","start":1,"len":1,"code":"nz","rank":2,"tail":";"},
            {"text":"￥","start":2,"len":1},
        ]});
        let codes = response_char_codes("怎呢￥", &v).unwrap();
        assert_eq!(codes.len(), 3);
        let first = codes[0].as_ref().unwrap();
        assert_eq!(first.code, "jq");
        assert_eq!(first.rank, 1);
        let second = codes[1].as_ref().unwrap();
        assert_eq!(second.code, "nz");
        assert_eq!(second.rank, 2);
        assert_eq!(second.tail, ";");
        assert!(codes[2].is_none(), "无码字为 None");
    }
    #[test]
    fn char_codes_reject_word_entries() {
        let v = serde_json::json!({"words":[
            {"text":"怎么","start":0,"len":2,"code":"zm","rank":1,"tail":"_"},
        ]});
        assert!(response_char_codes("怎么", &v).is_err());
    }
    #[test]
    fn prompt_word_colors_use_the_six_extracted_classes() {
        use WordColorClass::*;
        for (word, code, expected) in [
            ("词组", "a", Some(One)),
            ("更长的词", "a", Some(One)),
            ("𠮷甲", "a", Some(One)),
            ("词组", "ab", Some(Short)),
            ("三字词", "abc", Some(Short)),
            ("更长的词", "abc", Some(Short)),
            ("词组", "abc", Some(Two3)),
            ("词组", "abcd", Some(Two4)),
            ("三字词", "abcd", Some(Three)),
            ("四字词语", "abcd", Some(Long)),
            ("𠮷甲", "abc", Some(Two3)),
            ("甲", "a", None),
            // 含标点多字词按原版口径参与分档（2026-11-12 对齐）
            ("……", "a", Some(One)),
            ("……", "abc", Some(Two3)),
            ("词，词", "abc", Some(Short)),
            ("怎么办？", "k", Some(One)),
            ("词组", "", None),
        ] {
            assert_eq!(word_color_class(word, code), expected, "{word}/{code}");
        }
    }
    #[test]
    fn malformed_or_incomplete_recipe_is_rejected() {
        let good = serde_json::json!({"words":[{"text":"你好","start":0,"len":2,"code":"ab","rank":1,"tail":"_"}]});
        assert!(response_entries("你好", &good).is_ok());
        assert!(response_entries("你好！", &good).is_err());
        assert!(response_entries("世界", &good).is_err());
        assert_eq!(summarize(&FullAnalysis::default(), true).avg_code_len, 0.);
    }
    #[test]
    fn native_multichar_punctuation_counts_one_shifted_key_chord() {
        assert!(is_multi_word("世界"));
        assert!(!is_multi_word("的"));
        // Punctuation-glued multi-character words now count as words for the
        // bracket and tint, matching the original trainer's rendering.
        assert!(is_multi_word("……"));
        assert!(is_multi_word("——"));
        assert!(is_multi_word("怎么办？"));
        assert_eq!(display_keys("ab ^_$"), "ab_↑6↑-↑4");
        let v = serde_json::json!({"words":[
            {"text":"…","start":0,"len":1},{"text":"…","start":1,"len":1},
            {"text":"—","start":2,"len":1},{"text":"—","start":3,"len":1},
            {"text":"【","start":4,"len":1},{"text":"】","start":5,"len":1},
            {"text":"￥","start":6,"len":1}]});
        let fa = FullAnalysis {
            entries: response_entries("……——【】￥", &v).unwrap(),
            ..Default::default()
        };
        let summary = summarize(&fa, true);
        assert_eq!(
            (
                summary.char_count,
                summary.missing_chars,
                summary.known_keys
            ),
            (7, 0, 8)
        );
        let singles = FullAnalysis {
            entries: ["…", "—", "·"]
                .into_iter()
                .map(|word| FullAnalysisEntry {
                    word: word.into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let summary = summarize(&singles, true);
        assert_eq!(
            (
                summary.char_count,
                summary.missing_chars,
                summary.known_keys
            ),
            (3, 0, 6)
        );
    }
}
