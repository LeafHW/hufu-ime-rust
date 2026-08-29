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
            Some(k) => {
                let schema_before = host.engine.config.schema.current.clone();
                let r = host.process_key(k);
                // Ctrl+M 切方案：落盘 + 重装整句（与 HTTP /api/schema 行为一致）
                if host.engine.config.schema.current != schema_before {
                    let _ = host.engine.config.save(&host.config_path);
                    host.setup_sentence();
                }
                host.after_ime_op(); // 神经重排派发（异步）
                r
            }
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
            let show_index = host.engine.config.candidates.show_index;
            let delay_show_ms = host.engine.config.candidates.delay_show_ms;
            match hufu_skin::Skin::load(&p) {
                Ok(s) => {
                    serde_json::json!({"skin": s, "show_index": show_index, "delay_show_ms": delay_show_ms})
                }
                Err(e) => {
                    eprintln!("皮肤 {id} 加载失败，候选窗回默认: {e}");
                    serde_json::json!({
                        "skin": hufu_skin::Skin::default(),
                        "show_index": show_index,
                        "delay_show_ms": delay_show_ms
                    })
                }
            }
        }
        // 输入法激活态上报（DLL Activate/Deactivate）：驱动托盘图标显隐
        "ime" => {
            let active = req.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
            crate::tray::on_ime_state(active);
            serde_json::json!({"ok": true})
        }
        // 语言栏「中」按钮点击：开设置页（与托盘双击/Ctrl+Alt+H 同通道）
        "settings" => {
            crate::tray::open_settings();
            serde_json::json!({"ok": true})
        }
        // 越进程候选窗（沉浸式宿主如开始菜单搜索：DLL 自绘窗被 DWM
        // cloaked、UIElement 被宿主拒绝 → server 代画【用户皮肤】）
        "cand" => {
            let items: Vec<(String, String)> = req
                .get("items")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .map(|c| {
                            (
                                c.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                                c.get("comment").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let raw = req.get("raw").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let sel = req.get("selected").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
            let x = req.get("x").and_then(|x| x.as_i64()).unwrap_or(100) as i32;
            let y = req.get("y").and_then(|x| x.as_i64()).unwrap_or(100) as i32;
            let skin = req.get("skin").cloned().unwrap_or(serde_json::Value::Null);
            crate::candwin::show(
                crate::candwin::CandFrame { items, raw, selected: sel, skin },
                x,
                y,
            );
            serde_json::json!({"ok": true})
        }
        "cand_hide" => {
            crate::candwin::hide();
            serde_json::json!({"ok": true})
        }
        "sound" => {
            // tag → {data: base64 wav, volume}（文件缺失返回 404 语义 null）
            let tag = req.get("tag").and_then(|t| t.as_str()).unwrap_or("");
            let safe = ["key", "select", "commit", "page"];
            if !safe.contains(&tag) {
                return serde_json::json!({"error": "未知音效"});
            }
            let vol = host.engine.config.sound.volume;
            let p = host.data_dir.join("音效").join(format!("{tag}.wav"));
            match std::fs::read(&p) {
                Ok(bytes) => serde_json::json!({
                    "data": base64_encode(&bytes),
                    "volume": vol,
                }),
                Err(_) => serde_json::json!({"data": null, "volume": vol}),
            }
        }
        "clipboard" => {
            // {exe} → {text}：白名单校验 + 读剪贴板（Ctrl+Shift+V 剪贴板上屏）
            let cfg = host.engine.config.clipboard.clone();
            if !cfg.enabled {
                return serde_json::json!({"text": null, "reason": "disabled"});
            }
            let exe = req.get("exe").and_then(|t| t.as_str()).unwrap_or("");
            let exe = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
            if !cfg.allows(exe) {
                return serde_json::json!({"text": null, "reason": "whitelist"});
            }
            #[cfg(windows)]
            let text = crate::clipboard::read_text();
            #[cfg(not(windows))]
            let text = String::new();
            serde_json::json!({"text": text})
        }
        op => serde_json::json!({"error": format!("未知操作: {op}")}),
    }
}

/// 标准 base64 编码（服务端自足实现，免新增依赖）。
fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
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
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl: *const u16,
            revision: u32,
            sd: *mut *mut core::ffi::c_void,
            returned: *mut u32,
        ) -> i32;
        fn LocalFree(h: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
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
        // 管道 DACL（SDDL）：显式授 Everyone + ALL APPLICATION PACKAGES
        // 读写——默认 DACL（仅创建者/管理员）会拒绝 AppContainer 宿主
        //（开始菜单搜索 SearchHost 等 SystemApps）→ 搜索框里虎符取词
        // 失败、字母直通（2026-08-29 实测病灶之一）。
        #[repr(C)]
        struct SecurityAttributes {
            nLength: u32,
            lp_security_descriptor: *mut core::ffi::c_void,
            inherit_handle: i32,
        }
        let sddl: Vec<u16> = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GWGR;;;WD)(A;;GWGR;;;S-1-15-2-1)(A;;GWGR;;;S-1-15-2-2)\0"
            .encode_utf16()
            .collect();
        let mut sd: *mut core::ffi::c_void = std::ptr::null_mut();
        let sd_ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1, // SDDL_REVISION_1
                &mut sd,
                std::ptr::null_mut(),
            )
        } != 0;
        // 诊断落盘：SDDL 是否成功转换（AppContainer 管道连通排查）
        let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
        let _ = std::fs::write(
            r"C:\ProgramData\HuFu\diag\pipe-sddl.txt",
            format!("sd_ok={sd_ok} err={:?}\n", std::io::Error::last_os_error()),
        );
        let sa = SecurityAttributes {
            nLength: std::mem::size_of::<SecurityAttributes>() as u32,
            lp_security_descriptor: sd,
            inherit_handle: 0,
        };
        let sa_ptr: *const core::ffi::c_void = if sd_ok {
            &sa as *const SecurityAttributes as *const core::ffi::c_void
        } else {
            std::ptr::null()
        };
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
                    sa_ptr,
                )
            };
            if h == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            if unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } == 0 {
                let err = std::io::Error::last_os_error();
                // ERROR_NO_DATA=232：客户端已断开，继续接受下一连接
                if err.raw_os_error() == Some(232) {
                    unsafe { CloseHandle(h) };
                    continue;
                }
                // ERROR_PIPE_CONNECTED=535：客户端在 Create 与 Connect 之间已连上（竞态），
                // 视为已连接，正常服务 —— 之前当致命错误退出，会把监听线程带崩。
                if err.raw_os_error() == Some(535) {
                    let host = host.clone();
                    std::thread::spawn(move || serve_conn(h, &host));
                    continue;
                }
                // 其他错误：日志 + 短歇再战，绝不退出（管道是输入法生命线，
                // 任何单次异常都不能杀掉监听）
                eprintln!("管道连接异常: {err}，50ms 后继续");
                unsafe { CloseHandle(h) };
                std::thread::sleep(std::time::Duration::from_millis(50));
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
mod unix_imp {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;

    fn sock_path() -> std::path::PathBuf {
        std::env::var("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
            .join("hufu-ime.sock")
    }

    fn serve_conn(mut stream: std::os::unix::net::UnixStream, host: &Mutex<Host>) {
        loop {
            let mut head = [0u8; 4];
            if stream.read_exact(&mut head).is_err() {
                break;
            }
            let len = u32::from_le_bytes(head) as usize;
            if len == 0 || len > BUF {
                break;
            }
            let mut body = vec![0u8; len];
            if stream.read_exact(&mut body).is_err() {
                break;
            }
            let req: serde_json::Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => serde_json::json!({"error": e.to_string()}),
            };
            let resp = dispatch(host, &req);
            let out = serde_json::to_vec(&resp).unwrap_or_default();
            let mut frame = (out.len() as u32).to_le_bytes().to_vec();
            frame.extend_from_slice(&out);
            if stream.write_all(&frame).is_err() {
                break;
            }
        }
    }

    pub fn run(host: std::sync::Arc<Mutex<Host>>) -> std::io::Result<()> {
        let path = sock_path();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        eprintln!("HuFu unix socket: {}", path.display());
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    let host = host.clone();
                    std::thread::spawn(move || serve_conn(s, &host));
                }
                Err(_) => continue,
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
pub use unix_imp::run as run_pipe;
