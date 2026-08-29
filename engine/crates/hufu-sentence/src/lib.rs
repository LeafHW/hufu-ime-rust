//! hufu-sentence —— TCSKNM02 ngram 整句引擎。
//!
//! 模型：TigerClaw 生态 `sentence-ngram-*.bin`（明文 TCSKNM02，Kneser-Ney trigram）。
//! 解码：字级 beam search（按 raw 位置分桶，同文本 logsumexp 聚合质量）
//! + 名次惩罚 + 出字奖励 + 终态孤立生僻惩罚（emit 期全文计算）
//! + 补充词奖励（不进质量）；选重后缀锁所在段名次；
//! 不完全尾候选（把尾部未成码的前缀视为「下一词在打」）供提前上屏置信评估。
//! 全流程对齐 Rime 虎整句 tiger_sentence.lua。

pub mod model;

use hufu_config::SentenceWeights;
use hufu_dict::dict::Dict;
use hufu_dict::supplement::Supplement;
use hufu_engine::{parse_rank_locks, SentenceDecoder, SentenceHit, SentenceDecode};
use hufu_types::{Candidate, CandidateKind};
use model::{BOS, EOS, NgramModel};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// 整句引擎。
pub struct SentenceEngine {
    pub model: NgramModel,
    pub dict: Arc<Dict>,
    supplement: HashMap<String, f64>,
    pub weights: SentenceWeights,
    /// 上次解码缓存（raw → 解码结果）
    cache: Mutex<Option<(String, Arc<SentenceDecode>)>>,
}

/// beam 内部状态。
#[derive(Clone)]
struct St {
    prev2: u32,
    prev1: u32,
    text: String,
    /// 排序分（含名次惩罚）
    score: f64,
    /// 同文本聚合质量（logsumexp；不含补充奖励）
    mass: f64,
    max_rank: usize,
    /// 词边界：(累计字数, base 消耗位置)
    word_ends: Vec<(usize, usize)>,
    segmented: String,
}

fn logsumexp(a: f64, b: f64) -> f64 {
    let m = a.max(b);
    m + ((a - m).exp() + (b - m).exp()).ln()
}

/// 分桶：同 text 聚合（Rime ensure_aggregated）；limit 后内容保留可反复读取。
struct Bucket {
    best: HashMap<String, St>,
    mass: HashMap<String, f64>,
    order: Vec<String>,
    truncated: bool,
}

impl Bucket {
    fn new() -> Bucket {
        Bucket {
            best: HashMap::new(),
            mass: HashMap::new(),
            order: Vec::new(),
            truncated: false,
        }
    }

    fn add(&mut self, item: St) {
        let mass_in = item.mass;
        let item_text = item.text.clone();
        match self.best.get(&item_text) {
            Some(prev) => {
                // 已有同文本：质量 logsumexp 累加（limit 清空 mass 表后取 best.mass）
                let prev_mass = self.mass.get(&item_text).copied().unwrap_or(prev.mass);
                let newm = logsumexp(prev_mass, mass_in);
                self.mass.insert(item_text.clone(), newm);
                let dup_better = item.max_rank < prev.max_rank
                    || (item.max_rank == prev.max_rank && item.score > prev.score);
                if dup_better {
                    self.best.insert(item_text.clone(), item);
                }
                if let Some(st) = self.best.get_mut(&item_text) {
                    st.mass = newm;
                }
            }
            None => {
                self.order.push(item_text.clone());
                self.mass.insert(item_text.clone(), mass_in);
                self.best.insert(item_text.clone(), item);
            }
        }
    }

    /// 就地收敛为 top-limit（Rime dedup_limit + states[pos]=current 写回保留）。
    fn limit(&mut self, limit: usize) {
        let mut list: Vec<St> = self
            .order
            .drain(..)
            .filter_map(|t| self.best.remove(&t))
            .collect();
        self.mass.clear();
        if list.len() > limit {
            self.truncated = true;
        }
        list.sort_by(|a, b| {
            a.max_rank
                .cmp(&b.max_rank)
                .then(b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal))
                .then(a.text.cmp(&b.text))
        });
        list.truncate(limit);
        for st in &list {
            self.order.push(st.text.clone());
        }
        for st in list {
            self.best.insert(st.text.clone(), st);
        }
    }

    fn snapshot(&self) -> Vec<St> {
        self.order.iter().filter_map(|t| self.best.get(t).cloned()).collect()
    }
}

fn chars_of(s: &str) -> Vec<char> {
    s.chars().collect()
}

impl SentenceEngine {
    pub fn load(
        model_path: &Path,
        dict: Arc<Dict>,
        supplement: &Supplement,
        weights: SentenceWeights,
    ) -> std::io::Result<SentenceEngine> {
        let model = NgramModel::load(model_path)?;
        Ok(Self::with_model(model, dict, supplement, weights))
    }

    pub fn with_model(
        model: NgramModel,
        dict: Arc<Dict>,
        supplement: &Supplement,
        weights: SentenceWeights,
    ) -> SentenceEngine {
        let supplement = supplement
            .entries
            .iter()
            .map(|e| (e.word.clone(), e.weight))
            .collect();
        SentenceEngine {
            model,
            dict,
            supplement,
            weights,
            cache: Mutex::new(None),
        }
    }

    /// 补充词奖励（对数域；不进质量）。
    fn supplement_reward(&self, word: &str) -> f64 {
        match self.supplement.get(word) {
            Some(w) => (self.weights.supplement_baseline
                + self.weights.supplement_scale * ((w / 1000.0).ln().max(0.0)))
            .min(self.weights.supplement_maximum),
            None => 0.0,
        }
    }

    /// 终态孤立生僻惩罚（emit 期，全文一次）。
    fn isolation_penalty(&self, text: &str) -> f64 {
        let chars = chars_of(text);
        let mut penalty = 0.0;
        for (i, &c) in chars.iter().enumerate() {
            let cp = c as u32;
            if self.model.freq_rank(cp) > self.weights.isolation_threshold {
                let left_hit = i > 0 && self.model.has_bigram(chars[i - 1] as u32, cp);
                let right_hit =
                    i + 1 < chars.len() && self.model.has_bigram(cp, chars[i + 1] as u32);
                if !left_hit && !right_hit {
                    penalty += self.weights.isolation_lambda;
                }
            }
        }
        penalty
    }

    /// 尾部是否为「未完成编码」前缀（Rime incomplete_code_tail）。
    fn incomplete_tail(&self, tail: &[char]) -> bool {
        if tail.is_empty() || !tail.iter().all(|c| c.is_ascii_lowercase()) {
            return false;
        }
        let tail_s: String = tail.iter().collect();
        // 必须是某码的前缀
        let any_prefix = self
            .dict
            .prefix_matches(&tail_s)
            .iter()
            .any(|(len, _)| *len >= tail.len());
        if !any_prefix {
            return false;
        }
        // 长尾不得恰为完整码
        if tail.len() >= 2 {
            let complete = self
                .dict
                .prefix_matches(&tail_s)
                .iter()
                .any(|(len, _)| *len == tail.len());
            if complete {
                return false;
            }
        }
        true
    }

    /// 核心解码（对齐 Rime decode_full + emit + build_early_commit_candidates）。
    fn decode_internal(&self, raw: &str) -> SentenceDecode {
        let parsed = parse_rank_locks(raw);
        let base: Vec<char> = parsed.base.chars().collect();
        let n = base.len();
        let w = &self.weights;
        if n == 0 || n > w.max_raw_length {
            return SentenceDecode {
                hits: Vec::new(),
                truncated: false,
                early_hits: Vec::new(),
                early_truncated: false,
            };
        }

        // 每个位置的编码切分预计算：segs[pos] = Vec<(code_len, entries)>
        let mut segs: Vec<Vec<(usize, Vec<(String, usize)>)>> = vec![Vec::new(); n];
        for pos in 0..n {
            let tail: String = base[pos..].iter().collect();
            for (code_len, idxs) in self.dict.prefix_matches(&tail) {
                if code_len == 0 || pos + code_len > n {
                    continue;
                }
                let entries: Vec<(String, usize)> = idxs
                    .iter()
                    .enumerate()
                    .filter_map(|(rank, &idx)| {
                        self.dict.entries.get(idx as usize).map(|e| (e.text.clone(), rank))
                    })
                    .collect();
                if !entries.is_empty() {
                    segs[pos].push((code_len, entries));
                }
            }
        }

        let max_code_len = segs
            .iter()
            .flatten()
            .map(|(l, _)| *l)
            .max()
            .unwrap_or(1);

        // beam 分桶
        let mut buckets: Vec<Bucket> = (0..=n).map(|_| Bucket::new()).collect();
        buckets[0].add(St {
            prev2: BOS,
            prev1: BOS,
            text: String::new(),
            score: 0.0,
            mass: 0.0,
            max_rank: 1,
            word_ends: Vec::new(),
            segmented: String::new(),
        });

        let allow_all_ranks = n <= 4;
        for pos in 0..n {
            buckets[pos].limit(w.beam_width);
            for state in buckets[pos].snapshot() {
                for (code_len, entries) in &segs[pos] {
                    let end = pos + code_len;
                    // 锁位置必须是段边界：段不得跨越锁终点
                    if parsed.locks.iter().any(|(l, _)| *l > pos && *l < end) {
                        continue;
                    }
                    let lock = parsed.locks.iter().find(|(l, _)| *l == end);
                    // 多码整句时段跨须 ≥2（含选重后缀字符；Rime 段跨规则，
                    // tiger_sentence.lua L562 同款）。【实测 2026-08-31】放开
                    // 此规则（允许一简段入整句）100 句基准 exact 92.93%→
                    // 78.79%、字准 99.39%→98.12%——单码段制造大量噪声路径，
                    // 禁令是质量担当，不可动。「的(u) 窒(eyi)」类编码冲突靠
                    // 提前上屏的边界位置解决（见 tiger_sentence.lua live 版）。
                    let span = end - pos + if lock.is_some() { 1 } else { 0 };
                    if n > 1 && span < 2 {
                        continue;
                    }
                    for (text, rank) in entries {
                        let rank1b = rank + 1; // 码表名次（1 起）
                        if let Some((_, r)) = lock {
                            if rank1b != *r {
                                continue;
                            }
                        } else if !allow_all_ranks && rank1b != 1 {
                            // >4 码无锁只取第 1 候选
                            continue;
                        }
                        let mut ns = state.clone();
                        let prev_before_word = state.prev1;
                        let mut supp_added = 0.0;
                        for c in text.chars() {
                            let cp = c as u32;
                            let p3 = self.model.trigram_prob(ns.prev2, ns.prev1, cp);
                            ns.score += (p3.max(1e-12).ln()) as f64;
                            ns.score += w.emitted_character_reward;
                            ns.mass += (p3.max(1e-12).ln()) as f64 + w.emitted_character_reward;
                            ns.prev2 = ns.prev1;
                            ns.prev1 = cp;
                            let _ = prev_before_word;
                        }
                        if rank1b > 1 {
                            let pen = w.rank_penalty * (rank1b as f64).ln();
                            ns.score -= pen;
                            ns.mass -= pen;
                        }
                        let supp = self.supplement_reward(text);
                        ns.score += supp;
                        supp_added += supp;
                        let _ = supp_added;
                        let piece: String = base[pos..end].iter().collect();
                        if ns.segmented.is_empty() {
                            ns.segmented = piece;
                        } else {
                            ns.segmented.push(' ');
                            ns.segmented.push_str(&piece);
                        }
                        ns.text.push_str(text);
                        ns.max_rank = ns.max_rank.max(rank1b);
                        ns.word_ends.push((ns.text.chars().count(), end));
                        buckets[end].add(ns);
                    }
                }
            }
        }

        // 终态 emit：EOS + 孤立惩罚（Rime build_early_commit_candidates 先并入完整态）
        buckets[n].limit(w.beam_width);
        let fin_trunc = buckets[n].truncated;
        let finals = buckets[n].snapshot();
        let mut hits: Vec<SentenceHit> = finals
            .iter()
            .map(|st| {
                let eos = (self.model.trigram_prob(st.prev2, st.prev1, EOS).max(1e-12).ln()) as f64;
                let iso = self.isolation_penalty(&st.text);
                SentenceHit {
                    score: st.score + eos - iso,
                    confidence: st.mass + eos - iso,
                    text: st.text.clone(),
                    max_rank: st.max_rank,
                    word_ends: st.word_ends.clone(),
                    segmented: st.segmented.clone(),
                }
            })
            .collect();
        hits.sort_by(|a, b| {
            a.max_rank
                .cmp(&b.max_rank)
                .then(b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal))
                .then(a.text.cmp(&b.text))
        });
        hits.truncate(w.candidate_limit);

        // 不完全尾候选（提前上屏置信源）：完整态先并入，尾部未成码前缀态合并（Rime 同构）
        let mut early_mass: HashMap<String, f64> = HashMap::new();
        let mut early_best: HashMap<String, SentenceHit> = HashMap::new();
        // 完整态先入列
        for st in &finals {
            if st.text.is_empty() {
                continue;
            }
            let eos =
                (self.model.trigram_prob(st.prev2, st.prev1, EOS).max(1e-12).ln()) as f64;
            let iso = self.isolation_penalty(&st.text);
            let conf = st.mass + eos - iso;
            let score = st.score + eos - iso;
            let key = st.text.clone();
            let newm = match early_mass.get(&key) {
                Some(m) => logsumexp(*m, conf),
                None => conf,
            };
            early_mass.insert(key.clone(), newm);
            early_best.insert(
                key,
                SentenceHit {
                    score,
                    confidence: conf,
                    text: st.text.clone(),
                    max_rank: st.max_rank.max(1),
                    word_ends: st.word_ends.clone(),
                    segmented: st.segmented.clone(),
                },
            );
        }
        let mut uses_incomplete = false;
        let mut early_trunc = fin_trunc;
        let max_tail = (max_code_len.saturating_sub(1)).min(n.saturating_sub(1));
        for tail_len in 1..=max_tail {
            let consumed = n - tail_len;
            let tail = &base[consumed..];
            if !self.incomplete_tail(tail) {
                continue;
            }
            buckets[consumed].limit(w.beam_width);
            let partial = buckets[consumed].snapshot();
            if !partial.is_empty() {
                uses_incomplete = true;
                early_trunc = early_trunc || buckets[consumed].truncated;
                for st in &partial {
                    if st.text.is_empty() {
                        continue;
                    }
                    let eos = (self.model.trigram_prob(st.prev2, st.prev1, EOS).max(1e-12).ln())
                        as f64;
                    let iso = self.isolation_penalty(&st.text);
                    let conf = st.mass + eos - iso;
                    let score = st.score + eos - iso;
                    let key = st.text.clone();
                    let newm = match early_mass.get(&key) {
                        Some(m) => logsumexp(*m, conf),
                        None => conf,
                    };
                    early_mass.insert(key.clone(), newm);
                    let better = early_best
                        .get(&key)
                        .map(|p| conf > p.confidence)
                        .unwrap_or(true);
                    if better {
                        early_best.insert(
                            key,
                            SentenceHit {
                                score,
                                confidence: conf,
                                text: st.text.clone(),
                                max_rank: st.max_rank.max(1),
                                word_ends: st.word_ends.clone(),
                                segmented: st.segmented.clone(),
                            },
                        );
                    }
                }
            }
        }
        let mut early_hits: Vec<SentenceHit> =
            early_best.into_values().collect();
        if !uses_incomplete {
            early_hits.clear();
            early_trunc = false;
        }
        early_hits.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.text.cmp(&b.text))
        });
        early_hits.truncate(w.candidate_limit);

        SentenceDecode {
            hits,
            truncated: fin_trunc,
            early_hits,
            early_truncated: early_trunc,
        }
    }

    fn decode_cached(&self, raw: &str) -> Arc<SentenceDecode> {
        let mut cache = self.cache.lock().unwrap();
        if let Some((prev_raw, prev_out)) = cache.as_ref() {
            if prev_raw == raw {
                return prev_out.clone();
            }
        }
        let out = Arc::new(self.decode_internal(raw));
        *cache = Some((raw.to_string(), out.clone()));
        out
    }

    /// 供测试与工具直接调用。
    pub fn decode_to_strings(&self, raw: &str) -> Vec<String> {
        self.decode_cached(raw).hits.iter().map(|h| h.text.clone()).collect()
    }

    /// 置信前缀提案（Rime confidence_proposal）：软最大前缀质量占比 ≥ 阈值的最长真前缀。
    pub fn confidence_proposal(
        &self,
        cands: &[&SentenceHit],
        threshold: f64,
    ) -> String {
        if cands.is_empty() {
            return String::new();
        }
        let max_score = cands.iter().map(|c| c.confidence).fold(f64::NEG_INFINITY, f64::max);
        let total: f64 = cands.iter().map(|c| (c.confidence - max_score).exp()).sum();
        // 前缀质量（按字符前缀）
        let mut prefix_mass: Vec<(Vec<char>, f64)> = Vec::new(); // (prefix chars, mass)
        for c in cands {
            let weight = (c.confidence - max_score).exp();
            let chars = chars_of(&c.text);
            let mut prefix: Vec<char> = Vec::new();
            for l in 1..chars.len().saturating_sub(1) + 1 {
                if l > chars.len() - 1 {
                    break;
                }
                prefix.push(chars[l - 1]);
                if let Some((_, m)) = prefix_mass.iter_mut().find(|(p, _)| *p == prefix) {
                    *m += weight;
                } else {
                    prefix_mass.push((prefix.clone(), weight));
                }
            }
        }
        let mut proposal: Vec<char> = Vec::new();
        for (p, m) in &prefix_mass {
            if m / total >= threshold && p.len() > proposal.len() {
                proposal = p.clone();
            }
        }
        proposal.into_iter().collect()
    }
}

impl SentenceDecoder for SentenceEngine {
    fn decode_rich(&self, raw: &str) -> Arc<SentenceDecode> {
        self.decode_cached(raw)
    }

    fn decode(&self, raw: &str) -> Vec<Candidate> {
        self.decode_cached(raw)
            .hits
            .iter()
            .map(|h| {
                let mut c = Candidate::new(h.text.clone(), raw.to_string(), CandidateKind::Sentence);
                c.weight = h.score;
                c
            })
            .collect()
    }
}
