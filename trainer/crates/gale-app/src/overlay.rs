//! 二级页独立顶层窗口。
//!
//! 面板不再画在主窗 client 内，而是由一个独立 `WS_POPUP` 窗口承载：
//! - 自己的 DirectComposition `Renderer`；
//! - 鼠标按“屏幕点 → 主窗客户区逻辑像素”转发，命中表仍由主窗维护；
//! - 顶栏空白区按住可手动拖动窗口到主窗之外。
//!
//! 这里只做窗口/渲染/输入转发，不改主窗的 Modal 状态机。

use gale_win::d2d::{Frame, Renderer};
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static CLASS_ATOM: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

const CLASS_NAME: PCWSTR = w!("HuFuOverlayWindow");

pub struct Overlay {
    hwnd: HWND,
    renderer: Renderer,
    w: u32,
    h: u32,
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEACTIVATE => LRESULT(MA_ACTIVATE as isize),
        // 滚轮：lParam 是屏幕坐标（与其它鼠标消息不同），按主窗客户区换算后转发，
        // 主窗的 panel_scroll / 列表滚动逻辑照常生效（小屏上面板需要滚动时）。
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            forward_screen_to_main(msg, wp, lp);
            LRESULT(0)
        }
        // 【命中 2026-11-14】overlay 自己不再处理标题栏拖动：原来「header 带里
        // 直接吞掉 LBUTTONDOWN 进入拖动」把 × 与右上角按钮的点击一起吃掉了
        // （用户实测「× 那里点不动、点黑边右上角反而能关」）。现在所有鼠标消息
        // 一律转发给主窗，由主窗按命中表分派；拖动改由主窗的 modal_drag 走
        // `modal_shift`，窗口位置每帧按面板几何跟随（main.rs 的 set_rect）。
        WM_LBUTTONDOWN => {
            // 【拖动修复 2026-11-14】按下时把鼠标抓到 overlay：Win32 没有隐式
            // 捕获，不 SetCapture 的话按住期间 WM_MOUSEMOVE 只会落到光标
            // 下方的窗口——光标一旦离开 overlay 界（拖动必然发生），overlay
            // 一条 Move 都收不到，forward 断供、面板纹丝不动（用户实测
            // 「拖不出跟打器」的根因）。抓到 overlay 后整个按住期间 Move
            // 全部转发主窗，面板跟手；松开在 WM_LBUTTONUP 释放。子窗
            // （RichEdit）上的点击不经过 overlay_proc，文本编辑不受影响。
            unsafe {
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd);
            }
            forward_to_main(hwnd, msg, wp, lp);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            // 【拖动排查 2026-11-14b】左键按住期间的 overlay→main 转发插桩：
            // 确认捕获窗口是 overlay 且 Move 消息真的在往主窗转发。
            let held = unsafe {
                windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(
                    windows::Win32::UI::Input::KeyboardAndMouse::VK_LBUTTON.0 as i32,
                ) < 0
            };
            if held {
                let (cx, cy) = (
                    (lp.0 as u32 & 0xffff) as i16 as i32,
                    ((lp.0 as u32 >> 16) & 0xffff) as i16 as i32,
                );
                gale_win::input_host::diag_log(&format!("OVMOVE client=({cx},{cy})"));
            }
            forward_to_main(hwnd, msg, wp, lp);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            // 【拖动修复 2026-11-14】与按下时的 SetCapture 配对释放。
            unsafe {
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
            }
            forward_to_main(hwnd, msg, wp, lp);
            LRESULT(0)
        }
        WM_RBUTTONDOWN | WM_RBUTTONUP => {
            forward_to_main(hwnd, msg, wp, lp);
            LRESULT(0)
        }
        WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
            let main = HWND(MAIN_HWND.load(Ordering::Relaxed) as *mut _);
            if !main.0.is_null() {
                let _ = unsafe { PostMessageW(main, msg, wp, lp) };
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let main = HWND(MAIN_HWND.load(Ordering::Relaxed) as *mut _);
            if !main.0.is_null() {
                let _ = unsafe { PostMessageW(main, WM_KEYDOWN, WPARAM(0x1B), LPARAM(0)) };
            }
            // 窗口自身不关；交主窗决定（主窗下一帧看到 modal 已关闭会隐藏 overlay）。
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// 转发「lParam 已是屏幕坐标」的消息（滚轮）：原样透传屏幕坐标。
/// 主窗 WM_MOUSEWHEEL 本来就按屏幕坐标处理（ScreenToClient + lx() 除 scale），
/// 这里再做任何换算都会造成二次平移/二次除 scale → 停靠面板/成绩面板命中失效。
/// 【滚轮坐标 2026-11-14】
fn forward_screen_to_main(msg: u32, wp: WPARAM, lp: LPARAM) {
    let main = HWND(MAIN_HWND.load(Ordering::Relaxed) as *mut _);
    if main.0.is_null() {
        return;
    }
    unsafe {
        let _ = PostMessageW(main, msg, wp, lp);
        let _ = PostMessageW(main, WM_NULL, WPARAM(0), LPARAM(0));
    }
}

/// 屏幕工作区（主窗客户区 DIP 坐标系）：`(l,t,r,b)`。
/// `near`＝屏幕物理像素锚点：Some 时按该点选显示器（拖动跟随目标屏），
/// None 时按主窗所在显示器（旧行为）。
fn work_area_dip(main: HWND, near: Option<POINT>) -> Option<(f32, f32, f32, f32)> {
    unsafe {
        // 【多屏 2026-11-14】按请求矩形中心选显示器：拖动把面板带向另一屏时
        // 工作区跟随目标屏；原 MonitorFromWindow(main) 永远主屏，多屏拖不过去。
        // 单屏下两者恒等，行为不变。
        let mon = match near {
            Some(pt) => MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST),
            None => MonitorFromWindow(main, MONITOR_DEFAULTTONEAREST),
        };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(mon, &mut info).as_bool() {
            return None;
        }
        let mut origin = POINT { x: 0, y: 0 };
        if !ClientToScreen(main, &mut origin).as_bool() {
            return None;
        }
        let sc = gale_win::window::dpi_scale().max(0.01);
        let left = (info.rcWork.left - origin.x) as f32 / sc;
        let top = (info.rcWork.top - origin.y) as f32 / sc;
        let right = (info.rcWork.right - origin.x) as f32 / sc;
        let bottom = (info.rcWork.bottom - origin.y) as f32 / sc;
        Some((left, top, right, bottom))
    }
}

/// 把理想面板窗口 `(x,y,w,h)`（主窗客户区 DIP）收进屏幕工作区并返回新几何。
/// 小屏（Rules 948 DIP 高、Preferences 808）上高度被压缩，内容改由面板滚动承接，
/// 页脚仍钉在可视底边；工作区取不到时原样返回。
pub fn fit_to_work(main: isize, x: f32, y: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
    fit_impl(main, x, y, w, h, false)
}

/// 【拖出屏幕 2026-11-14】宽松版收边：拖动/已挪动过的面板允许大部分推出
/// 屏幕外，只保证顶部一条 `KEEP_W×KEEP_H` 的抓回带（含 × 按钮）留在工作
/// 区内——与原生窗口「可拖出屏幕但不至于彻底丢失」的语义一致。用户实测
/// 「不能拖出屏幕」指的是旧 fit_to_work 把整窗硬钳进工作区。
pub fn fit_loose(main: isize, x: f32, y: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
    fit_impl(main, x, y, w, h, true)
}

/// 抓回带尺寸（DIP）：宽 200 保证标题文字 + × 可见，高 64 覆盖标题栏带。
const KEEP_W: f32 = 200.;
const KEEP_H: f32 = 64.;

fn fit_impl(main: isize, x: f32, y: f32, w: f32, h: f32, loose: bool) -> (f32, f32, f32, f32) {
    // 【多屏 2026-11-14】用「请求矩形中心」当显示器锚点：面板想去的屏就是
    // 它被钳进的工作区（拖动跨屏时一旦中心越过屏界即换区），而不是永远
    // 被主窗所在屏的工作区拽回来。
    let near = unsafe {
        let mut origin = POINT { x: 0, y: 0 };
        if ClientToScreen(HWND(main as *mut _), &mut origin).as_bool() {
            let sc = gale_win::window::dpi_scale().max(0.01);
            Some(POINT {
                x: origin.x + ((x + w / 2.) * sc) as i32,
                y: origin.y + ((y + h / 2.) * sc) as i32,
            })
        } else {
            None
        }
    };
    let Some((left, top, right, bottom)) = work_area_dip(HWND(main as *mut _), near) else {
        return (x, y, w, h);
    };
    clamp_into_work(x, y, w, h, left, top, right, bottom, loose)
}

/// 纯数学收边（无 Win32 依赖，可离屏单测）：先把尺寸压进工作区
/// （≤area−16、≥320/240 下限），再按两种语义钳位置：
/// - tight：整窗收进工作区（留 8px 边）；
/// - loose：只保 KEEP_W×KEEP_H 抓回带在工作区内（窗口可大部分出屏）。
/// 坐标全为主窗客户区 DIP，工作区为 `(left,top,right,bottom)`。
pub fn clamp_into_work(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    loose: bool,
) -> (f32, f32, f32, f32) {
    let (area_w, area_h) = ((right - left).max(320.), (bottom - top).max(240.));
    let w = w.min(area_w - 16.).max(320.);
    let h = h.min(area_h - 16.).max(240.);
    if loose {
        // 只锚「顶部抓回带与工作区相交 ≥ KEEP_W×KEEP_H」：
        // x∈[left+KEEP_W−w, right−KEEP_W]，y∈[top+KEEP_H−h, bottom−KEEP_H]。
        // 窗宽恒 ≥320 > KEEP_W，区间恒非空；显示区异常小时退回紧钳。
        let x = x.clamp(left + KEEP_W - w, (right - KEEP_W).max(left + KEEP_W - w));
        let y = y.clamp(top + KEEP_H - h, (bottom - KEEP_H).max(top + KEEP_H - h));
        (x, y, w, h)
    } else {
        let x = x.clamp(left + 8., (right - 8. - w).max(left + 8.));
        let y = y.clamp(top + 8., (bottom - 8. - h).max(top + 8.));
        (x, y, w, h)
    }
}

/// 【拖动跟手 2026-11-14】绝对坐标语义的拖动位移求解：
/// `shift = shift0 + cursor − grab`。
/// overlay 转发坐标经 ClientToScreen 换算后是「屏幕锚定」的绝对坐标，
/// 手位移只取决于光标两端点，与窗口自身移动无关——半速 bug（迭代式
/// `s ← s + (ΔM − ΔW)` 的不动点 s*=ΔM/2+shift0）在数学上不可能出现。
pub fn drag_shift_absolute(
    shift0: (f32, f32),
    grab: (f32, f32),
    cursor: (f32, f32),
) -> (f32, f32) {
    (
        shift0.0 + cursor.0 - grab.0,
        shift0.1 + cursor.1 - grab.1,
    )
}

fn forward_to_main(overlay: HWND, msg: u32, wp: WPARAM, lp: LPARAM) {
    let main = HWND(MAIN_HWND.load(Ordering::Relaxed) as *mut _);
    if main.0.is_null() {
        return;
    }
    unsafe {
        let mut p = POINT {
            x: (lp.0 as u32 & 0xffff) as i16 as i32,
            y: ((lp.0 as u32 >> 16) & 0xffff) as i16 as i32,
        };
        if !ClientToScreen(overlay, &mut p).as_bool() {
            return;
        }
        let mut origin = POINT { x: 0, y: 0 };
        if !ClientToScreen(main, &mut origin).as_bool() {
            return;
        }
        // 【命中 2026-11-14 根因】主窗 wndproc 收到 WM_LBUTTONDOWN 后会自己做
        // `lx(v) = v / dpi_scale` 转逻辑像素。这里若再除一次，坐标就被缩放两遍
        // （125% 下点 (1161,67) 变成 (743,43)），表现为「UI 与点击位置对不上」。
        // 所以这里只做「overlay 客户区 → 主窗客户区」的物理像素换算，不再除 scale。
        let x = p.x - origin.x;
        let y = p.y - origin.y;
        let packed = (((y as i16 as u16) as isize) << 16) | ((x as i16 as u16) as isize);
        let _ = PostMessageW(main, msg, wp, LPARAM(packed));
        // 让主循环醒来消费点击。
        let _ = PostMessageW(main, WM_NULL, WPARAM(0), LPARAM(0));
    }
}

impl Overlay {
    /// 在主窗客户区逻辑坐标 `(x,y,w,h)`（DIP）处创建独立面板窗口。
    pub fn create(
        main: isize,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        scale: f32,
    ) -> windows::core::Result<Self> {
        unsafe {
            MAIN_HWND.store(main, Ordering::Relaxed);
            let _ = CLASS_ATOM.get_or_init(|| {
                let hinst = GetModuleHandleW(None).unwrap_or_default();
                let wc = WNDCLASSW {
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(overlay_proc),
                    hInstance: hinst.into(),
                    lpszClassName: CLASS_NAME,
                    hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                    ..Default::default()
                };
                RegisterClassW(&wc) != 0
            });

            let mut origin = POINT { x: 0, y: 0 };
            let _ = ClientToScreen(HWND(main as *mut _), &mut origin);
            let sx = origin.x + (x * scale).round() as i32;
            let sy = origin.y + (y * scale).round() as i32;
            let pw = (w * scale).round() as i32;
            let ph = (h * scale).round() as i32;

            let mut title: Vec<u16> = "虎符跟打".encode_utf16().collect();
            title.push(0);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                CLASS_NAME,
                PCWSTR(title.as_ptr()),
                WS_POPUP | WS_CLIPCHILDREN,
                sx,
                sy,
                pw.max(1),
                ph.max(1),
                HWND(main as *mut _),
                None,
                GetModuleHandleW(None)?,
                None,
            )?;
            // 【2026-11-14】独立窗要挂原生 RichEdit（规则名/正则/替换、服务器输入框）。
            // 建窗时必须带 WS_CLIPCHILDREN：否则 DComp 合成内容会盖住同窗 child HWND
            // （实测控件有文本、可输入、样式正常，却一个像素都不画；换成绑定 HWND 的
            // 交换链也一样被盖）。带上 WS_CLIPCHILDREN 后子窗回到合成内容之上，正常显示。
            let mut renderer = match Renderer::new(hwnd.0 as isize, pw.max(1) as u32, ph.max(1) as u32) {
                Ok(renderer) => renderer,
                Err(error) => {
                    let _ = DestroyWindow(hwnd);
                    return Err(error);
                }
            };
            // 【帧缓存 2026-11-14】overlay 的 WM_SIZE 由 set_rect 每帧同步几何
            // 并立即补帧，last_frame 重放缓存无消费者——关掉省每帧整表深拷贝。
            renderer.set_keep_last_frame(false);
            Ok(Self {
                hwnd,
                renderer,
                w: pw.max(1) as u32,
                h: ph.max(1) as u32,
            })
        }
    }

    pub fn hwnd_isize(&self) -> isize {
        self.hwnd.0 as isize
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// 【overlay 字体 2026-11-14】把主窗的 cfg.font_family 同步到独立窗
    /// Renderer 的 FontKit（含 layout 缓存失效），与主窗口径一致。
    pub fn set_body_family(&mut self, requested: &str) {
        self.renderer.set_body_family(requested);
    }

    /// 当前窗口左上角相对主窗客户区的逻辑坐标（DIP）。
    pub fn origin_in_main(&self) -> (f32, f32) {
        unsafe {
            let main = HWND(MAIN_HWND.load(Ordering::Relaxed) as *mut _);
            let mut p = POINT { x: 0, y: 0 };
            let _ = ClientToScreen(self.hwnd, &mut p);
            let mut origin = POINT { x: 0, y: 0 };
            let _ = ClientToScreen(main, &mut origin);
            let sc = gale_win::window::dpi_scale().max(0.01);
            ((p.x - origin.x) as f32 / sc, (p.y - origin.y) as f32 / sc)
        }
    }

    /// 【命中与尺寸 2026-11-14】面板尺寸/位置随内容变化（设置页切标签后
    /// `preferences::content_height` 变高变矮、拖动 `modal_shift` 改位置）时，
    /// 窗口必须跟着走：否则卡片只占窗口一部分，多出来的透明圈会吃掉边缘点击
    /// （用户实测「UI 与点击对不上」）。x/y/w/h 都是主窗客户区 DIP。
    pub fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32, scale: f32) -> windows::core::Result<()> {
        unsafe {
            let main = HWND(MAIN_HWND.load(Ordering::Relaxed) as *mut _);
            let mut origin = POINT { x: 0, y: 0 };
            if !main.0.is_null() {
                let _ = ClientToScreen(main, &mut origin);
            }
            let sx = origin.x + (x * scale).round() as i32;
            let sy = origin.y + (y * scale).round() as i32;
            let pw = (w * scale).round() as i32;
            let ph = (h * scale).round() as i32;
            let mut rc = RECT::default();
            let _ = GetWindowRect(self.hwnd, &mut rc);
            let same = rc.left == sx
                && rc.top == sy
                && rc.right - rc.left == pw.max(1)
                && rc.bottom - rc.top == ph.max(1);
            if same {
                return Ok(());
            }
            // 【拖动排查 2026-11-14b】窗口几何变更插桩：确认 set_rect 真的
            // 在动窗口（same=true 的静默帧不打，避免刷屏）。
            gale_win::input_host::diag_log(&format!(
                "SETRECT want=({sx},{sy},{pw},{ph}) cur=({},{},{},{})",
                rc.left,
                rc.top,
                rc.right - rc.left,
                rc.bottom - rc.top
            ));
            let _ = SetWindowPos(
                self.hwnd,
                None,
                sx,
                sy,
                pw.max(1),
                ph.max(1),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            if pw.max(1) != self.w as i32 || ph.max(1) != self.h as i32 {
                self.w = pw.max(1) as u32;
                self.h = ph.max(1) as u32;
                self.renderer.resize(self.w, self.h)?;
            }
        }
        Ok(())
    }

    pub fn present(&mut self, panel: &Frame) -> windows::core::Result<()> {
        self.renderer.set_draw_offset(0.0, 0.0);
        let _ = (self.w, self.h);
        self.renderer.draw_frame(panel)
    }

    /// Export this window's rendered surface. A dock uses a translated draw
    /// origin, but its clipboard crop is always local to the overlay surface.
    pub fn copy_bmp_to_clipboard(
        &mut self,
        crop: Option<(u32, u32, u32, u32)>,
    ) -> std::result::Result<(), String> {
        self.renderer.copy_bmp_to_clipboard(crop)
    }

    /// 【截图复制 2026-11-15】导出本窗渲染面（供跨屏拼接整篇文章）。
    pub fn export_dib(
        &mut self,
        crop: Option<(u32, u32, u32, u32)>,
    ) -> std::result::Result<Vec<u8>, String> {
        self.renderer.export_dib(crop)
    }

    /// 把拼好的整篇 DIB 写进剪贴板。
    pub fn write_dib_to_clipboard(&mut self, dib: &[u8]) -> std::result::Result<(), String> {
        self.renderer.write_dib_to_clipboard(dib)
    }

    pub fn retain_export_frame(&mut self, enabled: bool) {
        self.renderer.set_keep_last_frame(enabled);
    }

    /// 用主帧坐标绘制：`(dx,dy)` 是希望显示的区域左上角（DIP）。
    /// overlay 窗口只显示该区域，其余被窗口裁掉。
    pub fn present_region(
        &mut self,
        frame: &Frame,
        dx: f32,
        dy: f32,
    ) -> windows::core::Result<()> {
        self.renderer.set_draw_offset(dx, dy);
        self.renderer.draw_frame(frame)
    }

    pub fn show(&self, show: bool) {
        unsafe {
            // 最小化/隐藏主窗时不许把面板再拉回桌面（见 panel_show_allowed）。
            let want = panel_show_allowed(show, owner_allows_show(), false);
            if IsWindowVisible(self.hwnd).as_bool() == want {
                // 【层级修复 2026-11-15】主窗被 SetForegroundWindow 抬到前面后，
                // owned 的 overlay 会被压在下面：看得见却点不到（点击全落到主窗）。
                // 每帧把已在显示的面板抬到 owner 之上，保证整页浮窗可点。
                if want {
                    let _ = SetWindowPos(
                        self.hwnd,
                        HWND_TOP,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
                return;
            }
            if !want {
                // 【拖动修复 2026-11-14】隐藏前释放按下时抓的鼠标：隐藏窗口
                // 若继续持有 capture，用户随后的点击会被它吃掉转发进主窗
                // （幽灵捕获）。销毁路径 DestroyWindow 系统自动释放，无需处理。
                if windows::Win32::UI::Input::KeyboardAndMouse::GetCapture() == self.hwnd {
                    let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                }
            }
            let _ = ShowWindow(self.hwnd, if want { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            // 【拖动排查 2026-11-14】销毁插桩：拿错误码地面真相。
            let r = DestroyWindow(self.hwnd);
            let (okv, err): (bool, i32) = match &r {
                Ok(_) => (true, 0),
                Err(e) => (false, e.code().0),
            };
            let still = IsWindow(self.hwnd);
            let gerr = GetLastError().0;
            gale_win::input_host::diag_log(&format!(
                "overlay drop: hwnd={:p} DestroyWindow={} err={} is_window_after={} gerr={}",
                self.hwnd.0, okv, err, still.as_bool(), gerr
            ));
            let _ = r;
        }
    }
}

/// 【审计修复 2026-11-15】独立面板是否允许显示：只要主窗不可见或已最小化，
/// 一律不允许。
///
/// 用户实测：打开「理论分析」后最小化跟打器，桌面上留下一个孤零零的分析
/// 面板浮窗。面板是 WS_POPUP 独立窗（owner=主窗），系统在最小化时会连坐
/// 隐藏它；但本程序最小化后仍继续跑帧循环，每帧又 `show(true)` 把它拉回屏幕。
/// 开关统一收在这里，调用方无需各自判断。
fn panel_show_allowed(requested: bool, owner_visible: bool, owner_minimized: bool) -> bool {
    requested && owner_visible && !owner_minimized
}

/// owner（主窗）当前是否处于可显示状态；没有主窗时放行（独立测试路径）。
fn owner_allows_show() -> bool {
    let main = HWND(MAIN_HWND.load(Ordering::Relaxed) as *mut _);
    if main.0.is_null() {
        return true;
    }
    unsafe { IsWindowVisible(main).as_bool() && !IsIconic(main).as_bool() }
}

#[cfg(test)]
mod fit_tests {
    use super::*;

    /// 【审计修复 2026-11-15】主窗最小化/隐藏时，面板窗一律不许再显示。
    #[test]
    fn panel_never_shows_while_the_main_window_is_minimized_or_hidden() {
        assert!(panel_show_allowed(true, true, false));
        assert!(!panel_show_allowed(true, false, false));
        assert!(!panel_show_allowed(true, true, true));
        assert!(!panel_show_allowed(false, true, false));
    }

    /// 本机工作区（DPI 100%，主窗在原点时 DIP==物理）。
    const WORK: (f32, f32, f32, f32) = (0., 0., 2520., 1632.);

    #[test]
    fn loose_allows_mostly_offscreen_keeps_grab_band() {
        // 992x800 面板右推到 x=2000：窗右缘 2992 已出屏（>2520），
        // 但顶部抓回带 200x64 仍在工作区内 → 不钳（用户「能拖出屏幕」）。
        let (x, y, w, h) = clamp_into_work(2000., 700., 992., 800., WORK.0, WORK.1, WORK.2, WORK.3, true);
        assert_eq!((x, y, w, h), (2000., 700., 992., 800.));
        // 顶推 y=-40：窗大部分在屏上方，顶部 64px 抓回带在内 → 不钳。
        let (x, y, _, _) = clamp_into_work(500., -40., 992., 800., WORK.0, WORK.1, WORK.2, WORK.3, true);
        assert_eq!((x, y), (500., -40.));
    }

    #[test]
    fn loose_pulls_back_when_band_would_leave() {
        // x=2600 时抓回带左缘 2600 ≥ right−KEEP_W=2320 → 拉回 2320。
        let (x, _, _, _) = clamp_into_work(2600., 700., 992., 800., WORK.0, WORK.1, WORK.2, WORK.3, true);
        assert_eq!(x, WORK.2 - KEEP_W);
        // y=-800 时窗顶出屏超过 KEEP_H → 拉回 top+KEEP_H−h = −736。
        let (_, y, _, _) = clamp_into_work(500., -800., 992., 800., WORK.0, WORK.1, WORK.2, WORK.3, true);
        assert_eq!(y, WORK.1 + KEEP_H - 800.);
    }

    #[test]
    fn tight_keeps_whole_window_inside() {
        // 紧钳：整窗收进工作区留 8px（repro-drag4 D3 实测右缘 2520-8-992=1520）。
        let (x, _, w, _) = clamp_into_work(2000., 700., 992., 800., WORK.0, WORK.1, WORK.2, WORK.3, false);
        assert_eq!((x, w), (1520., 992.));
        let (_, y, _, h) = clamp_into_work(500., -40., 992., 800., WORK.0, WORK.1, WORK.2, WORK.3, false);
        assert_eq!((y, h), (8., 800.));
    }

    #[test]
    fn oversized_panel_caps_to_area() {
        // 高 1008 > 工作区 1632−16：压到 1616；宽同理；下限 320/240。
        let (_, _, w, h) = clamp_into_work(0., 0., 9999., 1008., WORK.0, WORK.1, WORK.2, WORK.3, false);
        assert_eq!((w, h), (2504., 1008.));
        let (_, _, w, h) = clamp_into_work(0., 0., 100., 100., WORK.0, WORK.1, WORK.2, WORK.3, false);
        assert_eq!((w, h), (320., 240.));
    }

    #[test]
    fn tiny_workarea_stays_sane() {
        // 800x480 微型工作区（老上网本）：尺寸仍被压到 area−16，钳区间非空。
        let (x, y, w, h) = clamp_into_work(0., 0., 992., 800., 0., 0., 800., 480., false);
        assert_eq!((w, h), (784., 464.));
        assert!((0. ..=800. - 8. - w).contains(&x));
        assert!((0. ..=480. - 8. - h).contains(&y));
    }
}

#[cfg(test)]
mod drag_tests {
    use super::*;

    #[test]
    fn absolute_shift_equals_cursor_delta() {
        // 光标 500,300 → 400,250，shift0=(0,0)：shift 精确等于光标位移。
        let s = drag_shift_absolute((0., 0.), (500., 300.), (400., 250.));
        assert_eq!(s, (-100., -50.));
        // 非零 shift0（面板已被挪过）叠加。
        let s = drag_shift_absolute((120., -60.), (500., 300.), (450., 320.));
        assert_eq!(s, (70., -40.));
    }

    #[test]
    fn absolute_shift_immune_to_window_motion() {
        // 半速回归钉：窗口随手移动（每步窗口位移==面板 shift 增量）时，
        // 求解仍只看光标两端点。旧迭代式 s←s+(ΔM−ΔW) 在窗口跟手
        // （ΔW==ΔM）时恒不动——恰是「纹丝不动」bug；绝对式无此病。
        let shift0 = (0f32, 0f32);
        let grab = (1000f32, 800f32);
        for cursor in [940f32, 880., 820.] {
            let s = drag_shift_absolute(shift0, grab, (cursor, 800.));
            assert_eq!(s, (cursor - 1000., 0.));
        }
    }
}
