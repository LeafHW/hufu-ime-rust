//! Native shortcut regression. All messages and keyboard state belong to this
//! test thread; no foreground window, SendInput, or user's input is involved.

use gale_win::input_host::{drain_cmds, CommandKey, HostConfig, HostEvent, InputHost};
use gale_win::window::{pump_messages_once, ComSta};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::RichEdit::EM_STOPGROUPTYPING;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardState, SetKeyboardState};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, PeekMessageW, PostMessageW, SendMessageW, MSG, PM_NOREMOVE,
    WINDOW_EX_STYLE, WM_CHAR, WM_KEYDOWN, WM_KEYUP, WS_OVERLAPPEDWINDOW,
};

struct NativeFixture {
    parent: HWND,
    saved_keyboard: [u8; 256],
}

impl NativeFixture {
    unsafe fn create() -> Self {
        let mut saved_keyboard = [0; 256];
        GetKeyboardState(&mut saved_keyboard).expect("read test-thread keyboard state");
        // STATIC is a system class. This parent stays hidden and does not post
        // WM_QUIT or use the application's main-window singleton on destruction.
        let parent = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("trainer undo shortcut regression"),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            320,
            100,
            None,
            None,
            GetModuleHandleW(None).expect("test module"),
            None,
        )
        .expect("create hidden test parent");
        let fixture = Self {
            parent,
            saved_keyboard,
        };
        SetKeyboardState(&[0; 256]).expect("clear only this thread's keyboard state");
        fixture
    }
}

impl Drop for NativeFixture {
    fn drop(&mut self) {
        unsafe {
            let _ = SetKeyboardState(&self.saved_keyboard);
            let _ = DestroyWindow(self.parent);
        }
    }
}

fn pump_pending() {
    for _ in 0..512 {
        let mut message = MSG::default();
        if !unsafe { PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE).as_bool() } {
            return;
        }
        assert!(
            pump_messages_once(),
            "unexpected WM_QUIT in private test queue"
        );
    }
    panic!("private native test queue did not drain");
}

fn post_ctrl_shortcut(hwnd: HWND, vk: u32, scan_code: u32) {
    unsafe {
        let mut keyboard = [0; 256];
        keyboard[0x11] = 0x80; // VK_CONTROL
        keyboard[0xa2] = 0x80; // VK_LCONTROL
        keyboard[vk as usize] = 0x80;
        SetKeyboardState(&keyboard).expect("set private Ctrl chord");
        PostMessageW(
            hwnd,
            WM_KEYDOWN,
            WPARAM(vk as usize),
            LPARAM((1 | (scan_code << 16)) as isize),
        )
        .expect("post shortcut keydown");
        PostMessageW(
            hwnd,
            WM_KEYUP,
            WPARAM(vk as usize),
            LPARAM((1u32 | (scan_code << 16) | (1 << 30) | (1 << 31)) as isize),
        )
        .expect("post shortcut keyup");
    }
    // Uses the product pretranslation -> TSF -> TranslateMessage -> RichEdit
    // pipeline, rather than sending WM_UNDO directly around the broken route.
    pump_pending();
    unsafe {
        SetKeyboardState(&[0; 256]).expect("release private Ctrl chord");
    }
}

#[test]
#[ignore = "requires Windows RichEdit and a desktop session; run explicitly"]
fn physical_ctrl_z_and_ctrl_y_reach_native_undo_and_document_diff() {
    let _com = ComSta::new().expect("initialize private test-thread TSF pump");
    let fixture = unsafe { NativeFixture::create() };
    let mut host = InputHost::create(&HostConfig {
        parent: fixture.parent.0 as isize,
        x: 0,
        y: 0,
        w: 200,
        h: 30,
    })
    .expect("create real RichEdit input host");
    pump_pending();
    host.reset_text();
    let hwnd = HWND(host.hwnd as *mut core::ffi::c_void);
    unsafe {
        SendMessageW(hwnd, WM_CHAR, WPARAM('甲' as usize), LPARAM(1));
        SendMessageW(hwnd, EM_STOPGROUPTYPING, WPARAM(0), LPARAM(0));
    }
    assert!(host
        .poll()
        .iter()
        .any(|event| matches!(event, HostEvent::Commit(s) if s == "甲")));
    unsafe {
        SendMessageW(hwnd, WM_CHAR, WPARAM('乙' as usize), LPARAM(1));
        SendMessageW(hwnd, EM_STOPGROUPTYPING, WPARAM(0), LPARAM(0));
    }
    assert!(host
        .poll()
        .iter()
        .any(|event| matches!(event, HostEvent::Commit(s) if s == "乙")));
    assert_eq!(host.committed_text(), "甲乙");
    assert!(drain_cmds().is_empty());

    post_ctrl_shortcut(hwnd, 0x5a, 0x2c); // Ctrl+Z
    assert_eq!(
        host.committed_text(),
        "甲",
        "Ctrl+Z must undo the last native input group"
    );
    let undone = host.poll();
    assert_eq!(
        undone
            .iter()
            .filter(|event| matches!(event, HostEvent::Backspace))
            .count(),
        1
    );
    assert!(!undone
        .iter()
        .any(|event| matches!(event, HostEvent::Commit(_) | HostEvent::Key { .. })));
    assert!(
        drain_cmds().is_empty(),
        "undo is not an application command"
    );

    post_ctrl_shortcut(hwnd, 0x59, 0x15); // Ctrl+Y
    assert_eq!(
        host.committed_text(),
        "甲乙",
        "Ctrl+Y must redo the native input group"
    );
    let redone = host.poll();
    assert!(redone
        .iter()
        .any(|event| matches!(event, HostEvent::Commit(s) if s == "乙")));
    assert!(!redone
        .iter()
        .any(|event| matches!(event, HostEvent::Backspace | HostEvent::Key { .. })));
    assert!(
        drain_cmds().is_empty(),
        "redo is not an application command"
    );

    post_ctrl_shortcut(hwnd, 0x56, 0x2f); // Ctrl+V loads an article, never pastes into training text.
    assert_eq!(host.committed_text(), "甲乙");
    assert_eq!(drain_cmds(), vec![CommandKey::LoadClipboard]);

    post_ctrl_shortcut(hwnd, 0x41, 0x1e); // Ctrl+A remains blocked in the training host.
    unsafe {
        SendMessageW(hwnd, WM_CHAR, WPARAM('丙' as usize), LPARAM(1));
    }
    assert_eq!(
        host.committed_text(),
        "甲乙丙",
        "Ctrl+A must not select and replace training text"
    );
    assert!(drain_cmds().is_empty());
}
