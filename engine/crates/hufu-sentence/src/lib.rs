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
use hufu_engine::{
    parse_rank_locks, parse_rank_locks_keep_digits, SentenceDecoder, SentenceHit, SentenceDecode,
};
use hufu_types::{Candidate, CandidateKind};
use model::{BOS, EOS, NgramModel};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use supplement_automaton::SupplementAutomaton;

/// 整句引擎。
pub struct SentenceEngine {
    pub model: NgramModel,
    pub dict: Arc<Dict>,
    supplement: SupplementAutomaton,
    pub weights: SentenceWeights,
    /// 【整句高频字过滤上限】组句里的「字 → 被禁用的码」表：频序
    /// （hufu-dict 的 TOP4000）不超过 `weights.high_freq_limit` 的单字，
    /// 只允许用整句最优码 `Dict::best_code_with_min_len(ch, 2)` 参与
    /// 组句，该字其余 ≥2 码记在这里，命中即丢弃该条目。
    /// 装配期（`with_model`）一次性建好：遍历码表的「字 → 码」表，逐字
    /// 查一次频序（`rank_of` 首次调用建 O(1) 索引）——虎整句码表单字数
    /// 十万级，成本一次性落在装载路径内；热路径只查集合。上限 0 时是空表
    /// ⇒ 判定恒 false、零构建开销 ⇒ 与不启用逐位一致。
    blocked: HashMap<char, HashSet<String>>,
    /// 解码缓存：last=同 raw 结果缓存；prefix=上次解码的过程桶
    /// （增量解码：新 raw 为旧 raw 追加且 base 前缀一致时，复用
    /// 前部桶只重算尾部窗口）。
    cache: Mutex<EngineCache>,
    /// 【用户词注入 2026-09-06】/jc 加的词参与整句词图（与码表段并列
    /// 的独立 Seg，rank=1：长句只放行 rank1 段，用户词须以并列首选
    /// 身份进入；排序仍由 ngram 主导）。RwLock 热更新：引擎 reload
    /// 用户数据后 set_user_words 同步，无需重建 ngram。码表原序不动。
    user_words: std::sync::RwLock<Vec<(String, String)>>,
}

impl SentenceEngine {
    /// 热更新用户词表（code, text）。清空传空即可。
    pub fn set_user_words(&self, words: Vec<(String, String)>) {
        if let Ok(mut g) = self.user_words.write() {
            *g = words;
        }
        // 解码缓存失效：last/prefix 桶是旧词图的结果，不含新词
        if let Ok(mut c) = self.cache.lock() {
            *c = EngineCache::default();
        }
    }
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
/// 【增量门槛 2026-09-08】20→4：原 20 只避开「前缀≤尾窗时 split=0
/// 白做簿记」的场景，非正确性要求（增量算法对任意长度安全，≤12 键
/// 时 split=0 自然退化为全量）。实测 beam30000 连打爬坡段（4-19 键）
/// 正是全量在付成本——门槛降到 4 后该区间开始部分增量（重算 12 键
/// 尾窗 < 句长即有收益）。
const INC_MIN_PREFIX: usize = 4;
const INC_REDO_TAIL: usize = 8;
const INC_MAX_DELTA: usize = 3;
/// 【增量尾窗束宽 2026-09-08】beam30000「质量+不卡」两全的钥匙：
/// 增量重算只覆盖尾部 ≤12 键（REDO_TAIL+回退），前部 30000 束的
/// 路径多样性已锁定在复用桶中——尾段组合空间小，重算用小束
/// 承载。实测每键成本主要由尾窗束宽决定（尾窗 6000 对 beam≤6000
/// 无减负，6000 依然卡）——默认 1500，HUFU_INC_TAIL env 可调
/// （参数扫描用）。
fn inc_tail_beam() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("HUFU_INC_TAIL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2000)
    })
}
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
    /// 【隐式二选 2026-09-12】路径含隐式二选段（无锁 3/4 码 rank2）：
    /// engine 据此在候选组装时把过真词地板的隐式二选词初排压前
    ///（不等 Qwen 异步重排——用户实测「重排到首位有反应时间」）。
    implicit2: bool,
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

/// 【整句高频字过滤上限】装配期构建禁用表（语义见 `SentenceEngine::blocked`）。
///
/// 逐条固化规则与豁免：
/// - 只扫单字（词条不参与）；
/// - 频序表外的字不参与；
/// - 频序 > N 的字不参与；
/// - 整句最优码取不到（`None`）的整字不过滤——宁可不筛，不误删；
/// - 只登记 ≥2 码：1 码条目在判定侧本就不比较（官方整句口径排除 1 码）。
///
/// N = 0 直接返回空表：不进循环、零构建成本，判定恒 false。
fn build_blocked(dict: &Dict, limit: usize) -> HashMap<char, HashSet<String>> {
    let mut blocked: HashMap<char, HashSet<String>> = HashMap::new();
    if limit == 0 {
        return blocked;
    }
    for (text, codes) in dict.text_to_codes.iter() {
        let mut chars = text.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            continue; // 词条不参与
        };
        let Some(rank) = hufu_dict::freq::rank_of(ch) else {
            continue; // 频序表外
        };
        if rank > limit {
            continue;
        }
        // 整句最优码（官方口径：一简字不用 1 码）
        let Some(best) = dict.best_code_with_min_len(text, 2) else {
            continue; // 最优码取不到 ⇒ 整字不过滤
        };
        let banned: HashSet<String> = codes
            .iter()
            .filter(|code| code.chars().count() >= 2 && code.as_str() != best)
            .cloned()
            .collect();
        if !banned.is_empty() {
            blocked.insert(ch, banned);
        }
    }
    blocked
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
        // 【整句高频字过滤上限】禁用表随引擎装配一次定型（改 weights 的
        // 该键需重建引擎，与码表/模型同为装配期数据）。
        let blocked = build_blocked(&dict, weights.high_freq_limit);
        SentenceEngine {
            model,
            dict,
            supplement: automaton,
            weights,
            blocked,
            cache: Mutex::new(EngineCache::default()),
            user_words: std::sync::RwLock::new(Vec::new()),
        }
    }

    /// 【整句高频字过滤上限】条目级判定：单字条目 × 段消耗 ≥2 键 × 该字
    /// 的该码在禁用表里 ⇒ 丢弃。1 码段不比较（官方整句口径排除 1 码）；
    /// 词条不参与；上限 0（空表）恒 false。
    fn entry_blocked(&self, code: &str, text: &str, code_len: usize) -> bool {
        if code_len < 2 || self.blocked.is_empty() {
            return false;
        }
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(ch), None) => self
                .blocked
                .get(&ch)
                .is_some_and(|codes| codes.contains(code)),
            _ => false,
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
        // 【单次字典查询 2026-09-11】旧实现连查两次 prefix_matches
        //（any_prefix 一次、complete 一次）——emit 期尾循环逐档调用，
        // 白做一倍 Trie 走查。一轮判定两个条件。
        let matches = self.dict.prefix_matches(&tail_s);
        let mut any_prefix = false;
        let mut complete = false;
        for (len, _) in &matches {
            if *len >= tail.len() {
                any_prefix = true;
            }
            if tail.len() >= 2 && *len == tail.len() {
                complete = true;
            }
        }
        // 必须是某码的前缀；长尾不得恰为完整码
        any_prefix && !complete
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
        // 【数字编码 2026-09-05】数字编码表（a8=来、u3=的）：数字按
        // 码表延续判定——是编码字符则保留（整体或任意后缀是词条，跨
        // 段如 vvb8 的 b8=如），无延续（选重锁转的内部数字）仍做锁；
        // 普通表数字一律锁。
        let parsed = if self.weights.digit_codes {
            let dict = &self.dict;
            let is_code = |p: &str| {
                let cs: Vec<char> = p.chars().collect();
                (1..=cs.len()).any(|j| {
                    let s: String = cs[cs.len() - j..].iter().collect();
                    !dict.lookup(&s).is_empty() || !dict.completions(&s, 1).is_empty()
                })
            };
            parse_rank_locks_keep_digits(raw, &is_code)
        } else {
            parse_rank_locks(raw)
        };
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
        // 【修复 segs O(n²) 2026-09-11】原因：原先每个 pos 都重建
        // base[pos..] 整串（下方码表 tail 与用户词 tail_s 两处，各
        // O(n-pos)，n 个 pos 合计 O(n²) 字符拷贝+堆分配——外层逐 pos
        // 遍历、内层每次从 pos 重扫到串尾）→ 手段：一次遍历建全文
        // String + 每字符字节游标表 offs，pos 处 O(1) 借切片复用同一
        // 尾串。切片内容与原重建逐字节相同（prefix_matches 在首个
        // 失配字符处即停走，尾串传多长都不改变返回），语义等价。
        let base_s: String = base.iter().collect();
        let mut offs: Vec<usize> = Vec::with_capacity(n + 1);
        let mut byte_at = 0usize;
        for &c in &base {
            offs.push(byte_at);
            byte_at += c.len_utf8();
        }
        offs.push(byte_at);
        for pos in start_pos..n {
            let tail: &str = &base_s[offs[pos]..];
            for (code_len, idxs) in self.dict.prefix_matches(tail) {
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
                //    虎爪顶屏次选流（码+; 逐段确认）依赖此路径。
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
                        self.dict.entries.get(idx as usize).and_then(|e| {
                            // 【整句高频字过滤上限】单字 × 非最优码 × 频序 ≤ N
                            // ⇒ 丢弃（判定查装配期建好的集合）。名次仍按原
                            // enumerate 计——过滤不重编号。
                            if self.entry_blocked(&e.code, &e.text, code_len) {
                                return None;
                            }
                            let exact = e.code.chars().count() == code_len;
                            Some((e.text.clone(), rank, exact))
                        })
                    })
                    .collect();
                if !entries.is_empty() {
                    segs[pos].push(Seg { end, lock_rank, entries });
                }
            }
            // 【用户词注入 2026-09-06】/jc 用户词独立 Seg（与码表段并列）：
            // rank=1（长句 rank1 放行——用户词与码表首选并列首选，ngram
            // 拍板排序）；exact=true（码=消耗键数）。一简禁令、锁边界与
            // 码表段同规则。码表已有同码同词时跳过（去重）。
            if let Ok(uw) = self.user_words.read() {
                if !uw.is_empty() {
                    // 【修复 segs O(n²) 2026-09-11】同上：复用尾串切片，
                    // 不再每个 pos 重建 tail_s 整串（O(n-pos)×n）。
                    let tail_s: &str = tail;
                    for (code, text) in uw.iter() {
                        let cl = code.chars().count();
                        // 【码长放宽 2026-09-09】原 cl>4 一刀切拒收——虎码
                        // 词组编码变体（语料注入：蚩奼 sfcbtrq=7 码）与
                        // /jc 长词用户词全被丢。放宽到 16（4字×4码上限）。
                        if cl == 0 || pos + cl > n || cl > 16 || cl > tail_s.chars().count() {
                            continue;
                        }
                        if !tail_s.starts_with(code.as_str()) {
                            continue;
                        }
                        let end = pos + cl;
                        // 同位段去重（码表已含该词）
                        let dup = segs[pos].iter().any(|s| {
                            s.end == end && s.entries.iter().any(|(t, _, _)| t == text)
                        });
                        if dup {
                            continue;
                        }
                        // 一简禁令：句中 1 码段须被锁钉住（码表段同款豁免）
                        if cl == 1 && n > 4 && end < n {
                            if !parsed.locks.iter().any(|(l, _)| *l as usize == end) {
                                continue;
                            }
                        }
                        // 段不得跨越锁终点
                        if parsed
                            .locks
                            .iter()
                            .any(|(l, _)| *l as usize > pos && (*l as usize) < end)
                        {
                            continue;
                        }
                        let lock_rank = parsed
                            .locks
                            .iter()
                            .find(|(l, _)| *l as usize == end)
                            .map(|(_, r)| *r);
                        // 锁名次>1 时用户词（rank1）不匹配锁：跳过
                        //（锁 r=1 放行——用户词即显示序第 1）
                        if let Some(r) = lock_rank {
                            if r != 1 {
                                continue;
                            }
                        }
                        segs[pos].push(Seg {
                            end,
                            lock_rank,
                            entries: vec![(text.clone(), 0, true)],
                        });
                    }
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
        let is_resume = resume.is_some();
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
                implicit2: false,
            });
        }

        let allow_all_ranks = n <= 4;
        // 长句 beam 分档（性能）：解码耗时随长度超线性增长（22键≈15ms、
        // 48键≈340ms），打长句时每键 300ms+ 而打字约 150ms/键，滞后累积
        // 出现「编码打完了候选还在逐字录入」。长句时优质路径早已大幅领
        // 先，尾部宽度的边际收益极小——按长度降档用极小的质量代价换回
        // 响应速度。分档界与比例经 100 句基准回归校准。
        // 【短句降档 2026-09-08】实测跟打器 05:50 段连打：16 键内全量
        // 30000 束，引擎往返 42→446ms 随句长递增（增量缓存门槛
        // base≥20，短句每键全量）——打字节奏被拖垮=用户「越打越卡」。
        // n≤16 从全束降为 2/5 束（30000→12000）：短句首选路径领先幅度
        // 通常极大，边际束宽对 exact 无感。
        // 【min 收口 2026-09-08】各档 max 下限会把小束宽反向抬高
        //（beam=200 时 max(12000)=12000≠用户设定）——min(原值) 保证
        // 降档永不升束：200→200、30000→12000。
        let beam = if n <= 16 {
            (w.beam_width * 2 / 5).max(12000).min(w.beam_width)
        } else if n <= 24 {
            (w.beam_width * 3 / 5).max(400).min(w.beam_width)
        } else if n <= 32 {
            (w.beam_width / 5).max(300).min(w.beam_width)
        } else if n <= 48 {
            (w.beam_width / 8).max(200).min(w.beam_width)
        } else {
            // 【性能】超长句（>48 码）：尾键延迟实测 40-50ms——再降档
            // 换响应（bench 实测：/24 无额外收益，/16 max100 最优——
            // avg 49→24ms、p95 102→38ms、exact 90% 持平）
            (w.beam_width / 16).max(100).min(w.beam_width)
        };
        // 【增量尾窗束宽】见 inc_tail_beam() 注释：增量重算只算尾部
        // ≤12 键，小束承载尾段组合；全束质量保留在前部复用桶。
        let beam = if is_resume {
            beam.min(inc_tail_beam())
        } else {
            beam
        };
        // 【性能 2026-09-08】env 读取移出循环：Windows 上 env::var 走
        // 进程环境块+内部锁，原先在 beam 循环体内每位置读一次，48 码
        // 句每次解码白读 48 次（审计报告 E-3）。OnceLock 一次定型。
        static INC_DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let inc_debug = *INC_DEBUG.get_or_init(|| std::env::var("HUFU_INC_DEBUG").is_ok());
        for pos in start_pos..n {
            buckets[pos].limit(beam);
            if inc_debug {
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
                    //   b) 有选重锁（顶屏确认流，如 cn;j;c 的 c）。
                    // 短码且无锁（zhh/egy 类）不豁免——否则 zh(其)+h(道)
                    // 两字路径压过码表精确词「虎」，单字首选错位（用户
                    // 实测 zhh 首选变「其道」；虎爪/Rime 该码只有「虎」）。
                    let span = end - pos + if lock.is_some() { 1 } else { 0 };
                    if n > 1 && span < 2 {
                        // 尾段豁免仅限带锁（顶屏确认流，如「cn;j;c」「cbfe;u」
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
                        // 【隐式二选 2026-09-12 用户需求】整句录入时允许
                        // 3/4 码段的编码 2 选在无锁（不打选重键）情况下
                        // 参与组句：brybks 出「奴隶」（隶=bks 二选）、
                        // eyieqdk 出「桎梏」（桎=eyi 二选）。1/2 码段不放开
                        //（用户拍板：只限 3 码和 4 码的字，1 码 2 码不计入）；
                        // rank≥3 一律不放开（只放开「2 选」）。
                        // 【二十二修 2026-10-09 词不提权】隐式二选收窄为
                        // 「单字」：实现原先对多字词条同样放行（fyy 的
                        // 2 选「一点点」3 字词免选重组句+记账 rank1），
                        // 高频词在流式 raw≤4 窗口靠 LM+出字奖励结构性
                        // 压过 1 选单字，fyyciugk（下车维持）被抢跑成
                        // 「一点点…」——超出「的字」拍板范围，收回。
                        let implicit2 = lock.is_none()
                            && rank1b == 2
                            && text.chars().count() == 1
                            && {
                                let seg_keys = end - pos;
                                seg_keys == 3 || seg_keys == 4
                            };
                        if let Some(r) = lock {
                            if rank1b != r {
                                continue;
                            }
                        } else if !allow_all_ranks && rank1b != 1 && !implicit2 {
                            // >4 码无锁只取第 1 候选（隐式二选除外）
                            continue;
                        }
                        let mut ns = state.clone();
                        // 【二十二修·出字奖励按段计】原实现每输出一字加
                        // 一份 emitted_character_reward——多字词按字数放
                        // 大（「一点点」3 份 vs「下」1 份），词在分数上
                        // 结构性碾压 1 选单字=变相提权。用户拍板「词不
                        // 提权，单字该怎么样怎么样」：奖励改为每段一次
                        //（同段内单字/词同酬），字数优势只剩 LM 概率
                        // 本身。mass（提前上屏置信）同口径，抢跑虚高
                        // 一并消除。
                        ns.score += w.emitted_character_reward;
                        ns.mass += w.emitted_character_reward;
                        for c in text.chars() {
                            let cp = c as u32;
                            let p3 = self.model.trigram_prob(ns.prev2, ns.prev1, cp);
                            ns.score += (p3.max(1e-12).ln()) as f64;
                            ns.mass += (p3.max(1e-12).ln()) as f64;
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
                        // 【隐式二选记账】隐式二选段（无锁 3/4 码 rank2）
                        // 的 max_rank/sum_rank 按 1 记：排序门槛
                        //（max_rank 硬优先）与 rerank 深度约束
                        //（sum_rank=用户选重代价——用户没打选重键，没
                        // 付这个代价）都不得歧视它；分数竞争里的
                        // rank_penalty 折价保留——「权重（ngram 分数）
                        // 高才首选，不高就靠后」（用户拍板的排序语义）。
                        // 显式锁定的 rank2（打了 ;/'/数字）保持真实记
                        // 账——那是用户真实付出的选重代价。
                        let book_rank = if implicit2 { 1 } else { rank1b };
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
                        ns.max_rank = ns.max_rank.max(book_rank);
                        ns.sum_rank += book_rank;
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
                        if implicit2 {
                            ns.implicit2 = true;
                        }
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
                    implicit2: st.implicit2,
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
                    implicit2: st.implicit2,
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
                                implicit2: st.implicit2,
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
        let parsed_new = if self.weights.digit_codes {
            let dict = &self.dict;
            let is_code = |p: &str| {
                let cs: Vec<char> = p.chars().collect();
                (1..=cs.len()).any(|j| {
                    let s: String = cs[cs.len() - j..].iter().collect();
                    !dict.lookup(&s).is_empty() || !dict.completions(&s, 1).is_empty()
                })
            };
            parse_rank_locks_keep_digits(raw, &is_code)
        } else {
            parse_rank_locks(raw)
        };
        let new_base_len = parsed_new.base.chars().count();
        let can = cache
            .prefix
            .as_ref()
            .map(|(p_raw, buckets)| {
                let old_base_len = buckets.len().saturating_sub(1);
                let delta = new_base_len as isize - old_base_len as isize;
                if !raw.starts_with(p_raw.as_str()) || old_base_len < INC_MIN_PREFIX {
                    return false;
                }
                // 情形A（常规追加）：base 增长 1-3。
                if (1..=INC_MAX_DELTA as isize).contains(&delta) {
                    return true;
                }
                // 情形B（选重锁追加 2026-09-08）：raw 追加了 ≤4 字符而
                // base 没长 = 纯锁后缀（选重 ;' 数字）。锁字符不进 base
                //（桶结构不变），锁影响的段选择发生在尾部——重算区
                //（REDO_TAIL+4 回退）覆盖尾部锁位置，增量安全。跟打选重
                // 密集场景原本每选重一次 30000 全量重建（实测曲线
                // 52→501ms 爬升=「感觉又卡回去」的主源）。
                delta == 0 && raw.len() - p_raw.len() <= 4
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
        // 【修复 env 每调读 2026-09-11】原因：decode_cached 每次调用
        //（= 每键解码）同步读 HUFU_INC_DEBUG——Windows 上 env::var 走
        // 进程环境块+内部锁，热路径白读（decode_internal 内同类读取
        // 已于 2026-09-08 移出循环改 OnceLock，此处漏网）→ 手段：同款
        // OnceLock 一次定型。
        static INC_DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let dbg = *INC_DEBUG.get_or_init(|| std::env::var("HUFU_INC_DEBUG").is_ok());
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

    // 【去重 confidence_proposal 2026-09-11】此处原有一份
    // confidence_proposal 方法实现，与 hufu_engine::confidence_proposal
    // 自由函数近似重复（后者多返回份额值，且为全仓库唯一被使用的版本：
    // hufu-engine 提前上屏与 examples/sentenceprobe 均调用它；本方法
    // 无任何调用点，系迁移残余）→ 删除副本，调用点统一走
    // hufu_engine::confidence_proposal。
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

    /// 【用户词注入 2026-09-06】/jc 加词参与整句词图（热更新+缓存失效）
    fn set_user_words(&self, words: &[(String, String)]) {
        SentenceEngine::set_user_words(self, words.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hufu_dict::entry::DictEntry;
    use hufu_dict::freq;
    use hufu_dict::supplement::Supplement;

    /// 合成码表：覆盖组句里会碰到的五种条目形态。
    /// - 的（频序 1）：a / aa / aab —— 多码单字（含 1 码）
    /// - 一（频序 2）：b / bb / bbb —— 多码单字（边界上的第 N+1 个字）
    /// - 是（频序 3）：c —— 只有 1 个码的字（整句最优码取不到）
    /// - 鑫（表外）：dd / ddd —— 频序表外的字
    /// - 我们：ab —— 多字词
    fn tiny_dict() -> Dict {
        let mk = |code: &str, text: &str, weight: f64, seq: u32| DictEntry {
            weight,
            ..DictEntry::new(code, text, seq)
        };
        Dict::from_entries(
            "test",
            vec![
                mk("a", "的", 100.0, 0),
                mk("aa", "的", 90.0, 1),
                mk("aab", "的", 80.0, 2),
                mk("b", "一", 100.0, 3),
                mk("bb", "一", 90.0, 4),
                mk("bbb", "一", 80.0, 5),
                mk("c", "是", 100.0, 6),
                mk("dd", "鑫", 100.0, 7),
                mk("ddd", "鑫", 90.0, 8),
                mk("ab", "我们", 100.0, 9),
            ],
        )
    }

    fn engine_with_weights(dict: Dict, weights: SentenceWeights) -> SentenceEngine {
        SentenceEngine::with_model(
            model::tiny_model(),
            Arc::new(dict),
            &Supplement::default(),
            weights,
        )
    }

    /// 显式设定上限的引擎。
    fn engine(dict: Dict, limit: usize) -> SentenceEngine {
        engine_with_weights(
            dict,
            SentenceWeights {
                high_freq_limit: limit,
                ..Default::default()
            },
        )
    }

    /// 该字码表里「不是整句最优码」的那个 ≥2 码（测试数据保证存在）。
    fn non_best_code(dict: &Dict, text: &str) -> String {
        let best = dict.best_code_with_min_len(text, 2).expect("有 ≥2 码");
        dict.codes_of(text)
            .into_iter()
            .find(|code| code.chars().count() >= 2 && code != best)
            .expect("存在非最优码")
    }

    fn hits(eng: &SentenceEngine, raw: &str) -> Vec<String> {
        eng.decode_to_strings(raw)
    }

    /// 逐位转储（文本 + 分数 + 名次 + 精确位 + 分段 + 提前上屏），
    /// 供「上限 0 与不启用逐位一致」的严格比较。
    fn dump(eng: &SentenceEngine, raws: &[&str]) -> String {
        let mut out = String::new();
        for raw in raws {
            let dec = eng.decode_rich(raw);
            out.push_str(&format!(
                "raw={raw} truncated={} early_truncated={} hits={:?} early={:?}\n",
                dec.truncated, dec.early_truncated, dec.hits, dec.early_hits
            ));
        }
        out
    }

    const CRAFTED_RAWS: [&str; 12] = [
        "a", "aa", "aab", "aadd", "aabdd", "bbdd", "bbbdd", "c", "dd", "ddd", "dddd", "ab",
    ];

    /// 上下限为 0（默认，不限制）与不启用逐位一致：禁用表为空 ⇒ 判定对
    /// 码表任意条目恒 false，且非最优码路径照常参与组句（不是空转）。
    #[test]
    fn zero_limit_is_identical_to_disabled() {
        let off = engine(tiny_dict(), 0);
        // ① 上限 0 ⇒ 空表：零构建成本、热路径只做一次空表判断
        assert!(off.blocked.is_empty());
        // ② 判定恒 false：码表全部条目逐条验证（1 码单字 / 多码单字 / 词 / 表外字）
        let dict = tiny_dict();
        for e in &dict.entries {
            let len = e.code.chars().count();
            assert!(
                !off.entry_blocked(&e.code, &e.text, len),
                "上限 0 不得过滤任何条目: {} {}",
                e.code,
                e.text
            );
            assert!(!off.entry_blocked(&e.code, &e.text, 2));
        }
        // ③ 与「不显式设键」的默认权重引擎逐位一致（分数、名次、提前上屏全量比较）
        let disabled = engine_with_weights(tiny_dict(), SentenceWeights::default());
        assert_eq!(dump(&off, &CRAFTED_RAWS), dump(&disabled, &CRAFTED_RAWS));
        // ④ 非最优码路径确实在组句里（否则上面的一致性是空转）
        let dict = tiny_dict();
        let other = non_best_code(&dict, "的");
        let raw = format!("{other}dd");
        assert!(
            hits(&off, &raw).iter().any(|t| t == "的鑫"),
            "上限 0 时非最优码路径应参与组句: raw={raw} hits={:?}",
            hits(&off, &raw)
        );
    }

    /// 上限 N：前 N 高频字的非最优码单字段被剔除；最优码、1 码段、
    /// 多字词、表外字、最优码取不到的字都不受影响。
    #[test]
    fn limit_drops_non_best_code_of_top_n_single_chars() {
        let dict = tiny_dict();
        assert_eq!(freq::rank_of('的'), Some(1));
        assert_eq!(freq::rank_of('一'), Some(2));
        assert_eq!(freq::rank_of('是'), Some(3));
        assert!(freq::rank_of('鑫').is_none());
        let best_de = dict.best_code_with_min_len("的", 2).unwrap().to_string();
        let other_de = non_best_code(&dict, "的");
        assert_ne!(best_de, other_de);

        let off = engine(tiny_dict(), 0);
        let on1 = engine(tiny_dict(), 1); // 只覆盖频序 1（的）
        let on_all = engine(tiny_dict(), 4000);
        // 非最优码：不限制时在，上限 1 时该条目被丢弃
        let raw_other = format!("{other_de}dd");
        assert!(hits(&off, &raw_other).iter().any(|t| t == "的鑫"));
        assert!(!hits(&on1, &raw_other).iter().any(|t| t == "的鑫"));
        assert!(on1.blocked.contains_key(&'的'));
        assert!(!on1.blocked[&'的'].contains(&best_de));
        // 该字的 1 码条目（最优码之外）不进禁用表，也不被判丢弃
        assert!(!on1.blocked[&'的'].contains("a"));
        assert!(!on1.entry_blocked("a", "的", 1));
        // 最优码：上限 1 时照常参与组句
        let raw_best = format!("{best_de}dd");
        assert!(
            hits(&on1, &raw_best).iter().any(|t| t == "的鑫"),
            "最优码不应被过滤: raw={raw_best} hits={:?}",
            hits(&on1, &raw_best)
        );
        // 多字词不受影响
        assert!(hits(&on_all, "ab").iter().any(|t| t == "我们"));
        assert!(!on_all.entry_blocked("ab", "我们", 2));
        // 只有 1 码的字（最优码取不到）不受影响，也不进禁用表
        assert!(hits(&on_all, "c").iter().any(|t| t == "是"));
        assert!(!on_all.blocked.contains_key(&'是'));
        // 表外字的非最优码不受影响
        let other_xin = non_best_code(&dict, "鑫");
        let raw_xin = format!("{other_xin}dd");
        assert!(
            hits(&on_all, &raw_xin).iter().any(|t| t == "鑫鑫"),
            "表外字不应被过滤: raw={raw_xin} hits={:?}",
            hits(&on_all, &raw_xin)
        );
    }

    /// 频序边界：上限 N 覆盖第 N 个字、不覆盖第 N+1 个。
    #[test]
    fn limit_boundary_covers_exactly_n_chars() {
        let dict = tiny_dict();
        let raw_de = format!("{}dd", non_best_code(&dict, "的")); // 频序 1
        let raw_yi = format!("{}dd", non_best_code(&dict, "一")); // 频序 2
        let on1 = engine(tiny_dict(), 1);
        let on2 = engine(tiny_dict(), 2);
        // N=1：第 1 个字剔除，第 2 个字保留
        assert!(!hits(&on1, &raw_de).iter().any(|t| t == "的鑫"));
        assert!(
            hits(&on1, &raw_yi).iter().any(|t| t == "一鑫"),
            "上限 1 不该波及频序 2 的字: hits={:?}",
            hits(&on1, &raw_yi)
        );
        assert!(on1.blocked.contains_key(&'的'));
        assert!(!on1.blocked.contains_key(&'一'));
        // N=2：第 2 个字也剔除
        assert!(!hits(&on2, &raw_yi).iter().any(|t| t == "一鑫"));
        assert!(on2.blocked.contains_key(&'一'));
    }
}
