//! {加词}/{加权} 弹窗：`/jc {加词}`、`/jq {加权}` 选中后弹小窗。
//! 加词=三输入框（词 / 编码 / 选重位）+ 候选顺序实时预览；加权=两框
//!（词 / 权重，缺省 1000；编码由 server 反查最优码）。预览做成候选窗
//! 样式：每项「序号+词」分色（label/text 色）流式排列，新词高亮底块；
//! 配色与字体套用当前皮肤（colors + layout.font_*）。每次键入
//!（EN_UPDATE=0x400）立即刷新；预览变多窗口自适应加高，确定/取消恒
//! 在底部。窗口独立线程跑消息循环不阻塞 TSF。

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, FW_BOLD, FW_NORMAL,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

const ID_WORD: i32 = 101;
const ID_CODE: i32 = 102;
const ID_POS: i32 = 103;
/// 【占位符 2026-10-08】加词框「占位符补位」勾选框（选重位超出
/// 现有候选数时用带圈数字 ③④… 补空位，词恰落第 N 位）。
const ID_PH: i32 = 104;
const ID_OK: i32 = 1001;
const ID_CANCEL: i32 = 1002;

// 预览动态项 id 段（CTLCOLOR 按段分色；GWLP_USERDATA 存文字 COLORREF）
const ID_CUR_BASE: i32 = 2000; // 现有项
const ID_AFT_BASE: i32 = 2200; // 加入后普通项
const ID_AFT_PH: i32 = 2400; // 加入后占位项（③④… 用标签色压暗）
const ID_AFT_NEW: i32 = 2600; // 加入后新词高亮项
const ID_EN_UPDATE: u32 = 0x400; // EN_UPDATE（0x200 是 EN_KILLFOCUS！）

const CLASS: PCWSTR = w!("HuFuAddWord");
// 【整体加大 20% 2026-10-08】原布局 396/16/364/284 按 1.2 倍放大
//（用户反馈加词框偏小，对齐虎爪弹窗观感）——原值注释在侧便于对照。
const WIN_W: i32 = 475; // 396
const PV_X: i32 = 19; // 16
const PV_W: i32 = 437; // 364 预览排版宽（候选流）
const EDIT_W: i32 = 341; // 284 输入框宽（收窄，不再通栏）
/// 行几何（同批 1.2 倍）：首行 y、行距、标签高、编辑框下沉/高。
const ROW_Y0: i32 = 17; // 14
const ROW_PITCH: i32 = 74; // 62
const LBL_W: i32 = 396; // 330
const LBL_H: i32 = 29; // 24
const EDIT_DY: i32 = 31; // 26
const EDIT_H: i32 = 40; // 33
/// 三行编辑框之后的占位符勾选行 y（ROW_Y0+3*ROW_PITCH+EDIT_DY+EDIT_H
/// 之上留 6px）与预览表头/预览起始 y。
const PH_ROW_Y: i32 = 242;
const PV_TITLE_Y: i32 = 276; // 原 202
const PV_START_Y: i32 = 306; // 原 228
/// 按钮几何（1.2 倍）。
const BTN_OK_X: i32 = 259; // 216
const BTN_CANCEL_X: i32 = 373; // 311
const BTN_W: i32 = 98; // 82
const BTN_H: i32 = 41; // 34

/// 带圈数字（占位符）：1-20 → ①…⑳（U+2460+），21-35 → ㉑…㉟，
/// 36-50 → ㊱…㊿；越界 None（选重位超 50 不补位）。
fn circled_num(n: usize) -> Option<String> {
    let c = if (1..=20).contains(&n) {
        char::from_u32(0x2460 + n as u32 - 1)
    } else if (21..=35).contains(&n) {
        char::from_u32(0x3251 + n as u32 - 21)
    } else if (36..=50).contains(&n) {
        char::from_u32(0x32B1 + n as u32 - 36)
    } else {
        None
    };
    c.map(|ch| ch.to_string())
}

/// 皮肤数据（首次弹窗拉取；失败回退深色系）
struct Skin {
    bg: u32,
    text: u32,
    label: u32,
    // 【死字段删除】hilite/hilite_label/hilite_bg 三字段只写不读——
    // 预览着色已收敛为 NEW_RED（新词）/text（其余）/label（表头），
    // 高亮底块不再参与弹窗渲染。
    font_face: Vec<u16>, // UTF-16 含 null
    font_pt: i32,
    label_pt: i32,
    cand_spacing: i32,
}
static SKIN: std::sync::Mutex<Option<Skin>> = std::sync::Mutex::new(None);
static BG_BRUSH: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
// 【三十四修·死代码删除】HILITE_BRUSH（建后无消费，wndproc 只用
// BG_BRUSH）已删。
static FONT_MAIN: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
static FONT_LABEL: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
/// 新词专用：加粗+下划线（红色由 CTLCOLOR 分段着色）
static FONT_NEW: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
/// 新词红（COLORREF 0x00BBGGRR：R255 G80 B80）
const NEW_RED: u32 = 0x50_50_FF;
/// 预览动态控件（刷新时销毁重建）
static ITEMS: std::sync::Mutex<Vec<isize>> = std::sync::Mutex::new(Vec::new());

fn parse_hex(s: &str) -> Option<u32> {
    let t = s.trim().trim_start_matches('#');
    if t.len() < 6 {
        return None;
    }
    let r = u32::from_str_radix(&t[0..2], 16).ok()?;
    let g = u32::from_str_radix(&t[2..4], 16).ok()?;
    let b = u32::from_str_radix(&t[4..6], 16).ok()?;
    Some(b << 16 | g << 8 | r) // COLORREF 0x00BBGGRR
}

fn utf16z(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

/// 深色向白混合（皮肤底色太黑时提亮窗口底，避免大片死黑）。
fn lighten(c: u32, k: f32) -> u32 {
    let ch = |v: u32| -> u32 { v + ((255 - v) as f32 * k) as u32 };
    let r = ch(c & 0xFF);
    let g = ch((c >> 8) & 0xFF);
    let b = ch((c >> 16) & 0xFF);
    b << 16 | g << 8 | r
}

// ── 【占位符记忆 2026-10-09】勾选态首选走 server 管道（server 落
// 数据\addword-ph.json），HKCU 只作管道不通时的兜底——NTQQ 渲染进程
// 跑在低完整性沙箱里写 HKCU 静默失败（用户实测「QQ 里勾了不记」的
// 根因），而管道在沙箱宿主里打字本就必通。回读同款双路。──
fn ph_remember(on: bool) {
    if crate::ipc::call(&serde_json::json!({"op": "aw_ph_set", "on": on})).is_some() {
        return;
    }
    ph_remember_hkcu(on);
}

fn ph_remember_hkcu(on: bool) {
    #[link(name = "advapi32")]
    extern "system" {
        fn RegCreateKeyExW(
            key: isize,
            subkey: *const u16,
            reserved: u32,
            class: *const u16,
            options: u32,
            desired: u32,
            sa: *const core::ffi::c_void,
            result: *mut isize,
            dispos: *mut u32,
        ) -> i32;
        fn RegSetValueExW(
            key: isize,
            name: *const u16,
            reserved: u32,
            vtype: u32,
            data: *const u8,
            cb: u32,
        ) -> i32;
        fn RegCloseKey(key: isize) -> i32;
    }
    unsafe {
        let sub: Vec<u16> = "Software\\HuFu".encode_utf16().chain([0]).collect();
        let name: Vec<u16> = "addword_ph".encode_utf16().chain([0]).collect();
        let mut hkey: isize = 0;
        if RegCreateKeyExW(
            -2147483647i64 as isize, /* HKEY_CURRENT_USER */
            sub.as_ptr(),
            0,
            std::ptr::null(),
            0,
            0x2002, /* KEY_SET_VALUE */
            std::ptr::null(),
            &mut hkey,
            std::ptr::null_mut(),
        ) == 0
        {
            let v: u32 = if on { 1 } else { 0 };
            RegSetValueExW(
                hkey,
                name.as_ptr(),
                0,
                4, /* REG_DWORD */
                &v as *const u32 as *const u8,
                4,
            );
            RegCloseKey(hkey);
        }
    }
}

fn ph_recall() -> bool {
    if let Some(r) = crate::ipc::call(&serde_json::json!({"op": "aw_ph_get"})) {
        return r.get("on").and_then(|v| v.as_bool()).unwrap_or(false);
    }
    ph_recall_hkcu()
}

fn ph_recall_hkcu() -> bool {
    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            key: isize,
            subkey: *const u16,
            reserved: u32,
            desired: u32,
            result: *mut isize,
        ) -> i32;
        fn RegQueryValueExW(
            key: isize,
            name: *const u16,
            reserved: *mut u32,
            vtype: *mut u32,
            data: *mut u8,
            cb: *mut u32,
        ) -> i32;
        fn RegCloseKey(key: isize) -> i32;
    }
    unsafe {
        let sub: Vec<u16> = "Software\\HuFu".encode_utf16().chain([0]).collect();
        let name: Vec<u16> = "addword_ph".encode_utf16().chain([0]).collect();
        let mut hkey: isize = 0;
        if RegOpenKeyExW(
            -2147483647i64 as isize, /* HKEY_CURRENT_USER */
            sub.as_ptr(),
            0,
            0x2001, /* KEY_QUERY_VALUE */
            &mut hkey,
        ) != 0
        {
            return false;
        }
        let mut v: u32 = 0;
        let mut cb: u32 = 4;
        let ok = RegQueryValueExW(
            hkey,
            name.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut v as *mut u32 as *mut u8,
            &mut cb,
        ) == 0
            && v == 1;
        RegCloseKey(hkey);
        ok
    }
}

/// 拉皮肤配色+字体（弹窗线程调用一次；失败回退深色+微软雅黑）。
fn load_skin() {
    let mut g = SKIN.lock().unwrap_or_else(|p| p.into_inner());
    if g.is_some() {
        return;
    }
    let sk = crate::ipc::call(&serde_json::json!({"op": "skin"}))
        .and_then(|v| v.get("skin").cloned())
        .unwrap_or(serde_json::Value::Null);
    let get_color = |k: &str| -> Option<u32> {
        sk.pointer(&format!("/colors/{k}"))
            .and_then(|x| x.as_str())
            .and_then(parse_hex)
    };
    let face = sk
        .pointer("/layout/font_face")
        .and_then(|x| x.as_str())
        .unwrap_or("Microsoft YaHei UI")
        .to_string();
    let font_pt = sk
        .pointer("/layout/font_point")
        .and_then(|x| x.as_f64())
        .unwrap_or(16.0) as i32;
    let label_pt = sk
        .pointer("/layout/label_font_point")
        .and_then(|x| x.as_f64())
        .unwrap_or(12.0) as i32;
    let cand_spacing = sk
        .pointer("/layout/candidate_spacing")
        .and_then(|x| x.as_f64())
        .unwrap_or(6.0) as i32;
    let s = Skin {
        // 窗口底色提亮 20%：候选窗小面积用原底色可以，整窗大面积
        // 直接用会死黑（用户反馈），向白混一档。
        bg: lighten(get_color("back_color").unwrap_or(0x22_2E_16_u32), 0.20),
        text: get_color("text_color")
            .or_else(|| get_color("candidate_text_color"))
            .unwrap_or(0xEC_E2_D7_u32),
        label: get_color("label_color")
            .or_else(|| get_color("comment_text_color"))
            .unwrap_or(0xB5_A6_8A),
        font_face: utf16z(&face),
        // 字号比皮肤候选窗大两档再 ×1.2（弹窗阅读距离远；用户两轮
        // 要求加大，2026-10-08 随窗口整体放大 20%）
        font_pt: (((font_pt + 6) as f32 * 1.2).round() as i32).max(20),
        label_pt: (((label_pt + 5) as f32 * 1.2).round() as i32).max(16),
        cand_spacing: cand_spacing.max(2),
    };
    unsafe {
        let face_ptr = PCWSTR(s.font_face.as_ptr());
        let mk_font = |pt: i32, weight: i32| {
            CreateFontW(
                -pt,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                0,
                0,
                CLEARTYPE_QUALITY.0 as u32,
                0,
                face_ptr,
            )
        };
        *FONT_MAIN.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(mk_font(s.font_pt, FW_NORMAL.0 as i32).0 as usize);
        *FONT_LABEL.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(mk_font(s.label_pt, FW_NORMAL.0 as i32).0 as usize);
        // 新词：加粗 + 下划线
        let bold_underline = CreateFontW(
            -s.font_pt,
            0,
            0,
            0,
            FW_BOLD.0 as i32,
            0,
            1, // underline
            0,
            DEFAULT_CHARSET.0 as u32,
            0,
            0,
            CLEARTYPE_QUALITY.0 as u32,
            0,
            PCWSTR(s.font_face.as_ptr()),
        );
        *FONT_NEW.lock().unwrap_or_else(|p| p.into_inner()) = Some(bold_underline.0 as usize);
        *BG_BRUSH.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(CreateSolidBrush(COLORREF(s.bg)).0 as usize);
    }
    *g = Some(s);
}

/// 窗模式：false=加词（三框+预览），true=加权（词+权重两框）。
/// 【/jq 加权 2026-09-06】加权=提升该词权重（server 反查最优码，
/// 写 用户词.txt 词行 weight 列；缺省 1000）。
static MODE_WEIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn is_weight_mode() -> bool {
    MODE_WEIGHT.load(std::sync::atomic::Ordering::Relaxed)
}

/// 弹出加词窗（非阻塞：新线程 + 消息循环）。
pub fn open() {
    MODE_WEIGHT.store(false, std::sync::atomic::Ordering::Relaxed);
    open_common();
}

/// 【右键调频菜单 2026-10-06】带预填弹出加词窗：候选窗右键「加词」
/// 带出 (词, 编码, 选重位)——与 {加词}/、jc 同窗口同提交链路
///（/api/user_word/add），仅初始值预填。窗口已开时不强填（消费即弃，
/// 防陈旧预填滞留到下次开窗）。
pub fn open_prefilled(word: &str, code: &str, pos: &str) {
    MODE_WEIGHT.store(false, std::sync::atomic::Ordering::Relaxed);
    *PREFILL.lock().unwrap_or_else(|p| p.into_inner()) = Some((
        word.to_string(),
        code.to_string(),
        pos.to_string(),
    ));
    open_common();
}

/// 右键「加词」预填（词, 编码, 选重位）——开窗线程消费即清。
static PREFILL: std::sync::Mutex<Option<(String, String, String)>> =
    std::sync::Mutex::new(None);

/// 弹出加权窗（/jq {加权} 触发；词+权重两框）。
pub fn open_weight() {
    MODE_WEIGHT.store(true, std::sync::atomic::Ordering::Relaxed);
    open_common();
}

/// 【小窗打开判定 2026-09-12】加词/加权窗存活且可见——主文档 sink
/// 的按键直通用（激活间隙防组段写进主文档，见 tsf::dispatch 头部）。
/// 复用 ADDWORD_HWND 登记句柄；异常清理态由 IsWindow 兜底。
pub fn is_open() -> bool {
    let g = ADDWORD_HWND.lock().unwrap_or_else(|p| p.into_inner());
    *g != 0 && unsafe { IsWindow(HWND(*g as *mut _)).as_bool() }
}

// 【三十四修·死代码删除】current_hwnd()（十二修无条件直通门后零调用）已删。

/// 小窗线程 id 登记（0=无）——五修：dispatch 直通门只挡非小窗线程。
static ADDWORD_TID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// 当前线程是否小窗线程——直通门放行判定（词框键由小窗线程自己的
/// sink 处理，主线程的才需要直通防残留）。
pub fn in_window_thread() -> bool {
    // 【三十六修】单次 load（旧实现两次 load 之间存在 TOCTOU——首读
    // 非零、次读已被另一线程改写/清零，判定失真）。
    let tid = ADDWORD_TID.load(std::sync::atomic::Ordering::Relaxed);
    tid != 0
        && unsafe {
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn GetCurrentThreadId() -> u32;
            }
            GetCurrentThreadId()
        } == tid
}

/// 【T4 v5】加词窗消息：回灌链两拍——AW_FLUSH=无段提交转正（起 30ms
/// 定时器）；AW_TAIL=补建尾巴段（再起 30ms）。
pub const AW_FLUSH_MSG: u32 = 0x8000 + 0x550;
pub const AW_TAIL_MSG: u32 = 0x8000 + 0x552;
/// 【T4 v3b】冲刷定时器 id——CUAS 物化边界是宿主泵周期而非编辑会话。
const AW_FLUSH_TIMER: usize = 0x551;
const AW_TAIL_TIMER: usize = 0x553;
/// 【词框实时编码 2026-10-07】词行标签控件 id（编码尾巴显示位——
/// EDIT 回退模式下用；RichEdit 模式编码内联显示在框内）。
pub const AW_LIVE_LABEL: i32 = 110;
/// 【RichEdit 2026-10-07】词框是否为 RichEdit（TSF 原生）——true 时
/// tsf.rs 走标准组段路径（编码内联显示/上屏即时）；false（创建失败
/// 回退 EDIT）走 v3c CUAS 特判路径。
static AW_RICH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 词框是否 RichEdit 模式。
pub fn aw_rich() -> bool {
    AW_RICH.load(std::sync::atomic::Ordering::Relaxed)
}

/// 【词框实时编码】把打字中的编码尾巴写到词行标签（raw 空则还原）。
/// STATIC 不发 EN_UPDATE，无回流。
pub fn aw_live_code(raw: &str) {
    let h = aw_hwnd();
    if h == 0 {
        return;
    }
    let base = if is_weight_mode() {
        "词（要加权的字或词）"
    } else {
        "词（要打出的内容）"
    };
    let owned;
    let text: &str = if raw.is_empty() {
        base
    } else {
        // 编码尾巴只显 ASCII 原始码
        let ascii: String = raw.chars().filter(|c| c.is_ascii()).collect();
        if ascii.is_empty() {
            return;
        }
        owned = format!("{base} · 编码 {ascii}");
        &owned
    };
    let v: Vec<u16> = text.encode_utf16().chain([0]).collect();
    unsafe {
        if let Ok(lbl) =
            GetDlgItem(windows::Win32::Foundation::HWND(h as *mut _), AW_LIVE_LABEL)
        {
            let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                lbl,
                PCWSTR(v.as_ptr()),
            );
        }
    }
}

/// 【T4 v3】活着的加词窗句柄（0=无）——通用查询（RichEdit 化后回灌
/// PostMessage 已停用）。
pub fn aw_hwnd() -> isize {
    let guard = ADDWORD_HWND.lock().unwrap_or_else(|p| p.into_inner());
    *guard
}

/// 【词框候选内嵌·标题栏 2026-09-12 八修】小窗线程的候选显示在小窗
/// 加词窗单例登记（0=无）：open_common 临界区内读写，消息循环
/// 结束清零。短锁使用，绝不跨消息循环持有。
static ADDWORD_HWND: std::sync::Mutex<isize> = std::sync::Mutex::new(0);

fn open_common() {
    std::thread::spawn(|| unsafe {
        // 【B1 修复 2026-09-13 三十四修】单例复查提前到线程起手（store
        // tid 与 TIP 自激活**之前**）：旧序 = store tid → 全套自激活 →
        // 临界区才发现窗口已在 → return——连按 /jc 每次 (a) B 线程
        // 覆盖 ADDWORD_TID 后悬空（真小窗线程 A 的键被直通门判为主
        // 线程，词框打不了中文）(b) 泄漏一套 TIP 激活+键 sink 不回收。
        // 新序：先查窗口在不在——在=前置复用直接退出（零登记零激活）。
        {
            let mut guard = ADDWORD_HWND.lock().unwrap_or_else(|p| p.into_inner());
            if *guard != 0 && IsWindow(HWND(*guard as *mut _)).as_bool() {
                let existing = HWND(*guard as *mut _);
                let _ = ShowWindow(existing, SW_SHOWNORMAL);
                let _ = SetForegroundWindow(existing);
                crate::tsf::trace("addword 窗口已在，前置复用（早退）");
                return;
            }
            if let Ok(existing) = FindWindowW(CLASS, None) {
                let mut pid: u32 = 0;
                let _ = GetWindowThreadProcessId(existing, Some(&mut pid));
                if pid == std::process::id() && !existing.0.is_null() {
                    *guard = existing.0 as isize;
                    let _ = ShowWindow(existing, SW_SHOWNORMAL);
                    let _ = SetForegroundWindow(existing);
                    crate::tsf::trace("addword 窗口已在（FindWindow 早退复用）");
                    return;
                }
            }
        }
        // 【三十六修·B1 残余竞态收口】tid 登记从这里（线程起手）移到
        crate::tsf::trace("addword: 打开 v3（自动编码跟踪+占位符提示+20%放大）");
        // 下方窗口登记临界区内：连按 /jc 两线程同过前置检查时，双线程
        // 都在此 store tid → 后 store 者覆盖真窗口线程的 tid（悬挂），
        // in_window_thread() 对真小窗线程失真 → 直通门误判（词框打
        // 不了中文）+ 输家 TIP 激活泄漏。tid 现与 *guard 登记同临界区
        // 同点写入——输家在任何早退路径都不再触碰 tid。
        // 【词框 TSF 化 2026-09-12 三修】线程 TSF 化的完整链：STA COM
        // → 显式 CoCreateInstance(ThreadMgr)（msctf 判定线程 TSF-
        // enabled 的标志=线程持有 ThreadMgr；只有 CoInitialize 不够，
        // 二修实锤键仍不进 sink）→ ActivateLanguageProfile（本线程
        // 当前输入法=HuFu）→ 而后创建的 EDIT 被 msctf 接管：获焦时
        // 关联 DocumentMgr + 激活 TIP（HuFuTs 实例化，Activate 在本
        // 线程跑，键 sink 装上）。ThreadMgr 保活到消息循环结束（变量
        // _tm 活着）；线程退出全部回收。
        // ThreadMgr/tid 写线程局部（tsf::set_thread_tm）——每线程各
        // 持自己的；进程级 Shared 只留首激活（主线程）锚不漂移。
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
        let _tm: windows::core::Result<windows::Win32::UI::TextServices::ITfThreadMgr> =
            windows::Win32::System::Com::CoCreateInstance(
                &windows::Win32::UI::TextServices::CLSID_TF_ThreadMgr,
                None,
                windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
            );
        if let Ok(tm) = &_tm {
            // tid=0 占位（下方四修会用 ThreadMgr::Activate 的真 tid 覆盖）
            crate::tsf::set_thread_tm(tm.clone(), 0);
        }
        let _ = (|| -> windows::core::Result<()> {
            // 【四修 2026-09-12】ActivateLanguageProfile 只是"选择"当前
            // 输入法——TIP 实例**不激活**（激活标记实锤：小窗线程 tid
            // 从未出现，键 sink 从未装，词框键入无 TIP 接收，字母直通）。
            // 焦点驱动的懒激活对该 EDIT 不发生（msctf 不接管非标准宿主
            // 线程的焦点），故显式自激活：
            //   ThreadMgr::Activate() → client id（本线程）
            //   CoCreateInstance(自家 CLSID) → ITfTextInputProcessor
            //   tip.Activate(tm, tid) → AdviseKeyEventSink 装上
            // 三修的线程局部化保证此激活不覆盖主线程锚（g.thread_mgr
            // 只认首激活；本线程走 THREAD_TM/THREAD_TID）。
            let profiles: windows::Win32::UI::TextServices::ITfInputProcessorProfiles =
                windows::Win32::System::Com::CoCreateInstance(
                    &windows::Win32::UI::TextServices::CLSID_TF_InputProcessorProfiles,
                    None,
                    windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
                )?;
            profiles.ActivateLanguageProfile(
                &crate::CLSID_HUFU_TSF,
                0x0804u16,
                &crate::com::PROFILE_GUID,
            )?;
            let tm = match _tm.as_ref() {
                Ok(t) => t.clone(),
                Err(e) => {
                    crate::tsf::trace(&format!("addword自激活: ThreadMgr缺失 {e:?}"));
                    return Err(e.clone());
                }
            };
            let tid = match tm.Activate() {
                Ok(t) => t,
                Err(e) => {
                    crate::tsf::trace(&format!("addword自激活: ThreadMgr.Activate失败 {e:?}"));
                    return Err(e);
                }
            };
            crate::tsf::set_thread_tm(tm.clone(), tid);
            let tip: windows::Win32::UI::TextServices::ITfTextInputProcessor =
                match windows::Win32::System::Com::CoCreateInstance(
                    &crate::CLSID_HUFU_TSF,
                    None,
                    windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
                ) {
                    Ok(t) => t,
                    Err(e) => {
                        crate::tsf::trace(&format!("addword自激活: CoCreate自家CLSID失败 {e:?}"));
                        return Err(e);
                    }
                };
            if let Err(e) = tip.Activate(&tm, tid) {
                crate::tsf::trace(&format!("addword自激活: tip.Activate失败 {e:?}"));
                return Err(e);
            }
            crate::tsf::trace(&format!(
                "addword: 小窗线程 TIP 已自激活 tid={tid}（词框键 sink 装上）"
            ));
            Ok(())
        })();
        crate::tsf::trace("addword open（线程已起）");
        // 【单例 2026-09-06】窗口已在（本进程）→ 前置复用，不再多开
        //（用户连按 /jq 会叠开多个同位窗口，只看得见第一个）
        // 【竞态闸门 2026-09-11】FindWindow(无)→CreateWindow 两步无锁：
        // 连按 /jq 两个线程同时查空 → 各建一窗（旧实现真实竞态）。
        // 临界区（ADDWORD_HWND 锁）内复查+建窗+登记；锁绝不跨消息
        // 循环持有。
        let mut guard = ADDWORD_HWND.lock().unwrap_or_else(|p| p.into_inner());
        if *guard != 0 && IsWindow(HWND(*guard as *mut _)).as_bool() {
            let existing = HWND(*guard as *mut _);
            let _ = ShowWindow(existing, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(existing);
            crate::tsf::trace("addword 窗口已在，前置复用");
            return;
        }
        if let Ok(existing) = FindWindowW(CLASS, None) {
            let mut pid: u32 = 0;
            let _ = GetWindowThreadProcessId(existing, Some(&mut pid));
            if pid == std::process::id() && !existing.0.is_null() {
                *guard = existing.0 as isize;
                let _ = ShowWindow(existing, SW_SHOWNORMAL);
                let _ = SetForegroundWindow(existing);
                crate::tsf::trace("addword 窗口已在（FindWindow 复用）");
                return;
            }
        }
        load_skin();
        let hmod = GetModuleHandleW(None).unwrap_or_default();
        let hinst = HINSTANCE(hmod.0);
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: windows::Win32::Graphics::Gdi::GetSysColorBrush(
                windows::Win32::Graphics::Gdi::COLOR_WINDOW,
            ),
            lpszClassName: CLASS,
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        // 【T4 修 2026-10-07】标题必须 NUL 终止——旧代码裸 collect() 无
        // 终止符，一直靠栈上恰逢 0 侥幸（插入探针挪了栈布局即现乱码尾）。
        let title: Vec<u16> = if is_weight_mode() {
            "虎符 · 加权".encode_utf16().chain([0]).collect()
        } else {
            "虎符 · 加词".encode_utf16().chain([0]).collect()
        };
        let h = outer_h(if is_weight_mode() { 240 } else { 360 }); // 200/300×1.2
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_DLGMODALFRAME,
            CLASS,
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            (sw - WIN_W) / 2,
            (sh - h) / 2,
            WIN_W,
            h,
            None,
            None,
            hinst,
            None,
        )
        .unwrap_or_default();
        if hwnd.0.is_null() {
            crate::tsf::trace("addword CreateWindow 失败（窗口未建）");
            return;
        }
        // 登记本窗口句柄（后续 open 前置复用）+ 本线程 id（五修：直通
        // 门判定用）——同一临界区同一写入点，B1 残余竞态收口（三十六
        // 修，见上方注释）。放锁再跑消息循环。
        *guard = hwnd.0 as isize;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThreadId() -> u32;
        }
        let my_tid = GetCurrentThreadId();
        ADDWORD_TID.store(my_tid, std::sync::atomic::Ordering::Relaxed);
        drop(guard);
        let _ = ShowWindow(hwnd, SW_SHOW);
        // 【六修 2026-09-12】AttachThreadInput 抢前台——此前裸调
        // SetForegroundWindow 被系统前台锁定静默拒绝（跨线程输入队
        // 列：小窗线程≠前台线程），trace 实锤小窗从未成为前台：键全
        // 被 QQ 主线程 sink 吃掉、组段建在主文档 (1085,793)（「原光
        // 标残留+词框只有字母」的真正根因）。AttachThreadInput 共享
        // 前台线程的输入状态后 SetForegroundWindow/SetFocus 才有效；
        // 抢完立刻脱离 attach（短窗，不留输入耦合）。附带诊断：前台
        // 抢夺成败打 trace。
        #[link(name = "user32")]
        unsafe extern "system" {
            fn AttachThreadInput(idattach: u32, idattachto: u32, fattach: i32) -> i32;
        }
        let fg = GetForegroundWindow();
        let fg_tid = if fg.0.is_null() {
            0
        } else {
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(fg, Some(&mut pid))
        };
        let my_tid = GetCurrentThreadId();
        let mut fg_ok = SetForegroundWindow(hwnd).as_bool();
        if !fg_ok && fg_tid != 0 && fg_tid != my_tid {
            let at = AttachThreadInput(my_tid, fg_tid, 1);
            fg_ok = SetForegroundWindow(hwnd).as_bool();
            // 焦点直接给词框（attach 态下跨线程 SetFocus 有效）
            if let Some(fe) = FIRST_EDIT
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .map(|h| HWND(h as *mut _))
            {
                let _ = SetFocus(fe);
            }
            let _ = AttachThreadInput(my_tid, fg_tid, 0);
            crate::tsf::trace(&format!(
                "addword: attach={at} 前台抢夺={fg_ok}（fg_tid={fg_tid}）"
            ));
        } else {
            crate::tsf::trace(&format!("addword: 前台抢夺={fg_ok}（直接成功）"));
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // 【RichEdit Tab 2026-10-07】词框 RichEdit 的 DLGC 应答吞
            // Tab——IsDialogMessage 拿不到导航权。词框内 Tab/Shift+Tab
            // 手动导航三框环（词→编码→选重→词）。
            if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN
                && msg.wParam.0 as u16 == 0x09 /* VK_TAB */
            {
                let foc = unsafe {
                    windows::Win32::UI::Input::KeyboardAndMouse::GetFocus()
                };
                // 【编码/选重位对换 2026-10-08】Tab 环与行序同步：
                // 词→选重位→编码→词（注释「词→编码→选重」同步更正）
                let order = [ID_WORD, ID_POS, ID_CODE];
                let cur = order.iter().position(|i| {
                    matches!(
                        unsafe { GetDlgItem(hwnd, *i) },
                        Ok(h) if h == foc
                    )
                });
                if let Some(c) = cur {
                    #[link(name = "user32")]
                    unsafe extern "system" {
                        fn GetKeyState(vkey: i32) -> i16;
                    }
                    let shift = (unsafe { GetKeyState(0x10) } as u16 & 0x8000) != 0;
                    let n = order.len();
                    let next = if shift { (c + n - 1) % n } else { (c + 1) % n };
                    if let Ok(h) = GetDlgItem(hwnd, order[next]) {
                        let _ = unsafe {
                            windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(h)
                        };
                        continue;
                    }
                }
            }
            if !IsDialogMessageW(hwnd, &mut msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        // 消息循环退出（窗口已销毁）——清登记，下次 open 可再建
        let mut guard = ADDWORD_HWND.lock().unwrap_or_else(|p| p.into_inner());
        *guard = 0;
        // 清线程 id（防系统复用该 tid 时误判 in_window_thread）
        ADDWORD_TID.store(0, std::sync::atomic::Ordering::Relaxed);
    });
}

/// client 高 → 外框高。
unsafe fn outer_h(client_h: i32) -> i32 {
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: WIN_W,
        bottom: client_h,
    };
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    let _ = AdjustWindowRect(&mut rc, style, false);
    rc.bottom - rc.top
}

unsafe fn create_child(
    parent: HWND,
    cls: PCWSTR,
    text: PCWSTR,
    ex: WINDOW_EX_STYLE,
    style_extra: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: i32,
) -> HWND {
    let hmod = GetModuleHandleW(None).unwrap_or_default();
    CreateWindowExW(
        ex,
        cls,
        text,
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style_extra),
        x,
        y,
        w,
        h,
        parent,
        HMENU(id as *mut _),
        HINSTANCE(hmod.0),
        None,
    )
    .unwrap_or_default()
}

/// 【词框聚焦 2026-09-12】小窗首个 EDIT（词框）句柄——WM_ACTIVATE
/// 激活时聚焦用（isize 规避 HWND 跨静态的 Sync 问题）。
static FIRST_EDIT: std::sync::Mutex<Option<isize>> = std::sync::Mutex::new(None);
/// 【编码自动跟踪 2026-10-08】编码框当前内容是否系自动填写（true=词框
/// 每变都重算覆盖；用户手改过即 false，清空编码框回到 true）。
static CODE_AUTO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
/// 我们自己 SetWindowTextW(ID_CODE) 触发的 EN_UPDATE 回声标记（不算
/// 用户手改）。
static SET_CODE_ECHO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 【回声风暴修复 2026-10-08 v3】最后一条**程序化写入**编码框的值。
/// RichEdit 对邻近子窗扰动（预览销毁/重建 STATIC）会重发 EN_UPDATE——
/// 回声旗标只够挡第一条，风暴重发的文本==最后写入值：据此识别，不算
/// 用户手改（否则 CODE_AUTO 被误关 → 打第 2 个字起自动编码停摆，
/// 用户实测「中→dg 有了，再打 心 还是 DG」的根因）。
static CODE_LAST_SET: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
use std::sync::atomic::Ordering;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ACTIVATE => {
            // 【词框聚焦 2026-09-12 v1.5.3+】小窗激活时把键盘焦点落
            // 到词框 EDIT——此前只在 WM_CREATE 里 SetFocus（窗口未
            // 激活时设置无效），激活后焦点停在顶层窗自身：用户在词框
            // 打虎码，键全部被宿主主文档的 TSF sink 吃掉——组段建在
            // 主文档（原光标处 preedit 残留+候选框弹在主文档旁），
            // 词框打不进字。标准对话框模式：激活即聚焦首控件。
            // 低字节 WA_ACTIVE/WA_CLICKACTIVE 均聚焦；WA_INACTIVE 跳过。
            let lo = (wp.0 & 0xFFFF) as u16;
            if lo != 0 {
                let fe = FIRST_EDIT
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .unwrap_or(0);
                if fe != 0 {
                    let _ = SetFocus(HWND(fe as *mut _));
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_CREATE => {
            let mk = |s: &str| -> Vec<u16> {
                let mut v: Vec<u16> = s.encode_utf16().collect();
                v.push(0);
                v
            };
            let mut first_edit = HWND::default();
            // 【加权模式 2026-09-06】两框：词 + 权重（编码由 server
            // 反查；不建选重位）；加词模式=原有三框
            let rows: Vec<(&str, i32, u32)> = if is_weight_mode() {
                vec![
                    ("词（要加权的字或词）", ID_WORD, WS_TABSTOP.0 | 0x80u32),
                    (
                        "权重（留空=1000）",
                        ID_CODE,
                        WS_TABSTOP.0 | 0x80u32 | 0x2000u32,
                    ),
                ]
            } else {
                // 【编码/选重位对换 2026-10-08】行序改为 词→选重位→编码
                //（用户流程拍板：打词→选重位→少数情况才改编码，配合
                // 编码框自动反查预填）。id 不变（ID_CODE/ID_POS），仅
                // 行位与 Tab 序对换。
                vec![
                    ("词（要打出的内容）", ID_WORD, WS_TABSTOP.0 | 0x80u32),
                    (
                        "选重位（第几选，留空=首选）",
                        ID_POS,
                        WS_TABSTOP.0 | 0x80u32 | 0x2000u32,
                    ),
                    ("编码（打什么出它）", ID_CODE, WS_TABSTOP.0 | 0x80u32),
                ]
            };
            // 【T4 根治·词框 RichEdit v2 2026-10-07】词框用 RichEdit50W
            //（TSF 原生通道：编码内联显示、上屏即时、无 CUAS 延迟/重放
            // 全套怪癖）。msftedit.dll 先显式载入；创建失败回退 EDIT +
            // v3c 特判路径（AW_RICH=false），双保险。每个子件建窗结果
            // 全量留痕（上轮闪框排查零证据，这轮必须带证据迭代）。
            {
                #[link(name = "kernel32")]
                unsafe extern "system" {
                    fn LoadLibraryW(name: *const u16) -> isize;
                }
                let mn: Vec<u16> = "msftedit.dll\0".encode_utf16().collect();
                let ok = unsafe { LoadLibraryW(mn.as_ptr()) } != 0;
                crate::tsf::trace(&format!("awRich msftedit 载入={ok}"));
            }
            for (i, (label, id, extra)) in rows.iter().enumerate() {
                let y = ROW_Y0 + i as i32 * ROW_PITCH;
                let lbl_txt = mk(label);
                // 【词框实时编码 2026-10-07】词行标签给 id：EDIT 回退模式
                // 下把编码尾巴显示在标签上（RichEdit 模式框内内联显示）。
                let lbl = create_child(
                    hwnd,
                    w!("STATIC"),
                    PCWSTR(lbl_txt.as_ptr()),
                    WINDOW_EX_STYLE(0),
                    0,
                    PV_X,
                    y,
                    LBL_W,
                    LBL_H,
                    if *id == ID_WORD { AW_LIVE_LABEL } else { 0 },
                );
                set_item_font(lbl, true);
                // 【三框统一 RichEdit 2026-10-07】外观/行为一致；词框
                // TSF 原生通道。RichEdit 不认 ES_NUMBER——选重位/权重
                // 框数字过滤改在 EN_UPDATE 里做。创建失败回退 EDIT。
                let mut ed = create_child(
                    hwnd,
                    w!("RICHEDIT50W"),
                    w!(""),
                    WS_EX_CLIENTEDGE,
                    *extra,
                    PV_X,
                    y + EDIT_DY,
                    EDIT_W,
                    EDIT_H,
                    *id,
                );
                if *id == ID_WORD {
                    crate::tsf::trace(&format!(
                        "awRich 词框建 {2} hwnd=0x{0:x} err={1}",
                        ed.0 as usize,
                        std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
                        if ed.0.is_null() { "失败" } else { "成功" }
                    ));
                    if ed.0.is_null() {
                        ed = create_child(
                            hwnd,
                            w!("EDIT"),
                            w!(""),
                            WS_EX_CLIENTEDGE,
                            *extra,
                            PV_X,
                            y + EDIT_DY,
                            EDIT_W,
                            EDIT_H,
                            *id,
                        );
                        AW_RICH.store(false, std::sync::atomic::Ordering::Relaxed);
                    } else {
                        AW_RICH.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                } else if ed.0.is_null() {
                    // 选重位/编码框 RichEdit 建失败——回退 EDIT（保数字
                    // 框 ES_NUMBER 生效；词框才是 RichEdit 关键路径）
                    ed = create_child(
                        hwnd,
                        w!("EDIT"),
                        w!(""),
                        WS_EX_CLIENTEDGE,
                        *extra,
                        PV_X,
                        y + EDIT_DY,
                        EDIT_W,
                        EDIT_H,
                        *id,
                    );
                }
                set_item_font(ed, false);
                // 【EDIT 免 IME·分字段 2026-09-10】只有编码框（打什么出
                // 它，纯字母）与选重位/权重框（纯数字）断开输入上下文——
                // 在这些框打 jav/123 若唤起输入法会先出候选不上字母。
                // 【词框恢复 IME 2026-09-10 用户反馈】词框恰恰要打中文
                //（HuFu 自己在词框组段正常，也可切其他输入法）——此前
                // 一刀切全断导致词框打不了中文。
                // 动态取 ImmAssociateContext：mingw 交叉工具链无 imm32
                // import lib（链接报 cannot find -limm32），imm32.dll
                // 运行期必然在（IMM 子系统）——GetProcAddress 最稳。
                if *id == ID_CODE || *id == ID_POS {
                    #[link(name = "kernel32")]
                    unsafe extern "system" {
                        fn GetModuleHandleW(name: *const u16) -> isize;
                        fn GetProcAddress(
                            module: isize,
                            name: *const u8,
                        ) -> *const core::ffi::c_void;
                    }
                    unsafe {
                        let mn: Vec<u16> = "imm32.dll\0".encode_utf16().collect();
                        let md = GetModuleHandleW(mn.as_ptr());
                        if md != 0 {
                            let p =
                                GetProcAddress(md, c"ImmAssociateContext".as_ptr() as *const u8);
                            if !p.is_null() {
                                type Iac = unsafe extern "system" fn(isize, isize) -> isize;
                                let f: Iac = std::mem::transmute(p);
                                let _ = f(ed.0 as isize, 0);
                            }
                        }
                    }
                }
                if i == 0 {
                    first_edit = ed;
                }
            }
            // 【右键调频菜单 2026-10-06】预填三框（词/编码/选重位）：
            // 消费即清——窗口复用/复开不残留旧值。
            // 【v4·开窗重置自动态 2026-10-09】配合去掉「空值重新武装」：
            // 每次开窗 CODE_AUTO 回 true（上次会话手改编码不再拖累本次
            // 自动反查）；预填写编码触发的 EN_UPDATE 随即置 false（调用
            // 方给的编码视为手改——次序保证此语义不变）。
            CODE_AUTO.store(true, std::sync::atomic::Ordering::Relaxed);
            SET_CODE_ECHO.store(false, std::sync::atomic::Ordering::Relaxed);
            *CODE_LAST_SET.lock().unwrap_or_else(|p| p.into_inner()) = None;
            {
                let prefill = PREFILL.lock().unwrap_or_else(|p| p.into_inner()).take();
                if let Some((w, c, p)) = prefill.filter(|_| !is_weight_mode()) {
                    // 【预填次序 2026-10-08】先编码/选重位后词：编码预填
                    // 触发的 EN_UPDATE 会把 CODE_AUTO 置 false（调用方给
                    // 的编码视为手改），词框后填即不会再触发自动反查覆盖。
                    for (id, t) in [(ID_CODE, c), (ID_POS, p), (ID_WORD, w)] {
                        if let Ok(h) = GetDlgItem(hwnd, id) {
                            let v: Vec<u16> =
                                t.encode_utf16().chain([0]).collect();
                            let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                                h,
                                PCWSTR(v.as_ptr()),
                            );
                        }
                    }
                    crate::tsf::trace("addword 预填（右键调频菜单·加词）");
                }
            }
            if !is_weight_mode() {
                // 【占位符勾选 2026-10-08】选重位超出该码现有候选数时，
                // 用带圈数字 ③④… 补空位使词恰落第 N 位（用户例：d 码
                // 现有「中 哪个」，加「是」选 4 位 → 中 哪个 ③ 是）。
                let ph_txt = mk("占位符（选重位超出时用 ③ ④ … 补齐空位）");
                let chb = create_child(
                    hwnd,
                    w!("BUTTON"),
                    PCWSTR(ph_txt.as_ptr()),
                    WINDOW_EX_STYLE(0),
                    WS_TABSTOP.0 | 0x3u32, // BS_AUTOCHECKBOX=0x3！曾误传
                    // 0x2=BS_CHECKBOX（手动状态型：点击不自动打勾）——
                    // 「点不动」四轮反馈的绝对根因：BN_CLICKED 每击必
                    // 发，但盒子视觉恒空、BM_GETCHECK 恒 0，勾选逻辑
                    // 从未真正生效过
                    PV_X,
                    PH_ROW_Y,
                    LBL_W,
                    LBL_H,
                    ID_PH,
                );
                set_item_font(chb, true);
                // 【占位符记忆 2026-10-09】回读上次勾选态（BM_SETCHECK
                // 0x00F1）——开过就一直开，关过就一直关。
                if ph_recall() {
                    let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                        chb,
                        0x00F1, /* BM_SETCHECK */
                        WPARAM(1),
                        LPARAM(0),
                    );
                }
                let t1 = mk("该编码候选（实时，第 N 选参考）：");
                let pt = create_child(
                    hwnd,
                    w!("STATIC"),
                    PCWSTR(t1.as_ptr()),
                    WINDOW_EX_STYLE(0),
                    0,
                    PV_X,
                    PV_TITLE_Y,
                    408, // 340×1.2
                    26,  // 22×1.2
                    0,
                );
                set_item_font(pt, true);
            }
            for (label, id) in [("确定", ID_OK), ("取消", ID_CANCEL)] {
                let btxt = mk(label);
                let btn = create_child(
                    hwnd,
                    w!("BUTTON"),
                    PCWSTR(btxt.as_ptr()),
                    WINDOW_EX_STYLE(0),
                    WS_TABSTOP.0,
                    if id == ID_OK { BTN_OK_X } else { BTN_CANCEL_X },
                    PH_ROW_Y + 70, // 初始位（refresh_preview 钉底重排）
                    BTN_W,
                    BTN_H,
                    id,
                );
                set_item_font(btn, false);
            }
            if !first_edit.0.is_null() {
                let _ = SetFocus(first_edit);
                // 【词框聚焦 2026-09-12】存静态：WM_ACTIVATE 激活时聚焦
                //（WM_CREATE 时窗口未激活 SetFocus 无效）。
                *FIRST_EDIT.lock().unwrap_or_else(|p| p.into_inner()) = Some(first_edit.0 as isize);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            let brush = BG_BRUSH
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .map(|b| windows::Win32::Graphics::Gdi::HBRUSH(b as *mut _));
            if let Some(br) = brush {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let hdc = windows::Win32::Graphics::Gdi::HDC(wp.0 as *mut _);
                let _ = windows::Win32::Graphics::Gdi::FillRect(hdc, &rc, br);
                LRESULT(1)
            } else {
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
        WM_CTLCOLORSTATIC => {
            let ctrl = HWND(lp.0 as _);
            let cid = GetWindowLongPtrW(ctrl, GWLP_ID) as i32;
            let sc = SKIN.lock().unwrap_or_else(|p| p.into_inner());
            let Some(c) = sc.as_ref() else {
                return DefWindowProcW(hwnd, msg, wp, lp);
            };
            let hdc = windows::Win32::Graphics::Gdi::HDC(wp.0 as _);
            // 新词项：红色（字体已加粗带下划线）；其余按段取皮肤色。
            //（ID_CUR_BASE=2000 < ID_AFT_BASE=2200 < ID_AFT_NEW=2600，
            // 单条 `>= ID_CUR_BASE` 即覆盖两个普通段）
            let fg: u32 = if cid >= ID_AFT_NEW {
                NEW_RED
            } else if cid >= ID_AFT_PH {
                // 【占位符 2026-10-08】占位项 ③④…：标签色压暗（区别
                // 于真候选；序号+词同段同色）
                c.label
            } else if cid >= ID_CUR_BASE {
                c.text
            } else {
                c.label
            };
            let _ = windows::Win32::Graphics::Gdi::SetTextColor(hdc, COLORREF(fg));
            let _ = windows::Win32::Graphics::Gdi::SetBkColor(hdc, COLORREF(c.bg));
            if let Some(b) = *BG_BRUSH.lock().unwrap_or_else(|p| p.into_inner()) {
                return LRESULT(b as isize);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_CTLCOLOREDIT => {
            let sc = SKIN.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(c) = sc.as_ref() {
                let hdc = windows::Win32::Graphics::Gdi::HDC(wp.0 as _);
                let _ = windows::Win32::Graphics::Gdi::SetTextColor(hdc, COLORREF(c.text));
                let _ = windows::Win32::Graphics::Gdi::SetBkColor(hdc, COLORREF(c.bg));
                if let Some(b) = *BG_BRUSH.lock().unwrap_or_else(|p| p.into_inner()) {
                    return LRESULT(b as isize);
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        // 【T4 v5】回灌链：第一拍（AW_FLUSH→30ms→pending 转正）、
        // 第二拍（AW_TAIL→30ms→补建尾巴段）。
        m if m == AW_FLUSH_MSG || m == AW_TAIL_MSG => {
            let tail = m == AW_TAIL_MSG;
            let _ = unsafe {
                windows::Win32::UI::WindowsAndMessaging::SetTimer(
                    hwnd,
                    if tail { AW_TAIL_TIMER } else { AW_FLUSH_TIMER },
                    30,
                    None,
                )
            };
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_TIMER => {
            let id = wp.0 as usize;
            if id == AW_FLUSH_TIMER {
                let _ = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::KillTimer(hwnd, AW_FLUSH_TIMER)
                };
                crate::tsf::aw_flush_pending();
                LRESULT(0)
            } else if id == AW_TAIL_TIMER {
                let _ = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::KillTimer(hwnd, AW_TAIL_TIMER)
                };
                crate::tsf::aw_flush_tail();
                LRESULT(0)
            } else {
                // 【RichEdit 2026-10-07】其余定时器必须还給 DefWindowProc
                // ——RichEdit 内部靠定时器（光标闪烁/布局），全吞=闪框。
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
        WM_COMMAND => {
            let id = (wp.0 as u32 & 0xFFFF) as i32;
            let notif = (wp.0 as u32 >> 16) as u32;
            // 【Tab 到编码框光标置尾 2026-10-09】EN_SETFOCUS(0x0100) 时
            // EM_SETSEL(尾,尾)：Tab 进编码框光标落在自动编码之后——
            // 不满意直接退格改，不用先按 End/点框尾（用户规格）。
            // 鼠标点击入框时本消息先行、点击定位随后覆盖，互不干扰。
            if id == ID_CODE && notif == 0x0100 {
                if let Ok(h) = GetDlgItem(hwnd, ID_CODE) {
                    let n = unsafe { GetWindowTextLengthW(h) };
                    let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                        h,
                        0x00B1, /* EM_SETSEL */
                        WPARAM(n as usize),
                        LPARAM(n as isize),
                    );
                }
                return LRESULT(0);
            }
            // EN_UPDATE=0x400：文本每变一次立即刷（0x200 是 EN_KILLFOCUS，
            // 上版误用导致「光标移走才刷新」）
            if (id == ID_CODE || id == ID_POS || id == ID_WORD) && notif == ID_EN_UPDATE {
                let rd = |i: i32| -> String {
                    match GetDlgItem(hwnd, i) {
                        Ok(h) => {
                            let n = GetWindowTextLengthW(h);
                            if n <= 0 {
                                return String::new();
                            }
                            let mut b = vec![0u16; n as usize + 1];
                            let g = GetWindowTextW(h, &mut b);
                            String::from_utf16_lossy(&b[..g.max(0) as usize])
                        }
                        Err(_) => String::new(),
                    }
                };
                let (wv, cv, pv) = (rd(ID_WORD), rd(ID_CODE), rd(ID_POS));
                // 【程序化写编码框的回声 2026-10-08】SET_CODE_ECHO 事件
                // （我们自己 SetWindowTextW 触发）不算用户手改，编码保
                // 持自动态，直接刷预览。
                if id == ID_CODE && SET_CODE_ECHO.swap(false, Ordering::Relaxed) {
                    unsafe { refresh_preview(hwnd) };
                    return LRESULT(0);
                }
                if id == ID_CODE {
                    // 【空格泄漏防御 2026-10-09】真机取证：/jc 上屏空格会经
                    // 词框 TIP sink 双投——开窗 1 秒后编码框出现「 」，
                    // ≠CODE_LAST_SET 被误判手改 → CODE_AUTO=false → 自动
                    // 编码死（旧版靠「空值重新武装」硬撑，连带删空回填
                    // bug）。编码框合法内容无空格：纯空白=泄漏，清空且
                    // 不算手改。
                    if !cv.is_empty() && cv.trim().is_empty() {
                        if let Ok(h) = GetDlgItem(hwnd, ID_CODE) {
                            SET_CODE_ECHO.store(true, Ordering::Relaxed);
                            *CODE_LAST_SET
                                .lock()
                                .unwrap_or_else(|p| p.into_inner()) = Some(String::new());
                            let _ =
                                windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                                    h,
                                    PCWSTR([0u16].as_ptr()),
                                );
                            return LRESULT(0);
                        }
                    }
                    // 用户手改编码 → 停止自动跟踪（清空编码框即恢复）。
                    // 【v3】文本==最后程序化写入值 = 预览销毁扰动重发的
                    // 回声（旗标已被第一条消费），不算手改。
                    // 【v5·空值不算手改 2026-10-09】真机取证：开窗首个
                    // EN_UPDATE 常为空文本（控件创建/回声），对照
                    // CODE_LAST_SET=None 必不等 → 误判手改 → 自动编码
                    // 开窗即死（旧版靠「空值重新武装」续命，连带删空
                    // 回填 bug）。空编码框不构成任何「改」——永不置
                    // false；非空且≠最后程序化值才是手改。
                    let storm_echo = cv.trim().is_empty() || {
                        let last = CODE_LAST_SET.lock().unwrap_or_else(|p| p.into_inner());
                        last.as_deref() == Some(cv.trim())
                    };
                    if !storm_echo {
                        CODE_AUTO.store(false, Ordering::Relaxed);
                    }
                }
                // 【自动填写编码·实时跟踪 2026-10-08 v2】词框每变一次
                // （加字/减字/清空）都按当前词重算编码：自动态（编码框
                // 空、或内容系自动填的）持续覆盖更新；用户手改过编码则
                // 不再覆盖，清空编码框即恢复自动。首版「只填一次」不符
                // 合打字节奏（用户反馈：打第 2 个字、回删 1 个字编码都
                // 不更新）。
                // 【v4·空值不再重新武装 2026-10-09】用户实锤：手删编码
                // 删到空的一瞬间自动编码又回来了（RichEdit 风暴重发的
                // 词框 EN_UPDATE 带「编码已空」命中 `|| cv.is_empty()`
                // 重新武装 → 立即回填，只能 Ctrl+A 重打）。去掉空值
                // 重新武装：手改（含删空）后自动永久停（重开窗重置，
                // 见创建处）。
                if !is_weight_mode() && id == ID_WORD {
                    let auto = CODE_AUTO.load(Ordering::Relaxed);
                    let wtrim = wv.trim().to_string();
                    if auto {
                        if wtrim.is_empty() {
                            // 词清空 → 自动态下编码框同步清空（回声事件刷预览）
                            if !cv.is_empty() {
                                if let Ok(h) = GetDlgItem(hwnd, ID_CODE) {
                                    SET_CODE_ECHO.store(true, Ordering::Relaxed);
                                    *CODE_LAST_SET
                                        .lock()
                                        .unwrap_or_else(|p| p.into_inner()) = Some(String::new());
                                    let _ =
                                        windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                                            h,
                                            PCWSTR([0u16].as_ptr()),
                                        );
                                    return LRESULT(0);
                                }
                            }
                        } else if let Some(hint) = word_code(&wtrim) {
                            if hint != cv.trim() {
                                if let Ok(h) = GetDlgItem(hwnd, ID_CODE) {
                                    let v: Vec<u16> =
                                        hint.encode_utf16().chain([0]).collect();
                                    SET_CODE_ECHO.store(true, Ordering::Relaxed);
                                    CODE_AUTO.store(true, Ordering::Relaxed);
                                    *CODE_LAST_SET
                                        .lock()
                                        .unwrap_or_else(|p| p.into_inner()) = Some(hint.clone());
                                    crate::tsf::trace(&format!(
                                        "addword: 自动编码 「{wtrim}」→ {hint}"
                                    ));
                                    let _ =
                                        windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                                            h,
                                            PCWSTR(v.as_ptr()),
                                        );
                                    // 触发的 ID_CODE 回声事件会带新编码刷预览。
                                    return LRESULT(0);
                                }
                            }
                        }
                        // hint 查不到（如生僻内容）→ 保留现值，落到下面
                        // 统一预览刷新。
                    }
                }
                // 【RichEdit 数字过滤 2026-10-07】RichEdit 不认
                // ES_NUMBER——选重位/加权权重框在 EN_UPDATE 剔除非
                // 数字（写回后光标置尾）。
                if id == ID_POS || (id == ID_CODE && is_weight_mode()) {
                    let src = if id == ID_POS { &pv } else { &cv };
                    let filtered: String =
                        src.chars().filter(|c| c.is_ascii_digit()).collect();
                    if filtered != *src {
                        if let Ok(h) = GetDlgItem(hwnd, id) {
                            let v: Vec<u16> =
                                filtered.encode_utf16().chain([0]).collect();
                            let _ =
                                windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                                    h,
                                    PCWSTR(v.as_ptr()),
                                );
                            let n = filtered.encode_utf16().count() as i32;
                            let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                                h,
                                0x00B1, /* EM_SETSEL */
                                WPARAM(n as usize),
                                LPARAM(n as isize),
                            );
                            return LRESULT(0);
                        }
                    }
                }
                // 【RichEdit 死循环掐断 2026-10-07】RichEdit 对邻近子窗
                // 扰动（预览区销毁/重建 STATIC）会重发 EN_UPDATE——每
                // 2-3ms 一环。三框文本没变就不重刷预览。
                static LAST_TXT: std::sync::Mutex<(String, String, String)> =
                    std::sync::Mutex::new((
                        String::new(),
                        String::new(),
                        String::new(),
                    ));
                {
                    let mut last = LAST_TXT.lock().unwrap_or_else(|p| p.into_inner());
                    if *last == (wv.clone(), cv.clone(), pv.clone()) {
                        return LRESULT(0);
                    }
                    *last = (wv, cv, pv);
                }
                unsafe { refresh_preview(hwnd) };
            }
            // 【占位符勾选 2026-10-08】勾/去勾即时重排预览（BN_CLICKED=0）
            if id == ID_PH && notif == 0 {
                let checked = match GetDlgItem(hwnd, ID_PH) {
                    Ok(h) => {
                        windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                            h,
                            0x00F0, /* BM_GETCHECK */
                            WPARAM(0),
                            LPARAM(0),
                        )
                        .0 as i32
                            == 1
                    }
                    Err(_) => false,
                };
                crate::tsf::trace(&format!("addword: 占位符勾选事件 checked={checked}"));
                // 【占位符记忆 2026-10-09】用户定调：这次开了下次进来还
                // 开着，关了下下次也保持关——写 HKCU\Software\HuFu\
                // addword_ph（REG_DWORD），对话框构建时回读。REG 支路
                // 失败静默（记忆是锦上添花，不能阻塞勾选主流程）。
                ph_remember(checked);
                unsafe { refresh_preview(hwnd) };
                return LRESULT(0);
            }
            let want = (id == ID_OK || id == ID_CANCEL || id == 1 || id == 2) && notif == 0;
            if want {
                if id == ID_OK || id == 1 {
                    unsafe { submit(hwnd) };
                }
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // 【T4 v3】窗毁清 pending/tail（防跨窗残留）
            let _ = crate::tsf::aw_clear_pending();
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn set_item_font(h: HWND, is_label: bool) {
    let store = if is_label {
        FONT_LABEL.lock().unwrap_or_else(|p| p.into_inner())
    } else {
        FONT_MAIN.lock().unwrap_or_else(|p| p.into_inner())
    };
    if let Some(f) = *store {
        let _ = SendMessageW(h, WM_SETFONT, WPARAM(f), LPARAM(1));
    }
}

/// 新词项字体（加粗+下划线）。
unsafe fn set_new_font(h: HWND) {
    if let Some(f) = *FONT_NEW.lock().unwrap_or_else(|p| p.into_inner()) {
        let _ = SendMessageW(h, WM_SETFONT, WPARAM(f), LPARAM(1));
    }
}

/// 文本显示宽估算（正文字号：CJK=1em、ASCII≈0.56em）。
fn text_w(s: &str, em: i32) -> i32 {
    s.chars()
        .map(|c| {
            if c.is_ascii() {
                (em as f32 * 0.56) as i32
            } else {
                em
            }
        })
        .sum()
}

/// 【词宽实测 2026-11】GetTextExtentPoint32W 按实际字体测显示宽。
/// text_w 估算（CJK=1em/ASCII≈0.56em）对加粗+下划线的新词字体普遍
/// 偏窄（宽 ASCII W/M≈0.7-0.8em、无独立粗体字重时 GDI 合成粗体还有
/// 右侧 overhang），STATIC 按估算宽硬裁=「预览新词吞最后一个字」
///（用户实锤；纯显示层——预览与提交同读框、落库数据完整）。主候选
/// 窗是 DirectWrite 实测宽，本窗是全仓唯一用估算宽的候选式渲染，
/// 改 GDI 实测对齐。测量失败（DC/字体异常）兜底回估算。
unsafe fn measure_w(s: &str, font_usize: usize) -> Option<i32> {
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        GetDC, GetTextExtentPoint32W, HGDIOBJ, ReleaseDC, SelectObject,
    };
    if s.is_empty() {
        return Some(0);
    }
    let hdc = GetDC(None);
    if hdc.is_invalid() {
        return None;
    }
    let old = SelectObject(hdc, HGDIOBJ(font_usize as *mut core::ffi::c_void));
    let txt = utf16z(s);
    let mut sz = SIZE::default();
    let ok = GetTextExtentPoint32W(hdc, &txt[..txt.len() - 1], &mut sz);
    SelectObject(hdc, old);
    ReleaseDC(None, hdc);
    if ok.as_bool() {
        Some(sz.cx)
    } else {
        None
    }
}

/// 预览一行候选项：序号(label 色) + 词(text 色) 流式排列换行。
/// 返回排版后的下一行 y。new_idx=加入后行中新词下标（高亮块）。
/// 【占位符 2026-10-08】ph_start..ph_end 段为占位项（③④…），用
/// ID_AFT_PH 段（标签色）压暗显示。
unsafe fn draw_items(
    hwnd: HWND,
    y0: i32,
    head: &str,
    texts: &[String],
    new_idx: Option<usize>,
    id_base: i32,
    line_h: i32,
    ph_start: usize,
    ph_end: usize,
) -> i32 {
    let em = SKIN
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|s| s.font_pt)
        .unwrap_or(16);
    let lem = SKIN
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|s| s.label_pt)
        .unwrap_or(12);
    let spacing = SKIN
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|s| s.cand_spacing)
        .unwrap_or(6);
    // 【词宽实测 2026-11】三字体取锁一次；测量用与渲染同一字体
    //（新词=加粗+下划线），失败回估算。
    let (label_font, main_font, new_font) = {
        let l = FONT_LABEL.lock().unwrap_or_else(|p| p.into_inner());
        let m = FONT_MAIN.lock().unwrap_or_else(|p| p.into_inner());
        let n = FONT_NEW.lock().unwrap_or_else(|p| p.into_inner());
        (*l, *m, *n)
    };
    let mw = |s: &str, f: Option<usize>, em: i32| -> i32 {
        match f {
            Some(h) => unsafe { measure_w(s, h) }.unwrap_or_else(|| text_w(s, em)),
            None => text_w(s, em),
        }
    };
    // 行首标签（「现有：」/「加入后：」）
    let head_txt = utf16z(head);
    let head_w = mw(head, label_font, lem) + 4;
    // 【SS_NOPREFIX=0x80】词含 & 时 STATIC 默认把它当加速键前缀吞掉
    //（"R&B"→"RB"、尾随 & 直接消失）——预览 STATIC 一律加。
    let h = create_child(
        hwnd,
        w!("STATIC"),
        PCWSTR(head_txt.as_ptr()),
        WINDOW_EX_STYLE(0),
        0x80,
        PV_X,
        y0,
        head_w + 2,
        line_h,
        0,
    );
    set_item_font(h, true);
    ITEMS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(h.0 as isize);

    let mut x = PV_X + head_w;
    let mut y = y0;
    let right = PV_X + PV_W;
    for (i, t) in texts.iter().enumerate() {
        let label = format!("{}.", i + 1);
        let is_new = new_idx == Some(i);
        let lw = mw(&label, label_font, lem) + 2;
        let tw = mw(t, if is_new { new_font } else { main_font }, em) + 2;
        let need = lw + tw + spacing;
        if x + need > right {
            x = PV_X + 17; // 续行缩进（14×1.2）
            y += line_h;
        }
        let base = if is_new {
            ID_AFT_NEW
        } else if i >= ph_start && i < ph_end {
            ID_AFT_PH
        } else {
            id_base
        };
        // 序号（新词行用高亮 id 段着色）
        let lbl_txt = utf16z(&label);
        let hl = create_child(
            hwnd,
            w!("STATIC"),
            PCWSTR(lbl_txt.as_ptr()),
            WINDOW_EX_STYLE(0),
            0x80,
            x,
            y + (line_h - lem - 4),
            lw,
            lem + 4,
            base, // 【三十四修】旧 `if is_new { base } else { base }` 恒等式简化
        );
        set_item_font(hl, true);
        ITEMS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(hl.0 as isize);
        // 词（新词用加粗+下划线字体；宽=实测，不再按估算硬裁吞尾字）
        let w_txt = utf16z(t);
        let wd = create_child(
            hwnd,
            w!("STATIC"),
            PCWSTR(w_txt.as_ptr()),
            WINDOW_EX_STYLE(0),
            0x80,
            x + lw,
            y + (line_h - em - 6) / 2,
            tw,
            em + 6,
            base + 1,
        );
        if is_new {
            set_new_font(wd);
        } else {
            set_item_font(wd, false);
        }
        ITEMS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(wd.0 as isize);
        x += need;
        // 【三十四修·死代码删除】cid 计数器（`cid += 2; let _ = cid;`
        // 只写不读）已删。
    }
    y + line_h
}

/// 清空旧预览项。
unsafe fn clear_items() {
    let mut g = ITEMS.lock().unwrap_or_else(|p| p.into_inner());
    for h in g.iter() {
        let _ = DestroyWindow(HWND(*h as *mut _));
    }
    g.clear();
}

/// 刷新预览 + 自适应布局。
unsafe fn refresh_preview(hwnd: HWND) {
    let read_box = |id: i32| -> String {
        let Ok(h) = GetDlgItem(hwnd, id) else {
            return String::new();
        };
        let len = GetWindowTextLengthW(h);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(h, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    };
    // 【加权模式 2026-09-06】单行提示：确定后 server 反查最优码并
    // 以用户词 weight 提权（词将排到候选前部）
    if is_weight_mode() {
        clear_items();
        let word = read_box(ID_WORD).trim().to_string();
        let wv: i64 = read_box(ID_CODE).trim().parse().unwrap_or(1000);
        let wv = if read_box(ID_CODE).trim().is_empty() {
            1000
        } else {
            wv
        };
        let msg = if word.is_empty() {
            "输入词后确定：编码自动反查，该词将提到候选前部。".to_string()
        } else {
            format!("『{word}』权重 {wv}：确定后提到候选前部。")
        };
        let em = SKIN
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|s| s.font_pt)
            .unwrap_or(16);
        let t = utf16z(&msg);
        // 【SS_NOPREFIX 2026-11】msg 含用户词（& 直通不吞）；宽=PV_W 通栏，
        // 长词由 STATIC 自身裁剪（表头提示语，与候选行不同路）。
        let h = create_child(
            hwnd,
            w!("STATIC"),
            PCWSTR(t.as_ptr()),
            WINDOW_EX_STYLE(0),
            0x80,
            PV_X,
            187, // 156×1.2（加权两行底 162 下留白）
            PV_W,
            em + 10,
            0,
        );
        set_item_font(h, false);
        ITEMS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(h.0 as isize);
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, true);
        return;
    }
    let code = read_box(ID_CODE).trim().to_string();
    let word = read_box(ID_WORD).trim().to_string();
    let pos: usize = read_box(ID_POS).trim().parse().unwrap_or(0);
    // 【占位符 2026-10-08】勾选态（BM_GETCHECK；加权模式无此框恒 false）
    let ph_on = match GetDlgItem(hwnd, ID_PH) {
        Ok(h) => {
            windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                h,
                0x00F0, /* BM_GETCHECK */
                WPARAM(0),
                LPARAM(0),
            )
            .0 as i32
                == 1 /* BST_CHECKED */
        }
        Err(_) => false,
    };

    let line_h = SKIN
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|s| s.font_pt + 17)
        .unwrap_or(41);

    clear_items();
    let mut y = PV_START_Y;
    let mut old_len_for_hint = 0usize;
    if code.is_empty() {
        // 占位提示
        let t = utf16z("（输入编码后显示该码候选）");
        let h = create_child(
            hwnd,
            w!("STATIC"),
            PCWSTR(t.as_ptr()),
            WINDOW_EX_STYLE(0),
            0,
            PV_X,
            y,
            PV_W,
            line_h,
            0,
        );
        set_item_font(h, true);
        ITEMS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(h.0 as isize);
        y += line_h;
    } else {
        match code_preview(&code) {
            Some(texts) => {
                let cur: Vec<String> = if texts.is_empty() {
                    vec!["（无候选——新词将成为首选）".to_string()]
                } else {
                    texts.clone()
                };
                y = draw_items(hwnd, y, "现有：", &cur, None, ID_CUR_BASE, line_h, 0, 0);
                if !word.is_empty() {
                    // 与 server add / Schema 插入同规则模拟。
                    // 【占位符补位 2026-10-08】勾选且 pos-1 超出现有数
                    // 时，用带圈数字 ③④… 补满空位使词恰落第 N 位（用户
                    // 例：d 码现有「中 哪个」+加「是」选 4 位 →
                    // 中 哪个 ③ 是；选 5 位 → 中 哪个 ③ ④ 是）。占位段
                    // 传给 draw_items 用标签色压暗（ID_AFT_PH 段）。
                    let mut sim: Vec<String> =
                        texts.iter().filter(|t| **t != word).cloned().collect();
                    let old_len = sim.len();
                    old_len_for_hint = old_len;
                    let mut ph_end = old_len;
                    if ph_on && pos >= 1 && pos - 1 > old_len && pos <= 50 {
                        for p in old_len..pos - 1 {
                            if let Some(c) = circled_num(p + 1) {
                                sim.push(c);
                            }
                        }
                        ph_end = sim.len();
                    }
                    // 【占位符位加词=替换 2026-10-09】第 pos 位现有候选
                    // 恰是占位词（带圈数字）→ 原位替换（server
                    // /api/user_word/add 同规则：先 {删除}占位词再
                    // {添加}pN，其余候选不动）；非占位位照旧插入后移。
                    // 【判定口径·用户拍板 2026-10-09】纯形态判定，与
                    // server 同规则：带圈数字即占位符，码表来源同样
                    // 替换（占位符码表方案导入即用）。
                    let idx = if pos >= 1 {
                        (pos as usize - 1).min(sim.len())
                    } else {
                        0
                    };
                    let is_ph = sim
                        .get(idx)
                        .map(|t| {
                            t.chars().count() == 1
                                && t.chars()
                                    .next()
                                    .map(|c| {
                                        let u = c as u32;
                                        (0x2460..=0x2473).contains(&u)
                                            || (0x3251..=0x325F).contains(&u)
                                    })
                                    .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if is_ph {
                        sim[idx] = word.clone();
                    } else {
                        sim.insert(idx, word.clone());
                    }
                    // 【预演取证 2026-10-09】用户报「词跑第2位、占位符在
                    // 第4位」——纸面推演全部正确，落此痕下次实测直接对账。
                    crate::tsf::trace(&format!(
                        "addword 预演: ph={ph_on} pos={pos} old_len={old_len} idx={idx} → {}",
                        sim.join("·")
                    ));
                    // 「现有」与「加入后」隔开一行距，视觉分组
                    y = draw_items(
                        hwnd,
                        y + line_h / 2 + 4,
                        "加入后：",
                        &sim,
                        Some(idx),
                        ID_AFT_BASE,
                        line_h,
                        old_len,
                        ph_end,
                    );
                }
            }
            None => {
                let t = utf16z("（查询失败）");
                let h = create_child(
                    hwnd,
                    w!("STATIC"),
                    PCWSTR(t.as_ptr()),
                    WINDOW_EX_STYLE(0),
                    0,
                    PV_X,
                    y,
                    PV_W,
                    line_h,
                    0,
                );
                set_item_font(h, true);
                ITEMS
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(h.0 as isize);
                y += line_h;
            }
        }
    }

    // 【占位符勾选反馈 2026-10-08 v2】勾选后无论预览内容是否变化都补
    // 一行状态说明（勾选必可见反馈——空码/空词/候选已够时预览不变，
    // 用户以为「点不动」的根因）。放预览区末尾，任何状态都画。
    if ph_on {
        let hint = if pos == 0 {
            "☑ 占位符已开：填选重位 N（N 超出现有候选数+1）后预览即补 ③ ④ …"
        } else if pos >= 1 && pos - 1 <= old_len_for_hint {
            "☑ 占位符已开：现有候选已够排到该位，本次不补位"
        } else {
            "☑ 占位符已开：第 ③ 位起已按带圈数字补齐空位"
        };
        let t = utf16z(hint);
        let h = create_child(
            hwnd,
            w!("STATIC"),
            PCWSTR(t.as_ptr()),
            WINDOW_EX_STYLE(0),
            0,
            PV_X,
            y + 6,
            PV_W,
            line_h,
            0,
        );
        set_item_font(h, true);
        ITEMS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(h.0 as isize);
        y += line_h + 6;
    }

    // 自适应：按钮钉底、窗口随高
    let btn_y = y + 12;
    let client_h = btn_y + BTN_H + 17;
    for (id, dx) in [(ID_OK, BTN_OK_X), (ID_CANCEL, BTN_CANCEL_X)] {
        if let Ok(ch) = GetDlgItem(hwnd, id) {
            let _ = SetWindowPos(
                ch,
                None,
                dx,
                btn_y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    let _ = SetWindowPos(
        hwnd,
        None,
        0,
        0,
        WIN_W,
        outer_h(client_h),
        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, true);
}

/// GET /api/code_preview?code=xxx → 候选文本列表。
fn code_preview(code: &str) -> Option<Vec<String>> {
    use std::io::{Read, Write};
    let enc: String = code
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let req = format!(
        "GET /api/code_preview?code={enc} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    );
    let mut s = std::net::TcpStream::connect(("127.0.0.1", 4390)).ok()?;
    // 【读超时 2026-09-11】与 http_post_json 同款：实时预览路径在
    // EN_UPDATE 每键触发，server 卡住会冻结加词窗 UI。
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(std::time::Duration::from_secs(2)));
    s.write_all(req.as_bytes()).ok()?;
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    let body = resp.split_once("\r\n\r\n").map(|(_, b)| b)?;
    let v: serde_json::Value = serde_json::from_str(body.trim_start()).ok()?;
    v.get("texts").and_then(|t| t.as_array()).map(|a| {
        a.iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect()
    })
}

/// GET /api/word_code?text=词 → 编码提示（词典反查最优码，新词按通用
/// 组词规则生成）。【加词自动填码 2026-10-08】词框变动且编码框空时预填。
fn word_code(text: &str) -> Option<String> {
    use std::io::{Read, Write};
    let enc: String = text
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let req = format!(
        "GET /api/word_code?text={enc} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    );
    let mut s = std::net::TcpStream::connect(("127.0.0.1", 4390)).ok()?;
    // 读超时同 code_preview：预填路径在词框 EN_UPDATE 每键触发。
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(std::time::Duration::from_secs(2)));
    s.write_all(req.as_bytes()).ok()?;
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    let body = resp.split_once("\r\n\r\n").map(|(_, b)| b)?;
    let v: serde_json::Value = serde_json::from_str(body.trim_start()).ok()?;
    v.get("code")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())
        .map(|c| c.to_string())
}

/// 读输入框 → POST server 加词。
unsafe fn submit(hwnd: HWND) {
    let read_edit = |id: i32| -> Option<String> {
        let h = GetDlgItem(hwnd, id).ok()?;
        let len = GetWindowTextLengthW(h);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(h, &mut buf);
        if n <= 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..n as usize]))
    };
    let (word, code, pos) = (read_edit(ID_WORD), read_edit(ID_CODE), read_edit(ID_POS));
    let pos_num = pos
        .as_deref()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(0);
    // 【加权模式 2026-09-06】词+权重（缺省 1000）→ server 反查最优码
    if is_weight_mode() {
        let Some(word) = word else { return };
        let word = word.trim().to_string();
        if word.is_empty() {
            return;
        }
        let wv: i64 = code
            .as_deref()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(1000);
        if post_weight(&word, wv) {
            crate::tsf::trace(&format!("addweight ok: {word} *{wv}"));
        } else {
            crate::tsf::trace(&format!("addweight POST 失败: {word} *{wv}"));
        }
        return;
    }
    let (Some(word), Some(code)) = (word, code) else {
        return;
    };
    let word = word.trim().to_string();
    let code = code.trim().to_string();
    if word.is_empty() || code.is_empty() {
        return;
    }
    // 【占位符补位提交 2026-10-08】勾选且 pos-1 超出现有候选数时，
    // 先按序提交带圈数字占位词条（③@p3、④@p4…——server {添加}pN
    // 时序回放逐条落位），词最后落在恰好的第 N 位。与预览同规则。
    let ph_on = match GetDlgItem(hwnd, ID_PH) {
        Ok(h) => {
            windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                h,
                0x00F0, /* BM_GETCHECK */
                WPARAM(0),
                LPARAM(0),
            )
            .0 as i32
                == 1
        }
        Err(_) => false,
    };
    if ph_on && pos_num >= 2 {
        let existing = code_preview(&code)
            .map(|texts| texts.iter().filter(|t| **t != word).count())
            .unwrap_or(0);
        if pos_num as usize - 1 > existing && pos_num <= 50 {
            for p in existing..pos_num as usize - 1 {
                if let Some(c) = circled_num(p + 1) {
                    if !post_add(&code, &c, p as i64 + 1) {
                        crate::tsf::trace(&format!(
                            "addword 占位符 POST 失败: {code} -> {c} @{}",
                            p + 1
                        ));
                    }
                }
            }
        }
    }
    if post_add(&code, &word, pos_num) {
        crate::tsf::trace(&format!("addword ok: {code} -> {word} @{pos_num}"));
    } else {
        crate::tsf::trace(&format!("addword POST 失败: {code} -> {word} @{pos_num}"));
    }
}

/// 裸 HTTP POST 127.0.0.1:4390 /api/user_word/weight。
/// 【JSON 安全 2026-09-11】旧手拼 esc 只转义 \ 和 "——词里带换行/
/// 制表符等控制字符会拼出非法 JSON（server 端解析 400，加词静默
/// 失败）。改 serde_json 正规序列化。
fn post_weight(word: &str, weight: i64) -> bool {
    let body = serde_json::json!({"text": word, "weight": weight}).to_string();
    http_post_json("/api/user_word/weight", &body)
}

/// 裸 HTTP POST 127.0.0.1:4390 /api/user_word/add。（同上 JSON 安全）
fn post_add(code: &str, word: &str, pos: i64) -> bool {
    let body = serde_json::json!({"code": code, "text": word, "pos": pos}).to_string();
    http_post_json("/api/user_word/add", &body)
}

/// 公共 POST（path + JSON body → 200 即成功）。
fn http_post_json(path: &str, body: &str) -> bool {
    use std::io::{Read, Write};
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let mut s = match std::net::TcpStream::connect(("127.0.0.1", 4390)) {
        Ok(s) => s,
        Err(e) => {
            crate::tsf::trace(&format!("addword tcp connect err: {e}"));
            return false;
        }
    };
    // 【读超时 2026-09-11】旧实现只有写超时，读无超时——server 卡住
    // 时加词窗 UI 线程（submit 是按钮点击路径）无限冻结。
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(std::time::Duration::from_secs(2)));
    if let Err(e) = s.write_all(req.as_bytes()) {
        crate::tsf::trace(&format!("addword tcp write err: {e}"));
        return false;
    }
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200")
}
