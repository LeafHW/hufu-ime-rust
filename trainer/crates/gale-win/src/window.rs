//! 主窗口 + DWM 材质三态：纯色 / 磨砂(Acrylic) / 毛玻璃(Blur)。
//!
//! 材质实现（对照 _research/web-survey.md 结论）：
//! - Win11 22H2+：`DwmSetWindowAttribute(DWMWA_SYSTEMBACKDROP_TYPE)`
//!   - Acrylic → DWMSBT_TRANSIENTWINDOW (3)
//!   - Blur(BgBlur) → DWMSBT_MAINWINDOW (1)（Mica 属"毛玻璃系"观感）
//! - Win10/早期 Win11：undocumented `SetWindowCompositionAttribute`
//!   ACCENT_ENABLE_ACRYLICBLURBEHIND(4) / ACCENT_ENABLE_BLURBEHIND(3)
//! - 纯色：不启用任何 backdrop，D2D 层直接填充不透明底色
//!
//! 窗口层：Win32 注册 + 创建 + 消息循环（帧驱动渲染在 d2d.rs；本模块只管窗口与材质）。
//! 跟打器主窗口不是 RichEdit——RichEdit 是打字热区的子控件（input_host.rs）；
//! 主窗口本身是普通 HWND 层级，用于承载 D2D 交换链与 DWM 材质。

use gale_core::config::Material;
use std::sync::Mutex;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmEnableBlurBehindWindow, DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE, DWM_BB_BLURREGION,
    DWM_BB_ENABLE, DWM_BLURBEHIND,
};
use windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject, HRGN};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetCursorInfo, GetCursorPos,
    GetMessageW,
    KillTimer,
    LoadCursorW, PeekMessageW, PostQuitMessage, RegisterClassW, SetCursor, SetForegroundWindow,
    SetTimer, SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, HTBOTTOM,
    HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCLIENT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
    CURSORINFO, CURSOR_SHOWING, HWND_NOTOPMOST, HWND_TOPMOST, IDC_ARROW, IDC_SIZENS, MSG, PM_REMOVE,
    SW_RESTORE, SW_SHOW,
    ShowCursor, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WINDOW_EX_STYLE, WM_COMMAND, WM_DESTROY, WM_DPICHANGED,
    WM_ERASEBKGND, WM_GETMINMAXINFO, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_NCCALCSIZE, WM_NCHITTEST, WM_PAINT, WM_QUIT, WM_RBUTTONDOWN, WM_SETFOCUS, WM_SETCURSOR,
    WM_SIZE, WM_TIMER, WNDCLASSW, WS_OVERLAPPEDWINDOW, HCURSOR,
};
use windows::Win32::UI::WindowsAndMessaging::{SM_CXPADDEDBORDER, SM_CXSIZEFRAME};
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TRACKMOUSEEVENT, TME_LEAVE};

// ── DPI 感知（150% 缩放模糊根治，2026-10-08）──────────────────────────
// gale 此前是 DPI-unaware 进程：窗口按 96-DPI 逻辑像素渲染，DWM 把整张
// 位图拉伸 1.5x 贴屏 → 全界面模糊。修法 = 进程声明 Per-Monitor V2 感知：
// - 渲染按物理像素（交换链 = GetClientRect 物理尺寸，矢量文字原生物理光栅化 → 清晰）
// - UI 布局层保持 96-DPI 逻辑像素坐标系不动（ui.rs/modals.rs 常量与
//   cfg.font_size 全是逻辑值），由 D2D 全局 SetTransform(scale) 一步放大
// - 鼠标物理坐标 ÷ scale = 逻辑坐标再喂命中测试
// scale 存全局原子量（建窗时算一次；WM_DPICHANGED 更新）。

/// 当前 DPI 缩放系数（物理 px ÷ 逻辑 px；96 DPI = 1.0）
static DPI_SCALE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x0100_0000);

fn store_dpi_scale(scale: f32) {
    // f32 位模式存 u32（1.0 = 0x3F800000）；读回 f32::from_bits
    DPI_SCALE.store(scale.to_bits(), std::sync::atomic::Ordering::Release);
}

/// 进程声明 Per-Monitor V2 DPI 感知（必须在任何窗口创建之前调用）。
/// 失败（旧 Win10 1703 以前）退回 SetProcessDPIAware（系统感知，仍清晰，
/// 只是跨屏拖动不实时重缩放——跟打器场景可接受）。
pub fn enable_per_monitor_dpi_awareness() -> bool {
    // GALE_NO_DPI=1：诊断开关（分叉定位 DPI 相关崩溃时用）
    if std::env::var("GALE_NO_DPI").as_deref() == Ok("1") {
        return false;
    }
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_ok() {
            return true;
        }
        // 兜底：系统感知（Vista 时代 API，任何版本都有）
        windows::Win32::UI::WindowsAndMessaging::SetProcessDPIAware().as_bool()
    }
}

/// 读取窗口当前 DPI → 更新全局 scale（建窗后 / WM_DPICHANGED 时调用）
pub fn refresh_window_dpi(hwnd: HWND) -> f32 {
    let dpi = unsafe {
        let d = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd);
        if d == 0 {
            96
        } else {
            d
        }
    };
    let scale = dpi as f32 / 96.0;
    store_dpi_scale(scale);
    scale
}

/// 当前 DPI 缩放系数（物理 px ÷ 逻辑 px）
pub fn dpi_scale() -> f32 {
    let bits = DPI_SCALE.load(std::sync::atomic::Ordering::Acquire);
    let s = f32::from_bits(bits);
    if s.is_finite() && s > 0.05 && s < 20.0 {
        s
    } else {
        1.0
    }
}

/// DPI 看门狗（主循环每帧调用，内部节流 1s）：对账全局 scale 与窗口真实
/// DPI。伪造 0x2E0 消息可能骗过校验污染 scale（实测被改成 2.71 → 拖动锚点
/// 错乱、窗口自动变大）——本函数保证最多 1 秒内自愈回真值。
pub fn dpi_watchdog(hwnd: HWND) {
    use std::time::Instant;
    static LAST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
    if let Ok(mut g) = LAST.lock() {
        let now = Instant::now();
        let due = g
            .map(|t| now.duration_since(t).as_secs() >= 1)
            .unwrap_or(true);
        if !due {
            return;
        }
        *g = Some(now);
    }
    let dpi = unsafe {
        let d = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd);
        if d == 0 {
            96
        } else {
            d
        }
    };
    let truth = dpi as f32 / 96.0;
    let cur = dpi_scale();
    if (truth - cur).abs() > 0.01 {
        if std::env::var("GALE_VERBOSE").as_deref() == Ok("1") {
            eprintln!("[dpi] 看门狗纠偏 {cur:.2} → {truth:.2}");
        }
        store_dpi_scale(truth);
        REQUEST_FRAME.store(true, std::sync::atomic::Ordering::Release);
    }
}

/// 物理 px → 逻辑 px（鼠标命中测试用）
pub fn phys_to_logical(v: i32) -> f32 {
    v as f32 / dpi_scale()
}

// Native WM_CLOSE destroys the HWND before the app exits its loop. Capture the
// normal rectangle first, also when closing a minimized/maximized window.
static NORMAL_BOUNDS: Mutex<Option<(i32, i32, i32, i32)>> = Mutex::new(None);

unsafe fn normal_window_bounds(hwnd: HWND) -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowPlacement, WINDOWPLACEMENT};
    let mut p = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
    GetWindowPlacement(hwnd, &mut p).ok()?;
    let r = p.rcNormalPosition;
    let mut monitor = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let handle = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    if !GetMonitorInfoW(handle, &mut monitor).as_bool() { return None; }
    // WINDOWPLACEMENT uses workspace coordinates for normal application windows.
    let x = r.left + monitor.rcWork.left - monitor.rcMonitor.left;
    let y = r.top + monitor.rcWork.top - monitor.rcMonitor.top;
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    (w > 0 && h > 0).then_some((x, y, w, h))
}

fn clamp_restored_bounds(x: i32, y: i32, w: i32, h: i32, work: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
    let (left, top, right, bottom) = work;
    let w = w.max(1).min((right - left).max(1));
    let h = h.max(1).min((bottom - top).max(1));
    (x.clamp(left, right.saturating_sub(w).max(left)), y.clamp(top, bottom.saturating_sub(h).max(top)), w, h)
}

#[cfg(test)]
mod placement_tests {
    use super::*;
    #[test]
    fn secondary_monitor_keeps_negative_coordinates_and_fits_its_work_area() {
        assert_eq!(clamp_restored_bounds(-1800, 70, 1280, 800, (-1920, 40, 0, 1080)), (-1800,70,1280,800));
        assert_eq!(clamp_restored_bounds(2500, 90, 1600, 1000, (1920, 0, 3840, 1440)), (2240,90,1600,1000));
    }
    #[test]
    fn removed_monitor_and_small_screen_keep_whole_restored_window_visible() {
        assert_eq!(clamp_restored_bounds(-1800, -32000, 1280, 800, (0, 40, 1024, 768)), (0,40,1024,728));
        assert_eq!(clamp_restored_bounds(9000, 9000, 960, 630, (0, 0, 1920, 1080)), (960,450,960,630));
    }
}

// ── 鼠标事件队列（界面组消费；与 input_host::CMD_QUEUE 同款静态队列）──

/// 主窗口客户区鼠标事件（物理像素；Wheel 的 x/y 为光标位置）
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MouseEv {
    Move { x: i32, y: i32 },
    /// 指针离开客户区（TrackMouseEvent/TME_LEAVE）：恢复光标显示。
    Leave,
    LDown { x: i32, y: i32 },
    LUp { x: i32, y: i32 },
    /// 右键按下：跟打器在窗口内自绘上下文菜单（不是系统菜单）。
    RDown { x: i32, y: i32 },
    Wheel { delta: i32, x: i32, y: i32 },
}

static MOUSE_QUEUE: Mutex<Vec<MouseEv>> = Mutex::new(Vec::new());

/// 光标提示（2026-11-12）：应用层每帧根据命中层设置，wndproc 在
/// WM_SETCURSOR 里据此换系统光标——对照模式的分栏把手要有上下调整光标。
/// 【2026-11-13】不再提供四向箭头：窗口任意处可拖动，但指针保持普通箭头。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorHint {
    Arrow,
    /// 分栏/成绩面板把手（上下调整）
    ResizeNS,
}

static CURSOR_HINT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set_cursor_hint(hint: CursorHint) {
    let v = match hint {
        CursorHint::Arrow => 0u8,
        CursorHint::ResizeNS => 2u8,
    };
    CURSOR_HINT.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// WM_MOUSELEAVE 固定值（不同 windows crate 模块归类不一，按消息号使用）。
const WM_MOUSELEAVE_MSG: u32 = 0x02A3;

static TRACKING_MOUSE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 【鼠标自动隐藏 2026-11-13】键入时隐藏光标、动鼠标即恢复。
/// 用「WM_SETCURSOR 里 SetCursor(NULL)」实现，不碰 ShowCursor 的全局计数。
static CURSOR_HIDDEN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 【鼠标自动隐藏 2026-11-13】光标屏幕坐标：不依赖鼠标消息（指针常悬在
/// RichEdit 子窗上，主窗口收不到 WM_MOUSEMOVE，无法靠事件知道位置）。
pub fn cursor_screen_pos() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

/// 光标是否落在主窗口客户区内（含打字热区这类子窗覆盖区域）。
pub fn cursor_inside_client(hwnd: HWND) -> bool {
    if hwnd.0.is_null() {
        return false;
    }
    let (sx, sy) = cursor_screen_pos();
    let mut p = POINT { x: sx, y: sy };
    let mut rc = windows::Win32::Foundation::RECT::default();
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut p);
        let _ = GetClientRect(hwnd, &mut rc);
    }
    p.x >= 0 && p.y >= 0 && p.x < rc.right && p.y < rc.bottom
}

/// 【鼠标隐藏 2026-11-13】不做「状态相同就跳过」的短路：输入法候选窗/子控件
/// 可能在隐藏期间调用 ShowCursor(TRUE) 把光标重新显示出来（表现为「按一下消失、
/// 再按一下又出来」）。这里每次调用都重新压计数，主循环在想要隐藏期间每帧调用，
/// 外部谁抬回来都会被下一次调用重新压下去。
pub fn set_cursor_hidden(hide: bool) {
    CURSOR_HIDDEN.store(hide, std::sync::atomic::Ordering::Relaxed);
    unsafe {
        if hide {
            // 只在「系统认为光标可见」时压一次计数：每敲一个键都会调用本函数，
            // 无条件 while 循环会把显示计数越压越低（-1 → -2 → …），恢复时虽能
            // 补回来，但计数漂移期间任何外部 ShowCursor(TRUE) 都可能露出光标。
            if cursor_visible() {
                while ShowCursor(false) >= 0 {}
            }
            let _ = SetCursor(HCURSOR(core::ptr::null_mut()));
        } else {
            while ShowCursor(true) < 0 {}
            if let Ok(cursor) = LoadCursorW(None, IDC_ARROW) {
                SetCursor(cursor);
            }
        }
    }
}

/// 系统真正的光标显示状态（ShowCursor 显示计数 >=0 且未被隐藏时 true）。
fn cursor_visible() -> bool {
    let mut info = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        if GetCursorInfo(&mut info).is_err() {
            return true;
        }
    }
    info.flags.0 & CURSOR_SHOWING.0 != 0
}

/// 【鼠标隐藏 2026-11-13】每帧调用的守门员：只有「真的又被显示出来」时才再压一次，
/// 避免 ShowCursor 计数无限漂移；同时把光标按成空。
pub fn enforce_cursor_hidden() {
    if !CURSOR_HIDDEN.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    if cursor_visible() {
        set_cursor_hidden(true);
    } else {
        unsafe {
            let _ = SetCursor(HCURSOR(core::ptr::null_mut()));
        }
    }
}

/// 子控件（RichEdit 打字热区）的 WM_SETCURSOR 里调用：隐藏期一律压成空光标。
pub fn hide_cursor_in_wm_setcursor() -> bool {
    if !CURSOR_HIDDEN.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    unsafe {
        let _ = SetCursor(HCURSOR(core::ptr::null_mut()));
    }
    true
}

#[allow(dead_code)]
pub fn cursor_hidden() -> bool {
    CURSOR_HIDDEN.load(std::sync::atomic::Ordering::Relaxed)
}

/// 队列上限（防爆内存；主循环每帧 drain，正常远达不到）
const MOUSE_QUEUE_MAX: usize = 256;

fn push_mouse(ev: MouseEv) {
    if let Ok(mut q) = MOUSE_QUEUE.lock() {
        if q.len() < MOUSE_QUEUE_MAX {
            q.push(ev);
        } else {
            // 满：丢最旧的一半（Move 类事件可丢；LDown/LUp 靠前保留）
            let keep = q.len() / 2;
            q.drain(..keep);
            q.push(ev);
        }
    }
}

/// 取走累积的鼠标事件（主循环每帧调用）
pub fn drain_mouse() -> Vec<MouseEv> {
    match MOUSE_QUEUE.lock() {
        Ok(mut q) => {
            let taken = q.clone();
            q.clear();
            taken
        }
        Err(_) => Vec::new(),
    }
}

/// lParam 低 16 位（有符号）
fn lo16(v: u32) -> u16 {
    (v & 0xFFFF) as u16
}

/// lParam 高 16 位（有符号）
fn hi16(v: u32) -> u16 {
    (v >> 16) as u16
}

// STA/OLE lifetime and the standard TSF message/keystroke pump.
thread_local! {static TSF_PUMP:std::cell::RefCell<Option<NativeTsfPump>>=const{std::cell::RefCell::new(None)};}
struct NativeTsfPump {
    manager: windows::Win32::UI::TextServices::ITfThreadMgr,
    pump: windows::Win32::UI::TextServices::ITfMessagePump,
    keys: windows::Win32::UI::TextServices::ITfKeystrokeMgr,
}
/// The real focused RichEdit context, never an application-created/fabricated
/// text store. Clone the manager before COM calls, which may reenter the pump.
pub(crate) fn focused_tsf_context(
    hwnd: HWND,
) -> Option<windows::Win32::UI::TextServices::ITfContext> {
    let manager = TSF_PUMP.with(|v| v.borrow().as_ref().map(|p| p.manager.clone()))?;
    unsafe {
        let context = manager.GetFocus().ok()?.GetTop().ok()?;
        if context.GetActiveView().ok()?.GetWnd().ok()? == hwnd {
            Some(context)
        } else {
            None
        }
    }
}
pub struct ComSta;
impl ComSta {
    pub fn new() -> windows::core::Result<Self> {
        unsafe {
            OleInitialize(None)?;
            use windows::core::Interface;
            use windows::Win32::{
                System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
                UI::TextServices::*,
            };
            let manager: ITfThreadMgr =
                CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)?;
            manager.Activate()?;
            let pump = manager.cast::<ITfMessagePump>()?;
            let keys = manager.cast::<ITfKeystrokeMgr>()?;
            TSF_PUMP.with(|v| {
                *v.borrow_mut() = Some(NativeTsfPump {
                    manager,
                    pump,
                    keys,
                })
            });
            Ok(Self)
        }
    }
}
impl Drop for ComSta {
    fn drop(&mut self) {
        TSF_PUMP.with(|v| {
            if let Some(p) = v.borrow_mut().take() {
                unsafe {
                    let _ = p.manager.Deactivate();
                }
            }
        });
        unsafe {
            windows::Win32::System::Ole::OleUninitialize();
        }
    }
}
fn tsf_peek(msg: &mut MSG) -> bool {
    TSF_PUMP.with(|v| {
        if let Some(p) = v.borrow().as_ref() {
            let mut available = windows::Win32::Foundation::BOOL(0);
            if unsafe {
                p.pump
                    .PeekMessageW(msg, None, 0, 0, PM_REMOVE.0, &mut available)
            }
            .is_ok()
            {
                return available.as_bool();
            }
        }
        unsafe { PeekMessageW(msg, None, 0, 0, PM_REMOVE).as_bool() }
    })
}
fn tsf_filter(msg: &MSG) -> bool {
    // VK_PACKET is already Unicode input (touch keyboard/accessibility/SendInput).
    // Preserve TranslateMessage/WM_CHAR instead of offering it as a physical key.
    if msg.wParam.0 == 0xE7 {
        return false;
    }
    use windows::Win32::UI::WindowsAndMessaging::{
        WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };
    TSF_PUMP.with(|v| {
        let b = v.borrow();
        let Some(p) = b.as_ref() else {
            return false;
        };
        unsafe {
            if msg.message == WM_KEYDOWN
                && msg.hwnd.0 as isize == crate::input_host::relay_hwnd()
                && crate::input_host::diagnostics_enabled()
            {
                let layout = windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout(0);
                use windows::core::Interface;
                use windows::Win32::UI::TextServices::*;
                if let Ok(profiles) =
                    windows::Win32::System::Com::CoCreateInstance::<_, ITfInputProcessorProfileMgr>(
                        &CLSID_TF_InputProcessorProfiles,
                        None,
                        windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
                    )
                {
                    let mut profile = TF_INPUTPROCESSORPROFILE::default();
                    let result = profiles.GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &mut profile);
                    crate::input_host::diag_log(&format!(
                        "TSF active_profile={profile:?} result={result:?}"
                    ));
                }
                if let Ok(compartments) = p.manager.cast::<ITfCompartmentMgr>() {
                    for (name, key) in [
                        ("open", GUID_COMPARTMENT_KEYBOARD_OPENCLOSE),
                        ("conversion", GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION),
                        ("disabled", GUID_COMPARTMENT_KEYBOARD_DISABLED),
                    ] {
                        let value = compartments.GetCompartment(&key).and_then(|c| c.GetValue());
                        crate::input_host::diag_log(&format!("TSF compartment {name}={value:?}"));
                    }
                }

                crate::input_host::diag_log(&format!(
                    "TSF physical vk={:#x} hkl={:x} focus_doc={} thread_focus={}",
                    msg.wParam.0,
                    layout.0 as usize,
                    p.manager.GetFocus().is_ok(),
                    p.manager
                        .IsThreadFocus()
                        .map(|b| b.as_bool())
                        .unwrap_or(false)
                ));
            }
            match msg.message {
                WM_KEYDOWN | WM_SYSKEYDOWN => p
                    .keys
                    .TestKeyDown(msg.wParam, msg.lParam)
                    .ok()
                    .filter(|b| b.as_bool())
                    .and_then(|_| p.keys.KeyDown(msg.wParam, msg.lParam).ok())
                    .is_some_and(|b| b.as_bool()),
                WM_KEYUP | WM_SYSKEYUP => p
                    .keys
                    .TestKeyUp(msg.wParam, msg.lParam)
                    .ok()
                    .filter(|b| b.as_bool())
                    .and_then(|_| p.keys.KeyUp(msg.wParam, msg.lParam).ok())
                    .is_some_and(|b| b.as_bool()),
                _ => false,
            }
        }
    })
}

// ── 单实例互斥 ──────────────────────────────────────────────────────
// 2026-10-02 实锤：用户连开 3 个实例（三个 hwnd 相继 enable 互零键击），
// relocate 副本与再双击的 exe 同时在跑 → 输入焦点被抢、键击进不了任何
// 窗口 → "打不了中文"。命名互斥体全局唯一（Session namespace），
// 后来者只负责把已有窗口唤醒到前台。

/// 单实例检查：拿到互斥体返回 None（我是唯一实例）；已有实例则唤醒
/// 其主窗口并返回 Some(hwnd)。唤醒失败（极旧实例无窗口）不阻塞——
/// 仍返回 Some(0) 让后来者退出（避免再开一个抢焦点的实例）。
static INSTANCE_MUTEX: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
pub fn activate_existing_instance() -> Option<isize> {
    use windows::core::w;
    use windows::Win32::Foundation::WAIT_EVENT;
    use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
    unsafe {
        let profile = std::env::var_os("USERPROFILE").unwrap_or_default();
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        profile.hash(&mut hash);
        let name: Vec<u16> = format!("Local\\HuFuTrainer-{:x}\0", hash.finish())
            .encode_utf16()
            .collect();
        let handle = match CreateMutexW(None, false, windows::core::PCWSTR(name.as_ptr())) {
            Ok(h) => h,
            Err(_) => return Some(0),
        };
        // 申请所有权探测：立即成功=无主（我是第一个）；WAIT_ABANDONED
        // （旧实例僵死）也视为无主接管，避免永远打不开。
        let wait: WAIT_EVENT = WaitForSingleObject(handle, 0);
        if wait.0 == 0 /*WAIT_OBJECT_0*/ || wait.0 == 128
        /*WAIT_ABANDONED*/
        {
            // 唯一实例：句柄持有到进程退出（std::mem::forget 防 CloseHandle；
            // OS 在进程结束时自动回收互斥体）
            INSTANCE_MUTEX.store(handle.0 as isize, std::sync::atomic::Ordering::Relaxed);
            return None;
        }
        // 已有实例：找它的主窗口唤醒
        let class = w!("HuFuTrainerWindow");
        let found = windows::Win32::UI::WindowsAndMessaging::FindWindowExW(
            HWND(std::ptr::null_mut()),
            HWND(std::ptr::null_mut()),
            class,
            None,
        )
        .map(|h| h.0 as isize)
        .unwrap_or(0);
        if found != 0 {
            let h = HWND(found as *mut _);
            let _ = ShowWindow(h, SW_RESTORE);
            let _ = SetForegroundWindow(h);
        }
        Some(found)
    }
}

/// ACCENT_* 枚举（SetWindowCompositionAttribute 用；磨砂=ACRYLICBLURBEHIND，毛玻璃=BLURBEHIND）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccentFlags {
    Disable = 0,
    EnableGradient = 1,
    EnableTransparentGradient = 2,
    EnableBlurBehind = 3,
    EnableAcrylicBlurBehind = 4,
    InvalidState = 5,
}

/// DWM 材质应用参数
#[derive(Debug, Clone)]
pub struct MaterialParams {
    pub hwnd: isize,
    pub material: Material,
    /// 背景色（RGB）+ 不透明度
    pub tint_rgb: (u8, u8, u8),
    pub opacity: u8,
}

impl MaterialParams {
    pub fn accent(&self) -> AccentFlags {
        // 晴跟打 parity（2026-10 用户验收后重裁）：晴跟打无任何系统毛玻璃——
        // 透明=纯 alpha 透出（WPF AllowsTransparency 同构）。gale 对齐：
        // - 透明(Blur)：backdrop None + SWCA BLURBEHIND=3（真采窗后内容模糊，
        //   无 luminosity 提白层——Win11 上实测仍生效）
        // - 磨砂(Acrylic)：SWCA ACRYLICBLURBEHIND=4 + 用户不透明度进 tint
        //   （不再用 DWMSBT_TRANSIENTWINDOW——系统 Acrylic 自带提白层=泛白根因）
        match self.material {
            Material::Solid => AccentFlags::Disable,
            Material::Acrylic => AccentFlags::EnableAcrylicBlurBehind,
            Material::Blur => AccentFlags::EnableBlurBehind,
        }
    }

    /// AABBGGRR 格式的 tint 色。磨砂/透明都用 tint：
    /// - 磨砂：alpha = 用户不透明度直映射（0-255），晴跟打 #AARRGGBB 同语义
    /// - 透明：alpha 固定 0（纯模糊，不加底色——用户要"看得清后面"）
    pub fn tint_abgr(&self) -> u32 {
        let (r, g, b) = self.tint_rgb;
        let a = match self.material {
            Material::Acrylic => (self.opacity as f32 / 100.0 * 255.0) as u32,
            Material::Blur => 0,
            Material::Solid => self.opacity as u32,
        };
        (a as u32) << 24 | (b as u32) << 16 | (g as u32) << 8 | r as u32
    }
}

/// ACCENT_POLICY（undocumented API 的 ABI；Win10 路径）
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AccentPolicy {
    pub accent_state: u32,
    pub accent_flags: u32,
    pub gradient_color: u32,
    pub animation_id: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct WindowCompositionAttribData {
    attribute: u32,
    data: *mut core::ffi::c_void,
    size: usize,
}

const WCA_ACCENT_POLICY: u32 = 19;

/// DWMWA_SYSTEMBACKDROP_TYPE 取值
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backdrop {
    Auto = 0,
    None = 1,
    MainWindow = 2,      // Mica
    TransientWindow = 3, // Acrylic
    TabbedWindow = 4,    // Mica Alt
}

/// 应用材质。晴跟打 parity 重裁（2026-10）：
/// 晴跟打开源实现（TypeSunny MainWindow）= WPF AllowsTransparency + 纯 alpha 背景，
/// **无任何系统 Acrylic/Mica backdrop**——系统 backdrop 自带 luminosity 提白层
/// （Acrylic=Mica 同源），这正是"磨砂泛白""透明完全不透"的根因：
/// - 泛白：DWMSBT_TRANSIENTWINDOW 系统 Acrylic 强制叠白色 luminosity 层
/// - 不透：DWMSBT_MAINWINDOW(Mica) 只采样壁纸、完全不透窗口后方内容
/// 对齐方案（全部弃用 DWMWA_SYSTEMBACKDROP_TYPE，恒置 None）：
/// - 透明 = SWCA ACCENT_ENABLE_BLURBEHIND(3) + tint alpha 0（真采窗后模糊，无提白）
/// - 磨砂 = SWCA ACCENT_ENABLE_ACRYLICBLURBEHIND(4) + 用户不透明度进 tint
/// - 纯色 = SWCA Disable + D2D 不透明底
/// 所有非 Solid 材质都保留空区域 blur-behind（winit 同款）——普通 HWND 的
/// redirection surface 不带 alpha 通道，没有它 DWM 把透明像素合成成黑色。
pub fn apply_material(p: &MaterialParams) -> bool {
    let hwnd = HWND(p.hwnd as *mut core::ffi::c_void);
    unsafe {
        let mut ok = true;
        // 恒清系统 backdrop（防残留：曾设过 TRANSIENTWINDOW/MAINWINDOW 的话
        // 退回 None，否则旧 backdrop 会继续叠加提白/挡透）
        let none = Backdrop::None as u32;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &none as *const u32 as *const core::ffi::c_void,
            4,
        );
        if matches!(p.material, Material::Solid) {
            // 纯色：关 blur-behind（回默认 redirection），SWCA Disable
            let bb = DWM_BLURBEHIND {
                dwFlags: DWM_BB_ENABLE,
                fEnable: false.into(),
                hRgnBlur: HRGN::default(),
                fTransitionOnMaximized: false.into(),
            };
            let _ = DwmEnableBlurBehindWindow(hwnd, &bb);
        } else if std::env::var_os("GALE_NO_BB").is_some() {
            // 诊断开关：完全跳过 blur-behind——NOREDIRECTIONBITMAP + PREMULTIPLIED
            // 交换链直通 DWM（Chrome/VSCode 无边框透明路径）
            let bb = DWM_BLURBEHIND {
                dwFlags: DWM_BB_ENABLE,
                fEnable: false.into(),
                hRgnBlur: HRGN::default(),
                fTransitionOnMaximized: false.into(),
            };
            let _ = DwmEnableBlurBehindWindow(hwnd, &bb);
        } else {
            // 透明/磨砂：空区域 blur-behind（尊重像素 alpha + 进 DWM 合成路径）
            let region = CreateRectRgn(0, 0, -1, -1);
            let bb = DWM_BLURBEHIND {
                dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION,
                fEnable: true.into(),
                hRgnBlur: region,
                fTransitionOnMaximized: false.into(),
            };
            ok = DwmEnableBlurBehindWindow(hwnd, &bb).is_ok();
            let _ = DeleteObject(region);
        }
        // SWCA（undocumented，静态导入失败时整个 EXE 起不来，所以 GetProcAddress 动态解析）
        if let Some(f) = load_set_window_composition_attribute() {
            let mut policy = AccentPolicy {
                accent_state: p.accent() as u32,
                accent_flags: 0,
                gradient_color: p.tint_abgr(),
                animation_id: 0,
            };
            let mut data = WindowCompositionAttribData {
                attribute: WCA_ACCENT_POLICY,
                data: &mut policy as *mut AccentPolicy as *mut core::ffi::c_void,
                size: std::mem::size_of::<AccentPolicy>(),
            };
            ok &= f(hwnd, &mut data) != 0;
        }
        ok
    }
}

type SetCompositionFn = unsafe extern "system" fn(HWND, *mut WindowCompositionAttribData) -> i32;

/// 动态解析 SetWindowCompositionAttribute（Win10 有，Win11 部分版本保留）
fn load_set_window_composition_attribute() -> Option<SetCompositionFn> {
    unsafe {
        let user32 =
            windows::Win32::System::LibraryLoader::GetModuleHandleW(w!("user32.dll")).ok()?;
        let addr = windows::Win32::System::LibraryLoader::GetProcAddress(
            user32,
            windows::core::s!("SetWindowCompositionAttribute"),
        )?;
        Some(std::mem::transmute(addr))
    }
}

/// 主窗口句柄（单例）
pub struct MainWindow {
    pub hwnd: isize,
}

// ── 拖动尺寸同步补帧（防黑屏）──
// 拖边框期间系统进模态尺寸循环，主循环不跑帧，新暴露区域无内容 → 黑。
// WM_SIZE 时同步用主题底色填一帧（渲染器地址由 main 注册于此）。
static SIZE_FILL_RENDERER: Mutex<Option<usize>> = Mutex::new(None);

/// 注册渲染器地址（main 创建 Renderer 后调用；拖动 WM_SIZE 时同步补帧用）
pub fn register_size_fill_renderer(r: *mut crate::d2d::Renderer) {
    if let Ok(mut slot) = SIZE_FILL_RENDERER.lock() {
        *slot = Some(r as usize);
    }
}

pub fn unregister_size_fill_renderer() {
    if let Ok(mut slot) = SIZE_FILL_RENDERER.lock() {
        *slot = None;
    }
}

fn size_fill_once(w: i32, h: i32) {
    if let Ok(slot) = SIZE_FILL_RENDERER.lock() {
        if let Some(addr) = *slot {
            let r = unsafe { &mut *(addr as *mut crate::d2d::Renderer) };
            r.on_window_size_sync_fill(w, h);
        }
    }
}

// ── 真帧重建回调（WM_SIZE 补帧升级版）──
// 回调由 gale-app 注册：按新尺寸重建完整 UI 帧（文字保持原字号、布局重排），
// 比纯色/位图重放更接近"放大缩小"的直觉。回调只在主线程 wndproc 调用
// （与主循环同线程、不同时活跃）；static 需要 Sync，包一层手动 Send 壳。
struct RebuildFnHolder(RebuildFn);
struct RebuildFn(Box<dyn Fn(i32, i32)>);
unsafe impl Send for RebuildFnHolder {}
static SIZE_REBUILD: Mutex<Option<RebuildFnHolder>> = Mutex::new(None);

pub fn set_size_rebuild_callback(f: Box<dyn Fn(i32, i32)>) {
    if let Ok(mut slot) = SIZE_REBUILD.lock() {
        *slot = Some(RebuildFnHolder(RebuildFn(f)));
    }
}

pub fn clear_size_rebuild_callback() {
    if let Ok(mut slot) = SIZE_REBUILD.lock() {
        *slot = None;
    }
}

// ── 焦点归还开关（模态面板期暂停 relay）──
// 模态打开时主窗口需要持有焦点（面板上的 Esc/F 键按模态语义处理，不进 RichEdit）；
// 关闭模态时恢复自动归还。默认 true（跟打常态：焦点永远在 RichEdit 宿主）。
// （历史 imm32 后端注释随模块删除；现输入层恒为 RichEdit 宿主。）
static FOCUS_RELAY_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

/// 设置 WM_SETFOCUS→RichEdit 自动归还是否生效（模态面板开=关掉自动归还）
pub fn set_focus_relay(enabled: bool) {
    FOCUS_RELAY_ENABLED.store(enabled, std::sync::atomic::Ordering::Release);
}

pub fn focus_relay_enabled() -> bool {
    FOCUS_RELAY_ENABLED.load(std::sync::atomic::Ordering::Acquire)
}

/// GDI 面清黑（按 hwnd；白条根治，2026-10-02）：
/// DComp topmost 视觉之下是窗口的 GDI redirection surface，本程序全部
/// UI 画在交换链、GDI 面无人管理，而 Windows 分配/扩展它时不保证干净
/// （启动残迹=用户看到的 178x19 白条；WM_SIZE 扩展同理）。透明交换链
/// 会把这块残迹透出来，D2D Clear 擦不掉（不同合成层）。PatBlt BLACKNESS
/// 整面刷黑一次即可（黑垫在透明区后=不可见）；无任何代码再画 GDI，一次即净。
unsafe fn gdi_scrub_hwnd(hwnd: HWND) {
    // WS_EX_NOREDIRECTIONBITMAP 窗口没有 GDI redirection surface，
    // 无面可擦（GetDC 给的是屏幕 DC 兼容面，PatBlt 无处落地）——直接跳过。
    let ex = windows::Win32::UI::WindowsAndMessaging::GetWindowLongW(
        hwnd,
        windows::Win32::UI::WindowsAndMessaging::GWL_EXSTYLE,
    );
    if ex & 0x0020_0000 != 0 {
        return; // WS_EX_NOREDIRECTIONBITMAP
    }
    let dc = windows::Win32::Graphics::Gdi::GetDC(hwnd);
    if dc.is_invalid() {
        return;
    }
    let mut rc = windows::Win32::Foundation::RECT::default();
    let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rc);
    let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
    if w > 0 && h > 0 {
        // BLACKNESS rop=0x42：整面刷黑，不依赖刷子选择
        let _ = windows::Win32::Graphics::Gdi::PatBlt(
            dc,
            0,
            0,
            w,
            h,
            windows::Win32::Graphics::Gdi::ROP_CODE(0x00000042),
        );
    }
    let _ = windows::Win32::Graphics::Gdi::ReleaseDC(hwnd, dc);
}

/// 诊断开关：GALE_WNDPROC_MIN=1 时 main_proc 只保留 IME 钩子 +
/// WM_DESTROY/WM_TIMER，其余消息全部直落 DefWindowProc（见 main_proc 注释）
fn wndproc_min() -> bool {
    static MIN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let on = std::env::var("GALE_WNDPROC_MIN").as_deref() == Ok("1");
        MIN.store(on, std::sync::atomic::Ordering::Relaxed);
    });
    MIN.load(std::sync::atomic::Ordering::Relaxed)
}

/// 主窗口过程：纯容器 —— 渲染承载 + 焦点归还 + 尺寸/销毁。
///
/// 输入层固定为 RichEdit 宿主（input_host）；自建 HIMC 的 ime_imm 后端
/// 已删除（probe-imm REPORT 证伪：CUAS 桥拆不掉、自建上下文挂不稳）。
unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == windows::Win32::UI::WindowsAndMessaging::WM_ACTIVATE && wp.0 & 0xffff == 0 {
        // Preserve focus loss relative to keys, including native reactivation
        // inside the same message-pump iteration.
        crate::input_host::note_focus_lost();
        // 失焦时无论如何恢复光标（防止隐藏态卡住）。
        set_cursor_hidden(false);
    }
    if msg == windows::Win32::UI::WindowsAndMessaging::WM_CLOSE {
        *NORMAL_BOUNDS.lock().unwrap_or_else(|e| e.into_inner()) = normal_window_bounds(hwnd);
    }
    // ★ 诊断开关 GALE_WNDPROC_MIN=1（2026-10-06 "打不了字"二分定位）：
    //   除 WM_DESTROY/WM_TIMER 外全部直落 DefWindowProc——绕开所有
    //   自定义消息臂（NCHITTEST/NCCALCSIZE/DPICHANGED/GETMINMAXINFO/
    //   鼠标臂/焦点归还）。窗口可显示可退出，视觉/交互退化没关系，
    //   只测输入法桥是否恢复组字。
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    if wndproc_min() && msg != WM_TIMER {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    // 鼠标事件入队（纯观测转发；DefWindowProc 照走，不影响焦点/命中测试）。
    // 坐标口径：物理像素（PMv2 感知下 WM_MOUSEMOVE lParam 即物理 px）。
    // 命中层是逻辑坐标系 → 这里统一 ÷scale 转换（四舍五入保整数对齐）。
    let sc = dpi_scale();
    let lx = |v: i32| ((v as f32) / sc + 0.5).floor() as i32;
    match msg {
        WM_MOUSEMOVE => {
            // 只订阅一次 TME_LEAVE：指针离开客户区时恢复光标（隐藏只在窗内有效）。
            if !TRACKING_MOUSE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut track);
            }
            let (x, y) = (lo16(lp.0 as u32) as i32, hi16(lp.0 as u32) as i32);
            push_mouse(MouseEv::Move { x: lx(x), y: lx(y) });
            // 拖动成行时唤醒主循环（空闲阻塞路径靠消息唤醒；MOUSE_QUEUE 是
            // 旁路观测，不经消息队列不会自动唤醒 wait_idle）
            request_frame();
        }
        WM_MOUSELEAVE_MSG => {
            TRACKING_MOUSE.store(false, std::sync::atomic::Ordering::Relaxed);
            push_mouse(MouseEv::Leave);
            request_frame();
        }
        WM_LBUTTONDOWN => {
            let (x, y) = (lo16(lp.0 as u32) as i32, hi16(lp.0 as u32) as i32);
            // ★ NOACTIVATE 窗口点击抢键盘焦点（2026-10-06）：系统不给
            //   WS_EX_NOACTIVATE 窗口做鼠标激活——别的程序拿到焦点后点回
            //   跟打器，键盘/输入法仍归别人 = "点不进来打不了字"。
            //   每次点击若焦点不在本窗则 SetFocus 拉回（已聚焦时跳过，
            //   不做 NULL 重入——那会拆 CUAS 桥（历史 ime_imm 实测教训，模块已删）。
            unsafe {
                if windows::Win32::UI::Input::KeyboardAndMouse::GetFocus().0 as isize
                    != hwnd.0 as isize
                {
                    let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(hwnd);
                }
            }
            push_mouse(MouseEv::LDown { x: lx(x), y: lx(y) });
        }
        WM_LBUTTONUP => {
            let (x, y) = (lo16(lp.0 as u32) as i32, hi16(lp.0 as u32) as i32);
            push_mouse(MouseEv::LUp { x: lx(x), y: lx(y) });
        }
        // 右键：不弹系统菜单，交给应用层在窗口内自绘外观菜单。
        WM_RBUTTONDOWN => {
            let (x, y) = (lo16(lp.0 as u32) as i32, hi16(lp.0 as u32) as i32);
            push_mouse(MouseEv::RDown { x: lx(x), y: lx(y) });
            request_frame();
        }
        WM_MOUSEWHEEL => {
            // wParam 高 16 位是有符号滚轮增量；lParam 是屏幕坐标（换算客户区）
            let delta = ((wp.0 as u32) >> 16) as i16 as i32;
            let mut p = windows::Win32::Foundation::POINT {
                x: lo16(lp.0 as u32) as i32,
                y: hi16(lp.0 as u32) as i32,
            };
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut p);
            push_mouse(MouseEv::Wheel {
                delta,
                x: lx(p.x),
                y: lx(p.y),
            });
        }
        _ => {}
    }
    match msg {
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_COMMAND if crate::font_picker::selection_command(wp, lp) => {
            request_frame();
            LRESULT(0)
        }
        WM_SETFOCUS => {
            // 焦点归还：主窗口只在 RichEdit 宿主不存在（启动前/销毁后）时
            // 持有焦点；模态面板打开时（set_focus_relay(false)）
            // 主窗口合法持有焦点。
            if !focus_relay_enabled() {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            let relay = crate::input_host::relay_hwnd();
            if relay != 0 && relay != hwnd.0 as isize {
                let h = HWND(relay as *mut core::ffi::c_void);
                if windows::Win32::UI::WindowsAndMessaging::IsWindow(h).as_bool() {
                    let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(h);
                    return LRESULT(0);
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        // 光标：应用层每帧下发提示（0=默认/2=上下调整）；键入时隐藏（SetCursor(NULL)），
        // 鼠标一动应用层会恢复，因此这里只需在隐藏态短路返回。
        WM_SETCURSOR => {
            let hit = lo16(lp.0 as u32) as u32;
            if hit == HTCLIENT as u32 {
                if CURSOR_HIDDEN.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = SetCursor(HCURSOR(core::ptr::null_mut()));
                    return LRESULT(1);
                }
                let id = match CURSOR_HINT.load(std::sync::atomic::Ordering::Relaxed) {
                    2 => Some(IDC_SIZENS),
                    _ => None,
                };
                if let Some(id) = id {
                    if let Ok(cursor) = LoadCursorW(None, id) {
                        SetCursor(cursor);
                        return LRESULT(1);
                    }
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        // Return the full window rectangle for both creation and resize passes.
        // Keeping the standard resize style preserves snapping and system shortcuts.
        WM_NCCALCSIZE => LRESULT(0),
        WM_NCHITTEST => {
            // 无边框窗口的手工命中测试（用户要求：去掉空框、保留拖动/缩放）。
            // 坐标：lParam 低/高 16 位 = 屏幕坐标（有符号）。
            let px = lo16(lp.0 as u32) as i16 as i32;
            let py = hi16(lp.0 as u32) as i16 as i32;
            let mut rc = windows::Win32::Foundation::RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut rc);
            // 边框热区宽 = 系统尺寸边框 + 镶边（和有边框窗口手感一致）。
            // GetSystemMetricsForDpi 按本窗 DPI 显式查询（PMv2 语境可靠）。
            let dpi = {
                let d = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd);
                if d == 0 {
                    96
                } else {
                    d
                }
            };
            let fm = windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi)
                + windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
            let frame = (fm as i32).max(4);
            let on_left = px < rc.left + frame;
            let on_right = px >= rc.right - frame;
            let on_top = py < rc.top + frame;
            let on_bottom = py >= rc.bottom - frame;
            // ★ 顶栏不再整宽 HTCAPTION（2026-10-07 用户实测：顶栏 chips 全在
            //   拖动带里，点击被系统转成 NC 拖动循环，WM_LBUTTONDOWN 永不到达
            //   = "按钮只能快捷键"的根因）。顶栏 chips 变 HTCLIENT 可点；
            //   顶部边框带（仅 frame 像素高）仍可拖动/缩放。整窗拖动改由
            //   客户区按住拖动路径承担（见 main.rs MOUSE 消费段）。
            if on_top && on_left {
                LRESULT(HTTOPLEFT as isize)
            } else if on_top && on_right {
                LRESULT(HTTOPRIGHT as isize)
            } else if on_bottom && on_left {
                LRESULT(HTBOTTOMLEFT as isize)
            } else if on_bottom && on_right {
                LRESULT(HTBOTTOMRIGHT as isize)
            } else if on_left {
                LRESULT(HTLEFT as isize)
            } else if on_right {
                LRESULT(HTRIGHT as isize)
            } else if on_top {
                LRESULT(HTTOP as isize)
            } else if on_bottom {
                LRESULT(HTBOTTOM as isize)
            } else {
                LRESULT(HTCLIENT as isize)
            }
        }
        WM_PAINT => {
            // 空闲唤醒源：被遮挡/恢复/尺寸变化时系统要重绘——请求一帧后验证。
            // 不在此直接渲染（wndproc 里跑 D2D present 是重入雷区），只置请求位。
            REQUEST_FRAME.store(true, std::sync::atomic::Ordering::Release);
            // 必须走 ValidateRect 清脏区，否则系统持续以 100% CPU 发 WM_PAINT
            let _ = windows::Win32::Graphics::Gdi::ValidateRect(hwnd, None);
            LRESULT(0)
        }
        WM_SIZE => {
            // 拖边框补帧（防灰/黑）：优先走 gale-app 注册的真帧重建
            // （文字原字号、布局按新尺寸重排）；无回调时退回渲染器自补。
            let w = lo16(lp.0 as u32) as i32;
            let h = hi16(lp.0 as u32) as i32;
            // 白条复发防线：窗口尺寸变化时 Windows 会重分配/扩展 GDI 面，
            // 新暴露区域可能又带未初始化残迹（与启动时同机制）。跟一枪
            // 清黑（黑色垫在透明交换链之下不可见），代价一次 PatBlt。
            if w > 0 && h > 0 {
                gdi_scrub_hwnd(hwnd);
            }
            let mut handled = false;
            if let Ok(mut guard) = SIZE_REBUILD.lock() {
                if let Some(holder) = guard.as_mut() {
                    (holder.0).0(w, h);
                    handled = true;
                }
            }
            if !handled {
                size_fill_once(w, h);
            }
            REQUEST_FRAME.store(true, std::sync::atomic::Ordering::Release);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_ERASEBKGND => LRESULT(1), // 背景永远由 D2D 全帧绘制，禁止 GDI 擦除（防闪白）
        WM_DPICHANGED => {
            // PMv2 跨屏拖动 / 用户改缩放：wParam 高 16 位 = 新 DPI，
            // lParam = 建议的新窗口矩形（物理 px，已按比例缩放）。
            //
            // ★★★ 噪声防御总纲（2026-10-05 两轮实战）：系统里有东西跟着
            //   鼠标移动狂发伪造 0x02E0（实测 6 分钟 2.5 万条），参数随机：
            //   wp=0/1/2、wp 高 16 位偶尔落进 96..384、lp=堆指针或小整数。
            //   曾因此（a）解引用野指针 SEGV；（b）scale 被改成 2.71 →
            //   拖动锚点飞出窗口 → 窗口自动变大。
            //   防线：全套指纹（DPI 范围 + lp 对齐 + 建议矩形与当前尺寸的
            //   DPI 比值数学关系 + 虚拟桌面位置）全部通过才动任何状态；
            //   scale 只认 GetDpiForWindow 窗口真值，不信消息参数。
            if std::env::var("GALE_VERBOSE").as_deref() == Ok("1") {
                eprintln!("[dpi] 0x02E0 wp={:#010x} lp={:#x}", wp.0, lp.0);
            }
            let new_dpi = ((wp.0 as u32) >> 16) as u32;
            let dpi_ok = (96..=384).contains(&new_dpi);
            let old_scale = dpi_scale(); // 建议矩形比值校验用
            let lp_v = lp.0 as usize;
            if dpi_ok && lp_v >= 0x1_0000 && (lp_v & 0x3) == 0 {
                // 按字段 unaligned 读（真实 DPICHANGED 的 lParam 是堆上 RECT，
                // 不会小于 64K；这个门槛同时挡掉误入的 packed 坐标值）
                let p = lp.0 as *const i32;
                let (l, t, r, b) = unsafe {
                    (
                        p.read_unaligned(),
                        p.add(1).read_unaligned(),
                        p.add(2).read_unaligned(),
                        p.add(3).read_unaligned(),
                    )
                };
                // ★★ 尺寸指纹二次校验（2026-10-05 用户报"拖动后窗口变大"）：
                //   真系统建议矩形 = 当前窗口尺寸 × 新旧 DPI 比（数学上必然），
                //   且位置在虚拟桌面范围内。伪造消息即使骗过 wp/lp 指纹，
                //   乱读出的矩形也几乎不可能同时满足这两条。不满足 → 只更新
                //   scale，不动窗口几何。
                let mut cur = windows::Win32::Foundation::RECT::default();
                let have_cur =
                    windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut cur).is_ok();
                let ratio = new_dpi as f32 / (old_scale * 96.0).max(1.0);
                let exp_w = (cur.right - cur.left) as f32 * ratio;
                let exp_h = (cur.bottom - cur.top) as f32 * ratio;
                let sw = (r - l) as f32;
                let sh = (b - t) as f32;
                let size_ok = have_cur
                    && (sw - exp_w).abs() <= exp_w * 0.25
                    && (sh - exp_h).abs() <= exp_h * 0.25;
                let vs_l = windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                    windows::Win32::UI::WindowsAndMessaging::SM_XVIRTUALSCREEN,
                );
                let vs_t = windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                    windows::Win32::UI::WindowsAndMessaging::SM_YVIRTUALSCREEN,
                );
                let vs_r = vs_l
                    + windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                        windows::Win32::UI::WindowsAndMessaging::SM_CXVIRTUALSCREEN,
                    );
                let vs_b = vs_t
                    + windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                        windows::Win32::UI::WindowsAndMessaging::SM_CYVIRTUALSCREEN,
                    );
                let m = 200; // 位置容差：建议矩形可能贴边/略越界
                let pos_ok = l >= vs_l - m && t >= vs_t - m && r <= vs_r + m && b <= vs_b + m;
                if r > l && b > t && size_ok && pos_ok {
                    let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                        hwnd,
                        None,
                        l,
                        t,
                        r - l,
                        b - t,
                        windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                            | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                    );
                    // ★ scale 只认窗口真值（伪造消息参数会把 scale 改飞：
                    // 实测被改成 2.71 → 拖动锚点错乱 → 窗口自动变大）
                    refresh_window_dpi(hwnd);
                } else if std::env::var("GALE_VERBOSE").as_deref() == Ok("1") {
                    eprintln!(
                        "[dpi] 拒绝可疑矩形 rect=({l},{t},{r},{b}) expect≈({exp_w:.0}x{exp_h:.0}) size_ok={size_ok} pos_ok={pos_ok}"
                    );
                }
            }
            // UI 布局是逻辑坐标系（尺寸不变），但物理交换链变了 → 请求重帧
            REQUEST_FRAME.store(true, std::sync::atomic::Ordering::Release);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_GETMINMAXINFO => {
            let info = lp.0 as *mut windows::Win32::UI::WindowsAndMessaging::MINMAXINFO;
            if !info.is_null() {
                (*info).ptMinTrackSize.x = (640. * dpi_scale()) as i32;
                (*info).ptMinTrackSize.y = (420. * dpi_scale()) as i32;
            }

            // ★ 拖动防 Aero Snap（2026-10-05 用户报"随便拖到别的地方整个窗口变大"）：
            //   自定义客户区拖动走 WM_NCLBUTTONDOWN(HTCAPTION) 模态移动循环，
            //   该循环拖到屏幕边缘会被系统贴边最大化/半屏接管。拖动期间把
            //   最大 track 尺寸钳到当前窗口大小 → 贴边松手尺寸不变。
            //   标志位只在拖动循环期间为真，平时边框手动缩放不受影响。
            if NC_DRAG_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
                let p = lp.0 as *mut windows::Win32::UI::WindowsAndMessaging::MINMAXINFO;
                if !p.is_null() {
                    let mut rc = windows::Win32::Foundation::RECT::default();
                    if windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut rc).is_ok()
                        && rc.right > rc.left
                        && rc.bottom > rc.top
                    {
                        unsafe {
                            (*p).ptMaxTrackSize.x = rc.right - rc.left;
                            (*p).ptMaxTrackSize.y = rc.bottom - rc.top;
                            (*p).ptMaxSize.x = rc.right - rc.left;
                            (*p).ptMaxSize.y = rc.bottom - rc.top;
                        }
                    }
                }
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
        WM_TIMER => {
            if wp.0 as usize == FRAME_TIMER_ID {
                // 帧计时器 tick：置请求位。主循环空闲时会把计时器停掉，
                // 所以收到 tick 只可能发生在"动画进行中"（或刚唤醒的首帧）。
                REQUEST_FRAME.store(true, std::sync::atomic::Ordering::Release);
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

impl MainWindow {
    /// 创建无边框主窗口（尺寸物理像素；visible 起始可见性）
    pub fn create(title: &str, w: i32, h: i32) -> windows::core::Result<Self> {
        unsafe {
            let hinstance = GetModuleHandleW(None)?;
            let class_name = windows::core::w!("HuFuTrainerWindow");
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(main_proc),
                hInstance: hinstance.into(),
                lpszClassName: class_name,
                hCursor: windows::Win32::UI::WindowsAndMessaging::LoadCursorW(
                    None,
                    windows::Win32::UI::WindowsAndMessaging::IDC_ARROW,
                )
                .unwrap_or_default(),
                ..Default::default()
            };
            let atom = RegisterClassW(&wc);
            if atom == 0 {
                return Err(windows::core::Error::from_win32());
            }
            let mut title_w: Vec<u16> = title.encode_utf16().collect();
            title_w.push(0);
            let style = WS_OVERLAPPEDWINDOW; // 可调有边框（无边框化由 WM_NCCALCSIZE 吞掉标题条）
            let _ = style;
            // Native RichEdit children need a real redirected GDI surface. The
            // DirectComposition target is below child windows, while WS_CLIPCHILDREN
            // keeps parent painting from overwriting their pixels.
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                windows::core::PCWSTR(title_w.as_ptr()),
                WS_OVERLAPPEDWINDOW | windows::Win32::UI::WindowsAndMessaging::WS_CLIPCHILDREN,
                100,
                100,
                w,
                h,
                None,
                None,
                windows::Win32::Foundation::HINSTANCE(hinstance.0),
                None,
            )?;
            let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                    | windows::Win32::UI::WindowsAndMessaging::SWP_FRAMECHANGED,
            );
            let corner_preference: u32 = 2; // DWMWCP_ROUND (ignored on pre-Win11)
            let _ = DwmSetWindowAttribute(
                hwnd,
                windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(33),
                &corner_preference as *const _ as *const _,
                std::mem::size_of::<u32>() as u32,
            );
            // 建窗即定标：PMv2 下本窗 DPI 决定全局 scale（渲染变换/鼠标换算
            // 都从这里取值；后续 WM_DPICHANGED 更新）
            refresh_window_dpi(hwnd);
            Ok(Self {
                hwnd: hwnd.0 as isize,
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut core::ffi::c_void)
    }

    /// GDI 面清黑（白条根治，2026-10-02）：
    /// DComp topmost 视觉之下是窗口的 GDI redirection surface。本程序
    /// 全部 UI 画在交换链（GDI 面无人管理），而 GDI 面初始分配不保证
    /// 干净——启动早期系统/IME/未初始化内存会在上面留下【白色矩形残迹】
    /// （实测 178x19 @ (40,40)，用户"只留跟打器也有的白条"即此物）。
    /// 交换链 alpha=0 的透明区会把它透出来，且 Clear/resize 都擦不掉
    /// （不在这个合成层）。修法：渲染链建立后用 PatBlt 把 GDI 面整面
    /// 涂黑一次（黑色在透明区后方=不可见，等于"擦干净"）。残留绘制
    /// 不会再发生（主循环/IME 均不画 GDI），一次即永久。
    pub fn gdi_scrub(&self) {
        unsafe { gdi_scrub_hwnd(self.hwnd()) }
    }

    /// 显示并抢占前台焦点（TSF 初始化前必须可见+聚焦——CUAS 在焦点时刻
    /// 建线程 TSF 单例；v9 探针实证的顺序铁律）
    pub fn show_and_focus(&self) {
        unsafe {
            let h = self.hwnd();
            let _ = ShowWindow(h, SW_SHOW);
            let _ = SetForegroundWindow(h);
        }
    }

    pub fn client_size(&self) -> (i32, i32) {
        unsafe {
            let mut r = windows::Win32::Foundation::RECT::default();
            let _ = GetClientRect(self.hwnd(), &mut r);
            (r.right - r.left, r.bottom - r.top)
        }
    }

    /// 当前窗口外框（屏幕物理像素）：用户调好的位置/大小落盘用。
    pub fn window_rect(&self) -> Option<(i32, i32, i32, i32)> {
        unsafe {
            if let Some(bounds) = normal_window_bounds(self.hwnd()) {
                *NORMAL_BOUNDS.lock().unwrap_or_else(|e| e.into_inner()) = Some(bounds);
                return Some(bounds);
            }
            *NORMAL_BOUNDS.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    /// Restore on the saved monitor, at that monitor's DPI. A removed monitor
    /// falls back to the nearest surviving work area with the whole window visible.
    pub fn restore_bounds(&self, x: i32, y: i32, w: i32, h: i32, physical_position: bool) {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST};
        use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
        use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};
        unsafe {
            let old_scale = dpi_scale();
            let (x, y) = if physical_position { (x, y) } else { ((x as f32 * old_scale).round() as i32, (y as f32 * old_scale).round() as i32) };
            let rect = RECT { left: x, top: y, right: x.saturating_add(1), bottom: y.saturating_add(1) };
            let monitor = MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() { return; }
            let (mut dx, mut dy) = (96, 96);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            let scale = dx.max(96) as f32 / 96.;
            let (x, y, w, h) = clamp_restored_bounds(x, y, (w.max(640) as f32 * scale).round() as i32, (h.max(420) as f32 * scale).round() as i32,
                (info.rcWork.left, info.rcWork.top, info.rcWork.right, info.rcWork.bottom));
            // Complete the cross-monitor DPI transition while still hidden,
            // then set the final size so WM_DPICHANGED cannot replace it.
            let _ = SetWindowPos(self.hwnd(), None, x, y, 0, 0, SWP_NOZORDER | SWP_NOACTIVATE | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE);
            refresh_window_dpi(self.hwnd());
            let _ = SetWindowPos(self.hwnd(), None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    /// 启动时恢复上次保存的位置（不改大小）。
    pub fn move_to(&self, x: i32, y: i32) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                self.hwnd(),
                None,
                x,
                y,
                0,
                0,
                windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
            );
        }
    }
}

/// 标准消息泵（阻塞；帧驱动渲染在别处用计时器/呈现回调）
pub fn run_message_loop() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !tsf_filter(&msg) {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

/// 非阻塞消息泵一步：有消息处理消息；无消息返回 true。
/// 返回 false = WM_QUIT（应退出主循环）。
pub fn pump_messages_once() -> bool {
    unsafe {
        let mut msg = MSG::default();
        if tsf_peek(&mut msg) {
            if msg.message == WM_QUIT {
                return false;
            }
            if crate::input_host::capture_physical_key(&msg) {
                return true;
            }
            if !focus_relay_enabled()
                && msg.message == windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN
                && msg.wParam.0 == 9
            {
                use windows::Win32::UI::WindowsAndMessaging::{
                    GetAncestor, GetNextDlgTabItem, GA_ROOT,
                };
                let root = GetAncestor(msg.hwnd, GA_ROOT);
                let previous = windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x10) < 0;
                if let Ok(next) = GetNextDlgTabItem(root, msg.hwnd, previous) {
                    let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(next);
                }
                return true;
            }
            if !tsf_filter(&msg) {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        true
    }
}

/// 启动帧计时器（USER_TIMER_MINIMUM≈10ms；返回计时器 id）
pub fn start_frame_timer(hwnd: isize) -> usize {
    unsafe {
        let id = SetTimer(
            HWND(hwnd as *mut core::ffi::c_void),
            FRAME_TIMER_ID,
            16,
            None,
        );
        id as usize
    }
}

/// 停止帧计时器（进入空闲阻塞前调用）
pub fn stop_frame_timer(hwnd: isize) {
    unsafe {
        let _ = KillTimer(HWND(hwnd as *mut core::ffi::c_void), FRAME_TIMER_ID);
    }
}

// ── 按需帧（request-frame）原语 ──────────────────────────────────────────
// 目标（_research/d2d-fx-notes.md §1.2）：动画期计时器驱动 60-120fps；
// 空闲期彻底停帧回纯阻塞等待——挂机 0.0% CPU（疾风1代口碑点）。
//
// 三条唤醒路径：
// 1. WM_TIMER/WM_PAINT（见 main_proc）置 REQUEST_FRAME；
// 2. 外部代码（输入事件、动效触发、载文等）调用 request_frame()；
// 3. 鼠标/键盘消息本身会唤醒阻塞泵（PeekMessage 侧每轮先查请求位）。

/// 帧请求位：true = 需要渲染一帧（渲染后由消费方 take 清零）
static REQUEST_FRAME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 请求渲染一帧（输入事件/动效触发时调用；幂等）
pub fn request_frame() {
    REQUEST_FRAME.store(true, std::sync::atomic::Ordering::Release);
}

/// 取走帧请求（主循环每轮调用；true = 本轮应渲染）
pub fn take_frame_request() -> bool {
    REQUEST_FRAME.swap(false, std::sync::atomic::Ordering::AcqRel)
}

/// 是否有挂起的帧请求（只读探测）
pub fn has_frame_request() -> bool {
    REQUEST_FRAME.load(std::sync::atomic::Ordering::Acquire)
}

/// 空闲等待：阻塞直到新消息到达或帧请求被置位。
///
/// 与 run_message_loop 不同，本函数在任意消息（含 RichEdit 子控件的
/// 输入消息——它们派发时同样经过线程消息队列）到达时返回，让调用方
/// 决定渲染与否；WM_QUIT 返回 false。
///
/// timeout_ms：MsgWaitForMultipleObjects 的超时（None = 无限等待）。
/// 超时返回 Ok(true) 但不消费消息——调用方下一轮 pump 再处理。
pub fn wait_idle(timeout_ms: Option<u32>) -> bool {
    unsafe {
        // 注意：windows 0.58 里 MsgWaitForMultipleObjects 挂在
        // WindowsAndMessaging 模块（非 System::Threading），参数顺序也是
        // (handles, wait_all, timeout, wake_mask)。
        use windows::Win32::UI::WindowsAndMessaging::{MsgWaitForMultipleObjects, QS_ALLINPUT};
        let dw_wake_mask = QS_ALLINPUT;
        let timeout = timeout_ms.unwrap_or(u32::MAX);
        let r = MsgWaitForMultipleObjects(None, false, timeout, dw_wake_mask);
        let _ = r;
        // 消息的实际派发由调用方 pump_messages_once 完成；这里只负责唤醒。
        true
    }
}

const FRAME_TIMER_ID: usize = 1;

/// 自定义拖动（客户区按住→HTCAPTION 模态循环）进行中标志：
/// WM_GETMINMAXINFO 用它把最大 track 尺寸钳到当前大小，屏蔽 Aero Snap
/// 贴边最大化/半屏。由 gale-app 在进/出拖动循环时设置/清除。
pub static NC_DRAG_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 设置自定义拖动标志（进拖动循环前 true，出来后 false）。
pub fn set_nc_drag_active(on: bool) {
    NC_DRAG_ACTIVE.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// 【窗口置顶 2026-11-12】右键菜单/设置页切换：始终在最前但不抢焦点。
pub fn set_topmost(hwnd: HWND, on: bool) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            if on { HWND_TOPMOST } else { HWND_NOTOPMOST },
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_queue_roundtrip() {
        push_mouse(MouseEv::LDown { x: 10, y: 20 });
        push_mouse(MouseEv::Move { x: 12, y: 22 });
        let evs = drain_mouse();
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0], MouseEv::LDown { x: 10, y: 20 });
        assert_eq!(evs[1], MouseEv::Move { x: 12, y: 22 });
        assert!(drain_mouse().is_empty());
    }

    #[test]
    fn mouse_queue_bounded() {
        let max_i: i32 = MOUSE_QUEUE_MAX as i32;
        for i in 0..(max_i + 40) {
            push_mouse(MouseEv::Move { x: i, y: 0 });
        }
        // 满后丢旧保新：队列长度受控且最后一条是最新事件
        let evs = drain_mouse();
        assert!(evs.len() <= MOUSE_QUEUE_MAX + 1);
        assert_eq!(
            *evs.last().unwrap(),
            MouseEv::Move {
                x: max_i + 39,
                y: 0
            }
        );
    }

    #[test]
    fn lparam_decode() {
        // GET_X_LPARAM 语义：低/高 16 位有符号扩展（负坐标在客户区左侧/上侧）
        assert_eq!(lo16(0xFFFF_FFF0) as i16 as i32, -16);
        assert_eq!(hi16(0xFFF0_0000) as i16 as i32, -16);
        assert_eq!(lo16(0x0000_0140) as i16 as i32, 320);
        // y=232, x=0 的 lParam：y 在高 16 位
        assert_eq!(hi16(0x00E8_0000) as i16 as i32, 232);
        assert_eq!(lo16(0x00E8_0000) as i16 as i32, 0);
        // y=232, x=0 的反例：x=232 存低 16 位时高位取 0
        assert_eq!(hi16(0x0000_00E8) as i16 as i32, 0);
        assert_eq!(lo16(0x0000_00E8) as i16 as i32, 232);
    }

    #[test]
    fn accent_mapping() {
        // 晴跟打 parity（2026-10 重裁）：磨砂=ACRYLICBLURBEHIND、透明=BLURBEHIND、
        // 纯色=DISABLE；tint alpha：磨砂按用户不透明度映射、透明恒 0（纯模糊无底色）
        let p = |m: Material| MaterialParams {
            hwnd: 0,
            material: m,
            tint_rgb: (0x26, 0x26, 0x2a),
            opacity: 200,
        };
        assert_eq!(p(Material::Solid).accent(), AccentFlags::Disable);
        assert_eq!(
            p(Material::Acrylic).accent(),
            AccentFlags::EnableAcrylicBlurBehind
        );
        assert_eq!(p(Material::Blur).accent(), AccentFlags::EnableBlurBehind);
        // 磨砂 tint alpha = opacity/100*255（200% 钳到 u8 上限语义由调用方保证，此处按公式）
        let t = p(Material::Acrylic).tint_abgr();
        assert_eq!(t & 0xff, 0x26);
        assert_eq!((t >> 24) & 0xff, (200.0 / 100.0 * 255.0) as u32 & 0xff);
        // 透明 tint alpha 恒 0（不加底色）
        let t2 = p(Material::Blur).tint_abgr();
        assert_eq!((t2 >> 24) & 0xff, 0);
        // 50% 磨砂 → alpha≈127
        let p50 = |m: Material| MaterialParams {
            hwnd: 0,
            material: m,
            tint_rgb: (0x26, 0x26, 0x2a),
            opacity: 50,
        };
        let t3 = p50(Material::Acrylic).tint_abgr();
        assert_eq!((t3 >> 24) & 0xff, 127);
    }

    #[test]
    fn backdrop_mapping() {
        // SDK 常量固定；apply_material 现恒写 None=1 清系统 backdrop（不再按材质映射）
        assert_eq!(Backdrop::TransientWindow as u32, 3);
        assert_eq!(Backdrop::MainWindow as u32, 2);
        assert_eq!(Backdrop::None as u32, 1);
    }
}
