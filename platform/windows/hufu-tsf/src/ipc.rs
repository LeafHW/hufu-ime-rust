//! 管道 IPC 客户端：帧协议 4 字节小端长度 + JSON。

use serde_json::Value;
use std::io::{Read, Write};
use std::os::windows::io::{FromRawHandle, RawHandle};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *const core::ffi::c_void,
        disp: u32,
        flags: u32,
        template: isize,
    ) -> isize;
    fn WaitNamedPipeW(name: *const u16, timeout: u32) -> i32;
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const INVALID: isize = -1;
const PIPE: &str = r"\\.\pipe\hufu-ime";

/// 单次请求（每次新建连接；本地管道往返 <100µs）。
pub fn call(req: &Value) -> Option<Value> {
    unsafe {
        let name: Vec<u16> = PIPE.encode_utf16().chain([0]).collect();
        let mut h;
        let mut tries = 0;
        loop {
            h = CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                0,
            );
            if h != INVALID {
                break;
            }
            // 管道忙碌：等待后重试
            if WaitNamedPipeW(name.as_ptr(), 500) == 0 || tries >= 3 {
                return None;
            }
            tries += 1;
        }
        let mut f = std::fs::File::from_raw_handle(h as RawHandle);
        let body = serde_json::to_vec(req).ok()?;
        let mut frame = (body.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&body);
        if f.write_all(&frame).is_err() {
            return None;
        }
        let mut head = [0u8; 4];
        f.read_exact(&mut head).ok()?;
        let len = u32::from_le_bytes(head) as usize;
        if len == 0 || len > (1 << 20) {
            return None;
        }
        let mut buf = vec![0u8; len];
        f.read_exact(&mut buf).ok()?;
        serde_json::from_slice(&buf).ok()
    }
}

/// 按键请求 → (consumed, commit, state, sound)
pub fn key_request(
    key: &str,
    shift: bool,
    ctrl: bool,
    alt: bool,
) -> Option<(bool, String, Value, Option<String>)> {
    let resp = call(&serde_json::json!({
        "op": "key",
        "key": key,
        "modifiers": { "shift": shift, "ctrl": ctrl, "alt": alt }
    }))?;
    let outcome = resp.get("outcome")?;
    let consumed = outcome.get("consumed").and_then(|v| v.as_bool()).unwrap_or(false);
    let commit = outcome
        .get("commit")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let state = resp.get("state").cloned().unwrap_or(Value::Null);
    let sound = outcome
        .get("sound")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Some((consumed, commit, state, sound))
}

/// 唤醒：探测服务器是否在。
pub fn ping() -> bool {
    call(&serde_json::json!({"op": "ping"}))
        .and_then(|v| v.get("ok").and_then(|o| o.as_bool()))
        .unwrap_or(false)
}

/// 剪贴板上屏：{exe} → 文本（未启用/白名单拒/空剪贴板 → None 或空串）。
pub fn clipboard_request(exe: &str) -> Option<String> {
    let resp = call(&serde_json::json!({"op": "clipboard", "exe": exe}))?;
    match resp.get("text") {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}
