//! Win32 剪贴板桥：UTF-8 文本读写（群载文 / 发成绩通道）。
//!
//! 群文监听线程与 UI 线程都读剪贴板：OpenClipboard(HWND) 独占打开，
//! 失败重试 3 次（别的程序偶尔持有）。
//!
//! 写入保护（发成绩通道借用户剪贴板过桥，用完要还）：
//! - 写入前在同一打开会话里快照当前 CF_UNICODETEXT；
//! - 写入成功后 30 秒无他人写入则后台还原用户原文（600ms 探针提前放弃）；
//!   期间 GetClipboardSequenceNumber 变化即放弃（用户或别的程序动过）；
//! - EmptyClipboard 成功但 SetClipboardData 失败（此刻剪贴板已被清空）
//!   立即回写快照，不留空窗。

use std::sync::Mutex;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};

/// 延迟恢复状态：写入前的用户文本 + 写入完成时的剪贴板序列号 + 恢复期限。
/// 连续写入只保留**第一次**的快照（那时才是用户内容），期限顺延、序列号刷新。
struct RestoreState {
    /// 含结尾 NUL，可直接 SetClipboardData
    data: Vec<u16>,
    seq_after: u32,
    deadline: std::time::Instant,
}
static RESTORE: Mutex<Option<RestoreState>> = Mutex::new(None);
const RESTORE_DELAY: std::time::Duration = std::time::Duration::from_secs(30);
const RESTORE_PROBE: std::time::Duration = std::time::Duration::from_millis(600);

/// 剪贴板已打开期间锁定拷贝当前 CF_UNICODETEXT（含结尾 NUL）。不拥有句柄。
unsafe fn snapshot_locked() -> Option<Vec<u16>> {
    IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).ok()?;
    let h = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
    let hg = HGLOBAL(h.0);
    let units = GlobalSize(hg) / 2;
    if units == 0 || units > 1 << 20 {
        return None;
    }
    let ptr = GlobalLock(hg);
    if ptr.is_null() {
        return None;
    }
    let data = std::slice::from_raw_parts(ptr as *const u16, units);
    let end = data.iter().position(|c| *c == 0).unwrap_or(units);
    let mut out = data[..end].to_vec();
    out.push(0);
    let _ = GlobalUnlock(hg);
    Some(out)
}

/// 把 UTF-16（含结尾 NUL）写回剪贴板；成功后句柄归系统，失败自释放。
unsafe fn set_text_raw(data: &[u16]) -> bool {
    let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, data.len() * 2) else {
        return false;
    };
    let ptr = GlobalLock(h);
    if ptr.is_null() {
        let _ = GlobalFree(h);
        return false;
    }
    std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u16, data.len());
    let _ = GlobalUnlock(h);
    for _ in 0..3 {
        if OpenClipboard(None).is_err() {
            std::thread::sleep(std::time::Duration::from_millis(15));
            continue;
        }
        let ok = EmptyClipboard().is_ok()
            && SetClipboardData(CF_UNICODETEXT.0 as u32, HANDLE(h.0)).is_ok();
        let _ = CloseClipboard();
        if ok {
            return true; // 系统接管 h
        }
        let _ = GlobalFree(h); // Set 失败，句柄仍归我们；重试无意义
        return false;
    }
    let _ = GlobalFree(h);
    false
}

/// 写入成功后登记延迟恢复（保首次快照、顺延期限），并起看护线程到点还原。
fn register_restore(snap: Option<Vec<u16>>, seq_after: u32) {
    if let Ok(mut slot) = RESTORE.lock() {
        match slot.as_mut() {
            Some(st) => {
                st.seq_after = seq_after;
                st.deadline = std::time::Instant::now() + RESTORE_DELAY;
            }
            None => {
                if let Some(data) = snap {
                    *slot = Some(RestoreState {
                        data,
                        seq_after,
                        deadline: std::time::Instant::now() + RESTORE_DELAY,
                    });
                }
            }
        }
    }
    std::thread::spawn(|| {
        let my_deadline = RESTORE
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|st| st.deadline));
        let Some(my_deadline) = my_deadline else {
            return;
        };
        std::thread::sleep(RESTORE_PROBE);
        // 600ms 探针：期间有人动过剪贴板（含用户自己复制）→ 立即放弃还原
        let data = {
            let Ok(guard) = RESTORE.lock() else { return };
            match guard.as_ref() {
                Some(st) if st.deadline == my_deadline => {
                    if unsafe { GetClipboardSequenceNumber() } != st.seq_after {
                        return; // 保持状态不动：后续写入的看护线程接管
                    }
                    st.data.clone()
                }
                _ => return, // 状态已被后续写入替换
            }
        };
        let now = std::time::Instant::now();
        if my_deadline > now {
            std::thread::sleep(my_deadline - now);
        }
        let Ok(mut guard) = RESTORE.lock() else {
            return;
        };
        match guard.as_ref() {
            Some(st) if st.deadline == my_deadline => {
                if unsafe { GetClipboardSequenceNumber() } != st.seq_after {
                    *guard = None; // 到点时已被他人覆盖，放弃
                    return;
                }
                *guard = None; // 本次还原成为最新内容，取消后续还原
                               // Keep the restoration mutex through the actual write. An
                               // explicit image copy uses the same mutex, so an already-due
                               // restore cannot overwrite that newer copy after it returns.
                unsafe {
                    let _ = set_text_raw(&data);
                };
            }
            _ => {}
        }
    });
}

/// 读剪贴板 UTF-16 → String；无文本/失败返回 None
pub unsafe fn read_text() -> Option<String> {
    for _ in 0..3 {
        if OpenClipboard(None).is_err() {
            std::thread::sleep(std::time::Duration::from_millis(15));
            continue;
        }
        let result = (|| {
            IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).ok()?;
            let h = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            // HGLOBAL 锁定拷贝（不拥有句柄，不能 Free）
            let hg = HGLOBAL(h.0);
            let units = GlobalSize(hg) / 2;
            if units == 0 || units > 1 << 20 {
                return None;
            }
            let ptr = GlobalLock(hg);
            if ptr.is_null() {
                return None;
            }
            let data = std::slice::from_raw_parts(ptr as *const u16, units);
            let out = data
                .iter()
                .position(|c| *c == 0)
                .map(|end| String::from_utf16_lossy(&data[..end]));
            let _ = GlobalUnlock(hg);
            out
        })();
        let _ = CloseClipboard();
        if result.is_some() {
            return result;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    None
}

/// 写剪贴板（String → UTF-16）；失败忽略（调用方只用于发成绩/载文提示）
pub unsafe fn write_text(s: &str) {
    let _ = try_write_text(s);
}

/// Explicit image copy: keep the DIB until another clipboard write, without the
/// temporary text bridge's delayed restoration. The caller supplies an owned
/// BITMAPINFOHEADER + pixels buffer; Windows takes the HGLOBAL only on success.
pub(crate) fn try_write_dib(owner: isize, dib: &[u8]) -> Result<(), String> {
    if owner == 0 || dib.len() < 40 || dib.len() > u32::MAX as usize {
        return Err("复制图像的数据或窗口无效".into());
    }
    let mut restore = RESTORE.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, dib.len())
            .map_err(|_| "无法分配剪贴板图像内存".to_string())?;
        let ptr = GlobalLock(h);
        if ptr.is_null() {
            let _ = GlobalFree(h);
            return Err("无法锁定剪贴板图像内存".into());
        }
        std::ptr::copy_nonoverlapping(dib.as_ptr(), ptr as *mut u8, dib.len());
        let _ = GlobalUnlock(h);
        for _ in 0..3 {
            // A real owner is required: EmptyClipboard after OpenClipboard(NULL)
            // can leave the clipboard owner NULL and make SetClipboardData fail.
            if OpenClipboard(HWND(owner as *mut _)).is_err() {
                std::thread::sleep(std::time::Duration::from_millis(15));
                continue;
            }
            let snap = snapshot_locked();
            let emptied = EmptyClipboard().is_ok();
            let copied = emptied && SetClipboardData(CF_DIB.0 as u32, HANDLE(h.0)).is_ok();
            let _ = CloseClipboard();
            if copied {
                // System owns h. Cancel any earlier temporary text restoration.
                *restore = None;
                return Ok(());
            }
            let _ = GlobalFree(h);
            if emptied {
                if let Some(data) = snap {
                    let _ = set_text_raw(&data);
                }
            }
            return Err("无法写入剪贴板图像，请稍后重试".into());
        }
        let _ = GlobalFree(h);
        Err("剪贴板正被其他程序占用，请稍后重试".into())
    }
}

/// 写剪贴板并确认系统接受了内容；先分配再清空，分配失败不会丢掉原内容。
pub unsafe fn try_write_text(s: &str) -> Result<(), String> {
    if s.contains('\0') || s.len() > 1024 * 1024 {
        return Err("复制内容包含空字符或超过1MB".into());
    }
    let ws: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    let h =
        GlobalAlloc(GMEM_MOVEABLE, ws.len() * 2).map_err(|_| "无法分配剪贴板内存".to_string())?;
    let ptr = GlobalLock(h);
    if ptr.is_null() {
        let _ = GlobalFree(h);
        return Err("无法锁定剪贴板内存".into());
    }
    std::ptr::copy_nonoverlapping(ws.as_ptr(), ptr as *mut u16, ws.len());
    let _ = GlobalUnlock(h);
    for _ in 0..3 {
        if OpenClipboard(None).is_err() {
            std::thread::sleep(std::time::Duration::from_millis(15));
            continue;
        }
        // 同一会话内先快照用户原文（含结尾 NUL），再清空写入
        let snap = unsafe { snapshot_locked() };
        let emptied = unsafe { EmptyClipboard() }.is_ok();
        let set = if emptied {
            let handle = windows::Win32::Foundation::HANDLE(h.0);
            unsafe { SetClipboardData(CF_UNICODETEXT.0 as u32, handle) }.map(|_| ())
        } else {
            Err(windows::core::Error::from_win32())
        };
        let seq_after = unsafe { GetClipboardSequenceNumber() };
        unsafe {
            let _ = CloseClipboard();
        }
        if set.is_ok() {
            register_restore(snap, seq_after);
            return Ok(());
        } // System owns h after SetClipboardData succeeds.
        let _ = GlobalFree(h);
        // EmptyClipboard 成功但 SetClipboardData 失败：剪贴板此刻是空的，
        // 立即回写快照不留空窗，并把延迟恢复排到 30 秒后（同保首策略）。
        if emptied {
            if let Some(data) = snap.clone() {
                if unsafe { set_text_raw(&data) } {
                    let seq_after = unsafe { GetClipboardSequenceNumber() };
                    register_restore(snap, seq_after);
                    return Err("无法写入剪贴板，已还原原内容，请稍后重试".into());
                }
            }
        }
        return Err("无法写入剪贴板，请稍后重试".into());
    }
    let _ = GlobalFree(h);
    Err("剪贴板正被其他程序占用，请稍后重试".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn utf16_roundtrip_data() {
        // 纯数据检查（不碰系统剪贴板，测试环境无桌面会话也可跑）
        let s = "-----第1段-----\n跟打测试";
        let ws: Vec<u16> = s.encode_utf16().collect();
        assert_eq!(String::from_utf16_lossy(&ws), s);
    }
}
