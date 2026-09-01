//! 管道 IPC 客户端：帧协议 4 字节小端长度 + JSON。

use serde_json::Value;
use std::io::{Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};

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
    fn PeekNamedPipe(
        h: isize,
        buf: *mut core::ffi::c_void,
        size: u32,
        read: *mut u32,
        avail: *mut u32,
        left: *mut u32,
    ) -> i32;
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
/// cb 必须按架构给（x64 sizeof=104 / i386=68）；rest 冗余放大到 96B
/// 保证两种架构下系统按 cb 读取时都在本结构内存内。
#[repr(C)]
#[allow(dead_code)]
struct SpawnBlock {
    si_cb: u32,
    si_rest: [u64; 12], // reserved..std_error 全零即可
    pi: [isize; 4],     // hProcess/hThread/pid/tid
}
impl SpawnBlock {
    fn zeroed() -> Self {
        SpawnBlock {
            si_cb: if cfg!(target_arch = "x86") { 68 } else { 104 },
            si_rest: [0; 12],
            pi: [0; 4],
        }
    }
}

/// 自愈：server 不在（管道打不开且无实例等待）时拉起 hufu-server.exe。
/// 每进程只试一次，防拉起风暴。返回 true=本次调用确实拉起了。
/// 读 HKCU\Software\HuFu 的 InstallDir（安装器写入的绿色模式安装目录）。
fn read_installdir() -> Option<String> {
    use std::ffi::c_void;
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegOpenKeyExW(hkey: *mut c_void, name: *const u16, opt: u32, access: u32, out: *mut *mut c_void) -> i32;
        fn RegQueryValueExW(hkey: *mut c_void, name: *const u16, res: *mut u32, typ: *mut u32, data: *mut u8, size: *mut u32) -> i32;
        fn RegCloseKey(hkey: *mut c_void) -> i32;
    }
    const HKEY_CURRENT_USER: *mut c_void = 0x8000_0001usize as *mut c_void;
    const KEY_QUERY_VALUE: u32 = 0x0001;
    const KEY_WOW64_64KEY: u32 = 0x0100;
    const REG_SZ: u32 = 1;
    unsafe {
        let sub: Vec<u16> = "Software\\HuFu".encode_utf16().chain([0]).collect();
        let val: Vec<u16> = "InstallDir".encode_utf16().chain([0]).collect();
        let mut hk = std::ptr::null_mut::<c_void>();
        if RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_QUERY_VALUE | KEY_WOW64_64KEY, &mut hk) != 0 {
            return None;
        }
        let mut typ = 0u32;
        let mut size = 0u32;
        if RegQueryValueExW(hk, val.as_ptr(), std::ptr::null_mut(), &mut typ, std::ptr::null_mut(), &mut size) != 0
            || typ != REG_SZ || size == 0 {
            RegCloseKey(hk);
            return None;
        }
        let mut size = size.min(32768);
        let mut buf = vec![0u8; size as usize];
        let ok = RegQueryValueExW(hk, val.as_ptr(), std::ptr::null_mut(), std::ptr::null_mut(), buf.as_mut_ptr(), &mut size) == 0;
        RegCloseKey(hk);
        if !ok { return None; }
        let units: Vec<u16> = buf[..size as usize]
            .chunks_exact(2)
            .map(|c| u16::from_ne_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        Some(String::from_utf16_lossy(&units))
    }
}

fn ensure_server() -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    static TRIED: AtomicBool = AtomicBool::new(false);
    if TRIED.swap(true, Ordering::SeqCst) {
        return false;
    }
    // 候选：宿主 exe 同目录（开发态）→ 注册表 InstallDir（绿色原地安装：
    // DLL 在 SystemIME 而程序在安装目录，安装器写入 HKCU\Software\HuFu）
    // → %LOCALAPPDATA%\HuFu（旧版布局兼容）→ 工程绝对路径（开发态兜底）。
    // 数据目录同理：安装态在 exe 旁「数据」目录，开发态回退工程 hufu-data。
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let dev_data = std::env::var("HUFU_DEV_DATA").unwrap_or_default(); // 配套 HUFU_DEV_SERVER
    let local_app = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let mut candidates = vec![
        format!("{exe_dir}\\hufu-server.exe"),
    ];
    if let Some(dir) = read_installdir() {
        candidates.push(format!("{}\\hufu-server.exe", dir.trim_end_matches('\\')));
    }
    candidates.push(format!("{local_app}\\HuFu\\hufu-server.exe"));
    // dev 兜底必须显式提供（HUFU_DEV_SERVER=开发机绝对路径）：
    // 发行 DLL 内置开发路径会在卸载窗口期拉起 dev 版 server（旧数据
    // 串场、提权宿主还会锁管道 ACL）——10 轮净室实测教训。
    if let Ok(dev_exe) = std::env::var("HUFU_DEV_SERVER") {
        if !dev_exe.is_empty() {
            candidates.push(dev_exe);
        }
    }
    for exe in candidates {
        if !std::path::Path::new(&exe).exists() {
            continue;
        }
        // 数据目录跟随 server 自身：安装态候选用 exe 旁「数据」（server
        // 不带 --data 时即此默认）；无旁挂数据时若提供了 HUFU_DEV_DATA
        // 则显式指过去，否则裸启动（server 自有 fallback）。
        let data_of = {
            let beside = format!("{exe}\\..\\数据");
            let beside = std::path::Path::new(&beside)
                .canonicalize()
                .unwrap_or_else(|_| std::path::PathBuf::from(&beside));
            if beside.exists() {
                None // 就用 server 默认（exe 同目录\数据）
            } else if dev_data.is_empty() {
                None
            } else {
                Some(dev_data.clone())
            }
        };
        let wexe: Vec<u16> = exe.encode_utf16().chain([0]).collect();
        let cmdline = match &data_of {
            Some(d) => format!("\"{exe}\" --data \"{d}\""),
            None => format!("\"{exe}\""),
        };
        let mut cmd: Vec<u16> = cmdline.encode_utf16().chain([0]).collect();
        let mut blk = SpawnBlock::zeroed();
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
        // 【读超时】阻塞 read_exact 无超时——server 端 dispatch 持全局
        // Host 锁，长操作（切方案重装整句等）排队期间响应悬死，调用方
        // 线程（常为宿主 UI 线程）永久冻结（VSCode「点击候选框应用未
        // 响应」事故）。PeekNamedPipe 轮询，硬上限 2 秒，超时走降级。
        let raw_pipe = f.as_raw_handle() as isize;
        let wait_response = |total_ms: u64| -> Option<()> {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(total_ms);
            // 【响应等待分级】server 处理一次 key 通常 <3ms：先自旋让快
            // 响应零等待，再让步、再 1ms/10ms 粒度递进。旧版上来就
            // sleep(10ms)——每键白交 10ms 量子税，真实打字延迟的大头。
            let start = std::time::Instant::now();
            loop {
                let mut avail: u32 = 0;
                if PeekNamedPipe(raw_pipe, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut()) != 0 {
                    if avail > 0 {
                        return Some(());
                    }
                } else {
                    return None; // 管道断（server 退出）
                }
                if std::time::Instant::now() >= deadline {
                    return None;
                }
                let el = start.elapsed();
                if el.as_micros() < 600 {
                    for _ in 0..32 {
                        std::hint::spin_loop();
                    }
                } else if el.as_millis() < 4 {
                    std::thread::yield_now();
                } else if el.as_millis() < 20 {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        };
        wait_response(2000)?;
        let mut head = [0u8; 4];
        f.read_exact(&mut head).ok()?;
        let len = u32::from_le_bytes(head) as usize;
        if len == 0 || len > (1 << 20) {
            return None;
        }
        // body 可能分片到达：头 4 字节已到不代表全帧已到
        wait_response(1000)?;
        let mut buf = vec![0u8; len];
        f.read_exact(&mut buf).ok()?;
        serde_json::from_slice(&buf).ok()
    }
}

/// 按键请求 → (consumed, commit, back, state, sound)
/// line_end：前端检测到组段逼近窗口右缘（软换行边界）——引擎提前上屏
/// 确认键数 2→1，组段早一步缩短（见 engine Session::line_end_hint）。
pub fn key_request(
    key: &str,
    shift: bool,
    ctrl: bool,
    alt: bool,
    line_end: bool,
) -> Option<(bool, String, u8, Value, Option<String>)> {
    let resp = call(&serde_json::json!({
        "op": "key",
        "key": key,
        "modifiers": { "shift": shift, "ctrl": ctrl, "alt": alt },
        "line_end": line_end
    }))?;
    let outcome = resp.get("outcome")?;
    let consumed = outcome.get("consumed").and_then(|v| v.as_bool()).unwrap_or(false);
    let commit = outcome
        .get("commit")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let back = outcome.get("back").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
    let state = resp.get("state").cloned().unwrap_or(Value::Null);
    let sound = outcome
        .get("sound")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Some((consumed, commit, back, state, sound))
}

/// 唤醒：探测服务器是否在。
pub fn ping() -> bool {
    call(&serde_json::json!({"op": "ping"}))
        .and_then(|v| v.get("ok").and_then(|o| o.as_bool()))
        .unwrap_or(false)
}

/// 轮询用：拉当前会话态（不产生按键副作用）。供候选窗停顿期刷新
/// （异步重排到达后主动换序，用户不必按键即可看到新首选）。
pub fn state_request() -> Option<Value> {
    let resp = call(&serde_json::json!({"op": "state"}))?;
    resp.get("state").cloned()
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
