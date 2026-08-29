//! 越进程候选窗：server（普通桌面进程）代画——**用户皮肤像素级复刻**。
//!
//! 为什么不能「直接把 DLL 自绘窗移过来」：DLL 在 SearchHost 进程里
//! 创建的窗口被 DWM 以 DWM_CLOAKED_SHELL 整体隐身（与位置无关，
//! 移到哪都不可见）；ITfUIElement 也被宿主拒绝渲染。唯一出路就是
//! 换进程画。本模块把 DLL candwin2.rs 的渲染规格 1:1 复刻到 server：
//! - UpdateLayeredWindow 逐像素 alpha 分层窗（半透明材质+软投影+圆角）
//! - 材质：translucent → tint×0.85×opacity；solid → back_color
//! - 投影：10 层外扩圆角矩形衰减（内浓外淡），shadow_m 外扩边距
//! - 布局/配色/字号/胶囊高亮全部同公式（margin/line_h/label_w 26/
//!   candidate_spacing/hilite_padding/pill_v/注释 0.78em 右贴）
//! - 文本：GDI 灰度抗锯齿 → coverage 提取 → 预乘合成（皮肤字体）
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
const WM_ERASEBKGND: u32 = 0x0014;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOSIZE: u32 = 0x0001;
const SW_HIDE: i32 = 0;
const HWND_TOPMOST: isize = -1;
const HWND_NOTOPMOST: isize = -2;
const WS_POPUP: u32 = 0x8000_0000;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const WS_EX_TOPMOST: u32 = 0x0000_0008;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
const WS_EX_LAYERED: u32 = 0x0008_0000;
const GWL_EXSTYLE: i32 = -20;
const ULW_ALPHA: u32 = 2;
const AC_SRC_OVER: u8 = 1;
const AC_SRC_ALPHA: u8 = 1;

#[repr(C)]
struct RECT { left: i32, top: i32, right: i32, bottom: i32 }

#[repr(C)]
struct POINT { x: i32, y: i32 }

#[repr(C)]
struct SIZE { cx: i32, cy: i32 }

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
struct BLENDFUNCTION {
    blend_op: u8,
    blend_flags: u8,
    source_constant_alpha: u8,
    alpha_format: u8,
}

#[repr(C)]
struct BITMAPINFOHEADER {
    bi_size: u32,
    bi_width: i32,
    bi_height: i32,
    bi_planes: u16,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_size_image: u32,
    bi_x_pels: i32,
    bi_y_pels: i32,
    bi_clr_used: u32,
    bi_clr_important: u32,
}

#[repr(C)]
struct BITMAPINFO {
    bmi_header: BITMAPINFOHEADER,
    bmi_colors: [u32; 1],
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
    fn GetClientRect(hwnd: isize, r: *mut RECT) -> i32;
    fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
    fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    fn ValidateRect(hwnd: isize, r: *const RECT) -> i32;
    fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn BringWindowToTop(hwnd: isize) -> i32;
    fn IsWindow(hwnd: isize) -> i32;
    fn IsWindowVisible(hwnd: isize) -> i32;
    fn UpdateLayeredWindow(
        hwnd: isize, hdcdst: isize, pptdst: *const POINT, psize: *const SIZE,
        hdcsrc: isize, pptsrc: *const POINT, crkey: u32,
        pblend: *const BLENDFUNCTION, flags: u32,
    ) -> i32;
    fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
    fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
}

#[link(name = "gdi32")]
extern "system" {
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
    fn CreateCompatibleDC(hdc: isize) -> isize;
    fn DeleteDC(hdc: isize) -> i32;
    fn CreateDIBSection(
        hdc: isize, bmi: *const BITMAPINFO, usage: u32,
        bits: *mut *mut core::ffi::c_void, section: isize, offset: u32,
    ) -> isize;
    fn GdiFlush() -> i32;
}

// ── 皮肤取值（与 DLL candwin2.rs 同字段同默认值）──

/// hex → RGBA(0-255)；#RRGGBB 视为不透明
fn parse_hex4(s: &str) -> Option<(u8, u8, u8, u8)> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 && s.len() != 8 {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some((
        b(0)?,
        b(2)?,
        b(4)?,
        if s.len() == 8 { b(6)? } else { 255 },
    ))
}

fn skin_color4(skin: &serde_json::Value, key: &str, default: &str) -> (u8, u8, u8, u8) {
    let hex = skin
        .pointer(&format!("/skin/colors/{key}"))
        .or_else(|| skin.get("colors").and_then(|c| c.get(key)))
        .and_then(|x| x.as_str())
        .unwrap_or(default);
    parse_hex4(hex).unwrap_or((32, 32, 34, 230))
}

fn skin_layout(skin: &serde_json::Value, key: &str, default: f32) -> f32 {
    skin.pointer(&format!("/skin/layout/{key}"))
        .or_else(|| skin.get("layout").and_then(|l| l.get(key)))
        .and_then(|x| x.as_f64())
        .unwrap_or(default as f64) as f32
}

fn skin_font_face(skin: &serde_json::Value) -> String {
    skin.pointer("/skin/layout/font_face")
        .or_else(|| skin.get("layout").and_then(|l| l.get("font_face")))
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("Microsoft YaHei UI")
        .to_string()
}

// ── 画布：预乘 BGRA，逐像素合成 ──

struct Canvas {
    w: i32,
    h: i32,
    /// 预乘 BGRA（[b,g,r,a] × w*h）
    px: Vec<f32>,
}

impl Canvas {
    fn new(w: i32, h: i32) -> Canvas {
        Canvas { w, h, px: vec![0.0; (w as usize) * (h as usize) * 4] }
    }
}

/// 单像素合成（预乘 over）：dst' = src + dst*(1-sa)
#[inline]
fn blend_px(px: &mut [f32], i: usize, sb: f32, sg: f32, sr: f32, sa: f32) {
    let k = 1.0 - sa;
    px[i] = sb + px[i] * k;
    px[i + 1] = sg + px[i + 1] * k;
    px[i + 2] = sr + px[i + 2] * k;
    px[i + 3] = sa + px[i + 3] * k;
}

/// 圆角矩形 SDF（点在边上为 0，内部为负）；矩形 (x0,y0)-(x1,y1)，角半径 r
fn sd_round_rect(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> f32 {
    let cx = (x0 + x1) * 0.5;
    let cy = (y0 + y1) * 0.5;
    let hx = (x1 - x0) * 0.5;
    let hy = (y1 - y0) * 0.5;
    let r = r.min(hx).min(hy);
    let dx = (px - cx).abs() - (hx - r);
    let dy = (py - cy).abs() - (hy - r);
    let ox = dx.max(0.0);
    let oy = dy.max(0.0);
    ((ox * ox + oy * oy).sqrt() - r).min(dx.max(dy))
}

/// 填充圆角矩形（带 1px 抗锯齿），预乘 src
/// 【预乘铁律】src 颜色分量必须 × 总 alpha（ra×cov），否则半透明
/// 会呈现为不透明（实测底色 #262626@68% 显示成纯 #262626）
fn fill_round_rect(
    c: &mut Canvas,
    x0: f32, y0: f32, x1: f32, y1: f32, r: f32,
    col: (u8, u8, u8, u8),
) {
    let (rb, gb, rb_, ra) = (
        col.2 as f32 / 255.0,
        col.1 as f32 / 255.0,
        col.0 as f32 / 255.0,
        col.3 as f32 / 255.0,
    );
    let xa = (x0 - 1.5).floor().max(0.0) as i32;
    let xb = (x1 + 1.5).ceil().min(c.w as f32) as i32;
    let ya = (y0 - 1.5).floor().max(0.0) as i32;
    let yb = (y1 + 1.5).ceil().min(c.h as f32) as i32;
    for y in ya..yb {
        for x in xa..xb {
            let d = sd_round_rect(x as f32 + 0.5, y as f32 + 0.5, x0, y0, x1, y1, r);
            let cov = (0.5 - d).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            let a = ra * cov;
            let i = ((y as usize * c.w as usize) + x as usize) * 4;
            blend_px(&mut c.px, i, rb * a, gb * a, rb_ * a, a);
        }
    }
}

/// 描边圆角矩形（宽 bw，带 AA），预乘 src
fn stroke_round_rect(
    c: &mut Canvas,
    x0: f32, y0: f32, x1: f32, y1: f32, r: f32, bw: f32,
    col: (u8, u8, u8, u8),
) {
    let (rb, gb, rb_, ra) = (
        col.2 as f32 / 255.0,
        col.1 as f32 / 255.0,
        col.0 as f32 / 255.0,
        col.3 as f32 / 255.0,
    );
    let inset = bw * 0.5;
    let xa = (x0 - 1.5).floor().max(0.0) as i32;
    let xb = (x1 + 1.5).ceil().min(c.w as f32) as i32;
    let ya = (y0 - 1.5).floor().max(0.0) as i32;
    let yb = (y1 + 1.5).ceil().min(c.h as f32) as i32;
    for y in ya..yb {
        for x in xa..xb {
            let d = sd_round_rect(
                x as f32 + 0.5, y as f32 + 0.5,
                x0 + inset, y0 + inset, x1 - inset, y1 - inset, (r - inset).max(0.0),
            );
            let cov = (bw * 0.5 + 0.5 - d.abs()).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            let a = ra * cov;
            let i = ((y as usize * c.w as usize) + x as usize) * 4;
            blend_px(&mut c.px, i, rb * a, gb * a, rb_ * a, a);
        }
    }
}

/// 把 GDI 灰度 AA 文字的 coverage 合成进画布（文字色 col）
fn composite_text(
    c: &mut Canvas,
    cov_bits: &[u8], pitch: usize,
    bx: i32, by: i32, bw: i32, bh: i32,
    col: (u8, u8, u8, u8),
) {
    let (rb, gb, rb_, base_a) = (
        col.2 as f32 / 255.0,
        col.1 as f32 / 255.0,
        col.0 as f32 / 255.0,
        col.3 as f32 / 255.0,
    );
    for row in 0..bh {
        let cy = by + row;
        if cy < 0 || cy >= c.h {
            continue;
        }
        for colx in 0..bw {
            let cx = bx + colx;
            if cx < 0 || cx >= c.w {
                continue;
            }
            // 【坐标病根】coverage 在 DIB 里是绝对位置 (bx+colx, by+row)
            // ——曾按相对索引读左上角空白区，文字全部丢失（窗口只剩
            // 底板+胶囊的空面板，用户实测 Store 候选无字即此）
            let cov = cov_bits[((by + row) as usize) * pitch + ((bx + colx) as usize) * 4]
                as f32
                / 255.0;
            if cov <= 0.0 {
                continue;
            }
            let a = base_a * cov;
            let i = ((cy as usize * c.w as usize) + cx as usize) * 4;
            blend_px(&mut c.px, i, rb * a, gb * a, rb_ * a, a);
        }
    }
}

/// 渲染整帧 → (w_out, h_out, BGRA 预乘字节, shadow_m)
fn render_frame(f: &CandFrame) -> (i32, i32, Vec<u8>, i32) {
    let skin = &f.skin;

    // ── 皮肤参数（与 candwin2 show() 同公式）──
    let font_pt = skin_layout(skin, "font_point", 16.0);
    let radius = skin_layout(skin, "corner_radius", 8.0);
    let margin_x = skin_layout(skin, "margin_x", 8.0);
    let margin_y = skin_layout(skin, "margin_y", 5.0);
    let line_h = font_pt * 96.0 / 72.0 + skin_layout(skin, "line_spacing", 3.0) + 5.0;
    let width_cfg = skin_layout(skin, "width", 0.0);
    let min_width = skin_layout(skin, "min_width", 150.0).max(100.0);
    let label_pt = skin_layout(skin, "label_font_point", 0.0);
    let em = font_pt * 96.0 / 72.0;
    let cand_spacing = skin_layout(skin, "candidate_spacing", 6.0);
    let hilite_pad = skin_layout(skin, "hilite_padding", 4.0);
    let hi_radius = skin_layout(skin, "hilited_corner_radius", 6.0);
    let show_index = skin
        .get("show_index")
        .and_then(|x| x.as_bool())
        .unwrap_or(true);
    let label_w = if show_index { 26.0f32 } else { 0.0 };

    // 投影（多层外扩衰减）
    let shadow_radius = skin_layout(skin, "shadow_radius", 6.0).clamp(0.0, 24.0);
    let shadow_off_y = skin_layout(skin, "shadow_offset_y", 2.0);
    let has_shadow = shadow_radius >= 1.0;
    let shadow_m = if has_shadow {
        (shadow_radius * 1.6 + 5.0 + shadow_off_y.abs()).ceil() as i32
    } else {
        0
    };

    // 材质：solid=底色 / translucent|glass|frosted=tint 半透明（×opacity）
    let kind = skin
        .pointer("/skin/material/kind")
        .or_else(|| skin.get("material").and_then(|m| m.get("kind")))
        .and_then(|x| x.as_str())
        .unwrap_or("solid")
        .to_string();
    let mat = skin.pointer("/skin/material").or_else(|| skin.get("material"));
    let opacity = mat
        .and_then(|m| m.get("opacity"))
        .and_then(|x| x.as_f64())
        .unwrap_or(1.0)
        .clamp(0.0, 1.0) as f32;
    let tint_hex = skin
        .pointer("/skin/material/tint")
        .or_else(|| skin.get("material").and_then(|m| m.get("tint")))
        .and_then(|x| x.as_str())
        .and_then(parse_hex4);
    let bg_col = if kind == "solid" {
        let c = skin_color4(skin, "back_color", "#202022E6");
        (c.0, c.1, c.2, (c.3 as f32 * opacity) as u8)
    } else {
        let t = tint_hex.unwrap_or((28, 28, 30, 204));
        let a = match kind.as_str() {
            "glass" => t.3 as f32 / 255.0 * 0.55,
            _ => t.3 as f32 / 255.0 * 0.85, // translucent / frosted
        };
        (t.0, t.1, t.2, (a * opacity * 255.0) as u8)
    };
    let border_col = skin_color4(skin, "border_color", "#FFFFFF26");
    let border_w = skin_layout(skin, "border_width", 1.0).max(0.0);

    // ── GDI：字体 + 测宽 + 文字 coverage ──
    let face: Vec<u16> = {
        let mut v: Vec<u16> = skin_font_face(skin).encode_utf16().collect();
        v.push(0);
        v
    };
    unsafe {
        let hdc = CreateCompatibleDC(0);
        // 灰度 AA（quality=4）：coverage 通道一致，ClearType 会偏色
        let mk_font = |h: f32| {
            CreateFontW(
                -(h.max(4.0).round() as i32), 0, 0, 0, 400, 0, 0, 0,
                0x86 /*DEFAULT_CHARSET*/, 0, 0, 4 /*ANTIALIASED*/, 0, face.as_ptr(),
            )
        };
        let h_main = mk_font(em);
        let h_small = mk_font(em * 0.78);
        let h_label = if label_pt > 0.0 {
            mk_font(label_pt * 96.0 / 72.0)
        } else {
            mk_font(em * 0.78)
        };

        let measure = |hf: isize, s: &str| -> f32 {
            if s.is_empty() || hf == 0 {
                return 0.0;
            }
            let old = SelectObject(hdc, hf);
            let ws: Vec<u16> = s.encode_utf16().collect();
            let mut sz = SIZE { cx: 0, cy: 0 };
            let _ = GetTextExtentPoint32W(hdc, ws.as_ptr(), ws.len() as i32, &mut sz);
            let _ = SelectObject(hdc, old);
            sz.cx as f32
        };

        let n = f.items.len().min(9);
        let mut max_text = 0.0f32;
        let mut max_cmt = 0.0f32;
        for (t, c) in f.items.iter().take(n) {
            max_text = max_text.max(measure(h_main, t));
            let cw = if c.is_empty() { 0.0 } else { measure(h_small, c) };
            if !c.is_empty() {
                max_cmt = max_cmt.max(cw);
            }
        }

        // 宽度（竖排）：标签列 + 最宽候选（或长码）+（注释列）+ 余量
        let raw_w = if f.items.is_empty() && !f.raw.is_empty() {
            measure(h_main, &f.raw)
        } else {
            0.0
        };
        let mut need = margin_x + label_w + max_text.max(raw_w) + margin_x + 6.0;
        if max_cmt > 0.0 {
            need += 6.0 + max_cmt;
        }
        let width = if width_cfg > 0.0 {
            width_cfg
        } else {
            need.clamp(min_width, 300.0)
        };
        let code_row = if f.raw.is_empty() { 0.0f32 } else { 1.0 };
        let rows = n as f32 + code_row;
        let height = margin_y * 2.0 + line_h * rows + cand_spacing * (rows - 1.0).max(0.0);

        let w_out = (width as i32) + 2 * shadow_m;
        let h_out = (height as i32) + 2 * shadow_m;
        let m = shadow_m as f32;
        let mut canvas = Canvas::new(w_out.max(1), h_out.max(1));

        // ── 投影：10 层外扩圆角矩形衰减（内浓外淡，同 DLL 顺序）──
        if has_shadow {
            let sc = skin_color4(skin, "shadow_color", "#000000FF");
            let sa = sc.3 as f32 / 255.0;
            if sa > 0.004 {
                const PASSES: usize = 10;
                for i in (1..=PASSES).rev() {
                    let t = i as f32 / PASSES as f32;
                    let grow = shadow_radius * t;
                    let a = sa * (1.0 - t) * (1.0 - t);
                    let col = (sc.0, sc.1, sc.2, (a * 255.0) as u8);
                    fill_round_rect(
                        &mut canvas,
                        m - grow,
                        m - grow + shadow_off_y * t,
                        m + width + grow,
                        m + height + grow + shadow_off_y * t,
                        radius + grow,
                        col,
                    );
                }
            }
        }

        // ── 底板（材质半透明）+ 边框 ──
        fill_round_rect(&mut canvas, m, m, m + width, m + height, radius, bg_col);
        if border_w > 0.0 {
            stroke_round_rect(
                &mut canvas, m, m, m + width, m + height, radius, border_w, border_col,
            );
        }

        // ── 高亮胶囊（先画，文字后画）──
        let sel = f.selected.min(n.saturating_sub(1));
        let y0 = margin_y + (line_h + cand_spacing) * code_row;
        // pill_v：胶囊上下（em 行盒垂直居中 + hilite_pad）
        let pill_v = |y: f32| -> (f32, f32) {
            let half = (line_h - em) / 2.0;
            let ih = em + hilite_pad * 2.0;
            if ih <= line_h {
                (y + half - hilite_pad, y + half + em + hilite_pad)
            } else {
                let off = (line_h - ih) / 2.0;
                (y + off, y + off + ih)
            }
        };
        if n > 0 {
            let y_sel = y0 + (line_h + cand_spacing) * sel as f32;
            let (pt, pb) = pill_v(y_sel);
            fill_round_rect(
                &mut canvas,
                m + margin_x - hilite_pad,
                m + pt,
                m + width - margin_x + hilite_pad,
                m + pb,
                hi_radius,
                skin_color4(skin, "hilited_candidate_back_color", "#404046FF"),
            );
        }

        // ── 文字（GDI coverage → 合成）──
        // 文字 DIB（顶朝下 biHeight 负，32bpp）
        let mut bmi = BITMAPINFO {
            bmi_header: BITMAPINFOHEADER {
                bi_size: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                bi_width: w_out.max(1),
                bi_height: -h_out.max(1),
                bi_planes: 1,
                bi_bit_count: 32,
                bi_compression: 0,
                bi_size_image: 0,
                bi_x_pels: 0,
                bi_y_pels: 0,
                bi_clr_used: 0,
                bi_clr_important: 0,
            },
            bmi_colors: [0],
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(hdc, &bmi, 0, &mut bits, 0, 0);
        if dib != 0 && !bits.is_null() {
            let old_bmp = SelectObject(hdc, dib);
            let _ = SetBkMode(hdc, 1 /*TRANSPARENT*/);
            let _ = SetTextColor(hdc, 0x00FF_FF_FF);
            let pitch = (w_out as usize) * 4;

            // ── 光学垂直居中自标定（对齐 candwin2 的墨盒补偿）──
            // 文楷类字体升降部大：GDI 字格（tmHeight）比请求的 em 高
            // 一截，而 draw_text 按 em 居中字格 → 墨迹整体偏下（实测
            // 低 3.5px）。这里直接画"永"探针扫描真实墨盒 [top,bot]，
            // dy = em/2 − 墨盒中心（负=上移），全行共用保行栅格一致。
            // 字体度量推导会因字格缩放失真，实测扫描才是真值。
            let mut dy = 0.0f32;
            {
                let s = "永";
                let pw = measure(h_main, s).ceil() as i32;
                if pw > 0 && pw < w_out {
                    let ph = (line_h.ceil() as i32 + 8).min(h_out);
                    // 清探针区（DIB 新建本为 0，稳妥再清一次）
                    for row in 0..ph {
                        let off = row as usize * pitch;
                        std::ptr::write_bytes((bits as *mut u8).add(off), 0, pw as usize * 4 + 4);
                    }
                    let ws: Vec<u16> = s.encode_utf16().collect();
                    let old_f = SelectObject(hdc, h_main);
                    let _ = TextOutW(hdc, 0, 0, ws.as_ptr(), ws.len() as i32);
                    let _ = SelectObject(hdc, old_f);
                    let _ = GdiFlush();
                    let mut top = -1i32;
                    let mut bot = -1i32;
                    for row in 0..ph {
                        for col in 0..=pw {
                            if (bits as *const u8).add(row as usize * pitch + col as usize * 4)
                                .read_volatile()
                                > 0
                            {
                                if top < 0 {
                                    top = row;
                                }
                                bot = row;
                            }
                        }
                    }
                    if top >= 0 && bot > top {
                        dy = (em - (top + bot) as f32) * 0.5;
                        dy = dy.clamp(-6.0, 6.0);
                    }
                    // 探针墨迹清掉，不进合成
                    for row in 0..ph {
                        let off = row as usize * pitch;
                        std::ptr::write_bytes((bits as *mut u8).add(off), 0, pw as usize * 4 + 4);
                    }
                }
            }

            // 一段文字：清 bbox → 画 → coverage 合成
            let mut draw_text = |canvas: &mut Canvas, hf: isize, s: &str, x: f32, y_row: f32, fh: f32, col: (u8, u8, u8, u8)| {
                if s.is_empty() || hf == 0 {
                    return;
                }
                let wtxt = measure(hf, s);
                if wtxt <= 0.0 {
                    return;
                }
                let bx = x as i32;
                let by = (y_row + (line_h - fh) / 2.0 + dy) as i32;
                let bw = (wtxt.ceil() as i32) + 2;
                let bh = (line_h.ceil() as i32) + 2;
                // 清 bbox（coverage DIB 复用）
                for row in 0..bh.min(h_out - by.max(0)) {
                    let off = ((by.max(0) + row) as usize) * pitch + (bx.max(0) as usize) * 4;
                    let cnt = (bw.min(w_out - bx.max(0))).max(0) as usize * 4;
                    if off + cnt <= bits as usize + pitch * h_out as usize {
                        std::ptr::write_bytes(
                            (bits as *mut u8).add(off), 0, cnt,
                        );
                    }
                }
                let ws: Vec<u16> = s.encode_utf16().collect();
                let old_f = SelectObject(hdc, hf);
                let _ = TextOutW(hdc, bx, by, ws.as_ptr(), ws.len() as i32);
                let _ = SelectObject(hdc, old_f);
                let _ = GdiFlush();
                composite_text(canvas, std::slice::from_raw_parts(bits as *const u8, pitch * h_out as usize), pitch, bx, by, bw, bh, col);
            };

            // 编码行
            if !f.raw.is_empty() {
                draw_text(
                    &mut canvas, h_main, &f.raw, m + margin_x, m + margin_y, em,
                    skin_color4(skin, "text_color", "#E8E8EAFF"),
                );
            }
            // 候选行
            let text_x = margin_x + label_w;
            let cmt_x = if max_cmt > 0.0 {
                width - margin_x - max_cmt - 2.0
            } else {
                width
            };
            for (i, (text, cmt)) in f.items.iter().take(n).enumerate() {
                let y = y0 + (line_h + cand_spacing) * i as f32;
                let is_sel = i == sel;
                let c_label = if is_sel {
                    skin_color4(skin, "hilited_candidate_label_color", "#FFD75EFF")
                } else {
                    skin_color4(skin, "label_color", "#C9C9C9FF")
                };
                let c_text = if is_sel {
                    skin_color4(skin, "hilited_candidate_text_color", "#FFFFFFFF")
                } else {
                    skin_color4(skin, "candidate_text_color", "#E8E8EAFF")
                };
                let c_cmt = if is_sel {
                    skin_color4(skin, "hilited_comment_text_color", "#C9C9C9FF")
                } else {
                    skin_color4(skin, "comment_text_color", "#9A9AA0FF")
                };
                if show_index {
                    draw_text(&mut canvas, h_label, &format!("{}.", i + 1), m + margin_x, m + y, if label_pt > 0.0 { label_pt * 96.0 / 72.0 } else { em * 0.78 }, c_label);
                }
                draw_text(&mut canvas, h_main, text, m + text_x, m + y, em, c_text);
                if !cmt.is_empty() && max_cmt > 0.0 {
                    draw_text(&mut canvas, h_small, cmt, m + cmt_x, m + y, em * 0.78, c_cmt);
                }
            }
            let _ = SelectObject(hdc, old_bmp);
            let _ = DeleteObject(dib);
        }

        let _ = DeleteObject(h_main);
        let _ = DeleteObject(h_small);
        let _ = DeleteObject(h_label);
        let _ = DeleteDC(hdc);

        // ── 导出预乘 BGRA 字节 ──
        let mut out = vec![0u8; canvas.px.len()];
        for (i, v) in canvas.px.iter().enumerate() {
            out[i] = (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
        (w_out.max(1), h_out.max(1), out, shadow_m)
    }
}

extern "system" fn wnd_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    // panic 绝不能越过 FFI 边界（=进程 abort）——兜住，保住窗口线程
    let r = std::panic::catch_unwind(|| wnd_proc_inner(hwnd, msg, wparam, lparam));
    r.unwrap_or(0)
}

fn wnd_proc_inner(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_PAINT => {
            // ULW 窗口不走 WM_PAINT 上屏；验证掉即可
            unsafe { ValidateRect(hwnd, std::ptr::null()) };
            0
        }
        WM_ERASEBKGND => 1,
        WM_APP_CAND => unsafe {
            // lparam = Box<CandFrame> 指针（pipe 线程移交所有权）
            let frame: Box<CandFrame> = Box::from_raw(lparam as *mut CandFrame);
            let (w_out, h_out, bytes, shadow_m) = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render_frame(&frame))) {
                Ok(v) => v,
                Err(_) => {
                    let _ = std::fs::write(r"C:\ProgramData\HuFu\diag\ulw-dbg.txt", "render_frame PANIC\n");
                    return 0;
                }
            };            *frame_lock() = Some(*frame);
            let x = (wparam >> 32) as i32;
            let y = (wparam as u32) as i32;
            // 内容锚点 (x,y) → 窗口原点 = (x-m, y-m)（投影边距外扩）
            let wx = x - shadow_m;
            let wy = y - shadow_m;
            // 沉浸层压 topmost → NOTOPMOST→TOPMOST 重挂越过（仅会话
            // 首帧，防闪）。【禁忌】AttachThreadInput（见 force_top 注释）
            let first_show = !was_raised();
            force_top(hwnd, first_show);
            mark_raised();
            // 【坑】被 SW_HIDE 过的窗口 UpdateLayeredWindow 不会自动
            // 再显示（实测 vis=0、ret=1）——先 SW_SHOWNOACTIVATE
            let _ = ShowWindow(hwnd, 4 /*SW_SHOWNOACTIVATE*/);
            // 逐像素 alpha 上屏（同时完成 移动+尺寸+显示）
            let hdc = CreateCompatibleDC(0);
            let mut bmi = BITMAPINFO {
                bmi_header: BITMAPINFOHEADER {
                    bi_size: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    bi_width: w_out,
                    bi_height: -h_out,
                    bi_planes: 1,
                    bi_bit_count: 32,
                    bi_compression: 0,
                    bi_size_image: 0,
                    bi_x_pels: 0,
                    bi_y_pels: 0,
                    bi_clr_used: 0,
                    bi_clr_important: 0,
                },
                bmi_colors: [0],
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(hdc, &bmi, 0, &mut bits, 0, 0);
            if dib != 0 && !bits.is_null() {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), bits as *mut u8, bytes.len());
                let old = SelectObject(hdc, dib);
                let blend = BLENDFUNCTION {
                    blend_op: AC_SRC_OVER,
                    blend_flags: 0,
                    source_constant_alpha: 255,
                    alpha_format: AC_SRC_ALPHA,
                };
                let pt_dst = POINT { x: wx, y: wy };
                let sz = SIZE { cx: w_out, cy: h_out };
                let pt_src = POINT { x: 0, y: 0 };
                let ulw_r = UpdateLayeredWindow(
                    hwnd, 0, &pt_dst, &sz, hdc, &pt_src, 0, &blend, ULW_ALPHA,
                );
                if ulw_r == 0 {
                    let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
                    let _ = std::fs::write(r"C:\ProgramData\HuFu\diag\ulw-dbg.txt", "UpdateLayeredWindow FAILED\n");
                }
                let _ = SelectObject(hdc, old);
                let _ = DeleteObject(dib);
            }
            let _ = DeleteDC(hdc);
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

const WM_APP_REINIT: u32 = 0x8003;

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
unsafe fn force_top(hwnd: isize, rebanded: bool) {
    if rebanded {
        let _ = SetWindowPos(
            hwnd, HWND_NOTOPMOST, 0, 0, 0, 0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        );
    }
    let _ = SetWindowPos(
        hwnd, HWND_TOPMOST, 0, 0, 0, 0,
        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
    );
    let _ = BringWindowToTop(hwnd);
}

/// 中毒免疫锁：panic 后状态仍在，读旧值胜过整线程卡死
fn frame_lock() -> std::sync::MutexGuard<'static, Option<CandFrame>> {
    FRAME.lock().unwrap_or_else(|e| e.into_inner())
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

/// 在 tray 线程创建（消息循环已有）：注册类 + 分层隐藏窗口
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
    let ex = WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_LAYERED;
    let hwnd = unsafe {
        CreateWindowExW(
            ex,
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
        format!("init atom={_atom} hwnd={hwnd} layered\n"),
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
