//! # ime_imm —— IMM32 自建 HIMC 输入后端（晴跟打 Pro 路线，输入层重建主通道）
//!
//! 权威规格：`gale/docs/imm32-recipe.md`（挖自晴跟打 Pro 逆向 + Avalonia 11.3.2
//! 开源对照）。RichEdit50W 中继（input_host）实测：虎符（纯 TSF）无候选框、
//! 虎娘/微软拼音候选可见但上屏失败；用户指示改走晴跟打 Pro 的路线：**纯 IMM32、
//! 零 TSF 代码**，全靠系统 CUAS 把纯 TSF 输入法桥接成 WM_IME_* 消息流。
//!
//! ## 模型（配方 Phase 3/4/5）
//! - enable_imm 三步式：`ImmGetContext` 复用默认 → 取不到才 `ImmCreateContext`
//!   → `ImmAssociateContext`；关联后 `CreateCaret(hwnd, None, 2, 2)`。
//!   **不保存不恢复原 HIMC**（disable = DestroyCaret + reset +
//!   `ImmAssociateContext(hwnd, NULL)`）。
//! - WM_IME 拦截面**只有四条**：SETCONTEXT（只剥 ISC_SHOWUICOMPOSITIONWINDOW，
//!   **候选窗 UI 位保留**——虎符要出候选框；先 DefWindowProc(改后 lp) 再
//!   update_input_method(GetKeyboardLayout(0))，return 0）、STARTCOMPOSITION
//!   （return 0 不走 DefWindowProc）、COMPOSITION（处理完**落 DefWindowProc**）、
//!   ENDCOMPOSITION（return 0）。NOTIFY / IME_CHAR / SELECT / CONTROL /
//!   KEYDOWN / KEYUP / COMPOSITIONFULL / REQUEST 一概不拦截。
//! - 上屏唯一正门：WM_IME_COMPOSITION 的 GCS_RESULTSTR → Commit +
//!   `ignore_wmchar = true`（WM_IME_CHAR 经 DefWindowProc 转成的 WM_CHAR 由
//!   WM_CHAR 规则兜住，不双上屏；WM_KEYUP/WM_SYSKEYUP 复位 false）。
//! - WM_CHAR：`is_composing || ignore_wmchar || c < 32` → 丢；否则 Commit。
//! - END 防御：结束时组字串非空且没有 RESULTSTR → 当上屏发 Commit（防丢字）。
//! - 候选定位 move_ime_window：仅 `GetActiveWindow()==hwnd` 时干活；
//!   先 `SetCaretPos(x2,y2)`（CUAS/TSF 输入法读系统光标——虎符能定位的关键）；
//!   然后晴式钉死：`CFS_FORCE_POSITION` 组字窗 + `ImmSetCandidateWindow`
//!   `CFS_CANDIDATEPOS` dwIndex 0..=3，坐标 = 宿主客户区、1px rcArea。
//!   去抖 = 坐标精确相等才跳过。触发点：STARTCOMPOSITION（缓存坐标重放）/
//!   光标移动（app 调 set_ime_anchor）/提交后（END 重放）。
//! - update_input_method(hkl) 去重（lang_id+hwnd 没变就返回，绝不每帧调），
//!   挂在 WM_ACTIVATE / WM_IME_SETCONTEXT / WM_INPUTLANGCHANGE。
//! - reset()：`ImmNotifyIME(NI_COMPOSITIONSTR, CPS_COMPLETE, 0)` 强制上屏，
//!   前置 ignore_composition=true 丢弃回声；cancel() 用 CPS_CANCEL。
//! - WM_KEYDOWN：vk==0xE5（VK_PROCESSKEY）用扫描码 MapVirtualKey 还原物理键
//!   再入统计；命令键（Esc/F1-F8/F10/Alt+E/Ctrl←→）只在**非组字**时捕获，
//!   组字中除 Esc 外全放行；普通键绝不吞（IMM 键路由依赖 DefWindowProc）。
//! - WM_SYSCOMMAND(SC_KEYMENU) 吞（防 Alt 撕菜单焦点）；WM_SYSCHAR 吞
//!   （Alt+字母字符直通是疾风1代漏字根源，input_host 同款双保险）。
//!
//! ## 与 input_host 的关系
//! 事件流同构：直接复用 `input_host::{HostEvent, CommandKey}` 类型（同一类型，
//! 主程序切换后端零改动）；命令队列/键时戳队列为本模块自有（input_host 的
//! push_cmd 私有，且不动 input_host.rs——其他成员在改）。本后端激活时主程序
//! 应 drain 本模块的 `drain_cmds`/`drain_key_times`。
//!
//! ## 现场开关（env）
//! - `GALE_IMM_ANCHOR_SCREEN=1`：锚点改传屏幕坐标（input_host 旧口径）做 A/B；
//! - `GALE_IMM_CFS_EXCLUDE=1`：候选定位在晴式之上叠加 Avalonia 式 CFS_EXCLUDE
//!   光标矩形（配方 §9.6：叠加效果未证实，默认只用晴式钉死）。

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;

use gale_core::stats::StatsEngine;
use gale_core::GaleResult;

use crate::input_host::{CommandKey, HostEvent};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, HBITMAP};
use windows::Win32::UI::Input::Ime::{
    ImmAssociateContext, ImmCreateContext, ImmDestroyContext, ImmGetCompositionStringW,
    ImmGetContext, ImmNotifyIME, ImmReleaseContext, ImmSetCandidateWindow, ImmSetCompositionWindow,
    CANDIDATEFORM, CFS_CANDIDATEPOS, CFS_EXCLUDE, CFS_FORCE_POSITION, COMPOSITIONFORM, CPS_CANCEL,
    CPS_COMPLETE, GCS_COMPSTR, GCS_RESULTSTR, HIMC, IME_COMPOSITION_STRING,
    ISC_SHOWUICOMPOSITIONWINDOW, NI_COMPOSITIONSTR,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetActiveWindow, GetFocus, GetKeyState, GetKeyboardLayout, MapVirtualKeyW, SetFocus,
    MAPVK_VSC_TO_VK, VK_CONTROL, VK_MENU, VK_PROCESSKEY, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateCaret, DefWindowProcW, DestroyCaret, SetCaretPos, SC_KEYMENU, WM_ACTIVATE, WM_CHAR,
    WM_DESTROY, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_SETCONTEXT,
    WM_IME_STARTCOMPOSITION, WM_INPUTLANGCHANGE, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_SYSCHAR,
    WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow};

// ── 键码（windows 0.58 有绑定的是 VIRTUAL_KEY 类型，这里与 input_host 一致用裸值）──
const VK_BACK: u32 = 0x08; // Backspace
const VK_TAB: u32 = 0x09;
const VK_ENTER: u32 = 0x0D;
const VK_ESC: u32 = 0x1B;
const VK_SPACE: u32 = 0x20;
const VK_LEFT: u32 = 0x25;
const VK_RIGHT: u32 = 0x27;
const VK_INSERT: u32 = 0x2D;
const VK_E: u32 = 0x45;
const VK_F1: u32 = 0x70;

/// F1..F8 → CommandKey 映射表（0x70..=0x77 索引）
const FKEYS: [CommandKey; 8] = [
    CommandKey::F1,
    CommandKey::F2,
    CommandKey::F3,
    CommandKey::F4,
    CommandKey::F5,
    CommandKey::F6,
    CommandKey::F7,
    CommandKey::F8,
];

/// SetCaretPos 用的光标右下角纵深（app 只给锚点时的兜底光标高度）
const CARET_FALLBACK_H: i32 = 18;

// ── 进程级状态 ──
static HOST_HWND: AtomicIsize = AtomicIsize::new(0);
/// 自建 HIMC（0 = 复用了窗口默认上下文；disable 时只销毁自建的）
static OWN_HIMC: AtomicIsize = AtomicIsize::new(0);
static ENABLED: AtomicBool = AtomicBool::new(false);
/// enable 时窗口已聚焦 → 主循环首帧需要一次 SetFocus 重入（补发 SETCONTEXT）
static NEEDS_FOCUS_REENTRY: AtomicBool = AtomicBool::new(false);
/// 二次重入计时起点（tick ms；None=不需要）
static REENTRY_T0: AtomicIsize = AtomicIsize::new(-1);
static COMPOSING: AtomicBool = AtomicBool::new(false);
/// 事件队列（wnd_proc_hook 生产，poll 消费）
static EVENTS: Mutex<Vec<HostEvent>> = Mutex::new(Vec::new());
/// 命令键队列（主循环 drain_cmds 消费）
static CMD_QUEUE: Mutex<Vec<CommandKey>> = Mutex::new(Vec::new());
/// 每键时间戳队列（统计引擎消费；元素 = (物理vk, 毫秒)）
static KEY_TIMES: Mutex<Vec<(u32, u64)>> = Mutex::new(Vec::new());
/// update_input_method 去抖：(lang_id, hwnd)
static LAST_LANG: Mutex<Option<(u32, isize)>> = Mutex::new(None);
/// move_ime_window 去抖：上次光标矩形（客户区，精确相等跳过）
static LAST_RECT: Mutex<Option<(i32, i32, i32, i32)>> = Mutex::new(None);
/// 纯逻辑状态机（组字/预编辑/WM_CHAR 门禁/代理对拼装）
static CORE: Mutex<ImmCore> = Mutex::new(ImmCore::new());

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 统一的核心状态访问：执行后同步 COMPOSING 全局镜像。
fn core_op<R>(f: impl FnOnce(&mut ImmCore) -> R) -> R {
    let mut g = CORE.lock().unwrap_or_else(|e| e.into_inner());
    let r = f(&mut g);
    COMPOSING.store(g.composing, Ordering::Relaxed);
    r
}

fn flush_events(evs: Vec<HostEvent>) {
    if evs.is_empty() {
        return;
    }
    let mut q = EVENTS.lock().unwrap_or_else(|e| e.into_inner());
    if q.len() + evs.len() > 4096 {
        let keep_from = q.len().saturating_sub(2048);
        q.drain(..keep_from);
    }
    q.extend(evs);
}

fn drain_events() -> Vec<HostEvent> {
    match EVENTS.lock() {
        Ok(mut q) => std::mem::take(&mut *q),
        Err(_) => Vec::new(),
    }
}

fn push_cmd(k: CommandKey) {
    let mut q = CMD_QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if q.len() < 64 {
        q.push(k);
    }
}

/// 取走累积的命令键（主循环每帧调用；本后端激活时用它，不用 input_host 的）
pub fn drain_cmds() -> Vec<CommandKey> {
    match CMD_QUEUE.lock() {
        Ok(mut q) => std::mem::take(&mut *q),
        Err(_) => Vec::new(),
    }
}

/// 0xE5（VK_PROCESSKEY）→ 扫描码还原物理键（lp 16..23 位 = 扫描码）。
/// 还原失败保持 0xE5（is_stat_key 区段表含 0xE5，统计不丢）。
fn physical_vk(vk: u32, lp: isize) -> u32 {
    if vk != VK_PROCESSKEY.0 as u32 {
        return vk;
    }
    let scan = ((lp >> 16) & 0xFF) as u32;
    if scan == 0 {
        return vk;
    }
    let mapped = unsafe { MapVirtualKeyW(scan, MAPVK_VSC_TO_VK) };
    if mapped == 0 {
        vk
    } else {
        mapped
    }
}

fn record_key_time(vk: u32) {
    if !StatsEngine::is_stat_key(vk) {
        return;
    }
    let mut kt = KEY_TIMES.lock().unwrap_or_else(|e| e.into_inner());
    kt.push((vk, now_ms()));
    if kt.len() > 512 {
        kt.drain(..256);
    }
}

/// 取走击键时间戳队列（统计引擎消费；元素 = (物理vk, 毫秒)）
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

/// 组字中？（对外只读快照）
pub fn is_composing() -> bool {
    COMPOSING.load(Ordering::Relaxed)
}

/// 后端是否已启用
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// 宿主窗口句柄（0 = 未启用）
pub fn host_hwnd() -> isize {
    HOST_HWND.load(Ordering::Relaxed)
}

/// 当前预编辑串快照（渲染层直读；空串 = 无组字）
pub fn preedit_snapshot() -> String {
    core_op(|c| c.preedit.clone())
}

fn enabled_for(hwnd: isize) -> bool {
    ENABLED.load(Ordering::Relaxed) && HOST_HWND.load(Ordering::Relaxed) == hwnd
}

// ══════════════════════════════════════════════════════════════════════
// 纯逻辑状态机（全部可无窗口单测；语义 = 配方 Phase 3）
// ══════════════════════════════════════════════════════════════════════

/// WM_KEYDOWN 的裁决结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// 放行 DefWindowProc（IMM 键路由依赖它；普通字符也走这里）
    Pass,
    /// 已处理（返回 0）；附带产生的命令键（可选）
    Handled(Option<CommandKey>),
    /// 已处理 + 产生一次 Backspace 输入事件（非组字期退格 = 跟打回改）
    Backspace,
}

/// IMM 输入核心状态
#[derive(Debug, Clone)]
struct ImmCore {
    composing: bool,
    preedit: String,
    /// RESULTSTR 提交后置 true：丢弃 WM_IME_CHAR→DefWindowProc 派生的 WM_CHAR
    /// 重复；WM_KEYUP/WM_SYSKEYUP 复位 false（配方 Q4）。
    ignore_wmchar: bool,
    /// reset() 强制上屏前置 true：丢弃 CPS_COMPLETE 造成的回声组字消息
    ignore_composition: bool,
    /// KILLFOCUS 打断组字时挂起，ENDCOMPOSITION 时清（配方 Phase 3）
    killfocus_deferred: bool,
    /// WM_CHAR 的 UTF-16 高位代理暂存
    pending_high: Option<u16>,
}

impl ImmCore {
    const fn new() -> Self {
        Self {
            composing: false,
            preedit: String::new(),
            ignore_wmchar: false,
            ignore_composition: false,
            killfocus_deferred: false,
            pending_high: None,
        }
    }

    fn reset_state(&mut self) {
        self.composing = false;
        self.preedit.clear();
        self.ignore_wmchar = false;
        self.ignore_composition = false;
        self.killfocus_deferred = false;
        self.pending_high = None;
    }

    /// set_preedit(None/Some(""))：清组字 → Composing(false)；无 Preedit("") 事件
    /// （清空由 Composing(false) 表达，与 input_host 语义一致）
    fn set_preedit(&mut self, s: Option<&str>) -> Vec<HostEvent> {
        let t = s.unwrap_or("");
        let now_composing = !t.is_empty();
        let mut evs = Vec::new();
        if now_composing != self.composing {
            self.composing = now_composing;
            evs.push(HostEvent::Composing(now_composing));
        }
        if t != self.preedit {
            if !t.is_empty() {
                evs.push(HostEvent::Preedit(t.to_string()));
            }
            self.preedit = t.to_string();
        }
        evs
    }

    /// WM_IME_STARTCOMPOSITION：清残留预编辑 + 置组字（幂等）
    fn on_start(&mut self) -> Vec<HostEvent> {
        self.preedit.clear();
        if self.composing {
            Vec::new()
        } else {
            self.composing = true;
            vec![HostEvent::Composing(true)]
        }
    }

    /// WM_IME_COMPOSITION（配方 handle_composition；result/comp 由调用方按
    /// is_composing 前置检查从 HIMC 读出，None = 该位未置或未组字）
    fn on_composition(
        &mut self,
        flags: u32,
        result: Option<&str>,
        comp: Option<&str>,
    ) -> Vec<HostEvent> {
        if self.ignore_composition {
            self.ignore_composition = false;
            return Vec::new();
        }
        let mut evs = Vec::new();
        if flags == 0 {
            evs.extend(self.set_preedit(Some("")));
        }
        if flags & GCS_RESULTSTR.0 != 0 {
            if let Some(s) = result {
                if !s.is_empty() {
                    // ★ 上屏进文本层的唯一正门
                    evs.extend(self.set_preedit(None));
                    evs.push(HostEvent::Commit(s.to_string()));
                    self.ignore_wmchar = true;
                }
            }
        }
        if flags & GCS_COMPSTR.0 != 0 {
            evs.extend(self.set_preedit(comp));
        }
        // GCS_CURSORPOS 不消费（预编辑自绘）
        evs
    }

    /// WM_IME_ENDCOMPOSITION：防御——组字串非空且没走 RESULTSTR → 当上屏发
    /// Commit（防丢字）；然后清组字态、清挂起的 KILLFOCUS。
    fn on_end(&mut self) -> Vec<HostEvent> {
        let mut evs = Vec::new();
        if !self.preedit.is_empty() {
            evs.push(HostEvent::Commit(self.preedit.clone()));
            self.ignore_wmchar = true;
        }
        evs.extend(self.set_preedit(None));
        self.killfocus_deferred = false;
        evs
    }

    /// UTF-16 单元 → 完整字符（0/1/2 个；代理对拼装，孤儿代理按替换符补发）
    fn stash_surrogate(&mut self, w: u16) -> Vec<char> {
        if (0xD800..=0xDBFF).contains(&w) {
            match self.pending_high.replace(w) {
                Some(_) => vec!['\u{FFFD}'],
                None => Vec::new(),
            }
        } else if (0xDC00..=0xDFFF).contains(&w) {
            match self.pending_high.take() {
                Some(h) => {
                    let c = 0x10000u32 + (((h as u32) - 0xD800) << 10) + ((w as u32) - 0xDC00);
                    vec![char::from_u32(c).unwrap_or('\u{FFFD}')]
                }
                None => vec!['\u{FFFD}'],
            }
        } else {
            self.pending_high = None;
            vec![char::from_u32(w as u32).unwrap_or('\u{FFFD}')]
        }
    }

    /// WM_CHAR：is_composing || ignore_wmchar || c<32 → 丢；否则 Commit（配方 Q4）
    fn on_wm_char(&mut self, w: u16) -> Vec<HostEvent> {
        if self.composing || self.ignore_wmchar || (w as u32) < 32 {
            return Vec::new();
        }
        let mut evs = Vec::new();
        for c in self.stash_surrogate(w) {
            evs.push(HostEvent::Commit(c.to_string()));
        }
        evs
    }

    /// 非组字期物理退格 → Backspace 事件
    fn on_backspace(&mut self) -> Vec<HostEvent> {
        vec![HostEvent::Backspace]
    }

    /// WM_KEYDOWN/WM_SYSKEYDOWN 裁决（纯逻辑，不碰 Win32）
    ///
    /// 优先级：Alt 系 → Ctrl 系 → 组字门禁（除 Esc 全放行）→ Shift+Insert 粘贴
    /// → 命令键（非组字）→ 其余放行。
    fn decide_key(&self, vk: u32, ctrl: bool, alt: bool, shift: bool) -> KeyAction {
        if alt && !ctrl {
            // Alt 系全吞防漏字（Alt+字母字符直通是疾风1代漏字根源）
            return KeyAction::Handled(if vk == VK_E {
                Some(CommandKey::AltE)
            } else {
                None
            });
        }
        if ctrl {
            return match vk {
                VK_LEFT => KeyAction::Handled(Some(CommandKey::CtrlLeft)),
                VK_RIGHT => KeyAction::Handled(Some(CommandKey::CtrlRight)),
                _ => KeyAction::Handled(None), // 其余 Ctrl 组合吞（防粘贴/复制/全选）
            };
        }
        if self.composing {
            // 组字期除 Esc 外全放行给输入法（空格选重/退格删拼音/F 键归 IME）
            return match vk {
                VK_ESC => KeyAction::Handled(Some(CommandKey::Escape)),
                _ => KeyAction::Pass,
            };
        }
        if shift && vk == VK_INSERT {
            return KeyAction::Handled(None); // Shift+Insert 粘贴
        }
        match vk {
            VK_ESC => KeyAction::Handled(Some(CommandKey::Escape)),
            vk @ VK_F1..=0x77 => KeyAction::Handled(Some(FKEYS[(vk - VK_F1) as usize])),
            0x79 => KeyAction::Handled(Some(CommandKey::F10)),
            VK_ENTER => KeyAction::Handled(Some(CommandKey::Enter)),
            VK_TAB => KeyAction::Handled(None), // Tab
            VK_SPACE => KeyAction::Handled(Some(CommandKey::Space)),
            VK_BACK => KeyAction::Backspace,
            _ => KeyAction::Pass, // 字母/数字/标点/0xE5 → DefWindowProc → IME
        }
    }
}

/// WM_IME_COMPOSITION flags 位解码（日志与测试共用）
fn flag_parts(flags: u32) -> (bool, bool) {
    (flags & GCS_RESULTSTR.0 != 0, flags & GCS_COMPSTR.0 != 0)
}

/// 锚点矩形是否需要下发（晴去抖口径：左上角精确相等才跳过）
fn rect_should_update(last: Option<(i32, i32, i32, i32)>, x1: i32, y1: i32) -> bool {
    match last {
        Some((lx, ly, _, _)) => lx != x1 || ly != y1,
        None => true,
    }
}

// ══════════════════════════════════════════════════════════════════════
// Win32 胶水（HIMC 生命周期 / 消息钩子 / 锚点 / 强制上屏）
// ══════════════════════════════════════════════════════════════════════

/// 从 HIMC 读组字/结果串（配方两步式：先探字节数再取串）
unsafe fn comp_string_raw(himc: HIMC, flag: IME_COMPOSITION_STRING) -> String {
    let len = ImmGetCompositionStringW(himc, flag, None, 0);
    if len <= 0 {
        return String::new();
    }
    let mut b = vec![0u16; len as usize / 2 + 1];
    let n = ImmGetCompositionStringW(
        himc,
        flag,
        Some(b.as_mut_ptr() as *mut core::ffi::c_void),
        b.len() as u32 * 2,
    );
    if n > 0 {
        String::from_utf16_lossy(&b[..(n as usize) / 2])
    } else {
        String::new()
    }
}

fn env_flag(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => v == "1" || v.eq_ignore_ascii_case("true"),
        Err(_) => false,
    }
}

/// 锚点坐标口径：晴跟打/MSDN = 宿主窗口客户区坐标直传；
/// GALE_IMM_ANCHOR_SCREEN=1 切屏幕坐标（input_host 旧 RichEdit 口径）做现场 A/B。
fn anchor_coords(hwnd: HWND, x: i32, y: i32) -> (i32, i32) {
    if env_flag("GALE_IMM_ANCHOR_SCREEN") {
        let mut pt = POINT { x, y };
        unsafe {
            let _ = ClientToScreen(hwnd, &mut pt);
        }
        (pt.x, pt.y)
    } else {
        (x, y)
    }
}

/// 候选窗/组字窗定位（配方 Phase 4 move_ime_window）。
/// 光标矩形为客户区物理像素；返回 true = 本次下发了定位。
fn move_ime_window(x1: i32, y1: i32, x2: i32, y2: i32) -> bool {
    let hwnd_v = HOST_HWND.load(Ordering::Relaxed);
    if hwnd_v == 0 || !ENABLED.load(Ordering::Relaxed) {
        return false;
    }
    unsafe {
        // 晴/Qt 口径：仅窗口激活时干活（GetActiveWindow 在 KeyboardAndMouse 模块）
        if GetActiveWindow().0 as isize != hwnd_v {
            return false;
        }
    }
    let last = LAST_RECT.lock().ok().and_then(|g| *g);
    if !rect_should_update(last, x1, y1) {
        return false;
    }
    if let Ok(mut r) = LAST_RECT.lock() {
        *r = Some((x1, y1, x2, y2));
    }
    unsafe {
        let hwnd = HWND(hwnd_v as *mut core::ffi::c_void);
        // CUAS/TSF 输入法读系统光标——虎符能定位的关键
        let _ = SetCaretPos(x2, y2);
        let himc = ImmGetContext(hwnd);
        if himc.is_invalid() {
            return false;
        }
        let (px1, py1) = anchor_coords(hwnd, x1, y1);
        let (px2, py2) = anchor_coords(hwnd, x2, y2);
        let mut did = false;
        // 方案 A（Avalonia 式，可叠加，默认关）：CFS_EXCLUDE 光标矩形，
        // 纯 IMM32 老输入法也认的排除区通道
        if env_flag("GALE_IMM_CFS_EXCLUDE") {
            let excl = CANDIDATEFORM {
                dwIndex: 0,
                dwStyle: CFS_EXCLUDE,
                ptCurrentPos: POINT { x: px1, y: py1 },
                rcArea: RECT {
                    left: px1,
                    top: py1,
                    right: px2,
                    bottom: py2 + 1,
                },
            };
            did |= ImmSetCandidateWindow(himc, &excl).as_bool();
        }
        // 方案 B（晴式钉死，默认）：CFS_FORCE_POSITION 组字窗 +
        // CFS_CANDIDATEPOS 候选窗 dwIndex 0..=3，1px rcArea
        let area = RECT {
            left: px1,
            top: py1,
            right: px1 + 1,
            bottom: py1 + 1,
        };
        let cf = COMPOSITIONFORM {
            dwStyle: CFS_FORCE_POSITION,
            ptCurrentPos: POINT { x: px1, y: py1 },
            rcArea: area,
        };
        did |= ImmSetCompositionWindow(himc, &cf).as_bool();
        for i in 0..=3u32 {
            let cand = CANDIDATEFORM {
                dwIndex: i,
                dwStyle: CFS_CANDIDATEPOS,
                ptCurrentPos: POINT { x: px1, y: py1 },
                rcArea: area,
            };
            did |= ImmSetCandidateWindow(himc, &cand).as_bool();
        }
        let _ = ImmReleaseContext(hwnd, himc);
        crate::input_host::diag_log(&format!(
            "IMM move_ime_window rect=({x1},{y1},{x2},{y2})→pin({px1},{py1}) did={did}"
        ));
        did
    }
}

/// 用缓存坐标重放定位（绕过去抖；STARTCOMPOSITION/提交后触发，晴 refresh 同款）
fn refresh_position() -> bool {
    let last = LAST_RECT.lock().ok().and_then(|g| *g);
    match last {
        Some((x1, y1, x2, y2)) => {
            // 重放：临时清去抖记忆再调 move_ime_window（仅激活时生效）
            if let Ok(mut r) = LAST_RECT.lock() {
                *r = None;
            }
            move_ime_window(x1, y1, x2, y2)
        }
        None => false,
    }
}

/// 把渲染层光标位置（宿主客户区坐标）设为 IME 组字窗/候选窗锚点。
/// 单点形式：右下角按 CARET_FALLBACK_H 兜底；矩形形式见 set_ime_anchor_rect。
pub fn set_ime_anchor(client_x: i32, client_y: i32) -> bool {
    set_ime_anchor_rect(client_x, client_y, client_x, client_y + CARET_FALLBACK_H)
}

/// 完整光标矩形形式（客户区物理像素；x2,y2 = 光标右下角，喂 SetCaretPos）
pub fn set_ime_anchor_rect(x1: i32, y1: i32, x2: i32, y2: i32) -> bool {
    move_ime_window(x1, y1, x2, y2)
}

/// 重置锚点记忆（段切换/窗口移动后调；下一次 set_ime_anchor 必定下发）
pub fn reset_ime_anchor() {
    if let Ok(mut r) = LAST_RECT.lock() {
        *r = None;
    }
}

/// 输入法上下文随语言/窗口迁移（配方：lang_id+hwnd 去重，绝不每帧调）。
/// 挂在 WM_ACTIVATE / WM_IME_SETCONTEXT / WM_INPUTLANGCHANGE。
pub fn update_input_method(hkl: isize) {
    let hwnd_v = HOST_HWND.load(Ordering::Relaxed);
    if hwnd_v == 0 || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let lang = (hkl as usize & 0xFFFF) as u32;
    let last = LAST_LANG.lock().ok().and_then(|g| *g);
    if last == Some((lang, hwnd_v)) {
        return; // 语言与窗口都没变：绝不重复操作 IME 状态
    }
    if let Ok(mut l) = LAST_LANG.lock() {
        *l = Some((lang, hwnd_v));
    }
    // 上下文失效（迁移中被换掉/解除）→ 重建关联
    unsafe {
        let h = HWND(hwnd_v as *mut core::ffi::c_void);
        let cur = ImmGetContext(h);
        if cur.is_invalid() {
            let himc = ImmCreateContext();
            if !himc.is_invalid() {
                let _ = ImmAssociateContext(h, himc);
                OWN_HIMC.store(himc.0 as isize, Ordering::Relaxed);
                let _ = CreateCaret(h, HBITMAP(std::ptr::null_mut()), 2, 2);
                crate::input_host::diag_log(&format!(
                    "IMM update_input_method：lang={lang:#x} 上下文失效 → 重建 {:#x}",
                    himc.0 as isize
                ));
            }
        } else {
            let _ = ImmReleaseContext(h, cur);
        }
    }
    crate::input_host::diag_log(&format!(
        "IMM update_input_method：lang={lang:#x} hwnd={hwnd_v:#x}"
    ));
}

/// 启用（配方 EnableImm 三步式）：ImmGetContext 复用默认 → 取不到才
/// ImmCreateContext → ImmAssociateContext；关联后 CreateCaret(hwnd, None, 2, 2)。
/// **不保存原 HIMC**（disable 一律关联 NULL）。
/// ★ 2026-10-01 血泪回滚：虎符（TSF TIP）挂在【窗口默认 HIMC】的 CUAS
///   桥上。显式 ImmCreateContext 换新上下文会【丢掉】系统已为默认 HIMC
///   建好的 TSF 桥 → 虎符收键不组字（吞键）。真实键盘铁证：
///   rt3-072813 会话（own_himc=0x0 复用默认）用户虎符整段跟打正常；
///   改显式 HIMC 后所有会话虎符吞键。微拼走 IMM 兼容层不受影响，
///   所以 074817 显式 HIMC 下微拼仍能打出[要]——曾误导诊断方向。
pub fn enable_imm(hwnd: isize) -> GaleResult<()> {
    if hwnd == 0 {
        return gale_core::gale_err!("enable_imm：hwnd=0");
    }
    unsafe {
        if ENABLED.load(Ordering::Relaxed) {
            if HOST_HWND.load(Ordering::Relaxed) == hwnd {
                return Ok(()); // 去重复用
            }
            disable_imm(); // 换宿主窗口：先卸老
        }
        let h = HWND(hwnd as *mut core::ffi::c_void);
        // 复用默认 HIMC（虎符等 TSF TIP 的桥就建在它上面）；取不到才自建
        let mut himc = ImmGetContext(h);
        let mut created: isize = 0;
        if himc.is_invalid() {
            himc = ImmCreateContext();
            if himc.is_invalid() {
                return gale_core::gale_err!("ImmCreateContext 失败（IMM32 通道不可用）");
            }
            let _ = ImmAssociateContext(h, himc);
            created = himc.0 as isize;
        } else {
            let _ = ImmReleaseContext(h, himc); // 复用默认：取了就还（不持有句柄）
        }
        let _ = CreateCaret(h, HBITMAP(std::ptr::null_mut()), 2, 2);
        OWN_HIMC.store(created, Ordering::Relaxed);
        HOST_HWND.store(hwnd, Ordering::Relaxed);
        ENABLED.store(true, Ordering::Relaxed);
        core_op(|c| c.reset_state());
        if let Ok(mut q) = EVENTS.lock() {
            q.clear();
        }
        if let Ok(mut l) = LAST_LANG.lock() {
            *l = None; // 强制首个 update_input_method 生效
        }
        reset_ime_anchor();
        // ★ 2026-10-06 回归修复：enable 时窗口已聚焦（show_and_focus 之后
        //   挂接的启动序）→ msctf 的 SETCONTEXT/CUAS 绑定发生在挂 HIMC
        //   之前，桥没建到本窗。置 NEEDS_FOCUS_REENTRY，由主循环首帧调
        //   focus_reentry_once() 复刻"用户点击"（SetFocus 重入）补一次
        //   SETCONTEXT→CUAS 绑桥。（此前 setter 被连根删掉，重入机制变
        //   死代码 → 键被虎符吃掉但组字永不来 = "打不了字"。）
        if GetForegroundWindow() == h || GetFocus() == h {
            NEEDS_FOCUS_REENTRY.store(true, Ordering::Relaxed);
        }
        crate::input_host::diag_log(&format!(
            "IMM enable：hwnd={hwnd:#x} own_himc={created:#x}（0=复用默认）已挂接"
        ));
        Ok(())
    }
}

/// 主循环消费的焦点重入：完整复刻"用户点击窗口"的激活效果。
/// 执行点：首帧 + 800ms 后各一次（早期重入会被启动期焦点抖动覆盖；
/// 二次重入确保 msctf 的焦点跟踪与 CUAS 桥都拿到稳定的 fSet=true）。
/// 幂等；仅 enable 时已聚焦的场景触发。
pub fn focus_reentry_once() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let hwnd_v = HOST_HWND.load(Ordering::Relaxed);
    if hwnd_v == 0 {
        return;
    }
    // 第一跳：enable 时已聚焦 → 首帧执行
    if NEEDS_FOCUS_REENTRY.swap(false, Ordering::Relaxed) {
        unsafe {
            let h = HWND(hwnd_v as *mut core::ffi::c_void);
            if GetForegroundWindow() == h || GetFocus() == h {
                let _ = SetFocus(HWND(std::ptr::null_mut()));
                let _ = SetFocus(h);
                let _ = SetForegroundWindow(h);
                REENTRY_T0.store(
                    crate::input_host::query_tick_ms() as isize,
                    Ordering::Relaxed,
                );
                crate::input_host::diag_log("IMM focus_reentry①：SetFocus 重入 + 前台自确认");
            } else {
                crate::input_host::diag_log("IMM focus_reentry：窗口未聚焦，跳过");
            }
        }
        return;
    }
    // 第二跳：800ms 后再重入一次（对冲启动期 msctf 焦点跟踪的晚绑定）
    let t0 = REENTRY_T0.load(Ordering::Relaxed);
    if t0 >= 0 {
        let now = crate::input_host::query_tick_ms() as isize;
        if now.saturating_sub(t0) >= 800 && now.saturating_sub(t0) < 3000 {
            REENTRY_T0.store(-1, Ordering::Relaxed);
            unsafe {
                let h = HWND(hwnd_v as *mut core::ffi::c_void);
                if GetForegroundWindow() == h || GetFocus() == h {
                    let _ = SetFocus(HWND(std::ptr::null_mut()));
                    let _ = SetFocus(h);
                    crate::input_host::diag_log("IMM focus_reentry②：800ms 二次重入完成");
                }
            }
        }
    }
}

/// 禁用（配方 DisableImm）：DestroyCaret + 状态复位 +
/// ImmAssociateContext(hwnd, NULL)。自建上下文销毁；**不恢复原 HIMC**。
pub fn disable_imm() {
    let hwnd_v = HOST_HWND.swap(0, Ordering::Relaxed);
    ENABLED.store(false, Ordering::Relaxed);
    COMPOSING.store(false, Ordering::Relaxed);
    let own = OWN_HIMC.swap(0, Ordering::Relaxed);
    core_op(|c| c.reset_state());
    if let Ok(mut q) = EVENTS.lock() {
        q.clear();
    }
    if let Ok(mut l) = LAST_LANG.lock() {
        *l = None;
    }
    reset_ime_anchor();
    if hwnd_v == 0 {
        return;
    }
    unsafe {
        let _ = DestroyCaret();
        let h = HWND(hwnd_v as *mut core::ffi::c_void);
        let _ = ImmAssociateContext(h, HIMC(std::ptr::null_mut()));
        if own != 0 {
            let _ = ImmDestroyContext(HIMC(own as *mut core::ffi::c_void));
        }
        crate::input_host::diag_log(&format!(
            "IMM disable：hwnd={hwnd_v:#x} DestroyCaret + 关联 NULL（own={own:#x} 已销毁）"
        ));
    }
}

/// 强制上屏（模式切换/清段/失焦用，配方 Reset）：CPS_COMPLETE 让 IME 立即提交
/// 当前组字；前置 ignore_composition 丢弃回声（app 层不再要这段文本）。
pub fn reset() {
    let hwnd_v = HOST_HWND.load(Ordering::Relaxed);
    if hwnd_v == 0 {
        return;
    }
    unsafe {
        let h = HWND(hwnd_v as *mut core::ffi::c_void);
        let himc = ImmGetContext(h);
        if !himc.is_invalid() {
            core_op(|c| {
                if c.composing {
                    c.ignore_composition = true;
                }
                c.ignore_wmchar = true;
                c.composing = false;
                c.preedit.clear();
            });
            let _ = ImmNotifyIME(himc, NI_COMPOSITIONSTR, CPS_COMPLETE, 0);
            let _ = ImmReleaseContext(h, himc);
        } else {
            core_op(|c| {
                c.composing = false;
                c.preedit.clear();
            });
        }
        crate::input_host::diag_log("IMM reset：CPS_COMPLETE 强制上屏（回声丢弃）");
    }
}

/// 取消组字（丢弃，Hyper 同款）：CPS_CANCEL；先清本地组字态防 END 防御性上屏。
pub fn cancel() {
    let hwnd_v = HOST_HWND.load(Ordering::Relaxed);
    if hwnd_v == 0 {
        return;
    }
    unsafe {
        let h = HWND(hwnd_v as *mut core::ffi::c_void);
        let himc = ImmGetContext(h);
        if !himc.is_invalid() {
            core_op(|c| {
                c.composing = false;
                c.preedit.clear();
                c.ignore_composition = true;
            });
            let _ = ImmNotifyIME(himc, NI_COMPOSITIONSTR, CPS_CANCEL, 0);
            let _ = ImmReleaseContext(h, himc);
        }
        crate::input_host::diag_log("IMM cancel：CPS_CANCEL 丢弃组字");
    }
}

/// 轮询：取走事件队列（消息驱动模型；配方无每帧 HIMC 轮询——绝不每帧碰 IME 状态）
pub fn poll() -> Vec<HostEvent> {
    drain_events()
}

/// 窗口过程钩子：调用方窗口过程在 DefWindowProc **之前**调用。
/// 返回 Some(LRESULT) = 已处理（直接返回该值）；None = 放行 DefWindowProc。
/// 未启用（或消息不属于宿主窗口）时一律 None。
pub fn wnd_proc_hook(hwnd: isize, msg: u32, wp: usize, lp: isize) -> Option<LRESULT> {
    if !enabled_for(hwnd) {
        return None;
    }
    unsafe {
        ime_hook_proc(
            HWND(hwnd as *mut core::ffi::c_void),
            msg,
            WPARAM(wp),
            LPARAM(lp),
        )
    }
}

unsafe fn ime_hook_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    use crate::input_host::diag_log;
    match msg {
        WM_IME_SETCONTEXT => {
            // 只剥默认组字窗 UI 位（组字串自绘）；候选窗 UI 位保留——虎符候选
            // 必须可见。先 DefWindowProc(改后 lp)，再 update_input_method，return 0。
            let stripped = LPARAM((lp.0 as u32 & !ISC_SHOWUICOMPOSITIONWINDOW) as isize);
            let _ = DefWindowProcW(hwnd, msg, wp, stripped);
            diag_log(&format!(
                "IMM SETCONTEXT fSet={} lp=0x{:X}→0x{:X}",
                wp.0 != 0,
                lp.0 as u32,
                stripped.0 as u32
            ));
            update_input_method(GetKeyboardLayout(0).0 as isize);
            Some(LRESULT(0))
        }
        WM_IME_STARTCOMPOSITION => {
            diag_log("IMM STARTCOMPOSITION");
            let evs = core_op(|c| c.on_start());
            flush_events(evs);
            // 用缓存坐标重放候选定位（配方触发点）
            refresh_position();
            Some(LRESULT(0)) // 不走 DefWindowProc
        }
        WM_IME_COMPOSITION => {
            let flags = lp.0 as u32;
            let (has_result, has_comp) = flag_parts(flags);
            let composing_now = core_op(|c| c.composing);
            // 前置 is_composing 检查（未组字直接空）；两步式取串
            let (result, comp) = if composing_now {
                let himc = ImmGetContext(hwnd);
                if himc.is_invalid() {
                    (None, None)
                } else {
                    let r = if has_result {
                        Some(comp_string_raw(himc, GCS_RESULTSTR))
                    } else {
                        None
                    };
                    let c = if has_comp {
                        Some(comp_string_raw(himc, GCS_COMPSTR))
                    } else {
                        None
                    };
                    let _ = ImmReleaseContext(hwnd, himc);
                    (r, c)
                }
            } else {
                (None, None)
            };
            let evs = core_op(|c| c.on_composition(flags, result.as_deref(), comp.as_deref()));
            diag_log(&format!(
                "IMM COMPOSITION flags=0x{flags:X} RESULT=[{}] COMP=[{}] → {} 事件",
                result.as_deref().unwrap_or(""),
                comp.as_deref().unwrap_or(""),
                evs.len()
            ));
            flush_events(evs);
            None // ★ 处理完落 DefWindowProc（配方拦截面）
        }
        WM_IME_ENDCOMPOSITION => {
            diag_log("IMM ENDCOMPOSITION");
            let evs = core_op(|c| c.on_end());
            diag_log(&format!("IMM ENDCOMPOSITION → {} 事件", evs.len()));
            flush_events(evs);
            // 提交后重定位候选窗（配方触发点）
            refresh_position();
            Some(LRESULT(0)) // 不走 DefWindowProc
        }
        WM_INPUTLANGCHANGE => {
            diag_log("IMM INPUTLANGCHANGE（切换输入法）");
            update_input_method(lp.0);
            None // 落 DefWindowProc 传给子窗
        }
        WM_ACTIVATE => {
            update_input_method(GetKeyboardLayout(0).0 as isize);
            None
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wp.0 as u32;
            let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
            let alt = GetKeyState(VK_MENU.0 as i32) < 0;
            let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
            // 每键记账（含组字期）：0xE5 用扫描码还原物理键再入统计
            let phys = physical_vk(vk, lp.0);
            record_key_time(phys);
            // 统计键 → HostEvent::Key（击键/码长/键准的数据源；richedit 后端
            // 由 route_key 同位记账。模态期 main 侧门禁读掉即弃，安全）
            if StatsEngine::is_stat_key(phys) && !ctrl && !alt {
                flush_events(vec![HostEvent::Key { vk: phys }]);
            }
            let (action, composing_now) =
                core_op(|c| (c.decide_key(vk, ctrl, alt, shift), c.composing));
            // 焦点核对：组字消息（STARTCOMPOSITION 等）路由到 GetFocus() 窗口；
            // 若焦点不在本窗，IME 组字进了别的窗口 → 键穿过但永远不上屏
            let focus_now = GetFocus();
            let focus_stray = focus_now.0 as isize != 0 && focus_now.0 as isize != hwnd.0 as isize;
            diag_log(&format!(
                "IMM KEYDOWN vk=0x{vk:X} phys=0x{phys:X} ctrl={ctrl} alt={alt} shift={shift} composing={composing_now} focus_stray={focus_stray} → {action:?}"
            ));
            match action {
                KeyAction::Pass => None, // 绝不吞：DefWindowProc → user32 → IME 键路由
                KeyAction::Handled(cmd) => {
                    if let Some(k) = cmd {
                        push_cmd(k);
                    }
                    Some(LRESULT(0))
                }
                KeyAction::Backspace => {
                    let evs = core_op(|c| c.on_backspace());
                    flush_events(evs);
                    Some(LRESULT(0))
                }
            }
        }
        WM_KEYUP | WM_SYSKEYUP => {
            // ignore_wmchar 复位（RESULTSTR 的 WM_CHAR 抑制窗口关闭）
            core_op(|c| c.ignore_wmchar = false);
            None
        }
        WM_CHAR => {
            let w = wp.0 as u16;
            let dropped = core_op(|c| {
                let evs = c.on_wm_char(w);
                (evs, c.composing || c.ignore_wmchar)
            });
            diag_log(&format!(
                "IMM CHAR 0x{w:X} dropped={} → {} 事件",
                dropped.1,
                dropped.0.len()
            ));
            flush_events(dropped.0);
            Some(LRESULT(0))
        }
        WM_KILLFOCUS => {
            // 组字被打断：挂起到 ENDCOMPOSITION 再了断（配方 Phase 3）
            core_op(|c| {
                if c.composing {
                    c.killfocus_deferred = true;
                }
            });
            None
        }
        WM_SYSCOMMAND => {
            if wp.0 == SC_KEYMENU as usize {
                diag_log("IMM SYSCOMMAND SC_KEYMENU 吞（防 Alt 撕菜单焦点）");
                Some(LRESULT(0))
            } else {
                None
            }
        }
        WM_SYSCHAR => {
            // Alt+字母字符直通是疾风1代漏字根源，吞（input_host 同款双保险）
            diag_log(&format!("IMM SYSCHAR 0x{:X} 吞", wp.0 as u32));
            Some(LRESULT(0))
        }
        WM_DESTROY => {
            diag_log("IMM DESTROY → disable");
            disable_imm();
            None
        }
        // WM_IME_NOTIFY / WM_IME_CHAR / WM_IME_SELECT / WM_IME_CONTROL /
        // WM_IME_KEYDOWN / WM_IME_KEYUP / WM_IME_COMPOSITIONFULL / WM_IME_REQUEST
        // 一概不拦截（配方拦截面：只处理 SETCONTEXT/START/COMPOSITION/END）
        _ => None,
    }
}

// ══════════════════════════════════════════════════════════════════════
// 宿主句柄（RAII 便捷封装；单例语义）
// ══════════════════════════════════════════════════════════════════════

/// IMM32 输入宿主（enable_imm 的 RAII 封装；Drop 时 disable_imm）
pub struct Ime32Host {
    pub hwnd: isize,
}

unsafe impl Send for Ime32Host {}

impl Ime32Host {
    /// 创建宿主：三步式挂接 HIMC + 系统光标（见 enable_imm）
    pub fn attach(hwnd: isize) -> GaleResult<Self> {
        enable_imm(hwnd)?;
        Ok(Self { hwnd })
    }

    /// 轮询事件队列（同模块级 poll）
    pub fn poll(&mut self) -> Vec<HostEvent> {
        poll()
    }

    /// 候选窗/组字窗锚点（客户区坐标；含去抖）
    pub fn set_ime_anchor(&self, client_x: i32, client_y: i32) -> bool {
        set_ime_anchor(client_x, client_y)
    }

    /// 重置锚点记忆（段切换/窗口移动后调）
    pub fn reset_anchor(&self) {
        reset_ime_anchor();
    }

    /// 主动卸载（等价 Drop）
    pub fn detach(&mut self) {
        disable_imm();
    }
}

impl Drop for Ime32Host {
    fn drop(&mut self) {
        disable_imm();
    }
}

// ══════════════════════════════════════════════════════════════════════
// 测试（全部不依赖真窗口）
// ══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    /// HostEvent 无 PartialEq，用字符串签名断言序列
    fn sig(evs: &[HostEvent]) -> Vec<String> {
        evs.iter()
            .map(|e| match e {
                HostEvent::Commit(s) => format!("C:{s}"),
                HostEvent::Backspace => "BS".into(),
                HostEvent::Composing(b) => format!("Comp:{b}"),
                HostEvent::Preedit(s) => format!("P:{s}"),
                HostEvent::Cancelled => "Cancel".into(),
                HostEvent::Key { vk } => format!("K:{vk}"),
            })
            .collect()
    }

    #[test]
    fn flag_decode() {
        // crate 常量值锚定（配方 §6 Phase 7 对照）
        assert_eq!(GCS_COMPSTR.0, 0x0008);
        assert_eq!(GCS_RESULTSTR.0, 0x0800);
        assert_eq!(windows::Win32::UI::Input::Ime::GCS_CURSORPOS.0, 0x0080);
        assert_eq!(CFS_FORCE_POSITION, 0x20);
        assert_eq!(CFS_CANDIDATEPOS, 0x40);
        assert_eq!(CFS_EXCLUDE, 0x80);
        assert_eq!(NI_COMPOSITIONSTR.0, 21); // 0x15
        assert_eq!(CPS_COMPLETE.0, 1);
        assert_eq!(CPS_CANCEL.0, 4);
        assert_eq!(ISC_SHOWUICOMPOSITIONWINDOW, 0x8000_0000);
        assert_eq!(VK_PROCESSKEY.0 as u32, 0xE5);
        assert_eq!(SC_KEYMENU, 61696); // 0xF100
                                       // 位解码
        assert_eq!(flag_parts(GCS_RESULTSTR.0), (true, false));
        assert_eq!(flag_parts(GCS_COMPSTR.0), (false, true));
        assert_eq!(flag_parts(GCS_RESULTSTR.0 | GCS_COMPSTR.0), (true, true));
        assert_eq!(flag_parts(0), (false, false));
    }

    #[test]
    fn decide_key_idle_command_matrix() {
        let c = ImmCore::new();
        assert_eq!(
            c.decide_key(VK_ESC, false, false, false),
            KeyAction::Handled(Some(CommandKey::Escape))
        );
        assert_eq!(
            c.decide_key(0x70, false, false, false),
            KeyAction::Handled(Some(CommandKey::F1))
        );
        assert_eq!(
            c.decide_key(0x71, false, false, false),
            KeyAction::Handled(Some(CommandKey::F2))
        );
        assert_eq!(
            c.decide_key(0x72, false, false, false),
            KeyAction::Handled(Some(CommandKey::F3))
        );
        assert_eq!(
            c.decide_key(0x73, false, false, false),
            KeyAction::Handled(Some(CommandKey::F4))
        );
        assert_eq!(
            c.decide_key(0x74, false, false, false),
            KeyAction::Handled(Some(CommandKey::F5))
        );
        assert_eq!(
            c.decide_key(0x75, false, false, false),
            KeyAction::Handled(Some(CommandKey::F6))
        );
        assert_eq!(
            c.decide_key(0x76, false, false, false),
            KeyAction::Handled(Some(CommandKey::F7))
        );
        assert_eq!(
            c.decide_key(0x77, false, false, false),
            KeyAction::Handled(Some(CommandKey::F8))
        );
        assert_eq!(
            c.decide_key(0x79, false, false, false),
            KeyAction::Handled(Some(CommandKey::F10))
        );
        assert_eq!(
            c.decide_key(VK_ENTER, false, false, false),
            KeyAction::Handled(Some(CommandKey::Enter))
        );
        assert_eq!(
            c.decide_key(VK_TAB, false, false, false),
            KeyAction::Handled(None)
        );
        assert_eq!(
            c.decide_key(VK_SPACE, false, false, false),
            KeyAction::Handled(Some(CommandKey::Space))
        );
        assert_eq!(
            c.decide_key(VK_BACK, false, false, false),
            KeyAction::Backspace
        );
        // 字母/数字/标点/0xE5 → 放行（IMM 键路由）
        assert_eq!(c.decide_key(0x41, false, false, false), KeyAction::Pass);
        assert_eq!(c.decide_key(0x30, false, false, false), KeyAction::Pass);
        assert_eq!(c.decide_key(0xE5, false, false, false), KeyAction::Pass);
    }

    #[test]
    fn decide_key_composing_gate() {
        let mut c = ImmCore::new();
        c.set_preedit(Some("ni"));
        assert!(c.composing);
        // 组字期除 Esc 外全放行（空格选重/退格删拼音/F 键/回车归输入法）
        assert_eq!(
            c.decide_key(VK_ESC, false, false, false),
            KeyAction::Handled(Some(CommandKey::Escape))
        );
        assert_eq!(c.decide_key(VK_SPACE, false, false, false), KeyAction::Pass);
        assert_eq!(c.decide_key(VK_BACK, false, false, false), KeyAction::Pass);
        assert_eq!(c.decide_key(0x41, false, false, false), KeyAction::Pass);
        assert_eq!(c.decide_key(0x70, false, false, false), KeyAction::Pass);
        assert_eq!(c.decide_key(VK_ENTER, false, false, false), KeyAction::Pass);
        assert_eq!(c.decide_key(VK_INSERT, false, false, true), KeyAction::Pass);
    }

    #[test]
    fn decide_key_modifiers() {
        let c = ImmCore::new();
        assert_eq!(
            c.decide_key(VK_E, false, true, false),
            KeyAction::Handled(Some(CommandKey::AltE))
        );
        assert_eq!(
            c.decide_key(0x46, false, true, false),
            KeyAction::Handled(None)
        ); // Alt+F 吞
        assert_eq!(
            c.decide_key(VK_LEFT, true, false, false),
            KeyAction::Handled(Some(CommandKey::CtrlLeft))
        );
        assert_eq!(
            c.decide_key(VK_RIGHT, true, false, false),
            KeyAction::Handled(Some(CommandKey::CtrlRight))
        );
        assert_eq!(
            c.decide_key(0x56, true, false, false),
            KeyAction::Handled(None)
        ); // Ctrl+V 吞
           // Shift+Insert 粘贴（非组字）吞
        assert_eq!(
            c.decide_key(VK_INSERT, false, false, true),
            KeyAction::Handled(None)
        );
    }

    #[test]
    fn composition_result_commits_and_sets_wmchar_guard() {
        let mut c = ImmCore::new();
        c.set_preedit(Some("ni")); // 组字中
                                   // RESULTSTR：清组字 → Commit → ignore_wmchar=true（WM_CHAR 重复被兜住）
        let evs = c.on_composition(GCS_RESULTSTR.0, Some("你"), None);
        assert_eq!(sig(&evs), vec!["Comp:false", "C:你"]);
        assert!(c.ignore_wmchar);
        assert_eq!(c.preedit, "");
        assert!(!c.composing);
        // RESULTSTR 后派生的 WM_CHAR 重复被丢弃
        assert!(c.on_wm_char('你' as u16).is_empty());
        // WM_KEYUP 复位后恢复直通
        c.ignore_wmchar = false;
        let evs = c.on_wm_char('a' as u16);
        assert_eq!(sig(&evs), vec!["C:a"]);
    }

    #[test]
    fn composition_result_then_new_comp() {
        let mut c = ImmCore::new();
        c.set_preedit(Some("ni"));
        // 同一条消息：RESULTSTR 提交 + 新组字开始
        let evs = c.on_composition(GCS_RESULTSTR.0 | GCS_COMPSTR.0, Some("你"), Some("h"));
        assert_eq!(sig(&evs), vec!["Comp:false", "C:你", "Comp:true", "P:h"]);
        assert!(c.composing);
        assert_eq!(c.preedit, "h");
    }

    #[test]
    fn composition_comp_only_and_flags_zero() {
        let mut c = ImmCore::new();
        let evs = c.on_composition(GCS_COMPSTR.0, None, Some("ni"));
        assert_eq!(sig(&evs), vec!["Comp:true", "P:ni"]);
        // flags==0 → 清组字
        let evs = c.on_composition(0, None, None);
        assert_eq!(sig(&evs), vec!["Comp:false"]);
        // 空结果不产生事件
        let evs = c.on_composition(GCS_RESULTSTR.0, Some(""), None);
        assert!(evs.is_empty());
    }

    #[test]
    fn ignore_composition_drops_echo_once_and_resets() {
        let mut c = ImmCore::new();
        c.ignore_composition = true; // reset() 置位
        assert!(c
            .on_composition(GCS_RESULTSTR.0, Some("你"), None)
            .is_empty());
        assert!(!c.ignore_composition); // 消费一次即复位
                                        // reset() 已置 composing=false；回声丢弃后新组字正常走完整迁移
        let evs = c.on_composition(GCS_COMPSTR.0, None, Some("hao"));
        assert_eq!(sig(&evs), vec!["Comp:true", "P:hao"]);
    }

    #[test]
    fn end_defensive_commit_residual_preedit() {
        // 防御：IME 没发 RESULTSTR 只发 END → 残留组字当上屏（防丢字）
        let mut c = ImmCore::new();
        c.set_preedit(Some("nihao"));
        let evs = c.on_end();
        assert_eq!(sig(&evs), vec!["C:nihao", "Comp:false"]);
        assert!(c.ignore_wmchar); // 防 WM_CHAR 重复
    }

    #[test]
    fn end_after_normal_commit_no_double() {
        let mut c = ImmCore::new();
        c.set_preedit(Some("ni"));
        c.on_composition(GCS_RESULTSTR.0, Some("你"), None); // 正常提交清了 preedit
        let evs = c.on_end();
        // 不再有防御性 Commit（残留组字已被 RESULTSTR 正常收割）
        assert!(!evs.iter().any(|e| matches!(e, HostEvent::Commit(_))));
    }

    #[test]
    fn wm_char_gate_matrix() {
        let mut c = ImmCore::new();
        // 控制字符丢
        for ctrl_c in [8u16, 9, 13, 27, 0] {
            assert!(c.on_wm_char(ctrl_c).is_empty(), "c={ctrl_c}");
        }
        // 正常字符上屏
        assert_eq!(sig(&c.on_wm_char('a' as u16)), vec!["C:a"]);
        // 组字期丢（被 IME 吃掉的键不应产生直通）
        c.set_preedit(Some("ni"));
        assert!(c.on_wm_char('a' as u16).is_empty());
        c.set_preedit(None);
        // ignore_wmchar 期丢
        c.ignore_wmchar = true;
        assert!(c.on_wm_char('a' as u16).is_empty());
        c.ignore_wmchar = false;
        assert_eq!(sig(&c.on_wm_char('b' as u16)), vec!["C:b"]);
    }

    #[test]
    fn wm_char_surrogate_pair() {
        let mut c = ImmCore::new();
        assert!(c.on_wm_char(0xD83D).is_empty()); // 高位暂存
        let evs = c.on_wm_char(0xDE00); // 低位拼装 = 😀
        assert_eq!(sig(&evs), vec!["C:😀"]);
    }

    #[test]
    fn wm_char_orphan_surrogates() {
        let mut c = ImmCore::new();
        assert!(c.on_wm_char(0xD83D).is_empty());
        // 高位后跟常规字符：孤儿丢弃，常规字符正常上屏
        let evs = c.on_wm_char('b' as u16);
        assert_eq!(sig(&evs), vec!["C:b"]);
        // 连续两个高位：第一个按替换符补发
        let mut c2 = ImmCore::new();
        assert!(c2.on_wm_char(0xD83D).is_empty());
        let evs = c2.on_wm_char(0xD83C);
        assert_eq!(sig(&evs), vec!["C:\u{FFFD}"]);
    }

    #[test]
    fn start_composition_idempotent() {
        let mut c = ImmCore::new();
        assert_eq!(sig(&c.on_start()), vec!["Comp:true"]);
        assert!(c.on_start().is_empty()); // 幂等
                                          // START 清残留预编辑但不发清空事件（马上会有新 COMPSTR）
        let mut c2 = ImmCore::new();
        c2.preedit = "残留".into();
        assert_eq!(sig(&c2.on_start()), vec!["Comp:true"]);
        assert_eq!(c2.preedit, "");
    }

    #[test]
    fn backspace_event() {
        let mut c = ImmCore::new();
        assert_eq!(sig(&c.on_backspace()), vec!["BS"]);
    }

    #[test]
    fn anchor_debounce_exact_equality() {
        assert!(rect_should_update(None, 5, 5));
        assert!(!rect_should_update(Some((5, 5, 5, 23)), 5, 5)); // 左上精确相等跳过
        assert!(rect_should_update(Some((5, 5, 5, 23)), 6, 5));
        assert!(rect_should_update(Some((5, 5, 5, 23)), 5, 6));
    }

    #[test]
    fn cmd_queue_roundtrip_and_bounded() {
        let _ = drain_cmds(); // 隔离其他测试残留
        push_cmd(CommandKey::F2);
        push_cmd(CommandKey::AltE);
        let cmds = drain_cmds();
        assert_eq!(cmds, vec![CommandKey::F2, CommandKey::AltE]);
        assert!(drain_cmds().is_empty());
        for _ in 0..80 {
            push_cmd(CommandKey::Enter);
        }
        assert!(drain_cmds().len() <= 64);
    }

    #[test]
    fn key_times_records_stat_keys_only() {
        let _ = drain_key_times(1024); // 隔离
        record_key_time(0x41); // 字母 = 统计键
        record_key_time(0xE5); // 虎符重映射兜底 = 统计键
        record_key_time(0x70); // F1 = 非统计键
        let kt = drain_key_times(1024);
        let vks: Vec<u32> = kt.iter().map(|(v, _)| *v).collect();
        assert_eq!(vks, vec![0x41, 0xE5]);
    }

    #[test]
    fn physical_vk_recovery_from_processkey() {
        // 非 0xE5 原样返回
        assert_eq!(physical_vk(0x41, 0), 0x41);
        assert_eq!(physical_vk(VK_SPACE, 0), VK_SPACE);
        // 0xE5 + 'A' 扫描码（0x1E，lp 16..23 位）→ 物理键 0x41
        let lp = (0x1Eisize) << 16;
        assert_eq!(physical_vk(0xE5, lp), 0x41);
        // 0xE5 + 无扫描码 → 保持 0xE5（统计区段表兜底）
        assert_eq!(physical_vk(0xE5, 0), 0xE5);
    }

    #[test]
    fn disabled_hook_is_noop() {
        // 未启用时钩子一律放行（不碰消息流）
        assert!(wnd_proc_hook(0x1234, WM_KEYDOWN, 0x41, 0).is_none());
        assert!(wnd_proc_hook(0x1234, WM_IME_COMPOSITION, 0, GCS_RESULTSTR.0 as isize).is_none());
        assert!(wnd_proc_hook(0x1234, WM_IME_SETCONTEXT, 1, 0).is_none());
        assert!(!is_enabled());
        assert_eq!(host_hwnd(), 0);
        // disable/update 在未启用时为无害 no-op
        disable_imm();
        reset();
        cancel();
        update_input_method(0x0804_0000);
    }
}
