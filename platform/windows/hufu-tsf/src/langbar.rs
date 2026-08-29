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
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

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

/// 更新模式并广播所有 sink（msctf 重拉图标/文字）。返回是否有变化。
pub fn set_mode(zh: bool) -> bool {
    if CHINESE.swap(zh, Ordering::Relaxed) == zh {
        return false;
    }
    notify_sinks();
    push_compartments();
    true
}

// ═══════════════ compartment 同步：系统输入指示「中/A」的真身 ═══════════════
// 任务栏输入指示区的中/英牌是 EXPLORER 画的，读的是 TSF 转换模式
// compartment（微软拼音/Rime 同款路线）：
// - GUID_COMPARTMENT_KEYBOARD_OPENCLOSE=1（输入法开着）
// - GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION = NATIVE(中)/ALPHA(英)
// 写线程 compartment + 全局 compartment；并监听外部变化（用户点
// 系统牌 → 引擎跟上）。反馈环由「值与预期一致则忽略」掐断。

thread_local! {
    /// 本线程 compartment 源（Activate 时装；thread mgr 本身实现该接口）
    static COMP_THREAD: std::cell::RefCell<Option<ITfCompartmentMgr>> =
        const { std::cell::RefCell::new(None) };
    /// 全局 compartment 源（进程只装一次）
    static COMP_GLOBAL: std::cell::RefCell<Option<ITfCompartmentMgr>> =
        const { std::cell::RefCell::new(None) };
    static COMP_COOKIE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static COMP_CLIENT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}
static GLOBAL_DONE: AtomicBool = AtomicBool::new(false);

/// Activate 时调用：装 compartment 引用 + 挂监听 + 推初值。
/// tid = TSF client id（SetValue 需要）。
pub fn install_compartments(tm: &ITfThreadMgr, tid: u32) {
    COMP_CLIENT.with(|c| c.set(tid));
    if let Ok(cm) = tm.cast::<ITfCompartmentMgr>() {
        COMP_THREAD.with(|c| *c.borrow_mut() = Some(cm));
    }
    if !GLOBAL_DONE.swap(true, Ordering::Relaxed) {
        if let Ok(g) = unsafe { tm.GetGlobalCompartment() } {
            COMP_GLOBAL.with(|c| *c.borrow_mut() = Some(g));
        }
        // 全局监听只挂一次（进程存续期内常驻）
        advise_conversion(COMP_GLOBAL.with(|c| c.borrow().clone()), tid);
    }
    advise_conversion(COMP_THREAD.with(|c| c.borrow().clone()), tid);
    push_compartments();
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

/// 把当前模式推进 OPENCLOSE + CONVERSION（线程 + 全局）
pub fn push_compartments() {
    let conv: i32 = if is_chinese() {
        TF_CONVERSIONMODE_NATIVE as i32
    } else {
        TF_CONVERSIONMODE_ALPHANUMERIC as i32
    };
    let tid = COMP_CLIENT.with(|c| c.get());
    for src in [
        COMP_THREAD.with(|c| c.borrow().clone()),
        COMP_GLOBAL.with(|c| c.borrow().clone()),
    ]
    .into_iter()
    .flatten()
    {
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
        let sink: ITfCompartmentEventSink = ModeSink.into();
        let _ = src.AdviseSink(&ITfCompartmentEventSink::IID, &sink);
        // cookie 不存（进程存续期常驻；Deactivate 只摘线程那份）
    }
}

/// 外部改了转换模式（用户点系统牌）：值 ≠ 预期 → 引擎跟上切换；
/// 相等（自己的写入回声）→ 忽略。反馈环就此掐断。
#[implement(ITfCompartmentEventSink)]
struct ModeSink;

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
        // 读触发侧的当前值（线程优先，全局兜底）
        let mut actual: Option<i32> = None;
        for src in [
            COMP_THREAD.with(|c| c.borrow().clone()),
            COMP_GLOBAL.with(|c| c.borrow().clone()),
        ]
        .into_iter()
        .flatten()
        {
            if let Ok(comp) =
                unsafe { cm_compartment(&src, &GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION) }
            {
                if let Ok(v) = unsafe { comp.GetValue() } {
                    if let Some(n) = unsafe { var_i32(&v) } {
                        actual = Some(n);
                        break;
                    }
                }
            }
        }
        if let Some(n) = actual {
            if n != expected {
                if let Some(resp) = crate::ipc::call(&serde_json::json!({"op": "toggle_lang"})) {
                    let zh = resp
                        .pointer("/state/chinese")
                        .and_then(|v| v.as_bool())
                        .unwrap_or_else(is_chinese);
                    set_mode(zh); // 内部会推 compartment（此时值已一致，回声无害）
                } else {
                    push_compartments(); // 引擎不可达：把牌推回真实态
                }
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
        HuFuLangBar {
            icon_zh: make_glyph_icon("中"),
            icon_en: make_glyph_icon("英"),
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
    let item: ITfLangBarItem = unsafe { HuFuLangBar::new().into() };
    let r = unsafe { mgr.AddItem(&item) };
    // 诊断标记：Activate 后 %TEMP%\hufu-langbar.txt 可查挂载结果
    let msg = match &r {
        Ok(()) => format!(
            "ok pid={} zh={} t={:?}\n",
            std::process::id(),
            is_chinese(),
            std::time::SystemTime::now()
        ),
        Err(e) => format!("FAIL {:#010x} pid={}\n", e.code().0, std::process::id()),
    };
    let _ = std::fs::write(std::env::temp_dir().join("hufu-langbar.txt"), msg);
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
        Ok(windows::core::BSTR::from(if is_chinese() {
            "虎符输入法 · 中文模式（左键切英文，右键打开设置）"
        } else {
            "虎符输入法 · 英文模式（左键切中文，右键打开设置）"
        }))
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
    fn GetForegroundWindow() -> isize;
}
const MF_STRING: u32 = 0x0;
const MF_SEPARATOR: u32 = 0x800;
const MF_CHECKED: u32 = 0x8;
const TPM_RETURNCMD: u32 = 0x100;
const TPM_RIGHTALIGN: u32 = 0x8;
const TPM_BOTTOMALIGN: u32 = 0x20;

/// 右键小菜单：码表清单（当前 ✓）+ 分隔线 + 设置…
/// 返回 true 表示已处理（弹了菜单）；菜单动作就地执行。
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
        let wset: Vec<u16> = "设置…".encode_utf16().chain([0]).collect();
        AppendMenuW(m, MF_STRING, 1, wset.as_ptr());
        // 前台窗口作 owner（经典托盘菜单套路：保焦点使外部点击可撤销）
        let fg = GetForegroundWindow();
        let sel = TrackPopupMenu(
            m,
            TPM_RETURNCMD | TPM_RIGHTALIGN | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            0,
            fg,
            std::ptr::null(),
        );
        DestroyMenu(m);
        if sel == 1 {
            let _ = crate::ipc::call(&serde_json::json!({"op": "settings"}));
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
        // 诊断：点击路由确认（左右键实测排查用）
        let _ = std::fs::write(
            std::env::temp_dir().join("hufu-langbar-click.txt"),
            format!("click={} t={:?}\n", click.0, std::time::SystemTime::now()),
        );
        if click.0 == 1 {
            // 右键：小菜单（码表切换 + 设置…）
            unsafe { popup_menu(pt) };
        } else {
            // 左键：切换中英（引擎无条件清编码后切换，回填图标/文字）
            if let Some(resp) = crate::ipc::call(&serde_json::json!({"op": "toggle_lang"})) {
                let zh = resp
                    .pointer("/state/chinese")
                    .and_then(|v| v.as_bool())
                    .unwrap_or_else(is_chinese);
                set_mode(zh);
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
        Ok(windows::core::BSTR::from(if is_chinese() { "中" } else { "英" }))
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
