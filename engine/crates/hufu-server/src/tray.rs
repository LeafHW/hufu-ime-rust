//! Windows 托盘图标（cfg(windows)）。
//!
//! 隐藏消息窗口 + Shell_NotifyIconW：
//! - 双击/左键：打开设置页（默认浏览器）
//! - 右键菜单：打开设置 / 中英切换（管道 op state）/ 退出
//! 退出托盘即退出整个 server 进程（engine 状态已持久化在磁盘）。

#![allow(non_snake_case)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

#[link(name = "shell32")]
extern "system" {
    fn Shell_NotifyIconW(dwMessage: u32, lpData: *const NOTIFYICONDATAW) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateDIBSection(
        hdc: isize, pbmi: *const BITMAPINFO, usage: u32,
        ppvbits: *mut *mut std::ffi::c_void, hsection: isize, offset: u32,
    ) -> isize;
    fn CreateCompatibleDC(hdc: isize) -> isize;
    fn DeleteDC(hdc: isize) -> i32;
    fn DeleteObject(h: isize) -> i32;
}
#[link(name = "user32")]
extern "system" {
    fn CreateIconIndirect(piconinfo: *const ICONINFO) -> isize;
}

#[repr(C)]
struct ICONINFO {
    fIcon: i32,
    xHotspot: u32,
    yHotspot: u32,
    hbmMask: isize,
    hbmColor: isize,
}

#[repr(C)]
struct BITMAPINFOHEADER {
    biSize: u32,
    biWidth: i32,
    biHeight: i32,
    biPlanes: u16,
    biBitCount: u16,
    biCompression: u32,
    biSizeImage: u32,
    biXPelsPerMeter: i32,
    biYPelsPerMeter: i32,
    biClrUsed: u32,
    biClrImportant: u32,
}

#[repr(C)]
struct BITMAPINFO {
    bmiHeader: BITMAPINFOHEADER,
    bmiColors: [u32; 1],
}

/// 简约「虎爪三痕」托盘图标：深色圆角方 + 三道白色斜爪痕（4× 超采样抗锯齿）。
fn make_hu_icon() -> isize {
    const S: usize = 32; // 图标边长
    const SS: usize = 4; // 超采样倍数
    // 爪痕线段（单位像素，32 空间）：右上→左下三道，长度渐短
    let claws: [((f32, f32), (f32, f32)); 3] = [
        ((9.0, 6.5), (17.5, 25.0)),
        ((15.5, 6.0), (22.0, 20.5)),
        ((21.5, 7.5), (26.5, 17.5)),
    ];
    let claw_w = 1.9f32; // 半宽
    let r = 7.5f32; // 圆角半径
    let bg = [0.105, 0.105, 0.118f32]; // #1B1B1E
    let fg = [0.96, 0.96, 0.97f32]; // #F5F5F7
    let mut buf = vec![0u8; S * S * 4]; // 预乘 BGRA
    let mut mask = vec![0u8; S * S * 4]; // AND 位图（32bpp 对齐，全 0=不遮）
    for y in 0..S {
        for x in 0..S {
            // 4×4 超采样
            let mut cov_bg = 0u32;
            let mut cov_fg = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f32 + (sx as f32 + 0.5) / SS as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / SS as f32;
                    // 圆角方形覆盖
                    let dx = (px - 16.0).abs().max(0.0) - (16.0 - r);
                    let dy = (py - 16.0).abs().max(0.0) - (16.0 - r);
                    if dx.max(dy) <= 0.0 {
                        cov_bg += 1;
                        // 爪痕：任一线段距离 ≤ 半宽
                        for ((x0, y0), (x1, y1)) in claws {
                            let vx = x1 - x0;
                            let vy = y1 - y0;
                            let t = (((px - x0) * vx + (py - y0) * vy) / (vx * vx + vy * vy))
                                .clamp(0.0, 1.0);
                            let cx = x0 + t * vx;
                            let cy = y0 + t * vy;
                            let d2 = (px - cx) * (px - cx) + (py - cy) * (py - cy);
                            if d2 <= claw_w * claw_w {
                                cov_fg += 1;
                                break;
                            }
                        }
                    }
                }
            }
            let a_bg = cov_bg as f32 / (SS * SS) as f32;
            let a_fg = cov_fg as f32 / (SS * SS) as f32;
            let a = a_bg;
            if a > 0.0 {
                // 前景按覆盖混合到背景色上，再乘总 alpha（预乘 BGRA）
                let blend = |i: usize| -> u8 {
                    let v = fg[i] * a_fg + bg[i] * (a_bg - a_fg) / a_bg.max(1e-6);
                    (v * a * 255.0) as u8
                };
                let i = (y * S + x) * 4;
                buf[i] = blend(2); // B
                buf[i + 1] = blend(1); // G
                buf[i + 2] = blend(0); // R
                buf[i + 3] = (a * 255.0) as u8; // A
            }
        }
    }
    unsafe {
        let hdc = CreateCompatibleDC(0);
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: S as i32,
                biHeight: -(S as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [0],
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hcolor = CreateDIBSection(hdc, &bi, 0, &mut bits, 0, 0);
        let mut mbits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hmask = CreateDIBSection(hdc, &bi, 0, &mut mbits, 0, 0);
        if hcolor == 0 || hmask == 0 || bits.is_null() || mbits.is_null() {
            if hcolor != 0 { let _ = DeleteObject(hcolor); }
            if hmask != 0 { let _ = DeleteObject(hmask); }
            let _ = DeleteDC(hdc);
            return 0;
        }
        std::ptr::copy_nonoverlapping(buf.as_ptr(), bits as *mut u8, buf.len());
        std::ptr::copy_nonoverlapping(mask.as_ptr(), mbits as *mut u8, mask.len());
        let ii = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hmask,
            hbmColor: hcolor,
        };
        let hicon = CreateIconIndirect(&ii);
        // ICONINFO 文档：位图所有权归系统，不删；DC 可删
        let _ = DeleteDC(hdc);
        hicon
    }
}
#[link(name = "user32")]
extern "system" {
    fn CreateWindowExW(
        dwExStyle: u32, lpClassName: *const u16, lpWindowName: *const u16,
        dwStyle: u32, x: i32, y: i32, nWidth: i32, nHeight: i32,
        hWndParent: isize, hMenu: isize, hInstance: isize, lpParam: isize,
    ) -> isize;
    fn DefWindowProcW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn RegisterClassW(lpwcx: *const WNDCLASSW) -> u16;
    fn CreatePopupMenu() -> isize;
    fn AppendMenuW(hmenu: isize, uflags: u32, idm: usize, text: *const u16) -> i32;
    fn TrackPopupMenu(
        hmenu: isize, uflags: u32, x: i32, y: i32, nres: i32,
        hwnd: isize, prcrect: isize,
    ) -> i32;
    fn GetCursorPos(lppoint: *mut POINT) -> i32;
    fn SetForegroundWindow(hwnd: isize) -> i32;
    fn DestroyMenu(hmenu: isize) -> i32;
    fn PostQuitMessage(exitcode: i32);
    fn LoadImageW(hinst: isize, name: *const u16, typ: u32, cx: i32, cy: i32, load: u32) -> isize;
}
#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(lpmmodulename: *const u16) -> isize;
    fn GetCurrentThreadId() -> u32;
}

#[repr(C)]
struct POINT { x: i32, y: i32 }

#[repr(C)]
struct WNDCLASSW {
    style: u32,
    lpfnWndProc: extern "system" fn(isize, u32, usize, isize) -> isize,
    cbClsExtra: i32,
    cbWndExtra: i32,
    hInstance: isize,
    hIcon: isize,
    hCursor: isize,
    hbrBackground: isize,
    lpszMenuName: *const u16,
    lpszClassName: *const u16,
}

#[repr(C)]
struct NOTIFYICONDATAW {
    cbSize: u32,
    hWnd: isize,
    uID: u32,
    uFlags: u32,
    uCallbackMessage: u32,
    hIcon: isize,
    szTip: [u16; 128],
    dwState: u32,
    dwStateMask: u32,
    szInfo: [u16; 256],
    uVersion: u32,
    szInfoTitle: [u16; 64],
    dwInfoFlags: u32,
    guidItem: [u8; 16],
    hBalloonIcon: isize,
}

const NIM_ADD: u32 = 0x0;
const NIM_MODIFY: u32 = 0x1;
const NIM_DELETE: u32 = 0x2;
const NIF_MESSAGE: u32 = 0x1;
const NIF_TIP: u32 = 0x4;
const WM_APP: u32 = 0x8000;
const WM_DESTROY: u32 = 0x0002;
const WM_COMMAND: u32 = 0x0111;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_LBUTTONUP: u32 = 0x0202;
const TPM_RIGHTBUTTON: u32 = 0x0002;
const TPM_RETURNCMD: u32 = 0x0100;
const MF_STRING: u32 = 0x0;
const MF_SEPARATOR: u32 = 0x800;

const IDM_SETTINGS: usize = 1;
const IDM_QUIT: usize = 3;

static TRAY_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static ASK_QUIT: AtomicBool = AtomicBool::new(false);
/// 打开设置页的信号（主线程 select 循环外执行）
static mut OPEN_SETTINGS: Option<Sender<()>> = None;

extern "system" fn wnd_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    unsafe {
        match msg {
            WM_APP => {
                match (lparam & 0xFFFF) as u32 {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                        if let Some(tx) = OPEN_SETTINGS.as_ref() {
                            let _ = tx.send(());
                        }
                    }
                    WM_RBUTTONUP => {
                        let hmenu = CreatePopupMenu();
                        let s1: Vec<u16> = "打开设置页\u{0}".encode_utf16().collect();
                        let s2: Vec<u16> = "退出 HuFu\u{0}".encode_utf16().collect();
                        AppendMenuW(hmenu, MF_STRING, IDM_SETTINGS, s1.as_ptr());
                        AppendMenuW(hmenu, MF_SEPARATOR, 0, std::ptr::null());
                        AppendMenuW(hmenu, MF_STRING, IDM_QUIT, s2.as_ptr());
                        let mut pt = POINT { x: 0, y: 0 };
                        GetCursorPos(&mut pt);
                        SetForegroundWindow(hwnd);
                        let cmd = TrackPopupMenu(
                            hmenu, TPM_RIGHTBUTTON | TPM_RETURNCMD,
                            pt.x, pt.y, 0, hwnd, 0,
                        );
                        DestroyMenu(hmenu);
                        if cmd == IDM_SETTINGS as i32 {
                            if let Some(tx) = OPEN_SETTINGS.as_ref() {
                                let _ = tx.send(());
                            }
                        } else if cmd == IDM_QUIT as i32 {
                            ASK_QUIT.store(true, Ordering::SeqCst);
                            PostQuitMessage(0);
                        }
                    }
                    _ => {}
                }
                0
            }
            WM_COMMAND => 0,
            WM_DESTROY => {
                let nid = nid_of(hwnd);
                Shell_NotifyIconW(NIM_DELETE, &nid);
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn nid_of(hwnd: isize) -> NOTIFYICONDATAW {
    let mut tip = [0u16; 128];
    let t: Vec<u16> = "HuFu 虎符输入法".encode_utf16().collect();
    tip[..t.len()].copy_from_slice(&t);
    let hicon = make_hu_icon();
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_APP,
        hIcon: hicon,
        szTip: tip,
        dwState: 0,
        dwStateMask: 0,
        szInfo: [0; 256],
        uVersion: 0,
        szInfoTitle: [0; 64],
        dwInfoFlags: 0,
        guidItem: [0; 16],
        hBalloonIcon: 0,
    }
}

/// 在独立线程跑托盘消息循环。返回 (ask_quit 标志引用, 设置页请求接收端)。
/// 传入 `quit_tx`：托盘退出时发 () 通知主循环退出。
pub fn spawn(quit_tx: Sender<()>, open_tx: Sender<()>) {
    std::thread::spawn(move || unsafe {
        unsafe {
            OPEN_SETTINGS = Some(open_tx);
        }
        let hinst = GetModuleHandleW(std::ptr::null());
        let cls: Vec<u16> = "HuFuTrayWnd\0".encode_utf16().collect();
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: wnd_proc,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: 0,
            hCursor: 0,
            hbrBackground: 0,
            lpszMenuName: std::ptr::null(),
            lpszClassName: cls.as_ptr(),
        };
        let atom = RegisterClassW(&wc);
        if atom == 0 {
            return;
        }
        let hwnd = CreateWindowExW(
            0, cls.as_ptr(), std::ptr::null(), 0,
            0, 0, 0, 0, 0, 0, hinst, 0,
        );
        if hwnd == 0 {
            return;
        }
        TRAY_HWND.store(hwnd, Ordering::SeqCst);
        let nid = nid_of(hwnd);
        if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
            // 已有同名图标（残留）→ 先删再加
            Shell_NotifyIconW(NIM_DELETE, &nid);
            Shell_NotifyIconW(NIM_ADD, &nid);
        }
        let _ = GetCurrentThreadId();
        let mut m = MSG { hwnd: 0, message: 0, wParam: 0, lParam: 0, time: 0, pt: POINT { x: 0, y: 0 } };
        loop {
            let r = GetMessageW(&mut m, 0, 0, 0);
            if r <= 0 {
                break;
            }
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        // 通知主循环退出
        let _ = quit_tx.send(());
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid_of(hwnd));
    });
}

#[repr(C)]
struct MSG {
    hwnd: isize,
    message: u32,
    wParam: usize,
    lParam: isize,
    time: u32,
    pt: POINT,
}

#[link(name = "user32")]
extern "system" {
    fn GetMessageW(lpmsg: *mut MSG, hwnd: isize, min: u32, max: u32) -> i32;
    fn TranslateMessage(lpmsg: *const MSG) -> i32;
    fn DispatchMessageW(lpmsg: *const MSG) -> isize;
}
