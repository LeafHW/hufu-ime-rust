//! Minimal WebView2 window host for x86_64-pc-windows-gnu.
//!
//! Design constraints (proven by this spike):
//!   * no MSVC toolchain / no import libraries for WebView2Loader
//!   * no `windows` crate >= 0.61, no WinRT (`api-ms-win-core-winrt-*`)
//!   * WebView2Loader.dll is loaded with LoadLibraryW + GetProcAddress
//!   * all COM interfaces are hand-written #[repr(C)] vtables, accessed by
//!     vtable slot index (verified against WebView2.h 1.0.4314.35 / the
//!     webview2-com-sys 0.38.2 bindings).
//!
//! The host creates a top-level window titled "虎符设置", creates a WebView2
//! environment + controller for it, navigates to the configured URL, logs the
//! navigation result (plus a DOM probe via ExecuteScript) to a log file, and
//! exits cleanly on WM_CLOSE.

#![windows_subsystem = "windows"]
#![allow(non_snake_case, non_camel_case_types)]

mod win32;

use std::ffi::c_void;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use win32::*;

#[repr(C)]
struct MONITORINFO {
    cbSize: u32,
    rcMonitor: RECT,
    rcWork: RECT,
    dwFlags: u32,
}

#[link(name = "user32")]
extern "system" {
    fn MonitorFromWindow(hwnd: HWND, flags: u32) -> isize;
    fn GetMonitorInfoW(monitor: isize, info: *mut MONITORINFO) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateSolidBrush(color: u32) -> isize;
    fn FillRect(hdc: isize, rc: *const RECT, brush: isize) -> i32;
    fn DeleteObject(obj: isize) -> i32;
}
#[link(name = "user32")]
extern "system" {
    fn SetFocus(hwnd: HWND) -> HWND;
    fn SetActiveWindow(hwnd: HWND) -> HWND;
    fn keybd_event(bvk: u8, bscan: u8, dwflags: u32, dwextrainfo: usize);
    fn GetForegroundWindow() -> HWND;
}

const WM_ACTIVATE_MSG: u32 = 0x0006;
const KEYEVENTF_KEYUP: u32 = 0x0002;

const MONITOR_DEFAULTTONEAREST: u32 = 2;

// ---------------------------------------------------------------------------
// COM basics
// ---------------------------------------------------------------------------

type HRESULT = i32;

const S_OK: HRESULT = 0;
const E_NOINTERFACE: HRESULT = 0x8000_4002u32 as i32;
const E_POINTER: HRESULT = 0x8000_4003u32 as i32;

/// Win32 GUID memory layout (little-endian data1/2/3, raw data4).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct GUID {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const IID_IUNKNOWN: GUID = GUID {
    data1: 0,
    data2: 0,
    data3: 0,
    data4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46],
};

// 4e8a3389-c9d8-4bd2-b6b5-124fee6cc14d
const IID_ENV_HANDLER: GUID = GUID {
    data1: 0x4e8a_3389,
    data2: 0xc9d8,
    data3: 0x4bd2,
    data4: [0xb6, 0xb5, 0x12, 0x4f, 0xee, 0x6c, 0xc1, 0x4d],
};

// 6c4819f3-c9b7-4260-8127-c9f5bde7f68c
const IID_CONTROLLER_HANDLER: GUID = GUID {
    data1: 0x6c48_19f3,
    data2: 0xc9b7,
    data3: 0x4260,
    data4: [0x81, 0x27, 0xc9, 0xf5, 0xbd, 0xe7, 0xf6, 0x8c],
};

// d33a35bf-1c49-4f98-93ab-006e0533fe1c
const IID_NAV_HANDLER: GUID = GUID {
    data1: 0xd33a_35bf,
    data2: 0x1c49,
    data3: 0x4f98,
    data4: [0x93, 0xab, 0x00, 0x6e, 0x05, 0x33, 0xfe, 0x1c],
};

// 49511172-cc67-4bca-9923-137112f4c4cc
const IID_SCRIPT_HANDLER: GUID = GUID {
    data1: 0x4951_1172,
    data2: 0xcc67,
    data3: 0x4bca,
    data4: [0x99, 0x23, 0x13, 0x71, 0x12, 0xf4, 0xc4, 0xcc],
};

// 57213f19-00e6-49fa-8e07-898ea01ecbd2
const IID_MSG_HANDLER: GUID = GUID {
    data1: 0x5721_3f19,
    data2: 0x00e6,
    data3: 0x49fa,
    data4: [0x8e, 0x07, 0x89, 0x8e, 0xa0, 0x1e, 0xcb, 0xd2],
};

// fdb5ab74-af33-4854-84f0-0a631deb5eba (ICoreWebView2Settings3)
const IID_SETTINGS3: GUID = GUID {
    data1: 0xfdb5_ab74,
    data2: 0xaf33,
    data3: 0x4854,
    data4: [0x84, 0xf0, 0x0a, 0x63, 0x1d, 0xeb, 0x5e, 0xba],
};

/// Read function pointer `index` out of the object's vtable.
unsafe fn com_fn<T>(this: *mut c_void, index: usize) -> T {
    let vtbl = *(this as *mut *const *const c_void);
    let fp = *vtbl.add(index);
    std::mem::transmute_copy(&fp)
}

/// IUnknown::QueryInterface（vtable 0）。
unsafe fn com_query_interface(this: *mut c_void, iid: &GUID, out: *mut *mut c_void) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT =
        com_fn(this, 0);
    f(this, iid, out)
}

/// ICoreWebView2::get_Settings（vtable 3）。
unsafe fn wv_get_settings(webview: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT =
        com_fn(webview, 3);
    f(webview, out)
}

unsafe fn com_add_ref(this: *mut c_void) -> u32 {
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = com_fn(this, 1);
    f(this)
}

#[allow(dead_code)]
unsafe fn com_release(this: *mut c_void) -> u32 {
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = com_fn(this, 2);
    f(this)
}

// ICoreWebView2Environment: IUnknown(0..2), CreateCoreWebView2Controller(3),
// CreateWebResourceResponse(4), get_BrowserVersionString(5).
unsafe fn env_create_controller(
    env: *mut c_void,
    hwnd: HWND,
    handler: *mut c_void,
) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, HWND, *mut c_void) -> HRESULT = com_fn(env, 3);
    f(env, hwnd, handler)
}

unsafe fn env_browser_version(env: *mut c_void) -> Option<String> {
    let f: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> HRESULT = com_fn(env, 5);
    let mut p: *mut u16 = null_mut();
    let hr = f(env, &mut p);
    if hr >= 0 && !p.is_null() {
        let s = from_wide_ptr(p);
        CoTaskMemFree(p as *mut c_void);
        Some(s)
    } else {
        None
    }
}

// ICoreWebView2Controller vtable order (verified):
//   3 get_IsVisible, 4 put_IsVisible, 5 get_Bounds, 6 put_Bounds,
//   7/8 ZoomFactor, 9/10 add/remove ZoomFactorChanged, 11 SetBoundsAndZoomFactor,
//   12 MoveFocus, 13/14 MoveFocusRequested, 15/16 GotFocus, 17/18 LostFocus,
//   19/20 AcceleratorKeyPressed, 21/22 ParentWindow,
//   23 NotifyParentWindowPositionChanged, 24 Close, 25 get_CoreWebView2
unsafe fn controller_put_is_visible(controller: *mut c_void, visible: i32) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT = com_fn(controller, 4);
    f(controller, visible)
}

unsafe fn controller_put_bounds(controller: *mut c_void, bounds: RECT) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, RECT) -> HRESULT = com_fn(controller, 6);
    f(controller, bounds)
}

/// ICoreWebView2Controller::MoveFocus（vtable 12）；reason 0=Programmatic。
unsafe fn controller_move_focus(controller: *mut c_void, reason: i32) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT = com_fn(controller, 12);
    f(controller, reason)
}

unsafe fn controller_close(controller: *mut c_void) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void) -> HRESULT = com_fn(controller, 24);
    f(controller)
}

unsafe fn controller_get_core_webview2(
    controller: *mut c_void,
    out: *mut *mut c_void,
) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT =
        com_fn(controller, 25);
    f(controller, out)
}

// ICoreWebView2 vtable order (verified against WebView2.h 1.0.4314.35):
//   3 Settings, 4 Source, 5 Navigate, 6 NavigateToString,
//   7/8 NavigationStarting, 9/10 ContentLoading, 11/12 SourceChanged,
//   13/14 HistoryChanged, 15/16 NavigationCompleted, ...
//   27/28 Add/RemoveScriptToExecuteOnDocumentCreated, 29 ExecuteScript
unsafe fn wv_navigate(webview: *mut c_void, uri: *const u16) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT = com_fn(webview, 5);
    f(webview, uri)
}

/// [P0-2a 2026-11-01] NavigateToString（vtable 6）：导航失败/启动超时兜底
/// 错误页（内嵌 HTML，不依赖 server 在线）。
unsafe fn wv_navigate_to_string(webview: *mut c_void, html: *const u16) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT = com_fn(webview, 6);
    f(webview, html)
}

unsafe fn wv_add_navigation_completed(
    webview: *mut c_void,
    handler: *mut c_void,
    token: *mut i64,
) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut i64) -> HRESULT =
        com_fn(webview, 15);
    f(webview, handler, token)
}

unsafe fn wv_execute_script(
    webview: *mut c_void,
    script: *const u16,
    handler: *mut c_void,
) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, *const u16, *mut c_void) -> HRESULT =
        com_fn(webview, 29);
    f(webview, script, handler)
}

// ICoreWebView2: 33 PostWebMessageAsString, 34 add_WebMessageReceived.
unsafe fn wv_add_web_message_received(
    webview: *mut c_void,
    handler: *mut c_void,
    token: *mut i64,
) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut i64) -> HRESULT =
        com_fn(webview, 34);
    f(webview, handler, token)
}

unsafe fn wv_post_message_string(webview: *mut c_void, msg: *const u16) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT = com_fn(webview, 33);
    f(webview, msg)
}

// ICoreWebView2WebMessageReceivedEventArgs: 3 Source, 4 get_WebMessageAsJson,
// 5 TryGetWebMessageAsString.
unsafe fn msg_args_try_string(args: *mut c_void) -> Option<String> {
    if args.is_null() {
        return None;
    }
    let f: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> HRESULT = com_fn(args, 5);
    let mut p: *mut u16 = null_mut();
    let hr = f(args, &mut p);
    if hr >= 0 && !p.is_null() {
        let s = from_wide_ptr(p);
        CoTaskMemFree(p as *mut c_void);
        Some(s)
    } else {
        None
    }
}

unsafe fn snap_window(hwnd: HWND, zone: &str) {
    let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    if mon == 0 {
        return;
    }
    let mut info: MONITORINFO = std::mem::zeroed();
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(mon, &mut info) == 0 {
        return;
    }
    let wa = info.rcWork;
    let ww = (wa.right - wa.left).max(320);
    let wh = (wa.bottom - wa.top).max(240);
    let (x, y, w, h) = match zone {
        "left" => (wa.left, wa.top, ww / 2, wh),
        "right" => (wa.left + ww / 2, wa.top, ww - ww / 2, wh),
        "tl" => (wa.left, wa.top, ww / 2, wh / 2),
        "tr" => (wa.left + ww / 2, wa.top, ww - ww / 2, wh / 2),
        "bl" => (wa.left, wa.top + wh / 2, ww / 2, wh - wh / 2),
        "br" => (wa.left + ww / 2, wa.top + wh / 2, ww - ww / 2, wh - wh / 2),
        _ => return,
    };
    let _ = ShowWindow(hwnd, SW_RESTORE);
    let _ = SetWindowPos(
        hwnd,
        0,
        x,
        y,
        w,
        h,
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
}

/// 【2026-11-03 浅色黑边】`#rrggbb` -> Win32 COLORREF（0x00BBGGRR）。
fn parse_hex_color(text: &str) -> Option<u32> {
    let t = text.trim().trim_start_matches('#');
    if t.len() != 6 {
        return None;
    }
    let r = u32::from_str_radix(&t[0..2], 16).ok()?;
    let g = u32::from_str_radix(&t[2..4], 16).ok()?;
    let b = u32::from_str_radix(&t[4..6], 16).ok()?;
    Some((b << 16) | (g << 8) | r)
}

/// 【2026-11-03 浅色黑边】用页面同色重绘 7px 非客户区（拉伸）边框。
///
/// 窗口是 WS_POPUP|WS_THICKFRAME：客户区比窗口小 7px，这圈边框默认由系统
/// 按窗口深色/浅色标志绘制。此前壳无条件 dark=1，浅色主题下就是一圈黑边，
/// 圆角处也被黑边吃掉。这里直接接管 WM_NCPAINT：
///   上 / 右 / 下 = 页面底色（--bg）
///   左          = 侧栏底色（--bg2，标题栏以下）
/// 于是边框与内容同色、视觉上等于内容铺满整窗，只剩 DWM 的圆角外轮廓。
unsafe fn paint_window_frame(app: *mut App) {
    let a = &*app;
    let mut wr = RECT::default();
    if GetWindowRect(a.hwnd, &mut wr) == 0 {
        return;
    }
    let mut cr = RECT::default();
    if GetClientRect(a.hwnd, &mut cr) == 0 {
        return;
    }
    let mut org = POINT { x: 0, y: 0 };
    if ClientToScreen(a.hwnd, &mut org) == 0 {
        return;
    }
    let in_left = (org.x - wr.left).max(0);
    let in_top = (org.y - wr.top).max(0);
    let in_right = (wr.right - (org.x + cr.width())).max(0);
    let in_bottom = (wr.bottom - (org.y + cr.height())).max(0);
    if in_left == 0 && in_top == 0 && in_right == 0 && in_bottom == 0 {
        return;
    }
    let hdc = GetWindowDC(a.hwnd);
    if hdc == 0 {
        return;
    }
    let bg = CreateSolidBrush(a.frame_bg);
    let side = CreateSolidBrush(a.frame_side);
    let w = wr.width();
    let h = wr.height();
    let top = RECT { left: 0, top: 0, right: w, bottom: in_top };
    let _ = FillRect(hdc, &top, bg);
    let right = RECT { left: w - in_right, top: 0, right: w, bottom: h };
    let _ = FillRect(hdc, &right, bg);
    let bottom = RECT { left: 0, top: h - in_bottom, right: w, bottom: h };
    let _ = FillRect(hdc, &bottom, bg);
    /* 左侧从标题栏下沿开始才是侧栏底色，顶部那条仍属标题栏 */
    let left = RECT { left: 0, top: in_top, right: in_left, bottom: h };
    let _ = FillRect(hdc, &left, side);
    let _ = DeleteObject(bg);
    let _ = DeleteObject(side);
    let _ = ReleaseDC(a.hwnd, hdc);
}

unsafe fn handle_window_message(app: *mut App, msg: &str) {
    let hwnd = (*app).hwnd;
    (*app).log(&format!("web message: {msg}"));
    match msg {
        "win:minimize" => {
            ShowWindow(hwnd, SW_MINIMIZE);
        }
        "win:maximize" => {
            if IsZoomed(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            } else {
                ShowWindow(hwnd, SW_MAXIMIZE);
            }
        }
        "win:close" => {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
        "win:ready" => {
            KillTimer(hwnd, 2);
            // [P0-2b] React 已就绪：解除启动超时看护
            KillTimer(hwnd, 3);
            // 【2026-11-02 白闪根治 v1】不立即显示：再等 100ms 让合成器把
            // WebView2 首帧真正提交上屏（React ready ≠ 首帧已上屏），显示
            // 时用户看到的第一眼就是已渲染页面而非白色表面。前端配合：
            // win:ready 本身等两帧 + 80ms 才发（Shell.tsx），两级合计约
            // 180ms。不加深色遮罩（v2 方案已否决）：遮罩色无法跟随浅色
            // 主题，会在浅色界面下引入「黑→白」二次闪变。
            SetTimer(hwnd, 4, 100, null());
            (*app).log("win:ready received; reveal scheduled in 100ms");
        }
        "win:drag" => {
            ReleaseCapture();
            SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as WPARAM, 0);
        }
        other if other.starts_with("win:snap-") => {
            snap_window(hwnd, &other["win:snap-".len()..]);
        }
        other if other.starts_with("win:theme:") => {
            /* 形如 win:theme:#f5f2ea:#efe9dd:light —— 页面解析后的主题色。
               壳据此：①按主题设置 DWM 深色标志（窗框/描边跟主题）；
               ②用同色重绘 7px 非客户区边框；③重画边框。 */
            let rest = &other["win:theme:".len()..];
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() >= 3 {
                if let (Some(bg), Some(side)) =
                    (parse_hex_color(parts[0]), parse_hex_color(parts[1]))
                {
                    (*app).frame_bg = bg;
                    (*app).frame_side = side;
                }
                apply_window_theme(hwnd, parts[2] != "light", parse_hex_color(parts[0]));
                RedrawWindow(hwnd, null(), 0, 0x0001 | 0x0020 | 0x0080);
                (*app).log(&format!("frame theme: {rest}"));
            }
        }
        _ => {}
    }
}

unsafe fn sync_maximized_state(app: *mut App) {
    if (*app).webview.is_null() {
        return;
    }
    let state = if IsZoomed((*app).hwnd) != 0 {
        "win:maximized"
    } else {
        "win:restored"
    };
    let w = wide(state);
    let _ = wv_post_message_string((*app).webview, w.as_ptr());
}

// ICoreWebView2NavigationCompletedEventArgs: 3 get_IsSuccess,
// 4 get_WebErrorStatus, 5 get_NavigationId.
unsafe fn nav_args_is_success(args: *mut c_void, out: *mut i32) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT = com_fn(args, 3);
    f(args, out)
}

/// [P0-2a] 导航失败的 WebErrorStatus（ICoreWebView2WebErrorStatus 枚举值），
/// 用于错误页/日志指认原因（1=连接被拒，2=超时，3=主机名解析失败…）。
unsafe fn nav_args_web_error_status(args: *mut c_void, out: *mut i32) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT = com_fn(args, 4);
    f(args, out)
}

// ---------------------------------------------------------------------------
// Hand-written COM callback handlers
// ---------------------------------------------------------------------------

const KIND_ENV: u32 = 1;
const KIND_CONTROLLER: u32 = 2;
const KIND_NAV: u32 = 3;
const KIND_SCRIPT: u32 = 4;
const KIND_MSG: u32 = 5;

#[repr(C)]
struct Handler {
    vtbl: *const HandlerVtbl,
    refs: u32,
    kind: u32,
    iid: GUID,
    app: *mut App,
}

#[repr(C)]
struct HandlerVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    // Deliberately typed with `usize` for the second argument:
    //  * env/controller/script handlers receive an HRESULT there,
    //  * navigation-completed handlers receive the ICoreWebView2* sender there.
    // On the Windows ABI both travel in the same 64-bit argument slot, and the
    // HRESULT case is narrowed back with `as i32` below. Using `HRESULT` would
    // truncate the sender pointer on a navigation callback.
    invoke: unsafe extern "system" fn(*mut c_void, usize, *mut c_void) -> HRESULT,
}

static HANDLER_VTBL: HandlerVtbl = HandlerVtbl {
    query_interface: handler_qi,
    add_ref: handler_add_ref,
    release: handler_release,
    invoke: handler_invoke,
};

fn handler_new(kind: u32, iid: GUID, app: *mut App) -> *mut c_void {
    let h = Box::new(Handler {
        vtbl: &HANDLER_VTBL,
        refs: 1,
        kind,
        iid,
        app,
    });
    Box::into_raw(h) as *mut c_void
}

unsafe extern "system" fn handler_qi(
    this: *mut c_void,
    iid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if this.is_null() || iid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    let h = &*(this as *const Handler);
    if *iid == IID_IUNKNOWN || *iid == h.iid {
        *ppv = this;
        handler_add_ref(this);
        S_OK
    } else {
        *ppv = null_mut();
        E_NOINTERFACE
    }
}

unsafe extern "system" fn handler_add_ref(this: *mut c_void) -> u32 {
    if this.is_null() {
        return 0;
    }
    let h = &mut *(this as *mut Handler);
    h.refs = h.refs.saturating_add(1);
    h.refs
}

/// The runtime owns its own reference; the host deliberately never frees a
/// handler (a handful of small allocations per process, which keeps lifetime
/// handling trivial for a spike). We still decrement for honest bookkeeping.
unsafe extern "system" fn handler_release(this: *mut c_void) -> u32 {
    if this.is_null() {
        return 0;
    }
    let h = &mut *(this as *mut Handler);
    if h.refs > 0 {
        h.refs -= 1;
    }
    h.refs
}

unsafe extern "system" fn handler_invoke(
    this: *mut c_void,
    p1: usize,
    p2: *mut c_void,
) -> HRESULT {
    if this.is_null() {
        return E_POINTER;
    }
    let h = &*(this as *const Handler);
    let app = h.app;
    if app.is_null() {
        return S_OK;
    }

    match h.kind {
        KIND_ENV => {
            let hr = p1 as i32;
            if hr < 0 || p2.is_null() {
                fail_close(
                    app,
                    &format!("CreateCoreWebView2Environment failed hr=0x{:08X}", hr as u32),
                );
                return S_OK;
            }
            (*app).env = p2;
            com_add_ref(p2);
            let version = env_browser_version(p2).unwrap_or_else(|| "<unknown>".to_string());
            (*app).log(&format!(
                "environment created; WebView2 runtime version = {version}"
            ));

            let controller_handler = handler_new(KIND_CONTROLLER, IID_CONTROLLER_HANDLER, app);
            let hr2 = env_create_controller(p2, (*app).hwnd, controller_handler);
            (*app).log(&format!(
                "CreateCoreWebView2Controller -> hr=0x{:08X}",
                hr2 as u32
            ));
            if hr2 < 0 {
                fail_close(app, "CreateCoreWebView2Controller call failed");
            }
        }
        KIND_CONTROLLER => {
            let hr = p1 as i32;
            if hr < 0 || p2.is_null() {
                fail_close(
                    app,
                    &format!("controller creation failed hr=0x{:08X}", hr as u32),
                );
                return S_OK;
            }
            (*app).controller = p2;
            com_add_ref(p2);
            (*app).log("controller created");

            let hr_vis = controller_put_is_visible(p2, 1);
            (*app).log(&format!("put_IsVisible(TRUE) -> hr=0x{:08X}", hr_vis as u32));
            resize_webview(app);

            let mut webview: *mut c_void = null_mut();
            let hr_wv = controller_get_core_webview2(p2, &mut webview);
            if hr_wv < 0 || webview.is_null() {
                fail_close(
                    app,
                    &format!("get_CoreWebView2 failed hr=0x{:08X}", hr_wv as u32),
                );
                return S_OK;
            }
            com_add_ref(webview);
            (*app).webview = webview;
            (*app).log("ICoreWebView2 acquired");

            // 设置页自己的 Ctrl+K / F5 等快捷键不能被 WebView2 浏览器层吞掉。
            // Settings3 追加方法：QueryInterface 后 put_AreBrowserAcceleratorKeysEnabled。
            let mut settings: *mut c_void = null_mut();
            let hr_set = wv_get_settings(webview, &mut settings);
            if hr_set >= 0 && !settings.is_null() {
                let mut s3: *mut c_void = null_mut();
                let hr_q = com_query_interface(settings, &IID_SETTINGS3, &mut s3);
                if hr_q >= 0 && !s3.is_null() {
                    let f: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT =
                        com_fn(s3, 24);
                    let hr_acc = f(s3, 0);
                    (*app).log(&format!(
                        "put_AreBrowserAcceleratorKeysEnabled(FALSE) -> hr=0x{:08X}",
                        hr_acc as u32
                    ));
                    com_release(s3);
                } else {
                    (*app).log(&format!(
                        "Settings3 QI failed hr=0x{:08X} (browser accelerators stay default)",
                        hr_q as u32
                    ));
                }
                com_release(settings);
            } else {
                (*app).log(&format!(
                    "get_Settings failed hr=0x{:08X}",
                    hr_set as u32
                ));
            }

            let msg_handler = handler_new(KIND_MSG, IID_MSG_HANDLER, app);
            let mut msg_token: i64 = 0;
            let hr_msg = wv_add_web_message_received(webview, msg_handler, &mut msg_token);
            (*app).log(&format!(
                "add_WebMessageReceived -> hr=0x{:08X} token={msg_token}",
                hr_msg as u32
            ));

            let nav_handler = handler_new(KIND_NAV, IID_NAV_HANDLER, app);
            let mut token: i64 = 0;
            let hr_ev = wv_add_navigation_completed(webview, nav_handler, &mut token);
            (*app).log(&format!(
                "add_NavigationCompleted -> hr=0x{:08X} token={token}",
                hr_ev as u32
            ));

            let hr_nav = wv_navigate(webview, (*app).url.as_ptr());
            (*app).log(&format!(
                "Navigate({}) -> hr=0x{:08X}",
                (*app).url_text,
                hr_nav as u32
            ));
            if hr_nav < 0 {
                fail_close(app, "Navigate call failed");
            }
        }
        KIND_NAV => {
            // p1 = sender ICoreWebView2*, p2 = ICoreWebView2NavigationCompletedEventArgs*
            let args = p2;
            let mut success: i32 = 0;
            let hr_ok = if args.is_null() {
                -1
            } else {
                nav_args_is_success(args, &mut success)
            };
            (*app).log(&format!(
                "NavigationCompleted: is_success={success} (args hr=0x{:08X})",
                hr_ok as u32
            ));
            // [P0-2a] 错误页自身的导航不算业务结果
            let was_error_nav = (*app).error_nav_pending;
            (*app).error_nav_pending = false;
            if was_error_nav {
                if success == 0 {
                    // 错误页都渲染失败（极小概率）：消息框兜底后退出
                    fail_close(app, "错误页渲染失败（WebView2 状态异常）");
                }
                return S_OK;
            }
            if success != 0 {
                (*app).nav_ok = true;
                // [P0-2a] 重试成功：退出错误态，恢复正常显示路径
                if (*app).error_mode {
                    (*app).error_mode = false;
                    (*app).log("retry navigation succeeded; leaving error mode");
                }
                (*app).log("SPIKE_READY navigation succeeded");
                // Let the page paint one frame while hidden, then reveal.
                SetTimer((*app).hwnd, 2, 1500, null());
            } else {
                // [P0-2a 2026-11-01] 导航失败（server 未起/端口被占/防火墙）：
                // 旧版这里什么都不做——窗口永不显示、进程静默挂起（验收
                // P0-2/D-1）。现在显示内嵌错误页（含目标地址与重试按钮）。
                let mut web_err: i32 = 0;
                if !args.is_null() {
                    let _ = nav_args_web_error_status(args, &mut web_err);
                }
                show_error_page(
                    app,
                    "无法打开虎符设置页",
                    &format!(
                        "hufu-server 可能未启动或端口被占用。\n导航失败（WebErrorStatus={web_err}）。\n\n点「重试」重新连接；也可关闭后从托盘/开始菜单重新打开。"
                    ),
                );
                return S_OK;
            }

            // Prove that the document is really there, not just that a window
            // exists: ask the page for its title / readyState / text snippet.
            if !(*app).webview.is_null() && !(*app).error_mode {
                let script_handler = handler_new(KIND_SCRIPT, IID_SCRIPT_HANDLER, app);
                let js = wide(
                    "JSON.stringify({title:document.title,ready:document.readyState,\
                     url:location.href,text:((document.body&&document.body.innerText)||'').slice(0,160)})",
                );
                let hr_js = wv_execute_script((*app).webview, js.as_ptr(), script_handler);
                (*app).log(&format!(
                    "ExecuteScript(DOM evidence) -> hr=0x{:08X}",
                    hr_js as u32
                ));
            }
        }
        KIND_SCRIPT => {
            let hr = p1 as i32;
            if hr < 0 {
                (*app).log(&format!(
                    "ExecuteScript completed with error hr=0x{:08X}",
                    hr as u32
                ));
            } else {
                let result = from_wide_ptr(p2 as *const u16);
                let short: String = result.chars().take(600).collect();
                (*app).log(&format!("DOM evidence: {short}"));
            }
        }
        KIND_MSG => {
            if let Some(msg) = msg_args_try_string(p2) {
                handle_window_message(app, &msg);
            }
        }
        _ => {}
    }

    S_OK
}

// ---------------------------------------------------------------------------
// App state / logging
// ---------------------------------------------------------------------------

struct App {
    hwnd: HWND,
    log_path: PathBuf,
    url: Vec<u16>,
    url_text: String,
    user_data: Vec<u16>,
    env: *mut c_void,
    controller: *mut c_void,
    webview: *mut c_void,
    nav_ok: bool,
    failed: bool,
    /// [P0-2a/b] 窗口是否已对用户可见（win:ready / 首帧 timer / 错误页）。
    shown: bool,
    /// [P0-2a] 当前停在错误页（跳过 DOM 探测脚本；重试成功后复位）。
    error_mode: bool,
    /// [P0-2a] 本次 NavigationCompleted 属于错误页自身的导航。
    error_nav_pending: bool,
    /// 【2026-11-03 修】前台/焦点补偿 timer 6 的已重试次数：最多 5 次
    ///（约 600ms），否则窗口抢不到前台时会每 120ms 无限重试并反复
    /// 合成 Alt 按键，干扰用户其他窗口。
    focus_retry: u32,
    /// 【2026-11-03 浅色黑边】7px 非客户区边框的填充色（COLORREF 0x00BBGGRR）：
    /// 页面底色 / 侧栏底色，由页面 win:theme 消息同步；默认按深色主题。
    frame_bg: u32,
    frame_side: u32,
}

impl App {
    fn log(&self, msg: &str) {
        log_line(&self.log_path, msg);
    }
}

fn log_line(path: &Path, msg: &str) {
    let mut st = SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut st) };
    let line = format!(
        "[{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}] {}\r\n",
        st.wYear,
        st.wMonth,
        st.wDay,
        st.wHour,
        st.wMinute,
        st.wSecond,
        st.wMilliseconds,
        msg
    );
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

unsafe fn fail_close(app: *mut App, msg: &str) {
    (*app).failed = true;
    (*app).log(&format!("FATAL: {msg}"));
    // [P0-2a 2026-11-01] 失败也要让用户看见（旧版静默退出、无任何提示）：
    // 此时通常还没有 WebView 可用，用系统消息框兜底。模态消息循环内
    // WebView2 的回调照常分发（标准 Win32 模式）。
    let text = wide(&format!(
        "虎符设置窗口启动失败：\n\n{msg}\n\n请确认 hufu-server.exe 正在运行，\n或重新打开设置窗口重试。"
    ));
    let caption = wide("虎符设置");
    MessageBoxW((*app).hwnd, text.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONERROR);
    PostMessageW((*app).hwnd, WM_CLOSE, 0, 0);
}

/// HTML 转义（错误页 detail 含 url/错误码等外部字符串，最小转义 & < >）。
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// [P0-2a/b 2026-11-01] 统一失败兜底：WebView 在场 → NavigateToString 内嵌
/// 错误页（深色风格、重试/关闭按钮）并立即显示窗口；WebView 不在场 →
/// 系统消息框后关闭。此前导航失败/启动超时窗口永不显示、进程静默挂起
///（验收 P0-2/D-1）。
unsafe fn show_error_page(app: *mut App, headline: &str, detail: &str) {
    (*app).shown = true;
    (*app).error_mode = true;
    KillTimer((*app).hwnd, 2);
    KillTimer((*app).hwnd, 3);
    (*app).log(&format!("ERROR PAGE: {headline} | {detail}"));
    if (*app).webview.is_null() {
        let text = wide(&format!("{headline}\n\n{detail}\n\n目标地址：{}", (*app).url_text));
        let caption = wide("虎符设置");
        MessageBoxW(
            (*app).hwnd,
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
        PostMessageW((*app).hwnd, WM_CLOSE, 0, 0);
        return;
    }
    // 目标地址嵌成 JS 字符串字面量（format!("{:?}") 的转义对 JS 同构），
    // 重试按钮 location.href 直跳；关闭按钮走既有 win:close 消息链。
    let url_js = format!("{:?}", (*app).url_text);
    let html = format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>虎符设置 · 连接失败</title>
<style>
html,body{{margin:0;height:100%;background:#171413;color:#e8e0d2;
font-family:"Segoe UI","Microsoft YaHei",system-ui,sans-serif}}
.wrap{{height:100%;display:flex;flex-direction:column;align-items:center;
justify-content:center;gap:16px;padding:32px;text-align:center;box-sizing:border-box}}
h1{{font-size:20px;margin:0;font-weight:600}}
p{{font-size:13px;line-height:1.8;color:#b7ac99;margin:0;white-space:pre-wrap;max-width:560px}}
code{{color:#d9c48a;word-break:break-all}}
.btns{{display:flex;gap:12px}}
button{{min-width:112px;padding:9px 18px;font-size:14px;border-radius:8px;
border:1px solid #574a33;background:#2a241c;color:#e8d9b0;cursor:pointer}}
button:hover{{background:#3a3123}}
</style></head><body><div class="wrap">
<h1>{headline}</h1>
<p>{detail}</p>
<p><code>{url}</code></p>
<div class="btns">
<button id="retry">重试</button>
<button id="close">关闭</button>
</div></div>
<script>
var TARGET={url_js};
document.getElementById('retry').onclick=function(){{location.href=TARGET}};
document.getElementById('close').onclick=function(){{
  if(window.chrome&&window.chrome.webview){{window.chrome.webview.postMessage('win:close')}}
  else{{window.close()}}
}};
</script></body></html>"#,
        headline = escape_html(headline),
        detail = escape_html(detail),
        url = escape_html(&(*app).url_text),
        url_js = url_js,
    );
    (*app).error_nav_pending = true;
    let w = wide(&html);
    let hr = wv_navigate_to_string((*app).webview, w.as_ptr());
    (*app).log(&format!("NavigateToString(error page) -> hr=0x{:08X}", hr as u32));
    activate_and_focus(app);
}

unsafe fn app_ptr(hwnd: HWND) -> *mut App {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App
}

unsafe fn resize_webview(app: *mut App) {
    if app.is_null() {
        return;
    }
    let a = &*app;
    if a.controller.is_null() {
        return;
    }
    let mut rc = RECT::default();
    if GetClientRect(a.hwnd, &mut rc) != 0 && rc.width() > 0 && rc.height() > 0 {
        let hr = controller_put_bounds(a.controller, rc);
        if hr < 0 {
            a.log(&format!(
                "put_Bounds({}x{}) -> hr=0x{:08X}",
                rc.width(),
                rc.height(),
                hr as u32
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Focus / activation
// ---------------------------------------------------------------------------

/// 键盘焦点真正落到 WebView2 文档：只 SetForegroundWindow 会让窗口在前，
/// 但 DOM 收不到 keydown（用户需先点一下的根因）。Programmatic MoveFocus
/// 把焦点交给 controller；WM_ACTIVATE 时再补一次。
unsafe fn focus_webview(app: *mut App) {
    if app.is_null() {
        return;
    }
    let a = &*app;
    let _ = SetFocus(a.hwnd);
    if !a.controller.is_null() {
        let hr = controller_move_focus(a.controller, 0);
        a.log(&format!("MoveFocus(Programmatic) -> hr=0x{:08X}", hr as u32));
    }
}

/// 从后台进程拉窗口到前台。先直接 SetForegroundWindow；只有系统真的
/// 拒绝时才合成 Alt 按抬解锁。旧实现无条件发 Alt，会在首次 Ctrl+K 前
/// 留下菜单/输入态，表现为第一次按键无效并伴随系统提示音。
unsafe fn ensure_foreground(hwnd: HWND) -> bool {
    if GetForegroundWindow() == hwnd {
        return true;
    }
    let _ = SetForegroundWindow(hwnd);
    if GetForegroundWindow() == hwnd {
        return true;
    }
    keybd_event(0x12, 0, 0, 0); // VK_MENU down
    let ret = SetForegroundWindow(hwnd);
    keybd_event(0x12, 0, KEYEVENTF_KEYUP, 0);
    let _ = ret;
    GetForegroundWindow() == hwnd
}

unsafe fn activate_and_focus(app: *mut App) {
    if app.is_null() {
        return;
    }
    let hwnd = (*app).hwnd;
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = UpdateWindow(hwnd);
    let fg = ensure_foreground(hwnd);
    let _ = SetActiveWindow(hwnd);
    focus_webview(app);
    (*app).log(&format!(
        "activate/focus: foreground={fg} fgHwnd=0x{:X} focus=0x{:X}",
        GetForegroundWindow() as usize,
        SetFocus(hwnd) as usize
    ));
    // 前台权/渲染焦点可能在首帧后才稳定：分几次补，不依赖用户第一下按键。
    (*app).focus_retry = 0;
    SetTimer(hwnd, 6, 120, null());
}

unsafe fn retry_foreground_and_focus(app: *mut App) {
    if app.is_null() {
        return;
    }
    let hwnd = (*app).hwnd;
    if GetForegroundWindow() != hwnd {
        ensure_foreground(hwnd);
    }
    let _ = SetActiveWindow(hwnd);
    focus_webview(app);
    (*app).focus_retry = (*app).focus_retry.saturating_add(1);
    // 最多重试 5 次（约 600ms）；成功或超限都必须停表，否则每 120ms
    // 抢一次前台 + 合成 Alt，会持续打扰用户已切换到的其他窗口。
    if GetForegroundWindow() == hwnd || (*app).focus_retry >= 5 {
        KillTimer(hwnd, 6);
        if GetForegroundWindow() == hwnd {
            (*app).log("foreground/focus retry succeeded");
        } else {
            (*app).log("foreground/focus retry gave up after 5 tries");
        }
    }
}

// ---------------------------------------------------------------------------
// Window procedure
// ---------------------------------------------------------------------------

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ACTIVATE_MSG => {
            // Alt-Tab / 任务栏 / 二次 Ctrl+Alt+H 激活时，把键盘焦点交还文档。
            if (wparam & 0xFFFF) != 0 {
                let app = app_ptr(hwnd);
                if !app.is_null() {
                    focus_webview(app);
                }
            }
            0
        }
        WM_NCCALCSIZE => {
            /* 【2026-11-03】顶部不再保留系统 7px 拉伸边框：把客户区顶边抬到
               窗口顶，标题栏自绘一直铺到边。否则主题切换时 DWM 会按系统
               深浅色把这条边框单独渐变（用户实锤「上面那条线不跟着扩散」）。
               必须用 NCCALCSIZE 传入的「新窗口矩形」顶边，不能用
               GetWindowRect：从最大化还原时后者还是旧坐标，客户区会被
               算成负 top、比窗口还高。最大化窗口顶边在屏幕外，用显示器
               工作区顶边兜底。 */
            let mut win_top = 0i32;
            let reuse = wparam != 0 && lparam != 0;
            if reuse {
                let p = lparam as *const NCCALCSIZE_PARAMS;
                win_top = (*p).rgrc[0].top;
            }
            let res = DefWindowProcW(hwnd, msg, wparam, lparam);
            if reuse {
                let mut top = win_top;
                let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                if mon != 0 {
                    let mut mi: MONITORINFO = std::mem::zeroed();
                    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                    if GetMonitorInfoW(mon, &mut mi) != 0 && top < mi.rcWork.top {
                        top = mi.rcWork.top;
                    }
                }
                let p = lparam as *mut NCCALCSIZE_PARAMS;
                (*p).rgrc[0].top = top;
            }
            res
        }
        WM_NCHITTEST => {
            /* 顶边框去掉后补回上边/四角的缩放手势（5px 命中区）。 */
            if IsZoomed(hwnd) == 0 {
                let mut wr = RECT::default();
                if GetWindowRect(hwnd, &mut wr) != 0 {
                    let x = ((lparam as u32) & 0xFFFF) as i16 as i32;
                    let y = (((lparam as u32) >> 16) & 0xFFFF) as i16 as i32;
                    let e = 5;
                    let left = x < wr.left + e;
                    let right = x >= wr.right - e;
                    let top = y < wr.top + e;
                    let bottom = y >= wr.bottom - e;
                    if top && left {
                        return HTTOPLEFT;
                    }
                    if top && right {
                        return HTTOPRIGHT;
                    }
                    if bottom && left {
                        return HTBOTTOMLEFT;
                    }
                    if bottom && right {
                        return HTBOTTOMRIGHT;
                    }
                    if top {
                        return HTTOP;
                    }
                    if bottom {
                        return HTBOTTOM;
                    }
                    if left {
                        return HTLEFT;
                    }
                    if right {
                        return HTRIGHT;
                    }
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_NCPAINT => {
            /* 【2026-11-03 浅色黑边】接管非客户区绘制：默认由系统按「深色标题栏」
               把 7px 拉伸边框画成深灰，浅色主题下就是一圈黑边。这里改用页面
               同色填充（上/右/下=页面底色，左=侧栏底色），只剩 DWM 圆角外轮廓。 */
            let app = app_ptr(hwnd);
            if !app.is_null() {
                paint_window_frame(app);
                return 0;
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_SIZE => {
            let app = app_ptr(hwnd);
            if !app.is_null() {
                resize_webview(app);
                sync_maximized_state(app);
            }
            0
        }
        WM_TIMER => {
            if wparam == 4 {
                // 【2026-11-02 白闪根治 v1】win:ready 后 100ms 揭示：
                // 此时 WebView2 首帧已提交，直接显示不会露白。
                KillTimer(hwnd, 4);
                let app = app_ptr(hwnd);
                if !app.is_null() && !(*app).shown {
                    (*app).shown = true;
                    activate_and_focus(app);
                    SetTimer(hwnd, 5, 90, null());
                    (*app).log("window shown after React ready (+100ms first frame)");
                }
                return 0;
            }
            if wparam == 6 {
                let app = app_ptr(hwnd);
                retry_foreground_and_focus(app);
                return 0;
            }
            if wparam == 5 {
                // 揭示后再补一次焦点：首帧定时器/导航完成时 DOM 可能还没
                // 准备好接收 keyboard focus。
                KillTimer(hwnd, 5);
                let app = app_ptr(hwnd);
                focus_webview(app);
                return 0;
            }
            if wparam == 2 {
                KillTimer(hwnd, 2);
                // [P0-2b] 首帧揭示完成：解除启动超时看护
                KillTimer(hwnd, 3);
                let app = app_ptr(hwnd);
                if !app.is_null() {
                    (*app).shown = true;
                    activate_and_focus(app);
                    SetTimer(hwnd, 5, 90, null());
                    (*app).log("window shown after navigation/paint");
                }
            } else if wparam == 3 {
                // [P0-2b 2026-11-01] 启动超时看护（10s）：窗口仍不可见 =
                // 环境创建挂起/导航永不完成（server 挂死、防火墙黑洞）。
                // 走与导航失败相同的兜底（错误页/消息框），不再静默挂起。
                KillTimer(hwnd, 3);
                let app = app_ptr(hwnd);
                if !app.is_null() && !(*app).shown && !(*app).failed {
                    (*app).log("startup watchdog fired (10s without visible window)");
                    show_error_page(
                        app,
                        "虎符设置页启动超时",
                        "10 秒内未能加载设置页。\nhufu-server 可能未响应或网络策略拦截了本地连接。\n\n点「重试」重新连接。",
                    );
                }
            } else {
                KillTimer(hwnd, 1);
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
            0
        }
        WM_DPICHANGED => {
            let app = app_ptr(hwnd);
            if !app.is_null() {
                (*app).log(&format!(
                    "WM_DPICHANGED dpi={}",
                    (wparam & 0xffff) as u32
                ));
            }
            // lParam is a RECT* with the suggested window rect for the new DPI.
            let r = lparam as *const RECT;
            if !r.is_null() {
                let r = &*r;
                let _ = SetWindowPos(
                    hwnd,
                    0,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            0
        }
        WM_CLOSE => {
            let app = app_ptr(hwnd);
            if !app.is_null() {
                let a = &*app;
                a.log("WM_CLOSE received; closing WebView2 controller");
                if !a.controller.is_null() {
                    let hr = controller_close(a.controller);
                    a.log(&format!("controller Close -> hr=0x{:08X}", hr as u32));
                }
            }
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ---------------------------------------------------------------------------
// WebView2Loader.dll discovery
// ---------------------------------------------------------------------------

type CreateEnvFn = unsafe extern "system" fn(
    browser_executable_folder: *const u16,
    user_data_folder: *const u16,
    environment_options: *mut c_void,
    environment_created_handler: *mut c_void,
) -> HRESULT;

unsafe fn load_webview2(exe_dir: &Path, log_path: &Path) -> Result<CreateEnvFn, String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("WEBVIEW2_LOADER") {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(exe_dir.join("WebView2Loader.dll"));

    let mut dir = Some(exe_dir.to_path_buf());
    for _ in 0..6 {
        let Some(d) = dir else { break };
        candidates.push(d.join("WebView2Loader.dll"));
        candidates.push(d.join("vendor").join("WebView2Loader.dll"));
        dir = d.parent().map(Path::to_path_buf);
    }

    for path in &candidates {
        if !path.is_file() {
            continue;
        }
        let wide_path = wide_os(path);
        let module = LoadLibraryW(wide_path.as_ptr());
        if module == 0 {
            log_line(
                log_path,
                &format!(
                    "LoadLibraryW({}) failed; GetLastError={}",
                    path.display(),
                    GetLastError()
                ),
            );
            continue;
        }
        let symbol = b"CreateCoreWebView2EnvironmentWithOptions\0";
        let proc = GetProcAddress(module, symbol.as_ptr());
        if proc.is_null() {
            log_line(
                log_path,
                &format!(
                    "GetProcAddress in {} failed; GetLastError={}",
                    path.display(),
                    GetLastError()
                ),
            );
            continue;
        }
        log_line(
            log_path,
            &format!("WebView2Loader loaded from {}", path.display()),
        );
        return Ok(std::mem::transmute(proc));
    }

    // Last chance: let Windows search the normal DLL search path (the exe
    // directory is searched first for LoadLibraryW anyway).
    let module = LoadLibraryW(wide("WebView2Loader.dll").as_ptr());
    if module != 0 {
        let symbol = b"CreateCoreWebView2EnvironmentWithOptions\0";
        let proc = GetProcAddress(module, symbol.as_ptr());
        if !proc.is_null() {
            log_line(
                log_path,
                "WebView2Loader loaded through the default DLL search path",
            );
            return Ok(std::mem::transmute(proc));
        }
    }

    Err(format!(
        "WebView2Loader.dll was not found / does not export \
         CreateCoreWebView2EnvironmentWithOptions (GetLastError={})",
        GetLastError()
    ))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_os(p: &Path) -> Vec<u16> {
    p.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

unsafe fn from_wide_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while len < 8192 && *p.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
}

fn exe_dir() -> PathBuf {
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(0, buf.as_mut_ptr(), buf.len() as u32) };
    let path = String::from_utf16_lossy(&buf[..n as usize]);
    PathBuf::from(path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

// ---------------------------------------------------------------------------
// 设置页 URL 端口探测（[P0 白屏修复 2026-11-02]）
// ---------------------------------------------------------------------------
// 背景：开始菜单快捷方式曾写死 `--url http://127.0.0.1:4390/next?shell=1`。
// 4390 被系统保留（Hyper-V/WinNAT 排除端口段等）时 hufu-server 会退到
// 17890+（本机实测 17890），双击快捷方式 = Navigate 到没人监听的端口 = 白屏；
// 托盘入口用 server 实际端口所以正常。
//
// 口径：候选端口按序探测「GET /api/about」，响应体必须同时含 `github`、
// `hufu-ime-rust`、`exe_dir` 三个形状特征才认定是虎符 server（旧口径
// 只要 `"version"` 就把别家本地服务认成虎符）。单端口 300ms 连接/读写
// 超时，总预算 1.5s（正常情况首个候选几毫秒命中）。探测在创建窗口之前
// 完成（此时还没进消息循环），最坏多花约 1.5s 启动时间。
const DEFAULT_PORT: u16 = 4390;
const PROBE_PORTS: [u16; 7] = [4390, 17890, 17891, 17892, 17893, 17894, 27890];
const PROBE_PORT_TIMEOUT: Duration = Duration::from_millis(300);
const PROBE_TOTAL_BUDGET: Duration = Duration::from_millis(1500);

fn default_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/next?shell=1")
}

struct CliArgs {
    url: String,
    /// `--url` 是否显式给出（命中虎符特征或非候选端口才保留；否则回退探测）
    url_explicit: bool,
    user_data: PathBuf,
    log: PathBuf,
    auto_close_secs: Option<u32>,
    dpi: Option<u32>,
    /// `--theme light|dark|system`：追加到设置页 URL 的 ?theme= 参数
    /// （前端 RT-13 已支持，一次性生效不写回 localStorage）。主要用于
    /// 启动白闪/主题验证探针（flash-probe.ps1），正常启动不传。
    theme: Option<String>,
}

fn parse_args(exe_dir: &Path) -> CliArgs {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| exe_dir.to_path_buf());
    let user_data = base.join("HuFu").join("WebView2");
    let mut args = CliArgs {
        url: default_url(DEFAULT_PORT),
        url_explicit: false,
        log: user_data.join("hufu-settings.log"),
        user_data,
        auto_close_secs: None,
        dpi: None,
        theme: None,
    };
    let argv: Vec<String> = std::env::args().collect();
    let mut i = 1usize;
    while i < argv.len() {
        match argv[i].as_str() {
            "--url" if i + 1 < argv.len() => {
                args.url = argv[i + 1].clone();
                args.url_explicit = true;
                i += 2;
            }
            "--user-data" if i + 1 < argv.len() => {
                args.user_data = PathBuf::from(&argv[i + 1]);
                i += 2;
            }
            "--log" if i + 1 < argv.len() => {
                args.log = PathBuf::from(&argv[i + 1]);
                i += 2;
            }
            "--auto-close" if i + 1 < argv.len() => {
                args.auto_close_secs = argv[i + 1].parse().ok();
                i += 2;
            }
            "--dpi" if i + 1 < argv.len() => {
                args.dpi = argv[i + 1].parse::<u32>().ok().filter(|d| (72..=480).contains(d));
                i += 2;
            }
            "--theme" if i + 1 < argv.len() => {
                let v = argv[i + 1].clone();
                if v == "light" || v == "dark" || v == "system" {
                    args.theme = Some(v);
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    if !args.user_data.is_absolute() {
        args.user_data = exe_dir.join(&args.user_data);
    }
    if !args.log.is_absolute() {
        args.log = exe_dir.join(&args.log);
    }
    args
}

/// 从 `http://host:port/path?query` 里抠出 (host, port)；无 authority 返回
/// None。不引 url crate——本机探测只需要这一种形态。
fn split_http_host_port(url: &str) -> Option<(String, u16)> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => Some((host.to_string(), port.parse().ok()?)),
        Some(_) => None,
        None => Some((authority.to_string(), 80)),
    }
}

/// 单端口探测结论。
enum ProbeHit {
    /// /api/about 响应体命中虎符特征：确认是目标 server。
    Hufu,
    /// TCP 连上了，但 300ms 内一个字节都没回来（server 忙/accept 排队）。
    /// 不算命中，但在总预算内值得重试一次——本机实测 server 偶发 >1s 响应。
    TcpOnly,
    /// 连上了、也回了 HTTP，但不是虎符（别家服务占着端口）。
    OtherHttp,
    /// 连不上：端口没人监听 / 被系统拒。
    Dead,
}

/// 探测 host:port 是否是虎符 server：HTTP/1.0 + `GET /api/about`，读完响应
/// 体后判定（只认 `github` + `exe_dir` 形状；`"version"` 子串会把别家服务误认成虎符）。
fn probe_hufu_http(host: &str, port: u16, timeout: Duration) -> ProbeHit {
    let Ok(mut addrs) = (host, port).to_socket_addrs() else {
        return ProbeHit::Dead;
    };
    let Some(addr) = addrs.next() else {
        return ProbeHit::Dead;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, timeout) else {
        return ProbeHit::Dead;
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let req = format!("GET /api/about HTTP/1.0\r\nHost: {host}:{port}\r\n\r\n");
    if stream.write_all(req.as_bytes()).is_err() {
        return ProbeHit::Dead;
    }
    let mut resp = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            break;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let _ = stream.set_read_timeout(Some(left.min(timeout)));
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                resp.extend_from_slice(&chunk[..n]);
                if resp.len() >= 64 * 1024 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    if resp.is_empty() {
        return ProbeHit::TcpOnly;
    }
    let text = String::from_utf8_lossy(&resp);
    if !text.starts_with("HTTP/") {
        return ProbeHit::OtherHttp; // 端口有人监听但说的不是 HTTP
    }
    if text.contains("\"github\"") && text.contains("hufu-ime-rust") && text.contains("\"exe_dir\"") {
        ProbeHit::Hufu
    } else {
        ProbeHit::OtherHttp
    }
}

/// 本次单端口探测可用的超时：不超过 300ms，也不超过 1.5s 总预算。
fn probe_slice(deadline: Instant) -> Option<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        None
    } else {
        Some(left.min(PROBE_PORT_TIMEOUT))
    }
}

/// 决定最终设置页 URL：
///   * `--url` 显式给出且命中虎符特征，或落在非候选端口（开发用 Vite 等）
///     → 保留：托盘/脚本按 server 实际端口传参走这条；
///   * 否则按 PROBE_PORTS 探测首个虎符 server，命中即用该端口；
///   * 「连上但 300ms 无响应」的端口在剩余预算内各重试一次（瞬时慢不误杀）；
///   * 全程未命中 → 回退到探测到的无监听端口，绝不导航到已知外来 HTTP 服务。
fn resolve_url(args: &mut CliArgs) {
    let deadline = Instant::now() + PROBE_TOTAL_BUDGET;
    if args.url_explicit {
        let explicit = args.url.clone();
        let parsed = split_http_host_port(&explicit);
        let hit = parsed
            .as_ref()
            .and_then(|(host, port)| probe_slice(deadline).map(|t| probe_hufu_http(host, *port, t)));
        let is_hufu = matches!(&hit, Some(ProbeHit::Hufu));
        let candidate_port = parsed
            .as_ref()
            .map(|(_, p)| *p)
            .filter(|p| PROBE_PORTS.contains(p));
        let dev_keep = candidate_port.is_none()
            && matches!(&hit, Some(ProbeHit::OtherHttp) | Some(ProbeHit::TcpOnly));
        if is_hufu || dev_keep {
            let note = if is_hufu {
                ""
            } else {
                "（非虎符特征，非候选端口，按用户指定保留）"
            };
            log_line(&args.log, &format!("explicit --url kept{note}：{explicit}"));
            return;
        }
        log_line(
            &args.log,
            &format!("explicit --url 非虎符/不可达，回退候选端口探测：{explicit}"),
        );
    }

    let mut tried: Vec<String> = Vec::new();
    let mut slow: Vec<u16> = Vec::new();
    let mut hit_port: Option<u16> = None;
    let mut dead_port: Option<u16> = None;
    for port in PROBE_PORTS {
        let Some(timeout) = probe_slice(deadline) else {
            tried.push("总预算耗尽".to_string());
            break;
        };
        match probe_hufu_http("127.0.0.1", port, timeout) {
            ProbeHit::Hufu => {
                hit_port = Some(port);
                break;
            }
            ProbeHit::TcpOnly => {
                tried.push(format!("{port}=连上但无响应"));
                slow.push(port);
            }
            ProbeHit::OtherHttp => tried.push(format!("{port}=别家 HTTP")),
            ProbeHit::Dead => {
                tried.push(format!("{port}=无监听"));
                // 只记首个无监听候选（4390 优先）：错误页「重试」指回
                // 4390，用户随后启动 server 即可恢复；记最后一个会把
                // 重试指到 27890，起服务后也连不上。
                if dead_port.is_none() {
                    dead_port = Some(port);
                }
            }
        }
    }
    // 首轮没命中：给「连上但超时」的端口一次重试（server 偶发慢于 300ms）
    if hit_port.is_none() {
        for port in slow {
            let Some(timeout) = probe_slice(deadline) else {
                break;
            };
            if matches!(probe_hufu_http("127.0.0.1", port, timeout), ProbeHit::Hufu) {
                hit_port = Some(port);
                break;
            }
        }
    }
    if let Some(port) = hit_port {
        args.url = default_url(port);
        log_line(
            &args.log,
            &format!(
                "port probe hit hufu server 127.0.0.1:{port}；target URL 改用：{}",
                args.url
            ),
        );
        return;
    }
    // 【2026-11-02 P1】没有虎符 server 时不要回退到 4390：4390 可能被别家
    // HTTP 服务占着（探测结果 OtherHttp），导航过去就是打开外来页面。
    // 优先用探测到「无监听」的候选端口（必然白屏/错误页，但安全）；
    // 全候选都被别家占时用 4391（不在候选表内，仍不指向已知外来服务）。
    let fallback_port = dead_port.unwrap_or_else(|| DEFAULT_PORT.saturating_add(1));
    args.url = default_url(fallback_port);
    log_line(
        &args.log,
        &format!(
            "port probe missed [{}]；回退无监听端口 {fallback_port}（不导航到非虎符服务）：{}",
            tried.join(", "),
            args.url
        ),
    );
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> std::process::ExitCode {
    let code = unsafe { real_main() };
    std::process::ExitCode::from(code as u8)
}

unsafe fn real_main() -> i32 {
    let exe_dir = exe_dir();
    let mut args = parse_args(&exe_dir);

    if let Some(parent) = args.log.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let user_data = absolutize(&args.user_data);
    if let Err(err) = fs::create_dir_all(&user_data) {
        log_line(
            &args.log,
            &format!("failed to create user data dir {}: {err}", user_data.display()),
        );
        return 2;
    }

    log_line(
        &args.log,
        &format!(
            "=== hufu-settings start (pid {}) ===",
            std::process::id()
        ),
    );
    log_line(
        &args.log,
        &format!("exe dir: {}", exe_dir.display()),
    );
    log_line(
        &args.log,
        &format!("user data dir: {}", user_data.display()),
    );
    // [P0-2c 2026-11-01] 单实例互斥：命名互斥体占名，二次启动（连按
    // Ctrl+Alt+H / 托盘连点）激活已有窗口后直接退出——旧版会在首窗
    // 1.5s 隐身期内再 spawn 隐形进程（验收 D-2/P0-2）。互斥体先于窗口
    // 创建持有（句柄进程存续期间不关，退出由内核回收）；另一实例可能
    // 尚未建窗（FindWindowW 扑空）也无妨——它随后要么正常显示、要么
    // 自己走错误兜底。
    {
        let mutex_name = wide("Local\\Hufu-HufuSettings-SingleInstance");
        let hmutex = CreateMutexW(null_mut(), 0, mutex_name.as_ptr());
        if hmutex != 0 && GetLastError() == ERROR_ALREADY_EXISTS {
            let cls = wide("HufuSettingsWnd");
            let existing = FindWindowW(cls.as_ptr(), null());
            if existing != 0 {
                if IsIconic(existing) != 0 {
                    let _ = ShowWindow(existing, SW_RESTORE);
                }
                let _ = SetForegroundWindow(existing);
                log_line(
                    &args.log,
                    "another instance already running; raised its window and exiting",
                );
            } else {
                log_line(
                    &args.log,
                    "another instance already running (window not found yet); exiting",
                );
            }
            return 0;
        }
    }

    // [P0 白屏修复 2026-11-02] 建窗/导航前把端口定死：快捷方式不再写死
    // 4390（见 resolve_url）。探测只在这个线程、建窗之前跑，总预算 1.5s。
    resolve_url(&mut args);
    // 【--theme】追加到最终 URL（默认与显式 URL 都已含 query，用 & 拼；
    // 防御性判断避免重复追加）。
    if let Some(theme) = args.theme.clone() {
        if !args.url.contains("theme=") {
            let sep = if args.url.contains('?') { '&' } else { '?' };
            args.url = format!("{}{sep}theme={theme}", args.url);
        }
    }
    log_line(&args.log, &format!("target URL: {}", args.url));

    // Per-Monitor V2: the window is sized in DIPs and WebView2 follows the
    // monitor scale. `--dpi` is a dev-only override for high-DPI screenshots.
    let dpi_mode = set_process_dpi_awareness();
    log_line(&args.log, &format!("dpi awareness: {dpi_mode}"));
    let dpi = args.dpi.unwrap_or_else(system_dpi);
    if args.dpi.is_some() {
        std::env::set_var(
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            format!("--force-device-scale-factor={:.4}", dpi as f64 / 96.0),
        );
    }
    let scale = dpi as f64 / 96.0;
    let mut win_w = (1280.0 * scale).round() as i32;
    let mut win_h = (800.0 * scale).round() as i32;
    let wa = work_area().unwrap_or(RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    });
    let wa_w = wa.width().max(320);
    let wa_h = wa.height().max(240);
    let min_w = 720.min(wa_w);
    let min_h = 480.min(wa_h);
    win_w = win_w.clamp(min_w, wa_w);
    win_h = win_h.clamp(min_h, wa_h);
    let win_x = wa.left + (wa_w - win_w) / 2;
    let win_y = wa.top + (wa_h - win_h) / 2;
    log_line(
        &args.log,
        &format!(
            "dpi={dpi} scale={:.3} window={}x{} at {},{} workarea={}x{}",
            scale, win_w, win_h, win_x, win_y, wa_w, wa_h
        ),
    );

    let hr_co = CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED);
    log_line(
        &args.log,
        &format!("CoInitializeEx(APARTMENTTHREADED) -> hr=0x{:08X}", hr_co as u32),
    );

    let create_env = match load_webview2(&exe_dir, &args.log) {
        Ok(f) => f,
        Err(err) => {
            log_line(&args.log, &format!("FATAL: {err}"));
            // [P0-2a] WebView2 运行时/装载器缺失也要让用户看见（旧版静默退）
            let text = wide(&format!(
                "无法加载 WebView2 运行时：\n\n{err}\n\n请安装「Evergreen WebView2 运行时」后重试。"
            ));
            let caption = wide("虎符设置");
            MessageBoxW(0, text.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONERROR);
            return 2;
        }
    };

    let hinstance = GetModuleHandleW(null());
    let bg_brush = CreateSolidBrush(0x00171413);
    let class_name = wide("HufuSettingsWnd");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: wndproc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: 0,
        hCursor: 0,
        hbrBackground: bg_brush,
        lpszMenuName: null(),
        lpszClassName: class_name.as_ptr(),
        hIconSm: 0,
    };
    let atom = RegisterClassExW(&wc);
    if atom == 0 {
        let err = GetLastError();
        if err != 1410 {
            // 1410 = ERROR_CLASS_ALREADY_EXISTS
            log_line(
                &args.log,
                &format!("RegisterClassExW failed; GetLastError={err}"),
            );
            return 2;
        }
    }

    let title = wide("虎符设置");
    let hwnd = CreateWindowExW(
        0,
        class_name.as_ptr(),
        title.as_ptr(),
        WS_POPUP | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_SYSMENU | WS_CLIPCHILDREN,
        win_x,
        win_y,
        win_w,
        win_h,
        0,
        0,
        hinstance,
        null_mut(),
    );
    if hwnd == 0 {
        log_line(
            &args.log,
            &format!("CreateWindowExW failed; GetLastError={}", GetLastError()),
        );
        return 2;
    }

    let app = Box::into_raw(Box::new(App {
        hwnd,
        log_path: args.log.clone(),
        url: wide(&args.url),
        url_text: args.url.clone(),
        user_data: wide_os(&user_data),
        env: null_mut(),
        controller: null_mut(),
        webview: null_mut(),
        nav_ok: false,
        failed: false,
        shown: false,
        error_mode: false,
        error_nav_pending: false,
        focus_retry: 0,
        frame_bg: 0x0017_1413,   // #131417 (dark --bg)
        frame_side: 0x001c_1817, // #17181c (dark --bg2)
    }));
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize);

    // Keep the window hidden until the React shell has navigated/rendered.
    // Showing it immediately is what produced the big blank block before the
    // UI appeared (white before, black after the dark-brush fix).
    apply_window_theme(hwnd, true, None);
    (*app).log("window created (hidden until NavigationCompleted); title=\"虎符设置\"");

    if let Some(secs) = args.auto_close_secs {
        SetTimer(hwnd, 1, secs.saturating_mul(1000), null());
        (*app).log(&format!("auto-close timer armed for {secs}s"));
    }

    let env_handler = handler_new(KIND_ENV, IID_ENV_HANDLER, app);
    let hr_env = create_env(
        null(),
        (*app).user_data.as_ptr(),
        null_mut(),
        env_handler,
    );
    (*app).log(&format!(
        "CreateCoreWebView2EnvironmentWithOptions -> hr=0x{:08X}",
        hr_env as u32
    ));
    // [P0-2b 2026-11-01] 启动超时看护：10s 内窗口未显示（env 挂起/导航
    // 永不完成）即走错误兜底。win:ready / 首帧 timer 揭示时解除。
    SetTimer(hwnd, 3, 10_000, null());
    (*app).log("startup watchdog armed (10s)");
    if hr_env < 0 {
        // [P0-2a] 环境 async 创建失败同样有 UI（旧版静默退出）
        (*app).failed = true;
        let text = wide(&format!(
            "无法创建 WebView2 环境（hr=0x{:08X}）。\n\n请确认已安装 WebView2 运行时后重试。",
            hr_env as u32
        ));
        let caption = wide("虎符设置");
        MessageBoxW(hwnd, text.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONERROR);
        DestroyWindow(hwnd);
    }

    let mut msg: MSG = std::mem::zeroed();
    loop {
        let ret = GetMessageW(&mut msg, 0, 0, 0);
        if ret == 0 {
            break;
        }
        if ret == -1 {
            (*app).log("GetMessageW returned -1");
            (*app).failed = true;
            break;
        }
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    (*app).log(&format!(
        "message loop exited; nav_ok={} failed={}",
        (*app).nav_ok, (*app).failed
    ));

    CoUninitialize();

    if (*app).failed {
        2
    } else {
        0
    }
}

fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}
