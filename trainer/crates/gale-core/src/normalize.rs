//! 载文标点/引号规范（原跟打器 theory.normalize_quotes 移植，2026-11-12）。
//!
//! 目标：把 PDF/网页复制文里键数统计会失真的写法修成可打字形——
//! 全角数字、破折号变体、中文句内 ASCII 标点、省略号、直引号方向、
//! 引号内句末标点顺序。分段逐段处理时引号状态跨段携带。
//!
//! 只处理正文；标题、段标记等元数据不进本模块。

/// 直引号配对状态，跨段携带（与原版 initial_quote_state 对应）。
#[derive(Debug, Clone, Copy, Default)]
pub struct QuoteState {
    pub double_open: bool,
    pub single_open: bool,
}

const RIGHT_SET: &str = "”’）】》\"'“‘’（）《》【】";

fn is_cjk(c: char) -> bool {
    ('\u{3400}'..='\u{9fff}').contains(&c)
}

/// 前字符是否属于“中文句内”——汉字或中文右引号/右书名号/右括号。
fn prev_is_chinese(prev: Option<char>) -> bool {
    match prev {
        Some(c) => is_cjk(c) || "”’）》】".contains(c),
        None => false,
    }
}

/// 全文规范。返回 (规范后文本, 修改处数)。引号方向状态经 `state` 跨段携带。
pub fn normalize_text(text: &str, state: &mut QuoteState) -> (String, usize) {
    let mut changed = 0usize;

    // ── 第一阶段：全角数字、破折号变体（无条件） ─────────────────────────
    let chars: Vec<char> = text
        .chars()
        .map(|c| match c {
            '０'..='９' => {
                changed += 1;
                char::from_digit(c as u32 - 0xFF10, 10).unwrap_or(c)
            }
            _ => c,
        })
        .collect();
    let chars = fold_dash_variants(chars, &mut changed);

    // ── 第二阶段：中文句内 ASCII/变体标点 → 全角，带前后文条件 ──────────
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        let prev = if i == 0 { None } else { Some(chars[i - 1]) };
        if !prev_is_chinese(prev) {
            out.push(c);
            i += 1;
            continue;
        }
        match c {
            '?' | '？' | '﹖' => {
                changed += usize::from(c != '？');
                out.push('？');
                i += 1;
            }
            '!' | '！' | '﹗' => {
                changed += usize::from(c != '！');
                out.push('！');
                i += 1;
            }
            ':' | '：' | '﹕' => {
                let next = peek_non_space(&chars, i + 1);
                let followed = matches!(
                    next,
                    Some('"') | Some('\'') | Some('“') | Some('‘') | Some('（') | Some('《') | Some('【')
                ) || next.is_some_and(is_cjk);
                if followed {
                    changed += usize::from(c != '：');
                    out.push('：');
                } else {
                    out.push(c);
                }
                i += 1;
            }
            ';' | '；' | '﹔' => {
                let next = peek_non_space(&chars, i + 1);
                let followed = next.map_or(true, |n| RIGHT_SET.contains(n));
                if followed {
                    changed += usize::from(c != '；');
                    out.push('；');
                } else {
                    out.push(c);
                }
                i += 1;
            }
            ',' | '，' | '﹐' | '､' => {
                let next = peek_non_space(&chars, i + 1);
                let followed = next.is_some_and(|n| is_cjk(n) || "“‘（《【".contains(n));
                if followed {
                    changed += usize::from(c != '，');
                    out.push('，');
                } else {
                    out.push(c);
                }
                i += 1;
            }
            '.' | '．' | '｡' => {
                // 原版：`\.{3,}` / `…{3,}`（前瞻右界或文末）→ ……；单点右界 → 。
                if c == '.' {
                    let mut run = 0usize;
                    while i + run < chars.len() && chars[i + run] == '.' {
                        run += 1;
                    }
                    let next = peek_non_space(&chars, i + run);
                    let at_end = next.map_or(true, |n| RIGHT_SET.contains(n));
                    if run >= 3 && at_end {
                        changed += 1;
                        out.push('…');
                        out.push('…');
                        i += run;
                    } else if run == 1 && at_end {
                        changed += 1;
                        out.push('。');
                        i += 1;
                    } else {
                        out.extend_from_slice(&chars[i..i + run]);
                        i += run;
                    }
                } else {
                    let next = peek_non_space(&chars, i + 1);
                    let at_end = next.map_or(true, |n| RIGHT_SET.contains(n));
                    if at_end {
                        changed += 1;
                        out.push('。');
                    } else {
                        out.push(c);
                    }
                    i += 1;
                }
            }
            '…' => {
                let mut run = 0usize;
                while i + run < chars.len() && chars[i + run] == '…' {
                    run += 1;
                }
                let next = peek_non_space(&chars, i + run);
                let at_end = next.map_or(true, |n| RIGHT_SET.contains(n));
                if run >= 3 && at_end {
                    changed += 1;
                    out.push('…');
                    out.push('…');
                    i += run;
                } else {
                    out.extend_from_slice(&chars[i..i + run]);
                    i += run;
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }

    // ── 第三阶段：已知中文标点两侧的 ASCII 空白是排版噪声 ────────────────
    let fullwidth = "，。！？；：、";
    let mut compact: Vec<char> = Vec::with_capacity(out.len());
    let mut idx = 0usize;
    while idx < out.len() {
        let c = out[idx];
        if c == ' ' {
            let prev_ok = compact.last().is_some_and(|p| fullwidth.contains(*p));
            let next_ok = out[idx + 1..]
                .iter()
                .find(|n| **n != ' ')
                .is_some_and(|n| fullwidth.contains(*n));
            if prev_ok || next_ok {
                changed += 1;
                idx += 1;
                continue;
            }
        }
        compact.push(c);
        idx += 1;
    }

    // ── 第四阶段：引号方向 ────────────────────────────────────────────────
    let mapped: Vec<char> = compact
        .into_iter()
        .map(|c| match c {
            '「' | '〝' => {
                changed += 1;
                '“'
            }
            '」' | '〞' => {
                changed += 1;
                '”'
            }
            '『' => {
                changed += 1;
                '‘'
            }
            '』' => {
                changed += 1;
                '’'
            }
            _ => c,
        })
        .collect();
    let mut quoted: Vec<char> = Vec::with_capacity(mapped.len());
    for c in mapped {
        match c {
            '"' | '＂' => {
                changed += 1;
                quoted.push(if state.double_open { '”' } else { '“' });
                state.double_open = !state.double_open;
            }
            '\'' => {
                changed += 1;
                quoted.push(if state.single_open { '’' } else { '‘' });
                state.single_open = !state.single_open;
            }
            _ => quoted.push(c),
        }
    }

    // ── 第五阶段：完整引语句的句末标点收进引号内（“好”。→ “好。”）─────
    let mut final_chars: Vec<char> = Vec::with_capacity(quoted.len());
    let mut idx = 0usize;
    while idx < quoted.len() {
        let c = quoted[idx];
        if (c == '”' || c == '’')
            && matches!(quoted.get(idx + 1), Some('。') | Some('！') | Some('？'))
        {
            changed += 1;
            final_chars.push(quoted[idx + 1]);
            final_chars.push(c);
            idx += 2;
        } else {
            final_chars.push(c);
            idx += 1;
        }
    }

    (final_chars.into_iter().collect(), changed)
}

/// U+2015 横线对、全角连字符对、制表横线对 → ——。
fn fold_dash_variants(chars: Vec<char>, changed: &mut usize) -> Vec<char> {
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        let pair = chars.get(i + 1).copied();
        let is_variant = |x: char| matches!(x, '\u{2015}' | '－' | '─');
        if is_variant(c) && pair.is_some_and(is_variant) {
            *changed += 1;
            out.push('—');
            out.push('—');
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// 从 `from` 起跳过空格后的第一个字符；串尾视为 None（原版前瞻右界）。
fn peek_non_space(chars: &[char], from: usize) -> Option<char> {
    let mut j = from;
    while j < chars.len() && chars[j] == ' ' {
        j += 1;
    }
    chars.get(j).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(text: &str) -> String {
        let mut state = QuoteState::default();
        normalize_text(text, &mut state).0
    }

    #[test]
    fn fullwidth_digits_fold() {
        assert_eq!(norm("第３章"), "第3章");
    }

    #[test]
    fn chinese_context_ascii_punct_becomes_fullwidth() {
        assert_eq!(norm("去哪?"), "去哪？");
        assert_eq!(norm("好啊!"), "好啊！");
        assert_eq!(norm("他说:\"走\""), "他说：“走”");
    }

    #[test]
    fn ascii_punctuation_in_english_stays() {
        assert_eq!(norm("What? Really!"), "What? Really!");
        assert_eq!(norm("3.14 or 1.5"), "3.14 or 1.5");
    }

    #[test]
    fn dots_and_ellipsis() {
        assert_eq!(norm("完了..."), "完了……");
        assert_eq!(norm("完了."), "完了。");
        // 两个点不属于任何规范形，保持原样
        assert_eq!(norm("完了.."), "完了..");
    }

    #[test]
    fn spaces_around_fullwidth_punct_are_layout_noise() {
        assert_eq!(norm("去哪 ？ 走走 。"), "去哪？走走。");
    }

    #[test]
    fn straight_quotes_get_direction_and_carry_across_segments() {
        let mut state = QuoteState::default();
        let (a, _) = normalize_text("他说\"上\"，她说\"下\"", &mut state);
        assert_eq!(a, "他说“上”，她说“下”");
        // 偶数个引号后状态回到关闭
        assert!(!state.double_open);
        // 上一段开启的引号在下一段先闭合
        let (open, _) = normalize_text("他说\"上", &mut state);
        assert_eq!(open, "他说“上");
        assert!(state.double_open);
        let (b, _) = normalize_text("还没说完\"他说", &mut state);
        assert_eq!(b, "还没说完”他说");
        assert!(!state.double_open);
    }

    #[test]
    fn terminal_mark_moves_inside_quotes() {
        assert_eq!(norm("他说\"好\"。"), "他说“好。”");
    }

    #[test]
    fn dash_variants_fold() {
        assert_eq!(norm("一条\u{2015}\u{2015}线"), "一条——线");
        assert_eq!(norm("－－分隔－－"), "——分隔——");
    }

    #[test]
    fn colon_requires_chinese_following() {
        // 冒号后是英文（URL 风格）不转全角
        assert_eq!(norm("网址:https://x"), "网址:https://x");
        // 后跟中文才转
        assert_eq!(norm("如下:事情"), "如下：事情");
    }

    #[test]
    fn unchanged_text_reports_zero() {
        let mut state = QuoteState::default();
        let (text, changed) = normalize_text("正常中文，没有问题。", &mut state);
        assert_eq!(text, "正常中文，没有问题。");
        assert_eq!(changed, 0);
    }
}
