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
    cp2id: HashMap<u32, u16>,
    /// 码点 → 字频名次（1 起，按 unigram 概率降序；未收录 usize::MAX）
    freq_rank: HashMap<u32, usize>,
    unigram_p: Vec<f64>,
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
            || rd_u64(d, 16) as usize != d.len()
            || rd_u32(d, 24) != 5
            || rd_u32(d, 32) != 256
        {
            return Err(bad("布局"));
        }
        let qbytes = if version == 2 { 1 } else { 2 };
        let step_div = if version == 2 { 1e9 } else { 1e12 };
        let vocab_count = rd_u32(d, 28) as usize;
        let index_stride = rd_u32(d, 36) as usize;
        let vocab_offset = rd_u64(d, 40) as usize;
        let vocab_bytes = rd_u64(d, 48) as usize;
        let unknown_id = rd_u16(d, 56);
        let bos_id = rd_u16(d, 58);
        let eos_id = rd_u16(d, 60);
        if index_stride == 0 || vocab_offset + vocab_bytes > d.len() {
            return Err(bad("词表区间"));
        }
        let mut dirs: [Vec<Bucket>; 4] = Default::default();
        let mut quant: [Quant; 5] = [Quant { pmin: 0.0, pstep: 0.0, bmin: 0.0, bstep: 0.0 }; 5];
        for o in 0..4 {
            let dir_off = rd_u64(d, 64 + o * 24) as usize;
            let rec = rd_u64(d, 64 + o * 24 + 16) as usize;
            if dir_off + 256 * BUCKET_META > d.len() {
                return Err(bad("directory 区间"));
            }
            let mut v = Vec::with_capacity(256);
            for b in 0..256 {
                let p = dir_off + b * BUCKET_META;
                // p+0..16 是格式里的 blocks_offset：当前实现靠 index_offset/index_count/block_count
                // 定位，用不到，故不读进结构体（字段已删，格式说明见文件头）。
                v.push(Bucket {
                    index_offset: rd_u64(d, p + 16),
                    index_count: rd_u32(d, p + 24),
                    block_count: rd_u32(d, p + 28),
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
        let vb = &d[vocab_offset..vocab_offset + vocab_bytes];
        let mut cp2id = HashMap::with_capacity(vocab_count);
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
                cp2id.insert(cp, id as u16);
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
            cp2id,
            freq_rank,
            unigram_p,
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
            _ => self.cp2id.get(&cp).copied().unwrap_or(self.unknown_id),
        }
    }

    /// 单字符事件打分（线性域概率），等价 Lua score_history：
    /// 从最长可用历史（≤4）逐级回退。
    pub fn step_prob(&self, h: &[u32], cp: u32) -> f64 {
        let target = self.id_of(cp);
        let n = h.len().min(4);
        let mut total = 0.0f64; // log10 域回退累计
        for ctx_len in (1..=n).rev() {
            let start = h.len() - ctx_len;
            let mut ids = Vec::with_capacity(ctx_len);
            for &c in &h[start..] {
                ids.push(self.id_of(c));
            }
            if let Some((p, bow, observed)) = self.lookup(ctx_len + 1, &ids, target) {
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
        let offset = rd_u64(d, ip + 8) as usize;
        let finish = if entry + 1 < meta.index_count as usize {
            rd_u64(d, ip + INDEX_ENTRY + 8) as usize
        } else {
            meta.index_offset as usize
        };
        let qbytes = self.qbytes;
        let succ_bytes = 2 + qbytes;
        // 页内线性扫描（≤ index_stride 条块）
        let mut pos = offset;
        let block_hdr = ctx_len * 2 + qbytes + 2;
        while pos + block_hdr <= finish {
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
