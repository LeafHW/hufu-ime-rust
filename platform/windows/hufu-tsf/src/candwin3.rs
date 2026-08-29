//! candwin3：打包宿主（开始菜单 SearchHost / Microsoft Store / UWP）
//! 专用候选窗——v1 路线复活。
//!
//! 【考古结论】早期版本（WS_EX_LAYERED 普通分层窗 + UpdateLayeredWindow
//! 逐像素 alpha）在 UWP 里**可见、跟光标、一切正常**；被 DWM 以
//! DWM_CLOAKED_SHELL 整体隐身的是 candwin2 的 NOREDIRECTIONBITMAP+
//! DComp 直通窗。此前「v1 同样被隐身」的结论被 cand2_dead 漂移 bug
//! 污染，不可信——本模块即对该路线的回归验证与复活。
//!
//! 渲染：CPU 逐像素预乘合成（与 hufu-server candwin.rs 同一渲染器，
//! 2026-08-29 像素级验证通过）——半透明材质 tint、10 层衰减投影、
//! 圆角 SDF 抗锯齿、1px 边框、胶囊高亮、GDI 灰度 AA 文字 coverage。
//! 定位：candwin2 同款（插入点下方优先/出屏上翻/粘性位置/单调过滤/
//! 多显示器虚拟屏钳制）。
//! 【限制】仅竖排布局（当前皮肤即竖排）；横排皮肤在打包宿主暂按竖排。

use serde_json::Value;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, GdiFlush,
    GetTextExtentPoint32W, SelectObject, SetBkMode, SetTextColor, TextOutW, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HFONT, HGDIOBJ, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetForegroundWindow, GetSystemMetrics, GetWindowRect,
    IsWindowVisible, RegisterClassW, ShowWindow, CW_USEDEFAULT, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_HIDE, SW_SHOWNOACTIVATE,
    WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

// ── 常量（windows crate 缺的零星几个）──
const AC_SRC_OVER: u8 = 1;
const AC_SRC_ALPHA: u8 = 1;
const ULW_ALPHA: u32 = 2;

#[repr(C)]
#[allow(non_snake_case)]
struct BLENDFUNCTION {
    BlendOp: u8,
    BlendFlags: u8,
    SourceConstantAlpha: u8,
    AlphaFormat: u8,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn UpdateLayeredWindow(
        hwnd: HWND,
        hdcdst: HDC,
        pptdst: *const POINT,
        psize: *const SIZE,
        hdcsrc: HDC,
        pptsrc: *const POINT,
        crkey: COLORREF,
        pblend: *const BLENDFUNCTION,
        dwflags: u32,
    ) -> i32;
}

pub struct CandWin3 {
    hwnd: HWND,
    /// 粘性位置（锚点暂缺时沿用，防瞬移屏幕中下）
    sticky_pos: Option<(i32, i32)>,
    last_raw_len: usize,
    /// 连续被 DWM cloaked 的帧数（逃生门：达阈值切 server 代画）
    pub(crate) cloaked_streak: u32,
}

// ── 皮肤取值（与 candwin2/server 同字段同默认）──

fn parse_hex4(s: &str) -> Option<(u8, u8, u8, u8)> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 && s.len() != 8 {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some((b(0)?, b(2)?, b(4)?, if s.len() == 8 { b(6)? } else { 255 }))
}

fn skin_color4(skin: &Value, key: &str, default: &str) -> (u8, u8, u8, u8) {
    let hex = skin
        .pointer(&format!("/skin/colors/{key}"))
        .or_else(|| skin.get("colors").and_then(|c| c.get(key)))
        .and_then(|x| x.as_str())
        .unwrap_or(default);
    parse_hex4(hex).unwrap_or((32, 32, 34, 230))
}

fn layout_f(skin: &Value, key: &str, default: f32) -> f32 {
    skin.pointer(&format!("/skin/layout/{key}"))
        .or_else(|| skin.get("layout").and_then(|l| l.get(key)))
        .and_then(|x| x.as_f64())
        .unwrap_or(default as f64) as f32
}

fn font_face(skin: &Value) -> String {
    skin.pointer("/skin/layout/font_face")
        .or_else(|| skin.get("layout").and_then(|l| l.get("font_face")))
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("Microsoft YaHei UI")
        .to_string()
}

// ── 画布：预乘 BGRA（f32），逐像素合成（与 server 渲染器同源）──

struct Canvas {
    w: i32,
    h: i32,
    px: Vec<f32>,
}

impl Canvas {
    fn new(w: i32, h: i32) -> Canvas {
        Canvas { w, h, px: vec![0.0; (w as usize) * (h as usize) * 4] }
    }
}

/// 预乘 over：dst' = src + dst*(1-sa)
#[inline]
fn blend_px(px: &mut [f32], i: usize, sb: f32, sg: f32, sr: f32, sa: f32) {
    let k = 1.0 - sa;
    px[i] = sb + px[i] * k;
    px[i + 1] = sg + px[i + 1] * k;
    px[i + 2] = sr + px[i + 2] * k;
    px[i + 3] = sa + px[i + 3] * k;
}

/// 圆角矩形 SDF（点在边上为 0，内部为负）
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

/// 填充圆角矩形（1px 抗锯齿）。【预乘铁律】颜色分量 × 总 alpha
fn fill_round_rect(
    c: &mut Canvas,
    x0: f32, y0: f32, x1: f32, y1: f32, r: f32,
    col: (u8, u8, u8, u8),
) {
    let (rb, gb, rn, ra) = (
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
            blend_px(&mut c.px, i, rb * a, gb * a, rn * a, a);
        }
    }
}

/// 描边圆角矩形（宽 bw，居中于 inset 路径）
fn stroke_round_rect(
    c: &mut Canvas,
    x0: f32, y0: f32, x1: f32, y1: f32, r: f32, bw: f32,
    col: (u8, u8, u8, u8),
) {
    let (rb, gb, rn, ra) = (
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
            blend_px(&mut c.px, i, rb * a, gb * a, rn * a, a);
        }
    }
}

/// GDI 灰度 AA 文字 coverage → 皮肤色合成
#[allow(clippy::too_many_arguments)]
fn composite_text(
    c: &mut Canvas,
    cov_bits: &[u8],
    pitch: usize,
    bx: i32,
    by: i32,
    bw: i32,
    bh: i32,
    col: (u8, u8, u8, u8),
) {
    let (rb, gb, rn, base_a) = (
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
            let cov = cov_bits[(row as usize) * pitch + colx as usize * 4] as f32 / 255.0;
            if cov <= 0.0 {
                continue;
            }
            let a = base_a * cov;
            let i = ((cy as usize * c.w as usize) + cx as usize) * 4;
            blend_px(&mut c.px, i, rb * a, gb * a, rn * a, a);
        }
    }
}

/// DIB 头（顶朝下：biHeight 为负，行序与屏幕一致）
fn dib_header(w: i32, h: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [Default::default()],
    }
}

impl CandWin3 {
    /// 建窗（失败返回 None，调用方走 server 代画兜底）
    pub fn new() -> Option<CandWin3> {
        unsafe {
            let class: Vec<u16> = "HuFuCandWin3\0".encode_utf16().collect();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(defwindowproc_w),
                lpszClassName: PCWSTR(class.as_ptr()),
                hbrBackground: Default::default(),
                ..Default::default()
            };
            let _atom = RegisterClassW(&wc);
            let ex = WINDOW_EX_STYLE(
                WS_EX_LAYERED.0 | WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0 | WS_EX_NOACTIVATE.0,
            );
            let hwnd = CreateWindowExW(
                ex,
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(WS_POPUP.0),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                10,
                10,
                HWND::default(),
                None,
                None,
                None,
            )
            .unwrap_or_default();
            if hwnd.0.is_null() {
                return None;
            }
            Some(CandWin3 {
                hwnd,
                sticky_pos: None,
                last_raw_len: 0,
                cloaked_streak: 0,
            })
        }
    }

    /// 显示/更新一帧（接口与 CandidateWindowV2::show 对齐）
    pub fn show(
        &mut self,
        cands: &[(String, String)],
        raw: &str,
        skin: &Value,
        anchor: Option<&RECT>,
        selected: usize,
    ) {
        let show_index = skin
            .get("show_index")
            .and_then(|x| x.as_bool())
            .unwrap_or(true);
        let font_pt = layout_f(skin, "font_point", 16.0);
        let radius = layout_f(skin, "corner_radius", 8.0);
        let margin_x = layout_f(skin, "margin_x", 8.0);
        let margin_y = layout_f(skin, "margin_y", 5.0);
        let line_h = font_pt * 96.0 / 72.0 + layout_f(skin, "line_spacing", 3.0) + 5.0;
        let width_cfg = layout_f(skin, "width", 0.0);
        let min_width = layout_f(skin, "min_width", 150.0).max(100.0);
        let label_w = if show_index { 26.0f32 } else { 0.0 };
        let em = font_pt * 96.0 / 72.0;
        let cand_spacing = layout_f(skin, "candidate_spacing", 6.0);
        let hilite_pad = layout_f(skin, "hilite_padding", 4.0);
        let hi_radius = layout_f(skin, "hilited_corner_radius", 6.0);
        let label_pt = layout_f(skin, "label_font_point", 0.0);
        let border_w = layout_f(skin, "border_width", 1.0).max(0.0);

        let shadow_radius = layout_f(skin, "shadow_radius", 6.0).clamp(0.0, 24.0);
        let shadow_off_y = layout_f(skin, "shadow_offset_y", 2.0);
        let has_shadow = shadow_radius >= 1.0;
        let shadow_m = if has_shadow {
            (shadow_radius * 1.6 + 5.0 + shadow_off_y.abs()).ceil() as i32
        } else {
            0
        };

        // 材质（candwin2 同语义：solid=底色；其余=tint 半透明 ×opacity）
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
        let tint_hex = mat
            .and_then(|m| m.get("tint"))
            .and_then(|x| x.as_str())
            .and_then(parse_hex4);
        let bg_col = if kind == "solid" {
            let c = skin_color4(skin, "back_color", "#202022E6");
            (c.0, c.1, c.2, (c.3 as f32 * opacity) as u8)
        } else {
            let t = tint_hex.unwrap_or((28, 28, 30, 204));
            let a = match kind.as_str() {
                "glass" => t.3 as f32 / 255.0 * 0.55,
                _ => t.3 as f32 / 255.0 * 0.85,
            };
            (t.0, t.1, t.2, (a * opacity * 255.0) as u8)
        };

        // ── GDI：字体、测宽、文字 coverage ──
        unsafe {
            let hdc = CreateCompatibleDC(None);
            let face: Vec<u16> = {
                let mut v: Vec<u16> = font_face(skin).encode_utf16().collect();
                v.push(0);
                v
            };
            let mk_font = |h: f32| -> HFONT {
                CreateFontW(
                    -(h.max(4.0).round() as i32),
                    0,
                    0,
                    0,
                    400, // FW_NORMAL
                    0,
                    0,
                    0,
                    0x86, // DEFAULT_CHARSET
                    0,
                    0,
                    4, // ANTIALIASED_QUALITY（灰度 AA：coverage 通道一致）
                    0,
                    PCWSTR(face.as_ptr()),
                )
            };
            let h_main = mk_font(em);
            let h_small = mk_font(em * 0.78);
            let label_h = if label_pt > 0.0 { label_pt * 96.0 / 72.0 } else { em * 0.78 };
            let h_label = if label_pt > 0.0 { mk_font(label_h) } else { HFONT::default() };
            let h_label_eff = if h_label.is_invalid() { h_small } else { h_label };

            let measure = |hf: HFONT, s: &str| -> f32 {
                if s.is_empty() || hf.is_invalid() {
                    return 0.0;
                }
                let old = SelectObject(hdc, HGDIOBJ(hf.0));
                let ws: Vec<u16> = s.encode_utf16().collect();
                let mut sz = SIZE { cx: 0, cy: 0 };
                let _ = GetTextExtentPoint32W(hdc, &ws, &mut sz);
                let _ = SelectObject(hdc, old);
                sz.cx as f32
            };

            let n = cands.len().min(9);
            let mut max_text = 0.0f32;
            let mut max_cmt = 0.0f32;
            for (t, c) in cands.iter().take(n) {
                max_text = max_text.max(measure(h_main, t));
                if !c.is_empty() {
                    max_cmt = max_cmt.max(measure(h_small, c));
                }
            }
            let raw_w = if cands.is_empty() && !raw.is_empty() {
                measure(h_main, raw)
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
            let code_row = if raw.is_empty() { 0.0f32 } else { 1.0 };
            let rows = n as f32 + code_row;
            let height = margin_y * 2.0 + line_h * rows + cand_spacing * (rows - 1.0).max(0.0);

            let w_out = width as i32 + 2 * shadow_m;
            let h_out = height as i32 + 2 * shadow_m;
            let m = shadow_m as f32;
            let mut canvas = Canvas::new(w_out.max(1), h_out.max(1));

            // 1) 投影：10 层外扩衰减（内浓外淡）
            if has_shadow {
                let sc = skin_color4(skin, "shadow_color", "#000000FF");
                let sa = sc.3 as f32 / 255.0;
                if sa > 0.004 {
                    const PASSES: usize = 10;
                    for i in (1..=PASSES).rev() {
                        let t = i as f32 / PASSES as f32;
                        let grow = shadow_radius * t;
                        let a = sa * (1.0 - t) * (1.0 - t);
                        fill_round_rect(
                            &mut canvas,
                            m - grow,
                            m - grow + shadow_off_y * t,
                            m + width + grow,
                            m + height + grow + shadow_off_y * t,
                            radius + grow,
                            (sc.0, sc.1, sc.2, (a * 255.0) as u8),
                        );
                    }
                }
            }
            // 2) 底板 + 边框
            fill_round_rect(&mut canvas, m, m, m + width, m + height, radius, bg_col);
            if border_w > 0.0 {
                stroke_round_rect(
                    &mut canvas, m, m, m + width, m + height, radius, border_w,
                    skin_color4(skin, "border_color", "#FFFFFF26"),
                );
            }
            // 3) 高亮胶囊
            let sel = selected.min(n.saturating_sub(1));
            let y0 = margin_y + (line_h + cand_spacing) * code_row;
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

            // 4) 文字（coverage DIB）
            let mut bmi = dib_header(w_out.max(1), h_out.max(1));
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
                .unwrap_or_default();
            if dib.is_invalid() || bits.is_null() {
                let _ = DeleteObject(HGDIOBJ(h_main.0));
                let _ = DeleteObject(HGDIOBJ(h_small.0));
                if !h_label.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(h_label.0));
                }
                let _ = DeleteDC(hdc);
                return;
            }
            let old_bmp = SelectObject(hdc, HGDIOBJ(dib.0));
            let _ = SetBkMode(hdc, TRANSPARENT);
            let _ = SetTextColor(hdc, COLORREF(0x00FF_FF_FF));
            let pitch = w_out as usize * 4;
            let cov_len = pitch * h_out as usize;

            let mut draw_text = |canvas: &mut Canvas,
                                 hf: HFONT,
                                 s: &str,
                                 x: f32,
                                 y_row: f32,
                                 fh: f32,
                                 col: (u8, u8, u8, u8)| {
                if s.is_empty() || hf.is_invalid() {
                    return;
                }
                let wtxt = measure(hf, s);
                if wtxt <= 0.0 {
                    return;
                }
                let bx = x as i32;
                let by = (y_row + (line_h - fh) / 2.0) as i32;
                let bw = (wtxt.ceil() as i32) + 2;
                let bh = (line_h.ceil() as i32) + 2;
                // 清 bbox（coverage DIB 复用）
                for row in 0..bh {
                    let yy = by + row;
                    if yy < 0 || yy >= h_out {
                        continue;
                    }
                    let off = yy as usize * pitch + bx.max(0) as usize * 4;
                    let cnt = bw.min(w_out - bx.max(0)).max(0) as usize * 4;
                    if off + cnt <= cov_len {
                        std::ptr::write_bytes(bits.cast::<u8>().add(off), 0, cnt);
                    }
                }
                let ws: Vec<u16> = s.encode_utf16().collect();
                let old_f = SelectObject(hdc, HGDIOBJ(hf.0));
                let _ = TextOutW(hdc, bx, by, &ws);
                let _ = SelectObject(hdc, old_f);
                let _ = GdiFlush();
                composite_text(
                    canvas,
                    std::slice::from_raw_parts(bits.cast::<u8>(), cov_len),
                    pitch,
                    bx,
                    by,
                    bw,
                    bh,
                    col,
                );
            };

            if !raw.is_empty() {
                draw_text(
                    &mut canvas, h_main, raw, m + margin_x, m + margin_y, em,
                    skin_color4(skin, "text_color", "#E8E8EAFF"),
                );
            }
            let text_x = margin_x + label_w;
            let cmt_x = if max_cmt > 0.0 {
                width - margin_x - max_cmt - 2.0
            } else {
                width
            };
            for (i, (text, cmt)) in cands.iter().take(n).enumerate() {
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
                    draw_text(
                        &mut canvas, h_label_eff, &format!("{}.", i + 1), m + margin_x, m + y,
                        label_h, c_label,
                    );
                }
                draw_text(&mut canvas, h_main, text, m + text_x, m + y, em, c_text);
                if !cmt.is_empty() && max_cmt > 0.0 {
                    draw_text(&mut canvas, h_small, cmt, m + cmt_x, m + y, em * 0.78, c_cmt);
                }
            }
            let _ = SelectObject(hdc, old_bmp);
            let _ = DeleteObject(HGDIOBJ(dib.0));
            let _ = DeleteObject(HGDIOBJ(h_main.0));
            let _ = DeleteObject(HGDIOBJ(h_small.0));
            if !h_label.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(h_label.0));
            }
            let _ = DeleteDC(hdc);

            // 5) 导出预乘 BGRA 字节
            let mut bytes = vec![0u8; canvas.px.len()];
            for (i, v) in canvas.px.iter().enumerate() {
                bytes[i] = (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            }

            // ── 定位（candwin2 同款：插入点下方优先/出屏上翻/粘性/
            //    单调过滤/虚拟屏钳制）──
            let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            let grew = raw.len() >= self.last_raw_len;
            self.last_raw_len = raw.len();
            let (x, y) = match anchor {
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
                            let x = if grew && x < ox - 2 { ox } else { x };
                            let y = if grew && (y - oy).abs() <= 26 { oy } else { y };
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
                    None => {
                        crate::tsf::diag_note("cw3 anchor+sticky 双缺，退到焦点窗口定位");
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
            };
            self.sticky_pos = Some((x, y));

            // ── 上屏：ULW 一次完成 移动+尺寸+逐像素 alpha 显示 ──
            // 【顺序】必须先 ULW 再探 cloak——未显示窗口的 cloak 读数
            // 是建窗瞬态垃圾值（SearchHost/Store 首帧 cloak=2 即此）
            let scr = CreateCompatibleDC(None);
            let mut obits: *mut core::ffi::c_void = std::ptr::null_mut();
            let odib = CreateDIBSection(scr, &dib_header(w_out, h_out), DIB_RGB_COLORS, &mut obits, None, 0)
                .unwrap_or_default();
            let mut ulw_ret = -1i32;
            if !odib.is_invalid() && !obits.is_null() {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), obits.cast::<u8>(), bytes.len());
                let old = SelectObject(scr, HGDIOBJ(odib.0));
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA,
                };
                let pt_dst = POINT { x: x - shadow_m, y: y - shadow_m };
                let sz = SIZE { cx: w_out, cy: h_out };
                let pt_src = POINT { x: 0, y: 0 };
                let ok = UpdateLayeredWindow(
                    self.hwnd,
                    HDC::default(),
                    &pt_dst,
                    &sz,
                    scr,
                    &pt_src,
                    COLORREF(0),
                    &blend,
                    ULW_ALPHA,
                );
                if ok == 0 {
                    // 【坑】被 SW_HIDE 过的窗口 ULW 不会自动再显示——
                    // 先 SW_SHOWNOACTIVATE 再重试（server 侧实测）
                    let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                    let _ = UpdateLayeredWindow(
                        self.hwnd,
                        HDC::default(),
                        &pt_dst,
                        &sz,
                        scr,
                        &pt_src,
                        COLORREF(0),
                        &blend,
                        ULW_ALPHA,
                    );
                }
                ulw_ret = ok;
                let _ = SelectObject(scr, old);
                let _ = DeleteObject(HGDIOBJ(odib.0));
            }
            let _ = DeleteDC(scr);

            // cloak 逃生门探测（ULW 之后，读数可信；dwmapi 动态获取）
            let mut cloaked: u32 = 0;
            let mut hr: i32 = -1;
            {
                #[link(name = "kernel32")]
                unsafe extern "system" {
                    fn GetModuleHandleW(name: *const u16) -> isize;
                    fn GetProcAddress(module: isize, name: *const u8) -> *const core::ffi::c_void;
                }
                type Dwma = unsafe fn(HWND, u32, *mut core::ffi::c_void, u32) -> i32;
                let mn: Vec<u16> = "dwmapi.dll\0".encode_utf16().collect();
                let md = GetModuleHandleW(mn.as_ptr());
                if md != 0 {
                    let p = GetProcAddress(md, c"DwmGetWindowAttribute".as_ptr() as *const u8);
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
                "cw3 show anchor={} x={} y={} w={} h={} ulw={} cloak={}({:#x}) hr={:#x} streak={}",
                anchor.is_some(),
                x,
                y,
                w_out,
                h_out,
                ulw_ret,
                cloaked,
                cloaked,
                hr,
                self.cloaked_streak
            ));
        }
    }

    pub fn hide(&mut self) {
        // 组段结束：作废「正向打字」单调锁；粘性位置保留（同 candwin2）
        self.last_raw_len = usize::MAX;
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
