//! Word-hint routing reconstructed from the extracted display rules.
//!
//! Display strategies keep isolated phrases intact, then use a pending-candidate
//! DP to resolve overlaps. A short first candidate cannot silently disappear
//! before another code: it must be selected or committed by punctuation.
use super::{Hint, HintStrategy, RequiredSpan, RouteMode};

#[derive(Clone)]
pub(super) struct Choice {
    pub len: usize,
    pub hint: Hint,
    pub unique: bool,
}

#[derive(Clone)]
pub(super) struct Step {
    pub start: usize,
    pub len: usize,
    pub hint: Hint,
}

#[derive(Clone)]
struct Cell {
    cost: [i64; 7],
    previous: Option<(usize, usize, Step)>,
}

fn metric(
    strategy: HintStrategy,
    mode: RouteMode,
    missing: usize,
    keys: usize,
    nonfirst: usize,
    base: usize,
    phrase: usize,
    rank: usize,
) -> [i64; 7] {
    let (m, k, nf, b, p, r) = (
        missing as i64,
        keys as i64,
        nonfirst as i64,
        base as i64,
        phrase as i64,
        rank as i64,
    );
    if mode == RouteMode::Theory {
        return [m, k, nf, 1, 0, 0, 0];
    }
    match strategy {
        // v4.8.3 squares each step's phrase length before update sums the
        // vectors: the route objective is -sum(p_i * p_i), not -sum(p_i)^2.
        HintStrategy::Phrases => [m, k, -p * p, b, r, nf, 1],
        HintStrategy::Shortest => [m, k, b, r, nf, 1, -p],
    }
}

fn punctuation(ch: char) -> bool {
    "，。！？；：、（）【】《》“”‘’—…·".contains(ch)
}

fn can_start(state: usize, first_key: char, is_punctuation: bool, selection_keys: &[char]) -> bool {
    state == 0
        || (!selection_keys.contains(&first_key)
            && first_key != '='
            && (state == 1 || is_punctuation))
}

fn allowed(start: usize, end: usize, locks: &[RequiredSpan], breaks: &[usize]) -> bool {
    let next_break = breaks.partition_point(|&point| point <= start);
    if breaks.get(next_break).is_some_and(|&point| point < end) {
        return false;
    }
    let next_lock = locks.partition_point(|span| span.start + span.len <= start);
    locks.get(next_lock).is_none_or(|span| {
        span.start >= end || (span.start == start && span.start + span.len == end)
    })
}

fn update(
    scores: &mut [[Option<Cell>; 3]],
    end: usize,
    state: usize,
    start: usize,
    old_state: usize,
    delta: [i64; 7],
    step: Step,
) {
    let Some(previous) = &scores[start][old_state] else {
        return;
    };
    let mut cost = previous.cost;
    for (total, amount) in cost.iter_mut().zip(delta) {
        *total += amount;
    }
    if scores[end][state]
        .as_ref()
        .is_none_or(|cell| cost < cell.cost)
    {
        scores[end][state] = Some(Cell {
            cost,
            previous: Some((start, old_state, step)),
        });
    }
}

/// Both display strategies preserve an isolated dictionary phrase. Overlapping
/// phrases remain free for the DP, rather than being resolved greedily.
pub(super) fn isolated(
    choices: &[Vec<Choice>],
    n: usize,
    required: &[RequiredSpan],
    breaks: &[usize],
) -> Vec<RequiredSpan> {
    let mut differences = vec![0isize; n + 1];
    let mut spans = Vec::new();
    for (start, options) in choices.iter().enumerate() {
        let mut seen = std::collections::HashSet::new();
        for option in options {
            // All alternative codes for the same surface count as one span.
            if option.len <= 1 || !seen.insert(option.len) {
                continue;
            }
            let end = start + option.len;
            if !allowed(start, end, required, breaks) {
                continue;
            }
            differences[start] += 1;
            differences[end] -= 1;
            spans.push(RequiredSpan {
                start,
                len: option.len,
            });
        }
    }
    let mut coverage = vec![0isize; n];
    let mut value = 0;
    for (i, point) in coverage.iter_mut().enumerate() {
        value += differences[i];
        *point = value;
    }
    spans.retain(|span| {
        coverage[span.start..span.start + span.len]
            .iter()
            .all(|&count| count == 1)
    });
    spans.extend_from_slice(required);
    spans.sort_by_key(|span| (span.start, span.len));
    spans.dedup();
    spans
}

pub(super) fn route(
    chars: &[char],
    choices: &[Vec<Choice>],
    strategy: HintStrategy,
    mode: RouteMode,
    auto_four: bool,
    selection_keys: &[char],
    required: &[RequiredSpan],
    breaks: &[usize],
    mut budget: impl FnMut() -> bool,
) -> Result<Vec<Step>, &'static str> {
    let n = chars.len();
    let locks = if mode == RouteMode::Display {
        isolated(choices, n, required, breaks)
    } else {
        required.to_vec()
    };
    let mut scores: Vec<[Option<Cell>; 3]> = (0..=n).map(|_| [None, None, None]).collect();
    scores[0][0] = Some(Cell {
        cost: [0; 7],
        previous: None,
    });
    for start in 0..n {
        if !budget() {
            return Err("词提分析预算已用尽，请缩短文章后重试");
        }
        for old_state in 0..3 {
            if scores[start][old_state].is_none() {
                continue;
            }
            // A paragraph starts a fresh IME state; it cannot confirm the
            // preceding paragraph's pending short candidate for free.
            if old_state != 0 && breaks.binary_search(&start).is_ok() {
                continue;
            }
            let ch = chars[start];
            let has_entry = choices[start].iter().any(|entry| entry.len == 1);
            let paired = start + 1 < n && ch == chars[start + 1] && matches!(ch, '…' | '—');
            if paired
                && !choices[start].iter().any(|entry| entry.len == 2)
                && allowed(start, start + 2, &locks, breaks)
                && can_start(old_state, '↑', true, selection_keys)
            {
                update(
                    &mut scores,
                    start + 2,
                    0,
                    start,
                    old_state,
                    metric(strategy, mode, 0, 2, 0, 2, 2, 0),
                    Step {
                        start,
                        len: 2,
                        hint: Hint::missing(),
                    },
                );
            }
            if punctuation(ch) && !has_entry && allowed(start, start + 1, &locks, breaks) {
                let text = ch.to_string();
                let keys = super::literal_key_count(&text).unwrap_or(2);
                let first = match ch {
                    '，' => ',',
                    '。' => '.',
                    '、' => '/',
                    '；' => ';',
                    '【' => '[',
                    '】' => ']',
                    '‘' | '’' => '\'',
                    _ => '↑',
                };
                if can_start(old_state, first, true, selection_keys) {
                    update(
                        &mut scores,
                        start + 1,
                        0,
                        start,
                        old_state,
                        metric(strategy, mode, 0, keys, 0, keys, 0, 0),
                        Step {
                            start,
                            len: 1,
                            hint: Hint::missing(),
                        },
                    );
                }
            } else if ch.is_ascii()
                && (' '..='~').contains(&ch)
                && old_state != 2
                && allowed(start, start + 1, &locks, breaks)
            {
                // Shift is a physical key, so retain the trainer's physical
                // accounting for uppercase and shifted ASCII punctuation.
                let keys = super::literal_key_count(&ch.to_string()).unwrap_or(1);
                update(
                    &mut scores,
                    start + 1,
                    0,
                    start,
                    old_state,
                    metric(strategy, mode, 0, keys, 0, keys, 0, 0),
                    Step {
                        start,
                        len: 1,
                        hint: Hint::missing(),
                    },
                );
            }
            for option in &choices[start] {
                if !budget() {
                    return Err("词提分析预算已用尽，请缩短文章后重试");
                }
                let end = start + option.len;
                if !allowed(start, end, &locks, breaks) {
                    continue;
                }
                let Some(code) = option.hint.code.as_deref() else {
                    continue;
                };
                let is_punctuation = chars[start..end].iter().all(|&c| punctuation(c));
                if !can_start(
                    old_state,
                    code.chars().next().unwrap_or('='),
                    is_punctuation,
                    selection_keys,
                ) {
                    continue;
                }
                let rank = option.hint.rank.max(1);
                let phrase = if option.len > 1 { option.len } else { 0 };
                // Explicit selection is enumerated before a bare first
                // candidate, matching the reference's stable tie handling.
                if !option.hint.tail.is_empty() {
                    update(
                        &mut scores,
                        end,
                        0,
                        start,
                        old_state,
                        metric(
                            strategy,
                            mode,
                            0,
                            code.len() + option.hint.tail.chars().count(),
                            usize::from(rank > 1),
                            code.len(),
                            phrase,
                            rank - 1,
                        ),
                        Step {
                            start,
                            len: option.len,
                            hint: option.hint.clone(),
                        },
                    );
                }
                if rank == 1 {
                    let state = if auto_four && code.len() >= 4 || is_punctuation && option.unique {
                        0
                    } else {
                        2
                    };
                    let mut hint = option.hint.clone();
                    hint.tail.clear();
                    update(
                        &mut scores,
                        end,
                        state,
                        start,
                        old_state,
                        metric(strategy, mode, 0, code.len(), 0, code.len(), phrase, 0),
                        Step {
                            start,
                            len: option.len,
                            hint,
                        },
                    );
                }
            }
            if !punctuation(ch)
                && !(ch.is_ascii() && (' '..='~').contains(&ch))
                && old_state != 2
                && allowed(start, start + 1, &locks, breaks)
            {
                update(
                    &mut scores,
                    start + 1,
                    0,
                    start,
                    old_state,
                    metric(
                        strategy,
                        mode,
                        usize::from(!matches!(ch, '\n' | '\r')),
                        0,
                        0,
                        0,
                        0,
                        0,
                    ),
                    Step {
                        start,
                        len: 1,
                        hint: Hint::missing(),
                    },
                );
            }
        }
    }
    if scores[n][0].is_none() {
        return Err("当前锁定词无法按新词提规则完成，请重新载入词提");
    }
    let (mut position, mut state) = (n, 0);
    let mut result = Vec::new();
    while let Some((start, old_state, step)) = scores[position][state]
        .as_ref()
        .and_then(|cell| cell.previous.clone())
    {
        result.push(step);
        position = start;
        state = old_state;
    }
    result.reverse();
    Ok(result)
}

#[cfg(test)]
#[path = "rules_tests.rs"]
mod tests;
