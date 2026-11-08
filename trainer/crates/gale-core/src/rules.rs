//! Frozen round rules and bounded, body-only text transformations.
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrainingMode {
    #[default]
    Practice,
    Race,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuoteRules {
    pub enabled: bool,
    pub double: bool,
    pub single: bool,
    pub corner: bool,
    pub double_corner: bool,
}
impl Default for QuoteRules {
    fn default() -> Self {
        Self {
            enabled: true,
            double: true,
            single: true,
            corner: true,
            double_corner: true,
        }
    }
}
impl QuoteRules {
    pub fn equivalent(&self, a: char, b: char) -> bool {
        if a == b {
            return true;
        }
        if !self.enabled {
            return false;
        }
        [
            (self.double, "“”\"＂〝〞"),
            (self.single, "‘’'＇"),
            (self.corner, "「」"),
            (self.double_corner, "『』"),
        ]
        .iter()
        .any(|(on, members)| *on && members.contains(a) && members.contains(b))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionRules {
    pub mode: TrainingMode,
    pub quotes: QuoteRules,
    pub word_hints: bool,
    pub pause_on_blur: bool,
    pub allow_paste: bool,
}
impl Default for SessionRules {
    fn default() -> Self {
        Self {
            mode: TrainingMode::Practice,
            quotes: QuoteRules::default(),
            word_hints: true,
            pause_on_blur: true,
            allow_paste: false,
        }
    }
}
impl SessionRules {
    pub fn normalized(mut self) -> Self {
        if self.mode == TrainingMode::Race {
            self.word_hints = false;
            self.pause_on_blur = false;
            self.allow_paste = false;
        }
        self
    }
    pub fn fingerprint(&self) -> String {
        fingerprint(&serde_json::to_vec(self).unwrap_or_default())
    }
}
pub fn fingerprint(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleaningRule {
    pub id: String,
    pub name: String,
    pub pattern: String,
    pub replacement: String,
    pub enabled: bool,
    /// Display metadata only: race permissions never trust imported `safe`/`builtin` flags.
    #[serde(default)]
    pub safe: bool,
    #[serde(default)]
    pub builtin: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleaningPreset {
    pub rules: Vec<CleaningRule>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleaningResult {
    pub raw_text: String,
    pub cleaned_text: String,
    pub hits: Vec<(String, usize)>,
    pub content_changed: bool,
}

const MAX_INPUT: usize = 100_000;
const MAX_OUTPUT_BYTES: usize = 1_600_000;
const MAX_WORK_BYTES: usize = 8_000_000;
const MAX_PRESET_BYTES: usize = 131_072;
impl CleaningPreset {
    pub fn builtin() -> Self {
        let rules = [
            ("bom", "字节序标记", "\\x{FEFF}", "", true),
            (
                "spaces",
                "异体空格",
                "[\\x{00A0}\\x{1680}\\x{2000}-\\x{200A}\\x{202F}\\x{205F}]",
                " ",
                true,
            ),
            ("fullspace", "全角空格", "\\x{3000}", " ", false),
            ("dashes", "破折号", "[—–―]{2,}", "——", false),
            ("ellipsis", "省略号", "\\.{6,}|…{2,}", "……", false),
        ]
        .into_iter()
        .map(|(id, name, pattern, replacement, safe)| CleaningRule {
            id: id.into(),
            name: name.into(),
            pattern: pattern.into(),
            replacement: replacement.into(),
            enabled: true,
            safe,
            builtin: true,
        })
        .collect();
        Self { rules }
    }
    pub fn import_json(text: &str) -> Result<Self, String> {
        if text.len() > MAX_PRESET_BYTES {
            return Err("规则文件超过 128 KiB".into());
        }
        let preset: Self = serde_json::from_str(text).map_err(|e| e.to_string())?;
        preset.validate()?;
        Ok(preset)
    }
    pub fn export_json(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_string_pretty(self).map_err(|e| e.to_string())
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.rules.len() > 64 {
            return Err("最多 64 条清洗规则".into());
        }
        for rule in &self.rules {
            if rule.pattern.chars().count() > 512
                || rule.replacement.chars().count() > 512
                || rule.id.len() > 128
                || rule.name.len() > 256
            {
                return Err(format!("规则 {} 超过长度限制", rule.name));
            }
            RegexBuilder::new(&rule.pattern)
                .size_limit(256 * 1024)
                .dfa_size_limit(256 * 1024)
                .nest_limit(32)
                .build()
                .map_err(|e| format!("{}: {e}", rule.name))?;
        }
        Ok(())
    }
}

/// The caller passes an already parsed segment body. Titles and marks never enter this API.
/// Every output append is checked *before* allocation; intermediate strings share one work budget.
pub fn clean_body(
    raw: &str,
    preset: &CleaningPreset,
    mode: TrainingMode,
) -> Result<CleaningResult, String> {
    if raw.chars().count() > MAX_INPUT {
        return Err("正文超过 100000 字".into());
    }
    preset.validate()?;
    let trusted = CleaningPreset::builtin();
    let output_limit = MAX_OUTPUT_BYTES.min(raw.len().saturating_mul(4).saturating_add(4096));
    let mut work = 0usize;
    let mut text = raw.to_string();
    let mut hits = Vec::new();
    let mut content_changed = false;
    for rule in preset.rules.iter().filter(|r| r.enabled) {
        let race_safe = trusted.rules.iter().any(|b| {
            b.safe
                && b.id == rule.id
                && b.pattern == rule.pattern
                && b.replacement == rule.replacement
        });
        if mode == TrainingMode::Race && !race_safe {
            return Err(format!("赛制禁止自定义或内容级清洗：{}", rule.name));
        }
        work = work.checked_add(text.len()).ok_or("清洗工作量超限")?;
        if work > MAX_WORK_BYTES {
            return Err("清洗工作量超限".into());
        }
        let re = RegexBuilder::new(&rule.pattern)
            .size_limit(256 * 1024)
            .dfa_size_limit(256 * 1024)
            .nest_limit(32)
            .build()
            .map_err(|e| e.to_string())?;
        let mut out = String::new();
        let mut end = 0;
        let mut count = 0;
        for cap in re.captures_iter(&text) {
            let m = cap.get(0).ok_or("清洗匹配无效")?;
            let replacement = expand_bounded(
                &cap,
                &rule.replacement,
                output_limit.saturating_sub(out.len()),
            )?;
            for part in [&text[end..m.start()], &replacement] {
                if out.len().saturating_add(part.len()) > output_limit {
                    return Err("清洗后正文过长（增长上限）".into());
                }
                out.push_str(part);
            }
            end = m.end();
            count += 1;
            work = work.saturating_add(replacement.len());
            if work > MAX_WORK_BYTES {
                return Err("清洗工作量超限".into());
            }
        }
        if out.len().saturating_add(text.len() - end) > output_limit {
            return Err("清洗后正文过长".into());
        }
        out.push_str(&text[end..]);
        if count > 0 {
            hits.push((rule.id.clone(), count));
            content_changed |= !race_safe && out != text;
        }
        text = out;
    }
    if !raw.is_empty() && text.is_empty() {
        return Err("清洗结果为空，已保留原文".into());
    }
    Ok(CleaningResult {
        raw_text: raw.into(),
        cleaned_text: text,
        hits,
        content_changed,
    })
}

// The standard capture expander grows an intermediate String before the caller can
// inspect it. Expand captures piece by piece so "$0" repeated 256 times is bounded too.
fn expand_bounded(
    caps: &regex::Captures<'_>,
    template: &str,
    limit: usize,
) -> Result<String, String> {
    let mut out = String::new();
    let mut offset = 0;
    while offset < template.len() {
        let rest = &template[offset..];
        let (piece, consumed) = if rest.starts_with("$$") {
            ("$", 2)
        } else if let Some(after) = rest.strip_prefix("${") {
            if let Some(end) = after.find('}') {
                let name = &after[..end];
                let value = name
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| caps.get(n))
                    .or_else(|| caps.name(name))
                    .map(|m| m.as_str())
                    .unwrap_or("");
                (value, end + 3)
            } else {
                ("$", 1)
            }
        } else if let Some(after) = rest.strip_prefix('$') {
            let len = after
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
            if len == 0 {
                ("$", 1)
            } else {
                let name = &after[..len];
                let value = name
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| caps.get(n))
                    .or_else(|| caps.name(name))
                    .map(|m| m.as_str())
                    .unwrap_or("");
                (value, len + 1)
            }
        } else {
            let end = rest.find('$').unwrap_or(rest.len());
            (&rest[..end], end)
        };
        if out.len().saturating_add(piece.len()) > limit {
            return Err("清洗替换展开超过增长上限".into());
        }
        out.push_str(piece);
        offset += consumed;
    }
    Ok(out)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoundMetadata {
    pub mode: TrainingMode,
    /// Frozen working body after explicit cleaning and structural LF normalization.
    pub text_hash: String,
    /// Original segment body, including original spaces and line endings.
    #[serde(default)]
    pub raw_text_hash: String,
    pub rules_hash: String,
    pub quote_relaxed: usize,
    pub cleaned: bool,
    pub pasted: bool,
    pub complete: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quotes_only_same_class() {
        let q = QuoteRules::default();
        assert!(q.equivalent('“', '”'));
        assert!(q.equivalent('＂', '"'));
        assert!(q.equivalent('「', '」'));
        assert!(!q.equivalent('“', '‘'));
        assert!(!q.equivalent('《', '》'));
    }
    #[test]
    fn expansion_stops_before_huge_allocation() {
        let r = CleaningRule {
            id: "x".into(),
            name: "x".into(),
            pattern: ".".into(),
            replacement: "x".repeat(512),
            enabled: true,
            safe: true,
            builtin: true,
        };
        let p = CleaningPreset {
            rules: vec![r.clone(), r],
        };
        assert!(clean_body(&"a".repeat(100000), &p, TrainingMode::Practice).is_err());
    }
    #[test]
    fn repeated_capture_expansion_is_bounded_too() {
        let r = CleaningRule {
            id: "capture".into(),
            name: "capture".into(),
            pattern: "(?s)(.*)".into(),
            replacement: "$1".repeat(256),
            enabled: true,
            safe: false,
            builtin: false,
        };
        assert!(clean_body(
            &"a".repeat(100000),
            &CleaningPreset { rules: vec![r] },
            TrainingMode::Practice
        )
        .is_err());
    }
    #[test]
    fn imported_flags_cannot_bypass_race() {
        let p = CleaningPreset {
            rules: vec![CleaningRule {
                id: "bom".into(),
                name: "fake".into(),
                pattern: ".".into(),
                replacement: "a".into(),
                enabled: true,
                safe: true,
                builtin: true,
            }],
        };
        assert!(clean_body("正文", &p, TrainingMode::Race).is_err());
    }
    #[test]
    fn no_joiner_deletion_and_roundtrip() {
        let p = CleaningPreset::builtin();
        let s = "👩\u{200d}💻\u{200c}文字";
        assert_eq!(
            clean_body(s, &p, TrainingMode::Practice)
                .unwrap()
                .cleaned_text,
            s
        );
        assert_eq!(
            CleaningPreset::import_json(&p.export_json().unwrap()).unwrap(),
            p
        );
    }
    #[test]
    fn regex_limits_and_raw_preserved() {
        let mut p = CleaningPreset::builtin();
        p.rules.truncate(1);
        let c = clean_body("\u{feff}正文", &p, TrainingMode::Race).unwrap();
        assert_eq!(c.raw_text, "\u{feff}正文");
        assert_eq!(c.cleaned_text, "正文");
        p.rules[0].pattern = "(?=a)".into();
        assert!(p.validate().is_err());
    }
}
