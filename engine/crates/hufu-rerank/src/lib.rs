//! hufu-rerank：Qwen3 GGUF 整句候选重排（纯 Rust，q8 流式反量化 + gemm 并行）。

pub mod bpe;
pub mod gguf;
pub mod model;

use model::Qwen3;

pub struct Reranker {
    m: Qwen3,
    tok: bpe::Bpe,
}

impl Reranker {
    pub fn load(path: &str) -> Result<Self, String> {
        let m = Qwen3::load(path)?;
        // BPE 从元数据构建（重开文件只读 meta，页缓存共享）
        let g = gguf::GgufFile::open(path).map_err(|e| e.to_string())?;
        let tok = bpe::Bpe::from_gguf(&g).ok_or("GGUF 缺 tokenizer.ggml 元数据")?;
        Ok(Self { m, tok })
    }

    /// 候选平均 logprob：ctx 为前文；每候选一次前向
    pub fn score(&self, ctx: &str, cands: &[String]) -> Vec<f64> {
        let eot = self.tok.eot;
        let ctx_ids = self.tok.encode(ctx);
        let mut ids = vec![eot];
        ids.extend(ctx_ids.iter().copied());
        let base = ids.len();
        cands
            .iter()
            .map(|c| {
                let cand_ids = self.tok.encode(c);
                if cand_ids.is_empty() {
                    return f64::NEG_INFINITY;
                }
                let mut all = ids.clone();
                all.extend(cand_ids.iter().copied());
                // 位置 i 的 token 由 i-1 预测：打分位置 = base-1 .. all.len()-2
                let need: Vec<usize> = ((base - 1)..all.len() - 1).collect();
                if std::env::var("GGUF_DEBUG").is_ok() {
                    eprintln!("score[{c}] ctx_ids={ctx_ids:?} l={} need={need:?}", all.len());
                }
                let logits = self.m.forward_scored_logits(&all, &need);
                if std::env::var("GGUF_DEBUG").is_ok() {
                    eprintln!(
                        "dbg ids={all:?} need={need:?} logits0前5={:?} vocab={}",
                        &logits[0][..5.min(logits[0].len())],
                        logits[0].len()
                    );
                }
                if std::env::var("GGUF_DEBUG").is_ok() && !logits.is_empty() {
                    let li = &logits[logits.len() - 1];
                    let mut idx: Vec<usize> = (0..li.len()).collect();
                    idx.sort_by(|a, b| li[*b].partial_cmp(&li[*a]).unwrap());
                    let n5 = 5.min(idx.len());
                    let pairs: Vec<String> = idx[..n5].iter().map(|&i| format!("{}:{:.2}", self.tok.id_to_str(i), li[i])).collect();
                    eprintln!("top5 = [{}]", pairs.join(" "));
                }
                let mut total = 0.0f64;
                for (k, li) in logits.iter().enumerate() {
                    let target = all[base + k] as usize;
                    let mx = li.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                    let sum: f64 = li.iter().map(|v| (*v - mx) as f64).map(f64::exp).sum();
                    let p = ((li[target] - mx) as f64).exp() / sum;
                    total += p.ln();
                }
                total / cand_ids.len() as f64
            })
            .collect()
    }
}
