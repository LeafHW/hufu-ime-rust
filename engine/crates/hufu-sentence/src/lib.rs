//! hufu-sentence —— TCSKNM02 ngram 整句引擎。
//!
//! 模型：TigerClaw 生态 `sentence-ngram-*.bin`（明文 TCSKNM02，Kneser-Ney trigram）。
//! 解码：字级 beam search（按 raw 位置分桶）+ 名次惩罚 + 出字奖励 + 孤立生僻惩罚
//! + 补充语料奖励；候选尾缀 `;`/`'`/数字作选重；置信前缀提前上屏提案。

pub mod model;

use hufu_config::SentenceWeights;
use hufu_dict::dict::Dict;
use hufu_dict::supplement::Supplement;
use hufu_engine::SentenceDecoder;
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
    /// 上次解码缓存（raw → 候选），供 early_commit_proposal 复用
    cache: Mutex<Option<(String, Vec<BeamOutput>)>>,
}

/// 解码产物（含前缀→raw 位置映射，供提前上屏）。
#[derive(Clone, Debug)]
struct BeamOutput {
    text: String,
    score: f64,
    /// 每个字符发射前的 raw 位置（consumed 映射）
    boundaries: Vec<usize>,
}

/// 选重后缀解析：尾部 `;`(第2) `'`(第3) 数字(第N)。
fn parse_selector(raw: &str) -> (String, Option<usize>) {
    let mut chars: Vec<char> = raw.chars().collect();
    let mut sel = None;
    while let Some(&last) = chars.last() {
        let s = match last {
            ';' => Some(1), // 0 基次选
            '\'' => Some(2),
            '0'..='9' => Some(last.to_digit(10).unwrap() as usize % 10), // 0→第10
            _ => None,
        };
        match s {
            Some(v) => {
                sel = Some(v);
                chars.pop();
                break; // 只取最后一个
            }
            None => break,
        }
    }
    (chars.into_iter().collect(), sel)
}

#[derive(Clone)]
struct BeamState {
    prev2: u32,
    prev1: u32,
    text: String,
    score: f64,
    /// 每字符发射前的 raw 位置
    boundaries: Vec<usize>,
    /// 当前词起始 raw 位置（词内字符共享）
    word_start: usize,
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

    /// 补充语料奖励（对数域）。
    fn supplement_reward(&self, word: &str) -> f64 {
        match self.supplement.get(word) {
            Some(w) => (self.weights.supplement_baseline
                + self.weights.supplement_scale * ((w / 1000.0).ln().max(0.0)))
            .min(self.weights.supplement_maximum),
            None => 0.0,
        }
    }

    /// 核心解码：返回带边界信息的候选（已按分数降序）。
    fn decode_internal(&self, raw: &str) -> Vec<BeamOutput> {
        let (base, selector) = parse_selector(raw);
        let raw_chars: Vec<char> = base.chars().collect();
        let n = raw_chars.len();
        let w = &self.weights;
        if n == 0 || n > w.max_raw_length || n <= 4 {
            // 整句只处理 >4 码
            return Vec::new();
        }

        // 每个位置的编码切分（长码优先），预计算一次
        // segs[pos] = Vec<(code_len, Vec<(text, rank)>)>
        let mut segs: Vec<Vec<(usize, Vec<(String, usize)>)>> = vec![Vec::new(); n];
        for pos in 0..n {
            let tail: String = raw_chars[pos..].iter().collect();
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

        // beam：按 pos 分桶
        let mut buckets: Vec<Vec<BeamState>> = vec![Vec::new(); n + 1];
        buckets[0].push(BeamState {
            prev2: BOS,
            prev1: BOS,
            text: String::new(),
            score: 0.0,
            boundaries: Vec::new(),
            word_start: 0,
        });

        for pos in 0..n {
            let mut bucket = std::mem::take(&mut buckets[pos]);
            bucket.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
            bucket.truncate(w.beam_width);
            for state in bucket {
                // 终态
                if pos == n {
                    continue;
                }
                let prev_before_word = state.prev1;
                for (code_len, entries) in &segs[pos] {
                    let end = pos + code_len;
                    for (text, rank) in entries {
                        // >4 码段只允许 rank=1（显式选重除外）
                        if pos >= 4 && *rank > 0 {
                            continue;
                        }
                        let mut ns = state.clone();
                        ns.word_start = pos;
                        let mut prev_char = prev_before_word;
                        let mut isolation_hits = 0usize;
                        for c in text.chars() {
                            let cp = c as u32;
                            let p3 = self.model.trigram_prob(ns.prev2, ns.prev1, cp);
                            ns.score += (p3.max(1e-12).ln()) as f64;
                            ns.score += w.emitted_character_reward;
                            // 孤立生僻：低频且与上文无 bigram
                            if self.model.freq_rank(cp) > w.isolation_threshold
                                && !self.model.has_bigram(prev_char, cp)
                            {
                                isolation_hits += 1;
                            }
                            ns.score -= w.isolation_lambda * isolation_hits as f64;
                            ns.boundaries.push(pos);
                            ns.prev2 = ns.prev1;
                            ns.prev1 = cp;
                            prev_char = cp;
                        }
                        if *rank > 0 {
                            ns.score -= w.rank_penalty * (*rank as f64 + 1.0).ln();
                        }
                        ns.score += self.supplement_reward(text);
                        ns.text.push_str(text);
                        buckets[end].push(ns);
                    }
                }
            }
        }

        // 终态：EOS 概率 + 去重
        let mut finals: Vec<BeamState> = std::mem::take(&mut buckets[n]);
        let mut out: Vec<BeamOutput> = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for mut st in finals {
            let eos = self.model.trigram_prob(st.prev2, st.prev1, EOS);
            st.score += (eos.max(1e-12).ln()) as f64;
            match seen.get(&st.text) {
                Some(&i) => {
                    if st.score > out[i].score {
                        out[i].score = st.score;
                        out[i].boundaries = st.boundaries;
                    }
                }
                None => {
                    seen.insert(st.text.clone(), out.len());
                    out.push(BeamOutput {
                        text: st.text,
                        score: st.score,
                        boundaries: st.boundaries,
                    });
                }
            }
        }
        out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(w.candidate_limit);

        // 选重后缀：把指定名次候选提到首位
        if let Some(k) = selector {
            if k < out.len() {
                let pick = out.remove(k);
                out.insert(0, pick);
            }
        }
        out
    }

    fn decode_cached(&self, raw: &str) -> Vec<BeamOutput> {
        let mut cache = self.cache.lock().unwrap();
        if let Some((prev_raw, prev_out)) = cache.as_ref() {
            if prev_raw == raw {
                return prev_out.clone();
            }
        }
        let out = self.decode_internal(raw);
        *cache = Some((raw.to_string(), out.clone()));
        out
    }

    /// 供测试与工具直接调用。
    pub fn decode_to_strings(&self, raw: &str) -> Vec<String> {
        self.decode_cached(raw).iter().map(|o| o.text.clone()).collect()
    }
}

impl SentenceDecoder for SentenceEngine {
    fn decode(&self, raw: &str) -> Vec<Candidate> {
        self.decode_cached(raw)
            .iter()
            .map(|o| {
                let mut c = Candidate::new(o.text.clone(), raw.to_string(), CandidateKind::Sentence);
                c.weight = o.score;
                c
            })
            .collect()
    }

    fn early_commit_proposal(&self, raw: &str) -> Option<(String, usize)> {
        let outs = self.decode_cached(raw);
        if outs.len() < 2 {
            return None;
        }
        // 总质量与前缀质量（对数域 → 线性占比）
        let total: f64 = outs.iter().map(|o| o.score.exp()).sum();
        let top = &outs[0];
        let chars: Vec<char> = top.text.chars().collect();
        // 从长到短找第一个质量占比 ≥ confidence 的前缀
        let mut best: Option<(String, usize)> = None;
        for l in (1..chars.len()).rev() {
            let prefix: String = chars[..l].iter().collect();
            let mass: f64 = outs
                .iter()
                .filter(|o| o.text.starts_with(&prefix))
                .map(|o| o.score.exp())
                .sum();
            if mass / total >= self.weights.confidence {
                // consumed = 拼出该前缀消耗的 raw 长度（用 top 的边界）
                let consumed = top.boundaries.get(l).copied().unwrap_or(0);
                if consumed > 0 {
                    best = Some((prefix, consumed));
                }
                break;
            }
        }
        best
    }
}
