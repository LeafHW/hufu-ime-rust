//! 【SSF M1】skin.ini 文本解码与宽容解析。
//!
//! 编码：UTF-16LE/BOM 为主（搜狗官方口径），UTF-8(BOM)、GBK 兜底。
//! 解析：节名/键名统一 ASCII 小写；`;`/`#` 注释；重复键取后者并记录
//! （虎娘对消费节内重复键直接报错，这里按「尽力加载」原则降级为告警，
//! 因为存量真皮肤存在历史遗留重复键）。未消费节（[StatusBar] 等）保留
//! 原文进 [`Ini::ignored_sections`]，不参与键解析——对齐虎娘
//! "Only candidate-window sections are consumed; keep their syntax strict"。

use super::SsfError;

/// 单条目上限（与 MAX_ASSET_BYTES 同源，但 skin.ini 通常 <8KB）。
const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
const MAX_LINES: usize = 20_000;
const MAX_LINE_CHARS: usize = 16_384;

/// 字节 → 文本。BOM 优先：FF FE→UTF-16LE、FE FF→UTF-16BE、EF BB BF→UTF-8；
/// 无 BOM 先试严格 UTF-8，失败按 GBK（历史皮肤编辑器的本地编码）。
pub fn decode_text(bytes: &[u8]) -> Result<String, SsfError> {
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(SsfError::Message("skin.ini 超过 4MiB 上限".into()));
    }
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let body = &bytes[2..];
        if !body.len().is_multiple_of(2) {
            return Err(SsfError::Message("skin.ini UTF-16 长度为奇数".into()));
        }
        let units: Vec<u16> = body
            .as_chunks::<2>().0.iter()
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .collect();
        return String::from_utf16(&units)
            .map_err(|_| SsfError::Message("skin.ini 含非法 UTF-16 代理对".into()));
    }
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let body = &bytes[2..];
        if !body.len().is_multiple_of(2) {
            return Err(SsfError::Message("skin.ini UTF-16 长度为奇数".into()));
        }
        let units: Vec<u16> = body
            .as_chunks::<2>().0.iter()
            .map(|p| u16::from_be_bytes([p[0], p[1]]))
            .collect();
        return String::from_utf16(&units)
            .map_err(|_| SsfError::Message("skin.ini 含非法 UTF-16 代理对".into()));
    }
    let body = if bytes.len() >= 3 && bytes[0..3] == [0xEF, 0xBB, 0xBF] {
        &bytes[3..]
    } else {
        bytes
    };
    if let Ok(text) = std::str::from_utf8(body) {
        return Ok(text.to_string());
    }
    let (text, _, had_errors) = encoding_rs::GBK.decode(body);
    if had_errors {
        return Err(SsfError::Message(
            "skin.ini 编码无法识别（需 UTF-8/UTF-16/GBK）".into(),
        ));
    }
    Ok(text.into_owned())
}

/// 键值表（键已小写、值已 trim）。
pub type Section = Vec<(String, String)>;

/// 解析结果：消费节（小写名 → 有序键值，重复键取后者并记入 duplicates），
/// 以及被忽略节的名字列表。
#[derive(Debug, Default)]
pub struct Ini {
    pub sections: std::collections::BTreeMap<String, Section>,
    pub ignored_sections: Vec<String>,
    pub duplicates: Vec<(String, String)>,
}

/// 消费的节名（小写）。
pub const CONSUMED: [&str; 6] = [
    "general",
    "display",
    "scheme_h1",
    "scheme_h2",
    "scheme_v1",
    "scheme_v2",
];

pub fn parse_ini(text: &str) -> Result<Ini, SsfError> {
    let text = text.trim_start_matches('\u{feff}'); // 容忍双重 BOM（真皮肤见过）
    let mut ini = Ini::default();
    let mut section = String::new();
    let mut in_consumed = false;
    let mut lines = 0usize;
    for raw_line in text.split('\n') {
        lines += 1;
        if lines > MAX_LINES {
            return Err(SsfError::Message("skin.ini 行数超限".into()));
        }
        let line: String = raw_line.trim_matches([' ', '\t', '\r']).to_string();
        if line.len() > MAX_LINE_CHARS || line.contains('\0') {
            return Err(SsfError::Message("skin.ini 存在超长/非法行".into()));
        }
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if !line.ends_with(']') {
                return Err(SsfError::Message("skin.ini 节名格式非法".into()));
            }
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            in_consumed = CONSUMED.contains(&section.as_str());
            if !in_consumed && !ini.ignored_sections.contains(&section) {
                ini.ignored_sections.push(section.clone());
            }
            continue;
        }
        if !in_consumed || section.is_empty() {
            continue; // 未消费节的键直接跳过（含历史遗留垃圾数据）
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(SsfError::Message("skin.ini 键值行格式非法".into()));
        };
        let key = key.trim_matches([' ', '\t']).to_ascii_lowercase();
        let value = value.trim_matches([' ', '\t', '\r']).to_string();
        if key.is_empty() {
            return Err(SsfError::Message("skin.ini 存在空键".into()));
        }
        let pairs = ini.sections.entry(section.clone()).or_default();
        if let Some(existing) = pairs.iter_mut().find(|(k, _)| *k == key) {
            ini.duplicates.push((section.clone(), key.clone()));
            existing.1 = value; // 重复键取后者 + 告警
        } else {
            pairs.push((key, value));
        }
    }
    Ok(ini)
}
