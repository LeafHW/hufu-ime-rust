//! QQ 会话列表 UIA 结构探针（STA+消息泵版）：定位置顶会话容器。
//!
//! 教训：QQ(Chromium) 的 DOM 只通过 MSAA(IAccessible) / 浏览器侧无障碍暴露，
//! 裸 UIA 客户端如果先碰到 Win32 默认 provider（隐藏的
//! Chrome_RenderWidgetHostHWND / Intermediate D3D Window），整棵树就只会剩
//! 1 个 HWND 子元素。因此探针先做 oleacc 预热（对**顶层**面板窗口取
//! OBJID_CLIENT 的 IAccessible 并持有），再建 UIA 客户端查询。
//!
//! 三种模式（env PROBE_MODE）：
//! - `msaa`（默认）：oleacc 预热 + 新建 UIA（不设短超时）
//! - `plain`：只新建 UIA（不设短超时、不预热）
//! - `bridge`：走产品 `uia_bridge::uia()` 的默认路径（1.5s 连接超时）
#![allow(clippy::all)]
#[path = "../src/qq_bridge.rs"]
mod qq_bridge;
#[path = "../src/uia_bridge.rs"]
mod uia_bridge;

use windows::core::{w, Interface};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement,
    IUIAutomationFocusChangedEventHandler, IUIAutomationFocusChangedEventHandler_Impl,
    TreeScope_Children, TreeScope_Descendants,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage,
    RegisterClassW, SetTimer, TranslateMessage, CS_HREDRAW, CS_VREDRAW, HWND_MESSAGE, MSG,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_DESTROY, WM_TIMER, WNDCLASSW,
};

static mut TICKS: u32 = 0;
/// oleacc 预热拿到的 IAccessible：整个探针期间持有，进程退出即回收。
static mut MSAA_PTR: *mut core::ffi::c_void = std::ptr::null_mut();

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

fn mode() -> String {
    std::env::var("PROBE_MODE").unwrap_or_else(|_| "msaa".into())
}

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
            .take(40)
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

fn descendants(uia: &IUIAutomation, e: &IUIAutomationElement) -> u32 {
    unsafe {
        uia.CreateTrueCondition()
            .ok()
            .and_then(|c| e.FindAll(TreeScope_Descendants, &c).ok())
            .and_then(|a| a.Length().ok())
            .unwrap_or(0) as u32
    }
}

/// 新建一个 UIA 客户端；`timeout` = Some(ms) 时设连接/事务超时（产品默认 1500ms）。
fn new_uia(timeout: Option<u32>) -> Option<IUIAutomation> {
    unsafe {
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        if let Some(ms) = timeout {
            if let Ok(u2) = uia.cast::<windows::Win32::UI::Accessibility::IUIAutomation2>() {
                let _ = u2.SetConnectionTimeout(ms);
                let _ = u2.SetTransactionTimeout(ms);
            }
        }
        Some(uia)
    }
}

/// oleacc 预热：对窗口取 OBJID_CLIENT 的 IAccessible（Chromium 由此开 DOM 无障碍）。
fn msaa_boot(hwnd: isize, label: &str) -> bool {
    use windows::Win32::UI::Accessibility::AccessibleObjectFromWindow;
    const OBJID_CLIENT_U32: u32 = 0xFFFFFFFC; // (u32)(-4)
    let iid_iaccessible =
        windows::core::GUID::from_u128(0x618716e0_ac8e_11d3_9815_00c04f2ef09f);
    let mut pv: *mut core::ffi::c_void = std::ptr::null_mut();
    let r = unsafe {
        windows::Win32::UI::Accessibility::AccessibleObjectFromWindow(
            HWND(hwnd as *mut core::ffi::c_void),
            OBJID_CLIENT_U32,
            &iid_iaccessible,
            &mut pv,
        )
    };
    let hr = match &r {
        Ok(()) => "OK".to_string(),
        Err(e) => format!("{e}"),
    };
    println!("[{label}] MSAA hr={hr} pv_null={}", pv.is_null());
    if !pv.is_null() {
        unsafe { MSAA_PTR = pv };
        true
    } else {
        false
    }
}

/// 单个元素的直属子树（class + name），用于看条目内部文本节点结构。
fn dump_item_children(uia: &IUIAutomation, e: &IUIAutomationElement, indent: &str) {
    unsafe {
        let Ok(cond) = uia.CreateTrueCondition() else {
            return;
        };
        let Ok(all) = e.FindAll(TreeScope_Descendants, &cond) else {
            return;
        };
        let n = all.Length().unwrap_or(0).min(24);
        for i in 0..n {
            let Ok(c) = all.GetElement(i) else { continue };
            let ct = format!("{:?}", c.CurrentControlType());
            println!(
                "{indent}   - [{ct}] cls='{}' name={:?}",
                cls_of(&c),
                name_of(&c)
            );
        }
    }
}

fn dump(uia: &IUIAutomation, hwnd: isize, tag: &str) {
    let root = match unsafe { uia.ElementFromHandle(HWND(hwnd as *mut core::ffi::c_void)) } {
        Ok(v) => v,
        Err(e) => {
            println!("[{tag}] ElementFromHandle failed: {e}");
            return;
        }
    };
    println!(
        "[{tag}] root cls='{}' name={:?} desc={}",
        cls_of(&root),
        name_of(&root),
        descendants(uia, &root)
    );
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
    println!("[{tag}] == class histogram 命中（contact/list/pin/top/item）==");
    for (c, n) in &hist {
        if c.contains("contact") || c.contains("list") || c.contains("pin") || c.contains("top")
            || c.contains("item")
        {
            println!("[{tag}]   {c} × {n}");
        }
    }

    let mut hits = Vec::new();
    walk(uia, &root, 0, 12, &mut hits, tag);
    println!("[{tag}] == contact/list 容器（d=depth, kids=直系子数）==");
    for (d, c, n, k) in &hits {
        println!("[{tag}]   d{d}  {c}  name={n:?}  kids={k}");
    }

    // 会话条目明细：类名 + 内部文本节点（置顶项/选中项的多 class 拼接在这里最明显）
    unsafe {
        if let Ok(cond) = uia.CreateTrueCondition() {
            if let Ok(all) = root.FindAll(TreeScope_Descendants, &cond) {
                let n = all.Length().unwrap_or(0).min(4000);
                let mut shown = 0usize;
                for i in 0..n {
                    let Ok(e) = all.GetElement(i) else { continue };
                    let c = cls_of(&e);
                    if !c.contains("recent-contact-item") {
                        continue;
                    }
                    println!("[{tag}] ITEM idx={i} cls='{c}'");
                    dump_item_children(uia, &e, &format!("[{tag}]"));
                    shown += 1;
                    if shown >= 24 {
                        break;
                    }
                }
            }
        }
    }

    let names = uia_bridge::list_sessions(hwnd);
    println!("[{tag}] == list_sessions() → {} 条 ==", names.len());
    for (i, s) in names.iter().enumerate().take(40) {
        println!("[{tag}]   {i}: {s}");
    }
}


/// 对其它进程的 Chrome_WidgetWin_1 窗口做同样的 UIA 读，判断是"客户端普遍不行"
/// 还是 QQ 特有（Chromium 家族都走同一套 provider）。
fn probe_other_chromium() {
    struct Ctx {
        rows: Vec<(isize, u32, String)>,
    }
    extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> windows::Win32::Foundation::BOOL {
        unsafe {
            let ctx = &mut *(lparam.0 as *mut Ctx);
            if ctx.rows.len() >= 8 {
                return windows::Win32::Foundation::BOOL(1);
            }
            if !windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd).as_bool() {
                return windows::Win32::Foundation::BOOL(1);
            }
            let mut cls = [0u16; 64];
            let n = windows::Win32::UI::WindowsAndMessaging::GetClassNameW(hwnd, &mut cls);
            let class = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
            if class != "Chrome_WidgetWin_1" {
                return windows::Win32::Foundation::BOOL(1);
            }
            let mut buf = [0u16; 256];
            let tn = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, &mut buf);
            let title = String::from_utf16_lossy(&buf[..tn.max(0) as usize]);
            if title == "QQ" {
                return windows::Win32::Foundation::BOOL(1);
            }
            let mut pid = 0u32;
            windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut pid));
            ctx.rows.push((hwnd.0 as isize, pid, title.chars().take(18).collect()));
            windows::Win32::Foundation::BOOL(1)
        }
    }
    let mut ctx = Ctx { rows: Vec::new() };
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::EnumWindows(
            Some(cb),
            LPARAM(&mut ctx as *mut Ctx as isize),
        );
    }
    for (hwnd, pid, title) in ctx.rows {
        if let Some(u) = new_uia(None) {
            let d = unsafe { u.ElementFromHandle(HWND(hwnd as *mut core::ffi::c_void)) }
                .map(|e| descendants(&u, &e))
                .unwrap_or(0);
            println!("[ctrl] hwnd={hwnd} pid={pid} title={title:?} UIA desc={d}");
        }
    }
}


/// MSAA 兜底探测：对面板自身与全部子 HWND 取 IAccessible，打印 accChildCount/accName。
/// 用途：判断 Chromium 的 DOM 是否只经 MSAA 暴露（DComp 下 legacy 窗是隐藏的，
/// IAccessible 可能挂在别的子窗上）。

fn probe_msaa(hwnd: isize, label: &str) {
    use windows::Win32::UI::Accessibility::IAccessible;
    let iid_iaccessible =
        windows::core::GUID::from_u128(0x618716e0_ac8e_11d3_9815_00c04f2ef09f);
    let mut pv: *mut core::ffi::c_void = std::ptr::null_mut();
    let r = unsafe {
        windows::Win32::UI::Accessibility::AccessibleObjectFromWindow(
            HWND(hwnd as *mut core::ffi::c_void),
            0xFFFFFFFC, // OBJID_CLIENT
            &iid_iaccessible,
            &mut pv,
        )
    };
    if r.is_err() || pv.is_null() {
        println!("[msaa] {label} hwnd={hwnd} FAIL");
        return;
    }
    let acc: IAccessible = unsafe { std::mem::transmute(pv) };
    let n = unsafe { acc.accChildCount().unwrap_or(-1) };
    let mut name = String::new();
    // accName(idxChild=0) = 自身名字
    if let Ok(v) = unsafe { acc.get_accName(&windows::core::VARIANT::from(0i32)) } {
        name = v.to_string();
    }
    println!("[msaa] {label} hwnd={hwnd} OK children={n} name={name:?}");
    // 列出前 20 个子节点名字（若为 VT_I4 简单子元素则读 accName）
    for i in 1..=n.min(20) {
        let idx = windows::core::VARIANT::from(i);
        if let Ok(child) = unsafe { acc.get_accChild(&idx) } {
            if let Ok(ca) = child.cast::<IAccessible>() {
                let cn = unsafe {
                    ca.get_accName(&windows::core::VARIANT::from(0i32))
                        .ok()
                        .map(|v| v.to_string())
                        .unwrap_or_default()
                };
                let cc = unsafe { ca.accChildCount().unwrap_or(-1) };
                println!("[msaa]   child[{i}] name={cn:?} kids={cc}");
            } else {
                // 简单子元素：直接用父的 accName(i)
                let cn = unsafe {
                    acc.get_accName(&idx).ok().map(|v| v.to_string()).unwrap_or_default()
                };
                println!("[msaa]   child[{i}] (simple) name={cn:?}");
            }
        }
    }
}


/// 离线校验：把本机真实 UIA 数据（2026-11-09 NTQQ 9.9.36，取自 .NET UIA 客户端
/// 的 dump：类名 + Text 节点序列）回放进修复后的匹配逻辑，验证置顶条目不再被丢。
/// 说明：本探针在部分环境里只拿到 Win32 默认 provider（desc≈1），实时树不可读，
/// 这一段是**同一份真实数据的离线校验**，不替代实时读取。
fn replay_real_rows() {
    let rows: &[(&str, &[&str])] = &[
        (
            "recent-contact-item recent-contact-item--top rc-pin-item--read",
            &["虎码训练", "♂", "营", "23:38", "Z-E-R-O.5梦：", "@晴", "我只是打这个文打熟了"],
        ),
        (
            "recent-contact-item",
            &["群助手", "23:41", "[", "@全体成员", "]", "18个群聊有新消息"],
        ),
        (
            "recent-contact-item recent-contact-item--selected",
            &["虎码新手村2群", "23:41", "芝士798：", "我现在就是中转", "19"],
        ),
        (
            "recent-contact-item",
            &["五笔修炼基地（小鹤/郑码/仓颉）", "23:41", "かなた：", "随‏便　149116", "8"],
        ),
        (
            "recent-contact-item",
            &["仙鹤•养殖场", "23:40", "[", "有新文件", "]", "龟心四贱：", "[图片]", "99+"],
        ),
        (
            "recent-contact-item",
            &["月计神社6群-蜘蛛巢分社（老红白粉丝群⑥群）", "23:39", "[", "有新文件", "]"],
        ),
        ("recent-contact-item", &["小鹤双拼新手村", "23:31", "阿胡：", "[图片]"]),
        ("recent-contact-item", &["QQ邮箱提醒", "23:25", "810439322: 晴发文 - 验证码"]),
        ("recent-contact-item", &["百鹤岛", "23:24", "梦：", "[图片]"]),
        ("recent-contact-item", &["聚贤阁", "23:15", "七星级嫖虫：", "火旺是什么东西啊", "99+"]),
        ("recent-contact-item", &["虎魄&Pain&虎杯", "23:11", "leaf：", "大差不差"]),
        ("recent-contact-item", &["睡大摸的", "🦒", "屋", "22:42", "红鲤鱼：", "外号", "99+"]),
        ("recent-contact-item", &["092五笔正规闲聊群④", "22:36", "[", "有新文件", "]"]),
        ("recent-contact-item", &["虎码输入法-新手村", "22:11", "石崇海：", "这咋了都", "3"]),
        ("recent-contact-item", &["落云宗", "22:07", "北海：", "挺好的！"]),
        ("recent-contact-item", &["转转回收群", "20:36", "出村是骗局：", "不凹了", "99+"]),
    ];
    let names: Vec<String> = rows
        .iter()
        .filter(|(cls, _)| uia_bridge::class_matches(cls, "recent-contact-item"))
        .map(|(_, texts)| uia_bridge::join_item_texts(texts))
        .filter(|n| !n.is_empty())
        .collect();
    println!("== 离线校验（真实 QQ 会话条目回放）→ {} 条 ==", names.len());
    for (i, n) in names.iter().enumerate() {
        let mark = if n.contains("虎码训练") { "  ← 置顶群（修复前丢失）" } else { "" };
        println!("  {i}: {n}{mark}");
    }
    let pinned = names.iter().any(|n| n == "虎码训练♂营");
    println!("pinned_present={pinned}");
}

fn walk(
    uia: &IUIAutomation,
    e: &IUIAutomationElement,
    depth: usize,
    max_depth: usize,
    hits: &mut Vec<(usize, String, String, usize)>,
    tag: &str,
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
            println!("[{tag}]      └─ {}", sub.join(" | "));
        }
    }
    for k in children(uia, e) {
        walk(uia, &k, depth + 1, max_depth, hits, tag);
    }
}

fn find_render_widget(root: isize) -> Option<isize> {
    struct Ctx {
        found: Option<isize>,
    }
    extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> windows::Win32::Foundation::BOOL {
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
        let _ = windows::Win32::UI::WindowsAndMessaging::EnumChildWindows(
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
            let t = {
                let v = TICKS;
                TICKS = v + 1;
                v
            };
            let mode = mode();
            let Some(p) = qq_bridge::main_panel() else {
                println!("NO_PANEL");
                PostQuitMessage(0);
                return LRESULT(0);
            };
            if t == 0 {
                println!("mode={mode} panel hwnd={} title={:?}", p.hwnd, p.title);
                // 顺序实验：有些 Win11 环境下 legacy CUIAutomation 会把进程绑到旧 core，
                // 先建 CUIAutomation8 再查，看是否会走 UiaManager 新通道。
                if std::env::var("PROBE_FIRST").as_deref() == Ok("c8") {
                    unsafe {
                        if let Ok(u8) = CoCreateInstance::<_, IUIAutomation>(
                            &windows::Win32::UI::Accessibility::CUIAutomation8,
                            None,
                            CLSCTX_INPROC_SERVER,
                        ) {
                            let d = u8
                                .ElementFromHandle(HWND(p.hwnd as *mut core::ffi::c_void))
                                .map(|e| descendants(&u8, &e))
                                .unwrap_or(0);
                            println!("[first-c8] panel desc={d}");
                        }
                    }
                }
                // MTA 线程对照：UIA 客户端官方推荐 MTA
                {
                    let hwnd2 = p.hwnd;
                    std::thread::spawn(move || unsafe {
                        let hr = windows::Win32::System::Com::CoInitializeEx(
                            None,
                            windows::Win32::System::Com::COINIT_MULTITHREADED,
                        );
                        println!("[mta] CoInitialize MTA hr={:X}", hr.0);
                        if let Ok(u) = CoCreateInstance::<_, IUIAutomation>(
                            &CUIAutomation,
                            None,
                            CLSCTX_INPROC_SERVER,
                        ) {
                            let d = u
                                .ElementFromHandle(HWND(hwnd2 as *mut core::ffi::c_void))
                                .map(|e| descendants(&u, &e))
                                .unwrap_or(0);
                            println!("[mta] uia panel desc={d}");
                        }
                        if let Ok(u8) = CoCreateInstance::<_, IUIAutomation>(
                            &windows::Win32::UI::Accessibility::CUIAutomation8,
                            None,
                            CLSCTX_INPROC_SERVER,
                        ) {
                            let d = u8
                                .ElementFromHandle(HWND(hwnd2 as *mut core::ffi::c_void))
                                .map(|e| descendants(&u8, &e))
                                .unwrap_or(0);
                            println!("[mta] uia8 panel desc={d}");
                        }
                    })
                    .join()
                    .ok();
                }
                println!("render_widget={:?}", find_render_widget(p.hwnd));
                if mode == "msaa" {
                    // 关键顺序：先对**顶层**面板 HWND 做 oleacc 预热，再建 UIA 客户端
                    msaa_boot(p.hwnd, "panel-top");
                    // 顺带看看渲染子窗与"视觉树"窗口
                    if let Some(rw) = find_render_widget(p.hwnd) {
                        msaa_boot(rw, "render-widget");
                    }
                }
                unsafe {
                    println!("UiaClientsAreListening={}", windows::Win32::UI::Accessibility::UiaClientsAreListening().as_bool());
                }
                // MSAA 兜底探测：面板 + 全部子窗
                {
                    let mut hwnds = vec![p.hwnd];
                    if let Some(rw) = find_render_widget(p.hwnd) {
                        hwnds.push(rw);
                    }
                    // 面板直接子窗（Intermediate D3D Window 等）
                    struct C2 {
                        v: Vec<isize>,
                    }
                    extern "system" fn cb2(
                        hwnd: HWND,
                        lparam: LPARAM,
                    ) -> windows::Win32::Foundation::BOOL
                    {
                        unsafe {
                            let c = &mut *(lparam.0 as *mut C2);
                            if c.v.len() < 8 {
                                c.v.push(hwnd.0 as isize);
                            }
                            windows::Win32::Foundation::BOOL(1)
                        }
                    }
                    let mut c2 = C2 { v: Vec::new() };
                    unsafe {
                        let _ = windows::Win32::UI::WindowsAndMessaging::EnumChildWindows(
                            HWND(p.hwnd as *mut core::ffi::c_void),
                            Some(cb2),
                            LPARAM(&mut c2 as *mut C2 as isize),
                        );
                    }
                    hwnds.extend(c2.v);
                    for h in hwnds {
                        probe_msaa(h, "dump");
                    }
                }
                // 对照：其它 Chromium 应用（Chrome/ChatGPT/DSH 等）在同一客户端能否读出树
                probe_other_chromium();
                // CUIAutomation8（UIA3）：.NET 客户端走的就是新 core，试它是否带 UiaManager
                unsafe {
                    if let Ok(u8) = CoCreateInstance::<_, IUIAutomation>(
                        &windows::Win32::UI::Accessibility::CUIAutomation8,
                        None,
                        CLSCTX_INPROC_SERVER,
                    ) {
                        let d = u8
                            .ElementFromHandle(HWND(p.hwnd as *mut core::ffi::c_void))
                            .map(|e| descendants(&u8, &e))
                            .unwrap_or(0);
                        println!("uia(CUIAutomation8) panel desc={d}");
                    } else {
                        println!("uia(CUIAutomation8) create FAILED");
                    }
                }
                let h: IUIAutomationFocusChangedEventHandler = NoopHandler.into();
                if let Some(u) = new_uia(None) {
                    let _ = u.AddFocusChangedEventHandler(None, &h);
                    let d = u
                        .ElementFromHandle(HWND(p.hwnd as *mut core::ffi::c_void))
                        .map(|e| descendants(&u, &e))
                        .unwrap_or(0);
                    println!("uia(new,timeout=None) panel desc={d}");
                }
                // 产品默认路径（1.5s 超时）对照
                if let Some(u) = new_uia(Some(1500)) {
                    let _ = u.AddFocusChangedEventHandler(None, &h);
                    let d = u
                        .ElementFromHandle(HWND(p.hwnd as *mut core::ffi::c_void))
                        .map(|e| descendants(&u, &e))
                        .unwrap_or(0);
                    println!("uia(new,timeout=1500) panel desc={d}");
                }
                // 事件处理器活到进程结束（UiaClientsAreListening→true）
                std::mem::forget(h);
            }
            if mode == "msaa" {
                // 反复请求：Chromium 的 IAccessible 可能只在"无障碍已开"后才挂上
                msaa_boot(p.hwnd, &format!("top-t{t}"));
                if t == 2 {
                    let ok = unsafe {
                        windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(HWND(
                            p.hwnd as *mut core::ffi::c_void,
                        ))
                    };
                    println!("t2 SetForegroundWindow(QQ) ok={}", ok.as_bool());
                }
            }
            // 每 tick 用同一实例复测，看无障碍是否被点亮
            if let Some(u) = { new_uia(None) } {
                if t < 8 {
                    let d = u
                        .ElementFromHandle(HWND(p.hwnd as *mut core::ffi::c_void))
                        .map(|e| descendants(&u, &e))
                        .unwrap_or(0);
                    println!("tick {t} uia(new) panel desc={d}");
                }
                if t == 8 {
                    dump(&u, p.hwnd, "t8-new");
                }
            }
            if t == 12 {
                replay_real_rows();
                let names = uia_bridge::list_sessions(p.hwnd);
                println!("== t12 uia_bridge::list_sessions → {} 条 ==", names.len());
                for (i, s) in names.iter().enumerate().take(40) {
                    println!("  {i}: {s}");
                }
                PostQuitMessage(0);
                return LRESULT(0);
            }
            return LRESULT(1);
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
        SetTimer(hwnd, 1, 700, None);
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
