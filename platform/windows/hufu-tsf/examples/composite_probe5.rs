//! v5 探针 5：1:1 复刻微软官方 Win32 合成样例顺序。
//! CoInitializeEx(STA) → DQ(NONE) → 建窗 → Compositor → DWT → 红块。

use windows::core::Interface;
use windows::Foundation::Numerics as N;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::CoInitializeEx;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT as wrt;
use windows::Win32::UI::WindowsAndMessaging::*;

unsafe extern "system" fn wndproc(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe fn run() {
    // ① 官方第一步：CoInitializeEx STA。
    let hr = CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    println!("CoInitEx hr: {}", hr.0);
    // ② 建窗在前（顺序实验）。
    let hinst: windows::Win32::Foundation::HINSTANCE =
        GetModuleHandleW(None).unwrap().into();
    let cls: windows::core::PCWSTR = windows::core::w!("HUFA_BALL_PROBE5");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinst,
        lpszClassName: cls,
        hbrBackground: windows::Win32::Graphics::Gdi::CreateSolidBrush(COLORREF(0x00FFFFFF)),
        ..Default::default()
    };
    RegisterClassExW(&wc);
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        cls,
        windows::core::w!("ball probe5"),
        WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0 | WS_VISIBLE.0),
        60, 60, 220, 160,
        HWND(std::ptr::null_mut()),
        HMENU(std::ptr::null_mut()),
        hinst,
        None,
    )
    .unwrap();
    println!("hwnd: {:?}", hwnd.0);
    // ③ 泵空窗口消息（WM_CREATE 等 pending——排除队列状态干扰）
    //    → DQ → Compositor。
    let mut msg0 = MSG::default();
    while PeekMessageW(&mut msg0, None, 0, 0, PM_REMOVE).as_bool() {
        let _ = DispatchMessageW(&msg0);
    }
    let opts = wrt::DispatcherQueueOptions {
        dwSize: std::mem::size_of::<wrt::DispatcherQueueOptions>() as u32,
        threadType: wrt::DQTYPE_THREAD_CURRENT,
        apartmentType: wrt::DQTAT_COM_NONE,
    };
    match wrt::CreateDispatcherQueueController(opts) {
        Ok(_) => println!("DQ: ok"),
        Err(e) => println!("DQ: {}", e),
    }
    // ④ Compositor + DWT + 红块。
    match windows::UI::Composition::Compositor::new() {
        Ok(compositor) => {
            println!("Compositor: ok");
            let di: ICompositorDesktopInterop = compositor.cast().unwrap();
            let target: DesktopWindowTarget = di
                .CreateDesktopWindowTarget(hwnd, true)
                .unwrap()
                .cast()
                .unwrap();
            println!("DWT: ok");
            let root = compositor.CreateContainerVisual().unwrap();
            let red = compositor.CreateColorBrush().unwrap();
            red.SetColor(windows::UI::Color { A: 255, R: 220, G: 30, B: 30 })
                .unwrap();
            let probe = compositor.CreateSpriteVisual().unwrap();
            probe.SetBrush(&red).unwrap();
            probe.SetSize(N::Vector2 { X: 140.0, Y: 90.0 }).unwrap();
            root.Children().unwrap().InsertAtTop(&probe).unwrap();
            target.SetRoot(&root).unwrap();
            println!("树+红块: ok");
            let t0 = std::time::Instant::now();
            let mut msg = MSG::default();
            while t0.elapsed().as_secs() < 8 {
                if GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = DispatchMessageW(&msg);
                }
            }
        }
        Err(e) => println!("Compositor FAIL: {:08X} {}", e.code().0 as u32, e.message()),
    }
}

fn main() {
    unsafe { run() }
}
