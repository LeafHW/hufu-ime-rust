//! TSF 驱动虎娘整句基准宿主（2026-10-18，路线 A→P1 合体）。
//!
//! 宿主 = 屏角小窗 + RichEdit50W。TSF 初始化：ThreadMgr+Activate →
//! EM_GETOLEINTERFACE 拿文档 → ITfInputProcessorProfiles::ActivateLanguageProfile
//! （**线程维度**，不动系统全局 IME）→ SetFocus(文档)。
//! 喂键双模式：
//!   km        —— ITfKeystrokeMgr::KeyDown 程序化喂键（不打扰用户；若 TIP 挂了
//!                前景 sink 即通）
//!   sendinput —— SetForegroundWindow 借前台 + SendInput 真键入（走 CUAS/真实
//!                管线，最接近用户实打；跑批期间借用前台）
//! 提交文本从 RichEdit 读回（EM_GETTEXTEX）逐句比对。
//!
//! 输入每行：`原文\t全码`（小写字母）。用法：
//!   tigirl-tsf-bench <pairs.tsv> <out.csv> [maxN] [km|sendinput] [msPerKey]

use std::io::{BufWriter, Write};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::ProcessStatus::EnumProcessModules;
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::TextServices::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLSID_TIGIRL: GUID = GUID::from_u128(0x69cb1b2f_cde7_43a2_96c9_ebf642e780eb);
const PROFILE_TIGIRL: GUID = GUID::from_u128(0x43201c7b_f615_469d_9d54_906d9270975e);
const CLSID_TF_PROFILES: GUID = GUID::from_u128(0x33c53a50_f456_4884_b049_85fd643ecfed);
const LANG_ZH_CN: u16 = 0x0804;

#[repr(C)]
struct GETTEXTEX {
    cb: u32,
    flags: u32,
    codepage: u32,
    lpDefaultChar: *mut u16,
    lpUsedDefChar: *mut u16,
}

unsafe fn rich_get_text(hwnd: HWND) -> String {
    const GT_DEFAULT: u32 = 0;
    let mut gt = GETTEXTEX {
        cb: 1 << 22,
        flags: GT_DEFAULT,
        codepage: 1200,
        lpDefaultChar: std::ptr::null_mut(),
        lpUsedDefChar: std::ptr::null_mut(),
    };
    let mut buf: Vec<u16> = vec![0; (1 << 22) as usize / 2];
    let r = SendMessageW(
        hwnd,
        0x045E, // EM_GETTEXTEX
        WPARAM(&mut gt as *mut _ as usize),
        LPARAM(buf.as_mut_ptr() as isize),
    );
    let n = r.0.max(0) as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

unsafe fn rich_clear(hwnd: HWND) {
    SendMessageW(hwnd, 0x00B1 /*EM_SETSEL*/, WPARAM(0), LPARAM(-1));
    let empty: [u16; 1] = [0];
    SendMessageW(
        hwnd,
        0x00C2, // EM_REPLACESEL
        WPARAM(0),
        LPARAM(empty.as_ptr() as isize),
    );
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

unsafe fn send_key(vk: VIRTUAL_KEY, up: bool) {
    let mut inputs = [INPUT::default(); 1];
    let sc = MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC_EX) as u16;
    inputs[0].r#type = INPUT_KEYBOARD;
    inputs[0].Anonymous.ki = KEYBDINPUT {
        wVk: vk,
        wScan: sc,
        dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
        time: 0,
        dwExtraInfo: 0,
    };
    let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
}

// post 模式：把键投递到自己的 RichEdit 窗口（不需要前台，可多进程并行后台跑）。
// RichEdit 收到 WM_KEYDOWN 会走自己的消息处理 → TranslateMessage/TSF KeystrokeMgr。
unsafe fn post_key(hwnd: HWND, vk: VIRTUAL_KEY, up: bool) {
    let sc = (MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC_EX) as u32) & 0xFF;
    let mut lp: u32 = 1 | (sc << 16);
    if up {
        lp |= (1 << 30) | (1 << 31);
    }
    let _ = PostMessageW(
        hwnd,
        if up { WM_KEYUP } else { WM_KEYDOWN },
        WPARAM(vk.0 as usize),
        LPARAM(lp as isize),
    );
}

/// 同步投递（SendMessage）：某些 TSF 实现只对同步按键生效。
unsafe fn send_msg_key(hwnd: HWND, vk: VIRTUAL_KEY, up: bool) {
    let sc = (MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC_EX) as u32) & 0xFF;
    let mut lp: u32 = 1 | (sc << 16);
    if up {
        lp |= (1 << 30) | (1 << 31);
    }
    SendMessageW(
        hwnd,
        if up { WM_KEYUP } else { WM_KEYDOWN },
        WPARAM(vk.0 as usize),
        LPARAM(lp as isize),
    );
}

/// km2：先 TestKeyDown 再 KeyDown（应用标准用法）。
unsafe fn km2_key(km: &ITfKeystrokeMgr, vk: VIRTUAL_KEY) {
    let sc = (MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC_EX) as u32) & 0xFF;
    let lp = LPARAM((1u32 | (sc << 16)) as isize);
    if km.TestKeyDown(WPARAM(vk.0 as usize), lp).unwrap_or_default().as_bool() {
        let _ = km.KeyDown(WPARAM(vk.0 as usize), lp);
    }
}

/// 廉价读尾部：GetWindowTextW 固定缓冲，只取开头 512 字（句长 < 100 字足够）。
unsafe fn tail_is_ascii_letters(rich: HWND, want_n: usize) -> usize {
    let mut buf = [0u16; 512];
    let n = GetWindowTextW(rich, &mut buf);
    let s = String::from_utf16_lossy(&buf[..(n as usize).min(buf.len())]);
    let k = s
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphabetic())
        .count();
    if k >= want_n { k } else { 0 }
}

/// 背压节流：发键后轮询，直到虎娘吃下这串字母（尾部 ASCII 消失）或超预算。
/// 返回是否在预算内被吃掉——这就是「虎娘的真实吞吐」，不做就丢键。
unsafe fn wait_consumed(rich: HWND, budget_ms: u64) -> bool {
    let t0 = std::time::Instant::now();
    let mut appeared = false;
    loop {
        pump();
        if tail_is_ascii_letters(rich, 1) > 0 {
            appeared = true;
        } else if appeared {
            return true;
        }
        if (t0.elapsed().as_millis() as u64) >= budget_ms {
            // 一直没出现字母 = IME 立即吃掉；出现后没消失 = 真的卡住
            return !appeared;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// 取当前文本长度（固定 512 字缓冲，句长足够）——用于自适应节拍判断按键是否被吃下。
unsafe fn text_len(rich: HWND) -> usize {
    let mut buf = [0u16; 512];
    GetWindowTextW(rich, &mut buf) as usize
}

/// 自适应节拍：发键后等文本发生变化（组段推进/上屏），最多 budget_ms。
/// 比固定 sleep 更稳：不吃够就绝不发下一个键（2026-10-21 实测固定 8ms 会丢键）。
unsafe fn wait_advance(rich: HWND, before: usize, budget_ms: u64) -> bool {
    let t0 = std::time::Instant::now();
    loop {
        pump();
        if text_len(rich) != before {
            return true;
        }
        if (t0.elapsed().as_millis() as u64) >= budget_ms {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

unsafe extern "system" fn host_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control() && !c.is_whitespace()).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: tigirl-tsf-bench <pairs.tsv> <out.csv> [maxN] [km|sendinput] [msPerKey]");
        std::process::exit(2);
    }
    let path = &args[1];
    let out_path = &args[2];
    let max_n: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
    let mode = args.get(4).map(|s| s.as_str()).unwrap_or("sendinput");
    let ms_per_key: u64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(3);

    let lines: Vec<String> = std::fs::read_to_string(path)
        .expect("读语料失败")
        .lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .filter(|l| !l.is_empty())
        .take(max_n)
        .collect();
    eprintln!("载入 {} 句 · 模式 {} · {}ms/键", lines.len(), mode, ms_per_key);

    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).unwrap();

        // ── 宿主窗口（屏角小窗，sendinput 模式需要真实前台）+ RichEdit50W ──
        let _lib = LoadLibraryW(w!("msftedit.dll"));
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let cls: Vec<u16> = "TigirlBenchHost\0".encode_utf16().collect();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(host_wndproc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hInstance: hinst,
            ..Default::default()
        };
        let atom = RegisterClassW(&wc);
        assert!(atom != 0, "RegisterClassW 失败");
        let title: Vec<u16> = "tigirl-bench\0".encode_utf16().collect();
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(cls.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            8,
            8,
            700,
            300,
            HWND(std::ptr::null_mut()),
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
            660,
            240,
            Some(&hwnd),
            None,
            None,
            None,
        )
        .expect("RichEdit 创建失败");
        pump();

        // ── TSF：ThreadMgr + Activate ──
        let tm: ITfThreadMgr =
            CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER).unwrap();
        let _tid = tm.Activate().unwrap();

        // RichEdit 文档
        let mut doc: Option<ITfDocumentMgr> = None;
        let iid_doc = <ITfDocumentMgr as Interface>::IID;
        let mut pv: *mut core::ffi::c_void = std::ptr::null_mut();
        let hr = SendMessageW(
            rich,
            0x043C, // EM_GETOLEINTERFACE
            WPARAM(&iid_doc as *const _ as usize),
            LPARAM(&mut pv as *mut _ as isize),
        );
        if hr.0 != 0 && !pv.is_null() {
            doc = Some(std::mem::transmute(pv));
            eprintln!("[EM_GETOLEINTERFACE] 拿到 ITfDocumentMgr ✓");
        } else {
            eprintln!("[EM_GETOLEINTERFACE] 失败 hr={:X}", hr.0 as u32);
        }

        // ── 线程维度激活虎娘（ActivateLanguageProfile=per-thread，不动全局）──
        let profiles: ITfInputProcessorProfiles =
            CoCreateInstance(&CLSID_TF_PROFILES, None, CLSCTX_INPROC_SERVER).unwrap();
        profiles
            .ActivateLanguageProfile(&CLSID_TIGIRL, LANG_ZH_CN, &PROFILE_TIGIRL)
            .expect("ActivateLanguageProfile(虎娘) 失败");
        eprintln!("[ActivateLanguageProfile] 虎娘 ✓（线程维度）");

        // 焦点文档（TIP 激活后再设，触发 OnSetFocus）
        if let Some(d) = &doc {
            let _ = tm.SetFocus(d);
            eprintln!("[SetFocus] 文档焦点 ✓");
        }

        // 诊断：模块加载 + 线程焦点
        {
            let mut hmods = [HMODULE(std::ptr::null_mut()); 1024];
            let mut cb = 0u32;
            let _ = EnumProcessModules(GetCurrentProcess(), hmods.as_mut_ptr(), 8192, &mut cb);
            let n = (cb / 8) as usize;
            let mut tigirl = false;
            for i in 0..n.min(1024) {
                let mut name = [0u16; 260];
                let len = GetModuleFileNameW(hmods[i], &mut name);
                let s = String::from_utf16_lossy(&name[..len as usize]);
                if s.to_lowercase().contains("tigirl") {
                    tigirl = true;
                }
            }
            eprintln!("[modules] 虎娘DLL加载={}", tigirl);
            match tm.IsThreadFocus() {
                Ok(b) => eprintln!("[IsThreadFocus] = {}", b.as_bool()),
                Err(e) => eprintln!("[IsThreadFocus] err {:?}", e),
            }
        }

        let km: ITfKeystrokeMgr = tm.cast().unwrap();
        let use_sendinput = mode == "sendinput" || mode == "sendinput-bg";
        let use_post = mode == "post";
        let use_sendmsg = mode == "send";
        let use_km2 = mode == "km2";

        if use_sendinput {
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(rich);
            std::thread::sleep(std::time::Duration::from_millis(150));
            pump();
            // 安静模式：窗口挪到屏幕外 + 最小化，尽量不打扰人（键仍进本窗口）
            if std::env::var("TIGIRL_QUIET").map(|v| v == "1").unwrap_or(false) {
                // 只挪到屏幕外，不最小化（最小化会丢焦点/丢 TIP，实测 0%）
                let _ = SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    -32000,
                    -32000,
                    240,
                    120,
                    SWP_NOACTIVATE,
                );
                pump();
            }
            eprintln!(
                "[sendinput] 前台={} 焦点rich={}",
                GetForegroundWindow().0 == hwnd.0,
                GetFocus().0 == rich.0
            );
        } else {
            // km/post/send 模式：不抢前台，但窗口焦点与 TSF 文档焦点要给上，
            // 否则 TIP 的键事件 sink 拿不到上下文（2026-10-21 实测补）。
            let _ = SetActiveWindow(hwnd);
            let _ = SetFocus(rich);
            pump();
            if let Some(d) = &doc {
                let _ = tm.SetFocus(d);
            }
            std::thread::sleep(std::time::Duration::from_millis(120));
            pump();
            eprintln!(
                "[bg] 前台={} 焦点rich={}",
                GetForegroundWindow().0 == hwnd.0,
                GetFocus().0 == rich.0
            );
        }

        // ── 冒烟：喂 nihao ──
        {
            for &ch in b"nihao" {
                let vk = vk_of(ch).unwrap();
                if use_sendinput {
                    let before_len = text_len(rich);
                    send_key(vk, false);
                    send_key(vk, true);
                    if std::env::var("TIGIRL_ADAPTIVE").map(|v| v == "1").unwrap_or(false) {
                        wait_advance(rich, before_len, 200);
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(ms_per_key));
                    }
                } else if use_post {
                    post_key(rich, vk, false);
                    post_key(rich, vk, true);
                    std::thread::sleep(std::time::Duration::from_millis(ms_per_key));
                } else if use_sendmsg {
                    send_msg_key(rich, vk, false);
                    send_msg_key(rich, vk, true);
                    std::thread::sleep(std::time::Duration::from_millis(ms_per_key));
                } else if use_km2 {
                    km2_key(&km, vk);
                } else {
                    let lp = LPARAM(((((MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC_EX) as u32 & 0xFF) << 16)
                        | 1 | (1 << 30) | (1u32 << 31)) & 0xFFFF_FFFF) as isize);
                    let _ = km.KeyDown(WPARAM(vk.0 as usize), lp);
                }
                pump();
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            pump();
            let t = rich_get_text(rich);
            eprintln!("[smoke] nihao → {:?}", t.chars().take(30).collect::<String>());
            rich_clear(rich);
            pump();
        }

        let mut out = BufWriter::new(std::fs::File::create(out_path).expect("建输出失败"));
        writeln!(out, "idx\tok\texpected\tgot\tms").unwrap();
        let mut pass = 0usize;
        let mut total = 0usize;
        let mut broken = 0usize;
        let sw = std::time::Instant::now();

        for (i, line) in lines.iter().enumerate() {
            let (text, code) = match line.split_once('\t') {
                Some(p) => p,
                None => continue,
            };
            total += 1;
            if use_sendinput {
                // 失焦自愈：跑批期间用户可能点了别处；抢回前台与 RichEdit 焦点
                if GetForegroundWindow().0 != hwnd.0 {
                    let _ = SetForegroundWindow(hwnd);
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
                let _ = SetFocus(rich);
            }
            rich_clear(rich);
            pump();
            let t_sent = std::time::Instant::now();

            let fuse_on = std::env::var("TIGIRL_FUSE").map(|v| v == "1").unwrap_or(false);
            let mut ascii_streak = 0u32;
            'keyloop: for &ch in code.as_bytes() {
                let Some(vk) = vk_of(ch) else { continue };
                if use_sendinput {
                    // 熔断：文本尾部连续 ASCII 字母 = TIP 断流（组段卡死/失焦），剩余键无意义
                    // 仅在 TIGIRL_FUSE=1 时才读文本——否则每键 4MB 读回是主要开销。
                    if fuse_on {
                        let cur = rich_get_text(rich);
                        let tail_ascii = cur
                            .chars()
                            .rev()
                            .take_while(|c| c.is_ascii_alphabetic())
                            .count() as u32;
                        ascii_streak = if tail_ascii > 0 { tail_ascii } else { 0 };
                    }
                    // 熔断默认关：整句模式下组段里本来就会显示原码字母，
                    // 尾部 ASCII 是正常现象（2026-10-21 实测：开熔断误杀 15/20，
                    // 关掉同一句 100% 通过）。需要时 TIGIRL_FUSE=1 打开。
                    if ascii_streak >= 6
                        && std::env::var("TIGIRL_FUSE").map(|v| v == "1").unwrap_or(false)
                    {
                        eprintln!("[fuse] idx{i} TIP 断流（尾部字母×{ascii_streak}），复活流程+中止本句");
                        broken += 1;
                        // 复活：ESC 冲销 + 线程维度重激活虎娘 + 文档焦点重设
                        send_key(VK_ESCAPE, false);
                        send_key(VK_ESCAPE, true);
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        pump();
                        let _ = profiles.ActivateLanguageProfile(&CLSID_TIGIRL, LANG_ZH_CN, &PROFILE_TIGIRL);
                        if let Some(d) = &doc {
                            let _ = tm.SetFocus(d);
                        }
                        let _ = SetFocus(rich);
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        pump();
                        break 'keyloop;
                    }
                    let before_len = text_len(rich);
                    send_key(vk, false);
                    send_key(vk, true);
                    if std::env::var("TIGIRL_ADAPTIVE").map(|v| v == "1").unwrap_or(false) {
                        wait_advance(rich, before_len, 200);
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(ms_per_key));
                    }
                } else if use_post {
                    post_key(rich, vk, false);
                    post_key(rich, vk, true);
                    std::thread::sleep(std::time::Duration::from_millis(ms_per_key));
                } else if use_sendmsg {
                    send_msg_key(rich, vk, false);
                    send_msg_key(rich, vk, true);
                    std::thread::sleep(std::time::Duration::from_millis(ms_per_key));
                } else if use_km2 {
                    km2_key(&km, vk);
                } else {
                    let lp = LPARAM(((((MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC_EX) as u32 & 0xFF) << 16)
                        | 1 | (1 << 30) | (1u32 << 31)) & 0xFFFF_FFFF) as isize);
                    let _ = km.KeyDown(WPARAM(vk.0 as usize), lp);
                }
                pump();
            }
            // 结句：space 兜底提交（先给最后一段一点收拾时间）
            if use_sendinput {
                std::thread::sleep(std::time::Duration::from_millis(80));
                send_key(VK_SPACE, false);
                send_key(VK_SPACE, true);
                std::thread::sleep(std::time::Duration::from_millis(10));
            } else if use_post {
                post_key(rich, VK_SPACE, false);
                post_key(rich, VK_SPACE, true);
                std::thread::sleep(std::time::Duration::from_millis(10));
            } else if use_sendmsg {
                send_msg_key(rich, VK_SPACE, false);
                send_msg_key(rich, VK_SPACE, true);
                std::thread::sleep(std::time::Duration::from_millis(10));
            } else if use_km2 {
                km2_key(&km, VK_SPACE);
            } else {
                let lp = LPARAM(((((MapVirtualKeyW(VK_SPACE.0 as u32, MAPVK_VK_TO_VSC_EX) as u32 & 0xFF) << 16)
                    | 1 | (1 << 30) | (1u32 << 31)) & 0xFFFF_FFFF) as isize);
                let _ = km.KeyDown(WPARAM(VK_SPACE.0 as usize), lp);
            }
            pump();
            std::thread::sleep(std::time::Duration::from_millis(2));

            let got = clean(&rich_get_text(rich));
            let ok = got == text;
            if ok {
                pass += 1;
            }
            let sent_ms = t_sent.elapsed().as_secs_f64() * 1000.0;
            writeln!(out, "{}\t{}\t{}\t{}\t{:.1}", i, if ok { 1 } else { 0 }, text, got, sent_ms).unwrap();
            if (i + 1) % 100 == 0 {
                eprintln!("进度 {}/{} 通过 {} 用时 {}s", i + 1, total, pass, sw.elapsed().as_secs());
                out.flush().ok();
            }
        }

        out.flush().ok();
        eprintln!(
            "═══ 完成：{}/{} = {:.3}%（断流句 {broken}）═══",
            pass,
            total,
            pass as f64 / total as f64 * 100.0
        );

        let _ = tm.Deactivate();
        let _ = DestroyWindow(rich);
        let _ = DestroyWindow(hwnd);
        let _ = GetCurrentThreadId();
    }
}
