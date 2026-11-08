//! 系统颜色选择器（comdlg32 `ChooseColorW`）——设置面板「自定义颜色」的
//! 拾色入口。全屏色板是 Windows 自带的标准对话框（用户熟悉、零依赖实现）。
//!
//! 约定：
//! - 必须在 UI 线程调用（own = 主窗口 hwnd；对话框模态挂靠它）；
//! - `lpCustColors` 需要跨调用存活的静态 16×COLORREF 缓冲（Windows 直接
//!   持有该指针直到对话框关闭——栈上缓冲 = UAF，静态是文档要求）；
//! - COLORREF 字节序 = 0x00BBGGRR（低字节红）——与主题的 (r,g,b) 相反；
//! - 用户取消返回 None，调用方不动任何状态。

use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::UI::Controls::Dialogs::{ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW};

/// 自定义颜色槽（对话框左下角 16 格「自定义颜色」；跨调用持久）
static mut CUST_COLORS: [COLORREF; 16] = [COLORREF(0); 16];

/// 弹出系统取色器。
/// - `owner`：主窗口 hwnd（模态挂靠；对话框期间主窗口不收输入）
/// - `current`：(r, g, b) 预填当前色（CC_RGBINIT 打开时高亮定位该色）
///
/// 返回 `Some((r, g, b))` = 用户点确定；`None` = 用户取消。
/// 内部不碰任何 gale 状态——纯 Win32 往返。
pub fn choose_color(owner: isize, current: (u8, u8, u8)) -> Option<(u8, u8, u8)> {
    unsafe {
        // COLORREF = 0x00BBGGRR（低字节红）
        let initial =
            COLORREF(current.0 as u32 | ((current.1 as u32) << 8) | ((current.2 as u32) << 16));
        let mut cc = CHOOSECOLORW {
            lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: HWND(owner as *mut _),
            hInstance: HWND(std::ptr::null_mut()),
            rgbResult: initial,
            lpCustColors: std::ptr::addr_of_mut!(CUST_COLORS) as *mut COLORREF,
            Flags: CC_FULLOPEN | CC_RGBINIT,
            lCustData: windows::Win32::Foundation::LPARAM(0),
            lpfnHook: None,
            lpTemplateName: windows::core::PCWSTR::null(),
        };
        if ChooseColorW(&mut cc).as_bool() {
            let c = cc.rgbResult.0;
            Some((
                (c & 0xFF) as u8,
                ((c >> 8) & 0xFF) as u8,
                ((c >> 16) & 0xFF) as u8,
            ))
        } else {
            // 用户取消（或对话框打开失败——对用户语义等价：什么都不改）
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_byte_order() {
        // COLORREF = 0x00BBGGRR（低字节红）——纯逻辑往返，不弹真对话框
        //（旧版直接调 choose_color 会在 cargo test 时弹出系统取色器卡住套件）。
        let initial = COLORREF(0x12u32 | (0x34 << 8) | (0x56 << 16));
        let c = initial.0;
        let back = (
            (c & 0xFF) as u8,
            ((c >> 8) & 0xFF) as u8,
            ((c >> 16) & 0xFF) as u8,
        );
        assert_eq!(back, (0x12, 0x34, 0x56));
    }
}
