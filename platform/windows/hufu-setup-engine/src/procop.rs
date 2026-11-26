//! 进程/启动基础层：Toolhelp 枚举杀查、CreateProcessW 隐藏/捕获/分离启动、
//! ShellExecuteExW 提权等待启动。
//! 血律对照：install.ps1 Start-Process/Stop-Process/Invoke-SmokeReg 的进程语义。
//!
//! - Get-Process name → Toolhelp 快照 PROCESSENTRY32W.szExeFile 全名不区分大小写
//!   比较（PS 按进程名匹配，调用方传全名如 "hufu-server.exe"）。
//! - Stop-Process -Force → OpenProcess(PROCESS_TERMINATE)+TerminateProcess(1)，
//!   与 PS 同款无权限兜底（杀不动的项不计入击杀数）。
//! - Start-Process -WindowStyle Hidden -PassThru + WaitForExit(ms)（超时 Kill，
//!   新维护契约：超时杀掉并确认退出，调用者必须按失败处理）→ spawn_wait_hidden /
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
unsafe fn confirm_exit(h: HANDLE) {
    // No rollback is allowed while a registration child may still be alive.
    // On a wait error retain the handle and retry instead of reporting a false exit.
    while WaitForSingleObject(h, 1000).0 != WAIT_OBJECT_0.0 {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

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
            // 超时击杀并等待确认退出，再允许调用者回退注册。
            let _ = TerminateProcess(h, 1);
            // Never return while the helper can still mutate registration.
            confirm_exit(h);
            SpawnResult::Timeout
        }
        _ => {
            let _ = TerminateProcess(h, 1);
            confirm_exit(h);
            SpawnResult::Fail
        } // WAIT_FAILED: do not expose a still-running child to rollback
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

/// Capture a maintenance helper's diagnostics without a predictable log file
/// that an elevated process could overwrite. The inherited file is exclusive,
/// create-new, and deleted automatically when its final handle closes.
pub fn spawn_diagnostic(exe: &Path, args: &str, timeout_ms: u32) -> (SpawnResult, String) {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use std::sync::atomic::{AtomicU64, Ordering};
    use windows::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "hufu-helper-{}-{}-{}.log",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = match std::fs::OpenOptions::new().read(true).write(true)
        .create_new(true).share_mode(0).custom_flags(0x04000000) // FILE_FLAG_DELETE_ON_CLOSE
        .open(&path) {
        Ok(file) => file,
        Err(error) => return (SpawnResult::Fail, format!("create diagnostic capture: {error}")),
    };
    let handle = HANDLE(file.as_raw_handle());
    let result = unsafe {
        if let Err(error) = SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT) {
            return (SpawnResult::Fail, format!("inherit diagnostic capture: {error}"));
        }
        spawn_with_redir(exe, args, timeout_ms, Some(handle))
    };
    let mut bytes = Vec::new();
    if let Err(error) = file.seek(SeekFrom::Start(0)).and_then(|_| (&mut file).take(65537).read_to_end(&mut bytes)) {
        return (SpawnResult::Fail, format!("{result:?}; read diagnostic capture: {error}"));
    }
    let truncated = bytes.len() > 65536;
    bytes.truncate(65536);
    let mut output = String::from_utf8_lossy(&bytes).into_owned();
    if truncated { output.push_str("\n[helper output truncated at 64 KiB]"); }
    (result, output)
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
            // SW_HIDE(0)：引擎已改 GUI 子系统（2026-11-05 重启闪终端修复），
            // 此值防御性保留 0——未来若以 console 工具替换引擎，SHOWNORMAL
            // 会让提权子进程顶出可见控制台。UAC 弹窗在 secure desktop 由
            // consent.exe 负责，与 nShow 无关。提权段输出走 --log/默认
            // elevated 日志文件，隐藏不丢任何东西。
            nShow: 0,
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
            v if v == WAIT_TIMEOUT.0 => {
                // The unelevated parent cannot terminate an elevated helper.
                // Wait for its definitive exit before the caller attempts rollback.
                confirm_exit(sei.hProcess);
                SpawnResult::Timeout
            }
            _ => {
                confirm_exit(sei.hProcess);
                SpawnResult::Fail
            }
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
    // 【B6 复审加固 2026-11-05】裸名（explorer.exe/ctfmon.exe）作
    // lpApplicationName 时 CreateProcessW 按当前目录补全而非走搜索路径
    // （MSDN 语义）——安装目录若恰有同名文件会执行错文件。统一解析为
    // %SystemRoot% 绝对路径：先 System32（ctfmon；WOW64 重定向到
    // SysWOW64 亦有副本，安全），不存在再退 C:\Windows 根（explorer），
    // 都没有则维持原名（CreateProcessW 自带有限搜索兜底）。
    let resolved;
    let exe = if !exe.contains('\\') {
        resolved = std::env::var("SystemRoot")
            .ok()
            .and_then(|w| {
                let sys32 = format!("{w}\\System32\\{exe}");
                if std::path::Path::new(&sys32).is_file() {
                    Some(sys32)
                } else {
                    let root = format!("{w}\\{exe}");
                    std::path::Path::new(&root).is_file().then_some(root)
                }
            })
            .unwrap_or_else(|| exe.to_string());
        &resolved
    } else {
        exe
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "spawned only by diagnostic_capture_preserves_both_streams_and_exit_code"]
    fn diagnostic_child() {
        println!("registration stdout marker");
        eprintln!("registration stderr hresult=0x80004005");
        std::process::exit(7);
    }
    #[test]
    fn diagnostic_capture_preserves_both_streams_and_exit_code() {
        let (result, output) = spawn_diagnostic(
            &std::env::current_exe().unwrap(),
            "--exact procop::tests::diagnostic_child --ignored --nocapture", 5000);
        assert_eq!(result, SpawnResult::Exit(7));
        assert!(output.contains("registration stdout marker"));
        assert!(output.contains("registration stderr hresult=0x80004005"));
    }
    #[test]
    #[ignore = "spawned only by timeout_confirms_child_exit"]
    fn timeout_child() {
        println!("child_pid={}", std::process::id());
        std::thread::sleep(std::time::Duration::from_secs(30));
    }
    #[test]
    fn timeout_confirms_child_exit() {
        let (result, text) = spawn_diagnostic(
            &std::env::current_exe().unwrap(),
            "--exact procop::tests::timeout_child --ignored --nocapture",
            2000,
        );
        assert_eq!(result, SpawnResult::Timeout);
        let pid = text
            .lines()
            .find_map(|s| s.strip_prefix("child_pid="))
            .expect("child actually started")
            .trim()
            .parse::<u32>()
            .unwrap();
        unsafe {
            use windows::Win32::System::Threading::{
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            };
            if let Ok(handle) = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                pid,
            ) {
                assert_eq!(
                    WaitForSingleObject(handle, 0).0,
                    WAIT_OBJECT_0.0,
                    "registration child still running after timeout return"
                );
                let _ = CloseHandle(handle);
            }
        }
    }
}
