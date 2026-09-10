//! 候选窗 v2：D3D11 + DirectComposition + Direct2D + DWM 真实材质。
//!
//! - 窗口：WS_POPUP + WS_EX_NOREDIRECTIONBITMAP（DComp 直通，逐像素 alpha）
//! - 材质（皮肤 material.kind）→ SetWindowCompositionAttribute accent：
//!   solid=不透明 / translucent=半透明渐变 / frosted=Acrylic 磨砂 /
//!   glass=HostBackdrop 玻璃（Win11 22H2+）
//! - 文本：DirectWrite；圆角/高亮：D2D FillRoundedRectangle
//! - 初始化失败时上层回退 v1（GDI 分层窗口）

use serde_json::Value;
use windows::core::Interface;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::PCWSTR;

// ── DWM accent（未公开 API，Win10 1803+ 全系统 IME 通用做法）──

/// cand2 窗口过程：DefWindowProc 转发 + 鼠标消息诊断日志。
/// 【排查中】用户实测「正常应用里点击候选框导致应用卡死」——本过程
/// 记录点击/移动消息到达与时刻，卡死复现后由日志定位卡点。
extern "system" fn cand2_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    const DIAG: once_bool::Diag = once_bool::Diag::new();
    if DIAG.enabled() {
        let tag = match msg {
            0x200 => "move",
            0x201 => "ldown",
            0x202 => "lup",
            0x204 => "rdown",
            0x205 => "rup",
            0x84 => "nchittest",
            0x21 => "mactivate",
            0xA1 => "ncldown",
            0xA4 => "ncrdown",
            0x20 => "setcursor",
            0xA0 => "activate",
            _ => "",
        };
        if !tag.is_empty() {
            crate::tsf::diag_note(&format!(
                "cw2 mouse {tag} t={:?}",
                std::time::SystemTime::now()
            ));
        }
    }
    // 【NOREDIRECTIONBITMAP+DComp 窗的 hit-test 修正】DWM 按 visual
    // 内容 alpha 判定命中：悬停时代码在候选字上命中，但阴影/圆角/
    // 透明边缘按下会被判穿透——按钮消息根本不进 wndproc（QQ 实测
    // setcursor/move 到达、ldown/rdown 从未出现）。显式返回
    // HTCLIENT 强制整窗客户区命中。
    if msg == 0x84 {
        // WM_NCHITTEST → HTCLIENT
        return LRESULT(1);
    }
    // 【鼠标交互 2026-09-10 用户拍板】左键按住拖动=拖到哪里固定在哪
    // 里（跨组段保持）；右键=解除固定，候选窗回光标处恢复跟随。
    // （旧「拖动+右键锁定/再右键解除」双路径已废——分叉时序曾实测
    // 拖 A 锁定→拖 B→打字回 A；锁标小窗一并移除。）
    // 冻结事故教训（已修）：本窗口过程的按钮消息自持自理、绝不经
    // DefWindowProc 的激活路径；窗口操作仅发生在用户主动交互的
    // 消息路径（非 TSF 焦点回调），无死锁面。
    match msg {
        0x201 => {
            // WM_LBUTTONDOWN：记录按下起点（死区内不拖）并捕获鼠标
            crate::tsf::trace("cw2: ldown 到达");
            unsafe {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                *CAND_DOWN.lock().unwrap_or_else(|e| e.into_inner()) = Some((pt.x, pt.y));
                *CAND_DRAG.lock().unwrap_or_else(|e| e.into_inner()) = None;
                let _ = SetCapture(hwnd);
            }
            return LRESULT(0);
        }
        0x200 => {
            // WM_MOUSEMOVE：按下且累计位移超过 4px 死区才激活拖拽；
            // 拖拽中随鼠标移动窗口（clamp 虚拟屏幕内）
            unsafe {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                // 死区判定：未激活时距离起点 >4px 才升级为拖拽
                if CAND_DRAG
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_none()
                {
                    let down = *CAND_DOWN.lock().unwrap_or_else(|e| e.into_inner());
                    match down {
                        Some((sx, sy)) => {
                            if (pt.x - sx).abs() <= 4 && (pt.y - sy).abs() <= 4 {
                                return LRESULT(0);
                            }
                        }
                        None => return LRESULT(0),
                    }
                    // 越过死区：此刻激活拖拽（偏移=当前鼠标−窗口原点）
                    let mut wr = RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    };
                    let _ = GetWindowRect(hwnd, &mut wr);
                    *CAND_DRAG.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some((pt.x - wr.left, pt.y - wr.top));
                    // 【pin 双保险】本按下周期内真正拖过——置一次性
                    // 标记，0x202 兜底锁定（见 WM_LBUTTONUP）。
                    *CAND_DRAGGED_ONCE.lock().unwrap_or_else(|e| e.into_inner()) = true;
                    crate::tsf::trace("cw2: drag 激活（越过死区）");
                }
                let drag = *CAND_DRAG.lock().unwrap_or_else(|e| e.into_inner());
                if let Some((dx, dy)) = drag {
                    let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
                    let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
                    let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
                    let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
                    let mut wr = RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    };
                    let _ = GetWindowRect(hwnd, &mut wr);
                    let w = (wr.right - wr.left).max(1);
                    let h = (wr.bottom - wr.top).max(1);
                    let x = (pt.x - dx).clamp(vx, (vx + vw - w).max(vx));
                    let y = (pt.y - dy).clamp(vy, (vy + vh - h).max(vy));
                    let _ = SetWindowPos(
                        hwnd,
                        HWND(std::ptr::null_mut()),
                        x,
                        y,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                    shadowwin_follow(hwnd);
                }
            }
            return LRESULT(0);
        }
        0x202 => {
            // WM_LBUTTONUP：结束拖拽；只有真拖过（越过死区）松手位置才
            // 交给 show() 作 sticky——纯单击（抖动在死区内）什么都不动。
            // 【拖动即固定 2026-09-10 用户拍板】松手同时把固定位更新为
            // 松手处：拖到哪里就固定在哪里，无需再右键锁定（旧两套
            // 分叉语义——未固定态松手只 sticky 本组段+固定态松手回写
            // ——叠加右键时序曾实测「拖 A 锁定→拖 B→打字回 A」）。
            unsafe {
                // 【顺序关键 2026-09-10】必须先清 CAND_DOWN 再
                // ReleaseCapture：后者会【同步】派发 0x215，若此刻
                // DOWN 仍非空会被「拖拽进行中」判定 SetCapture 夺回
                // ——捕获永远不释放，全屏其他窗口点不了（用户实测
                // VSCode/QQ 拖一次后别处全锁死）。
                *CAND_DOWN.lock().unwrap_or_else(|e| e.into_inner()) = None;
                let _ = ReleaseCapture();
                // 【pin 双保险 2026-09-10 用户拍板】「拖动的时候不要求
                // 光标存活，只要求位置能跟着锁定」：本按下周期内真正
                // 拖过（越过死区即置 CAND_DRAGGED_ONCE）就以窗口当前
                // 位置写固定位——即使拖拽态中途被 0x215 之外的路径
                // 意外清掉，松手照样锁位置。
                let dragged = CAND_DRAG
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
                    .is_some();
                let dragged_once = std::mem::take(
                    &mut *CAND_DRAGGED_ONCE.lock().unwrap_or_else(|e| e.into_inner()),
                );
                if dragged || dragged_once {
                    crate::tsf::trace(if dragged {
                        "cw2: lup 拖动结束"
                    } else {
                        "cw2: lup 拖动结束（DRAG 已失，按曾拖过锁定）"
                    });
                    let mut wr = RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    };
                    let _ = GetWindowRect(hwnd, &mut wr);
                    *CAND_DROP_AT.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some((wr.left, wr.top));
                    *CAND_PINNED.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some((wr.left, wr.top));
                    crate::tsf::diag_note(&format!("cw2 pin 拖动即固定 ({},{})", wr.left, wr.top));
                } else {
                    crate::tsf::trace("cw2: lup 未拖动（死区内）");
                }
            }
            *CAND_DRAG.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return LRESULT(0);
        }
        0x204 => {
            // WM_RBUTTONDOWN：【右键=解锁 2026-09-10 用户拍板】清除固定
            // 位与拖拽钉住残留——候选窗回到光标处恢复跟随。未固定时
            // 右键无操作（不再有「右键固定」路径）。
            crate::tsf::trace("cw2: rdown（右键解锁）");
            let mut pinned = CAND_PINNED.lock().unwrap_or_else(|e| e.into_inner());
            if pinned.is_some() {
                *pinned = None;
                drop(pinned);
                *CAND_DROP_AT.lock().unwrap_or_else(|e| e.into_inner()) = None;
                *CAND_UNSTICK.lock().unwrap_or_else(|e| e.into_inner()) = true;
                crate::tsf::diag_note("cw2 pin 右键解除（回跟随光标）");
            }
            return LRESULT(0);
        }
        0x20A => {
            // 【滚轮缩放候选框】WM_MOUSEWHEEL（Win10+ 默认「悬停时滚动
            // 非活动窗口」，光标在框上即到达）：上滚放大、下滚缩小，
            // 每格 ±1pt（10~36 clamp）。字号经 server 写回当前皮肤
            // layout.font_point（持久化），随后本地皮肤副本同步新字号
            // 并用缓存的上帧渲染参数立即重绘——不等 2.5s 皮肤缓存过期、
            // 不依赖键事件触发 update_ui。
            // 【2026-09-06 序号跟随修复】此前只 patch font_point——
            // 序号字级（label_font_point）仍是旧值，且把皮肤重拉时限
            // 推后，滚轮时序号原地不动、要等下一组段才跳变。现在
            // server 响应带回新 label_font_point，一并写进本地副本，
            // 主字与序号同一帧同步缩放。
            let delta: i32 = if ((wparam.0 >> 16) as i16) > 0 { 1 } else { -1 };
            // 【锁外渲染 2026-09-11】旧实现持 shared 锁整帧重绘（show
            // 5-15ms）——滚轮期间按键路径抢不到锁（打字卡手）。锁内
            // 只取渲染参数，take 窗口对象后放锁渲染，再放回。
            if let Some(r) = crate::ipc::call(&serde_json::json!({
                "op": "skin_font_delta", "delta": delta
            })) {
                let new_pt = r.get("font_point").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                let new_lb = r.get("label_font_point").and_then(|x| x.as_f64());
                if new_pt > 0.0 {
                    if let Some(gsh) = crate::tsf::G_SHARED.get() {
                        let shared = gsh.0.clone();
                        let (mut cand2, mut last, skin, caret) = {
                            let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                            let patched = if let Some(l) = g.skin.pointer_mut("/skin/layout") {
                                l["font_point"] = serde_json::json!(new_pt);
                                if let Some(lb) = new_lb {
                                    l["label_font_point"] = serde_json::json!(lb);
                                }
                                true
                            } else if let Some(l) = g.skin.get_mut("layout") {
                                l["font_point"] = serde_json::json!(new_pt);
                                if let Some(lb) = new_lb {
                                    l["label_font_point"] = serde_json::json!(lb);
                                }
                                true
                            } else {
                                false
                            };
                            if patched {
                                // 副本已同步新字号：刷新缓存时限，暂不重拉
                                g.skin_stale = false;
                                g.skin_loaded_at = std::time::Instant::now();
                            }
                            (g.cand2.take(), g.last_show.take(), g.skin.clone(), g.caret)
                        };
                        if let (Some(c), Some((cands, raw, sel))) = (cand2.as_mut(), last.as_mut())
                        {
                            c.show(cands, raw, &skin, caret.as_ref(), *sel);
                        }
                        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                        g.last_show = last;
                        // 放回：期间若有线程重建了 cand2（cand2_dead 自愈路径），
                        // 保留新的，旧窗隐藏丢弃
                        match (g.cand2.take(), cand2) {
                            (None, mine) => g.cand2 = mine,
                            (Some(newer), Some(mut mine)) => {
                                mine.hide();
                                g.cand2 = Some(newer);
                            }
                            (Some(newer), None) => g.cand2 = Some(newer),
                            (None, None) => {}
                        }
                    }
                }
            }
            return LRESULT(0);
        }
        0x215 => {
            // 【CAPTURECHANGED 2026-09-11】拖拽中捕获被系统夺走（弹窗/
            // 切窗/权限 UAC）时旧实现不清拖拽态——之后无按键的
            // MOUSEMOVE 也会继续拖着候选窗走（真按钮已松）。清之。
            // 【拖拽中夺回 2026-09-10】实测 VSCode/Chromium 在按下候选
            // 窗杀组段后会异步 SetCapture 抢走鼠标（trace：drag 激活
            // →1.1s→ lup「未拖动」=DRAG 已被本分支清掉，拖动白做、
            // pin 不写）。按钮仍按住（CAND_DOWN 有值）= 用户拖拽
            // 进行中：立即 SetCapture 夺回，拖拽不断；真松手由 0x202
            // 收尾。真弹窗抢鼠标时用户必松手，同样由 0x202 收尾，
            // 不会死循环（夺回后无按下态的 0x215 不再触发本路径）。
            unsafe {
                let down = *CAND_DOWN.lock().unwrap_or_else(|e| e.into_inner());
                if down.is_some() {
                    let _ = SetCapture(hwnd);
                    crate::tsf::trace("cw2: 捕获被夺→夺回（拖拽进行中）");
                } else {
                    *CAND_DRAG.lock().unwrap_or_else(|e| e.into_inner()) = None;
                }
            }
            return LRESULT(0);
        }
        0x205 | 0x207 | 0x208 => return LRESULT(0), // 右/中键抬起吞
        // 【rect 只增不减 2026-09-11】内容余量区（窗口 rect 大于内容
        // 的透明部分）鼠标穿透——不挡住底下去往宿主应用的点击。
        0x0084 => {
            // WM_NCHITTEST：lparam 屏幕坐标
            if let Some(gsh) = crate::tsf::G_SHARED.get() {
                let shared = gsh.0.clone();
                let g = shared.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(c) = g.cand2.as_ref() {
                    let (cw, ch) = c.content_size.get();
                    let mut pt = POINT { x: 0, y: 0 };
                    let mut ok = false;
                    unsafe {
                        let mut p = POINT {
                            x: (lparam.0 as u32 & 0xFFFF) as i16 as i32,
                            y: ((lparam.0 as u32 >> 16) & 0xFFFF) as i16 as i32,
                        };
                        if ScreenToClient(hwnd, &mut p).as_bool() {
                            pt = p;
                            ok = true;
                        }
                    }
                    if ok && (pt.x >= cw || pt.y >= ch || pt.x < 0 || pt.y < 0) {
                        return LRESULT(-1); // HTTRANSPARENT
                    }
                }
            }
        }
        // 异步隐藏（hide() PostMessage 而来——焦点回调里同步 ShowWindow
        // 会与 MSCTF/Chromium 焦点临界区死锁）
        crate::candwin2::WM_APP_HIDE_CAND => {
            // 【退场动画退役 2026-09-11】用户判「调不好」：淡出与半透明
            // 面板天然相克（渐隐帧压在新上屏文字上=变黑/重叠，连打时
            // 收放循环=一闪一闪）。收窗一律即时隐藏——干净利落。
            //（入场长大/尺寸过渡/跟光标滑动保留；入场淡入仍由皮肤
            // fade_ms>0 显式开启才有。）
            unsafe {
                if let Some(gsh) = crate::tsf::G_SHARED.get() {
                    let shared = gsh.0.clone();
                    let mut cand2 = {
                        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                        g.cand2.take()
                    };
                    if let Some(c) = cand2.as_mut() {
                        c.last_hide_at = Some(std::time::Instant::now());
                        // 【尺寸动效】隐藏即整窗退役——动效与余量基准
                        // 归零，下个会话按首个内容重定
                        c.size_anim = None;
                        c.chrome_override.set(None);
                        c.scale_in.set(false);
                        c.pos_anim = None;
                        c.fade = None;
                        c.live_size.set((0, 0));
                    }
                    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                    match (g.cand2.take(), cand2) {
                        (None, mine) => g.cand2 = mine,
                        (Some(newer), Some(mut mine)) => {
                            mine.hide();
                            g.cand2 = Some(newer);
                        }
                        (Some(newer), None) => g.cand2 = Some(newer),
                        (None, None) => {}
                    }
                }
                let _ = KillTimer(hwnd, FADE_TIMER_ID);
                let _ = KillTimer(hwnd, EXPAND_TIMER_ID);
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            return LRESULT(0);
        }
        // 【动效 2026-09-11】WM_TIMER：渐隐渐显 tick + 注释展开延时。
        // 渲染/状态变更走滚轮缩放同款 take/put-back（锁外渲染，不抢
        // 按键路径的锁）。
        0x113 => {
            let id = wparam.0 as usize;
            if id == FADE_TIMER_ID {
                unsafe { fade_tick_shared(hwnd) };
                return LRESULT(0);
            }
            if id == EXPAND_TIMER_ID {
                unsafe { expand_tick_shared(hwnd) };
                return LRESULT(0);
            }
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

mod once_bool {
    /// 进程内一次性诊断开关：写标志文件才启用（默认零开销）。
    pub struct Diag;
    impl Diag {
        pub const fn new() -> Diag {
            Diag
        }
        pub fn enabled(&self) -> bool {
            static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *V.get_or_init(|| {
                std::path::Path::new(r"C:\ProgramData\HuFu\diag\cand2-mouse").exists()
            })
        }
    }
}

const ACCENT_DISABLED: u32 = 0;
const WCA_ACCENT_POLICY: u32 = 19;

#[repr(C)]
struct AccentPolicy {
    accent_state: u32,
    accent_flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

#[repr(C)]
struct WinCompAttrData {
    attribute: u32,
    data: *mut core::ffi::c_void,
    size_of_data: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn SetWindowCompositionAttribute(hwnd: HWND, data: *mut WinCompAttrData) -> BOOL;
}

fn apply_accent(hwnd: HWND, state: u32, tint: [u8; 4]) {
    // gradient_color 布局 0xAABBGGRR
    let abgr = (u32::from(tint[3]) << 24)
        | (u32::from(tint[2]) << 16)
        | (u32::from(tint[1]) << 8)
        | u32::from(tint[0]);
    let mut policy = AccentPolicy {
        accent_state: state,
        accent_flags: 0,
        gradient_color: abgr,
        animation_id: 0,
    };
    let mut data = WinCompAttrData {
        attribute: WCA_ACCENT_POLICY,
        data: &mut policy as *mut AccentPolicy as *mut core::ffi::c_void,
        size_of_data: std::mem::size_of::<AccentPolicy>() as u32,
    };
    unsafe {
        let _ = SetWindowCompositionAttribute(hwnd, &mut data);
    }
}

// ── 皮肤取色 ──

fn parse_hex(s: &str) -> Option<[u8; 4]> {
    let s = s.trim_start_matches('#');
    // 【panic 防护 2026-09-09】非 ASCII 颜色串（手改皮肤含中文，len 恰
    // 6/8）按 &str 字节切片会切进 UTF-8 字符中间 panic——渲染线程
    // panic=宿主进程崩。先校验 ASCII 再按字节取。
    if !s.is_ascii() || (s.len() != 8 && s.len() != 6) {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?, if s.len() == 8 { b(6)? } else { 0xFF }])
}

fn color_f(v: &Value, key: &str, default: &str) -> D2D1_COLOR_F {
    let hex = v
        .pointer(&format!("/skin/colors/{key}"))
        .and_then(|x| x.as_str())
        .or_else(|| {
            v.get("colors")
                .and_then(|c| c.get(key))
                .and_then(|x| x.as_str())
        })
        .unwrap_or(default);
    let c = parse_hex(hex).unwrap_or([32, 32, 34, 230]);
    D2D1_COLOR_F {
        r: c[0] as f32 / 255.0,
        g: c[1] as f32 / 255.0,
        b: c[2] as f32 / 255.0,
        a: c[3] as f32 / 255.0,
    }
}

fn layout_f(v: &Value, key: &str, default: f32) -> f32 {
    v.pointer(&format!("/skin/layout/{key}"))
        .or_else(|| v.get("layout").and_then(|l| l.get(key)))
        .and_then(|x| x.as_f64())
        .unwrap_or(default as f64) as f32
}

fn material_kind(v: &Value) -> String {
    v.pointer("/skin/material/kind")
        .or_else(|| v.get("material").and_then(|m| m.get("kind")))
        .and_then(|x| x.as_str())
        .unwrap_or("solid")
        .to_string()
}

// ── 窗口本体 ──

pub struct CandidateWindowV2 {
    pub(crate) hwnd: HWND,
    ctx: Option<ID2D1DeviceContext>,
    swapchain: Option<IDXGISwapChain1>,
    dcomp: Option<IDCompositionDevice>,
    target: Option<IDCompositionTarget>,
    visual: Option<IDCompositionVisual>,
    dwrite: Option<IDWriteFactory>,
    dxgi: Option<IDXGIDevice>,
    size: (i32, i32),
    /// 粘性定位：最近一次有效锚点坐标。锚点偶发丢失（GetTextExt 在
    /// 异步编辑会话未就绪时失败）时沿用上次位置——绝不能瞬移屏幕中央，
    /// 那正是候选框「在光标周围乱跳」的病根。
    sticky_pos: Option<(i32, i32)>,
    /// 【拖拽钉住 2026-09-08】拖拽松手设的 sticky 是「组段级钉住」：
    /// 本组段内窗口留在松手处（忽略 caret 锚），hide（收窗/失焦/
    /// 上屏断段）时解除——下一组段恢复跟随。旧行为 sticky 只作
    /// 防抖基准，松手后下一次重绘即弹回 caret（用户实测「拖动后
    /// 回到原位」）。永久固定仍走右键 pin。
    sticky_drag: bool,
    /// 上次 show 的编码长度：判断「正向打字」还是「退格/新组段」。
    /// 正向打字时光标只应右移/不动——据此过滤应用返回的旧布局回退值。
    last_raw_len: usize,
    /// 测试回读：show() 后从 D2D 目标位图取整帧 BGRA（渲染层真值，不经 DWM）
    pub(crate) readback: bool,
    pub(crate) last_pixels: Option<Vec<u8>>,
    /// 诊断：最近一次渲染的光学垂直位移（readback 模式填充）
    pub(crate) last_dy: Option<f32>,
    /// 诊断：readback 像素尺寸
    pub(crate) last_size: (u32, u32),
    /// 连续被 DWM cloaked（显示中但不可见）的帧数；打包宿主里
    /// DComp 直通窗可能被整体隐身 → 达阈值切换 v1 传统混合窗
    pub(crate) cloaked_streak: u32,
    /// 【每帧开销缓存】字体格式三件套按 (face,pt,label_pt) 复用——
    /// CreateTextFormat 含系统字体匹配（百 µs 级），打字每键一帧
    /// ×3 个格式是渲染路径大头；皮肤/字号不变时零创建。
    pub(crate) tf_cache: Option<(
        (String, f32, f32),
        (
            Option<IDWriteTextFormat>,
            Option<IDWriteTextFormat>,
            Option<IDWriteTextFormat>,
        ),
    )>,
    /// 【每帧开销缓存】光学垂直补偿 dy 按 (face,pt) 复用——probe
    /// 每帧两次 CreateTextLayout+GetOverhangMetrics 可省。
    pub(crate) dy_cache: Option<((String, f32), f32)>,
    /// 【每帧开销缓存】阴影 command list+effect 按 (w,h,radius,oy,argb)
    /// 复用——宽度不变的连续帧（同长度候选）零重建；变宽时重建。
    pub(crate) shadow_cache: Option<(
        (u32, u32, u32, u32, (i32, i32), u32, u32),
        (ID2D1CommandList, ID2D1Effect),
    )>,
    /// 【毛玻璃退役 2026-09-11】glass_raw/glass_cache（抓屏+自绘模糊）
    /// 已随毛玻璃整链删除。accent 幂等键残留清理用。
    pub(crate) acrylic_last: std::cell::Cell<u64>,
    /// RGN 幂等键（尺寸+半径打包；MAX=未设）
    pub(crate) rgn_last: std::cell::Cell<u64>,
    /// 【动效 2026-09-11·虎爪对标】渐隐渐显时长 ms（皮肤 layout.fade_ms，
    /// 代码默认 120，0=关）。动画纯表现层：每 tick 仅改 DComp visual
    /// Opacity + Commit（µs 级，不重绘）；内容首帧即全量渲染，绝不延迟
    /// 候选刷新；连打静默期（250ms）内直接全显防频闪。
    pub(crate) fade_ms: u32,
    /// 渐变进行态：Some((渐显?, 起点))；None=静止
    pub(crate) fade: Option<(bool, std::time::Instant)>,
    /// 【动效 tick 重渲染标记】fade_tick 驱动的 show() 复渲染——不得
    /// 触发「内容更新打断渐隐」的取消规则（那是用户键入路径专用）。
    pub(crate) internal_rerender: bool,
    /// 上次真正隐藏时刻（静默期判定：show 距 hide <250ms 全显不动画）
    pub(crate) last_hide_at: Option<std::time::Instant>,
    /// 【尺寸动效 2026-09-11】(from, to, t0)：可见更新时窗口 rect 从当前
    /// 插值平滑逼近目标（内容按目标布局即刻渲染，缓冲只增不减、余量
    /// 渐进揭示/收拢）。None=无进行中的尺寸动效。
    pub(crate) size_anim: Option<((i32, i32), (i32, i32), std::time::Instant)>,
    /// 【高亮锚定入场 2026-09-11】首出长大从高亮候选「长出来」：动画盒
    /// 中心锚定高亮胶囊中心（内容平移 −bx/−by、窗口跟盒滑动），其余
    /// 内容向四周展开——而非从窗口左上角出现。逻辑内容坐标。
    pub(crate) hl_center: std::cell::Cell<Option<(f32, f32)>>,
    /// 入场动效进行中（盒心锚定模式；完成/隐藏即清）
    pub(crate) scale_in: std::cell::Cell<bool>,
    /// 尺寸动效时长 ms（皮肤 layout.size_ms，默认 120，0=瞬跳）——注释
    /// 展开/收起、候选数变化等一切宽高变化都平滑过渡；连打重定目标
    /// （从当前插值位置追赶新目标，不跳变）。
    pub(crate) size_ms: u32,
    /// 【位置滑动 2026-09-11】整句自动上屏后剩余内容跳到新光标、候选
    /// 跟着走——位置过渡（从→到 屏幕坐标 + t0），窗口位置丝滑滑过去
    /// 而非一跳一跳。None=瞬移。
    pub(crate) pos_anim: Option<((i32, i32), (i32, i32), std::time::Instant)>,
    /// 位置动效时长 ms（皮肤 layout.pos_ms，默认 120，0=瞬跳）
    pub(crate) pos_ms: u32,
    /// 【动效开关 2026-09-11】设置页全局：false=一切动效瞬跳
    pub(crate) anim_on: std::cell::Cell<bool>,
    /// 退场淡出默认时长（150ms × 全局速度）
    pub(crate) fade_ms_eff: u32,
    /// 最近一次 SWP 应用过的窗口左上角屏幕坐标（位置动效的起臂基准）
    pub(crate) live_pos: std::cell::Cell<(i32, i32)>,
    /// 【拉伸动效 2026-09-11】当前帧外壳（背景/边框/阴影/RGN）的物理
    /// 窗口尺寸覆盖（含阴影边距）：动效 tick 每帧设置为当前插值尺寸
    /// ——面板外壳被「拉过去」（延伸感），内容按目标布局裁在外壳内；
    /// 稳态帧 None（目标尺寸渲染，零开销）。
    pub(crate) chrome_override: std::cell::Cell<Option<(i32, i32)>>,
    /// 上次显示时刻（hide 距 show <250ms 直接隐藏不动画）
    pub(crate) last_show_at: Option<std::time::Instant>,
    /// 最近 SetWindowPos 应用过的窗口尺寸（检测尺寸变化→rect 同步）
    last_swp_size: std::cell::Cell<(i32, i32)>,
    /// 【rect 只增不减 2026-09-11】可见期间窗口 rect 的当前生效尺寸
    ///（内容收窄时窗口不缩——DWM 对「收缩」的 DComp 表面重绑会丢弃
    /// 后续半透明呈现，渐显会全程不上屏；增长/初始放置无此问题）。
    /// 隐藏时归零（新会话按首个内容重新定基准）。余量区域由
    /// WM_NCHITTEST 返回 HTTRANSPARENT 穿透鼠标。
    live_size: std::cell::Cell<(i32, i32)>,
    /// 当前内容实际尺寸（逻辑 px，用于命中测试区分内容区/透明余量）
    pub(crate) content_size: std::cell::Cell<(i32, i32)>,
    /// 【注释展开延时】本组段注释是否已展开：首显（hidden→visible）重置，
    /// 连打期间每帧重置倒计时，停手 comment_delay_ms 后补一帧全注释
    /// （展开后保持到组段结束）。抑制期空注释参与布局——列宽自然收起，
    /// 渲染路径零改动；兼防连打期长注释的窗宽抖动。
    pub(crate) comments_expanded: bool,
}

/// 【阴影圆角外遮罩】PushLayer：整画布 − 窗口圆角（even-odd 几何组），
/// 高斯弥散只出现在窗口轮廓之外。返回 true=已 Push（调用方 DrawImage
/// 后须 PopLayer）；false=几何创建失败（免 Pop）。
unsafe fn push_shadow_mask(
    ctx: &windows::Win32::Graphics::Direct2D::ID2D1DeviceContext,
    width: f32,
    height: f32,
    w_out: u32,
    h_out: u32,
    shadow_m: f32,
    radius: f32,
    dpi: f32,
    bx: f32,
    by: f32,
) -> bool {
    // 【高DPI二次缩放修复 2026-09-11】调用方现已在 identity 世界变换
    // 下 Push 本 mask（对齐玻璃段正序：先切 identity 再 Push——mask
    // 按当时 transform 解释）。窗口洞几何原为逻辑坐标（靠 dpi 主变换
    // 换算物理），identity 下必须显式乘 dpi；大矩形本就物理（w_out）。
    let f = match ctx.GetFactory() {
        Ok(f) => f,
        Err(_) => return false,
    };
    let big = match f.CreateRectangleGeometry(&D2D_RECT_F {
        left: -1.0e6,
        top: -1.0e6,
        right: w_out as f32 + 1.0e6,
        bottom: h_out as f32 + 1.0e6,
    }) {
        Ok(g) => g,
        Err(_) => return false,
    };
    let win = match f.CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: (shadow_m + bx) * dpi,
            top: (shadow_m + by) * dpi,
            right: (shadow_m + bx + width) * dpi,
            bottom: (shadow_m + by + height) * dpi,
        },
        radiusX: radius * dpi,
        radiusY: radius * dpi,
    }) {
        Ok(g) => g,
        Err(_) => return false,
    };
    let big: windows::Win32::Graphics::Direct2D::ID2D1Geometry = match big.cast() {
        Ok(g) => g,
        Err(_) => return false,
    };
    let win: windows::Win32::Graphics::Direct2D::ID2D1Geometry = match win.cast() {
        Ok(g) => g,
        Err(_) => return false,
    };
    let grp: windows::Win32::Graphics::Direct2D::ID2D1Geometry =
        match f.CreateGeometryGroup(D2D1_FILL_MODE_ALTERNATE, &[Some(big), Some(win)]) {
            Ok(g) => match g.cast() {
                Ok(g) => g,
                Err(_) => return false,
            },
            Err(_) => return false,
        };
    let mut lp = D2D1_LAYER_PARAMETERS1::default();
    lp.contentBounds = D2D_RECT_F {
        left: -1.0e6,
        top: -1.0e6,
        right: 1.0e6,
        bottom: 1.0e6,
    };
    lp.geometricMask = std::mem::ManuallyDrop::new(Some(grp));
    lp.maskAntialiasMode = D2D1_ANTIALIAS_MODE_PER_PRIMITIVE;
    lp.maskTransform = windows::Foundation::Numerics::Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: 0.0,
        M32: 0.0,
    };
    lp.opacity = 1.0;
    let _ = ctx.PushLayer(&lp, None);
    true
}

/// 【毛玻璃抓屏 2026-09-08】BitBlt 抓屏幕矩形（物理像素）为 BGRA 字节。
/// 调用时机=SetWindowPos 之前（窗口未画到新位置，抓到干净底）。
/// GDI BitBlt 300×150 亚毫秒级，无需权限（区别于 Graphics.Capture）。
/// DIB 32bpp top-down：GDI 字节序 BGRA；alpha 通道 BitBlt 不写（内容
/// 未定义，实测全 0）——函数内强制置 255（见【终极根因】注释）。
/// 【阶段验证 2026-09-08·用户指令】皮肤系统穷举重做：
/// stage.txt（C:\ProgramData\HuFu\diag\stage.txt）控制渲染分层——
/// 1=纯毛玻璃（透明+抓屏模糊，无遮罩/染色/阴影/内容）
/// 2=+染色 3=+圆角遮罩 4=+阴影 5/0=全功能。每加一层自动化验证
/// （条纹探针+梯度量化），定位第一个打破毛玻璃的层。
fn read_diag_stage() -> u32 {
    std::fs::read_to_string(r"C:\ProgramData\HuFu\diag\stage.txt")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0)
        .min(5)
}

/// 【毛玻璃 v3·DWM acrylic 2026-09-08】参照 window-vibrancy / TranslucentTB
///（GitHub 成熟方案，复用文件头部现成的 apply_accent 基础设施）：
/// NOREDIRECTIONBITMAP+DComp 窗口配 ACCENT_ENABLE_ACRYLICBLURBEHIND——
/// DWM 合成器直接给窗口底下做系统级真毛玻璃。零抓屏（自绘方案抓到
/// 自己黑块的死结消除）、零模糊算法。染色=GradientColor(0xAABBGGRR)。
unsafe fn capture_screen_rgba(x: i32, y: i32, w: u32, h: u32) -> Option<Vec<u8>> {
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
    };
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return None;
    }
    let screen = GetDC(HWND(std::ptr::null_mut()));
    if screen.is_invalid() {
        return None;
    }
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
    if bits.is_null() {
        ReleaseDC(HWND(std::ptr::null_mut()), screen);
        return None;
    }
    let memdc = CreateCompatibleDC(screen);
    let old = SelectObject(memdc, HGDIOBJ(dib.0));
    let ok = BitBlt(memdc, 0, 0, w as i32, h as i32, screen, x, y, SRCCOPY);
    let _ = SelectObject(memdc, old);
    let out = if ok.is_ok() {
        let mut px = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec();
        // 【终极根因 2026-09-08】GDI BitBlt 不写 alpha 通道（内容未定义，
        // 实测全 0）——PREMULTIPLIED 模式下 alpha=0=完全透明，毛玻璃层
        // 画的是全透明位图（红裁决能显示、模糊层不可见的真凶）。
        // 屏幕内容恒不透明：置 255。
        for i in (3..px.len()).step_by(4) {
            px[i] = 255;
        }
        Some(px)
    } else {
        None
    };
    let _ = DeleteObject(HGDIOBJ(dib.0));
    let _ = DeleteDC(memdc);
    ReleaseDC(HWND(std::ptr::null_mut()), screen);
    out
}

impl CandidateWindowV2 {
    /// 初始化设备管线；任何一步失败返回 None（调用方回退 v1）。
    pub fn new() -> Option<CandidateWindowV2> {
        unsafe {
            let class: Vec<u16> = "HuFuCandWin2\0".encode_utf16().collect();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(cand2_wndproc),
                // 类光标：NULL 会让鼠标移入时系统 fallback 到忙碌光标
                //（开始菜单/UWP 里实测「沙漏/转圈」）——显式箭头。
                hCursor: LoadCursorW(HINSTANCE(std::ptr::null_mut()), IDC_ARROW)
                    .unwrap_or(HCURSOR(std::ptr::null_mut())),
                lpszClassName: PCWSTR(class.as_ptr()),
                hbrBackground: HBRUSH(std::ptr::null_mut()),
                ..Default::default()
            };
            let _atom = RegisterClassW(&wc);
            // 注：曾因「点击候选框冻结」加过 WS_EX_TRANSPARENT 鼠标穿透
            // ——后经反汇编定位真凶为焦点回调内同步 ShowWindow 死锁
            // （已修），穿透撤销以支持拖拽/右键固定交互。
            let ex = WINDOW_EX_STYLE(
                WS_EX_TOOLWINDOW.0
                    | WS_EX_TOPMOST.0
                    | WS_EX_NOACTIVATE.0
                    | WS_EX_NOREDIRECTIONBITMAP.0,
            );
            let hwnd = CreateWindowExW(
                ex,
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(WS_POPUP.0),
                0,
                0,
                10,
                10,
                HWND(std::ptr::null_mut()),
                HMENU(std::ptr::null_mut()),
                HINSTANCE(std::ptr::null_mut()),
                None,
            )
            .unwrap_or_default();
            if hwnd.0.is_null() {
                return None;
            }

            // D3D11 设备（硬件 → WARP 兜底），必须 BGRA 供 D2D 互操作
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            for dt in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
                let ok = D3D11CreateDevice(
                    None,
                    dt,
                    HMODULE(std::ptr::null_mut()),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )
                .is_ok();
                if ok && device.is_some() {
                    break;
                }
            }
            let device = device?;
            let _ = context; // 无需常驻 D3D 上下文，D2D 自管

            let dxgi_dev: IDXGIDevice = device.cast().ok()?;
            let _factory: IDXGIFactory2 = CreateDXGIFactory1().ok()?;
            let factory2d: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_MULTI_THREADED, None).ok()?;
            let d2d_dev: ID2D1Device = factory2d.CreateDevice(&dxgi_dev).ok()?;
            let ctx: ID2D1DeviceContext = d2d_dev
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
                .ok()?;
            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi_dev).ok()?;
            let target = dcomp.CreateTargetForHwnd(hwnd, BOOL(1)).ok()?;
            let visual = dcomp.CreateVisual().ok()?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;

            Some(CandidateWindowV2 {
                hwnd,
                ctx: Some(ctx),
                swapchain: None,
                dcomp: Some(dcomp),
                target: Some(target),
                visual: Some(visual),
                dwrite: Some(dwrite),
                dxgi: Some(dxgi_dev.clone()),
                readback: false,
                sticky_pos: None,
                sticky_drag: false,
                last_raw_len: 0,
                last_pixels: None,
                last_dy: None,
                last_size: (0, 0),
                size: (0, 0),
                cloaked_streak: 0,
                tf_cache: None,
                dy_cache: None,
                shadow_cache: None,
                acrylic_last: std::cell::Cell::new(u64::MAX),
                rgn_last: std::cell::Cell::new(u64::MAX),
                fade_ms: 0,
                fade: None,
                internal_rerender: false,
                last_hide_at: None,
                last_show_at: None,
                size_anim: None,
                chrome_override: std::cell::Cell::new(None),
                hl_center: std::cell::Cell::new(None),
                scale_in: std::cell::Cell::new(false),
                size_ms: 90,
                pos_anim: None,
                pos_ms: 120,
                anim_on: std::cell::Cell::new(true),
                fade_ms_eff: 120,
                live_pos: std::cell::Cell::new((0, 0)),
                last_swp_size: std::cell::Cell::new((0, 0)),
                live_size: std::cell::Cell::new((0, 0)),
                content_size: std::cell::Cell::new((0, 0)),
                comments_expanded: true,
            })
        }
    }

    fn ensure_swapchain(&mut self, w: u32, h: u32) -> bool {
        // 【grow-only 2026-09-11】缓冲只增不减：内容变窄不再重建链。
        // 动机：重建（SetContent 重绑）后 DWM 会停止跟踪半透明帧的
        // 合成（像素取证：resize 后整段渐显 ramp 不上屏，直到某帧
        // 全不透明才「唤醒」）——收起→展开、连打变宽全中招。缓冲大
        // 于窗口的部分由 DWM 按窗口裁剪（旧帧残留实验已证）。
        // 代价：单窗 VRAM 上限 ~4MB（1024²×2buf×4B），可忽略。
        if self.swapchain.is_some() && w <= self.size.0 as u32 && h <= self.size.1 as u32 {
            return true;
        }
        unsafe {
            if let Some(ctx) = &self.ctx {
                ctx.SetTarget(None);
            }
            // 尺寸向上取整到 64 的倍数：减少重建次数（连打宽度微变不再
            // 触发）；下限 256×160 覆盖最小面板。
            let alloc_w = (((w.max(256)) + 63) / 64) * 64;
            let alloc_h = (((h.max(160)) + 63) / 64) * 64;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: alloc_w,
                Height: alloc_h,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: BOOL(0),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                Flags: 0,
            };
            // 重建 swapchain（仅增长时；候选窗小、代价可忽略）
            self.swapchain = None;
            let chain = match self.create_chain_from_ctx(&desc) {
                Some(c) => c,
                None => return false,
            };
            if let (Some(visual), Some(target), Some(dc)) =
                (&self.visual, &self.target, &self.dcomp)
            {
                if visual.SetContent(&chain).is_err()
                    || target.SetRoot(visual).is_err()
                    || dc.Commit().is_err()
                {
                    crate::tsf::trace("cw2: dcomp attach FAIL");
                    return false;
                }
                // 【已知限制 2026-09-11】曾试 SetClip（windows 0.58 无
                // float 重载→静态动画对象亦无效）。DWM 对部分窗口状态
                // 会把 DComp 表面提升到 MPO overlay——半透帧被拍平
                // （渐显退化为直接出现，内容仍正确）。动效因此默认关
                // （fade_ms=0 皮肤可开）。
                crate::tsf::trace(&format!(
                    "cw2: swapchain 重建+重绑 {alloc_w}×{alloc_h}（内容 {w}×{h}）+Clip"
                ));
            }
            self.swapchain = Some(chain);
            self.size = (alloc_w as i32, alloc_h as i32);
            true
        }
    }

    unsafe fn create_chain_from_ctx(
        &mut self,
        desc: &DXGI_SWAP_CHAIN_DESC1,
    ) -> Option<IDXGISwapChain1> {
        // 用 new() 时存下的 DXGI 设备（ID2D1Device QI 不出 IDXGIDevice）；
        // factory 必须与设备同源（device→adapter→GetParent），否则 INVALID_CALL
        let dxgi_dev: IDXGIDevice = self.dxgi.clone()?;
        let adapter: IDXGIAdapter = match dxgi_dev.GetAdapter() {
            Ok(a) => a,
            Err(e) => {
                crate::tsf::trace(&format!("cw2: GetAdapter err 0x{:08X}", e.code().0 as u32));
                return None;
            }
        };
        let factory: IDXGIFactory2 = match adapter.GetParent() {
            Ok(f) => f,
            Err(e) => {
                crate::tsf::trace(&format!("cw2: factory err 0x{:08X}", e.code().0 as u32));
                return None;
            }
        };
        factory
            .CreateSwapChainForComposition(&dxgi_dev, desc, None)
            .map_err(|e| {
                crate::tsf::trace(&format!(
                    "cw2: CreateSwapChain err 0x{:08X}",
                    e.code().0 as u32
                ));
                e
            })
            .ok()
    }
    /// 渲染并显示。anchor=插入点屏幕矩形：候选窗优先悬于其上方。selected=高亮行（页内 0 起）。
    pub fn show(
        &mut self,
        cands: &[(String, String)],
        raw: &str,
        skin: &Value,
        anchor: Option<&RECT>,
        selected: usize,
    ) {
        // 【5K/高 DPI 缩放 2026-09-06】窗口尺寸/渲染此前全部按 96-DPI 逻辑
        // 像素算——宿主 Per-Monitor V2 时这些被当物理像素用，200%/300%
        // 缩放屏上候选窗整体偏小。中心化修法：取窗口 DPI 得 scale，位图/
        // 窗口尺寸×scale，渲染层 SetTransform 缩放（DWrite 文本按目标
        // 分辨率光栅化，不糊），内容逻辑坐标全部不变。
        let dpi_scale =
            unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(self.hwnd).max(96) as f32 / 96.0 };
        // 【DPI 测试旋钮 2026-09-11】HUFU_FAKE_DPI=<dpi>：pad-dump/smoke
        // 取证用——100% 屏上伪造高 DPI 复现「高 DPI 阴影二次缩放」类
        // 问题（真机该值不存在，零影响）。值=目标 DPI（如 144=150%）。
        let dpi_scale = std::env::var("HUFU_FAKE_DPI")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| *v >= 96.0 && *v <= 480.0)
            .map(|v| v / 96.0)
            .unwrap_or(dpi_scale);
        // 【动效 2026-09-11·虎爪对标】渐隐渐显 + 注释展开延时。
        // 皮肤键（layout 节，缺省走代码默认）：fade_ms=120（0=关；
        // DWM 把静态窄窗提升到 MPO overlay 时渐变退化为直接出现，
        // 内容仍正确——生产路径弹窗必经尺寸增长，实测为合成态）、
        // comment_delay_ms=400（0=注释常显）。
        let was_visible = unsafe { IsWindowVisible(self.hwnd).as_bool() };
        // 阴影 alpha 复位：上次渐变留下的整窗 alpha 不能带进无动画帧
        if self.fade.is_none() {
            shadowwin_set_alpha(1.0);
        }
        let now = std::time::Instant::now();
        // 【动效全局开关+速度 2026-09-11】设置页·皮肤页：anim（bool，
        // 默认开）/ anim_speed（倍率，默认 1.0=当前速度）——server 注入
        // 皮肤对象顶层。关闭=一切动效瞬跳（含退场淡出）；速度统一乘
        // 尺寸/位置/淡出时长。
        let anim_on = skin
            .pointer("/skin/anim")
            .or_else(|| skin.get("anim"))
            .and_then(|x| x.as_bool())
            .unwrap_or(true);
        let anim_spd = skin
            .pointer("/skin/anim_speed")
            .or_else(|| skin.get("anim_speed"))
            .and_then(|x| x.as_f64())
            .unwrap_or(1.0)
            .clamp(0.25, 4.0) as f32;
        self.anim_on.set(anim_on);
        let anim_spd = if anim_on { anim_spd } else { 0.0 };
        // 【动效口径 2026-09-11 终版④】透明度渐变终判弃用（半透面板+
        // 深色底，任何 alpha 过渡都「变深/透底」——用户三度否决）。首出
        // /收尾改纯运动：首键从 72% 长大到目标（边框阴影跟着拉出），
        // 收尾收拢到 70% 后隐藏。fade_ms 皮肤键保留可开。
        self.fade_ms = (layout_f(skin, "fade_ms", 0.0).clamp(0.0, 600.0) * anim_spd) as u32;
        self.fade_ms_eff = (120.0 * anim_spd) as u32;
        self.size_ms = (layout_f(skin, "size_ms", 90.0).clamp(0.0, 600.0) * anim_spd) as u32;
        self.pos_ms = (layout_f(skin, "pos_ms", 120.0).clamp(0.0, 600.0) * anim_spd) as u32;
        let cmt_delay = layout_f(skin, "comment_delay_ms", 400.0).clamp(0.0, 5000.0) as u32;
        if !was_visible {
            // 新组段首显：注释展开态重置（0=常显直接展开）
            self.comments_expanded = cmt_delay == 0;
            // 静默期防频闪：距上次隐藏 <250ms（连打逐字上屏的收放循环）
            // 直接全显——刻意出现的窗才做渐显
            let quiet = self
                .last_hide_at
                .map(|t| now.duration_since(t).as_millis() >= FADE_QUIET_MS)
                .unwrap_or(true);
            if self.fade_ms > 0 && quiet {
                self.fade = Some((true, now));
                unsafe {
                    let _ = SetTimer(self.hwnd, FADE_TIMER_ID, FADE_TICK_MS, None);
                }
            } else {
                self.fade = None;
            }
            self.last_show_at = Some(now);
        } else {
            // 内容更新帧：渐显进行中照常换内容不打断；渐隐被新内容打断
            // → 立即回全显（窗口复活，不该继续淡出）。动效 tick 的内部
            // 复渲染不算用户内容更新，不触发取消。
            if let Some((false, _)) = self.fade {
                if !self.internal_rerender {
                    self.fade = None;
                    unsafe {
                        let _ = KillTimer(self.hwnd, FADE_TIMER_ID);
                    }
                }
            }
            // 【会话语义】last_show_at 只在本可见会话首显置位——vis_long
            //（退场门控）量「窗刻意在场多久」；此前每键刷新导致正常打字
            // 收尾总被判连打直藏（「消失没动画」根因）
        }
        if !self.comments_expanded && cmt_delay > 0 && !self.internal_rerender {
            // 连打期间逐帧重置倒计时（同 id SetTimer=重置）→ 停手
            // delay 后补一帧全注释；展开后保持到本组段结束。动效 tick
            // 的内部复渲染不重置（否则渐显期每次 tick 都推迟展开）。
            unsafe {
                let _ = SetTimer(self.hwnd, EXPAND_TIMER_ID, cmt_delay.max(1), None);
            }
        }
        // 注释抑制：未展开时空注释参与布局（列宽自然收起，渲染路径零改动）
        let cands_suppressed: Vec<(String, String)>;
        let cands: &[(String, String)] = if self.comments_expanded {
            cands
        } else {
            cands_suppressed = cands
                .iter()
                .map(|(t, _)| (t.clone(), String::new()))
                .collect();
            &cands_suppressed
        };
        // 序号显示：引擎 state 经 pipe skin 响应附带（根级 show_index）
        let show_index = skin
            .get("show_index")
            .and_then(|x| x.as_bool())
            .unwrap_or(true);
        // 【毛玻璃退役 2026-09-11】材质 glass/毛玻璃整链已删（kind 值
        // 不再分支，旧 glass 皮肤按 translucent 渲染）；accent 一次性
        // 置 DISABLED 清残留（热切换过毛玻璃的窗恢复普通合成）。
        {
            let key: u64 = (ACCENT_DISABLED as u64) << 32;
            if self.acrylic_last.get() != key {
                apply_accent(self.hwnd, ACCENT_DISABLED, [28, 28, 30, 0]);
                self.acrylic_last.set(key);
            }
        }

        let font_pt = layout_f(skin, "font_point", 16.0);
        let radius = layout_f(skin, "corner_radius", 8.0);
        let margin_x = layout_f(skin, "margin_x", 8.0);
        let margin_y = layout_f(skin, "margin_y", 6.0);
        let line_h = font_pt * 96.0 / 72.0 + layout_f(skin, "line_spacing", 3.0) + 5.0;
        // width>0 固定宽；0=按内容自适应（min_width~340 收夹）
        let width_cfg = layout_f(skin, "width", 0.0);
        let min_width = layout_f(skin, "min_width", 150.0).max(100.0);
        // 序号列宽：按本页实际序号宽度自适应（测量块内计算）。
        // 滚轮放大序号后列宽随字号缩放（「1.」「10.」不换行摞字），
        // 序号与正文紧贴（2026-09-06 用户两轮实测反馈后：无固定基准，
        // 实测宽 + 2px）。
        let mut label_w = if show_index { 0.0f32 } else { 0.0 };
        let em = font_pt * 96.0 / 72.0;
        // 横排（skin.layout.horizontal）：候选单行横铺，weasel 式
        let horizontal = skin
            .pointer("/skin/layout/horizontal")
            .or_else(|| skin.get("layout").and_then(|l| l.get("horizontal")))
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        // 【超屏修复】超长句候选框超出屏幕（用户实测）：横排宽度纯
        // 内容自适应且编码行整段参与定宽，无上限。三管齐下：
        // ① 显示层预截断——超长 raw/候选只保尾部（正在打的部分），
        //    前缀「…」；同名遮蔽参数，后续测量/定宽/渲染全部同源。
        //    截宽按估算（CJK≈em、ASCII≈0.55em），宁可少截不可截不满。
        // ② 横排宽度封顶工作区宽（见 width 计算处 w.min(w_cap)）。
        // ③ 位置 clamp 原已有——宽度封顶后 clamp 区间不再倒置。
        let screen_w = unsafe { GetSystemMetrics(SM_CXFULLSCREEN) }.max(200) as f32;
        let w_cap = (screen_w - 24.0).max(320.0);
        let trunc_tail = |s: &str, cap: f32| -> String {
            let est = |s: &str| {
                s.chars()
                    .map(|c| if c.is_ascii() { em * 0.55 } else { em })
                    .sum::<f32>()
                    + em // 「…」前缀余量
            };
            if est(s) <= cap {
                return s.to_string();
            }
            let chars: Vec<char> = s.chars().collect();
            let mut k = chars.len();
            while k > 6 {
                if est(&chars[chars.len() - k..].iter().collect::<String>()) <= cap {
                    return std::iter::once('…')
                        .chain(chars[chars.len() - k..].iter().copied())
                        .collect();
                }
                k -= 2;
            }
            std::iter::once('…')
                .chain(chars[chars.len() - 6..].iter().copied())
                .collect()
        };
        let raw_disp_cap = if horizontal { w_cap * 0.7 } else { 260.0 };
        let cand_disp_cap = if horizontal { w_cap * 0.35 } else { 240.0 };
        let raw = trunc_tail(raw, raw_disp_cap.max(100.0));
        let cands: Vec<(String, String)> = cands
            .iter()
            .map(|(t, c)| (trunc_tail(t, cand_disp_cap.max(80.0)), c.clone()))
            .collect();
        // 【每帧开销缓存】块前取块后存（测量/渲染 unsafe 块内 self 有
        // 借用，不能就地读写缓存字段）——字体三件套 / 光学 dy / 阴影
        // cl+effect，键不变则零创建。
        let tf_cache_in = self.tf_cache.clone();
        let dy_cache_in = self.dy_cache.clone();
        let shadow_cache_in = self.shadow_cache.clone();
        let cand_spacing = layout_f(skin, "candidate_spacing", 6.0);
        let hilite_pad = layout_f(skin, "hilite_padding", 4.0);
        // 【皮肤元素自查 2026-09-08】对照 weasel 语义激活三个死参数：
        // · label_format：序号 printf 格式（%s→序号）。此前 DLL 写死
        //   "N."——出厂皮肤 7 款 "%s"（无点）的分化设计从未生效。
        // · hilite_spacing：weasel 语义 = 序号↔正文、正文↔注释的统一
        //   间距。此前两处写死 2px/3px 不一致。出厂皮肤统一改 2（保持
        //   用户定稿的紧贴视觉），语义交还皮肤参数。
        // · real_margin（weasel Layout.cpp）：内容边距 = max(margin,
        //   hilite_padding)——高亮胶囊左右外扩 hilite_pad，margin 小于
        //   它时候选胶囊会出血到窗外（负坐标）。
        let label_fmt: String = skin
            .pointer("/skin/layout/label_format")
            .or_else(|| skin.get("layout").and_then(|l| l.get("label_format")))
            .and_then(|x| x.as_str())
            .unwrap_or("%s.")
            .to_string();
        // 【序号样式 2026-09-08】label_style：digit（默认）/zh（一二三…）
        // /roman（Ⅰ Ⅱ Ⅲ…）——%s 的替换字形。第 10 候选在 digit 下仍显
        // 0（1234567890，与引擎 0=10 选重键一致）；zh/roman 用十/Ⅹ。
        let label_style: String = skin
            .pointer("/skin/layout/label_style")
            .or_else(|| skin.get("layout").and_then(|l| l.get("label_style")))
            .and_then(|x| x.as_str())
            .unwrap_or("digit")
            .to_string();
        let fmt_label = |n: usize| -> String {
            let d: String = match label_style.as_str() {
                "zh" => {
                    const ZH: [&str; 10] =
                        ["一", "二", "三", "四", "五", "六", "七", "八", "九", "十"];
                    ZH[(if n == 10 { 10 } else { n }) - 1].to_string()
                }
                "roman" => {
                    const RM: [&str; 10] = ["Ⅰ", "Ⅱ", "Ⅲ", "Ⅳ", "Ⅴ", "Ⅵ", "Ⅶ", "Ⅷ", "Ⅸ", "Ⅹ"];
                    RM[(if n == 10 { 10 } else { n }) - 1].to_string()
                }
                _ => {
                    // 【10 选序号】第 10 候选显示 0（1234567890）
                    if n == 10 {
                        "0".to_string()
                    } else {
                        n.to_string()
                    }
                }
            };
            match label_fmt.find("%s") {
                Some(p) => format!("{}{}{}", &label_fmt[..p], d, &label_fmt[p + 2..]),
                None => format!("{d}."),
            }
        };
        let hsp = layout_f(skin, "hilite_spacing", 2.0);
        // mark_text（weasel 语义=高亮候选标记）：非空时高亮胶囊左缘
        // 内侧画细竖条（字符本身不画字形，用细条更精致；缺省空=不画）
        let mark_en = !skin
            .pointer("/skin/layout/mark_text")
            .or_else(|| skin.get("layout").and_then(|l| l.get("mark_text")))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .is_empty();
        // 【布局口径重构 2026-09-08·对齐 weasel schema】用户定调：
        // 高亮只有两个几何属性——①「高亮与边框的距离」gap（四边同
        // 一值）②「高亮内距」hilite_padding（四边同一值，管胶囊↔
        // 文字）。废除此前 margin_x/y 分别推导+调平+收窄的补丁堆
        //（v2 调平/SQUEEZE_X/SQUEEZE_Y 全删——口径不一的根源）。
        // gap=(margin_x+margin_y)/2：老皮肤自动对称化；设置页已合
        // 一「边距」滑杆（同写两字段），新口径下恒 mx=my=gap。
        // 行槽高 row_h=max(line_h, em+2hp)：胶囊需要时撑高，胶囊与
        // 文字在槽内居中——胶囊四边到窗恒= gap（weasel margin 语义）。
        let gap = (margin_x + margin_y) / 2.0;
        let rm_x = gap;
        let rm_y = gap;
        let pill_h = em + hilite_pad * 2.0;
        let row_h = line_h.max(pill_h);

        // 字体与内容测宽先行（宽度取决于最长候选）
        let mut tf_cache_out: Option<(
            (String, f32, f32),
            (
                Option<IDWriteTextFormat>,
                Option<IDWriteTextFormat>,
                Option<IDWriteTextFormat>,
            ),
        )> = None;
        let mut dy_cache_out: Option<((String, f32), f32)> = None;
        let mut shadow_cache_out: Option<(
            (u32, u32, u32, u32, (i32, i32), u32, u32),
            (ID2D1CommandList, ID2D1Effect),
        )> = None;
        let (tf, tf_label, tf_small, cand_ws, geo) = unsafe {
            let dwrite = match &self.dwrite {
                Some(d) => d.clone(),
                None => return,
            };
            let font_face: String = {
                let f = skin
                    .pointer("/skin/layout/font_face")
                    .or_else(|| skin.get("layout").and_then(|l| l.get("font_face")))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                if f.is_empty() {
                    "Microsoft YaHei UI".into()
                } else {
                    f.to_string()
                }
            };
            let locale: Vec<u16> = "zh-CN\0".encode_utf16().collect();
            // 字体族缺失时回退雅黑（防 CreateTextFormat 失败 → 全窗无字）
            let mk_tf = |fam: &str, em: f32| -> Option<IDWriteTextFormat> {
                let mut b: Vec<u16> = fam.encode_utf16().collect();
                b.push(0);
                dwrite
                    .CreateTextFormat(
                        PCWSTR(b.as_ptr()),
                        None,
                        DWRITE_FONT_WEIGHT_NORMAL,
                        DWRITE_FONT_STYLE_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        em,
                        PCWSTR(locale.as_ptr()),
                    )
                    .ok()
            };
            let tf;
            let tf_small;
            let tf_label;
            // 标签序号字体（layout.label_font_point；0/缺省回退 0.78 倍正文）
            let label_pt = layout_f(skin, "label_font_point", 0.0);
            let tf_key = (font_face.clone(), font_pt, label_pt);
            let tf_hit = tf_cache_in
                .as_ref()
                .map(|(k, _)| *k == tf_key)
                .unwrap_or(false);
            if tf_hit {
                let (_, v) = tf_cache_in.as_ref().unwrap();
                tf = v.0.clone();
                tf_small = v.1.clone();
                tf_label = v.2.clone();
            } else {
                tf = mk_tf(&font_face, em).or_else(|| mk_tf("Microsoft YaHei UI", em));
                tf_small =
                    mk_tf(&font_face, em * 0.78).or_else(|| mk_tf("Microsoft YaHei UI", em * 0.78));
                // 文本垂直居中（高亮胶囊上下留白对称的关键）
                for t in [&tf, &tf_small] {
                    if let Some(t) = t {
                        let _ = t.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                    }
                }
                tf_label = if label_pt > 0.0 {
                    mk_tf(&font_face, label_pt * 96.0 / 72.0)
                        .or_else(|| mk_tf("Microsoft YaHei UI", label_pt * 96.0 / 72.0))
                        .or(tf_small.clone())
                } else {
                    tf_small.clone()
                };
                if let Some(t) = &tf_label {
                    let _ = t.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                }
                tf_cache_out = Some((tf_key, (tf.clone(), tf_small.clone(), tf_label.clone())));
            }

            let measure = |tf: &Option<IDWriteTextFormat>, s: &str| -> f32 {
                if s.is_empty() {
                    return 0.0;
                }
                if let Some(tf) = tf {
                    let w: Vec<u16> = s.encode_utf16().collect();
                    if let Ok(l) = dwrite.CreateTextLayout(&w, tf, 4096.0, line_h.max(8.0)) {
                        let mut m = DWRITE_TEXT_METRICS::default();
                        if l.GetMetrics(&mut m).is_ok() {
                            return m.width.ceil();
                        }
                    }
                }
                // 兜底：按字数估宽
                s.chars().count() as f32 * em
            };
            // 序号列宽自适应（见 label_w 定义处注释）：竖排列宽按本页
            // 实际最大序号「N.」实测（只显示 min(len,10) 个）。序号↔
            // 正文间距 2px（2026-09-06 用户两轮反馈「隔太远」：4px 设计
            // 值 + 「.」字形尾部侧空 + 汉字墨盒头部侧空，视觉 ≈6-8px
            // 偏松——收紧为紧贴值）。
            let n_show = cands.len().min(10);
            if show_index && n_show > 0 {
                let wmax = measure(&tf_label, &fmt_label(n_show));
                label_w = label_w.max(wmax + hsp);
            }
            let mut max_text = 0.0f32;
            let mut max_cmt = 0.0f32;
            let mut cand_ws: Vec<(f32, f32, f32)> = Vec::new();
            for (i, (t, c)) in cands.iter().enumerate() {
                let tw = measure(&tf, t.as_str());
                let cw = if c.is_empty() {
                    0.0
                } else {
                    measure(&tf_small, c.as_str())
                };
                // 本格序号宽（label_format 格式化实测 + hsp 紧贴间距）：横排正文紧跟序号
                let iw = if show_index && i < 10 {
                    measure(&tf_label, &fmt_label(i + 1)).max(10.0) + hsp
                } else {
                    0.0
                };
                max_text = max_text.max(tw);
                if !c.is_empty() {
                    max_cmt = max_cmt.max(cw);
                }
                cand_ws.push((tw, cw, iw));
            }
            // 光学垂直居中：CJK 墨盒在行盒内整体偏上（行盒含下降部空白，
            // 段落居中只对齐行盒）→ 视觉上下内边距不等（下面多）。
            // GetOverhangMetrics 给墨迹相对布局盒的突出量（负=内缩），
            // 位移 = 行中心 − 墨盒中心。
            let probe_slack = |txt: &str, t: &Option<IDWriteTextFormat>| -> Option<(f32, f32)> {
                if let Some(t) = t {
                    let ws: Vec<u16> = txt.encode_utf16().collect();
                    if let Ok(l) = dwrite.CreateTextLayout(&ws, t, 4096.0, line_h.max(8.0)) {
                        if let Ok(o) = l.GetOverhangMetrics() {
                            return Some((-o.top, -o.bottom)); // (顶 slack, 底 slack)
                        }
                    }
                }
                None
            };
            // 光学补偿计算（缓存 miss 时用）：墨盒在行盒内偏上，
            // 位移 = 行中心 − 墨盒中心（底 slack − 顶 slack 的一半）
            let probe_dy =
                |probe: &dyn Fn(&str, &Option<IDWriteTextFormat>) -> Option<(f32, f32)>,
                 t: &Option<IDWriteTextFormat>|
                 -> f32 {
                    if let Some((top_slack, bot_slack)) = probe("永", t) {
                        ((bot_slack - top_slack) * 0.5).clamp(-6.0, 6.0)
                    } else {
                        0.0
                    }
                };
            let dy;
            let dy_key = (font_face.clone(), font_pt);
            if let Some((k, v)) = &dy_cache_in {
                if *k == dy_key {
                    dy = *v;
                } else {
                    dy = probe_dy(&probe_slack, &tf);
                    dy_cache_out = Some((dy_key, dy));
                }
            } else {
                dy = probe_dy(&probe_slack, &tf);
                dy_cache_out = Some((dy_key, dy));
            }
            // 注意：编码行定宽策略——
            // 横排：编码与候选同行（左，2026-09-05），宽度参与定宽；
            // 【2026-09-06 竖排根修】竖排同样参与定宽：此前竖排恒 0 导致
            // 窗宽只随候选列——反查「·〔反查〕 ni」类长编码行超出窗宽，
            // DrawText 自动换行下移、被后画的候选行覆盖（用户实测
            // 「字母超 3 个编码下移被候选挡住」）。竖排也量编码宽，
            // 窗口加宽容纳编码单行（显示层 trunc_tail 的 260px 截断仍在，
            // 长码尾部保留，不会无限撑宽）。
            let raw_w = if raw.is_empty() {
                0.0
            } else {
                measure(&tf, raw.as_str())
            };
            // 【注释配额 2026-09-07】两处布局封顶（竖排固定宽/300、横排
            // w_cap）与最长注释冲突时按配额截断注释（尾部 …），不再让
            // 注释列侵入文本列——旧版 cmt_x 以 max_cmt 定位、横排按原
            // cw 推进格子，长注释（拆分+拼音+unicode 多段拼接）直接压到
            // 候选文本上或溢出重叠（用户实测「候选挤在一起」）。
            let mut cmt_disp: Vec<String> = cands.iter().map(|(_, c)| c.clone()).collect();
            let trunc_cmt = |s: &str, quota: f32| -> String {
                if quota <= 0.0 || s.is_empty() {
                    return String::new();
                }
                let mut out: String = s.to_string();
                loop {
                    let mut t = out.clone();
                    t.push('…');
                    if measure(&tf_small, &t) <= quota {
                        return t;
                    }
                    if out.pop().is_none() {
                        return String::new();
                    }
                }
            };
            let (width, text_x, cmt_x, cmt_w) = if horizontal {
                // 横排内容自适应：Σ(标签+文本+注释+间隔)，上限 w_cap。
                // 【撤销 min_width 下限 2026-09-08】用户实测「最低宽度
                // 受限」——虎码横排候选少时窄窗更精致，min_width(150)
                // 让窗窄不下去；weasel 语义此处不适用，恢复纯自适应。
                let mut w = rm_x * 2.0 + hilite_pad * 2.0;
                if raw_w > 0.0 {
                    w += raw_w + 10.0; // 编码段（左）+ 编码↔候选间隔
                }
                for (_i, (tw, cw, iw)) in cand_ws.iter().enumerate() {
                    if _i > 0 {
                        w += cand_spacing;
                    }
                    w += iw + tw + if *cw > 0.0 { hsp + cw } else { 0.0 };
                }
                let w_full = w.max(raw_w + rm_x * 2.0 + hilite_pad * 2.0);
                // 【超屏修复】横排宽度封顶：工作区宽 − 余量。超屏时注释
                // 预算按剩余空间等比压缩（不足 12px 整列不显示），逐条
                // 截断加 …；格子推进宽同步收缩，尾部候选不再溢出重叠。
                if w_full > w_cap {
                    let budget: f32 = cand_ws
                        .iter()
                        .filter(|(_, c, _)| *c > 0.0)
                        .map(|(_, c, _)| hsp + c)
                        .sum();
                    let avail = w_cap - (w_full - budget);
                    if avail <= 12.0 {
                        for i in 0..cand_ws.len() {
                            cand_ws[i].1 = 0.0;
                            cmt_disp[i].clear();
                        }
                    } else if budget > 0.0 {
                        let scale = avail / budget;
                        for i in 0..cand_ws.len() {
                            let cw = cand_ws[i].1;
                            if cw > 0.0 {
                                let quota = ((cw + hsp) * scale - hsp).max(0.0);
                                cmt_disp[i] = trunc_cmt(&cmt_disp[i], quota);
                                cand_ws[i].1 = if cmt_disp[i].is_empty() { 0.0 } else { quota };
                            }
                        }
                    }
                }
                (w_full.min(w_cap), 0.0, 0.0, 0.0)
            } else {
                // 标签列 + 最宽候选 +（备注列）+ 高亮胶囊余量
                // 【口径统一】胶囊四边=gap：文字列从 gap+hp 起、width 含
                // 两端 hp；胶囊 [gap, width-gap] 不再 ±hp 外扩。
                let mut need =
                    rm_x + hilite_pad + label_w + max_text.max(raw_w) + hilite_pad + rm_x + 6.0;
                if max_cmt > 0.0 {
                    need += 6.0 + max_cmt;
                }
                let width = if width_cfg > 0.0 {
                    width_cfg
                } else {
                    // 【最大号溢出修复 2026-09-09】原 clamp(min,300) 硬上
                    // 限：大字号（滚轮最大）下 margin/pad/label 随字号联动
                    // 放大，need≈内容(≤260)+边距≈358 被 300 砍——编码行/
                    // 候选文字超窗绘制（多人实测「元素溢出边框包不住」）。
                    // 上限改工作区宽（与横排 w_cap 同源）：大字号=宽窗，
                    // 仅超屏封顶；内容已有显示层截断（260/240）兜底。
                    need.clamp(min_width, w_cap)
                };
                let text_x = rm_x + hilite_pad + label_w;
                // 注释列配额：右端对齐不变，宽压到「文本列右侧余量」；
                // 超配额逐条截断（…）。固定宽皮肤装不下整条注释时宁可
                // 截断注释也不压文本列。
                let quota = if max_cmt > 0.0 {
                    (width - rm_x - 2.0 - (text_x + max_text.max(raw_w) + 6.0)).max(0.0)
                } else {
                    0.0
                };
                if quota <= 0.0 {
                    for i in 0..cand_ws.len() {
                        cand_ws[i].1 = 0.0;
                        cmt_disp[i].clear();
                    }
                } else if quota < max_cmt {
                    for (i, (_, c)) in cands.iter().enumerate() {
                        if cand_ws[i].1 > quota {
                            cmt_disp[i] = trunc_cmt(c, quota);
                            cand_ws[i].1 = if cmt_disp[i].is_empty() { 0.0 } else { quota };
                        }
                    }
                }
                let (cmt_x, cmt_w) = if quota > 0.0 {
                    (width - rm_x - quota - 2.0, quota + 2.0)
                } else {
                    (width, 0.0)
                };
                (width, text_x, cmt_x, cmt_w)
            };
            (
                tf,
                tf_label,
                tf_small,
                cand_ws,
                (cmt_disp, (width, text_x, cmt_x, cmt_w, dy, raw_w)),
            )
        };
        let (v_width, text_x, cmt_x, _cmt_w, dy, raw_w) = geo.1;
        let cmt_disp = geo.0;
        // 编码行仅在有内容时占一行（show_code=false 且无 aux 时收缩）；
        // 横排编码与候选同行（左），不占独立行（2026-09-05）
        let code_row = if raw.is_empty() || horizontal {
            0.0
        } else {
            1.0
        };
        // 横排：内容即宽（纯自适应）；竖排：固定宽/自适应原逻辑
        let width = v_width;
        let height = if horizontal {
            // 高度贴合内容：gap×2 + 行槽高×行数 + 编码行后行距（与渲染 y0 一致）
            //【口径重构】line_h→row_h（行槽撑高装胶囊，胶囊四边=gap）
            rm_y * 2.0 + row_h * (1.0 + code_row) + cand_spacing * code_row
        } else {
            // 行距只计行间（编码行后 1 个 + 候选行间 rows-1 个）——渲染 y0 同步
            let rows = cands.len().min(10) as f32 + code_row;
            rm_y * 2.0 + row_h * rows + cand_spacing * (rows - 1.0).max(0.0)
        };

        let w = width as u32;
        let h = height as u32;
        // 投影：shadow_radius>0 时窗口四周外扩边距，阴影画在边距里
        //（内容绘制整体平移进边距内，见渲染段 SetTransform）
        // 【阴影弱联动 2026-09-08】字号放大时阴影半径按 √比例 放大：
        // 等比放大（×2.57）会把同样 alpha 的高斯摊到 2.57 倍面积，
        // 视觉浓度骤降（用户实测「变大后阴影浓度不太够」）。√比例
        //（×1.6）摊薄减半，浓度观感保留；渲染时计算不落盘，滚轮
        // 缩放来回不漂移。基准 14.5=hufu-skin 出厂主字号。
        let _shadow_base = layout_f(skin, "shadow_radius", 6.0).clamp(0.0, 60.0);
        let font_scale = (font_pt / 14.5).clamp(0.5, 3.0);
        let shadow_radius = (_shadow_base * font_scale.sqrt()).clamp(0.0, 60.0);
        // 【默认阴影偏移 0】用户定稿：所有皮肤默认阴影偏移=0（居中）。
        // 【玻璃零偏移 2026-09-09】毛玻璃模式不允许阴影偏移（SDF 居中
        // 投影，偏移破坏对称）——即使皮肤数据带偏移也钳为 0（与
        // hufu-skin save 归一、设置页禁用滑杆三重一致）。
        let shadow_off_y = layout_f(skin, "shadow_offset_y", 0.0);
        // 【2026-09-06 阴影水平偏移】用户规格：阴影加左右偏移（默认 0=居中）
        let shadow_off_x = layout_f(skin, "shadow_offset_x", 0.0);
        let has_shadow = shadow_radius >= 1.0;
        // 【阴影位图边距】按 D2D1Shadow 的模糊扩散精确覆盖：σ=radius*0.5+1，
        // 高斯扩散 3σ 覆盖 99.7%——小于此会在位图边界被直角截断（用户
        // 实测「超出 R 角的直角色块」= 弥散阴影遭位图边缘切割）。
        let shadow_m = if has_shadow {
            let sigma = shadow_radius * 0.5 + 1.0;
            (sigma * 3.0 + 6.0 + shadow_off_y.abs().max(shadow_off_x.abs())).ceil()
        } else {
            0.0
        };
        let w_out = ((w + 2 * shadow_m as u32) as f32 * dpi_scale) as u32;
        let h_out = ((h + 2 * shadow_m as u32) as f32 * dpi_scale) as u32;
        // 【拉伸动效 2026-09-11】渲染前判定（仅真实内容更新/展开帧——
        // 内部 tick 复渲染与回读取证帧不重臂）：宽高变化超阈值 → 启动/
        // 重定尺寸动画；此后每 tick 由 fade_tick_shared 以「当前插值
        // 尺寸」重绘外壳（背景/边框/阴影跟着边缘拉过去=延伸感），内容
        // 按目标布局裁在外壳内。
        if !self.internal_rerender {
            let target = (w_out as i32, h_out as i32);
            let cur = match self.size_anim {
                Some((f, t, t0)) => size_ease(f, t, t0.elapsed().as_millis() as u32, self.size_ms),
                None => self.live_size.get(),
            };
            if self.readback {
                self.size_anim = None;
                self.chrome_override.set(None);
            } else if was_visible
                && self.size_ms > 0
                // 【起臂阈值 24/14 2026-09-11】普通逐字打字的行宽增长
                // （~8-17px/键）不动画——文字即时更新（连打不闪不滞后，
                // 用户实测逐键闪的规避）；动画留给结构性大变化（注释
                // 展开、横竖切换、候选大改）。旧 10/8 阈值=几乎每键起
                // 臂 → 内容层遮罩逐键在岗 = 文字闪的放大器。
                && ((target.0 - cur.0).abs() > 24 || (target.1 - cur.1).abs() > 14)
            {
                self.size_anim = Some((cur, target, std::time::Instant::now()));
                // 起臂帧即按当前尺寸渲染外壳（否则首帧按目标画、下一
                // tick 又缩回=边缘/阴影跳一下）
                self.chrome_override.set(Some(cur));
                unsafe {
                    let _ = SetTimer(self.hwnd, FADE_TIMER_ID, FADE_TICK_MS, None);
                }
            } else if !was_visible
                && self.size_ms > 0
                && self.fade_ms == 0
                && self
                    .last_hide_at
                    .map(|t| {
                        std::time::Instant::now().duration_since(t).as_millis() >= FADE_QUIET_MS
                    })
                    .unwrap_or(true)
            {
                // 【首出长大 2026-09-11】刻意出现的窗（静默门外）从 72%
                // 拉到目标——纯尺寸动效（零透明度变化=无变深/透底），
                // 盒心锚定高亮胶囊（「从高亮区出现」）；连打循环直接全显
                let start = (
                    (target.0 as f32 * 0.72) as i32,
                    (target.1 as f32 * 0.72) as i32,
                );
                self.size_anim = Some((start, target, std::time::Instant::now()));
                self.chrome_override.set(Some(start));
                self.scale_in.set(true);
                unsafe {
                    let _ = SetTimer(self.hwnd, FADE_TIMER_ID, FADE_TICK_MS, None);
                }
            } else if !self.scale_in.get() {
                self.size_anim = None;
                self.chrome_override.set(None);
            }
        }
        // 外壳有效物理尺寸：动效中=当前插值（窗口尺寸，含阴影边距），
        // 稳态=目标
        let (cw_out, ch_out) = match self.chrome_override.get() {
            Some((cw, ch)) => (cw.max(1) as u32, ch.max(1) as u32),
            None => (w_out.max(1), h_out.max(1)),
        };
        // 外壳内容盒（逻辑系）：从物理窗口尺寸减阴影边距换算
        let (chw, chh) = match self.chrome_override.get() {
            Some((cw, ch)) => (
                ((cw as f32 / dpi_scale) - 2.0 * shadow_m).max(1.0),
                ((ch as f32 / dpi_scale) - 2.0 * shadow_m).max(1.0),
            ),
            None => (width, height),
        };
        // 【高亮锚定 v2 2026-09-11】入场动画：窗口位置/尺寸=目标全程
        // 稳定（零位移=零跳变），动画=窗口内的「外壳盒」从高亮胶囊处
        // 长到全窗。盒左上 = damp·(高亮−盒半) 且钳制在窗内 [0, 尺寸−
        // 盒]：高亮贴左/贴顶（横紧首候选、竖排首行）→ 盒贴对应边缘
        // 就地生长（左/上无内容不空跳）；高亮居中 → 对称展开。damp 随
        // 进度归零，完成帧=整窗。普通尺寸动效/稳态 bx=by=0。
        let (bx, by) = if self.scale_in.get() && self.size_anim.is_some() {
            let (hx, hy) = self.hl_center.get().unwrap_or((width * 0.5, height * 0.5));
            let k = match self.size_anim {
                Some((_, _, t0)) => {
                    (t0.elapsed().as_millis() as f32 / self.size_ms.max(1) as f32).clamp(0.0, 1.0)
                }
                None => 1.0,
            };
            let damp = 1.0 - k;
            (
                (damp * (hx - chw * 0.5)).clamp(0.0, (width - chw).max(0.0)),
                (damp * (hy - chh * 0.5)).clamp(0.0, (height - chh).max(0.0)),
            )
        } else {
            (0.0, 0.0)
        };
        'sizedraw: {
            // 【零位移配套】缓冲按目标窗口（非插值壳）——动画全程窗口
            // 恒定，缓冲每键至多扩一次（grow-only），无逐 tick resize
            if !self.ensure_swapchain(w_out.max(1), h_out.max(1)) {
                crate::tsf::trace("cw2: ensure_swapchain FAIL");
                return;
            }

            unsafe {
                let chain = match &self.swapchain {
                    Some(c) => c.clone(),
                    None => return,
                };
                let surface: IDXGISurface = match chain.GetBuffer(0) {
                    Ok(s) => s,
                    Err(_) => return,
                };
                let ctx = match &self.ctx {
                    Some(c) => c.clone(),
                    None => return,
                };
                let bp = D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                    colorContext: std::mem::ManuallyDrop::new(None),
                };
                let bitmap = match ctx.CreateBitmapFromDxgiSurface(&surface, Some(&bp)) {
                    Ok(b) => b,
                    Err(_) => return,
                };
                ctx.SetTarget(&bitmap);
                ctx.BeginDraw();
                // 【动效 2026-09-11】过渡帧整帧透明度（渐隐渐显）：PushLayer
                // opacity 包住全部绘制（含玻璃/自绘阴影内容）——稳态 alpha=1
                // 零开销（不 Push）。alpha 由 fade 状态推导（fade_alpha）。
                let fade_a = self.fade_alpha();
                let fade_layer_on = fade_a < 0.999;
                if fade_layer_on {
                    let mut lp = D2D1_LAYER_PARAMETERS1::default();
                    lp.contentBounds = D2D_RECT_F {
                        left: -1.0e6,
                        top: -1.0e6,
                        right: 1.0e6,
                        bottom: 1.0e6,
                    };
                    lp.opacity = fade_a;
                    ctx.PushLayer(&lp, None);
                }
                // 【阶段验证】stage 分层渲染开关（见 read_diag_stage 注释）
                let stage = read_diag_stage();
                ctx.SetTransform(&windows::Foundation::Numerics::Matrix3x2 {
                    M11: dpi_scale,
                    M12: 0.0,
                    M21: 0.0,
                    M22: dpi_scale,
                    M31: 0.0,
                    M32: 0.0,
                });

                // 背景：一律清透明后画「圆角」底——四角保持透明，窗口才是真圆角
                // （旧行 solid 用 Clear 铺满整窗把圆角补成直角）
                // 材质简化：solid=底色 / translucent|frosted(旧皮肤兼容)=tint 半透明 /
                // glass=毛玻璃（抓屏+D2D 高斯模糊+圆角裁剪，2026-09-08）；
                // material.opacity(0-1) 统一控透明度。
                let _ = ctx.Clear(Some(&D2D1_COLOR_F {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                }));
                // 【毛玻璃退役 2026-09-11】自绘毛玻璃路径整块移除（v3 起
                // 已由 DWM acrylic 承担、现 accent 也已 DISABLED；kind 值不再分支。
                // 投影：多层外扩圆角矩形衰减近似高斯模糊（外坐标空间，
                // 内容平移前画——内容面板会盖住投影内圈，只留柔和外沿）
                if has_shadow && !(stage >= 1 && stage <= 3) {
                    // 【阴影透明度】material.shadow_alpha 独立滑条（颜色自带
                    // alpha 忽略——与纯色模型一致的语义）
                    let shadow_alpha = skin
                        .pointer("/skin/material/shadow_alpha")
                        .or_else(|| skin.get("material").and_then(|m| m.get("shadow_alpha")))
                        .and_then(|x| x.as_f64())
                        .unwrap_or(1.0)
                        .clamp(0.0, 1.0) as f32;
                    let mut sc = color_f(skin, "shadow_color", "#000000FF");
                    sc.a = shadow_alpha;
                    if sc.a > 0.004 {
                        // 【每帧缓存键】(w,h,corner_radius,shadow_radius,oy,ox,argb,dpi)
                        // ——宽度不变的连续帧（同长度候选/翻页）零重建 command
                        // list+effect。【2026-09-08 分离 BUG 修复】原键漏了
                        // shadow_radius：拖「阴影大小」滑杆时 shadow_m→窗口
                        // 尺寸变，但内容 w/h/圆角/偏移/颜色全没变→键命中→
                        // 重放旧 effect（旧 blur、旧 shadow_m 坐标的圆角矩形）
                        // 画进新窗口，阴影与窗体错位（用户实测「偶发阴影和
                        // 候选分离」的根因）。
                        let sc_packed = ((sc.r * 255.0) as u32)
                            | (((sc.g * 255.0) as u32) << 8)
                            | (((sc.b * 255.0) as u32) << 16)
                            | (((sc.a * 255.0) as u32) << 24);
                        let sh_key = (
                            // 【拉伸动效修残影 2026-09-11】键必须用「有效
                            // 外壳尺寸」chw/chh——否则收窄动画（乃至完成后的
                            // 稳态帧）命中起臂时录下的宽阴影缓存，右侧一直
                            // 复放宽阴影=残留。逐 tick 尺寸变→逐帧重建
                            //（command list+effect，亚毫秒）。
                            chw as u32,
                            chh as u32,
                            (radius * 4.0) as u32,
                            (shadow_radius * 4.0) as u32,
                            ((shadow_off_y * 4.0) as i32, (shadow_off_x * 4.0) as i32),
                            sc_packed,
                            (dpi_scale * 100.0) as u32,
                        );
                        // 【真 D2D 高斯阴影】用户两轮判多层近似「太锐利」——
                        // 换 D2D1Shadow 效果（系统级高斯模糊）：窗口形状画进
                        // command list → Shadow 效果 → DrawImage 回主画布。
                        let fx = (|| -> Option<()> {
                            unsafe {
                                if let Some((k, v)) = &shadow_cache_in {
                                    if *k == sh_key {
                                        // 命中：直接绘制缓存的 effect 输出
                                        let eff_img: ID2D1Image = v.1.cast().ok()?;
                                        // 【高DPI二次缩放修复 2026-09-11】
                                        // effect 输出=物理像素（command list 录制
                                        // 时几何已乘 dpi_scale）；此前 DrawImage
                                        // 在 dpi 世界变换下画 → 高 DPI 屏内容再
                                        // 乘一次 scale：150% 屏阴影 ×2.25 倍位、
                                        // 右下偏移出窗（用户实测「阴影超大范围
                                        // 偏离」，1.4.8 引入 dpi 中心化后高 DPI
                                        // 机器必现、100% 屏 ×1 不可见——本机全
                                        // 100% 故历轮复现不了）。玻璃段 2026-09-08
                                        // 已修（identity+物理坐标），纯色段漏修。
                                        // 正序（对齐玻璃段）：identity → Push mask
                                        // （洞几何乘 dpi）→ DrawImage（物理偏移）
                                        // → 恢复 dpi 主变换 → Pop。
                                        ctx.SetTransform(
                                            &windows::Foundation::Numerics::Matrix3x2 {
                                                M11: 1.0,
                                                M12: 0.0,
                                                M21: 0.0,
                                                M22: 1.0,
                                                M31: 0.0,
                                                M32: 0.0,
                                            },
                                        );
                                        let mask_ok = push_shadow_mask(
                                            &ctx, chw, chh, cw_out, ch_out, shadow_m, radius,
                                            dpi_scale, bx, by,
                                        );
                                        // 【阴影分离修复·终版 2026-09-08】曾加
                                        // GetImageLocalBounds 补偿——实错：DrawImage
                                        // 的 targetOffset 对齐 image 坐标原点 (0,0)
                                        //（effect 输出继承 command list 坐标系，
                                        // 圆角矩形在 shadow_m 处），不是 bounds 左
                                        // 上——补偿把阴影平移出左上角（白底像素
                                        // 分析：浓阴影聚窗口左上）。原始分离根因
                                        // 是缓存键漏 shadow_radius（本版已在键中）。
                                        // identity 下偏移同样须物理（×dpi）。
                                        let off = D2D_POINT_2F {
                                            x: shadow_off_x * dpi_scale,
                                            y: shadow_off_y * dpi_scale,
                                        };
                                        ctx.DrawImage(
                                            &eff_img,
                                            Some(&off as *const _),
                                            None,
                                            D2D1_INTERPOLATION_MODE_LINEAR,
                                            D2D1_COMPOSITE_MODE_SOURCE_OVER,
                                        );
                                        ctx.SetTransform(
                                            &windows::Foundation::Numerics::Matrix3x2 {
                                                M11: dpi_scale,
                                                M12: 0.0,
                                                M21: 0.0,
                                                M22: dpi_scale,
                                                M31: 0.0,
                                                M32: 0.0,
                                            },
                                        );
                                        if mask_ok {
                                            ctx.PopLayer();
                                        }
                                        return Some(());
                                    }
                                }
                                let cl = ctx.CreateCommandList().ok()?;
                                let saved = ctx.GetTarget().ok();
                                let cl_img: ID2D1Image = cl.cast().ok()?;
                                ctx.SetTarget(Some(&cl_img));
                                let wb = ctx
                                    .CreateSolidColorBrush(
                                        &D2D1_COLOR_F {
                                            r: 0.0,
                                            g: 0.0,
                                            b: 0.0,
                                            a: 1.0,
                                        },
                                        None,
                                    )
                                    .ok()?;
                                let rr_win = D2D1_ROUNDED_RECT {
                                    rect: D2D_RECT_F {
                                        left: shadow_m + bx,
                                        top: shadow_m + by,
                                        right: shadow_m + bx + chw,
                                        bottom: shadow_m + by + chh,
                                    },
                                    radiusX: radius,
                                    radiusY: radius,
                                };
                                ctx.FillRoundedRectangle(&rr_win, &wb);
                                cl.Close().ok()?;
                                ctx.SetTarget(saved.as_ref());
                                let effect = ctx.CreateEffect(&CLSID_D2D1Shadow).ok()?;
                                let blur = shadow_radius * 0.5 + 1.0;
                                let _ = effect.SetValue(
                                    0,
                                    D2D1_PROPERTY_TYPE_FLOAT,
                                    &blur.to_ne_bytes(),
                                );
                                let col = D2D_VECTOR_4F {
                                    x: sc.r,
                                    y: sc.g,
                                    z: sc.b,
                                    w: sc.a,
                                };
                                let _ = effect.SetValue(
                                    1,
                                    D2D1_PROPERTY_TYPE_VECTOR4,
                                    &[
                                        col.x.to_ne_bytes(),
                                        col.y.to_ne_bytes(),
                                        col.z.to_ne_bytes(),
                                        col.w.to_ne_bytes(),
                                    ]
                                    .concat(),
                                );
                                let eff_img: ID2D1Image = effect.cast().ok()?;
                                effect.SetInput(0, &cl_img, true);
                                // 【圆角外遮罩】高斯向内弥散会进窗口内部；用
                                // 直角矩形清除会在圆角外留直角切割痕（用户实测
                                // 「直角色块」）。改 Layer 几何遮罩：整画布 −
                                // 窗口圆角（even-odd）——阴影只在窗外绘制。
                                // 【高DPI二次缩放修复 2026-09-11】同缓存命中
                                // 分支：identity → Push mask（洞×dpi）→
                                // DrawImage（物理偏移）→ 恢复 dpi → Pop。
                                ctx.SetTransform(&windows::Foundation::Numerics::Matrix3x2 {
                                    M11: 1.0,
                                    M12: 0.0,
                                    M21: 0.0,
                                    M22: 1.0,
                                    M31: 0.0,
                                    M32: 0.0,
                                });
                                let mask_ok = push_shadow_mask(
                                    &ctx, chw, chh, cw_out, ch_out, shadow_m, radius, dpi_scale,
                                    bx, by,
                                );
                                // 【阴影分离修复·终版】同缓存命中分支：无补偿
                                // 原语义（详见上方注释）。
                                let off = D2D_POINT_2F {
                                    x: shadow_off_x * dpi_scale,
                                    y: shadow_off_y * dpi_scale,
                                };
                                ctx.DrawImage(
                                    &eff_img,
                                    Some(&off as *const _),
                                    None,
                                    D2D1_INTERPOLATION_MODE_LINEAR,
                                    D2D1_COMPOSITE_MODE_SOURCE_OVER,
                                );
                                ctx.SetTransform(&windows::Foundation::Numerics::Matrix3x2 {
                                    M11: dpi_scale,
                                    M12: 0.0,
                                    M21: 0.0,
                                    M22: dpi_scale,
                                    M31: 0.0,
                                    M32: 0.0,
                                });
                                if mask_ok {
                                    ctx.PopLayer();
                                }
                                // 【每帧缓存】存 command list+effect（键内含
                                // w/h/圆角/偏移/颜色，任一变即重建）
                                shadow_cache_out = Some((sh_key, (cl, effect)));
                                Some(())
                            }
                        })();
                        // 兜底：效果路径失败（老驱动）→ 下方环带多层近似
                        if fx.is_none() {
                            // 【阴影诊断 2026-09-08】用户实测「辐射状阴影+与候选分离」
                            // ——症状指向本兜底分支（环带多层）。记录失败原因到 trace。
                            unsafe {
                                let line = format!(
                                    "[shadow] D2D effect 失败→环带兜底 r={} w={} h={} m={:.1}\n",
                                    shadow_radius, w, h, shadow_m
                                );
                                if let Ok(mut f) = std::fs::OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open(std::env::temp_dir().join("hufu-tsf-trace.log"))
                                {
                                    use std::io::Write;
                                    let _ = f.write_all(line.as_bytes());
                                }
                            }
                            if let Ok(b) = ctx.CreateSolidColorBrush(&sc, None) {
                                // 【环带阴影】旧实现多层实心圆角矩形「内浓外淡」
                                // 依赖不透明窗底盖住内圈——窗底全透明时阴影盖满
                                // 整窗（用户实测 bug）。改 even-odd 几何环带：每层
                                // 只画「外圈 − 窗口」的环，窗口内部永远无阴影。
                                let factory = ctx.GetFactory().ok();
                                // 模糊感：层数多 + 高斯衰减（exp(-kt²)）——层间
                                // 台阶不可见，观感≈CSS box-shadow 的高斯模糊。
                                const PASSES: usize = 28;
                                // 窗口自身圆角矩形（环带的内边界）
                                let win_geom = factory.as_ref().and_then(|f| {
                                    let rr = D2D1_ROUNDED_RECT {
                                        rect: D2D_RECT_F {
                                            left: shadow_m + bx,
                                            top: shadow_m + by,
                                            right: shadow_m + bx + chw,
                                            bottom: shadow_m + by + chh,
                                        },
                                        radiusX: radius,
                                        radiusY: radius,
                                    };
                                    f.CreateRoundedRectangleGeometry(&rr).ok()
                                });
                                for i in (1..=PASSES).rev() {
                                    let t = i as f32 / PASSES as f32; // 外圈 t=1 → 内圈趋 0
                                    let grow = shadow_radius * t;
                                    // 高斯衰减：贴边最浓向外平滑消散（旧 (1-t)²
                                    // 台阶感强——「阴影太锐利」的根因）
                                    let a = sc.a * (-4.5 * t * t).exp();
                                    b.SetColor(&D2D1_COLOR_F {
                                        r: sc.r,
                                        g: sc.g,
                                        b: sc.b,
                                        a,
                                    });
                                    let rr = D2D1_ROUNDED_RECT {
                                        rect: D2D_RECT_F {
                                            left: shadow_m + bx - grow + shadow_off_x * t,
                                            top: shadow_m + by - grow + shadow_off_y * t,
                                            right: shadow_m + bx + chw + grow + shadow_off_x * t,
                                            bottom: shadow_m + by + chh + grow + shadow_off_y * t,
                                        },
                                        radiusX: radius + grow,
                                        radiusY: radius + grow,
                                    };
                                    // 每层 = 外圈几何 − 窗口几何 的 even-odd 环带
                                    //（窗口内部永远无阴影；全透明窗只剩轮廓外投影）
                                    let ring = (|| -> Option<()> {
                                        let f = factory.as_ref()?;
                                        let wg = win_geom.as_ref()?;
                                        let outer: Option<
                                            windows::Win32::Graphics::Direct2D::ID2D1Geometry,
                                        > = f
                                            .CreateRoundedRectangleGeometry(&rr)
                                            .ok()
                                            .and_then(|g| g.cast().ok());
                                        let outer = outer?;
                                        let inner: windows::Win32::Graphics::Direct2D::ID2D1Geometry =
                                        wg.clone().cast().ok()?;
                                        let grp = f
                                            .CreateGeometryGroup(
                                                D2D1_FILL_MODE_ALTERNATE,
                                                &[Some(outer), Some(inner)],
                                            )
                                            .ok()?;
                                        ctx.FillGeometry(&grp, &b, None);
                                        Some(())
                                    })();
                                    if ring.is_none() {
                                        // 几何路径失败兜底：退回实心（旧行为）
                                        ctx.FillRoundedRectangle(&rr, &b);
                                    }
                                }
                            }
                        } // fx.is_none() 环带兜底结束
                    }
                }
                // 内容整体平移进阴影边距内（此后所有内容坐标不变）；
                // 矩阵 = 缩放（高 DPI）× 平移（shadow_m 物理 px）
                ctx.SetTransform(&windows::Foundation::Numerics::Matrix3x2 {
                    M11: dpi_scale,
                    M12: 0.0,
                    M21: 0.0,
                    M22: dpi_scale,
                    M31: shadow_m * dpi_scale,
                    M32: shadow_m * dpi_scale,
                });
                // 【整体透明度】非文字元素的总乘法系数（块外供共用）
                let master = skin
                    .pointer("/skin/material/master_alpha")
                    .or_else(|| skin.get("material").and_then(|m| m.get("master_alpha")))
                    .and_then(|x| x.as_f64())
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0) as f32;
                // 【高亮透明度】高亮候选底独立系数
                let hilite_a = skin
                    .pointer("/skin/material/hilite_alpha")
                    .or_else(|| skin.get("material").and_then(|m| m.get("hilite_alpha")))
                    .and_then(|x| x.as_f64())
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0) as f32;
                {
                    // 【纯色模型 v2·用户定稿】颜色只管色相（alpha 分量忽略）：
                    // 窗底/边框/编码底 alpha = master；高亮底 = hilite_a；文字恒 1。
                    let back = color_f(skin, "back_color", "#202022E6");
                    let bg_c = D2D1_COLOR_F {
                        r: back.r,
                        g: back.g,
                        b: back.b,
                        a: if stage == 1 { 0.0 } else { master },
                    };
                    if bg_c.a > 0.004 {
                        if let Ok(b) = ctx.CreateSolidColorBrush(&bg_c, None) {
                            let rr = D2D1_ROUNDED_RECT {
                                rect: D2D_RECT_F {
                                    left: bx,
                                    top: by,
                                    right: bx + chw,
                                    bottom: by + chh,
                                },
                                radiusX: radius,
                                radiusY: radius,
                            };
                            ctx.FillRoundedRectangle(&rr, &b);
                        }
                    }
                    // 【纯色模型】暗化层已废弃（材质系统移除）——不画
                }
                // 【拉伸动效】内容裁剪：动效帧中外壳小于目标布局——把
                // 编码行/候选/注释裁在外壳内（增长=渐进露出，收拢=渐进
                // 收起）；稳态帧不推（零开销）。
                // 【圆角裁剪 2026-09-11】用户实测慢速（200%+速度）下延伸
                // 过程中「边框/阴影是直角」：面板/边框/阴影几何全程圆，
                // 直角来自此处方形 Clip 把贴边元素（编码行底、高亮胶囊）
                // 切出直边——改 PushLayer+圆角几何遮罩（与外壳同 radius），
                // 动画中内容缘也随圆角收边，完成帧恢复全圆。
                let chrome_clip_on = self.chrome_override.get().is_some();
                if chrome_clip_on {
                    unsafe {
                        let clip_rect = D2D_RECT_F {
                            left: bx,
                            top: by,
                            right: bx + chw,
                            bottom: by + chh,
                        };
                        // 圆角几何遮罩（与外壳同 radius；PushLayer 内部
                        // AddRef，局部几何可随作用域释放）
                        let geom: Option<windows::Win32::Graphics::Direct2D::ID2D1Geometry> =
                            ctx.GetFactory().ok().and_then(|f| {
                                f.CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
                                    rect: clip_rect,
                                    radiusX: radius,
                                    radiusY: radius,
                                })
                                .ok()
                                .and_then(|g| g.cast().ok())
                            });
                        match geom {
                            Some(g) => {
                                let mut lp = D2D1_LAYER_PARAMETERS1::default();
                                lp.contentBounds = clip_rect;
                                lp.geometricMask = std::mem::ManuallyDrop::new(Some(g));
                                lp.maskAntialiasMode = D2D1_ANTIALIAS_MODE_PER_PRIMITIVE;
                                // 【maskTransform 必须显式单位阵 2026-09-11】
                                // default() 的零矩阵在部分驱动上把遮罩塌缩
                                // 成点 → 层内内容整帧不画=「文字闪」（壳在
                                // 层外不闪）且圆角失效退直角。对齐
                                // push_shadow_mask 的显式单位阵写法。
                                lp.maskTransform = windows::Foundation::Numerics::Matrix3x2 {
                                    M11: 1.0,
                                    M12: 0.0,
                                    M21: 0.0,
                                    M22: 1.0,
                                    M31: 0.0,
                                    M32: 0.0,
                                };
                                ctx.PushLayer(&lp, None);
                            }
                            None => {
                                // 几何创建失败兜底：无 mask 的层=方形
                                // contentBounds 裁剪（与旧 Clip 等效）——
                                // Push/Pop 两侧统一走 Layer，配对无忧
                                let mut lp = D2D1_LAYER_PARAMETERS1::default();
                                lp.contentBounds = clip_rect;
                                ctx.PushLayer(&lp, None);
                            }
                        }
                    }
                }

                // 【纯色模型 v2】非文字元素 alpha = master（颜色自带 a 忽略）；
                // 高亮底 alpha = hilite_a；文字画刷 alpha 恒 1.0。
                let elem_alpha = |mut c: D2D1_COLOR_F| {
                    c.a = master;
                    c
                };
                let text_alpha = |mut c: D2D1_COLOR_F| {
                    c.a = 1.0;
                    c
                };
                let mkbrush =
                    |ctx: &ID2D1DeviceContext, c: D2D1_COLOR_F| -> Option<ID2D1SolidColorBrush> {
                        ctx.CreateSolidColorBrush(&c, None).ok()
                    };
                let b_text = mkbrush(
                    &ctx,
                    text_alpha(color_f(skin, "candidate_text_color", "#E8E8EAFF")),
                );
                let b_label = mkbrush(&ctx, text_alpha(color_f(skin, "label_color", "#C9C9C9FF")));
                let b_raw = mkbrush(
                    &ctx,
                    text_alpha(color_f(skin, "hilited_text_color", "#E8E8EAFF")),
                );
                // 编码区背景（preedit_back_color；alpha=0 的皮肤不画）
                let b_preedit_bg = {
                    let c = elem_alpha(color_f(skin, "preedit_back_color", "#00000000"));
                    (c.a > 0.01).then(|| mkbrush(&ctx, c)).flatten()
                };
                let b_cmt = mkbrush(
                    &ctx,
                    text_alpha(color_f(skin, "comment_text_color", "#9A9AA0FF")),
                );
                let b_hi = mkbrush(&ctx, {
                    let mut c = color_f(skin, "hilited_candidate_back_color", "#404046FF");
                    c.a = hilite_a;
                    c
                });
                let b_hi_txt = mkbrush(
                    &ctx,
                    text_alpha(color_f(skin, "hilited_candidate_text_color", "#FFFFFFFF")),
                );
                let b_hi_lbl = mkbrush(
                    &ctx,
                    text_alpha(color_f(skin, "hilited_candidate_label_color", "#FFD75EFF")),
                );
                let b_hi_cmt = mkbrush(
                    &ctx,
                    text_alpha(color_f(skin, "hilited_comment_text_color", "#C9C9C9FF")),
                );
                let b_border = mkbrush(&ctx, {
                    // 【边框透明度】material.border_alpha 独立滑条（颜色自带 a 忽略）
                    let border_alpha = skin
                        .pointer("/skin/material/border_alpha")
                        .or_else(|| skin.get("material").and_then(|m| m.get("border_alpha")))
                        .and_then(|x| x.as_f64())
                        .unwrap_or(1.0)
                        .clamp(0.0, 1.0) as f32;
                    let mut c = color_f(skin, "border_color", "#FFFFFF26");
                    c.a = border_alpha;
                    c
                });

                let draw = |ctx: &ID2D1DeviceContext,
                            tf: &Option<IDWriteTextFormat>,
                            s: &str,
                            x: f32,
                            y: f32,
                            w: f32,
                            h: f32,
                            brush: &Option<ID2D1SolidColorBrush>| {
                    if let (Some(tf), Some(brush)) = (tf, brush) {
                        let ws: Vec<u16> = s.encode_utf16().collect();
                        if ws.is_empty() {
                            return;
                        }
                        let rect = D2D_RECT_F {
                            left: x,
                            top: y,
                            right: x + w,
                            bottom: y + h,
                        };
                        let _ = ctx.DrawText(
                            &ws,
                            tf,
                            &rect,
                            brush,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                };

                // 【阶段验证】stage 1-4：跳过内容绘制（编码行/胶囊/文字/边框）
                let draw_content = !(stage >= 1 && stage <= 4);
                if draw_content {
                    // 编码行（有内容才画；候选行相应下移一行 + 行距）；dy=光学垂直居中位移
                    if !raw.is_empty() {
                        // 编码区背景（皮肤 preedit_back_color 带透明度时才画）
                        if let Some(bg) = &b_preedit_bg {
                            let rr = D2D1_ROUNDED_RECT {
                                rect: D2D_RECT_F {
                                    left: rm_x,
                                    top: rm_y,
                                    right: width - rm_x,
                                    bottom: rm_y + line_h,
                                },
                                radiusX: 4.0,
                                radiusY: 4.0,
                            };
                            unsafe {
                                ctx.FillRoundedRectangle(&rr, bg);
                            }
                        }
                        draw(
                            &ctx,
                            &tf,
                            raw.as_str(),
                            rm_x,
                            rm_y + dy,
                            width - rm_x * 2.0,
                            row_h,
                            &b_raw,
                        );
                    }
                    // 【对齐修正 2026-09-06】dy（文本光学居中）此前平移整行（含
                    // 高亮胶囊/窗边距）——窗顶与窗底到高亮区的间隙差 ±dy（用户
                    // 实测「外框与高亮区上下距离不一样」）。现 dy 只作用于文本
                    // draw（行内光学居中），行框/胶囊/窗框几何全部按对称 margin
                    // 布置。
                    let y0 = rm_y + (row_h + cand_spacing) * code_row;

                    // 【口径重构 2026-09-08】胶囊几何只有两个属性：gap（胶囊↔
                    // 窗边，四边同值）与 hilite_pad（胶囊↔文字，四边同值）。
                    // 行槽 row_h 已在测量段撑高到能装下胶囊（max(line_h,
                    // em+2hp)）——胶囊在行槽内垂直居中即四边= gap；文字在槽
                    // 内由 DWrite 布局居中。旧版「放不下时溢出/对齐修正」的
                    // 补丁链全部废除。
                    let pill_v = |y: f32| -> (f32, f32) {
                        let off = (row_h - pill_h) / 2.0;
                        (y + off, y + off + pill_h)
                    };

                    // 候选行
                    let sel = selected.min(cands.len().saturating_sub(1));
                    if horizontal {
                        // ── 横排：单行铺开，每格 = 序号+文本(+注释)，高亮为整格胶囊 ──
                        // 编码段在左（同行）：候选起点右移 raw_w+间隔（2026-09-05）
                        let mut x =
                            rm_x + hilite_pad + if raw_w > 0.0 { raw_w + 10.0 } else { 0.0 };
                        let y = y0;
                        for (i, (text, _)) in cands.iter().enumerate().take(10) {
                            let cmt: &str = cmt_disp.get(i).map(|s| s.as_str()).unwrap_or("");
                            let (tw, cw, iw) = cand_ws.get(i).copied().unwrap_or((0.0, 0.0, 0.0));
                            let cell_w = iw + tw + if cw > 0.0 { hsp + cw } else { 0.0 };
                            if i > 0 {
                                x += cand_spacing;
                            }
                            if i == sel {
                                // 【高亮锚定】横排：捕获高亮胶囊中心（入场
                                // 动画盒以此为锚）
                                self.hl_center
                                    .set(Some((x + cell_w * 0.5, y + row_h * 0.5)));
                                if let Some(b) = &b_hi {
                                    let (pt, pb) = pill_v(y);
                                    let rr = D2D1_ROUNDED_RECT {
                                        rect: D2D_RECT_F {
                                            left: x - hilite_pad,
                                            top: pt,
                                            right: x + cell_w + hilite_pad,
                                            bottom: pb,
                                        },
                                        radiusX: layout_f(skin, "hilited_corner_radius", radius),
                                        radiusY: layout_f(skin, "hilited_corner_radius", radius),
                                    };
                                    ctx.FillRoundedRectangle(&rr, b);
                                    // mark_text：高亮胶囊左缘内侧细竖条（weasel 语义）
                                    if mark_en {
                                        let mw = 2.0f32.min(hilite_pad);
                                        let my = y + row_h * 0.2;
                                        let mh = row_h * 0.6;
                                        let mrr = D2D1_ROUNDED_RECT {
                                            rect: D2D_RECT_F {
                                                left: x - hilite_pad + (hilite_pad - mw) / 2.0,
                                                top: my,
                                                right: x - hilite_pad
                                                    + (hilite_pad - mw) / 2.0
                                                    + mw,
                                                bottom: my + mh,
                                            },
                                            radiusX: 1.0,
                                            radiusY: 1.0,
                                        };
                                        let mb = b_hi_lbl
                                            .as_ref()
                                            .or_else(|| b_hi_txt.as_ref())
                                            .unwrap_or(b);
                                        ctx.FillRoundedRectangle(&mrr, mb);
                                    }
                                }
                            }
                            let (bt, bl, bc) = if i == sel {
                                (&b_hi_txt, &b_hi_lbl, &b_hi_cmt)
                            } else {
                                (&b_text, &b_label, &b_cmt)
                            };
                            let mut cx = x;
                            if show_index {
                                draw(
                                    &ctx,
                                    &tf_label,
                                    &fmt_label(i + 1),
                                    cx,
                                    y + dy,
                                    iw,
                                    row_h,
                                    bl,
                                );
                                cx += iw;
                            }
                            draw(&ctx, &tf, text, cx, y + dy, tw + 2.0, row_h, bt);
                            cx += tw;
                            if !cmt.is_empty() && cw > 0.0 {
                                draw(&ctx, &tf_small, cmt, cx + hsp, y + dy, cw + 2.0, row_h, bc);
                            }
                            x += cell_w;
                        }
                    } else {
                        // ── 竖排（原布局 + candidate_spacing 行距 + hilite_padding 统一内边距）──
                        for (i, (text, _)) in cands.iter().enumerate().take(10) {
                            let cmt: &str = cmt_disp.get(i).map(|s| s.as_str()).unwrap_or("");
                            let y = y0 + (row_h + cand_spacing) * i as f32;
                            if i == sel {
                                // 高亮行（圆角胶囊；↑↓ 移动）：胶囊四边 = gap（口径
                                // 统一 2026-09-08——不再 ±hilite_pad 外扩，文字列
                                // 已在胶囊内 gap+hp 起）
                                // 【高亮锚定】竖排：捕获胶囊中心（入场动画盒锚点）
                                self.hl_center.set(Some((width * 0.5, y + row_h * 0.5)));
                                if let Some(b) = &b_hi {
                                    let (pt, pb) = pill_v(y);
                                    let rr = D2D1_ROUNDED_RECT {
                                        rect: D2D_RECT_F {
                                            left: rm_x,
                                            top: pt,
                                            right: width - rm_x,
                                            bottom: pb,
                                        },
                                        radiusX: layout_f(skin, "hilited_corner_radius", radius),
                                        radiusY: layout_f(skin, "hilited_corner_radius", radius),
                                    };
                                    ctx.FillRoundedRectangle(&rr, b);
                                    // mark_text：高亮胶囊左缘内侧细竖条（weasel 语义）
                                    if mark_en {
                                        let mw = 2.0f32.min(hilite_pad);
                                        let my = y + row_h * 0.2;
                                        let mh = row_h * 0.6;
                                        let mrr = D2D1_ROUNDED_RECT {
                                            rect: D2D_RECT_F {
                                                left: rm_x + (hilite_pad - mw) / 2.0,
                                                top: my,
                                                right: rm_x + (hilite_pad - mw) / 2.0 + mw,
                                                bottom: my + mh,
                                            },
                                            radiusX: 1.0,
                                            radiusY: 1.0,
                                        };
                                        let mb = b_hi_lbl
                                            .as_ref()
                                            .or_else(|| b_hi_txt.as_ref())
                                            .unwrap_or(b);
                                        ctx.FillRoundedRectangle(&mrr, mb);
                                    }
                                }
                            }
                            let (bt, bl, bc) = if i == sel {
                                (&b_hi_txt, &b_hi_lbl, &b_hi_cmt)
                            } else {
                                (&b_text, &b_label, &b_cmt)
                            };
                            if show_index {
                                draw(
                                    &ctx,
                                    &tf_label,
                                    &fmt_label(i + 1),
                                    rm_x + hilite_pad,
                                    y + dy,
                                    label_w,
                                    row_h,
                                    bl,
                                );
                            }
                            draw(
                                &ctx,
                                &tf,
                                text,
                                text_x,
                                y + dy,
                                cmt_x - text_x - 4.0,
                                row_h,
                                bt,
                            );
                            if !cmt.is_empty() {
                                draw(
                                    &ctx,
                                    &tf_small,
                                    cmt,
                                    cmt_x,
                                    y + dy,
                                    width - cmt_x - rm_x + 4.0,
                                    row_h,
                                    bc,
                                );
                            }
                        }
                    }

                    // 边框（v3.7 描边方案用户否决已撤；属外壳：画在动画
                    // 盒缘 +bx/+by）
                    if let Some(b) = &b_border {
                        let bw = layout_f(skin, "border_width", 1.0);
                        let rr = D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: bx + bw / 2.0,
                                top: by + bw / 2.0,
                                right: bx + chw - bw / 2.0,
                                bottom: by + chh - bw / 2.0,
                            },
                            radiusX: radius,
                            radiusY: radius,
                        };
                        let _ = ctx.DrawRoundedRectangle(&rr, b, bw, None);
                    }
                } // draw_content
                  // 【拉伸动效】内容裁剪收层（与上方 Push 配对；圆角遮罩
                  // 与方形兜底都走 Layer——Pop 恒为 PopLayer）
                if chrome_clip_on {
                    unsafe {
                        ctx.PopLayer();
                    }
                }

                // 【动效 2026-09-11】过渡帧收层（与 BeginDraw 后的 PushLayer
                // 配对；稳态未 Push 不 Pop）
                if fade_layer_on {
                    ctx.PopLayer();
                }
                let _ = ctx.EndDraw(None, None);
                ctx.SetTarget(None);

                // 测试回读：EndDraw 后目标位图已非活动，拷到 CPU 位图取整帧 BGRA
                if self.readback {
                    use windows::Win32::Graphics::Direct2D::{
                        D2D1_BITMAP_OPTIONS, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                        D2D1_BITMAP_OPTIONS_CPU_READ,
                    };
                    // 回读整窗（含阴影边距），与 SetWindowPos 尺寸一致
                    let (w_px, h_px) = (w_out, h_out);
                    if w_px > 0 && h_px > 0 {
                        let props = windows::Win32::Graphics::Direct2D::D2D1_BITMAP_PROPERTIES1 {
                        pixelFormat: windows::Win32::Graphics::Direct2D::Common::D2D1_PIXEL_FORMAT {
                            format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: windows::Win32::Graphics::Direct2D::Common::D2D1_ALPHA_MODE_PREMULTIPLIED,
                        },
                        dpiX: 96.0,
                        dpiY: 96.0,
                        bitmapOptions: D2D1_BITMAP_OPTIONS(D2D1_BITMAP_OPTIONS_CPU_READ.0 | D2D1_BITMAP_OPTIONS_CANNOT_DRAW.0),
                        ..Default::default()
                    };
                        let _ = D2D1_BITMAP_OPTIONS::default();
                        if let Ok(cpu) = ctx.CreateBitmap(
                            windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_U {
                                width: w_px,
                                height: h_px,
                            },
                            None,
                            0,
                            &props,
                        ) {
                            unsafe {
                                if let Ok(bmp0) = bitmap.cast::<ID2D1Bitmap>() {
                                    if cpu
                                    .CopyFromBitmap(
                                        None,
                                        Some(&bmp0),
                                        Some(&windows::Win32::Graphics::Direct2D::Common::D2D_RECT_U {
                                            left: 0,
                                            top: 0,
                                            right: w_px,
                                            bottom: h_px,
                                        }),
                                    )
                                    .is_ok()
                                {
                                    if let Ok(mapped) = cpu.Map(
                                        windows::Win32::Graphics::Direct2D::D2D1_MAP_OPTIONS_READ,
                                    ) {
                                        let mut data = vec![0u8; (w_px * h_px * 4) as usize];
                                        let pitch = mapped.pitch as usize;
                                        for row in 0..h_px as usize {
                                            let src = mapped.bits.add(row * pitch) as *const u8;
                                            data[row * (w_px as usize) * 4..(row + 1) * (w_px as usize) * 4]
                                                .copy_from_slice(std::slice::from_raw_parts(src, (w_px * 4) as usize));
                                        }
                                        let _ = cpu.Unmap();
                                        self.last_pixels = Some(data);
                                        self.last_dy = Some(dy);
                                        self.last_size = (w_px, h_px);
                                    }
                                }
                                }
                            }
                        }
                    }
                }

                let hr = chain.Present(1, DXGI_PRESENT(0));
                if hr.is_err() {
                    crate::tsf::trace(&format!("cw2: Present 失败 0x{:08X}", hr.0 as u32));
                }
            }
        } // 'sizedraw 结束（收缩动效延迟渲染时整段跳过）

        // 定位：优先插入点下方，出屏翻到上方；锚点丢失沿用上次位置。
        // **组段内单调过滤**（跟打器类异步布局应用的跳动终结者）：
        // 正向打字（编码不减）时光标只应右移/不动——x 拒绝回退值
        //（旧布局查询结果比当前光标靠左）、y 锁定到「换行级」变化
        //（>26px 才认）——上下逐键摆动在构造上不可能发生。退格/新
        // 组段（编码变短）放行全部变化。
        unsafe {
            // 虚拟屏幕坐标系（多显示器安全）：主屏 SM_CXSCREEN 会把
            // 副屏负坐标错误钳回主屏
            let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            let grew = raw.len() >= self.last_raw_len;
            self.last_raw_len = raw.len();
            // 拖拽松手交接：一次性消费（设 sticky 并标记组段级钉住——
            // 本组段留在松手处，hide 时解除）。
            // 【坐标系统一 2026-09-08】DROP_AT 来自 wndproc 的
            // GetWindowRect = 窗口原点（锚点 − shadow_m 外扩）；
            // sticky_pos 本帧坐标系 = 内容锚点系（SetWindowPos 统一
            // 减 shadow_m）。不转换则松手/锁定后窗口往左上偏一个
            // 阴影边距（用户实测「锁定时有点跳动」）。
            let m_off = (shadow_m * dpi_scale) as i32;
            // 【毛玻璃诊断钩子 2026-09-08】pin.txt 存在 → 固定到其中
            // 坐标（"x,y"，窗口原点系）。自动化验证用（把窗口钉在屏幕
            // 中央壁纸上肉眼看模糊），删文件即恢复正常。
            {
                let mut pinned = CAND_PINNED.lock().unwrap_or_else(|e| e.into_inner());
                if pinned.is_none() {
                    if let Ok(s) = std::fs::read_to_string(r"C:\ProgramData\HuFu\diag\pin.txt") {
                        let t = s.trim();
                        if let Some((a, b)) = t.split_once(',') {
                            if let (Ok(px), Ok(py)) =
                                (a.trim().parse::<i32>(), b.trim().parse::<i32>())
                            {
                                *pinned = Some((px, py));
                                crate::tsf::diag_note(&format!("cw2 pin.txt 钩子 ({px},{py})"));
                            }
                        }
                    }
                }
            }
            if let Some(p) = CAND_DROP_AT
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                self.sticky_pos = Some((p.0 + m_off, p.1 + m_off));
                self.sticky_drag = true;
            }
            // 【右键解锁 2026-09-10】清除拖拽钉住残留（sticky 是实例
            // 字段、wndproc 摸不到 self——经全局标记转交：右键置位，
            // 下一帧 show 消费），否则解除固定后本组段仍钉旧位置不回
            // 跟随光标。
            {
                let mut un = CAND_UNSTICK.lock().unwrap_or_else(|e| e.into_inner());
                if *un {
                    *un = false;
                    self.sticky_pos = None;
                    self.sticky_drag = false;
                }
            }
            let (x, y) = if let Some((px, py)) =
                *CAND_PINNED.lock().unwrap_or_else(|e| e.into_inner())
            {
                // 【固定模式】右键固定：忽略光标锚点，钉在用户固定处
                //（跨组段/上屏/新一轮候选全部保持；右键再解除）。
                // 拖动松手会回写 pin（见 WM_LBUTTONUP）——打字必然用
                // 最新固定位。pin 同为窗口原点系：+m_off 转回锚点系。
                crate::tsf::diag_note(&format!("cw2 pin use ({px},{py})"));
                let x = (px + m_off).clamp(vx, (vx + vw - width as i32).max(vx));
                let y = (py + m_off).clamp(vy, (vy + vh - height as i32).max(vy));
                (x, y)
            } else if self.sticky_drag && self.sticky_pos.is_some() {
                // 【拖拽钉住】松手设的 sticky 优先于锚点：本组段内
                // 窗口钉在松手处不回弹（clamp 防出屏）
                let (ox, oy) = self.sticky_pos.unwrap();
                let x = ox.clamp(vx, (vx + vw - width as i32).max(vx));
                let y = oy.clamp(vy, (vy + vh - height as i32).max(vy));
                (x, y)
            } else {
                match anchor {
                    Some(r) => {
                        let x = (r.left).clamp(vx, (vx + vw - width as i32).max(vx));
                        let below = r.bottom + 4;
                        let y = if below + height as i32 <= vy + vh {
                            below
                        } else {
                            (r.top - height as i32 - 4).max(vy)
                        };
                        match self.sticky_pos {
                            Some((ox, oy)) => {
                                // 软换行判定：x 想回退（<旧行尾）且 y 发生换行级
                                // 变化（>26px 行高阈值）同时成立 = 新行开始——
                                // x 回到新行行首是合法回退，禁令解除。否则单调锁
                                // 会把换行后的 X 钉死在旧行尾（实测虎魄：换行
                                // x 2361→1625 被拒，候选框只上下动、不横向跟到
                                // 新行打字点）。仅 y 超阈值不构成豁免——跟打器
                                // 滚动步进可达 29px，x 正常增长帧不得误放行。
                                let line_broke = x < ox - 2 && (y - oy).abs() > 26;
                                // x：正向打字拒绝回退（旧布局值）；软换行除外
                                let x = if grew && !line_broke && x < ox - 2 {
                                    ox
                                } else {
                                    x
                                };
                                // y：正向打字只认换行级变化（行高 ~29px，阈值 26）
                                let y = if grew && (y - oy).abs() <= 26 { oy } else { y };
                                // 2px 迟滞：亚像素取整误差/回流微动不搬窗
                                if (x - ox).abs() <= 2 && (y - oy).abs() <= 2 {
                                    (ox, oy)
                                } else {
                                    (x, y)
                                }
                            }
                            None => (x, y),
                        }
                    }
                    None => match self.sticky_pos {
                        Some(p) => p,
                        // 从未有过真实锚点且本帧也取不到：先记诊断；若无
                        // 历史位置则退到「焦点窗口内左下」而非整帧隐藏
                        //（SearchHost 等宿主 GetTextExt 常失败——搜索框候选
                        // 框不显示的病根）。下一帧锚点就绪即回到正常定位。
                        None => {
                            crate::tsf::diag_note("cw2 anchor+sticky 双缺，退到焦点窗口定位");
                            let fg = GetForegroundWindow();
                            if fg.0.is_null() {
                                let _ = ShowWindow(self.hwnd, SW_HIDE);
                                return;
                            }
                            let mut fr = RECT {
                                left: 0,
                                top: 0,
                                right: 0,
                                bottom: 0,
                            };
                            let _ = GetWindowRect(fg, &mut fr);
                            let x = fr.left + 16;
                            let below = fr.bottom - ((height as i32) * 2).min(fr.bottom - fr.top);
                            (x, below.max(fr.top))
                        }
                    },
                }
            };
            // 【右缘兜底】正向打字的 x 单调锁（宽度增长时拒回退）会把
            // 已 clamp 的新 x 顶回旧位置——窗口变宽后旧 x+新宽超右缘
            //（用户实测：跟打器超长句候选框超出屏幕；宽度封顶后根因
            // 转到这里）。每帧输出前统一夹回，屏幕边界优先于位置记忆。
            let x = x.clamp(vx, (vx + vw - width as i32 - shadow_m as i32).max(vx));
            let y = y.clamp(vy, (vy + vh - height as i32).max(vy));
            self.sticky_pos = Some((x, y));
            // 诊断：搜索框等宿主锚点缺失排查（visible=0 说明本帧被隐藏）
            // + DWM cloaked 检测（显示中但被 DWM 隐身 → 连续 2 帧后
            //   由调用方切换 v1 传统混合窗——SearchHost 里 DComp 直通
            //   窗被整体 cloaked 的自愈路径）。dwmapi 经 GetProcAddress
            //   动态获取（mingw 工具链无 dwmapi 导入库）。
            let mut cloaked: u32 = 0;
            let mut hr: i32 = -1;
            unsafe {
                #[link(name = "kernel32")]
                unsafe extern "system" {
                    fn GetModuleHandleW(name: *const u16) -> isize;
                    fn GetProcAddress(module: isize, name: *const u8) -> *const core::ffi::c_void;
                }
                // 【i386 ABI】必须 extern "system"（stdcall）：x64 上 Rust
                // 默认约定与 Win64 恰好兼容掩盖了此错，32 位下 cdecl 调用
                // stdcall 函数 → 栈清理错位 → 崩（Pain 打器按键闪退根因）。
                type Dwma =
                    unsafe extern "system" fn(HWND, u32, *mut core::ffi::c_void, u32) -> i32;
                let mn: Vec<u16> = "dwmapi.dll\0".encode_utf16().collect();
                let m = GetModuleHandleW(mn.as_ptr());
                if m != 0 {
                    let p = GetProcAddress(m, c"DwmGetWindowAttribute".as_ptr() as *const u8);
                    if !p.is_null() {
                        let f: Dwma = std::mem::transmute(p);
                        hr = f(
                            self.hwnd,
                            14, // DWMWA_CLOAKED
                            &mut cloaked as *mut u32 as *mut core::ffi::c_void,
                            4,
                        );
                    }
                }
            }
            if cloaked != 0 {
                self.cloaked_streak += 1;
            } else {
                self.cloaked_streak = 0;
            }
            crate::tsf::diag_note(&format!(
                "cw2 layout dbg: font_pt={font_pt} em={em} line_h={line_h} horiz={horizontal} \
                 cands={} rawlen={} max_text={} width={width} height={height} w_out={w_out} h_out={h_out}",
                cands.len(),
                raw.chars().count(),
                cand_ws
                    .iter()
                    .map(|(tw, _, _)| *tw)
                    .fold(0.0f32, f32::max)
            ));
            crate::tsf::diag_note(&format!(
                "cw2 show anchor={} x={} y={} w={} h={} vis={} cloak={}({:#x}) hr={:#x} streak={}",
                anchor.is_some(),
                x,
                y,
                w_out,
                h_out,
                IsWindowVisible(self.hwnd).0,
                cloaked,
                cloaked,
                hr,
                self.cloaked_streak
            ));
            // 【每帧缓存】块后存回（测量/渲染块内 self 有借用）
            if let Some(v) = tf_cache_out {
                self.tf_cache = Some(v);
            }
            if let Some(v) = dy_cache_out {
                self.dy_cache = Some(v);
            }
            if let Some(v) = shadow_cache_out {
                self.shadow_cache = Some(v);
            }
            // 内容坐标 → 窗口坐标（内容在阴影边距内侧；高 DPI 下边距同乘 scale）
            // 【拖拽防闪 2026-09-08】拖拽中（鼠标按住移动）本线程与 wndproc
            // 的 WM_MOUSEMOVE 并发 SetWindowPos 同一窗口——两处位置打架
            // = 窗口来回跳变（用户实测「拖动候选闪烁」）。拖拽期间跳过
            // show() 的定位（渲染/Present 照常，位置交给拖拽消息控制；
            // 拖动 NOSIZE 尺寸不变，全跳过安全）；松手后 CAND_DROP_AT
            // 生效回正。
            let dragging = CAND_DRAG
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some();
            // 【毛玻璃退役 2026-09-11】抓屏/自绘模糊/玻璃阴影窗路径整块
            // 删除（kind 不再分支）；shadowwin 由此不再启用。
            // 【err=183 噪声修复 2026-09-08】GetLastError 在 API 成功时
            // 不清零——历史日志大量 err=183 是前序调用残留，误导排查
            //（SetWindowPos 实际成功）。仅真失败（返回 0）才报错。
            let sp_ok = if !dragging {
                // 【零位移 2026-09-11·终版】窗口尺寸一步到位=目标（每键
                // 仅一次 SWP，与无动效时代同频——动画全程窗口不 resize，
                // 杜绝 flip-model 逐帧中间态 resize 的 DWM 拉伸闪烁；
                // 用户实测「按键一下字闪一下」的根源）。动画=壳盒
                // （chrome_override）在稳定窗口内从小长大——入场 v2 与
                // 拉伸动效统一此语义。命中盒按目标内容（揭示中余量仍穿透）。
                let apply = (w_out as i32, h_out as i32);
                // 【位置滑动】可见中且目标位移动于 6px → 起臂位置动效
                //（整句自动上屏：候选跟新光标丝滑滑过去）；首显/小位移
                // 瞬移。tick 每 15ms move-only 步进（不重绘，零成本）。
                let (tx, ty) = (
                    x - (shadow_m * dpi_scale) as i32,
                    y - (shadow_m * dpi_scale) as i32,
                );
                if was_visible && self.pos_ms > 0 && !self.internal_rerender {
                    let (lx, ly) = self.live_pos.get();
                    let d = (tx - lx).abs().max((ty - ly).abs());
                    if d >= 6 {
                        self.pos_anim = Some(((lx, ly), (tx, ty), std::time::Instant::now()));
                        unsafe {
                            let _ = SetTimer(self.hwnd, FADE_TIMER_ID, FADE_TICK_MS, None);
                        }
                    } else if d > 0 {
                        self.pos_anim = None;
                    }
                }
                let (px, py) = match self.pos_anim {
                    Some((f, t, t0)) => {
                        size_ease(f, t, t0.elapsed().as_millis() as u32, self.pos_ms)
                    }
                    None => (tx, ty),
                };
                self.live_size.set(apply);
                self.content_size.set((w_out as i32, h_out as i32));
                self.live_pos.set((px, py));
                SetWindowPos(
                    self.hwnd,
                    HWND_TOPMOST,
                    px,
                    py,
                    apply.0,
                    apply.1,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                )
            } else {
                // 拖拽中窗口可能仍隐藏（首次 show 未显示）：确保可见
                SetWindowPos(
                    self.hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                )
            };
            if sp_ok.is_err() {
                crate::tsf::trace(&format!(
                    "cw2: SetWindowPos({x},{y}) 失败 err={:?} visible={}",
                    sp_ok,
                    IsWindowVisible(self.hwnd).0
                ));
            }
            // 【毛玻璃退役 2026-09-11】glass RGN/DWM 圆角/NC 链整块删除；
            // 仅保留残留清理（曾开过毛玻璃的窗恢复全窗区域+方角）。
            if self.rgn_last.get() != 0 {
                unsafe {
                    let _ = SetWindowRgn(self.hwnd, HRGN(std::ptr::null_mut()), true);
                }
                self.rgn_last.set(0);
            }
            // 【锁标已移除 2026-09-10】固定态不再有视觉指示（拖动即
            // 固定、右键即解锁——位置本身即状态，无需锁标小窗）。
        }
    }

    /// 【动效 2026-09-11】渐隐渐显当前帧透明度（渲染级）：由 fade 状态
    /// 推导，二次缓动（起手快收尾缓）。稳态（fade=None）恒 1.0。
    /// 【方案变更】DComp Visual3::SetOpacity2 对 NOREDIRECTIONBITMAP+
    /// swapchain 窗实测无效（overlay 直通绕过合成属性——GPI 像素取证
    /// 0.15/1.0 均亮 147），改 D2D PushLayer(opacity) 包整帧：仅过渡帧
    /// 生效、稳态零开销，且玻璃/阴影随内容一起淡入（比 visual 级更完整）。
    pub(crate) fn fade_alpha(&self) -> f32 {
        match self.fade {
            None => 1.0,
            Some((fading_in, t0)) => {
                // 【淡出专用 2026-09-11】进场=下限曲线（防透底重叠）；
                // 退场=全幅 1→0（窗正在离开，透底即目的）。退场时长：
                // fade_ms>0 用之，否则 120ms 默认。
                // 【退场起步即沉 2026-09-11】二次缓动起步太平（前 1/3 程
                // 几乎不透明）+200%+速度拉长后被观感为「先压重再消失」
                // （半透明面板压在新上屏文字上=变重）——改三次曲线：首帧
                // 即显著下沉，全程只做「变淡」。
                let dur_ms = if fading_in {
                    self.fade_ms.max(1)
                } else if self.fade_ms > 0 {
                    self.fade_ms
                } else {
                    self.fade_ms_eff.max(1)
                } as f64;
                let p = (t0.elapsed().as_secs_f64() * 1000.0 / dur_ms).clamp(0.0, 1.0);
                if fading_in {
                    (FADE_FLOOR + (1.0 - FADE_FLOOR) * (1.0 - (1.0 - p) * (1.0 - p))) as f32
                } else {
                    (((1.0 - p) * (1.0 - p) * (1.0 - p)) * 1.0) as f32
                }
            }
        }
    }

    /// 【动效 2026-09-11】渐隐渐显 tick（FADE_TIMER_ID 驱动）：只管状态
    /// 推进（完成/真隐藏），alpha 由 fade_alpha() 推导、fade_tick_shared
    /// 用 last_show 参数复渲染呈现。返回 true=动画结束（调用方 KillTimer）。
    /// 渐隐完成时真隐藏——本 tick 在 wndproc 消息线程（非 TSF 焦点
    /// 回调），同步 SW_HIDE 安全。
    pub(crate) fn fade_tick(&mut self) -> bool {
        let Some((fading_in, t0)) = self.fade else {
            return true;
        };
        // 时长：皮肤 fade_ms>0 用之，否则全局速度版的 120ms 默认
        let ms = if self.fade_ms > 0 {
            self.fade_ms as f64
        } else {
            self.fade_ms_eff.max(1) as f64
        };
        let done = t0.elapsed().as_secs_f64() * 1000.0 >= ms;
        if done {
            self.fade = None;
            if !fading_in {
                unsafe {
                    let _ = KillTimer(self.hwnd, EXPAND_TIMER_ID);
                    let _ = KillTimer(self.hwnd, FADE_TIMER_ID);
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
                shadowwin_set_alpha(1.0);
                self.last_hide_at = Some(std::time::Instant::now());
                // 【rect 只增不减→尺寸动效】窗退役：动效与余量基准归零，
                // 下会话重定
                self.size_anim = None;
                self.chrome_override.set(None);
                self.scale_in.set(false);
                self.pos_anim = None;
                self.live_size.set((0, 0));
            }
        }
        done
    }

    /// 鼠标当前是否悬停在本候选窗上（OnSetFocus 守卫用：交互中的
    /// 点击连带焦点事件不清组段、不隐藏窗口）。
    pub fn is_mouse_over(&self) -> bool {
        unsafe {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            WindowFromPoint(pt).0 == self.hwnd.0
        }
    }

    /// 窗口当前是否可见（poll 前台兜底用：只在可见时记日志/收尾）
    pub fn is_visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    pub fn hide(&mut self) {
        // 组段结束：作废「正向打字」单调锁——置 MAX 使下一帧必判
        // 「非增长」→ 新组段首帧自由定位（修单键接单键锁死旧位置）。
        // 粘性位置**保留**：跨组段的位置记忆，新组段首帧锚点暂不可
        // 用时沿用近处而非瞬移屏幕中下（清掉它正是「时不时跳到屏幕
        // 中下方」的病根）。
        self.last_raw_len = usize::MAX;
        // 【位置滑动】收窗即作废位置动效（下个组段首显瞬移新位）
        self.pos_anim = None;
        // 【拖拽钉住解除】收窗（上屏断段/失焦/翻段）即解除拖拽钉住
        // ——下一组段恢复跟随 caret。
        self.sticky_drag = false;
        // 候选窗隐藏时阴影窗同退（组段间不孤零零挂着）
        shadowwin_hide();
        // 【绝不同步 ShowWindow】焦点回调（OnSetFocus）里同步 SW_HIDE
        // 与 MSCTF/Chromium 焦点临界区死锁——VSCode 点击冻结事故实锤
        // （栈：OnSetFocus → ShowWindow 永不返回）。改为 PostMessage
        // 排队，焦点回调返回后由消息循环执行隐藏。
        unsafe {
            let _ = PostMessageW(self.hwnd, WM_APP_HIDE_CAND, WPARAM(0), LPARAM(0));
        }
    }
}

/// 隐藏候选窗的应用层消息（PostMessage 异步隐藏用）
pub const WM_APP_HIDE_CAND: u32 = 0x4948; // "IH"

/// 【动效 2026-09-11】渐隐渐显 tick 定时器 id（15ms≈67fps）与
/// 注释展开延时定时器 id——挂在本窗消息队列，wndproc 0x113 消费。
pub const FADE_TIMER_ID: usize = 0x4846_5550; // 'HuFZ'
pub const EXPAND_TIMER_ID: usize = 0x4846_5551; // 'HuFa'
pub const FADE_TICK_MS: u32 = 15;
/// 静默期：show↔hide 间隔小于此值直接跳过动画（连打逐字上屏的
/// 收放循环不频闪）——仅管入场侧
const FADE_QUIET_MS: u128 = 250;
/// 【退场门退役 2026-09-11】退场动画整体移除（收窗即时隐藏）——
/// 常量随之删除；入场静默期（FADE_QUIET_MS）保留。
/// 【淡入淡出下限 2026-09-11】动画期间整帧 alpha 的最低值——低于此
/// 值面板接近全透、底层文字透出（用户「重叠感」）。0.6×皮肤自身
/// master_alpha≈0.68 → 最低有效不透明 ≈0.41：柔和淡入且不重叠。
const FADE_FLOOR: f64 = 0.6;

/// 【动效】渐隐渐显 + 尺寸动效 tick：take cand2+last_show → 推进 fade
/// 状态 →（fade 活跃时）按当前 alpha 复渲染 → 尺寸插值步进（只 SWP
/// 不重绘——内容按目标布局早已在缓冲）→ 放回。两者皆结束 KillTimer。
unsafe fn fade_tick_shared(hwnd: HWND) {
    let Some(gsh) = crate::tsf::G_SHARED.get() else {
        return;
    };
    let shared = gsh.0.clone();
    let (mut cand2, last, skin, caret) = {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        // 【动效窗口让渡】标记持有中——TSF 线程此刻 is_none() 不新建
        // 第二窗（短等放回），组段收尾挂 pending_cand_hide
        g.cand2_busy = true;
        (g.cand2.take(), g.last_show.clone(), g.skin.clone(), g.caret)
    };
    let mut anim_done = true;
    if let (Some(c), Some((cands, raw, sel))) = (cand2.as_mut(), last) {
        let had_fade = c.fade.is_some();
        let fade_done = c.fade_tick();
        if had_fade && c.is_visible() {
            // 仍在显示（渐显过渡/完成帧；渐隐完成时已 SW_HIDE 跳过复渲染
            // ——show() 的 SWP_SHOWWINDOW 会把刚藏的窗复活）
            c.internal_rerender = true;
            c.show(&cands, &raw, &skin, caret.as_ref(), sel);
            c.internal_rerender = false;
            // 阴影窗 alpha 同步面板（否则面板渐入、阴影全浓=重叠感）
            shadowwin_set_alpha(c.fade_alpha());
            if fade_done {
                shadowwin_set_alpha(1.0);
            }
        }
        // 【拉伸动效步进】每 tick 以当前插值尺寸整帧重绘：外壳（背景/
        // 边框/阴影）画在插值尺寸上=边缘把边框阴影「拉过去」（延伸
        // 感），内容按目标布局裁在外壳内；完成帧解除覆盖按目标渲染。
        if let Some((f, t, t0)) = c.size_anim {
            let ms = t0.elapsed().as_millis() as u32;
            let cur = size_ease(f, t, ms, c.size_ms);
            let finished = cur == t;
            if finished {
                c.size_anim = None;
                c.chrome_override.set(None);
                c.scale_in.set(false);
            } else {
                anim_done = false;
                c.chrome_override.set(Some(cur));
            }
            c.live_size.set(cur);
            if c.is_visible() {
                c.internal_rerender = true;
                let _ = c.show(&cands, &raw, &skin, caret.as_ref(), sel);
                c.internal_rerender = false;
            }
        }
        // 【位置滑动步进】move-only（内容不变不重绘）：插值坐标推进
        // 窗口跟光标滑动；完成即清。首显起臂在 show() 的 SWP 处。
        if let Some((f, t, t0)) = c.pos_anim {
            let cur = size_ease(f, t, t0.elapsed().as_millis() as u32, c.pos_ms);
            if cur == t {
                c.pos_anim = None;
                c.live_pos.set(t);
            } else {
                anim_done = false;
                c.live_pos.set(cur);
                if c.is_visible() {
                    let _ = SetWindowPos(
                        hwnd,
                        HWND_TOPMOST,
                        cur.0,
                        cur.1,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
            }
        }
        if c.fade.is_some() {
            anim_done = false;
        }
    }
    if anim_done {
        let _ = KillTimer(hwnd, FADE_TIMER_ID);
    }
    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
    g.cand2_busy = false;
    // 组段已在持有期间收尾 → 代为隐藏（旧窗漏藏=残留阴影）
    let pending_hide = std::mem::take(&mut g.pending_cand_hide);
    match (g.cand2.take(), cand2) {
        (None, Some(mut mine)) => {
            if pending_hide {
                mine.hide();
            } else {
                g.cand2 = Some(mine);
            }
        }
        (Some(newer), Some(mut mine)) => {
            mine.hide();
            g.cand2 = Some(newer);
        }
        (Some(newer), None) => g.cand2 = Some(newer),
        (None, None) => {}
    }
}

/// 【注释展开延时】到点补一帧全注释：窗口已不可见（组段已收）则弃；
/// 已展开则幂等清理；否则置展开位并按 last_show 缓存参数重渲染
/// （take/put-back，锁外渲染）。
unsafe fn expand_tick_shared(hwnd: HWND) {
    if !IsWindowVisible(hwnd).as_bool() {
        let _ = KillTimer(hwnd, EXPAND_TIMER_ID);
        return;
    }
    let Some(gsh) = crate::tsf::G_SHARED.get() else {
        return;
    };
    let shared = gsh.0.clone();
    let (mut cand2, last, skin, caret) = {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        // 【动效窗口让渡】同 fade_tick：持有标记 + pending 隐藏
        g.cand2_busy = true;
        (g.cand2.take(), g.last_show.clone(), g.skin.clone(), g.caret)
    };
    if let (Some(c), Some((cands, raw, sel))) = (cand2.as_mut(), last) {
        if !c.comments_expanded {
            c.comments_expanded = true;
            let _ = KillTimer(hwnd, EXPAND_TIMER_ID);
            c.show(&cands, &raw, &skin, caret.as_ref(), sel);
        }
    }
    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
    g.cand2_busy = false;
    let pending_hide = std::mem::take(&mut g.pending_cand_hide);
    match (g.cand2.take(), cand2) {
        (None, Some(mut mine)) => {
            if pending_hide {
                mine.hide();
            } else {
                g.cand2 = Some(mine);
            }
        }
        (Some(newer), Some(mut mine)) => {
            mine.hide();
            g.cand2 = Some(newer);
        }
        (Some(newer), None) => g.cand2 = Some(newer),
        (None, None) => {}
    }
}

/// 拖拽状态：(鼠标屏幕位 − 窗口原点) 偏移；None=非拖拽中。
static CAND_DRAG: std::sync::Mutex<Option<(i32, i32)>> = std::sync::Mutex::new(None);
/// 【点击死区 2026-09-08】左键按下时的鼠标屏幕位（未激活拖拽）。
/// MOUSEMOVE 累计位移 >4px 才激活 CAND_DRAG——单击的鼠标抖动
/// （1-2px）不拖窗、松手不固化 pin（用户实测「锁了之后左键点一下
/// 跳一下/位移一下」：抖动被当拖拽，窗口挪一点还把偏移固化）。
static CAND_DOWN: std::sync::Mutex<Option<(i32, i32)>> = std::sync::Mutex::new(None);

/// 候选窗固定位置（窗口原点，屏幕坐标）；None=未固定。
/// 【2026-09-10 用户拍板】拖动松手即固定（拖到哪里固定在哪里，跨组
/// 段/上屏保持）；右键解除恢复跟随光标。进程级（每应用独立记忆）。
pub static CAND_PINNED: std::sync::Mutex<Option<(i32, i32)>> = std::sync::Mutex::new(None);

/// 拖拽松手位置（wndproc → show() 一次性消费：设为 sticky_pos，
/// 本组段内留在松手处；新组段锚点就绪即恢复跟随光标）。
static CAND_DROP_AT: std::sync::Mutex<Option<(i32, i32)>> = std::sync::Mutex::new(None);

/// 【右键解锁 2026-09-10】wndproc → show() 一次性标记：右键解除固定
/// 时置位，下一帧 show 清 sticky_pos/sticky_drag（拖拽钉住残留）。
static CAND_UNSTICK: std::sync::Mutex<bool> = std::sync::Mutex::new(false);

/// 【pin 双保险 2026-09-10】本按下周期内真正拖过（越过死区）——
/// 即使拖拽态中途被意外清掉，0x202 松手仍按窗口当前位置写固定位
///（用户拍板：拖动时不要求光标存活，只要位置能锁住）。
static CAND_DRAGGED_ONCE: std::sync::Mutex<bool> = std::sync::Mutex::new(false);

// ── 独立阴影窗（毛玻璃 v3.5）──
// glass 候选窗无边距（accent 限制窗口=面板）——自绘阴影没有边距区
// 可画、DwmExtendFrame 对 DComp 直呈窗无效（实测无阴影）。独立分层
// 窗（分层 ULW）：尺寸=面板+2m 边距，SDF 高斯衰减 alpha 阴影，
// ULW 上屏，Z 序在候选窗下（先置顶，候选窗随后 TOPMOST 盖上）。
static SHADOW_HWND: std::sync::Mutex<Option<isize>> = std::sync::Mutex::new(None);
static SHADOW_KEY: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
static SHADOW_M: std::sync::Mutex<u32> = std::sync::Mutex::new(0); // 当前边距（follow 用）

unsafe extern "system" fn shadow_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// 圆角矩形 SDF（带符号距离，外正内负）
fn sd_round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let ax = qx.max(0.0);
    let ay = qy.max(0.0);
    ((ax * ax + ay * ay).sqrt() + qx.max(qy).min(0.0) - r) as f32
}

/// 渲染阴影位图并 ULW 上屏（尺寸/参数变化才重渲染）
/// g_size=σ₁ 基准（glass_shadow_size）、g_alpha=浓度（glass_shadow_alpha）
unsafe fn shadowwin_render(
    hwnd: HWND,
    w: u32,
    h: u32,
    m: u32,
    radius: u32,
    g_size: f32,
    off_x: i32,
    off_y: i32,
    g_alpha: f32,
) {
    let key = format!("{w}:{h}:{m}:{radius}:{g_size:.1}:{off_x}:{off_y}:{g_alpha:.2}");
    if *SHADOW_KEY.lock().unwrap_or_else(|e| e.into_inner()) == key {
        return;
    }
    // 【KEY 提交时序 2026-09-09】幂等键改为渲染成功后提交——原实现
    // 先写后渲染，CreateDIBSection 失败（GDI 内存耗尽等）后同 key 永
    // 不重试：窗口尺寸已变而位图是旧的，UpdateLayeredWindow 把旧位图
    // 拉伸到新尺寸=阴影变形「糊成一坨」（多人实测截图实锤）。
    let hdc = CreateCompatibleDC(HDC(std::ptr::null_mut()));
    let mut bmi = windows::Win32::Graphics::Gdi::BITMAPINFO {
        bmiHeader: windows::Win32::Graphics::Gdi::BITMAPINFOHEADER {
            biSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0,
            ..Default::default()
        },
        bmiColors: [windows::Win32::Graphics::Gdi::RGBQUAD::default()],
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = match CreateDIBSection(
        hdc,
        &bmi as *const _,
        windows::Win32::Graphics::Gdi::DIB_USAGE(0),
        &mut bits,
        None,
        0,
    ) {
        Ok(d) if !bits.is_null() => d,
        _ => {
            let _ = DeleteDC(hdc);
            return;
        }
    };
    let old = SelectObject(hdc, windows::Win32::Graphics::Gdi::HGDIOBJ(dib.0));
    // SDF 双高斯衰减【v3.9 玻璃阴影独立可调】σ₁=g_size（拖尾 2.6σ₁）、
    // 浓度=g_alpha——与纯色阴影（shadow_radius/shadow_alpha）完全独立。
    let (fw, fh) = (w as f32, h as f32);
    let (phw, phh) = ((fw - 2.0 * m as f32) / 2.0, (fh - 2.0 * m as f32) / 2.0);
    let (scx, scy) = (fw / 2.0 - off_x as f32, fh / 2.0 - off_y as f32);
    let sigma1 = g_size.max(0.1);
    let sigma2 = sigma1 * 2.6;
    let base_a = g_alpha.clamp(0.0, 1.0);
    let px = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
    let mut o = 0usize;
    for y in 0..h {
        for x in 0..w {
            let d = sd_round_rect(
                x as f32 + 0.5,
                y as f32 + 0.5,
                scx,
                scy,
                phw,
                phh,
                radius as f32,
            );
            // 面板内部无影；外侧双高斯（近浓+远晕）
            let a = if d <= 0.0 {
                0.0
            } else {
                let t1 = d / sigma1;
                let t2 = d / sigma2;
                base_a * (0.62 * (-t1 * t1 * 0.5).exp() + 0.38 * (-t2 * t2 * 0.5).exp())
            };
            let a8 = (a * 255.0).round() as u8;
            // BGRA 预乘（黑影：BGR=0）
            px[o] = 0;
            px[o + 1] = 0;
            px[o + 2] = 0;
            px[o + 3] = a8;
            o += 4;
        }
    }
    let blend = windows::Win32::Graphics::Gdi::BLENDFUNCTION {
        BlendOp: 0,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: 1, // AC_SRC_ALPHA（预乘）
    };
    let pt = windows::Win32::Foundation::POINT { x: 0, y: 0 };
    let sz = windows::Win32::Foundation::SIZE {
        cx: w as i32,
        cy: h as i32,
    };
    let _ = UpdateLayeredWindow(
        hwnd,
        None,
        None,
        Some(&sz as *const windows::Win32::Foundation::SIZE),
        hdc,
        Some(&pt as *const windows::Win32::Foundation::POINT),
        windows::Win32::Foundation::COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    );
    // 【DIB 泄漏修复 2026-09-09】GDI 规定：选入 DC 的对象 DeleteObject
    // 必失败——原实现在选入状态直接删，每次参数/尺寸变化泄漏一张
    // w×h×4 位图（数百 KB/键），累积耗尽 GDI 内存→后续 CreateDIBSection
    // 失败（联动 KEY 时序 bug=阴影变形）。先恢复 old 出选再删。
    SelectObject(hdc, old);
    let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(dib.0));
    let _ = DeleteDC(hdc);
    // bmi 仅用一次（字段都被赋值），消未用警告
    let _ = &mut bmi;
    // 渲染成功才提交幂等键
    *SHADOW_KEY.lock().unwrap_or_else(|e| e.into_inner()) = key;
}

/// glass 候选窗定位前调用：阴影窗先就位（目标坐标 x,y），随后候选窗
/// TOPMOST 压在其上（Z 序正确：阴影在候选窗下、桌面上）。
pub fn shadowwin_show(
    cand: HWND,
    x: i32,
    y: i32,
    w_out: u32,
    h_out: u32,
    radius_phys: u32,
    g_size: f32,
    off_x: i32,
    off_y: i32,
    g_alpha: f32,
) {
    // 【创建竞态修复 2026-09-09】check（锁内）→create（锁外）→写回（锁
    // 内）原实现两线程并发时各建一个阴影窗，先建的永不销毁=「阴影在别
    // 的地方糊成一坨」（旧窗停留旧帧）。创建全程持锁（double-check）。
    let h = {
        let mut g = SHADOW_HWND.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = *g {
            unsafe {
                if IsWindow(HWND(h as *mut _)).as_bool() {
                    h
                } else {
                    0
                }
            }
        } else {
            0
        }
    };
    let h = if h != 0 {
        h
    } else {
        let mut g = SHADOW_HWND.lock().unwrap_or_else(|e| e.into_inner());
        // double-check：等锁期间另一线程可能已创建
        if let Some(h) = *g {
            unsafe {
                if IsWindow(HWND(h as *mut _)).as_bool() {
                    h
                } else {
                    0
                }
            }
        } else {
            0
        }
    };
    let h = if h != 0 {
        h
    } else {
        unsafe {
            let class: Vec<u16> = "HuFuCandShadow\0".encode_utf16().collect();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(shadow_wndproc),
                hCursor: LoadCursorW(HINSTANCE(std::ptr::null_mut()), IDC_ARROW)
                    .unwrap_or(HCURSOR(std::ptr::null_mut())),
                lpszClassName: PCWSTR(class.as_ptr()),
                hbrBackground: HBRUSH(std::ptr::null_mut()),
                ..Default::default()
            };
            let _atom = RegisterClassW(&wc);
            let ex = WINDOW_EX_STYLE(
                WS_EX_TOOLWINDOW.0
                    | WS_EX_TOPMOST.0
                    | WS_EX_NOACTIVATE.0
                    | WS_EX_LAYERED.0
                    | WS_EX_TRANSPARENT.0,
            );
            match CreateWindowExW(
                ex,
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(WS_POPUP.0),
                0,
                0,
                10,
                10,
                HWND(std::ptr::null_mut()),
                HMENU(std::ptr::null_mut()),
                HINSTANCE(std::ptr::null_mut()),
                None,
            ) {
                Ok(hw) if !hw.0.is_null() => {
                    // 持锁写回：竞态双建时后建的也记录（先建的已被
                    // 另一线程写回则此处覆盖前先销毁新建的，防泄漏）
                    let mut g2 = SHADOW_HWND.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(prev) = *g2 {
                        if prev != hw.0 as isize && IsWindow(HWND(prev as *mut _)).as_bool() {
                            let _ = DestroyWindow(HWND(prev as *mut _));
                        }
                    }
                    *g2 = Some(hw.0 as isize);
                    hw.0 as isize
                }
                _ => return,
            }
        }
    };
    if h == 0 {
        return;
    }
    unsafe {
        // 边距按独立 σ₂（拖尾 3σ₂≈47px 覆盖）——位图边界截断拖尾会出硬边
        let m =
            (g_size.max(0.1) * 2.6 * 3.0 + 6.0 + off_x.abs().max(off_y.abs()) as f32).ceil() as u32;
        *SHADOW_M.lock().unwrap_or_else(|e| e.into_inner()) = m;
        let sw = w_out + 2 * m;
        let sh2 = h_out + 2 * m;
        let _ = cand;
        // 先渲染内容再定位显示（避免空白帧）
        SetWindowPos(
            HWND(h as *mut _),
            HWND_TOPMOST,
            x - m as i32,
            y - m as i32,
            sw as i32,
            sh2 as i32,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        shadowwin_render(
            HWND(h as *mut _),
            sw,
            sh2,
            m,
            radius_phys,
            g_size,
            off_x,
            off_y,
            g_alpha,
        );
    }
}

/// 【尺寸动效 2026-09-11】smoothstep 插值：t∈[0,ms] 映射进度
/// p=3t²-2t³（缓起-加速-缓收，「成长感」明确——ease-out 起步即
/// 大位移被实测判「无动画感」），返回 from→to 的即时尺寸。
pub(crate) fn size_ease(from: (i32, i32), to: (i32, i32), t_ms: u32, dur_ms: u32) -> (i32, i32) {
    if dur_ms == 0 || t_ms >= dur_ms {
        return to;
    }
    let x = t_ms as f32 / dur_ms as f32;
    let p = x * x * (3.0 - 2.0 * x);
    let l = |a: i32, b: i32| a + ((b - a) as f32 * p).round() as i32;
    (l(from.0, to.0), l(from.1, to.1))
}

/// 【动效】渐隐渐显阴影窗同步：整窗 alpha 跟随面板 fade 值（否则
/// 面板半透渐入、阴影全浓=「重叠」感）。无阴影窗（纯色模式）安全跳过。
pub(crate) fn shadowwin_set_alpha(a: f32) {
    let g = SHADOW_HWND.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(h) = *g {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SetLayeredWindowAttributes(
                HWND(h as *mut _),
                windows::Win32::Foundation::COLORREF(0),
                (a.clamp(0.0, 1.0) * 255.0) as u8,
                LWA_ALPHA,
            );
        }
    }
}

/// 候选窗隐藏时联动隐藏阴影
pub fn shadowwin_hide() {
    if let Some(h) = *SHADOW_HWND.lock().unwrap_or_else(|e| e.into_inner()) {
        unsafe {
            let _ = ShowWindow(HWND(h as _), SW_HIDE);
        }
    }
}

/// 【玻璃阴影几何取证 2026-09-11】pad-dump 扫档用：候选窗与阴影窗的
/// 屏幕矩形对（None=阴影窗未建/已亡——纯色模式无独立阴影窗）。
/// 扫档器断言：阴影窗=候选窗四边等量外扩（居中，与 DPI 无关）。
pub(crate) fn shadow_geo(cand: HWND) -> Option<(RECT, RECT)> {
    let h = (*SHADOW_HWND.lock().unwrap_or_else(|e| e.into_inner()))?;
    unsafe {
        if !IsWindow(HWND(h as *mut _)).as_bool() {
            return None;
        }
        let mut sc = RECT::default();
        if GetWindowRect(HWND(h as *mut _), &mut sc).is_err() {
            return None;
        }
        let mut cc = RECT::default();
        if GetWindowRect(cand, &mut cc).is_err() {
            return None;
        }
        Some((cc, sc))
    }
}

/// 拖拽移动时阴影窗跟随（候选窗原点-m 边距）
pub fn shadowwin_follow(cand: HWND) {
    let m = *SHADOW_M.lock().unwrap_or_else(|e| e.into_inner());
    if m == 0 {
        return;
    }
    if let Some(h) = *SHADOW_HWND.lock().unwrap_or_else(|e| e.into_inner()) {
        unsafe {
            if IsWindow(HWND(h as _)).as_bool() && IsWindowVisible(HWND(h as _)).as_bool() {
                let mut wr = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                let _ = GetWindowRect(cand, &mut wr);
                let _ = SetWindowPos(
                    HWND(h as *mut _),
                    HWND(std::ptr::null_mut()),
                    wr.left - m as i32,
                    wr.top - m as i32,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }
}
