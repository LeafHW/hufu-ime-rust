//! QQ NT 主面板的 UI Automation 桥（茧跟打/雨天系 GClass29 同款思路）。
//!
//! NTQQ 是 Electron 窗口，主面板内嵌「会话列表 + 消息区 + ProseMirror 输入框」，
//! 全部通过 UIA 树暴露。因此**无需弹出独立聊天窗、无需窗口激活、无需按键注入**：
//! - 列会话：会话列表容器（cls 含 recent-contact-list）下的 recent-contact-item
//! - 抓消息：消息区容器（cls 含 chat-msg-area__vlist）内 Text 节点 Name
//! - 发文/成绩：只复制并打开QQ，由用户确认目标和草稿后手动发送
//!
//! 查询策略：能用精确类名走 CreatePropertyCondition(ClassName) 让 UIA 服务端过滤，
//! 为空再回退带回测的全树 TrueCondition 扫描（Electron 类名可能带版本后缀）。
//!
//! 实测（2026-10-01，NTQQ 9.9.36）：会话项名会按字符拆分（“虎码训练”“♂”“营”），
//! 拼接后匹配；消息文本节点 Name 为整段消息内容，跟打文“-----第N段-----”直接可解析。

use windows::core::Interface;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationCondition, IUIAutomationElement,
    TreeScope_Descendants, UIA_ClassNamePropertyId, UIA_ControlTypePropertyId,
    UIA_TextControlTypeId,
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

/// 精确类名的服务端过滤查询：每个类名一个 ClassName PropertyCondition，
/// Or 合并成一次 FindAll（UIA PropertyCondition 只支持精确匹配）。
/// 结果按文档序返回、上限 10 000；任一精确类名命中即返回，全部为空返回
/// `None`，调用方回退到带回测的 TrueCondition 扫描（Electron 类名可能带
/// 版本后缀，如 `chat-msg-area__vlist--vkBD3`）。
fn find_all_by_class(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    exact: &[&str],
) -> Option<Vec<IUIAutomationElement>> {
    unsafe {
        let mut conds: Vec<IUIAutomationCondition> = Vec::new();
        for cls in exact {
            if let Ok(c) = uia.CreatePropertyCondition(
                UIA_ClassNamePropertyId,
                &windows::core::VARIANT::from(*cls),
            ) {
                conds.push(c);
            }
        }
        if conds.is_empty() {
            return None;
        }
        let mut combined = conds.pop()?;
        while let Some(c) = conds.pop() {
            combined = uia.CreateOrCondition(&c, &combined).ok()?;
        }
        let all = root.FindAll(TreeScope_Descendants, &combined).ok()?;
        let n = all.Length().unwrap_or(0).min(10_000);
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            if let Ok(e) = all.GetElement(i) {
                out.push(e);
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }
}

/// 单个精确类名取首个元素（同 `find_all_by_class` 的服务端过滤语义）。
fn find_by_exact_class(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    class: &str,
) -> Option<IUIAutomationElement> {
    find_all_by_class(uia, root, &[class]).and_then(|v| v.into_iter().next())
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

/// 会话列表容器（精确类名优先，带后缀版本回退 contains 扫描）。
fn session_list(uia: &IUIAutomation, root: &IUIAutomationElement) -> Option<IUIAutomationElement> {
    find_by_exact_class(uia, root, "recent-contact-list")
        .or_else(|| find_by_class_contains(uia, root, "recent-contact-list"))
}

/// 消息区虚拟列表容器（同上：精确优先、contains 兜底）。
fn message_vlist(uia: &IUIAutomation, root: &IUIAutomationElement) -> Option<IUIAutomationElement> {
    find_by_exact_class(uia, root, "chat-msg-area__vlist")
        .or_else(|| find_by_class_contains(uia, root, "chat-msg-area__vlist"))
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
    // 优先服务端精确类名过滤；为空回退 TrueCondition 扫描（≤2000，防巨树）
    let items = find_all_by_class(&uia, &list, &["recent-contact-item"]).unwrap_or_else(|| {
        unsafe {
            let Ok(cond) = uia.CreateTrueCondition() else {
                return vec![];
            };
            let Ok(all) = list.FindAll(TreeScope_Descendants, &cond) else {
                return vec![];
            };
            (0..all.Length().unwrap_or(0).min(2000))
                .filter_map(|i| all.GetElement(i).ok())
                .filter(|item| {
                    item.CurrentClassName()
                        .ok()
                        .map(|n| n.to_string().starts_with("recent-contact-item"))
                        .unwrap_or(false)
                })
                .collect()
        }
    });
    let mut names = Vec::new();
    for item in items.iter().take(2000) {
        let name = item_texts_joined(&uia, item, 3).trim().to_string();
        if !name.is_empty() {
            names.push(name);
        }
    }
    names
}

/// 当前打开会话的标题（chat-header__contact-name 的 Name）。
pub fn current_session_title(hwnd: isize) -> Option<String> {
    let uia = uia()?;
    let root = panel_root(&uia, hwnd)?;
    let hdr = find_by_exact_class(&uia, &root, "chat-header__contact-name")
        .or_else(|| find_by_class_prefix(&uia, &root, "chat-header__contact-name"))?;
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

/// 消息区里逐条消息的**正文**（msg-content-container 下的 Text，天然不含
/// 发送者名/等级/身份徽章——那些在 message-container 直挂的兄弟 Text 节点上）。
/// 返回顺序 = 消息区显示顺序（旧→新）。
pub fn read_message_bodies(hwnd: isize) -> Option<Vec<String>> {
    let uia = uia()?;
    let root = panel_root(&uia, hwnd)?;
    let pane = message_vlist(&uia, &root)?;
    let mut bodies = vec![];
    // 两种正文容器是精确类名：--others（他人）/ --self（自己）。Or 合并一次
    // 查询，UIA 服务端过滤且结果保持文档序（他人/自己消息按时间交错）。
    // 为空时回退 TrueCondition 扫描 + contains 回测（≤10 000，防巨树）。
    let groups = find_all_by_class(
        &uia,
        &pane,
        &["msg-content-container--others", "msg-content-container--self"],
    )
    .unwrap_or_else(|| {
        unsafe {
            let Ok(cond) = uia.CreateTrueCondition() else {
                return vec![];
            };
            let Ok(all) = pane.FindAll(TreeScope_Descendants, &cond) else {
                return vec![];
            };
            (0..all.Length().unwrap_or(0).min(10_000))
                .filter_map(|i| all.GetElement(i).ok())
                .filter(|el| {
                    el.CurrentClassName()
                        .ok()
                        .map(|n| n.to_string().contains("msg-content-container"))
                        .unwrap_or(false)
                })
                .collect()
        }
    });
    let mut total = 0usize;
    for el in groups.iter().take(10_000) {
        let body = child_texts_joined(&uia, el);
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
    if bodies.is_empty() {
        None
    } else {
        Some(bodies)
    }
}

/// Automatic QQ sending is intentionally unavailable: display titles do not prove
/// group identity, and NTQQ does not expose a safe editor ValuePattern. The UI
/// offers explicit copy + show QQ; the user reviews the destination and sends.
/// Kept as the module's documented API boundary even while nothing calls it.
#[allow(dead_code)]
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
