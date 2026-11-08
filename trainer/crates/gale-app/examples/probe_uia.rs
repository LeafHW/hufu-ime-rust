#[path = "../src/qq_bridge.rs"]
mod qq_bridge;
#[path = "../src/uia_bridge.rs"]
mod uia_bridge;

fn main() {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_MULTITHREADED,
        );
    }
    unsafe {
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
        use windows::Win32::UI::Accessibility::{CUIAutomation, CUIAutomation8, IUIAutomation};
        let v8: windows::core::Result<IUIAutomation> =
            CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER);
        let v1: windows::core::Result<IUIAutomation> =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER);
        println!("V8 {:?} V1 {:?}", v8.is_ok(), v1.is_ok());
    }
    match qq_bridge::main_panel() {
        Some(p) => {
            println!("PANEL hwnd={} title={}", p.hwnd, p.title);
            let names = uia_bridge::list_sessions(p.hwnd);
            println!("SESSIONS {} {:?}", names.len(), names);
        }
        None => println!("NO_PANEL"),
    }
}
