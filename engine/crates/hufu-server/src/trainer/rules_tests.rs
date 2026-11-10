//! Pure Rust fixtures independently transcribed from the recovered algorithm's
//! constrained oracle and verified v4.8.3 instructions. No extracted code or
//! runtime evidence files are loaded.
use super::*;

const SELECTORS: &str = "_234567890";
type Entry<'a> = (&'a str, &'a str, usize, bool);
type Expected<'a> = (usize, usize, &'a str, usize, &'a str);

fn fixture_choices(text: &str, entries: &[Entry<'_>]) -> Vec<Vec<Choice>> {
    let chars: Vec<char> = text.chars().collect();
    let selectors: Vec<char> = SELECTORS.chars().collect();
    let mut choices: Vec<Vec<Choice>> = (0..chars.len()).map(|_| Vec::new()).collect();
    for (start, options) in choices.iter_mut().enumerate() {
        for &(word, code, rank, unique) in entries {
            let word_chars: Vec<char> = word.chars().collect();
            if chars[start..].starts_with(&word_chars) {
                let rank = rank.max(1);
                let tail = format!(
                    "{}{}",
                    "=".repeat((rank - 1) / selectors.len()),
                    selectors[(rank - 1) % selectors.len()]
                );
                options.push(Choice {
                    len: word_chars.len(),
                    hint: Hint {
                        code: Some(code.to_owned()),
                        rank,
                        tail,
                    },
                    unique,
                });
            }
        }
        // The reference enumerates word lengths ascending, preserving the
        // dictionary's order among alternative codes for the same surface.
        options.sort_by_key(|choice| choice.len);
    }
    choices
}

fn check_route(
    text: &str,
    entries: &[Entry<'_>],
    strategy: HintStrategy,
    mode: RouteMode,
    auto_four: bool,
    required: &[RequiredSpan],
    breaks: &[usize],
    expected: &[Expected<'_>],
) {
    let chars: Vec<char> = text.chars().collect();
    let choices = fixture_choices(text, entries);
    let selectors: Vec<char> = SELECTORS.chars().collect();
    let result = route(
        &chars,
        &choices,
        strategy,
        mode,
        auto_four,
        &selectors,
        required,
        breaks,
        || true,
    )
    .expect("oracle fixture must have a complete route");
    let actual: Vec<_> = result
        .iter()
        .map(|step| {
            (
                step.start,
                step.len,
                step.hint.code.as_deref().unwrap_or(""),
                step.hint.rank,
                step.hint.tail.as_str(),
            )
        })
        .collect();
    assert_eq!(actual.as_slice(), expected);
}

#[test]
fn short_first_candidate_at_end_requires_explicit_selection() {
    for mode in [RouteMode::Display, RouteMode::Theory] {
        check_route(
            "甲",
            &[("甲", "a", 1, false)],
            HintStrategy::Shortest,
            mode,
            true,
            &[],
            &[],
            &[(0, 1, "a", 1, "_")],
        );
    }
}

#[test]
fn punctuation_commits_the_preceding_short_first_candidate() {
    for mode in [RouteMode::Display, RouteMode::Theory] {
        check_route(
            "甲，",
            &[("甲", "a", 1, false)],
            HintStrategy::Shortest,
            mode,
            true,
            &[],
            &[],
            &[(0, 1, "a", 1, ""), (1, 1, "", 0, "")],
        );
    }
}

#[test]
fn adjacent_short_codes_both_need_their_selection_key() {
    check_route(
        "甲乙",
        &[("甲", "a", 1, false), ("乙", "b", 1, false)],
        HintStrategy::Shortest,
        RouteMode::Theory,
        true,
        &[],
        &[],
        &[(0, 1, "a", 1, "_"), (1, 1, "b", 1, "_")],
    );
}

#[test]
fn four_code_auto_commit_can_be_disabled() {
    let entries = [("甲", "abcd", 1, false), ("乙", "efgh", 1, false)];
    check_route(
        "甲乙",
        &entries,
        HintStrategy::Shortest,
        RouteMode::Theory,
        true,
        &[],
        &[],
        &[(0, 1, "abcd", 1, ""), (1, 1, "efgh", 1, "")],
    );
    check_route(
        "甲乙",
        &entries,
        HintStrategy::Shortest,
        RouteMode::Theory,
        false,
        &[],
        &[],
        &[(0, 1, "abcd", 1, "_"), (1, 1, "efgh", 1, "_")],
    );
}

#[test]
fn display_keeps_second_candidate_when_its_base_code_is_shorter() {
    let entries = [("甲乙", "abc", 2, false), ("甲乙", "abcd", 1, false)];
    for strategy in [HintStrategy::Shortest, HintStrategy::Phrases] {
        check_route(
            "甲乙",
            &entries,
            strategy,
            RouteMode::Display,
            true,
            &[],
            &[],
            &[(0, 2, "abc", 2, "2")],
        );
    }
    check_route(
        "甲乙",
        &entries,
        HintStrategy::Shortest,
        RouteMode::Theory,
        true,
        &[],
        &[],
        &[(0, 2, "abcd", 1, "")],
    );
}

#[test]
fn paging_selection_cost_precedes_base_code_length_in_both_displays() {
    // Rank22 is a==2 (4 keys), while ab_ needs only 3 keys. This fixture
    // deliberately catches the reference's misleading shortest-mode docstring.
    let entries = [("甲乙", "a", 22, false), ("甲乙", "ab", 1, false)];
    for strategy in [HintStrategy::Shortest, HintStrategy::Phrases] {
        check_route(
            "甲乙",
            &entries,
            strategy,
            RouteMode::Display,
            true,
            &[],
            &[],
            &[(0, 2, "ab", 1, "_")],
        );
    }
    check_route(
        "甲乙",
        &[("甲乙", "a", 22, false)],
        HintStrategy::Shortest,
        RouteMode::Theory,
        true,
        &[],
        &[],
        &[(0, 2, "a", 22, "==2")],
    );
}

#[test]
fn both_displays_preserve_an_isolated_phrase_even_when_singles_are_cheaper() {
    let entries = [
        ("甲乙", "abcdefghij", 1, false),
        ("甲", "a", 1, false),
        ("乙", "b", 1, false),
    ];
    for strategy in [HintStrategy::Shortest, HintStrategy::Phrases] {
        check_route(
            "甲乙",
            &entries,
            strategy,
            RouteMode::Display,
            true,
            &[],
            &[],
            &[(0, 2, "abcdefghij", 1, "")],
        );
    }
    check_route(
        "甲乙",
        &entries,
        HintStrategy::Shortest,
        RouteMode::Theory,
        true,
        &[],
        &[],
        &[(0, 1, "a", 1, "_"), (1, 1, "b", 1, "_")],
    );
}

#[test]
fn alternative_codes_for_one_surface_do_not_create_false_overlap() {
    let entries = [
        ("甲乙", "abcdefghij", 1, false),
        ("甲乙", "abcdefghijkl", 1, false),
        ("甲", "a", 1, false),
        ("乙", "b", 1, false),
    ];
    let choices = fixture_choices("甲乙", &entries);
    assert_eq!(
        isolated(&choices, 2, &[], &[]),
        vec![RequiredSpan { start: 0, len: 2 }]
    );
    check_route(
        "甲乙",
        &entries,
        HintStrategy::Shortest,
        RouteMode::Display,
        true,
        &[],
        &[],
        &[(0, 2, "abcdefghij", 1, "")],
    );
}

#[test]
fn overlapping_words_allow_the_two_display_objectives_to_differ() {
    let entries = [
        ("甲乙丙", "abcdef", 1, false),
        ("甲乙", "abcdefghij", 1, false),
        ("甲", "a", 1, false),
        ("乙", "b", 1, false),
        ("丙", "c", 1, false),
    ];
    check_route(
        "甲乙丙",
        &entries,
        HintStrategy::Phrases,
        RouteMode::Display,
        true,
        &[],
        &[],
        &[(0, 3, "abcdef", 1, "")],
    );
    check_route(
        "甲乙丙",
        &entries,
        HintStrategy::Shortest,
        RouteMode::Display,
        true,
        &[],
        &[],
        &[
            (0, 1, "a", 1, "_"),
            (1, 1, "b", 1, "_"),
            (2, 1, "c", 1, "_"),
        ],
    );
}

#[test]
fn phrase_priority_squares_each_word_before_accumulating_and_changes_equal_key_routes() {
    let entries = [
        ("甲乙丙丁", "abcd", 2, false),
        ("甲乙", "a", 1, false),
        ("丙丁", "bc", 1, false),
    ];
    // Both routes cover four characters and need five keys. The old linear
    // objective chose a_ + bc_ for its smaller base length (3 versus 4).
    // Summing per-word squares now favors abcd2: 4^2 > 2^2 + 2^2.
    check_route(
        "甲乙丙丁",
        &entries,
        HintStrategy::Phrases,
        RouteMode::Display,
        true,
        &[],
        &[],
        &[(0, 4, "abcd", 2, "2")],
    );
    check_route(
        "甲乙丙丁",
        &entries,
        HintStrategy::Shortest,
        RouteMode::Display,
        true,
        &[],
        &[],
        &[(0, 2, "a", 1, "_"), (2, 2, "bc", 1, "_")],
    );
    for strategy in [HintStrategy::Shortest, HintStrategy::Phrases] {
        check_route(
            "甲乙丙丁",
            &entries,
            strategy,
            RouteMode::Theory,
            true,
            &[],
            &[],
            &[(0, 2, "a", 1, "_"), (2, 2, "bc", 1, "_")],
        );
    }
}

#[test]
fn fewer_actual_keys_still_precede_squared_phrase_length() {
    let entries = [
        ("甲乙丙丁", "abcdef", 1, false),
        ("甲乙", "a", 1, false),
        ("丙丁", "bc", 1, false),
    ];
    // The long word scores 16 versus 8, but needs six keys instead of five.
    check_route(
        "甲乙丙丁",
        &entries,
        HintStrategy::Phrases,
        RouteMode::Display,
        true,
        &[],
        &[],
        &[(0, 2, "a", 1, "_"), (2, 2, "bc", 1, "_")],
    );
}

#[test]
fn fewer_missing_characters_still_precede_keys_and_squared_phrase_length() {
    let entries = [
        ("甲乙丙", "a", 1, false),
        ("甲乙", "bc", 1, false),
        ("丙丁", "de", 1, false),
    ];
    // The three-character word plus an unknown 丁 costs only two keys and
    // scores 9, but the complete two-word route has no missing character.
    check_route(
        "甲乙丙丁",
        &entries,
        HintStrategy::Phrases,
        RouteMode::Display,
        true,
        &[],
        &[],
        &[(0, 2, "bc", 1, "_"), (2, 2, "de", 1, "_")],
    );
}

#[test]
fn unknown_ascii_and_literal_punctuation_remain_in_the_route() {
    check_route(
        "汉A，",
        &[],
        HintStrategy::Shortest,
        RouteMode::Theory,
        true,
        &[],
        &[],
        &[(0, 1, "", 0, ""), (1, 1, "", 0, ""), (2, 1, "", 0, "")],
    );
}

#[test]
fn paired_punctuation_commits_a_short_candidate_as_one_literal_step() {
    for mark in ["……", "——"] {
        check_route(
            &format!("甲{mark}"),
            &[("甲", "a", 1, false)],
            HintStrategy::Shortest,
            RouteMode::Theory,
            true,
            &[],
            &[],
            &[(0, 1, "a", 1, ""), (1, 2, "", 0, "")],
        );
    }
}

#[test]
fn selection_or_page_key_cannot_commit_the_preceding_pending_candidate() {
    for code in ["2", "="] {
        check_route(
            "甲，",
            &[("甲", "a", 1, false), ("，", code, 1, true)],
            HintStrategy::Shortest,
            RouteMode::Theory,
            true,
            &[],
            &[],
            &[(0, 1, "a", 1, "_"), (1, 1, code, 1, "")],
        );
    }
}

#[test]
fn dictionary_punctuation_only_auto_commits_when_its_code_is_unique() {
    for (unique, tail) in [(true, ""), (false, "_")] {
        check_route(
            "，甲",
            &[("，", ",", 1, unique), ("甲", "a", 1, false)],
            HintStrategy::Shortest,
            RouteMode::Theory,
            true,
            &[],
            &[],
            &[(0, 1, ",", 1, tail), (1, 1, "a", 1, "_")],
        );
    }
}

#[test]
fn required_span_prevents_splitting_or_merging_across_its_edges() {
    let entries = [
        ("甲乙丙", "abcd", 1, false),
        ("甲乙", "abcdefghij", 1, false),
        ("甲", "a", 1, false),
        ("乙", "b", 1, false),
        ("丙", "c", 1, false),
    ];
    let required = [RequiredSpan { start: 0, len: 2 }];
    for mode in [RouteMode::Theory, RouteMode::Display] {
        check_route(
            "甲乙丙",
            &entries,
            HintStrategy::Shortest,
            mode,
            true,
            &required,
            &[],
            &[(0, 2, "abcdefghij", 1, ""), (2, 1, "c", 1, "_")],
        );
    }
}

#[test]
fn unknown_character_cannot_silently_clear_a_pending_short_candidate() {
    check_route(
        "甲汉",
        &[("甲", "a", 1, false)],
        HintStrategy::Shortest,
        RouteMode::Theory,
        true,
        &[],
        &[],
        &[(0, 1, "a", 1, "_"), (1, 1, "", 0, "")],
    );
}

#[test]
fn paragraph_boundary_cannot_confirm_a_previous_short_candidate() {
    check_route(
        "甲，",
        &[("甲", "a", 1, false)],
        HintStrategy::Shortest,
        RouteMode::Theory,
        true,
        &[],
        &[1],
        &[(0, 1, "a", 1, "_"), (1, 1, "", 0, "")],
    );
}
