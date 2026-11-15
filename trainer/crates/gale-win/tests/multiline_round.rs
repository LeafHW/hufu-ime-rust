//! Exercise Enter through the real pretranslation/TSF/RichEdit path. Windows and
//! keyboard state stay private to this test thread; no foreground input is used.

use gale_core::compare::{Comparer, InputEvent};
use gale_core::segment::parse_group_text;
use gale_win::input_host::{
    drain_cmds, set_round_done, set_round_typing, CommandKey, HostConfig, HostEvent, InputHost,
};
use gale_win::window::{pump_messages_once, ComSta};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
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
        GetKeyboardState(&mut saved_keyboard).unwrap();
        let parent = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("Multiline trainer regression"),
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
        .unwrap();
        SetKeyboardState(&[0; 256]).unwrap();
        Self {
            parent,
            saved_keyboard,
        }
    }
}

impl Drop for NativeFixture {
    fn drop(&mut self) {
        set_round_done(false);
        set_round_typing(false, false, false);
        unsafe {
            let _ = SetKeyboardState(&self.saved_keyboard);
            let _ = DestroyWindow(self.parent);
        }
    }
}

fn sync_gate(comparer: &Comparer) {
    set_round_done(comparer.finished_with_correct_last_char());
    set_round_typing(
        !comparer.finished_with_correct_last_char(),
        comparer.next_target_char() == Some('\n'),
        comparer.text.contains(' '),
    );
}

fn feed_pending(host: &mut InputHost, comparer: &mut Comparer) -> Vec<HostEvent> {
    let mut observed = Vec::new();
    for _ in 0..512 {
        let mut message = MSG::default();
        if !unsafe { PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE).as_bool() } {
            break;
        }
        assert!(pump_messages_once(), "unexpected private WM_QUIT");
        for event in host.poll() {
            if let Some(input) = Option::<InputEvent>::from(&event) {
                comparer.feed(&input, 100);
            }
            observed.push(event);
        }
        // The application also polls input and updates gates after every pump
        // step, before processing the next queued message.
        sync_gate(comparer);
    }
    observed
}

fn post_key(hwnd: HWND, vk: u32, scan: u32) {
    unsafe {
        PostMessageW(
            hwnd,
            WM_KEYDOWN,
            WPARAM(vk as usize),
            LPARAM((1 | scan << 16) as isize),
        )
        .unwrap();
        PostMessageW(
            hwnd,
            WM_KEYUP,
            WPARAM(vk as usize),
            LPARAM((1u32 | scan << 16 | 1 << 30 | 1 << 31) as isize),
        )
        .unwrap();
    }
}

fn type_char(host: &mut InputHost, comparer: &mut Comparer, character: char) {
    unsafe {
        SendMessageW(
            HWND(host.hwnd as *mut _),
            WM_CHAR,
            WPARAM(character as usize),
            LPARAM(1),
        );
    }
    for event in host.poll() {
        if let Some(input) = Option::<InputEvent>::from(&event) {
            comparer.feed(&input, 100);
        }
    }
    sync_gate(comparer);
}

#[test]
#[ignore = "requires Windows RichEdit and a desktop session; run explicitly"]
fn group_line_break_is_typable_without_leaking_hint_or_continuation_keys() {
    let _com = ComSta::new().unwrap();
    let fixture = unsafe { NativeFixture::create() };
    let mut host = InputHost::create(&HostConfig {
        parent: fixture.parent.0 as isize,
        x: 0,
        y: 0,
        w: 200,
        h: 30,
    })
    .unwrap();
    let parsed = parse_group_text("《发文》 多行群文\n-----第1段-----\n甲\n乙");
    assert_eq!(parsed.segments.len(), 1);
    assert_eq!(parsed.segments[0].text, "甲\n乙");
    let mut comparer = Comparer::new(&parsed.segments[0].text);
    feed_pending(&mut host, &mut comparer);
    host.reset_text();
    sync_gate(&comparer);
    let hwnd = HWND(host.hwnd as *mut _);
    let _ = drain_cmds();

    post_key(hwnd, 0x0d, 0x1c);
    let hint = feed_pending(&mut host, &mut comparer);
    assert_eq!(drain_cmds(), vec![CommandKey::Enter]);
    assert_eq!(
        host.committed_text(),
        "",
        "hint Enter must not leak WM_CHAR"
    );
    assert_eq!(comparer.pos, 0);
    assert!(
        hint.iter()
            .any(|event| matches!(event, HostEvent::Key { vk: 0x0d })),
        "a typing-state hint retains its physical-key statistics"
    );
    post_key(hwnd, 0x20, 0x39);
    feed_pending(&mut host, &mut comparer);
    assert_eq!(drain_cmds(), vec![CommandKey::Space]);
    assert_eq!(
        host.committed_text(),
        "",
        "hint Space must not become training text"
    );

    type_char(&mut host, &mut comparer, '甲');
    post_key(hwnd, 0x0d, 0x1c);
    let newline = feed_pending(&mut host, &mut comparer);
    assert!(
        drain_cmds().is_empty(),
        "required LF is input, not a round command"
    );
    assert!(newline
        .iter()
        .any(|event| matches!(event, HostEvent::Commit(value) if value == "\n")));
    assert!(
        newline
            .iter()
            .any(|event| matches!(event, HostEvent::Key { vk: 0x0d })),
        "a required newline remains a real typing key"
    );
    assert_eq!(host.committed_text(), "甲\n");
    assert_eq!(comparer.pos, 2);
    type_char(&mut host, &mut comparer, '乙');
    assert!(
        comparer.finished_with_correct_last_char(),
        "the actual multiline body must finish"
    );
    assert_eq!(comparer.wrong, 0);

    post_key(hwnd, 0x0d, 0x1c);
    let continuation = feed_pending(&mut host, &mut comparer);
    assert_eq!(drain_cmds(), vec![CommandKey::Enter]);
    assert_eq!(host.committed_text(), "甲\n乙");
    assert!(
        !continuation
            .iter()
            .any(|event| matches!(event, HostEvent::Key { .. } | HostEvent::Commit(_))),
        "completed-round continuation stays outside typing statistics"
    );
    post_key(hwnd, 0x20, 0x39);
    feed_pending(&mut host, &mut comparer);
    assert_eq!(drain_cmds(), vec![CommandKey::Space]);
    assert_eq!(host.committed_text(), "甲\n乙");

    // Preserve the existing policy: an article containing ASCII spaces accepts
    // them as ordinary input. Enter still only inserts an expected line break.
    host.reset_text();
    comparer = Comparer::new("甲 乙");
    sync_gate(&comparer);
    type_char(&mut host, &mut comparer, '甲');
    post_key(hwnd, 0x20, 0x39);
    feed_pending(&mut host, &mut comparer);
    assert!(drain_cmds().is_empty());
    assert_eq!(host.committed_text(), "甲 ");
    post_key(hwnd, 0x0d, 0x1c);
    feed_pending(&mut host, &mut comparer);
    assert_eq!(drain_cmds(), vec![CommandKey::Enter]);
    assert_eq!(host.committed_text(), "甲 ");
    type_char(&mut host, &mut comparer, '乙');
    assert!(comparer.finished_with_correct_last_char());
    assert_eq!(comparer.wrong, 0);
}
