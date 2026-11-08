//! Probability memoization must only remove repeated LM work: all observable
//! values, floating point bits, and incremental search state stay unchanged.
use super::*;
use hufu_dict::entry::DictEntry;
use hufu_dict::supplement::SupplementEntry;

fn dict(entries: &[(&str, &str)]) -> Arc<Dict> {
    Arc::new(Dict::from_entries("prob-cache-test", entries.iter().enumerate()
        .map(|(i, &(code, text))| DictEntry::new(code, text, i as u32)).collect()))
}

fn engine(entries: &[(&str, &str)], weights: SentenceWeights) -> SentenceEngine {
    let supplement = Supplement { entries: vec![
        SupplementEntry { word: "甲乙".into(), weight: 2000.0 },
        SupplementEntry { word: "丙丁戊".into(), weight: 8000.0 },
    ] };
    let mut engine = SentenceEngine::with_lm(
        Lm::Five(model03::probability_cache_test_model()), dict(entries), &supplement, weights,
    );
    engine.two_stage = false;
    engine
}

fn standard_engine() -> SentenceEngine {
    engine(&[
        ("aa", "甲"), ("aaa", "甲"), ("aaa", "己"), ("aaaa", "甲甲"),
        ("aabb", "甲乙"), ("bb", "乙"), ("b", "乙"), ("cc", "丙"), ("c", "丙"),
        ("dd", "丁"), ("ee", "戊"), ("ff", "己"), ("gg", "庚"), ("hh", "辛"),
        ("ii", "壬"), ("jj", "癸"), ("kk", "𠀀"), ("uu", "外"),
        ("rr", "甲"), ("rr", "乙"), ("rr", "丙"), ("rr", "丁"), ("rr", "戊"),
        ("rr", "己"), ("rr", "庚"), ("rr", "辛"), ("rr", "壬"), ("rr", "癸"),
    ], SentenceWeights::default())
}

fn five(engine: &SentenceEngine) -> &FivegramModel {
    match &engine.model { Lm::Five(model) => model, _ => panic!("expected NM03 fixture") }
}

fn assert_hit_eq(actual: &SentenceHit, expected: &SentenceHit, context: &str) {
    assert_eq!(actual.text, expected.text, "{context}: text");
    assert_eq!(actual.score.to_bits(), expected.score.to_bits(), "{context}: score: {actual:?} / {expected:?}");
    assert_eq!(actual.confidence.to_bits(), expected.confidence.to_bits(), "{context}: confidence: {actual:?} / {expected:?}");
    assert_eq!((actual.max_rank, actual.sum_rank, actual.exact, &actual.word_ends,
        &actual.segmented, actual.partial, actual.implicit2),
        (expected.max_rank, expected.sum_rank, expected.exact, &expected.word_ends,
        &expected.segmented, expected.partial, expected.implicit2), "{context}: hit metadata");
}

fn assert_decode_eq(actual: &SentenceDecode, expected: &SentenceDecode, context: &str) {
    assert_eq!((actual.truncated, actual.early_truncated), (expected.truncated, expected.early_truncated), "{context}: truncation");
    for (name, a, b) in [("hits", &actual.hits, &expected.hits),
                         ("early_hits", &actual.early_hits, &expected.early_hits)] {
        assert_eq!(a.len(), b.len(), "{context}: {name} length");
        for (i, (a, b)) in a.iter().zip(b).enumerate() {
            assert_hit_eq(a, b, &format!("{context}: {name}[{i}]"));
        }
    }
}

fn assert_state_eq(a: &St, b: &St, context: &str) {
    assert_eq!((a.history(), &a.text, a.max_rank, a.sum_rank, a.exact, &a.word_ends,
        a.supp_state, a.implicit2),
        (b.history(), &b.text, b.max_rank, b.sum_rank, b.exact, &b.word_ends,
        b.supp_state, b.implicit2), "{context}: state metadata");
    assert_eq!((a.score.to_bits(), a.mass.to_bits(), a.supp_bonus.to_bits()),
        (b.score.to_bits(), b.mass.to_bits(), b.supp_bonus.to_bits()), "{context}: state scores");
}

fn assert_buckets_eq(a: &Option<Vec<Bucket>>, b: &Option<Vec<Bucket>>, context: &str) {
    match (a, b) {
        (None, None) => {},
        (Some(a), Some(b)) => {
            assert_eq!(a.len(), b.len(), "{context}: bucket count");
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                assert_eq!(a.truncated, b.truncated, "{context}: bucket {i} truncation");
                assert_eq!(a.order, b.order, "{context}: bucket {i} traversal order");
                assert_eq!(a.best.len(), b.best.len(), "{context}: bucket {i} best count");
                assert_eq!(a.mass.len(), b.mass.len(), "{context}: bucket {i} mass count");
                for (text, state) in &a.best {
                    assert_state_eq(state, b.best.get(text).expect("same best keys"), &format!("{context}: bucket {i}: {text}"));
                }
                for (text, mass) in &a.mass {
                    assert_eq!(mass.to_bits(), b.mass.get(text).expect("same mass keys").to_bits(), "{context}: bucket {i}: {text} mass");
                }
            }
        },
        _ => panic!("{context}: bucket presence differs"),
    }
}

fn compare_full(engine: &SentenceEngine, raw: &str, quick: bool) -> (SentenceDecode, Option<Vec<Bucket>>) {
    let (actual, a_buckets, a_bound) = engine.decode_internal::<true>(raw, None, quick);
    let (expected, b_buckets, b_bound) = engine.decode_internal::<false>(raw, None, quick);
    assert_decode_eq(&actual, &expected, raw);
    assert_buckets_eq(&a_buckets, &b_buckets, raw);
    assert_eq!(a_bound, b_bound, "{raw}: quick bound");
    (actual, a_buckets)
}

#[test]
fn probability_fixture_exercises_four_history_positions_and_special_tokens() {
    let m = model03::probability_cache_test_model();
    let h1 = ['甲' as u32, '乙' as u32, '丙' as u32, '丁' as u32];
    let h2 = ['己' as u32, '乙' as u32, '丙' as u32, '丁' as u32];
    let p1 = m.step_prob(&h1, '戊' as u32);
    let p2 = m.step_prob(&h2, '戊' as u32);
    assert!((p1 - 10f64.powf(-1.5)).abs() < 1e-12);
    assert!((p2 - 10f64.powf(-3.0)).abs() < 1e-12);
    assert_ne!(p1.to_bits(), p2.to_bits(), "oldest history must participate in the cache key");
    assert_ne!(m.step_prob(&h1, EOS).to_bits(), m.step_prob(&h2, EOS).to_bits());
    assert_ne!(m.step_prob(&[BOS; 4], '甲' as u32).to_bits(), m.step_prob(&[], '甲' as u32).to_bits());
    assert_eq!(m.id_of('外' as u32), m.id_of('界' as u32));
    assert_eq!(m.step_prob(&h1, '外' as u32).to_bits(), m.step_prob(&h1, '界' as u32).to_bits());
    assert!(m.step_prob(&h1, '外' as u32) < m.step_prob(&[], '外' as u32), "missing successors exercise backoff weights");
    assert_ne!(m.id_of('𠀀' as u32), m.id_of('外' as u32));
}

#[test]
fn probability_cache_preserves_full_decode_bits_and_partial_tails() {
    let engine = standard_engine();
    for raw in ["", "aa", "aaa", "aabb", "aabbccdd", "aabbccddeeffgghh",
        "aabbccddeeffgghhb", "aaabbccddeeffgghhii", "aaabbccddeeffgghhiijjkkuu",
        "aabbaabbaabbaabbaabb", "aabbccddeeffgghhiijjkkuub", "invalid",
    ] {
        compare_full(&engine, raw, false);
    }
    let (partial, _) = compare_full(&engine, "aabbccddeeffgghhb", false);
    assert!(partial.early_hits.iter().any(|h| h.partial), "partial EOS path must run");
    let (fivegram, _) = compare_full(&engine, "aaabbccddeeffgghhii", false);
    assert!(fivegram.hits.iter().any(|h| h.text.starts_with("甲乙丙丁戊")));
    assert!(fivegram.hits.iter().any(|h| h.text.starts_with("己乙丙丁戊") && h.implicit2));
    compare_full(&engine, &"aa".repeat(65), false); // max_raw_length rejection
}

#[test]
fn probability_cache_reuses_queries_but_keeps_short_decode_bypass() {
    let engine = standard_engine();
    let model = five(&engine);
    for raw in ["aabbccddeeffgghh", "aabbccddeeffgghhb", "aaabbccddeeffgghhiijj"] {
        model.reset_test_queries();
        engine.decode_internal::<false>(raw, None, false);
        let original = model.test_queries();
        model.reset_test_queries();
        engine.decode_internal::<true>(raw, None, false);
        let cached = model.test_queries();
        assert!(original > 0);
        if raw.len() <= 16 {
            assert_eq!(cached, original, "short decode must bypass hash-map work");
        } else {
            assert!(cached < original, "real cache hits expected: {raw}: {cached} / {original}");
        }
        // A second identical decode has the same misses: memo lifetime is one
        // decode, and cannot reuse probabilities across model/word reloads.
        model.reset_test_queries();
        engine.decode_internal::<true>(raw, None, false);
        assert_eq!(model.test_queries(), cached);
    }
}

#[test]
fn probability_cache_bypasses_very_narrow_beams() {
    let raw = "aabbccddeeffgghhii";
    for beam in 1..=3 {
        for quick in [false, true] {
            let mut engine = standard_engine();
            if quick { engine.quick_beam = beam; } else { engine.weights.beam_width = beam; }
            let model = five(&engine);
            model.reset_test_queries();
            let (expected, b, bound_b) = engine.decode_internal::<false>(raw, None, quick);
            let original = model.test_queries();
            model.reset_test_queries();
            let (actual, a, bound_a) = engine.decode_internal::<true>(raw, None, quick);
            assert_eq!(model.test_queries(), original, "narrow beam must bypass memoization");
            assert_decode_eq(&actual, &expected, "narrow beam");
            assert_buckets_eq(&a, &b, "narrow beam");
            assert_eq!(bound_a, bound_b);
        }
    }
}

#[test]
fn probability_cache_preserves_same_text_mass_and_quick_pruning() {
    let weights = SentenceWeights { beam_width: 64, candidate_limit: 8, ..Default::default() };
    let mut engine = SentenceEngine::with_lm(
        Lm::Five(model03::probability_cache_test_model()),
        dict(&[("aa", "甲"), ("aaaa", "甲甲")]), &Supplement::default(), weights,
    );
    let (_, buckets) = compare_full(&engine, &"aa".repeat(10), false);
    let final_bucket = buckets.as_ref().unwrap().last().unwrap();
    let state = final_bucket.best.get(&"甲".repeat(10)).unwrap();
    assert!(state.mass > state.score, "distinct segmentations must contribute mass to the same text");
    engine.set_user_words(["乙", "丙", "丁", "戊"].into_iter()
        .map(|text| ("aa".into(), text.into())).collect());
    engine.quick_beam = 4;
    let (_, buckets) = compare_full(&engine, &"aa".repeat(10), true);
    assert!(buckets.as_ref().unwrap().iter().any(|b| b.truncated),
        "quick pruning must actually execute in the intermediate buckets");
    assert!(engine.decode_internal::<true>(&"aa".repeat(10), None, true).2);
}

fn resume(mut buckets: Vec<Bucket>, new_len: usize) -> Option<(Vec<Bucket>, usize, usize)> {
    let split = (buckets.len() - 1).saturating_sub(INC_REDO_TAIL);
    let start = split.saturating_sub(4);
    buckets.truncate(split + 1);
    buckets.resize_with(new_len + 1, Bucket::new);
    Some((buckets, start, split))
}

#[test]
fn probability_cache_preserves_incremental_buckets_and_appended_rank_locks() {
    let engine = standard_engine();
    let raw = "aaabbccddeeffgghhiijjrr";
    let mut a_previous = None;
    let mut b_previous = None;
    for end in 1..=raw.len() {
        let prefix = &raw[..end];
        let a_resume = a_previous.take().and_then(|b| resume(b, end));
        let b_resume = b_previous.take().and_then(|b| resume(b, end));
        let (a, a_buckets, a_bound) = engine.decode_internal::<true>(prefix, a_resume, false);
        let (b, b_buckets, b_bound) = engine.decode_internal::<false>(prefix, b_resume, false);
        assert_decode_eq(&a, &b, prefix);
        assert_buckets_eq(&a_buckets, &b_buckets, prefix);
        assert_eq!(a_bound, b_bound);
        a_previous = a_buckets;
        b_previous = b_buckets;
    }
    // Same-base-length resume is the product's lock-append path.
    let locked = format!("{raw}9");
    let (a, a_buckets, _) = engine.decode_internal::<true>(&locked, resume(a_previous.unwrap(), raw.len()), false);
    let (b, b_buckets, _) = engine.decode_internal::<false>(&locked, resume(b_previous.unwrap(), raw.len()), false);
    assert_decode_eq(&a, &b, &locked);
    assert_buckets_eq(&a_buckets, &b_buckets, &locked);
    assert!(a.hits.iter().any(|hit| hit.text.ends_with('壬') && hit.max_rank == 9));
}

#[test]
fn probability_cache_preserves_explicit_rank_locks_and_digit_codes() {
    let engine = standard_engine();
    for (suffix, expected) in [("9", "壬"), ("0", "癸"), (";", "乙"), ("'", "丙")] {
        let raw = format!("aabbccddeeffgghhrr{suffix}");
        let (out, _) = compare_full(&engine, &raw, false);
        assert!(out.hits.iter().any(|h| h.text.ends_with(expected)), "explicit lock not exercised: {raw}");
    }
    let digit = engine_with_digits();
    for raw in ["a8b8c8d8e8f8g8h8", "a8b8c8d8e8f8g8h8i8", "a8b8c8d8e8f8g8h8i8;"] {
        let (out, _) = compare_full(&digit, raw, false);
        assert!(!out.hits.is_empty(), "numeric code must remain part of a segment: {raw}");
    }
}

fn engine_with_digits() -> SentenceEngine {
    engine(&[("a8", "甲"), ("b8", "乙"), ("c8", "丙"), ("d8", "丁"),
        ("e8", "戊"), ("f8", "己"), ("g8", "庚"), ("h8", "辛"),
        ("i8", "壬"), ("i8", "癸")],
        SentenceWeights { digit_codes: true, ..Default::default() })
}

#[test]
fn probability_cache_capacity_turnover_preserves_all_search_state() {
    let engine = engine(&[("zz", "甲")], SentenceWeights {
        beam_width: 512, candidate_limit: 16, ..Default::default()
    });
    let targets: Vec<char> = "甲乙丙丁戊己庚辛壬癸子丑".chars().collect();
    engine.set_user_words(targets.iter().map(|c| ("zz".into(), c.to_string())).collect());
    let raw = "zz".repeat(9);
    let model = five(&engine);
    model.reset_test_queries();
    let (a, a_buckets, a_bound) = engine.decode_internal::<true>(&raw, None, false);
    let cached = model.test_queries();
    model.reset_test_queries();
    let (b, b_buckets, b_bound) = engine.decode_internal::<false>(&raw, None, false);
    let original = model.test_queries();
    assert_decode_eq(&a, &b, "capacity turnover");
    assert_buckets_eq(&a_buckets, &b_buckets, "capacity turnover");
    assert_eq!(a_bound, b_bound);
    let histories: HashSet<_> = a_buckets.as_ref().unwrap()[6].best.values().map(St::history).collect();
    // Every state in bucket 6 expands to every rank-1 user word at the next
    // position. These alone exceed the memo cap, regardless of earlier events.
    assert!(histories.len() * targets.len() > 4096, "fixture must pass the 4096 unique-event cap");
    assert!(original > 4096);
    assert!(cached < original, "cache must still produce real hits across capacity turnover: {cached} / {original}");
    assert!(a.truncated, "beam truncation must also be compared");
}

#[test]
fn probability_cache_does_not_survive_user_word_reload() {
    let engine = standard_engine();
    let raw = "aabbccddeeffgghhqq";
    assert!(engine.decode_cached(raw).hits.is_empty());
    assert!(engine.cache.lock().unwrap().last.is_some());
    for word in ["新", "词"] {
        engine.set_user_words(vec![("qq".into(), word.into())]);
        {
            let cache = engine.cache.lock().unwrap();
            assert!(cache.last.is_none() && cache.prefix.is_none());
        }
        let actual = engine.decode_cached(raw);
        let expected = engine.decode_internal::<false>(raw, None, false).0;
        assert_decode_eq(&actual, &expected, word);
        assert!(actual.hits.iter().any(|h| h.text.ends_with(word)));
    }
    engine.set_user_words(Vec::new());
    let actual = engine.decode_cached(raw);
    assert!(actual.hits.is_empty());
    assert_decode_eq(&actual, &engine.decode_internal::<false>(raw, None, false).0, "clear user words");
}

#[test]
fn probability_cache_keeps_nm02_and_nm04_unchanged() {
    for quantized in [false, true] {
        let model = trigram_fixture(quantized);
        assert!(model.bigram_prob('甲' as u32, '乙' as u32) > model.unigram_prob('乙' as u32));
        let engine = SentenceEngine::with_model(model,
            dict(&[("aa", "甲"), ("bb", "乙"), ("cc", "丙"), ("b", "乙")]),
            &Supplement::default(), SentenceWeights::default());
        for raw in ["aa", "aabbcc", "aabbccaabbccaabbcc", "aabbccaabbccaabbccb"] {
            compare_full(&engine, raw, false);
        }
    }
}

fn trigram_fixture(quantized: bool) -> NgramModel {
    fn put(d: &mut [u8], at: usize, bytes: &[u8]) { d[at..at + bytes.len()].copy_from_slice(bytes); }
    fn context(d: &mut Vec<u8>, key: i64, target: char, quantized: bool) {
        d.extend_from_slice(&key.to_le_bytes());
        d.extend_from_slice(&0.4f32.to_le_bytes());
        d.extend_from_slice(&1i32.to_le_bytes());
        d.extend_from_slice(&(target as i32).to_le_bytes());
        if quantized { d.push(245); } else { d.extend_from_slice(&0.6f32.to_le_bytes()); }
    }
    let mut d = vec![0; 104];
    d[..8].copy_from_slice(if quantized { b"TCSKNM04" } else { b"TCSKNM02" });
    put(&mut d, 8, &1i32.to_le_bytes());
    put(&mut d, 12, &104i32.to_le_bytes());
    let mut uni = vec![(0u32, 1e-7f32), (EOS, 0.002), ('甲' as u32, 0.5), ('乙' as u32, 0.3), ('丙' as u32, 0.2)];
    uni.sort_by_key(|&(cp, _)| cp);
    for &(cp, prob) in &uni {
        d.extend_from_slice(&(cp as i32).to_le_bytes());
        d.extend_from_slice(&prob.to_le_bytes());
    }
    let bi_block = d.len();
    let bi_key = '甲' as i64;
    context(&mut d, bi_key, '乙', quantized);
    let bi_index = d.len();
    d.extend_from_slice(&bi_key.to_le_bytes());
    d.extend_from_slice(&(bi_block as i64).to_le_bytes());
    let tri_block = d.len();
    let tri_key = ('甲' as i64) * (1 << 21) + '乙' as i64;
    context(&mut d, tri_key, '丙', quantized);
    let tri_index = d.len();
    d.extend_from_slice(&tri_key.to_le_bytes());
    d.extend_from_slice(&(tri_block as i64).to_le_bytes());
    let len = d.len() as i64;
    for (at, value) in [(16, len), (24, 1), (32, uni.len() as i64), (40, 104),
        (56, bi_block as i64), (64, bi_index as i64), (72, 1),
        (88, tri_block as i64), (96, tri_index as i64)] {
        put(&mut d, at, &value.to_le_bytes());
    }
    for at in [48, 52, 80] { put(&mut d, at, &1i32.to_le_bytes()); }
    NgramModel::from_bytes(d).unwrap()
}
