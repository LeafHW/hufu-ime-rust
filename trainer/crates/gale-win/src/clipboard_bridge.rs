//! Win32 剪贴板桥：UTF-8 文本读写（群载文 / 发成绩通道）。
//!
//! 群文监听线程与 UI 线程都读剪贴板：OpenClipboard(HWND) 独占打开，
//! 失败重试 3 次（别的程序偶尔持有）。

use windows::Win32::Foundation::{GlobalFree, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;

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
        let result = EmptyClipboard().and_then(|_| {
            let handle = windows::Win32::Foundation::HANDLE(h.0);
            SetClipboardData(CF_UNICODETEXT.0 as u32, handle).map(|_| ())
        });
        unsafe {
            let _ = CloseClipboard();
        }
        if result.is_ok() {
            return Ok(());
        } // System owns h after SetClipboardData succeeds.
        let _ = GlobalFree(h);
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
