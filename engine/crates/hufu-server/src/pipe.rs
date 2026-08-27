//! Windows 命名管道 IPC 服务（`\\.\pipe\hufu-ime`）。
//!
//! 帧协议：4 字节小端长度 + JSON。请求 `{"op":...}`：
//! `key`（含 key/modifiers）/ `state` / `reset` / `focus` / `ping`。
//! 与 HTTP API 共享 Host 与 parse_key。

use crate::host::{parse_key, Host};
use std::io::ErrorKind;
use std::sync::Mutex;

const PIPE_NAME: &str = r"\\.\pipe\hufu-ime";
const BUF: usize = 1 << 20;

/// 分派一个操作。返回 JSON 响应。
pub fn dispatch(host: &Mutex<Host>, req: &serde_json::Value) -> serde_json::Value {
    let mut host = host.lock().unwrap_or_else(|p| p.into_inner());
    match req.get("op").and_then(|o| o.as_str()).unwrap_or("") {
        "ping" => serde_json::json!({"ok": true, "server": "hufu"}),
        "key" => match parse_key(req) {
            Some(k) => host.process_key(k),
            None => serde_json::json!({"error": "按键描述无效"}),
        },
        "state" => {
            let state = host.engine.state(&host.session);
            serde_json::json!({
                "state": state,
                "current_schema": host.engine.config.schema.current,
                "sentence_active": host.engine.sentence_active(),
            })
        }
        "reset" => {
            host.session = hufu_engine::Session::new(true);
            let state = host.engine.state(&host.session);
            serde_json::json!({"state": state})
        }
        "focus" => {
            // 焦点切换：v1 单会话，仅清空
            host.session.clear();
            let state = host.engine.state(&host.session);
            serde_json::json!({"state": state})
        }
        "skin" => {
            let id = host.engine.config.appearance.skin.clone();
            let p = host.skins_dir().join(format!("{id}.json"));
            match hufu_skin::Skin::load(&p) {
                Ok(s) => serde_json::json!({"skin": s}),
                Err(_) => serde_json::json!({"skin": hufu_skin::Skin::default()}),
            }
        }
        op => serde_json::json!({"error": format!("未知操作: {op}")}),
    }
}

#[cfg(windows)]
mod imp {
    use super::*;

    // windows-sys 原型（保持零特性门）
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateNamedPipeW(
            name: *const u16,
            open_mode: u32,
            pipe_mode: u32,
            instances: u32,
            out_buf: u32,
            in_buf: u32,
            timeout: u32,
            sa: *const core::ffi::c_void,
        ) -> isize;
        fn ConnectNamedPipe(pipe: isize, overlapped: *mut core::ffi::c_void) -> i32;
        fn DisconnectNamedPipe(pipe: isize) -> i32;
        fn ReadFile(
            h: isize,
            buf: *mut u8,
            len: u32,
            read: *mut u32,
            overlapped: *mut core::ffi::c_void,
        ) -> i32;
        fn WriteFile(
            h: isize,
            buf: *const u8,
            len: u32,
            written: *mut u32,
            overlapped: *mut core::ffi::c_void,
        ) -> i32;
        fn CloseHandle(h: isize) -> i32;
    }

    const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
    const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
    const PIPE_READMODE_BYTE: u32 = 0x0000_0000;
    const PIPE_WAIT: u32 = 0x0000_0000;
    const PIPE_UNLIMITED_INSTANCES: u32 = 255;
    const INVALID_HANDLE_VALUE: isize = -1;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    fn read_exact(h: isize, n: usize) -> std::io::Result<Vec<u8>> {
        let mut buf = vec![0u8; n];
        let mut done = 0usize;
        while done < n {
            let mut got = 0u32;
            let ok = unsafe {
                ReadFile(h, buf.as_mut_ptr().add(done), (n - done) as u32, &mut got, std::ptr::null_mut())
            };
            if ok == 0 || got == 0 {
                return Err(std::io::Error::new(ErrorKind::UnexpectedEof, "管道关闭"));
            }
            done += got as usize;
        }
        Ok(buf)
    }

    fn write_all(h: isize, buf: &[u8]) -> std::io::Result<()> {
        let mut done = 0usize;
        while done < buf.len() {
            let mut wrote = 0u32;
            let ok = unsafe {
                WriteFile(h, buf.as_ptr().add(done), (buf.len() - done) as u32, &mut wrote, std::ptr::null_mut())
            };
            if ok == 0 {
                return Err(std::io::Error::new(ErrorKind::BrokenPipe, "管道写入失败"));
            }
            done += wrote as usize;
        }
        Ok(())
    }

    /// 单连接处理：读帧 → 派发 → 写帧，直至断开。
    fn serve_conn(h: isize, host: &Mutex<Host>) {
        loop {
            let head = match read_exact(h, 4) {
                Ok(b) => b,
                Err(_) => break,
            };
            let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
            if len == 0 || len > BUF {
                break;
            }
            let body = match read_exact(h, len) {
                Ok(b) => b,
                Err(_) => break,
            };
            let req: serde_json::Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => serde_json::json!({"error": e.to_string()}),
            };
            let resp = dispatch(host, &req);
            let mut out = serde_json::to_vec(&resp).unwrap_or_default();
            if out.len() > BUF {
                out = serde_json::json!({"error": "响应过大"}).to_string().into_bytes();
            }
            let mut frame = (out.len() as u32).to_le_bytes().to_vec();
            frame.extend_from_slice(&out);
            if write_all(h, &frame).is_err() {
                break;
            }
        }
        unsafe {
            DisconnectNamedPipe(h);
            CloseHandle(h);
        }
    }

    /// 阻塞运行管道服务（每实例一线程）。
    pub fn run(host: std::sync::Arc<Mutex<Host>>) -> std::io::Result<()> {
        let name = wide(PIPE_NAME);
        loop {
            let h = unsafe {
                CreateNamedPipeW(
                    name.as_ptr(),
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    64 * 1024,
                    64 * 1024,
                    0,
                    std::ptr::null(),
                )
            };
            if h == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            if unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } == 0 {
                let err = std::io::Error::last_os_error();
                unsafe { CloseHandle(h) };
                // ERROR_NO_DATA=232：客户端已断开，继续接受下一连接
                if err.raw_os_error() != Some(232) {
                    return Err(err);
                }
                continue;
            }
            let host = host.clone();
            std::thread::spawn(move || serve_conn(h, &host));
        }
    }
}

#[cfg(windows)]
pub use imp::run as run_pipe;

#[cfg(not(windows))]
pub fn run_pipe(_host: std::sync::Arc<Mutex<Host>>) -> std::io::Result<()> {
    Err(std::io::Error::new(ErrorKind::Unsupported, "仅 Windows"))
}
