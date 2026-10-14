//! 公共件：常量、路径推导、输出三通（stdout+引擎日志+install.log）、权限判定。
//! 血律对照：install.ps1 / uninstall.ps1 的常量与文案原样搬运。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CLSID: &str = "{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}";
pub const PROFILE: &str = "{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}";
pub const TFCAT_KBD: &str = "{533C5E0E-5AC0-4ABD-B6F1-251B82B7BE7D}";
/// HKLM TIP 分类 8 项（install.ps1 L182-191）
pub const TIP_CATS: [&str; 8] = [
    "{046B8C80-1647-40F7-9B21-B93B81AABC1B}",
    "{13A016DF-560B-46CD-947A-4C3AF1E0E35D}",
    "{25504FB4-7BAB-4BC1-9C69-CF81890F0EF5}",
    "{34745C63-B2F0-4784-8B67-5E12C8701A31}",
    "{364215D9-75BC-11D7-A6EF-00065B84435C}",
    "{49D2F9CE-1F5E-11D7-A6D3-00065B84435C}",
    "{49D2F9CF-1F5E-11D7-A6D3-00065B84435C}",
    "{CCF05DD7-4A87-11D7-A6E2-00065B84435C}",
];
/// 切换器装配表基键（install.ps1 L403）
pub const ASM_KBD_CAT: &str = "{34745C63-B2F0-4784-8B67-5E12C8701A31}";

pub fn tip_str() -> String {
    format!("0804:{}{}", CLSID, PROFILE)
}

/// 安装目录 = 本 exe 所在目录（原地安装语义，install.ps1 L20-23）
pub fn install_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn sysdir_x64() -> PathBuf {
    PathBuf::from(r"C:\Windows\SystemIME\HuFu")
}
pub fn sysdir_x86() -> PathBuf {
    PathBuf::from(r"C:\Windows\SysWOW64\SystemIME\HuFu")
}
pub fn sysdll_x64() -> PathBuf {
    sysdir_x64().join("hufu_tsf.dll")
}
pub fn sysdll_x86() -> PathBuf {
    sysdir_x86().join("hufu_tsf32.dll")
}

pub fn temp_dir() -> PathBuf {
    std::env::var_os("TEMP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows\Temp"))
}
pub fn localappdata() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_default()
}
pub fn programdata() -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
}
pub fn elevated_log() -> PathBuf {
    temp_dir().join("hufu-install-elevated.log")
}

/// 本地时间戳，与 PS Get-Date -Format 'yyyy-MM-dd HH:mm:ss' 对齐
pub fn now_stamp() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    unsafe {
        let st = GetLocalTime();
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
        )
    }
}

/// 输出三通：进度行 → stdout(UTF-8)+引擎日志；install.log 行 → 安装目录 install.log。
pub struct Out {
    pub logfile: Option<PathBuf>,
    install_log_path: PathBuf,
    buf: std::io::BufWriter<std::io::Stdout>,
}

impl Out {
    pub fn new(logfile: Option<PathBuf>) -> Self {
        let ilp = install_dir().join("install.log");
        Out {
            logfile,
            install_log_path: ilp,
            buf: std::io::BufWriter::new(std::io::stdout()),
        }
    }
    /// 进度行（stdout + 引擎日志）
    pub fn line(&mut self, s: &str) {
        let _ = self.buf.write_all(s.as_bytes());
        let _ = self.buf.write_all(b"\r\n");
        let _ = self.buf.flush();
        self.append_engine_log(s);
    }
    /// 只进 stdout（不落任何日志文件）
    pub fn host_only(&mut self, s: &str) {
        let _ = self.buf.write_all(s.as_bytes());
        let _ = self.buf.write_all(b"\r\n");
        let _ = self.buf.flush();
    }
    /// install.log 追加（自动加 `[时间] ` 前缀，与 PS 版逐字节对齐）
    pub fn install_log(&mut self, s: &str) {
        let line = format!("[{}] {}", now_stamp(), s);
        let p = self.install_log_path.clone();
        Self::append_file(&p, &line);
    }
    fn append_engine_log(&mut self, s: &str) {
        if let Some(p) = self.logfile.clone() {
            Self::append_file(&p, s);
        }
    }
    fn append_file(p: &Path, s: &str) {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(p) {
            let _ = f.write_all(s.as_bytes());
            let _ = f.write_all(b"\r\n");
        }
    }
}

/// 当前令牌是否已提权（install.ps1 L230-231 语义）
pub fn is_admin() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut tok = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).is_err() {
            return false;
        }
        let mut elev = TOKEN_ELEVATION::default();
        let mut ret = 0u32;
        let ok = GetTokenInformation(
            tok,
            TokenElevation,
            Some(&mut elev as *mut _ as *mut core::ffi::c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret,
        )
        .is_ok();
        let _ = CloseHandle(tok);
        ok && elev.TokenIsElevated != 0
    }
}

/// 令牌所属组里是否有管理员组（含 deny-only，install.ps1 L232-235 whoami /groups 语义）：
/// 未提权的管理员账户=true（可弹 UAC），标准用户=false。
pub fn in_admin_group() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::{GetTokenInformation, TokenGroups, TOKEN_GROUPS, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut tok = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).is_err() {
            return false;
        }
        let mut need = 0u32;
        let _ = GetTokenInformation(tok, TokenGroups, None, 0, &mut need);
        if need == 0 {
            let _ = CloseHandle(tok);
            return false;
        }
        let mut buf = vec![0u8; need as usize];
        let ok = GetTokenInformation(
            tok,
            TokenGroups,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            need,
            &mut need,
        )
        .is_ok();
        let _ = CloseHandle(tok);
        if !ok {
            return false;
        }
        let tg = &*(buf.as_ptr() as *const TOKEN_GROUPS);
        // S-1-5-32-544 逐 RID 比对（deny-only 也在组列表里，whoami /groups 语义）。
        // Groups 在 windows-rs 声明为定长 1 的柔性数组尾——必须 as_ptr().add(i)
        // 越过声明边界访问，直接下标索引会在 GroupCount>1 时越界 panic。
        let groups_ptr: *const windows::Win32::Security::SID_AND_ATTRIBUTES = tg.Groups.as_ptr();
        let mut found = false;
        for i in 0..tg.GroupCount as usize {
            let sid = (*groups_ptr.add(i)).Sid;
            if sid.0.is_null() {
                continue;
            }
            if sid_matches_admin(sid.0) {
                found = true;
                break;
            }
        }
        found
    }
}

/// SID 字节串是否为 S-1-5-32-544（Revision=1, SubAuthorityCount=2,
/// IdentifierAuthority=5, Sub=32, 544）
unsafe fn sid_matches_admin(sid: *mut core::ffi::c_void) -> bool {
    const SID_REVISION: u8 = 1;
    const ADM_SUB0: u32 = 32;
    const ADM_SUB1: u32 = 544;
    let p = sid as *const u8;
    if p.is_null() {
        return false;
    }
    let rev = *p;
    let count = *p.offset(1);
    if rev != SID_REVISION || count != 2 {
        return false;
    }
    // _SID 布局：Revision(1) SubAuthorityCount(1) IdentifierAuthority(6) SubAuthority[Count](4×N)
    let auth = p.offset(2) as *const u8;
    let mut auth_val: u64 = 0;
    for i in 0..6usize {
        auth_val = (auth_val << 8) | *auth.add(i) as u64;
    }
    if auth_val != 5 {
        return false;
    }
    let sub = p.offset(8) as *const u32;
    let s0 = *sub as u32;
    let s1 = *sub.add(1) as u32;
    s0 == ADM_SUB0 && s1 == ADM_SUB1
}
