//! PFRO 延迟删除登记：MoveFileExW(src, null, MOVEFILE_DELAY_UNTIL_REBOOT)。
//! 血律对照：hufu-setup-tools.ps1 L418-436 Register-PfroDelete——SystemIME
//! 腾位残留（HuFu.old* 内文件 + HuFu\*.old*）的重启兜底删除。
//! PFRO 只登记文件不删目录，调用方保证只传文件路径；提权上下文调用
//! （PendingFileRenameOperations 在 HKLM 下，非提权写入会失败）。

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};

/// 登记重启删除；目标不存在也返回 true（幂等——tools 侧只对存在的文件
/// 收集登记，重复登记/已删目标不是错误）。
pub fn schedule_delete(path: &Path) -> bool {
    if !path.exists() {
        return true;
    }
    let w: Vec<u16> = {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    unsafe {
        MoveFileExW(
            PCWSTR::from_raw(w.as_ptr()),
            PCWSTR::null(), // 目标 null + DELAY_UNTIL_REBOOT = 重启时删除
            MOVEFILE_DELAY_UNTIL_REBOOT,
        )
        .is_ok()
    }
}
