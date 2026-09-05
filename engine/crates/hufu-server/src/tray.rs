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

/// 托盘「中」字图标：深色圆角方 + 白色「中」（口环 + 贯通竖，
/// 4× 超采样抗锯齿）。虎爪定稿分工：任务栏输入指示 pill 显示爪印
/// （DLL 内嵌资源），托盘常驻「中」作设置入口（虎爪/Rime 同款观感）。
fn make_zh_icon() -> isize {
    const S: usize = 32; // 图标边长
    const SS: usize = 4; // 超采样倍数
    // 「中」几何：口 外沿 + 笔宽；竖：中心 x 与上下端
    let (bx0, by0, bx1, by1) = (8.8f32, 10.6, 23.2, 25.4);
    let stroke = 2.5f32;
    let (vx, vy0, vy1) = (16.0f32, 5.4, 27.6);
    let r = 7.5f32; // 底圆角半径
    let half = 14.0f32; // 半边长（28×28 内容，32 画布留 2px 边距）
    let bg = [0.105, 0.105, 0.118f32]; // #1B1B1E
    let fg = [0.96, 0.96, 0.97f32]; // #F5F5F7
    let mut buf = vec![0u8; S * S * 4]; // 预乘 BGRA
    for y in 0..S {
        for x in 0..S {
            let mut cov_bg = 0u32;
            let mut cov_fg = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f32 + (sx as f32 + 0.5) / SS as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / SS as f32;
                    let qx = (px - 16.0).abs() - (half - r);
                    let qy = (py - 16.0).abs() - (half - r);
                    let d = (qx.max(0.0) * qx.max(0.0) + qy.max(0.0) * qy.max(0.0)).sqrt();
                    if d <= r {
                        cov_bg += 1;
                        // 口：环 = 在外沿内 且 不在缩进 stroke 的内沿内
                        let in_outer = px >= bx0 && px <= bx1 && py >= by0 && py <= by1;
                        let in_inner = px >= bx0 + stroke
                            && px <= bx1 - stroke
                            && py >= by0 + stroke
                            && py <= by1 - stroke;
                        // 竖：中心线 ± stroke/2，贯穿 y 范围
                        let in_bar = (px - vx).abs() <= stroke / 2.0 && py >= vy0 && py <= vy1;
                        if (in_outer && !in_inner) || in_bar {
                            cov_fg += 1;
                        }
                    }
                }
            }
            let a_bg = cov_bg as f32 / (SS * SS) as f32;
            let a_fg = cov_fg as f32 / (SS * SS) as f32;
            let a = a_bg;
            if a > 0.0 {
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
    bgra_to_hicon(&buf)
}

/// 简约「虎爪三痕」图标（备用/历史）：深色圆角方 + 三道白色斜爪痕。
#[allow(dead_code)]
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
    let half = 14.0f32; // 半边长（28×28 占 32 画布，留 2px 边距）
    let bg = [0.105, 0.105, 0.118f32]; // #1B1B1E
    let fg = [0.96, 0.96, 0.97f32]; // #F5F5F7
    let mut buf = vec![0u8; S * S * 4]; // 预乘 BGRA
    for y in 0..S {
        for x in 0..S {
            // 4×4 超采样
            let mut cov_bg = 0u32;
            let mut cov_fg = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f32 + (sx as f32 + 0.5) / SS as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / SS as f32;
                    // 圆角方形 SDF：q = |p-c|-(half-r)，d = hypot(max(q,0)) ≤ r 为内部
                    let qx = (px - 16.0).abs() - (half - r);
                    let qy = (py - 16.0).abs() - (half - r);
                    let d = (qx.max(0.0) * qx.max(0.0) + qy.max(0.0) * qy.max(0.0)).sqrt();
                    if d <= r {
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
        // 掩码必须是 1bpp 单色位图（传 32bpp 会让 CreateIconIndirect 失败 → 图标退回默认）
        let mut mbi = bi;
        mbi.bmiHeader.biBitCount = 1;
        let mut mbits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hmask = CreateDIBSection(hdc, &mbi, 0, &mut mbits, 0, 0);
        if hcolor == 0 || hmask == 0 || bits.is_null() || mbits.is_null() {
            if hcolor != 0 { let _ = DeleteObject(hcolor); }
            if hmask != 0 { let _ = DeleteObject(hmask); }
            let _ = DeleteDC(hdc);
            eprintln!("托盘图标位图创建失败");
            return 0;
        }
        std::ptr::copy_nonoverlapping(buf.as_ptr(), bits as *mut u8, buf.len());
        // 1bpp 行按 32 位对齐（32px → 4 字节/行），全 0 = 不遮任何像素（alpha 全权）
        let stride_bytes = ((S + 31) / 32) * 4;
        std::ptr::write_bytes(mbits as *mut u8, 0, stride_bytes * S);
        let ii = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hmask,
            hbmColor: hcolor,
        };
        let hicon = CreateIconIndirect(&ii);
        if hicon == 0 {
            eprintln!("CreateIconIndirect 失败");
        }
        // ICONINFO 文档：位图所有权归系统，不删；DC 可删
        let _ = DeleteDC(hdc);
        hicon
    }
}

/// 预乘 BGRA（32×32）→ HICON（DIB + CreateIconIndirect；1bpp 全零掩码）
fn bgra_to_hicon(buf: &[u8]) -> isize {
    const S: usize = 32;
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
        // 掩码必须是 1bpp 单色位图（传 32bpp 会让 CreateIconIndirect 失败 → 图标退回默认）
        let mut mbi = bi;
        mbi.bmiHeader.biBitCount = 1;
        let mut mbits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hmask = CreateDIBSection(hdc, &mbi, 0, &mut mbits, 0, 0);
        if hcolor == 0 || hmask == 0 || bits.is_null() || mbits.is_null() {
            if hcolor != 0 { let _ = DeleteObject(hcolor); }
            if hmask != 0 { let _ = DeleteObject(hmask); }
            let _ = DeleteDC(hdc);
            eprintln!("托盘图标位图创建失败");
            return 0;
        }
        std::ptr::copy_nonoverlapping(buf.as_ptr(), bits as *mut u8, buf.len());
        // 1bpp 行按 32 位对齐（32px → 4 字节/行），全 0 = 不遮任何像素（alpha 全权）
        let stride_bytes = ((S + 31) / 32) * 4;
        std::ptr::write_bytes(mbits as *mut u8, 0, stride_bytes * S);
        let ii = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hmask,
            hbmColor: hcolor,
        };
        let hicon = CreateIconIndirect(&ii);
        if hicon == 0 {
            eprintln!("CreateIconIndirect 失败");
        }
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
    fn RegisterHotKey(hwnd: isize, id: i32, modifiers: u32, vk: u32) -> i32;
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
    fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn SetTimer(hwnd: isize, idevent: usize, elapse: u32, timerproc: isize) -> usize;
    fn KillTimer(hwnd: isize, idevent: usize) -> i32;
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
const NIF_ICON: u32 = 0x2;
const NIF_TIP: u32 = 0x4;
const WM_APP: u32 = 0x8000;
const WM_HOTKEY: u32 = 0x0312;
const WM_TIMER: u32 = 0x0113;
/// 「输入法激活态变化」投递消息（wparam=1 激活 / 0 未激活）
const WM_APP_IME: u32 = 0x8001;
/// 激活消失防抖定时器 id（进程切换焦点时旧进程 Deactivate 与新进程
/// Activate 交错，先 false 后 true——延时确认再隐藏，避免图标闪烁）
const TIMER_IME_HIDE: usize = 1;

static TRAY_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static ASK_QUIT: AtomicBool = AtomicBool::new(false);
/// 虎符输入法当前是否激活（任一进程的 DLL Activate 上报）
static IME_ACTIVE: AtomicBool = AtomicBool::new(false);
/// 托盘图标当前是否已添加（显隐由输入法激活态驱动）
static ICON_ADDED: AtomicBool = AtomicBool::new(false);

/// tray 隐藏窗口句柄（candwin 死窗重建请求用）
pub fn tray_hwnd() -> isize {
    TRAY_HWND.load(Ordering::SeqCst)
}

/// DLL 侧 Activate/Deactivate 经管道上报（pipe.rs op "ime" 转发至此）。
/// 线程安全：Post 到托盘窗口线程处理（Shell_NotifyIcon 必须在创建
/// 图标的线程上调用）。
pub fn on_ime_state(active: bool) {
    IME_ACTIVE.store(active, Ordering::SeqCst);
    let hwnd = TRAY_HWND.load(Ordering::SeqCst);
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(hwnd, WM_APP_IME, usize::from(active), 0);
        }
    }
}
const WM_DESTROY: u32 = 0x0002;
const WM_COMMAND: u32 = 0x0111;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_LBUTTONUP: u32 = 0x0202;
const TPM_RIGHTBUTTON: u32 = 0x0002;
const TPM_RETURNCMD: u32 = 0x0100;
const MF_STRING: u32 = 0x0;
const MF_SEPARATOR: u32 = 0x800;
const MF_POPUP: u32 = 0x10;
const MF_CHECKED: u32 = 0x8;

const IDM_SETTINGS: usize = 1;
const IDM_QUIT: usize = 3;
/// 方案子菜单命令 id 基址：100+序号（上限 40 个方案）
const IDM_SCHEMA_BASE: i32 = 100;
const SCHEMA_MAX: usize = 40;

/// 打开设置页的信号（主线程 select 循环外执行）
static mut OPEN_SETTINGS: Option<Sender<()>> = None;

/// 外部（管道 op "settings"：语言栏「中」按钮点击）请求打开设置页
pub fn open_settings() {
    unsafe {
        if let Some(tx) = OPEN_SETTINGS.as_ref() {
            let _ = tx.send(());
        }
    }
}
/// 引擎宿主（托盘右键「切换方案」直调，与 HTTP 路由同源逻辑）
static mut SHARED: Option<std::sync::Arc<std::sync::Mutex<crate::host::Host>>> = None;

/// 码表目录方案列表 + 当前方案名（快照；锁内只做目录读与字段拷贝）。
/// 【死锁教训】pipe 线程已在 dispatch 持锁，不可经此函数二次锁——
/// 只供托盘自己的菜单线程使用。
fn schema_snapshot() -> (Vec<String>, String) {
    let shared = unsafe {
        #[allow(static_mut_refs)]
        SHARED.as_ref()
    };
    let Some(shared) = shared else { return (Vec::new(), String::new()) };
    let Ok(host) = shared.lock() else { return (Vec::new(), String::new()) };
    let dir = hufu_engine::Engine::resolve_data_sub(
        &host.data_dir,
        &host.engine.config.schema.dir,
    );
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                // 码表子目录多为 junction：file_type() 判非目录，须 path().is_dir()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names.truncate(SCHEMA_MAX);
    (names, host.engine.config.schema.current.clone())
}

/// 切换方案（与 POST /api/schema 同逻辑：换方案 + 清会话 + 重建整句 + 落盘）。
/// pipe "set_schema" 在 dispatch 持锁内直做同逻辑（见 pipe.rs），不绕这里。
fn switch_schema(name: &str) {
    let shared = unsafe {
        #[allow(static_mut_refs)]
        SHARED.as_ref()
    };
    let Some(shared) = shared else { return };
    let Ok(mut host) = shared.lock() else { return };
    if host.engine.switch_schema(name).is_ok() {
        host.session.clear();
        host.setup_sentence();
        let _ = host.engine.config.save(&host.config_path);
    }
}

extern "system" fn wnd_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    unsafe {
        match msg {
            // 候选窗重建（宿主退出销毁了子窗口时由 pipe 线程请求）
            0x8003 => {
                crate::candwin::reinit_if_dead();
                0
            }
            WM_HOTKEY => {
                // Ctrl+Alt+H → 设置页（与托盘双击同通道）
                if wparam == 0x4846 {
                    if let Some(tx) = OPEN_SETTINGS.as_ref() {
                        let _ = tx.send(());
                    }
                }
                0
            }
            WM_APP => {
                match (lparam & 0xFFFF) as u32 {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                        if let Some(tx) = OPEN_SETTINGS.as_ref() {
                            let _ = tx.send(());
                        }
                    }
                    WM_RBUTTONUP => {
                        let hmenu = CreatePopupMenu();
                        // 方案子菜单（当前方案打勾，点击即切）
                        let (schemas, current) = schema_snapshot();
                        let mut submenu_names: Vec<String> = Vec::new();
                        if !schemas.is_empty() {
                            let sub = CreatePopupMenu();
                            for (i, name) in schemas.iter().enumerate() {
                                let label: Vec<u16> =
                                    format!("{name}\0").encode_utf16().collect();
                                let flags = if *name == current {
                                    MF_STRING | MF_CHECKED
                                } else {
                                    MF_STRING
                                };
                                AppendMenuW(
                                    sub,
                                    flags,
                                    (IDM_SCHEMA_BASE + i as i32) as usize,
                                    label.as_ptr(),
                                );
                                submenu_names.push(name.clone());
                            }
                            let title: Vec<u16> = "切换方案\0".encode_utf16().collect();
                            AppendMenuW(hmenu, MF_POPUP, sub as usize, title.as_ptr());
                            AppendMenuW(hmenu, MF_SEPARATOR, 0, std::ptr::null());
                        }
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
                        if cmd >= IDM_SCHEMA_BASE
                            && ((cmd - IDM_SCHEMA_BASE) as usize) < submenu_names.len()
                        {
                            switch_schema(&submenu_names[(cmd - IDM_SCHEMA_BASE) as usize]);
                        } else if cmd == IDM_SETTINGS as i32 {
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
            WM_APP_IME => {
                // 输入法激活态变化。【无托盘模式】不再上图标，此消息仅存档。
                0
            }
            WM_TIMER => {
                if wparam == TIMER_IME_HIDE {
                    let _ = KillTimer(hwnd, TIMER_IME_HIDE);
                    // 常驻化：不再因失活隐藏
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
    let hicon = make_zh_icon();
    let hicon = if hicon != 0 {
        hicon
    } else {
        // 兜底：系统默认图标，至少可见
        unsafe {
            LoadImageW(0, 32512 as *const u16, 1, 0, 0, 0x8000)
        }
    };
    // 固定 GUID 身份（NIF_GUID）：无 GUID 时 Windows 按「窗口+图标」
    // 指纹识别托盘项——进程重启/重装后指纹变化，用户设过的「常驻
    // 任务栏（不进折叠区）」就被当成新图标遗忘，图标重新躲进隐藏区。
    // 固定 GUID 后身份跨重启/重装稳定，常驻设置一次永久记住。
    const NIF_GUID: u32 = 0x10;
    // {7C9A2B44-3E11-4F5A-9D6C-8B1E0A55F3A2}（GUID 内存序）
    const TRAY_GUID: [u8; 16] = [
        0x44, 0x2B, 0x9A, 0x7C, 0x11, 0x3E, 0x5A, 0x4F, 0x9D, 0x6C, 0x8B, 0x1E, 0x0A, 0x55,
        0xF3, 0xA2,
    ];
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        // NIF_ICON 必须置位：漏了它 Shell_NotifyIcon 无视 hIcon（此前图标永远是系统默认的真因）
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_GUID,
        uCallbackMessage: WM_APP,
        hIcon: hicon,
        szTip: tip,
        dwState: 0,
        dwStateMask: 0,
        szInfo: [0; 256],
        uVersion: 0,
        szInfoTitle: [0; 64],
        dwInfoFlags: 0,
        guidItem: TRAY_GUID,
        hBalloonIcon: 0,
    }
}

/// 在独立线程跑托盘消息循环。返回 (ask_quit 标志引用, 设置页请求接收端)。
/// 传入 `quit_tx`：托盘退出时发 () 通知主循环退出。
pub fn spawn(
    quit_tx: Sender<()>,
    open_tx: Sender<()>,
    shared: Option<std::sync::Arc<std::sync::Mutex<crate::host::Host>>>,
) {
    std::thread::spawn(move || unsafe {
        unsafe {
            OPEN_SETTINGS = Some(open_tx);
            SHARED = shared;
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
        // 全局热键 Ctrl+Alt+H：随时呼出设置页，不依赖托盘图标是否
        // 被折叠进隐藏区（「常驻任务栏」是 Windows 用户设置，应用无权
        // 自我提升；热键让设置入口与托盘可见性彻底解耦）。
        const MOD_ALT: u32 = 0x1;
        const MOD_CONTROL: u32 = 0x2;
        const HOTKEY_ID_SETTINGS: i32 = 0x4846; // "HF"
        let hk = RegisterHotKey(hwnd, HOTKEY_ID_SETTINGS, MOD_CONTROL | MOD_ALT, 0x48 /*'H'*/);
        let _ = hk; // 注册失败（被占用）不致命：托盘菜单/设置.bat 仍在
        // 【无托盘模式】（用户定稿）：不显示任何托盘图标。设置入口=
        // Ctrl+Alt+H 全局热键（+ 设置.bat/开始菜单快捷方式）。消息
        // 窗口与热键必须保留（WM_HOTKEY 靠窗口接收）。
        let _ = GetCurrentThreadId();
        // 越进程候选窗：在 tray 线程创建（消息循环共用）——沉浸式
        // 宿主（开始菜单搜索）里 DLL 自绘窗被 DWM cloaked，server 代画
        crate::candwin::init_on_tray_thread();
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
