//! QQ 会话列表 UIA 结构探针（STA+消息泵版）：定位置顶会话容器。
//! 教训：QQ(Chromium) UIA 树只有 UI STA 带消息泵才可见——MTA/裸 STA 均为空。
#![allow(clippy::all)]
#[path = "../src/qq_bridge.rs"]
mod qq_bridge;
#[path = "../src/uia_bridge.rs"]
mod uia_bridge;

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationElement, TreeScope_Children, TreeScope_Descendants,
};
use windows::Win32::UI::Accessibility::{
    IUIAutomationFocusChangedEventHandler, IUIAutomationFocusChangedEventHandler_Impl,
};

/// 空事件处理器：只为把 UiaClientsAreListening() 翻 true（Chromium 激活开关）。
#[windows::core::implement(IUIAutomationFocusChangedEventHandler)]
struct NoopHandler;
impl IUIAutomationFocusChangedEventHandler_Impl for NoopHandler_Impl {
    fn HandleFocusChangedEvent(
        &self,
        _sender: Option<&IUIAutomationElement>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage,
    RegisterClassW, SetTimer, TranslateMessage, CS_HREDRAW, CS_VREDRAW, HWND_MESSAGE, MSG,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_DESTROY, WM_TIMER, WNDCLASSW,
};

static mut TICKS: u32 = 0;

fn cls_of(e: &IUIAutomationElement) -> String {
    unsafe {
        e.CurrentClassName()
            .map(|s| s.to_string())
            .unwrap_or_default()
    }
}

fn name_of(e: &IUIAutomationElement) -> String {
    unsafe {
        e.CurrentName()
            .map(|s| s.to_string())
            .unwrap_or_default()
            .chars()
            .take(24)
            .collect()
    }
}

fn children(uia: &IUIAutomation, e: &IUIAutomationElement) -> Vec<IUIAutomationElement> {
    unsafe {
        let Ok(cond) = uia.CreateTrueCondition() else {
            return vec![];
        };
        let Ok(all) = e.FindAll(TreeScope_Children, &cond) else {
            return vec![];
        };
        (0..all.Length().unwrap_or(0).min(80))
            .filter_map(|i| all.GetElement(i).ok())
            .collect()
    }
}

fn dump(uia: &IUIAutomation, hwnd_qq: isize) {
    let root = match unsafe { uia.ElementFromHandle(HWND(hwnd_qq as *mut core::ffi::c_void)) } {
        Ok(v) => v,
        Err(e) => {
            println!("ElementFromHandle failed: {e}");
            return;
        }
    };
    let mut hist: std::collections::BTreeMap<String, usize> = Default::default();
    unsafe {
        if let Ok(cond) = uia.CreateTrueCondition() {
            if let Ok(all) = root.FindAll(TreeScope_Descendants, &cond) {
                let n = all.Length().unwrap_or(0).min(4000);
                for i in 0..n {
                    if let Ok(e) = all.GetElement(i) {
                        let c = cls_of(&e);
                        if !c.is_empty() {
                            *hist.entry(c).or_default() += 1;
                        }
                    }
                }
            }
        }
    }
    println!("== histogram 全条目 {}（contact/list/pinned/top）==", hist.len());
    for (c, n) in &hist {
        if c.contains("contact")
            || c.contains("list")
            || c.contains("pinned")
            || c.contains("top")
        {
            println!("  {c} × {n}");
        }
    }
    println!("== histogram 前 40 ==");
    for (c, n) in hist.iter().take(40) {
        println!("  {c} × {n}");
    }

    let mut hits = Vec::new();
    walk(uia, &root, 0, 10, &mut hits);
    println!("== contact/list 容器（d=depth, kids=直系子数）==");
    for (d, c, n, k) in &hits {
        println!("  d{d}  {c}  name={n:?}  kids={k}");
    }

    let names = uia_bridge::list_sessions(hwnd_qq);
    println!("== list_sessions() → {} 条 ==", names.len());
    for (i, s) in names.iter().enumerate().take(30) {
        println!("  {i}: {s}");
    }
}

fn walk(
    uia: &IUIAutomation,
    e: &IUIAutomationElement,
    depth: usize,
    max_depth: usize,
    hits: &mut Vec<(usize, String, String, usize)>,
) {
    if depth > max_depth {
        return;
    }
    let cls = cls_of(e);
    if cls.contains("contact") || cls.contains("list") {
        let kids = children(uia, e);
        hits.push((depth, cls.clone(), name_of(e), kids.len()));
        let sub: Vec<String> = kids
            .iter()
            .take(14)
            .map(|k| format!("{}×{}", cls_of(k), name_of(k)))
            .collect();
        if !sub.is_empty() {
            println!("     └─ {}", sub.join(" | "));
        }
    }
    for k in children(uia, e) {
        walk(uia, &k, depth + 1, max_depth, hits);
    }
}

fn find_render_widget(root: isize) -> Option<isize> {
    struct Ctx {
        found: Option<isize>,
    }
    extern "system" fn cb(
        hwnd: HWND,
        lparam: LPARAM,
    ) -> windows::Win32::Foundation::BOOL {
        unsafe {
            let ctx = &mut *(lparam.0 as *mut Ctx);
            if ctx.found.is_some() {
                return windows::Win32::Foundation::BOOL(0);
            }
            let mut cls = [0u16; 64];
            let n = windows::Win32::UI::WindowsAndMessaging::GetClassNameW(hwnd, &mut cls);
            let class = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
            if class == "Chrome_RenderWidgetHostHWND" {
                ctx.found = Some(hwnd.0 as isize);
                return windows::Win32::Foundation::BOOL(0);
            }
            windows::Win32::Foundation::BOOL(1)
        }
    }
    let mut ctx = Ctx { found: None };
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::EnumChildWindows(
            HWND(root as *mut core::ffi::c_void),
            Some(cb),
            LPARAM(&mut ctx as *mut _ as isize),
        );
    }
    ctx.found
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_TIMER {
            let t = TICKS;
            TICKS = t + 1;
            use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
            use windows::Win32::UI::Accessibility::CUIAutomation;
            let uia_res: windows::core::Result<IUIAutomation> =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER);
            let Ok(uia) = uia_res else {
                println!("UIA_INIT_FAIL");
                PostQuitMessage(0);
                return LRESULT(0);
            };
            let Some(p) = qq_bridge::main_panel() else {
                println!("NO_PANEL");
                PostQuitMessage(0);
                return LRESULT(0);
            };
            // MSAA 预热（WM_GETOBJECT OBJID_CLIENT 是 Chromium 开无障碍的可靠开关）
            if t == 0 {
                use windows::Win32::UI::Accessibility::AccessibleObjectFromWindow;
                const OBJID_CLIENT_U32: u32 = 0xFFFFFFFC; // (u32)(-4)
                let iid_iaccessible = windows::core::GUID::from_u128(
                    0x618716e0_ac8e_11d3_9815_00c04f2ef09f,
                );
                let mut pv: *mut core::ffi::c_void = std::ptr::null_mut();
                let r = unsafe {
                    AccessibleObjectFromWindow(
                        HWND(
                            find_render_widget(p.hwnd)
                                .unwrap_or(p.hwnd) as *mut core::ffi::c_void
                        ),
                        OBJID_CLIENT_U32,
                        &iid_iaccessible,
                        &mut pv,
                    )
                };
                if !pv.is_null() {
                    let unk: windows::core::IUnknown = unsafe { std::mem::transmute(pv) };
                    drop(unk);
                }
                let hr_text = match &r {
                    Ok(()) => "OK".to_string(),
                    Err(e) => format!("{e}"),
                };
                println!("tick0 MSAA hr={hr_text} pv_null={}", pv.is_null());
            }
            // 激活试验：注册 UIA 事件处理器（UiaClientsAreListening→true，Chromium 由此开 a11y）
            static mut HANDLER: Option<IUIAutomationFocusChangedEventHandler> = None;
            if t == 0 {
                let h: IUIAutomationFocusChangedEventHandler = NoopHandler.into();
                {
                    let r = unsafe { uia.AddFocusChangedEventHandler(None, &h) };
                    let hr = match &r {
                        Ok(()) => "OK".into(),
                        Err(e) => format!("{e}"),
                    };
                    println!("tick0 AddFocusChangedEventHandler {hr}");
                    HANDLER = Some(h);
                }
            }
            // 关键：Chromium provider 挂在 Chrome_RenderWidgetHostHWND 子窗上，
            // 对它 ElementFromHandle（内部 WM_GETOBJECT）才会点亮整棵树。
            let render_hwnd = find_render_widget(p.hwnd);
            println!("tick {t} render_hwnd={render_hwnd:?}");
            let target = render_hwnd.unwrap_or(p.hwnd);
            if let Ok(root) =
                unsafe { uia.ElementFromHandle(HWND(target as *mut core::ffi::c_void)) }
            {
                let n =
                    unsafe { root.CurrentName().map(|s| s.to_string()).unwrap_or_default() };
                let desc = unsafe {
                    uia.CreateTrueCondition()
                        .ok()
                        .and_then(|c| root.FindAll(TreeScope_Descendants, &c).ok())
                        .and_then(|a| a.Length().ok())
                        .unwrap_or(0)
                };
                println!("tick {t} target name={n:?} descendants={desc}");
            }
            if t == 2 {
                // 组合激活：客户端监听器在场 + QQ 获得前台焦点
                let ok = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(
                        HWND(p.hwnd as *mut core::ffi::c_void),
                    )
                };
                println!("tick2 SetForegroundWindow(QQ) ok={}", ok.as_bool());
            }
            if t < 40 {
                return LRESULT(1);
            }
            dump(&uia, p.hwnd);
            if let Some(rh) = render_hwnd {
                println!("===== 再从 render widget 直接 dump =====");
                dump(&uia, rh);
            }
            {
                // 窗口状态诊断
                let mut r = windows::Win32::Foundation::RECT::default();
                let _ = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetWindowRect(
                        HWND(p.hwnd as *mut core::ffi::c_void),
                        &mut r,
                    )
                };
                let iconic = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::IsIconic(
                        HWND(p.hwnd as *mut core::ffi::c_void),
                    )
                };
                println!("QQ rect={:?} iconic={}", r, iconic.as_bool());
            }
            // 所有 QQ/微信顶层窗的 UIA 树规模（独立聊天窗 vs 主面板）
            for w in qq_bridge::list_chat_windows() {
                let desc = unsafe {
                    uia.ElementFromHandle(HWND(w.hwnd as *mut core::ffi::c_void))
                        .ok()
                        .and_then(|root| {
                            uia.CreateTrueCondition().ok().and_then(|c| {
                                root.FindAll(TreeScope_Descendants, &c)
                                    .ok()
                                    .and_then(|a| a.Length().ok())
                            })
                        })
                        .unwrap_or(0)
                };
                println!("CHAT hwnd={} title={:?} descendants={}", w.hwnd, w.title, desc);
            }
            dump(&uia, p.hwnd);
            PostQuitMessage(0);
            return LRESULT(0);
        }
        if msg == WM_DESTROY {
            PostQuitMessage(0);
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

fn main() {
    unsafe {
        let hr = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
        println!("CoInitialize STA hr={:X}", hr.0);
        let hinstance = windows::Win32::Foundation::HMODULE(std::ptr::null_mut());
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: w!("GalePinProbe"),
            ..Default::default()
        };
        let atom = RegisterClassW(&wc);
        println!("atom={atom}");
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("GalePinProbe"),
            None,
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            windows::Win32::Foundation::HINSTANCE(hinstance.0),
            Some(std::ptr::null()),
        )
        .expect("probe window");
        SetTimer(hwnd, 1, 500, None);
        loop {
            let mut m = MSG::default();
            let r = GetMessageW(&mut m, None, 0, 0);
            if !r.as_bool() {
                break;
            }
            let _ = TranslateMessage(&m);
            let _ = DispatchMessageW(&m);
        }
    }
}
