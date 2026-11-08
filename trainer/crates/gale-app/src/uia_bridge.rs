//! QQ NT 主面板的 UI Automation 桥（茧跟打/雨天系 GClass29 同款思路）。
//!
//! NTQQ 是 Electron 窗口，主面板内嵌「会话列表 + 消息区 + ProseMirror 输入框」，
//! 全部通过 UIA 树暴露。因此**无需弹出独立聊天窗、无需窗口激活、无需按键注入**：
//! - 列会话：会话列表容器（cls 含 recent-contact-list）下的 recent-contact-item
//! - 切换会话：对列表项 Invoke()（程序化点击）
//! - 抓消息：消息区容器（cls 含 chat-msg-area__vlist）内 Text 节点 Name
//! - 发文/成绩：只复制并打开QQ，由用户确认目标和草稿后手动发送
//!
//! 实测（2026-10-01，NTQQ 9.9.36）：会话项名会按字符拆分（“虎码训练”“♂”“营”），
//! 拼接后匹配；消息文本节点 Name 为整段消息内容，跟打文“-----第N段-----”直接可解析。
#![allow(dead_code)]

use windows::core::Interface;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationElement, IUIAutomationInvokePattern,
    TreeScope_Descendants, UIA_ControlTypePropertyId, UIA_InvokePatternId, UIA_TextControlTypeId,
};

/// UIA 实例按线程各持一份（thread_local）：
/// - 主循环线程：OleInitialize(STA) 已就位，CoInitializeEx(STA) 返回
///   S_FALSE（已初始化，合法）——同旧 OnceLock 行为。
/// - 成绩发送线程（2026-10 群卡顿修复）：CoInitializeEx(MTA) 先行
///   （UIA 客户端官方推荐 MTA），这里 STA 请求返回 RPC_E_CHANGED_MODE——
///   无妨，COM 已初始化即可直接 CoCreateInstance。
///   此前 OnceLock 全进程一份、跨套间共用接口指针——MTA 线程借 STA
///   套间代理虽"能跑"，但每次调用跨套间 marshal，且时序上不保证主线程
///   已先初始化。thread_local 让每线程走自己的套间，干净且无锁。
fn uia() -> Option<IUIAutomation> {
    thread_local! {
        static UIA: Option<IUIAutomation> = unsafe {
            // Err（含 RPC_E_CHANGED_MODE：线程已是别的套间模型）不致命——
            // COM 已初始化的线程照样能创建组件
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let value: Option<IUIAutomation> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok();
            if let Some(u) = &value {
                if let Ok(u2) = u.cast::<IUIAutomation2>() {
                    let _ = u2.SetConnectionTimeout(1500);
                    let _ = u2.SetTransactionTimeout(1500);
                }
            }
            value
        };
    }
    UIA.with(|u| u.clone())
}

/// QQ 主面板（HWND）的 UIA 根元素。
fn panel_root(uia: &IUIAutomation, hwnd: isize) -> Option<IUIAutomationElement> {
    if !crate::qq_bridge::is_qq_window(hwnd) {
        return None;
    }
    unsafe {
        uia.ElementFromHandle(windows::Win32::Foundation::HWND(hwnd as *mut _))
            .ok()
    }
}

/// Search the accessibility tree without assuming QQ containers are Pane/Group.
/// QQ has shipped the contact list as List; UIA control type is not a stable selector.
fn find_by_class_prefix(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    prefix: &str,
) -> Option<IUIAutomationElement> {
    find_class(uia, root, |name| name.starts_with(prefix))
}
fn find_by_class_contains(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    needle: &str,
) -> Option<IUIAutomationElement> {
    find_class(uia, root, |name| name.contains(needle))
}
fn find_class(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    predicate: impl Fn(&str) -> bool,
) -> Option<IUIAutomationElement> {
    unsafe {
        let all = root
            .FindAll(TreeScope_Descendants, &uia.CreateTrueCondition().ok()?)
            .ok()?;
        for i in 0..all.Length().ok()?.min(10_000) {
            let el = all.GetElement(i).ok()?;
            if el
                .CurrentClassName()
                .ok()
                .map(|n| predicate(&n.to_string()))
                .unwrap_or(false)
            {
                return Some(el);
            }
        }
    }
    None
}

/// 元素下全部 Text 节点 Name 逐行拼接（多行消息体保结构）。
fn child_texts_joined(uia: &IUIAutomation, el: &IUIAutomationElement) -> String {
    let mut out = String::new();
    unsafe {
        let Ok(cond) = uia.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &windows::core::VARIANT::from(UIA_TextControlTypeId.0),
        ) else {
            return out;
        };
        let Ok(texts) = el.FindAll(TreeScope_Descendants, &cond) else {
            return out;
        };
        for i in 0..texts.Length().unwrap_or(0).min(10_000) {
            let Ok(te) = texts.GetElement(i) else {
                continue;
            };
            if let Ok(s) = te.CurrentName() {
                let s = s.to_string();
                let s = s.trim();
                if !s.is_empty() {
                    // Oversized bodies are rejected whole; never train on silently truncated text.
                    if out.len().saturating_add(s.len()).saturating_add(1) > 1024 * 1024 {
                        return String::new();
                    }
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(s);
                }
            }
        }
    }
    out
}

/// 会话列表容器。
fn session_list(uia: &IUIAutomation, root: &IUIAutomationElement) -> Option<IUIAutomationElement> {
    find_by_class_contains(uia, root, "recent-contact-list")
}

/// 会话项内前 limit 个 Text 子节点 Name 拼接（NTQQ 把“虎码训练♂营”拆成 3 段；
/// 时间戳（含':'）及其后的摘要节点被跳过）。
fn item_texts_joined(uia: &IUIAutomation, item: &IUIAutomationElement, limit: i32) -> String {
    let mut joined = String::new();
    unsafe {
        let Ok(cond) = uia.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &windows::core::VARIANT::from(UIA_TextControlTypeId.0),
        ) else {
            return joined;
        };
        let Ok(texts) = item.FindAll(TreeScope_Descendants, &cond) else {
            return joined;
        };
        for i in 0..texts.Length().unwrap_or(0).min(limit) {
            let Ok(te) = texts.GetElement(i) else {
                continue;
            };
            if let Ok(s) = te.CurrentName() {
                let s = s.to_string();
                // 时间戳（21:33）或未读标记之后的节点不是名字
                if s.contains(':') || s.contains("未读") {
                    break;
                }
                joined.push_str(s.trim());
            }
        }
    }
    joined
}

/// Recent conversations are display names, not verified group IDs. Keep duplicates so
/// the selection path can reject ambiguous names rather than silently picking one.
pub fn list_sessions(hwnd: isize) -> Vec<String> {
    let Some(uia) = uia() else { return vec![] };
    let Some(root) = panel_root(&uia, hwnd) else {
        return vec![];
    };
    let list = session_list(&uia, &root).or_else(|| {
        std::thread::sleep(std::time::Duration::from_millis(250));
        session_list(&uia, &root)
    });
    let Some(list) = list else { return vec![] };
    unsafe {
        let Ok(cond) = uia.CreateTrueCondition() else {
            return vec![];
        };
        let Ok(items) = list.FindAll(TreeScope_Descendants, &cond) else {
            return vec![];
        };
        let mut names = Vec::new();
        for i in 0..items.Length().unwrap_or(0).min(2000) {
            let Ok(item) = items.GetElement(i) else {
                continue;
            };
            if item
                .CurrentClassName()
                .ok()
                .map(|n| n.to_string().starts_with("recent-contact-item"))
                .unwrap_or(false)
            {
                let name = item_texts_joined(&uia, &item, 3).trim().to_string();
                if !name.is_empty() {
                    names.push(name);
                }
            }
        }
        names
    }
}

/// 当前打开会话的标题（chat-header__contact-name 的 Name）。
pub fn current_session_title(hwnd: isize) -> Option<String> {
    let uia = uia()?;
    let root = panel_root(&uia, hwnd)?;
    let hdr = find_by_class_prefix(&uia, &root, "chat-header__contact-name")?;
    unsafe {
        let name = hdr.CurrentName().ok()?.to_string();
        let name = if name.trim().is_empty() {
            child_texts_joined(&uia, &hdr)
        } else {
            name
        };
        if name.trim().is_empty() {
            None
        } else {
            Some(name)
        }
    }
}

/// Select only one exact, unique conversation via its own Invoke pattern.
/// Never click screen coordinates, inject keyboard events, or use contains(name).
/// If QQ exposes no safe Invoke pattern, ask the user to open that conversation.
pub fn switch_session(hwnd: isize, name: &str) -> Option<String> {
    let uia = uia()?;
    let root = panel_root(&uia, hwnd)?;
    let list = session_list(&uia, &root)?;
    unsafe {
        let all = list
            .FindAll(TreeScope_Descendants, &uia.CreateTrueCondition().ok()?)
            .ok()?;
        let mut matching = Vec::new();
        for i in 0..all.Length().ok()?.min(2000) {
            let item = all.GetElement(i).ok()?;
            if !item
                .CurrentClassName()
                .ok()?
                .to_string()
                .starts_with("recent-contact-item")
            {
                continue;
            }
            if item_texts_joined(&uia, &item, 3).trim() == name.trim() {
                matching.push(item);
            }
        }
        if matching.len() != 1 {
            return None;
        }
        if current_session_title(hwnd).as_deref() == Some(name.trim()) {
            return Some(name.trim().to_string());
        }
        let invoke = matching[0]
            .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            .ok()?;
        invoke.Invoke().ok()?;
        let until = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        while std::time::Instant::now() < until {
            let title = current_session_title(hwnd)?;
            if title.trim() == name.trim() {
                return Some(title);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    None
}

/// 抓当前会话消息区全部文本（Text 节点 Name 按行拼接）——“群载文”数据源。
pub fn read_messages(hwnd: isize) -> Option<String> {
    let uia = uia()?;
    let root = panel_root(&uia, hwnd)?;
    let pane = find_by_class_contains(&uia, &root, "chat-msg-area__vlist")?;
    let mut lines = vec![];
    unsafe {
        let Ok(cond) = uia.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &windows::core::VARIANT::from(UIA_TextControlTypeId.0),
        ) else {
            return None;
        };
        let Ok(texts) = pane.FindAll(TreeScope_Descendants, &cond) else {
            return None;
        };
        let mut total = 0usize;
        for i in 0..texts.Length().unwrap_or(0).min(10_000) {
            let Ok(te) = texts.GetElement(i) else {
                continue;
            };
            if let Ok(s) = te.CurrentName() {
                let s = s.to_string();
                let s = s.trim();
                if !s.is_empty() {
                    total = total.saturating_add(s.len());
                    if total > 1024 * 1024 {
                        return None;
                    }
                    lines.push(s.to_string());
                }
            }
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// 消息区里逐条消息的**正文**（msg-content-container 下的 Text，天然不含
/// 发送者名/等级/身份徽章——那些在 message-container 直挂的兄弟 Text 节点上）。
/// 返回顺序 = 消息区显示顺序（旧→新）。
pub fn read_message_bodies(hwnd: isize) -> Option<Vec<String>> {
    let uia = uia()?;
    let root = panel_root(&uia, hwnd)?;
    let pane = find_by_class_contains(&uia, &root, "chat-msg-area__vlist")?;
    let mut bodies = vec![];
    unsafe {
        // Container control types vary across QQ versions; class identifies the body.
        let Ok(cond) = uia.CreateTrueCondition() else {
            return None;
        };
        let Ok(groups) = pane.FindAll(TreeScope_Descendants, &cond) else {
            return None;
        };
        let mut total = 0usize;
        for i in 0..groups.Length().unwrap_or(0).min(10_000) {
            let Ok(el) = groups.GetElement(i) else {
                continue;
            };
            let Ok(cn) = el.CurrentClassName() else {
                continue;
            };
            let cn = cn.to_string();
            // 两种正文容器：他人消息 / 自己消息（后缀 --others / --self）
            if cn.contains("msg-content-container") {
                let body = child_texts_joined(&uia, &el);
                if !body.is_empty() {
                    // Some builds expose nested matching containers. Avoid duplicate bodies.
                    if bodies.last() != Some(&body) {
                        total = total.saturating_add(body.len());
                        if total > 5 * 1024 * 1024 {
                            return None;
                        }
                        bodies.push(body);
                    }
                }
            }
        }
    }
    if bodies.is_empty() {
        None
    } else {
        Some(bodies)
    }
}

/// Automatic QQ sending is intentionally unavailable: display titles do not prove
/// group identity, and NTQQ does not expose a safe editor ValuePattern. The UI
/// offers explicit copy + show QQ; the user reviews the destination and sends.
pub fn send_message(_hwnd: isize, _text: &str) -> Result<(), String> {
    Err("QQ自动发送未启用：请复制后在目标群手动粘贴发送".into())
}

/// Read a stable snapshot of the currently visible conversation. Reject a switch
/// while reading; never substitute an unrelated clipboard value on UIA failure.
pub fn read_current_article(hwnd: isize, expected_title: &str) -> Result<String, String> {
    if list_sessions(hwnd)
        .iter()
        .filter(|name| name.trim() == expected_title.trim())
        .count()
        > 1
    {
        return Err("QQ存在同名会话，无法核实目标，请手动复制原文后载入".into());
    }
    let before = current_session_title(hwnd).ok_or("无法读取当前QQ会话标题，请手动复制群文")?;
    if before.trim() != expected_title.trim() {
        return Err("QQ当前会话与所选会话不同，请先手动打开目标群".into());
    }
    let bodies = read_message_bodies(hwnd).ok_or("QQ未提供可读取的消息正文，请手动复制群文")?;
    let after = current_session_title(hwnd).ok_or("读取期间QQ窗口已关闭")?;
    if before != after {
        return Err("读取期间QQ会话已切换，未载入任何内容".into());
    }
    bodies
        .into_iter()
        .rev()
        .find(|body| {
            body.len() <= 1024 * 1024
                && body
                    .lines()
                    .any(|line| line.trim_start().starts_with("-----第"))
        })
        .ok_or_else(|| "当前会话可见消息中没有带段标的跟打文，请复制原文后载入".into())
}
