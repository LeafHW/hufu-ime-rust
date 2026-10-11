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
    foreground_within(50)
}

/// 【空闲判定辅助 2026-09-08】距最近一次按键是否在 within_ms 内——
/// 重排线程的模型卸载判定用（打字中永不卸，审计 E-1：词典域短码
/// 不产生重排任务，只看任务会把「正在打字」误判空闲）。
pub fn foreground_within(within_ms: u64) -> bool {
    let last = FOREGROUND_MS.load(Ordering::Relaxed);
    if last == 0 {
        return false;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    now.saturating_sub(last) < within_ms
}

/// 【重排防死机·物理核口径 2026-10-12】线程上限规范按「机器逻辑核心
/// 数的一半」——available_parallelism 受进程亲和性影响（亲和性已锁
/// 半核时它返回的就是半数值，再折半会过杀），GetSystemInfo 恒返回
/// 物理拓扑全部逻辑核。
#[cfg(windows)]
fn machine_logical_cores() -> usize {
    use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
    unsafe {
        // SYSTEM_INFO 含匿名联合无 Default——零初始化（dwNumberOfProcessors
        // 必被 GetSystemInfo 全量覆写）。
        let mut si: SYSTEM_INFO = std::mem::zeroed();
        GetSystemInfo(&mut si);
        si.dwNumberOfProcessors as usize
    }
}
#[cfg(not(windows))]
fn machine_logical_cores() -> usize {
    std::thread::available_parallelism()
        .map(|c| c.get())
        .unwrap_or(4)
}

/// 【重排防死机·线程铁律 2026-10-12】见 E:\DSH-KF\虎符重排打包强制规范.md：
/// 推理线程数 ≤ 机器逻辑核一半（16 线程机 ≤8；此处再叠 6 线程内存带宽
/// 封顶——实测 14 线程抢内存带宽把下一键解码从 ~30ms 拖到 150-170ms，
/// 6 线程停顿期 ~500ms 内仍可完成）。线程统一 BelowNormal + EcoQoS
///（执行速度节流：让推理线程保持深省电档，不拉 SoC 电源态——防死机
/// 的电源管理侧半边；亲和性/进程优先级在 hufu-server main 写死）。
#[cfg(windows)]
unsafe fn thread_anti_freeze_setup() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadInformation, SetThreadPriority,
        ThreadPowerThrottling, THREAD_POWER_THROTTLING_CURRENT_VERSION,
        THREAD_POWER_THROTTLING_EXECUTION_SPEED, THREAD_POWER_THROTTLING_STATE,
    };
    // THREAD_PRIORITY_BELOW_NORMAL = -1
    SetThreadPriority(GetCurrentThread(), -1);
    // EcoQoS：Version=1, Control/State=EXECUTION_SPEED（进节流档）
    let state = THREAD_POWER_THROTTLING_STATE {
        Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
    };
    SetThreadInformation(
        GetCurrentThread(),
        ThreadPowerThrottling,
        &state as *const _ as *const core::ffi::c_void,
        std::mem::size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
    );
}

/// 重排专用线程池：线程数铁律见上（≤物理半核 ∩ ≤6，下限 2）+ Windows
/// BelowNormal/EcoQoS。不抢全局池：打分永远让位于前台（管道解码在
/// normal 优先级）。
fn rerank_pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        // 半核铁律：机器逻辑核一半（下限 2）∩ 6（内存带宽封顶）。
        // 16 线程机 = min(8, 6) = 6（与旧值一致，带宽实测口径）；
        // 8 线程机 = 4；32 线程机 = 6。
        let half = (machine_logical_cores() / 2).max(2);
        let n = half.min(6);
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .thread_name(|i| format!("hufu-rerank-{i}"))
            .start_handler(|_i| {
                #[cfg(windows)]
                unsafe {
                    thread_anti_freeze_setup();
                }
            })
            .build()
            .expect("rerank pool")
    })
}

/// 【空闲首击热身·迷你池 2026-10-12】>120s 空闲后首击先用它跑 2 线程
/// 迷你前向（forward_warmup）——逐键全核爆发引发「深省电→爆频」电源
/// 管理震荡（AMD APU 平台状态机转换处整机挂死取证），轻推电源态再上
/// 正式请求。线程同款 BelowNormal+EcoQoS。常驻懒建（热身秒级间隔可
/// 复用；空转 2 线程零成本）。
fn warmup_pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .thread_name(|i| format!("hufu-warmup-{i}"))
            .start_handler(|_i| {
                #[cfg(windows)]
                unsafe {
                    thread_anti_freeze_setup();
                }
            })
            .build()
            .expect("warmup pool")
    })
}

/// 热身期标志：matmul_w 的池指向开关（见其注释）。forward_warmup 进出
/// 置/复位；打分与热身在 host 重排线程串行，无竞态。
static WARMUP_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
        let prof2 = std::env::var("GGUF_PROF").is_ok();
        let t_deq = std::time::Instant::now();
        let deq_us = std::sync::atomic::AtomicU64::new(0);
        // 专用池内并行（BelowNormal/EcoQoS + 打字让键），不与前台争核。
        // 【池指向 2026-10-12】热身期（WARMUP_ACTIVE，见 forward_warmup）
        // 指到 2 线程迷你池——mini 前向保持 2 线程；正常路径恒走全局
        // 专用池。打分与热身在 host 重排线程上串行（同线程先热身后打
        // 分），标志位无竞态面。
        let pool = if WARMUP_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
            warmup_pool()
        } else {
            rerank_pool()
        };
        pool.install(|| {
            (0..n)
                .into_par_iter()
                .step_by(tile)
                .for_each(|p0| {
                    // 前台 50ms 内有按键 → 让出 CPU 小睡，避免连打时键延迟被放大
                    if foreground_recent() {
                        std::thread::sleep(std::time::Duration::from_millis(3));
                    }
                    let pn = tile.min(n - p0);
                    let tdq = std::time::Instant::now();
                    let w = self.dequant_rows(info, p0, pn);
                    if prof2 { deq_us.fetch_add(tdq.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed); }
                    // 瘦 M（≤16，整句打分 l≤16）：gemm crate 每调用 pack 整块
                    // rhs（~1MB×196 次/前向）占绝对大头（实测 0.6 GFLOPS）。
                    // 手写 AVX2 点积：out(i,p)=dot(x_i, w_p)，K 连续 stride=1，
                    // 无 pack，直接 10~30×。
                    if m <= 16 && k % 8 == 0 && dot_avx2_ok() {
                        unsafe {
                            for p in 0..pn {
                                let wrow = &w[p * k..(p + 1) * k];
                                for i in 0..m {
                                    let xr = &x[i * k..(i + 1) * k];
                                    *out_ref.0.add(i * n + p0 + p) = dot_avx2(xr, wrow);
                                }
                            }
                        }
                        return;
                    }
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
        if prof2 {
            let d = deq_us.load(std::sync::atomic::Ordering::Relaxed) as u128 / 1000;
            eprintln!("PROF mm_call dequant={}ms tile_calc={}ms", d, t_deq.elapsed().as_millis() as u128 - d);
        }
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

    /// 全序列一次前向到末层归一（embedding→层栈→output_norm）。打分与
    /// 热身共用主体（forward_scored_logits 再接 logits；forward_warmup
    /// 到此为止——词表矩阵是最大单块，热身无需）。
    fn forward_hidden(&self, ids: &[u32]) -> Vec<f32> {
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
        let prof = std::env::var("GGUF_PROF").is_ok();
        let (mut t_mm, mut t_attn, mut t_small) = (0u128, 0u128, 0u128);
        let mut mm_t = std::time::Instant::now();
        for li in 0..c.layers {
            let pfx = format!("blk.{li}.");
            if prof { t_small += mm_t.elapsed().as_micros(); mm_t = std::time::Instant::now(); }
            let an = self.small(&(pfx.clone() + "attn_norm.weight"));
            if prof { t_mm += mm_t.elapsed().as_micros(); mm_t = std::time::Instant::now(); }
            let h = Self::norm_rows(&an, &x, hidden, c.eps);

            let mut q = vec![0f32; l * c.heads * hd];
            self.matmul_w(&(pfx.clone() + "attn_q.weight"), &h, l, &mut q);
            let mut k = vec![0f32; l * c.kv_heads * hd];
            self.matmul_w(&(pfx.clone() + "attn_k.weight"), &h, l, &mut k);
            let mut v = vec![0f32; l * c.kv_heads * hd];
            self.matmul_w(&(pfx.clone() + "attn_v.weight"), &h, l, &mut v);
            if prof { t_mm += mm_t.elapsed().as_micros(); mm_t = std::time::Instant::now(); }

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
            if prof { t_mm += mm_t.elapsed().as_micros(); mm_t = std::time::Instant::now(); }
            for (dst, s) in x.iter_mut().zip(&o) {
                *dst += s;
            }

            let fn2 = self.small(&(pfx.clone() + "ffn_norm.weight"));
            let h2 = Self::norm_rows(&fn2, &x, hidden, c.eps);
            let mut gate = vec![0f32; l * c.inter];
            self.matmul_w(&(pfx.clone() + "ffn_gate.weight"), &h2, l, &mut gate);
            let mut up = vec![0f32; l * c.inter];
            self.matmul_w(&(pfx.clone() + "ffn_up.weight"), &h2, l, &mut up);
            if prof { t_mm += mm_t.elapsed().as_micros(); mm_t = std::time::Instant::now(); }
            for (g, u) in gate.iter_mut().zip(&up) {
                let sig = 1.0 / (1.0 + (-*g).exp());
                *g = *g * sig * u;
            }
            let mut down = vec![0f32; l * hidden];
            self.matmul_w(&(pfx.clone() + "ffn_down.weight"), &gate, l, &mut down);
            for (dst, s) in x.iter_mut().zip(&down) {
                *dst += s;
            }
            if prof { t_attn += mm_t.elapsed().as_micros(); mm_t = std::time::Instant::now(); }
        }
        if prof {
            eprintln!("PROF mm={}ms attn+small={}ms small={}ms", t_mm / 1000, t_attn / 1000, t_small / 1000);
        }

        let fnorm = self.small("output_norm.weight");
        let mut normed = Vec::with_capacity(l * hidden);
        for r in x.chunks_exact(hidden) {
            let n = rmsnorm(r, c.eps);
            for (a, b) in n.into_iter().zip(&fnorm) {
                normed.push(a * b);
            }
        }
        normed
    }

    /// 【空闲首击热身 2026-10-12】2 token 迷你前向（warmup_pool 2 线程，
    /// 跳过词表 logits）：把全部层权重读一遍/AVX 通路/电源态轻推到位，
    /// 正式请求随后不再从深省电直爆全核。>120s 空闲后首击前由 host.rs
    /// 调（Reranker::warmup）。
    pub fn forward_warmup(&self) {
        // token id 1 恒存在（<vocab 最小规模）；2 token 覆盖因果掩码/rope
        // 的非平凡路径（1 token 退化情形不值得信任）。
        WARMUP_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
        warmup_pool().install(|| {
            let _ = self.forward_hidden(&[1u32, 1]);
        });
        WARMUP_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// 全序列一次前向；need 为需打分的位置（预测下一 token），返回对应 logits 行
    pub fn forward_scored_logits(&self, ids: &[u32], need: &[usize]) -> Vec<Vec<f32>> {
        let c = &self.cfg;
        let hidden = c.hidden;
        let normed = self.forward_hidden(ids);

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

// ── 瘦 M 点积 kernel（AVX2+FMA）──────────────────────────────────────
// 整句打分 l≤16：每输出元素就是一次 K 长点积，无需通用 gemm 的
// 打包/微核选择——直接向量化点积即贴近峰值。

#[cfg(target_arch = "x86_64")]
pub fn dot_avx2_ok() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma")
    })
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn dot_avx2(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;
    let mut acc = _mm256_setzero_ps();
    let mut i = 0;
    while i + 32 <= a.len() {
        let a0 = _mm256_loadu_ps(a.as_ptr().add(i));
        let b0 = _mm256_loadu_ps(b.as_ptr().add(i));
        acc = _mm256_fmadd_ps(a0, b0, acc);
        let a1 = _mm256_loadu_ps(a.as_ptr().add(i + 8));
        let b1 = _mm256_loadu_ps(b.as_ptr().add(i + 8));
        acc = _mm256_fmadd_ps(a1, b1, acc);
        let a2 = _mm256_loadu_ps(a.as_ptr().add(i + 16));
        let b2 = _mm256_loadu_ps(b.as_ptr().add(i + 16));
        acc = _mm256_fmadd_ps(a2, b2, acc);
        let a3 = _mm256_loadu_ps(a.as_ptr().add(i + 24));
        let b3 = _mm256_loadu_ps(b.as_ptr().add(i + 24));
        acc = _mm256_fmadd_ps(a3, b3, acc);
        i += 32;
    }
    while i + 8 <= a.len() {
        acc = _mm256_fmadd_ps(
            _mm256_loadu_ps(a.as_ptr().add(i)),
            _mm256_loadu_ps(b.as_ptr().add(i)),
            acc,
        );
        i += 8;
    }
    let r = _mm_add_ps(_mm256_castps256_ps128(acc), _mm256_extractf128_ps(acc, 1));
    let r = _mm_add_ps(r, _mm_movehl_ps(r, r));
    let r = _mm_add_ss(r, _mm_shuffle_ps(r, r, 1));
    _mm_cvtss_f32(r)
}
