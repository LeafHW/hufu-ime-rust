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
    fn CreateProcessW(
        app: *const u16,
        cmd: *mut u16,
        sa: *const core::ffi::c_void,
        thread_sa: *const core::ffi::c_void,
        inherit: i32,
        flags: u32,
        env: *const core::ffi::c_void,
        cwd: *const u16,
        si: *mut core::ffi::c_void,
        pi: *mut core::ffi::c_void,
    ) -> i32;
    fn CloseHandle(h: isize) -> i32;
    fn GetLastError() -> u32;
}

/// 最近一次管道连接失败的 Win32 错误码（诊断用；0=无失败记录）
pub static LAST_PIPE_ERR: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const INVALID: isize = -1;
const PIPE: &str = r"\\.\pipe\hufu-ime";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// STARTUPINFOW + PROCESS_INFORMATION 原始布局（CreateProcessW 用）。
#[repr(C)]
#[allow(dead_code)]
struct SpawnBlock {
    si_cb: u32,
    si_rest: [u64; 10], // reserved..std_error 全零即可
    pi: [isize; 4],     // hProcess/hThread/pid/tid
}

/// 自愈：server 不在（管道打不开且无实例等待）时拉起 hufu-server.exe。
/// 每进程只试一次，防拉起风暴。返回 true=本次调用确实拉起了。
fn ensure_server() -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    static TRIED: AtomicBool = AtomicBool::new(false);
    if TRIED.swap(true, Ordering::SeqCst) {
        return false;
    }
    // 候选：宿主 exe 同目录（安装态）→ 工程绝对路径（开发态）。
    // 数据目录同理：安装态在 exe 旁「数据」目录，开发态回退工程 hufu-data。
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let dev_data = r"E:\DSH-KF\hufu\hufu-data";
    let data_dir = if std::path::Path::new(&format!("{exe_dir}\\数据")).exists() {
        format!("{exe_dir}\\数据")
    } else {
        dev_data.to_string()
    };
    let candidates = [
        format!("{exe_dir}\\hufu-server.exe"),
        r"E:\DSH-KF\hufu\engine\target\release\hufu-server.exe".to_string(),
    ];
    for exe in candidates {
        if !std::path::Path::new(&exe).exists() {
            continue;
        }
        let wexe: Vec<u16> = exe.encode_utf16().chain([0]).collect();
        let mut cmd: Vec<u16> = format!("\"{exe}\" --data \"{data_dir}\"")
            .encode_utf16()
            .chain([0])
            .collect();
        let mut blk = SpawnBlock {
            si_cb: 104, // sizeof(STARTUPINFOW)
            si_rest: [0; 10],
            pi: [0; 4],
        };
        let ok = unsafe {
            CreateProcessW(
                wexe.as_ptr(),
                cmd.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                CREATE_NO_WINDOW,
                std::ptr::null(),
                std::ptr::null(),
                &mut blk as *mut _ as *mut core::ffi::c_void,
                &mut blk.pi as *mut _ as *mut core::ffi::c_void,
            )
        };
        if ok != 0 {
            unsafe {
                CloseHandle(blk.pi[0]);
                CloseHandle(blk.pi[1]);
            }
            crate::tsf::trace("ipc: 已自愈拉起 hufu-server");
            return true;
        }
    }
    false
}

/// 单次请求（每次新建连接；本地管道往返 <100µs）。
/// 【等待策略】server 缺席时的阻塞上限（02:04 全机卡死事故根因）：
/// - 本进程首次发现缺席：拉起 server 并给足启动等待（3×1000ms，
///   仅每进程一次）
/// - 之后仍缺席：快败（2×150ms）——server 已死时绝不能把宿主
///   每一次按键拖进秒级等待，宁可这一帧走降级路径
pub fn call(req: &Value) -> Option<Value> {
    unsafe {
        let name: Vec<u16> = PIPE.encode_utf16().chain([0]).collect();
        let mut h;
        let mut tries = 0u32;
        let mut wait_ms: u32 = 150;
        let mut max_tries: u32 = 2;
        let mut spawned = false;
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
            LAST_PIPE_ERR.store(unsafe { GetLastError() }, std::sync::atomic::Ordering::SeqCst);
            // 打不开：server 不在则拉起（首遇给足启动时间）
            if !spawned {
                spawned = true;
                if ensure_server() {
                    wait_ms = 1000;
                    max_tries = 3;
                }
            }
            if WaitNamedPipeW(name.as_ptr(), wait_ms) == 0 || tries >= max_tries {
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

/// 测试钩子：重置引擎会话（回默认中文态；真实应用里的 Shift 切换会污染
/// 全局会话态，冒烟前需归零）。
pub fn reset_session() -> bool {
    call(&serde_json::json!({"op": "reset"})).is_some()
}

/// 剪贴板上屏：{exe} → 文本（未启用/白名单拒/空剪贴板 → None 或空串）。
pub fn clipboard_request(exe: &str) -> Option<String> {
    let resp = call(&serde_json::json!({"op": "clipboard", "exe": exe}))?;
    match resp.get("text") {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}
