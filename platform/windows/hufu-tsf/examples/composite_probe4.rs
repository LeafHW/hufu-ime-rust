//! v5 探针 4：跨线程 DesktopWindowTarget 渲染性决定性实验。
//! 主线程：建窗+消息循环。工作线程（无窗）：DQ+Compositor+DWT(主线程窗)
//! +红块。若红块显示→跨线程 DWT 可行（DLL 不渲染另有原因）。

use windows::core::Interface;
use windows::Foundation::Numerics as N;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT as wrt;
use windows::Win32::UI::WindowsAndMessaging::*;

unsafe extern "system" fn wndproc(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn synth_thread(h: isize) {
    unsafe {
        let hwnd = HWND(h as *mut _);
        let hr = wrt::RoInitialize(wrt::RO_INIT_SINGLETHREADED);
        println!("[T] RoInit: {:?}", hr.map(|_| ()));
        let opts = wrt::DispatcherQueueOptions {
            dwSize: std::mem::size_of::<wrt::DispatcherQueueOptions>() as u32,
            threadType: wrt::DQTYPE_THREAD_CURRENT,
            apartmentType: wrt::DQTAT_COM_ASTA,
        };
        match wrt::CreateDispatcherQueueController(opts) {
            Ok(_) => println!("[T] DQ: ok"),
            Err(e) => println!("[T] DQ: {}", e),
        }
        match windows::UI::Composition::Compositor::new() {
            Ok(compositor) => {
                println!("[T] Compositor: ok");
                let di: ICompositorDesktopInterop = compositor.cast().unwrap();
                match di.CreateDesktopWindowTarget(hwnd, true) {
                    Ok(t) => {
                        let target: DesktopWindowTarget = t.cast().unwrap();
                        println!("[T] DWT: ok");
                        let root = compositor.CreateContainerVisual().unwrap();
                        let red = compositor.CreateColorBrush().unwrap();
                        red.SetColor(windows::UI::Color {
                            A: 255, R: 220, G: 30, B: 30,
                        })
                        .unwrap();
                        let probe = compositor.CreateSpriteVisual().unwrap();
                        probe.SetBrush(&red).unwrap();
                        probe
                            .SetSize(N::Vector2 { X: 140.0, Y: 90.0 })
                            .unwrap();
                        root.Children().unwrap().InsertAtTop(&probe).unwrap();
                        target.SetRoot(&root).unwrap();
                        println!("[T] 树+红块: ok");
                        // 泵消息（DQ flush）。
                        let mut msg = MSG::default();
                        loop {
                            if PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                                let _ = DispatchMessageW(&msg);
                                continue;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                    }
                    Err(e) => println!("[T] DWT FAIL: {:08X}", e.code().0 as u32),
                }
            }
            Err(e) => println!("[T] Compositor FAIL: {:08X}", e.code().0 as u32),
        }
    }
}

unsafe fn run() {
    let hinst: windows::Win32::Foundation::HINSTANCE =
        GetModuleHandleW(None).unwrap().into();
    let cls: windows::core::PCWSTR = windows::core::w!("HUFA_BALL_PROBE4");
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
        windows::core::w!("ball probe4"),
        WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0 | WS_VISIBLE.0),
        60, 60, 220, 160,
        HWND(std::ptr::null_mut()),
        HMENU(std::ptr::null_mut()),
        hinst,
        None,
    )
    .unwrap();
    println!("[M] hwnd: {:?}", hwnd.0);
    // 工作线程（无窗）：Compositor+跨线程 DWT。
    let h = hwnd.0 as isize;
    std::thread::spawn(move || synth_thread(h));
    // 主线程消息循环 8 秒。
    let t0 = std::time::Instant::now();
    let mut msg = MSG::default();
    while t0.elapsed().as_secs() < 8 {
        if GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = DispatchMessageW(&msg);
        }
    }
}

fn main() {
    unsafe { run() }
}
