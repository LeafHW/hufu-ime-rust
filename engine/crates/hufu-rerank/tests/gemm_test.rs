//! gemm 封装语义单测：C(m×n)=A(m×k)·Wᵀ，W 存 (n×k) 行主
use rayon::prelude::*;

struct SendPtr(*mut f32);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}

fn gemm_tile(m: usize, n: usize, k: usize, a: &[f32], w: &[f32]) -> Vec<f32> {
    let mut out = vec![0f32; m * n];
    let out_ptr = SendPtr(out.as_mut_ptr());
    let out_ref = &out_ptr;
    (0..n)
        .into_par_iter()
        .step_by(256)
        .for_each(|p0| {
            let pn = 256.min(n - p0);
            let wtile = &w[p0 * k..(p0 + pn) * k];
            unsafe {
                gemm::gemm(
                    m, pn, k,
                    out_ref.0.add(p0), 1, n as isize, false,
                    a.as_ptr(), 1, k as isize,
                    wtile.as_ptr(), k as isize, 1,
                    0.0, 1.0, false, false, false,
                    gemm::Parallelism::None,
                );
            }
        });
    out
}

fn naive(m: usize, n: usize, k: usize, a: &[f32], w: &[f32]) -> Vec<f32> {
    let mut out = vec![0f32; m * n];
    for i in 0..m {
        for p in 0..n {
            let mut s = 0.0;
            for j in 0..k {
                s += a[i * k + j] * w[p * k + j];
            }
            out[i * n + p] = s;
        }
    }
    out
}

#[test]
fn gemm_matches_naive() {
    let (m, n, k) = (2usize, 2048usize, 1024usize);
    let a: Vec<f32> = (0..m * k).map(|i| ((i as f32) * 0.37).sin() * 0.1).collect();
    let w: Vec<f32> = (0..n * k).map(|i| ((i as f32) * 0.11).cos() * 0.1).collect();
    let got = gemm_tile(m, n, k, &a, &w);
    let want = naive(m, n, k, &a, &w);
    let mut bad = 0;
    for (i, (g, wv)) in got.iter().zip(&want).enumerate() {
        if (g - wv).abs() > 1e-3 * (1.0 + wv.abs()) {
            if bad < 5 {
                eprintln!("位置{i}: got {g} want {wv}");
            }
            bad += 1;
        }
    }
    assert_eq!(bad, 0, "不匹配 {bad} 处");
}
