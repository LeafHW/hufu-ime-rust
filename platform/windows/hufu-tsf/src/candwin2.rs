//! 候选窗 v2：D3D11 + DirectComposition + Direct2D + DWM 真实材质。
//!
//! - 窗口：WS_POPUP + WS_EX_NOREDIRECTIONBITMAP（DComp 直通，逐像素 alpha）
//! - 材质（皮肤 material.kind）→ SetWindowCompositionAttribute accent：
//!   solid=不透明 / translucent=半透明渐变 / frosted=Acrylic 磨砂 /
//!   glass=HostBackdrop 玻璃（Win11 22H2+）
//! - 文本：DirectWrite；圆角/高亮：D2D FillRoundedRectangle
//! - 初始化失败时上层回退 v1（GDI 分层窗口）

use serde_json::Value;
use windows::core::Interface;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{SetCapture, ReleaseCapture};
use windows_core::PCWSTR;

// ── DWM accent（未公开 API，Win10 1803+ 全系统 IME 通用做法）──

/// cand2 窗口过程：DefWindowProc 转发 + 鼠标消息诊断日志。
/// 【排查中】用户实测「正常应用里点击候选框导致应用卡死」——本过程
/// 记录点击/移动消息到达与时刻，卡死复现后由日志定位卡点。
extern "system" fn cand2_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    const DIAG: once_bool::Diag = once_bool::Diag::new();
    if DIAG.enabled() {
        let tag = match msg {
            0x200 => "move",
            0x201 => "ldown",
            0x202 => "lup",
            0x204 => "rdown",
            0x205 => "rup",
            0x84 => "nchittest",
            0x21 => "mactivate",
            0xA1 => "ncldown",
            0xA4 => "ncrdown",
            0x20 => "setcursor",
            0xA0 => "activate",
            _ => "",
        };
        if !tag.is_empty() {
            crate::tsf::diag_note(&format!("cw2 mouse {tag} t={:?}", std::time::SystemTime::now()));
        }
    }
    // 【NOREDIRECTIONBITMAP+DComp 窗的 hit-test 修正】DWM 按 visual
    // 内容 alpha 判定命中：悬停时代码在候选字上命中，但阴影/圆角/
    // 透明边缘按下会被判穿透——按钮消息根本不进 wndproc（QQ 实测
    // setcursor/move 到达、ldown/rdown 从未出现）。显式返回
    // HTCLIENT 强制整窗客户区命中。
    if msg == 0x84 {
        // WM_NCHITTEST → HTCLIENT
        return LRESULT(1);
    }
    // 【鼠标交互】左键按住拖拽移动候选窗；右键固定/解除固定位置。
    // 冻结事故教训（已修）：本窗口过程的按钮消息自持自理、绝不经
    // DefWindowProc 的激活路径；窗口操作仅发生在用户主动交互的
    // 消息路径（非 TSF 焦点回调），无死锁面。
    match msg {
        0x201 => {
            // WM_LBUTTONDOWN：记录拖拽偏移并捕获鼠标
            crate::tsf::trace("cw2: ldown 到达");
            unsafe {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                let _ = GetWindowRect(hwnd, &mut wr);
                *CAND_DRAG.lock().unwrap() = Some((pt.x - wr.left, pt.y - wr.top));
                let _ = SetCapture(hwnd);
            }
            return LRESULT(0);
        }
        0x200 => {
            // WM_MOUSEMOVE：拖拽中随鼠标移动窗口（clamp 虚拟屏幕内）
            let drag = *CAND_DRAG.lock().unwrap();
            if let Some((dx, dy)) = drag {
                unsafe {
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
                    let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
                    let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
                    let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
                    let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                    let _ = GetWindowRect(hwnd, &mut wr);
                    let w = (wr.right - wr.left).max(1);
                    let h = (wr.bottom - wr.top).max(1);
                    let x = (pt.x - dx).clamp(vx, (vx + vw - w).max(vx));
                    let y = (pt.y - dy).clamp(vy, (vy + vh - h).max(vy));
                    let _ = SetWindowPos(
                        hwnd,
                        HWND(std::ptr::null_mut()),
                        x,
                        y,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                    lockwin_follow(hwnd);
                }
            }
            return LRESULT(0);
        }
        0x202 => {
            // WM_LBUTTONUP：结束拖拽，松手位置交给 show() 作 sticky
            //（本组段内留在松手处；新组段锚点就绪即恢复跟随——想
            // 永久固定请右键）
            unsafe {
                let _ = ReleaseCapture();
                if CAND_DRAG.lock().unwrap().is_some() {
                    let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                    let _ = GetWindowRect(hwnd, &mut wr);
                    *CAND_DROP_AT.lock().unwrap() = Some((wr.left, wr.top));
                    // 【固定态拖动】锁定期拖到哪锁到哪：松手位置同步写回
                    // 固定坐标（旧行为：永远弹回第一次右键锁定处）。
                    let mut pinned = CAND_PINNED.lock().unwrap();
                    if pinned.is_some() {
                        *pinned = Some((wr.left, wr.top));
                        drop(pinned);
                        crate::tsf::diag_note(&format!(
                            "cw2 pin 拖动松手回写 ({},{})",
                            wr.left, wr.top
                        ));
                    }
                }
            }
            *CAND_DRAG.lock().unwrap() = None;
            return LRESULT(0);
        }
        0x204 => {
            // WM_RBUTTONDOWN：固定/解除固定（锁标志即时反馈）
            crate::tsf::trace("cw2: rdown 到达");
            let mut pinned = CAND_PINNED.lock().unwrap();
            if pinned.is_some() {
                *pinned = None;
                lockwin_hide();
            } else {
                unsafe {
                    let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                    let _ = GetWindowRect(hwnd, &mut wr);
                    *pinned = Some((wr.left, wr.top));
                }
                drop(pinned);
                lockwin_show_at(hwnd);
            }
            return LRESULT(0);
        }
        0x20A => {
            // 【滚轮缩放候选框】WM_MOUSEWHEEL（Win10+ 默认「悬停时滚动
            // 非活动窗口」，光标在框上即到达）：上滚放大、下滚缩小，
            // 每格 ±1pt（10~36 clamp）。字号经 server 写回当前皮肤
            // layout.font_point（持久化），随后本地皮肤副本同步新字号
            // 并用缓存的上帧渲染参数立即重绘——不等 2.5s 皮肤缓存过期、
            // 不依赖键事件触发 update_ui。
            let delta: i32 = if ((wparam.0 >> 16) as i16) > 0 { 1 } else { -1 };
            if let Some(r) = crate::ipc::call(&serde_json::json!({
                "op": "skin_font_delta", "delta": delta
            })) {
                let new_pt = r.get("font_point").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                if new_pt > 0.0 {
                    if let Some(gsh) = crate::tsf::G_SHARED.get() {
                        let shared = gsh.0.clone();
                        let mut g = shared.lock().unwrap();
                        let patched = if let Some(l) = g.skin.pointer_mut("/skin/layout") {
                            l["font_point"] = serde_json::json!(new_pt);
                            true
                        } else if let Some(l) = g.skin.get_mut("layout") {
                            l["font_point"] = serde_json::json!(new_pt);
                            true
                        } else {
                            false
                        };
                        if patched {
                            // 副本已同步新字号：刷新缓存时限，暂不重拉
                            g.skin_stale = false;
                            g.skin_loaded_at = std::time::Instant::now();
                        }
                        let last = g.last_show.take();
                        let skin = g.skin.clone();
                        let caret = g.caret;
                        if let (Some(c), Some((cands, raw, sel))) = (g.cand2.as_mut(), last) {
                            c.show(&cands, &raw, &skin, caret.as_ref(), sel);
                            g.last_show = Some((cands, raw, sel));
                        }
                    }
                }
            }
            return LRESULT(0);
        }
        0x205 | 0x207 | 0x208 => return LRESULT(0), // 右/中键抬起吞
        // 异步隐藏（hide() PostMessage 而来——焦点回调里同步 ShowWindow
        // 会与 MSCTF/Chromium 焦点临界区死锁）
        crate::candwin2::WM_APP_HIDE_CAND => {
            unsafe { let _ = ShowWindow(hwnd, SW_HIDE); }
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

mod once_bool {
    /// 进程内一次性诊断开关：写标志文件才启用（默认零开销）。
    pub struct Diag;
    impl Diag {
        pub const fn new() -> Diag {
            Diag
        }
        pub fn enabled(&self) -> bool {
            static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *V.get_or_init(|| std::path::Path::new(r"C:\ProgramData\HuFu\diag\cand2-mouse").exists())
        }
    }
}

const ACCENT_DISABLED: u32 = 0;
const ACCENT_ENABLE_TRANSPARENTGRADIENT: u32 = 2;
const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;
const ACCENT_ENABLE_HOSTBACKDROP: u32 = 6;
const WCA_ACCENT_POLICY: u32 = 19;

#[repr(C)]
struct AccentPolicy {
    accent_state: u32,
    accent_flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

#[repr(C)]
struct WinCompAttrData {
    attribute: u32,
    data: *mut core::ffi::c_void,
    size_of_data: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn SetWindowCompositionAttribute(hwnd: HWND, data: *mut WinCompAttrData) -> BOOL;
}

fn apply_accent(hwnd: HWND, state: u32, tint: [u8; 4]) {
    // gradient_color 布局 0xAABBGGRR
    let abgr = (u32::from(tint[3]) << 24)
        | (u32::from(tint[2]) << 16)
        | (u32::from(tint[1]) << 8)
        | u32::from(tint[0]);
    let mut policy = AccentPolicy {
        accent_state: state,
        accent_flags: 0,
        gradient_color: abgr,
        animation_id: 0,
    };
    let mut data = WinCompAttrData {
        attribute: WCA_ACCENT_POLICY,
        data: &mut policy as *mut AccentPolicy as *mut core::ffi::c_void,
        size_of_data: std::mem::size_of::<AccentPolicy>() as u32,
    };
    unsafe {
        let _ = SetWindowCompositionAttribute(hwnd, &mut data);
    }
}

// ── 皮肤取色 ──

fn parse_hex(s: &str) -> Option<[u8; 4]> {
    let s = s.trim_start_matches('#');
    if s.len() != 8 && s.len() != 6 {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some([
        b(0)?,
        b(2)?,
        b(4)?,
        if s.len() == 8 { b(6)? } else { 0xFF },
    ])
}

fn color_f(v: &Value, key: &str, default: &str) -> D2D1_COLOR_F {
    let hex = v
        .pointer(&format!("/skin/colors/{key}"))
        .and_then(|x| x.as_str())
        .or_else(|| v.get("colors").and_then(|c| c.get(key)).and_then(|x| x.as_str()))
        .unwrap_or(default);
    let c = parse_hex(hex).unwrap_or([32, 32, 34, 230]);
    D2D1_COLOR_F {
        r: c[0] as f32 / 255.0,
        g: c[1] as f32 / 255.0,
        b: c[2] as f32 / 255.0,
        a: c[3] as f32 / 255.0,
    }
}

fn layout_f(v: &Value, key: &str, default: f32) -> f32 {
    v.pointer(&format!("/skin/layout/{key}"))
        .or_else(|| v.get("layout").and_then(|l| l.get(key)))
        .and_then(|x| x.as_f64())
        .unwrap_or(default as f64) as f32
}

fn material_kind(v: &Value) -> String {
    v.pointer("/skin/material/kind")
        .or_else(|| v.get("material").and_then(|m| m.get("kind")))
        .and_then(|x| x.as_str())
        .unwrap_or("solid")
        .to_string()
}

// ── 窗口本体 ──

pub struct CandidateWindowV2 {
    pub(crate) hwnd: HWND,
    ctx: Option<ID2D1DeviceContext>,
    swapchain: Option<IDXGISwapChain1>,
    dcomp: Option<IDCompositionDevice>,
    target: Option<IDCompositionTarget>,
    visual: Option<IDCompositionVisual>,
    dwrite: Option<IDWriteFactory>,
    dxgi: Option<IDXGIDevice>,
    size: (i32, i32),
    /// 粘性定位：最近一次有效锚点坐标。锚点偶发丢失（GetTextExt 在
    /// 异步编辑会话未就绪时失败）时沿用上次位置——绝不能瞬移屏幕中央，
    /// 那正是候选框「在光标周围乱跳」的病根。
    sticky_pos: Option<(i32, i32)>,
    /// 上次 show 的编码长度：判断「正向打字」还是「退格/新组段」。
    /// 正向打字时光标只应右移/不动——据此过滤应用返回的旧布局回退值。
    last_raw_len: usize,
    /// 测试回读：show() 后从 D2D 目标位图取整帧 BGRA（渲染层真值，不经 DWM）
    pub(crate) readback: bool,
    pub(crate) last_pixels: Option<Vec<u8>>,
    /// 诊断：最近一次渲染的光学垂直位移（readback 模式填充）
    pub(crate) last_dy: Option<f32>,
    /// 诊断：readback 像素尺寸
    pub(crate) last_size: (u32, u32),
    /// 连续被 DWM cloaked（显示中但不可见）的帧数；打包宿主里
    /// DComp 直通窗可能被整体隐身 → 达阈值切换 v1 传统混合窗
    pub(crate) cloaked_streak: u32,
    /// 【每帧开销缓存】字体格式三件套按 (face,pt,label_pt) 复用——
    /// CreateTextFormat 含系统字体匹配（百 µs 级），打字每键一帧
    /// ×3 个格式是渲染路径大头；皮肤/字号不变时零创建。
    pub(crate) tf_cache: Option<((String, f32, f32), (Option<IDWriteTextFormat>, Option<IDWriteTextFormat>, Option<IDWriteTextFormat>))>,
    /// 【每帧开销缓存】光学垂直补偿 dy 按 (face,pt) 复用——probe
    /// 每帧两次 CreateTextLayout+GetOverhangMetrics 可省。
    pub(crate) dy_cache: Option<((String, f32), f32)>,
    /// 【每帧开销缓存】阴影 command list+effect 按 (w,h,radius,oy,argb)
    /// 复用——宽度不变的连续帧（同长度候选）零重建；变宽时重建。
    pub(crate) shadow_cache: Option<((u32, u32, u32, i32, u32), (ID2D1CommandList, ID2D1Effect))>,
}

/// 【阴影圆角外遮罩】PushLayer：整画布 − 窗口圆角（even-odd 几何组），
/// 高斯弥散只出现在窗口轮廓之外。返回 true=已 Push（调用方 DrawImage
/// 后须 PopLayer）；false=几何创建失败（免 Pop）。
unsafe fn push_shadow_mask(
    ctx: &windows::Win32::Graphics::Direct2D::ID2D1DeviceContext,
    width: f32,
    height: f32,
    w_out: u32,
    h_out: u32,
    shadow_m: f32,
    radius: f32,
) -> bool {
    let f = match ctx.GetFactory() {
        Ok(f) => f,
        Err(_) => return false,
    };
    let big = match f.CreateRectangleGeometry(&D2D_RECT_F {
        left: -1.0e6,
        top: -1.0e6,
        right: w_out as f32 + 1.0e6,
        bottom: h_out as f32 + 1.0e6,
    }) {
        Ok(g) => g,
        Err(_) => return false,
    };
    let win = match f.CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: shadow_m,
            top: shadow_m,
            right: shadow_m + width,
            bottom: shadow_m + height,
        },
        radiusX: radius,
        radiusY: radius,
    }) {
        Ok(g) => g,
        Err(_) => return false,
    };
    let big: windows::Win32::Graphics::Direct2D::ID2D1Geometry = match big.cast() {
        Ok(g) => g,
        Err(_) => return false,
    };
    let win: windows::Win32::Graphics::Direct2D::ID2D1Geometry = match win.cast() {
        Ok(g) => g,
        Err(_) => return false,
    };
    let grp: windows::Win32::Graphics::Direct2D::ID2D1Geometry = match f
        .CreateGeometryGroup(D2D1_FILL_MODE_ALTERNATE, &[Some(big), Some(win)])
    {
        Ok(g) => match g.cast() {
            Ok(g) => g,
            Err(_) => return false,
        },
        Err(_) => return false,
    };
    let mut lp = D2D1_LAYER_PARAMETERS1::default();
    lp.contentBounds = D2D_RECT_F {
        left: -1.0e6,
        top: -1.0e6,
        right: 1.0e6,
        bottom: 1.0e6,
    };
    lp.geometricMask = std::mem::ManuallyDrop::new(Some(grp));
    lp.maskAntialiasMode = D2D1_ANTIALIAS_MODE_PER_PRIMITIVE;
    lp.maskTransform = windows::Foundation::Numerics::Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: 0.0,
        M32: 0.0,
    };
    lp.opacity = 1.0;
    let _ = ctx.PushLayer(&lp, None);
    true
}

impl CandidateWindowV2 {
    /// 初始化设备管线；任何一步失败返回 None（调用方回退 v1）。
    pub fn new() -> Option<CandidateWindowV2> {
        unsafe {
            let class: Vec<u16> = "HuFuCandWin2\0".encode_utf16().collect();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(cand2_wndproc),
                // 类光标：NULL 会让鼠标移入时系统 fallback 到忙碌光标
                //（开始菜单/UWP 里实测「沙漏/转圈」）——显式箭头。
                hCursor: LoadCursorW(HINSTANCE(std::ptr::null_mut()), IDC_ARROW)
                    .unwrap_or(HCURSOR(std::ptr::null_mut())),
                lpszClassName: PCWSTR(class.as_ptr()),
                hbrBackground: HBRUSH(std::ptr::null_mut()),
                ..Default::default()
            };
            let _atom = RegisterClassW(&wc);
            // 注：曾因「点击候选框冻结」加过 WS_EX_TRANSPARENT 鼠标穿透
            // ——后经反汇编定位真凶为焦点回调内同步 ShowWindow 死锁
            // （已修），穿透撤销以支持拖拽/右键固定交互。
            let ex = WINDOW_EX_STYLE(
                WS_EX_TOOLWINDOW.0
                    | WS_EX_TOPMOST.0
                    | WS_EX_NOACTIVATE.0
                    | WS_EX_NOREDIRECTIONBITMAP.0,
            );
            let hwnd = CreateWindowExW(
                ex,
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(WS_POPUP.0),
                0,
                0,
                10,
                10,
                HWND(std::ptr::null_mut()),
                HMENU(std::ptr::null_mut()),
                HINSTANCE(std::ptr::null_mut()),
                None,
            )
            .unwrap_or_default();
            if hwnd.0.is_null() {
                return None;
            }

            // D3D11 设备（硬件 → WARP 兜底），必须 BGRA 供 D2D 互操作
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            for dt in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
                let ok = D3D11CreateDevice(
                    None,
                    dt,
                    HMODULE(std::ptr::null_mut()),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )
                .is_ok();
                if ok && device.is_some() {
                    break;
                }
            }
            let device = device?;
            let _ = context; // 无需常驻 D3D 上下文，D2D 自管

            let dxgi_dev: IDXGIDevice = device.cast().ok()?;
            let factory: IDXGIFactory2 = CreateDXGIFactory1().ok()?;
            let factory2d: ID2D1Factory1 = D2D1CreateFactory(
                D2D1_FACTORY_TYPE_MULTI_THREADED,
                None,
            )
            .ok()?;
            let d2d_dev: ID2D1Device = factory2d.CreateDevice(&dxgi_dev).ok()?;
            let ctx: ID2D1DeviceContext = d2d_dev
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
                .ok()?;
            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi_dev).ok()?;
            let target = dcomp.CreateTargetForHwnd(hwnd, BOOL(1)).ok()?;
            let visual = dcomp.CreateVisual().ok()?;
            let dwrite: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;

            Some(CandidateWindowV2 {
                hwnd,
                ctx: Some(ctx),
                swapchain: None,
                dcomp: Some(dcomp),
                target: Some(target),
                visual: Some(visual),
                dwrite: Some(dwrite),
                dxgi: Some(dxgi_dev.clone()),
                readback: false,
                sticky_pos: None,
                last_raw_len: 0,
                last_pixels: None,
                last_dy: None,
                last_size: (0, 0),
                size: (0, 0),
                cloaked_streak: 0,
                tf_cache: None,
                dy_cache: None,
                shadow_cache: None,
            })
        }
    }

    fn ensure_swapchain(&mut self, w: u32, h: u32) -> bool {
        if self.size == (w as i32, h as i32) && self.swapchain.is_some() {
            return true;
        }
        unsafe {
            if let Some(ctx) = &self.ctx {
                ctx.SetTarget(None);
            }
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: w,
                Height: h,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: BOOL(0),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                Flags: 0,
            };
            // 重建 swapchain（尺寸变更；候选窗小、代价可忽略）
            self.swapchain = None;
            let chain = match self.create_chain_from_ctx(&desc) {
                Some(c) => c,
                None => return false,
            };
            if let (Some(visual), Some(target), Some(dc)) =
                (&self.visual, &self.target, &self.dcomp)
            {
                if visual.SetContent(&chain).is_err() || target.SetRoot(visual).is_err() || dc.Commit().is_err() {
                    crate::tsf::trace("cw2: dcomp attach FAIL");
                    return false;
                }
            }
            self.swapchain = Some(chain);
            self.size = (w as i32, h as i32);
            true
        }
    }

    unsafe fn create_chain_from_ctx(&mut self, desc: &DXGI_SWAP_CHAIN_DESC1) -> Option<IDXGISwapChain1> {
        // 用 new() 时存下的 DXGI 设备（ID2D1Device QI 不出 IDXGIDevice）；
        // factory 必须与设备同源（device→adapter→GetParent），否则 INVALID_CALL
        let dxgi_dev: IDXGIDevice = self.dxgi.clone()?;
        let adapter: IDXGIAdapter = match dxgi_dev.GetAdapter() {
            Ok(a) => a,
            Err(e) => {
                crate::tsf::trace(&format!("cw2: GetAdapter err 0x{:08X}", e.code().0 as u32));
                return None;
            }
        };
        let factory: IDXGIFactory2 = match adapter.GetParent() {
            Ok(f) => f,
            Err(e) => {
                crate::tsf::trace(&format!("cw2: factory err 0x{:08X}", e.code().0 as u32));
                return None;
            }
        };
        factory.CreateSwapChainForComposition(&dxgi_dev, desc, None)
            .map_err(|e| {
                crate::tsf::trace(&format!("cw2: CreateSwapChain err 0x{:08X}", e.code().0 as u32));
                e
            })
            .ok()
    }
    /// 渲染并显示。anchor=插入点屏幕矩形：候选窗优先悬于其上方。selected=高亮行（页内 0 起）。
    pub fn show(&mut self, cands: &[(String, String)], raw: &str, skin: &Value, anchor: Option<&RECT>, selected: usize) {
        // 序号显示：引擎 state 经 pipe skin 响应附带（根级 show_index）
        let show_index = skin
            .get("show_index")
            .and_then(|x| x.as_bool())
            .unwrap_or(true);
        let kind = material_kind(skin);
        let tint_hex = skin
            .pointer("/skin/material/tint")
            .or_else(|| skin.get("material").and_then(|m| m.get("tint")))
            .and_then(|x| x.as_str())
            .and_then(parse_hex);

        let font_pt = layout_f(skin, "font_point", 16.0);
        let radius = layout_f(skin, "corner_radius", 8.0);
        let margin_x = layout_f(skin, "margin_x", 8.0);
        let margin_y = layout_f(skin, "margin_y", 5.0);
        let line_h = font_pt * 96.0 / 72.0 + layout_f(skin, "line_spacing", 3.0) + 5.0;
        // width>0 固定宽；0=按内容自适应（min_width~340 收夹）
        let width_cfg = layout_f(skin, "width", 0.0);
        let min_width = layout_f(skin, "min_width", 150.0).max(100.0);
        let label_w = if show_index { 26.0f32 } else { 0.0 };
        let em = font_pt * 96.0 / 72.0;
        // 横排（skin.layout.horizontal）：候选单行横铺，weasel 式
        let horizontal = skin
            .pointer("/skin/layout/horizontal")
            .or_else(|| skin.get("layout").and_then(|l| l.get("horizontal")))
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        // 【超屏修复】超长句候选框超出屏幕（用户实测）：横排宽度纯
        // 内容自适应且编码行整段参与定宽，无上限。三管齐下：
        // ① 显示层预截断——超长 raw/候选只保尾部（正在打的部分），
        //    前缀「…」；同名遮蔽参数，后续测量/定宽/渲染全部同源。
        //    截宽按估算（CJK≈em、ASCII≈0.55em），宁可少截不可截不满。
        // ② 横排宽度封顶工作区宽（见 width 计算处 w.min(w_cap)）。
        // ③ 位置 clamp 原已有——宽度封顶后 clamp 区间不再倒置。
        let screen_w = unsafe { GetSystemMetrics(SM_CXFULLSCREEN) }.max(200) as f32;
        let w_cap = (screen_w - 24.0).max(320.0);
        let trunc_tail = |s: &str, cap: f32| -> String {
            let est = |s: &str| {
                s.chars()
                    .map(|c| if c.is_ascii() { em * 0.55 } else { em })
                    .sum::<f32>() + em // 「…」前缀余量
            };
            if est(s) <= cap {
                return s.to_string();
            }
            let chars: Vec<char> = s.chars().collect();
            let mut k = chars.len();
            while k > 6 {
                if est(&chars[chars.len() - k..].iter().collect::<String>()) <= cap {
                    return std::iter::once('…')
                        .chain(chars[chars.len() - k..].iter().copied())
                        .collect();
                }
                k -= 2;
            }
            std::iter::once('…')
                .chain(chars[chars.len() - 6..].iter().copied())
                .collect()
        };
        let raw_disp_cap = if horizontal { w_cap * 0.7 } else { 260.0 };
        let cand_disp_cap = if horizontal { w_cap * 0.35 } else { 240.0 };
        let raw = trunc_tail(raw, raw_disp_cap.max(100.0));
        let cands: Vec<(String, String)> = cands
            .iter()
            .map(|(t, c)| (trunc_tail(t, cand_disp_cap.max(80.0)), c.clone()))
            .collect();
        // 【每帧开销缓存】块前取块后存（测量/渲染 unsafe 块内 self 有
        // 借用，不能就地读写缓存字段）——字体三件套 / 光学 dy / 阴影
        // cl+effect，键不变则零创建。
        let tf_cache_in = self.tf_cache.clone();
        let dy_cache_in = self.dy_cache.clone();
        let shadow_cache_in = self.shadow_cache.clone();
        let cand_spacing = layout_f(skin, "candidate_spacing", 6.0);
        let hilite_pad = layout_f(skin, "hilite_padding", 4.0);

        // 字体与内容测宽先行（宽度取决于最长候选）
        let mut tf_cache_out: Option<((String, f32, f32), (Option<IDWriteTextFormat>, Option<IDWriteTextFormat>, Option<IDWriteTextFormat>))> = None;
        let mut dy_cache_out: Option<((String, f32), f32)> = None;
        let mut shadow_cache_out: Option<((u32, u32, u32, i32, u32), (ID2D1CommandList, ID2D1Effect))> = None;
        let (tf, tf_label, tf_small, cand_ws, geo) = unsafe {
            let dwrite = match &self.dwrite {
                Some(d) => d.clone(),
                None => return,
            };
            let font_face: String = {
                let f = skin
                    .pointer("/skin/layout/font_face")
                    .or_else(|| skin.get("layout").and_then(|l| l.get("font_face")))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                if f.is_empty() {
                    "Microsoft YaHei UI".into()
                } else {
                    f.to_string()
                }
            };
            let locale: Vec<u16> = "zh-CN\0".encode_utf16().collect();
            // 字体族缺失时回退雅黑（防 CreateTextFormat 失败 → 全窗无字）
            let mk_tf = |fam: &str, em: f32| -> Option<IDWriteTextFormat> {
                let mut b: Vec<u16> = fam.encode_utf16().collect();
                b.push(0);
                dwrite
                    .CreateTextFormat(
                        PCWSTR(b.as_ptr()),
                        None,
                        DWRITE_FONT_WEIGHT_NORMAL,
                        DWRITE_FONT_STYLE_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        em,
                        PCWSTR(locale.as_ptr()),
                    )
                    .ok()
            };
            let tf;
            let tf_small;
            let tf_label;
            // 标签序号字体（layout.label_font_point；0/缺省回退 0.78 倍正文）
            let label_pt = layout_f(skin, "label_font_point", 0.0);
            let tf_key = (font_face.clone(), font_pt, label_pt);
            let tf_hit = tf_cache_in
                .as_ref()
                .map(|(k, _)| *k == tf_key)
                .unwrap_or(false);
            if tf_hit {
                let (_, v) = tf_cache_in.as_ref().unwrap();
                tf = v.0.clone();
                tf_small = v.1.clone();
                tf_label = v.2.clone();
            } else {
                tf = mk_tf(&font_face, em).or_else(|| mk_tf("Microsoft YaHei UI", em));
                tf_small =
                    mk_tf(&font_face, em * 0.78).or_else(|| mk_tf("Microsoft YaHei UI", em * 0.78));
                // 文本垂直居中（高亮胶囊上下留白对称的关键）
                for t in [&tf, &tf_small] {
                    if let Some(t) = t {
                        let _ = t.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                    }
                }
                tf_label = if label_pt > 0.0 {
                    mk_tf(&font_face, label_pt * 96.0 / 72.0)
                        .or_else(|| mk_tf("Microsoft YaHei UI", label_pt * 96.0 / 72.0))
                        .or(tf_small.clone())
                } else {
                    tf_small.clone()
                };
                if let Some(t) = &tf_label {
                    let _ = t.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                }
                tf_cache_out = Some((
                    tf_key,
                    (tf.clone(), tf_small.clone(), tf_label.clone()),
                ));
            }

            let measure = |tf: &Option<IDWriteTextFormat>, s: &str| -> f32 {
                if s.is_empty() {
                    return 0.0;
                }
                if let Some(tf) = tf {
                    let w: Vec<u16> = s.encode_utf16().collect();
                    if let Ok(l) = dwrite.CreateTextLayout(&w, tf, 4096.0, line_h.max(8.0)) {
                        let mut m = DWRITE_TEXT_METRICS::default();
                        if l.GetMetrics(&mut m).is_ok() {
                            return m.width.ceil();
                        }
                    }
                }
                // 兜底：按字数估宽
                s.chars().count() as f32 * em
            };
            let mut max_text = 0.0f32;
            let mut max_cmt = 0.0f32;
            let mut cand_ws: Vec<(f32, f32)> = Vec::new();
            for (t, c) in &cands {
                let tw = measure(&tf, t.as_str());
                let cw = if c.is_empty() { 0.0 } else { measure(&tf_small, c.as_str()) };
                max_text = max_text.max(tw);
                if !c.is_empty() {
                    max_cmt = max_cmt.max(cw);
                }
                cand_ws.push((tw, cw));
            }
            // 光学垂直居中：CJK 墨盒在行盒内整体偏上（行盒含下降部空白，
            // 段落居中只对齐行盒）→ 视觉上下内边距不等（下面多）。
            // GetOverhangMetrics 给墨迹相对布局盒的突出量（负=内缩），
            // 位移 = 行中心 − 墨盒中心。
            let probe_slack = |txt: &str, t: &Option<IDWriteTextFormat>| -> Option<(f32, f32)> {
                if let Some(t) = t {
                    let ws: Vec<u16> = txt.encode_utf16().collect();
                    if let Ok(l) = dwrite.CreateTextLayout(&ws, t, 4096.0, line_h.max(8.0)) {
                        if let Ok(o) = l.GetOverhangMetrics() {
                            return Some((-o.top, -o.bottom)); // (顶 slack, 底 slack)
                        }
                    }
                }
                None
            };
            // 光学补偿计算（缓存 miss 时用）：墨盒在行盒内偏上，
            // 位移 = 行中心 − 墨盒中心（底 slack − 顶 slack 的一半）
            let probe_dy = |probe: &dyn Fn(&str, &Option<IDWriteTextFormat>) -> Option<(f32, f32)>,
                            t: &Option<IDWriteTextFormat>|
             -> f32 {
                if let Some((top_slack, bot_slack)) = probe("永", t) {
                    ((bot_slack - top_slack) * 0.5).clamp(-6.0, 6.0)
                } else {
                    0.0
                }
            };
            let dy;
            let dy_key = (font_face.clone(), font_pt);
            if let Some((k, v)) = &dy_cache_in {
                if *k == dy_key {
                    dy = *v;
                } else {
                    dy = probe_dy(&probe_slack, &tf);
                    dy_cache_out = Some((dy_key, dy));
                }
            } else {
                dy = probe_dy(&probe_slack, &tf);
                dy_cache_out = Some((dy_key, dy));
            }
            // 注意：编码行不参与定宽（长码截断显示，框宽只随候选内容）；
            // 例外一：仅提示行窗口（反查/命令进入提示，无候选）时由提示行定宽；
            // 例外二：横排布局编码与候选同行（编码在左，2026-09-05 用户
            // 反馈「横排编码独占一行在候选上面」），编码宽度参与定宽。
            let raw_w = if cands.is_empty() || horizontal {
                if raw.is_empty() {
                    0.0
                } else {
                    measure(&tf, raw.as_str())
                }
            } else {
                0.0
            };
            // 【注释配额 2026-09-07】两处布局封顶（竖排固定宽/300、横排
            // w_cap）与最长注释冲突时按配额截断注释（尾部 …），不再让
            // 注释列侵入文本列——旧版 cmt_x 以 max_cmt 定位、横排按原
            // cw 推进格子，长注释（拆分+拼音+unicode 多段拼接）直接压到
            // 候选文本上或溢出重叠（用户实测「候选挤在一起」）。
            let mut cmt_disp: Vec<String> = cands.iter().map(|(_, c)| c.clone()).collect();
            let trunc_cmt = |s: &str, quota: f32| -> String {
                if quota <= 0.0 || s.is_empty() {
                    return String::new();
                }
                let mut out: String = s.to_string();
                loop {
                    let mut t = out.clone();
                    t.push('…');
                    if measure(&tf_small, &t) <= quota {
                        return t;
                    }
                    if out.pop().is_none() {
                        return String::new();
                    }
                }
            };
            let (width, text_x, cmt_x, cmt_w) = if horizontal {
                // 横排纯内容自适应：Σ(标签+文本+注释+间隔)，不受固定宽/最小宽约束
                let mut w = margin_x * 2.0;
                if raw_w > 0.0 {
                    w += raw_w + 10.0; // 编码段（左）+ 编码↔候选间隔
                }
                for (_i, (tw, cw)) in cand_ws.iter().enumerate() {
                    if _i > 0 {
                        w += cand_spacing;
                    }
                    w += label_w * 0.72 + tw + if *cw > 0.0 { 3.0 + cw } else { 0.0 };
                }
                let w_full = w.max(raw_w + margin_x * 2.0);
                // 【超屏修复】横排宽度封顶：工作区宽 − 余量。超屏时注释
                // 预算按剩余空间等比压缩（不足 12px 整列不显示），逐条
                // 截断加 …；格子推进宽同步收缩，尾部候选不再溢出重叠。
                if w_full > w_cap {
                    let budget: f32 = cand_ws
                        .iter()
                        .filter(|(_, c)| *c > 0.0)
                        .map(|(_, c)| 3.0 + c)
                        .sum();
                    let avail = w_cap - (w_full - budget);
                    if avail <= 12.0 {
                        for i in 0..cand_ws.len() {
                            cand_ws[i].1 = 0.0;
                            cmt_disp[i].clear();
                        }
                    } else if budget > 0.0 {
                        let scale = avail / budget;
                        for i in 0..cand_ws.len() {
                            let cw = cand_ws[i].1;
                            if cw > 0.0 {
                                let quota = ((cw + 3.0) * scale - 3.0).max(0.0);
                                cmt_disp[i] = trunc_cmt(&cmt_disp[i], quota);
                                cand_ws[i].1 = if cmt_disp[i].is_empty() { 0.0 } else { quota };
                            }
                        }
                    }
                }
                (w_full.min(w_cap), 0.0, 0.0, 0.0)
            } else {
                // 标签列 + 最宽候选 +（备注列）+ 高亮胶囊余量
                let mut need = margin_x + label_w + max_text.max(raw_w) + margin_x + 6.0;
                if max_cmt > 0.0 {
                    need += 6.0 + max_cmt;
                }
                let width = if width_cfg > 0.0 {
                    width_cfg
                } else {
                    need.clamp(min_width, 300.0)
                };
                let text_x = margin_x + label_w;
                // 注释列配额：右端对齐不变，宽压到「文本列右侧余量」；
                // 超配额逐条截断（…）。固定宽皮肤装不下整条注释时宁可
                // 截断注释也不压文本列。
                let quota = if max_cmt > 0.0 {
                    (width - margin_x - 2.0 - (text_x + max_text.max(raw_w) + 6.0)).max(0.0)
                } else {
                    0.0
                };
                if quota <= 0.0 {
                    for i in 0..cand_ws.len() {
                        cand_ws[i].1 = 0.0;
                        cmt_disp[i].clear();
                    }
                } else if quota < max_cmt {
                    for (i, (_, c)) in cands.iter().enumerate() {
                        if cand_ws[i].1 > quota {
                            cmt_disp[i] = trunc_cmt(c, quota);
                            cand_ws[i].1 = if cmt_disp[i].is_empty() { 0.0 } else { quota };
                        }
                    }
                }
                let (cmt_x, cmt_w) = if quota > 0.0 {
                    (width - margin_x - quota - 2.0, quota + 2.0)
                } else {
                    (width, 0.0)
                };
                (width, text_x, cmt_x, cmt_w)
            };
            (tf, tf_label, tf_small, cand_ws, (cmt_disp, (width, text_x, cmt_x, cmt_w, dy, raw_w)))
        };
        let (v_width, text_x, cmt_x, cmt_w, dy, raw_w) = geo.1;
        let cmt_disp = geo.0;
        // 编码行仅在有内容时占一行（show_code=false 且无 aux 时收缩）；
        // 横排编码与候选同行（左），不占独立行（2026-09-05）
        let code_row = if raw.is_empty() || horizontal { 0.0 } else { 1.0 };
        // 横排：内容即宽（纯自适应）；竖排：固定宽/自适应原逻辑
        let width = v_width;
        let height = if horizontal {
            // 高度贴合内容：margin×2 + 行高×行数 + 编码行后行距（与渲染 y0 一致）
            margin_y * 2.0 + line_h * (1.0 + code_row) + cand_spacing * code_row
        } else {
            // 行距只计行间（编码行后 1 个 + 候选行间 rows-1 个）——渲染 y0 同步
            let rows = cands.len().min(10) as f32 + code_row;
            margin_y * 2.0 + line_h * rows + cand_spacing * (rows - 1.0).max(0.0)
        };

        let w = width as u32;
        let h = height as u32;
        // 投影：shadow_radius>0 时窗口四周外扩边距，阴影画在边距里
        //（内容绘制整体平移进边距内，见渲染段 SetTransform）
        let shadow_radius = layout_f(skin, "shadow_radius", 6.0).clamp(0.0, 24.0);
        let shadow_off_y = layout_f(skin, "shadow_offset_y", 2.0);
        let has_shadow = shadow_radius >= 1.0;
        // 【阴影位图边距】按 D2D1Shadow 的模糊扩散精确覆盖：σ=radius*0.5+1，
        // 高斯扩散 3σ 覆盖 99.7%——小于此会在位图边界被直角截断（用户
        // 实测「超出 R 角的直角色块」= 弥散阴影遭位图边缘切割）。
        let shadow_m = if has_shadow {
            let sigma = shadow_radius * 0.5 + 1.0;
            (sigma * 3.0 + 6.0 + shadow_off_y.abs()).ceil()
        } else {
            0.0
        };
        let w_out = w + 2 * shadow_m as u32;
        let h_out = h + 2 * shadow_m as u32;
        if !self.ensure_swapchain(w_out.max(1), h_out.max(1)) {
            crate::tsf::trace("cw2: ensure_swapchain FAIL");
            return;
        }

        unsafe {
            let chain = match &self.swapchain {
                Some(c) => c.clone(),
                None => return,
            };
            let surface: IDXGISurface = match chain.GetBuffer(0) {
                Ok(s) => s,
                Err(_) => return,
            };
            let ctx = match &self.ctx {
                Some(c) => c.clone(),
                None => return,
            };
            let bp = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let bitmap = match ctx.CreateBitmapFromDxgiSurface(&surface, Some(&bp)) {
                Ok(b) => b,
                Err(_) => return,
            };
            ctx.SetTarget(&bitmap);
            ctx.BeginDraw();
            ctx.SetTransform(&windows::Foundation::Numerics::Matrix3x2 {
                M11: 1.0,
                M12: 0.0,
                M21: 0.0,
                M22: 1.0,
                M31: 0.0,
                M32: 0.0,
            });

            // 背景：一律清透明后画「圆角」底——四角保持透明，窗口才是真圆角
            // （旧行 solid 用 Clear 铺满整窗把圆角补成直角）
            // 材质简化：solid=底色 / translucent|glass|frosted(旧皮肤兼容)=tint 半透明；
            // 毛玻璃(噪点/DWM accent) 已移除；material.opacity(0-1) 统一控透明度。
            let _ = ctx.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            // 投影：多层外扩圆角矩形衰减近似高斯模糊（外坐标空间，
            // 内容平移前画——内容面板会盖住投影内圈，只留柔和外沿）
            if has_shadow {
                // 【阴影透明度】material.shadow_alpha 独立滑条（颜色自带
                // alpha 忽略——与纯色模型一致的语义）
                let shadow_alpha = skin
                    .pointer("/skin/material/shadow_alpha")
                    .or_else(|| skin.get("material").and_then(|m| m.get("shadow_alpha")))
                    .and_then(|x| x.as_f64())
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0) as f32;
                let mut sc = color_f(skin, "shadow_color", "#000000FF");
                sc.a = shadow_alpha;
                if sc.a > 0.004 {
                    // 【每帧缓存键】(w,h,radius,oy,argb)——宽度不变的连续
                    // 帧（同长度候选/翻页）零重建 command list+effect。
                    let sc_packed = ((sc.r * 255.0) as u32)
                        | (((sc.g * 255.0) as u32) << 8)
                        | (((sc.b * 255.0) as u32) << 16)
                        | (((sc.a * 255.0) as u32) << 24);
                    let sh_key = (
                        width as u32,
                        height as u32,
                        (radius * 4.0) as u32,
                        (shadow_off_y * 4.0) as i32,
                        sc_packed,
                    );
                    // 【真 D2D 高斯阴影】用户两轮判多层近似「太锐利」——
                    // 换 D2D1Shadow 效果（系统级高斯模糊）：窗口形状画进
                    // command list → Shadow 效果 → DrawImage 回主画布。
                    let fx = (|| -> Option<()> {
                        unsafe {
                            if let Some((k, v)) = &shadow_cache_in {
                                if *k == sh_key {
                                    // 命中：直接绘制缓存的 effect 输出
                                    let eff_img: ID2D1Image = v.1.cast().ok()?;
                                    let mask_ok = push_shadow_mask(
                                        &ctx,
                                        width,
                                        height,
                                        w_out,
                                        h_out,
                                        shadow_m,
                                        radius,
                                    );
                                    let off = D2D_POINT_2F { x: 0.0, y: shadow_off_y };
                                    ctx.DrawImage(
                                        &eff_img,
                                        Some(&off as *const _),
                                        None,
                                        D2D1_INTERPOLATION_MODE_LINEAR,
                                        D2D1_COMPOSITE_MODE_SOURCE_OVER,
                                    );
                                    if mask_ok {
                                        ctx.PopLayer();
                                    }
                                    return Some(());
                                }
                            }
                            let cl = ctx.CreateCommandList().ok()?;
                            let saved = ctx.GetTarget().ok();
                            let cl_img: ID2D1Image = cl.cast().ok()?;
                            ctx.SetTarget(Some(&cl_img));
                            let wb = ctx
                                .CreateSolidColorBrush(
                                    &D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 },
                                    None,
                                )
                                .ok()?;
                            let rr_win = D2D1_ROUNDED_RECT {
                                rect: D2D_RECT_F {
                                    left: shadow_m,
                                    top: shadow_m,
                                    right: shadow_m + width,
                                    bottom: shadow_m + height,
                                },
                                radiusX: radius,
                                radiusY: radius,
                            };
                            ctx.FillRoundedRectangle(&rr_win, &wb);
                            cl.Close().ok()?;
                            ctx.SetTarget(saved.as_ref());
                            let effect = ctx.CreateEffect(&CLSID_D2D1Shadow).ok()?;
                            let blur = shadow_radius * 0.5 + 1.0;
                            let _ = effect.SetValue(
                                0,
                                D2D1_PROPERTY_TYPE_FLOAT,
                                &blur.to_ne_bytes(),
                            );
                            let col = D2D_VECTOR_4F { x: sc.r, y: sc.g, z: sc.b, w: sc.a };
                            let _ = effect.SetValue(
                                1,
                                D2D1_PROPERTY_TYPE_VECTOR4,
                                &[
                                    col.x.to_ne_bytes(),
                                    col.y.to_ne_bytes(),
                                    col.z.to_ne_bytes(),
                                    col.w.to_ne_bytes(),
                                ]
                                .concat(),
                            );
                            let eff_img: ID2D1Image = effect.cast().ok()?;
                            effect.SetInput(0, &cl_img, true);
                            // 【圆角外遮罩】高斯向内弥散会进窗口内部；用
                            // 直角矩形清除会在圆角外留直角切割痕（用户实测
                            // 「直角色块」）。改 Layer 几何遮罩：整画布 −
                            // 窗口圆角（even-odd）——阴影只在窗外绘制。
                            let mask_ok = push_shadow_mask(
                                &ctx,
                                width,
                                height,
                                w_out,
                                h_out,
                                shadow_m,
                                radius,
                            );
                            let off = D2D_POINT_2F { x: 0.0, y: shadow_off_y };
                            ctx.DrawImage(
                                &eff_img,
                                Some(&off as *const _),
                                None,
                                D2D1_INTERPOLATION_MODE_LINEAR,
                                D2D1_COMPOSITE_MODE_SOURCE_OVER,
                            );
                            if mask_ok {
                                ctx.PopLayer();
                            }
                            // 【每帧缓存】存 command list+effect（键内含
                            // w/h/圆角/偏移/颜色，任一变即重建）
                            shadow_cache_out = Some((sh_key, (cl, effect)));
                            Some(())
                        }
                    })();
                    // 兜底：效果路径失败（老驱动）→ 下方环带多层近似
                    if fx.is_none() {
                    if let Ok(b) = ctx.CreateSolidColorBrush(&sc, None) {
                        // 【环带阴影】旧实现多层实心圆角矩形「内浓外淡」
                        // 依赖不透明窗底盖住内圈——窗底全透明时阴影盖满
                        // 整窗（用户实测 bug）。改 even-odd 几何环带：每层
                        // 只画「外圈 − 窗口」的环，窗口内部永远无阴影。
                        let factory = ctx.GetFactory().ok();
                        // 模糊感：层数多 + 高斯衰减（exp(-kt²)）——层间
                        // 台阶不可见，观感≈CSS box-shadow 的高斯模糊。
                        const PASSES: usize = 28;
                        // 窗口自身圆角矩形（环带的内边界）
                        let win_geom = factory.as_ref().and_then(|f| {
                            let rr = D2D1_ROUNDED_RECT {
                                rect: D2D_RECT_F {
                                    left: shadow_m,
                                    top: shadow_m,
                                    right: shadow_m + width,
                                    bottom: shadow_m + height,
                                },
                                radiusX: radius,
                                radiusY: radius,
                            };
                            f.CreateRoundedRectangleGeometry(&rr).ok()
                        });
                        for i in (1..=PASSES).rev() {
                            let t = i as f32 / PASSES as f32; // 外圈 t=1 → 内圈趋 0
                            let grow = shadow_radius * t;
                            // 高斯衰减：贴边最浓向外平滑消散（旧 (1-t)²
                            // 台阶感强——「阴影太锐利」的根因）
                            let a = sc.a * (-4.5 * t * t).exp();
                            b.SetColor(&D2D1_COLOR_F {
                                r: sc.r,
                                g: sc.g,
                                b: sc.b,
                                a,
                            });
                            let rr = D2D1_ROUNDED_RECT {
                                rect: D2D_RECT_F {
                                    left: shadow_m - grow,
                                    top: shadow_m - grow + shadow_off_y * t,
                                    right: shadow_m + width + grow,
                                    bottom: shadow_m + height + grow
                                        + shadow_off_y * t,
                                },
                                radiusX: radius + grow,
                                radiusY: radius + grow,
                            };
                            // 每层 = 外圈几何 − 窗口几何 的 even-odd 环带
                            //（窗口内部永远无阴影；全透明窗只剩轮廓外投影）
                            let ring = (|| -> Option<()> {
                                let f = factory.as_ref()?;
                                let wg = win_geom.as_ref()?;
                                let outer: Option<windows::Win32::Graphics::Direct2D::ID2D1Geometry> = f
                                    .CreateRoundedRectangleGeometry(&rr)
                                    .ok()
                                    .and_then(|g| g.cast().ok());
                                let outer = outer?;
                                let inner: windows::Win32::Graphics::Direct2D::ID2D1Geometry =
                                    wg.clone().cast().ok()?;
                                let grp = f
                                    .CreateGeometryGroup(
                                        D2D1_FILL_MODE_ALTERNATE,
                                        &[Some(outer), Some(inner)],
                                    )
                                    .ok()?;
                                ctx.FillGeometry(&grp, &b, None);
                                Some(())
                            })();
                            if ring.is_none() {
                                // 几何路径失败兜底：退回实心（旧行为）
                                ctx.FillRoundedRectangle(&rr, &b);
                            }
                        }
                    }
                    } // fx.is_none() 环带兜底结束
                }
            }
            // 内容整体平移进阴影边距内（此后所有内容坐标不变）
            ctx.SetTransform(&windows::Foundation::Numerics::Matrix3x2 {
                M11: 1.0,
                M12: 0.0,
                M21: 0.0,
                M22: 1.0,
                M31: shadow_m,
                M32: shadow_m,
            });
            // 【整体透明度】非文字元素的总乘法系数（块外供共用）
            let master = skin
                .pointer("/skin/material/master_alpha")
                .or_else(|| skin.get("material").and_then(|m| m.get("master_alpha")))
                .and_then(|x| x.as_f64())
                .unwrap_or(1.0)
                .clamp(0.0, 1.0) as f32;
            // 【高亮透明度】高亮候选底独立系数
            let hilite_a = skin
                .pointer("/skin/material/hilite_alpha")
                .or_else(|| skin.get("material").and_then(|m| m.get("hilite_alpha")))
                .and_then(|x| x.as_f64())
                .unwrap_or(1.0)
                .clamp(0.0, 1.0) as f32;
            {
                // 【纯色模型 v2·用户定稿】颜色只管色相（alpha 分量忽略）：
                // 窗底/边框/编码底 alpha = master；高亮底 = hilite_a；文字恒 1。
                let back = color_f(skin, "back_color", "#202022E6");
                let bg_c = D2D1_COLOR_F { r: back.r, g: back.g, b: back.b, a: master };
                if bg_c.a > 0.004 {
                    if let Ok(b) = ctx.CreateSolidColorBrush(&bg_c, None) {
                        let rr = D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F { left: 0.0, top: 0.0, right: width, bottom: height },
                            radiusX: radius,
                            radiusY: radius,
                        };
                        ctx.FillRoundedRectangle(&rr, &b);
                    }
                }
                // 【纯色模型】暗化层已废弃（材质系统移除）——不画
            }

            // 【纯色模型 v2】非文字元素 alpha = master（颜色自带 a 忽略）；
            // 高亮底 alpha = hilite_a；文字画刷 alpha 恒 1.0。
            let elem_alpha = |mut c: D2D1_COLOR_F| {
                c.a = master;
                c
            };
            let text_alpha = |mut c: D2D1_COLOR_F| {
                c.a = 1.0;
                c
            };
            let mkbrush = |ctx: &ID2D1DeviceContext, c: D2D1_COLOR_F| -> Option<ID2D1SolidColorBrush> {
                ctx.CreateSolidColorBrush(&c, None).ok()
            };
            let b_text = mkbrush(&ctx, text_alpha(color_f(skin, "candidate_text_color", "#E8E8EAFF")));
            let b_label = mkbrush(&ctx, text_alpha(color_f(skin, "label_color", "#C9C9C9FF")));
            let b_raw = mkbrush(&ctx, text_alpha(color_f(skin, "hilited_text_color", "#E8E8EAFF")));
            // 编码区背景（preedit_back_color；alpha=0 的皮肤不画）
            let b_preedit_bg = {
                let c = elem_alpha(color_f(skin, "preedit_back_color", "#00000000"));
                (c.a > 0.01).then(|| mkbrush(&ctx, c)).flatten()
            };
            let b_cmt = mkbrush(&ctx, text_alpha(color_f(skin, "comment_text_color", "#9A9AA0FF")));
            let b_hi = mkbrush(&ctx, {
                let mut c = color_f(skin, "hilited_candidate_back_color", "#404046FF");
                c.a = hilite_a;
                c
            });
            let b_hi_txt = mkbrush(&ctx, text_alpha(color_f(skin, "hilited_candidate_text_color", "#FFFFFFFF")));
            let b_hi_lbl = mkbrush(&ctx, text_alpha(color_f(skin, "hilited_candidate_label_color", "#FFD75EFF")));
            let b_hi_cmt = mkbrush(&ctx, text_alpha(color_f(skin, "hilited_comment_text_color", "#C9C9C9FF")));
            let b_border = mkbrush(&ctx, {
                // 【边框透明度】material.border_alpha 独立滑条（颜色自带 a 忽略）
                let border_alpha = skin
                    .pointer("/skin/material/border_alpha")
                    .or_else(|| skin.get("material").and_then(|m| m.get("border_alpha")))
                    .and_then(|x| x.as_f64())
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0) as f32;
                let mut c = color_f(skin, "border_color", "#FFFFFF26");
                c.a = border_alpha;
                c
            });

            let draw = |ctx: &ID2D1DeviceContext,
                        tf: &Option<IDWriteTextFormat>,
                        s: &str,
                        x: f32,
                        y: f32,
                        w: f32,
                        h: f32,
                        brush: &Option<ID2D1SolidColorBrush>| {
                if let (Some(tf), Some(brush)) = (tf, brush) {
                    let ws: Vec<u16> = s.encode_utf16().collect();
                    if ws.is_empty() {
                        return;
                    }
                    let rect = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h };
                    let _ = ctx.DrawText(
                        &ws,
                        tf,
                        &rect,
                        brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            };

            // 编码行（有内容才画；候选行相应下移一行 + 行距）；dy=光学垂直居中位移
            if !raw.is_empty() {
                // 编码区背景（皮肤 preedit_back_color 带透明度时才画）
                if let Some(bg) = &b_preedit_bg {
                    let rr = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: margin_x,
                            top: margin_y + dy,
                            right: width - margin_x,
                            bottom: margin_y + dy + line_h,
                        },
                        radiusX: 4.0,
                        radiusY: 4.0,
                    };
                    unsafe {
                        ctx.FillRoundedRectangle(&rr, bg);
                    }
                }
                draw(&ctx, &tf, raw.as_str(), margin_x, margin_y + dy, width - margin_x * 2.0, line_h, &b_raw);
            }
            let y0 = margin_y + (line_h + cand_spacing) * code_row + dy;

            // 高亮胶囊统一内边距：四边都 = hilite_pad。
            // 文本盒（em 高、垂直居中于行）向外扩 hilite_pad；放不下时整胶囊在行内居中，
            // 保证上下左右内边距始终一致（旧版 top 被夹底没夹，左右还各有隐藏 ±4）。
            // 【光学修正】汉字字面中心略低于几何中心（字体上下伸部不
            // 对称——DrawText 居中时视觉偏下=胶囊显得偏上）：胶囊整体
            // 下移 6% em 对齐视觉中心（用户实测「高亮偏上」的修正）。
            let pill_v = |y: f32| -> (f32, f32) {
                let half = (line_h - em) / 2.0;
                let optical = em * 0.06;
                let ih = em + hilite_pad * 2.0;
                if ih <= line_h {
                    (
                        y + half - hilite_pad + optical,
                        y + half + em + hilite_pad + optical,
                    )
                } else {
                    let off = (line_h - ih) / 2.0;
                    (y + off + optical, y + off + ih + optical)
                }
            };

            // 候选行
            let sel = selected.min(cands.len().saturating_sub(1));
            if horizontal {
                // ── 横排：单行铺开，每格 = 序号+文本(+注释)，高亮为整格胶囊 ──
                // 编码段在左（同行）：候选起点右移 raw_w+间隔（2026-09-05）
                let mut x = margin_x + if raw_w > 0.0 { raw_w + 10.0 } else { 0.0 };
                let y = y0;
                for (i, (text, _)) in cands.iter().enumerate().take(10) {
                    let cmt: &str = cmt_disp.get(i).map(|s| s.as_str()).unwrap_or("");
                    let (tw, cw) = cand_ws.get(i).copied().unwrap_or((0.0, 0.0));
                    let cell_w = label_w * 0.72 + tw + if cw > 0.0 { 3.0 + cw } else { 0.0 };
                    if i > 0 {
                        x += cand_spacing;
                    }
                    if i == sel {
                        if let Some(b) = &b_hi {
                            let (pt, pb) = pill_v(y);
                            let rr = D2D1_ROUNDED_RECT {
                                rect: D2D_RECT_F {
                                    left: x - hilite_pad,
                                    top: pt,
                                    right: x + cell_w + hilite_pad,
                                    bottom: pb,
                                },
                                radiusX: layout_f(skin, "hilited_corner_radius", 6.0),
                                radiusY: layout_f(skin, "hilited_corner_radius", 6.0),
                            };
                            ctx.FillRoundedRectangle(&rr, b);
                        }
                    }
                    let (bt, bl, bc) = if i == sel {
                        (&b_hi_txt, &b_hi_lbl, &b_hi_cmt)
                    } else {
                        (&b_text, &b_label, &b_cmt)
                    };
                    let mut cx = x;
                    if show_index {
                        draw(&ctx, &tf_label, &format!("{}.", i + 1), cx, y, label_w * 0.72, line_h, bl);
                        cx += label_w * 0.72;
                    }
                    draw(&ctx, &tf, text, cx, y, tw + 2.0, line_h, bt);
                    cx += tw;
                    if !cmt.is_empty() && cw > 0.0 {
                        draw(&ctx, &tf_small, cmt, cx + 3.0, y, cw + 2.0, line_h, bc);
                    }
                    x += cell_w;
                }
            } else {
                // ── 竖排（原布局 + candidate_spacing 行距 + hilite_padding 统一内边距）──
                for (i, (text, _)) in cands.iter().enumerate().take(10) {
                    let cmt: &str = cmt_disp.get(i).map(|s| s.as_str()).unwrap_or("");
                    let y = y0 + (line_h + cand_spacing) * i as f32;
                    if i == sel {
                        // 高亮行（圆角胶囊；↑↓ 移动；左右对称 = margin_x 外扩 hilite_pad）
                        if let Some(b) = &b_hi {
                            let (pt, pb) = pill_v(y);
                            let rr = D2D1_ROUNDED_RECT {
                                rect: D2D_RECT_F {
                                    left: margin_x - hilite_pad,
                                    top: pt,
                                    right: width - margin_x + hilite_pad,
                                    bottom: pb,
                                },
                                radiusX: layout_f(skin, "hilited_corner_radius", 6.0),
                                radiusY: layout_f(skin, "hilited_corner_radius", 6.0),
                            };
                            ctx.FillRoundedRectangle(&rr, b);
                        }
                    }
                    let (bt, bl, bc) = if i == sel {
                        (&b_hi_txt, &b_hi_lbl, &b_hi_cmt)
                    } else {
                        (&b_text, &b_label, &b_cmt)
                    };
                    if show_index {
                        draw(&ctx, &tf_label, &format!("{}.", i + 1), margin_x, y, label_w, line_h, bl);
                    }
                    draw(&ctx, &tf, text, text_x, y, cmt_x - text_x - 4.0, line_h, bt);
                    if !cmt.is_empty() {
                        draw(&ctx, &tf_small, cmt, cmt_x, y, width - cmt_x - margin_x + 4.0, line_h, bc);
                    }
                }
            }

            // 边框
            if let Some(b) = &b_border {
                let bw = layout_f(skin, "border_width", 1.0);
                let rr = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: bw / 2.0,
                        top: bw / 2.0,
                        right: width - bw / 2.0,
                        bottom: height - bw / 2.0,
                    },
                    radiusX: radius,
                    radiusY: radius,
                };
                let _ = ctx.DrawRoundedRectangle(&rr, b, bw, None);
            }

            let _ = ctx.EndDraw(None, None);
            ctx.SetTarget(None);

            // 测试回读：EndDraw 后目标位图已非活动，拷到 CPU 位图取整帧 BGRA
            if self.readback {
                use windows::Win32::Graphics::Direct2D::{
                    D2D1_BITMAP_OPTIONS, D2D1_BITMAP_OPTIONS_CPU_READ,
                    D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                };
                // 回读整窗（含阴影边距），与 SetWindowPos 尺寸一致
                let (w_px, h_px) = (w_out, h_out);
                if w_px > 0 && h_px > 0 {
                    let props = windows::Win32::Graphics::Direct2D::D2D1_BITMAP_PROPERTIES1 {
                        pixelFormat: windows::Win32::Graphics::Direct2D::Common::D2D1_PIXEL_FORMAT {
                            format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: windows::Win32::Graphics::Direct2D::Common::D2D1_ALPHA_MODE_PREMULTIPLIED,
                        },
                        dpiX: 96.0,
                        dpiY: 96.0,
                        bitmapOptions: D2D1_BITMAP_OPTIONS(D2D1_BITMAP_OPTIONS_CPU_READ.0 | D2D1_BITMAP_OPTIONS_CANNOT_DRAW.0),
                        ..Default::default()
                    };
                    let _ = D2D1_BITMAP_OPTIONS::default();
                    if let Ok(cpu) = ctx.CreateBitmap(
                        windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_U { width: w_px, height: h_px },
                        None,
                        0,
                        &props,
                    ) {
                        unsafe {
                            if let Ok(bmp0) = bitmap.cast::<ID2D1Bitmap>() {
                                if cpu
                                    .CopyFromBitmap(
                                        None,
                                        Some(&bmp0),
                                        Some(&windows::Win32::Graphics::Direct2D::Common::D2D_RECT_U {
                                            left: 0,
                                            top: 0,
                                            right: w_px,
                                            bottom: h_px,
                                        }),
                                    )
                                    .is_ok()
                                {
                                    if let Ok(mapped) = cpu.Map(
                                        windows::Win32::Graphics::Direct2D::D2D1_MAP_OPTIONS_READ,
                                    ) {
                                        let mut data = vec![0u8; (w_px * h_px * 4) as usize];
                                        let pitch = mapped.pitch as usize;
                                        for row in 0..h_px as usize {
                                            let src = mapped.bits.add(row * pitch) as *const u8;
                                            data[row * (w_px as usize) * 4..(row + 1) * (w_px as usize) * 4]
                                                .copy_from_slice(std::slice::from_raw_parts(src, (w_px * 4) as usize));
                                        }
                                        let _ = cpu.Unmap();
                                        self.last_pixels = Some(data);
                                        self.last_dy = Some(dy);
                                        self.last_size = (w_px, h_px);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let _ = chain.Present(1, DXGI_PRESENT(0));
        }

        // DWM accent 已弃用：NOREDIRECTIONBITMAP+DComp 窗口上不生效，
        // 透明/半透明完全由自绘层 alpha（material.opacity + kind）承担。
        let _ = tint_hex;

        // 定位：优先插入点下方，出屏翻到上方；锚点丢失沿用上次位置。
        // **组段内单调过滤**（跟打器类异步布局应用的跳动终结者）：
        // 正向打字（编码不减）时光标只应右移/不动——x 拒绝回退值
        //（旧布局查询结果比当前光标靠左）、y 锁定到「换行级」变化
        //（>26px 才认）——上下逐键摆动在构造上不可能发生。退格/新
        // 组段（编码变短）放行全部变化。
        unsafe {
            // 虚拟屏幕坐标系（多显示器安全）：主屏 SM_CXSCREEN 会把
            // 副屏负坐标错误钳回主屏
            let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            let grew = raw.len() >= self.last_raw_len;
            self.last_raw_len = raw.len();
            // 拖拽松手交接：一次性消费（设 sticky，本组段留在松手处）
            if let Some(p) = CAND_DROP_AT.lock().unwrap().take() {
                self.sticky_pos = Some(p);
            }
            let (x, y) = if let Some((px, py)) = *CAND_PINNED.lock().unwrap() {
                // 【固定模式】右键固定：忽略光标锚点，钉在用户固定处
                //（跨组段/上屏/新一轮候选全部保持；右键再解除）。
                // 拖动松手会回写 pin（见 WM_LBUTTONUP）——打字必然用
                // 最新固定位。
                crate::tsf::diag_note(&format!("cw2 pin use ({px},{py})"));
                let x = px.clamp(vx, (vx + vw - width as i32).max(vx));
                let y = py.clamp(vy, (vy + vh - height as i32).max(vy));
                (x, y)
            } else {
            match anchor {
                Some(r) => {
                    let x = (r.left).clamp(vx, (vx + vw - width as i32).max(vx));
                    let below = r.bottom + 4;
                    let y = if below + height as i32 <= vy + vh {
                        below
                    } else {
                        (r.top - height as i32 - 4).max(vy)
                    };
                    match self.sticky_pos {
                        Some((ox, oy)) => {
                            // 软换行判定：x 想回退（<旧行尾）且 y 发生换行级
                            // 变化（>26px 行高阈值）同时成立 = 新行开始——
                            // x 回到新行行首是合法回退，禁令解除。否则单调锁
                            // 会把换行后的 X 钉死在旧行尾（实测虎魄：换行
                            // x 2361→1625 被拒，候选框只上下动、不横向跟到
                            // 新行打字点）。仅 y 超阈值不构成豁免——跟打器
                            // 滚动步进可达 29px，x 正常增长帧不得误放行。
                            let line_broke = x < ox - 2 && (y - oy).abs() > 26;
                            // x：正向打字拒绝回退（旧布局值）；软换行除外
                            let x = if grew && !line_broke && x < ox - 2 { ox } else { x };
                            // y：正向打字只认换行级变化（行高 ~29px，阈值 26）
                            let y = if grew && (y - oy).abs() <= 26 { oy } else { y };
                            // 2px 迟滞：亚像素取整误差/回流微动不搬窗
                            if (x - ox).abs() <= 2 && (y - oy).abs() <= 2 {
                                (ox, oy)
                            } else {
                                (x, y)
                            }
                        }
                        None => (x, y),
                    }
                }
                None => match self.sticky_pos {
                    Some(p) => p,
                    // 从未有过真实锚点且本帧也取不到：先记诊断；若无
                    // 历史位置则退到「焦点窗口内左下」而非整帧隐藏
                    //（SearchHost 等宿主 GetTextExt 常失败——搜索框候选
                    // 框不显示的病根）。下一帧锚点就绪即回到正常定位。
                    None => {
                        crate::tsf::diag_note("cw2 anchor+sticky 双缺，退到焦点窗口定位");
                        let fg = GetForegroundWindow();
                        if fg.0.is_null() {
                            let _ = ShowWindow(self.hwnd, SW_HIDE);
                            return;
                        }
                        let mut fr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                        let _ = GetWindowRect(fg, &mut fr);
                        let x = fr.left + 16;
                        let below = fr.bottom - ((height as i32) * 2).min(fr.bottom - fr.top);
                        (x, below.max(fr.top))
                    }
                },
            }
            };
            // 【右缘兜底】正向打字的 x 单调锁（宽度增长时拒回退）会把
            // 已 clamp 的新 x 顶回旧位置——窗口变宽后旧 x+新宽超右缘
            //（用户实测：跟打器超长句候选框超出屏幕；宽度封顶后根因
            // 转到这里）。每帧输出前统一夹回，屏幕边界优先于位置记忆。
            let x = x.clamp(vx, (vx + vw - width as i32 - shadow_m as i32).max(vx));
            let y = y.clamp(vy, (vy + vh - height as i32).max(vy));
            self.sticky_pos = Some((x, y));
            // 诊断：搜索框等宿主锚点缺失排查（visible=0 说明本帧被隐藏）
            // + DWM cloaked 检测（显示中但被 DWM 隐身 → 连续 2 帧后
            //   由调用方切换 v1 传统混合窗——SearchHost 里 DComp 直通
            //   窗被整体 cloaked 的自愈路径）。dwmapi 经 GetProcAddress
            //   动态获取（mingw 工具链无 dwmapi 导入库）。
            let mut cloaked: u32 = 0;
            let mut hr: i32 = -1;
            unsafe {
                #[link(name = "kernel32")]
                unsafe extern "system" {
                    fn GetModuleHandleW(name: *const u16) -> isize;
                    fn GetProcAddress(module: isize, name: *const u8) -> *const core::ffi::c_void;
                }
                // 【i386 ABI】必须 extern "system"（stdcall）：x64 上 Rust
                // 默认约定与 Win64 恰好兼容掩盖了此错，32 位下 cdecl 调用
                // stdcall 函数 → 栈清理错位 → 崩（Pain 打器按键闪退根因）。
                type Dwma = unsafe extern "system" fn(HWND, u32, *mut core::ffi::c_void, u32) -> i32;
                let mn: Vec<u16> = "dwmapi.dll\0".encode_utf16().collect();
                let m = GetModuleHandleW(mn.as_ptr());
                if m != 0 {
                    let p = GetProcAddress(m, c"DwmGetWindowAttribute".as_ptr() as *const u8);
                    if !p.is_null() {
                        let f: Dwma = std::mem::transmute(p);
                        hr = f(
                            self.hwnd,
                            14, // DWMWA_CLOAKED
                            &mut cloaked as *mut u32 as *mut core::ffi::c_void,
                            4,
                        );
                    }
                }
            }
            if cloaked != 0 {
                self.cloaked_streak += 1;
            } else {
                self.cloaked_streak = 0;
            }
            crate::tsf::diag_note(&format!(
                "cw2 layout dbg: font_pt={font_pt} em={em} line_h={line_h} horiz={horizontal} \
                 cands={} rawlen={} max_text={} width={width} height={height} w_out={w_out} h_out={h_out}",
                cands.len(),
                raw.chars().count(),
                cand_ws
                    .iter()
                    .map(|(tw, _)| *tw)
                    .fold(0.0f32, f32::max)
            ));
            crate::tsf::diag_note(&format!(
                "cw2 show anchor={} x={} y={} w={} h={} vis={} cloak={}({:#x}) hr={:#x} streak={}",
                anchor.is_some(),
                x,
                y,
                w_out,
                h_out,
                IsWindowVisible(self.hwnd).0,
                cloaked,
                cloaked,
                hr,
                self.cloaked_streak
            ));
            // 【每帧缓存】块后存回（测量/渲染块内 self 有借用）
            if let Some(v) = tf_cache_out {
                self.tf_cache = Some(v);
            }
            if let Some(v) = dy_cache_out {
                self.dy_cache = Some(v);
            }
            if let Some(v) = shadow_cache_out {
                self.shadow_cache = Some(v);
            }
            // 内容坐标 → 窗口坐标（内容在阴影边距内侧）
            let _ = SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                x - shadow_m as i32,
                y - shadow_m as i32,
                w_out as i32,
                h_out as i32,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            crate::tsf::trace(&format!(
                "cw2: SetWindowPos({x},{y}) err={} visible={}",
                GetLastError().0,
                IsWindowVisible(self.hwnd).0
            ));
            // 固定中：锁指示窗跟随/重现（组段间 hide/show 循环里
            // 锁与候选窗同进退；show_at 幂等：定位+显示）
            if CAND_PINNED.lock().unwrap().is_some() {
                lockwin_show_at(self.hwnd);
            }
        }
    }

    /// 鼠标当前是否悬停在本候选窗上（OnSetFocus 守卫用：交互中的
    /// 点击连带焦点事件不清组段、不隐藏窗口）。
    pub fn is_mouse_over(&self) -> bool {
        unsafe {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            WindowFromPoint(pt).0 == self.hwnd.0
        }
    }

    /// 窗口当前是否可见（poll 前台兜底用：只在可见时记日志/收尾）
    pub fn is_visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    pub fn hide(&mut self) {
        // 组段结束：作废「正向打字」单调锁——置 MAX 使下一帧必判
        // 「非增长」→ 新组段首帧自由定位（修单键接单键锁死旧位置）。
        // 粘性位置**保留**：跨组段的位置记忆，新组段首帧锚点暂不可
        // 用时沿用近处而非瞬移屏幕中下（清掉它正是「时不时跳到屏幕
        // 中下方」的病根）。
        self.last_raw_len = usize::MAX;
        // 候选窗隐藏时锁指示窗同退（组段间不孤零零挂一个锁）
        lockwin_hide();
        // 【绝不同步 ShowWindow】焦点回调（OnSetFocus）里同步 SW_HIDE
        // 与 MSCTF/Chromium 焦点临界区死锁——VSCode 点击冻结事故实锤
        // （栈：OnSetFocus → ShowWindow 永不返回）。改为 PostMessage
        // 排队，焦点回调返回后由消息循环执行隐藏。
        unsafe {
            let _ = PostMessageW(
                self.hwnd,
                WM_APP_HIDE_CAND,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

/// 隐藏候选窗的应用层消息（PostMessage 异步隐藏用）
pub const WM_APP_HIDE_CAND: u32 = 0x4948; // "IH"

/// 拖拽状态：(鼠标屏幕位 − 窗口原点) 偏移；None=非拖拽中。
static CAND_DRAG: std::sync::Mutex<Option<(i32, i32)>> = std::sync::Mutex::new(None);

/// 候选窗固定位置（窗口原点，屏幕坐标）；None=未固定。
/// 右键切换：固定后 show() 忽略光标锚点钉在此处，跨组段/上屏保持；
/// 再次右键解除恢复跟随光标。进程级（每个应用独立记忆）。
pub static CAND_PINNED: std::sync::Mutex<Option<(i32, i32)>> = std::sync::Mutex::new(None);

/// 拖拽松手位置（wndproc → show() 一次性消费：设为 sticky_pos，
/// 本组段内留在松手处；新组段锚点就绪即恢复跟随光标）。
static CAND_DROP_AT: std::sync::Mutex<Option<(i32, i32)>> = std::sync::Mutex::new(None);

// ── 固定锁指示小窗（独立 GDI 分层窗）──
// 右键固定/解除【即时】反馈：锁不画在候选窗渲染帧里（那要等下一次
// 键入触发渲染），而是独立小窗——右键当场显示/隐藏，并跟随候选窗
// 移动（拖拽/show() 定位联动）。鼠标穿透，纯视觉指示。

static LOCK_HWND: std::sync::Mutex<Option<isize>> = std::sync::Mutex::new(None);

/// 创建锁指示窗（幂等）：注册类 → 分层小窗 → GDI 画锁 → ULW 上屏（隐藏态）
fn lockwin_create() -> isize {
    if let Some(h) = *LOCK_HWND.lock().unwrap() {
        unsafe {
            if IsWindow(HWND(h as *mut _)).as_bool() {
                return h;
            }
        }
    }
    unsafe {
        let class: Vec<u16> = "HuFuCandLock\0".encode_utf16().collect();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(defwindowproc_w),
            hCursor: LoadCursorW(HINSTANCE(std::ptr::null_mut()), IDC_ARROW)
                .unwrap_or(HCURSOR(std::ptr::null_mut())),
            lpszClassName: PCWSTR(class.as_ptr()),
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            ..Default::default()
        };
        let atom = RegisterClassW(&wc);
        if atom == 0 {
            crate::tsf::trace(&format!("lockwin: RegisterClass 失败 err={}", GetLastError().0));
        }
        let ex = WINDOW_EX_STYLE(
            WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0 | WS_EX_NOACTIVATE.0
                | WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0,
        );
        let hwnd = match CreateWindowExW(
            ex,
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE(WS_POPUP.0),
            0, 0, 10, 12,
            HWND(std::ptr::null_mut()),
            HMENU(std::ptr::null_mut()),
            HINSTANCE(std::ptr::null_mut()),
            None,
        ) {
            Ok(h) if !h.0.is_null() => h,
            _ => {
                crate::tsf::trace(&format!("lockwin: CreateWindow 失败 err={}", GetLastError().0));
                return 0;
            }
        };
        // 画锁位图（预乘 alpha）：锁环白描边 + 锁体白填充（10x12 小锁）
        let (w, h) = (10i32, 12i32);
        let hdc = CreateCompatibleDC(HDC(std::ptr::null_mut()));
        let mut bmi = windows::Win32::Graphics::Gdi::BITMAPINFO {
            bmiHeader: windows::Win32::Graphics::Gdi::BITMAPINFOHEADER {
                biSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [windows::Win32::Graphics::Gdi::RGBQUAD::default()],
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = match CreateDIBSection(hdc, &bmi as *const _, windows::Win32::Graphics::Gdi::DIB_USAGE(0), &mut bits, None, 0) {
            Ok(d) if !bits.is_null() => d,
            _ => {
                crate::tsf::trace(&format!("lockwin: DIB 失败 err={}", GetLastError().0));
                let _ = DeleteDC(hdc);
                return 0;
            }
        };
        let _old = SelectObject(hdc, windows::Win32::Graphics::Gdi::HGDIOBJ(dib.0));
        // 黑底（透明）上画白锁
        let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00FFFFFF));
        let pen = CreatePen(PS_SOLID, 1, windows::Win32::Foundation::COLORREF(0x00FFFFFF));
        let oldb = SelectObject(hdc, windows::Win32::Graphics::Gdi::HGDIOBJ(brush.0));
        let oldp = SelectObject(hdc, windows::Win32::Graphics::Gdi::HGDIOBJ(pen.0));
        // 锁体（圆角矩形填充）
        let _ = RoundRect(hdc, 1, 6, 9, 11, 2, 2);
        // 锁环（上半弧描边）
        let _ = Arc(hdc, 2, 0, 8, 8, 8, 4, 2, 4);
        let _ = SelectObject(hdc, oldb);
        let _ = SelectObject(hdc, oldp);
        let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(brush.0));
        let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(pen.0));
        // 【关键】此处【不可】把 DIB 选出 DC——ULW 用 hdcSrc 当前选入
        // 的位图上屏，提前恢复 old（空 1x1 单色位图）会让锁窗内容
        // 全空（vis=1 但屏幕上无锁——Chrome/QQ/跟打器全看不见的
        // 病根；PS 验证版没做恢复所以显示成功）。位图随 DC 一起销毁
        // 即可，old 无须恢复。
        // GDI 不写 alpha：非黑像素 alpha 置 255（预乘已满足，白=255×1.0）
        {
            let px = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
            for i in (0..px.len()).step_by(4) {
                if px[i] != 0 || px[i + 1] != 0 || px[i + 2] != 0 {
                    px[i + 3] = 255;
                } else {
                    px[i] = 0; px[i + 1] = 0; px[i + 2] = 0; px[i + 3] = 0;
                }
            }
        }
        // ULW 上屏（窗口保持隐藏，显示由 ShowWindow 控制）。
        // pptDst=None：不动位置（Some(0,0) 会把窗拽到屏幕左上角——
        // 位置由 show_at 的 SetWindowPos 负责）。
        let blend = windows::Win32::Graphics::Gdi::BLENDFUNCTION {
            BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: 235, AlphaFormat: 1,
        };
        let pt = windows::Win32::Foundation::POINT { x: 0, y: 0 };
        let sz = windows::Win32::Foundation::SIZE { cx: w, cy: h };
        let _ = UpdateLayeredWindow(
            hwnd,
            None,
            None,
            Some(&sz as *const windows::Win32::Foundation::SIZE),
            hdc,
            Some(&pt as *const windows::Win32::Foundation::POINT),
            windows::Win32::Foundation::COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(dib.0));
        let _ = DeleteDC(hdc);
        *LOCK_HWND.lock().unwrap() = Some(hwnd.0 as isize);
        crate::tsf::trace(&format!("lockwin: created hwnd={:p}", hwnd.0));
        hwnd.0 as isize
    }
}

/// 右键固定：立即显示锁（定位于候选窗内容区右上角附近）
fn lockwin_show_at(cand: HWND) {
    let h = lockwin_create();
    if h == 0 {
        return;
    }
    unsafe {
        let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let _ = GetWindowRect(cand, &mut wr);
        // 左上角（候选窗窗口矩形左上角内侧）
        let x = wr.left + 5;
        let y = wr.top + 5;
        // 候选窗每帧 SetWindowPos(HWND_TOPMOST) 会把自己顶到 TOPMOST
        // 层最上——锁窗若不跟着置顶会被候选窗盖住（vis=1 但看不见
        // 的病根）。置顶而非 NOZORDER。
        let _ = SetWindowPos(
            HWND(h as *mut _),
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
        let _ = ShowWindow(HWND(h as *mut _), SW_SHOWNOACTIVATE);
        crate::tsf::trace(&format!("lockwin: show L-top ({x},{y}) vis={} err={}", IsWindowVisible(HWND(h as *mut _)).0, GetLastError().0));
    }
}

/// 解除固定/失焦隐藏锁
fn lockwin_hide() {
    if let Some(h) = *LOCK_HWND.lock().unwrap() {
        unsafe {
            let _ = ShowWindow(HWND(h as _), SW_HIDE);
        }
    }
}

/// 候选窗移动时联动锁窗（仅当固定中）
fn lockwin_follow(cand: HWND) {
    if CAND_PINNED.lock().unwrap().is_none() {
        return;
    }
    if let Some(h) = *LOCK_HWND.lock().unwrap() {
        unsafe {
            if IsWindow(HWND(h as _)).as_bool() {
                let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                let _ = GetWindowRect(cand, &mut wr);
                let _ = SetWindowPos(
                    HWND(h as *mut _),
                    HWND_TOPMOST,
                    wr.left + 5,
                    wr.top + 5,
                    0, 0,
                    SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }
}

unsafe extern "system" fn defwindowproc_w(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
