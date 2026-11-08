//! 按键注入（keybd_event）——仅供「发到群」增强链使用：把已校验的成绩/
//! 发文内容粘贴进 QQ 输入框并回车。跟打器其余路径不注入按键。
//!
//! 约束：调用方必须先完成「剪贴板写入 + 窗口激活 + 会话标题校验」，失败
//! 场景一律中止并保留剪贴板（可行性报告 · 通道C 增强模式③）。

use std::thread;
use std::time::Duration;

use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYBD_EVENT_FLAGS};

const VK_CONTROL: u8 = 0x11;
const VK_V: u8 = 0x56;
const VK_RETURN: u8 = 0x0D;

const KEYEVENTF_KEYUP: u32 = 0x0002;

/// 等待窗口切换/输入法状态稳定后再注入。
fn settle() {
    thread::sleep(Duration::from_millis(60));
}

/// 在已激活窗口里执行 Ctrl+V（粘贴）。
pub fn ctrl_v() {
    settle();
    let down = KEYBD_EVENT_FLAGS(0);
    let up = KEYBD_EVENT_FLAGS(KEYEVENTF_KEYUP);
    unsafe {
        keybd_event(VK_CONTROL, 0, down, 0);
        keybd_event(VK_V, 0, down, 0);
        keybd_event(VK_V, 0, up, 0);
        keybd_event(VK_CONTROL, 0, up, 0);
    }
}

/// 在已激活窗口里按一次回车（发送）。
pub fn enter() {
    settle();
    unsafe {
        keybd_event(VK_RETURN, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(VK_RETURN, 0, KEYBD_EVENT_FLAGS(KEYEVENTF_KEYUP), 0);
    }
}

/// 粘贴并回车（QQ 输入框：Ctrl+V 上屏 → Enter 发送）。
pub fn paste_and_enter() {
    ctrl_v();
    thread::sleep(Duration::from_millis(140));
    enter();
}

#[cfg(test)]
mod tests {
    #[test]
    fn vk_codes_stable() {
        // 常量防漂：VK 值由 Win32 固定，改错会静默按错键。
        assert_eq!(super::VK_CONTROL, 0x11);
        assert_eq!(super::VK_V, 0x56);
        assert_eq!(super::VK_RETURN, 0x0D);
        assert_eq!(super::KEYEVENTF_KEYUP, 0x0002);
    }
}
