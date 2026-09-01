//! 模型统计工具：succ 长度分布 / 码点有序性 / 语料命中压力模拟。
//! 用法: stat <model.bin> [corpus.txt]

use std::collections::HashMap;

fn rd_i32(d: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
}
fn rd_i64(d: &[u8], off: usize) -> i64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[off..off + 8]);
    i64::from_le_bytes(b)
}
fn pack_key(first: u32, second: u32) -> i64 {
    (first as i64) * (1 << 21) + (second as i64)
}

struct Stats {
    n: usize,
    sorted_runs: usize, // 有序succ条数
    unsorted_runs: usize,
    lens: Vec<usize>,
}

fn scan_blocks(
    d: &[u8],
    blocks_off: usize,
    index_off: usize,
    index_count: usize,
    stride: usize,
    label: &str,
) -> Stats {
    let mut st = Stats { n: 0, sorted_runs: 0, unsorted_runs: 0, lens: Vec::new() };
    for page in 0..index_count {
        let idx = index_off + page * 16;
        if idx + 16 > d.len() {
            break;
        }
        let block = rd_i64(d, idx + 8) as usize;
        if block < blocks_off || block >= d.len() {
            continue;
        }
        let mut off = block;
        for _ in 0..stride {
            if off + 16 > d.len() {
                break;
            }
            let succ = rd_i32(d, off + 12) as usize;
            if succ == 0 {
                break; // 越界哨兵/脏数据
            }
            let succ_off = off + 16;
            if succ_off + succ * 8 > d.len() {
                break;
            }
            // 有序性
            let mut sorted = true;
            let mut prev: i64 = -1;
            for i in 0..succ {
                let cp = rd_i32(d, succ_off + i * 8) as i64;
                if cp < prev {
                    sorted = false;
                    break;
                }
                prev = cp;
            }
            if sorted {
                st.sorted_runs += 1;
            } else {
                st.unsorted_runs += 1;
            }
            st.lens.push(succ);
            st.n += 1;
            off += 16 + succ * 8;
        }
    }
    st.lens.sort_unstable();
    let q = |f: f64| -> usize {
        if st.lens.is_empty() {
            0
        } else {
            st.lens[((st.lens.len() as f64) * f) as usize % st.lens.len()]
        }
    };
    let avg = if st.lens.is_empty() {
        0.0
    } else {
        st.lens.iter().sum::<usize>() as f64 / st.lens.len() as f64
    };
    println!(
        "{label}: ctx记录={}  succ长度 avg={avg:.1} p50={} p90={} p99={} max={}  有序={}/{} ({:.1}%)",
        st.n,
        q(0.5),
        q(0.9),
        q(0.99),
        st.lens.last().copied().unwrap_or(0),
        st.sorted_runs,
        st.sorted_runs + st.unsorted_runs,
        100.0 * st.sorted_runs as f64 / (st.sorted_runs + st.unsorted_runs).max(1) as f64
    );
    st
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let data = std::fs::read(path).expect("read model");
    let stride = rd_i64(&data, 24) as usize;
    let bi_blocks = rd_i64(&data, 56) as usize;
    let bi_index = rd_i64(&data, 64) as usize;
    let bi_n = rd_i32(&data, 52) as usize;
    let tri_blocks = rd_i64(&data, 88) as usize;
    let tri_index = rd_i64(&data, 96) as usize;
    let tri_n = rd_i32(&data, 80) as usize;
    println!("stride={stride} bi_pages={bi_n} tri_pages={tri_n}");

    let _bi = scan_blocks(&data, bi_blocks, bi_index, bi_n, stride, "bigram ");
    let tri = scan_blocks(&data, tri_blocks, tri_index, tri_n, stride, "trigram");

    // 语料压力模拟：真实句子字对的 succ 长度
    if let Some(corp) = args.get(2) {
        let text = std::fs::read_to_string(corp).expect("read corpus");
        let mut hit: Vec<usize> = Vec::new();
        let mut miss = 0usize;
        let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
        let mut cache: HashMap<(u32, u32), Option<usize>> = HashMap::new();
        for w in chars.windows(2) {
            let (w1, w2) = (w[0] as u32, w[1] as u32);
            if let Some(r) = cache.get(&(w1, w2)) {
                match r {
                    Some(l) => hit.push(*l),
                    None => miss += 1,
                }
                continue;
            }
            let key = pack_key(w1, w2);
            // 二分页 + 页内线性（同 find_ctx）
            let mut lo = 0usize;
            let mut hi = tri_n;
            while lo < hi {
                let mid = (lo + hi) / 2;
                if rd_i64(&data, tri_index + mid * 16) <= key {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            let mut found = None;
            if lo > 0 {
                let block = rd_i64(&data, tri_index + (lo - 1) * 16 + 8) as usize;
                let mut off = block;
                'outer: for _ in 0..stride {
                    if off + 16 > data.len() {
                        break;
                    }
                    let k = rd_i64(&data, off);
                    if k == key {
                        found = Some(rd_i32(&data, off + 12) as usize);
                        break 'outer;
                    }
                    if k > key {
                        break 'outer;
                    }
                    let succ = rd_i32(&data, off + 12) as usize;
                    off += 16 + succ * 8;
                }
            }
            cache.insert((w1, w2), found);
            match found {
                Some(l) => hit.push(l),
                None => miss += 1,
            }
        }
        hit.sort_unstable();
        let q = |f: f64| hit[((hit.len() as f64) * f) as usize % hit.len().max(1)];
        let avg = hit.iter().sum::<usize>() as f64 / hit.len().max(1) as f64;
        println!(
            "语料字对: 查询={}  ctx命中={} (miss={miss})  命中succ长度 avg={avg:.1} p50={} p90={} p99={} max={}",
            hit.len() + miss,
            hit.len(),
            q(0.5),
            q(0.9),
            q(0.99),
            hit.last().copied().unwrap_or(0)
        );
        // 大表扫描成本：succ>32 的命中占比（线性扫描的痛感来源）
        let big = hit.iter().filter(|&&l| l > 32).count();
        println!("  命中里 succ>32 的: {big}/{} ({:.1}%)", hit.len(), 100.0 * big as f64 / hit.len().max(1) as f64);
        let _ = &tri;
    }
}
