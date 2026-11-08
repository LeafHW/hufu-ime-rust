//! QQ窗口识别与显式复制。跟打器不注入按键、不自动粘贴/回车、不覆盖QQ草稿。
//! - 聊天窗是独立顶层窗口，标题 = 群名/好友名（class Chrome_WidgetWin_1，
//!   进程名 QQ）；主面板标题恒为 "QQ" 需排除；
//! - QQ发文仅明确复制后打开目标窗口，由用户确认并发送；
//! - 群载文只从稳定会话的UIA正文读取，失败提示手动复制。

use std::ffi::c_void;
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, SwitchToThisWindow, SW_RESTORE,
};

/// 一个可选择的聊天窗口（标题 = 群名/好友名）
#[derive(Clone, Debug, PartialEq)]
pub struct ChatWindow {
    pub title: String,
    pub hwnd: isize,
}

struct EnumCtx {
    out: Vec<ChatWindow>,
}

extern "system" fn enum_cb(hwnd: HWND, lparam: windows::Win32::Foundation::LPARAM) -> BOOL {
    unsafe {
        let ctx = &mut *(lparam.0 as *mut EnumCtx);
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        // 最小化的窗不列（SwitchToThisWindow 虽能恢复，但列表应以"打开的聊天窗"为准）
        if IsIconic(hwnd).as_bool() {
            return BOOL(1);
        }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n <= 0 {
            return BOOL(1);
        }
        let title = String::from_utf16_lossy(&buf[..n as usize]);
        // 排除：主面板（标题恰为 "QQ"）、图片查看器、「群聊的聊天记录」侧载窗
        // （NTQQ 的记录侧载窗时隐时现，不是跟打目标；独立群聊窗标题=群名）。
        if title == "QQ" || title.contains("图片查看器") || title.contains("聊天记录") {
            return BOOL(1);
        }
        // 类名：NTQQ = Chrome_WidgetWin_1；老 QQ = TXGuiFoundation
        let mut cls = [0u16; 64];
        let cn = GetClassNameW(hwnd, &mut cls);
        let class = String::from_utf16_lossy(&cls[..cn.max(0) as usize]);
        if class != "Chrome_WidgetWin_1" && class != "TXGuiFoundation" {
            return BOOL(1);
        }
        // 只接受可验证的QQ进程；拿不到身份时不列为目标。
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if !process_name(pid)
            .map(|p| p.eq_ignore_ascii_case("QQ"))
            .unwrap_or(false)
        {
            return BOOL(1);
        }
        ctx.out.push(ChatWindow {
            title,
            hwnd: hwnd.0 as isize,
        });
        BOOL(1)
    }
}

fn process_name(pid: u32) -> Option<String> {
    // 实测（本机）：PROCESS_QUERY_INFORMATION|VM_READ 打不开 QQ（对话级保护），
    // PROCESS_QUERY_LIMITED_INFORMATION 总是可行且足够拿到镜像路径。
    // Toolhelp32 快照在此环境只见部分进程，不可用。
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            h,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        let stem = full.rsplit(['\\', '/']).next().unwrap_or("");
        let stem = stem.strip_suffix(".exe").unwrap_or(stem);
        Some(stem.to_string())
    }
}

/// 枚举当前所有可选聊天窗（QQ / 微信，排除主面板）
pub fn list_chat_windows() -> Vec<ChatWindow> {
    let mut ctx = EnumCtx { out: Vec::new() };
    unsafe {
        let _ = EnumWindows(
            Some(enum_cb),
            windows::Win32::Foundation::LPARAM(&mut ctx as *mut EnumCtx as isize),
        );
    }
    ctx.out
}

/// 整改7：QQ 读取屏的实时会话列表 = 主面板 UIA 会话列表 ∪ 独立聊天窗，
/// 按标题去重（主面板优先——read_current_article 的核验路径挂主面板；
/// 独立窗兜底，无主面板时也可选）。QQ 未开 → 空列表（调用方提示"未检测到 QQ"）。
pub fn list_sessions_merged() -> Vec<ChatWindow> {
    fn push_unique(out: &mut Vec<ChatWindow>, w: ChatWindow) {
        if !w.title.is_empty() && !out.iter().any(|c| c.title == w.title) {
            out.push(w);
        }
    }
    let mut out = Vec::new();
    if let Some(p) = main_panel() {
        for name in crate::uia_bridge::list_sessions(p.hwnd) {
            push_unique(
                &mut out,
                ChatWindow {
                    title: name,
                    hwnd: p.hwnd,
                },
            );
        }
    }
    for w in list_chat_windows() {
        push_unique(&mut out, w);
    }
    out
}

/// 找 QQ 主面板 hwnd（标题恰为 "QQ"、可见、QQ/微信进程）。
/// 群选会话/抓文/发成绩的 UIA 路径都挂在主面板上（不再要求独立聊天窗）。
pub fn main_panel() -> Option<ChatWindow> {
    struct Ctx {
        found: Option<isize>,
    }
    extern "system" fn cb(hwnd: HWND, lparam: windows::Win32::Foundation::LPARAM) -> BOOL {
        unsafe {
            let ctx = &mut *(lparam.0 as *mut Ctx);
            if ctx.found.is_some() || !IsWindowVisible(hwnd).as_bool() {
                return BOOL(1);
            }
            let mut buf = [0u16; 512];
            let n = GetWindowTextW(hwnd, &mut buf);
            if n <= 0 {
                return BOOL(1);
            }
            let title = String::from_utf16_lossy(&buf[..n as usize]);
            if title != "QQ" {
                return BOOL(1);
            }
            let mut cls = [0u16; 64];
            let cn = GetClassNameW(hwnd, &mut cls);
            let class = String::from_utf16_lossy(&cls[..cn.max(0) as usize]);
            if class != "Chrome_WidgetWin_1" && class != "TXGuiFoundation" {
                return BOOL(1);
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if !process_name(pid)
                .map(|p| p.eq_ignore_ascii_case("QQ"))
                .unwrap_or(false)
            {
                return BOOL(1);
            }
            ctx.found = Some(hwnd.0 as isize);
            BOOL(1)
        }
    }
    let mut ctx = Ctx { found: None };
    unsafe {
        let _ = EnumWindows(
            Some(cb),
            windows::Win32::Foundation::LPARAM(&mut ctx as *mut Ctx as isize),
        );
    }
    ctx.found.map(|hwnd| ChatWindow {
        title: "QQ".into(),
        hwnd,
    })
}

/// 窗口是否仍有效（关掉的群窗要自动解绑）
pub fn hwnd_alive(hwnd: isize) -> bool {
    unsafe { IsWindowVisible(HWND(hwnd as *mut c_void)).as_bool() }
}

/// 按标题重找窗口（重启 QQ 后 hwnd 会变，按标题兜底重绑）
pub fn find_by_title(title: &str) -> Option<ChatWindow> {
    let mut matches = list_chat_windows().into_iter().filter(|w| w.title == title);
    let first = matches.next()?;
    if matches.next().is_some() {
        None
    } else {
        Some(first)
    }
}

/// 激活窗口（含最小化恢复），等待 UI 就绪。
/// 返回 false = 激活失败（窗口已不可用），调用方应中止发送/抓取。
fn activate(hwnd: isize) -> bool {
    unsafe {
        let h = HWND(hwnd as *mut c_void);
        if IsIconic(h).as_bool() {
            // ShowWindow(SW_RESTORE) 会解除最小化；SwitchToThisWindow 对
            // 最小化窗行为不稳定
            let _ = ShowWindow(h, SW_RESTORE);
        }
        SwitchToThisWindow(h, true);
        let _ = SetForegroundWindow(h);
    }
    thread::sleep(Duration::from_millis(280));
    unsafe { GetForegroundWindow().0 as isize == hwnd }
}

/// Fail closed if the HWND now belongs to another process or cannot be inspected.
pub fn is_qq_window(hwnd: isize) -> bool {
    unsafe {
        let h = HWND(hwnd as *mut c_void);
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if !process_name(pid)
            .map(|n| n.eq_ignore_ascii_case("QQ"))
            .unwrap_or(false)
        {
            return false;
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(h, &mut class);
        matches!(
            String::from_utf16_lossy(&class[..n.max(0) as usize]).as_str(),
            "Chrome_WidgetWin_1" | "TXGuiFoundation"
        )
    }
}

/// Copy is a preparation step, never a successful send. The caller should show
/// “已复制，请在QQ目标群粘贴发送”. No draft is read, overwritten or submitted.
pub fn copy_and_show_chat(chat_hwnd: isize, text: &str) -> Result<(), String> {
    if text.is_empty() || text.len() > 1024 * 1024 {
        return Err("待复制内容为空或过大".into());
    }
    if !is_qq_window(chat_hwnd) {
        return Err("QQ目标窗口已失效，请重新选择".into());
    }
    unsafe {
        gale_win::clipboard_bridge::try_write_text(text)?;
    }
    if !activate(chat_hwnd) {
        return Err("已复制，但无法打开QQ；请手动切到目标群粘贴".into());
    }
    Ok(())
}

/// 【群三件套】选群即切换：独立聊天窗直接置前；主面板条目（hwnd=主面板）
/// 无法程序化切会话，激活面板并返回 false，由调用方提示用户点开该群。
pub fn focus_chat(chat: &ChatWindow) -> bool {
    if let Some(p) = main_panel() {
        if p.hwnd == chat.hwnd {
            let _ = activate(p.hwnd);
            return false;
        }
    }
    activate(chat.hwnd)
}

/// 【群三件套 2026-11-09】增强发送（可行性报告 · 通道C 增强模式③）：
/// 复制 → 激活 QQ → UIA 校验当前会话标题 → Ctrl+V → 回车。
/// 任一步失败立即中止并保留剪贴板，绝不错发到别的群。
/// 首次 UIA 连接可能返回不完整树，故激活后等待再校验（200ms 级）。
pub fn send_message_enhanced(chat: &ChatWindow, text: &str) -> Result<(), String> {
    if text.is_empty() || text.len() > 1024 * 1024 {
        return Err("待发送内容为空或过大".into());
    }
    if !is_qq_window(chat.hwnd) {
        return Err("QQ目标窗口已失效，请重新选择".into());
    }
    // 1) 先写剪贴板（失败不激活、不动 QQ）
    unsafe {
        gale_win::clipboard_bridge::try_write_text(text)?;
    }
    // 2) 激活目标窗口
    if !activate(chat.hwnd) {
        return Err("已复制，但无法打开QQ；请手动切到目标群粘贴".into());
    }
    // 3) 等 Chromium UIA 树稳定后校验当前会话就是目标群
    thread::sleep(Duration::from_millis(220));
    let cur = crate::uia_bridge::current_session_title(chat.hwnd);
    match cur {
        Some(name) if name == chat.title || name.contains(&chat.title) => {}
        Some(other) => {
            return Err(format!(
                "已中止：QQ 当前会话是「{other}」不是「{}」，内容已留在剪贴板",
                chat.title
            ))
        }
        None => {
            return Err("已中止：无法校验 QQ 当前会话（UIA 读取失败），内容已留在剪贴板".into())
        }
    }
    // 4) 粘贴 + 回车
    gale_win::keys::paste_and_enter();
    Ok(())
}

/// UIA-only reading replaces unsafe Ctrl+A/C injection (which can copy an old
/// draft, an unrelated view, or a stale clipboard). Failure leaves clipboard intact.
pub fn fetch_text_from(chat_hwnd: isize, _gale_hwnd: isize) -> Result<String, String> {
    if !is_qq_window(chat_hwnd) {
        return Err("QQ目标窗口已失效".into());
    }
    let title = crate::uia_bridge::current_session_title(chat_hwnd)
        .ok_or("QQ当前版本无法读取会话，请手动复制跟打文")?;
    crate::uia_bridge::read_current_article(chat_hwnd, &title)
}
