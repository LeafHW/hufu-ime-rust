//! TCSKNM02 ngram 模型加载与查询（布局已对真实文件校验）。
//!
//! 文件布局（小端，104 字节头）：
//! ```text
//! magic[8]="TCSKNM02", version:i32@8, header_size:i32@12,
//! file_size:i64@16, index_stride:i64@24, uni_count:i64@32, uni_off:i64@40,
//! bi_ctx_count:i32@48, bi_index_count:i32@52,
//! bi_blocks_off:i64@56, bi_index_off:i64@64,
//! tri_ctx_count:i64@72, tri_index_count:i32@80,
//! tri_blocks_off:i64@88, tri_index_off:i64@96
//!
//! unigram: uni_count × { cp:i32, prob:f32 }（按码点升序；[0]=(unknown, 回退概率)）
//! bigram/trigram 分页块: 每页 index_stride 个上下文，页内按 key 升序
//!   上下文记录 { key:i64, lambda:f32, succ_count:i32, succ[succ_count] × { cp:i32, prob:f32 } }
//! 页索引: index_count × { first_ctx_key:i64, block_offset:i64 }（稀疏，按 first_key 升序）
//!
//! 键：bigram ctx = 前一字符码点（BOS=0x02）；trigram ctx = w1×2^21 + w2
//! 概率：P2(c|w) = p2 + λ2(w)×P1(c)；P3(c|w1,w2) = p3 + λ3(w1,w2)×P2(c|w2)
//! ```

use std::collections::HashMap;
use std::path::Path;

const MAGIC: &[u8; 8] = b"TCSKNM02";
pub const BOS: u32 = 0x02;
pub const EOS: u32 = 0x03;

/// 模型数据源：mmap 只读映射（生产）或堆 Vec（测试）。
/// 【性能】214MB 模型改 mmap：私有内存 -214MB（页缓存承载可被
/// 系统换出），启动免同步全量读盘（冷启动提速）；查询路径
/// （rd_* 按字节读）完全不变。
enum ModelData {
    Map(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl std::ops::Deref for ModelData {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        match self {
            ModelData::Map(m) => m.as_ref(),
            ModelData::Owned(v) => v.as_slice(),
        }
    }
}

/// 已加载的 ngram 模型（数据段 mmap 驻留，索引二分查询）。
pub struct NgramModel {
    data: ModelData,
    pub index_stride: usize,
    uni_count: usize,
    uni_off: usize,
    bi_blocks_off: usize,
    bi_index_off: usize,
    bi_index_count: usize,
    tri_blocks_off: usize,
    tri_index_off: usize,
    tri_index_count: usize,
    /// 码点 → unigram 序号
    uni_pos: HashMap<u32, usize>,
    /// 码点 → 字频名次（1 起，按概率降序）
    freq_rank: HashMap<u32, usize>,
}

#[inline]
fn rd_i32(d: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
}
#[inline]
fn rd_f32(d: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
}
#[inline]
fn rd_i64(d: &[u8], off: usize) -> i64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[off..off + 8]);
    i64::from_le_bytes(b)
}
#[inline]
fn pack_key(first: u32, second: u32) -> i64 {
    (first as i64) * (1 << 21) + (second as i64)
}

impl NgramModel {
    pub fn load(path: &Path) -> std::io::Result<NgramModel> {
        // mmap 只读映射（惰性换入：首查触发缺页，冷启动不再同步
        // 读整文件 214MB）。独占写场景不存在——模型文件只读。
        let file = std::fs::File::open(path)?;
        let map = unsafe { memmap2::MmapOptions::new().map(&file)? };
        Self::build(ModelData::Map(map))
    }

    pub fn from_bytes(data: Vec<u8>) -> std::io::Result<NgramModel> {
        Self::build(ModelData::Owned(data))
    }

    fn build(data: ModelData) -> std::io::Result<NgramModel> {
        let data_ref: &[u8] = &data;
        if data_ref.len() < 104 || &data_ref[0..8] != MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "非 TCSKNM02 模型文件（魔数不符）",
            ));
        }
        let index_stride = rd_i64(data_ref, 24) as usize;
        let uni_count = rd_i64(data_ref, 32) as usize;
        let uni_off = rd_i64(data_ref, 40) as usize;
        let bi_index_count = rd_i32(data_ref, 52) as usize;
        let bi_blocks_off = rd_i64(data_ref, 56) as usize;
        let bi_index_off = rd_i64(data_ref, 64) as usize;
        let tri_index_count = rd_i32(data_ref, 80) as usize;
        let tri_blocks_off = rd_i64(data_ref, 88) as usize;
        let tri_index_off = rd_i64(data_ref, 96) as usize;

        let mut uni_pos = HashMap::with_capacity(uni_count);
        let mut unigrams: Vec<(u32, f32)> = Vec::with_capacity(uni_count);
        for i in 0..uni_count {
            let off = uni_off + i * 8;
            let cp = rd_i32(data_ref, off) as u32;
            let p = rd_f32(data_ref, off + 4);
            uni_pos.insert(cp, i);
            unigrams.push((cp, p));
        }
        let mut order: Vec<usize> = (0..unigrams.len()).collect();
        order.sort_by(|&a, &b| {
            unigrams[b]
                .1
                .partial_cmp(&unigrams[a].1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let freq_rank: HashMap<u32, usize> = order
            .into_iter()
            .enumerate()
            .map(|(rank, idx)| (unigrams[idx].0, rank + 1))
            .collect();

        Ok(NgramModel {
            data,
            index_stride,
            uni_count,
            uni_off,
            bi_blocks_off,
            bi_index_off,
            bi_index_count,
            tri_blocks_off,
            tri_index_off,
            tri_index_count,
            uni_pos,
            freq_rank,
        })
    }

    /// unigram 概率（线性域；未收录回退到首条）。
    pub fn unigram_prob(&self, cp: u32) -> f32 {
        let idx = match self.uni_pos.get(&cp) {
            Some(i) => *i,
            None => 0,
        };
        let d: &[u8] = &self.data;
        rd_f32(d, self.uni_off + idx * 8 + 4)
    }

    /// 字频名次（1 起；未收录返回 usize::MAX）。
    pub fn freq_rank(&self, cp: u32) -> usize {
        self.freq_rank.get(&cp).copied().unwrap_or(usize::MAX)
    }

    /// 分页块中查找上下文键 → (后继数组偏移, λ, 后继数)。
    fn find_ctx(
        &self,
        blocks_off: usize,
        index_off: usize,
        index_count: usize,
        key: i64,
    ) -> Option<(usize, f32, usize)> {
        // 页索引按 first_ctx_key 升序：二分找最后一个 first_key <= key 的页
        let mut lo = 0usize;
        let mut hi = index_count;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if rd_i64(&self.data, index_off + mid * 16) <= key {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            return None;
        }
        let page = lo - 1;
        let block = rd_i64(&self.data, index_off + page * 16 + 8) as usize;
        // 页内顺序扫描（≤ index_stride 条，键升序可提前退出）
        let mut off = block;
        for _ in 0..self.index_stride {
            if off + 16 > self.data.len() {
                return None;
            }
            let k = rd_i64(&self.data, off);
            if k == key {
                let lambda = rd_f32(&self.data, off + 8);
                let succ = rd_i32(&self.data, off + 12) as usize;
                return Some((off + 16, lambda, succ));
            }
            if k > key {
                return None;
            }
            let succ = rd_i32(&self.data, off + 12) as usize;
            off += 16 + succ * 8;
        }
        None
    }

    /// 上下文查询 → (p_stored, λ)。
    fn ctx_lookup(
        &self,
        blocks_off: usize,
        index_off: usize,
        index_count: usize,
        key: i64,
        cp: u32,
    ) -> Option<(f32, f32)> {
        let (succ_off, lambda, succ) = self.find_ctx(blocks_off, index_off, index_count, key)?;
        // 后继多时二分（码点有序与否未知，先线性，短表足够快）
        for i in 0..succ {
            let off = succ_off + i * 8;
            if rd_i32(&self.data, off) as u32 == cp {
                return Some((rd_f32(&self.data, off + 4), lambda));
            }
        }
        Some((0.0, lambda))
    }

    /// P(c|w)：bigram ctx 键 = w 本身。
    pub fn bigram_prob(&self, w: u32, c: u32) -> f32 {
        match self.ctx_lookup(
            self.bi_blocks_off,
            self.bi_index_off,
            self.bi_index_count,
            w as i64,
            c,
        ) {
            Some((p2, lambda)) => p2 + lambda * self.unigram_prob(c),
            None => self.unigram_prob(c),
        }
    }

    /// P(c|w1,w2) = p3 + λ3 × P(c|w2)。
    pub fn trigram_prob(&self, w1: u32, w2: u32, c: u32) -> f32 {
        let key = pack_key(w1, w2);
        match self.ctx_lookup(
            self.tri_blocks_off,
            self.tri_index_off,
            self.tri_index_count,
            key,
            c,
        ) {
            Some((p3, lambda)) => p3 + lambda * self.bigram_prob(w2, c),
            None => self.bigram_prob(w2, c),
        }
    }

    /// bigram (w → c) 是否观测到。
    pub fn has_bigram(&self, w: u32, c: u32) -> bool {
        match self.find_ctx(
            self.bi_blocks_off,
            self.bi_index_off,
            self.bi_index_count,
            w as i64,
        ) {
            Some((succ_off, _lambda, succ)) => {
                (0..succ).any(|i| rd_i32(&self.data, succ_off + i * 8) as u32 == c)
            }
            None => false,
        }
    }

    pub fn uni_count(&self) -> usize {
        self.uni_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造微型 TCSKNM02：
    /// unigram: unk(1e-7), EOS(0.002), 我(0.5), 们(0.3), 是(0.2)
    /// bigram ctx 我 → 们 p=0.6 λ=0.4
    /// trigram ctx (我,们) → 是 p=0.7 λ=0.3
    fn tiny_model() -> NgramModel {
        let mut uni = vec![
            (0u32, 1e-7f32),
            (EOS, 0.002),
            ('我' as u32, 0.5),
            ('们' as u32, 0.3),
            ('是' as u32, 0.2),
        ];
        uni.sort_by_key(|(cp, _)| *cp);
        let mut d: Vec<u8> = Vec::new();
        d.extend_from_slice(MAGIC);
        d.extend_from_slice(&1i32.to_le_bytes()); // version @8
        d.extend_from_slice(&104i32.to_le_bytes()); // header_size @12
        let patch = |d: &mut Vec<u8>, off: usize, bytes: &[u8]| {
            d[off..off + bytes.len()].copy_from_slice(bytes);
        };
        // 头部占位
        d.extend_from_slice(&[0u8; 104 - 16]);

        let mut body: Vec<u8> = Vec::new();
        let uni_off = 104usize;
        for (cp, p) in &uni {
            body.extend_from_slice(&(*cp as i32).to_le_bytes());
            body.extend_from_slice(&p.to_le_bytes());
        }
        let bi_block_off = uni_off + body.len();
        let wo = '我' as i64;
        // bigram ctx 键 = wo
        body.extend_from_slice(&wo.to_le_bytes());
        body.extend_from_slice(&0.4f32.to_le_bytes());
        body.extend_from_slice(&1i32.to_le_bytes());
        body.extend_from_slice(&('们' as i32).to_le_bytes());
        body.extend_from_slice(&0.6f32.to_le_bytes());
        let bi_index_off = bi_block_off + 16 + 8;
        body.extend_from_slice(&wo.to_le_bytes());
        body.extend_from_slice(&(bi_block_off as i64).to_le_bytes());
        let tri_block_off = bi_index_off + 16;
        let tri_key = pack_key('我' as u32, '们' as u32);
        body.extend_from_slice(&tri_key.to_le_bytes());
        body.extend_from_slice(&0.3f32.to_le_bytes());
        body.extend_from_slice(&1i32.to_le_bytes());
        body.extend_from_slice(&('是' as i32).to_le_bytes());
        body.extend_from_slice(&0.7f32.to_le_bytes());
        let tri_index_off = tri_block_off + 16 + 8;
        body.extend_from_slice(&tri_key.to_le_bytes());
        body.extend_from_slice(&(tri_block_off as i64).to_le_bytes());

        d.extend_from_slice(&body);
        // 回填头部（i64 用 8 字节）
        patch(&mut d, 16, &0i64.to_le_bytes()); // file_size 占位
        patch(&mut d, 24, &1i64.to_le_bytes()); // index_stride
        patch(&mut d, 32, &(uni.len() as i64).to_le_bytes());
        patch(&mut d, 40, &(uni_off as i64).to_le_bytes());
        patch(&mut d, 48, &1i32.to_le_bytes()); // bi_ctx_count
        patch(&mut d, 52, &1i32.to_le_bytes()); // bi_index_count
        patch(&mut d, 56, &(bi_block_off as i64).to_le_bytes());
        patch(&mut d, 64, &(bi_index_off as i64).to_le_bytes());
        patch(&mut d, 72, &1i64.to_le_bytes()); // tri_ctx_count
        patch(&mut d, 80, &1i32.to_le_bytes()); // tri_index_count
        patch(&mut d, 88, &(tri_block_off as i64).to_le_bytes());
        patch(&mut d, 96, &(tri_index_off as i64).to_le_bytes());
        NgramModel::from_bytes(d).unwrap()
    }

    #[test]
    fn tiny_model_queries() {
        let m = tiny_model();
        assert!((m.unigram_prob('我' as u32) - 0.5).abs() < 1e-6);
        assert!((m.unigram_prob(EOS) - 0.002).abs() < 1e-6);
        assert_eq!(m.unigram_prob(0xFFFF), 1e-7);

        let p = m.bigram_prob('我' as u32, '们' as u32);
        assert!((p - (0.6 + 0.4 * 0.3)).abs() < 1e-5, "P(们|我)={p}");
        let p = m.bigram_prob('我' as u32, '是' as u32);
        assert!((p - 0.4 * 0.2).abs() < 1e-5, "P(是|我)={p}");
        // 未知上文 → unigram
        let p = m.bigram_prob('们' as u32, '我' as u32);
        assert!((p - 0.5).abs() < 1e-6, "P(我|们)={p}");

        let p = m.trigram_prob('我' as u32, '们' as u32, '是' as u32);
        // P(是|们)=unigram(是)=0.2（们 无 bigram 上下文）→ 0.7 + 0.3×0.2
        assert!((p - 0.76).abs() < 1e-5, "P(是|我们)={p}");
        assert!(m.has_bigram('我' as u32, '们' as u32));
        assert!(!m.has_bigram('们' as u32, '我' as u32));
    }
}
