//! TCSKNM03 五阶 ngram 模型加载与查询（2026-09-25 移植自 Rime 侧
//! tiger_sentence_fivegram.lua 参考实现，布局以其 docs/TCSKNM03.md 为准）。
//!
//! 文件布局（小端，256 字节头）：
//! ```text
//! magic[8]="TCSKNM03", version:u32@8(1=Q16/2=Q8), header_size:u32@12=256,
//! file_size:u64@16, order:u32@24=5, vocab_count:u32@28, 256:u32@32,
//! index_stride:u32@36, vocab_offset:u64@40, vocab_bytes:u64@48,
//! unknown:u16@56, bos:u16@58, eos:u16@60,
//! sections[2..5] @64+（各 24B）: directory_offset:u64, block_count:u64, record_count:u64
//! quant[5] @160+（各 16B）: pmin:i32(1e-7), pstep:u32(1e-9@v2/1e-12@v1),
//!                           bmin:i32(1e-7), bstep:u32(1e-9@v2/1e-12@v1)
//! ```
//! 之后为词表段与 2..5 阶的 256 桶 directory（每桶 40B）：
//! blocks_offset:u64, blocks_bytes:u64, index_offset:u64,
//! index_count:u32, block_count:u32, record_count:u64。
//! 索引项 16B = 上下文 id(u16×(order-1)) + 页偏移 u64。
//! 块记录 = 上下文 id(u16×(order-1)) + 回退量化码 + 后继数 u16
//!          + 后继[] × { id:u16, 量化码 }。
//! 查询链：5→4→3→2 阶逐级回退（ARPA 风格，观察→用其概率，
//! 未观察→累计回退权再降阶），兜底 unigram。

use memmap2::Mmap;
use std::collections::HashMap;
use std::path::Path;

const MAGIC: &[u8; 8] = b"TCSKNM03";
const HEADER_SIZE: usize = 256;
const BUCKET_META: usize = 40;
const INDEX_ENTRY: usize = 16;

/// 五阶模型（mmap 只读驻留）。
pub struct FivegramModel {
    data: Mmap,
    pub index_stride: usize,
    pub(crate) vocab_count: usize,
    unknown_id: u16,
    bos_id: u16,
    eos_id: u16,
    /// 量化字节宽（v2=1, v1=2）
    qbytes: usize,
    /// 2..=5 阶的 256 桶 directory
    dirs: [Vec<Bucket>; 4],
    /// 各阶量化参数（下标 order-2：quant[0]=unigram … quant[4]=5 阶）
    quant: [Quant; 5],
    /// 码点 → 词表 id（单字 token；BOS/EOS 特判 0x02/0x03）
    /// 【id 查询直表 2026-11-09】step_prob 每事件 id_of×5（4 历史+目标），
    /// 原单表 HashMap（SipHash 键）是解码内层最热的查询之一；BMP 拆
    /// 65536 槽直表（Option<u16>，miss=None），非 BMP（增补平面字，
    /// 词表极少）留小 HashMap。两源内容与原单表逐条相同、miss 同走
    /// unknown_id，映射语义零变化。
    cp2id_bmp: Vec<Option<u16>>,
    cp2id_rest: HashMap<u32, u16>,
    /// 码点 → 字频名次（1 起，按 unigram 概率降序；未收录 usize::MAX）
    freq_rank: HashMap<u32, usize>,
    unigram_p: Vec<f64>,
    #[cfg(test)]
    test_queries: std::sync::atomic::AtomicUsize,
}

#[derive(Clone, Copy)]
struct Quant {
    pmin: f64,
    pstep: f64,
    bmin: f64,
    bstep: f64,
}

#[derive(Clone, Copy)]
struct Bucket {
    index_offset: u64,
    index_count: u32,
    block_count: u32,
}

#[inline]
fn rd_u16(d: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([d[off], d[off + 1]])
}
#[inline]
fn rd_u32(d: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
}
#[inline]
fn rd_u64(d: &[u8], off: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[off..off + 8]);
    u64::from_le_bytes(b)
}

impl FivegramModel {
    pub fn load(path: &Path) -> std::io::Result<FivegramModel> {
        let file = std::fs::File::open(path)?;
        let map = unsafe { memmap2::MmapOptions::new().map(&file)? };
        Self::build(map)
    }

    fn build(data: Mmap) -> std::io::Result<FivegramModel> {
        let bad = |what: &str| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("TCSKNM03 模型无效（{what}）"),
            )
        };
        let d: &[u8] = &data;
        if d.len() < HEADER_SIZE || &d[0..8] != MAGIC {
            return Err(bad("魔数"));
        }
        let version = rd_u32(d, 8);
        if version != 1 && version != 2 {
            return Err(bad("version"));
        }
        if rd_u32(d, 12) as usize != HEADER_SIZE
            || rd_u64(d, 16) != d.len() as u64
            || rd_u32(d, 24) != 5
            || rd_u32(d, 32) != 256
        {
            return Err(bad("布局"));
        }
        let qbytes = if version == 2 { 1 } else { 2 };
        let step_div = if version == 2 { 1e9 } else { 1e12 };
        let vocab_count = rd_u32(d, 28) as usize;
        let index_stride = rd_u32(d, 36) as usize;
        let vocab_offset = usize::try_from(rd_u64(d, 40)).map_err(|_| bad("词表区间"))?;
        let vocab_bytes = usize::try_from(rd_u64(d, 48)).map_err(|_| bad("词表区间"))?;
        let unknown_id = rd_u16(d, 56);
        let bos_id = rd_u16(d, 58);
        let eos_id = rd_u16(d, 60);
        let vocab_end = vocab_offset.checked_add(vocab_bytes)
            .filter(|&end| end <= d.len()).ok_or_else(|| bad("词表区间"))?;
        if index_stride == 0 {
            return Err(bad("词表区间"));
        }
        // 【P0-3 2026-11-03】vocab_count 只从头部读、未与 vocab_bytes
        // 关联：畸形/截断模型可声明 0xFFFFFFFF → with_capacity 申请约
        // 20~40GB → handle_alloc_error abort 整个 server（输入法中断）。
        // 每条词表记录至少 2B 长度 + 2×qbytes 概率，按字节区间封顶。
        let min_entry = 2usize + 2 * qbytes;
        let max_vocab = (vocab_bytes / min_entry).min(1_000_000).max(1);
        if vocab_count == 0 || vocab_count > max_vocab || vocab_count > u16::MAX as usize + 1 {
            return Err(bad("词表数量越界"));
        }
        if [unknown_id, bos_id, eos_id].iter().any(|&id| id as usize >= vocab_count) {
            return Err(bad("特殊词表 id 越界"));
        }
        let mut dirs: [Vec<Bucket>; 4] = Default::default();
        let mut quant: [Quant; 5] = [Quant { pmin: 0.0, pstep: 0.0, bmin: 0.0, bstep: 0.0 }; 5];
        for o in 0..4 {
            let dir_off = usize::try_from(rd_u64(d, 64 + o * 24))
                .map_err(|_| bad("directory 区间"))?;
            let rec = rd_u64(d, 64 + o * 24 + 16) as usize;
            if dir_off.checked_add(256 * BUCKET_META).filter(|&end| end <= d.len()).is_none() {
                return Err(bad("directory 区间"));
            }
            let mut v = Vec::with_capacity(256);
            for b in 0..256 {
                let p = dir_off + b * BUCKET_META;
                // p+0..16 是格式里的 blocks_offset：当前实现靠 index_offset/index_count/block_count
                // 定位，用不到，故不读进结构体（字段已删，格式说明见文件头）。
                let io = rd_u64(d, p + 16);
                let ic = rd_u32(d, p + 24) as usize;
                let bc = rd_u32(d, p + 28) as usize;
                // 【P0-3 补充】计数不得超过文件本身，防止后续按
                // index_count/block_count 做无界分配。
                let index_end = io.checked_add((ic as u64) * INDEX_ENTRY as u64);
                if index_end.filter(|&end| end <= d.len() as u64).is_none() || bc > d.len() {
                    return Err(bad("bucket 计数越界"));
                }
                v.push(Bucket {
                    index_offset: io,
                    index_count: ic as u32,
                    block_count: bc as u32,
                });
                let _ = rec;
            }
            dirs[o] = v;
        }
        for i in 0..5 {
            let p = 160 + i * 16;
            let sgn = |x: i32| x as f64 / 1e7;
            quant[i] = Quant {
                pmin: sgn(rd_u32(d, p) as i32),
                pstep: rd_u32(d, p + 4) as f64 / step_div,
                bmin: sgn(rd_u32(d, p + 8) as i32),
                bstep: rd_u32(d, p + 12) as f64 / step_div,
            };
        }
        // 词表：id 顺序 { len:u16, token:u8[len], p:q, b:q }；token 为 UTF-8。
        let vb = &d[vocab_offset..vocab_end];
        let mut cp2id_bmp: Vec<Option<u16>> = vec![None; 0x1_0000];
        let mut cp2id_rest: HashMap<u32, u16> = HashMap::new();
        let mut id2cp: Vec<Option<u32>> = Vec::with_capacity(vocab_count);
        let mut unigram_p = Vec::with_capacity(vocab_count);
        let mut pos = 0usize;
        for id in 0..vocab_count {
            if pos + 2 > vb.len() {
                return Err(bad("词表截断"));
            }
            let len = rd_u16(vb, pos) as usize;
            pos += 2;
            if pos + len + qbytes * 2 > vb.len() {
                return Err(bad("词表截断"));
            }
            let token = &vb[pos..pos + len];
            pos += len;
            let p = decode_prob(vb, pos, qbytes, quant[0]);
            pos += qbytes * 2;
            unigram_p.push(p);
            // 单字 token（1 个 UTF-8 字符）
            let mut one = None;
            if let Ok(s) = std::str::from_utf8(token) {
                let mut it = s.chars();
                if let (Some(c), None) = (it.next(), it.next()) {
                    one = Some(c as u32);
                }
            }
            if let Some(cp) = one {
                if (cp as usize) < 0x1_0000 {
                    cp2id_bmp[cp as usize] = Some(id as u16);
                } else {
                    cp2id_rest.insert(cp, id as u16);
                }
            }
            id2cp.push(one);
        }
        if pos != vb.len() {
            return Err(bad("词表长度"));
        }
        // 字频名次（1 起，概率降序）
        let mut order: Vec<usize> = (0..vocab_count).collect();
        order.sort_by(|&a, &b| unigram_p[b].partial_cmp(&unigram_p[a]).unwrap_or(std::cmp::Ordering::Equal));
        let mut freq_rank = HashMap::with_capacity(vocab_count);
        for (rank, &id) in order.iter().enumerate() {
            if let Some(cp) = id2cp[id] {
                freq_rank.insert(cp, rank + 1);
            }
        }
        Ok(FivegramModel {
            data,
            index_stride,
            vocab_count,
            unknown_id,
            bos_id,
            eos_id,
            qbytes,
            dirs,
            quant,
            cp2id_bmp,
            cp2id_rest,
            freq_rank,
            unigram_p,
            #[cfg(test)]
            test_queries: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    #[inline]
    /// 码点字频名次（1 起；未收录 usize::MAX）——engine 生僻字判据。
    pub(crate) fn freq_rank(&self, cp: u32) -> usize {
        self.freq_rank.get(&cp).copied().unwrap_or(usize::MAX)
    }

    pub(crate) fn id_of(&self, cp: u32) -> u16 {
        match cp {
            0x02 => self.bos_id,
            0x03 => self.eos_id,
            // 【id 查询直表 2026-11-09】BMP 走直表下标，非 BMP 走残留
            // 小表；两路 miss 都落 unknown_id——与原单表 HashMap 语义
            // 逐位一致（0x02/0x03 是控制字符，不进词表，特判恒先命中）。
            _ => {
                if (cp as usize) < 0x1_0000 {
                    self.cp2id_bmp[cp as usize].unwrap_or(self.unknown_id)
                } else {
                    self.cp2id_rest.get(&cp).copied().unwrap_or(self.unknown_id)
                }
            }
        }
    }

    /// 单字符事件打分（线性域概率），等价 Lua score_history：
    /// 从最长可用历史（≤4）逐级回退。
    pub fn step_prob(&self, h: &[u32], cp: u32) -> f64 {
        #[cfg(test)]
        self.test_queries.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let target = self.id_of(cp);
        let n = h.len().min(4);
        // 每个事件最多四字历史。各阶共用同一栈上 id 尾巴，避免每阶
        // 重新分配 Vec、重复哈希查词（长句 beam 的高频评分路径）。
        let mut history_ids = [0u16; 4];
        for (id, &c) in history_ids[..n].iter_mut().zip(&h[h.len() - n..]) {
            *id = self.id_of(c);
        }
        let mut total = 0.0f64; // log10 域回退累计
        for ctx_len in (1..=n).rev() {
            if let Some((p, bow, observed)) = self.lookup(ctx_len + 1, &history_ids[n - ctx_len..n], target) {
                if observed {
                    return 10f64.powf(total + p);
                }
                total += bow;
            }
        }
        let uni = self.unigram_p.get(target as usize).copied().unwrap_or(0.0);
        10f64.powf(total + uni)
    }

    /// (order, 上下文 ids, 目标 id) → (log10 概率, log10 回退, 是否观察)。
    pub(crate) fn lookup(&self, order: usize, ids: &[u16], target: u16) -> Option<(f64, f64, bool)> {
        if !(2..=5).contains(&order) || ids.len() != order - 1 {
            return None;
        }
        let o = order - 2; // dirs 下标
        let first = ids[0] as usize;
        let meta = &self.dirs[o][first % 256];
        if meta.block_count == 0 || meta.index_count == 0 {
            return None;
        }
        let d: &[u8] = &self.data;
        let idx = meta.index_offset as usize;
        let ctx_len = order - 1;
        // 二分找最后一个 ctx ≤ ids 的索引项
        let mut lo = 0usize;
        let mut hi = meta.index_count as usize;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let p = idx + mid * INDEX_ENTRY;
            let mut cmp = std::cmp::Ordering::Equal;
            for i in 0..ctx_len {
                let a = rd_u16(d, p + i * 2);
                let b = ids[i];
                if a < b {
                    cmp = std::cmp::Ordering::Less;
                    break;
                } else if a > b {
                    cmp = std::cmp::Ordering::Greater;
                    break;
                }
            }
            if cmp != std::cmp::Ordering::Greater {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            return None;
        }
        let entry = lo - 1;
        let ip = idx + entry * INDEX_ENTRY;
        let offset = usize::try_from(rd_u64(d, ip + 8)).ok()?;
        let finish = if entry + 1 < meta.index_count as usize {
            usize::try_from(rd_u64(d, ip + INDEX_ENTRY + 8)).ok()?
        } else {
            meta.index_offset as usize
        };
        // 索引区已在装载时验证；页指针与后继计数仍来自模型字节。
        // 坏页视为未观测，沿正常回退链继续，不能把按键线程打崩。
        if offset > finish || finish > idx {
            return None;
        }
        let qbytes = self.qbytes;
        let succ_bytes = 2 + qbytes;
        // 页内线性扫描（≤ index_stride 条块）
        let mut pos = offset;
        let block_hdr = ctx_len * 2 + qbytes + 2;
        let mut blocks_left = self.index_stride.min(meta.block_count as usize);
        while blocks_left > 0 && finish - pos >= block_hdr {
            blocks_left -= 1;
            let mut cmp = std::cmp::Ordering::Equal;
            for i in 0..ctx_len {
                let a = rd_u16(d, pos + i * 2);
                let b = ids[i];
                if a < b {
                    cmp = std::cmp::Ordering::Less;
                    break;
                } else if a > b {
                    cmp = std::cmp::Ordering::Greater;
                    break;
                }
            }
            let bow_q = read_q(d, pos + ctx_len * 2, qbytes);
            let count = rd_u16(d, pos + ctx_len * 2 + qbytes) as usize;
            let succ = pos + block_hdr;
            if count > (finish - succ) / succ_bytes {
                return None;
            }
            if cmp == std::cmp::Ordering::Equal {
                // 后继按 id 升序二分
                let (mut lo, mut hi) = (0usize, count);
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    let v = rd_u16(d, succ + mid * succ_bytes);
                    if v < target {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                let bow = decode_backoff(bow_q, self.quant[order - 2]);
                if lo < count && rd_u16(d, succ + lo * succ_bytes) == target {
                    let p = decode_prob(d, succ + lo * succ_bytes + 2, qbytes, self.quant[order - 1]);
                    return Some((p, bow, true));
                }
                return Some((0.0, bow, false));
            } else if cmp == std::cmp::Ordering::Greater {
                return None;
            }
            pos = succ + count * succ_bytes;
        }
        None
    }
}

impl std::fmt::Debug for FivegramModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FivegramModel")
            .field("vocab_count", &self.vocab_count)
            .field("index_stride", &self.index_stride)
            .finish()
    }
}

#[inline]
fn read_q(d: &[u8], off: usize, qbytes: usize) -> u32 {
    if qbytes == 1 {
        d[off] as u32
    } else {
        rd_u16(d, off) as u32
    }
}

#[cfg(test)]
impl FivegramModel {
    pub(crate) fn reset_test_queries(&self) {
        self.test_queries.store(0, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn test_queries(&self) -> usize {
        self.test_queries.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// A real NM03/Q8 file with all backoff orders, four-character context
/// dependence, special tokens, and an astral character. Keep this in memory so
/// sentence cache tests do not depend on an installed model or temporary files.
#[cfg(test)]
pub(crate) fn probability_cache_test_model() -> FivegramModel {
    fn put32(d: &mut [u8], at: usize, value: u32) {
        d[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn put64(d: &mut [u8], at: usize, value: u64) {
        d[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
    type Record = (Vec<u16>, u8, Vec<(u16, u8)>);
    let orders: [Vec<Record>; 4] = [
        vec![
            (vec![1], 41, vec![(3, 240), (8, 230)]),
            (vec![3], 51, vec![(3, 220), (4, 230)]),
            (vec![4], 61, vec![(5, 210)]),
            (vec![5], 71, vec![(6, 200)]),
            (vec![6], 81, vec![(7, 180)]),
            (vec![7], 91, vec![(2, 170), (8, 190)]),
            (vec![12], 31, vec![(13, 150)]),
        ],
        vec![
            (vec![1, 1], 41, vec![(3, 235), (8, 225)]),
            (vec![3, 4], 51, vec![(5, 245)]),
            (vec![8, 4], 61, vec![(5, 205)]),
            (vec![4, 5], 71, vec![(6, 215)]),
        ],
        vec![
            (vec![1, 1, 1], 41, vec![(3, 250), (8, 240)]),
            (vec![3, 4, 5], 51, vec![(6, 240)]),
            (vec![8, 4, 5], 61, vec![(6, 220)]),
            (vec![4, 5, 6], 71, vec![(2, 120), (7, 190)]),
        ],
        vec![
            (vec![1, 1, 1, 1], 41, vec![(3, 245), (8, 235)]),
            (vec![1, 1, 1, 3], 51, vec![(4, 230)]),
            (vec![1, 1, 1, 8], 61, vec![(4, 225)]),
            (vec![3, 4, 5, 6], 71, vec![(2, 70), (7, 250)]),
            (vec![8, 4, 5, 6], 81, vec![(2, 180), (7, 100)]),
            (vec![4, 5, 6, 7], 91, vec![(2, 210)]),
        ],
    ];
    let tokens = ["<unk>", "<s>", "</s>", "甲", "乙", "丙", "丁", "戊", "己", "庚", "辛", "壬", "癸", "𠀀"];
    let mut d = vec![0; HEADER_SIZE + 4 * 256 * BUCKET_META];
    d[..8].copy_from_slice(MAGIC);
    put32(&mut d, 8, 2);
    put32(&mut d, 12, HEADER_SIZE as u32);
    put32(&mut d, 24, 5);
    put32(&mut d, 28, tokens.len() as u32);
    put32(&mut d, 32, 256);
    put32(&mut d, 36, 2);
    d[58..60].copy_from_slice(&1u16.to_le_bytes());
    d[60..62].copy_from_slice(&2u16.to_le_bytes());
    for i in 0..5 {
        put32(&mut d, 160 + i * 16, (-40_000_000i32) as u32);
        put32(&mut d, 164 + i * 16, 10_000_000);
        put32(&mut d, 168 + i * 16, (-10_000_000i32) as u32);
        put32(&mut d, 172 + i * 16, 10_000_000);
    }
    let vocab = d.len();
    for (id, token) in tokens.iter().enumerate() {
        d.extend_from_slice(&(token.len() as u16).to_le_bytes());
        d.extend_from_slice(token.as_bytes());
        d.extend_from_slice(&[if id == 0 { 5 } else { 100 + id as u8 }, 0]);
    }
    let vocab_bytes = d.len() - vocab;
    put64(&mut d, 40, vocab as u64);
    put64(&mut d, 48, vocab_bytes as u64);
    for (o, records) in orders.into_iter().enumerate() {
        let directory = HEADER_SIZE + o * 256 * BUCKET_META;
        put64(&mut d, 64 + o * 24, directory as u64);
        put64(&mut d, 72 + o * 24, records.len() as u64);
        put64(&mut d, 80 + o * 24, records.iter().map(|r| r.2.len() as u64).sum());
        let mut buckets: std::collections::BTreeMap<u16, Vec<Record>> = Default::default();
        for record in records {
            buckets.entry(record.0[0] % 256).or_default().push(record);
        }
        for (first, mut records) in buckets {
            records.sort_by(|a, b| a.0.cmp(&b.0));
            let block = d.len();
            let mut indices = Vec::new();
            let mut successors = 0;
            for (i, (context, backoff, mut targets)) in records.iter().cloned().enumerate() {
                if i % 2 == 0 {
                    indices.push((context.clone(), d.len()));
                }
                for id in context { d.extend_from_slice(&id.to_le_bytes()); }
                d.push(backoff);
                d.extend_from_slice(&(targets.len() as u16).to_le_bytes());
                successors += targets.len();
                targets.sort_unstable_by_key(|&(id, _)| id);
                for (id, prob) in targets {
                    d.extend_from_slice(&id.to_le_bytes());
                    d.push(prob);
                }
            }
            let index = d.len();
            for (context, offset) in &indices {
                let entry = d.len();
                d.resize(entry + INDEX_ENTRY, 0);
                for (i, id) in context.iter().enumerate() {
                    d[entry + i * 2..entry + i * 2 + 2].copy_from_slice(&id.to_le_bytes());
                }
                put64(&mut d, entry + 8, *offset as u64);
            }
            let meta = directory + first as usize * BUCKET_META;
            put64(&mut d, meta, block as u64);
            put64(&mut d, meta + 8, (index - block) as u64);
            put64(&mut d, meta + 16, index as u64);
            put32(&mut d, meta + 24, indices.len() as u32);
            put32(&mut d, meta + 28, records.len() as u32);
            put64(&mut d, meta + 32, successors as u64);
        }
    }
    let len = d.len();
    put64(&mut d, 16, len as u64);
    let mut map = memmap2::MmapMut::map_anon(d.len()).unwrap();
    map.copy_from_slice(&d);
    FivegramModel::build(map.make_read_only().unwrap()).unwrap()
}

#[inline]
fn decode_prob(d: &[u8], off: usize, qbytes: usize, q: Quant) -> f64 {
    q.pmin + read_q(d, off, qbytes) as f64 * q.pstep
}

#[inline]
fn decode_backoff(qv: u32, q: Quant) -> f64 {
    if qv == 0 {
        0.0
    } else {
        q.bmin + (qv - 1) as f64 * q.bstep
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_u32(d: &mut [u8], p: usize, v: u32) {
        d[p..p + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn set_u64(d: &mut [u8], p: usize, v: u64) {
        d[p..p + 8].copy_from_slice(&v.to_le_bytes());
    }
    fn tiny_bytes() -> Vec<u8> {
        let mut d = vec![0u8; HEADER_SIZE + 4 * 256 * BUCKET_META];
        d[..8].copy_from_slice(MAGIC);
        set_u32(&mut d, 8, 2);
        set_u32(&mut d, 12, HEADER_SIZE as u32);
        set_u32(&mut d, 24, 5);
        set_u32(&mut d, 28, 3);
        set_u32(&mut d, 32, 256);
        set_u32(&mut d, 36, 1);
        for o in 0..4 {
            set_u64(&mut d, 64 + o * 24, (HEADER_SIZE + o * 256 * BUCKET_META) as u64);
        }
        for i in 0..5 {
            set_u32(&mut d, 160 + i * 16, (-20_000_000i32) as u32);
            set_u32(&mut d, 164 + i * 16, 10_000_000);
        }
        let vocab = d.len();
        for token in ["<unk>", "甲", "乙"] {
            d.extend_from_slice(&(token.len() as u16).to_le_bytes());
            d.extend_from_slice(token.as_bytes());
            d.extend_from_slice(&[100, 0]);
        }
        let vocab_bytes = d.len() - vocab;
        set_u64(&mut d, 40, vocab as u64);
        set_u64(&mut d, 48, vocab_bytes as u64);
        let block = d.len();
        // bigram 甲(id=1) -> 乙(id=2), log10 p=-1.5.
        d.extend_from_slice(&[1, 0, 0, 1, 0, 2, 0, 50]);
        let index = d.len();
        d.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]);
        d.extend_from_slice(&(block as u64).to_le_bytes());
        let bucket = HEADER_SIZE + BUCKET_META;
        set_u64(&mut d, bucket, block as u64);
        set_u64(&mut d, bucket + 8, (index - block) as u64);
        set_u64(&mut d, bucket + 16, index as u64);
        set_u32(&mut d, bucket + 24, 1);
        set_u32(&mut d, bucket + 28, 1);
        set_u64(&mut d, bucket + 32, 1);
        let len = d.len();
        set_u64(&mut d, 16, len as u64);
        d
    }
    fn from_bytes(d: &[u8]) -> std::io::Result<FivegramModel> {
        let mut map = memmap2::MmapMut::map_anon(d.len())?;
        map.copy_from_slice(d);
        FivegramModel::build(map.make_read_only()?)
    }

    #[test]
    fn tiny_fivegram_queries() {
        let m = from_bytes(&tiny_bytes()).unwrap();
        assert_eq!(m.id_of('甲' as u32), 1);
        let observed = m.step_prob(&['甲' as u32], '乙' as u32);
        assert!((observed - 10f64.powf(-1.5)).abs() < 1e-10);
        for history in [vec!['乙', '甲'], vec!['乙', '乙', '乙', '甲'], vec!['甲', '乙', '乙', '乙', '甲']] {
            let history: Vec<u32> = history.into_iter().map(|c| c as u32).collect();
            assert!((m.step_prob(&history, '乙' as u32) - observed).abs() < 1e-10);
        }
        assert!((m.step_prob(&[], '乙' as u32) - 0.1).abs() < 1e-10);
        let unseen = m.step_prob(&['乙' as u32], '甲' as u32);
        assert!((unseen - 0.1).abs() < 1e-10);
    }

    #[test]
    fn overflowing_layout_rejected() {
        for (off, value) in [(40, u64::MAX), (48, u64::MAX), (64, u64::MAX)] {
            let mut d = tiny_bytes();
            set_u64(&mut d, off, value);
            assert!(from_bytes(&d).is_err(), "bad offset at {off}");
        }
    }

    #[test]
    fn truncated_bucket_index_rejected() {
        let mut d = tiny_bytes();
        let len = d.len();
        set_u64(&mut d, HEADER_SIZE + BUCKET_META + 16, (len - 1) as u64);
        assert!(from_bytes(&d).is_err());
    }

    #[test]
    fn corrupt_page_and_successors_do_not_panic() {
        let original = tiny_bytes();
        let bucket = HEADER_SIZE + BUCKET_META;
        let index = rd_u64(&original, bucket + 16) as usize;
        let block = rd_u64(&original, index + 8) as usize;
        for pointer in [u64::MAX, original.len() as u64 + 1, index as u64 + 1] {
            let mut d = original.clone();
            set_u64(&mut d, index + 8, pointer);
            let m = from_bytes(&d).unwrap();
            assert!(m.lookup(2, &[1], 2).is_none());
        }
        let mut d = original;
        d[block + 3..block + 5].copy_from_slice(&u16::MAX.to_le_bytes());
        let m = from_bytes(&d).unwrap();
        assert!(m.lookup(2, &[1], 2).is_none());
        assert!((m.step_prob(&['甲' as u32], '乙' as u32) - 0.1).abs() < 1e-10);
    }

    /// 【id 查询直表 2026-11-09】直表+残留小表的 id_of 与「单表 HashMap」
    /// 原实现逐 cp 一致：BMP 全域扫描 + 增补平面采样（fixture 含 𠀀）
    /// + BOS/EOS 特判 + unknown 兜底。
    #[test]
    fn id_of_direct_table_matches_flat_map() {
        let m = probability_cache_test_model();
        let mut flat: HashMap<u32, u16> = HashMap::new();
        for (cp, v) in m.cp2id_bmp.iter().enumerate() {
            if let Some(id) = v {
                flat.insert(cp as u32, *id);
            }
        }
        flat.extend(m.cp2id_rest.iter().map(|(&k, &v)| (k, v)));
        let reference = |cp: u32| match cp {
            0x02 => m.bos_id,
            0x03 => m.eos_id,
            _ => flat.get(&cp).copied().unwrap_or(m.unknown_id),
        };
        for cp in 0u32..0x1_1000 {
            assert_eq!(m.id_of(cp), reference(cp), "cp={cp:#x}");
        }
        for cp in [0x1_1000u32, 0x2_0000, 0x2_0001, 0x1F_600, 0x10_FFFF] {
            assert_eq!(m.id_of(cp), reference(cp), "cp={cp:#x}");
        }
        assert!(flat.len() >= 1, "fixture 至少含一个词表字");
    }
}
