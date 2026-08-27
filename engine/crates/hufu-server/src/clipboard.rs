//! 剪贴板读取（cfg(windows)，纯 FFI 无依赖）。

// win32 FFI
#[link(name = "user32")]
extern "system" {
    fn OpenClipboard(hwnd: isize) -> i32;
    fn CloseClipboard() -> i32;
    fn GetClipboardData(format: u32) -> isize;
}
#[link(name = "kernel32")]
extern "system" {
    fn GlobalLock(hmem: isize) -> *mut u16;
    fn GlobalUnlock(hmem: isize) -> i32;
}

const CF_UNICODETEXT: u32 = 13;

/// 读剪贴板文本（UTF-16 → String）。失败/空返回空串。
pub fn read_text() -> String {
    unsafe {
        if OpenClipboard(0) == 0 {
            return String::new();
        }
        let mut out = String::new();
        let h = GetClipboardData(CF_UNICODETEXT);
        if h != 0 {
            let p = GlobalLock(h);
            if !p.is_null() {
                let mut len = 0usize;
                while *p.add(len) != 0 && len < 1_000_000 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(p, len);
                out = String::from_utf16_lossy(slice);
                let _ = GlobalUnlock(h);
            }
        }
        let _ = CloseClipboard();
        // 截断超长（防御）
        if out.chars().count() > 4096 {
            out.chars().take(4096).collect()
        } else {
            out
        }
    }
}
