//! Long paragraph input must fit the same 100,000 Unicode scalar limit as the
//! article loader. This uses a hidden RichEdit and no foreground keyboard input.

use gale_win::input_host::{HostConfig, HostEvent, InputHost};
use gale_win::window::ComSta;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, SendMessageW, WINDOW_EX_STYLE, WM_CHAR, WS_OVERLAPPEDWINDOW,
};

struct TestParent(HWND);
impl Drop for TestParent {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

#[test]
#[ignore = "requires Windows RichEdit and a desktop session; run explicitly"]
fn long_bmp_and_supplementary_paragraphs_accept_their_final_character() {
    let _com = ComSta::new().unwrap();
    let parent = TestParent(unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("Long paragraph input regression"),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            320,
            100,
            None,
            None,
            None,
            None,
        )
        .unwrap()
    });
    let mut host = InputHost::create(&HostConfig {
        parent: parent.0 .0 as isize,
        x: 0,
        y: 0,
        w: 200,
        h: 30,
    })
    .unwrap();
    let hwnd = HWND(host.hwnd as *mut _);
    for character in ['甲', '\u{20000}'] {
        // WM_SETTEXT may bypass RichEdit's input capacity. The final WM_CHAR
        // follows the actual native insertion path and catches that mismatch.
        host.set_text(&character.to_string().repeat(99_999));
        let mut encoded = [0u16; 2];
        for unit in character.encode_utf16(&mut encoded).iter().copied() {
            unsafe {
                SendMessageW(hwnd, WM_CHAR, WPARAM(unit as usize), LPARAM(1));
            }
        }
        let text = host.committed_text();
        assert_eq!(
            text.chars().count(),
            100_000,
            "native input must accept the final scalar for {character:?}"
        );
        assert_eq!(text.chars().last(), Some(character));
        let events = host.poll();
        assert!(
            events.iter().any(|event| matches!(event,
            HostEvent::Commit(value) if value == &character.to_string())),
            "the final input must also reach the comparer event stream"
        );
        assert!(!events
            .iter()
            .any(|event| matches!(event, HostEvent::Backspace)));
    }
}
