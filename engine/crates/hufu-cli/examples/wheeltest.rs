//! 滚轮字号跟随回归探针：调 pipe skin_font_delta，验证
//! label_font_point 与 font_point 等比缩放、往返复位、0=自动跟随语义。
//! `cargo run -p hufu-cli --release --example wheeltest`
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
        let h = CreateFileW(
            name.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0,
            std::ptr::null(), OPEN_EXISTING, 0, 0,
        );
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

fn main() {
    // 读当前皮肤（server 数据目录在运行目录下）
    let dir = std::env::current_dir().expect("取当前目录失败").join("数据").join("皮肤");
    let read_skin = || -> (f64, f64, String) {
        for id in ["hufu-moyan", "hufu-default"] {
            let p = dir.join(format!("{id}.json"));
            if let Ok(s) = std::fs::read_to_string(&p) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                    return (
                        v["layout"]["font_point"].as_f64().unwrap_or(0.0),
                        v["layout"]["label_font_point"].as_f64().unwrap_or(0.0),
                        id.to_string(),
                    );
                }
            }
        }
        panic!("皮肤文件读取失败：{}", dir.display());
    };

    let (f0, l0, id) = read_skin();
    println!("皮肤 {id}: 起始 font={f0} label={l0}");

    // +1：等比放大
    let r = call(&serde_json::json!({"op": "skin_font_delta", "delta": 1}));
    let f1 = r["font_point"].as_f64().unwrap_or(0.0);
    let l1 = r["label_font_point"].as_f64().unwrap_or(0.0);
    println!("滚轮+1 → font={f1} label={l1}");
    if l0 > 0.0 {
        let want = (f0 + 1.0) * (l0 / f0);
        assert!(
            (l1 - want).abs() < 0.05,
            "序号应等比跟随：want={want:.3} got={l1:.3}"
        );
    } else {
        assert_eq!(l1, l0, "label=0（自动跟随）应保持 0");
    }

    // -1：往返复位
    let r = call(&serde_json::json!({"op": "skin_font_delta", "delta": -1}));
    let f2 = r["font_point"].as_f64().unwrap_or(0.0);
    let l2 = r["label_font_point"].as_f64().unwrap_or(0.0);
    println!("滚轮-1 → font={f2} label={l2}");
    assert_eq!(f2 as i64, f0 as i64, "往返主字复位");
    if l0 > 0.0 {
        assert!(
            (l2 - l0).abs() < 0.05,
            "往返序号复位：want={l0} got={l2}"
        );
    }
    let (f3, l3, _) = read_skin();
    assert_eq!(f3 as i64, f2 as i64, "皮肤文件已持久化");
    assert!((l3 - l2).abs() < 0.05, "皮肤文件 label 已持久化");
    println!("滚轮序号跟随全部通过 ✓（等比缩放 / 往返复位 / 落盘）");
}
