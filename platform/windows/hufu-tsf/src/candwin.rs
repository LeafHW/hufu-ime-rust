//! 候选窗：顶层分层窗口 + GDI 逐像素 alpha（v1）。
//! 材质（毛玻璃/玻璃）在 v1 用半透明 tint + 边框高光模拟；
//! v2 将替换为 Direct2D + DirectComposition + DWM backdrop。

use serde_json::Value;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::PCWSTR;

pub struct CandidateWindow {
    hwnd: HWND,
}

fn color_of(v: &Value, key: &str, default: [u8; 4]) -> [u8; 4] {
    let s = v
        .get("skin")
        .and_then(|s| s.get("colors"))
        .and_then(|c| c.get(key))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    parse_hex(s).unwrap_or(default)
}

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

impl CandidateWindow {
    pub fn new() -> CandidateWindow {
        unsafe {
            let class: Vec<u16> = "HuFuCandWin\0".encode_utf16().collect();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(defwindowproc_w),
                lpszClassName: PCWSTR(class.as_ptr()),
                hbrBackground: HBRUSH(std::ptr::null_mut()),
                style: CS_HREDRAW | CS_VREDRAW,
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
            CandidateWindow { hwnd }
        }
    }

    /// 渲染并显示。
    pub fn show(&self, cands: &[(String, String)], raw: &str, skin: &Value) {
        unsafe {
            let back = color_of(skin, "back_color", [32, 32, 34, 0xE6]);
            let border = color_of(skin, "border_color", [255, 255, 255, 0x26]);
            let txt = color_of(skin, "text_color", [0xE8, 0xE8, 0xEA, 0xFF]);
            let cand_txt = color_of(skin, "candidate_text_color", [0xE8, 0xE8, 0xEA, 0xFF]);
            let cmt_txt = color_of(skin, "comment_text_color", [0x9A, 0x9A, 0xA0, 0xFF]);
            let hi_back = color_of(skin, "hilited_candidate_back_color", [64, 64, 70, 0xFF]);
            let hi_lbl = color_of(skin, "hilited_candidate_label_color", [0xFF, 0xD7, 0x5E, 0xFF]);

            let font_h = 26i32;
            let line_h = font_h + 8;
            let width = 300i32;
            let height = line_h * (cands.len() as i32 + 1) + 16;

            // 32bpp DIB（top-down，预乘 alpha 由底色提供）
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let null_hwnd = HWND(std::ptr::null_mut());
            let hdc = GetDC(null_hwnd);
            let hbmp = CreateDIBSection(
                hdc,
                &bmi,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            )
            .unwrap_or_default();
            let memdc = CreateCompatibleDC(hdc);
            let old = SelectObject(memdc, HGDIOBJ(hbmp.0));
            let _ = ReleaseDC(null_hwnd, hdc);
            let bits = bits as *mut u32;
            if bits.is_null() {
                return;
            }

            let bg_px = ((back[3] as u32) << 24)
                | ((back[2] as u32) << 16)
                | ((back[1] as u32) << 8)
                | back[0] as u32;
            for i in 0..(width * height) as usize {
                *bits.add(i) = bg_px;
            }

            // GDI 文本
            let _ = SetBkMode(memdc, TRANSPARENT);
            let make_font = |h: i32| {
                CreateFontW(
                    -h,
                    0,
                    0,
                    0,
                    FW_NORMAL.0 as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET.0 as u32,
                    0,
                    0,
                    CLEARTYPE_QUALITY.0 as u32,
                    DEFAULT_PITCH.0 as u32,
                    PCWSTR::null(),
                )
            };
            let hfont = make_font(font_h - 8);
            let oldfont = SelectObject(memdc, HGDIOBJ(hfont.0));

            let argb = |c: [u8; 4]| {
                COLORREF(u32::from(c[0]) | (u32::from(c[1]) << 8) | (u32::from(c[2]) << 16))
            };
            let out = |dc: HDC, s: &str, x: i32, y: i32| {
                let w: Vec<u16> = s.encode_utf16().collect();
                if !w.is_empty() {
                    TextOutW(dc, x, y, &w);
                }
            };

            // 编码行
            let _ = SetTextColor(memdc, argb(txt));
            out(memdc, raw, 12, 6);

            // 候选行
            for (i, (text, cmt)) in cands.iter().enumerate().take(9) {
                let y = 6 + line_h * (i as i32 + 1);
                if i == 0 {
                    let hbr = CreateSolidBrush(argb(hi_back));
                    FillRect(
                        memdc,
                        &RECT { left: 6, top: y - 2, right: width - 6, bottom: y + line_h - 4 },
                        hbr,
                    );
                    let _ = DeleteObject(HGDIOBJ(hbr.0));
                    let _ = SetTextColor(memdc, argb(hi_lbl));
                } else {
                    let _ = SetTextColor(memdc, argb(cand_txt));
                }
                out(memdc, &format!("{}. ", i + 1), 12, y);
                out(memdc, text, 40, y);
                if !cmt.is_empty() {
                    let _ = SetTextColor(memdc, argb(cmt_txt));
                    let hfont2 = make_font(font_h - 12);
                    let oldf2 = SelectObject(memdc, HGDIOBJ(hfont2.0));
                    out(memdc, cmt, 130, y + 3);
                    SelectObject(memdc, oldf2);
                    let _ = DeleteObject(HGDIOBJ(hfont2.0));
                }
            }
            SelectObject(memdc, oldfont);
            let _ = DeleteObject(HGDIOBJ(hfont.0));

            // 1px 边框
            let border_px = ((border[3] as u32) << 24)
                | ((border[2] as u32) << 16)
                | ((border[1] as u32) << 8)
                | border[0] as u32;
            for x in 0..width {
                *bits.add(x as usize) = border_px;
                *bits.add((height - 1) as usize * width as usize + x as usize) = border_px;
            }
            for y in 0..height {
                *bits.add(y as usize * width as usize) = border_px;
                *bits.add(y as usize * width as usize + (width - 1) as usize) = border_px;
            }

            // 定位：v1 屏幕下 1/3 居中
            let sw = GetSystemMetrics(SM_CXSCREEN);
            let sh = GetSystemMetrics(SM_CYSCREEN);
            let pt = POINT {
                x: (sw - width) / 2,
                y: sh * 2 / 3,
            };
            let size = SIZE { cx: width, cy: height };
            let ok = UpdateLayeredWindow(
                self.hwnd,
                HDC(std::ptr::null_mut()),
                Some(&pt as *const POINT),
                Some(&size as *const SIZE),
                memdc,
                Some(&POINT { x: 0, y: 0 } as *const POINT),
                COLORREF(0),
                None,
                ULW_ALPHA,
            )
            .is_ok();
            SelectObject(memdc, old);
            let _ = DeleteObject(HGDIOBJ(hbmp.0));
            let _ = DeleteDC(memdc);
            if ok {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
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
