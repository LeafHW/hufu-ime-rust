//! v5 探针 2：DispatcherQueue 四组合矩阵 × Compositor。

use windows::core::Interface;
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;

unsafe fn try_combo(tt: i32, ap: i32, name: &str) {
    let opts = windows::Win32::System::WinRT::DispatcherQueueOptions {
        dwSize: std::mem::size_of::<windows::Win32::System::WinRT::DispatcherQueueOptions>()
            as u32,
        threadType: windows::Win32::System::WinRT::DISPATCHERQUEUE_THREAD_TYPE(tt),
        apartmentType: windows::Win32::System::WinRT::DISPATCHERQUEUE_THREAD_APARTMENTTYPE(ap),
    };
    match windows::Win32::System::WinRT::CreateDispatcherQueueController(opts) {
        Ok(_) => {
            match windows::UI::Composition::Compositor::new() {
                Ok(c) => {
                    let qi = match c.cast::<ICompositorDesktopInterop>() {
                        Ok(_) => "yes".into(),
                        Err(e) => format!("no({:08X})", e.code().0 as u32),
                    };
                    let cv = match c.CreateContainerVisual() {
                        Ok(_) => "ok".into(),
                        Err(e) => format!("{:08X}", e.code().0 as u32),
                    };
                    println!("{}: DQ ok, Compositor ok, interop QI={}, CV={}", name, qi, cv);
                }
                Err(e) => println!("{}: DQ ok, Compositor FAIL {:08X}", name, e.code().0 as u32),
            }
        }
        Err(e) => println!("{}: DQ FAIL {:08X}", name, e.code().0 as u32),
    }
}

fn main() {
    unsafe {
        let hr = windows::Win32::System::WinRT::RoInitialize(
            windows::Win32::System::WinRT::RO_INIT_SINGLETHREADED,
        );
        println!("RoInitialize: {:?}", hr.map(|_| ()));
        // 每组合独立进程跑（DQ 一线程一次）——参数选
        let args: Vec<String> = std::env::args().collect();
        let which = if args.len() > 1 { args[1].clone() } else { "asta".into() };
        match which.as_str() {
            "asta" => try_combo(2, 1, "CURRENT+ASTA"), // CURRENT=2, ASTA=1
            "none" => try_combo(2, 0, "CURRENT+NONE"),
            "sta" => try_combo(2, 2, "CURRENT+STA"),
            "dedi" => try_combo(1, 1, "DEDICATED+ASTA"),
            _ => println!("usage: asta|none|sta|dedi"),
        }
    }
}
