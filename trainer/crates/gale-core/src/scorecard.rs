//! User-authored score templates. The legacy report remains a stable wire format.
use crate::score::{fmt_duration, format_score_report, race_badge, ScoreRecord};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub struct ScorePreset {
    pub id: &'static str,
    pub name: &'static str,
    pub template: &'static str,
}

pub const SCORE_PRESETS: &[ScorePreset] = &[
    ScorePreset {
        id: "classic",
        name: "经典成绩",
        template: "{legacy_report}",
    },
    ScorePreset {
        id: "detailed",
        name: "完整成绩",
        template: "{segment}  速度{speed}  击键{strokes}  键准{key_accuracy}%  字数{chars}  打词率{word_rate}  {retype_label}  码长{code_len}  总键数{keys}  回改{corrections}  退格{backspaces}  废码{waste_codes}  选重{select_count}  标顶{ding_count}  用时{time}  错字{wrong}  {race}  {signature}",
    },
    ScorePreset {
        id: "compact",
        name: "简洁成绩",
        template: "{segment}  速度{speed}  键准{key_accuracy}%  码长{code_len}  {retype_label}  {signature}",
    },
];

/// English identifiers are stable; Chinese aliases are accepted for hand editing.
pub const SCORE_FIELDS: &[(&str, &str)] = &[
    ("segment", "段号"),
    ("title", "文章标题"),
    ("source", "来源"),
    ("date", "日期"),
    ("speed", "速度（有错字时罚后/裸速）"),
    ("raw_speed", "裸速"),
    ("effective_speed", "有效速度"),
    ("strokes", "击键"),
    ("character_accuracy", "字准（当前正确字占比）"),
    ("key_accuracy", "键准（击键效率）"),
    ("code_len", "码长"),
    ("word_rate", "打词率（含百分号，旧记录显示未采集）"),
    ("select_count", "选重次数"),
    ("ding_count", "标顶次数"),
    ("retype_count", "重打次数（首打为0）"),
    ("retype_label", "首打/重打标记"),
    ("keys", "总键数"),
    ("backspaces", "退格次数"),
    ("corrections", "回改次数"),
    ("waste_codes", "废码次数"),
    ("wrong", "错字数"),
    ("chars", "字数"),
    ("time", "用时"),
    ("difficulty", "难度（未提供时为空）"),
    ("input_method", "输入法名"),
    ("race", "赛制和规则标记"),
    ("signature", "签名"),
    ("legacy_report", "完整经典成绩"),
];

pub fn preset(id: &str) -> &'static ScorePreset {
    SCORE_PRESETS
        .iter()
        .find(|p| p.id == id)
        .unwrap_or(&SCORE_PRESETS[0])
}

/// 用户要求成绩单不再出现「字准」。清掉历史模板/字段配置里的字准 token，
/// 并压掉留下的多余空格；自定义模板若显式写 `{character_accuracy}` 也会被移除。
pub fn strip_character_accuracy(template: &str) -> String {
    let mut out = template.to_string();
    for token in [
        "字准{character_accuracy}%",
        "字准{character_accuracy}",
        "{character_accuracy}%",
        "{character_accuracy}",
        "{字准}%",
        "{字准}",
    ] {
        out = out.replace(token, "");
    }
    out.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("
")
        .trim()
        .to_string()
}

/// Expands known `{field}` tokens wherever they occur. Unknown tokens are retained
/// so a typo is visible and newer templates remain readable by older versions.
/// `{{` and `}}` write literal braces; literal whitespace and newlines are kept.
pub fn format_score_template(r: &ScoreRecord, template: &str, signature: &str) -> String {
    let template = if template.trim().is_empty() {
        SCORE_PRESETS[0].template
    } else {
        template
    };
    let mut output = String::with_capacity(template.len() + 128);
    let mut rest = template;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("{{") {
            output.push('{');
            rest = after;
        } else if let Some(after) = rest.strip_prefix("}}") {
            output.push('}');
            rest = after;
        } else if rest.starts_with('{') {
            if let Some(end) = rest.find('}') {
                let token = &rest[1..end];
                if let Some(value) = field(r, token, signature) {
                    output.push_str(&value);
                } else {
                    output.push_str(&rest[..=end]);
                }
                rest = &rest[end + 1..];
            } else {
                output.push_str(rest);
                break;
            }
        } else {
            let ch = rest.chars().next().unwrap();
            output.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    output
}

fn field(r: &ScoreRecord, name: &str, signature: &str) -> Option<String> {
    Some(match name.trim() {
        "segment" | "段号" => {
            if r.mark.contains('-') {
                format!("段{}", r.mark)
            } else {
                format!("第{}段", r.mark)
            }
        }
        "title" | "标题" => r.title.clone(),
        "source" | "来源" => r.source.clone(),
        "date" | "日期" => r.date.clone(),
        "speed" | "速度" => {
            if r.wrong > 0 {
                format!("{:.2}/{:.2}", r.effective_speed, r.speed)
            } else {
                format!("{:.2}", r.speed)
            }
        }
        "raw_speed" | "裸速" => format!("{:.2}", r.speed),
        "effective_speed" | "有效速度" => format!("{:.2}", r.effective_speed),
        "strokes" | "strokes_per_sec" | "击键" => format!("{:.2}", r.strokes_per_sec),
        "character_accuracy" | "字准" => format!("{:.2}", r.character_accuracy()),
        "key_accuracy" | "accuracy" | "键准" => format!("{:.2}", r.accuracy),
        "code_len" | "码长" => format!("{:.2}", r.code_len),
        "word_rate" | "打词率" => r
            .word_rate
            .filter(|v| v.is_finite())
            .map(|v| format!("{:.2}%", v.clamp(0., 100.)))
            .unwrap_or_else(|| "未采集".into()),
        "select_count" | "select" | "选重" => r.select_count.to_string(),
        "ding_count" | "标顶" => r.ding_count.to_string(),
        "retype_count" | "retype" | "重打次数" => r.retype_count.to_string(),
        "retype_label" | "重打" => {
            if r.retype_count == 0 {
                "【首打】".into()
            } else {
                format!("重打{}", r.retype_count)
            }
        }
        "keys" | "总键数" => r.keys.to_string(),
        "backspaces" | "backspace" | "backs" | "退格" => r.backspaces.to_string(),
        "corrections" | "回改" => r.corrections.to_string(),
        "waste_codes" | "废码" => r.waste_codes.to_string(),
        "wrong" | "错字" => r.wrong.to_string(),
        "chars" | "字数" => r.chars.to_string(),
        "time" | "用时" => fmt_duration(r.time_ms),
        "difficulty" | "难度" => r
            .difficulty
            .as_ref()
            .map(|d| match d.score {
                Some(v) => format!("{}({:.2})", d.label, v),
                None => d.label.clone(),
            })
            .unwrap_or_default(),
        "input_method" | "输入法" => r.input_method.clone(),
        "race" | "赛制" => r
            .round
            .as_ref()
            .map(|m| {
                let mut s = race_badge(m.mode, m.word_hints).to_string();
                if !m.complete {
                    s.push_str("·未完成");
                }
                if m.cleaned {
                    s.push_str("·清洗");
                }
                if m.pasted {
                    s.push_str("·粘贴");
                }
                s
            })
            .unwrap_or_default(),
        "signature" | "签名" => signature.into(),
        "legacy_report" => {
            let legacy = format_score_report(r);
            match legacy.strip_suffix(crate::APP_NAME) {
                Some(prefix) => format!("{prefix}{signature}"),
                None => legacy,
            }
        }
        _ => return None,
    })
}

/// Per accepted output character: 0=punctuation, 1=single, 2=word. Keep a stack
/// so a correction removes its original attribution instead of inflating rate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WordRateTracker {
    classes: Vec<u8>,
}
impl WordRateTracker {
    pub fn commit(&mut self, chars: &[char]) {
        let punctuation: Vec<bool> = chars.iter().map(|c| is_punctuation(*c)).collect();
        let is_word = punctuation.iter().filter(|p| !**p).count() >= 2;
        self.classes.extend(punctuation.into_iter().map(|p| {
            if p {
                0
            } else if is_word {
                2
            } else {
                1
            }
        }));
    }
    pub fn backspace(&mut self) {
        self.classes.pop();
    }
    pub fn len(&self) -> usize {
        self.classes.len()
    }
    pub fn is_valid_for(&self, len: usize) -> bool {
        self.classes.len() == len && self.classes.iter().all(|c| *c <= 2)
    }
    pub fn percent(&self) -> f64 {
        let words = self.classes.iter().filter(|c| **c == 2).count();
        let chars = self.classes.iter().filter(|c| **c != 0).count();
        if chars == 0 {
            0.
        } else {
            words as f64 / chars as f64 * 100.
        }
    }
}
fn is_punctuation(c: char) -> bool {
    // Unicode punctuation plus the ASCII punctuation keys used by Chinese IMEs.
    // Spaces are intentionally retained as single/word output characters.
    static PUNCTUATION: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    c.is_ascii_punctuation()
        || PUNCTUATION
            .get_or_init(|| regex::Regex::new(r"^\p{P}$").unwrap())
            .is_match(c.encode_utf8(&mut [0; 4]))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record() -> ScoreRecord {
        serde_json::from_value(serde_json::json!({
            "ts":1,"date":"2026-11-08","title":"例文","source":"群文","mark":"2-8",
            "para_index":0,"chars":10,"keys":20,"backspaces":2,"wrong":1,"time_ms":83456,
            "speed":100.,"accuracy":80.,"code_len":2.,"effective_speed":95.,"strokes_per_sec":3.
        }))
        .unwrap()
    }
    #[test]
    fn character_accuracy_is_stripped_from_legacy_templates() {
        assert_eq!(
            strip_character_accuracy(
                "{segment}  速度{speed}  字准{character_accuracy}%  键准{key_accuracy}%"
            ),
            "{segment} 速度{speed} 键准{key_accuracy}%"
        );
        assert_eq!(strip_character_accuracy("{字准}%"), "");
        assert!(!SCORE_PRESETS.iter().any(|p| {
            p.template.contains("字准") || p.template.contains("character_accuracy")
        }));
        assert!(!crate::config::Config::default()
            .score_fields
            .contains("character_accuracy"));
    }

    #[test]
    fn templates_keep_legacy_and_distinguish_accuracy_and_optional_data() {
        let mut r = record();
        assert_eq!(
            format_score_template(&r, preset("classic").template, crate::APP_NAME),
            format_score_report(&r)
        );
        assert_eq!(format_score_template(&r, "{{字准}}={character_accuracy}% 键准={key_accuracy}% {word_rate} {retype_label} {unknown}", "签名"),
            "{字准}=90.00% 键准=80.00% 未采集 【首打】 {unknown}");
        r.word_rate = Some(62.5);
        r.retype_count = 2;
        assert_eq!(
            format_score_template(
                &r,
                "{segment}/{time}/{word_rate}/{retype_label}/{signature}",
                "自签"
            ),
            "段2-8/1:23.45/62.50%/重打2/自签"
        );
        assert_eq!(
            format_score_template(&r, "{accuracy}/{select}/{retype}/{backspace}", ""),
            "80.00/0/2/2"
        );
    }
    #[test]
    fn word_rate_excludes_punctuation_and_removes_corrected_attribution() {
        let mut words = WordRateTracker::default();
        words.commit(&"你好，".chars().collect::<Vec<_>>());
        words.commit(&['我']);
        assert!((words.percent() - 200. / 3.).abs() < 1e-9);
        words.backspace();
        words.backspace(); // punctuation has no effect on the denominator
        words.backspace(); // delete one word character
        words.commit(&['你']);
        assert_eq!(words.percent(), 50.);
        words.commit(&[' ']);
        assert!((words.percent() - 100. / 3.).abs() < 1e-9);
        assert!(words.is_valid_for(3));
    }
}
