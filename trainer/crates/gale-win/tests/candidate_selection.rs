//! Numeric, synchronous TIP outcome notifications are observed before document
//! diffs. This test never activates a window or sends global keyboard input.
use gale_win::input_host::{
    drain_commit_causes, set_selection_keys, HostConfig, HostEvent, InputHost,
};
use gale_win::window::ComSta;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetPropW, RegisterWindowMessageW, SendMessageW,
    WINDOW_EX_STYLE, WM_CHAR, WS_OVERLAPPEDWINDOW,
};

struct Parent(HWND);
impl Drop for Parent {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

#[test]
#[ignore = "requires Windows RichEdit and a desktop session; run explicitly"]
fn confirmed_selections_arrive_before_delayed_text_and_do_not_count_twice() {
    let _com = ComSta::new().expect("test-thread TSF pump");
    let parent = Parent(unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("selection outcome regression"),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            300,
            100,
            None,
            None,
            GetModuleHandleW(None).unwrap(),
            None,
        )
        .expect("hidden parent")
    });
    let mut host = InputHost::create(&HostConfig {
        parent: parent.0 .0 as isize,
        x: 0,
        y: 0,
        w: 200,
        h: 30,
    })
    .expect("real RichEdit host");
    host.reset_text();
    let hwnd = HWND(host.hwnd as _);
    let message = unsafe { RegisterWindowMessageW(w!("HuFu.Trainer.InputOutcome.v1")) };
    assert_ne!(message, 0);
    assert_eq!(
        unsafe { GetPropW(hwnd, w!("HuFu.Trainer.InputOutcome.v1")) }.0 as usize,
        1
    );
    set_selection_keys(&[0xBA, 0xDE]);

    // A sentence candidate can be locked before any characters are committed.
    unsafe {
        SendMessageW(hwnd, message, WPARAM(3), LPARAM(0xDE));
    }
    let locked = host.poll();
    assert!(matches!(
        locked.as_slice(),
        [HostEvent::CandidateSelection {
            rank: 3,
            vk: Some(0xDE)
        }]
    ));
    assert!(host.committed_text().is_empty());

    // A second successful lock and its immediate text must retain this order.
    unsafe {
        SendMessageW(hwnd, message, WPARAM(2), LPARAM(0xBA));
        SendMessageW(hwnd, WM_CHAR, WPARAM('甲' as usize), LPARAM(1));
    }
    let events = host.poll();
    let selected = events
        .iter()
        .position(|e| matches!(e, HostEvent::CandidateSelection { rank: 2, .. }))
        .unwrap();
    let committed = events
        .iter()
        .position(|e| matches!(e, HostEvent::Commit(s) if s == "甲"))
        .unwrap();
    assert!(
        selected < committed,
        "the final action must reach statistics before finalization"
    );
    assert!(!drain_commit_causes().iter().any(|cause| matches!(
        cause,
        gale_win::input_host::CommitCause::Select
            | gale_win::input_host::CommitCause::SelectExtra { .. }
    )));

    // null metadata (rank zero), invalid messages and document polling do not
    // invent a selection or replay the previous selection.
    unsafe {
        SendMessageW(hwnd, message, WPARAM(0), LPARAM(0));
    }
    assert!(host.poll().is_empty());
    unsafe {
        SendMessageW(hwnd, message, WPARAM(11), LPARAM(0x34));
    }
    assert!(host.poll().is_empty());
    host.reset_text();
    assert!(host.poll().is_empty());
}
