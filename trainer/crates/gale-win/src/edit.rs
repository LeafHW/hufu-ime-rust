//! Real RichEdit fields for the native panels. They keep normal TSF, clipboard,
//! selection, accessibility, and password semantics; no simulated text input.
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct NativeEdit {
    pub hwnd: isize,
}
impl NativeEdit {
    pub fn new(parent: isize, multiline: bool, password: bool) -> windows::core::Result<Self> {
        unsafe {
            crate::input_host::load_library();
            let mut style = WS_CHILD | WS_TABSTOP | WINDOW_STYLE(0x80); // ES_AUTOHSCROLL
            if multiline {
                style |= WS_VSCROLL | WINDOW_STYLE(0x4 | 0x40 | 0x1000);
            }
            if password {
                style |= WINDOW_STYLE(0x20);
            }
            let h = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("RichEdit50W"),
                w!(""),
                style,
                0,
                0,
                100,
                30,
                HWND(parent as *mut _),
                None,
                None,
                None,
            )?;
            SendMessageW(h, 1228, WPARAM(0x10000), LPARAM(0x10000));
            SendMessageW(h, 0x435, WPARAM(0), LPARAM(100_000));
            if password {
                SendMessageW(h, 0xCC, WPARAM(0x25CF), LPARAM(0));
            }
            Ok(Self { hwnd: h.0 as isize })
        }
    }
    pub fn text(&self) -> String {
        unsafe {
            let h = HWND(self.hwnd as *mut _);
            let n = GetWindowTextLengthW(h).max(0) as usize;
            let mut b = vec![0u16; n.saturating_add(1).min(100_002)];
            let got = GetWindowTextW(h, &mut b).max(0) as usize;
            String::from_utf16_lossy(&b[..got])
        }
    }
    pub fn set_text(&self, text: &str) {
        unsafe {
            let mut b: Vec<u16> = text.encode_utf16().collect();
            b.push(0);
            let _ = SetWindowTextW(HWND(self.hwnd as *mut _), PCWSTR(b.as_ptr()));
        }
    }
    pub fn place(&self, x: f32, y: f32, w: f32, h: f32) {
        unsafe {
            let s = crate::window::dpi_scale();
            let _ = SetWindowPos(
                HWND(self.hwnd as *mut _),
                None,
                (x * s) as i32,
                (y * s) as i32,
                (w * s).max(2.) as i32,
                (h * s).max(2.) as i32,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            // A composition Present does not invalidate the GDI child. Force the
            // newly shown/resized editor to paint its own background and text now.
            use windows::Win32::Graphics::Gdi::{
                RedrawWindow, RDW_FRAME, RDW_INVALIDATE, RDW_UPDATENOW,
            };
            let _ = RedrawWindow(
                HWND(self.hwnd as *mut _),
                None,
                None,
                RDW_INVALIDATE | RDW_UPDATENOW | RDW_FRAME,
            );
        }
    }
    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(HWND(self.hwnd as *mut _), SW_HIDE);
        }
    }
    pub fn focus(&self) {
        unsafe {
            let _ =
                windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(HWND(self.hwnd as *mut _));
        }
    }
    pub fn style(&self, bg: [f32; 4], fg: [f32; 4], size: f32) {
        unsafe {
            let rgb = |c: [f32; 4]| -> u32 {
                (c[0] * 255.) as u32 | ((c[1] * 255.) as u32) << 8 | ((c[2] * 255.) as u32) << 16
            };
            let h = HWND(self.hwnd as *mut _);
            SendMessageW(h, 1091, WPARAM(0), LPARAM(rgb(bg) as isize));
            #[repr(C)]
            struct Format {
                cb: u32,
                mask: u32,
                effects: u32,
                height: i32,
                offset: i32,
                color: u32,
                charset: u8,
                pitch: u8,
                face: [u16; 32],
            }
            let mut f = Format {
                cb: std::mem::size_of::<Format>() as u32,
                mask: 0xE0000000,
                effects: 0,
                height: (size * 15.) as i32,
                offset: 0,
                color: rgb(fg),
                charset: 1,
                pitch: 0,
                face: [0; 32],
            };
            for (dst, ch) in f.face.iter_mut().zip("Microsoft YaHei UI".encode_utf16()) {
                *dst = ch;
            }
            SendMessageW(h, 1092, WPARAM(4), LPARAM(&mut f as *mut _ as isize));
            SendMessageW(h, 1092, WPARAM(0), LPARAM(&mut f as *mut _ as isize));
        }
    }
}
impl Drop for NativeEdit {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(HWND(self.hwnd as *mut _));
        }
    }
}

pub fn reduced_motion() -> bool {
    unsafe {
        let mut enabled = windows::Win32::Foundation::BOOL(1);
        let _ = SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut enabled as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        !enabled.as_bool() || GetSystemMetrics(SM_REMOTESESSION) != 0
    }
}
