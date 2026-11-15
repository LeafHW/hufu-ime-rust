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
use windows::Win32::Graphics::Gdi::{
    CreateDIBitmap, GetDC, ReleaseDC, BITMAPINFO, CBM_INIT, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::System::Ole::{CF_BITMAP, CF_DIB, CF_UNICODETEXT};

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

/// 【长截图 2026-11-15】把多张竖向切片（32bpp BI_RGB，行序 top-down 或 bottom-up
/// 都认）拼成一张长图，输出 top-down（负 biHeight）；宽度必须一致。
pub fn stitch_dibs_vertical(parts: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let parse = |dib: &Vec<u8>| -> Result<(usize, i32, usize, u32), String> {
        if dib.len() < 40 {
            return Err("切片头长度不足".into());
        }
        let w = i32::from_le_bytes(dib[4..8].try_into().unwrap());
        let h = i32::from_le_bytes(dib[8..12].try_into().unwrap());
        let bpp = u16::from_le_bytes(dib[14..16].try_into().unwrap());
        let rows = h.unsigned_abs() as usize;
        let stride = (w as usize) * (bpp as usize / 8);
        let need = 40 + stride * rows;
        if bpp != 32 || w <= 0 || rows == 0 || dib.len() < need {
            return Err("切片格式不是 32 位位图".into());
        }
        Ok((w as usize, h, rows, stride as u32))
    };
    let (width, _, _, stride) = parse(&parts[0])?;
    let mut rows: Vec<&[u8]> = Vec::new();
    for part in parts {
        let (w, h, n, st) = parse(part)?;
        if w != width || st != stride {
            return Err("切片宽度不一致".into());
        }
        for i in 0..n {
            let src = if h < 0 { i } else { n - 1 - i };
            let start = 40 + src as usize * st as usize;
            rows.push(&part[start..start + st as usize]);
        }
    }
    let total = rows.len();
    let mut out = vec![0u8; 40 + stride as usize * total];
    out[..40].copy_from_slice(&parts[0][..40]);
    let height = -(total as i32);
    out[8..12].copy_from_slice(&height.to_le_bytes());
    let size_image = (stride as usize * total) as u32;
    out[20..24].copy_from_slice(&size_image.to_le_bytes());
    for (i, row) in rows.iter().enumerate() {
        let at = 40 + i * stride as usize;
        out[at..at + stride as usize].copy_from_slice(row);
    }
    Ok(out)
}

/// 【截图复制 2026-11-15】把 32 位 BI_RGB DIB 转成 24 位再进剪贴板。
/// QQ / 微信 等接收端对「32 位 + alpha」的 CF_DIB 处理不一致（常见现象是粘贴
/// 出黑块、透明，或干脆不认），24 位无 alpha 是最保险的交换格式。
pub(crate) fn dib_to_24(dib: &[u8]) -> Vec<u8> {
    if dib.len() < 40 {
        return dib.to_vec();
    }
    let bpp = u16::from_le_bytes([dib[14], dib[15]]);
    let compression = u32::from_le_bytes([dib[16], dib[17], dib[18], dib[19]]);
    if bpp != 32 || compression != 0 {
        return dib.to_vec();
    }
    let width = i32::from_le_bytes([dib[4], dib[5], dib[6], dib[7]]);
    let height = i32::from_le_bytes([dib[8], dib[9], dib[10], dib[11]]);
    let (w, h) = (width.unsigned_abs() as usize, height.unsigned_abs() as usize);
    if w == 0 || h == 0 {
        return dib.to_vec();
    }
    let src_stride = w * 4;
    let dst_stride = (w * 3 + 3) & !3;
    if dib.len() < 40 + src_stride * h {
        return dib.to_vec();
    }
    let mut out = vec![0u8; 40 + dst_stride * h];
    out[..40].copy_from_slice(&dib[..40]);
    out[14..16].copy_from_slice(&24u16.to_le_bytes());
    out[20..24].copy_from_slice(&((dst_stride * h) as u32).to_le_bytes());
    // 【截图复制 2026-11-15】CF_DIB 改成自底向上（正 biHeight）：
    // QQ 桌面版对内联 GDI 路径的 top-down（负 biHeight）DIB 不认，
    // 粘贴时直接没有图；浏览器（如 DSH 输入框）能读，所以只在这类客户端翻车。
    let top_down = i32::from_le_bytes([dib[8], dib[9], dib[10], dib[11]]) < 0;
    out[8..12].copy_from_slice(&(h as i32).to_le_bytes());
    for row in 0..h {
        let src_row = if top_down { h - 1 - row } else { row };
        let src = 40 + src_row * src_stride;
        let dst = 40 + row * dst_stride;
        for col in 0..w {
            out[dst + col * 3] = dib[src + col * 4];
            out[dst + col * 3 + 1] = dib[src + col * 4 + 1];
            out[dst + col * 3 + 2] = dib[src + col * 4 + 2];
        }
    }
    out
}

/// 【截图复制 2026-11-15】再从同一份像素做一个 HBITMAP，供只认 CF_BITMAP 的
/// 接收端（QQ 桌面版就是这种）粘贴。失败不影响 CF_DIB。
fn dib_to_bitmap(dib: &[u8]) -> Option<HBITMAP> {
    if dib.len() < 44 {
        return None;
    }
    unsafe {
        let header = std::ptr::read_unaligned(dib.as_ptr() as *const windows::Win32::Graphics::Gdi::BITMAPINFOHEADER);
        let bmi = BITMAPINFO {
            bmiHeader: header,
            bmiColors: [Default::default()],
        };
        let hdc = GetDC(None);
        if hdc.is_invalid() {
            return None;
        }
        let bits = dib.as_ptr().add(40) as *const core::ffi::c_void;
        let bitmap = CreateDIBitmap(hdc, Some(&bmi.bmiHeader), CBM_INIT as u32, Some(bits), Some(&bmi), DIB_RGB_COLORS);
        let _ = ReleaseDC(None, hdc);
        if bitmap.is_invalid() {
            None
        } else {
            Some(bitmap)
        }
    }
}

/// Explicit image copy: keep the DIB until another clipboard write, without the
/// temporary text bridge's delayed restoration. The caller supplies an owned
/// BITMAPINFOHEADER + pixels buffer; Windows takes the HGLOBAL only on success.
/// 【长截图 2026-11-15】把多张切片拼成一张长图后写剪贴板（内部先拼、再统一降 24 位）。
pub fn write_long_dib(owner: isize, parts: &[Vec<u8>]) -> Result<(), String> {
    let long = stitch_dibs_vertical(parts)?;
    try_write_dib(owner, &long)
}

pub(crate) fn try_write_dib(owner: isize, dib: &[u8]) -> Result<(), String> {
    if owner == 0 || dib.len() < 40 || dib.len() > u32::MAX as usize {
        return Err("复制图像的数据或窗口无效".into());
    }
    // 【截图复制】统一转 24 位无 alpha，兼容 QQ / 微信 的粘贴。
    let dib = &dib_to_24(dib);
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
            if copied {
                // 【截图复制 2026-11-15】同时挂一份 CF_BITMAP：QQ 桌面版粘贴时
                // 会先拿位图，只有 CF_DIB 时表现为「按了粘贴没反应」。
                if let Some(bitmap) = dib_to_bitmap(dib) {
                    let _ = SetClipboardData(CF_BITMAP.0 as u32, HANDLE(bitmap.0));
                }
            }
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
    use super::*;

    /// 【长截图 2026-11-15】竖向切片按顺序拼接，行序混用 top-down/bottom-up 也要读对。
    #[test]
    fn long_shots_stack_vertical_slices_in_order() {
        let make = |rgb: [u8; 4], h: i32| {
            let mut dib = vec![0u8; 40 + 4];
            dib[4..8].copy_from_slice(&1i32.to_le_bytes());
            dib[8..12].copy_from_slice(&h.to_le_bytes());
            dib[14..16].copy_from_slice(&32u16.to_le_bytes());
            dib[40..44].copy_from_slice(&rgb);
            dib
        };
        let top = make([255, 0, 0, 255], -1);
        let bottom_up = make([0, 0, 255, 255], 1);
        let stitched = stitch_dibs_vertical(&[top, bottom_up]).unwrap();
        let h = i32::from_le_bytes(stitched[8..12].try_into().unwrap());
        assert_eq!(h, -2, "拼好后必须是 2 行 top-down");
        assert_eq!(&stitched[40..44], &[255, 0, 0, 255]);
        assert_eq!(&stitched[44..48], &[0, 0, 255, 255]);
    }

    /// 【截图复制】32 位 DIB 必须转成 24 位：字节数按 4 字节对齐、头字段同步，
    /// 这是 QQ / 微信 能正常粘贴的关键。
    #[test]
    fn dib_is_downgraded_to_24_bits_for_chat_apps() {
        const W: usize = 3;
        const H: usize = 2;
        let mut dib = vec![0u8; 40 + W * H * 4];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&(W as i32).to_le_bytes());
        dib[8..12].copy_from_slice(&(-(H as i32)).to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[20..24].copy_from_slice(&((W * H * 4) as u32).to_le_bytes());
        for i in 0..W * H {
            dib[40 + i * 4] = 0x11;
            dib[40 + i * 4 + 1] = 0x22;
            dib[40 + i * 4 + 2] = 0x33;
            dib[40 + i * 4 + 3] = 0xff;
        }
        let out = super::dib_to_24(&dib);
        let stride = (W * 3 + 3) & !3;
        assert_eq!(out.len(), 40 + stride * H);
        assert_eq!(u16::from_le_bytes([out[14], out[15]]), 24);
        assert_eq!(
            u32::from_le_bytes([out[20], out[21], out[22], out[23]]) as usize,
            stride * H
        );
        assert_eq!(&out[40..43], &[0x11, 0x22, 0x33]);
        assert_eq!(&out[40 + stride..40 + stride + 3], &[0x11, 0x22, 0x33]);
    }

    fn utf16_roundtrip_data() {
        // 纯数据检查（不碰系统剪贴板，测试环境无桌面会话也可跑）
        let s = "-----第1段-----\n跟打测试";
        let ws: Vec<u16> = s.encode_utf16().collect();
        assert_eq!(String::from_utf16_lossy(&ws), s);
    }

    /// 真剪贴板回归：写入的 CF_DIB 必须能被系统读回。
    /// 显式 `cargo test -p gale-win -- --ignored dib_lands_on_clipboard` 运行。
    #[test]
    #[ignore]
    fn dib_lands_on_clipboard() {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::DataExchange::*;
        use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
        use windows::Win32::System::Ole::CF_DIB;
        use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;

        const W: u32 = 2;
        const H: u32 = 2;
        let mut dib = vec![0u8; 40 + (W * H * 4) as usize];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&(W as i32).to_le_bytes());
        dib[8..12].copy_from_slice(&(-(H as i32)).to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[20..24].copy_from_slice(&(W * H * 4).to_le_bytes());
        // 4 个像素：红、绿、蓝、白（BGRA，top-down 顺序无关紧要）。
        dib[40..44].copy_from_slice(&[0, 0, 255, 255]);
        dib[44..48].copy_from_slice(&[0, 255, 0, 255]);
        dib[48..52].copy_from_slice(&[255, 0, 0, 255]);
        dib[52..56].copy_from_slice(&[255, 255, 255, 255]);

        let owner = unsafe { GetDesktopWindow() };
        super::try_write_dib(owner.0 as isize, &dib).expect("CF_DIB write should succeed");

        unsafe {
            let opened = OpenClipboard(owner).is_ok();
            assert!(opened, "OpenClipboard should succeed after our own write");
            assert!(IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok(), "CF_DIB missing");
            // 【截图复制 2026-11-15】QQ 桌面版只认 CF_BITMAP，必须同时挂上。
            assert!(
                IsClipboardFormatAvailable(windows::Win32::System::Ole::CF_BITMAP.0 as u32).is_ok(),
                "CF_BITMAP missing"
            );
            let h = GetClipboardData(CF_DIB.0 as u32).expect("CF_DIB data handle");
            let p = GlobalLock(windows::Win32::Foundation::HGLOBAL(h.0));
            assert!(!p.is_null());
            let bytes = std::slice::from_raw_parts(p as *const u8, dib.len());
            // 【截图复制 2026-11-15】落盘的是 24 位：头部如实、像素 BGRA→BGR。
            // 【截图复制 2026-11-15】必须自底向上（正 biHeight），QQ 不认 top-down。
            let height = i32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
            assert!(height > 0, "biHeight must be positive, got {height}");
            assert_eq!(&bytes[0..4], &40u32.to_le_bytes());
            assert_eq!(u16::from_le_bytes([bytes[14], bytes[15]]), 24);
            assert_eq!(&bytes[40..43], &[255, 0, 0], "bottom-up 后首行应是原图末行");
            let _ = GlobalUnlock(windows::Win32::Foundation::HGLOBAL(h.0));
            let _ = CloseClipboard();
        }
    }
}
