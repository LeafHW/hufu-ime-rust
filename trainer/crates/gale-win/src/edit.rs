//! Real RichEdit fields for the native panels. They keep normal TSF, clipboard,
//! selection, accessibility, and password semantics; no simulated text input.
use std::collections::HashMap;
use std::sync::Mutex;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

/// place 去抖缓存（hwnd → 上次物理像素 rect）。主循环每帧 place，
/// rect 相同则跳过 SetWindowPos + RedrawWindow（2026-11-08）。
/// 注：不能放实例字段——gale-app 以结构体字面量构造 NativeEdit
///（main.rs apply_style 的宿主样式包装），加任何字段都会破坏外部构造。
static PLACE_RECTS: Mutex<Option<HashMap<isize, (i32, i32, i32, i32)>>> = Mutex::new(None);

#[derive(Clone, Copy, PartialEq, Eq)]
struct EditStyle {
    background: u32,
    foreground: u32,
    height: i32,
    dpi: u32,
}

// Like placement, styling is requested by every drawn frame. RichEdit treats
// EM_SETCHARFORMAT as a document change even when its values are unchanged.
// Keep the cache outside NativeEdit for the borrowed host wrapper described above.
static STYLES: Mutex<Option<HashMap<isize, EditStyle>>> = Mutex::new(None);

fn forget_style(hwnd: isize) {
    if let Ok(mut cache) = STYLES.lock() {
        if let Some(styles) = cache.as_mut() {
            styles.remove(&hwnd);
        }
    }
}

pub struct NativeEdit {
    pub hwnd: isize,
}
impl NativeEdit {
    pub fn new(parent: isize, multiline: bool, password: bool) -> windows::core::Result<Self> {
        unsafe {
            crate::input_host::load_library();
            let mut style = WS_CHILD | WS_TABSTOP;
            if multiline {
                // ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN. Horizontal
                // auto-scroll disables wrapping in RichEdit; only single-line
                // fields should carry ES_AUTOHSCROLL.
                style |= WS_VSCROLL | WINDOW_STYLE(0x4 | 0x40 | 0x1000);
            } else {
                style |= WINDOW_STYLE(0x80); // ES_AUTOHSCROLL
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
            if multiline {
                // EM_SETTARGETDEVICE: null device and zero width wrap to the
                // edit control's current client area, including after resize.
                SendMessageW(h, 0x448, WPARAM(0), LPARAM(0));
            }
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
        // Replacing the document may reset its character format. The next
        // style request must apply to this content, even with the same theme.
        forget_style(self.hwnd);
        unsafe {
            let mut b: Vec<u16> = text.encode_utf16().collect();
            b.push(0);
            let _ = SetWindowTextW(HWND(self.hwnd as *mut _), PCWSTR(b.as_ptr()));
        }
    }
    pub fn place(&self, x: f32, y: f32, w: f32, h: f32) {
        unsafe {
            let s = crate::window::dpi_scale();
            // 去抖：主循环每帧 place，物理像素 rect 相同则跳过 SetWindowPos
            // 与 RedrawWindow（缓存键 = hwnd，见 PLACE_RECTS 注释）
            let rect = (
                (x * s) as i32,
                (y * s) as i32,
                (w * s).max(2.) as i32,
                (h * s).max(2.) as i32,
            );
            let skip = PLACE_RECTS
                .lock()
                .map(|mut g| {
                    let map = g.get_or_insert_with(HashMap::new);
                    if map.get(&self.hwnd) == Some(&rect) {
                        true
                    } else {
                        map.insert(self.hwnd, rect);
                        false
                    }
                })
                .unwrap_or(false);
            if skip {
                return;
            }
            let _ = SetWindowPos(
                HWND(self.hwnd as *mut _),
                None,
                rect.0,
                rect.1,
                rect.2,
                rect.3,
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
        // place() also shows the control. Its next same-size call must not be
        // skipped after this explicit hide.
        if let Ok(mut cache) = PLACE_RECTS.lock() {
            if let Some(rects) = cache.as_mut() {
                rects.remove(&self.hwnd);
            }
        }
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
            let style = EditStyle {
                background: rgb(bg),
                foreground: rgb(fg),
                height: (size * 15.) as i32,
                dpi: crate::window::dpi_scale().to_bits(),
            };
            let unchanged = STYLES
                .lock()
                .map(|mut cache| {
                    let styles = cache.get_or_insert_with(HashMap::new);
                    if styles.get(&self.hwnd) == Some(&style) {
                        true
                    } else {
                        styles.insert(self.hwnd, style);
                        false
                    }
                })
                .unwrap_or(false);
            if unchanged {
                return;
            }
            // Release the cache lock before synchronous RichEdit messages:
            // they can notify the parent and re-enter its window procedure.
            let h = HWND(self.hwnd as *mut _);
            SendMessageW(h, 1091, WPARAM(0), LPARAM(style.background as isize));
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
                height: style.height,
                offset: 0,
                color: style.foreground,
                charset: 1,
                pitch: 0,
                face: [0; 32],
            };
            for (dst, ch) in f.face.iter_mut().zip("Microsoft YaHei UI".encode_utf16()) {
                *dst = ch;
            }
            let all = SendMessageW(h, 1092, WPARAM(4), LPARAM(&mut f as *mut _ as isize));
            let default = SendMessageW(h, 1092, WPARAM(0), LPARAM(&mut f as *mut _ as isize));
            if all.0 == 0 || default.0 == 0 {
                forget_style(self.hwnd);
            }
        }
    }
}
impl Drop for NativeEdit {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(HWND(self.hwnd as *mut _));
        }
        // 清去抖缓存：hwnd 会被系统复用，残留条目可能让复用 hwnd 的新窗口
        // 首次 place 被误跳过（SWP_SHOWWINDOW 不下发 → 编辑框不可见）。
        // （main.rs apply_style 的样式包装实例走 mem::forget 且从不 place，
        // 不会写缓存，故不受影响。）
        if let Ok(mut g) = PLACE_RECTS.lock() {
            if let Some(map) = g.as_mut() {
                map.remove(&self.hwnd);
            }
        }
        forget_style(self.hwnd);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        static EDIT_PROC: Cell<WNDPROC> = const { Cell::new(None) };
        static PARENT_PROC: Cell<WNDPROC> = const { Cell::new(None) };
        static STYLE_MESSAGES: Cell<usize> = const { Cell::new(0) };
        static UPDATE_NOTIFICATIONS: Cell<usize> = const { Cell::new(0) };
    }

    unsafe extern "system" fn edit_proc(
        hwnd: HWND,
        msg: u32,
        wp: WPARAM,
        lp: LPARAM,
    ) -> windows::Win32::Foundation::LRESULT {
        if matches!(msg, 1091 | 1092) {
            STYLE_MESSAGES.with(|count| count.set(count.get() + 1));
        }
        EDIT_PROC.with(|original| CallWindowProcW(original.get(), hwnd, msg, wp, lp))
    }

    unsafe extern "system" fn parent_proc(
        hwnd: HWND,
        msg: u32,
        wp: WPARAM,
        lp: LPARAM,
    ) -> windows::Win32::Foundation::LRESULT {
        if msg == WM_COMMAND && (wp.0 >> 16) as u32 == EN_UPDATE {
            UPDATE_NOTIFICATIONS.with(|count| count.set(count.get() + 1));
        }
        PARENT_PROC.with(|original| CallWindowProcW(original.get(), hwnd, msg, wp, lp))
    }

    struct TestParent(HWND);
    impl Drop for TestParent {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    #[test]
    #[ignore = "requires a Windows desktop; creates only a hidden private test window"]
    fn unchanged_frames_do_not_restyle_editors_and_hidden_fields_reappear() {
        unsafe {
            let parent = TestParent(
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("Editor style test"),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    600,
                    400,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            );
            PARENT_PROC.with(|original| {
                original.set(std::mem::transmute::<isize, WNDPROC>(SetWindowLongPtrW(
                    parent.0,
                    GWLP_WNDPROC,
                    parent_proc as *const () as isize,
                )))
            });
            let edit = NativeEdit::new(parent.0 .0 as isize, true, false).unwrap();
            let hwnd = HWND(edit.hwnd as *mut _);
            EDIT_PROC.with(|original| {
                original.set(std::mem::transmute::<isize, WNDPROC>(SetWindowLongPtrW(
                    hwnd,
                    GWLP_WNDPROC,
                    edit_proc as *const () as isize,
                )))
            });
            // Request native update notifications as the real parent does.
            SendMessageW(hwnd, 0x445, WPARAM(0), LPARAM(0x0004_0000)); // EM_SETEVENTMASK / ENM_UPDATE
            STYLE_MESSAGES.with(|count| count.set(0));
            UPDATE_NOTIFICATIONS.with(|count| count.set(0));
            let background = [0.1, 0.12, 0.15, 1.];
            let foreground = [0.9, 0.91, 0.92, 1.];
            edit.set_text("第一行\r\n第二行");
            edit.style(background, foreground, 16.);
            assert_eq!(STYLE_MESSAGES.with(Cell::get), 3);
            let notifications = UPDATE_NOTIFICATIONS.with(Cell::get);
            // An idle panel may redraw many times. None may mutate RichEdit
            // formatting or feed another EN_UPDATE back into the frame loop.
            for _ in 0..120 {
                edit.style(background, foreground, 16.);
            }
            assert_eq!(STYLE_MESSAGES.with(Cell::get), 3);
            assert_eq!(UPDATE_NOTIFICATIONS.with(Cell::get), notifications);

            edit.style(background, [1., 1., 1., 1.], 16.);
            edit.style(background, [1., 1., 1., 1.], 18.);
            assert_eq!(
                STYLE_MESSAGES.with(Cell::get),
                9,
                "color and size edits still apply immediately"
            );
            edit.set_text("替换后的正文");
            edit.style(background, [1., 1., 1., 1.], 18.);
            assert_eq!(
                STYLE_MESSAGES.with(Cell::get),
                12,
                "replacing text invalidates its style"
            );
            assert_eq!(edit.text(), "替换后的正文");

            edit.place(20., 30., 300., 120.);
            assert_ne!(GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_VISIBLE.0, 0);
            edit.hide();
            assert_eq!(GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_VISIBLE.0, 0);
            edit.place(20., 30., 300., 120.);
            assert_ne!(
                GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_VISIBLE.0,
                0,
                "the same cached rectangle must show an explicitly hidden editor"
            );

            let handle = edit.hwnd;
            drop(edit);
            assert!(!PLACE_RECTS
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .contains_key(&handle));
            assert!(!STYLES
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .contains_key(&handle));
        }
    }
}
