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
use windows_core::PCWSTR;

// ── DWM accent（未公开 API，Win10 1803+ 全系统 IME 通用做法）──

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
    hwnd: HWND,
    ctx: Option<ID2D1DeviceContext>,
    swapchain: Option<IDXGISwapChain1>,
    dcomp: Option<IDCompositionDevice>,
    target: Option<IDCompositionTarget>,
    visual: Option<IDCompositionVisual>,
    dwrite: Option<IDWriteFactory>,
    dxgi: Option<IDXGIDevice>,
    size: (i32, i32),
}

impl CandidateWindowV2 {
    /// 初始化设备管线；任何一步失败返回 None（调用方回退 v1）。
    pub fn new() -> Option<CandidateWindowV2> {
        unsafe {
            let class: Vec<u16> = "HuFuCandWin2\0".encode_utf16().collect();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(defwindowproc_w),
                lpszClassName: PCWSTR(class.as_ptr()),
                hbrBackground: HBRUSH(std::ptr::null_mut()),
                ..Default::default()
            };
            let _atom = RegisterClassW(&wc);
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
                size: (0, 0),
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
        let cand_spacing = layout_f(skin, "candidate_spacing", 6.0);
        let hilite_pad = layout_f(skin, "hilite_padding", 4.0);

        // 字体与内容测宽先行（宽度取决于最长候选）
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
            let tf = mk_tf(&font_face, em).or_else(|| mk_tf("Microsoft YaHei UI", em));
            let tf_small =
                mk_tf(&font_face, em * 0.78).or_else(|| mk_tf("Microsoft YaHei UI", em * 0.78));
            // 标签序号字体（layout.label_font_point；0/缺省回退 0.78 倍正文）
            let label_pt = layout_f(skin, "label_font_point", 0.0);
            let tf_label = if label_pt > 0.0 {
                mk_tf(&font_face, label_pt * 96.0 / 72.0)
                    .or_else(|| mk_tf("Microsoft YaHei UI", label_pt * 96.0 / 72.0))
                    .or(tf_small.clone())
            } else {
                tf_small.clone()
            };

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
            for (t, c) in cands {
                let tw = measure(&tf, t);
                let cw = if c.is_empty() { 0.0 } else { measure(&tf_small, c) };
                max_text = max_text.max(tw);
                if !c.is_empty() {
                    max_cmt = max_cmt.max(cw);
                }
                cand_ws.push((tw, cw));
            }
            // 注意：编码行不参与定宽（长码截断显示，框宽只随候选内容）；
            // 例外：仅提示行窗口（反查/命令进入提示，无候选）时由提示行定宽
            let raw_w = if cands.is_empty() && !raw.is_empty() {
                measure(&tf, raw)
            } else {
                0.0
            };
            let (width, text_x, cmt_x, cmt_w) = if horizontal {
                // 横排：Σ(标签+文本+注释+间隔)，几何在绘制期逐格推进
                let mut w = margin_x * 2.0;
                for (i, (tw, cw)) in cand_ws.iter().enumerate() {
                    if i > 0 {
                        w += cand_spacing;
                    }
                    w += label_w * 0.72 + tw + if *cw > 0.0 { 5.0 + cw } else { 0.0 };
                }
                let w = w.max(raw_w + margin_x * 2.0);
                (w, 0.0, 0.0, 0.0)
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
                let (cmt_x, cmt_w) = if max_cmt > 0.0 {
                    (width - margin_x - max_cmt - 2.0, max_cmt + 2.0)
                } else {
                    (width, 0.0)
                };
                (width, text_x, cmt_x, cmt_w)
            };
            (tf, tf_label, tf_small, cand_ws, (width, text_x, cmt_x, cmt_w))
        };
        let (v_width, text_x, cmt_x, cmt_w) = geo;
        // 编码行仅在有内容时占一行（show_code=false 且无 aux 时收缩）
        let code_row = if raw.is_empty() { 0.0 } else { 1.0 };
        let width = if horizontal {
            // 横排固定宽配置同样生效；自适应保底 min_width
            if width_cfg > 0.0 {
                width_cfg.max(v_width)
            } else {
                v_width.max(min_width)
            }
        } else {
            v_width
        };
        let height = if horizontal {
            margin_y * 2.0 + line_h * (1.0 + code_row) + 4.0
        } else {
            let rows = cands.len().min(9) as f32 + code_row;
            margin_y * 2.0 + (line_h + cand_spacing) * rows + 4.0
        };

        let w = width as u32;
        let h = height as u32;
        if !self.ensure_swapchain(w.max(1), h.max(1)) {
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

            // 背景：非 solid 材质清透明（模糊由 accent 提供）；solid 用皮肤底色
            if kind == "solid" {
                let _ = ctx.Clear(Some(&color_f(skin, "back_color", "#202022E6")));
            } else {
                let _ = ctx.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            }

            let mkbrush = |ctx: &ID2D1DeviceContext, c: D2D1_COLOR_F| -> Option<ID2D1SolidColorBrush> {
                ctx.CreateSolidColorBrush(&c, None).ok()
            };
            let b_text = mkbrush(&ctx, color_f(skin, "candidate_text_color", "#E8E8EAFF"));
            let b_label = mkbrush(&ctx, color_f(skin, "label_color", "#C9C9C9FF"));
            let b_raw = mkbrush(&ctx, color_f(skin, "text_color", "#E8E8EAFF"));
            let b_cmt = mkbrush(&ctx, color_f(skin, "comment_text_color", "#9A9AA0FF"));
            let b_hi = mkbrush(&ctx, color_f(skin, "hilited_candidate_back_color", "#404046FF"));
            let b_hi_txt = mkbrush(&ctx, color_f(skin, "hilited_candidate_text_color", "#FFFFFFFF"));
            let b_hi_lbl = mkbrush(&ctx, color_f(skin, "hilited_candidate_label_color", "#FFD75EFF"));
            let b_hi_cmt = mkbrush(&ctx, color_f(skin, "hilited_comment_text_color", "#C9C9C9FF"));
            let b_border = mkbrush(&ctx, color_f(skin, "border_color", "#FFFFFF26"));

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

            // 编码行（有内容才画；候选行相应上移）
            if !raw.is_empty() {
                draw(&ctx, &tf, raw, margin_x, margin_y, width - margin_x * 2.0, line_h, &b_raw);
            }
            let y0 = margin_y + line_h * code_row;

            // 候选行
            let sel = selected.min(cands.len().saturating_sub(1));
            if horizontal {
                // ── 横排：单行铺开，每格 = 序号+文本(+注释)，高亮为整格胶囊 ──
                let mut x = margin_x;
                let y = y0;
                for (i, (text, cmt)) in cands.iter().enumerate().take(9) {
                    let (tw, cw) = cand_ws.get(i).copied().unwrap_or((0.0, 0.0));
                    let cell_w = label_w * 0.72 + tw + if cw > 0.0 { 5.0 + cw } else { 0.0 };
                    if i > 0 {
                        x += cand_spacing;
                    }
                    if i == sel {
                        if let Some(b) = &b_hi {
                            let rr = D2D1_ROUNDED_RECT {
                                rect: D2D_RECT_F {
                                    left: x - hilite_pad,
                                    top: y,
                                    right: x + cell_w + hilite_pad,
                                    bottom: y + line_h - 2.0,
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
                    cx += tw + 2.0;
                    if !cmt.is_empty() && cw > 0.0 {
                        draw(&ctx, &tf_small, cmt, cx + 3.0, y + 2.0, cw + 2.0, line_h, bc);
                    }
                    x += cell_w;
                }
            } else {
                // ── 竖排（原布局 + candidate_spacing 行距 + hilite_padding 外扩）──
                for (i, (text, cmt)) in cands.iter().enumerate().take(9) {
                    let y = y0 + (line_h + cand_spacing) * i as f32;
                    if i == sel {
                        // 高亮行（圆角胶囊；↑↓ 移动）
                        if let Some(b) = &b_hi {
                            let rr = D2D1_ROUNDED_RECT {
                                rect: D2D_RECT_F {
                                    left: margin_x - 4.0 - hilite_pad,
                                    top: y,
                                    right: width - margin_x + 4.0 + hilite_pad,
                                    bottom: y + line_h - 2.0,
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
                        draw(&ctx, &tf_small, cmt, cmt_x, y + 2.0, width - cmt_x - margin_x + 4.0, line_h, bc);
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
            let _ = chain.Present(1, DXGI_PRESENT(0));
        }

        // accent 材质
        let tint = tint_hex.unwrap_or([28, 28, 30, 204]);
        match kind.as_str() {
            "frosted" => apply_accent(self.hwnd, ACCENT_ENABLE_ACRYLICBLURBEHIND, tint),
            "glass" => apply_accent(self.hwnd, ACCENT_ENABLE_HOSTBACKDROP, tint),
            "translucent" => {
                apply_accent(self.hwnd, ACCENT_ENABLE_TRANSPARENTGRADIENT, tint)
            }
            _ => apply_accent(self.hwnd, ACCENT_DISABLED, tint),
        }

        // 定位：优先插入点下方，出屏翻到上方；无 anchor 屏幕下 1/3 居中
        unsafe {
            let sw = GetSystemMetrics(SM_CXSCREEN);
            let sh = GetSystemMetrics(SM_CYSCREEN);
            let (x, y) = match anchor {
                Some(r) => {
                    let x = (r.left).clamp(0, (sw - width as i32).max(0));
                    let below = r.bottom + 4;
                    if below + height as i32 <= sh {
                        (x, below)
                    } else {
                        (x, (r.top - height as i32 - 4).max(0))
                    }
                }
                None => ((sw - width as i32) / 2, sh * 2 / 3),
            };
            let _ = SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                x,
                y,
                width as i32,
                height as i32,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            crate::tsf::trace(&format!(
                "cw2: SetWindowPos({x},{y}) err={} visible={}",
                GetLastError().0,
                IsWindowVisible(self.hwnd).0
            ));
        }
    }

    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
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
