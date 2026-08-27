//! 管道客户端探针：模拟 TSF DLL 的调用方式。
//! `cargo run -p hufu-cli --release --example pipeclient`
//! 前置：hufu-server 已运行（命名管道 \\.\pipe\hufu-ime）。
#![cfg(windows)]

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
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const INVALID: isize = -1;

fn call(req: &serde_json::Value) -> serde_json::Value {
    unsafe {
        let name: Vec<u16> = r"\\.\pipe\hufu-ime".encode_utf16().chain([0]).collect();
        let h = CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        );
        assert!(h != INVALID, "管道连接失败（hufu-server 未运行？）");
        // File 持有句柄，drop 时关闭
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
        let resp: serde_json::Value = serde_json::from_slice(&buf).expect("响应非 JSON");
        println!("[{}] {}", req["op"], serde_json::to_string(&resp).unwrap());
        resp
    }
}

fn main() {
    call(&serde_json::json!({"op":"ping"}));
    call(&serde_json::json!({"op":"reset"}));
    let k1 = call(&serde_json::json!({"op":"key","key":"u"}));
    assert_eq!(k1["state"]["raw"], "u", "u 键应建立编码");
    let k2 = call(&serde_json::json!({"op":"key","key":"space"}));
    assert_eq!(k2["outcome"]["commit"], "的", "空格应上屏首选");
    call(&serde_json::json!({"op":"state"}));
    let sk = call(&serde_json::json!({"op":"skin"}));
    assert!(sk["skin"]["colors"]["back_color"].is_string(), "皮肤应可用");
    println!("管道 IPC 全部通过 ✓");
}
