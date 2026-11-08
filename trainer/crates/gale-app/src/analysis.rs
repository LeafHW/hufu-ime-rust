//! Dictionary analysis shared by the live panel and native layout fixtures.
use std::collections::BTreeMap;

pub fn is_multi_word(text: &str) -> bool {
    text.chars()
        .filter(|ch| ch.is_alphanumeric())
        .take(2)
        .count()
        == 2
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
                c if c.is_ascii() && !c.is_control() => c.to_string(),
                _ => return None,
            })
        })
        .collect::<Option<Vec<String>>>()
        .map(|parts| parts.concat())
}

pub fn entry_keys(entry: &FullAnalysisEntry, auto4: bool, _max_code_length: usize) -> String {
    let Some(code) = &entry.code else {
        return literal_keys(&entry.word).unwrap_or_default();
    };
    let tail = if entry.rank == 1 && code.len() == 4 {
        if auto4 {
            ""
        } else {
            " "
        }
    } else {
        entry.tail.as_str()
    };
    format!("{}{}", code, tail.replace('_', " "))
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
        assert_eq!(summarize(&fa, false).known_keys, 10);
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
        assert!(!is_multi_word("……"));
        assert!(!is_multi_word("——"));
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
    }
}
