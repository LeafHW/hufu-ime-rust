//! TSF 语言栏按钮：任务栏输入指示区「中/A」状态牌（微软拼音风格）。
//!
//! - 图标/文字随中英模式切换（进程全局态，tsf.rs 每帧从引擎 state 同步）
//! - 左键 → 管道 op "toggle_lang" 切换中英（与 Shift 单击同语义）
//! - 右键 → 管道 op "settings" 打开设置页（与托盘双击同通道）
//! - 生命周期跟随输入法：Activate 挂载、Deactivate 移除
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use windows::core::{implement, Interface, PCWSTR, Result, GUID, VARIANT};
use windows::Win32::Foundation::{BOOL, COLORREF, E_INVALIDARG, RECT, SIZE};
use windows::Win32::UI::TextServices::{
    GUID_LBI_INPUTMODE, ITfCompartment, ITfCompartmentEventSink, ITfCompartmentEventSink_Impl,
    ITfCompartmentMgr, ITfLangBarItem, ITfLangBarItemButton, ITfLangBarItemButton_Impl,
    ITfLangBarItem_Impl, ITfLangBarItemMgr, ITfLangBarItemSink, ITfSource, ITfSource_Impl,
    ITfThreadMgr, TF_CONVERSIONMODE_ALPHANUMERIC, TF_CONVERSIONMODE_NATIVE, TF_LANGBARITEMINFO,
    GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION, GUID_COMPARTMENT_KEYBOARD_OPENCLOSE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW,
    SetForegroundWindow, HICON, HWND_MESSAGE, ICONINFO, WNDCLASSEXW,
};

/// COM 指针跨线程搬运套（sink 由 msctf 线程 Advise、模式切换线程通知：
/// 实际调用仍是 COM 默认自由线程封送语义，这里只解除 Rust 的静态限制）
struct SendSink(ITfLangBarItemSink);
unsafe impl Send for SendSink {}

/// 语言栏项 GUID：用系统保留的「输入模式」GUID——explorer 只把该
/// GUID 的按钮渲染进任务栏输入指示区（微软拼音/Rime 同款；自定义
/// GUID 会被归入浮动语言栏，实测任务栏不显示）
const LANGBAR_ITEM_GUID: GUID = GUID_LBI_INPUTMODE;

/// 服务 CLSID（与 com.rs 一致；GetInfo 需要）
const CLSID_HUFU: GUID = GUID::from_values(
    0x8F5C2A10,
    0x3E77,
    0x4B9C,
    [0xA1, 0xD4, 0x9E, 0x0B, 0x7C, 0x2F, 0x5A, 0x88],
);

const TF_LBI_STYLE_SHOWNINTRAY: u32 = 0x2; // 在任务栏角落（输入指示区）显示
const TF_LBI_STYLE_BTN_BUTTON: u32 = 0x10000;

// ── 进程全局模式态（tsf.rs 每帧同步；点击切换也走这里）──
static CHINESE: AtomicBool = AtomicBool::new(true);
/// msctf 挂的更新 sink：进程全局（多线程各挂各的项，共用一份名单）
static SINKS: Mutex<Vec<(u32, SendSink)>> = Mutex::new(Vec::new());
static NEXT_COOKIE: AtomicU32 = AtomicU32::new(0x4846_0001);

/// 读取当前中英态（msctf 拉 GetText/GetIcon 时用）
fn is_chinese() -> bool {
    CHINESE.load(Ordering::Relaxed)
}

/// 更新模式（sink 通知 + 线程 compartment 推送）。返回是否有变化。
/// 【v5：纯 STA】历史教训链——
/// (1) OnClick 线程内同步 notify 会挡 explorer 重入回查 → 曾异步化；
/// (2) 异步 MTA 工作线程跨套间裸调 STA 代理（sink、全局 compartment）
///     = UB → 一次切换后语言栏项死亡（OnClick 从此不触发，左右键全
///     死）——18:44 纯同步时代反复点击存活、异步化后每版必死的时
///     间线实证；
/// (3) 全局 compartment 的唯一用途（跨进程对账）已随 v4 引擎权威制
///     废除——系统焦点切换会把"每应用记忆模式"写进去（陈旧值），
///     对账等于跟系统记忆打架。
/// 故整条工作线程 + 全局侧拆除：只在本线程（STA）做线程 compartment
/// 推送与 sink 通知，进程内零跨套间调用。
pub fn set_mode(zh: bool) -> bool {
    let old = CHINESE.swap(zh, Ordering::Relaxed);
    if old == zh {
        return false;
    }
    // 【v6：延迟到消息泵空闲】铁证（v5 日志）：左键路径（OnClick 内）
    // set_mode 后右键存活，Shift 路径（ProcessKey 进行中）set_mode 后
    // 右键死——按键处理上下文里写 compartment / 发 sink 通知会打断
    // msctf 状态机、弄坏语言栏项连接。CHINESE 原子交换立即生效（读
    // 取方即刻拿到），msctf 副作用 PostMessage 到本线程常驻窗，按键
    // 处理完毕、线程回到消息循环后才执行。
    PENDING_ZH.store(zh, Ordering::Relaxed);
    let hwnd = DEFER_HWND.load(Ordering::Relaxed);
    log_diag(&format!("set_mode {old}->{zh}（排队）"));
    if hwnd != 0 {
        unsafe {
            PostMessageW(hwnd, WM_APP_DEFER, 0, 0);
        }
    } else {
        // 窗还没建（理论不会）：直接执行兜底
        push_thread_compartment();
        notify_sinks();
    }
    true
}

// ── 延迟执行窗（本线程 message-only；PostMessage 后消息泵空闲时执行）──
static DEFER_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static PENDING_ZH: AtomicBool = AtomicBool::new(true);
const WM_APP_DEFER: u32 = 0x8002;

unsafe extern "system" fn defer_wnd_proc(
    h: windows::Win32::Foundation::HWND,
    m: u32,
    _w: windows::Win32::Foundation::WPARAM,
    _l: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    if m == WM_APP_DEFER {
        let zh = PENDING_ZH.load(Ordering::Relaxed);
        log_diag(&format!("defer 推 zh={zh}"));
        push_thread_compartment();
        notify_sinks();
        return windows::Win32::Foundation::LRESULT(0);
    }
    unsafe { DefWindowProcW(h, m, _w, _l) }
}

/// 建（一次）延迟执行窗。必须在 UI 线程调（install 时）。
unsafe fn ensure_defer_window() {
    unsafe {
        if DEFER_HWND.load(Ordering::Relaxed) != 0 {
            return;
        }
        let cls: Vec<u16> = "HUFU_LB_DEFER\0".encode_utf16().collect();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(defer_wnd_proc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);
        let nm: Vec<u16> = "HuFu defer\0".encode_utf16().collect();
        if let Ok(w) = CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
            PCWSTR(cls.as_ptr()),
            PCWSTR(nm.as_ptr()),
            windows::Win32::UI::WindowsAndMessaging::WS_POPUP,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            None,
            None,
        ) {
            DEFER_HWND.store(w.0 as isize, Ordering::Relaxed);
        }
    }
}

/// 追加式诊断日志（%TEMP%\hufu-langbar.log；一行一条，带时间戳）。
/// 【教训】覆盖式日志只留最后一笔，排查点击序列时信息全丢。
fn log_diag(s: &str) {
    use std::io::Write;
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() % 86_400_000)
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("hufu-langbar.log"))
    {
        let _ = writeln!(f, "[{:02}:{:02}:{:02}] {s}", t / 3_600_000, t / 60_000 % 60, t / 1000 % 60);
    }
}

// ═══════════════ compartment 同步（v5：只写线程侧，纯 STA）═══════════════
// 写线程 compartment（OPENCLOSE=1 + CONV=中/英），供系统输入状态
// 界面在焦点/输入法切换时读取。【全局侧已废】全局 compartment 的
// 唯一用途是跨进程对账，而对账已被引擎权威制取代（系统会把"每应用
// 记忆模式"写回全局侧——陈旧值，追它等于跟系统记忆打架）；且从
// MTA 工作线程推全局 = 跨套间裸调 UB（语言栏项死亡真因之一）。

thread_local! {
    /// 本线程 compartment 源（Activate 时装；thread mgr 本身实现该接口）
    static COMP_THREAD: std::cell::RefCell<Option<ITfCompartmentMgr>> =
        const { std::cell::RefCell::new(None) };
    static COMP_COOKIE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}
/// TSF client id（SetValue 需要）
static TID: AtomicU32 = AtomicU32::new(0);

/// Activate 时调用：装 compartment 引用 + 挂监听 + 推初值。
/// tid = TSF client id（SetValue 需要）。
pub fn install_compartments(tm: &ITfThreadMgr, tid: u32) {
    TID.store(tid, Ordering::Relaxed);
    if let Ok(cm) = tm.cast::<ITfCompartmentMgr>() {
        COMP_THREAD.with(|c| *c.borrow_mut() = Some(cm));
    }
    advise_conversion(COMP_THREAD.with(|c| c.borrow().clone()), tid);
    push_thread_compartment();
}

/// Deactivate：摘本线程监听、清引用（全局的留着）
pub fn uninstall_compartments() {
    let cookie = COMP_COOKIE.with(|c| c.replace(0));
    if cookie != 0 {
        if let Some(cm) = COMP_THREAD.with(|c| c.borrow().clone()) {
            if let Ok(comp) = unsafe { cm.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION) } {
                if let Ok(src) = comp.cast::<ITfSource>() {
                    unsafe {
                        let _ = src.UnadviseSink(cookie);
                    }
                }
            }
        }
    }
    COMP_THREAD.with(|c| *c.borrow_mut() = None);
}

/// 把当前模式推进本线程 compartment（OPENCLOSE + CONV）。
pub fn push_compartments() {
    push_thread_compartment();
}

/// 线程 compartment（本线程上下文；set_mode/Activate 在 UI 线程调）
fn push_thread_compartment() {
    let conv: i32 = if is_chinese() {
        TF_CONVERSIONMODE_NATIVE as i32
    } else {
        TF_CONVERSIONMODE_ALPHANUMERIC as i32
    };
    let tid = TID.load(Ordering::Relaxed);
    let Some(src) = COMP_THREAD.with(|c| c.borrow().clone()) else {
        return;
    };
    unsafe {
        if let Ok(comp) = cm_compartment(&src, &GUID_COMPARTMENT_KEYBOARD_OPENCLOSE) {
            let v = VARIANT::from(1i32);
            let _ = comp.SetValue(tid, &v);
        }
        if let Ok(comp) = cm_compartment(&src, &GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION) {
            let v = VARIANT::from(conv);
            let _ = comp.SetValue(tid, &v);
        }
    }
}

/// compartment mgr → 指定 guid 的 compartment（GetCompartment 会自动建键）
unsafe fn cm_compartment(cm: &ITfCompartmentMgr, guid: *const GUID) -> Result<ITfCompartment> {
    unsafe { cm.GetCompartment(guid) }
}

/// 读 VARIANT 当 i32（VT_I4 = 3；偏移 8 = vt+3 保留字后）
unsafe fn var_i32(v: &VARIANT) -> Option<i32> {
    unsafe {
        let p = v as *const VARIANT as *const u16;
        if *p != 3 {
            return None;
        }
        let b = v as *const VARIANT as *const u8;
        Some(i32::from_le_bytes([*b.add(8), *b.add(9), *b.add(10), *b.add(11)]))
    }
}

fn advise_conversion(cm: Option<ITfCompartmentMgr>, _tid: u32) {
    let Some(cm) = cm else { return };
    unsafe {
        let Ok(comp) = cm.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION) else {
            return;
        };
        let Ok(src) = comp.cast::<ITfSource>() else {
            return;
        };
        let sink: ITfCompartmentEventSink = ModeSink {}.into();
        let _ = src.AdviseSink(&ITfCompartmentEventSink::IID, &sink);
        // cookie 不存（进程存续期常驻；Deactivate 只摘线程那份）
    }
}

/// 外部改了转换模式（用户点系统牌 / 其他进程广播）：值 ≠ 预期 →
/// 引擎【设值】跟上；相等（自己的写入回声）→ 忽略。
/// 【权威域拆分】sink 只读自己监听的那一侧：
#[implement(ITfCompartmentEventSink)]
struct ModeSink {}

impl ITfCompartmentEventSink_Impl for ModeSink_Impl {
    fn OnChange(&self, rguid: *const GUID) -> Result<()> {
        unsafe {
            if rguid.as_ref() != Some(&GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION) {
                return Ok(());
            }
        }
        let expected: i32 = if is_chinese() {
            TF_CONVERSIONMODE_NATIVE as i32
        } else {
            TF_CONVERSIONMODE_ALPHANUMERIC as i32
        };
        // 【v4/v5：纯观察者】compartment 不是权威——引擎才是。
        // 系统焦点切换会把「每应用记忆的输入模式」写进 compartment
        // （陈旧值），追它等于跟系统记忆打架；且 msctf 回调里做任何
        // 阻塞调用（管道）都会弄坏 msctf 连接（语言栏项死亡真因）。
        // 这里只读线程侧记一笔诊断日志，引擎状态由按键响应同步。
        let src = COMP_THREAD.with(|c| c.borrow().clone());
        let mut actual: Option<i32> = None;
        if let Some(cm) = src {
            if let Ok(comp) =
                unsafe { cm_compartment(&cm, &GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION) }
            {
                if let Ok(v) = unsafe { comp.GetValue() } {
                    actual = unsafe { var_i32(&v) };
                }
            }
        }
        if let Some(n) = actual {
            if n != expected {
                log_diag(&format!(
                    "OnChange[T] actual={n} expected={expected}（仅记录，引擎不追系统记忆）"
                ));
            }
        }
        Ok(())
    }
}

fn notify_sinks() {
    // TF_LBI_ICON|TF_LBI_TEXT|TF_LBI_TOOLTIP
    const UPD: u32 = 0x1 | 0x2 | 0x4;
    if let Ok(v) = SINKS.lock() {
        for (_, sink) in v.iter() {
            unsafe {
                let _ = sink.0.OnUpdate(UPD);
            }
        }
    }
}

#[implement(ITfLangBarItem, ITfLangBarItemButton, ITfSource)]
pub struct HuFuLangBar {
    icon_zh: isize, // HICON（纯字符「中」，无底牌）
    icon_en: isize, // 「英」
}

impl HuFuLangBar {
    pub fn new() -> HuFuLangBar {
        // 【品牌按钮】牌图标固定「虎」，不随模式变——系统牌只在
        // 输入法/焦点切换时重读图标（平台缓存），若显示中/英状态则
        // 会冻结在旧状态：用户看着错的字、系统点牌时还按错的字
        // 执行。品牌字永不撒谎；模式看打字即知。
        HuFuLangBar {
            icon_zh: make_glyph_icon("虎"),
            icon_en: make_glyph_icon("虎"),
        }
    }
}

/// ITfSource：msctf（语言栏宿主）会对项 AdviseSink(ITfLangBarItemSink)。
/// 不实现该接口时 AddItem 在真实宿主里可能 E_FAIL。名单进程全局共享
///（多线程的项一起收广播，cookie 全局唯一防误删）。
impl ITfSource_Impl for HuFuLangBar_Impl {
    fn AdviseSink(
        &self,
        riid: *const GUID,
        punk: Option<&windows::core::IUnknown>,
    ) -> Result<u32> {
        if unsafe { riid.as_ref() } != Some(&ITfLangBarItemSink::IID) {
            return Err(windows::core::Error::from(E_INVALIDARG));
        }
        let sink: ITfLangBarItemSink = punk
            .and_then(|u| u.cast().ok())
            .ok_or_else(|| windows::core::Error::from(E_INVALIDARG))?;
        let cookie = NEXT_COOKIE.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut v) = SINKS.lock() {
            v.push((cookie, SendSink(sink)));
        }
        Ok(cookie)
    }

    fn UnadviseSink(&self, dwcookie: u32) -> Result<()> {
        if let Ok(mut v) = SINKS.lock() {
            let before = v.len();
            v.retain(|(c, _)| *c != dwcookie);
            if v.len() != before {
                return Ok(());
            }
        }
        Err(windows::core::Error::from(E_INVALIDARG))
    }
}

/// 挂载到线程的语言栏（Activate 时调用）。项实例存 thread_local：
/// msctf 的 AddItem/RemoveItem 按对象引用操作（同一 GUID 认领）。
pub fn install(mgr: &ITfLangBarItemMgr) -> Result<()> {
    unsafe { ensure_defer_window() };
    let item: ITfLangBarItem = unsafe { HuFuLangBar::new().into() };
    let r = unsafe { mgr.AddItem(&item) };
    match &r {
        Ok(()) => log_diag(&format!("install ok pid={}", std::process::id())),
        Err(e) => log_diag(&format!("install FAIL pid={} err={:?}", std::process::id(), e.code())),
    }
    r?;
    LANGBAR_ITEM.with(|c| *c.borrow_mut() = Some(item));
    Ok(())
}

/// 供 tsf.rs Deactivate 使用：从线程语言栏摘除本项
pub fn uninstall(mgr: &ITfLangBarItemMgr) {
    LANGBAR_ITEM.with(|c| {
        if let Some(item) = c.borrow_mut().take() {
            unsafe {
                let _ = mgr.RemoveItem(&item);
            }
        }
    });
}

thread_local! {
    static LANGBAR_ITEM: std::cell::RefCell<Option<ITfLangBarItem>> =
        const { std::cell::RefCell::new(None) };
}

impl ITfLangBarItem_Impl for HuFuLangBar_Impl {
    fn GetInfo(&self, pclbid: *mut TF_LANGBARITEMINFO) -> Result<()> {
        if pclbid.is_null() {
            return Ok(());
        }
        let mut text = [0u16; 32];
        let t: Vec<u16> = "虎符输入法".encode_utf16().collect();
        text[..t.len()].copy_from_slice(&t);
        unsafe {
            (*pclbid).clsidService = CLSID_HUFU;
            (*pclbid).guidItem = LANGBAR_ITEM_GUID;
            (*pclbid).dwStyle = TF_LBI_STYLE_BTN_BUTTON | TF_LBI_STYLE_SHOWNINTRAY;
            (*pclbid).ulSort = 1;
            (*pclbid).szDescription = text;
        }
        Ok(())
    }

    fn GetStatus(&self) -> Result<u32> {
        Ok(0) // 无特殊状态（无 HIDDEN/Disabled）
    }

    fn Show(&self, _fshow: BOOL) -> Result<()> {
        Ok(())
    }

    fn GetTooltipString(&self) -> Result<windows::core::BSTR> {
        // 悬停提示固定文案（不随模式变，避免冻结误导）
        Ok(windows::core::BSTR::from(
            "虎符输入法（左键切中英，右键选码表/设置）",
        ))
    }
}

// ── 右键菜单裸 FFI（TrackPopupMenu + TPM_RETURNCMD 取回选项 id；
// windows crate 的签名拿不到返回值，这里按 Win32 原型直连）──
#[link(name = "user32")]
unsafe extern "system" {
    fn CreatePopupMenu() -> isize;
    fn AppendMenuW(m: isize, flags: u32, id: usize, text: *const u16) -> i32;
    fn TrackPopupMenu(
        m: isize,
        flags: u32,
        x: i32,
        y: i32,
        reserved: u32,
        hwnd: isize,
        rect: *const RECT,
    ) -> i32;
    fn DestroyMenu(m: isize) -> i32;
    fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
}
const MF_STRING: u32 = 0x0;
const MF_SEPARATOR: u32 = 0x800;
const MF_CHECKED: u32 = 0x8;
const TPM_RETURNCMD: u32 = 0x100;
const TPM_RIGHTBUTTON: u32 = 0x2;
const TPM_RIGHTALIGN: u32 = 0x8;
const TPM_BOTTOMALIGN: u32 = 0x20;

/// 右键小菜单：码表清单（当前 ✓）+ 分隔线 + 设置…
/// 【owner 教训】TrackPopupMenu 的 owner 窗口必须属于调用线程——借
/// GetForegroundWindow（他进程的窗口）会被静默拒绝（菜单不弹）。
/// 这里现建一个 message-only 窗口（HWND_MESSAGE 父）作 owner，
/// SetForegroundWindow 保焦点使外部点击可撤销，用完即毁。
unsafe fn popup_menu(pt: &windows::Win32::Foundation::POINT) {
    unsafe {
        // 取码表清单（server 不可达时只给设置项）
        let (schemas, current): (Vec<String>, String) =
            match crate::ipc::call(&serde_json::json!({"op": "schemas"})) {
                Some(r) => (
                    r.get("schemas")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default(),
                    r.get("current")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                ),
                None => (Vec::new(), String::new()),
            };
        let m = CreatePopupMenu();
        if m == 0 {
            return;
        }
        for (i, name) in schemas.iter().enumerate() {
            let w: Vec<u16> = name.encode_utf16().chain([0]).collect();
            let flags = MF_STRING
                + if name == &current {
                    MF_CHECKED
                } else {
                    0
                };
            AppendMenuW(m, flags, 100 + i, w.as_ptr());
        }
        if !schemas.is_empty() {
            AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
        }
        // 【2026-09-05】重载码表：改码表/补充语料后免重启即生效。
        let wrel: Vec<u16> = "重载码表".encode_utf16().chain([0]).collect();
        AppendMenuW(m, MF_STRING, 2, wrel.as_ptr());
        let wset: Vec<u16> = "设置…".encode_utf16().chain([0]).collect();
        AppendMenuW(m, MF_STRING, 1, wset.as_ptr());
        // 自建真弹出窗作 owner（调用线程持有 → 合法 owner）。
        // 【教训】message-only 窗口不能 SetForegroundWindow（不可见），
        // 焦点保不住 → 菜单有时秒关（sel=0）。真 WS_POPUP 0×0 窗可以。
        unsafe extern "system" fn menu_wnd_proc(
            h: windows::Win32::Foundation::HWND,
            m: u32,
            w: windows::Win32::Foundation::WPARAM,
            l: windows::Win32::Foundation::LPARAM,
        ) -> windows::Win32::Foundation::LRESULT {
            unsafe { DefWindowProcW(h, m, w, l) }
        }
        let cls: Vec<u16> = "HUFU_LB_MENU\0".encode_utf16().collect();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(menu_wnd_proc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);
        let nm: Vec<u16> = "HuFu 菜单宿主\0".encode_utf16().collect();
        let owner = CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
            PCWSTR(cls.as_ptr()),
            PCWSTR(nm.as_ptr()),
            windows::Win32::UI::WindowsAndMessaging::WS_POPUP,
            pt.x,
            pt.y,
            0,
            0,
            HWND_MESSAGE, // message-only 父：不进任务栏/切档
            None,
            None,
            None,
        )
        .unwrap_or_default();
        if owner.is_invalid() {
            log_diag("popup: 建窗失败");
            DestroyMenu(m);
            return;
        }
        // 【实测教训】不要 AttachThreadInput 到前台线程：右键时
        // explorer 正阻塞在本 OnClick 的 COM 调用里，挂上它的输入队列
        // 后菜单模态循环拿不到输入 → TrackPopupMenu 秒回 0（菜单从未
        // 显示，日志 sel=0 铁证）。直接 SetForegroundWindow 即可——
        // 任务栏指示牌右键时系统本来就给了显示许可。
        let _ = SetForegroundWindow(owner);
        let sel = TrackPopupMenu(
            m,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_RIGHTALIGN | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            0,
            owner.0 as isize,
            std::ptr::null(),
        );
        // WM_NULL 复位（KB135788：菜单系统状态机要求，缺它二次失灵）
        PostMessageW(owner.0 as isize, 0x0000, 0, 0);
        let _ = DestroyWindow(owner);
        DestroyMenu(m);
        log_diag(&format!("popup sel={sel} schemas={}", schemas.len()));
        if sel == 1 {
            let _ = crate::ipc::call(&serde_json::json!({"op": "settings"}));
        } else if sel == 2 {
            // 重载码表（当前方案原样重载；server 侧清会话+重建整句）
            let _ = crate::ipc::call(&serde_json::json!({"op": "reload_schema"}));
        } else if sel >= 100 {
            let idx = (sel - 100) as usize;
            if let Some(name) = schemas.get(idx) {
                let _ = crate::ipc::call(&serde_json::json!({
                    "op": "set_schema", "name": name
                }));
            }
        }
    }
}

impl ITfLangBarItemButton_Impl for HuFuLangBar_Impl {
    fn OnClick(
        &self,
        click: windows::Win32::UI::TextServices::TfLBIClick,
        pt: &windows::Win32::Foundation::POINT,
        _prcarea: *const RECT,
    ) -> Result<()> {
        // 诊断：追加式（点击种类 + 切换前态），序列可追溯
        let pre = is_chinese();
        log_diag(&format!("OnClick click={} pre_zh={pre}", click.0));
        if click.0 == 1 {
            // 右键：小菜单（码表切换 + 设置…）
            unsafe { popup_menu(pt) };
        } else {
            // 左键：切中英。【v3 曾误删】OnClick(click=2) 实际一直有
            // 送达（21:14 日志六笔铁证）；系统对自定义项【不会】自翻
            // compartment——驱动权全在我们。toggle 后按引擎回执刷新。
            match crate::ipc::call(&serde_json::json!({"op": "toggle_lang"})) {
                Some(resp) => {
                    let zh = resp
                        .pointer("/state/chinese")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(pre);
                    log_diag(&format!("toggle → engine_zh={zh}"));
                    set_mode(zh);
                }
                None => log_diag("toggle 管道失败"),
            }
        }
        Ok(())
    }

    fn InitMenu(&self, pmenu: Option<&windows::Win32::UI::TextServices::ITfMenu>) -> Result<()> {
        // 兼容走菜单协议的宿主：给一个设置项（右键主路在 OnClick 弹
        // Win32 菜单，码表清单在那里动态取）
        if let Some(m) = pmenu {
            let t: Vec<u16> = "设置…".encode_utf16().collect();
            unsafe {
                let _ = m.AddMenuItem(
                    1,
                    0, // TF_LBMENUF_NONE
                    windows::Win32::Graphics::Gdi::HBITMAP(std::ptr::null_mut()),
                    windows::Win32::Graphics::Gdi::HBITMAP(std::ptr::null_mut()),
                    &t,
                    std::ptr::null_mut(),
                );
            }
        }
        Ok(())
    }

    fn OnMenuSelect(&self, id: u32) -> Result<()> {
        if id == 1 {
            let _ = crate::ipc::call(&serde_json::json!({"op": "settings"}));
        }
        Ok(())
    }

    fn GetIcon(&self) -> Result<HICON> {
        Ok(HICON((if is_chinese() { self.icon_zh } else { self.icon_en }) as *mut _))
    }

    fn GetText(&self) -> Result<windows::core::BSTR> {
        // 固定「虎符」：不显示会冻结的中/英状态（见 new() 注释）
        Ok(windows::core::BSTR::from("虎符"))
    }
}

// ───────────────────────── 「中」「英」纯字符图标 ─────────────────────────
// 32×32 ARGB：GDI 画字符（雅黑粗体、灰度 AA）→ coverage，
// 【墨迹居中】——CJK 字符格含内隙，字格居中会整体偏上/偏下，与
// 相邻系统图标错位；这里按 coverage 的实际墨迹行盒重新居中。
// 白字核心 + 4 邻扩张黑晕（深/浅任务栏都可读），无底牌。
fn make_glyph_icon(ch: &str) -> isize {
    use windows::Win32::Graphics::Gdi::*;
    const S: i32 = 32;
    unsafe {
        let hdc = CreateCompatibleDC(None);
        let face: Vec<u16> = "Microsoft YaHei UI\0".encode_utf16().collect();
        let hf = CreateFontW(
            -31, // 撑满 32 画布（用户反馈要更大）
            0,
            0,
            0,
            700,
            0,
            0,
            0,
            0x86, // DEFAULT_CHARSET
            0,
            0,
            4, // ANTIALIASED_QUALITY（灰度 coverage）
            0,
            PCWSTR(face.as_ptr()),
        );
        let mut hdr = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: S,
                biHeight: -S,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(hdc, &hdr, DIB_RGB_COLORS, &mut bits, None, 0)
            .unwrap_or_default();
        if dib.is_invalid() || bits.is_null() || hf.is_invalid() {
            if !hf.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(hf.0));
            }
            let _ = DeleteDC(hdc);
            return 0;
        }
        let _ = SelectObject(hdc, HGDIOBJ(dib.0));
        let _ = SelectObject(hdc, HGDIOBJ(hf.0));
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, COLORREF(0x00FF_FF_FF));
        let ws: Vec<u16> = ch.encode_utf16().collect();
        let mut sz = SIZE { cx: 0, cy: 0 };
        let _ = GetTextExtentPoint32W(hdc, &ws, &mut sz);
        let _ = TextOutW(hdc, (S - sz.cx) / 2, (S - sz.cy) / 2, &ws);
        let _ = GdiFlush();
        // coverage = 蓝通道（白字 RGB 同值）
        let mut cov = vec![0f32; (S * S) as usize];
        for i in 0..(S * S) as usize {
            cov[i] = (bits as *const u8).add(i * 4).read_volatile() as f32 / 255.0;
        }
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteObject(HGDIOBJ(hf.0));
        let _ = DeleteDC(hdc);
        // 墨迹行盒 → 重定中心（整行平移 dy，消除字格内隙造成的错位）
        let mut ink_top = -1i32;
        let mut ink_bot = -1i32;
        for y in 0..S {
            let row_has = (0..S).any(|x| cov[(y * S + x) as usize] > 0.02);
            if row_has {
                if ink_top < 0 {
                    ink_top = y;
                }
                ink_bot = y;
            }
        }
        let dy = if ink_top >= 0 {
            // 目标墨心 = 画布中线；CJK 墨迹偏字格下方 → 通常上移
            ((S as f32 - 1.0) / 2.0 - (ink_top + ink_bot) as f32 / 2.0).round() as i32
        } else {
            0
        };
        let g = |x: i32, y: i32| -> f32 {
            // 平移后的采样（越界=空）
            let sy = y - dy;
            if x < 0 || sy < 0 || x >= S || sy >= S {
                0.0
            } else {
                cov[(sy * S + x) as usize]
            }
        };
        let mut out = vec![0u8; (S * S * 4) as usize];
        for y in 0..S {
            for x in 0..S {
                let c = g(x, y);
                let o = g(x - 1, y)
                    .max(g(x + 1, y))
                    .max(g(x, y - 1))
                    .max(g(x, y + 1));
                if c <= 0.0 && o <= 0.0 {
                    continue;
                }
                let a = c.max(o * 0.85);
                let w = c / a.max(1e-6); // 白占比（核心 1、晕 0）
                let v = (w * 245.0) as u8;
                let i = ((y * S + x) * 4) as usize;
                out[i] = v;
                out[i + 1] = v;
                out[i + 2] = v;
                out[i + 3] = (a * 255.0) as u8;
            }
        }
        bgra_to_hicon(&out, S, S)
    }
}

/// BGRA 缓冲 → HICON（CreateBitmap 直接带位 + CreateIconIndirect；
/// mask 全零单色位图，透明由 32bpp alpha 决定）
unsafe fn bgra_to_hicon(buf: &[u8], w: i32, h: i32) -> isize {
    use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject, HBITMAP};
    let color: HBITMAP = CreateBitmap(w, h, 1, 32, Some(buf.as_ptr() as *const _));
    let mask: HBITMAP = CreateBitmap(w, h, 1, 1, None);
    let info = ICONINFO {
        fIcon: windows::Win32::Foundation::BOOL(1),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color,
    };
    let hicon = CreateIconIndirect(&info).map(|h| h.0 as isize).unwrap_or(0);
    let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(color.0));
    let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(mask.0));
    hicon
}
