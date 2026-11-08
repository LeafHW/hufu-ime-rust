//! Real RichEdit input document. Physical keys are counted before the TSF pump;
//! committed text is diffed by Unicode scalars, while all control offsets use UTF-16.
//! Native composition remains owned by RichEdit and the active Windows input method.

#![allow(non_snake_case)]

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;

use gale_core::compare::InputEvent;
use gale_core::stats::StatsEngine;
use gale_core::GaleResult;

use windows::core::{w, Interface};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows::Win32::UI::Controls::RichEdit::{
    EM_GETTEXTEX, EM_GETTEXTLENGTHEX, GETTEXTEX, GETTEXTLENGTHEX, GTL_NUMCHARS, GTL_PRECISE,
    GT_DEFAULT,
};
use windows::Win32::UI::Input::Ime::{
    ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext, ImmSetCandidateWindow,
    ImmSetCompositionWindow, CANDIDATEFORM, CFS_FORCE_POSITION, COMPOSITIONFORM, GCS_COMPSTR,
    IMN_CHANGECANDIDATE, IMN_CLOSECANDIDATE, IMN_CLOSESTATUSWINDOW, IMN_GUIDELINE,
    IMN_OPENCANDIDATE, IMN_OPENSTATUSWINDOW, IMN_PRIVATE, IMN_SETCANDIDATEPOS,
    IMN_SETCOMPOSITIONFONT, IMN_SETCOMPOSITIONWINDOW, IMN_SETCONVERSIONMODE, IMN_SETOPENSTATUS,
    IMN_SETSENTENCEMODE, IMN_SETSTATUSWINDOWPOS, IMN_SOFTKBDDESTROYED,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, SetFocus, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::TextServices::{ITfContextComposition, ITfRangeACP};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, DefWindowProcW, GetCaretPos, GetParent, SendMessageW,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_WNDPROC, SWP_NOZORDER, SW_HIDE, SW_SHOWNA,
    WINDOW_EX_STYLE, WM_CHAR, WM_KEYDOWN, WM_SYSCHAR, WM_SYSKEYDOWN, WS_CHILD, WS_VISIBLE,
};

// ── RichEdit 常量（windows 0.58 未绑定的部分，手工定义，值与 SDK 一致）──
const EM_SETEVENTMASK: u32 = 1093;
const EM_SETSEL: u32 = 177;
const WM_SETTEXT: u32 = 0x000C;
const ENM_UPDATE: u32 = 0x0002;
/// EM_SETEDITSTYLE：设置编辑样式位（wParam=位，lParam=掩码）
const EM_SETEDITSTYLE: u32 = 1228;
/// SES_USECTF：显式开启 TSF 支持（RichEdit50W 默认开，显式设确保 CUAS 桥激活）
const SES_USECTF: usize = 0x1_0000;
/// EM_GETIMECOMPTEXT: legacy RichEdit/IMM mirror; pure TSF may leave it empty.
const EM_GETIMECOMPTEXT: u32 = 1266;

/// IMECOMPTEXT 结构（EM_GETIMECOMPTEXT 的 wParam；cb=目标缓冲字节数）
#[repr(C)]
struct ImeCompTextHeader {
    cb: i32,
    flags: u32,
}

/// 输入宿主向外抛的统一事件（翻译自 Win32 消息流）
#[derive(Debug, Clone)]
pub enum HostEvent {
    /// 上屏差分（新提交的字符）
    Commit(String),
    /// 一次退格
    Backspace,
    /// 组字开始/结束（候选窗可见性、渲染层打点用）
    Composing(bool),
    /// 组字串变化（preedit 显示）
    Preedit(String),
    /// Explicit Esc/Backspace cancellation, confirmed by a stable document
    /// across another message-pump boundary. A plain IME END is insufficient.
    Cancelled,
    /// 原始击键（统计用；vk 已过滤为统计键）
    Key { vk: u32 },
}

impl From<&HostEvent> for Option<InputEvent> {
    fn from(ev: &HostEvent) -> Option<InputEvent> {
        match ev {
            HostEvent::Commit(s) => Some(InputEvent::Commit { chars: s.clone() }),
            HostEvent::Backspace => Some(InputEvent::Backspace),
            _ => None,
        }
    }
}

/// 子类化截获的命令键（不进统计、转全局命令队列）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKey {
    Escape,
    F1, // 设置面板（jifeng1 语义）
    F2, // 文章库面板（jifeng1 语义；语义由 main.rs 定义，枚举只表键码）
    F3, // 重打本段
    F4, // 统计面板
    F5, // 文库循环选书
    F6, // 报告
    F7, // 发文面板
    F8, // 单字练习
    F9,
    F10,       // 设置面板（jifeng1：F1/F10 都开设置）
    Enter,     // 段末跳段
    Space,     // 段末跳段（组字中放行给输入法）
    CtrlLeft,  // 上一段
    CtrlRight, // 下一段
    AltE,      // 载文（剪贴板群文，兼容疾风1代）
    LoadClipboard,
    CopyResult,
    SendResult,
    ExportResult,
    RetryResult,
}

/// 宿主配置
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// 父窗口
    pub parent: isize,
    /// 初始位置大小（物理像素）
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

// ── 进程级单例状态（与 relay.rs 同构）──
static CMD_QUEUE: Mutex<Vec<CommandKey>> = Mutex::new(Vec::new());
static ORIG_PROC: AtomicIsize = AtomicIsize::new(0);
static RELAY_HWND: AtomicIsize = AtomicIsize::new(0);
static LIB_LOADED: AtomicBool = AtomicBool::new(false);
static COMPOSING: AtomicBool = AtomicBool::new(false);
static CANCEL_INTENT: AtomicBool = AtomicBool::new(false);
/// 每键时间戳队列（标顶/选重 20ms 窗判定预留），元素 = (vk, 毫秒)
static KEY_TIMES: Mutex<Vec<(u32, u64)>> = Mutex::new(Vec::new());
static PHYSICAL_EVENTS: Mutex<Vec<HostEvent>> = Mutex::new(Vec::new());

/// Observe the queued physical key before TranslateMessage/TSF rewrites VK_PROCESSKEY.
/// Only the real typing control contributes statistics; modal editors never do.
static ROUND_DONE: AtomicBool = AtomicBool::new(false);
static RESULT_KEY_TARGET: AtomicIsize = AtomicIsize::new(0);
pub fn set_round_done(done: bool) {
    ROUND_DONE.store(done, Ordering::Relaxed);
}
/// Only the completed-segment panel may receive continuation keys on its
/// container HWND. Native editors and confirmation panels keep their own keys.
pub fn set_result_key_target(hwnd: Option<isize>) {
    RESULT_KEY_TARGET.store(hwnd.unwrap_or(0), Ordering::Relaxed);
}
fn completion_command(
    vk: u32,
    done: bool,
    allowed_target: bool,
    composing: bool,
    ctrl: bool,
    alt: bool,
) -> Option<CommandKey> {
    if !done || !allowed_target || composing || ctrl || alt {
        return None;
    }
    match vk {
        0x20 => Some(CommandKey::Space),
        0x0D => Some(CommandKey::Enter),
        _ => None,
    }
}
/// Pretranslate shortcuts and count only physical typing keys. Returning true
/// suppresses TranslateMessage too, preventing a command's WM_CHAR leaking ahead.
pub fn capture_physical_key(msg: &windows::Win32::UI::WindowsAndMessaging::MSG) -> bool {
    if msg.message != WM_KEYDOWN && msg.message != WM_SYSKEYDOWN {
        return false;
    }
    let relay = msg.hwnd.0 as isize == relay_hwnd();
    let vk = msg.wParam.0 as u32;
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) < 0 };
    let alt = unsafe { GetKeyState(VK_MENU.0 as i32) < 0 };
    let composing = unsafe {
        let (a, b) = composing_channels(msg.hwnd);
        a || b
    };
    if relay && composing && matches!(vk, 0x1b | 0x08) {
        CANCEL_INTENT.store(true, Ordering::Relaxed);
    }
    if alt && (vk == 0x73 || vk == 0x20) {
        return false;
    }
    if ctrl && vk == 0x20 {
        return false;
    }
    let function = match vk {
        0x70 => Some(CommandKey::F1),
        0x71 => Some(CommandKey::F2),
        0x72 => Some(CommandKey::F3),
        0x73 => Some(CommandKey::F4),
        0x74 => Some(CommandKey::F5),
        0x75 => Some(CommandKey::F6),
        0x76 => Some(CommandKey::F7),
        0x77 => Some(CommandKey::F8),
        0x78 => Some(CommandKey::F9),
        0x79 => Some(CommandKey::F10),
        _ => None,
    };
    if let Some(k) = function.filter(|_| !composing) {
        push_cmd(k);
        return true;
    }
    if vk == 0x1b && !composing {
        push_cmd(CommandKey::Escape);
        return true;
    }
    let result_target = RESULT_KEY_TARGET.load(Ordering::Relaxed);
    if result_target != 0 && msg.hwnd.0 as isize == result_target && !composing && !ctrl && !alt {
        let command = match vk {
            0x43 => Some(CommandKey::CopyResult),
            0x53 => Some(CommandKey::SendResult),
            0x45 => Some(CommandKey::ExportResult),
            0x52 => Some(CommandKey::RetryResult),
            _ => None,
        };
        if let Some(command) = command {
            push_cmd(command);
            return true;
        }
    }
    if let Some(command) = completion_command(
        vk,
        ROUND_DONE.load(Ordering::Relaxed),
        relay || (result_target != 0 && msg.hwnd.0 as isize == result_target),
        composing,
        ctrl,
        alt,
    ) {
        push_cmd(command);
        return true;
    }
    if !relay {
        return false;
    }
    if ctrl {
        match vk {
            0x25 => push_cmd(CommandKey::CtrlLeft),
            0x27 => push_cmd(CommandKey::CtrlRight),
            0x56 => push_cmd(CommandKey::LoadClipboard),
            _ => {}
        }
        return true;
    }
    if alt {
        if vk == 0x45 {
            push_cmd(CommandKey::AltE);
        }
        return true;
    }
    if StatsEngine::is_stat_key(vk) {
        if let Ok(mut q) = PHYSICAL_EVENTS.lock() {
            q.push(HostEvent::Composing(composing));
            q.push(HostEvent::Key { vk });
        }
    }
    false
}

fn push_cmd(k: CommandKey) {
    if let Ok(mut q) = CMD_QUEUE.lock() {
        if q.len() < 64 {
            q.push(k);
        }
    }
}

/// 取走累积的命令键（主循环每帧调用）
pub fn drain_cmds() -> Vec<CommandKey> {
    match CMD_QUEUE.lock() {
        Ok(mut q) => {
            let taken = q.clone();
            q.clear();
            taken
        }
        Err(_) => Vec::new(),
    }
}

/// 组字中？（对外只读快照）
pub fn is_composing() -> bool {
    COMPOSING.load(Ordering::Relaxed)
}

/// RichEdit 中继是否拥有输入焦点（键泵让路判定：
/// TSF 后端激活时 RichEdit 不持焦点；后端切换/回退时它持有）。
pub fn relay_owns_focus() -> bool {
    let h = RELAY_HWND.load(Ordering::Relaxed);
    if h == 0 {
        return false;
    }
    unsafe {
        let fg = GetFocus();
        fg.0 as isize == h
    }
}

/// TSF 后端的窗口级键路由：命令键捕获 + 统计键记账。
/// 返回 Some(LRESULT) = 已处理（返回该值）；None = 放行 DefWindowProc。
/// 逻辑与 relay_proc 的命令键段一致（F1/F2/F3/F4/F5/F6/F7/F8/F10/Esc/
/// Enter/Space/Ctrl 箭头/Alt 系/Tab/Shift+Insert），统计键同样记账（组字中也记）。
/// 键位表（2026-10 lead 裁决）：F1 设置 / F7 发文面板 / F8 单字练习。
pub fn route_key(wp: usize, _lp: isize) -> Option<LRESULT> {
    let vk = wp as u32;
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) < 0 };
    let alt = unsafe { GetKeyState(VK_MENU.0 as i32) < 0 };
    // 统计键记账（组字状态也记——就是真实按键；虎符重映射 VK 0xE5 由
    // is_stat_key 区段表覆盖）
    if StatsEngine::is_stat_key(vk) {
        if let Ok(mut kt) = KEY_TIMES.lock() {
            kt.push((vk, query_tick_ms()));
            if kt.len() > 512 {
                kt.drain(..256);
            }
        }
    }
    if alt && !ctrl {
        // Alt 系全吞防漏字（WM_SYSCHAR 双保险在 main_proc）
        if vk == 0x45 {
            push_cmd(CommandKey::AltE);
        }
        return Some(LRESULT(0));
    }
    if ctrl {
        return match vk {
            0x25 => {
                push_cmd(CommandKey::CtrlLeft);
                Some(LRESULT(0))
            }
            0x27 => {
                push_cmd(CommandKey::CtrlRight);
                Some(LRESULT(0))
            }
            _ => Some(LRESULT(0)), // 其余 Ctrl 组合吞（防粘贴/复制/全选）
        };
    }
    match vk {
        0x1B => {
            push_cmd(CommandKey::Escape);
            Some(LRESULT(0))
        }
        0x70 => {
            push_cmd(CommandKey::F1);
            Some(LRESULT(0))
        }
        0x71 => {
            push_cmd(CommandKey::F2);
            Some(LRESULT(0))
        }
        0x72 => {
            push_cmd(CommandKey::F3);
            Some(LRESULT(0))
        }
        0x73 => {
            push_cmd(CommandKey::F4);
            Some(LRESULT(0))
        }
        0x74 => {
            push_cmd(CommandKey::F5);
            Some(LRESULT(0))
        }
        0x75 => {
            push_cmd(CommandKey::F6);
            Some(LRESULT(0))
        }
        0x76 => {
            push_cmd(CommandKey::F7);
            Some(LRESULT(0))
        }
        0x77 => {
            push_cmd(CommandKey::F8);
            Some(LRESULT(0))
        }
        0x79 => {
            push_cmd(CommandKey::F10);
            Some(LRESULT(0))
        }
        0x0D => {
            push_cmd(CommandKey::Enter);
            Some(LRESULT(0))
        }
        0x09 => Some(LRESULT(0)), // Tab
        0x20 => {
            // 组字中：空格归输入法（选重/顶屏）——但走到这里说明 TSF 泵没吃
            //（未组字），按段末跳段命令处理
            push_cmd(CommandKey::Space);
            Some(LRESULT(0))
        }
        _ => None,
    }
}

pub fn relay_hwnd() -> isize {
    RELAY_HWND.load(Ordering::Relaxed)
}

pub fn query_tick_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 组字中？（直查 HIMC；蓝本 relay.rs composing()）
unsafe fn composing_raw(hwnd: HWND) -> bool {
    let himc = ImmGetContext(hwnd);
    if himc.is_invalid() {
        return false;
    }
    let len = ImmGetCompositionStringW(himc, GCS_COMPSTR, None, 0);
    let _ = ImmReleaseContext(hwnd, himc);
    len > 0
}

/// IMM or actual TSF composition presence. A zero-length composition still owns
/// its keys even when neither legacy string mirror contains any characters.
fn composing_verdict(imm: bool, ctf: bool) -> bool {
    imm || ctf
}

/// 读双通道原始读数（诊断/判定共用；不落字符串，只判非空）
unsafe fn composing_channels(hwnd: HWND) -> (bool, bool) {
    let imm = composing_raw(hwnd);
    let ctf = tsf_composition(hwnd).active || !comp_string(hwnd).is_empty();
    (imm, ctf)
}

#[derive(Default, Debug)]
struct TsfComposition {
    active: bool,
    /// None means a composition exists but its ACP extent could not be read.
    /// The caller defers the diff instead of classifying preedit as a commit.
    ranges: Option<Vec<std::ops::Range<usize>>>,
}

unsafe fn tsf_composition(hwnd: HWND) -> TsfComposition {
    let Some(context) = crate::window::focused_tsf_context(hwnd) else {
        return TsfComposition::default();
    };
    let Ok(compositions) = context.cast::<ITfContextComposition>() else {
        return TsfComposition::default();
    };
    let Ok(enumerator) = compositions.EnumCompositions() else {
        return TsfComposition::default();
    };
    let mut state = TsfComposition {
        active: false,
        ranges: Some(Vec::new()),
    };
    // A RichEdit normally has one composition. Bound enumeration even if an
    // external TIP misbehaves; an uncertain active document is never scored.
    for _ in 0..64 {
        let mut item = [None];
        let mut fetched = 0;
        if enumerator.Next(&mut item, &mut fetched).is_err() {
            if state.active {
                state.ranges = None;
            }
            return state;
        }
        if fetched == 0 {
            return state;
        }
        state.active = true;
        let extent = (|| {
            let range = item[0].as_ref()?.GetRange().ok()?;
            let acp = range.cast::<ITfRangeACP>().ok()?;
            let (mut start, mut count) = (0, 0);
            acp.GetExtent(&mut start, &mut count).ok()?;
            let start = usize::try_from(start).ok()?;
            let count = usize::try_from(count).ok()?;
            Some(start..start.checked_add(count)?)
        })();
        match (state.ranges.as_mut(), extent) {
            (Some(ranges), Some(range)) => ranges.push(range),
            _ => state.ranges = None,
        }
    }
    state.ranges = None;
    state
}

/// RichEdit's default text form uses one UTF-16 CR per paragraph, matching TSF
/// ACP offsets. WM_GETTEXT may expand it to CRLF and cannot index ACP ranges.
unsafe fn document_utf16(hwnd: HWND) -> Vec<u16> {
    let length = GETTEXTLENGTHEX {
        flags: GTL_NUMCHARS | GTL_PRECISE,
        codepage: 1200,
    };
    let len = SendMessageW(
        hwnd,
        EM_GETTEXTLENGTHEX,
        WPARAM(&length as *const _ as usize),
        LPARAM(0),
    )
    .0
    .max(0) as usize;
    let mut buf = vec![0u16; len.saturating_add(1).min(200_002)];
    let params = GETTEXTEX {
        cb: (buf.len() * 2) as u32,
        flags: GT_DEFAULT,
        codepage: 1200,
        ..Default::default()
    };
    let copied = SendMessageW(
        hwnd,
        EM_GETTEXTEX,
        WPARAM(&params as *const _ as usize),
        LPARAM(buf.as_mut_ptr() as isize),
    )
    .0
    .max(0) as usize;
    buf.truncate(copied.min(buf.len().saturating_sub(1)));
    buf
}

fn normalize_document(text: &[u16]) -> String {
    String::from_utf16_lossy(text)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

/// Remove only verified active UTF-16 spans, before newline/scalar conversion.
/// An invalid span yields None so the existing committed snapshot is retained.
fn split_composition(full: &[u16], ranges: &[std::ops::Range<usize>]) -> Option<(String, String)> {
    let mut ranges = ranges.to_vec();
    ranges.sort_by_key(|r| r.start);
    let mut committed = Vec::with_capacity(full.len());
    let mut preedit = Vec::new();
    let mut cursor = 0;
    for range in ranges {
        if range.start < cursor || range.end < range.start || range.end > full.len() {
            return None;
        }
        // ACP must never bisect a supplementary character.
        for boundary in [range.start, range.end] {
            if boundary > 0
                && boundary < full.len()
                && (0xD800..=0xDBFF).contains(&full[boundary - 1])
                && (0xDC00..=0xDFFF).contains(&full[boundary])
            {
                return None;
            }
        }
        committed.extend_from_slice(&full[cursor..range.start]);
        preedit.extend_from_slice(&full[range.clone()]);
        cursor = range.end;
    }
    committed.extend_from_slice(&full[cursor..]);
    Some((normalize_document(&committed), normalize_document(&preedit)))
}

/// 读取当前组字串（preedit 显示；空串=无组字）。
///
/// 双通道（architecture.md Mandate #2）：
/// 1. EM_GETIMECOMPTEXT（RichEdit 私有消息）——纯 TSF 输入法（虎符）的组字真源；
/// 2. ImmGetContext + GCS_COMPSTR 兜底——IMM 型输入法（虎娘/微软拼音传统层）。
unsafe fn comp_string(hwnd: HWND) -> String {
    // 通道 1：EM_GETIMECOMPTEXT
    let mut buf = [0u16; 256];
    let mut ict = ImeCompTextHeader {
        cb: (buf.len() * 2) as i32,
        flags: 0,
    };
    let copied = SendMessageW(
        hwnd,
        EM_GETIMECOMPTEXT,
        WPARAM(&mut ict as *mut ImeCompTextHeader as usize),
        LPARAM(buf.as_mut_ptr() as isize),
    )
    .0 as usize;
    if copied > 0 && copied <= buf.len() {
        return String::from_utf16_lossy(&buf[..copied]);
    }
    // 通道 2：HIMC GCS_COMPSTR（IMM 镜像）
    let himc = ImmGetContext(hwnd);
    if himc.is_invalid() {
        return String::new();
    }
    let len = ImmGetCompositionStringW(himc, GCS_COMPSTR, None, 0);
    let s = if len > 0 {
        let mut b = vec![0u16; (len as usize) / 2 + 1];
        let k = ImmGetCompositionStringW(
            himc,
            GCS_COMPSTR,
            Some(b.as_mut_ptr() as *mut core::ffi::c_void),
            b.len() as u32 * 2,
        );
        if k > 0 {
            String::from_utf16_lossy(&b[..(k as usize) / 2])
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    let _ = ImmReleaseContext(hwnd, himc);
    s
}

// ── 输入诊断日志（GALE_INPUT_LOG=1 时启用；写 exe 旁 输入诊断.log）──
static INPUT_LOG: AtomicBool = AtomicBool::new(false);
static INPUT_LOG_PATH: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

fn input_log_enabled() -> bool {
    // 惰性初始化：环境变量或 exe 旁标志文件（发布包双击场景拿不到环境变量，
    // 留 输入诊断.开 文件即可永久开日志——用户侧收集证据用）
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let env_on = std::env::var("GALE_INPUT_LOG")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let flag_on = dir.join("输入诊断.开").exists();
        if env_on || flag_on {
            INPUT_LOG.store(true, Ordering::Relaxed);
            if let Ok(mut g) = INPUT_LOG_PATH.lock() {
                *g = Some(dir.join("输入诊断.log"));
            }
            if env_on {
                eprintln!("[gale] 输入诊断日志已开启（GALE_INPUT_LOG=1）");
            }
        }
    });
    INPUT_LOG.load(Ordering::Relaxed)
}

fn input_log(s: &str) {
    if !input_log_enabled() {
        return;
    }
    use std::io::Write;
    let Ok(Some(path)) = INPUT_LOG_PATH.lock().map(|g| g.clone()) else {
        return;
    };
    let ts = query_tick_ms() % 86_400_000; // 毫秒级时戳足够诊断
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "[+{ts}ms] {s}");
    }
}

/// tsf_focus 模块用的诊断日志出口（同 GALE_INPUT_LOG 开关）
pub fn diagnostics_enabled() -> bool {
    input_log_enabled()
}
pub fn diag_log(s: &str) {
    input_log(s);
}

unsafe extern "system" fn relay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let orig = ORIG_PROC.load(Ordering::Relaxed);
    macro_rules! call_orig {
        () => {{
            if orig != 0 {
                let f: windows::Win32::UI::WindowsAndMessaging::WNDPROC = std::mem::transmute(orig);
                CallWindowProcW(f, hwnd, msg, wp, lp)
            } else {
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }};
    }
    // 诊断：IME 消息与按键全记录（正常路径零开销——env 未设时直接跳过）
    if input_log_enabled() {
        match msg {
            641 => input_log(&format!("RichEdit WM_IME_SETCONTEXT fSet={}", wp.0)), // 0x281
            269 => input_log("RichEdit WM_IME_STARTCOMPOSITION"),                   // 0x10D
            271 => {
                // 0x10F
                let flags = lp.0 as u32;
                let comp = comp_string(hwnd);
                input_log(&format!(
                    "RichEdit WM_IME_COMPOSITION flags=0x{flags:X} GCS_RESULT={} GCS_COMP={} comp=\"{comp}\"",
                    flags & 0x0800 != 0, flags & 0x0080 != 0
                ));
            }
            270 => input_log("RichEdit WM_IME_ENDCOMPOSITION"), // 0x10E
            642 => {
                // 0x282 WM_IME_NOTIFY：wp=IMN_* 子码。子码名常量表让下次实测
                // 一份日志就能定位断在哪一环（候选开/换/关、状态切换全留痕）。
                let sub = wp.0 as u32;
                let name = match sub {
                    IMN_CLOSESTATUSWINDOW => "IMN_CLOSESTATUSWINDOW",
                    IMN_OPENSTATUSWINDOW => "IMN_OPENSTATUSWINDOW",
                    IMN_CHANGECANDIDATE => "IMN_CHANGECANDIDATE",
                    IMN_CLOSECANDIDATE => "IMN_CLOSECANDIDATE",
                    IMN_OPENCANDIDATE => "IMN_OPENCANDIDATE",
                    IMN_SETCONVERSIONMODE => "IMN_SETCONVERSIONMODE",
                    IMN_SETSENTENCEMODE => "IMN_SETSENTENCEMODE",
                    IMN_SETOPENSTATUS => "IMN_SETOPENSTATUS",
                    IMN_SETCANDIDATEPOS => "IMN_SETCANDIDATEPOS",
                    IMN_SETCOMPOSITIONFONT => "IMN_SETCOMPOSITIONFONT",
                    IMN_SETCOMPOSITIONWINDOW => "IMN_SETCOMPOSITIONWINDOW",
                    IMN_SETSTATUSWINDOWPOS => "IMN_SETSTATUSWINDOWPOS",
                    IMN_GUIDELINE => "IMN_GUIDELINE",
                    IMN_PRIVATE => "IMN_PRIVATE",
                    IMN_SOFTKBDDESTROYED => "IMN_SOFTKBDDESTROYED",
                    _ => "IMN_?(未知)",
                };
                input_log(&format!("RichEdit WM_IME_NOTIFY {name}({sub})"));
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                // 双通道读数一并入日志：imm=IMM GCS_COMPSTR 通道，ctf=EM_GETIMECOMPTEXT
                // 通道。纯 TSF 输入法预期 imm=false ctf=true；断链定位关键行。
                let (imm, ctf) = composing_channels(hwnd);
                input_log(&format!(
                    "RichEdit KEYDOWN vk=0x{:X} imm={imm} ctf={ctf} composing={}",
                    wp.0 as u32,
                    imm || ctf
                ));
            }
            WM_CHAR => {
                // 直通字符（IME 关/英文模式）：是否真的有字符进控件
                input_log(&format!("RichEdit CHAR ch=0x{:X}", wp.0 as u32));
            }
            _ => {}
        }
    }
    match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wp.0 as u32;
            let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
            let alt = GetKeyState(VK_MENU.0 as i32) < 0;
            if alt && (vk == 0x73 || vk == 0x20) {
                return call_orig!();
            }
            if ctrl && vk == 0x20 {
                return call_orig!();
            }
            if alt && !ctrl {
                // Alt 系全吞防漏字（WM_SYSCHAR 双保险在下面）
                if vk == 0x45 {
                    push_cmd(CommandKey::AltE);
                }
                return LRESULT(0);
            }
            if ctrl {
                match vk {
                    0x25 => {
                        push_cmd(CommandKey::CtrlLeft);
                        return LRESULT(0);
                    }
                    0x27 => {
                        push_cmd(CommandKey::CtrlRight);
                        return LRESULT(0);
                    }
                    0x56 => {
                        push_cmd(CommandKey::LoadClipboard);
                        return LRESULT(0);
                    }
                    _ => return LRESULT(0), // 其余 Ctrl 组合吞（防粘贴/复制/全选）
                }
            }
            // 组字判定：双通道（IMM GCS_COMPSTR 或 EM_GETIMECOMPTEXT 任一非空）。
            // 单查 IMM 通道会漏判纯 TSF 输入法（虎符）——CUAS 桥组字不一定反映
            // 到 IMM HIMC → 提交键被吞 → 候选框亮着但上不了屏（用户实测症状）。
            let (ch_imm, ch_ctf) = composing_channels(hwnd);
            let composing = composing_verdict(ch_imm, ch_ctf);
            if composing && (0x70..=0x79).contains(&vk) {
                return call_orig!();
            }
            match vk {
                0x1B if !composing => {
                    push_cmd(CommandKey::Escape);
                    return LRESULT(0);
                }
                0x70 => {
                    push_cmd(CommandKey::F1);
                    return LRESULT(0);
                }
                0x71 => {
                    push_cmd(CommandKey::F2);
                    return LRESULT(0);
                }
                0x72 => {
                    push_cmd(CommandKey::F3);
                    return LRESULT(0);
                }
                0x73 => {
                    push_cmd(CommandKey::F4);
                    return LRESULT(0);
                }
                0x74 => {
                    push_cmd(CommandKey::F5);
                    return LRESULT(0);
                }
                0x75 => {
                    push_cmd(CommandKey::F6);
                    return LRESULT(0);
                }
                0x76 => {
                    push_cmd(CommandKey::F7);
                    return LRESULT(0);
                }
                0x77 => {
                    push_cmd(CommandKey::F8);
                    return LRESULT(0);
                }
                0x78 => {
                    push_cmd(CommandKey::F9);
                    return LRESULT(0);
                }
                0x79 => {
                    push_cmd(CommandKey::F10);
                    return LRESULT(0);
                }
                0x0D => {
                    // 组字中：Enter 归输入法（确认上屏）放行；否则段末跳段命令
                    if !composing && ROUND_DONE.load(Ordering::Relaxed) {
                        push_cmd(CommandKey::Enter);
                        return LRESULT(0);
                    }
                }
                0x09 => return LRESULT(0), // Tab
                0x20 => {
                    // 组字中：空格归输入法（选重/顶屏）放行；否则段末跳段命令
                    if !composing && ROUND_DONE.load(Ordering::Relaxed) {
                        push_cmd(CommandKey::Space);
                        return LRESULT(0);
                    }
                }
                // 组字中的数字键（候选选重）与全部字母键天然落 `_ => {}` 放行，
                // 无需单列——call_orig 交给 RichEdit/IME 处理。
                _ => {}
            }
            if vk == 0x2D && GetKeyState(VK_SHIFT.0 as i32) < 0 {
                return LRESULT(0); // Shift+Insert 粘贴
            }
        }
        0x0300 | 0x0301 | 0x0302 | 0x0303 | 0x0304 => return LRESULT(0), // cut/copy/paste/clear/undo are not training commits
        WM_SYSCHAR => return LRESULT(0), // Alt+字母字符直通是疾风1代漏字根源，吞
        _ => {}
    }
    call_orig!()
}

/// 取走击键时间戳队列（统计引擎消费；TSF 后端也用同一队列——
/// route_key 记账，主循环 drain）
pub fn drain_key_times(max: usize) -> Vec<(u32, u64)> {
    match KEY_TIMES.lock() {
        Ok(mut kt) => {
            let take = kt.len().min(max);
            let out = kt[..take].to_vec();
            kt.drain(..take);
            out
        }
        Err(_) => Vec::new(),
    }
}

/// RichEdit50W 输入宿主（单例持有）
pub struct InputHost {
    pub hwnd: isize,
    pub composing: bool,
    pub last_text: String,
    pub last_preedit: String,
    cancellation: CompositionCancellation,
}

#[derive(Default)]
struct CompositionCancellation {
    pending: bool,
}
impl CompositionCancellation {
    fn observe(&mut self, was: bool, now: bool, unchanged: bool, intent: bool) -> bool {
        let confirmed = self.pending && unchanged;
        self.pending = was && !now && unchanged && intent;
        confirmed
    }
}

unsafe impl Send for InputHost {}

impl InputHost {
    /// 创建（或复用）宿主控件。失败返回 Err（msftedit 缺失等）。
    pub fn create(cfg: &HostConfig) -> GaleResult<Self> {
        unsafe {
            load_library();
            if !LIB_LOADED.load(Ordering::Relaxed) {
                return gale_core::gale_err!("msftedit.dll 载入失败（RichEdit50W 不可用）");
            }
            let existing = RELAY_HWND.load(Ordering::Relaxed);
            if existing != 0 {
                return Ok(Self {
                    hwnd: existing,
                    composing: false,
                    last_text: String::new(),
                    last_preedit: String::new(),
                    cancellation: CompositionCancellation::default(),
                });
            }
            let hinstance = match GetModuleHandleW(None) {
                Ok(h) => windows::Win32::Foundation::HINSTANCE(h.0),
                Err(_) => return gale_core::gale_err!("GetModuleHandleW 失败"),
            };
            // 先普通创建；带 LAYERED 创建会静默失败（jifeng2 教训，绝不重试）
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("RichEdit50W"),
                None,
                WS_CHILD
                    | WS_VISIBLE
                    | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(0x4 | 0x80 | 0x1000), // 可见但由渲染层遮挡/贴合，不用 LAYERED
                cfg.x,
                cfg.y,
                cfg.w,
                cfg.h,
                HWND(cfg.parent as *mut core::ffi::c_void),
                None,
                hinstance,
                None,
            )
            .map_err(|e| gale_core::GaleError::Msg(format!("RichEdit50W 创建失败: {e}")))?;
            // 事件掩码：EN_UPDATE（每次内容变化通知，差分驱动）
            SendMessageW(
                hwnd,
                EM_SETEVENTMASK,
                WPARAM(0),
                LPARAM(ENM_UPDATE as isize),
            );
            SendMessageW(hwnd, 0x435, WPARAM(0), LPARAM(100_000));
            // 显式开启 TSF 支持（RichEdit50W 默认开；显式设确保 CUAS→TSF 桥激活，
            // 纯 TSF 输入法（虎符）依赖此链路——architecture.md Mandate #1）
            SendMessageW(
                hwnd,
                EM_SETEDITSTYLE,
                WPARAM(SES_USECTF),
                LPARAM(SES_USECTF as isize),
            );
            let old = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, relay_proc as *const () as isize);
            ORIG_PROC.store(old, Ordering::Relaxed);
            RELAY_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
            // 视觉隐身（2026-10-01 修订）：不再 SetWindowRgn(1,1)。
            // 真实键盘实测（发布目录 RichEdit 后端）：1 像素 region 让虎符
            // （TSF TIP，按目标 rect/可见性决定挂载）不挂编辑上下文 →
            // imm/ctf 全 false、字母直通、无候选无组字。RichEdit 保持完整
            // 客户区（place() 每帧贴打字行），视觉由 D2D 自绘层完全覆盖
            //（渲染顺序：本控件在底、D2D 帧在上，无残影）。
            Ok(Self {
                hwnd: hwnd.0 as isize,
                composing: false,
                last_text: String::new(),
                last_preedit: String::new(),
                cancellation: CompositionCancellation::default(),
            })
        }
    }

    pub fn focus(&self) {
        unsafe {
            let _ = SetFocus(HWND(self.hwnd as *mut core::ffi::c_void));
        }
    }

    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(HWND(self.hwnd as *mut core::ffi::c_void), SW_SHOWNA);
        }
    }

    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(HWND(self.hwnd as *mut core::ffi::c_void), SW_HIDE);
        }
    }

    /// 贴到打字光标（客户区物理坐标）
    pub fn place(&self, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            let _ = SetWindowPos(
                HWND(self.hwnd as *mut _),
                None,
                x,
                y + 2,
                w,
                h,
                SWP_NOZORDER,
            );
        }
    }

    fn text_snapshot(&self) -> (Option<String>, String, bool) {
        unsafe {
            let hwnd = HWND(self.hwnd as *mut core::ffi::c_void);
            let tsf = tsf_composition(hwnd);
            let full = document_utf16(hwnd);
            if tsf.active {
                let split = tsf
                    .ranges
                    .as_deref()
                    .and_then(|ranges| split_composition(&full, ranges));
                return match split {
                    Some((committed, preedit)) => (Some(committed), preedit, true),
                    None => (None, comp_string(hwnd), true),
                };
            }
            let comp = comp_string(hwnd);
            if comp.is_empty() {
                return (Some(normalize_document(&full)), comp, false);
            }
            // Legacy IMM mirror fallback. Only remove a confirmed matching
            // suffix; a separate IMM preedit must not truncate committed text.
            let suffix: Vec<u16> = comp.encode_utf16().collect();
            let committed = if full.ends_with(&suffix) {
                normalize_document(&full[..full.len() - suffix.len()])
            } else {
                normalize_document(&full)
            };
            (Some(committed), comp, true)
        }
    }

    /// Read committed text after excluding the real active composition ranges.
    /// If a TIP cannot supply valid ACP extents, defer the diff conservatively.
    pub fn committed_text(&self) -> String {
        self.text_snapshot()
            .0
            .unwrap_or_else(|| self.last_text.clone())
    }

    /// 轮询：差分 → 事件流（含组字状态迁移与 preedit）。
    /// 主循环每帧调用；不依赖通知消息也能推进（稳）。
    pub fn poll(&mut self) -> Vec<HostEvent> {
        let mut evs = Vec::new();
        // 组字状态与 preedit
        let (committed, comp, now_composing) = self.text_snapshot();
        let unchanged = committed.as_deref() == Some(self.last_text.as_str());
        let intent = if !now_composing || !unchanged {
            CANCEL_INTENT.swap(false, Ordering::Relaxed)
        } else {
            false
        };
        if self
            .cancellation
            .observe(self.composing, now_composing, unchanged, intent)
        {
            // Resolve the old composition before attributing any new physical
            // key that arrived on this message-pump iteration.
            evs.push(HostEvent::Cancelled);
        }
        if let Ok(mut q) = PHYSICAL_EVENTS.lock() {
            evs.extend(q.drain(..));
        }
        if now_composing != self.composing {
            self.composing = now_composing;
            COMPOSING.store(now_composing, Ordering::Relaxed);
            evs.push(HostEvent::Composing(now_composing));
        }
        if comp != self.last_preedit {
            if input_log_enabled() {
                input_log(&format!(
                    "poll composition active={now_composing} preedit_utf16={} committed_chars={:?}",
                    comp.encode_utf16().count(),
                    committed.as_ref().map(|s| s.chars().count())
                ));
            }
            evs.push(HostEvent::Preedit(comp.clone()));
            self.last_preedit = comp.clone();
        }
        // 上屏差分
        let Some(committed) = committed else {
            return evs;
        };
        if input_log_enabled() && committed != self.last_text {
            input_log(&format!(
                "poll diff: last={} chars new={} chars → {}",
                self.last_text.chars().count(),
                committed.chars().count(),
                match (committed.chars().count(), self.last_text.chars().count()) {
                    (n, o) if n >= o => format!("Commit({} ch)", n - o),
                    (n, o) => format!("Backspace x{}", o - n),
                }
            ));
        }
        let diff = self.diff_events(&committed);
        let committed_group = diff.iter().any(|ev| matches!(ev, HostEvent::Commit(_)));
        evs.extend(diff);
        if now_composing && committed_group {
            // Top-code IMEs may commit the old group and start a new one in one
            // edit session. Commit clears the application's old composition;
            // restore the still-active real composition after that event.
            evs.push(HostEvent::Composing(true));
            evs.push(HostEvent::Preedit(comp));
        }
        evs
    }

    pub fn reset_text(&mut self) {
        self.set_text("");
    }

    pub fn set_text(&mut self, s: &str) {
        unsafe {
            let mut w: Vec<u16> = s.encode_utf16().collect();
            w.push(0);
            let _ = SendMessageW(
                HWND(self.hwnd as *mut core::ffi::c_void),
                WM_SETTEXT,
                None,
                LPARAM(w.as_ptr() as isize),
            );
            let len = s.encode_utf16().count() as isize;
            let _ = SendMessageW(
                HWND(self.hwnd as *mut core::ffi::c_void),
                EM_SETSEL,
                WPARAM(len as usize),
                LPARAM(len),
            );
        }
        self.last_text = s.to_string();
        self.composing = false;
        self.last_preedit.clear();
        self.cancellation = CompositionCancellation::default();
        COMPOSING.store(false, Ordering::Relaxed);
        CANCEL_INTENT.store(false, Ordering::Relaxed);
    }

    /// 截断到 idx 个字符（点击回退用）
    pub fn truncate(&mut self, idx: usize) {
        let keep: String = self.last_text.chars().take(idx).collect();
        self.set_text(&keep);
    }

    // ── 候选窗/组字窗定位（晴跟打 Pro Win32.cs:264-315 同款去抖）──
    //
    // 背景：gale 的 RichEdit 是空 region 不可见控件，输入法默认把组字窗/候选窗
    // 贴到控件认为的 caret 位置（可能落在屏幕边缘或旧位置）。真实 caret 坐标
    // 由渲染层（自绘打字行）掌握，通过 set_ime_anchor 喂进来。
    //
    // 去抖规则（晴跟打实证有效）：
    // - 坐标变化 > 阈值才真正调 ImmSetCompositionWindow(CFS_FORCE_POSITION) +
    //   ImmSetCandidateWindow——IME 收到定位调用会重排候选窗（闪），坐标没变
    //   就调是候选窗抖动根源；
    // - 坐标是**客户区**物理像素；ImmSet*Window 收的是**屏幕**坐标（内部换算）。

    /// 把渲染层的光标坐标（主窗口客户区物理像素）设为 IME 组字窗/候选窗锚点。
    /// 坐标变化超过去抖阈值才会真正下发 Imm 调用。返回 true = 本次下发了定位。
    pub fn set_ime_anchor(&self, client_x: i32, client_y: i32) -> bool {
        unsafe { set_ime_anchor_impl(self.hwnd, client_x, client_y) }
    }

    /// RichEdit 控件自身 caret 的客户区坐标（相对主窗口；探针/E2E 校准用）。
    /// 控件不可见但 caret 逻辑位置仍有效。返回 None = 控件无 caret。
    pub fn caret_client_pos(&self) -> Option<(i32, i32)> {
        unsafe {
            let host = HWND(self.hwnd as *mut core::ffi::c_void);
            let mut cpt = POINT::default();
            if GetCaretPos(&mut cpt).is_err() {
                return None;
            }
            // RichEdit 客户区 → 屏幕坐标（caret 在控件客户区内）
            let mut pt = cpt;
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(host, &mut pt);
            // 主窗口屏幕坐标 → 客户区
            let parent = GetParent(host).unwrap_or(host);
            let mut ppt = pt;
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(parent, &mut ppt);
            Some((ppt.x, ppt.y))
        }
    }

    /// 差分提取：新文本 − 旧文本 → Commit；变短 → Backspace×n
    /// （保留给单测的纯逻辑入口；poll 内部同规则）
    pub fn diff_events(&mut self, new_text: &str) -> Vec<HostEvent> {
        let mut evs = Vec::new();
        let common = self
            .last_text
            .chars()
            .zip(new_text.chars())
            .take_while(|(a, b)| a == b)
            .count();
        for _ in common..self.last_text.chars().count() {
            evs.push(HostEvent::Backspace);
        }
        let added: String = new_text.chars().skip(common).collect();
        if !added.is_empty() {
            evs.push(HostEvent::Commit(added));
        }
        self.last_text = new_text.to_string();
        evs
    }

    /// 重置锚点记忆（段切换/窗口移动后调；下一次 set_ime_anchor 必定下发）
    pub fn reset_anchor(&self) {
        reset_ime_anchor();
    }
}

pub fn load_library() {
    if LIB_LOADED.load(Ordering::Relaxed) {
        return;
    }
    unsafe {
        if LoadLibraryW(w!("msftedit.dll")).is_ok() {
            LIB_LOADED.store(true, Ordering::Relaxed);
        }
    }
}

// ── 候选窗/组字窗定位（晴跟打 Pro Win32.cs:264-315 同款去抖）──
//
// 背景：gale 的 RichEdit 是空 region 不可见控件，输入法默认把组字窗/候选窗
// 贴到控件认为的 caret 位置。真实 caret 坐标由渲染层（自绘打字行）掌握，
// 通过 InputHost::set_ime_anchor 喂进来。
//
// 去抖规则（晴跟打实证有效）：坐标变化 > 阈值才真正调
// ImmSetCompositionWindow(CFS_FORCE_POSITION) + ImmSetCandidateWindow ——
// IME 收到定位调用会重排候选窗（闪），坐标没变就调是候选窗抖动根源。
// 坐标是主窗口客户区物理像素；ImmSet*Window 收屏幕坐标（内部换算）。

/// 坐标去抖阈值（物理像素）：小于此位移不重设候选窗
const ANCHOR_JITTER_PX: i32 = 3;
/// 上次设置的锚点（None = 从未设过）
static LAST_ANCHOR: Mutex<Option<(i32, i32)>> = Mutex::new(None);

/// 锚点下发实现（InputHost::set_ime_anchor 的自由函数体）
unsafe fn set_ime_anchor_impl(hwnd: isize, client_x: i32, client_y: i32) -> bool {
    // 1) 去抖判定
    {
        let Ok(mut last) = LAST_ANCHOR.lock() else {
            return false;
        };
        if let Some((lx, ly)) = *last {
            if (lx - client_x).abs() <= ANCHOR_JITTER_PX
                && (ly - client_y).abs() <= ANCHOR_JITTER_PX
            {
                return false;
            }
        }
        *last = Some((client_x, client_y));
    }
    // 2) 主窗口客户区 → 屏幕坐标（RichEdit 是子窗口，锚点语义以主窗口为准）
    let host = HWND(hwnd as *mut core::ffi::c_void);
    let parent = GetParent(host).unwrap_or(host);
    let mut pt = POINT {
        x: client_x,
        y: client_y,
    };
    let _ = windows::Win32::Graphics::Gdi::ClientToScreen(parent, &mut pt);
    // 3) 组字窗定位（CFS_FORCE_POSITION：强制贴 pt，不让 IME 自己排版）
    let himc = windows::Win32::UI::Input::Ime::ImmGetContext(host);
    if himc.is_invalid() {
        return false;
    }
    let cf = COMPOSITIONFORM {
        dwStyle: CFS_FORCE_POSITION,
        ptCurrentPos: pt,
        rcArea: windows::Win32::Foundation::RECT::default(),
        ..Default::default()
    };
    let ok1 = ImmSetCompositionWindow(himc, &cf);
    // 4) 候选窗跟在组字窗正下方（晴跟打同款：y + 行高）
    let mut cand = CANDIDATEFORM {
        dwStyle: CFS_FORCE_POSITION,
        ptCurrentPos: POINT {
            x: pt.x,
            y: pt.y + 28,
        },
        rcArea: windows::Win32::Foundation::RECT::default(),
        ..Default::default()
    };
    let ok2 = ImmSetCandidateWindow(himc, &mut cand);
    let _ = windows::Win32::UI::Input::Ime::ImmReleaseContext(host, himc);
    input_log(&format!(
        "IME 锚点 → ({}, {}) comp={} cand={}",
        pt.x,
        pt.y,
        ok1.as_bool(),
        ok2.as_bool()
    ));
    ok1.as_bool()
}

/// 重置锚点去抖记忆（段切换/窗口移动后调；下一次 set_ime_anchor 必定下发）
pub fn reset_ime_anchor() {
    if let Ok(mut last) = LAST_ANCHOR.lock() {
        *last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_commit_and_backspace() {
        let mut h = InputHost {
            hwnd: 0,
            composing: false,
            last_text: String::new(),
            last_preedit: String::new(),
            cancellation: CompositionCancellation::default(),
        };
        let evs = h.diff_events("你好");
        assert!(matches!(&evs[0], HostEvent::Commit(s) if s == "你好"));
        let evs = h.diff_events("你"); // 变短 → Backspace
        assert!(matches!(&evs[0], HostEvent::Backspace));
        let evs = h.diff_events("你们"); // 追加 → Commit("们")
        assert!(matches!(&evs[0], HostEvent::Commit(s) if s == "们"));
        let evs = h.diff_events("你"); // 再删 → Backspace
        assert!(matches!(&evs[0], HostEvent::Backspace));
        let evs = h.diff_events("你们好"); // 追加两字 → Commit("们好")
        assert!(matches!(&evs[0], HostEvent::Commit(s) if s == "们好"));
    }

    #[test]
    fn same_length_and_supplementary_replacements_are_reported() {
        let mut h = InputHost {
            hwnd: 0,
            composing: false,
            last_text: "甲😀乙".into(),
            last_preedit: String::new(),
            cancellation: CompositionCancellation::default(),
        };
        let ev = h.diff_events("甲🦊乙");
        assert_eq!(ev.len(), 3);
        assert!(matches!(ev[0], HostEvent::Backspace));
        assert!(matches!(ev[1], HostEvent::Backspace));
        assert!(matches!(&ev[2],HostEvent::Commit(t)if t=="🦊乙"));
        assert!(h.diff_events("甲🦊乙").is_empty());
    }

    #[test]
    fn real_tsf_preedit_is_excluded_until_committed() {
        let mut h = InputHost {
            hwnd: 0,
            composing: false,
            last_text: String::new(),
            last_preedit: String::new(),
            cancellation: CompositionCancellation::default(),
        };
        let document: Vec<u16> = "j".encode_utf16().collect();
        let (committed, preedit) = split_composition(&document, &[0..1]).unwrap();
        assert_eq!(preedit, "j");
        assert!(h.diff_events(&committed).is_empty());
        // A top-code commit may insert a character and start the next
        // composition within the same key event.
        let document: Vec<u16> = "人sm".encode_utf16().collect();
        let (committed, preedit) = split_composition(&document, &[1..3]).unwrap();
        assert_eq!(preedit, "sm");
        let events = h.diff_events(&committed);
        assert!(matches!(&events[..], [HostEvent::Commit(s)] if s == "人"));
        let document: Vec<u16> = "人什么".encode_utf16().collect();
        let (committed, preedit) = split_composition(&document, &[]).unwrap();
        assert!(preedit.is_empty());
        let events = h.diff_events(&committed);
        assert!(matches!(&events[..], [HostEvent::Commit(s)] if s == "什么"));
    }

    #[test]
    fn tsf_acp_spans_are_applied_before_unicode_and_newline_conversion() {
        let document: Vec<u16> = "甲\r😀j乙".encode_utf16().collect();
        assert_eq!(
            split_composition(&document, &[4..5]),
            Some(("甲\n😀乙".into(), "j".into()))
        );
        assert_eq!(
            split_composition(&document, &[2..4]),
            Some(("甲\nj乙".into(), "😀".into()))
        );
        assert!(split_composition(&document, &[3..4]).is_none());
        assert!(split_composition(&document, &[4..7]).is_none());
        assert!(split_composition(&document, &[2..4, 3..5]).is_none());
        assert_eq!(
            split_composition(&document, &[4..4]),
            Some(("甲\n😀j乙".into(), String::new()))
        );
    }

    #[test]
    fn cancellation_requires_explicit_intent_and_another_stable_snapshot() {
        let mut cancellation = CompositionCancellation::default();
        // Normal END can precede COMMIT. An empty composition alone is not a
        // cancellation and cannot clear the key attribution window.
        assert!(!cancellation.observe(true, false, true, false));
        assert!(!cancellation.observe(false, false, false, false));
        assert!(!cancellation.observe(false, false, true, false));
        // Explicit Esc/Backspace with no committed change arms a probe.
        assert!(!cancellation.observe(true, false, true, true));
        assert!(cancellation.observe(false, false, true, false));
        assert!(!cancellation.observe(false, false, true, false));
        // A delayed commit invalidates the cancellation probe.
        assert!(!cancellation.observe(true, false, true, true));
        assert!(!cancellation.observe(false, false, false, false));
        // An immediate new composition resolves the old cancel first, before
        // its new physical key is attributed by the application.
        assert!(!cancellation.observe(true, false, true, true));
        assert!(cancellation.observe(false, true, true, false));
    }

    #[test]
    fn host_event_to_input_event() {
        let ev: Option<InputEvent> = (&HostEvent::Commit("甲".into())).into();
        assert_eq!(
            ev,
            Some(InputEvent::Commit {
                chars: "甲".into()
            })
        );
        let ev: Option<InputEvent> = (&HostEvent::Backspace).into();
        assert_eq!(ev, Some(InputEvent::Backspace));
        let ev: Option<InputEvent> = (&HostEvent::Preedit("n".into())).into();
        assert_eq!(ev, None);
    }

    #[test]
    fn command_key_queue_roundtrip() {
        push_cmd(CommandKey::F2);
        push_cmd(CommandKey::AltE);
        let cmds = drain_cmds();
        assert_eq!(cmds, vec![CommandKey::F2, CommandKey::AltE]);
        assert!(drain_cmds().is_empty());
    }

    #[test]
    fn continuation_keys_require_completed_round_and_explicit_target() {
        for (vk, command) in [(0x0D, CommandKey::Enter), (0x20, CommandKey::Space)] {
            // Both the typing host and the registered result container use
            // this path before the non-host key early return.
            assert_eq!(
                completion_command(vk, true, true, false, false, false),
                Some(command)
            );
            assert_eq!(
                completion_command(vk, false, true, false, false, false),
                None
            );
            assert_eq!(
                completion_command(vk, true, false, false, false, false),
                None
            );
            assert_eq!(completion_command(vk, true, true, true, false, false), None);
            assert_eq!(completion_command(vk, true, true, false, true, false), None);
            assert_eq!(completion_command(vk, true, true, false, false, true), None);
        }
        assert_eq!(
            completion_command(0x41, true, true, false, false, false),
            None
        );
    }

    /// 真实 RichEdit 控件的 WM_CHAR → poll 全链（GUI 单测；无头环境跳过）。
    /// E2E 断链排查用：证明"SendMessage(WM_CHAR) 进控件 → committed_text 差分 → Commit 事件"
    /// 在同线程内是否成立。需要桌面会话（CI/无头会 fail，故 ignore 默认不跑）。
    #[test]
    #[ignore]
    fn real_richedit_wmchar_to_poll() {
        // 宿主父窗口：临时顶层窗口（RichEdit 是子控件，必须有真实父窗口）
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );
        }
        let parent = match crate::window::MainWindow::create("e2e-richedit-probe", 300, 100) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("跳过：父窗口创建失败（{e}）");
                return;
            }
        };
        let cfg = HostConfig {
            parent: parent.hwnd,
            x: 0,
            y: 0,
            w: 200,
            h: 30,
        };
        let mut host = match InputHost::create(&cfg) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("跳过：RichEdit 不可用（{e}）");
                return;
            }
        };
        unsafe {
            let h = HWND(host.hwnd as *mut core::ffi::c_void);
            for ch in "天青色".chars() {
                let _ = SendMessageW(h, WM_CHAR, WPARAM(ch as usize), LPARAM(0));
            }
        }
        let evs = host.poll();
        let commits: Vec<String> = evs
            .iter()
            .filter_map(|e| match e {
                HostEvent::Commit(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        eprintln!("poll events: {evs:?}");
        assert_eq!(
            commits.join(""),
            "天青色",
            "WM_CHAR 后 poll 应差分出 Commit"
        );
    }

    #[test]
    fn queue_bounded() {
        for _ in 0..80 {
            push_cmd(CommandKey::Enter);
        }
        assert!(drain_cmds().len() <= 64);
    }

    // ── 组字判定矩阵（嫌疑1修复的规格：双通道任一非空 = 组字中）──
    // imm = IMM GCS_COMPSTR 通道（IMM 型输入法：虎娘/微软拼音）
    // ctf = EM_GETIMECOMPTEXT 通道（纯 TSF 输入法：虎符）
    // 断言：仅 imm 空 ctf 非空时旧实现误判"未组字"→提交键被吞——新判定必须为 true。
    #[test]
    fn composing_matrix_both_empty() {
        assert!(!composing_verdict(false, false), "双通道空 = 未组字");
    }

    #[test]
    fn composing_matrix_imm_only() {
        // IMM 型输入法（虎娘/微软拼音传统层）：GCS_COMPSTR 有、CTF 通道可能空
        assert!(composing_verdict(true, false), "IMM 通道非空即组字");
    }

    #[test]
    fn composing_matrix_ctf_only() {
        // 纯 TSF（虎符）：CUAS 桥组字不反映到 IMM 通道——旧实现在这里断链
        assert!(
            composing_verdict(false, true),
            "CTF 通道非空即组字（纯 TSF 关键路径）"
        );
    }

    #[test]
    fn composing_matrix_both_active() {
        assert!(composing_verdict(true, true), "双通道非空当然组字");
    }

    // ── 退格口径状态机（晴跟打 ImeBackspacePolicy 行为规格的独立实现）──
    // 规格（_ref\TypeSunny\UI\Modes\ImeBackspacePolicy.cs + 13 用例）：
    // - 组字中的退格 = 清组合，不算跟打回改（归 IME）；
    // - 提交后的退格才算回改；
    // - 退格把组合删空后，下一个物理退格仍归 IME（protect 窗口），
    //   再往后才归跟打区；
    // - 提交清空组合不置 protect（区分"Space 提交瞬间组合清空"与"退格清空"）；
    // - keyup 清 protect/进行中标志。

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum BsVerdict {
        /// 归输入法（清组合；不进跟打统计）
        ToIme,
        /// 归跟打区（回改判定走比对状态机）
        ToTyping,
    }

    /// 退格归属裁决器（组字边界状态机；纯逻辑可单测）
    #[derive(Debug, Default)]
    struct BackspaceRouter {
        composing: bool,
        /// 退格清空组合后置位：下一次退格仍归 IME
        protect: bool,
    }

    impl BackspaceRouter {
        /// 输入法侧通知：组字开始/更新/结束（commit 清空时 clear_kind=Commit）
        fn notify_composition(&mut self, now_composing: bool, cleared_by_commit: bool) {
            let was = self.composing;
            self.composing = now_composing;
            if was && !now_composing {
                if cleared_by_commit {
                    // 提交清空：不 protect，紧随的退格直接归跟打
                    self.protect = false;
                } else {
                    // 非提交清空（退格删空组合）：protect 一个窗口
                    self.protect = true;
                }
            }
        }

        /// 物理退格按下：裁决归属
        fn on_backspace(&mut self) -> BsVerdict {
            if self.composing {
                return BsVerdict::ToIme;
            }
            if self.protect {
                self.protect = false; // protect 只护一次
                return BsVerdict::ToIme;
            }
            BsVerdict::ToTyping
        }

        /// 物理退格弹起：清标志（会话边界安全网）
        fn on_backspace_up(&mut self) {
            self.protect = false;
        }
    }

    #[test]
    fn backspace_policy_composing_goes_to_ime() {
        let mut r = BackspaceRouter::default();
        r.notify_composition(true, false); // 组字开始
        assert_eq!(r.on_backspace(), BsVerdict::ToIme);
        assert_eq!(r.on_backspace(), BsVerdict::ToIme);
    }

    #[test]
    fn backspace_policy_after_commit_goes_to_typing() {
        let mut r = BackspaceRouter::default();
        r.notify_composition(true, false);
        r.notify_composition(false, true); // 空格提交，组合清空
        assert_eq!(r.on_backspace(), BsVerdict::ToTyping);
    }

    #[test]
    fn backspace_policy_depleted_composition_protects_once() {
        let mut r = BackspaceRouter::default();
        r.notify_composition(true, false);
        r.notify_composition(false, false); // 退格把组合删空（非提交）
        assert_eq!(r.on_backspace(), BsVerdict::ToIme); // protect 窗口
        assert_eq!(r.on_backspace(), BsVerdict::ToTyping); // 之后归跟打
    }

    #[test]
    fn backspace_policy_commit_clear_does_not_protect() {
        // 晴跟打反例回归：提交清空 vs 退格清空必须区分
        let mut r = BackspaceRouter::default();
        r.notify_composition(true, false);
        r.notify_composition(false, true); // Commit 清空
        assert_eq!(r.on_backspace(), BsVerdict::ToTyping);
        assert_eq!(r.on_backspace(), BsVerdict::ToTyping);
    }

    #[test]
    fn backspace_policy_keyup_resets_protect() {
        let mut r = BackspaceRouter::default();
        r.notify_composition(true, false);
        r.notify_composition(false, false);
        r.on_backspace_up(); // 物理弹起
        assert_eq!(r.on_backspace(), BsVerdict::ToTyping);
    }

    #[test]
    fn backspace_policy_idle_typing() {
        let mut r = BackspaceRouter::default();
        assert_eq!(r.on_backspace(), BsVerdict::ToTyping);
    }
}
