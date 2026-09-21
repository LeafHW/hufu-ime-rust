//! q8_0 解码 vs llama.cpp F32 转换件对照
//!
//! 【Linux 适配 2026-09-19】本测试依赖作者本机 Windows 对照件
//! （`E:\DSH-KF\...`），任何其他机器（含 Linux）都跑不了——默认跳过；
//! 需要时在作者机器上 `cargo test -- --ignored` 单独跑。
use hufu_rerank::gguf::GgufFile;

#[test]
#[ignore = "依赖作者本机 Windows 对照件（E:\\DSH-KF\\...），默认跳过"]
fn q8_decode_matches_llama_f32() {
    let q8 = GgufFile::open("E:\\DSH-KF\\TigerClaw\\sentence\\Models\\sentence-qwen-q8.gguf").unwrap();
    let f32m = GgufFile::open_lazy("E:\\DSH-KF\\tools\\llamacpp\\sentence-f32.gguf").unwrap();
    for name in ["blk.0.attn_q.weight", "blk.0.attn_k.weight", "blk.13.ffn_gate.weight", "token_embd.weight"] {
        let iq = &q8.tensors[name];
        let if3 = &f32m.tensors[name];
        let mine = q8.read_rows(iq, 0, 1).unwrap();
        let truth = f32m.read_rows(if3, 0, 1).unwrap();
        let mut maxdiff = 0f32;
        let mut maxi = 0usize;
        for i in 0..mine.len() {
            let d = (mine[i] - truth[i]).abs();
            if d > maxdiff {
                maxdiff = d;
                maxi = i;
            }
        }
        println!("{name}: k={} 我前4={:?} 真前4={:?} maxdiff={maxdiff} @ {maxi} 真值@max={}", mine.len(), &mine[..4], &truth[..4], truth[maxi]);
        // 逐块诊断第一个差异块
        for b in 0..4usize {
            let s = b * 32;
            let e = (s + 32).min(mine.len());
            println!("  块{b}: 我[{}..{}]={:?}", s, e, &mine[s..e.min(s + 6)]);
            println!("      真[{}..{}]={:?}", s, e, &truth[s..e.min(s + 6)]);
        }
    }
}
