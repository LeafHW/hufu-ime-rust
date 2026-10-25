//! probe.rs — 虎娘候选窗像素探测（只读）。
//!
//! 路线：屏角宿主窗 + RichEdit50W → 线程/进程维度把 **虎娘** 设为当前 TIP
//! （默认 TIP 是虎符，会抢键；必须显式 ActivateProfile + 回读校验）→
//! ITfKeystrokeMgr::KeyDown 喂码 → 枚举本进程窗口 + 截图 + UI 元素 dump。
//!
//! 用法：
//!   probe <outdir> <code1[,code2..]> [km|sendinput] [wait_ms] [label] [opts]
//!   opts 逗号分隔：
//!     vt        —— 用原始 vtable 调 ITfCandidateListUIElement::GetRects/GetFontSize
//!     actfirst  —— 先 ActivateLanguageProfile 再 SetFocus（旧顺序）
//!     ppm       —— 再用 ITfInputProcessorProfileMgr::ActivateProfile(FORPROCESS|DONTCARE...)
//!     ppmnodef  —— 再用 ActivateProfile(FORPROCESS)（不带 DONTCARE）
//!     noprof    —— 跳过 ActivateLanguageProfile
//!     nofocus   —— 不做 SetFocus(doc)
//!     nofg      —— 不抢前台

use std::fmt::Write as _;
use std::io::Write as _;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::ProcessStatus::EnumProcessModules;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::TextServices::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLSID_TIGIRL: GUID = GUID::from_u128(0x69cb1b2f_cde7_43a2_96c9_ebf642e780eb);
const PROFILE_TIGIRL: GUID = GUID::from_u128(0x43201c7b_f615_469d_9d54_906d9270975e);
const LANG_ZH_CN: u16 = 0x0804;
const TF_PROFILETYPE_INPUTPROCESSOR: u32 = 1;

#[link(name = "user32")]
extern "system" {
    fn PrintWindow(hwnd: HWND, hdc: HDC, flags: u32) -> i32;
}

#[repr(C)]
struct GETTEXTEX {
    cb: u32,
    flags: u32,
    codepage: u32,
    lpDefaultChar: *mut u16,
    lpUsedDefChar: *mut u16,
}

struct Ctx {
    log: std::fs::File,
    vt: bool,
}
impl Ctx {
    fn w(&mut self, s: &str) {
        eprintln!("{}", s);
        let _ = writeln!(self.log, "{}", s);
        let _ = self.log.flush();
    }
}

unsafe fn rich_get_text(hwnd: HWND) -> String {
    let mut gt = GETTEXTEX {
        cb: 1 << 20,
        flags: 0,
        codepage: 1200,
        lpDefaultChar: std::ptr::null_mut(),
        lpUsedDefChar: std::ptr::null_mut(),
    };
    let mut buf: Vec<u16> = vec![0; (1 << 20) / 2];
    let r = SendMessageW(
        hwnd,
        0x045E,
        WPARAM(&mut gt as *mut _ as usize),
        LPARAM(buf.as_mut_ptr() as isize),
    );
    let n = r.0.max(0) as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

unsafe fn rich_clear(hwnd: HWND) {
    SendMessageW(hwnd, 0x00B1, WPARAM(0), LPARAM(-1));
    let empty: [u16; 1] = [0];
    SendMessageW(hwnd, 0x00C2, WPARAM(0), LPARAM(empty.as_ptr() as isize));
}

unsafe fn pump() {
    let mut msg = MSG::default();
    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
        let _ = TranslateMessage(&msg);
        let _ = DispatchMessageW(&msg);
    }
}

unsafe fn pump_for(ms: u64) {
    let t0 = std::time::Instant::now();
    while (t0.elapsed().as_millis() as u64) < ms {
        pump();
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
}

fn vk_of(ch: u8) -> Option<VIRTUAL_KEY> {
    match ch {
        b'a'..=b'z' => Some(VIRTUAL_KEY((ch - b'a' + b'A') as u16)),
        b'0'..=b'9' => Some(VIRTUAL_KEY(ch as u16)),
        _ => None,
    }
}

unsafe fn key_km(km: &ITfKeystrokeMgr, vk: VIRTUAL_KEY) {
    let lp = LPARAM((((MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC_EX) as u32 & 0xFF) << 16)
        | 1
        | (1 << 30)
        | (1u32 << 31)) as isize);
    let _ = km.KeyDown(WPARAM(vk.0 as usize), lp);
}

unsafe fn key_si(vk: VIRTUAL_KEY) {
    let mut inputs = [INPUT::default(); 2];
    let sc = MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC_EX) as u16;
    for (i, up) in [false, true].iter().enumerate() {
        inputs[i].r#type = INPUT_KEYBOARD;
        inputs[i].Anonymous.ki = KEYBDINPUT {
            wVk: vk,
            wScan: sc,
            dwFlags: if *up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
            time: 0,
            dwExtraInfo: 0,
        };
    }
    let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
}

unsafe extern "system" fn host_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wp, lp)
}

// ───────────── 诊断：模块 / 活动 profile / 已注册 profile ─────────────

unsafe fn dump_modules(ctx: &mut Ctx, tag: &str) {
    let mut hmods = [HMODULE(std::ptr::null_mut()); 1024];
    let mut cb = 0u32;
    let _ = EnumProcessModules(GetCurrentProcess(), hmods.as_mut_ptr(), 8192, &mut cb);
    let n = ((cb / 8) as usize).min(1024);
    let mut names: Vec<String> = Vec::new();
    for i in 0..n {
        let mut name = [0u16; 300];
        let len = GetModuleFileNameW(hmods[i], &mut name);
        let s = String::from_utf16_lossy(&name[..len as usize]);
        if let Some(f) = s.rsplit('\\').next() {
            names.push(f.to_string());
        }
    }
    let tigirl = names.iter().find(|s| s.to_lowercase().contains("tigirl"));
    let hufu = names.iter().find(|s| s.to_lowercase().contains("hufu"));
    ctx.w(&format!(
        "[mods] {tag} n={n} tigirlLoaded={:?} hufuLoaded={:?}",
        tigirl.map(|s| s.as_str()).unwrap_or("NO"),
        hufu.map(|s| s.as_str()).unwrap_or("NO")
    ));
}

unsafe fn report_active(ppm: &ITfInputProcessorProfileMgr, ctx: &mut Ctx, tag: &str) {
    let mut p = TF_INPUTPROCESSORPROFILE::default();
    match ppm.GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &mut p) {
        Ok(()) => ctx.w(&format!(
            "[active] {tag} type={} lang=0x{:04X} clsid={:?} guid={:?} flags=0x{:X}",
            p.dwProfileType, p.langid, p.clsid, p.guidProfile, p.dwFlags
        )),
        Err(e) => ctx.w(&format!("[active] {tag} GetActiveProfile 失败: {e:?}")),
    }
}

unsafe fn report_focus(tm: &ITfThreadMgr, ctx: &mut Ctx, tag: &str) {
    let gw = GetFocus();
    let mut cls = [0u16; 128];
    let n = if gw.0.is_null() { 0 } else { GetClassNameW(gw, &mut cls) };
    let cname = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
    let tsf_focus = tm.GetFocus();
    let (has_doc, ptr) = match &tsf_focus {
        Ok(d) => (true, format!("{:?}", Interface::as_raw(d))),
        Err(e) => (false, format!("ERR {e:?}")),
    };
    ctx.w(&format!(
        "[focusstate] {tag} win32Focus={:?}({cname}) tsFocusDoc={has_doc} raw={ptr}",
        gw.0
    ));
    let _ = tsf_focus;
}

unsafe fn dump_profiles(ppm: &ITfInputProcessorProfileMgr, ctx: &mut Ctx) {
    match ppm.EnumProfiles(LANG_ZH_CN) {
        Ok(en) => {
            let mut i = 0;
            loop {
                let mut buf: [TF_INPUTPROCESSORPROFILE; 8] = [TF_INPUTPROCESSORPROFILE::default(); 8];
                let mut fetched = 0u32;
                if en.Next(&mut buf, &mut fetched).is_err() || fetched == 0 {
                    break;
                }
                for p in buf.iter().take(fetched as usize) {
                    ctx.w(&format!(
                        "[profiles] #{i} type={} lang=0x{:04X} clsid={} guidProfile={} flags=0x{:X}",
                        p.dwProfileType,
                        p.langid,
                        fmt_guid(&p.clsid),
                        fmt_guid(&p.guidProfile),
                        p.dwFlags
                    ));
                    i += 1;
                }
                if i > 64 {
                    break;
                }
            }
            ctx.w(&format!("[profiles] 共 {i} 条 (zh-CN)"));
        }
        Err(e) => ctx.w(&format!("[profiles] EnumProfiles 失败: {e:?}")),
    }
}

fn fmt_guid(g: &GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1, g.data2, g.data3, g.data4[0], g.data4[1], g.data4[2], g.data4[3], g.data4[4], g.data4[5], g.data4[6], g.data4[7]
    )
}

// ───────────── 窗口枚举 ─────────────

#[derive(Clone)]
struct WndRec {
    hwnd: isize,
    class: String,
    title: String,
    rect: RECT,
    visible: bool,
    style: u32,
    exstyle: u32,
    tid: u32,
    depth: i32,
    parent: isize,
    owner: isize,
}

unsafe fn rec_of(h: HWND, depth: i32) -> WndRec {
    let mut class = [0u16; 256];
    let n = GetClassNameW(h, &mut class);
    let mut title = [0u16; 512];
    let m = GetWindowTextW(h, &mut title);
    let mut rc = RECT::default();
    let _ = GetWindowRect(h, &mut rc);
    let mut tid = 0u32;
    let _ = GetWindowThreadProcessId(h, Some(&mut tid as *mut u32));
    WndRec {
        hwnd: h.0 as isize,
        class: String::from_utf16_lossy(&class[..n.max(0) as usize]),
        title: String::from_utf16_lossy(&title[..m.max(0) as usize]),
        rect: rc,
        visible: IsWindowVisible(h).as_bool(),
        style: GetWindowLongW(h, GWL_STYLE) as u32,
        exstyle: GetWindowLongW(h, GWL_EXSTYLE) as u32,
        tid,
        depth,
        parent: GetParent(h).map(|p| p.0 as isize).unwrap_or(0),
        owner: GetWindow(h, GW_OWNER).map(|p| p.0 as isize).unwrap_or(0),
    }
}

unsafe extern "system" fn enum_child_cb(h: HWND, lp: LPARAM) -> BOOL {
    let out = &mut *(lp.0 as *mut Vec<WndRec>);
    out.push(rec_of(h, 1));
    BOOL(1)
}

unsafe extern "system" fn enum_cb(h: HWND, lp: LPARAM) -> BOOL {
    let out = &mut *(lp.0 as *mut Vec<WndRec>);
    let mut pid = 0u32;
    let _ = GetWindowThreadProcessId(h, Some(&mut pid as *mut u32));
    if pid == windows::Win32::System::Threading::GetCurrentProcessId() {
        out.push(rec_of(h, 0));
        let mut kids: Vec<WndRec> = Vec::new();
        let _ = EnumChildWindows(h, Some(enum_child_cb), LPARAM(&mut kids as *mut _ as isize));
        out.extend(kids);
    }
    BOOL(1)
}

fn rh(r: &RECT) -> String {
    format!(
        "{},{} {}x{} (r={},b={})",
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
        r.right,
        r.bottom
    )
}

// ───────────── 截图 ─────────────

unsafe fn grab(hwnd: HWND, path: &str, kind: u32) -> bool {
    let mut rc = RECT::default();
    if GetWindowRect(hwnd, &mut rc).is_err() {
        return false;
    }
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;
    if w <= 0 || h <= 0 {
        return false;
    }
    let sdc = GetDC(HWND(std::ptr::null_mut()));
    let mdc = CreateCompatibleDC(sdc);
    let bmp = CreateCompatibleBitmap(sdc, w, h);
    let old = SelectObject(mdc, HGDIOBJ(bmp.0));
    let ok = match kind {
        0 => PrintWindow(hwnd, mdc, 0) != 0,
        2 => PrintWindow(hwnd, mdc, 2) != 0,
        _ => BitBlt(mdc, 0, 0, w, h, sdc, rc.left, rc.top, SRCCOPY | CAPTUREBLT).is_ok(),
    };
    let mut bi = BITMAPINFO::default();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB.0;
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let got = GetDIBits(mdc, bmp, 0, h as u32, Some(buf.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
    let mut saved = false;
    if got > 0 {
        if let Ok(mut f) = std::fs::File::create(path) {
            let hdr_size = 14u32 + 40u32;
            let pix = (w * h * 4) as u32;
            let mut head = Vec::with_capacity(54);
            head.extend_from_slice(b"BM");
            head.extend_from_slice(&(hdr_size + pix).to_le_bytes());
            head.extend_from_slice(&[0u8; 4]);
            head.extend_from_slice(&hdr_size.to_le_bytes());
            head.extend_from_slice(&40u32.to_le_bytes());
            head.extend_from_slice(&w.to_le_bytes());
            head.extend_from_slice(&(-h).to_le_bytes());
            head.extend_from_slice(&1u16.to_le_bytes());
            head.extend_from_slice(&32u16.to_le_bytes());
            head.extend_from_slice(&0u32.to_le_bytes());
            head.extend_from_slice(&pix.to_le_bytes());
            head.extend_from_slice(&[0u8; 16]);
            let _ = f.write_all(&head);
            let _ = f.write_all(&buf);
            saved = true;
        }
    }
    let _ = SelectObject(mdc, old);
    let _ = DeleteObject(HGDIOBJ(bmp.0));
    let _ = DeleteDC(mdc);
    ReleaseDC(HWND(std::ptr::null_mut()), sdc);
    saved && ok
}

// ───────────── TSF 候选 UI 元素 ─────────────

type GetRectsFn = unsafe extern "system" fn(*mut core::ffi::c_void, u32, u32, *mut RECT) -> HRESULT;
type GetFontSizeFn = unsafe extern "system" fn(*mut core::ffi::c_void, *const RECT, *mut u32) -> HRESULT;

unsafe fn vfunc<T>(el: &ITfCandidateListUIElement, idx: usize) -> T {
    let raw = Interface::as_raw(el) as *mut *const usize;
    let vt = *raw;
    std::mem::transmute_copy(&*(vt.add(idx)))
}

unsafe fn call_get_rects(el: &ITfCandidateListUIElement, i: u32, flags: u32) -> Option<RECT> {
    let f: GetRectsFn = vfunc(el, 15);
    let mut r = RECT::default();
    let hr = f(Interface::as_raw(el), i, flags, &mut r);
    if hr.is_ok() { Some(r) } else { None }
}

unsafe fn call_get_font_size(el: &ITfCandidateListUIElement) -> Option<u32> {
    let f: GetFontSizeFn = vfunc(el, 16);
    let mut fs = 0u32;
    let hr = f(Interface::as_raw(el), std::ptr::null(), &mut fs);
    if hr.is_ok() { Some(fs) } else { None }
}

unsafe fn dump_ui_elements(tm: &ITfThreadMgr, ctx: &mut Ctx, tag: &str) {
    let mgr: ITfUIElementMgr = match tm.cast() {
        Ok(m) => m,
        Err(e) => {
            ctx.w(&format!("[ui] {tag} QI ITfUIElementMgr 失败: {e:?}"));
            return;
        }
    };
    let en = match mgr.EnumUIElements() {
        Ok(e) => e,
        Err(e) => {
            ctx.w(&format!("[ui] {tag} EnumUIElements 失败: {e:?}"));
            return;
        }
    };
    let mut idx = 0;
    loop {
        let mut buf: [Option<ITfUIElement>; 4] = [None, None, None, None];
        let mut fetched = 0u32;
        if en.Next(&mut buf, &mut fetched).is_err() || fetched == 0 {
            break;
        }
        for el in buf.iter().take(fetched as usize) {
            let Some(el) = el else { continue };
            let desc = el.GetDescription().map(|b| b.to_string()).unwrap_or_default();
            let guid = el.GetGUID().map(|g| fmt_guid(&g)).unwrap_or_default();
            let shown = el.IsShown().map(|b| b.as_bool()).unwrap_or(false);
            ctx.w(&format!("[ui] {tag} el{idx} desc={desc:?} guid={guid} shown={shown}"));
            if let Ok(cl) = el.cast::<ITfCandidateListUIElement>() {
                let count = cl.GetCount().unwrap_or(0);
                let sel = cl.GetSelection().unwrap_or(u32::MAX);
                let page = cl.GetCurrentPage().unwrap_or(u32::MAX);
                let flags = cl.GetUpdatedFlags().unwrap_or(u32::MAX);
                ctx.w(&format!(
                    "[ui] {tag} el{idx} CAND count={count} selection={sel} currentPage={page} flags=0x{flags:X}"
                ));
                for i in 0..count.min(40) {
                    let s = cl.GetString(i).map(|b| b.to_string()).unwrap_or_default();
                    let mut line = format!("[ui] {tag} el{idx} cand[{i}]={s:?}");
                    if ctx.vt {
                        for fl in [0u32, 1u32] {
                            if let Some(r) = call_get_rects(&cl, i, fl) {
                                let _ = write!(line, " rect(fl={fl})={},{} {}x{}", r.left, r.top, r.right - r.left, r.bottom - r.top);
                            } else {
                                let _ = write!(line, " rect(fl={fl})=ERR");
                            }
                        }
                        match call_get_font_size(&cl) {
                            Some(fs) => {
                                let _ = write!(line, " fontSize={fs}");
                            }
                            None => {
                                let _ = write!(line, " fontSize=ERR");
                            }
                        }
                    }
                    ctx.w(&line);
                }
            }
            idx += 1;
        }
    }
    if idx == 0 {
        ctx.w(&format!("[ui] {tag} 无 UI 元素（组段未开始？）"));
    }
}

// ───────────── main ─────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: probe <outdir> <code1[,code2..]> [km|sendinput] [wait_ms] [label] [opts]");
        std::process::exit(2);
    }
    let outdir = args[1].clone();
    let codes: Vec<String> = args[2].split(',').map(|s| s.to_string()).collect();
    let mode = args.get(3).cloned().unwrap_or_else(|| "km".into());
    let wait_ms: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(320);
    let label = args.get(5).cloned().unwrap_or_else(|| "run".into());
    let opts: Vec<String> = args.get(6).map(|s| s.split(',').map(|x| x.to_string()).collect()).unwrap_or_default();
    let has = |o: &str| opts.iter().any(|x| x == o);
    // 宿主窗位置 x,y（缺省 8,8）——用于测试候选窗朝向是否随屏幕位置变化
    let mut wx = 8i32;
    let mut wy = 8i32;
    if let Some(pos) = opts.iter().find(|o| o.starts_with("at=")) {
        let p: Vec<i32> = pos[3..].split(':').filter_map(|v| v.parse().ok()).collect();
        if p.len() == 2 {
            wx = p[0];
            wy = p[1];
        }
    }
    std::fs::create_dir_all(&outdir).unwrap();

    let mut ctx = Ctx {
        log: std::fs::File::create(format!("{outdir}\\probe-{label}.log")).unwrap(),
        vt: has("vt"),
    };
    // TSF 环境提示（trace 由启动脚本设置环境变量）
    ctx.w(&format!(
        "[env] label={label} mode={mode} wait={wait_ms} opts={opts:?} frameTraceDir={:?}",
        std::env::var("TIGIRL_FRAME_TRACE_DIR").unwrap_or_else(|_| "unset".into())
    ));
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let _lib = LoadLibraryW(w!("msftedit.dll"));
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let cls: Vec<u16> = "TigirlProbeHost\0".encode_utf16().collect();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(host_wndproc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hInstance: hinst,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let title: Vec<u16> = "tigirl-probe\0".encode_utf16().collect();
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(cls.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            wx,
            wy,
            760,
            320,
            None,
            None,
            None,
            None,
        )
        .expect("CreateWindow 失败");
        let re_cls: Vec<u16> = "RICHEDIT50W\0".encode_utf16().collect();
        let rich = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(re_cls.as_ptr()),
            PCWSTR(std::ptr::null()),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_WANTRETURN) as u32),
            0,
            0,
            720,
            260,
            Some(&hwnd),
            None,
            None,
            None,
        )
        .expect("RichEdit 失败");
        pump();

        let tm: ITfThreadMgr = CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER).unwrap();
        let _tid = tm.Activate().unwrap();

        let mut doc: Option<ITfDocumentMgr> = None;
        let iid_doc = <ITfDocumentMgr as Interface>::IID;
        let mut pv: *mut core::ffi::c_void = std::ptr::null_mut();
        let hr = SendMessageW(rich, 0x043C, WPARAM(&iid_doc as *const _ as usize), LPARAM(&mut pv as *mut _ as isize));
        if hr.0 != 0 && !pv.is_null() {
            doc = Some(std::mem::transmute(pv));
        }
        ctx.w(&format!(
            "[env] EM_GETOLEINTERFACE hr={} docPtr={:?}",
            hr.0,
            pv
        ));

        let profiles: ITfInputProcessorProfiles =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER).unwrap();
        let ppm: ITfInputProcessorProfileMgr = profiles.cast().unwrap();

        let sysdpi = {
            let sdc = GetDC(HWND(std::ptr::null_mut()));
            let d = GetDeviceCaps(sdc, LOGPIXELSY);
            ReleaseDC(HWND(std::ptr::null_mut()), sdc);
            d
        };
        ctx.w(&format!("[env] doc={} dpiWin={} sysdpi={sysdpi}", doc.is_some(), GetDpiForWindow(hwnd)));

        if !has("nofg") {
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(rich);
            pump_for(100);
        }
        dump_profiles(&ppm, &mut ctx);
        report_active(&ppm, &mut ctx, "init");
        dump_modules(&mut ctx, "init");

        // 激活：完整版顺序 = SetFocus(doc) → ActivateLanguageProfile → （可选）ppm → 再 SetFocus
        if !has("actfirst") && !has("nofocus") && !has("nofocus2") {
            if let Some(d) = &doc {
                let r = tm.SetFocus(d);
                ctx.w(&format!("[focus] SetFocus(doc) 1st -> {r:?}"));
                report_focus(&tm, &mut ctx, "after-setfocus1");
            }
        }
        if !has("noprof") {
            match profiles.ActivateLanguageProfile(&CLSID_TIGIRL, LANG_ZH_CN, &PROFILE_TIGIRL) {
                Ok(()) => ctx.w("[act] ActivateLanguageProfile(虎娘) OK"),
                Err(e) => ctx.w(&format!("[act] ActivateLanguageProfile(虎娘) 失败: {e:?}")),
            }
            pump_for(120);
            report_active(&ppm, &mut ctx, "after-ALP");
            dump_modules(&mut ctx, "after-ALP");
        }
        if has("actfirst") {
            if let Some(d) = &doc {
                let r = tm.SetFocus(d);
                ctx.w(&format!("[focus] SetFocus(doc) after-ALP -> {r:?}"));
            }
            pump_for(60);
            report_active(&ppm, &mut ctx, "after-focus");
            dump_modules(&mut ctx, "after-focus");
        }
        if has("ppm") || has("ppmnodef") {
            let mut fl = TF_IPPMF_FORPROCESS;
            if has("ppm") {
                fl |= TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE;
            }
            match ppm.ActivateProfile(
                TF_PROFILETYPE_INPUTPROCESSOR,
                LANG_ZH_CN,
                &CLSID_TIGIRL,
                &PROFILE_TIGIRL,
                HKL(std::ptr::null_mut()),
                fl,
            ) {
                Ok(()) => ctx.w(&format!("[act] ppm.ActivateProfile(flags=0x{fl:X}) OK")),
                Err(e) => ctx.w(&format!("[act] ppm.ActivateProfile(flags=0x{fl:X}) 失败: {e:?}")),
            }
            pump_for(150);
            report_active(&ppm, &mut ctx, "after-ppm");
            dump_modules(&mut ctx, "after-ppm");
            if !has("nofocus") && !has("nofocus2") {
                if let Some(d) = &doc {
                    let r = tm.SetFocus(d);
                    ctx.w(&format!("[focus] SetFocus(doc) after-ppm -> {r:?}"));
                }
            }
            pump_for(80);
            report_active(&ppm, &mut ctx, "after-ppm-refocus");
            dump_modules(&mut ctx, "after-ppm-refocus");
            report_focus(&tm, &mut ctx, "after-ppm-refocus");
        }
        // refocus：把 Win32 焦点从 RichEdit 上摘掉再还回去，逼控件重新向线程管理器上报焦点
        if has("refocus") {
            let _ = SetFocus(hwnd);
            pump_for(50);
            let _ = SetFocus(rich);
            pump_for(150);
            ctx.w("[focus] refocus(hwnd→rich) 完成");
            report_focus(&tm, &mut ctx, "after-refocus");
            report_active(&ppm, &mut ctx, "after-refocus");
        }
        match tm.IsThreadFocus() {
            Ok(b) => ctx.w(&format!("[focus] IsThreadFocus={}", b.as_bool())),
            Err(e) => ctx.w(&format!("[focus] IsThreadFocus err {e:?}")),
        }

        let km: ITfKeystrokeMgr = tm.cast().unwrap();
        let use_si = mode == "sendinput";
        if use_si {
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(rich);
            pump_for(150);
        }

        ctx.w("[phase] baseline");
        dump_ui_elements(&tm, &mut ctx, "baseline");
        dump_windows(&mut ctx, &outdir, "baseline", &hwnd);

        for (i, code) in codes.iter().enumerate() {
            let tag = format!("{label}_c{i}_{code}");
            rich_clear(rich);
            pump_for(30);
            if use_si { key_si(VK_ESCAPE); } else { key_km(&km, VK_ESCAPE); }
            pump_for(20);
            for ch in code.as_bytes() {
                let Some(vk) = vk_of(*ch) else { continue };
                if use_si {
                    key_si(vk);
                    std::thread::sleep(std::time::Duration::from_millis(3));
                } else {
                    key_km(&km, vk);
                }
                pump_for(14);
            }
            pump_for(wait_ms);
            let comp = rich_get_text(rich);
            ctx.w(&format!(
                "[phase] code[{i}]={code:?} tag={tag} richText={:?}",
                comp.chars().take(40).collect::<String>()
            ));
            dump_ui_elements(&tm, &mut ctx, &tag);
            dump_windows(&mut ctx, &outdir, &tag, &hwnd);
            pump_for(140);
            dump_windows(&mut ctx, &outdir, &format!("{tag}_t2"), &hwnd);
            if use_si { key_si(VK_ESCAPE); } else { key_km(&km, VK_ESCAPE); }
            pump_for(60);
        }
        ctx.w("[phase] after-all");
        dump_windows(&mut ctx, &outdir, "afterall", &hwnd);

        let _ = tm.Deactivate();
        let _ = DestroyWindow(rich);
        let _ = DestroyWindow(hwnd);
    }
}

unsafe fn dump_windows(ctx: &mut Ctx, outdir: &str, tag: &str, host: &HWND) {
    let mut recs: Vec<WndRec> = Vec::new();
    let _ = EnumWindows(Some(enum_cb), LPARAM(&mut recs as *mut _ as isize));
    ctx.w(&format!("[win] ---- {tag} : {} windows ----", recs.len()));
    for r in &recs {
        ctx.w(&format!(
            "[win] {tag} hwnd=0x{:X} d={} vis={} cls={:?} title={:?} rect={} style=0x{:08X} ex=0x{:08X} tid={} parent=0x{:X} owner=0x{:X} host={}",
            r.hwnd, r.depth, r.visible, r.class, r.title, rh(&r.rect), r.style, r.exstyle, r.tid, r.parent, r.owner,
            r.hwnd == host.0 as isize
        ));
    }
    for r in &recs {
        let area = (r.rect.right - r.rect.left) as i64 * (r.rect.bottom - r.rect.top) as i64;
        if !r.visible || area <= 0 {
            continue;
        }
        if r.class == "TigirlProbeHost" {
            continue; // 宿主窗不用抓（省时间）
        }
        let base = format!("{outdir}\\{tag}_w{:X}", r.hwnd);
        let mut flags = String::new();
        if grab(HWND(r.hwnd as *mut _), &format!("{base}_pw0.bmp"), 0) { flags.push_str("pw0 "); }
        if grab(HWND(r.hwnd as *mut _), &format!("{base}_pw2.bmp"), 2) { flags.push_str("pw2 "); }
        if grab(HWND(r.hwnd as *mut _), &format!("{base}_scr.bmp"), 9) { flags.push_str("scr "); }
        ctx.w(&format!("[grab] {tag} hwnd=0x{:X} cls={:?} ok=[{}]", r.hwnd, r.class, flags));
    }
}
