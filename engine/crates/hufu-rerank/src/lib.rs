//! hufu-rerank：Qwen3 GGUF 整句候选重排（纯 Rust，q8 流式反量化 + gemm 并行）。

pub mod bpe;
pub mod gguf;
pub mod model;

/// 前台按键通知：gemm 分块循环据此让键（详见 model.rs）。
pub use model::note_foreground;

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

    /// 调试打分：返回 (sum(cand), mean(cand), logP(eot|ctx+cand), sum(cand)+logP(eot))
    pub fn score_debug(&self, ctx: &str, cand: &str) -> (f64, f64, f64, f64) {
        let eot = self.tok.eot;
        let ctx_ids = self.tok.encode(ctx);
        let cand_ids = self.tok.encode(cand);
        let mut ids = vec![eot];
        ids.extend(ctx_ids.iter().copied());
        let base = ids.len();
        ids.extend(cand_ids.iter().copied());
        let need: Vec<usize> = ((base - 1)..ids.len() - 1).collect();
        let logits = self.m.forward_scored_logits(&ids, &need);
        let mut total = 0.0f64;
        for (k, li) in logits.iter().enumerate() {
            let target = ids[base + k] as usize;
            let mx = li.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let s: f64 = li.iter().map(|v| (*v - mx) as f64).map(f64::exp).sum();
            total += (((li[target] - mx) as f64).exp() / s).ln();
        }
        let sum = total;
        let mean = total / cand_ids.len().max(1) as f64;
        // P(eot | ctx+cand)：末位补 eot，取其预测位
        let mut ids2 = ids.clone();
        ids2.push(eot);
        let need2 = [ids2.len() - 2];
        let logits2 = self.m.forward_scored_logits(&ids2, &need2);
        let li = &logits2[0];
        let target = eot as usize;
        let mx = li.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let s: f64 = li.iter().map(|v| (*v - mx) as f64).map(f64::exp).sum();
        let eot_lp = (((li[target] - mx) as f64).exp() / s).ln();
        (sum, mean, eot_lp, sum + eot_lp)
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
                // 序列总 logprob（= 对数似然）：不用 mean——
                // 生僻字走 BPE 字节回退会拆成 3~4 token，mean 会把「越生僻越长」洗成高分
                // （实测 EOT 后 两次=-14.7 反而输给 𰧓×4=-6.3/枚 的均值）。sum 下长候选
                // 自然受罚，与 llama.cpp / TigerClaw 的序列概率排序一致。
                total
            })
            .collect()
    }
}
