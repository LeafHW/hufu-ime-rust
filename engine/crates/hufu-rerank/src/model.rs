//! Qwen3 前向（q8 权重流式反量化 + gemm 并行），只出打分位置 logits。

use crate::gguf::{GgmlDType, GgufFile};
use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// 前台活动时间戳（ms，粗粒度即可）：宿主每收到一次按键调用 note_foreground()。
/// gemm 分块循环发现 50ms 内有按键 → 主动小睡让核（覆盖 80ms/键的连打节奏），
/// 前台解码优先；代价是打分变慢（异步无感）。
static FOREGROUND_MS: AtomicU64 = AtomicU64::new(0);

pub fn note_foreground() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    FOREGROUND_MS.store(now, Ordering::Relaxed);
}

fn foreground_recent() -> bool {
    let last = FOREGROUND_MS.load(Ordering::Relaxed);
    if last == 0 {
        return false;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    now.saturating_sub(last) < 50
}

/// 重排专用线程池：≤6 线程 + Windows BelowNormal 优先级。
/// 不抢全局池：打分永远让位于前台（管道解码在 normal 优先级），CPU 封顶 ~37%。
fn rerank_pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let n = std::thread::available_parallelism()
            .map(|c| c.get())
            .unwrap_or(4)
            .min(6)
            .max(2);
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .thread_name(|i| format!("hufu-rerank-{i}"))
            .start_handler(|_i| {
                #[cfg(windows)]
                unsafe {
                    // THREAD_PRIORITY_BELOW_NORMAL = -1
                    windows_sys::Win32::System::Threading::SetThreadPriority(
                        windows_sys::Win32::System::Threading::GetCurrentThread(),
                        -1,
                    );
                }
            })
            .build()
            .expect("rerank pool")
    })
}

pub struct Cfg {
    pub hidden: usize,
    pub inter: usize,
    pub layers: usize,
    pub heads: usize,
    pub kv_heads: usize,
    pub head_dim: usize,
    pub rope_theta: f64,
    pub eps: f64,
}

pub struct Qwen3 {
    g: GgufFile,
    pub cfg: Cfg,
}

struct SendPtr(*mut f32);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}

fn rmsnorm(x: &[f32], eps: f64) -> Vec<f32> {
    let ms = x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len() as f64;
    let s = (1.0 / (ms + eps).sqrt()) as f32;
    x.iter().map(|&v| v * s).collect()
}

impl Qwen3 {
    pub fn load(path: &str) -> Result<Self, String> {
        let g = if std::env::var("GGUF_LAZY").is_ok() {
            GgufFile::open_lazy(path).map_err(|e| e.to_string())?
        } else {
            GgufFile::open(path).map_err(|e| e.to_string())?
        };
        let cfg = Cfg {
            hidden: g.md_u32("qwen3.embedding_length", 1024) as usize,
            inter: g.md_u32("qwen3.feed_forward_length", 3072) as usize,
            layers: g.md_u32("qwen3.block_count", 28) as usize,
            heads: g.md_u32("qwen3.attention.head_count", 16) as usize,
            kv_heads: g.md_u32("qwen3.attention.head_count_kv", 8) as usize,
            head_dim: g.md_u32("qwen3.attention.key_length", 128) as usize,
            rope_theta: g.md_f64("qwen3.rope.freq_base", 1_000_000.0),
            eps: g.md_f64("qwen3.attention.layer_norm_rms_epsilon", 1e-6),
        };
        match g.tensors.get("token_embd.weight") {
            Some(t) if t.dtype.is_q8_0() || t.dtype == GgmlDType::F32 => {}
            _ => return Err("模型缺 token_embd.weight 或类型不支持".into()),
        }
        Ok(Self { g, cfg })
    }

    /// 读权重行块并反量化（行主 pn×k）。GGUF ne 序：shape=[cols(k), rows(n)]
    fn dequant_rows(&self, info: &crate::gguf::TensorInfo, p0: usize, pn: usize) -> Vec<f32> {
        self.g
            .read_rows(info, p0, pn)
            .unwrap_or_else(|e| panic!("读 {} 行[{p0},{}) 失败: {e}", info.name, p0 + pn))
    }


    /// out(m,n) = x(m,k)·W(n,k)ᵀ，W 为 q8 权重（流式反量化 + gemm/naive）
    fn matmul_w(&self, wname: &str, x: &[f32], m: usize, out: &mut [f32]) {
        let info = &self.g.tensors[wname];
        let n = info.shape[1];
        let k = info.shape[0];
        assert_eq!(x.len(), m * k, "{wname}: x.len={} m*k={}", x.len(), m * k);
        assert_eq!(out.len(), m * n, "{wname}: out.len={} m*n={}", out.len(), m * n);

        if std::env::var("GGUF_NAIVE").is_ok() {
            // 朴素对照路径（正确性二分用）
            for p in 0..n {
                let wcol = self.dequant_rows(info, p, 1);
                for i in 0..m {
                    let mut s = 0f64;
                    for j in 0..k {
                        s += (x[i * k + j] as f64) * (wcol[j] as f64);
                    }
                    out[i * n + p] = s as f32;
                }
            }
            return;
        }

        let tile = 256usize;
        let out_ptr = SendPtr(out.as_mut_ptr());
        let out_ref = &out_ptr;
        // 专用池内并行（BelowNormal 优先级 + 打字让键），不与前台争核
        rerank_pool().install(|| {
            (0..n)
                .into_par_iter()
                .step_by(tile)
                .for_each(|p0| {
                    // 前台 50ms 内有按键 → 让出 CPU 小睡，避免连打时键延迟被放大
                    if foreground_recent() {
                        std::thread::sleep(std::time::Duration::from_millis(3));
                    }
                    let pn = tile.min(n - p0);
                    let w = self.dequant_rows(info, p0, pn);
                    // C(m,pn) = X(m,k)·Wᵀ(k,pn)；B(j,p)=w[p*k+j] → rhs rs=1 cs=k
                    unsafe {
                        gemm::gemm(
                            m, pn, k,
                            out_ref.0.add(p0), 1, n as isize, false,
                            x.as_ptr(), 1, k as isize,
                            w.as_ptr(), k as isize, 1,
                            0.0, 1.0, false, false, false,
                            gemm::Parallelism::None,
                        );
                    }
                })
        });
    }

    /// 一维小张量（norm 权重等）整体读
    fn small(&self, name: &str) -> Vec<f32> {
        let info = &self.g.tensors[name];
        let rows = if info.shape.len() == 1 { 1 } else { info.shape[1] };
        self.dequant_rows(info, 0, rows)
    }

    /// 逐行 RMSNorm×w：x 为 l×hidden 行主
    fn norm_rows(w: &[f32], x: &[f32], hidden: usize, eps: f64) -> Vec<f32> {
        let mut out = Vec::with_capacity(x.len());
        for r in x.chunks_exact(hidden) {
            let n = rmsnorm(r, eps);
            for (a, b) in n.into_iter().zip(w) {
                out.push(a * b);
            }
        }
        out
    }

    fn embedding_row(&self, id: u32, out: &mut Vec<f32>) {
        let info = &self.g.tensors["token_embd.weight"];
        let k = info.shape[0]; // ne[0]=hidden
        let v = self.dequant_rows(info, id as usize, 1);
        out.clear();
        out.extend_from_slice(&v[..k]);
    }

    fn rope(&self, q: &mut [f32], k: &mut [f32], l: usize, hd: usize) {
        let half = hd / 2;
        let inv: Vec<f32> = (0..half)
            .map(|i| 1.0 / self.cfg.rope_theta.powf((2.0 * i as f64) / hd as f64) as f32)
            .collect();
        for pos in 0..l {
            for h in 0..self.cfg.heads {
                let base = (pos * self.cfg.heads + h) * hd;
                for i in 0..half {
                    let (s, c) = (inv[i] * pos as f32).sin_cos();
                    let qi = q[base + i];
                    let qj = q[base + half + i];
                    q[base + i] = qi * c - qj * s;
                    q[base + half + i] = qi * s + qj * c;
                }
            }
            for h in 0..self.cfg.kv_heads {
                let base = (pos * self.cfg.kv_heads + h) * hd;
                for i in 0..half {
                    let (s, c) = (inv[i] * pos as f32).sin_cos();
                    let ki = k[base + i];
                    let kj = k[base + half + i];
                    k[base + i] = ki * c - kj * s;
                    k[base + half + i] = ki * s + kj * c;
                }
            }
        }
    }

    /// 全序列一次前向；need 为需打分的位置（预测下一 token），返回对应 logits 行
    pub fn forward_scored_logits(&self, ids: &[u32], need: &[usize]) -> Vec<Vec<f32>> {
        let c = &self.cfg;
        let l = ids.len();
        let hidden = c.hidden;
        let hd = c.head_dim;
        let mut x: Vec<f32> = Vec::with_capacity(l * hidden);
        let mut row = Vec::new();
        for &id in ids {
            self.embedding_row(id, &mut row);
            x.extend_from_slice(&row);
        }

        let scale = 1.0 / (hd as f32).sqrt();
        for li in 0..c.layers {
            let pfx = format!("blk.{li}.");
            let an = self.small(&(pfx.clone() + "attn_norm.weight"));
            let h = Self::norm_rows(&an, &x, hidden, c.eps);

            let mut q = vec![0f32; l * c.heads * hd];
            self.matmul_w(&(pfx.clone() + "attn_q.weight"), &h, l, &mut q);
            let mut k = vec![0f32; l * c.kv_heads * hd];
            self.matmul_w(&(pfx.clone() + "attn_k.weight"), &h, l, &mut k);
            let mut v = vec![0f32; l * c.kv_heads * hd];
            self.matmul_w(&(pfx.clone() + "attn_v.weight"), &h, l, &mut v);

            // Qwen3 q/k_norm（head_dim 维 RMSNorm×w）
            let qn = self.small(&(pfx.clone() + "attn_q_norm.weight"));
            let kn = self.small(&(pfx.clone() + "attn_k_norm.weight"));
            for ch in q.chunks_exact_mut(hd) {
                let n = rmsnorm(ch, c.eps);
                for ((dst, a), b) in ch.iter_mut().zip(n).zip(&qn) {
                    *dst = a * b;
                }
            }
            for ch in k.chunks_exact_mut(hd) {
                let n = rmsnorm(ch, c.eps);
                for ((dst, a), b) in ch.iter_mut().zip(n).zip(&kn) {
                    *dst = a * b;
                }
            }

            self.rope(&mut q, &mut k, l, hd);

            // causal scaled dot-product（GQA）
            let rep = c.heads / c.kv_heads;
            let mut attn_out = vec![0f32; l * c.heads * hd];
            for pos in 0..l {
                for h_i in 0..c.heads {
                    let kvh = h_i / rep;
                    let qh = &q[(pos * c.heads + h_i) * hd..][..hd];
                    let mut scores = vec![0f32; pos + 1];
                    for (si, s) in scores.iter_mut().enumerate() {
                        let kh = &k[(si * c.kv_heads + kvh) * hd..][..hd];
                        *s = qh.iter().zip(kh).map(|(a, b)| a * b).sum::<f32>() * scale;
                    }
                    let mx = scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                    let sum: f32 = scores.iter().map(|s| (s - mx).exp()).sum();
                    for (si, s) in scores.iter().enumerate() {
                        let w = ((s - mx).exp()) / sum;
                        let vh = &v[(si * c.kv_heads + kvh) * hd..][..hd];
                        let dst = &mut attn_out[(pos * c.heads + h_i) * hd..][..hd];
                        for (d, vv) in dst.iter_mut().zip(vh) {
                            *d += w * vv;
                        }
                    }
                }
            }
            let mut o = vec![0f32; l * hidden];
            self.matmul_w(&(pfx.clone() + "attn_output.weight"), &attn_out, l, &mut o);
            for (dst, s) in x.iter_mut().zip(&o) {
                *dst += s;
            }

            let fn2 = self.small(&(pfx.clone() + "ffn_norm.weight"));
            let h2 = Self::norm_rows(&fn2, &x, hidden, c.eps);
            let mut gate = vec![0f32; l * c.inter];
            self.matmul_w(&(pfx.clone() + "ffn_gate.weight"), &h2, l, &mut gate);
            let mut up = vec![0f32; l * c.inter];
            self.matmul_w(&(pfx.clone() + "ffn_up.weight"), &h2, l, &mut up);
            for (g, u) in gate.iter_mut().zip(&up) {
                let sig = 1.0 / (1.0 + (-*g).exp());
                *g = *g * sig * u;
            }
            let mut down = vec![0f32; l * hidden];
            self.matmul_w(&(pfx.clone() + "ffn_down.weight"), &gate, l, &mut down);
            for (dst, s) in x.iter_mut().zip(&down) {
                *dst += s;
            }
        }

        let fnorm = self.small("output_norm.weight");
        let mut normed = Vec::with_capacity(l * hidden);
        for r in x.chunks_exact(hidden) {
            let n = rmsnorm(r, c.eps);
            for (a, b) in n.into_iter().zip(&fnorm) {
                normed.push(a * b);
            }
        }

        // 只算 need 位置的 logits：h_sel(t,hidden) × embedᵀ
        let t = need.len();
        let mut h_sel = Vec::with_capacity(t * hidden);
        for &p in need {
            h_sel.extend_from_slice(&normed[p * hidden..(p + 1) * hidden]);
        }
        let vocab = self.g.tensors["token_embd.weight"].shape[1];
        let mut logits = vec![0f32; t * vocab];
        self.matmul_w("token_embd.weight", &h_sel, t, &mut logits);
        logits.chunks_exact(vocab).map(|r| r.to_vec()).collect()
    }
}
