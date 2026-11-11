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
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows::Win32::UI::Controls::RichEdit::{
    EM_GETTEXTEX, EM_GETTEXTLENGTHEX, EM_SETUNDOLIMIT, GETTEXTEX, GETTEXTLENGTHEX, GTL_NUMCHARS,
    GTL_PRECISE, GT_DEFAULT,
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
    CallWindowProcW, CreateWindowExW, DefWindowProcW, GetCaretPos, GetParent, HideCaret,
    RegisterWindowMessageW, SendMessageW, SetPropW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    GWLP_WNDPROC, SWP_NOZORDER, SW_HIDE, SW_SHOWNA, WINDOW_EX_STYLE, WM_CHAR, WM_KEYDOWN, WM_KEYUP,
    WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP, WS_CHILD, WS_VISIBLE,
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
    /// A real editing/selection key that resumes the clock without adding a
    /// statistical key (undo/redo, or a configured non-statistical IME key).
    ResumeKey { vk: u32 },
    /// Top-level deactivation, ordered with physical keys. The host's final
    /// text diff follows this queue, so the app must flush it before pausing.
    FocusLost,
    /// The active HuFu TIP confirmed a successful explicit candidate selection.
    /// Sentence locks are reported immediately, even before text is committed.
    CandidateSelection { rank: u8, vk: Option<u32> },
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
/// 每键时间戳队列（统计击键 + 20ms 选重/标顶配对共用），元素 = (vk, 毫秒)。
/// 生产者：note_key（键泵在 TSF 改写 VK 前调用）；消费者：drain_key_times
///（统计）与 note_commit（配对判定，只读不消费）。
static KEY_TIMES: Mutex<Vec<(u32, u64)>> = Mutex::new(Vec::new());
/// Attribution must survive the per-frame heat-map drain until text arrives.
static COMMIT_KEY_HISTORY: Mutex<Vec<(u32, u64)>> = Mutex::new(Vec::new());
static EXACT_SELECTION_SOURCE: AtomicBool = AtomicBool::new(false);
static PHYSICAL_EVENTS: Mutex<Vec<HostEvent>> = Mutex::new(Vec::new());

/// Called from the top-level window procedure even when focus leaves and
/// returns within one native message-pump iteration.
pub fn note_focus_lost() {
    if let Ok(mut events) = PHYSICAL_EVENTS.lock() {
        events.push(HostEvent::FocusLost);
    }
}

fn push_resume_key(vk: u32) {
    if let Ok(mut events) = PHYSICAL_EVENTS.lock() {
        events.push(HostEvent::ResumeKey { vk });
    }
}
/// EM_SETEDITSTYLE(SES_USECTF) 校验结果（InputHost::create 写入；
/// false = TSF 桥可能没建立，上层可提示"TSF 输入法可能不可用"）
static CTF_OK: AtomicBool = AtomicBool::new(true);

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
    // Heat-map bookkeeping: skip keyboard auto-repeat (bit 30), the F row
    // (product shortcuts, not typed keys) and the shortcut payload keys of
    // Ctrl+←/→, Ctrl+V and Alt+E so the measured keyboard only shows real
    // typing keys. The 20 ms selection pairing shares this path.
    let repeat = (msg.lParam.0 as usize) & (1 << 30) != 0;
    let product_shortcut = (ctrl && !composing && matches!(vk, 0x25 | 0x27 | 0x56))
        || (alt && !composing && vk == 0x45);
    let function_row = (0x70..=0x7b).contains(&vk);
    if !repeat && !product_shortcut && !function_row {
        note_key(
            physical_heatmap_vk(vk, msg.lParam.0 as usize),
            query_tick_ms(),
        );
    }
    let physical_vk = physical_heatmap_vk(vk, msg.lParam.0 as usize);
    let resume_selection = {
        let configured = SELECTION_KEYS.lock().unwrap_or_else(|e| e.into_inner());
        nonstat_selection_resumes(physical_vk, composing, &configured)
    };
    if resume_selection {
        push_resume_key(physical_vk);
    }
    if ctrl && !composing {
        match vk {
            0x25 => push_cmd(CommandKey::CtrlLeft),
            0x27 => push_cmd(CommandKey::CtrlRight),
            0x56 => push_cmd(CommandKey::LoadClipboard),
            // Native undo/redo must reach RichEdit through the message pump;
            // relay_proc cannot handle a shortcut discarded before dispatch.
            0x5A | 0x59 => {
                push_resume_key(vk);
                return false;
            }
            _ => {}
        }
        return true;
    }
    if alt && !composing {
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

/// Resolve the physical side of modifiers and distinguish the numpad Enter.
fn physical_heatmap_vk(vk: u32, lparam: usize) -> u32 {
    let scan = (lparam >> 16) & 0xff;
    let extended = lparam & (1 << 24) != 0;
    match vk {
        0x10 => {
            if scan == 0x36 {
                0xa1
            } else {
                0xa0
            }
        }
        0x11 => {
            if extended {
                0xa3
            } else {
                0xa2
            }
        }
        0x12 => {
            if extended {
                0xa5
            } else {
                0xa4
            }
        }
        0x0d if extended => 0x0e,
        _ => vk,
    }
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

// TSF 后端窗口级键路由 route_key 已删除（2026-11-08 死代码清理）：
// 该路径从未被调用——命令键捕获与统计记账统一走 capture_physical_key
// （键泵预过滤）+ relay_proc（RichEdit 窗口过程），KEY_TIMES 记账由
// note_key 产品化承接（见下）。保留本注释给读过旧实现的人对齐。

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

// ── 输入诊断日志（GALE_INPUT_LOG=1 或 exe 旁 输入诊断.开 文件时启用）──
// 落点：优先 exe 旁 输入诊断.log（便携目录直写）；写失败（Program Files
// 只读等）自动切 %LOCALAPPDATA%\HuFu\trainer\logs\input-diagnostic.log 并
// 永久记住新路径。
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
    let ts = query_tick_ms() % 86_400_000; // 毫秒级时戳足够诊断
    let Ok(Some(path)) = INPUT_LOG_PATH.lock().map(|g| g.clone()) else {
        return;
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "[+{ts}ms] {s}");
        return;
    }
    // exe 旁写失败（Program Files 等）→ 回退 %LOCALAPPDATA%\HuFu\trainer\logs\
    // input-diagnostic.log；成功后永久记住新路径（后续直写，不再试错）。
    if let Some(fallback) = localappdata_log_path() {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&fallback)
        {
            let _ = writeln!(f, "[+{ts}ms] {s}");
            if let Ok(mut g) = INPUT_LOG_PATH.lock() {
                *g = Some(fallback);
            }
            eprintln!("[gale] 输入诊断日志：exe 旁不可写，已切换到 %LOCALAPPDATA%\\HuFu\\trainer\\logs\\input-diagnostic.log");
        }
    }
}

/// 诊断日志回退路径：%LOCALAPPDATA%\HuFu\trainer\logs\input-diagnostic.log
///（目录不存在则创建；LOCALAPPDATA 缺失时返回 None 放弃）
fn localappdata_log_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    let dir = std::path::PathBuf::from(base)
        .join("HuFu")
        .join("trainer")
        .join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("input-diagnostic.log"))
}

/// tsf_focus 模块用的诊断日志出口（同 GALE_INPUT_LOG 开关）
pub fn diagnostics_enabled() -> bool {
    input_log_enabled()
}
/// UI 提示用：输入诊断日志是否在写（含 LOCALAPPDATA 回退态）
pub fn diagnostic_enabled() -> bool {
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
    if msg >= 0xC000 && msg == input_outcome_message() {
        if wp.0 <= 10 {
            EXACT_SELECTION_SOURCE.store(true, Ordering::Relaxed);
            if wp.0 != 0 {
                let vk = (1..=255).contains(&lp.0).then_some(lp.0 as u32);
                if let Ok(mut events) = PHYSICAL_EVENTS.lock() {
                    events.push(HostEvent::CandidateSelection {
                        rank: wp.0 as u8,
                        vk,
                    });
                }
                input_log(&format!(
                    "exact candidate selection rank={} vk={vk:?}",
                    wp.0
                ));
            }
        }
        return LRESULT(0);
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
        0x0051 => {
            // WM_INPUTLANGCHANGE: the next IME may use legacy attribution.
            EXACT_SELECTION_SOURCE.store(false, Ordering::Relaxed);
            if let Ok(mut keys) = COMMIT_KEY_HISTORY.lock() {
                keys.clear();
            }
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wp.0 as u32;
            // ── 防作弊：训练正文区拦一切右键菜单通道（2026-11-08）──
            // VK_APPS（菜单键 0x5D）与 Shift+F10（等效菜单键；F10 走
            // WM_SYSKEYDOWN，Shift 态查 GetKeyState）直接吞。组字中也拦
            //（训练模式全拦）；随后系统补投的 WM_CONTEXTMENU 由该消息臂兜底。
            if vk == 0x5D || (vk == 0x79 && GetKeyState(VK_SHIFT.0 as i32) < 0) {
                return LRESULT(0);
            }
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
                    // Ctrl+Z / Ctrl+Y are legitimate corrections. RichEdit
                    // handles WM_UNDO/WM_REDO; the document diff below turns
                    // the shrink into Backspace events for the comparer.
                    0x5A | 0x59 => return call_orig!(),
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
        WM_KEYUP | WM_SYSKEYUP => {
            // VK_APPS 弹起也吞（与 keydown 对称，防系统补发菜单消息）
            if wp.0 as u32 == 0x5D {
                return LRESULT(0);
            }
        }
        0x007B => return LRESULT(0), // WM_CONTEXTMENU：训练模式全拦（右键/VK_APPS/Shift+F10 的公共出口）
        0x0300 | 0x0301 | 0x0302 | 0x0303 => return LRESULT(0), // cut/copy/paste/clear are not training commits
        0x0304 => return call_orig!(),                          // WM_UNDO is a user correction
        WM_SYSCHAR => return LRESULT(0), // Alt+字母字符直通是疾风1代漏字根源，吞
        _ => {}
    }
    call_orig!()
}

/// 取走击键时间戳队列（统计引擎消费；TSF 后端也用同一队列——
/// route_key 记账，主循环 drain）
/// 取走击键时间戳队列（统计引擎消费；主循环每帧在 poll 之后调，
/// 保证 note_commit 配对先于清空看到本帧击键）
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

/// 记录一次物理键按下（键泵在 TSF 改写 VK_PROCESSKEY 之前调用；
/// 统计击键与 20ms 选重/标顶配对共用同一队列）
pub fn note_key(vk: u32, t_ms: u64) {
    if let Ok(mut kt) = KEY_TIMES.lock() {
        kt.push((vk, t_ms));
        if kt.len() > 512 {
            kt.drain(..256);
        }
    }
    if let Ok(mut history) = COMMIT_KEY_HISTORY.lock() {
        history.push((vk, t_ms));
        if history.len() > 64 {
            history.drain(..32);
        }
    }
}

fn input_outcome_message() -> u32 {
    static MESSAGE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(w!("HuFu.Trainer.InputOutcome.v1")) })
}

/// 上屏归因（对照晴跟打 Pro Score.cs 的 20ms 窗语义）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitCause {
    /// 顶屏：配对窗内最后按键是编码键延续（字母）——继续打码把首选顶上屏
    Ding,
    /// 选重：配对窗内最后按键是数字或当前方案的选重键。
    Select,
    /// A configured non-statistical key actually selected a candidate. Only this
    /// proven use may add Shift/Caps/etc. to the completed output key batch.
    SelectExtra { vk: u32, lag_ms: u64 },
    /// 无法判定（窗内无键、或最后按键不在两类内）——不计入选重/顶屏统计
    Unknown,
}

/// 20ms 配对窗（毫秒）：|commit_ms − key_ms| ≤ 此值视为该键引发的上屏
const COMMIT_PAIR_WINDOW_MS: u64 = 20;

/// 上屏归因待取队列（drain_commit_causes 消费）
static COMMIT_CAUSES: Mutex<Vec<CommitCause>> = Mutex::new(Vec::new());

/// Additional physical selection keys supplied by the active IME scheme. This
/// does not change StatsEngine's ordinary key filter or create HostEvent::Key.
#[derive(Clone, Copy)]
struct SelectionKeys([bool; 256]);
impl SelectionKeys {
    const fn empty() -> Self {
        Self([false; 256])
    }
    fn from_vks(vks: &[u32]) -> Self {
        let mut keys = Self::empty();
        for &vk in vks {
            if vk == 0 || vk > 255 {
                continue;
            }
            keys.0[vk as usize] = true;
            // capture_physical_key resolves generic modifier messages to sides.
            // A generic configured key intentionally enables both physical sides.
            let pair = match vk {
                0x10 => Some((0xA0, 0xA1)),
                0x11 => Some((0xA2, 0xA3)),
                0x12 => Some((0xA4, 0xA5)),
                _ => None,
            };
            if let Some((left, right)) = pair {
                keys.0[left] = true;
                keys.0[right] = true;
            }
        }
        keys
    }
    fn contains(&self, vk: u32) -> bool {
        vk < 256 && self.0[vk as usize]
    }
}

fn nonstat_selection_resumes(vk: u32, composing: bool, keys: &SelectionKeys) -> bool {
    composing
        && !StatsEngine::is_stat_key(vk)
        && !matches!(vk, 0x1b | 0x70..=0x7b)
        && keys.contains(vk)
}
static SELECTION_KEYS: Mutex<SelectionKeys> = Mutex::new(SelectionKeys::empty());

/// Replaces the active scheme's extra selection keys; digits remain available.
/// Pass [] when a scheme has no extra keys. Generic Shift/Ctrl/Alt includes both
/// physical sides; side-specific VKs stay side-specific. Invalid VKs are ignored.
pub fn set_selection_keys(keys: &[u32]) {
    let mut configured = SELECTION_KEYS.lock().unwrap_or_else(|e| e.into_inner());
    *configured = SelectionKeys::from_vks(keys);
}

/// 记录一次上屏并做 20ms 配对判定（poll 差分出 Commit 时调用）。
/// 取配对窗内**最后**一次按键的 vk 分类（晴 Score.cs 语义：编码延续=顶屏、
/// 数字/方案选重键=选重）。纯 ASCII 直通提交（输入法关/英文模式）不归因——无 IME
/// 参与就没有选重/顶屏可言。
pub fn note_commit(t_ms: u64, committed_text: &str) {
    if committed_text.is_ascii() {
        return;
    }
    let mut cause = match COMMIT_KEY_HISTORY.lock() {
        Ok(mut history) => {
            let cause = history
                .iter()
                .rev()
                .find(|(_, key_ms)| *key_ms <= t_ms && t_ms - *key_ms <= COMMIT_PAIR_WINDOW_MS)
                .map(|(vk, key_ms)| {
                    let cause = classify_commit_cause(*vk);
                    // Physical 0x0E is the heatmap identity for extended Enter;
                    // HostEvent::Key still carries ordinary VK_RETURN and counted it.
                    if cause == CommitCause::Select && *vk != 0x0E && !StatsEngine::is_stat_key(*vk)
                    {
                        CommitCause::SelectExtra {
                            vk: *vk,
                            lag_ms: t_ms.abs_diff(*key_ms),
                        }
                    } else {
                        cause
                    }
                })
                .unwrap_or(CommitCause::Unknown);
            history.clear();
            cause
        }
        Err(_) => CommitCause::Unknown,
    };
    if EXACT_SELECTION_SOURCE.load(Ordering::Relaxed)
        && matches!(cause, CommitCause::Select | CommitCause::SelectExtra { .. })
    {
        // The synchronous TIP event already counted this action, or explicitly
        // proved no selection (for example an out-of-range key topping text).
        cause = CommitCause::Unknown;
    }
    input_log(&format!(
        "commit 配对 text={committed_text:?} cause={cause:?}"
    ));
    if let Ok(mut q) = COMMIT_CAUSES.lock() {
        q.push(cause);
        if q.len() > 512 {
            q.drain(..256);
        }
    }
}

/// 配对窗内最后按键的 vk 分类
fn classify_commit_cause(vk: u32) -> CommitCause {
    let keys = SELECTION_KEYS.lock().unwrap_or_else(|e| e.into_inner());
    classify_commit_cause_with_keys(vk, &keys)
}
fn classify_commit_cause_with_keys(vk: u32, keys: &SelectionKeys) -> CommitCause {
    if keys.contains(vk) {
        return CommitCause::Select;
    }
    match vk {
        0x41..=0x5A => CommitCause::Ding, // 字母 = 编码键延续 → 顶屏
        0x30..=0x39 | 0x60..=0x69 => CommitCause::Select, // 数字 = 选重
        _ => CommitCause::Unknown,
    }
}

/// 取走上屏归因队列（统计层消费；选重率/顶屏率口径由此出）
pub fn drain_commit_causes() -> Vec<CommitCause> {
    match COMMIT_CAUSES.lock() {
        Ok(mut q) => std::mem::take(&mut *q),
        Err(_) => Vec::new(),
    }
}

// ── 退格归属裁决（晴跟打 ImeBackspacePolicy 13 用例语义的独立实现，产品代码）──
//
// 规格（_ref\TypeSunny\UI\Modes\ImeBackspacePolicy.cs）：
// - 组字中的退格 = 清组合，归 IME（不算跟打回改）；
// - 提交后的退格才算回改（Correction）；
// - 退格把组合删空后，下一个物理退格仍归 IME（protect 窗口），再往后归跟打区；
// - 提交清空组合不置 protect（区分"Space 提交瞬间组合清空"与"退格删空"）；
// - keyup 清 protect。
// session 层接线口径：HostEvent::Composing(false) 与同批 Commit 同时出现 →
// on_composition_change(false, true)；只有 Composing(false) → (false, false)。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackspaceClass {
    /// 归输入法（清组合；不进跟打统计）
    ImeDelete,
    /// 归跟打区（回改判定走比对状态机）
    Correction,
}

/// 退格归属裁决器（组字边界状态机；纯逻辑可单测）
#[derive(Debug, Default)]
pub struct BackspaceRouter {
    composing: bool,
    /// 退格清空组合后置位：下一次退格仍归 IME
    protect: bool,
}

impl BackspaceRouter {
    /// 输入法侧通知组字状态迁移。cleared_by_commit = 组合是否因提交清空
    ///（同批出现 Commit 事件）——提交清空不置 protect，退格删空才置。
    pub fn on_composition_change(&mut self, now_composing: bool, cleared_by_commit: bool) {
        let was = self.composing;
        self.composing = now_composing;
        if was && !now_composing {
            self.protect = !cleared_by_commit;
        }
    }

    /// 物理退格按下：裁决归属。now_composing = 按键时刻的实时组字态
    ///（防御性同步——轮询间隙的迁移以按键时读数为准）。
    pub fn on_backspace_key(&mut self, now_composing: bool) -> BackspaceClass {
        self.composing = now_composing;
        if self.composing {
            return BackspaceClass::ImeDelete;
        }
        if self.protect {
            self.protect = false; // protect 只护一次
            return BackspaceClass::ImeDelete;
        }
        BackspaceClass::Correction
    }

    /// 物理退格弹起：清标志（会话边界安全网）
    pub fn on_backspace_up(&mut self) {
        self.protect = false;
    }
}

/// RichEdit50W 输入宿主（单例持有）
pub struct InputHost {
    pub hwnd: isize,
    pub composing: bool,
    pub last_text: String,
    pub last_preedit: String,
    cancellation: CompositionCancellation,
    /// EM_SETEDITSTYLE(SES_USECTF) 是否成功（false = TSF 输入法可能不可用）
    ctf_ok: bool,
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
                    ctf_ok: CTF_OK.load(Ordering::Relaxed),
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
            // EM_SETUNDOLIMIT 的 limit 在 wParam（lParam 恒 0）。使用 SDK
            // 常量，避免误发 0x435（EM_EXLIMITTEXT）而没有设置撤销容量。
            SendMessageW(hwnd, EM_SETUNDOLIMIT, WPARAM(100_000), LPARAM(0));
            // 显式开启 TSF 支持（RichEdit50W 默认开；显式设确保 CUAS→TSF 桥激活，
            // 纯 TSF 输入法（虎符）依赖此链路——architecture.md Mandate #1）。
            // 返回值校验（2026-11-08）：成功返回旧样式位（小非负值），失败为负
            // HRESULT——记日志 + CTF_OK=false 供上层提示"TSF 输入法可能不可用"。
            let ctf_ret = SendMessageW(
                hwnd,
                EM_SETEDITSTYLE,
                WPARAM(SES_USECTF),
                LPARAM(SES_USECTF as isize),
            );
            let ctf_ok = ctf_ret.0 >= 0;
            CTF_OK.store(ctf_ok, Ordering::Relaxed);
            if !ctf_ok {
                input_log(&format!(
                    "EM_SETEDITSTYLE(SES_USECTF) 失败 ret=0x{:X}（TSF 桥可能未建立）",
                    ctf_ret.0 as usize
                ));
                eprintln!(
                    "[gale] EM_SETEDITSTYLE(SES_USECTF) 失败 ret=0x{:X}",
                    ctf_ret.0 as usize
                );
            }
            let old = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, relay_proc as *const () as isize);
            ORIG_PROC.store(old, Ordering::Relaxed);
            RELAY_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
            if input_outcome_message() != 0 {
                // Only this typing control opts in; settings editors and other
                // applications never receive the numeric outcome notification.
                if let Err(error) =
                    SetPropW(hwnd, w!("HuFu.Trainer.InputOutcome.v1"), HANDLE(1 as _))
                {
                    eprintln!("[gale] 精确选重通知不可用: {error}");
                }
            }
            // 宿主形态（2026-11-08 如实修订）：RichEdit 是**可见的 2×2 物理像素**
            // 子窗口，由 place() 每帧贴随绘制光标（rect 去抖，见 place）。
            // 不能用 1×1 region 或隐藏窗口——实测（2026-10-01）会让虎符（TSF
            // TIP，按目标 rect/可见性决定挂载）不挂编辑上下文：imm/ctf 全
            // false、字母直通、无候选无组字。2×2 可见窗口维持 TIP 挂载，
            // 正文由 D2D 自绘层完全覆盖（本控件在底、D2D 帧在上，无残影）。
            Ok(Self {
                hwnd: hwnd.0 as isize,
                composing: false,
                last_text: String::new(),
                last_preedit: String::new(),
                cancellation: CompositionCancellation::default(),
                ctf_ok,
            })
        }
    }

    /// EM_SETEDITSTYLE(SES_USECTF) 校验结果。false = TSF 桥可能未建立，
    /// 上层据此提示"TSF 输入法可能不可用"（虎符等纯 TSF 输入法依赖此链路）。
    pub fn ctf_ok(&self) -> bool {
        self.ctf_ok
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
    /// 隐藏 2×2 输入宿主自带的闪烁光标：正文光标由 DirectWrite 自绘，
    /// 原生宿主只负责 TSF 上下文，不应在光标右下角露一个闪点。
    pub fn hide_caret(&self) {
        unsafe {
            let _ = HideCaret(HWND(self.hwnd as *mut core::ffi::c_void));
        }
    }

    /// 贴到打字光标（客户区物理坐标）。宿主保持 2×2 物理像素贴随绘制光标
    ///（见 create 注释）。2026-11-08 去抖：rect 与上次相同则跳过 SetWindowPos
    ///——主循环每帧调用不再产生系统调用开销。
    pub fn place(&self, x: i32, y: i32, w: i32, h: i32) {
        let rect = (x, y + 2, w, h);
        let skip = PLACE_RECT
            .lock()
            .map(|mut last| {
                if *last == Some(rect) {
                    true
                } else {
                    *last = Some(rect);
                    false
                }
            })
            .unwrap_or(false);
        if skip {
            return;
        }
        unsafe {
            let _ = SetWindowPos(
                HWND(self.hwnd as *mut _),
                None,
                rect.0,
                rect.1,
                rect.2,
                rect.3,
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
        // 20ms 选重/标顶配对（对差分出的每次上屏记一笔；纯 ASCII 内部跳过）
        for ev in &diff {
            if let HostEvent::Commit(s) = ev {
                note_commit(query_tick_ms(), s);
            }
        }
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
        EXACT_SELECTION_SOURCE.store(false, Ordering::Relaxed);
        if let Ok(mut history) = COMMIT_KEY_HISTORY.lock() {
            history.clear();
        }
        if let Ok(mut events) = PHYSICAL_EVENTS.lock() {
            events.clear();
        }
        if let Ok(mut causes) = COMMIT_CAUSES.lock() {
            causes.clear();
        }
    }

    /// 截断到 idx 个字符（点击回退用）
    pub fn truncate(&mut self, idx: usize) {
        let keep: String = self.last_text.chars().take(idx).collect();
        self.set_text(&keep);
    }

    // ── 候选窗/组字窗定位（晴跟打 Pro Win32.cs:264-315 同款去抖）──
    //
    // 背景：输入法默认把组字窗/候选窗贴到控件认为的 caret 位置（2×2 宿主
    // 的"caret"在屏幕上乱漂）。真实 caret 坐标由渲染层（自绘打字行）掌握，
    // 通过 set_anchor_from_caret 喂进来。
    //
    // 去抖规则（晴跟打实证有效）：
    // - 坐标或偏移变化 > 阈值才真正调 ImmSetCompositionWindow(CFS_FORCE_POSITION)
    //   + ImmSetCandidateWindow——IME 收到定位调用会重排候选窗（闪），没变
    //   就调是候选窗抖动根源；
    // - 输入坐标是**主窗口客户区**物理像素；ImmSet*Window 收**屏幕**坐标（内部换算）。

    /// 把渲染层掌握的光标位置设为 IME 组字窗/候选窗锚点（参数单位见各doc）。
    ///
    /// - caret_x / caret_y：光标左上角，主窗口客户区**物理像素**（f32 允许
    ///   亚像素输入，内部四舍五入取整）；
    /// - line_height_px：光标所在行**行高（物理像素）**——候选窗偏移基准；
    /// - dpi：当前窗口 DPI——组字窗与候选窗之间留 2 DIP 间隙，
    ///   gap_px = round(2 × dpi / 96)，下限 1px。
    ///
    /// 布局：组字窗贴 (caret_x, caret_y)；候选窗顶 = caret_y + line_height_px
    /// + gap_px。全部偏移由调用方基于行高/DPI 计算，本层不做任何像素硬编码。
    /// 坐标（或偏移）变化超过 3px 去抖阈值才真正下发 Imm 调用。
    /// 返回 true = 本次下发了定位。
    pub fn set_anchor_from_caret(
        &mut self,
        caret_x: f32,
        caret_y: f32,
        line_height_px: i32,
        dpi: u32,
    ) -> bool {
        let gap_px = ((dpi.max(96) as f32 / 96.0) * 2.0).round().max(1.0) as i32;
        let comp_x = caret_x.round() as i32;
        let comp_y = caret_y.round() as i32;
        let cand_dy = line_height_px.max(0).saturating_add(gap_px);
        unsafe { set_ime_anchor_impl(self.hwnd, comp_x, comp_y, cand_dy) }
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
// 背景：输入法默认把组字窗/候选窗贴到控件认为的 caret 位置。真实 caret
// 坐标由渲染层（自绘打字行）掌握，经 InputHost::set_anchor_from_caret 喂入。
//
// 去抖规则（晴跟打实证有效）：坐标或候选窗偏移变化 > 阈值才真正调
// ImmSetCompositionWindow(CFS_FORCE_POSITION) + ImmSetCandidateWindow ——
// IME 收到定位调用会重排候选窗（闪），没变就调是候选窗抖动根源。
// 输入坐标是主窗口客户区物理像素；ImmSet*Window 收屏幕坐标（内部换算）。

/// 坐标去抖阈值（物理像素）：小于此位移不重设候选窗
const ANCHOR_JITTER_PX: i32 = 3;
/// 上次设置的锚点 (x, y, cand_dy)（None = 从未设过）。cand_dy 进键：
/// 行高/字号变了候选窗偏移必须重发（即使光标没动）。
static LAST_ANCHOR: Mutex<Option<(i32, i32, i32)>> = Mutex::new(None);
/// 上次 place 的 rect（SetWindowPos 去抖；InputHost 单例 → 进程级缓存）
static PLACE_RECT: Mutex<Option<(i32, i32, i32, i32)>> = Mutex::new(None);

/// 锚点下发实现（InputHost::set_anchor_from_caret 的自由函数体）。
/// cand_dy = 候选窗顶相对组字窗锚点 y 的向下偏移（物理像素，调用方算好）。
unsafe fn set_ime_anchor_impl(hwnd: isize, client_x: i32, client_y: i32, cand_dy: i32) -> bool {
    // 1) 去抖判定（坐标与候选偏移任一超阈值即重发）
    {
        let Ok(mut last) = LAST_ANCHOR.lock() else {
            return false;
        };
        if let Some((lx, ly, ld)) = *last {
            if (lx - client_x).abs() <= ANCHOR_JITTER_PX
                && (ly - client_y).abs() <= ANCHOR_JITTER_PX
                && (ld - cand_dy).abs() == 0
            {
                return false;
            }
        }
        *last = Some((client_x, client_y, cand_dy));
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
    // 4) 候选窗在组字锚点正下方 cand_dy（= 行高 + 间隙，调用方按行高/DPI 给出）
    let mut cand = CANDIDATEFORM {
        dwStyle: CFS_FORCE_POSITION,
        ptCurrentPos: POINT {
            x: pt.x,
            y: pt.y + cand_dy,
        },
        rcArea: windows::Win32::Foundation::RECT::default(),
        ..Default::default()
    };
    let ok2 = ImmSetCandidateWindow(himc, &mut cand);
    let _ = windows::Win32::UI::Input::Ime::ImmReleaseContext(host, himc);
    input_log(&format!(
        "IME 锚点 → comp=({}, {}) cand_dy={cand_dy} cand=({}, {}) ok={}/{}",
        pt.x,
        pt.y,
        pt.x,
        pt.y + cand_dy,
        ok1.as_bool() as u8,
        ok2.as_bool() as u8
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
    fn physical_keyboard_sides_and_numpad_enter_remain_distinct() {
        assert_eq!(physical_heatmap_vk(0x10, 0x2a << 16), 0xa0);
        assert_eq!(physical_heatmap_vk(0x10, 0x36 << 16), 0xa1);
        assert_eq!(physical_heatmap_vk(0x11, 1 << 24), 0xa3);
        assert_eq!(physical_heatmap_vk(0x12, 0), 0xa4);
        assert_eq!(physical_heatmap_vk(0x0d, 1 << 24), 0x0e);
        assert_eq!(physical_heatmap_vk(0x14, 0), 0x14);
    }

    #[test]
    fn diff_commit_and_backspace() {
        let mut h = InputHost {
            hwnd: 0,
            composing: false,
            last_text: String::new(),
            last_preedit: String::new(),
            cancellation: CompositionCancellation::default(),
            ctf_ok: true,
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
            ctf_ok: true,
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
            ctf_ok: true,
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
        assert_eq!(Option::<InputEvent>::from(&HostEvent::FocusLost), None);
        assert_eq!(
            Option::<InputEvent>::from(&HostEvent::ResumeKey { vk: 0x5a }),
            None
        );
    }

    #[test]
    fn only_configured_nonstat_selection_keys_resume_during_composition() {
        let keys = SelectionKeys::from_vks(&[0x10, 0x14, 0x41, 0x1b, 0x70]);
        assert!(nonstat_selection_resumes(0xa0, true, &keys));
        assert!(nonstat_selection_resumes(0xa1, true, &keys));
        assert!(nonstat_selection_resumes(0x14, true, &keys));
        assert!(!nonstat_selection_resumes(0x14, false, &keys));
        assert!(!nonstat_selection_resumes(0x11, true, &keys));
        assert!(!nonstat_selection_resumes(0x41, true, &keys));
        assert!(!nonstat_selection_resumes(0x1b, true, &keys));
        assert!(!nonstat_selection_resumes(0x70, true, &keys));
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

    // ── 退格归属裁决（产品 BackspaceRouter；原 13 用例语义回归）──
    // 实现已上移为产品代码（pub struct BackspaceRouter），此处仅回归行为。

    #[test]
    fn backspace_policy_composing_goes_to_ime() {
        let mut r = BackspaceRouter::default();
        r.on_composition_change(true, false); // 组字开始
        assert_eq!(r.on_backspace_key(true), BackspaceClass::ImeDelete);
        assert_eq!(r.on_backspace_key(true), BackspaceClass::ImeDelete);
    }

    #[test]
    fn backspace_policy_after_commit_goes_to_typing() {
        let mut r = BackspaceRouter::default();
        r.on_composition_change(true, false);
        r.on_composition_change(false, true); // 空格提交，组合清空
        assert_eq!(r.on_backspace_key(false), BackspaceClass::Correction);
    }

    #[test]
    fn backspace_policy_depleted_composition_protects_once() {
        let mut r = BackspaceRouter::default();
        r.on_composition_change(true, false);
        r.on_composition_change(false, false); // 退格把组合删空（非提交）
        assert_eq!(r.on_backspace_key(false), BackspaceClass::ImeDelete); // protect 窗口
        assert_eq!(r.on_backspace_key(false), BackspaceClass::Correction); // 之后归跟打
    }

    #[test]
    fn backspace_policy_commit_clear_does_not_protect() {
        // 晴跟打反例回归：提交清空 vs 退格清空必须区分
        let mut r = BackspaceRouter::default();
        r.on_composition_change(true, false);
        r.on_composition_change(false, true); // Commit 清空
        assert_eq!(r.on_backspace_key(false), BackspaceClass::Correction);
        assert_eq!(r.on_backspace_key(false), BackspaceClass::Correction);
    }

    #[test]
    fn backspace_policy_keyup_resets_protect() {
        let mut r = BackspaceRouter::default();
        r.on_composition_change(true, false);
        r.on_composition_change(false, false);
        r.on_backspace_up(); // 物理弹起
        assert_eq!(r.on_backspace_key(false), BackspaceClass::Correction);
    }

    #[test]
    fn backspace_policy_idle_typing() {
        let mut r = BackspaceRouter::default();
        assert_eq!(r.on_backspace_key(false), BackspaceClass::Correction);
    }

    // ── 20ms 选重/标顶配对（注入时间序列验证）──
    #[test]
    fn commit_cause_pairing_within_20ms() {
        let _guard = COMMIT_PAIR_TEST_LOCK.lock().unwrap();
        EXACT_SELECTION_SOURCE.store(false, Ordering::Relaxed);
        set_selection_keys(&[]);
        // 隔离：先清两个静态队列（与其它测试共享进程）
        let _ = drain_key_times(1024);
        let _ = drain_commit_causes();

        // 字母延续 → 顶屏
        note_key(0x4A, 1_000);
        note_commit(1_005, "人");
        // 数字选重 → 选重
        note_key(0x32, 2_000);
        note_commit(2_010, "好");
        // 窗外（50ms）→ Unknown
        note_key(0x41, 3_000);
        note_commit(3_050, "字");
        // 窗内多键：最后一个键说了算（数字后补字母 → 顶屏）
        note_key(0x32, 4_000);
        note_key(0x4A, 4_015);
        note_commit(4_018, "词");
        // 边界：恰好 20ms 差 → 配对（数字 → 选重）
        note_key(0x33, 5_000);
        note_commit(5_020, "边界");
        // 窗内但非字母/数字（如 Esc）→ Unknown
        note_key(0x1B, 6_000);
        note_commit(6_005, "无类");
        // 纯 ASCII 直通（IME 关）→ 不入队
        note_key(0x4A, 7_000);
        note_commit(7_005, "abc");

        let causes = drain_commit_causes();
        assert_eq!(
            causes,
            vec![
                CommitCause::Ding,
                CommitCause::Select,
                CommitCause::Unknown,
                CommitCause::Ding,
                CommitCause::Select,
                CommitCause::Unknown,
            ],
            "配对序列不符: {causes:?}"
        );
        assert!(drain_commit_causes().is_empty(), "drain 后应清空");
        let _ = drain_key_times(1024); // 还原隔离
    }

    static COMMIT_PAIR_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn heatmap_drain_keeps_pending_commit_attribution_and_a_commit_consumes_it() {
        let _guard = COMMIT_PAIR_TEST_LOCK.lock().unwrap();
        EXACT_SELECTION_SOURCE.store(false, Ordering::Relaxed);
        COMMIT_KEY_HISTORY.lock().unwrap().clear();
        let _ = drain_key_times(1024);
        let _ = drain_commit_causes();
        set_selection_keys(&[0xBA]);
        note_key(0xBA, 1_000);
        assert_eq!(drain_key_times(1024), vec![(0xBA, 1_000)]);
        note_commit(1_015, "候选");
        note_commit(1_016, "另一组");
        assert_eq!(
            drain_commit_causes(),
            vec![CommitCause::Select, CommitCause::Unknown]
        );
        set_selection_keys(&[]);
    }

    #[test]
    fn exact_selection_notifications_suppress_legacy_selection_guessing() {
        let _guard = COMMIT_PAIR_TEST_LOCK.lock().unwrap();
        EXACT_SELECTION_SOURCE.store(true, Ordering::Relaxed);
        COMMIT_KEY_HISTORY.lock().unwrap().clear();
        let _ = drain_key_times(1024);
        let _ = drain_commit_causes();
        set_selection_keys(&[0xDE, 0x10]);
        for (index, vk) in [0x33, 0xDE, 0xA0].into_iter().enumerate() {
            let now = 1_000 + index as u64 * 100;
            note_key(vk, now);
            let _ = drain_key_times(1024);
            note_commit(now + 10, "候选");
        }
        assert_eq!(drain_commit_causes(), vec![CommitCause::Unknown; 3]);
        EXACT_SELECTION_SOURCE.store(false, Ordering::Relaxed);
        set_selection_keys(&[]);
    }

    #[test]
    fn scheme_selection_keys_support_oem_modifiers_and_replace_old_scheme() {
        let _guard = COMMIT_PAIR_TEST_LOCK.lock().unwrap();
        EXACT_SELECTION_SOURCE.store(false, Ordering::Relaxed);
        let _ = drain_key_times(1024);
        let _ = drain_commit_causes();
        set_selection_keys(&[0xBA, 0xDE, 0x10, 0x14, 0x51, 999]);
        for (i, vk) in [0xBA, 0xDE, 0xA0, 0xA1, 0x14, 0x51].into_iter().enumerate() {
            let now = 1000 + i as u64 * 100;
            note_key(vk, now);
            note_commit(now + 10, "候选");
        }
        assert_eq!(
            drain_commit_causes(),
            vec![
                CommitCause::Select,
                CommitCause::Select,
                CommitCause::SelectExtra {
                    vk: 0xA0,
                    lag_ms: 10
                },
                CommitCause::SelectExtra {
                    vk: 0xA1,
                    lag_ms: 10
                },
                CommitCause::SelectExtra {
                    vk: 0x14,
                    lag_ms: 10
                },
                CommitCause::Select,
            ]
        );
        // A custom letter selection overrides the generic letter=ding heuristic.
        assert_eq!(classify_commit_cause(0x51), CommitCause::Select);
        assert_eq!(classify_commit_cause(0x41), CommitCause::Ding);
        // Replacing a scheme must remove its old OEM and modifier bindings.
        set_selection_keys(&[0xA1]);
        assert_eq!(classify_commit_cause(0xA0), CommitCause::Unknown);
        assert_eq!(classify_commit_cause(0xA1), CommitCause::Select);
        assert_eq!(classify_commit_cause(0xBA), CommitCause::Unknown);
        assert_eq!(classify_commit_cause(0x14), CommitCause::Unknown);
        assert_eq!(classify_commit_cause(0x32), CommitCause::Select);
        assert_eq!(classify_commit_cause(0x62), CommitCause::Select);
        set_selection_keys(&[0x0E]);
        note_key(0x0E, 3_000);
        note_commit(3_005, "候选");
        assert_eq!(
            drain_commit_causes(),
            vec![CommitCause::Select],
            "numpad Enter already generated a statistical VK_RETURN event"
        );
        // This classification API cannot add modifiers to ordinary speed stats.
        for vk in [0x10, 0xA0, 0xA1, 0x14] {
            assert!(!StatsEngine::is_stat_key(vk));
        }
        set_selection_keys(&[]);
        let _ = drain_key_times(1024);
    }
}
