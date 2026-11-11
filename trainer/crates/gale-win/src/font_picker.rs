//! 外观页内的系统字体下拉。字体与字号独立选择，不打开 ChooseFont 对话框。
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, DeleteObject, CLEARTYPE_QUALITY, DEFAULT_CHARSET, FW_NORMAL, HFONT, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const CONTROL_ID: usize = 0x6f01;

pub fn selection_command(wp: WPARAM, lp: LPARAM) -> bool {
    wp.0 & 0xffff == CONTROL_ID && (wp.0 >> 16) & 0xffff == CBN_SELCHANGE as usize && lp.0 != 0
}

pub struct NativeFontPicker {
    hwnd: HWND,
    families: Vec<String>,
    selected: usize,
    rect: Option<(i32, i32, i32, i32)>,
    font: HFONT,
    scale_bits: u32,
}

impl NativeFontPicker {
    pub fn new(parent: isize, current: &str) -> Result<Self, String> {
        let families = crate::d2d::installed_font_families()?;
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0), w!("COMBOBOX"), w!("正文字体"),
                WS_CHILD | WS_TABSTOP | WS_VSCROLL
                    | WINDOW_STYLE((CBS_DROPDOWNLIST | CBS_HASSTRINGS | CBS_NOINTEGRALHEIGHT) as u32),
                0, 0, 100, 300, HWND(parent as *mut _),
                HMENU(CONTROL_ID as *mut _), None, None,
            ).map_err(|e| format!("字体下拉不可用：{e}"))?
        };
        let mut picker = Self {
            hwnd, families: Vec::new(), selected: usize::MAX,
            rect: None, font: HFONT::default(), scale_bits: 0,
        };
        picker.add_entry("", &format!("默认 · {}", crate::d2d::detected_cjk_family()))?;
        for family in families {
            picker.add_entry(&family, &family)?;
        }
        picker.set_current(current)?;
        Ok(picker)
    }

    fn add_entry(&mut self, family: &str, label: &str) -> Result<(), String> {
        let label: Vec<u16> = label.encode_utf16().chain([0]).collect();
        let added = unsafe {
            SendMessageW(self.hwnd, CB_ADDSTRING, WPARAM(0), LPARAM(label.as_ptr() as isize))
        }.0;
        if added < 0 {
            return Err("无法填充系统字体列表".into());
        }
        self.families.push(family.into());
        Ok(())
    }

    /// 配置同步不产生用户选择事件；保留已卸载字体的名称及回退说明。
    pub fn set_current(&mut self, current: &str) -> Result<(), String> {
        let index = if let Some(index) = self.families.iter().position(|name| name == current) {
            index
        } else {
            let resolved = crate::d2d::resolved_cjk_family(current);
            let label = if resolved == current {
                current.into()
            } else {
                format!("{current}（已回退 {resolved}）")
            };
            self.add_entry(current, &label)?;
            self.families.len() - 1
        };
        if self.selected != index {
            unsafe { SendMessageW(self.hwnd, CB_SETCURSEL, WPARAM(index), LPARAM(0)); }
            self.selected = index;
        }
        Ok(())
    }

    pub fn take_selection(&mut self) -> Option<String> {
        let index = unsafe { SendMessageW(self.hwnd, CB_GETCURSEL, WPARAM(0), LPARAM(0)) }.0;
        if index < 0 || index as usize == self.selected {
            return None;
        }
        let family = self.families.get(index as usize)?.clone();
        self.selected = index as usize;
        Some(family)
    }

    pub fn place(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let scale = crate::window::dpi_scale();
        unsafe {
            if self.scale_bits != scale.to_bits() {
                let font = CreateFontW(
                    -(15. * scale).round() as i32, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0,
                    DEFAULT_CHARSET.0 as u32, 0, 0, CLEARTYPE_QUALITY.0 as u32, 0,
                    w!("Microsoft YaHei UI"),
                );
                if !font.0.is_null() {
                    SendMessageW(self.hwnd, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
                    let _ = DeleteObject(HGDIOBJ(self.font.0));
                    self.font = font;
                }
                SendMessageW(self.hwnd, CB_SETITEMHEIGHT, WPARAM(usize::MAX),
                    LPARAM(((h - 6.) * scale).round().max(16.) as isize));
                SendMessageW(self.hwnd, CB_SETITEMHEIGHT, WPARAM(0),
                    LPARAM((24. * scale).round() as isize));
                self.scale_bits = scale.to_bits();
                self.rect = None;
            }
            // CBS_DROPDOWNLIST 的窗口高度还包含展开列表，折叠框高度由 SETITEMHEIGHT 管理。
            let rect = ((x * scale).round() as i32, (y * scale).round() as i32,
                (w * scale).round().max(2.) as i32, ((h + 240.) * scale).round() as i32);
            if self.rect != Some(rect) {
                let _ = SetWindowPos(self.hwnd, None, rect.0, rect.1, rect.2, rect.3,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                SendMessageW(self.hwnd, CB_SETDROPPEDWIDTH,
                    WPARAM((w.max(380.) * scale).round() as usize), LPARAM(0));
                self.rect = Some(rect);
            }
        }
    }

    pub fn hide(&mut self) {
        if self.rect.take().is_some() {
            unsafe {
                SendMessageW(self.hwnd, CB_SHOWDROPDOWN, WPARAM(0), LPARAM(0));
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }
}

impl Drop for NativeFontPicker {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteObject(HGDIOBJ(self.font.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_font_choices_are_unique_and_renderable() {
        let names = crate::d2d::installed_font_families().unwrap();
        assert!(!names.is_empty());
        assert!(names.windows(2).all(|pair| pair[0].to_lowercase() < pair[1].to_lowercase()));
        for name in names {
            assert_eq!(crate::d2d::resolved_cjk_family(&name), name);
        }
    }

    #[test]
    #[ignore = "requires a Windows desktop; creates only a hidden private test window"]
    fn native_selection_changes_only_the_font_family() {
        struct TestParent(HWND);
        impl Drop for TestParent {
            fn drop(&mut self) {
                unsafe { let _ = DestroyWindow(self.0); }
            }
        }
        unsafe {
            let parent = TestParent(CreateWindowExW(
                WINDOW_EX_STYLE(0), w!("STATIC"), w!("Font picker test"),
                WS_OVERLAPPEDWINDOW, 0, 0, 600, 400, None, None, None, None,
            ).unwrap());
            let mut picker = NativeFontPicker::new(parent.0.0 as isize, "").unwrap();
            assert_eq!(picker.take_selection(), None);
            assert!(picker.families.len() > 1);
            SendMessageW(picker.hwnd, CB_SETCURSEL, WPARAM(1), LPARAM(0));
            assert_eq!(picker.take_selection(), Some(picker.families[1].clone()));
            assert_eq!(picker.take_selection(), None);

            // Programmatic reset and a missing saved font never masquerade as user edits.
            picker.set_current("").unwrap();
            assert_eq!(picker.take_selection(), None);
            picker.set_current("__hufu_missing_font__").unwrap();
            assert_eq!(picker.take_selection(), None);
            assert_eq!(picker.families[picker.selected], "__hufu_missing_font__");

            picker.place(20., 30., 180., 32.);
            let mut rect = windows::Win32::Foundation::RECT::default();
            GetWindowRect(picker.hwnd, &mut rect).unwrap();
            let scale = crate::window::dpi_scale();
            assert!((20. * scale..=40. * scale).contains(&((rect.bottom - rect.top) as f32)));
            picker.hide();
            assert_eq!(GetWindowLongW(picker.hwnd, GWL_STYLE) as u32 & WS_VISIBLE.0, 0);
        }
    }
}
