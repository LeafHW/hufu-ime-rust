//! hufu-sentence —— TCSKNM02 ngram 整句引擎。
//!
//! 模型：TigerClaw 生态 `sentence-ngram-*.bin`（明文 TCSKNM02，Kneser-Ney trigram）。
//! 解码：字级 beam search（按 raw 位置分桶，同文本 logsumexp 聚合质量）
//! + 名次惩罚 + 出字奖励 + 终态孤立生僻惩罚（emit 期全文计算）
//! + 补充词奖励（不进质量）；选重后缀锁所在段名次；
//! 不完全尾候选（把尾部未成码的前缀视为「下一词在打」）供提前上屏置信评估。
//! 全流程对齐 Rime 虎整句 tiger_sentence.lua。

pub mod model;
pub mod supplement_automaton;

use hufu_config::SentenceWeights;
use hufu_dict::dict::Dict;
use hufu_dict::supplement::Supplement;
use hufu_engine::{parse_rank_locks, SentenceDecoder, SentenceHit, SentenceDecode};
use hufu_types::{Candidate, CandidateKind};
use model::{BOS, EOS, NgramModel};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use supplement_automaton::SupplementAutomaton;

/// 整句引擎。
pub struct SentenceEngine {
    pub model: NgramModel,
    pub dict: Arc<Dict>,
    supplement: SupplementAutomaton,
    pub weights: SentenceWeights,
    /// 解码缓存：last=同 raw 结果缓存；prefix=上次解码的过程桶
    /// （增量解码：新 raw 为旧 raw 追加且 base 前缀一致时，复用
    /// 前部桶只重算尾部窗口）。
    cache: Mutex<EngineCache>,
}

#[derive(Default)]
struct EngineCache {
    last: Option<(String, Arc<SentenceDecode>)>,
    prefix: Option<(String, Vec<Bucket>)>,
}

/// 增量解码参数：前缀最短长度 / 尾部重算窗口 / 单次最大追加键数。
/// 尾窗须覆盖旧尾段豁免（is_tail 依赖 n）与锁变化的回溯范围
/// （max_code_length=4 + 缓冲，12 保守）。前缀长度 ≤ 尾窗时 split=0
/// 退化为全量（主循环从 BOS 种子起算），不产生增量收益也绝不出错。
const INC_MIN_PREFIX: usize = 20;
const INC_REDO_TAIL: usize = 8;
const INC_MAX_DELTA: usize = 3;
/// 每段参与组句的码表词条上限（rank 截断）：虎码同码词呈长尾分布，
/// rank>8 的系统词极生僻，beam 展开却为每词条付一次 String clone。
const SEG_RANK_LIMIT: usize = 8;

/// beam 内部状态。
/// 【性能】segmented 不存（clone 热路径省 1/3 堆拷贝）：emit 期由
/// word_ends + base 重建（信息无损）。
#[derive(Clone)]
struct St {
    prev2: u32,
    prev1: u32,
    text: String,
    /// 排序分（含名次惩罚与补充词奖励）
    score: f64,
    /// 同文本聚合质量（logsumexp；不含补充奖励）
    mass: f64,
    max_rank: usize,
    /// 各段码表名次总和（选重深度；rerank 无锁约束用）
    sum_rank: usize,
    /// 全路径每段精确对应实打编码（无前缀扩展、无未打选重）
    exact: bool,
    /// 词边界：(累计字数, base 消耗位置)
    word_ends: Vec<(usize, usize)>,
    /// 补充语料 AC 自动机状态（沿全文逐字推进）
    supp_state: usize,
    /// 补充语料累计加分（2026-09-05：计入提前上屏置信——用户显式
    /// 加权的词（补充语料.txt）理应也影响提案，否则「上屏真爽」案
    /// 显示翻盘而提案仍被「火藏」拆段抢跑。dict_bias 仍不进置信。）
    supp_bonus: f64,
}

/// emit 期由词边界重建切分串（对齐旧 St.segmented 语义）。
fn segmented_of(word_ends: &[(usize, usize)], base: &[char]) -> String {
    let mut out = String::new();
    let mut prev_end = 0usize;
    for &(_, end) in word_ends {
        let piece: String = base[prev_end..end].iter().collect();
        if out.is_empty() {
            out = piece;
        } else {
            out.push(' ');
            out.push_str(&piece);
        }
        prev_end = end;
    }
    out
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
        let entries: Vec<(String, f64)> = supplement
            .entries
            .iter()
            .map(|e| (e.word.clone(), e.weight))
            .collect();
        let automaton = SupplementAutomaton::build(
            &entries,
            weights.supplement_baseline,
            weights.supplement_scale,
            weights.supplement_maximum,
        );
        SentenceEngine {
            model,
            dict,
            supplement: automaton,
            weights,
            cache: Mutex::new(EngineCache::default()),
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
    /// resume=Some((buckets, start_pos)) 时增量：复用前部桶、从 start_pos
    /// 续算（segs 与主循环都只跑尾部）。返回 (结果, 过程桶)——桶供下次
    /// 增量复用；n==0 或超 max_raw_length 时返回 None（不可复用）。
    fn decode_internal(
        &self,
        raw: &str,
        resume: Option<(Vec<Bucket>, usize)>,
    ) -> (SentenceDecode, Option<Vec<Bucket>>) {
        let parsed = parse_rank_locks(raw);
        let base: Vec<char> = parsed.base.chars().collect();
        let n = base.len();
        let w = &self.weights;
        if n == 0 || n > w.max_raw_length {
            return (
                SentenceDecode {
                    hits: Vec::new(),
                    truncated: false,
                    early_hits: Vec::new(),
                    early_truncated: false,
                },
                None,
            );
        }
        let start_pos = resume.as_ref().map(|(_, s)| *s).unwrap_or(0);

        // 每个位置的编码切分预计算：segs[pos] = Vec<Seg>
        // 增量时只需尾部窗内的（前部段不受新键影响）。
        // 【性能】锁关系（段终点锁名次/是否跨界锁）在此一次算清——
        // 原实现在 state×seg 内层循环反复 iter().any/find，与 state
        // 无关纯属重复（20 键句 ≈ 24 万次锁遍历/键）。
        #[derive(Clone)]
        struct Seg {
            end: usize,
            /// 段终点命中的锁名次（锁位置 == end）
            lock_rank: Option<usize>,
            /// (文本, 码表名次-1, 精确)：精确=该词条码表码长==消耗键数
            /// （「改变」码 vz; 只消耗 2 键=前缀扩展，不精确；「服」码
            /// vz 消耗 2 键=精确）。无锁短码候选过滤用（2026-09-05 用
            /// 户规则：选重的数字、词锁的 ; 都是编码的一部分——没打就
            /// 不出现在候选里，javz 只该有「们服」与整码字）。
            entries: Vec<(String, usize, bool)>,
        }
        let mut segs: Vec<Vec<Seg>> = vec![Vec::new(); n];
        for pos in start_pos..n {
            let tail: String = base[pos..].iter().collect();
            for (code_len, idxs) in self.dict.prefix_matches(&tail) {
                if code_len == 0 || pos + code_len > n {
                    continue;
                }
                let end = pos + code_len;
                // 一简禁令（对齐虎爪规范）：句中（n>4）不允许 1 码段——
                // 26 一简字在整句里必须打 2 码全码，其 1 码形式不参与
                // 组句。n≤4 是短码查词场景（对齐虎爪「总长≤4 检索全部
                // 字词」）保持宽容。
                // 两类豁免：
                // 1. 尾段（pos+code_len==n）= 用户正在打的下一词，放行
                //    （否则「cbfe;u」锁+一简尾切分路径全死）；
                // 2. 段终点被选重锁（;/'/数字）钉住的段 = 用户显式选定
                //    的名次（如「cn;j;」j 段由 ; 锁 rank），放行——
                //    虎爪顶功次选流（码+; 逐段确认）依赖此路径。
                if code_len == 1 && n > 4 && pos + 1 < n {
                    let seg_end = pos + 1;
                    if !parsed.locks.iter().any(|(l, _)| *l as usize == seg_end) {
                        continue;
                    }
                }
                // 锁位置必须是段边界：段不得跨越锁终点
                if parsed.locks.iter().any(|(l, _)| *l as usize > pos && (*l as usize) < end) {
                    continue;
                }
                let lock_rank = parsed
                    .locks
                    .iter()
                    .find(|(l, _)| *l as usize == end)
                    .map(|(_, r)| *r);
                let entries: Vec<(String, usize, bool)> = idxs
                    .iter()
                    .enumerate()
                    .take(SEG_RANK_LIMIT)
                    .filter_map(|(rank, &idx)| {
                        self.dict.entries.get(idx as usize).map(|e| {
                            let exact = e.code.chars().count() == code_len;
                            (e.text.clone(), rank, exact)
                        })
                    })
                    .collect();
                if !entries.is_empty() {
                    segs[pos].push(Seg { end, lock_rank, entries });
                }
            }
        }

        let max_code_len = segs
            .iter()
            .flatten()
            .map(|s| s.end)
            .max()
            .unwrap_or(1);

        // beam 分桶：增量时复用前部桶（其内容只依赖 base[..pos]，
        // 不受尾部新键影响），全量时新建并种入 BOS。
        let mut buckets: Vec<Bucket> = match resume {
            Some((b, _)) => b,
            None => (0..=n).map(|_| Bucket::new()).collect(),
        };
        if start_pos == 0 {
            buckets[0].add(St {
                prev2: BOS,
                prev1: BOS,
                text: String::new(),
                score: 0.0,
                mass: 0.0,
                max_rank: 1,
                sum_rank: 0,
                exact: true,
                word_ends: Vec::new(),
                supp_state: 0,
                supp_bonus: 0.0,
            });
        }

        let allow_all_ranks = n <= 4;
        // 长句 beam 分档（性能）：解码耗时随长度超线性增长（22键≈15ms、
        // 48键≈340ms），打长句时每键 300ms+ 而打字约 150ms/键，滞后累积
        // 出现「编码打完了候选还在逐字录入」。长句时优质路径早已大幅领
        // 先，尾部宽度的边际收益极小——按长度降档用极小的质量代价换回
        // 响应速度。分档界与比例经 100 句基准回归校准。
        let beam = if n <= 16 {
            w.beam_width
        } else if n <= 24 {
            (w.beam_width * 3 / 5).max(400)
        } else if n <= 32 {
            (w.beam_width / 5).max(300)
        } else if n <= 48 {
            (w.beam_width / 8).max(200)
        } else {
            // 【性能】超长句（>48 码）：尾键延迟实测 40-50ms——再降档
            // 换响应（bench 实测：/24 无额外收益，/16 max100 最优——
            // avg 49→24ms、p95 102→38ms、exact 90% 持平）
            (w.beam_width / 16).max(100)
        };
        for pos in start_pos..n {
            buckets[pos].limit(beam);
            if std::env::var("HUFU_INC_DEBUG").is_ok() {
                eprintln!(
                    "[inc] raw_len={n} start={start_pos} pos={pos} bucket_size={} segs={}",
                    buckets[pos].best.len(),
                    segs[pos].len()
                );
            }
            for state in buckets[pos].snapshot() {
                for seg in &segs[pos] {
                    let end = seg.end;
                    let lock = seg.lock_rank;
                    // 多码整句时段跨须 ≥2（含选重后缀字符；Rime 段跨规则，
                    // tiger_sentence.lua L562 同款；虎爪 ExpandRange L385
                    // num3-i<2 同款）。【实测 2026-08-31】放开此规则（允许
                    // 一简段入整句）100 句基准 exact 92.93%→78.79%——单码
                    // 段制造大量噪声路径，禁令是质量担当，不可动。
                    // 【尾段豁免 2026-09-04】end==n（消耗到 raw 末尾）=
                    // 用户正在打的下一词，仅两类场景放行单码段：
                    //   a) n>4（真整句，如 cbfe;u 的 u）；
                    //   b) 有选重锁（顶功确认流，如 cn;j;c 的 c）。
                    // 短码且无锁（zhh/egy 类）不豁免——否则 zh(其)+h(道)
                    // 两字路径压过码表精确词「虎」，单字首选错位（用户
                    // 实测 zhh 首选变「其道」；虎爪/Rime 该码只有「虎」）。
                    let span = end - pos + if lock.is_some() { 1 } else { 0 };
                    if n > 1 && span < 2 {
                        // 尾段豁免仅限带锁（顶功确认流，如「cn;j;c」「cbfe;u」
                        // 的锁+一简尾）。无锁一简尾=进行中（用户还要打）——
                        // 一简字进组句必须打 2 码全码（2026-09-05 用户实测
                        // uaegq 出「打干都」：都 的一简 q 段未经锁确认固化
                        // 进组合，属进行态不该出现）。
                        let is_tail = end == n;
                        let exempt = is_tail && !parsed.locks.is_empty();
                        if !exempt {
                            continue;
                        }
                    }
                    for (text, rank, seg_exact) in &seg.entries {
                        let rank1b = rank + 1; // 码表名次（1 起）
                        if let Some(r) = lock {
                            if rank1b != r {
                                continue;
                            }
                        } else if !allow_all_ranks && rank1b != 1 {
                            // >4 码无锁只取第 1 候选
                            continue;
                        }
                        let mut ns = state.clone();
                        for c in text.chars() {
                            let cp = c as u32;
                            let p3 = self.model.trigram_prob(ns.prev2, ns.prev1, cp);
                            ns.score += (p3.max(1e-12).ln()) as f64;
                            ns.score += w.emitted_character_reward;
                            ns.mass += (p3.max(1e-12).ln()) as f64 + w.emitted_character_reward;
                            ns.prev2 = ns.prev1;
                            ns.prev1 = cp;
                            // 补充词：AC 自动机沿全文推进（任意位置命中都加分）
                            let (st2, r) = self.supplement.advance(ns.supp_state, c);
                            ns.supp_state = st2;
                            ns.score += r;
                            ns.supp_bonus += r;
                        }
                        if rank1b > 1 {
                            let pen = w.rank_penalty * (rank1b as f64).ln();
                            ns.score -= pen;
                            ns.mass -= pen;
                        }
                        // 【dict_bias 接线 2026-09-03】短码窗口（n≤4）多字
                        // 码表词条温和加成：只进 score 不进 mass（与
                        // supplement 同语义——非概率项，不污染提前上屏置
                        // 信估计）。仅 n≤4 生效：整句（n>4）全程模型主导
                        //（500 句实测 bias 全局生效准率 99.80→96.60%——
                        // 码表冷词+1 压过正确的拆字/词组路径）；短码场景
                        // 码表词优先（srsr 常常 vs 发发 贴脸 0.002 分，无
                        // bias 时排序随扰动翻转）。量级 1.0：压得住拆字噪
                        // 声路径，压不过 ngram 强信号（领先 2-5 分）——
                        // 「码表词稳靠前，模型强词仍可反超」。
                        if n <= 4 && text.chars().count() >= 2 {
                            ns.score += w.dict_bias;
                        }
                        ns.text.push_str(text);
                        ns.max_rank = ns.max_rank.max(rank1b);
                        ns.sum_rank += rank1b;
                        // 精确累计：段词条码长==消耗键数（无前缀扩展）且
                        // 无选重（rank1，或被锁钉名次=用户打了选重键）。
                        // 任何一段不精确则整条路径 exact=false。
                        if !*seg_exact && lock.is_none() {
                            ns.exact = false;
                        }
                        if rank1b > 1 && lock.is_none() {
                            ns.exact = false;
                        }
                        ns.word_ends.push((ns.text.chars().count(), end));
                        buckets[end].add(ns);
                    }
                }
            }
        }

        // 终态 emit：EOS + 孤立惩罚（Rime build_early_commit_candidates 先并入完整态）
        // 【性能】isolation 按 text 缓存（纯 text 函数，hits/early 三轮共用）；
        // eos 依赖 (prev2,prev1)、segmented 依赖 word_ends——同 text 跨桶
        // 可不同，不缓存（原实现同 text 三轮全量重算 isolation，每生僻字
        // 2 次 bigram 查询，beam_width 终态下每键白付两轮）。
        buckets[n].limit(w.beam_width);
        let fin_trunc = buckets[n].truncated;
        let finals = buckets[n].snapshot();
        let mut iso_cache: HashMap<String, f64> = HashMap::new();
        let mut iso_of = |text: &str| -> f64 {
            if let Some(v) = iso_cache.get(text) {
                return *v;
            }
            let v = self.isolation_penalty(text);
            iso_cache.insert(text.to_string(), v);
            v
        };
        let mut hits: Vec<SentenceHit> = finals
            .iter()
            .map(|st| {
                let eos = (self.model.trigram_prob(st.prev2, st.prev1, EOS).max(1e-12).ln()) as f64;
                let iso = iso_of(&st.text);
                SentenceHit {
                    score: st.score + eos - iso,
                    confidence: st.mass + eos - iso + st.supp_bonus,
                    text: st.text.clone(),
                    max_rank: st.max_rank,
                    sum_rank: st.sum_rank,
                    exact: st.exact,
                    word_ends: st.word_ends.clone(),
                    segmented: segmented_of(&st.word_ends, &base),
                    partial: false,
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
            let eos = (self.model.trigram_prob(st.prev2, st.prev1, EOS).max(1e-12).ln()) as f64;
            let iso = iso_of(&st.text);
            let conf = st.mass + eos - iso + st.supp_bonus;
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
                    sum_rank: st.sum_rank,
                    exact: st.exact,
                    word_ends: st.word_ends.clone(),
                    segmented: segmented_of(&st.word_ends, &base),
                    partial: false,
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
            buckets[consumed].limit(beam);
            let partial = buckets[consumed].snapshot();
            if !partial.is_empty() {
                uses_incomplete = true;
                early_trunc = early_trunc || buckets[consumed].truncated;
                for st in &partial {
                    if st.text.is_empty() {
                        continue;
                    }
                    let eos = (self.model.trigram_prob(st.prev2, st.prev1, EOS).max(1e-12).ln()) as f64;
                    let iso = iso_of(&st.text);
                    let conf = st.mass + eos - iso + st.supp_bonus;
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
                                sum_rank: st.sum_rank,
                    exact: st.exact,
                                word_ends: st.word_ends.clone(),
                                segmented: segmented_of(&st.word_ends, &base),
                                partial: true,
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

        (
            SentenceDecode {
                hits,
                truncated: fin_trunc,
                early_hits,
                early_truncated: early_trunc,
            },
            Some(buckets),
        )
    }

    fn decode_cached(&self, raw: &str) -> Arc<SentenceDecode> {
        let mut cache = self.cache.lock().unwrap();
        if let Some((prev_raw, prev_out)) = cache.last.as_ref() {
            if prev_raw == raw {
                return prev_out.clone();
            }
        }
        // 增量解码：新 raw 为缓存前缀的追加（1-3 键），且 base 长度
        // 恰好增长（锁/选重后缀不增 base，走全量保证正确性）。
        // 复用前部 buckets，从 split=旧base长-尾窗 处重算。
        let parsed_new = parse_rank_locks(raw);
        let new_base_len = parsed_new.base.chars().count();
        let can = cache
            .prefix
            .as_ref()
            .map(|(p_raw, buckets)| {
                let old_base_len = buckets.len().saturating_sub(1);
                let delta = new_base_len as isize - old_base_len as isize;
                raw.starts_with(p_raw.as_str())
                    && (1..=INC_MAX_DELTA as isize).contains(&delta)
                    && old_base_len >= INC_MIN_PREFIX
            })
            .unwrap_or(false);
        let resume = if can {
            cache.prefix.take().map(|(p_raw, mut buckets)| {
                let old_base_len = buckets.len() - 1;
                // split=0 时退化为全量（主循环从 BOS 起算），此处不设下限：
                // 强制 split≥1 会丢弃 pos=0 的段展开，令全部后继桶断源
                // （13 键候选空 bug 的根因）。
                let split = old_base_len.saturating_sub(INC_REDO_TAIL);
                // 再回退一个最大段长（虎码 max_code_length=4）：保留区末尾
                // pos 的段可能跨界伸入重算区，这些展开必须重跑（Bucket::add
                // 聚合幂等，重复展开无副作用），否则跨界路径全丢。
                let start = split.saturating_sub(4);                buckets.truncate(split + 1);
                buckets.resize_with(new_base_len + 1, Bucket::new);
                (buckets, start, p_raw)
            })
        } else {
            None
        };
        let dbg = std::env::var("HUFU_INC_DEBUG").is_ok();
        if dbg {
            let sp = resume.as_ref().map(|(_, s, _)| *s).unwrap_or(0);
            eprintln!("[cache] raw={raw} start={sp}");
        }
        let (out, buckets) = self.decode_internal(raw, resume.map(|(b, s, _)| (b, s)));
        let out = Arc::new(out);
        cache.last = Some((raw.to_string(), out.clone()));
        if let Some(b) = buckets {
            cache.prefix = Some((raw.to_string(), b));
        } else {
            cache.prefix = None;
        }
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

    fn rare_hint(&self, ch: char) -> bool {
        self.model.is_rare(ch as u32, self.weights.isolation_threshold)
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
