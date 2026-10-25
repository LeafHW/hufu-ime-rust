//! bgdrive.rs - drive Tigirl fully in background: private desktop + TSF key events.
use std::io::Write as _;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::StationsAndDesktops::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::TextServices::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLSID_TIGIRL: GUID = GUID::from_u128(0x69cb1b2f_cde7_43a2_96c9_ebf642e780eb);
const PROFILE_TIGIRL: GUID = GUID::from_u128(0x43201c7b_f615_469d_9d54_906d9270975e);
const CLSID_TF_PROFILES: GUID = GUID::from_u128(0x33c53a50_f456_4884_b049_85fd643ecfed);
const LANG_ZH_CN: u16 = 0x0804;

#[repr(C)]
struct GETTEXTEX { cb: u32, flags: u32, codepage: u32, a: *mut u16, b: *mut u16 }

unsafe fn rich_get_text(hwnd: HWND) -> String {
    let mut gt = GETTEXTEX { cb: 1 << 16, flags: 0, codepage: 1200, a: std::ptr::null_mut(), b: std::ptr::null_mut() };
    let mut buf: Vec<u16> = vec![0; (1 << 16) / 2];
    let r = SendMessageW(hwnd, 0x045E, WPARAM(&mut gt as *mut _ as usize), LPARAM(buf.as_mut_ptr() as isize));
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

fn vk_of(ch: u8) -> Option<VIRTUAL_KEY> {
    match ch {
        b'a'..=b'z' => Some(VIRTUAL_KEY((ch - b'a' + b'A') as u16)),
        b'0'..=b'9' => Some(VIRTUAL_KEY(ch as u16)),
        _ => None,
    }
}

fn clean(s: &str) -> String { s.chars().filter(|c| !c.is_control() && !c.is_whitespace()).collect() }

unsafe extern "system" fn host_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT { DefWindowProcW(h, m, w, l) }

unsafe fn key_lp(vk: VIRTUAL_KEY) -> LPARAM {
    let sc = (MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC_EX) as u32) & 0xFF;
    LPARAM((1u32 | (sc << 16)) as isize)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 { eprintln!("usage: bgdrive <pairs.tsv> <out.csv> [maxN] [ms]"); std::process::exit(2); }
    let path = args[1].clone();
    let out_path = args[2].clone();
    let max_n: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
    let per_key_ms: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);

    let lines: Vec<String> = std::fs::read_to_string(&path).expect("read corpus failed")
        .lines().map(|l| l.trim_end_matches('\r').to_string()).filter(|l| !l.is_empty()).take(max_n).collect();
    eprintln!("[bg] loaded {} lines, per-key {}ms", lines.len(), per_key_ms);

    unsafe {
        if std::env::var("TIGIRL_DESKTOP").map(|v| v != "0").unwrap_or(true) {
            let desk_name: Vec<u16> = "TigirlBenchDesk\0".encode_utf16().collect();
            match CreateDesktopW(PCWSTR(desk_name.as_ptr()), PCWSTR::null(), None, DESKTOP_CONTROL_FLAGS(0), 0x1000_0000, None) {
                Ok(hdesk) => match SetThreadDesktop(hdesk) {
                    Ok(()) => eprintln!("[bg] private desktop OK"),
                    Err(e) => eprintln!("[bg] SetThreadDesktop failed {e:?}"),
                },
                Err(e) => eprintln!("[bg] CreateDesktop failed {e:?}"),
            }
        }
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).unwrap();
        let _lib = LoadLibraryW(w!("msftedit.dll"));
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let cls: Vec<u16> = "TigirlBgDriveHost\0".encode_utf16().collect();
        let wc = WNDCLASSW { lpfnWndProc: Some(host_proc), lpszClassName: PCWSTR(cls.as_ptr()), hInstance: hinst, ..Default::default() };
        assert!(RegisterClassW(&wc) != 0);
        let title: Vec<u16> = "tigirl-bg\0".encode_utf16().collect();
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR(cls.as_ptr()), PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE, 40, 40, 700, 260, None, None, None, None).expect("CreateWindow failed");
        let re_cls: Vec<u16> = "RICHEDIT50W\0".encode_utf16().collect();
        let rich = CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR(re_cls.as_ptr()), PCWSTR(std::ptr::null()),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_MULTILINE as u32), 0, 0, 660, 200,
            Some(&hwnd), None, None, None).expect("RichEdit failed");
        let _ = SetActiveWindow(hwnd);
        let _ = SetFocus(rich);
        pump();
        eprintln!("[bg] window ready, focus_rich={}", GetFocus().0 == rich.0);

        let tm: ITfThreadMgr = CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER).unwrap();
        let tid = tm.Activate().unwrap();
        let iid_doc = <ITfDocumentMgr as Interface>::IID;
        let mut pv: *mut core::ffi::c_void = std::ptr::null_mut();
        let hr = SendMessageW(rich, 0x043C, WPARAM(&iid_doc as *const _ as usize), LPARAM(&mut pv as *mut _ as isize));
        if hr.0 == 0 || pv.is_null() { eprintln!("[bg] EM_GETOLEINTERFACE failed hr={:X}", hr.0 as u32); return; }
        let doc: ITfDocumentMgr = std::mem::transmute(pv);
        let _ = tm.SetFocus(&doc);
        pump();
        let profiles: ITfInputProcessorProfiles = CoCreateInstance(&CLSID_TF_PROFILES, None, CLSCTX_INPROC_SERVER).unwrap();
        match profiles.ActivateLanguageProfile(&CLSID_TIGIRL, LANG_ZH_CN, &PROFILE_TIGIRL) {
            Ok(()) => eprintln!("[bg] ActivateLanguageProfile(Tigirl) OK tid={tid}"),
            Err(e) => eprintln!("[bg] ActivateLanguageProfile failed {e:?}"),
        }
        let _ = tm.SetFocus(&doc);
        pump();
        eprintln!("[bg] context GetTop={} IsThreadFocus={:?}", doc.GetTop().is_ok(), tm.IsThreadFocus().map(|b| b.as_bool()));
        let km: ITfKeystrokeMgr = match tm.cast() { Ok(v) => v, Err(e) => { eprintln!("[bg] QI ITfKeystrokeMgr failed {e:?}"); return; } };

        let mut out = std::fs::File::create(&out_path).expect("create out failed");
        writeln!(out, "idx\tok\texpected\tgot\tms").unwrap();
        let (mut pass, mut total) = (0usize, 0usize);
        for (i, line) in lines.iter().enumerate() {
            let Some((text, code)) = line.split_once('\t') else { continue };
            total += 1;
            rich_clear(rich);
            pump();
            let t0 = std::time::Instant::now();
            let mut eaten_total = 0usize;
            for &ch in code.as_bytes() {
                let Some(vk) = vk_of(ch) else { continue };
                let lp = key_lp(vk);
                if km.TestKeyDown(WPARAM(vk.0 as usize), lp).unwrap_or_default().as_bool() {
                    if km.KeyDown(WPARAM(vk.0 as usize), lp).is_ok() { eaten_total += 1; }
                }
                pump();
                if per_key_ms > 0 { std::thread::sleep(std::time::Duration::from_millis(per_key_ms)); }
            }
            let lp = key_lp(VK_SPACE);
            let _ = km.TestKeyDown(WPARAM(VK_SPACE.0 as usize), lp);
            let _ = km.KeyDown(WPARAM(VK_SPACE.0 as usize), lp);
            pump();
            std::thread::sleep(std::time::Duration::from_millis(5));
            let got = clean(&rich_get_text(rich));
            let ok = got == text;
            if ok { pass += 1; }
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            writeln!(out, "{}\t{}\t{}\t{}\t{:.1}", i, if ok { 1 } else { 0 }, text, got, ms).unwrap();
            if i == 0 { eprintln!("[bg] idx0 eaten {eaten_total}/{} got={:?}", code.len(), got); }
            if (i + 1) % 50 == 0 { eprintln!("[bg] progress {}/{total} pass {pass}", i + 1); out.flush().ok(); }
        }
        out.flush().ok();
        eprintln!("=== [bg] done: {pass}/{total} = {:.3}% ===", pass as f64 / total.max(1) as f64 * 100.0);
        let _ = tm.Deactivate();
        let _ = DestroyWindow(rich);
        let _ = DestroyWindow(hwnd);
    }
}