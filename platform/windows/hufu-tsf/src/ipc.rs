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
/// 【公开 2026-10-08】fontinfo 侧（lib.rs find_font_files）解析字体目录
/// 也用它——SystemIME 副本 DLL 的自身路径不在安装根，必须走注册表。
pub(crate) fn read_installdir() -> Option<String> {
    // 【lint 2026-10-26 审计】签名与 addword.rs 的既有声明对齐（isize
    // 句柄风格）——rustc 1.98 跨模块同名 extern 签名不一致会告警。
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegOpenKeyExW(
            key: isize,
            subkey: *const u16,
            reserved: u32,
            desired: u32,
            result: *mut isize,
        ) -> i32;
        fn RegQueryValueExW(
            key: isize,
            name: *const u16,
            reserved: *mut u32,
            vtype: *mut u32,
            data: *mut u8,
            cb: *mut u32,
        ) -> i32;
        fn RegCloseKey(key: isize) -> i32;
    }
    const HKEY_CURRENT_USER: isize = 0x8000_0001usize as isize;
    const KEY_QUERY_VALUE: u32 = 0x0001;
    const KEY_WOW64_64KEY: u32 = 0x0100;
    const REG_SZ: u32 = 1;
    unsafe {
        let sub: Vec<u16> = "Software\\HuFu".encode_utf16().chain([0]).collect();
        let val: Vec<u16> = "InstallDir".encode_utf16().chain([0]).collect();
        let mut hk: isize = 0;
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            0,
            KEY_QUERY_VALUE | KEY_WOW64_64KEY,
            &mut hk,
        ) != 0
        {
            return None;
        }
        let mut typ = 0u32;
        let mut size = 0u32;
        if RegQueryValueExW(
            hk,
            val.as_ptr(),
            std::ptr::null_mut(),
            &mut typ,
            std::ptr::null_mut(),
            &mut size,
        ) != 0
            || typ != REG_SZ
            || size == 0
        {
            RegCloseKey(hk);
            return None;
        }
        let mut size = size.min(32768);
        let mut buf = vec![0u8; size as usize];
        let ok = RegQueryValueExW(
            hk,
            val.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            &mut size,
        ) == 0;
        RegCloseKey(hk);
        if !ok {
            return None;
        }
        let units: Vec<u16> = buf[..size as usize]
            .chunks_exact(2)
            .map(|c| u16::from_ne_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        Some(String::from_utf16_lossy(&units))
    }
}

/// 【2026-11 安全阀】当前进程是否提权（TokenElevation）。
/// 提权宿主里拉起 server = 高完整性级别常驻进程，普通权限杀不掉——
/// 用户反馈「退出游戏也没用、必须重启电脑」的直接来源之一。
fn process_is_elevated() -> bool {
    #[repr(C)]
    struct TokenElevation {
        token_is_elevated: u32,
    }
    const TOKEN_QUERY: u32 = 0x0008;
    const TOKEN_ELEVATION_CLASS: i32 = 20;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
    }
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: isize, access: u32, token: *mut isize) -> i32;
        fn GetTokenInformation(
            token: isize,
            class: i32,
            info: *mut core::ffi::c_void,
            len: u32,
            ret_len: *mut u32,
        ) -> i32;
    }
    unsafe {
        let mut t: isize = 0;
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut t) == 0 {
            return false;
        }
        let mut te = TokenElevation {
            token_is_elevated: 0,
        };
        let mut ret = 0u32;
        let ok = GetTokenInformation(
            t,
            TOKEN_ELEVATION_CLASS,
            &mut te as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<TokenElevation>() as u32,
            &mut ret,
        );
        let _ = CloseHandle(t);
        ok != 0 && te.token_is_elevated != 0
    }
}

/// 【2026-11-02 P1 附带】命名管道是否已存在（不区分能否打开）。
/// `ensure_server` 只在 pipe=fail 时被调用；若 pipe 存在但连接失败
///（ACL/繁忙/服务端挂起），盲目再拉一个 server 会变成“每宿主一个
/// server”的风暴（本次 TSF 单测在沙箱里实测 20+ 个）。存在即不拉，
/// 30s 后由正常重试/用户重启解决。
fn server_pipe_present() -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn WaitNamedPipeW(name: *const u16, timeout: u32) -> i32;
    }
    // 【P0-5 2026-11-03】raw string 里的 `\0` 是字面「反斜杠+0」，不是
    // NUL 结尾：WaitNamedPipeW 读到未终止缓冲区（UB），实测 err=161
    // (ERROR_BAD_PATHNAME) → 旧判据「非 2 即存在」恒真 → ensure_server
    // 永不拉起（server 被杀后输入法永久打不了字）。这里显式补 NUL。
    let mut name: Vec<u16> = r"\\.\pipe\hufu-ime".encode_utf16().collect();
    name.push(0);
    let ok = unsafe { WaitNamedPipeW(name.as_ptr(), 0) };
    if ok != 0 {
        return true;
    }
    // 只把明确的「管道对象存在」错误当成存在；未知错误保守放行去拉起
    // （宁可多拉一次由单实例互斥兜底，也不要像旧版那样永久不再拉起）。
    match std::io::Error::last_os_error().raw_os_error().unwrap_or(0) {
        // 5=拒绝访问 / 121=信号超时 / 231=实例全忙：管道对象存在
        5 | 121 | 231 => true,
        // 2=ERROR_FILE_NOT_FOUND：确实没有管道
        2 => false,
        _ => false,
    }
}

/// 【审计 P3 2026-11-05】当前进程是否 AppContainer 沙箱（TokenIsAppContainer）。
/// 开始菜单/UWP 等宿主运行在 AppContainer：非 elevated 会穿过提权闸门，
/// 但 CreateProcessW(env=null) 继承受限 token，受限 server 建的管道 ACL
/// 可能拒普通进程——「server 缺席 + AppContainer 首拉」窗口的理论风险。
/// 与提权闸门同款：跳过拉起，由正常进程（explorer 等）负责拉 server。
fn process_is_appcontainer() -> bool {
    const TOKEN_QUERY: u32 = 0x0008;
    const TOKEN_APPCONTAINER_CLASS: i32 = 29;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
    }
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: isize, access: u32, token: *mut isize) -> i32;
        fn GetTokenInformation(
            token: isize,
            class: i32,
            info: *mut core::ffi::c_void,
            len: u32,
            ret_len: *mut u32,
        ) -> i32;
    }
    unsafe {
        let mut t: isize = 0;
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut t) == 0 {
            return false;
        }
        let mut is_ac: u32 = 0;
        let mut ret = 0u32;
        let ok = GetTokenInformation(
            t,
            TOKEN_APPCONTAINER_CLASS,
            &mut is_ac as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
            &mut ret,
        );
        let _ = CloseHandle(t);
        ok != 0 && is_ac != 0
    }
}

fn ensure_server() -> bool {
    // 【2026-11 安全阀】提权宿主不拉起 server（详见 process_is_elevated）：
    // 宁可在提权进程里打字失败，也不留一个普通权限杀不掉的常驻 server。
    if process_is_elevated() {
        crate::tsf::trace("ipc: 提权宿主 → 不拉起 server（防高 IL 常驻进程）");
        return false;
    }
    // 【审计 P3 2026-11-05】AppContainer 宿主同款跳过（防受限 token server）。
    if process_is_appcontainer() {
        crate::tsf::trace("ipc: AppContainer 宿主 → 不拉起 server（防受限 token server）");
        return false;
    }
    // 【2026-11-02】pipe 已经存在但连接失败（ACL/繁忙）→ 不再无限拉起。
    if server_pipe_present() {
        crate::tsf::trace("ipc: server pipe 已存在（连接失败可能是 ACL/忙）→ 不重复拉起");
        return false;
    }
    use std::sync::atomic::{AtomicU64, Ordering};
    // 【退避重试 2026-09-11】旧实现 AtomicBool 一次性闸门：首次拉起
    // 失败（安装目录未就绪/杀毒拦截）后本进程永远不再尝试——用户
    // 表现为「打字失效直到重启宿主」。改时间戳退避：失败 30s 后放行
    // 重试。
    // 【成功≠存活 2026-10-06】成功后钉死 u64::MAX 是今晚「打不了字」
    // 8 分钟事故的根因：CreateProcessW 成功只代表进程创建——多宿主
    // 并发拉起时单实例败者会秒退（互斥体让路，设计如此），胜者也
    // 可能随后被杀（打包腾位/任务管理器/崩溃）。DLL 一旦记过「我拉
    // 起过」就永久封口，之后 pipe=fail 永不再拉——实锤链：杀 server
    // →多宿主并发拉起→胜者存活→打包再杀胜者→全员 NEXT_TRY=MAX→
    // 用户打字 pipe=fail perr=2 无人复活。改：成功也只给 5s 短冷却
    //（最坏每 5s 一次空拉，单实例互斥兜底零副作用），失败维持 30s。
    static NEXT_TRY_MS: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let cur = NEXT_TRY_MS.load(Ordering::SeqCst);
    if now < cur {
        return false; // 冷却窗口内（成功 5s / 失败 30s）
    }
    if NEXT_TRY_MS
        .compare_exchange(cur, now + 30_000, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return false; // 并发拉起：让先到者去
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
    let mut candidates = vec![format!("{exe_dir}\\hufu-server.exe")];
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
            // 成功只给短冷却（见函数头注释：创建成功≠存活）
            NEXT_TRY_MS.store(now + 5_000, Ordering::SeqCst);
            crate::tsf::trace("ipc: 已自愈拉起 hufu-server（5s 后可再试）");
            return true;
        }
    }
    crate::tsf::trace("ipc: ensure_server 全候选失败（30s 后重试）");
    false
}

/// 【持久连接 2026-09-08】进程级管道复用：此前每请求新建连接（每键
/// + 每 40ms poll=每秒 30+ 连接），server 每连接 spawn 线程——线程
/// 创建风暴（系统采样 ±7 线程/秒）+ 偶发系统抖动下 accept 排队 =
/// 300ms 尾延迟（02:40:33/45 两卡顿实锤：dispatch→pipe back 330-
/// 353ms 而 server CPU 仅 4%）。server 端 serve_conn 本就是「单连接
/// 循环处理」+ PIPE_UNLIMITED_INSTANCES——客户端持久连接零改动即用。
/// 串行化（跨线程请求在锁上排队）无损：server dispatch 本就持全局
/// 锁串行。断线/写失败/读超时弃连接重连一次（防响应错位）。
/// 【双通道 2026-09-08】键路径专用连接：与后台 op（poll state/focus/
/// 音效）物理分离——单连接时代一次慢响应（读超时上限 2s）会把
/// PIPE_CONN 锁占住，后续按键全部排队（用户实测重启后仍卡：打字
/// 高峰撞上 poll 即卡）。键通道永不与后台争抢。
/// 通道状态：连接 + 响应 body 复用缓冲。【热路径缓冲复用】旧版每响应
/// `vec![0u8; len]`（每键一次分配 + 全量零填）——改为通道级缓冲
/// `clear()+resize()`，容量足够时零分配，零填只发生在扩容段；断线
/// 弃连接时缓冲保留复用。
struct Chan {
    file: Option<std::fs::File>,
    buf: Vec<u8>,
}
static PIPE_KEY: std::sync::Mutex<Chan> = std::sync::Mutex::new(Chan {
    file: None,
    buf: Vec::new(),
});
/// 后台通道（poll/focus/音效/探测）：读上限压到 150ms——后台慢就
/// 弃连接重来，绝不长时间占锁。
static PIPE_BG: std::sync::Mutex<Chan> = std::sync::Mutex::new(Chan {
    file: None,
    buf: Vec::new(),
});

/// 【P1-6 2026-11-04】连接失败退避截止（unix ms；0=无退避）。
/// 病灶：server 缺席时每次 connect_pipe 都要走 CreateFileW 失败 →
/// ensure_server（首遇）→ WaitNamedPipeW(150ms) ×2，单次调用最坏
/// ~450ms 全在调用线程（键路径=宿主 UI 线程；ssf 坏皮取图=每帧一次）。
/// 失败后 400ms 内直接返回 None：拉起窗口本就 >300ms，迟到的 server
/// 最多晚 400ms 被感知（下一键立即重连）。
static CONN_DOWN_UNTIL_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
/// 退避时长：400ms（>WaitNamedPipe 单轮 150ms，<人眼可感的 server 迟到）
const CONN_BACKOFF_MS: u64 = 400;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 建立新连接（含 server 缺席拉起逻辑——与旧版一致）。
unsafe fn connect_pipe() -> Option<std::fs::File> {
    // 【P1-6】退避窗口内零等待返回（不 CreateFile/不等待）
    let now = now_ms();
    let down_until = CONN_DOWN_UNTIL_MS.load(std::sync::atomic::Ordering::Relaxed);
    if now < down_until {
        return None;
    }
    let name: Vec<u16> = PIPE.encode_utf16().chain([0]).collect();
    let mut tries = 0u32;
    let mut wait_ms: u32 = 150;
    let mut max_tries: u32 = 2;
    let mut spawned = false;
    loop {
        let h = CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        );
        if h != INVALID {
            CONN_DOWN_UNTIL_MS.store(0, std::sync::atomic::Ordering::Relaxed);
            return Some(std::fs::File::from_raw_handle(h as RawHandle));
        }
        let err = unsafe { GetLastError() };
        LAST_PIPE_ERR.store(err, std::sync::atomic::Ordering::SeqCst);
        eprintln!("ipc: connect_pipe fail err={err}");
        // 打不开：server 不在则拉起（首遇给足启动时间）
        if !spawned {
            spawned = true;
            if ensure_server() {
                // 【拉起预算 2026-09-11】新进程冷启（载词典）需要余量，
                // 但 3×1000ms 太肥（后台 poll 线程会陪等 3s+）；压到
                // 2×800ms——实测词典装 <1s，够用且不拖死通道上限。
                wait_ms = 800;
                max_tries = 2;
            }
        }
        if WaitNamedPipeW(name.as_ptr(), wait_ms) == 0 || tries >= max_tries {
            // 【P1-6】整轮失败 → 进入 400ms 退避，避免每键/每帧重付等待
            CONN_DOWN_UNTIL_MS.store(
                now_ms() + CONN_BACKOFF_MS,
                std::sync::atomic::Ordering::Relaxed,
            );
            return None;
        }
        tries += 1;
    }
}

/// 后台请求（poll/focus/音效/探测）：后台通道 + 150ms 响应上限。
/// 【热路径】Value → 帧一次成型：4B 长度占位 + to_writer 直接序列化
/// 进同一 Vec（旧版 to_vec 出 body、再 to_le_bytes().to_vec() 出头、
/// extend 拼帧 = 每请求两次分配 + 一次多余 memcpy）。
pub fn call(req: &Value) -> Option<Value> {
    let mut frame: Vec<u8> = Vec::with_capacity(128);
    frame.extend_from_slice(&[0u8; 4]); // 长度占位，序列化后回填
    serde_json::to_writer(&mut frame, req).ok()?;
    let n = (frame.len() - 4) as u32;
    frame[..4].copy_from_slice(&n.to_le_bytes());
    call_on(&PIPE_BG, &frame, 150, 150)
}

fn call_on(
    slot: &std::sync::Mutex<Chan>,
    frame: &[u8],
    resp_timeout: u64,
    body_timeout: u64,
) -> Option<Value> {
    unsafe {
        let mut g = slot.lock().unwrap_or_else(|p| p.into_inner());
        for _attempt in 0..2 {
            if g.file.is_none() {
                g.file = connect_pipe();
                if g.file.is_none() {
                    return None;
                }
            }
            // 字段拆借：file 与 buf 互不重叠可同时可变借用（body 直写复用缓冲）
            let Chan { file, buf } = &mut *g;
            let f = file.as_mut().unwrap();
            // 【读超时】阻塞 read_exact 无超时——server 端 dispatch 持全局
            // Host 锁，长操作（切方案重装整句等）排队期间响应悬死，调用方
            // 线程（常为宿主 UI 线程）永久冻结（VSCode「点击候选框应用未
            // 响应」事故）。PeekNamedPipe 轮询，超时走降级。
            let raw_pipe = f.as_raw_handle() as isize;
            let wait_response = |total_ms: u64| -> Option<()> {
                let deadline =
                    std::time::Instant::now() + std::time::Duration::from_millis(total_ms);
                // 【响应等待分级】server 处理一次 key 通常 <3ms：先自旋让快
                // 响应零等待，再让步、再 1ms/10ms 粒度递进。旧版上来就
                // sleep(10ms)——每键白交 10ms 量子税，真实打字延迟的大头。
                let start = std::time::Instant::now();
                loop {
                    let mut avail: u32 = 0;
                    if PeekNamedPipe(
                        raw_pipe,
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        &mut avail,
                        std::ptr::null_mut(),
                    ) != 0
                    {
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
            // 【读满超时 2026-09-09】wait_response 只保证 ≥1 字节可读；
            // read_exact 等满 4 字节头/整帧 body 时若 server 中途挂死
            // （大帧分片到达后对端崩溃）仍会无限阻塞冻结宿主 UI 线程
            // ——改为循环 Peek 直到凑满 n 字节或超时，不满即弃连接。
            let read_full_timeout =
                |f: &mut std::fs::File, buf: &mut [u8], raw_pipe: isize, total_ms: u64| -> bool {
                    let deadline =
                        std::time::Instant::now() + std::time::Duration::from_millis(total_ms);
                    let mut filled = 0usize;
                    while filled < buf.len() {
                        let mut avail: u32 = 0;
                        if PeekNamedPipe(
                            raw_pipe,
                            std::ptr::null_mut(),
                            0,
                            std::ptr::null_mut(),
                            &mut avail,
                            std::ptr::null_mut(),
                        ) == 0
                        {
                            return false; // 管道断
                        }
                        if avail as usize >= buf.len() - filled {
                            if f.read_exact(&mut buf[filled..]).is_err() {
                                return false;
                            }
                            return true;
                        }
                        if avail > 0 {
                            // 先取走已到部分，继续等剩余
                            let take = (avail as usize).min(buf.len() - filled);
                            if f.read_exact(&mut buf[filled..filled + take]).is_err() {
                                return false;
                            }
                            filled += take;
                        }
                        if std::time::Instant::now() >= deadline {
                            return false;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                    true
                };
            if f.write_all(frame).is_err() {
                // 断线：弃连接重试一次
                *file = None;
                continue;
            }
            if wait_response(resp_timeout).is_none() {
                // 超时/断线：弃连接（防响应错位）走降级
                // 【审计 P2-F 2026-11-05】响应超时 =「server 活着但 dispatch
                // 挂死/全局锁被长操作占住」——此前该场景不设退避（退避
                // 只在 CreateFileW 失败路径），管道对象还在，下一键重连成
                // 功后再等满全额超时（键通道 2s、poll 每拍 150ms），挂死
                // 期间宿主 UI 持续卡。挂死窗口设 200ms 短退避（低于连接
                // 失败的 400ms：挂死可能自愈，且本次已付全价等待）。
                CONN_DOWN_UNTIL_MS.store(
                    now_ms() + 200,
                    std::sync::atomic::Ordering::Relaxed,
                );
                *file = None;
                return None;
            }
            let mut head = [0u8; 4];
            if !read_full_timeout(f, &mut head, raw_pipe, body_timeout.max(500)) {
                // 【审计 P2-F 2026-11-05】读不满同款：分片后挂死/断线，
                // 设短退避再弃连接（管道真断时下轮 connect 失败会覆盖为
                // 400ms 正常退避，两值取后写者，无冲突）。
                CONN_DOWN_UNTIL_MS.store(
                    now_ms() + 200,
                    std::sync::atomic::Ordering::Relaxed,
                );
                *file = None;
                return None;
            }
            let len = u32::from_le_bytes(head) as usize;
            // 32MB：皮肤 op 携带整份皮肤（挂件动图入皮肤后实测 13MB+，
            // 12MB 闸会让 op=skin 静默失败→全线回默认皮「迷雾」事故
            // 2026-10-03）；其余 op 响应远小于此，上限只作防错 sanity
            if len == 0 || len > (32 << 20) {
                *file = None;
                return None;
            }
            // body 可能分片到达：头 4 字节已到不代表全帧已到。
            // 【缓冲复用】直写通道级 buf（见 Chan 注释）
            buf.clear();
            buf.resize(len, 0);
            if !read_full_timeout(f, buf, raw_pipe, body_timeout.max(500)) {
                *file = None;
                return None;
            }
            return serde_json::from_slice(buf).ok();
        }
        None
    }
}

/// 按键请求 → (consumed, commit, back, state, sound, sound_vol, sound_pack)
/// line_end：前端检测到组段逼近窗口右缘（软换行边界）——引擎提前上屏
/// 确认键数 2→1，组段早一步缩短（见 engine Session::line_end_hint）。
pub fn key_request(
    key: &str,
    shift: bool,
    ctrl: bool,
    alt: bool,
    line_end: bool,
    digit_tail: Option<&str>,
) -> Option<(bool, String, u8, Value, Option<String>, u8, String)> {
    // 【七十七修·digit_tail 补齐】空态键随带 TestDown 记的直通数字
    // 尾巴——server 端做后缀补齐（engine tail 没有这段才追加，递键
    // 宿主 QQ 的 tail 不被覆盖污染）。
    // 【热路径手写序列化】键请求是最高频 IPC（每键一次）：json! 宏每次
    // 堆一棵 Value 树（两层 Map + 键值字符串 ~10 次分配）再遍历序列化。
    // 字面量字段直接写文本零分配；动态串（key/digit_tail）走
    // serde_json::to_writer 的标准字符串转义，与 json! 输出等价。
    let mut body: Vec<u8> = Vec::with_capacity(96);
    body.extend_from_slice(b"{\"op\":\"key\",\"key\":");
    serde_json::to_writer(&mut body, key).ok()?;
    body.extend_from_slice(b",\"modifiers\":{\"shift\":");
    body.extend_from_slice(if shift { b"true" } else { b"false" });
    body.extend_from_slice(b",\"ctrl\":");
    body.extend_from_slice(if ctrl { b"true" } else { b"false" });
    body.extend_from_slice(b",\"alt\":");
    body.extend_from_slice(if alt { b"true" } else { b"false" });
    body.extend_from_slice(b"},\"line_end\":");
    body.extend_from_slice(if line_end { b"true" } else { b"false" });
    if let Some(t) = digit_tail {
        body.extend_from_slice(b",\"digit_tail\":");
        serde_json::to_writer(&mut body, t).ok()?;
    }
    body.extend_from_slice(b",\"candidate_ui\":");
    serde_json::to_writer(&mut body, &crate::candidate_ui::request()).ok()?;
    body.extend_from_slice(b"}");
    let mut frame: Vec<u8> = Vec::with_capacity(body.len() + 4);
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    // 键通道（与后台物理分离）+ 完整超时（server 正常 <3ms；上限保留
    // 给极端恢复场景）
    let mut resp = match call_on(&PIPE_KEY, &frame, 2000, 1000) { Some(r)=>r, None=>{crate::candidate_ui::reset();return None;} };
    // 【热路径】state 用 take 移交所有权——旧版 .cloned() 每键深拷
    // 整棵 state 子树（候选数组/编码串全在里面）
    let state = resp
        .get_mut("state")
        .map(std::mem::take)
        .unwrap_or(Value::Null);
    let outcome = resp.get("outcome")?;
    let consumed = outcome
        .get("consumed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let commit = outcome
        .get("commit")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let back = outcome.get("back").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
    let sound = outcome
        .get("sound")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    // 音量每键随行（热生效：设置页改音量 → 下一键即新音量）
    let sound_vol = outcome
        .get("sound_vol")
        .and_then(|v| v.as_u64())
        .unwrap_or(70) as u8;
    // 【音效包 2026-10-30】包名随行：sound.rs 比对变更即失效缓存
    let sound_pack = outcome
        .get("sound_pack")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some((consumed, commit, back, state, sound, sound_vol, sound_pack))
}

// 【三十四修·死代码删除】ping()（零调用）已删。

/// 轮询用：拉当前会话态（不产生按键副作用）。供候选窗停顿期刷新
/// （异步重排到达后主动换序，用户不必按键即可看到新首选）。
pub fn state_request() -> Option<Value> {
    let resp = match call(&serde_json::json!({"op": "state", "candidate_ui": crate::candidate_ui::request()})) {Some(r)=>r,None=>{crate::candidate_ui::reset();return None;}};
    // 【2026-11 UI-less 异步刷新】轮询响应里带回了最新候选页（含后台重排/
    // 云候选到达后的结果）。这里无条件 publish 一次：宿主自绘（UI-less）
    // 时我们的候选窗被隐藏，渲染路径不一定走到 update_ui，靠这一行保证
    // 宿主看到的候选页始终是最新的（≈140ms 一拍；菜单未变则内部零动作）。
    if let Some(state) = resp.get("state") {
        crate::candidate_ui::publish(state);
    }
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
