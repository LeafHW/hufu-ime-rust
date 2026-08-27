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
    // IDI_APPLICATION(32512) 共享加载，保证可见
    let hicon = unsafe {
        LoadImageW(0, 32512 as *const u16, 1 /*IMAGE_ICON*/, 0, 0, 0x8000 /*LR_SHARED*/)
    };
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
