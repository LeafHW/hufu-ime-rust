//! 句首重排验证探针：模拟真实打字通道（op=key 走 session）。
//! cargo run -p hufu-cli --release --example verifysent
//! 步骤：reset → 逐键 agkadklecbsy → 记录即时候选（ngram 序）
//! → 停 1.8s（重排 debounce+推理）→ 再发一键（触发 apply_rerank）
//! → 记录刷新后候选。期望：首列从「痔问而不言」变为「阖口而不言」。
#![cfg(windows)]

use std::io::{Read, Write};
use std::os::windows::io::{FromRawHandle, RawHandle};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16, access: u32, share: u32, sa: *const core::ffi::c_void,
        disp: u32, flags: u32, template: isize,
    ) -> isize;
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const INVALID: isize = -1;

fn call(req: &serde_json::Value) -> serde_json::Value {
    unsafe {
        let name: Vec<u16> = r"\\.\pipe\hufu-ime".encode_utf16().chain([0]).collect();
        let h = CreateFileW(name.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0,
            std::ptr::null(), OPEN_EXISTING, 0, 0);
        assert!(h != INVALID, "管道连接失败（hufu-server 未运行？）");
        let mut f = std::fs::File::from_raw_handle(h as RawHandle);
        let body = serde_json::to_vec(req).unwrap();
        let mut frame = (body.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&body);
        f.write_all(&frame).expect("管道写入失败");
        let mut head = [0u8; 4];
        f.read_exact(&mut head).expect("管道读取失败");
        let len = u32::from_le_bytes(head) as usize;
        let mut buf = vec![0u8; len];
        f.read_exact(&mut buf).expect("管道读取失败");
        serde_json::from_slice(&buf).expect("响应非 JSON")
    }
}

fn top3(state: &serde_json::Value) -> String {
    state["candidates"]
        .as_array()
        .map(|a| {
            a.iter().take(3)
                .map(|c| c["text"].as_str().unwrap_or("?").to_string())
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .unwrap_or_else(|| "(无候选)".into())
}

fn main() {
    call(&serde_json::json!({"op":"reset"}));
    for ch in "agkadklecbsy".chars() {
        let r = call(&serde_json::json!({"op":"key","key": ch.to_string()}));
        let raw = r["state"]["raw"].as_str().unwrap_or("");
        let want: String = "agkadklecbsy"[..raw.len()].to_string();
        assert_eq!(raw, want, "编码应累积");
    }
    let s1 = call(&serde_json::json!({"op":"state"}));
    println!("即时(ngram序) 前3: {}", top3(&s1["state"]));
    // 时序：键入中每键触发 maybe_send（debounce 350ms）；停顿后第一键
    // 把 job 发出，推理约 0.5~1s，之后下一键应用缓存。故：等 → 键 →
    // 等（推理）→ 再键（应用）→ 查。
    println!("停 1.0s → 发键（触发 job）→ 停 1.5s（推理）→ 再键（应用）…");
    std::thread::sleep(std::time::Duration::from_millis(1000));
    call(&serde_json::json!({"op":"key","key":"up"}));
    std::thread::sleep(std::time::Duration::from_millis(1500));
    call(&serde_json::json!({"op":"key","key":"up"}));
    let s2 = call(&serde_json::json!({"op":"state"}));
    println!("刷新(重排后) 前3: {}", top3(&s2["state"]));
    let first = s2["state"]["candidates"].as_array()
        .and_then(|a| a.first())
        .and_then(|c| c["text"].as_str().unwrap_or("?").parse::<String>().ok())
        .or_else(|| s2["state"]["candidates"].as_array()
            .and_then(|a| a.first())
            .map(|c| c["text"].as_str().unwrap_or("?").to_string()));
    match first {
        Some(t) if t.contains("阖口") => println!("✓ 句首重排生效：阖口而不言 已到首选"),
        Some(t) => println!("✗ 首选仍为「{t}」"),
        None => println!("✗ 无候选"),
    }
    call(&serde_json::json!({"op":"reset"}));
}
