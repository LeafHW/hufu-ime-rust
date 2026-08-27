//! 从 GGUF tokenizer.ggml 元数据构建 GPT2 风格字节级 BPE（Qwen 系）。

use crate::gguf::GgufFile;
use std::collections::HashMap;

pub struct Bpe {
    vocab: HashMap<String, u32>,
    ids: Vec<String>, // id → 字节级片段（调试/解码用）
    merges: HashMap<(String, String), usize>, // (a,b) -> rank
    pub eot: u32,
}

/// GPT2 byte<->unicode 表
fn byte_chars() -> Vec<char> {
    let mut bs: Vec<u8> = Vec::new();
    for b in (b'!'..=b'~').chain(0xA1..=0xAC).chain(0xAE..=0xFF) {
        bs.push(b);
    }
    let mut cs: Vec<u32> = bs.iter().map(|&b| b as u32).collect();
    let mut n = 0u32;
    for b in 0..=255u8 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(256 + n);
            n += 1;
        }
    }
    let mut out = vec!['?'; 256];
    for (i, &b) in bs.iter().enumerate() {
        out[b as usize] = char::from_u32(cs[i]).unwrap();
    }
    out
}

fn is_cjk(c: char) -> bool {
    let u = c as u32;
    (0x3400..=0x4DBF).contains(&u)
        || (0x4E00..=0x9FFF).contains(&u)
        || (0x20000..=0x2A6DF).contains(&u)
        || (0x2A700..=0x3134F).contains(&u)
        || (0x30000..=0x323AF).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
}

/// 简化 Qwen 预分词：字母串（含 CJK 连串）/ 数字(≤3)/ 其他符号串 / 空白
fn pretokenize(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_alphabetic() {
            let mut buf = String::new();
            while i < chars.len() && chars[i].is_alphabetic() {
                buf.push(chars[i]);
                i += 1;
            }
            out.push(buf);
        } else if c.is_numeric() {
            let mut n = 0;
            let mut buf = String::new();
            while i < chars.len() && chars[i].is_numeric() && n < 3 {
                buf.push(chars[i]);
                i += 1;
                n += 1;
            }
            out.push(buf);
        } else if c.is_whitespace() {
            let mut buf = String::new();
            while i < chars.len() && chars[i].is_whitespace() {
                buf.push(chars[i]);
                i += 1;
            }
            out.push(buf);
        } else {
            let mut buf = String::new();
            while i < chars.len() && !chars[i].is_alphanumeric() && !chars[i].is_whitespace() {
                buf.push(chars[i]);
                i += 1;
            }
            out.push(buf);
        }
    }
    out
}

impl Bpe {
    pub fn from_gguf(f: &GgufFile) -> Option<Self> {
        let tokens = f.meta.get("tokenizer.ggml.tokens")?.as_arr()?;
        let merges = f.meta.get("tokenizer.ggml.merges")?.as_arr()?;
        let mut vocab = HashMap::with_capacity(tokens.len());
        let mut ids = Vec::with_capacity(tokens.len());
        let mut eot = 151643u32;
        for (i, t) in tokens.iter().enumerate() {
            let s = t.as_str()?.to_string();
            if s == "<|endoftext|>" {
                eot = i as u32;
            }
            ids.push(s.clone());
            vocab.insert(s, i as u32);
        }
        let mut mr = HashMap::with_capacity(merges.len());
        for (i, m) in merges.iter().enumerate() {
            let s = m.as_str()?;
            let (a, b) = s.split_once(' ')?;
            mr.insert((a.to_string(), b.to_string()), i);
        }
        Some(Self { vocab, ids, merges: mr, eot })
    }

    /// 字节级 BPE：初始符号 = 每个 UTF-8 字节经 GPT2 映射（vocab 以此形态存储）
    fn merge_word(&self, word: &[u8]) -> Vec<u32> {
        let bc = byte_chars();
        let mut syms: Vec<String> = word.iter().map(|&b| bc[b as usize].to_string()).collect();
        loop {
            let mut best: Option<(usize, usize)> = None; // (rank, idx)
            for i in 0..syms.len().saturating_sub(1) {
                if let Some(&r) = self.merges.get(&(syms[i].clone(), syms[i + 1].clone())) {
                    if best.map_or(true, |(br, _)| r < br) {
                        best = Some((r, i));
                    }
                }
            }
            let Some((_, i)) = best else { break };
            let merged = format!("{}{}", syms[i], syms[i + 1]);
            syms.splice(i..i + 2, [merged]);
        }
        syms.iter().filter_map(|s| self.vocab.get(s).copied()).collect()
    }

    /// token id → 原文（字节级还原，调试用）
    pub fn id_to_str(&self, id: usize) -> String {
        let Some(s) = self.ids.get(id) else { return format!("<{id}>") };
        let bc = rev_byte_chars();
        let bytes: Vec<u8> = s.chars().filter_map(|c| bc.get(&c).copied()).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut ids = Vec::new();
        for w in pretokenize(text) {
            ids.extend(self.merge_word(w.as_bytes()));
        }
        ids
    }
}

/// 占位避免未用告警（is_cjk 供调试/未来细化用）
#[allow(dead_code)]
fn _use(_: bool) {
    let _ = is_cjk('中');
}

pub fn rev_byte_chars() -> std::collections::HashMap<char, u8> {
    let bc = byte_chars();
    let mut m = std::collections::HashMap::with_capacity(256);
    for (b, s) in bc.iter().enumerate() {
        m.insert(*s, b as u8);
    }
    m
}