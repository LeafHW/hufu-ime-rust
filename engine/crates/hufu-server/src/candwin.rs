//! 越进程候选窗：server（普通桌面进程）代画。
//!
//! 病根链（2026-08-29 实测，开始菜单搜索 SearchHost）：
//! ① DLL 在打包宿主里的自绘窗口（DComp 直通/v1 混合）被 DWM 以
//!   DWM_CLOAKED_SHELL 整体隐身；
//! ② ITfCandidateListUIElement 被宿主拒绝（BeginUIElement pbShow=TRUE
//!   =「你自己画」），无处可画。
//! 破局：DLL 把候选数据+光标坐标经管道发来，server 在自己进程里开窗
//! 绘制——server 是普通权限桌面进程，窗口不受容器隐身限制。
//! FFI 风格与 tray.rs 一致（裸声明 + 手写结构体，无 windows crate）。

use std::sync::Mutex;

/// 候选帧数据（pipe 线程 → 窗口线程经 WM_APP 消息移交所有权）
pub struct CandFrame {
    pub items: Vec<(String, String)>,
    pub raw: String,
    pub selected: usize,
    /// 用户皮肤（与 DLL 自绘窗同一份 JSON；server 按同字段渲染）
    pub skin: serde_json::Value,
}

static FRAME: Mutex<Option<CandFrame>> = Mutex::new(None);
static WND: Mutex<Option<isize>> = Mutex::new(None);

const WM_APP_CAND: u32 = 0x8001; // 显示/更新（lparam=Box<CandFrame>）
const WM_APP_HIDE: u32 = 0x8002;
const WM_PAINT: u32 = 0x000F;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_SHOWWINDOW: u32 = 0x0040;
const SW_HIDE: i32 = 0;
const HWND_TOPMOST: isize = -1;
const WS_POPUP: u32 = 0x8000_0000;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const WS_EX_TOPMOST: u32 = 0x0000_0008;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;

#[repr(C)]
struct RECT { left: i32, top: i32, right: i32, bottom: i32 }

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
struct PAINTSTRUCT {
    hdc: isize,
    fErase: i32,
    rcPaint: RECT,
    fRestore: i32,
    fIncUpdate: i32,
    rgbReserved: [u8; 32],
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(lpwcx: *const WNDCLASSW) -> u16;
    fn CreateWindowExW(
        ex: u32, cls: *const u16, name: *const u16, style: u32,
        x: i32, y: i32, w: i32, h: i32, parent: isize, menu: isize,
        inst: isize, param: *const core::ffi::c_void,
    ) -> isize;
    fn DefWindowProcW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn BeginPaint(hwnd: isize, ps: *mut PAINTSTRUCT) -> isize;
    fn EndPaint(hwnd: isize, ps: *const PAINTSTRUCT) -> i32;
    fn GetClientRect(hwnd: isize, r: *mut RECT) -> i32;
    fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
    fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    fn InvalidateRect(hwnd: isize, r: *const RECT, erase: i32) -> i32;
    fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateSolidBrush(color: u32) -> isize;
    fn DeleteObject(o: isize) -> i32;
    fn CreateFontW(
        h: i32, w: i32, esc: i32, orient: i32, weight: i32,
        italic: u32, underline: u32, strikeout: u32, charset: u32,
        outprec: u32, clipprec: u32, quality: u32, pitch: u32,
        face: *const u16,
    ) -> isize;
    fn SelectObject(hdc: isize, o: isize) -> isize;
    fn SetTextColor(hdc: isize, color: u32) -> u32;
    fn SetBkMode(hdc: isize, mode: i32) -> i32;
    fn TextOutW(hdc: isize, x: i32, y: i32, s: *const u16, c: i32) -> i32;
    fn GetTextExtentPoint32W(hdc: isize, s: *const u16, c: i32, sz: *mut SIZE) -> i32;
    fn CreatePen(style: i32, width: i32, color: u32) -> isize;
    fn Rectangle(hdc: isize, l: i32, t: i32, r: i32, b: i32) -> i32;
    fn CreateRoundRectRgn(l: i32, t: i32, r: i32, b: i32, w: i32, h: i32) -> isize;
    fn FillRgn(hdc: isize, rgn: isize, brush: isize) -> i32;
    fn GetStockObject(idx: i32) -> isize;
}

#[repr(C)]
struct SIZE { cx: i32, cy: i32 }

unsafe fn rectangle_gdi(hdc: isize, l: i32, t: i32, r: i32, b: i32) -> i32 {
    Rectangle(hdc, l, t, r, b)
}

// ── 皮肤取值（与 DLL candwin2.rs 同字段同默认值）──

fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 && s.len() != 8 {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some((b(0)?, b(2)?, b(4)?))
}

/// 皮肤颜色 → GDI COLORREF（0x00BBGGRR）；找不到用默认
fn skin_color(skin: &serde_json::Value, key: &str, default: &str) -> u32 {
    let hex = skin
        .pointer(&format!("/skin/colors/{key}"))
        .or_else(|| skin.get("colors").and_then(|c| c.get(key)))
        .and_then(|x| x.as_str())
        .unwrap_or(default);
    match parse_hex(hex) {
        Some((r, g, b)) => ((b as u32) << 16) | ((g as u32) << 8) | r as u32,
        None => 0x00_20_20_20,
    }
}

/// 皮肤布局数值；找不到用默认
fn skin_layout(skin: &serde_json::Value, key: &str, default: f32) -> f32 {
    skin.pointer(&format!("/skin/layout/{key}"))
        .or_else(|| skin.get("layout").and_then(|l| l.get(key)))
        .and_then(|x| x.as_f64())
        .unwrap_or(default as f64) as f32
}

/// 皮肤字体（layout.font_face，默认跟随系统雅黑）
fn skin_font_face(skin: &serde_json::Value) -> String {
    skin.pointer("/skin/layout/font_face")
        .or_else(|| skin.get("layout").and_then(|l| l.get("font_face")))
        .and_then(|x| x.as_str())
        .unwrap_or("Microsoft YaHei UI")
        .to_string()
}

extern "system" fn wnd_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    // panic 绝不能越过 FFI 边界（=进程 abort）——兜住，保住窗口线程
    let r = std::panic::catch_unwind(|| wnd_proc_inner(hwnd, msg, wparam, lparam));
    r.unwrap_or(0)
}

fn wnd_proc_inner(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_PAINT => unsafe {
            let mut ps = PAINTSTRUCT {
                hdc: 0, fErase: 0, rcPaint: RECT { left: 0, top: 0, right: 0, bottom: 0 },
                fRestore: 0, fIncUpdate: 0, rgbReserved: [0; 32],
            };
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            let _ = GetClientRect(hwnd, &mut r);
            let (w, h) = (r.right, r.bottom);
            let guard = frame_lock();
            if let Some(f) = guard.as_ref() {
                let skin = &f.skin;
                // 皮肤底色 + 边框（DLL 同字段同默认）
                let bg = CreateSolidBrush(skin_color(skin, "back_color", "#202022E6"));
                let _ = fill_rect(hdc, &RECT { left: 0, top: 0, right: w, bottom: h }, bg);
                let _ = DeleteObject(bg);
                let bw = skin_layout(skin, "border_width", 1.0).max(0.0) as i32;
                if bw > 0 {
                    let border = CreatePen(0, bw, skin_color(skin, "border_color", "#FFFFFF26"));
                    let old_pen = SelectObject(hdc, border);
                    // 【GDI 坑】Rectangle() 会用当前画刷填充内部——默认
                    // 白刷曾把整个候选窗刷白（实测 ckA=FFFFFF 定位）。
                    // 换 NULL_BRUSH：只描边不填充。
                    let null_brush = GetStockObject(5 /*NULL_BRUSH*/);
                    let old_brush = SelectObject(hdc, null_brush);
                    let _ = rectangle_gdi(hdc, 0, 0, w - 1, h - 1);
                    let _ = SelectObject(hdc, old_brush);
                    let _ = SelectObject(hdc, old_pen);
                    let _ = DeleteObject(border);
                }
                // 布局参数（与 candwin2 show() 同公式）
                let font_pt = skin_layout(skin, "font_point", 16.0);
                let line_h = (font_pt * 96.0 / 72.0 + skin_layout(skin, "line_spacing", 3.0) + 5.0) as i32;
                let margin_x = skin_layout(skin, "margin_x", 8.0) as i32;
                let margin_y = skin_layout(skin, "margin_y", 5.0) as i32;
                let label_pt = skin_layout(skin, "label_font_point", 0.0);
                let label_h = if label_pt > 0.0 { label_pt } else { font_pt } as i32 * 96 / 72;
                let hi_radius = skin_layout(skin, "hilited_corner_radius", 6.0).max(0.0) as i32;
                let face: Vec<u16> = {
                    let mut v: Vec<u16> = skin_font_face(skin).encode_utf16().collect();
                    v.push(0);
                    v
                };
                let font_h = (font_pt * 96.0 / 72.0).round() as i32;
                let hfont = CreateFontW(
                    -font_h.max(12), 0, 0, 0, 400, 0, 0, 0, 0x86, 0, 0, 5, 0, face.as_ptr(),
                );
                let old = SelectObject(hdc, hfont);
                let _ = SetBkMode(hdc, 1); // TRANSPARENT
                let mut y = margin_y;
                // 编码行
                let raw_ws: Vec<u16> = f.raw.encode_utf16().collect();
                if !raw_ws.is_empty() {
                    let _ = SetTextColor(hdc, skin_color(skin, "text_color", "#E8E8EAFF"));
                    let _ = TextOutW(hdc, margin_x, y, raw_ws.as_ptr(), raw_ws.len() as i32);
                }
                y += line_h;
                // 候选行（竖排，与记事本里的自绘窗同观感）
                for (i, (text, cmt)) in f.items.iter().take(5).enumerate() {
                    let sel = i == f.selected;
                    if sel {
                        let hl = CreateSolidBrush(skin_color(skin, "hilited_candidate_back_color", "#404046FF"));
                        let hl_rgn = CreateRoundRectRgn(
                            margin_x / 2, y - 1, w - margin_x / 2, y + line_h - 2,
                            hi_radius.max(1), hi_radius.max(1),
                        );
                        let _ = FillRgn(hdc, hl_rgn, hl);
                        let _ = DeleteObject(hl_rgn);
                        let _ = DeleteObject(hl);
                    }
                    let mut x = margin_x + 2;
                    // 序号
                    let num = format!("{}.", i + 1);
                    let mut nws: Vec<u16> = num.encode_utf16().collect();
                    let _ = SetTextColor(
                        hdc,
                        if sel {
                            skin_color(skin, "hilited_candidate_label_color", "#FFD75EFF")
                        } else {
                            skin_color(skin, "label_color", "#C9C9C9FF")
                        },
                    );
                    let _ = TextOutW(hdc, x, y + (line_h - label_h) / 3, nws.as_ptr(), nws.len() as i32);
                    let mut nsz = SIZE { cx: 0, cy: 0 };
                    let _ = GetTextExtentPoint32W(hdc, nws.as_ptr(), nws.len() as i32, &mut nsz);
                    x += nsz.cx + 6;
                    // 候选文本
                    let mut bws: Vec<u16> = text.encode_utf16().collect();
                    let _ = SetTextColor(
                        hdc,
                        if sel {
                            skin_color(skin, "hilited_candidate_text_color", "#FFFFFFFF")
                        } else {
                            skin_color(skin, "candidate_text_color", "#E8E8EAFF")
                        },
                    );
                    let _ = TextOutW(hdc, x, y, bws.as_ptr(), bws.len() as i32);
                    let mut bsz = SIZE { cx: 0, cy: 0 };
                    let _ = GetTextExtentPoint32W(hdc, bws.as_ptr(), bws.len() as i32, &mut bsz);
                    x += bsz.cx + 8;
                    // 注释
                    if !cmt.is_empty() {
                        let mut cws: Vec<u16> = cmt.encode_utf16().collect();
                        let _ = SetTextColor(
                            hdc,
                            if sel {
                                skin_color(skin, "hilited_comment_text_color", "#C9C9C9FF")
                            } else {
                                skin_color(skin, "comment_text_color", "#9A9AA0FF")
                            },
                        );
                        let _ = TextOutW(hdc, x, y, cws.as_ptr(), cws.len() as i32);
                    }
                    y += line_h;
                }
                let _ = SelectObject(hdc, old);
                let _ = DeleteObject(hfont);
            }
            let _ = EndPaint(hwnd, &ps);
            0
        },
        WM_ERASEBKGND => {
            // 自擦：背景在 WM_PAINT 里整面画，阻 DefWindowProc/系统白底
            1
        }
        WM_APP_CAND => unsafe {
            // lparam = Box<CandFrame> 指针（pipe 线程移交所有权）
            let frame: Box<CandFrame> = Box::from_raw(lparam as *mut CandFrame);
            let n = frame.items.len().min(5);
            // 竖排尺寸（与 DLL candwin2 同公式）：宽=皮肤 width/min_width，
            // 高=上下边距 + 编码行 + n 行候选
            let font_pt = skin_layout(&frame.skin, "font_point", 16.0);
            let line_h = (font_pt * 96.0 / 72.0 + skin_layout(&frame.skin, "line_spacing", 3.0) + 5.0) as i32;
            let margin_y = skin_layout(&frame.skin, "margin_y", 5.0) as i32;
            let w = skin_layout(&frame.skin, "width", 0.0)
                .max(skin_layout(&frame.skin, "min_width", 150.0).max(100.0))
                .min(420.0) as i32;
            let h = margin_y * 2 + line_h * (n as i32 + 1);
            let x = (wparam >> 32) as i32;
            let y = (wparam as u32) as i32;
            let radius = skin_layout(&frame.skin, "corner_radius", 8.0).max(0.0) as i32;
            *frame_lock() = Some(*frame);
            // 沉浸层压 topmost → AttachThreadInput 组合拳强制置顶。
            // 只在「隐藏→再次显示」时做 NOTOPMOST→TOPMOST 重挂（每帧
            // 重挂会闪，用户实测「闪现」即此）。
            let first_show = !was_raised();
            force_top(hwnd, x, y, w, h, first_show);
            mark_raised();
            // 皮肤圆角（layout.corner_radius，默认 8）
            let rgn = if radius > 0 {
                CreateRoundRectRgn(0, 0, w + 1, h + 1, radius * 2, radius * 2)
            } else {
                0
            };
            if rgn != 0 {
                let _ = SetWindowRgn(hwnd, rgn, 1);
            }
            let _ = InvalidateRect(hwnd, std::ptr::null(), 1);
            0
        },
        WM_APP_HIDE => unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
            *frame_lock() = None;
            RAISED.store(false, std::sync::atomic::Ordering::SeqCst);
            0
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[link(name = "user32")]
extern "system" {
    fn FillRect(hdc: isize, r: *const RECT, brush: isize) -> i32;
    fn BringWindowToTop(hwnd: isize) -> i32;
    fn IsWindow(hwnd: isize) -> i32;
    fn SetWindowRgn(hwnd: isize, rgn: isize, redraw: i32) -> i32;
}

const HWND_NOTOPMOST: isize = -2;
const WM_APP_REINIT: u32 = 0x8003;
const WM_ERASEBKGND: u32 = 0x0014;

/// 本轮显示会话是否已做过重挂置顶（防每帧重挂闪烁）
static RAISED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn was_raised() -> bool {
    RAISED.load(std::sync::atomic::Ordering::SeqCst)
}

fn mark_raised() {
    RAISED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// 强制置顶：沉浸层（开始菜单全屏背景）压普通 topmost 窗。
/// NOTOPMOST→TOPMOST 重挂即可越过沉浸层（实测像素可见）。
/// 【禁忌】不要 AttachThreadInput：挂到宿主（AppContainer）线程后
/// 若对方不泵消息，本线程整体卡死——实测白窗+忙碌光标+候选永不再现。
/// rebanded=真时做重挂（仅会话首帧，防闪）。
unsafe fn force_top(hwnd: isize, x: i32, y: i32, w: i32, h: i32, rebanded: bool) {
    if rebanded {
        let _ = SetWindowPos(hwnd, HWND_NOTOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
    let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    let _ = BringWindowToTop(hwnd);
}

/// 中毒免疫锁：panic 后状态仍在，读旧值胜过整线程卡死
fn frame_lock() -> std::sync::MutexGuard<'static, Option<CandFrame>> {
    FRAME.lock().unwrap_or_else(|e| e.into_inner())
}

/// RAII 无关的别名（FillRect 在 user32）
unsafe fn fill_rect(hdc: isize, r: *const RECT, brush: isize) -> i32 {
    FillRect(hdc, r, brush)
}

/// pipe 线程调用：显示/更新候选（坐标=光标屏幕位置；沉浸式宿主下
/// DLL 已写死为屏幕左上角）
pub fn show(frame: CandFrame, x: i32, y: i32) {
    // 宿主（如 SearchHost）退出会连带销毁子窗口——检测死后请求
    // tray 线程重建（窗口必须由有消息循环的线程创建）
    {
        let hwnd = *WND.lock().unwrap();
        let dead = hwnd.map(|h| unsafe { IsWindow(h) } == 0).unwrap_or(true);
        if dead {
            let tray = crate::tray::tray_hwnd();
            if tray != 0 {
                unsafe {
                    let _ = PostMessageW(tray, WM_APP_REINIT, 0, 0);
                }
            }
            let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
            let _ = std::fs::write(r"C:\ProgramData\HuFu\diag\srv-cand.txt", "show: 窗口已死，请求重建\n");
            return;
        }
    }
    let hwnd = match *WND.lock().unwrap() {
        Some(h) => h,
        None => {
            let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
            let _ = std::fs::write(r"C:\ProgramData\HuFu\diag\srv-cand.txt", "show: WND=None（窗口未建）\n");
            return;
        }
    };
    let n = frame.items.len();
    let boxed = Box::into_raw(Box::new(frame));
    let wp = ((x as u64 as usize) << 32) | (y as u32 as usize);
    unsafe {
        let r = PostMessageW(hwnd, WM_APP_CAND, wp, boxed as isize);
        let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
        let _ = std::fs::write(
            r"C:\ProgramData\HuFu\diag\srv-cand.txt",
            format!("show n={n} x={x} y={y} post={r}\n"),
        );
    }
}

/// pipe 线程调用：隐藏
pub fn hide() {
    let hwnd = match *WND.lock().unwrap() {
        Some(h) => h,
        None => return,
    };
    unsafe {
        let _ = PostMessageW(hwnd, WM_APP_HIDE, 0, 0);
    }
}

/// 在 tray 线程创建（消息循环已有）：注册类 + 隐藏窗口
pub fn init_on_tray_thread() {
    let class: Vec<u16> = "HuFuSrvCand\0".encode_utf16().collect();
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: wnd_proc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: 0,
        hIcon: 0,
        hCursor: 0,
        hbrBackground: 0,
        lpszMenuName: std::ptr::null(),
        lpszClassName: class.as_ptr(),
    };
    let _atom = unsafe { RegisterClassW(&wc) };
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            class.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            0, 0, 200, 60,
            0, 0, 0,
            std::ptr::null(),
        )
    };
    if hwnd != 0 {
        *WND.lock().unwrap() = Some(hwnd);
    }
    let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
    let _ = std::fs::write(
        r"C:\ProgramData\HuFu\diag\srv-cand.txt",
        format!("init atom={_atom} hwnd={hwnd}\n"),
    );
}

/// tray 线程：宿主退出销毁子窗口后重建（由 tray wnd_proc 0x8003 调用）
pub fn reinit_if_dead() {
    let dead = {
        let hwnd = *WND.lock().unwrap();
        hwnd.map(|h| unsafe { IsWindow(h) } == 0).unwrap_or(true)
    };
    if dead {
        init_on_tray_thread();
    }
}
