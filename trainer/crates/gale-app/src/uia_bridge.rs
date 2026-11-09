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
    TreeScope_Descendants, UIA_ControlTypePropertyId, UIA_TextControlTypeId,
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

/// UIA 的 ClassName 是元素 class 属性的整串（多个类名以空格分隔），
/// 实测 QQ 9.9.36 的会话题条目：
///   普通   `recent-contact-item`
///   选中   `recent-contact-item recent-contact-item--selected`
///   置顶   `recent-contact-item recent-contact-item--top rc-pin-item--read`
/// 所以**不能用 PropertyCondition 精确等值**（只命中普通条目，置顶/选中全漏）。
/// 这里按空白切词后逐词判断：等于特指类名，或以「特指类名--」开头（BEM 修饰符）。
pub(crate) fn class_matches(cls: &str, spec: &str) -> bool {
    cls.split_whitespace().any(|tok| {
        tok == spec
            || tok
                .strip_prefix(spec)
                .map(|rest| rest.starts_with("--"))
                .unwrap_or(false)
    })
}

/// 类名（含状态修饰符）过滤的条目查询：空白切词后逐词匹配，结果按文档序
/// 返回、上限 10 000；全部为空返回 `None`，调用方回退到根级前缀扫描
/// （Electron 类名可能带版本后缀，如 `chat-msg-area__vlist--vkBD3`）。
fn find_all_by_class(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    exact: &[&str],
) -> Option<Vec<IUIAutomationElement>> {
    unsafe {
        let cond = uia.CreateTrueCondition().ok()?;
        let all = root.FindAll(TreeScope_Descendants, &cond).ok()?;
        let n = all.Length().unwrap_or(0).min(10_000);
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            let Ok(e) = all.GetElement(i) else {
                continue;
            };
            let Ok(cls) = e.CurrentClassName() else {
                continue;
            };
            let cls = cls.to_string();
            if exact.iter().any(|spec| class_matches(&cls, spec)) {
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

/// 类名里是否存在「含 needle 的词」（容器匹配用：recent-contact-list /
/// pinned-contact-list / contact-list--pinned 都能命中 contact-list）。
fn class_has_token_containing(cls: &str, needle: &str) -> bool {
    cls.split_whitespace().any(|tok| tok.contains(needle))
}

/// 全部会话列表容器。NTQQ 把会话列表拆成多个容器（置顶区 + 消息区各一个，
/// 类名带不同后缀/BEM 修饰），只取第一个会漏掉置顶会话。这里按**类名词**
/// 含 `contact-list` 收集（覆盖 recent-contact-list / pinned-contact-list /
/// 未来的 contact-list--pinned 等），嵌套重复容器由 list_sessions 去重。
fn session_lists(uia: &IUIAutomation, root: &IUIAutomationElement) -> Vec<IUIAutomationElement> {
    let mut out = Vec::new();
    unsafe {
        let Ok(cond) = uia.CreateTrueCondition() else {
            return out;
        };
        let Ok(all) = root.FindAll(TreeScope_Descendants, &cond) else {
            return out;
        };
        for i in 0..all.Length().unwrap_or(0).min(10_000) {
            let Ok(el) = all.GetElement(i) else {
                continue;
            };
            let Ok(cls) = el.CurrentClassName() else {
                continue;
            };
            let cls = cls.to_string();
            if class_has_token_containing(&cls, "contact-list") {
                out.push(el);
            }
        }
    }
    out
}

/// 消息区虚拟列表容器（同上：精确优先、contains 兜底）。
fn message_vlist(uia: &IUIAutomation, root: &IUIAutomationElement) -> Option<IUIAutomationElement> {
    find_by_exact_class(uia, root, "chat-msg-area__vlist")
        .or_else(|| find_by_class_contains(uia, root, "chat-msg-area__vlist"))
}

/// 会话项文本节点 → 显示名。NTQQ 把“虎码训练♂营”拆成多段 Text，名字后面
/// 跟时间戳（含 ':'）与摘要；置顶/公告角标可能排在最前，跳过但不截断。
/// 名字段数不设硬上限（只受时间戳/未读标记截断），避免 4 段以上群名被砍尾。
pub(crate) fn join_item_texts<S: AsRef<str>>(texts: &[S]) -> String {
    let mut joined = String::new();
    for raw in texts {
        let s = raw.as_ref().trim();
        if s.is_empty() {
            continue;
        }
        // 时间戳（21:33）或未读标记之后的节点不是名字
        if s.contains(':') || s.contains("未读") {
            break;
        }
        // 置顶项首节点可能是置顶角标文字，跳过而不截断
        if s == "置顶" || s == "公告" || s == "免打扰" {
            continue;
        }
        // 只防病态超长（正常群名远低于此），不是截断名字段
        if joined.len().saturating_add(s.len()) > 512 {
            break;
        }
        joined.push_str(s);
    }
    joined
}

/// 会话项内 Text 子节点 Name 依次交给 `join_item_texts`（上限 16 个节点，
/// 正常条目只有 1~3 段名字 + 时间戳 + 摘要）。
fn item_texts_joined(uia: &IUIAutomation, item: &IUIAutomationElement) -> String {
    let mut texts: Vec<String> = Vec::new();
    unsafe {
        let Ok(cond) = uia.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &windows::core::VARIANT::from(UIA_TextControlTypeId.0),
        ) else {
            return String::new();
        };
        let Ok(found) = item.FindAll(TreeScope_Descendants, &cond) else {
            return String::new();
        };
        for i in 0..found.Length().unwrap_or(0).min(16) {
            let Ok(te) = found.GetElement(i) else {
                continue;
            };
            if let Ok(s) = te.CurrentName() {
                texts.push(s.to_string());
            }
        }
    }
    join_item_texts(&texts)
}

/// 两个 UIA 元素是否同一元素（跨容器去重用）。
fn same_element(
    uia: &IUIAutomation,
    a: &IUIAutomationElement,
    b: &IUIAutomationElement,
) -> bool {
    unsafe {
        uia.CompareElements(a, b)
            .map(|v| v.as_bool())
            .unwrap_or(false)
    }
}

/// Recent conversations are display names, not verified group IDs. Keep duplicates so
/// the selection path can reject ambiguous names rather than silently picking one.
pub fn list_sessions(hwnd: isize) -> Vec<String> {
    let Some(uia) = uia() else { return vec![] };
    let Some(root) = panel_root(&uia, hwnd) else {
        return vec![];
    };
    // 多容器：置顶区 + 消息区分别枚举，全部并入（容器类名词含 contact-list）。
    let mut items: Vec<IUIAutomationElement> = Vec::new();
    let mut lists = session_lists(&uia, &root);
    if lists.is_empty() {
        std::thread::sleep(std::time::Duration::from_millis(250));
        lists = session_lists(&uia, &root);
    }
    let multi = lists.len() > 1;
    for list in lists.iter() {
        for item in find_all_by_class(&uia, list, &["recent-contact-item"])
            .into_iter()
            .flatten()
        {
            // 嵌套容器会让同一条目被收集两次；只在多容器时去重（跨进程
            // CompareElements 不便宜），顺序仍保持文档序。
            if multi && items.iter().any(|seen| same_element(&uia, seen, &item)) {
                continue;
            }
            items.push(item);
        }
    }
    // 根级兜底：容器类名改版时仍按条目类名（含状态修饰符）抓（≤2000，防巨树）。
    if items.is_empty() {
        unsafe {
            if let Ok(cond) = uia.CreateTrueCondition() {
                if let Ok(all) = root.FindAll(TreeScope_Descendants, &cond) {
                    items = (0..all.Length().unwrap_or(0).min(2000))
                        .filter_map(|i| all.GetElement(i).ok())
                        .filter(|item| {
                            item.CurrentClassName()
                                .ok()
                                .map(|n| class_matches(&n.to_string(), "recent-contact-item"))
                                .unwrap_or(false)
                        })
                        .collect();
                }
            }
        }
    }
    let mut names = Vec::new();
    for item in items.iter().take(2000) {
        let name = item_texts_joined(&uia, item).trim().to_string();
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
        &[
            "msg-content-container--others",
            "msg-content-container--self",
        ],
    )
    .unwrap_or_else(|| unsafe {
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

#[cfg(test)]
mod tests {
    use super::{class_has_token_containing, class_matches, join_item_texts};

    // 以下类名/文本序列取自 2026-11-09 本机 NTQQ 9.9.36（pid 17388）的真实 UIA 树
    // （会话列表容器 + 置顶条目 + 选中条目 + 普通条目），不是构造的示意值。

    #[test]
    fn class_matches_plain_item() {
        assert!(class_matches("recent-contact-item", "recent-contact-item"));
    }

    #[test]
    fn class_matches_pinned_item_with_state_classes() {
        // 置顶条目真实类名：类名属性是整串，旧的 PropertyCondition 精确等值匹配会漏
        let cls = "recent-contact-item recent-contact-item--top rc-pin-item--read";
        assert!(class_matches(cls, "recent-contact-item"));
    }

    #[test]
    fn class_matches_selected_item_with_state_classes() {
        // 当前打开会话的条目真实类名
        let cls = "recent-contact-item recent-contact-item--selected";
        assert!(class_matches(cls, "recent-contact-item"));
    }

    #[test]
    fn class_matches_rejects_unrelated_classes() {
        for cls in [
            "recent-contact-item-extra",
            "recent-contact-items",
            "rc-pin-item--read",
            "q-scroll-view scroll-view--memory",
            "",
        ] {
            assert!(
                !class_matches(cls, "recent-contact-item"),
                "不应命中: {cls:?}"
            );
        }
    }

    #[test]
    fn class_matches_container_names() {
        // 真实容器类名（置顶区与普通区共用同一个滚动容器）
        let real = "q-scroll-view scroll-view--memory scroll-view--show-scrollbar \
                    recent-contact-list recent-contact-default-theme-list";
        assert!(class_has_token_containing(real, "contact-list"));
        // 假设的置顶独立容器类名也要能命中（通用，不做单类名特判）
        assert!(class_has_token_containing("pinned-contact-list", "contact-list"));
        assert!(class_has_token_containing(
            "contact-list contact-list--pinned",
            "contact-list"
        ));
        // 消息区/输入区不能误判成会话容器
        assert!(!class_has_token_containing(
            "ml-area v-list-area list-wrap chat-msg-area__vlist",
            "contact-list"
        ));
    }

    #[test]
    fn join_pinned_item_name_stops_at_timestamp() {
        // 置顶条目「虎码训练♂营」的 Text 节点顺序：名字 3 段 → 时间 → 摘要
        let texts = [
            "虎码训练",
            "♂",
            "营",
            "23:38",
            "Z-E-R-O.5梦：",
            "@晴",
            "我只是打这个文打熟了",
        ];
        assert_eq!(join_item_texts(&texts), "虎码训练♂营");
    }

    #[test]
    fn join_selected_item_name() {
        let texts = ["虎码新手村2群", "23:41", "芝士798：", "我现在就是中转", "19"];
        assert_eq!(join_item_texts(&texts), "虎码新手村2群");
    }

    #[test]
    fn join_keeps_more_than_three_name_segments() {
        // 旧实现 limit=3 会把第 4 段名字砍掉；这里必须完整拼出
        let texts = ["虎码训练", "♂", "营", "②群", "23:38", "某人：", "内容"];
        assert_eq!(join_item_texts(&texts), "虎码训练♂营②群");
    }

    #[test]
    fn join_skips_pin_badge_but_keeps_name() {
        let texts = ["置顶", "虎码训练", "♂", "营", "23:38", "某人：", "内容"];
        assert_eq!(join_item_texts(&texts), "虎码训练♂营");
    }

    #[test]
    fn join_stops_at_unread_marker() {
        // 「群助手」条目：名字后是无未读时的摘要，未读标记之后不再取
        let texts = ["群助手", "23:41", "[", "@全体成员", "]", "18个群聊有新消息"];
        assert_eq!(join_item_texts(&texts), "群助手");
    }

    /// 端到端语义回归：用真实条目（类名 + Text 序列）跑「类名匹配 → 名字拼接」，
    /// 置顶群必须出现在结果里，且同名会话的重复保留语义不变。
    #[test]
    fn pinned_session_is_listed_and_duplicates_kept() {
        struct Row {
            cls: &'static str,
            texts: &'static [&'static str],
        }
        let rows = [
            Row {
                cls: "recent-contact-item recent-contact-item--top rc-pin-item--read",
                texts: &["虎码训练", "♂", "营", "23:38", "Z-E-R-O.5梦：", "@晴", "内容"],
            },
            Row {
                cls: "recent-contact-item",
                texts: &["群助手", "23:41", "[", "@全体成员", "]", "18个群聊有新消息"],
            },
            Row {
                cls: "recent-contact-item recent-contact-item--selected",
                texts: &["虎码新手村2群", "23:41", "芝士798：", "内容", "19"],
            },
            // 同名会话出现两次：必须两行都保留（选择端靠重复判定歧义，不能吞）
            Row {
                cls: "recent-contact-item",
                texts: &["虎码训练", "♂", "营", "23:40", "某人：", "内容"],
            },
        ];
        let names: Vec<String> = rows
            .iter()
            .filter(|r| class_matches(r.cls, "recent-contact-item"))
            .map(|r| join_item_texts(r.texts))
            .filter(|n| !n.is_empty())
            .collect();
        assert_eq!(
            names,
            vec![
                "虎码训练♂营".to_string(),
                "群助手".to_string(),
                "虎码新手村2群".to_string(),
                "虎码训练♂营".to_string(),
            ]
        );
        // 置顶群在列表首位（文档序），且两条同名都在
        assert_eq!(names.iter().filter(|n| n.as_str() == "虎码训练♂营").count(), 2);
    }
}
