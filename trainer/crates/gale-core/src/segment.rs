//! 段落切分与跟打文格式解析。
//!
//! 三种输入形态：
//! 1. **跟打码（群文）**：`-----第N段-----` 开头的水印行 + 正文；支持 `《发文》…《难度》…` 晴发文头部。
//! 2. **普通文章**：按目标字数智能切段（段尾回退到句末标点，蓝本疾风1代"智能切段"）。
//! 3. **本地文章库**：Pain 风格 文章/分类/xxx.txt。

use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// 还原胡泊式群文：发文端将正文 Unicode 码点 +1，载入时减回。
/// 与静打 v4.8.3 `qq_bridge.restore_shifted_body` 的静态提取结果对齐：
/// 至少三个密文标点（全角减号/同上号），且不比正常中文标点少，才尝试还原。
/// 只用于已识别群文的正文，标题和段标始终原样保留。
pub fn restore_shifted_body(body: &str) -> Cow<'_, str> {
    let markers = body.chars().filter(|c| matches!(*c, '－' | '〃')).count();
    let normal = body
        .chars()
        .filter(|c| matches!(*c, '，' | '。' | '！' | '？'))
        .count();
    if markers < 3 || markers < normal {
        return Cow::Borrowed(body);
    }

    let mut restored = String::with_capacity(body.len());
    for c in body.chars() {
        if matches!(c, '\r' | '\n' | '\0') {
            restored.push(c);
        } else if let Some(original) = char::from_u32(c as u32 - 1) {
            restored.push(original);
        } else {
            // 码点回退落在代理区时不是合法 Rust 文本，保留整段，避免部分还原。
            return Cow::Borrowed(body);
        }
    }
    if restored
        .chars()
        .filter(|c| matches!(*c, '，' | '。'))
        .count()
        >= markers
    {
        Cow::Owned(restored)
    } else {
        Cow::Borrowed(body)
    }
}

/// 一段跟打文
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    /// 段标记（发文段号 / 书内序号）
    pub mark: String,
    /// 正文（跟打目标）
    pub text: String,
}

/// 跟打码解析结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedGroupText {
    pub title: String,
    pub difficulty: Option<String>,
    pub segments: Vec<Segment>,
}

/// 常见水印/系统行前缀（命中即过滤）
const WATERMARK_PREFIXES: [&str; 6] =
    ["-----", "—----", "史上最专业", "打字群下载", "腾讯", "QQ群"];

/// 噪声子串（行内 contains 命中即整行丢弃，P2' 载文清理用）。
/// 照抄 jifeng article.rs:33-54 NOISE_PATTERNS（20 条）——只作用于**文件/文库载入路径**，
/// 不碰 parse_group_message（比赛正文一个字不能丢，web-research P2 警告）。
pub const NOISE_SUBSTRINGS: &[&str] = &[
    "（求全订）",
    "（求收藏）",
    "（求全订收藏）",
    "（精校版全本）",
    "（校对版全本）",
    "（实体版全本）",
    "[四月天VIP完结]",
    "[新浪vip完结]",
    "——晋江原创网[作品库]",
    "（V文完结）",
    "[潇湘vip完结]",
    "[红袖vip完结]",
    "[腾讯vip完结]",
    "[起点vip完结]",
    "www.",
    "http://",
    "https://",
    "最新章节",
    "本章未完",
    "点击下一页",
];

/// 常见字符替换（带圈数字 → 半角数字，jifeng article.rs:57-78 原样 20 对）。
/// 只作用于文件/文库载入路径（clean_loaded_text）。
pub const CIRCLED_REPLACEMENTS: &[(&str, &str)] = &[
    ("⑴", "1"),
    ("⑵", "2"),
    ("⑶", "3"),
    ("⑷", "4"),
    ("⑸", "5"),
    ("⑹", "6"),
    ("⑺", "7"),
    ("⑻", "8"),
    ("⑼", "9"),
    ("⑽", "10"),
    ("⑾", "11"),
    ("⑿", "12"),
    ("⒀", "13"),
    ("⒁", "14"),
    ("⒂", "15"),
    ("⒃", "16"),
    ("⒄", "17"),
    ("⒅", "18"),
    ("⒆", "19"),
    ("⒇", "20"),
];

/// 文件/文库载入路径的行级清理（jifeng clean_text 口径，article.rs:102-123）：
/// 去空行 → 丢噪声行（contains 子串表）→ 带圈数字替换。
/// **仅在载文链路调用**；群文/赛文正文严禁过此函数（成绩对拍生死线）。
pub fn clean_loaded_text(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    'line: for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        for p in NOISE_SUBSTRINGS {
            if line.contains(p) {
                continue 'line;
            }
        }
        let mut s = line.to_string();
        for (from, to) in CIRCLED_REPLACEMENTS {
            if s.contains(from) {
                s = s.replace(from, to);
            }
        }
        out.push(s);
    }
    out.join("\n")
}

/// 是否为水印/噪声行
pub fn is_watermark_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false; // 空行保留为分段线索，由上层决定
    }
    WATERMARK_PREFIXES.iter().any(|p| t.starts_with(p)) || t.starts_with("第") && t.ends_with("段")
}

/// 解析 `-----第N段-----` 格式的段号（严格：必须两侧都有横线；兼容中文长破折号 —）
pub fn parse_segment_mark(line: &str) -> Option<String> {
    let t = line.trim();
    // 统一把中文长破折号当横线处理
    let t = t.replace('—', "-");
    let stripped = t.trim_matches('-');
    if !t.starts_with('-') || !t.ends_with('-') || stripped.is_empty() {
        return None; // 两侧必须有横线
    }
    // stripped 形如 "第3段" / "第12段"
    let inner = stripped.strip_prefix("第")?.strip_suffix("段")?;
    if inner.chars().all(|c| c.is_ascii_digit()) && !inner.is_empty() {
        return Some(inner.to_string());
    }
    None
}

/// 《X》头部字段提取（晴发文格式：《发文》《普通》《难度》普 1.80）
pub fn extract_header_field(text: &str, name: &str) -> Option<String> {
    let marker = format!("《{name}》");
    let idx = text.find(&marker)?;
    let after = &text[idx + marker.len()..];
    // 值域到下一个《 或行尾
    let end = after.find('《').unwrap_or(after.len());
    let nl = after.find('\n').unwrap_or(after.len());
    let end = end.min(nl);
    Some(after[..end].trim().to_string())
}

/// 晴系群文消息解析结果（jifeng-core group_article.rs 无 regex 移植）：
/// `[普(1.84)]三国演义 [字数500]\n正文\n-----第2-277101段-晴发文`
/// 极速变体尾行：`-----第100000段-{标题}`（伪段号，标题在段标记后，JBS.cs:147）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupArticle {
    pub title: String,
    pub author: String,
    /// 段标记，如 "2-277101" 或 "12"
    pub mark: String,
    /// mark 中可解析出的纯数字段号；形如 "2-277101" 或无法解析时为 0
    pub segment: u32,
    /// 尾行最后一个 "-" 之后的发布者/平台名，如 "晴发文"；极速伪段号时为空（标题已回填）
    pub publisher: String,
    pub content: String,
    /// ("普", 1.84)；首行没有难度信息时为 None
    pub difficulty: Option<(String, f64)>,
    /// E1：正文含非 BMP 字符（U+10000+，emoji/扩展B汉字）——只标记不拒载，
    /// 供 UI/报告层提示"gale 按 char 计字与晴字素簇口径有偏差"。
    pub has_non_bmp: bool,
}

/// 去除零宽/不可见字符：U+200B..=U+200F、U+FEFF、U+2060、U+00AD
pub fn clean_invisible(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !matches!(
                *c as u32,
                0x200B | 0x200E..=0x200F | 0xFEFF | 0x2060 | 0x00AD
            )
        })
        .collect()
}

/// 段号字符集（P1' 勘误：晴系群文存在 `-----第十二段` 中文段号，ASCII-only 会整条解析失败）。
/// 等价 regex `\w+` 的 Unicode 侧：ASCII 字母数字 + `_` + `-` + CJK 统一表意/扩A/兼容表意 + 〇。
/// 注意 `段`（U+6BB5）本身在 CJK 区间内，必须显式排除——否则贪心扫描会吞掉终止符
/// （regex 的 \w+ 有回溯，手写循环没有；web-research P1 明细）。
fn is_mark_char(c: char) -> bool {
    if c == '段' {
        return false;
    }
    c.is_ascii_alphanumeric()
        || c == '_'
        || c == '-'
        || matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x3007)
}

/// 尾行特征在行内的字节区间 Some((start, end))，end 在 "段" 之后。
fn find_tail_marker(line: &str) -> Option<(usize, usize)> {
    let trimmed = line.trim_start();
    let start = line.len() - trimmed.len();
    let dash_bytes = trimmed
        .char_indices()
        .take_while(|(_, c)| matches!(c, '-' | '—'))
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    if trimmed[..dash_bytes].chars().count() < 5 || !trimmed[dash_bytes..].starts_with('第') {
        return None;
    }
    let mut i = start + dash_bytes + '第'.len_utf8();
    let mark_start = i;
    while i < line.len() {
        let c = line[i..].chars().next().unwrap();
        if is_mark_char(c) {
            i += c.len_utf8();
        } else {
            break;
        }
    }
    (i > mark_start && line[i..].starts_with('段')).then_some((start, i + '段'.len_utf8()))
}

/// 是否像一条晴系群文消息（任一行含尾行特征）
pub fn is_group_message(text: &str) -> bool {
    text.lines().any(|l| find_tail_marker(l).is_some())
}

/// 从尾行提取 `第([\w-]+)段` 捕获：返回 (mark, 段标记结束字节位)
fn extract_mark(tail: &str) -> Option<(String, usize)> {
    let idx = tail.find("第")?;
    let mut i = idx + "第".len();
    let start = i;
    while i < tail.len() {
        let c = tail[i..].chars().next().unwrap();
        if is_mark_char(c) {
            i += c.len_utf8();
        } else {
            break;
        }
    }
    if i == start || !tail[i..].starts_with('段') {
        return None;
    }
    Some((tail[start..i].to_string(), i + '段'.len_utf8()))
}

/// 去掉尾缀 ` [字数N]`，返回剩余部分
fn strip_chars_suffix(line: &str) -> &str {
    let t = line.trim_end();
    if !t.ends_with(']') {
        return t;
    }
    let open = match t.rfind("[字数") {
        Some(p) => p,
        None => return t,
    };
    let digits = &t[open + "[字数".len()..t.len() - 1];
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        t[..open].trim_end()
    } else {
        t
    }
}

/// 解析标题行（手写匹配替代 jifeng 三个正则）：
/// `[难度(分数)]标题 [字数N]` / `【难度(分数)】标题` / `【标题】` / `标题 [字数N]` / 纯标题
fn parse_title_line(line: &str) -> (String, Option<(String, f64)>) {
    let line = line.trim();

    // 半角方括号难度前缀：[普(1.84)]标题 [字数500]
    if let Some(diff) = split_diff_prefix(line, '[', ']') {
        return diff;
    }
    // 全角方括号难度前缀：【普(1.84)】标题
    if let Some(diff) = split_diff_prefix(line, '【', '】') {
        return diff;
    }

    let title = strip_chars_suffix(line);
    // 兼容【标题】包裹
    let title = if title.starts_with('【') && title.ends_with('】') && title.chars().count() > 2 {
        title['【'.len_utf8()..title.len() - '】'.len_utf8()].to_string()
    } else {
        title.to_string()
    };
    (title.trim().to_string(), None)
}

/// `前缀普(1.84)后缀` 拆分：open/close 为包裹字符。命中返回 (标题, Some((等级, 分数)))。
fn split_diff_prefix(
    line: &str,
    open: char,
    close: char,
) -> Option<(String, Option<(String, f64)>)> {
    if !line.starts_with(open) {
        return None;
    }
    let close_paren = line.find(')')?;
    // ")" 之后必须紧跟闭合包裹符
    let after = &line[close_paren + 1..];
    if !after.starts_with(close) {
        return None;
    }
    let inner = &line[open.len_utf8()..close_paren];
    // 等级部分（"(" 前）不允许再含包裹符/括号
    let paren = inner.find('(')?;
    let grade = &inner[..paren];
    if grade.is_empty()
        || grade.contains(open)
        || grade.contains(close)
        || grade.contains('(')
        || grade.contains(')')
    {
        return None;
    }
    let score_str = &inner[paren + 1..];
    if score_str.is_empty() || !score_str.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let score: f64 = score_str.parse().ok()?;
    let title = strip_chars_suffix(after[close.len_utf8()..].trim());
    if title.is_empty() {
        return None; // 对齐 jifeng 正则 `(.+?)`：标题至少 1 字符，否则整体不算难度前缀
    }
    Some((title.to_string(), Some((grade.trim().to_string(), score))))
}

/// 标题特征行判定（载文防线）：晴系/文来/极速系发文的**标题行**形态。
/// 用于两种场景：
/// 1. parse_group_message 里"逐条消息合并后首行不是标题"的防御性剥离；
/// 2. 老多段路径把标题行混进正文时的过滤。
/// 特征（各跟打器通用归纳）：
/// - `[难度普(3.52)]…` / `【难度…】…` 难度方括号前缀
/// - `… [字数144]` 字数方括号后缀
/// - `《…》` 纯书名号包裹
/// - `…-普4.54` / `…-水0.19` 文来"标题-难度等级+分值"尾缀
pub fn looks_like_title_line(line: &str) -> bool {
    let t = line.trim();
    if t.chars().count() > 60 {
        return false; // 标题行不会太长（正文行可能很长）
    }
    // 难度方括号前缀：[难度普(3.52)] / [普(1.84)]（带不带"难度"二字都认）
    if t.starts_with('[')
        && t.contains(']')
        && (t.contains("难度") || split_diff_prefix(t, '[', ']').is_some())
    {
        return true;
    }
    if t.starts_with('【') && t.contains('】') && t.contains("难度") {
        return true;
    }
    // 字数后缀：… [字数144]
    if t.ends_with(']') && t.contains("[字数") {
        return true;
    }
    // 纯书名号包裹：《道诡异仙》
    if t.starts_with('《') && t.ends_with('》') && t.chars().count() > 2 {
        return true;
    }
    // 文来尾缀：标题-普4.54 / 标题-水0.19 / 标题-难3.5（中低频，放最后）
    if let Some(pos) = t.rfind('-') {
        let suffix = &t[pos + 1..];
        let mut chars = suffix.chars();
        if let Some(first) = chars.next() {
            if matches!(first, '普' | '水' | '易' | '难' | '中' | '高' | '虚' | '虐') {
                let rest: String = chars.collect();
                if !rest.is_empty()
                    && rest
                        .chars()
                        .next()
                        .map(|c| c.is_ascii_digit())
                        .unwrap_or(false)
                {
                    return true;
                }
            }
        }
    }
    false
}
///
/// 规则：
/// - 按行拆分（丢空行）后找第一条含 `-----第[\w-]+段` 的行作为尾行；
/// - 段标记取尾行 `第([\w-]+)段`；发布者取尾行最后一个 `-` 之后（须在段标记之后）；
/// - **极速伪段号**：段号 100000 时发布者位实为 `{标题}`，回填 title（100000 是极速系
///   写死的哨兵段号，见 _ref JBS.cs:147 / _research/typesunny-qq.md §1.3）；
/// - 尾行之前：首行为标题行，其余为正文（逐行去首尾空白后拼接 + 清不可见字符）；
///   正文为空返回 None。
pub fn parse_group_message(text: &str) -> Option<GroupArticle> {
    let lines: Vec<&str> = text.trim_matches(['\r', '\n']).lines().collect();
    let i = lines.iter().position(|l| find_tail_marker(l).is_some())?;
    if i < 2 {
        // 没有标题行或没有正文
        return None;
    }

    let tail = lines[i].trim();
    let (mark, mark_end) = extract_mark(tail)?;
    let segment = mark.parse::<u32>().unwrap_or(0);
    let mut publisher = String::new();
    if let Some(pos) = tail.rfind('-') {
        // 只有最后一个 "-" 在段标记之后才视为发布者分隔符（对齐 jifeng mark_end_byte 守卫）
        if pos + 1 > mark_end {
            publisher = tail[pos + 1..].trim().to_string();
        }
    }

    // 正文行 = 尾行前的所有行，但**跳过混在开头的标题特征行**——
    // 真实场景：引用回复/消息合并把闲聊行或另一条的标题行拼在开头。
    // 策略：从 lines[1..i] 里把开头连续的标题特征行剔掉（只剔开头——
    // 正文中间出现"标题样"文字概率极低，且误剔正文代价大）。
    let mut body: Vec<&str> = lines[1..i].to_vec();
    // 开头的标题行（最多剔 2 行，防止把整个正文剔光）
    let mut strip = 0usize;
    while strip < 2 && strip < body.len() && looks_like_title_line(body[strip]) {
        strip += 1;
    }
    if strip > 0 && body.len() > strip {
        body.drain(0..strip);
    }
    let content = restore_shifted_body(&body.join("\n")).into_owned();
    if clean_invisible(&content).trim().is_empty() {
        return None;
    }

    let (mut title, mut difficulty) = parse_title_line(lines[0]);
    // 首行不像标题（闲聊/合并消息头部）而正文开头被剔出了标题特征行 →
    // 用被剔的那行作为标题来源（信息更准）
    if strip > 0 && !looks_like_title_line(lines[0]) {
        let (t2, d2) = parse_title_line(lines[1]);
        title = t2;
        if difficulty.is_none() {
            difficulty = d2;
        }
    }

    // 极速伪段号特判：`-----第100000段-{标题}` —— 发布者位是标题不是平台名
    if segment == 100_000 && !publisher.is_empty() {
        title = std::mem::take(&mut publisher);
    }

    Some(GroupArticle {
        title,
        author: String::new(),
        mark,
        segment,
        publisher,
        has_non_bmp: content.chars().any(|c| (c as u32) >= 0x1_0000),
        content,
        difficulty,
    })
}

/// 生成文来平台风格的发文消息（对齐 jifeng format_publish_online）：
/// `[普(1.84)]标题 [字数N]\n正文\n-----第{mark}段-晴发文`
pub fn format_publish_online(
    title: &str,
    content: &str,
    difficulty: Option<(String, f64)>,
    mark: &str,
) -> String {
    let prefix = difficulty
        .map(|(g, s)| format!("[{g}({s:.2})]"))
        .unwrap_or_default();
    if mark.is_empty() {
        return format!(
            "{prefix}{title} [字数{}]\n{content}\n-----晴发文",
            content.chars().count()
        );
    }
    format!(
        "{prefix}{title} [字数{}]\n{content}\n-----第{mark}段-晴发文",
        content.chars().count()
    )
}

/// 生成本地文库同步发文消息（Pro 变体，带全书进度）：
/// `[普(1.84)]西游记 [字数200]\n正文\n-----第3段- 共2974段  进度 400/594800字  本段200字 晴发文`
pub fn format_publish_local(
    title: &str,
    content: &str,
    seg_index: usize,
    seg_total: usize,
    total_chars: usize,
    difficulty: Option<(String, f64)>,
) -> String {
    let prefix = difficulty
        .map(|(g, s)| format!("[{g}({s:.2})]"))
        .unwrap_or_default();
    let seg_total = seg_total.max(1);
    let progress = seg_index * (total_chars / seg_total);
    let count = content.chars().count();
    format!(
        "{prefix}{title} [字数{count}]\n{content}\n-----第{}段- 共{seg_total}段  进度 {progress}/{total_chars}字  本段{count}字 晴发文",
        seg_index + 1
    )
}

/// 解析群文/跟打码全文 → 多段
///
/// 双格式自动识别：
/// - **旧版前置跟打码**（`-----第N段-----` 两侧横线）：多段流式解析（下方主循环）；
/// - **晴系群文消息**（jifeng `parse_group_message` 移植）：首行标题 + 正文 + 尾行 `-----第2-277101段-晴发文`，
///   也支持两侧横线的尾置段标，含极速伪段号特判（段号 100000 时标题在段标记后）。
/// 复制的 CR/CRLF 分行先作结构归一；没有识别出格式时原文保留为单段。
pub fn parse_group_text(raw: &str) -> ParsedGroupText {
    let original_raw = raw;
    let normalized = crate::compare::normalize_newlines(raw);
    let raw = normalized.as_ref();
    let lines: Vec<&str> = raw.lines().collect();
    let strict_marks: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| parse_segment_mark(l).map(|_| i))
        .collect();
    // The same strict marker can precede a legacy body or close a copied QQ
    // message. A title + body before the first marker and no body after the last
    // marker identifies the footer layout; otherwise keep legacy segmentation.
    let strict_footer = strict_marks.first().is_some_and(|&first| {
        lines[..first]
            .iter()
            .filter(|l| !l.trim().is_empty())
            .count()
            >= 2
            && lines[strict_marks.last().unwrap() + 1..]
                .iter()
                .all(|l| l.trim().is_empty())
            && !lines[..first].iter().all(|l| {
                let t = l.trim();
                t.is_empty() || t.starts_with("《发文》") || t.starts_with("《难度》")
            })
    });
    let has_strict = !strict_marks.is_empty() && !strict_footer;
    if !has_strict {
        let tails: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter_map(|(i, l)| find_tail_marker(l).map(|_| i))
            .collect();
        if tails.len() > 1 {
            let mut start = 0;
            let mut articles = Vec::new();
            for end in tails {
                let chunk = lines[start..=end].join("\n");
                if let Some(a) = parse_group_message(chunk.trim_start_matches(['\r', '\n'])) {
                    articles.push(a);
                }
                start = end + 1;
            }
            if let Some(first) = articles.first() {
                return ParsedGroupText {
                    title: first.title.clone(),
                    difficulty: first.difficulty.as_ref().map(|(g, s)| format!("{g} {s}")),
                    segments: articles
                        .iter()
                        .map(|a| Segment {
                            mark: a.mark.clone(),
                            text: a.content.clone(),
                        })
                        .collect(),
                };
            }
        }
        if let Some(a) = parse_group_message(raw) {
            return ParsedGroupText {
                title: a.title,
                difficulty: a.difficulty.map(|(g, s)| format!("{g} {s}")),
                segments: vec![Segment {
                    mark: a.mark,
                    text: a.content,
                }],
            };
        }
        // Unrecognized text is a body, not a format invitation. Do not delete its
        // book-title lines, whitespace or marker-looking examples heuristically.
        return ParsedGroupText {
            title: String::new(),
            difficulty: None,
            segments: if raw.is_empty() {
                Vec::new()
            } else {
                vec![Segment {
                    mark: String::new(),
                    text: original_raw.to_string(),
                }]
            },
        };
    }
    let mut title = String::new();
    let mut difficulty = None;
    if raw.trim_start().starts_with("《") {
        if let Some(v) = extract_header_field(raw, "发文") {
            title = v;
        }
        if let Some(v) = extract_header_field(raw, "难度") {
            difficulty = Some(v);
        }
    }
    let mut segments: Vec<Segment> = Vec::new();
    let mut cur_mark: Option<String> = None;
    let mut cur_lines: Vec<String> = Vec::new();

    let flush = |mark: &mut Option<String>, lines: &mut Vec<String>, out: &mut Vec<Segment>| {
        let body = lines.join("\n");
        if !body.trim().is_empty() {
            out.push(Segment {
                mark: mark.clone().unwrap_or_else(|| (out.len() + 1).to_string()),
                text: restore_shifted_body(&body).into_owned(),
            });
        }
        lines.clear();
    };

    for line in raw.lines() {
        if let Some(mark) = parse_segment_mark(line) {
            flush(&mut cur_mark, &mut cur_lines, &mut segments);
            cur_mark = Some(mark);
        } else if is_watermark_line(line)
            && !line.trim().is_empty()
            && line.trim().starts_with("-----")
        {
            // -----开头但没有段号的水印行：过滤（但 -----第N段----- 已在上面截住）
            continue;
        } else if line.trim().is_empty() {
            if cur_mark.is_some() {
                cur_lines.push(line.to_string());
            }
        } else {
            // 段号行与水印行之外的正文行；但顶部《…》头部行（《发文》《难度》等）跳过
            if segments.is_empty()
                && cur_mark.is_none()
                && cur_lines.is_empty()
                && (line.trim_start().starts_with('《') || line.contains("《难度》"))
            {
                continue;
            }
            cur_lines.push(line.to_string());
        }
    }
    // 尾段 flush
    flush(&mut cur_mark, &mut cur_lines, &mut segments);
    ParsedGroupText {
        title,
        difficulty,
        segments,
    }
}

/// 从一组消息（QQ 消息区逐条正文，顺序 = 显示顺序旧→新）里挑**最新的一条跟打文**。
///
/// 规则（添雨 FormType.cs F4 同款语义，.NET `Regex(@"…-----第\d+段…", RightToLeft)`
/// 的无正则移植）：
/// - 每条消息先按 `is_group_message` 判定是否含跟打文尾行特征（`-----第X段`）；
/// - 群里可能有**历史多条**发文，RightToLeft 语义 = 取**最后一条**（最新）；
/// - 成绩报告行（`第N段 速度…击键…`）没有 `-----` 前缀特征，天然不命中；
/// - 纯闲聊不命中。
/// 返回该条消息原文（调用方再走 parse_group_text → parse_group_message）。
pub fn pick_latest_article<'a>(messages: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let mut latest: Option<&'a str> = None;
    for m in messages {
        if is_group_message(m) {
            latest = Some(m);
        }
    }
    latest
}

/// 智能切段：把长文切成 ~target_chars 的段，段尾尽量回退到句末标点。
/// 句末标点：。！？；…— 和英文 .!?;
pub fn split_smart(raw: &str, target_chars: usize) -> Vec<Segment> {
    let target_chars = target_chars.clamp(1, 100000);
    let chars: Vec<char> = raw.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let sentence_end = ['。', '！', '？', '；', '…', '!', '?', ';', '.'];
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut idx = 1usize;
    while start < chars.len() {
        let target = (start + target_chars).min(chars.len());
        if target >= chars.len() {
            out.push(Segment {
                mark: (out.len() + 1).to_string(),
                text: chars[start..].iter().collect(),
            });
            break;
        }
        // 在 (start + 60%target, target] 内找最后一个句末标点
        let lo = start + (target_chars * 6 / 10);
        let mut cut = target;
        let mut best: Option<usize> = None;
        while idx <= target && idx < chars.len() {
            if sentence_end.contains(&chars[idx - 1]) {
                best = Some(idx);
            }
            idx += 1;
        }
        if let Some(b) = best {
            if b >= lo {
                cut = b;
            }
        }
        // A physical CRLF belongs to one structural newline. Keep its bytes together
        // so Session normalization never invents two newlines across segment bounds.
        if chars[cut - 1] == '\r' && chars.get(cut) == Some(&'\n') {
            if cut > start + 1 {
                cut -= 1;
            } else {
                cut += 1;
            }
        }
        let text: String = chars[start..cut].iter().collect();
        if !text.is_empty() {
            out.push(Segment {
                mark: (out.len() + 1).to_string(),
                text,
            });
        }
        start = cut;
        // 重置扫描游标到新段内
        idx = start + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifted_body_restores_hupo_unicode_vector() {
        // 固定向量同时覆盖汉字、标点、扩展区汉字、emoji、英文、数字和换行。
        let cipher = "佡奾－丗畍〃\r\n冎观－朌双〃𠀁😁!BCD!234\0";
        let plain = "你好，世界。\r\n再见，朋友。𠀀😀 ABC 123\0";
        let restored = restore_shifted_body(cipher);
        assert_eq!(restored, plain);
        assert_eq!(restored.chars().count(), cipher.chars().count());
        assert_eq!(restore_shifted_body(&restored), plain);
    }

    #[test]
    fn shifted_body_keeps_plain_text_and_ambiguous_markers() {
        for plain in [
            "",
            "正常正文，重复标记－〃－，也保留。没有加密！",
            "两个标记－〃不够判定",
            "\u{e000}－〃－", // 回退会落在非法 Unicode 代理区，整段保留。
        ] {
            assert!(matches!(restore_shifted_body(plain), Cow::Borrowed(_)));
            assert_eq!(restore_shifted_body(plain), plain);
        }
        // 静态证据中 markers == normal 仍会还原，避免把报告里的“多于”误作 >。
        assert_eq!(
            restore_shifted_body("－〃－，。！"),
            "，。，\u{ff0b}\u{3001}\u{ff00}"
        );
    }

    #[test]
    fn encrypted_group_body_loads_through_shared_parsers() {
        let cipher = "佡奾－丗畍〃冎观－朌双〃";
        let plain = "你好，世界。再见，朋友。";
        for tail in ["-----第7段-胡泊", "-----第7段-----"] {
            let message = format!("原样标题 [字数12]\n{cipher}\n{tail}");
            let article = parse_group_message(&message).unwrap();
            assert_eq!(article.title, "原样标题");
            assert_eq!(article.mark, "7");
            assert_eq!(article.content, plain);
            let parsed = parse_group_text(&message);
            assert_eq!(parsed.title, "原样标题");
            assert_eq!(parsed.segments[0].text, plain);
        }
        // 旧前置段标和一次复制多条群文也独立解码，普通文本不启发式改写。
        let legacy = format!("《发文》原样标题\n-----第7段-----\n{cipher}");
        assert_eq!(parse_group_text(&legacy).segments[0].text, plain);
        let several =
            format!("甲标题\n{cipher}\n-----第7段-胡泊\n\n乙标题\n正文。\n-----第8段-晴发文");
        let parsed = parse_group_text(&several);
        assert_eq!(parsed.segments[0].text, plain);
        assert_eq!(parsed.segments[1].text, "正文。");
        assert_eq!(parse_group_text(cipher).segments[0].text, cipher);
    }

    #[test]
    fn smart_split_keeps_crlf_pair_and_all_original_bytes() {
        for n in 1..=8 {
            let raw = "甲乙\r\n  丙\r\n\r\n丁";
            let parts = split_smart(raw, n);
            assert_eq!(
                parts.iter().map(|p| p.text.as_str()).collect::<String>(),
                raw
            );
            assert!(!parts
                .windows(2)
                .any(|p| p[0].text.ends_with('\r') && p[1].text.starts_with('\n')));
        }
    }

    #[test]
    fn raw_body_spaces_blank_lines_and_joiners_survive() {
        let raw = "标题\n  第一行  \n\n👩\u{200d}💻\n-----第2-277101段-晴发文";
        let p = parse_group_text(raw);
        assert_eq!(p.segments[0].text, "  第一行  \n\n👩\u{200d}💻");
        let plain = "《这是正文》\n  保留全部  \n\n下一行";
        assert_eq!(parse_group_text(plain).segments[0].text, plain);
    }
    #[test]
    fn several_online_messages_keep_independent_marks() {
        let p = parse_group_text(
            "标题\n甲乙\n-----第2-277101段-晴发文\n\n标题\n丙丁\n-----第3-277102段-晴发文",
        );
        assert_eq!(p.segments.len(), 2);
        assert_eq!(p.segments[0].mark, "2-277101");
        assert_eq!(p.segments[1].mark, "3-277102");
        assert_eq!(p.segments[1].text, "丙丁");
        assert!(!is_group_message(
            "正文中示例 -----第2-277101段-晴发文 仍是正文"
        ));
    }
    #[test]
    fn split_preserves_exact_scalar_stream_including_zero_size() {
        let raw = " 甲乙\n\n 丙丁  👩\u{200d}💻";
        let segments = split_smart(raw, 0);
        assert_eq!(
            segments.iter().map(|s| s.text.as_str()).collect::<String>(),
            raw
        );
        assert_eq!(parse_segment_mark("-----第1段"), None);
        assert_eq!(parse_segment_mark("第1段-----"), None);
    }

    #[test]
    fn mark_parse() {
        assert_eq!(parse_segment_mark("-----第3段-----"), Some("3".into()));
        assert_eq!(parse_segment_mark("—----第12段----—"), Some("12".into()));
        assert_eq!(parse_segment_mark("第3段"), None); // 无横线不是跟打码段号
        assert_eq!(parse_segment_mark("-----第x段-----"), None); // 非数字
        assert_eq!(parse_segment_mark("----- hello -----"), None);
    }

    #[test]
    fn group_text_two_segments() {
        let raw = "《发文》 词亲大人\n《难度》 普 1.80\n-----第1段-----\n今天天气很好。\n我们出去玩吧。\n-----第2段-----\n明天也不错。";
        let p = parse_group_text(raw);
        assert_eq!(p.title, "词亲大人");
        assert_eq!(p.difficulty.as_deref(), Some("普 1.80"));
        assert_eq!(p.segments.len(), 2);
        assert_eq!(p.segments[0].mark, "1");
        assert_eq!(p.segments[0].text, "今天天气很好。\n我们出去玩吧。");
        assert_eq!(p.segments[1].mark, "2");
        assert_eq!(p.segments[1].text, "明天也不错。");
    }

    #[test]
    fn watermark_filtered() {
        let raw = "-----第1段-----\n正文甲\n-----史上最专业的跟打器-----\n正文乙";
        let p = parse_group_text(raw);
        assert_eq!(p.segments.len(), 1);
        assert!(p.segments[0].text.contains("正文甲"));
        assert!(!p.segments[0].text.contains("专业"));
    }

    #[test]
    fn smart_split_snaps_to_sentence() {
        let text = "这是第一句话。这是第二句话！这是第三句话？这是第四句话；这是第五句话。";
        let segs = split_smart(text, 10);
        assert!(!segs.is_empty());
        for s in &segs {
            assert!(s.text.chars().count() <= 13, "段过长: {}", s.text);
            // 每段应以句末标点收尾（最后一段除外也应有）
            assert!(
                s.text.ends_with('。')
                    || s.text.ends_with('！')
                    || s.text.ends_with('？')
                    || s.text.ends_with('；'),
                "段尾: {}",
                s.text
            );
        }
        // 拼回不丢字
        let joined: String = segs
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("");
        let total: usize = segs.iter().map(|s| s.text.chars().count()).sum();
        assert_eq!(total, text.chars().count());
        assert_eq!(joined.chars().count(), text.chars().count());
    }

    #[test]
    fn smart_split_short_text_single() {
        let segs = split_smart("短文。", 50);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "短文。");
    }

    #[test]
    fn smart_split_no_punct_fallback() {
        let segs = split_smart(&"甲".repeat(120), 50);
        assert!(segs.len() >= 2);
        let total: usize = segs.iter().map(|s| s.text.chars().count()).sum();
        assert_eq!(total, 120);
    }

    // ── 晴系群文消息（jifeng group_article.rs 移植锚点，样例对齐其 tests 615-805）──

    #[test]
    fn clean_invisible_removes_zero_width() {
        assert_eq!(
            clean_invisible("a\u{200b}b\u{200c}c\u{200d}d\u{200e}e\u{200f}f"),
            "ab\u{200c}c\u{200d}def"
        );
        assert_eq!(clean_invisible("\u{feff}你\u{2060}好\u{ad}！"), "你好！");
        assert_eq!(clean_invisible("正常文本"), "正常文本");
        assert_eq!(clean_invisible(""), "");
    }

    #[test]
    fn is_group_message_variants() {
        assert!(is_group_message(
            "[普(1.84)]三国演义 [字数500]\n正文\n-----第2-277101段-晴发文"
        ));
        assert!(is_group_message("标题\n正文\n-----第12段-"));
        assert!(is_group_message("闲聊\n-----第3段-晴发文"));
        // 宽松 mark：字母/下划线（"第tdu段"）也算
        assert!(is_group_message("标题\n正文\n-----第tdu段-"));
        // P1'：中文段号（.NET \w 含 Unicode，jifeng regex 天然匹配）
        assert!(is_group_message("标题\n正文\n-----第十二段-晴发文"));
        assert!(!is_group_message("普通聊天消息"));
        assert!(!is_group_message("-----第 段")); // 中间没有标记
        assert!(!is_group_message(""));
    }

    #[test]
    fn parse_chinese_segment_number() {
        // P1'：`-----第十二段` 中文段号——mark 原样保留，数值转换得 0（对齐 jifeng：不做中文数字→数值）
        let msg = "西游记 [字数500]\n正文第一行。\n-----第十二段-晴发文";
        let a = parse_group_message(msg).expect("中文段号应解析成功");
        assert_eq!(a.mark, "十二");
        assert_eq!(a.segment, 0); // parse::<u32>() 失败 → 0（jifeng 同款）
        assert_eq!(a.publisher, "晴发文");
        assert_eq!(a.content, "正文第一行。");
        // 数字 mark 回归防退化
        let b = parse_group_message("标题\n正文。\n-----第2-277101段-晴发文").unwrap();
        assert_eq!(b.mark, "2-277101");
    }

    #[test]
    fn non_bmp_detection() {
        // E1：非 BMP 字符只标记不拒载（gale char 口径 vs 晴字素簇口径的已知偏差）
        let a = parse_group_message("标题\n正文。\n-----第1段-晴发文").unwrap();
        assert!(!a.has_non_bmp);
        // 𠀍 = U+2000D（CJK 扩展B）；emoji 😀 = U+1F600
        let b = parse_group_message("标题\n𠀍字打头。\n-----第1段-晴发文").unwrap();
        assert!(b.has_non_bmp);
        let c = parse_group_message("标题\n带😀的正文。\n-----第1段-晴发文").unwrap();
        assert!(c.has_non_bmp);
        // Comparer 侧同款探测
        let cmp = crate::compare::Comparer::new("普通正文");
        assert!(!cmp.has_non_bmp);
        let cmp2 = crate::compare::Comparer::new("含𠀍正文");
        assert!(cmp2.has_non_bmp);
    }

    #[test]
    fn clean_loaded_text_filters_and_replaces() {
        // P2'：噪声子串 contains 整行丢（前缀表查不到的行中水印必须能过滤）
        let src = "正文第一行。
（求全订）这是水印行
正文（求收藏）行中水印也整行丢
——晋江原创网[作品库]欢迎阅读
www.example.com 最新章节
本章未完，点击下一页
⑴圈号开头的内容⒇结束";
        let cleaned = clean_loaded_text(src);
        assert!(cleaned.contains("正文第一行。"), "{cleaned}");
        assert!(!cleaned.contains("求全订"), "{cleaned}");
        assert!(!cleaned.contains("行中水印"), "{cleaned}"); // contains 语义：行中命中也丢
        assert!(!cleaned.contains("晋江"), "{cleaned}");
        assert!(!cleaned.contains("www."), "{cleaned}");
        assert!(!cleaned.contains("本章未完"), "{cleaned}");
        // 带圈数字 → 半角
        assert!(cleaned.contains("1圈号开头的内容20结束"), "{cleaned}");
        // 空行剔除
        assert!(!cleaned.contains("\n\n"), "{cleaned}");
    }

    #[test]
    fn parse_full_online_message() {
        let msg =
            "[普(1.84)]三国演义 [字数500]\n第一行正文。\n第二行正文。\n-----第2-277101段-晴发文";
        let a = parse_group_message(msg).expect("应解析成功");
        assert_eq!(a.title, "三国演义");
        assert_eq!(a.difficulty, Some(("普".into(), 1.84)));
        assert_eq!(a.mark, "2-277101");
        assert_eq!(a.segment, 0); // "2-277101" 解析不了就 0
        assert_eq!(a.publisher, "晴发文");
        assert_eq!(a.content, "第一行正文。\n第二行正文。");
        assert!(a.author.is_empty());
    }

    #[test]
    fn parse_numeric_segment() {
        let msg = "标题 [字数200]\n正文内容。\n-----第12段-";
        let a = parse_group_message(msg).expect("应解析成功");
        assert_eq!(a.title, "标题");
        assert_eq!(a.difficulty, None);
        assert_eq!(a.mark, "12");
        assert_eq!(a.segment, 12);
        assert_eq!(a.publisher, ""); // 尾行最后是 "-"，发布者为空
        assert_eq!(a.content, "正文内容。");
    }

    #[test]
    fn parse_title_variants() {
        // 【】包裹标题
        let a = parse_group_message("【西游记】\n正文。\n-----第1段-晴发文").unwrap();
        assert_eq!(a.title, "西游记");
        assert_eq!(a.difficulty, None);
        // 整行当标题
        let a = parse_group_message("裸标题没有修饰\n正文。\n-----第1段-晴发文").unwrap();
        assert_eq!(a.title, "裸标题没有修饰");
        // 全角难度前缀
        let a = parse_group_message("【普(1.84)】三国演义\n正文。\n-----第1段-晴发文").unwrap();
        assert_eq!(a.title, "三国演义");
        assert_eq!(a.difficulty, Some(("普".into(), 1.84)));
    }

    #[test]
    fn parse_rejects() {
        // 没有特征行
        assert!(parse_group_message("普通文本\n两行").is_none());
        // 尾行就是第一行（没有正文）
        assert!(parse_group_message("-----第1段-晴发文").is_none());
        // 只有标题行 + 尾行，正文为空
        assert!(parse_group_message("标题\n-----第1段-晴发文").is_none());
        // 正文全被清空
        assert!(parse_group_message("标题\n\u{200b}\n-----第1段-晴发文").is_none());
    }

    #[test]
    fn parse_content_preserves_raw_for_explicit_cleaning() {
        let msg = "标题\n\u{3000} 行1 \t\n行2\u{200b}\n-----第1段-晴发文";
        let a = parse_group_message(msg).unwrap();
        assert_eq!(a.content, "\u{3000} 行1 \t\n行2\u{200b}");
    }

    #[test]
    fn parse_jisu_fake_segment_100000() {
        // 极速系伪段号：-----第100000段-{标题}（JBS.cs:147）——发布者位是标题
        let msg = "极速杯每日赛文\n正文内容。\r\n-----第100000段-极速跟打器每日一赛";
        let a = parse_group_message(msg).expect("应解析成功");
        assert_eq!(a.mark, "100000");
        assert_eq!(a.segment, 100_000);
        assert_eq!(a.title, "极速跟打器每日一赛"); // 标题回填，不是发布者
        assert_eq!(a.publisher, ""); // 发布者已让渡给标题
        assert_eq!(a.content, "正文内容。");
        // parse_group_text 集成：走晴系路径
        let p = parse_group_text(msg);
        assert_eq!(p.title, "极速跟打器每日一赛");
        assert_eq!(p.segments.len(), 1);
        assert_eq!(p.segments[0].mark, "100000");
    }

    #[test]
    fn group_text_routes_sunny_message() {
        // 无严格 `-----第N段-----` 行 → 晴系消息路径
        let raw = "[普(1.84)]三国演义 [字数500]\n正文。\n-----第2-277101段-晴发文";
        let p = parse_group_text(raw);
        assert_eq!(p.title, "三国演义");
        assert_eq!(p.difficulty.as_deref(), Some("普 1.84"));
        assert_eq!(p.segments.len(), 1);
        assert_eq!(p.segments[0].mark, "2-277101");
        assert_eq!(p.segments[0].text, "正文。");
    }

    #[test]
    fn copied_strict_footer_strips_title_and_keeps_body() {
        for newline in ["\n", "\r\n", "\r"] {
            for marker in ["-----第12段-----", "—----第12段----—"] {
                let raw = [
                    "[普(1.84)]三国演义 [字数500]",
                    "  第一行  ",
                    "",
                    "👩\u{200d}💻正文。",
                    marker,
                ]
                .join(newline);
                let p = parse_group_text(&raw);
                assert_eq!(p.title, "三国演义", "{raw:?}");
                assert_eq!(p.difficulty.as_deref(), Some("普 1.84"));
                assert_eq!(p.segments.len(), 1);
                assert_eq!(p.segments[0].mark, "12");
                assert_eq!(p.segments[0].text, "  第一行  \n\n👩\u{200d}💻正文。");
            }
        }
    }

    #[test]
    fn copied_strict_footer_messages_keep_separate_bodies() {
        let p = parse_group_text("甲标题\n甲乙\n-----第2段-----\n\n乙标题\n丙丁\n-----第3段-----");
        assert_eq!(p.title, "甲标题");
        assert_eq!(p.segments.len(), 2);
        assert_eq!(p.segments[0].mark, "2");
        assert_eq!(p.segments[0].text, "甲乙");
        assert_eq!(p.segments[1].mark, "3");
        assert_eq!(p.segments[1].text, "丙丁");
    }

    #[test]
    fn plain_copied_text_is_not_stripped_as_a_title() {
        for raw in [
            "《这是正文》\r  正文原样  \r\r下一行",
            "[普(1.84)]正文中的格式示例\n下一行也保留",
            "正文中的 -----第1段----- 示例\n  继续正文  ",
        ] {
            let p = parse_group_text(raw);
            assert!(p.title.is_empty());
            assert_eq!(p.segments.len(), 1);
            assert!(p.segments[0].mark.is_empty());
            assert_eq!(p.segments[0].text, raw);
        }
    }

    #[test]
    fn group_text_strict_multi_segment_still_works() {
        // 旧版严格多段格式不被晴系路径劫持
        let raw = "-----第1段-----\n甲乙\n-----第2段-----\n丙丁";
        let p = parse_group_text(raw);
        assert_eq!(p.segments.len(), 2);
        assert_eq!(p.segments[0].text, "甲乙");
        assert_eq!(p.segments[1].text, "丙丁");
    }

    #[test]
    fn merged_message_leading_chatter_and_title_stripped() {
        // 引用/合并场景：闲聊 + 晴系标题行混在正文开头 → 标题行剥离，闲聊行成为首行（标题）
        let merged =
            "感谢发文\n[难度普(3.52)]《道诡异仙》 [字数144]\n咕咕～正文。\n-----第5105段-晴发文";
        let a = parse_group_message(merged).expect("应解析");
        assert!(
            !a.content.contains("[难度"),
            "标题行必须从正文剥离：{}",
            a.content
        );
        assert!(!a.content.contains("[字数"));
        assert!(a.content.contains("咕咕～正文"));
        // 标题回填为被剔的标题行
        assert!(a.title.contains("道诡异仙"), "title={}", a.title);
    }

    #[test]
    fn merged_message_title_only_leading() {
        // 只有标题行混在开头（无闲聊头）：lines[0] 本身就是标题行 → 正常剥离
        let merged = "[难度普(3.52)]《道诡异仙》 [字数144]\n咕咕～正文。\n-----第5105段-晴发文";
        let a = parse_group_message(merged).expect("应解析");
        assert_eq!(a.title, "《道诡异仙》");
        assert_eq!(a.content, "咕咕～正文。");
    }

    #[test]
    fn looks_like_title_line_forms() {
        assert!(looks_like_title_line(
            "[难度普(3.52)]《道诡异仙》 [字数144]"
        ));
        assert!(looks_like_title_line("[普(1.84)]三国演义 [字数500]"));
        assert!(looks_like_title_line("《红楼梦》"));
        assert!(looks_like_title_line("飞狐外传（45万字）-普4.54"));
        assert!(looks_like_title_line("明朝那些事节选赏银一千两-水0.19"));
        assert!(looks_like_title_line("三国演义 [字数2974]"));
        // 非标题
        assert!(!looks_like_title_line(
            "咕咕～”熟悉的声音让李火旺下意识地抬头看向房梁"
        ));
        assert!(!looks_like_title_line(
            "正文-普通的一行字，带个-横线但尾缀不对"
        ));
        assert!(!looks_like_title_line("，动不动就是“赏银一千两”然后一个太监拿着一个放满银两的盘子走到大臣面前，大臣谢恩后拿钱回家。大致过程也是如此，但很多时候，电视剧的导演可能没有考虑过一千两银子的真实重量的问题，因为一千两银子有六十二斤半，合三十一公斤。要是一个大臣把三十一公斤的东西挂在脖子上…"));
    }

    #[test]
    fn pick_latest_article_takes_last_match() {
        let msgs = [
            "闲聊今天天气",                                                       // 噪声
            "[普(1.84)]三国演义 [字数500]\n旧的正文。\n-----第2-277101段-晴发文", // 旧文
            "第55569段 速度158.15 击键6.69 码长2.54 键准96.28% 字数209", // 成绩报告（无特征）
            "[普(2.00)]西游记 [字数300]\n新的正文。\n-----第3-277102段-晴发文", // 最新文
            "又一条闲聊",
        ];
        let got = pick_latest_article(msgs).expect("应命中");
        assert!(got.contains("新的正文"));
        assert!(got.contains("第3-277102段"));
        // 全是闲聊 → None
        assert!(pick_latest_article(["闲聊", "第999段 速度139.94 击键5.83"]).is_none());
    }

    #[test]
    fn format_publish_online_matches_jifeng() {
        let s = format_publish_online("三国演义", "正文", Some(("普".into(), 1.84)), "2-277101");
        assert_eq!(
            s,
            "[普(1.84)]三国演义 [字数2]\n正文\n-----第2-277101段-晴发文"
        );
        // 无难度
        let s = format_publish_online("标题", "正文内容", None, "3");
        assert_eq!(s, "标题 [字数4]\n正文内容\n-----第3段-晴发文");
        // 空 mark 用 "1"
        let s = format_publish_online("标题", "正文", None, "");
        assert_eq!(s, "标题 [字数2]\n正文\n-----晴发文");
    }

    #[test]
    fn format_publish_local_matches_pro_format() {
        // 对齐 Pro 原格式：-----第3段- 共2974段  进度 400/594800字  本段200字 晴发文
        let content = "字".repeat(200);
        let s = format_publish_local("西游记", &content, 2, 2974, 594800, None);
        assert_eq!(
            s,
            format!("西游记 [字数200]\n{content}\n-----第3段- 共2974段  进度 400/594800字  本段200字 晴发文")
        );
        // 带难度
        let s = format_publish_local("西游记", "正文", 0, 10, 2000, Some(("易".into(), 0.5)));
        assert_eq!(
            s,
            "[易(0.50)]西游记 [字数2]\n正文\n-----第1段- 共10段  进度 0/2000字  本段2字 晴发文"
        );
    }
}
