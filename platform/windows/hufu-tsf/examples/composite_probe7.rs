//! v5 探针 7：完整模拟 DLL 宿主环境。
//! 主线程：CoInit STA + 建窗 + 消息循环（= TSF 宿主形态）。
//! 合成线程：RoInit STA + DQ(ASTA) + Compositor + 跨线程 DWT + 红块 + 泵消息。

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
        let _dqc = match wrt::CreateDispatcherQueueController(opts) {
            Ok(c) => {
                println!("[T] DQ: ok");
                Some(c)
            }
            Err(e) => {
                println!("[T] DQ: {}", e);
                return;
            }
        };
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
                        // surface 路径：D3D→D2D→GraphicsDevice→surface→球。
                        use windows::Win32::Graphics::Direct2D::*;
                        use windows::Win32::Graphics::Direct2D::Common::*;
                        let mut d3d: Option<windows::Win32::Graphics::Direct3D11::ID3D11Device> = None;
                        windows::Win32::Graphics::Direct3D11::D3D11CreateDevice(
                            None,
                            windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE,
                            windows::Win32::Foundation::HMODULE::default(),
                            windows::Win32::Graphics::Direct3D11::D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                            None,
                            windows::Win32::Graphics::Direct3D11::D3D11_SDK_VERSION,
                            Some(&mut d3d), None, None,
                        ).unwrap();
                        let dxgi: windows::Win32::Graphics::Dxgi::IDXGIDevice = d3d.unwrap().cast().unwrap();
                        let f: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).unwrap();
                        let d2d = f.CreateDevice(&dxgi).unwrap();
                        let ci: windows::Win32::System::WinRT::Composition::ICompositorInterop = compositor.cast().unwrap();
                        let gdev = ci.CreateGraphicsDevice(&d2d).unwrap();
                        println!("[T] gdev: ok");
                        let surface = gdev.CreateDrawingSurface(
                            windows::Foundation::Size { Width: 22.0, Height: 22.0 },
                            windows::Graphics::DirectX::DirectXPixelFormat::B8G8R8A8UIntNormalized,
                            windows::Graphics::DirectX::DirectXAlphaMode::Premultiplied,
                        ).unwrap();
                        let sbr = compositor.CreateSurfaceBrush().unwrap();
                        sbr.SetSurface(&surface).unwrap();
                        let cv = compositor.CreateSpriteVisual().unwrap();
                        cv.SetBrush(&sbr).unwrap();
                        cv.SetSize(N::Vector2 { X: 22.0, Y: 22.0 }).unwrap();
                        cv.SetOffset(N::Vector3 { X: 40.0, Y: 90.0, Z: 0.0 });
                        root.Children().unwrap().InsertAtTop(&cv).unwrap();
                        // 画球。
                        let si: windows::Win32::System::WinRT::Composition::ICompositionDrawingSurfaceInterop = surface.cast().unwrap();
                        let mut off = windows::Win32::Foundation::POINT::default();
                        let ctx: ID2D1DeviceContext = si.BeginDraw(None, &mut off as *mut _).unwrap();
                        let _ = ctx.BeginDraw();
                        ctx.SetTransform(&windows::Foundation::Numerics::Matrix3x2::identity());
                        ctx.Clear(None);
                        ctx.SetTransform(&windows::Foundation::Numerics::Matrix3x2 {
                            M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0,
                            M31: off.x as f32, M32: off.y as f32,
                        });
                        let col = windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F { a: 1.0, r: 0.05, g: 0.05, b: 0.06 };
                        let br = ctx.CreateSolidColorBrush(&col, None).unwrap();
                        ctx.FillEllipse(&D2D1_ELLIPSE {
                            point: D2D_POINT_2F { x: 11.0, y: 11.0 },
                            radiusX: 11.0, radiusY: 11.0,
                        }, &br);
                        let _ = ctx.EndDraw(None, None);
                        let ed = si.EndDraw();
                        println!("[T] surface 球: EndDraw={:?} off=({},{})", ed.map(|_| ()), off.x, off.y);
                        target.SetRoot(&root).unwrap();
                        println!("[T] 树+红块+surface球: ok");
                        // 泵消息（DQ flush——DLL 同款循环）。
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
    // 主线程：CoInit STA（TSF 宿主形态）。
    let hr = CoInitializeEx(
        None,
        windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
    );
    println!("[M] CoInit: {}", hr.0);
    let hinst: windows::Win32::Foundation::HINSTANCE =
        GetModuleHandleW(None).unwrap().into();
    let cls: windows::core::PCWSTR = windows::core::w!("HUFA_BALL_PROBE7");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinst,
        lpszClassName: cls,
        hbrBackground: windows::Win32::Graphics::Gdi::CreateSolidBrush(COLORREF(0x00FFFFFF)),
        ..Default::default()
    };
    RegisterClassExW(&wc);
    // DLL 候选窗同款：WS_POPUP + NOREDIRECTIONBITMAP（无背景刷）。
    let hwnd = CreateWindowExW(
        WS_EX_NOREDIRECTIONBITMAP,
        cls,
        windows::core::w!("ball probe7"),
        WINDOW_STYLE(WS_POPUP.0 | WS_VISIBLE.0),
        60, 60, 220, 160,
        HWND(std::ptr::null_mut()),
        HMENU(std::ptr::null_mut()),
        hinst,
        None,
    )
    .unwrap();
    println!("[M] hwnd: {:?}", hwnd.0);
    let h = hwnd.0 as isize;
    std::thread::spawn(move || synth_thread(h));
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
