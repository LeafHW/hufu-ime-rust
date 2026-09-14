//! v5 探针 3：合成线程自建 HWND + DesktopWindowTarget + 红块。
//! 验证「线程拥有窗口」前提（跨线程 DWT 疑似孤儿不渲染）。
//! cargo run --release --example composite_probe3 -- 后截图看红块。

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

unsafe fn run() {
    // ① 先建窗（user32/GUI 线程化）——官方 Win32 合成样例顺序：
    //    窗 → CoInitialize → DQ → Compositor。RoInit 在建窗前=坑
    //    （本机实证 DQ 静默失效），顺序敏感。
    let hinst: windows::Win32::Foundation::HINSTANCE = GetModuleHandleW(None)
        .unwrap()
        .into();
    let cls: windows::core::PCWSTR = windows::core::w!("HUFA_BALL_PROBE");
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
        WINDOW_EX_STYLE(0), // 普通 redirection bitmap（排除 NOREDIR 变量）
        cls,
        windows::core::w!("ball probe"),
        WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0 | WS_VISIBLE.0),
        60, 60, 220, 160,
        HWND(std::ptr::null_mut()),
        windows::Win32::UI::WindowsAndMessaging::HMENU(std::ptr::null_mut()),
        hinst,
        None,
    )
    .unwrap();
    println!("hwnd: {:?}", hwnd.0);
    // ② 建窗之后：公寓 + DQ。
    let hr = wrt::RoInitialize(wrt::RO_INIT_SINGLETHREADED);
    println!("RoInit: {:?}", hr.map(|_| ()));
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
    let compositor = windows::UI::Composition::Compositor::new().unwrap();
    let di: ICompositorDesktopInterop = compositor.cast().unwrap();
    let target: DesktopWindowTarget = di
        .CreateDesktopWindowTarget(hwnd, true)
        .unwrap()
        .cast()
        .unwrap();
    println!("DWT: ok");
    let root = compositor.CreateContainerVisual().unwrap();
    let red = compositor.CreateColorBrush().unwrap();
    red.SetColor(windows::UI::Color { A: 255, R: 220, G: 30, B: 30 }).unwrap();
    let probe = compositor.CreateSpriteVisual().unwrap();
    probe.SetBrush(&red).unwrap();
    probe.SetSize(N::Vector2 { X: 140.0, Y: 90.0 }).unwrap();
    root.Children().unwrap().InsertAtTop(&probe).unwrap();
    target.SetRoot(&root).unwrap();
    println!("树+红块: ok（若渲染=跨线程 DWT 才是问题）");
    // ④ 消息循环 6 秒（供外部截图）。
    let t0 = std::time::Instant::now();
    let mut msg = MSG::default();
    while t0.elapsed().as_secs() < 6 {
        if GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = DispatchMessageW(&msg);
        }
    }
}

fn main() {
    unsafe { run() }
}
