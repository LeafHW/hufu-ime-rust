//! 进程/启动基础层：Toolhelp 枚举杀查、CreateProcessW 隐藏/捕获/分离启动、
//! ShellExecuteExW 提权等待启动。
//! 血律对照：install.ps1 Start-Process/Stop-Process/Invoke-SmokeReg 的进程语义。
//!
//! - Get-Process name → Toolhelp 快照 PROCESSENTRY32W.szExeFile 全名不区分大小写
//!   比较（PS 按进程名匹配，调用方传全名如 "hufu-server.exe"）。
//! - Stop-Process -Force → OpenProcess(PROCESS_TERMINATE)+TerminateProcess(1)，
//!   与 PS 同款无权限兜底（杀不动的项不计入击杀数）。
//! - Start-Process -WindowStyle Hidden -PassThru + WaitForExit(ms)（超时 Kill，
//!   Invoke-SmokeReg L74-82 语义：超时杀掉按 0/跳过处理）→ spawn_wait_hidden /
//!   spawn_capture。
//! - Start-Process -Verb RunAs -Wait → spawn_runas_wait（UAC 弹窗必须可见）。
//! - Start-Process（无等待）/ explorer.exe 中转降权（install.ps1 L510-518）→
//!   spawn_detached。
//! 全部走 W 系 API + 手动 Vec<u16>（UTF-16 路径含中文稳定，不 tostring 往返）。

use std::path::Path;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, OpenProcess, TerminateProcess, WaitForSingleObject,
    CREATE_NO_WINDOW, PROCESS_INFORMATION, PROCESS_TERMINATE, STARTF_USESTDHANDLES, STARTUPINFOW,
};
use windows::Win32::UI::Shell::{
    ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
};

/// Start-Process …-PassThru + WaitForExit 的三态：退出码 / 超时（已杀）/ 启动失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnResult {
    Exit(i32),
    Timeout,
    Fail,
}

// ── wide 字符串助手（&str 与 &Path 各一条无损通道，绝不 lossy 往返）──

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_path(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 命令行 = `"exe完整路径" args`（exe 加引号防空格歧义；args 由调用方
/// 预引好、原样拼接——契约：arg 含空格路径时调用方自带引号）。
fn build_cmdline(exe: &Path, args: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let mut v: Vec<u16> = Vec::new();
    v.push(b'"' as u16);
    v.extend(exe.as_os_str().encode_wide());
    v.push(b'"' as u16);
    if !args.is_empty() {
        v.push(b' ' as u16);
        v.extend(args.encode_utf16());
    }
    v.push(0);
    v
}

// ── Toolhelp 枚举 ──

/// 遍历进程快照；回调返回 true 提前停止。快照失败=静默空跑
/// （PS -ErrorAction SilentlyContinue 语义：枚举不了就当没找到）。
fn with_procs(mut f: impl FnMut(&PROCESSENTRY32W) -> bool) {
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return,
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                if f(&e) {
                    break;
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
}

fn entry_name(e: &PROCESSENTRY32W) -> String {
    let nul = e
        .szExeFile
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(e.szExeFile.len());
    String::from_utf16_lossy(&e.szExeFile[..nul])
}

/// 等待+超时收尸（隐藏/捕获启动共用；Invoke-SmokeReg 超时 Kill 语义）。
unsafe fn wait_reap(h: HANDLE, timeout_ms: u32) -> SpawnResult {
    match WaitForSingleObject(h, timeout_ms).0 {
        v if v == WAIT_OBJECT_0.0 => {
            let mut code: u32 = 0;
            if GetExitCodeProcess(h, &mut code).is_ok() {
                SpawnResult::Exit(code as i32)
            } else {
                SpawnResult::Fail
            }
        }
        v if v == WAIT_TIMEOUT.0 => {
            // 超时击杀 + 100ms 收尸（Stop-Process -Force 后不留僵尸句柄）
            let _ = TerminateProcess(h, 1);
            let _ = WaitForSingleObject(h, 100);
            SpawnResult::Timeout
        }
        _ => SpawnResult::Fail, // WAIT_FAILED 等
    }
}

/// CreateProcessW(CREATE_NO_WINDOW) + 等待；redir=Some 时子进程
/// stdout/stderr 都指向该可继承句柄（smoke enable 契约：输出落文件判据）。
unsafe fn spawn_with_redir(
    exe: &Path,
    args: &str,
    timeout_ms: u32,
    redir: Option<HANDLE>,
) -> SpawnResult {
    let mut si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut inherit = false;
    if let Some(h) = redir {
        si.dwFlags = STARTF_USESTDHANDLES;
        si.hStdOutput = h;
        si.hStdError = h;
        // stdin 不重定向（PS -RedirectStandardOutput 只管输出侧）
        inherit = true;
    }
    let exe_w = wide_path(exe);
    let mut cmd_w = build_cmdline(exe, args);
    let mut pi = PROCESS_INFORMATION::default();
    let ok = CreateProcessW(
        PCWSTR::from_raw(exe_w.as_ptr()),    // lpApplicationName：精确目标
        PWSTR::from_raw(cmd_w.as_mut_ptr()), // lpCommandLine：子进程 argv（0.58 要求可变 PWSTR）
        None,
        None,
        inherit,          // 重定向时必须 true（句柄继承）
        CREATE_NO_WINDOW, // -WindowStyle Hidden 等价
        None,
        PCWSTR::null(), // 工作目录继承父进程
        &si,
        &mut pi,
    )
    .is_ok();
    if !ok {
        return SpawnResult::Fail;
    }
    let _ = CloseHandle(pi.hThread);
    let r = wait_reap(pi.hProcess, timeout_ms);
    let _ = CloseHandle(pi.hProcess);
    r
}

// ── 公开接口（CONTRACTS.md procop.rs 签名，逐字一致）──

/// Get-Process name | Stop-Process -Force：快照匹配（szExeFile 不区分大小写
/// 全等）逐个 TerminateProcess，返回击杀数（OpenProcess/Terminate 失败不计）。
pub fn kill_by_name(name: &str) -> usize {
    let mut pids: Vec<u32> = Vec::new();
    with_procs(|e| {
        if entry_name(e).eq_ignore_ascii_case(name) {
            pids.push(e.th32ProcessID);
        }
        false
    });
    let mut killed = 0usize;
    for pid in pids {
        unsafe {
            if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                if TerminateProcess(h, 1).is_ok() {
                    let _ = WaitForSingleObject(h, 100); // 收尸
                    killed += 1;
                }
                let _ = CloseHandle(h);
            }
        }
    }
    killed
}

/// Get-Process name -ErrorAction SilentlyContinue → $null 判定。
pub fn proc_running(name: &str) -> bool {
    let mut hit = false;
    with_procs(|e| {
        if entry_name(e).eq_ignore_ascii_case(name) {
            hit = true;
            true // 提前收工
        } else {
            false
        }
    });
    hit
}

/// Start-Process -WindowStyle Hidden -PassThru + WaitForExit(timeout) + 超时 Kill。
pub fn spawn_wait_hidden(exe: &Path, args: &str, timeout_ms: u32) -> SpawnResult {
    unsafe { spawn_with_redir(exe, args, timeout_ms, None) }
}

/// 同 spawn_wait_hidden，但子进程 stdout/stderr 重定向到 out_file
/// （install.ps1 L437-444 smoke enable 输出文件判据：CREATE_ALWAYS 覆盖，
/// 父进程创建→子继承→父立刻 CloseHandle，不留锁文件的活句柄）。
pub fn spawn_capture(exe: &Path, args: &str, timeout_ms: u32, out_file: &Path) -> SpawnResult {
    unsafe {
        let out_w = wide_path(out_file);
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: true.into(), // 0.58 为 BOOL；true=可继承
        };
        let fh = match CreateFileW(
            PCWSTR::from_raw(out_w.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_READ, // 事后父进程可并发读（PS Get-Content 语义）
            Some(&sa),
            CREATE_ALWAYS, // 每次覆盖（$tmpOut 复用）
            FILE_ATTRIBUTE_NORMAL,
            None,
        ) {
            Ok(h) => h,
            Err(_) => return SpawnResult::Fail,
        };
        let r = spawn_with_redir(exe, args, timeout_ms, Some(fh));
        let _ = CloseHandle(fh); // 子进程已拿到继承副本，父侧立刻放
        r
    }
}

/// Start-Process -Verb RunAs -Wait：ShellExecuteExW("runas") + 等待 + 取退出码。
/// UAC 弹窗必须可见（不带 SEE_MASK_FLAG_NO_UI）；用户点"否"=API 直接失败=Fail。
pub fn spawn_runas_wait(exe: &Path, args: &str, timeout_ms: u32) -> SpawnResult {
    unsafe {
        let verb = wide("runas");
        let file = wide_path(exe);
        let params = if args.is_empty() {
            Vec::new()
        } else {
            wide(args)
        };
        let mut sei = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            // NOCLOSEPROCESS=拿 hProcess；NOASYNC=调用线程（可能 STA/UI）下
            // 强制同步完成；绝不加 NO_UI——UAC 确认框必须让用户看见。
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: PCWSTR::from_raw(verb.as_ptr()),
            lpFile: PCWSTR::from_raw(file.as_ptr()),
            lpParameters: if args.is_empty() {
                PCWSTR::null()
            } else {
                PCWSTR::from_raw(params.as_ptr())
            },
            nShow: 1, // SW_SHOWNORMAL 字面量（免引 WindowsAndMessaging feature）
            ..Default::default()
        };
        if ShellExecuteExW(&mut sei).is_err() {
            return SpawnResult::Fail; // 含 UAC 被拒（用户点"否"）
        }
        if sei.hProcess.is_invalid() {
            return SpawnResult::Fail; // NOCLOSEPROCESS 已置仍无句柄=防御分支
        }
        let r = match WaitForSingleObject(sei.hProcess, timeout_ms).0 {
            v if v == WAIT_OBJECT_0.0 => {
                let mut code: u32 = 0;
                if GetExitCodeProcess(sei.hProcess, &mut code).is_ok() {
                    SpawnResult::Exit(code as i32)
                } else {
                    SpawnResult::Fail
                }
            }
            v if v == WAIT_TIMEOUT.0 => SpawnResult::Timeout, // 不杀：非提权父进程无权 TERMINATE 提权子进程
            _ => SpawnResult::Fail,
        };
        let _ = CloseHandle(sei.hProcess);
        r
    }
}

/// Start-Process（无等待，火忘）：ctfmon / explorer 中转降权 / server。
/// CREATE_NO_WINDOW（不用 DETACHED_PROCESS——ctfmon/explorer 要正常桌面进程，
/// 该旗标对 GUI 进程本就无影响）；不等待，两句柄立即 Close。
/// explorer 传参空格路径需内层引号，本函数不加引号、arg 原样作命令行尾部：
/// spawn_detached("explorer.exe", "\"D:\\...\\hufu-server.exe\"")。
pub fn spawn_detached(exe: &str, arg: &str) {
    unsafe {
        let exe_w = wide(exe);
        let mut cmd: Vec<u16> = Vec::new();
        // exe 仅在含空格且未带引号时补引号（全路径 server 场景）；arg 原样
        if exe.contains(' ') && !exe.starts_with('"') {
            cmd.push(b'"' as u16);
            cmd.extend(exe.encode_utf16());
            cmd.push(b'"' as u16);
        } else {
            cmd.extend(exe.encode_utf16());
        }
        if !arg.is_empty() {
            cmd.push(b' ' as u16);
            cmd.extend(arg.encode_utf16());
        }
        cmd.push(0);
        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        let ok = CreateProcessW(
            PCWSTR::from_raw(exe_w.as_ptr()),
            PWSTR::from_raw(cmd.as_mut_ptr()),
            None,
            None,
            false, // 无重定向，不需要句柄继承
            CREATE_NO_WINDOW,
            None,
            PCWSTR::null(),
            &si,
            &mut pi,
        )
        .is_ok();
        if ok {
            let _ = CloseHandle(pi.hThread); // 不等待：火忘，句柄即刻放掉
            let _ = CloseHandle(pi.hProcess);
        }
    }
}
