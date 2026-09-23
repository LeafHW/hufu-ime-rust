//! 候选窗 v2：D3D11 + DirectComposition + Direct2D + DWM 真实材质。
//!
//! - 窗口：WS_POPUP + WS_EX_NOREDIRECTIONBITMAP（DComp 直通，逐像素 alpha）
//! - 材质（皮肤 material.kind）→ SetWindowCompositionAttribute accent：
//!   solid=不透明 / translucent=半透明渐变 / frosted=Acrylic 磨砂 /
//!   glass=HostBackdrop 玻璃（Win11 22H2+）
//! - 文本：DirectWrite；圆角/高亮：D2D FillRoundedRectangle
//! - 初始化失败时上层回退 v1（GDI 分层窗口）

use serde_json::Value;
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
use windows::core::Interface;
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
            // 【B3 修复 2026-09-13 三十四修】TL 词框小窗上的滚轮不再
            // 渲染主窗实例（跨线程 take g.cand2 show=残留窗同型病根；
            // fade_tick_shared 已分流、此处漏）。TL 线程直接忽略字号
            // 滚轮（词框字号由皮肤统一管理）。
            if crate::tsf::addword_tl_thread() {
                return LRESULT(0);
            }
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
        // 【三十六修·死块删除】WM_NCHITTEST（0x84）已在 wndproc 头部
        //（HTCLIENT 强制整窗命中，QQ 按钮消息实测教训）无条件 return，
        // 此臂自那时起不可达——其「余量区 HTTRANSPARENT 穿透」设计与
        // 头部强制 HTCLIENT 语义互斥（后者胜出）。B4 审计项随之关闭：
        // 经 g.cand2 读 content_size 的跨线程疑虑连代码带块消亡。
        // 阴影/余量带（约 90-150ms 动画帧）会挡宿主点击，属既定取舍。
        // 异步隐藏（hide() PostMessage 而来——焦点回调里同步 ShowWindow
        // 会与 MSCTF/Chromium 焦点临界区死锁）
        crate::candwin2::WM_APP_HIDE_CAND => {
            // 【五十五修·闪窗取证 2026-09-23】外部 25ms 采样证实闪不是
            // 可见性翻转可捕的时长——藏窗消息落地（全部隐藏源的唯一必
            // 经点）时窗口仍可见=真闪：毫秒级记档，与击键/commit 对齐
            // 即可归因。窗口已不可见时不记（零噪音）。
            if crate::tsf::trace_on() && unsafe { IsWindowVisible(hwnd) }.as_bool() {
                crate::tsf::trace(&format!(
                    "cw2: 藏窗落地(可见→SW_HIDE) hwnd={:x}",
                    hwnd.0 as usize
                ));
            }
            // 【退场动画退役 2026-09-11】用户判「调不好」：淡出与半透明
            // 面板天然相克（渐隐帧压在新上屏文字上=变黑/重叠，连打时
            // 收放循环=一闪一闪）。收窗一律即时隐藏——干净利落。
            //（二十四修·动效大瘦身后仅存平移/尺寸/高亮滑动三项。）
            // 【B2 修复 2026-09-13 三十四修】TL 词框小窗线程的隐藏消息
            // 走共用 wndproc 时，旧实现 take 的是 g.cand2 主实例——把
            // 主窗动效状态清掉（真 TL 实例只被 ShowWindow 隐藏、内部状
            // 态无人清=残留窗病根的没修完的腿）。按消息到达线程分流
            //（与 fade_tick_shared 同款）：TL 线程只动 TL 实例。
            if crate::tsf::addword_tl_thread() {
                unsafe {
                    if let Some(mut c) = crate::tsf::tl_cand_take() {
                        c.last_hide_at.set(Some(std::time::Instant::now()));
                        c.size_anim = None;
                        c.chrome_override.set(None);
                        c.pos_anim = None;
                        // 【三十六修】chase 三元组同步清（与 hide_now/
                        // focus_reset 同口径——残留 chase_target 会在下次
                        // show 臂发前被 tick 拖向旧目标）
                        c.chase_target = None;
                        c.chase_last = None;
                        c.chase_pos = None;
                        c.live_size.set((0, 0));
                        crate::tsf::tl_cand_put_back(Some(c));
                    }
                    let _ = KillTimer(hwnd, FADE_TIMER_ID);
                    let _ = KillTimer(hwnd, EXPAND_TIMER_ID);
                    // 【三十六修】主分支同款：HIDE_LATER 延迟收窗钟一并杀
                    let _ = KillTimer(hwnd, HIDE_LATER_TIMER_ID);
                    // 【六十修】看门狗随藏同杀
                    let _ = KillTimer(hwnd, IME_WATCHDOG_TIMER_ID);
                    unsafe { rawinput_listen(hwnd, false) };
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                return LRESULT(0);
            }
            unsafe {
                if let Some(gsh) = crate::tsf::G_SHARED.get() {
                    let shared = gsh.0.clone();
                    let mut cand2 = {
                        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                        g.cand2.take()
                    };
                    if let Some(c) = cand2.as_mut() {
                        // 【二十五修·y 锁跨段延续】真隐藏执行点只记时刻，
                        // 不再清 y 锁/首帧自由（上屏即收后每段都经此路，
                        // 无条件清锁=段间锚 y 锯齿穿透）。首帧自由与否由
                        // show() 延续门判定；焦点切换走 focus_reset 硬清。
                        c.last_hide_at.set(Some(std::time::Instant::now()));
                        // 【尺寸动效】隐藏即整窗退役——动效与余量基准
                        // 归零，下个会话按首个内容重定
                        c.size_anim = None;
                        c.chrome_override.set(None);
                        c.pos_anim = None;
                        // 【三十六修】chase 三元组同步清（与 hide_now/
                        // focus_reset 同口径）
                        c.chase_target = None;
                        c.chase_last = None;
                        c.chase_pos = None;
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
                    }
                }
                let _ = KillTimer(hwnd, FADE_TIMER_ID);
                let _ = KillTimer(hwnd, EXPAND_TIMER_ID);
                let _ = KillTimer(hwnd, HIDE_LATER_TIMER_ID);
                // 【六十修】看门狗随藏同杀
                let _ = KillTimer(hwnd, IME_WATCHDOG_TIMER_ID);
                unsafe { rawinput_listen(hwnd, false) };
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            return LRESULT(0);
        }
        // 【五十三修·动效高频驱动】winmm 5ms 回调投递的 posted tick：
        // 与 WM_TIMER 同一处理（fade_tick_shared 时间基准幂等），帧数
        // ≈3×——入场/逐键/高亮滑动的「一顿一顿」根治点。
        crate::candwin2::WM_APP_ANIM => {
            unsafe { fade_tick_shared(hwnd) };
            return LRESULT(0);
        }
        // 【六十修】冲销请求：转投消息泵执行（回调内同步会话会被拒）
        WM_APP_IME_SWITCH => {
            unsafe {
                let shared = if crate::tsf::addword_tl_thread() {
                    crate::tsf::tl_shared()
                } else {
                    tick_shared_for_hwnd(hwnd)
                };
                if let Some(shared) = shared {
                    crate::tsf::trace("imeswitch: 消息泵执行冲销");
                    crate::tsf::ime_switch_abort(&shared);
                }
            }
            return LRESULT(0);
        }
        // 【六十修·三层】Raw Input 全局键流：物理按键不经路由直达。
        // 切换热键图形现形即收尸（见 rawinput_switch_detect）。不吞，
        // 交还 DefWindowProc 清理。
        windows::Win32::UI::WindowsAndMessaging::WM_INPUT => {
            unsafe { rawinput_switch_detect(lparam, hwnd) };
        }
        // 【动效】WM_TIMER：动画 tick（尺寸/位置/高亮滑动，FADE_TIMER
        // 历史名沿用）+ 注释展开延时。渲染/状态变更走滚轮缩放同款
        // take/put-back（锁外渲染，不抢按键路径的锁）。
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
            if id == HIDE_LATER_TIMER_ID {
                unsafe {
                    let _ = KillTimer(hwnd, HIDE_LATER_TIMER_ID);
                    let _ = PostMessageW(hwnd, WM_APP_HIDE_CAND, WPARAM(0), LPARAM(0));
                }
                return LRESULT(0);
            }
            // 【六十修·切输入法看门狗】慢钟：窗在屏时轮询前景 TIP，
            // 易主即冲销+收窗（见 ime_watchdog_tick 注释）。
            if id == IME_WATCHDOG_TIMER_ID {
                unsafe { ime_watchdog_tick(hwnd) };
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

/// 皮肤色读取（/skin/colors/{key} 包装层或顶层 colors 双形态口径）。
pub(crate) fn color_f(v: &Value, key: &str, default: &str) -> D2D1_COLOR_F {
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

pub(crate) fn layout_f(v: &Value, key: &str, default: f32) -> f32 {
    v.pointer(&format!("/skin/layout/{key}"))
        .or_else(|| v.get("layout").and_then(|l| l.get(key)))
        .and_then(|x| x.as_f64())
        .unwrap_or(default as f64) as f32
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
    /// 【ULW 模式 2026-09-11】owned 窗（打包宿主）不走 DComp 呈现：
    /// D2D 照画到离屏位图（WARP 软件），每帧读回 → DIB →
    /// UpdateLayeredWindow 上屏。无 swapchain/无 NOREDIRECTIONBITMAP，
    /// 沙盒内仅依赖 GDI，规避 DComp/硬件 D3D 卡死。
    ulw: bool,
    offscreen: Option<windows::Win32::Graphics::Direct2D::ID2D1Bitmap1>,
    ulw_dc: isize,
    ulw_hbm: isize,
    ulw_bits: isize,
    ulw_w: i32,
    ulw_h: i32,
    /// 【五十六修·空帧拦截】上一帧 ULW 是否已上过非空内容——
    /// 全透明新帧（渲染偶发清屏后未落笔：清色全透明+绘制被裁剪/
    /// 设备抖动）如果照常上屏，可见效果=候选整窗消失一瞬间再
    /// 重现（用户实锤「候选闪」，30fps 像素抓帧抓到 646B 级全透
    /// 明帧）。拦截：全透明且上一帧有内容 → 跳过本帧 ULW（屏上
    /// 保留上一好帧），等下一帧真内容再上屏。
    ulw_last_nonempty: bool,
    size: (i32, i32),
    /// 粘性定位：最近一次有效锚点坐标。锚点偶发丢失（GetTextExt 在
    /// 异步编辑会话未就绪时失败）时沿用上次位置——绝不能瞬移屏幕中央，
    /// 那正是候选框「在光标周围乱跳」的病根。
    sticky_pos: Option<(i32, i32)>,
    /// 【四十八修·粘位焦点窗归属】sticky_pos 记录时所在的焦点窗
    ///（GetGUIThreadInfo(0).hwndFocus，0=查询失败）。同一编辑框内
    /// 锚丢失沿用旧位（原语义）；焦点窗已换（桌面连续重命名两个
    /// 文件=两个 Edit）而新帧锚缺失时，旧位属于**别的编辑框**——
    /// 沿用=候选停在上一个文件旁（用户实锤「桌面重命名候选离得
    /// 远·偏左一两个图标」），改落当前焦点编辑框正下方。
    sticky_focus_h: std::cell::Cell<isize>,
    /// 【拖拽钉住 2026-09-08】拖拽松手设的 sticky 是「组段级钉住」：
    /// 本组段内窗口留在松手处（忽略 caret 锚），hide（收窗/失焦/
    /// 上屏断段）时解除——下一组段恢复跟随。旧行为 sticky 只作
    /// 防抖基准，松手后下一次重绘即弹回 caret（用户实测「拖动后
    /// 回到原位」）。永久固定仍走右键 pin。
    sticky_drag: bool,
    /// 【四十四修·退化锚行高缓存】本进程见过的最后一个正常锚行高
    /// （高度 ≥8px 的锚矩形）。微信4.0 对同一插入点交替上报 16px
    /// 全高与 1px 退化两种矩形（top 恒同、bottom 差 15px），落点
    /// y=bottom+4 逐键荡秋千——入口处用本缓存把退化锚 bottom 补齐
    /// 到正常行高，两种形状算出同一落点。
    last_line_h: Option<i32>,
    /// 【三十四修·死字段删除】last_raw_len（grew 判定随单调锁 8707fc4
    /// 退役后只写不读，3 处写点全删）——「正向打字」过滤现由 est 与
    /// 位置动效的 2px 死区承担。
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
    /// 【高亮滑动 2026-10-09】高亮胶囊位移动效：Some((起点矩形 LTRB,
    /// t0, 时长 ms))。渲染时胶囊从起点矩形 ease-out 插值到目标位，完成
    /// 即清。数字/；选重闪帧与 ↑↓ 移动共用（皮肤 layout.hl_ms 默认
    /// 100，0=关）。
    pub(crate) hl_anim: std::cell::Cell<Option<((f32, f32, f32, f32), std::time::Instant, u32)>>,
    /// 上一帧渲染的胶囊矩形（下一程滑动的起点）。Cell：渲染路径 &self。
    pub(crate) hl_rect: std::cell::Cell<Option<(f32, f32, f32, f32)>>,
    /// 上一用户帧的 (候选列表, 高亮下标)——滑动臂的比对基准。
    pub(crate) hl_prev: std::cell::Cell<Option<(Vec<(String, String)>, usize)>>,
    /// 高亮滑动时长 ms（皮肤 hl_ms × anim_speed）。
    pub(crate) hl_ms: u32,
    /// 【动效 tick 重渲染标记】动画 tick（FADE_TIMER）驱动的 show()
    /// 复渲染——不算用户键入帧：不重记高亮基准、不重置注释倒计时、
    /// 不重臂尺寸/位置动效。
    pub(crate) internal_rerender: bool,
    /// 上次真正隐藏时刻。（历史：入场动画静默期判定用；二十四修动画
    /// 瘦身后仅剩写入——lib.rs smoke 取证路径仍清它，联动删除另议。）
    /// 【尺寸动效 2026-09-11】(from, to, t0)：可见更新时窗口 rect 从当前
    /// 插值平滑逼近目标（内容按目标布局即刻渲染，缓冲只增不减、余量
    /// 渐进揭示/收拢）。None=无进行中的尺寸动效。
    pub(crate) size_anim: Option<((i32, i32), (i32, i32), std::time::Instant)>,
    /// 【五十九修·形变恒速】本次 size_anim 的实际时长 ms。起臂时按
    /// 剩余距离定（60px=满程参照跑满 size_ms，小距离按比例缩短，
    /// 下限 24ms）：连打每键 ~14px 增长只跑 ~25ms（2-3 帧），不再
    /// 每键重臂重置全 110ms——后者=连打全程 200fps 整帧重绘风暴+
    /// 末键后拖 145ms 的形变尾巴（机器复现实锤：末键后 36 帧到
    /// +145ms；QQ 慢线程放大成秒级「打完了还在挨个出编码挨个形
    /// 变」）。结构性大变化（≥60px）仍满速满时长，观感不变。
    pub(crate) size_anim_dur: std::cell::Cell<u32>,
    /// 【五十九修·形变帧率封顶】上一次形变帧渲染时刻——tick 里连
    /// 续重绘间隔小于「屏幕刷新帧距」（morph_frame_interval_ms：
    /// 60Hz=16.7 / 144Hz=6.9 / 240Hz=4.2ms，窗口最近显示器实际刷
    /// 新率）则跳过本帧渲染（完成帧除外），杀掉 1-3ms 突发连渲染
    ///（winmm 250fps 驱动下队列挤成一坨）；高刷屏不少帧。
    pub(crate) size_anim_last_render: std::cell::Cell<std::time::Instant>,
    /// 尺寸动效时长 ms（皮肤 layout.size_ms，默认 150，0=瞬跳）——注释
    /// 展开/收起、候选数变化等一切宽高变化都平滑过渡；连打重定目标
    /// （从当前插值位置追赶新目标，不跳变）。
    pub(crate) size_ms: u32,
    /// 【虎娘对齐 2026-09-18 六修→七修修订】尺寸形变动效开关（皮肤
    /// layout.size_morph，默认开）。回弹感的根源是 smoothstep 逐键重
    /// 臂反复慢起——线性化（size_ease 七修）已根除，形变本身保留
    ///（用户实测：变长变宽变矮的过渡是要的，去掉的是弹）。皮肤
    /// "size_morph": 0 可关。
    pub(crate) size_morph: bool,
    /// 【位置滑动 2026-09-11】整句自动上屏后剩余内容跳到新光标、候选
    /// 跟着走——位置过渡（从→到 屏幕坐标 + t0），窗口位置丝滑滑过去
    /// 而非一跳一跳。None=瞬移。
    /// 位置滑动（起点,终点,起臂时刻,时长,曲线）。曲线 0=线性（逐键
    /// 跟随等，历史口径）；1=入场 ease-out（五十一修：立方缓出——快
    /// 出缓停，短时长下 15ms 级帧距也呈自然减速收尾）。
    pub(crate) pos_anim: Option<((i32, i32), (i32, i32), std::time::Instant, u32, u8)>,
    /// 【三十四修·chase 实验通道】追赶式跟随目标：Some=正向该点收敛
    /// （指数逼近+限速，纯时间基准）。与 pos_anim 互斥——臂发时互清。
    /// 开关=C:\ProgramData\HuFu\diag\chase 旗标文件（tsf::chase_on）。
    pub(crate) chase_target: Option<(i32, i32)>,
    /// chase 的 dt 基准：上次 tick 时刻。None=臂发后首 tick（默认 8ms）。
    chase_last: Option<std::time::Instant>,
    /// chase 的浮点实位（子像素积分器；live_pos 是其取整镜像）。
    chase_pos: Option<(f32, f32)>,
    /// 【顶屏钳位许可 2026-09-12 二十四次修正】仅 C&R（顶屏上屏）路径
    /// 置 true——「字宽<编码宽」的显示回退由正向钳位钉住原地等光标。
    /// 点击换位（SP 路径）不置=永不钳（用户实锤 75px 换位被幅度法误
    /// 伤）。锚追回/换行/大跳即自动解除。
    pub(crate) forward_hold: bool,
    /// 【五十四修·y 锁方向连续性】51 修的 y 稳定锁（|dy|≤26 钉住）治
    /// 锚抖动（双向振荡），但打字区平滑滚动（虎魄打到视口中段后每段
    /// y 单向 -7~-40）也被吃掉=窗滞后文字半行、累积>26 才跳（用户
    /// 「换行跟随不准」）。区分：抖动双向、滚动单向连续——首个小步
    /// 锁住（防单帧毛刺），连续同向的第二个小步起放行（真实滚动跟随，
    /// 一帧滞后）。0=无方向记忆。
    pub(crate) ylock_last_dir: std::cell::Cell<i32>,
    /// 【y 锁累计 2026-10-09 七】连续同向小步的累计幅度（防 WPS 锯齿
    /// 抖动借「同向放行」穿透：±1/±2 连续爬升-回落是抖动，累计小；
    /// 真滚动每帧 7-40px 累计快。方向反转时归零重计）。
    pub(crate) ylock_acc: std::cell::Cell<i32>,
    /// 【跨会话首帧自由 2026-10-09 八】hide→show 新会话首帧不背旧 y 锁
    /// 状态（焦点切换后新锚与旧显示位差 4-26px 且反向时会被钉旧 y
    /// 错位）。真隐藏时置 true；show 消费后归 false——首帧自由定位。
    pub(crate) show_frame_fresh: std::cell::Cell<bool>,
    /// 【二十五修】最近一次真隐藏时刻（y 锁跨段延续判据：1.5s 内
    /// 近距重显=同文档打字延续，不清 y 锁）
    pub(crate) last_hide_at: std::cell::Cell<Option<std::time::Instant>>,
    /// 【四十五修·锚到滑入 2026-10-29】上一帧显示位置是否来自真实锚点
    ///（GetTextExt/插入符）。XAML 宿主（资源管理器重命名/搜索框）组段
    /// 首帧锚缺失 → 显示位=焦点窗兜底/sticky 钉位；锚迟到后目标跳到
    /// 光标处——此前 d>500 一律瞬落=「动效不生效」。区分来源：从兜底
    /// 位修正到真实锚=连续动作快滑（≤260ms），跨格大跳（两帧都是真锚
    /// 之间）仍瞬落（四十三修语义保留）。
    pub(crate) last_pos_anchored: std::cell::Cell<bool>,
    /// 位置动效时长 ms（皮肤 layout.pos_ms，默认 100，0=瞬跳）
    pub(crate) pos_ms: u32,
    /// 【动效开关 2026-09-11】设置页全局：false=一切动效瞬跳
    pub(crate) anim_on: std::cell::Cell<bool>,
    /// 【动效提速 2026-10-08】全局速度倍率（设置页滑条 anim_speed，0~2，
    /// 0=瞬跳 1=默认 2=慢一倍）——原实现只乘尺寸/淡出时长，平移走距离
    /// 动态公式不乘滑条（调滑条平移不变）。现平移公式也乘，滑条统管
    /// 一切动效速度。
    pub(crate) anim_spd: std::cell::Cell<f32>,
    /// 最近一次 SWP 应用过的窗口左上角屏幕坐标（位置动效的起臂基准）
    pub(crate) live_pos: std::cell::Cell<(i32, i32)>,
    /// 【拉伸动效 2026-09-11】当前帧外壳（背景/边框/阴影/RGN）的物理
    /// 窗口尺寸覆盖（含阴影边距）：动效 tick 每帧设置为当前插值尺寸
    /// ——面板外壳被「拉过去」（延伸感），内容按目标布局裁在外壳内；
    /// 稳态帧 None（目标尺寸渲染，零开销）。
    pub(crate) chrome_override: std::cell::Cell<Option<(i32, i32)>>,
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
    // 【P6 修复 2026-09-13 三十四修】OnceLock 缓存：本函数在 show() 每
    // 帧调用（动画期 15ms 一次）——每帧同步文件 IO（stage.txt 即便不
    // 存在也要走一次 CreateFile 失败路径）。毛玻璃阶段验证早已完成，
    // 阶段旋钮进程生命周期内读一次足够。
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::fs::read_to_string(r"C:\ProgramData\HuFu\diag\stage.txt")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0)
            .min(5)
    })
}

/// 【毛玻璃 v3·DWM acrylic 2026-09-08】参照 window-vibrancy / TranslucentTB
///（GitHub 成熟方案）：NOREDIRECTIONBITMAP+DComp 窗口配
/// ACCENT_ENABLE_ACRYLICBLURBEHIND——DWM 合成器直接给窗口底下做系统级
/// 真毛玻璃。零抓屏、零模糊算法。染色=GradientColor(0xAABBGGRR)。
/// 【二十四修·死码清理】capture_screen_rgba（自绘毛玻璃抓屏）随毛玻璃
/// 整链退役删除——全仓库零调用。
impl CandidateWindowV2 {
    /// 【WPS 抑制放宽探针 2026-09-12】有历史位置即可先按旧位显示
    /// （tsf.rs 首帧抑制判定用——WPS 每键重组段的即时出候选）。
    pub(crate) fn has_sticky(&self) -> bool {
        self.sticky_pos.is_some()
    }

    /// 【三十一次修正·sticky 近锚判定 2026-09-12】用户实锤「首键定位
    /// 不对」：点击换位后立刻打字，布局未稳锚是旧值，即时显示贴旧位
    /// 跳错地方；单打快打光标没动时锚≈sticky，即时显示才安全。比较
    /// 传入锚（换算后的窗目标位）与 sticky 的距离，±24px 内算近。
    pub(crate) fn sticky_near(&self, x: i32, y: i32) -> bool {
        self.sticky_pos
            .is_some_and(|(sx, sy)| (x - sx).abs() <= 24 && (y - sy).abs() <= 24)
    }

    /// 【三十四修·段间键宽自校准】sticky 落点（内容锚点坐标系，与
    /// 锚点 rect 同系可直接作差）。
    pub(crate) fn sticky_xy(&self) -> Option<(i32, i32)> {
        self.sticky_pos
    }

    /// 兼容入口：无主顶层窗（常规宿主原行为）。
    /// 常规宿主：DComp 直通窗全功能路径（硬件 D3D + swapchain + 动效）。
    /// 打包宿主（owner 有值）用 new_owned 的 ULW 软件路径。
    pub fn new() -> Option<CandidateWindowV2> {
        // 【七十修】进程时钟精度 1ms（动画 tick 5ms 生效前提）——候选窗
        // 首次创建时一次性提升（DllMain 内调不安全：loader lock）。
        raise_timer_resolution_once();
        // 【特效退役 2026-09-22】ensure_thread(上屏特效线程预热)撤除
        unsafe {
            let class: Vec<u16> = "HuFuCandWin2\0".encode_utf16().collect();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(cand2_wndproc),
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
                ulw: false,
                offscreen: None,
                ulw_dc: 0,
                ulw_hbm: 0,
                ulw_bits: 0,
                ulw_w: 0,
                ulw_h: 0,
                ulw_last_nonempty: false,
                readback: false,
                sticky_pos: None,
                sticky_focus_h: std::cell::Cell::new(0),
                sticky_drag: false,
                last_line_h: None,
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
                hl_anim: std::cell::Cell::new(None),
                hl_rect: std::cell::Cell::new(None),
                hl_prev: std::cell::Cell::new(None),
                hl_ms: 100,
                internal_rerender: false,
                forward_hold: false,
                ylock_last_dir: std::cell::Cell::new(0),
                ylock_acc: std::cell::Cell::new(0),
                show_frame_fresh: std::cell::Cell::new(true),
            last_hide_at: std::cell::Cell::new(None),
            last_pos_anchored: std::cell::Cell::new(false),
                        size_anim: None,
                chrome_override: std::cell::Cell::new(None),
                size_ms: 90,
                size_anim_dur: std::cell::Cell::new(90),
                size_anim_last_render: std::cell::Cell::new(std::time::Instant::now()),
                size_morph: false,
                pos_anim: None,
                chase_target: None,
                chase_last: None,
                chase_pos: None,
                pos_ms: 100,
                anim_on: std::cell::Cell::new(true),
                anim_spd: std::cell::Cell::new(1.0),
                live_pos: std::cell::Cell::new((0, 0)),
                live_size: std::cell::Cell::new((0, 0)),
                content_size: std::cell::Cell::new((0, 0)),
                comments_expanded: true,
            })
        }
    }

    /// 当前 owner 句柄（0=无主；owned 模式重建判定用）。
    pub fn owner_hwnd(&self) -> isize {
        unsafe { GetWindowLongPtrW(self.hwnd, GWLP_HWNDPARENT) as isize }
    }

    /// 初始化设备管线；任何一步失败返回 None（调用方回退 v1）。
    /// 【ULW owned 模式 2026-09-11】owner=Some(宿主视图窗) 时建为
    /// owned 分层窗 + 软件呈现（WARP D3D + D2D 离屏位图 + GDI
    /// UpdateLayeredWindow）。此前直接 DComp 路线在打包宿主（Win11
    /// 记事本实测）D3D11CreateDevice **卡死 TSF 线程**（沙盒 GPU
    /// 管线）——现在：① 仅 WARP 驱动（纯 CPU 光栅，无内核 GPU 句柄
    /// 依赖）；② 设备初始化全部搬到工作线程，2.5s 超时守护——再
    /// 卡死也只卡孤儿线程，主线程返回 None 回退 server 代画；③ 窗口
    /// 无 WS_EX_NOREDIRECTIONBITMAP（ULW 需要 redirection surface）。
    pub fn new_owned(owner: Option<HWND>) -> Option<CandidateWindowV2> {
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
            // ULW：WS_EX_LAYERED 必需；绝不加 NOREDIRECTIONBITMAP
            let ex = WINDOW_EX_STYLE(
                WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0 | WS_EX_NOACTIVATE.0 | WS_EX_LAYERED.0,
            );
            let owner_hwnd = owner.unwrap_or(HWND(std::ptr::null_mut()));
            let hwnd = CreateWindowExW(
                ex,
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(WS_POPUP.0),
                0,
                0,
                10,
                10,
                owner_hwnd,
                HMENU(std::ptr::null_mut()),
                HINSTANCE(std::ptr::null_mut()),
                None,
            )
            .unwrap_or_default();
            if hwnd.0.is_null() {
                return None;
            }

            // ── 设备管线：工作线程初始化（卡死守护）──
            let (tx, rx) = std::sync::mpsc::channel::<
                Option<(ID2D1DeviceContext, IDWriteFactory, IDXGIDevice)>,
            >();
            std::thread::spawn(move || {
                // COM：本线程自初始化（宿主 STA 环境里 spawn 的线程默认无）
                let _ = windows::Win32::System::Com::CoInitializeEx(
                    None,
                    windows::Win32::System::Com::COINIT_MULTITHREADED,
                );
                let r = (|| {
                    // 仅 WARP：打包沙盒里硬件驱动调用是卡死源；WARP
                    // 纯 CPU 光栅。BGRA 供 D2D 互操作。
                    let mut device: Option<ID3D11Device> = None;
                    let mut context: Option<ID3D11DeviceContext> = None;
                    let ok = D3D11CreateDevice(
                        None,
                        D3D_DRIVER_TYPE_WARP,
                        HMODULE(std::ptr::null_mut()),
                        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                        None,
                        D3D11_SDK_VERSION,
                        Some(&mut device),
                        None,
                        Some(&mut context),
                    )
                    .is_ok();
                    if !ok || device.is_none() {
                        return None;
                    }
                    let device = device?;
                    let _ = context;
                    let dxgi_dev: IDXGIDevice = device.cast().ok()?;
                    let factory2d: ID2D1Factory1 =
                        D2D1CreateFactory(D2D1_FACTORY_TYPE_MULTI_THREADED, None).ok()?;
                    let d2d_dev: ID2D1Device = factory2d.CreateDevice(&dxgi_dev).ok()?;
                    let ctx: ID2D1DeviceContext = d2d_dev
                        .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
                        .ok()?;
                    let dwrite: IDWriteFactory =
                        DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
                    Some((ctx, dwrite, dxgi_dev))
                })();
                let _ = tx.send(r);
            });
            let (ctx, dwrite, dxgi_dev) =
                match rx.recv_timeout(std::time::Duration::from_millis(2500)) {
                    Ok(Some(v)) => v,
                    Ok(None) => {
                        crate::tsf::diag_note("cw2 owned: WARP 管线初始化失败 → 回退");
                        return None;
                    }
                    Err(_) => {
                        // 超时：工作线程卡在驱动调用（孤儿线程泄漏一枚）
                        crate::tsf::diag_note("cw2 owned: 设备初始化 2.5s 超时（沙盒卡死）→ 回退");
                        return None;
                    }
                };

            Some(CandidateWindowV2 {
                hwnd,
                ctx: Some(ctx),
                swapchain: None,
                dcomp: None,
                target: None,
                visual: None,
                dwrite: Some(dwrite),
                dxgi: Some(dxgi_dev.clone()),
                ulw: true,
                offscreen: None,
                ulw_dc: 0,
                ulw_hbm: 0,
                ulw_bits: 0,
                ulw_w: 0,
                ulw_h: 0,
                ulw_last_nonempty: false,
                readback: false,
                sticky_pos: None,
                sticky_focus_h: std::cell::Cell::new(0),
                sticky_drag: false,
                last_line_h: None,
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
                hl_anim: std::cell::Cell::new(None),
                hl_rect: std::cell::Cell::new(None),
                hl_prev: std::cell::Cell::new(None),
                hl_ms: 100,
                internal_rerender: false,
                forward_hold: false,
                ylock_last_dir: std::cell::Cell::new(0),
                ylock_acc: std::cell::Cell::new(0),
                show_frame_fresh: std::cell::Cell::new(true),
            last_hide_at: std::cell::Cell::new(None),
            last_pos_anchored: std::cell::Cell::new(false),
                        size_anim: None,
                chrome_override: std::cell::Cell::new(None),
                size_ms: 90,
                size_anim_dur: std::cell::Cell::new(90),
                size_anim_last_render: std::cell::Cell::new(std::time::Instant::now()),
                size_morph: false,
                pos_anim: None,
                chase_target: None,
                chase_last: None,
                chase_pos: None,
                pos_ms: 100,
                anim_on: std::cell::Cell::new(true),
                anim_spd: std::cell::Cell::new(1.0),
                live_pos: std::cell::Cell::new((0, 0)),
                live_size: std::cell::Cell::new((0, 0)),
                content_size: std::cell::Cell::new((0, 0)),
                comments_expanded: true,
            })
        }
    }

    fn ensure_swapchain(&mut self, w: u32, h: u32) -> bool {
        // 【ULW 模式】无 swapchain：离屏 D2D 位图当渲染目标（grow-only
        // 同策略）。Present 侧读回 → DIB → UpdateLayeredWindow。
        if self.ulw {
            if self.offscreen.is_some() && w <= self.size.0 as u32 && h <= self.size.1 as u32 {
                return true;
            }
            unsafe {
                if let Some(ctx) = &self.ctx {
                    ctx.SetTarget(None);
                }
                let alloc_w = (((w.max(256)) + 63) / 64) * 64;
                let alloc_h = (((h.max(160)) + 63) / 64) * 64;
                let Some(ctx) = &self.ctx else { return false };
                let bp = D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                    colorContext: std::mem::ManuallyDrop::new(None),
                };
                match ctx.CreateBitmap(
                    windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_U {
                        width: alloc_w,
                        height: alloc_h,
                    },
                    None,
                    0,
                    &bp,
                ) {
                    Ok(b) => {
                        self.offscreen = Some(b);
                        self.size = (alloc_w as i32, alloc_h as i32);
                        crate::tsf::trace(&format!(
                            "cw2 ulw: 离屏位图 {alloc_w}×{alloc_h}（内容 {w}×{h}）"
                        ));
                        true
                    }
                    Err(_) => {
                        crate::tsf::trace("cw2 ulw: CreateBitmap FAIL");
                        false
                    }
                }
            }
        } else {
            self.ensure_swapchain_dcomp(w, h)
        }
    }

    /// 【ULW 呈现】离屏 D2D 位图读回 → DIB → UpdateLayeredWindow。
    /// w/h = 本帧内容尺寸（像素，含阴影边距，与 SetWindowPos 一致）。
    /// 失败静默（下帧重试）；DIB/DC 按尺寸变化重建（常驻复用）。
    unsafe fn present_ulw(&mut self, w: i32, h: i32) {
        use windows::Win32::Graphics::Direct2D::{
            D2D1_BITMAP_OPTIONS, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_CPU_READ,
            D2D1_BITMAP_PROPERTIES1, ID2D1Bitmap,
        };

        if w <= 0 || h <= 0 {
            return;
        }
        let (Some(ctx), Some(off)) = (&self.ctx, &self.offscreen) else {
            return;
        };
        // 1) 读回：offscreen(TARGET) → cpu(CPU_READ) → Map
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS(
                D2D1_BITMAP_OPTIONS_CPU_READ.0 | D2D1_BITMAP_OPTIONS_CANNOT_DRAW.0,
            ),
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let Ok(cpu) = ctx.CreateBitmap(
            windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_U {
                width: w as u32,
                height: h as u32,
            },
            None,
            0,
            &props,
        ) else {
            return;
        };
        let copied = (|| {
            let src: ID2D1Bitmap = off.cast().ok()?;
            cpu.CopyFromBitmap(
                None,
                Some(&src),
                Some(&windows::Win32::Graphics::Direct2D::Common::D2D_RECT_U {
                    left: 0,
                    top: 0,
                    right: w as u32,
                    bottom: h as u32,
                }),
            )
            .ok()
        })();
        if copied.is_none() {
            return; // cpu 引用 drop 即释放
        }
        let Ok(mapped) = cpu.Map(windows::Win32::Graphics::Direct2D::D2D1_MAP_OPTIONS_READ) else {
            return;
        };

        // 2) DIB（尺寸变化重建；top-down 32bpp，premultiplied 直传）
        if self.ulw_hbm == 0 || self.ulw_w != w || self.ulw_h != h {
            if self.ulw_hbm != 0 {
                let _ = DeleteObject(HGDIOBJ(self.ulw_hbm as *mut _));
                self.ulw_hbm = 0;
            }
            if self.ulw_dc != 0 {
                let _ = DeleteDC(HDC(self.ulw_dc as *mut _));
                self.ulw_dc = 0;
            }
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: 0, // BI_RGB
                    biSizeImage: (w * h * 4) as u32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let hbm = match CreateDIBSection(
                HDC(std::ptr::null_mut()),
                &bi,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            ) {
                Ok(b) => b,
                Err(_) => {
                    let _ = cpu.Unmap();
                    return;
                }
            };
            if hbm.is_invalid() || bits.is_null() {
                let _ = cpu.Unmap();
                return;
            }
            let dc = CreateCompatibleDC(HDC(std::ptr::null_mut()));
            if dc.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(hbm.0));
                let _ = cpu.Unmap();
                return;
            }
            let old = SelectObject(dc, HGDIOBJ(hbm.0));
            if old.is_invalid() {
                let _ = DeleteDC(dc);
                let _ = DeleteObject(HGDIOBJ(hbm.0));
                let _ = cpu.Unmap();
                return;
            }
            self.ulw_hbm = hbm.0 as isize;
            self.ulw_dc = dc.0 as isize;
            self.ulw_w = w;
            self.ulw_h = h;
            self.ulw_bits = bits as isize;
        }
        // 3) 像素搬运（Map pitch → DIB 连续）
        if self.ulw_bits != 0 {
            let dst = self.ulw_bits as *mut u8;
            let pitch = mapped.pitch as usize;
            let src = mapped.bits as *const u8;
            let row = (w as usize) * 4;
            for r in 0..(h as usize) {
                std::ptr::copy_nonoverlapping(src.add(r * pitch), dst.add(r * row), row);
            }
        }
        let _ = cpu.Unmap();

        // 【五十六修·空帧拦截】扫 DIB alpha：全透明=渲染侧本帧清屏
        // 后未落笔（裁剪错位/设备抖动等偶发）。此前照常上屏=用户看
        // 到候选整窗消失一瞬（30fps 抓帧实锤 646B 级全透明帧）；现
        // 在跳过本帧 ULW，屏上保留上一好帧，下一帧真内容接上——
        // 闪不动。首帧（从未上过内容）不拦（窗口本来就没画面）。
        // 采样步长 8（每 8 像素查一点的 alpha 字节），153×105 帧
        // ~250 次字节读，纳秒级。
        if self.ulw_bits != 0 {
            let dst = self.ulw_bits as *const u8;
            let row_bytes = (w as usize) * 4;
            let stride = if w as usize > 8 { 8 } else { 1 };
            let mut any_opaque = false;
            'scan: for r in 0..(h as usize) {
                let base = dst.add(r * row_bytes);
                let mut c = 3usize; // BGRA 的 A 字节
                while c < row_bytes {
                    if *base.add(c) != 0 {
                        any_opaque = true;
                        break 'scan;
                    }
                    c += 4 * stride;
                }
            }
            if !any_opaque {
                if self.ulw_last_nonempty {
                    if crate::tsf::trace_on() {
                        crate::tsf::trace("cw2: 空帧拦截——本帧全透明，保留上一好帧（五十六修）");
                    }
                    return;
                }
            } else {
                self.ulw_last_nonempty = true;
            }
        }

        // 4) ULW 上屏（premultiplied AC_SRC_ALPHA；尺寸=窗口尺寸）
        let blend = BLENDFUNCTION {
            BlendOp: 0, // AC_SRC_OVER
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: 1, // AC_SRC_ALPHA（预乘；同阴影窗）
        };
        let pt = POINT { x: 0, y: 0 };
        let sz = SIZE { cx: w, cy: h };
        let ok = UpdateLayeredWindow(
            self.hwnd,
            None,
            None,
            Some(&sz as *const SIZE),
            HDC(self.ulw_dc as *mut _),
            Some(&pt as *const POINT),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        if ok.is_err() {
            crate::tsf::trace("cw2 ulw: UpdateLayeredWindow FAIL");
        }
    }

    fn ensure_swapchain_dcomp(&mut self, w: u32, h: u32) -> bool {
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
                // 会把 DComp 表面提升到 MPO overlay——半透帧被拍平。
                //（二十四修起无逐帧 alpha 动效，此限制不再触及。）
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
    /// 【高亮滑动 2026-10-09】高亮胶囊矩形插值：hl_anim 在身 → 从起点
    /// 矩形 ease-out（起步快收尾缓）滑到目标；到点即清。恒记录本帧
    /// 矩形（下一程的起点）。返回实际四边（LTRB，内容坐标）。
    fn hl_slide_rect(&self, target: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
        let mut r = target;
        if let Some((fr, t0, dur)) = self.hl_anim.get() {
            let d = dur.max(1) as f32;
            let p = (t0.elapsed().as_millis() as f32 / d).clamp(0.0, 1.0);
            if p >= 1.0 {
                self.hl_anim.set(None);
            } else {
                let e = 1.0 - (1.0 - p) * (1.0 - p);
                r = (
                    fr.0 + (target.0 - fr.0) * e,
                    fr.1 + (target.1 - fr.1) * e,
                    fr.2 + (target.2 - fr.2) * e,
                    fr.3 + (target.3 - fr.3) * e,
                );
            }
        }
        self.hl_rect.set(Some(r));
        r
    }

    pub fn show(
        &mut self,
        cands: &[(String, String)],
        raw: &str,
        skin: &Value,
        anchor: Option<&RECT>,
        selected: usize,
    ) {
        // 【三十四修·chase 修正 2】show 前置流程（3525 行钳位段）每帧都会
        // 把 sticky_pos 覆盖成本帧锚点——chase 首显起点若在定位段才读，
        // 「上一段落点」已变「本段落点」，起点≡终点，追赶永不臂=全程直
        // 出（用户实测）。函数头先抢救上一帧的 sticky。
        let sticky_prev = self.sticky_pos;
        // 【四十三修·咽喉宽度门 2026-09-22】Excel 实测（探针+trace 实锤，
        // 2026-09-22 复测确认 update_ui 层门不够）：焦点抖动路径
        //（CommitFocusRevoke 保组段）不经 update_ui 锚链直接 show，
        // 把公式栏编辑条整框 (750,531,2334,553)（1584px 宽、常数矩形、
        // 与所点格无关）当锚喂进来 → 候选追其右缘 (2305,528) 逃逸=
        // 「每个单元首键候选位置都不对」的全部真相。本门装在 show
        // 咽喉：表格宿主（excel/wps 系）+ 锚宽 >400px → 一律视为垃圾
        //（真插入符/组段框/EXCEL6 框 ≤~250px），降级为无锚帧——窗口
        // 钉在上一好位，等下一帧真值。所有车道（update_ui/焦点重显/
        // 补显/反查）必经此处，无处可绕。
        let anchor = if (crate::tsf::host_is_wps() || crate::tsf::host_is_excel())
            && anchor.is_some_and(|a| a.right - a.left > 400)
        {
            if crate::tsf::trace_on() {
                crate::tsf::trace("cw2: 咽喉门——锚宽>400 表格宿主整框垃圾锚→降级无锚");
            }
            None
        } else {
            anchor
        };
        // 【四十四修·退化锚行高补齐 2026-09-22】微信4.0 实锤（用户
        // 实打 trace）：同一插入点交替上报两种锚矩形——16px 全高
        //（正常，(1142,1174,1156,1190)）与 1px 退化（GetTextExt 抽
        // 风帧，(1151,1174,1153,1175)）。top 恒同（行根本没动）而
        // bottom 差 15px → 落点 y=bottom+4 逐键 ±15px 荡秋千（y 稳
        // 定锁拦不住：其放行条件=单步 ≥4 且同向累计 ≥8，双向 ±15
        // 大步每步都过门槛——锁为 WPS ±1-2 锯齿与单向滚动设计）。
        // 修法：高度 <8px 的退化锚用本进程缓存的上一个正常行高（≥8）
        // 补齐 bottom——两种形状算出同一落点，Y 恒定。无缓存（从未
        // 见过正常锚）不赌默认值，原样放行，拿到第一个正常锚即生效。
        let norm_rect: RECT;
        let anchor = match anchor {
            Some(a) => {
                let h = a.bottom - a.top;
                if h < 8 {
                    match self.last_line_h {
                        Some(lh) if lh >= 8 => {
                            norm_rect = RECT { bottom: a.top + lh, ..*a };
                            if crate::tsf::trace_on() {
                                crate::tsf::trace(&format!(
                                    "cw2: 退化锚补齐 高{h}→{lh}（宿主抽风帧，Y 荡秋千根治）"
                                ));
                            }
                            Some(&norm_rect)
                        }
                        _ => Some(a),
                    }
                } else {
                    self.last_line_h = Some(h);
                    Some(a)
                }
            }
            None => None,
        };
        // 【四十一修·show 入口观测（排障期）】在一切检查之前无条件打：
        // 定位"窗显示在旧位但不经观测"的矛盾（疑似 host 门提前 return
        // 或别的 SetWindowPos 直调）。
        // 【二十五修】先查开关再 format!（生产零分配）
        if crate::tsf::trace_on() {
            match anchor {
                Some(a) => crate::tsf::trace(&format!(
                    "cw2: show[入] 锚=({},{},{},{}) n={} raw='{}' ir={} pa={:?} sa={:?}",
                    a.left, a.top, a.right, a.bottom,
                    cands.len(),
                    raw.chars().take(8).collect::<String>(),
                    self.internal_rerender,
                    self.pos_anim.is_some(),
                    self.size_anim.map(|(f, t, _)| (f, t)),
                )),
                None => crate::tsf::trace("cw2: show[入] 锚=None"),
            }
            // 【四十七修·锚点对照观测】桌面重命名「候选离得远」排查：
            // 锚矩形与**焦点窗真实屏幕矩形**并排——锚来自哪条查询链
            //（GetTextExt/selection/系统插入符/est）一眼可辨真伪。
            unsafe {
                #[link(name = "user32")]
                unsafe extern "system" {
                    fn GetGUIThreadInfo(tid: u32, gi: *mut GTI) -> i32;
                    #[link_name = "GetClassNameW"]
                    fn GetClassNameW2(hwnd: HWND, s: *mut u16, c: i32) -> i32;
                }
                #[repr(C)]
                struct GTI {
                    cb: u32,
                    flags: u32,
                    hwnd_active: HWND,
                    hwnd_focus: HWND,
                    hwnd_capture: HWND,
                    hwnd_menu_owner: HWND,
                    hwnd_move_size: HWND,
                    hwnd_caret: HWND,
                    rc_caret: RECT,
                }
                let mut gi = GTI {
                    cb: std::mem::size_of::<GTI>() as u32,
                    flags: 0,
                    hwnd_active: HWND(std::ptr::null_mut()),
                    hwnd_focus: HWND(std::ptr::null_mut()),
                    hwnd_capture: HWND(std::ptr::null_mut()),
                    hwnd_menu_owner: HWND(std::ptr::null_mut()),
                    hwnd_move_size: HWND(std::ptr::null_mut()),
                    hwnd_caret: HWND(std::ptr::null_mut()),
                    rc_caret: RECT::default(),
                };
                if GetGUIThreadInfo(0, &mut gi) != 0 {
                    let fw = gi.hwnd_focus;
                    if !fw.0.is_null() {
                        let mut fr = RECT::default();
                        if GetWindowRect(fw, &mut fr).is_ok() {
                            let mut cls = [0u16; 32];
                            let n = GetClassNameW2(fw, cls.as_mut_ptr(), 32);
                            let cn = String::from_utf16_lossy(
                                &cls[..cls.iter().position(|&c| c == 0).unwrap_or(n.max(0) as usize)],
                            );
                            crate::tsf::trace(&format!(
                                "cw2: 锚对照 焦点窗={cn}@0x{:x} rect=({},{},{},{})",
                                fw.0 as usize, fr.left, fr.top, fr.right, fr.bottom
                            ));
                        }
                    }
                }
            }
        }
        // 【二十五修·闪帧收窗取消】新 show 到来=新组段开打——挂起的
        // hide_later 定时器（上段选重闪帧的收尾）必须取消，否则会在
        // 新组段显示中把窗收走（110ms 内 poll 才补回=一闪）。
        // 【二十六修·复渲染豁免】动画 tick 的复渲染（internal_rerender=
        // true：fade_tick 尺寸/高亮滑动步进）不是新内容，却同样走到这
        // 里——高亮滑动 ~240ms 内每 5ms 杀一次收场定时器，把闪帧的
        // 0.15s 收尾吃成永不触发，窗口残留到 2 秒宿主资格窗过期才被
        // 轮询兜底收掉（用户实锤「bu; 选重后候选留约两秒」，实测
        // 1.4s/~2s，组段句柄未清的宿主 >4s 不收）。复渲染不杀定时器。
        if !self.internal_rerender {
            unsafe {
                let _ = KillTimer(self.hwnd, HIDE_LATER_TIMER_ID);
            }
        }
        // 【排障后注】本观测+SWP主观测已破案（四十二修：EXCEL6 框外
        // 钉死），保留为诊断资产但降频：锚=None（tick 重渲染等非锚帧）
        // 不打，减少常规 trace 量。
        // 【三十九次修正·show 级焦点守卫】本进程非前台宿主（且非同族
        // /UWP 框架）→ 隐藏返回：失焦进程的残留会话不再显示。et.exe
        //（表格宿主）经同目录豁免放行——前台框架窗属 wps.exe。
        if !crate::tsf::host_may_show() {
            crate::tsf::trace("cw2: show被host门拦→SW_HIDE");
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            return;
        }
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
        // 【动效】注释展开延时。皮肤键（layout 节，缺省走代码默认）：
        // comment_delay_ms=400（0=注释常显）。时长键 size_ms/pos_ms/
        // hl_ms 见下方各自读取处。
        let was_visible = unsafe { IsWindowVisible(self.hwnd).as_bool() };
        // 【动效全局开关+速度】设置页·皮肤页：anim（bool，
        // 默认开）/ anim_speed（倍率，默认 1.0=当前速度）——server 注入
        // 皮肤对象顶层。关闭=一切动效瞬跳；速度统一乘
        // 尺寸/位置/高亮滑动时长。
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
            // 【口径 0~2 2026-09-11 用户拍板】滑条 0%–200%（0=瞬跳：
            // 时长×0=0，各动效起臂门 size_ms>0 等自然不臂）
            .clamp(0.0, 2.0) as f32;
        // 【owned 窗动效恢复 2026-09-11 二次】瞬跳版根除了直角但用户
        // 反馈「没动效了」——恢复 owned 窗动效（与常规宿主同源同参数）。
        // 直角防线保留：①方形 Clip+两轴元素钳制（cr/cb，横竖排对称）；
        // ②动画帧位图=缓动尺寸逐帧 ULW，无拉伸残影。若实测直角再现
        // 再回退瞬跳（用户拍板）。
        self.anim_on.set(anim_on);
        let anim_spd = if anim_on { anim_spd } else { 0.0 };
        self.anim_spd.set(anim_spd);
        // 【透明度渐变退役·二十四修】半透面板+深色底下任何 alpha
        // 过渡都「变深/透底」（用户三度否决）——fade 全链已删。
        // 【四十五修·基准再提速 2026-10-29】皮肤缺省档同步提速：尺寸
        // 形变 150→110ms、高亮滑动 100→80ms（「基准速度再快一些」；
        // pos_ms 只管 chase/辅助路径不动）。
        self.size_ms = (layout_f(skin, "size_ms", 110.0).clamp(0.0, 600.0) * anim_spd) as u32;
        self.pos_ms = (layout_f(skin, "pos_ms", 75.0).clamp(0.0, 600.0) * anim_spd) as u32;
        // 【六修·虎娘对齐】形变退役 → 【七修修订】形变保留（用户实测
        // 要的是去回弹不是去形变；线性化已根除回弹），默认开，
        // 皮肤 layout.size_morph=0 显式关。
        self.size_morph = layout_f(skin, "size_morph", 1.0) > 0.5;
        // 【虎娘对齐·首显滑动 2026-09-18】非整句组段首显：server 方案门
        // 控（first_show_slide，缺键=不滑）+ 渲染点注入的每键宽
        // （first_show_unit）。起点不靠注入坐标——四修：注入帧的
        // raw/caret 新鲜度随宿主帧序漂移，改为 show() 内从目标位反推：
        // fx = tx − 编码长×unit（「编码左端」= 光标右 − 编码宽，时序
        // 无关）。时长固定 ~100ms（虎娘探针实测同拍；逐键跟随原本就
        // 有；入场长大已随二十四修退役，不叠加）。
        // 【三修·两形态】g.skin=管道响应包装层（{skin:{...},...}）——
        // anim 同款「顶层或 /skin/ 两形态都认」，只查顶层永远 None
        //（实测 slide 恒 false=「怎么没效果」的真根因）。
        let skin_value = |key: &str| -> Option<&serde_json::Value> {
            skin.get(key).or_else(|| skin.get("skin").and_then(|s| s.get(key)))
        };
        let first_show_unit = skin_value("first_show_unit").and_then(|v| v.as_f64());
        let first_show_slide = skin_value("first_show_slide")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // 【高亮滑动 2026-10-09】胶囊滑动时长（二十七修定稿 100ms——
        // 收场钟 150ms 不变：滑动先播完，留一拍确认再收；皮肤 hl_ms
        // 可调，0=瞬跳）。
        self.hl_ms = (layout_f(skin, "hl_ms", 80.0).clamp(0.0, 600.0) * anim_spd) as u32;
        // 【二十五修·注释提速 2026-10-09】默认 400→200：注释列晚半拍
        // 展开=「候选慢半拍」观感主源之一（皮肤显式配置不受影响）。
        let cmt_delay = layout_f(skin, "comment_delay_ms", 200.0).clamp(0.0, 5000.0) as u32;
        if !was_visible {
            // 新组段首显：注释展开态重置（0=常显直接展开）
            self.comments_expanded = cmt_delay == 0;
        }
        // 【高亮滑动 2026-10-09】高亮下标变化（↑↓ 移动 / 数字、；选重
        // 闪帧）→ 胶囊从上一帧渲染矩形滑到新位（用户规格：uru3 要看
        // 到高亮移过去，箭头移动同款动效）。【二修 2026-10-09】列表变
        // 化也滑：提前上屏后的新段首键（bu;「好的」上屏→按 b，高亮从
        // 锁位次跳回第 1 项）从上一帧矩形滑过去——只对「位次跳变」生
        // 效；同位不滑（普通逐键 0→0，列表宽度微变，连打不漂移不闪）。
        // 跨会话首显（was_visible=false）不滑；动效 tick 复渲染不算。
        // prev 记在窗体（shared.last_show 锁外不可达，窗自记即够；比较
        // 用抑制前的原列表，与渲染入参同源）。
        if !self.internal_rerender && self.hl_ms > 0 {
            let prev = self.hl_prev.take();
            if was_visible {
                let prev_sel = prev.map(|(_, s)| s).unwrap_or(selected);
                if prev_sel != selected {
                    if let Some(fr) = self.hl_rect.get() {
                        crate::tsf::diag_note("动效: 高亮滑动起臂");
                        self.hl_anim
                            .set(Some((fr, std::time::Instant::now(), self.hl_ms)));
                        unsafe {
                            anim_tick_arm(self.hwnd);
                        }
                    }
                } else {
                    // 同位（常规逐键刷新/闪帧尾段二次渲染）：不动在身滑动
                    //——到点自清（连续反向移动=矩形连续插值）。
                }
            }
            self.hl_prev.set(Some((cands.to_vec(), selected)));
        }
        if !self.comments_expanded && cmt_delay > 0 && !self.internal_rerender {
            // 连打期间逐帧重置倒计时（同 id SetTimer=重置）→ 停手
            // delay 后补一帧全注释；展开后保持到本组段结束。动效 tick
            // 的内部复渲染不重置（否则动画期每次 tick 都推迟展开）。
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
        // 【八修·竖排最低宽裁剪 2026-09-18 用户拍板】竖排短内容被皮肤
        // min_width(150/120) 顶出右侧留白，观感空旷——读取后统一 ×⅔
        //（150→100、120→80），硬底 100→66。乘法在显式键之后=全皮肤
        // 生效（改默认值对显式皮肤无效）；横排不走此值（09-08 已纯
        // 自适应），固定宽皮肤（width>0）不经过 clamp，均不受影响。
        let min_width = (layout_f(skin, "min_width", 150.0) * 2.0 / 3.0).max(66.0);
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
        // 【DPI 口径修复】need/w 全是 96-DPI 逻辑像素，此前 w_cap 直接用
        // 物理屏宽：150% 缩放时横排候选可长到 1.5×屏宽（逻辑）≈2.25×
        // （物理）——资源管理器搜索框实测候选超出屏幕的主因之一。
        // 换算到逻辑域再封顶。
        let w_cap = ((screen_w - 24.0) / dpi_scale).max(320.0);
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
            // 【三十六修·下溢修复】串长 <6 时 chars.len()-6 usize 下溢
            // panic（渲染线程崩=宿主崩）。可达路径：竖排 cap=260/240
            // 且字号放大（滚轮 33-36pt 或皮肤 font_point 无钳位）时
            // 5 字编码 est 超限直落终段。
            std::iter::once('…')
                .chain(chars[chars.len().saturating_sub(6)..].iter().copied())
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
        // 【词框候选诊断 2026-09-12】用户实测词框候选「右边字缺」——
        // 打印布局输入与产物，一次复现定位宽度错在测量还是裁剪。
        if crate::addword::is_open() {
            crate::tsf::trace(&format!(
                "cw2diag: cands={} 每行测量宽={:?} 编码行={} w={w} h={h} 行槽={row_h:.1} 横排={horizontal}",
                cands.len(),
                cand_ws
                    .iter()
                    .map(|x| (x.0 as i32, x.1 as i32, (x.2 * 100.0) as i32))
                    .collect::<Vec<_>>(),
                code_row,
            ));
        }
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
                Some((f, t, t0)) => size_ease(f, t, t0.elapsed().as_millis() as u32, self.size_anim_dur.get().max(1)),
                None => self.live_size.get(),
            };
            if self.readback {
                self.size_anim = None;
                self.chrome_override.set(None);
            } else if was_visible
                && self.size_morph
                && self.size_ms > 0
                // 【起臂阈值 24/14 2026-09-11】普通逐字打字的行宽增长
                // （~8-17px/键）不动画——文字即时更新（连打不闪不滞后，
                // 用户实测逐键闪的规避）；动画留给结构性大变化（注释
                // 展开、横竖切换、候选大改）。旧 10/8 阈值=几乎每键起
                // 臂 → 内容层遮罩逐键在岗 = 文字闪的放大器。
                // 【八修配套·阈值 14 2026-09-18 用户拍板「降到14看看」】
                // 竖排窄窗后 14-24px 的宽度变化也常见（最低宽裁剪），
                // 瞬跳显突兀——宽度阈值 24→14（高度 14 不动），小变化
                // 也走形变；胶囊右缘已贴壳（ chw），逐键起臂不再闪。
                && ((target.0 - cur.0).abs() > 14 || (target.1 - cur.1).abs() > 14)
                // 【五十七修·零臂禁发】cur 退化（≤4px：藏窗处理把
                // live_size 清 (0,0)，藏后复显/焦点尾迹竞态里 show 在
                // SW_HIDE 落地前后读到 was_visible=true + live_size=(0,0)）
                // 时臂形变=从零长起：中间帧外壳≈0，内容裁剪全空=250fps
                // 空帧风暴（五十六修空帧拦截 47 连击实锤，sa=Some((0,0),
                // (153,105)) 60+ 帧全程空）。退化基准不臂——一步落目标
                // 尺寸；新会话观感由入场滑入（pos_anim）负责，尺寸形变
                // 只服务「真实壳→真实壳」的结构变化。
                && cur.0 > 4
                && cur.1 > 4
            {
                // 【五十九修·形变恒速】时长按剩余距离定（60px 满程跑
                // 满 size_ms，小距离按比例缩短、下限 24ms）——连打每
                // 键 ~14px 增长只跑 ~25ms（2-3 帧），不再每键重臂重置
                // 全 110ms（后者=连打全程 200fps 整帧重绘风暴 + 末键
                // 后 36 帧/+145ms 的形变尾巴，QQ 慢线程放大成秒级
                // 「打完了还在挨个出编码挨个形变」）。结构性大变化观
                // 感不变。
                let dist_px = (target.0 - cur.0)
                    .abs()
                    .max((target.1 - cur.1).abs())
                    .max(1) as f32;
                let dur = ((self.size_ms as f32) * (dist_px / 60.0))
                    .clamp(24.0, self.size_ms.max(24) as f32) as u32;
                self.size_anim_dur.set(dur.max(1));
                self.size_anim = Some((cur, target, std::time::Instant::now()));
                // 起臂帧即按当前尺寸渲染外壳（否则首帧按目标画、下一
                // tick 又缩回=边缘/阴影跳一下）
                self.chrome_override.set(Some(cur));
                crate::tsf::diag_note(&format!("cw2 尺寸起臂: {cur:?}→{target:?} dur={dur}"));
                unsafe {
                    anim_tick_arm(self.hwnd);
                }
            } else {
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
        // 【二十四修·动效大瘦身】入场/退场动画退役——bx/by 恒 0（稳态）。
        let (bx, by) = (0.0f32, 0.0f32);
        {
            // 【零位移配套】缓冲按实际窗口（含收窄轴向的缓动值——收窄帧
            // 窗口=缓动≥目标；grow-only 下不触发重建，宽缓冲沿用）。
            let (buf_w, buf_h) = match self.size_anim {
                Some((f, t, t0)) => {
                    let e = size_ease(f, t, t0.elapsed().as_millis() as u32, self.size_anim_dur.get().max(1));
                    (e.0.max(w_out as i32), e.1.max(h_out as i32))
                }
                None => (w_out as i32, h_out as i32),
            };
            if !self.ensure_swapchain(buf_w.max(1) as u32, buf_h.max(1) as u32) {
                crate::tsf::trace("cw2: ensure_swapchain FAIL");
                return;
            }

            unsafe {
                let ctx = match &self.ctx {
                    Some(c) => c.clone(),
                    None => return,
                };
                // 【ULW 模式】目标=离屏位图（无 swapchain/GetBuffer），
                // 绘制代码零改动（同 ID2D1DeviceContext），Present 侧分叉。
                let bitmap = if self.ulw {
                    match &self.offscreen {
                        Some(b) => b.clone(),
                        None => return,
                    }
                } else {
                    let chain = match &self.swapchain {
                        Some(c) => c.clone(),
                        None => return,
                    };
                    // 【后台缓冲索引修复 2026-09-11】FLIP_DISCARD+BufferCount=2
                    // 下 Present 后索引在 0/1 轮转——此前恒画 GetBuffer(0)=
                    // 隔帧画进正在显示的前台缓冲：内容不变时同像素看不出来，
                    // 一变（候选框尺寸/内容更新）就闪（用户实测「体积有变
                    // 化文字就闪」的真根因，与动画无关、一直潜在）。必须画
                    // GetCurrentBackBufferIndex() 返回的当前后台缓冲。
                    //（windows 0.58 该方法 impl 在 IDXGISwapChain3 上——cast 取用）
                    let bb_index = match chain.cast::<IDXGISwapChain3>() {
                        Ok(c3) => c3.GetCurrentBackBufferIndex(),
                        Err(_) => 0,
                    };
                    let surface: IDXGISurface = match chain.GetBuffer(bb_index) {
                        Ok(s) => s,
                        Err(_) => return,
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
                    match ctx.CreateBitmapFromDxgiSurface(&surface, Some(&bp)) {
                        Ok(b) => b,
                        Err(_) => return,
                    }
                };
                ctx.SetTarget(&bitmap);
                ctx.BeginDraw();
                // 【二十四修·动效大瘦身】渐隐渐显全链退役——恒不透明，
                // 整帧 alpha PushLayer 零开销路径已删（稳态分支本就恒真）。
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
                            {
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
                            {
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
                // 【方形 Clip 定稿 2026-09-11】两种圆角层形态（dpi 变换下
                // 推层 / identity+物理坐标照抄阴影外遮罩）在用户驱动上都
                // 引发「层内文字逐帧丢画」=文字闪——用户机对「文字画在
                // 几何遮罩层里」不稳（阴影遮罩只包几何填充且有缓存，故
                // 从未暴露）。定稿=恒方形 AxisAlignedClip（实锤无闪）。
                // 直角由「填充元素几何钳制」解决：编码行底/高亮胶囊的
                // 矩形在动效帧钳进动画壳内（各自自带圆角），直边不再触
                // 遮罩缘；文字仅稀疏字形碰缘（90ms 内不可见）。
                let chrome_clip_on = self.chrome_override.get().is_some();
                if chrome_clip_on {
                    ctx.PushAxisAlignedClip(
                        &D2D_RECT_F {
                            left: bx,
                            top: by,
                            right: bx + chw,
                            bottom: by + chh,
                        },
                        D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    );
                }
                // 【直角消除 2026-09-11】动效帧把填充元素（编码行底/高亮
                // 胶囊）的几何钳进动画壳内（右/下缘收到壳内缩进处）——
                // 它们自带圆角，钳入后直边不再触方形遮罩缘=无直角；稳态
                // 帧 None=原样。D2D 对退化圆角矩形自动缩半径，安全。
                let chrome_in_r: Option<f32> = chrome_clip_on.then_some(bx + chw - rm_x);
                let chrome_in_b: Option<f32> = chrome_clip_on.then_some(by + chh - rm_y);
                let cr = |v: f32| match chrome_in_r {
                    Some(lim) => v.min(lim),
                    None => v,
                };
                let cb = |v: f32| match chrome_in_b {
                    Some(lim) => v.min(lim),
                    None => v,
                };

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
                // 【反查灰条修复 2026-09-11】判定必须用皮肤自带 alpha——
                // 旧代码先 elem_alpha（a←master）再判 >0.01：皮肤明确
                // 透明（出厂全皮肤 alpha=00）也被强改 0.68 恒画。深色底
                // 不可见，白底（Typora）露出编码行位置的全宽暗带；普通
                // 打字编码行为空（内联）不触发，反查必带编码行 → 反查
                // 专属症状。顺序：自带 alpha 判定 → 可见者才按 master
                // 归一出画刷（可见皮肤的既有语义不变）。
                let b_preedit_bg = {
                    let c = color_f(skin, "preedit_back_color", "#00000000");
                    (c.a > 0.01).then(|| mkbrush(&ctx, elem_alpha(c))).flatten()
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
                                    right: cr(width - rm_x),
                                    bottom: cb(rm_y + line_h),
                                },
                                radiusX: 4.0,
                                radiusY: 4.0,
                            };
                            ctx.FillRoundedRectangle(&rr, bg);
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
                                if let Some(b) = &b_hi {
                                    let (pt, pb) = pill_v(y);
                                    // 【高亮滑动 2026-10-09】目标矩形→与上
                                    // 一帧矩形插值（↑↓/选重闪帧动效）；
                                    // mark 竖条随胶囊左缘走。
                                    let hr = self.hl_slide_rect((
                                        x - hilite_pad,
                                        pt,
                                        cr(x + cell_w + hilite_pad),
                                        cb(pb),
                                    ));
                                    let rr = D2D1_ROUNDED_RECT {
                                        rect: D2D_RECT_F {
                                            left: hr.0,
                                            top: hr.1,
                                            right: hr.2,
                                            bottom: hr.3,
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
                                                left: hr.0 + (hilite_pad - mw) / 2.0,
                                                top: my,
                                                right: cr(hr.0 + (hilite_pad - mw) / 2.0 + mw),
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
                                if let Some(b) = &b_hi {
                                    let (pt, pb) = pill_v(y);
                                    // 【高亮滑动 2026-10-09】竖排：上下滑动
                                    //（左右恒满宽）；mark 竖条随胶囊走。
                                    // 【八修配套 2026-09-18】右缘贴外壳插值
                                    // 宽 chw（稳态恒等 width）：宽度形变中
                                    // 无 hl_anim 时 hl_slide_rect 直接返回
                                    // 目标矩形——用目标 width 会「壳未到而
                                    // 胶囊先到」=闪现；贴 chw 即随壳生长/
                                    // 收拢，与拉伸动效同步。
                                    let hr =
                                        self.hl_slide_rect((rm_x, pt, cr(chw - rm_x), cb(pb)));
                                    let rr = D2D1_ROUNDED_RECT {
                                        rect: D2D_RECT_F {
                                            left: hr.0,
                                            top: hr.1,
                                            right: hr.2,
                                            bottom: hr.3,
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
                                                left: hr.0 + (hilite_pad - mw) / 2.0,
                                                top: my,
                                                right: hr.0 + (hilite_pad - mw) / 2.0 + mw,
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
                // 【拉伸动效】内容裁剪收层（与上方 PushAxisAlignedClip 配对）
                if chrome_clip_on {
                    ctx.PopAxisAlignedClip();
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

                if self.ulw {
                    // 【ULW 呈现】离屏位图 → CPU_READ 拷贝 → DIB →
                    // UpdateLayeredWindow（沙盒安全：纯 GDI 上屏）。
                    // 【变窄直角根除 2026-09-12】present 尺寸必须与
                    // SWP 的 apply 同算法（增轴=目标一步到位、减轴=当前
                    // 缓动）——ULW 的 psize 会覆盖窗口尺寸：若按目标
                    // w_out 传，收窄帧窗口被一步拉到小目标，壳（缓动
                    // 中、比目标大）被窗口边缘垂直切断=直角（用户实测
                    // UWP/开始菜单变窄出直角；notepad owned 复现实锤，
                    // 中间帧右上角 90° 硬切边）。DComp 路径 Present 不
                    // 改窗口尺寸故无此问题。
                    let (pw, ph) = match self.size_anim {
                        Some((f, t, t0)) => {
                            let e = size_ease(f, t, t0.elapsed().as_millis() as u32, self.size_anim_dur.get().max(1));
                            (e.0.max(w_out as i32), e.1.max(h_out as i32))
                        }
                        None => (w_out as i32, h_out as i32),
                    };
                    self.present_ulw(pw, ph);
                } else {
                    let chain = match &self.swapchain {
                        Some(c) => c.clone(),
                        None => return,
                    };
                    let hr = chain.Present(1, DXGI_PRESENT(0));
                    if hr.is_err() {
                        crate::tsf::trace(&format!("cw2: Present 失败 0x{:08X}", hr.0 as u32));
                    }
                }
            }
        } // 渲染段结束（测量→布局→绘制→Present）

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
            // 【P6 修复 2026-09-13 三十四修】OnceLock：原版每帧同步文件
            // IO（未固定态每次 show 都读 pin.txt）——诊断旋钮读一次足够。
            {
                static PIN_HOOK: std::sync::OnceLock<Option<(i32, i32)>> =
                    std::sync::OnceLock::new();
                let hook = *PIN_HOOK.get_or_init(|| {
                    std::fs::read_to_string(r"C:\ProgramData\HuFu\diag\pin.txt")
                        .ok()
                        .and_then(|s| {
                            let t = s.trim().to_string();
                            t.split_once(',').and_then(|(a, b)| {
                                match (a.trim().parse::<i32>(), b.trim().parse::<i32>()) {
                                    (Ok(px), Ok(py)) => Some((px, py)),
                                    _ => None,
                                }
                            })
                        })
                });
                let mut pinned = CAND_PINNED.lock().unwrap_or_else(|e| e.into_inner());
                if pinned.is_none() {
                    if let Some((px, py)) = hook {
                        *pinned = Some((px, py));
                        crate::tsf::diag_note(&format!("cw2 pin.txt 钩子 ({px},{py})"));
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
            // 【DPI 口径修复 2026-10-29】width/height 逻辑 → 物理
            //（同锚点分支；pin/拖拽钉住的旧位在高分屏上同样可越
            // 出屏幕右/下缘）。
            let mp = (shadow_m * dpi_scale) as i32;
            let wp = (width * dpi_scale) as i32;
            let hp = (height * dpi_scale) as i32;
            // 【四十五修·锚到滑入】是否走 pin 分支（末尾记
            // last_pos_anchored 用）。
            let pinned_used = CAND_PINNED
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some();
            let (x, y) = if let Some((px, py)) =
                *CAND_PINNED.lock().unwrap_or_else(|e| e.into_inner())
            {
                // 【固定模式】右键固定：忽略光标锚点，钉在用户固定处
                //（跨组段/上屏/新一轮候选全部保持；右键再解除）。
                // 拖动松手会回写 pin（见 WM_LBUTTONUP）——打字必然用
                // 最新固定位。pin 同为窗口原点系：+m_off 转回锚点系。
                // 【三十六修·门控】diag_enabled 前置，pin 固定期每帧
                // show 不再白付 format! 分配。
                if crate::tsf::diag_enabled() {
                    crate::tsf::diag_note(&format!("cw2 pin use ({px},{py})"));
                }
                let x = (px + m_off).clamp(vx, (vx + vw - mp - wp).max(vx));
                let y = (py + m_off).clamp(vy, (vy + vh - mp - hp).max(vy));
                (x, y)
            } else if self.sticky_drag && self.sticky_pos.is_some() {
                // 【拖拽钉住】松手设的 sticky 优先于锚点：本组段内
                // 窗口钉在松手处不回弹（clamp 防出屏）
                let (ox, oy) = self.sticky_pos.unwrap();
                let x = ox.clamp(vx, (vx + vw - mp - wp).max(vx));
                let y = oy.clamp(vy, (vy + vh - mp - hp).max(vy));
                (x, y)
            } else {
                // 【五十修·出屏锚作废】XAML 宿主（设置搜索等）GetTextExt
                // 可能返回另一坐标空间的烂值（实测锚=(1155,-259,1163,-240)
                //——y 出屏顶 259px，候选被带出屏；单屏 (0,0)-(2520,1680)
                // 实锤非副屏坐标）。锚整体在屏外（±200px 容差）=无效，
                // 转 None 走兜底链（焦点窗内定位，必然在屏内）。
                let anchor = anchor.filter(|r| {
                    !(r.right < vx - 200
                        || r.left > vx + vw + 200
                        || r.bottom < vy - 200
                        || r.top > vy + vh + 200)
                });
                if anchor.is_none() && crate::tsf::trace_on() {
                    crate::tsf::trace("cw2: 锚出屏作废 → 兜底链");
                }
                match anchor {
                    Some(r) => {
                        // 【实时光标跟随 2026-09-12 用户拍板】窗最左=
                        // 光标最右（rect.right）——紧贴光标右侧出现。
                        // 【三十八修·总高屏幕判断】曾把 2×m_off（阴影边
                        // 距）计入 clamp 与翻转判断——【二十九修 2026-09-18
                        // 用户拍板】废除：阴影是候选的附带装饰，边界收缩
                        // 只该看候选本体。大阴影皮肤（radius 拉满→m_off
                        // ~90px）被 ext=180px 平白顶离屏边，阴影越大推得
                        // 越远=本末倒置。改回纯内容尺寸：内容贴边即可，
                        // 阴影出屏由 DWM 裁掉（分层窗部分出屏无害，阴影
                        // 本就不可交互）。竖排贴底若再现，另查内容高口径。
                        // 【DPI 口径修复 2026-10-29】width/height 是逻辑
                        // 像素，vx/vw 是物理像素——此前 clamp 混域：150%
                        // 缩放时窗口物理宽=逻辑×1.5，右缘/底缘按逻辑宽
                        // 放行=候选本体（还要加上内容在窗内左移的阴影边
                        // 距 m_phys）整块超出屏幕（资源管理器搜索框实测
                        // 「候选框超出屏幕」根因）。统一换物理像素，内容
                        // 贴边、阴影照旧允许出屏裁掉（二十九修语义）。
                        let m_phys = (shadow_m as f32 * dpi_scale) as i32;
                        let wpx = (width * dpi_scale) as i32;
                        let hpx = (height * dpi_scale) as i32;
                        // 【四十九修·小编辑框正下方】重命名类小编辑框
                        //（焦点 Edit 宽 ≤400 物理 px 且锚行在框内）：
                        // 候选左对齐框左边（正下方）——桌面重命名框仅
                        // ~84px，光标右对齐=面板大半悬在框外右侧（用户
                        // 实锤「偏右了，不在正下方」）。宽编辑器（记事本
                        // /WPS）维持光标跟随拍板语义不变。
                        let caret_x = r.right;
                        let small_edit_x = {
                            #[repr(C)]
                            struct GTI5 {
                                cb: u32,
                                flags: u32,
                                hwnd_active: HWND,
                                hwnd_focus: HWND,
                                hwnd_capture: HWND,
                                hwnd_menu_owner: HWND,
                                hwnd_move_size: HWND,
                                hwnd_caret: HWND,
                                rc_caret: RECT,
                            }
                            #[link(name = "user32")]
                            unsafe extern "system" {
                                fn GetGUIThreadInfo(tid: u32, gi: *mut GTI5) -> i32;
                            }
                            let mut gi = GTI5 {
                                cb: std::mem::size_of::<GTI5>() as u32,
                                flags: 0,
                                hwnd_active: HWND(std::ptr::null_mut()),
                                hwnd_focus: HWND(std::ptr::null_mut()),
                                hwnd_capture: HWND(std::ptr::null_mut()),
                                hwnd_menu_owner: HWND(std::ptr::null_mut()),
                                hwnd_move_size: HWND(std::ptr::null_mut()),
                                hwnd_caret: HWND(std::ptr::null_mut()),
                                rc_caret: RECT::default(),
                            };
                            let mut out = None;
                            unsafe {
                                if GetGUIThreadInfo(0, &mut gi) != 0 && !gi.hwnd_focus.0.is_null() {
                                    let mut fr = RECT::default();
                                    if GetWindowRect(gi.hwnd_focus, &mut fr).is_ok() {
                                        let w = fr.right - fr.left;
                                        // 【勘误·同修】锚右缘=编码整段文本延伸
                                        //（GetTextExt 全段矩形），小框里天然横向
                                        // 溢出（桌面重命名框 84px、编码 60px+
                                        // ——第二帧 r.right=1626>框右 1601 即
                                        // 被原「锚在框内」校验弹回光标跟随，实
                                        // 锤面板追着文本向右跑）。只要求垂直带
                                        // 与框重叠：微型焦点框里的锚只会属于它。
                                        let v_overlap = r.top < fr.bottom + 8 && r.bottom > fr.top - 8;
                                        if w > 0 && w <= 400 && v_overlap {
                                            out = Some(fr.left);
                                        }
                                    }
                                }
                            }
                            out
                        };
                        let x_base = small_edit_x.unwrap_or(caret_x);
                        if small_edit_x.is_some() && crate::tsf::trace_on() {
                            crate::tsf::trace(&format!(
                                "cw2: 小编辑框左对齐 x={x_base}（光标x={caret_x}）"
                            ));
                        }
                        let x = x_base.clamp(vx, (vx + vw - m_phys - wpx).max(vx));
                        let below = r.bottom + 4;
                        let y = if below + hpx + m_phys <= vy + vh {
                            below
                        } else {
                            (r.top - hpx - m_phys - 4).max(vy)
                        };
                        // 【五十一修·y 稳定锁复刻 v1.5.2】老版本行为档
                        // 案实测（同 harness）：v1.5.0/1.5.2 段内 T 恒定
                        // 零上移，当前版第 3 段起 T 每键爬 -8~-35px。
                        // diff 定位：v1.5.2 此处有「正向打字 y 变化≤26px
                        // 一律钉住旧 y」的稳定锁（换行级变化>26 才放
                        // 行），9-12 的系列修正把该锁删成 6px 迟滞——
                        // 虎魄锚 y 抖动 8~35px 全部穿透=「打第二个编码
                        // 候选往上移」的病根。原样恢复：同段 y 微动钉
                        // 住，换行（y 差>26，含滚动行进）照常跟随。
                        // 【五十四修·方向连续性】51 修锁把打字区平滑滚
                        // 动（单向每段 -7~-40）也吃掉=窗滞后文字半行、
                        // 累积>26 才跳（用户「换行跟随不准」实测 trace
                        // 实锤）。抖动双向、滚动单向连续：同向小步累计
                        // 到门槛才放行。
                        // 【y 锁累计 2026-10-09 七】54 修「同向第二步即
                        // 放行」留洞：WPS 锚 y 锯齿（连续 2-3 帧 ±1~±2
                        // 同向爬升后回落）借放行窗穿透=「Y 轴轻微抖动」
                        //（WPS 单字打法四轮 180 段实测：y 微变 173 次，
                        // ±1/±2 双向游走）。改累计门槛：单帧 ≤3px 钉毛
                        // 刺；同向累计 ≥8px 且单步 ≥4px 才放行（真滚动
                        // 每帧 7-40 一步即过，锯齿累计小步全钉）；换行
                        // 级 >26px 立即放行；方向反转清零重计。
                        // 【跨会话首帧自由 2026-10-09 八】真隐藏后的第
                        // 一个显示帧自由定位（焦点切换新位置不背旧锁/
                        // 旧累计——反向 4-26px 小位移钉错位洞）。
                        // 【首帧微差仍钉 2026-10-09 十】fresh 只放行
                        // >3px 位移：≤3px 的段首锚差（WPS 锯齿 ±1/±2，
                        // u+空格 单键上屏流每段重现）钉住旧 y——跨焦点
                        // 大跳照常自由、段首微抖吃掉。
                        // 【二十五修·y 锁跨段延续 2026-10-09】上屏即收
                        // 后每段都 hide→show，若每次首帧都自由，段间锚
                        // y 锯齿（WPS ±4~30px）全数穿透=「偶发抖一下」
                        // （平稳宿主锚 y 恒定故无感）。1.5s 内近距重显
                        //（dx≤120/dy≤80，同文档打字节奏）=延续：不按
                        // 首帧自由处理，y 锁钉住段间锯齿；文本行距量化
                        //（同行≈0 / 跨行>26px），被钉住的不是真实换行。
                        // 焦点切换由 focus_reset 清 sticky（near 必假）
                        // 走自由；超时/远跳（点击换位）照常自由。
                        let mut fresh = self.show_frame_fresh.replace(false);
                        if fresh {
                            let cont = self
                                .last_hide_at
                                .get()
                                .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(1500));
                            let near = match self.sticky_pos {
                                Some((ox, oy)) => (x - ox).abs() <= 120 && (y - oy).abs() <= 80,
                                None => false,
                            };
                            if cont && near {
                                fresh = false;
                            }
                        }
                        let dy_lock = y - match self.sticky_pos {
                            Some((_, oy)) => oy,
                            None => y,
                        };
                        if fresh {
                            self.ylock_last_dir.set(0);
                            self.ylock_acc.set(0);
                        }
                        let same_dir = self.ylock_last_dir.get() != 0
                            && dy_lock != 0
                            && (self.ylock_last_dir.get() > 0) == (dy_lock > 0);
                        let acc_now = if same_dir {
                            self.ylock_acc.get() + dy_lock.abs()
                        } else {
                            dy_lock.abs()
                        };
                        let allow = (fresh && dy_lock.abs() > 3)
                            || dy_lock.abs() > 26
                            || (same_dir && acc_now >= 8 && dy_lock.abs() >= 4);
                        if dy_lock.abs() > 26 {
                            self.ylock_last_dir.set(0);
                            self.ylock_acc.set(0);
                        } else if dy_lock != 0 {
                            self.ylock_last_dir.set(if dy_lock > 0 { 1 } else { -1 });
                            self.ylock_acc.set(acc_now);
                        }
                        let y = match self.sticky_pos {
                            Some((_, oy)) if !allow => oy,
                            _ => y,
                        };
                        // 【删棘轮 2026-09-12 十六次修正】poll 真实帧的
                        // 小拉回（2-15px）正是校准（est 单键误差），棘轮
                        // 吃掉=误差攒到 15px 阶梯放行（用户实锤「跳一下
                        // 又回来」「打多了离光标远」）。双向滑动时代位
                        // 移全靠滑动呈现，亚像素由死区吸收。
                        // 【二十二次修正·正向钳位 2026-09-12 用户拍板】
                        // 单字版 dddd 顶屏：第 5 键触发前 4 码上屏 1 字，
                        // 字宽(~20px) < 编码宽(4×11=44px) → 光标真实回
                        // 退 → 候选回跳（用户要求：不回跳，原地等光标
                        // 过来再跟）。同行 x 回退 >6px 一律钉住原位；y
                        // 下移（换行）不受限；est/raw 内部照常对齐真
                        // 相——钉住的只是显示层，后续打字锚前进越过
                        // 原位即恢复跟随（通常 2 键内）。
                        // 【二十二次/二十四次修正·顶屏钳位（许可制）】
                        // 单字版顶屏上屏「字宽<编码宽」→ 光标回退 → 候选
                        // 回跳（用户拍板：原地等光标过来再跟）。幅度法
                        //（≤80px）误伤 75px 的点击换位——改许可制：仅
                        // C&R 路径置 forward_hold，SP（点击换位）永不钳。
                        // 锚追回（x≥ox-6）/换行（y 下移）/大跳（>300px）
                        // 时钳位条件自然失效并清除许可。
                        // 【三十二次修正·钳位窗口 300→80px 2026-09-12】用户
                        // 实锤「还是首键的问题，别的没问题」：点击换位左移
                        // 100-300px 落进钳位保持窗 → 首键钉在旧位（新段第
                        // 一键显示错误位置），后续键锚前进越过旧位才恢复。
                        // 顶屏真实回退量=编码宽-上屏字宽（4 键 44-20=24px、
                        // 6 键 66-20=46px），80px 上限足够覆盖且不再吞点击
                        // 换位；换行（y 下移）/大跳照旧自动释放。
                        let hold = self.forward_hold;
                        let (x, y) = match self.sticky_pos {
                            Some((ox, oy)) if hold && x < ox - 6 && x >= ox - 80 && y <= oy + 6 => {
                                (ox, y)
                            }
                            Some((ox, oy)) if (x - ox).abs() <= 6 && (y - oy).abs() <= 6 => {
                                (ox, oy)
                            }
                            _ => {
                                if hold {
                                    self.forward_hold = false;
                                }
                                (x, y)
                            }
                        };
                        (x, y)
                    }
                    None => match self.sticky_pos {
                        // 【四十八修·跨编辑框粘位作废】焦点窗已换而本帧
                        // 锚缺失：旧粘位属于上一个编辑框（桌面连续重命名
                        // 两个文件），沿用=候选钉在上一个文件旁（用户
                        // 实锤「偏左一两个图标」）。改落当前焦点编辑框
                        // 正下方。同窗（正常打字中的锚丢失帧）沿用原语义。
                        Some(p)
                            if self.sticky_focus_matches() || self.sticky_drag =>
                        {
                            p
                        }
                        Some(_) => {
                            if crate::tsf::trace_on() {
                                crate::tsf::trace("cw2: 粘位跨编辑框作废 → 焦点编辑框正下方");
                            }
                            self.focus_edit_below((height * dpi_scale) as i32)
                        }
                        // 从未有过真实锚点且本帧也取不到：先记诊断；若无
                        // 历史位置则退到「焦点窗口内左下」而非整帧隐藏
                        //（SearchHost 等宿主 GetTextExt 常失败——搜索框候选
                        // 框不显示的病根）。下一帧锚点就绪即回到正常定位。
                        None => {
                            crate::tsf::diag_note("cw2 anchor+sticky 双缺，退到焦点窗口定位");
                            // 【任务栏误兜底修复·根治 2026-09-12】开始菜单
                            // 打开时前台=SearchHost 的 CoreWindow——rect 覆盖
                            // 全屏，且 XAML 搜索框无经典 caret（锚点链全空）
                            // → 落「焦点窗口内左下」兜底 → fr.bottom-2h =
                            // 屏底 = 候选贴任务栏（用户实锤「A 窗口打完字
                            // 后开始菜单候选到任务栏」）。上一版只过滤了
                            // 任务栏类名，全屏宿主漏网。修：SearchHost 直接
                            // (12,12)（与 server 代画路径兜底同款）；前台是
                            // 任务栏类名或全屏/近全屏窗（覆盖工作区 ≥95%）
                            // 退工作区左上安全位——全屏窗的「内左下」永远
                            // 是屏底。
                            let mut wa = RECT {
                                left: 0,
                                top: 0,
                                right: 0,
                                bottom: 0,
                            };
                            {
                                #[link(name = "user32")]
                                unsafe extern "system" {
                                    fn SystemParametersInfoW(
                                        a: u32,
                                        b: u32,
                                        p: *mut core::ffi::c_void,
                                        f: u32,
                                    ) -> i32;
                                }
                                SystemParametersInfoW(
                                    0x30,
                                    0,
                                    &mut wa as *mut RECT as *mut core::ffi::c_void,
                                    0,
                                );
                            }
                            if crate::tsf::host_is_searchhost() {
                                (12, 12)
                            } else {
                                let fg = GetForegroundWindow();
                                let mut cls: [u16; 64] = [0; 64];
                                let fg_ok = !fg.0.is_null() && {
                                    #[link(name = "user32")]
                                    unsafe extern "system" {
                                        fn GetClassNameW(hwnd: HWND, s: *mut u16, c: i32) -> i32;
                                    }
                                    GetClassNameW(fg, cls.as_mut_ptr(), 64) > 0
                                };
                                let name: String = if fg_ok {
                                    String::from_utf16_lossy(
                                        &cls[..cls.iter().position(|&c| c == 0).unwrap_or(64)],
                                    )
                                } else {
                                    String::new()
                                };
                                let mut fr = RECT {
                                    left: 0,
                                    top: 0,
                                    right: 0,
                                    bottom: 0,
                                };
                                let have_rect = fg_ok && GetWindowRect(fg, &mut fr).is_ok();
                                let waw = (wa.right - wa.left).max(1) as f32;
                                let wah = (wa.bottom - wa.top).max(1) as f32;
                                let fullscreen = have_rect && {
                                    let fw = (fr.right - fr.left).max(0) as f32;
                                    let fh = (fr.bottom - fr.top).max(0) as f32;
                                    fw >= waw * 0.95 && fh >= wah * 0.95
                                };
                                let tray_like = !fg_ok
                                    || name.contains("Shell_TrayWnd")
                                    || name.contains("TrayShowDesktop")
                                    || name.contains("Shell_SecondaryTrayWnd");
                                if tray_like || fullscreen || !have_rect {
                                    (wa.left + 16, wa.top + 16)
                                } else {
                                    let x = fr.left + 16;
                                    let below =
                                        fr.bottom - ((height as i32) * 2).min(fr.bottom - fr.top);
                                    (x, below.max(fr.top))
                                }
                            }
                        }
                    },
                }
            };
            // 【右缘兜底】正向打字的 x 单调锁（宽度增长时拒回退）会把
            // 已 clamp 的新 x 顶回旧位置——窗口变宽后旧 x+新宽超右缘
            //（用户实测：跟打器超长句候选框超出屏幕；宽度封顶后根因
            // 转到这里）。每帧输出前统一夹回，屏幕边界优先于位置记忆。
            // 【二十九修】不再预留 shadow_m（同锚点 clamp 口径：只算
            // 候选本体，阴影出屏裁掉）。
            // 【DPI 口径修复 2026-10-29】同锚点分支：width/height 逻辑
            // 像素 → 物理像素（含内容左移的阴影边距），否则 150% 缩放
            // 屏上候选本体可越出屏幕右/下缘（实测「候选框超出屏幕」）。
            let m_phys0 = (shadow_m * dpi_scale) as i32;
            let wpx0 = (width * dpi_scale) as i32;
            let hpx0 = (height * dpi_scale) as i32;
            let x = x.clamp(vx, (vx + vw - m_phys0 - wpx0).max(vx));
            let y = y.clamp(vy, (vy + vh - m_phys0 - hpx0).max(vy));
            self.sticky_pos = Some((x, y));
            // 【四十八修】记录本帧粘位归属的焦点窗（沿用判据，见字段注释）
            {
                #[repr(C)]
                struct GTI2 {
                    cb: u32,
                    flags: u32,
                    hwnd_active: HWND,
                    hwnd_focus: HWND,
                    hwnd_capture: HWND,
                    hwnd_menu_owner: HWND,
                    hwnd_move_size: HWND,
                    hwnd_caret: HWND,
                    rc_caret: RECT,
                }
                #[link(name = "user32")]
                unsafe extern "system" {
                    fn GetGUIThreadInfo(tid: u32, gi: *mut GTI2) -> i32;
                }
                let mut gi = GTI2 {
                    cb: std::mem::size_of::<GTI2>() as u32,
                    flags: 0,
                    hwnd_active: HWND(std::ptr::null_mut()),
                    hwnd_focus: HWND(std::ptr::null_mut()),
                    hwnd_capture: HWND(std::ptr::null_mut()),
                    hwnd_menu_owner: HWND(std::ptr::null_mut()),
                    hwnd_move_size: HWND(std::ptr::null_mut()),
                    hwnd_caret: HWND(std::ptr::null_mut()),
                    rc_caret: RECT::default(),
                };
                let fh = if GetGUIThreadInfo(0, &mut gi) != 0 && !gi.hwnd_focus.0.is_null() {
                    gi.hwnd_focus.0 as isize
                } else {
                    0
                };
                self.sticky_focus_h.set(fh);
            }
            // 【四十五修·锚到滑入】记录本帧显示位是否来自真实锚点/
            // 用户钉位（供 per-key 起臂读上一帧值：从兜底位→真锚的
            // 修正位移走快滑而非瞬落——资源管理器重命名/搜索框组段
            // 首帧锚缺失后锚迟到场景「动效不生效」的修复）。
            let pinned_or_drag = pinned_used || self.sticky_drag;
            self.last_pos_anchored
                .set(pinned_or_drag || anchor.is_some());
            // 诊断：搜索框等宿主锚点缺失排查（visible=0 说明本帧被隐藏）
            // + DWM cloaked 检测（显示中但被 DWM 隐身 → 连续 2 帧后
            //   由调用方切换 v1 传统混合窗——SearchHost 里 DComp 直通
            //   窗被整体 cloaked 的自愈路径）。dwmapi 经 GetProcAddress
            //   动态获取（mingw 工具链无 dwmapi 导入库）。
            let mut cloaked: u32 = 0;
            let mut hr: i32 = -1;
            {
                // 【P7 修复 2026-09-13 三十四修】函数指针 OnceLock：原版
                // 每帧 GetModuleHandleW + UTF-16 分配 + GetProcAddress
                // （动画期 15ms 一次全白付）。dwmapi 经 GetProcAddress
                // 动态获取（mingw 工具链无 dwmapi 导入库）。
                // 【i386 ABI】必须 extern "system"（stdcall）：x64 上 Rust
                // 默认约定与 Win64 恰好兼容掩盖了此错，32 位下 cdecl 调用
                // stdcall 函数 → 栈清理错位 → 崩（Pain 打器按键闪退根因）。
                type Dwma =
                    unsafe extern "system" fn(HWND, u32, *mut core::ffi::c_void, u32) -> i32;
                static DWMA_GET: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
                let p = *DWMA_GET.get_or_init(|| {
                    #[link(name = "kernel32")]
                    unsafe extern "system" {
                        fn GetModuleHandleW(name: *const u16) -> isize;
                        fn GetProcAddress(
                            module: isize,
                            name: *const u8,
                        ) -> *const core::ffi::c_void;
                    }
                    let mn: Vec<u16> = "dwmapi.dll\0".encode_utf16().collect();
                    let m = GetModuleHandleW(mn.as_ptr());
                    if m != 0 {
                        GetProcAddress(m, c"DwmGetWindowAttribute".as_ptr() as *const u8) as usize
                    } else {
                        0
                    }
                });
                if p != 0 {
                    let f: Dwma = std::mem::transmute(p as *const core::ffi::c_void);
                    hr = f(
                        self.hwnd,
                        14, // DWMWA_CLOAKED
                        &mut cloaked as *mut u32 as *mut core::ffi::c_void,
                        4,
                    );
                }
            }
            if cloaked != 0 {
                self.cloaked_streak += 1;
            } else {
                self.cloaked_streak = 0;
            }
            // 【P8 修复 2026-09-13 三十四修】两条布局/锚点诊断日志原来
            // 每帧无条件 format!+写文件（生产路径热开销）；改为诊断
            // 开关开启才输出（C:\ProgramData\HuFu\diag\note 存在时——
            // diag_note 自身已有开关，这里先挡掉 format! 的分配本身）。
            if crate::tsf::diag_enabled() {
                crate::tsf::diag_note(&format!(
                    "cw2 layout dbg: font_pt={font_pt} em={em} line_h={line_h} horiz={horizontal} \
                     cands={} rawlen={} max_text={} width={width} height={height} w_out={w_out} h_out={h_out}",
                    cands.len(),
                    raw.chars().count(),
                    cand_ws.iter().map(|(tw, _, _)| *tw).fold(0.0f32, f32::max)
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
            }
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
            // 【三十二修·首显时钟重锚 2026-09-19】入场滑的 t0 起算于臂
            // 帧，但窗口真正出现在屏幕还要过 Present(1) 垂直同步等待 +
            // DWM 合成——切焦点后宿主 UI 线程最忙，这段延迟放大到 1-3
            // 个 vsync：窗口「出现在半路」再被 tick 拽着跳完剩余段 =
            // 「切焦首显动效顿顿」（全宿主共有的观感）。SWP 之后把
            // t0 重锚到当下，滑动从实际可见帧起播满全程。
            let mut entrance_armed = false;
            let sp_ok = if !dragging {
                // 【零位移·收窄轴向例外 2026-09-11】变宽轴向：窗口一步
                // 到位=目标（每键仅一次 SWP，壳在稳定窗口内长大——用户
                // 实锤变宽无直角）；收窄轴向：窗口必须跟缓动走——否则
                // 窗口先跳到小目标、动画壳从宽 cur 起步比窗口大，被窗
                // 口边缘直角切断（面板/边框/阴影一起切=「变窄出直角」
                // 的根源；变低同）。按轴取 max(eased,target)：增轴=target
                // （零位移），减轴=eased（窗缘恒=壳缘）。命中盒按目标。
                let apply = match self.size_anim {
                    Some((f, t, t0)) => {
                        let e = size_ease(f, t, t0.elapsed().as_millis() as u32, self.size_anim_dur.get().max(1));
                        (e.0.max(w_out as i32), e.1.max(h_out as i32))
                    }
                    None => (w_out as i32, h_out as i32),
                };
                // 【位置滑动·双向 2026-09-12 十六次修正】poll 真实帧把
                // est 超前拉回（按键即时 est +11/键，110ms 后 poll 真实
                // 校准）——左移瞬移规则把 30-40px 拉回变成可见跳变（用
                // 户实锤「第二键跳一下」「打多了离光标远」）。改为双向
                // 滑动：拉回也是 120ms 平滑滑——观感=恒定微调，与记事
                // 本同稳；误差上限=一键 est 误差（1-2px），永不累积。
                let (tx, ty) = (
                    x - (shadow_m * dpi_scale) as i32,
                    y - (shadow_m * dpi_scale) as i32,
                );
                let chase = crate::tsf::chase_on();
                // 【三十四修·chase 实验通道】旗标文件即开即关（无需重启
                // 宿主，下次首显生效）。语义=「窗口永远追赶光标」：目标=
                // 本帧锚位，起点=当前实位（首显=sticky 旧位=本段编码左
                // 端——真实位移零估算，距离恒准）；指数逼近（τ=35ms）+
                // 限速（2.5px/ms），tick 饥饿时步幅自适应无跳变。臂发后
                // 逐 tick 由 fade_tick_shared 步进。
                if chase && self.pos_ms > 0 && !self.internal_rerender {
                    self.pos_anim = None;
                    let start = if was_visible {
                        self.live_pos.get()
                    } else {
                        // sticky 存的是锚点坐标（未减阴影边距，见 3525 赋值
                        // 与主 SWP 的 tx=x−shadow 同源）——先转窗口坐标系再
                        // 作起点/距离门，否则首显起点偏右下=「从光标右下
                        // 角移过来」（用户实测）。读 sticky_prev（函数头抢
                        // 救的上一帧值）而非已被本帧覆盖的 self.sticky_pos。
                        let m = (shadow_m * dpi_scale) as i32;
                        let s = match sticky_prev {
                            Some(s) => {
                                let sw = (s.0 - m, s.1 - m);
                                if (tx - sw.0).abs().max((ty - sw.1).abs()) <= 150 {
                                    Some(sw)
                                } else {
                                    None
                                }
                            }
                            None => None,
                        };
                        // 【三十四修补·首段也有入场】无历史位（进程首段/焦
                        // 点切换后首段）不直出：编码左端=锚左−码长×键宽
                        // （tsf 注入的 first_show_unit，三十四修校准后即真
                        // 实键宽；未校准进程首段用兜底值，仅近似一次）。
                        let s = s.or_else(|| {
                            first_show_unit.and_then(|u| {
                                if u <= 0.5 {
                                    return None;
                                }
                                let travel = ((raw.chars().count().max(1)) as f32
                                    * u as f32)
                                    .round() as i32;
                                let cand = (tx - travel, ty);
                                if (3..=150).contains(&travel) {
                                    Some(cand)
                                } else {
                                    None
                                }
                            })
                        });
                        let st = s.unwrap_or((tx, ty));
                        self.live_pos.set(st);
                        st
                    };
                    // 【三十四修补·死区】位移 <3px 直接落位（复刻旧动效
                    // d<3 瞬跳语义）：锚点在 selection/GetTextExt/est 换
                    // 源之间的 ±1-3px 抖动若逐帧追赶，会变成可见的来回
                    // 蠕动=「回弹」（用户实测 srs+空格，旧版无此问题）。
                    let dmax = (tx - start.0).abs().max((ty - start.1).abs());
                    if dmax >= 3 {
                        self.chase_target = Some((tx, ty));
                        self.chase_last = None;
                        anim_tick_arm(self.hwnd);
                    } else {
                        self.live_pos.set((tx, ty));
                        self.chase_target = None;
                        self.chase_last = None;
                        self.chase_pos = None;
                    }
                    if self.chase_target.is_some() {
                        self.chase_pos = Some((start.0 as f32, start.1 as f32));
                    }
                    if crate::tsf::trace_on() {
                        crate::tsf::trace(&format!(
                            "chase 臂: start=({},{}) target=({},{}) was_vis={}",
                            start.0, start.1, tx, ty, was_visible
                        ));
                    }
                } else if was_visible && self.pos_ms > 0 && !self.internal_rerender {
                    self.chase_target = None;
                    self.chase_last = None;
                    self.chase_pos = None;
                    let (lx, ly) = self.live_pos.get();
                    let d = (tx - lx).abs().max((ty - ly).abs());
                    // 【四十三修·跨格大跳瞬落 2026-09-22】七十四修「位移
                    // 全平移」服务的是上屏大串 150-400px 随文流动；用户
                    // 实际打字节奏下收场钟残留窗跨格（WPS 实测 648px）与
                    // 垃圾锚逃逸回位（Excel 实测 1168px）被同一逻辑卷成
                    // 长滑=「候选飘移」。>500px 一律瞬落：跨格/回位不滑
                    // 行，≤400px 上屏大串档不受影响仍平移。
                    // 【大跳瞬跳·三十二修】用户拍板「光标在哪候选就从哪
                    // 出来，不从别的地方过来」——大距离跳变（点击换位/
                    // 反查跳行/切窗级，>150px）不再滑动，直接出现在新光
                    // 标处。
                    // 【三十八修·打字期快速滑动】40-150px 档原先也瞬跳
                    // ——est 批步进（重查帧一次 +多键）与真实帧拉回正
                    // 落这档（虎魄 344px 校准后 ~50px、QQ 重校 210px
                    // 拦截放开后 40-100px），瞬跳=用户"打着打着突然跳
                    // 很远"。改快速滑动（90-160ms）：位置变化可见但成
                    // 一个连续动作，非闪现。
                    // 【七十四修·位移全平移】用户拍板：不设瞬移上限——
                    // 任何位移（含上屏大串 150-400px、点击换位大跳）都
                    // 平移过去。旧阈值 150 时上屏一大串位移必超→直接取
                    // 消滑动=瞬移闪现（「上屏一大串就没有了会直接闪过
                    // 去」）。时长随距离动态、大步 clamp 快滑。
                    // 四十三修在 500px 处重开瞬落闸：七十四修的受害案例
                    // （上屏大串）≤400px，跨格/逃逸 ≥600px，500 界两全。
                    // 【四十五修·锚到滑入 2026-10-29】例外：上一帧显示位
                    // 不来自真锚（XAML 宿主组段首帧锚缺失→焦点窗兜底/
                    // sticky 钉位）——这是「兜底位→真锚」的修正位移，
                    // 不是跨格大跳：走 2.2 系数快滑（≤260ms 封顶，大位
                    // 移 ≈3px/ms，一个连续动作）。跨格/逃逸（两帧皆真
                    // 锚）仍瞬落。资源管理器重命名/搜索框「动效不生效」
                    // 的主修复。
                    if d > 500 && self.last_pos_anchored.get() {
                        self.live_pos.set((tx, ty));
                        self.pos_anim = None;
                        if crate::tsf::trace_on() {
                            crate::tsf::trace(&format!(
                                "cw2: 大跳瞬落 d={d} ({lx},{ly})→({tx},{ty})"
                            ));
                        }
                    } else if d >= 3 {
                        // 【统一节奏 2026-09-12 十次修正】小步进（3-6px）也
                        // 滑动——记事本流畅的本质=每键恒一次滑动节奏一致；
                        // 钉-跳交替=抖动感。亚像素抖由 sticky 2px 死区吃。
                        // 【十八次修正·动态时长】快打（~100ms/键）时固定时
                        // 长滑动被下一键打断，窗恒滞后锚（用户「跟不上」）。
                        // 时长按距离动态：小步短滑（60ms 下限）大步快滑。
                        // 【二十次修正 2026-09-12 用户拍板】上限 100ms。
                        // 【三十八修】40-150px 档（打字期批步进/拉回）上限
                        // 提到 160ms——距离更大需要稍长滑行才不显急促。
                        // 【七十四修】40px 以上档时长上限 220ms（覆盖
                        // 上屏大位移平移：400px/220ms≈2px/ms 快滑不拖
                        // 沓；下一键到达即重置新目标，无滞后）。
                        // 【动效提速 2026-10-08】整体提速约 30%：系数 5→3.5、
                        // 下限 60→45、40px 内档上限 100→75、大步上限 220→150；
                        // 且乘全局速度倍率（滑条统管平移）。
                        // 【七修·速度对齐虎娘 2026-09-18 用户拍板】系数
                        // 3.5→7.5：打字档恒速 ≈130px/s = 虎娘实测（12px/
                        // 90ms，64Hz 2px/tick 等效）；上限放宽 160ms 让恒速
                        // 到 ~21px 都不打折，更大位移仍是快滑（400px 落
                        // 160ms ≈ 2.5px/ms，74 修「不拖沓」语义保留）。
                        // 【四十五修·基准再提速 2026-10-29】用户实测
                        // 「动效感觉不流畅，基准速度再快一些」：7.5→5.0
                        // （恒速 ≈195px/s，1.5× 基准）、下限 45→40、上限
                        // 160→140（13px 键距 97→65ms，21px 158→105ms）。
                        // 修正位移（上一帧非锚位）：2.2 系数快滑 260ms
                        // 封顶（同大跳例外档）。
                        let spd = self.anim_spd.get().max(0.05);
                        let dur = if self.last_pos_anchored.get() {
                            ((d as f32 * 5.0 / spd) as u32).clamp(40, 140)
                        } else {
                            ((d as f32 * 2.2 / spd) as u32).clamp(60, 260)
                        };
                        self.pos_anim = Some(((lx, ly), (tx, ty), std::time::Instant::now(), dur, 0));
                        anim_tick_arm(self.hwnd);
                    } else if d > 0 {
                        self.pos_anim = None;
                    }
                } else if !was_visible && self.pos_ms > 0 && !self.internal_rerender {
                    self.chase_target = None;
                    self.chase_last = None;
                    self.chase_pos = None;
                    // 【虎娘对齐·首显滑动】组段首显（was_visible=false）原本
                    // 直接落锚；非整句方案改为从编码左端滑向光标右（虎娘
                    // 单字实测：窗口首现于编码左端，~100ms 滑到编码右端）。
                    // 臂门：距离 3..=150px——<3px 死区同逐键；>150px 是跨
                    // 焦点/点击换位级大跳，新位置直接出现（三十二修语义）。
                    // 时长固定 100ms（anim_speed 统管），不走逐键的距离
                    // clamp——一段首显 12px 会被压到 45ms，比虎娘急。
                    // 【冒烟豁免 2026-09-18】smoke 的 [18] 断言确定性时序，
                    // 首显滑动（100ms 位移）会打乱其可见性采样（实测
                    // 100% 档 vis=false）——冒烟进程内禁用（视觉时序特
                    // 性本就无法在冒烟里断言，实机由 trace 首显臂门覆盖）。
                    if first_show_slide
                        && std::env::var("HUFU_TSF_SMOKE").as_deref() != Ok("1")
                    {
                        if let Some(unit) = first_show_unit {
                            // 【四修·时序无关】起点从目标反推：编码左端=
                            // 光标右 − 编码宽；首显帧 raw 可能晚一拍
                            //（探针实测 raw='' cands=2），至少按一键宽走。
                            let raw_len = raw.chars().count();
                            let travel = ((raw_len.max(1)) as f32 * unit as f32).round()
                                as i32;
                            let fx = tx - travel;
                            if crate::tsf::trace_on() {
                                crate::tsf::trace(&format!(
                                    "首显臂门: unit={unit} raw_len={raw_len} travel={travel} fx={fx} tx={tx} slide={first_show_slide}"
                                ));
                            }
                            if (3..=150).contains(&travel) {
                                // 【五十三修·撤免滑】用户拍板「不要免滑」：
                                // 小行程保留滑动，顿挫改由动效高频驱动根治
                                //（winmm 5ms 回调→PostMessage，见 anim_boost），
                                // 15px/32ms 快滑在 ~5ms tick 下有 6-7 帧。
                                let spd = self.anim_spd.get().max(0.05);
                                // 【五修·首显提速 2026-09-18 用户拍板】固定
                                // 100ms 比逐键滑慢半拍，观感「出现慢」——
                                // 与逐键同一距离公式（统一节奏）。
                                // 【七修·速度对齐虎娘】同逐键：7.5 系数恒速
                                // ≈130px/s，上限 160ms。
                                // 【三十三修回退 2026-09-19】曾试入场快滑
                                // （4.0 系数/100ms 上限）配合全角行程，后
                                // 全角行程被用户否决、快滑随之回退——恢复
                                // 七修口径（7.5/160ms），仅保留三十一/三十
                                // 二修的即显与时钟重锚。
                                // 【四十五修·基准再提速 2026-10-29】同逐键
                                // 5.0/40..140（首显 15px 行程 112→75ms）。
                                // 【五十一修·入场再提速 2026-09-23 用户
                                // 拍板「更快速流畅」】3.2/28..110（15px
                                // 行程 75→32ms，≈470px/s）+ 曲线转 ease-out
                                //（曲线标志 1，见 pos_anim 注释）——快出
                                // 缓停，肉眼读作「弹入且稳稳停住」。
                                // 【五十八修】当日下午四轮曲线实验（并步
                                // 阈值/匀速/四次缓出）全部被判「一帧一帧
                                // /越来越卡」——真凶不在曲线：完成拍缺终
                                // 点 SWP 落位（见 tick 终点落位注释）+实
                                // 验期间 QQ 未重启测的是旧 DLL。曲线与
                                // 时长回本口径（用户上午认可的观感）。
                                let dur = ((travel as f32 * 3.2 / spd) as u32)
                                    .clamp(28, 110);
                                self.pos_anim =
                                    Some(((fx, ty), (tx, ty), std::time::Instant::now(), dur, 1));
                                entrance_armed = true;
                                // 起臂帧即记真实显示位：下一键 per-key 滑动
                                // 从滑行起点接续，而不是从上一段残值起步。
                                self.live_pos.set((fx, ty));
                                anim_tick_arm(self.hwnd);
                            }
                        }
                    }
                }
                // 【三十四修·chase】chase 生效时窗口停/起在 live_pos（当前
                // 实位或首显起点），由 tick 逐步逼近目标；非 chase 走原插值。
                let (px, py) = if chase && self.chase_target.is_some() && self.pos_ms > 0 {
                    self.live_pos.get()
                } else {
                    match self.pos_anim {
                        Some((f, t, t0, dur, ez)) => {
                            pos_anim_step(f, t, t0.elapsed().as_millis() as u32, dur, ez)
                        }
                        None => (tx, ty),
                    }
                };
                // 【四十一修·主 SWP 观测（三十六修删）】原条件
                // `|tx-x|>10`：tx ≡ x − m_off（阴影边距，3678 处推导），
                // 差值恒=m_off——凡带阴影皮肤恒真、日志每帧必发且信息
                // 量为零（恒定 delta）。锚↔目标差=设计常量，运动观测由
                // 下方三十九修（目标 vs 当前实位）承担，本块整删。
                // 【七十一修诊断·锚全帧】show 每帧打锚+sticky+目标+动
                // 效态（换行震荡排查：段模型已证稳定，显示层低值来源
                // 待定位——非降频，全帧）。
                if crate::tsf::trace_on() {
                    let (sx, sy) = self.sticky_pos.unwrap_or((0, 0));
                    let anim = if self.pos_anim.is_some() { "pos" } else { "-" };
                    let ain = if anchor.is_some() { "y" } else { "n" };
                    crate::tsf::trace(&format!(
                        "cw2: 全帧 锚入={ain} 锚位=({x},{y}) sticky=({sx},{sy}) 目标=({tx},{ty}) 显示=({px},{py}) anim={anim}"
                    ));
                }
                // 【三十九修·显示层观测】用户实锤"候选在两个位置来回
                // 跳"而锚序列（qc: raw/est）完全平滑——跳在锚→窗位
                // 置的显示层（suppress 补显/滑动/钳位交替），此前零
                // 观测=盲区。每次目标位移 >10px 打一行（小步进不打防
                // 日志爆炸），诊断直读。
                // 【三十六修·门控】trace_on() 前置——动画期 dd>10 帧每
                // 5ms 一次，关 trace 不该白付 format! 分配。
                {
                    let (lx2, ly2) = self.live_pos.get();
                    let dd = (tx - lx2).abs().max((ty - ly2).abs());
                    if dd > 10 && crate::tsf::trace_on() {
                        crate::tsf::trace(&format!(
                            "cw2: pos 目标({tx},{ty}) 当前({lx2},{ly2}) d={dd}"
                        ));
                    }
                }
                self.live_size.set(apply);
                self.content_size.set((w_out as i32, h_out as i32));
                self.live_pos.set((px, py));
                let swp_r = SetWindowPos(
                    self.hwnd,
                    HWND_TOPMOST,
                    px,
                    py,
                    apply.0,
                    apply.1,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
                // 【三十二修·首显时钟重锚】滑动从「窗口实际可见」起播满
                // 全程（臂帧→SWP 之间隔着渲染+Present 同步等待，见上）。
                // 仅入场滑重锚；逐键跟随的节奏是用户拍板调好的，不动。
                if entrance_armed {
                    if let Some((f, t, _, dur, ez)) = self.pos_anim {
                        self.pos_anim = Some((f, t, std::time::Instant::now(), dur, ez));
                        if crate::tsf::trace_on() {
                            crate::tsf::trace(&format!(
                                "cw2: 首显时钟重锚 pos=({px},{py})→({tx},{ty}) dur={dur}"
                            ));
                        }
                    }
                }
                swp_r
            } else {
                // 拖拽中窗口可能仍隐藏（首次 show 未显示）：确保可见
                crate::tsf::trace("cw2: SWP纯显示(NOMOVE)——窗留在当前位置显示");
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
            // 【六十修·切输入法看门狗】窗口可见即挂 200ms 慢钟：轮询本
            // 线程前景 TIP，非我们且窗仍在屏 → 冲销+收窗。四十五修的
            // ActiveLanguageProfileNotifySink 实测只收得见「切回来」
            //（全量 trace 零外源 clsid 事件）——Win+Space 现代切换器
            // 切走不广播给它，ISV 版 sink 又被系统恒拒（0x80040202），
            // 事件路全盲。本钟不依赖任何事件：Win+Space/鼠标点语言栏/
            // 触屏切法全兜住，最坏 200ms 残留（原=永留）。藏窗各执行
            // 点杀钟（WM_APP_HIDE_CAND 双分支）。
            if sp_ok.is_ok() {
                let r = SetTimer(self.hwnd, IME_WATCHDOG_TIMER_ID, 200, None);
                crate::tsf::trace(&format!(
                    "watchdog armed r={r:?}（0=失败）"
                ));
                // 【六十修·三层】同点位订阅全局原始键流（物理层兜底）
                unsafe { rawinput_listen(self.hwnd, true) };
            }
            // 【毛玻璃退役 2026-09-11】glass RGN/DWM 圆角/NC 链整块删除；
            // 仅保留残留清理（曾开过毛玻璃的窗恢复全窗区域+方角）。
            if self.rgn_last.get() != 0 {
                let _ = SetWindowRgn(self.hwnd, HRGN(std::ptr::null_mut()), true);
                self.rgn_last.set(0);
            }
            // 【锁标已移除 2026-09-10】固定态不再有视觉指示（拖动即
            // 固定、右键即解锁——位置本身即状态，无需锁标小窗）。
        }
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

    /// 【四十八修】本线程焦点窗是否仍是记录粘位时的那个（0=未知，
    /// 保守视为同一窗沿用原语义）。
    fn sticky_focus_matches(&self) -> bool {
        let recorded = self.sticky_focus_h.get();
        if recorded == 0 {
            return true;
        }
        #[repr(C)]
        struct GTI3 {
            cb: u32,
            flags: u32,
            hwnd_active: HWND,
            hwnd_focus: HWND,
            hwnd_capture: HWND,
            hwnd_menu_owner: HWND,
            hwnd_move_size: HWND,
            hwnd_caret: HWND,
            rc_caret: RECT,
        }
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetGUIThreadInfo(tid: u32, gi: *mut GTI3) -> i32;
        }
        let mut gi = GTI3 {
            cb: std::mem::size_of::<GTI3>() as u32,
            flags: 0,
            hwnd_active: HWND(std::ptr::null_mut()),
            hwnd_focus: HWND(std::ptr::null_mut()),
            hwnd_capture: HWND(std::ptr::null_mut()),
            hwnd_menu_owner: HWND(std::ptr::null_mut()),
            hwnd_move_size: HWND(std::ptr::null_mut()),
            hwnd_caret: HWND(std::ptr::null_mut()),
            rc_caret: RECT::default(),
        };
        unsafe {
            GetGUIThreadInfo(0, &mut gi) == 0
                || gi.hwnd_focus.0 as isize == recorded
        }
    }

    /// 【四十八修】当前焦点编辑框正下方的落位（跨编辑框粘位作废后的
    /// 兜底；h=候选物理高）。查询失败返回 None 退原「焦点窗口内左下」。
    fn focus_edit_below(&self, h: i32) -> (i32, i32) {
        #[repr(C)]
        struct GTI4 {
            cb: u32,
            flags: u32,
            hwnd_active: HWND,
            hwnd_focus: HWND,
            hwnd_capture: HWND,
            hwnd_menu_owner: HWND,
            hwnd_move_size: HWND,
            hwnd_caret: HWND,
            rc_caret: RECT,
        }
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetGUIThreadInfo(tid: u32, gi: *mut GTI4) -> i32;
        }
        let mut gi = GTI4 {
            cb: std::mem::size_of::<GTI4>() as u32,
            flags: 0,
            hwnd_active: HWND(std::ptr::null_mut()),
            hwnd_focus: HWND(std::ptr::null_mut()),
            hwnd_capture: HWND(std::ptr::null_mut()),
            hwnd_menu_owner: HWND(std::ptr::null_mut()),
            hwnd_move_size: HWND(std::ptr::null_mut()),
            hwnd_caret: HWND(std::ptr::null_mut()),
            rc_caret: RECT::default(),
        };
        unsafe {
            if GetGUIThreadInfo(0, &mut gi) == 0 || gi.hwnd_focus.0.is_null() {
                // 查询失败：退「焦点窗口内左下」同款兜底（工作区左上）
                return (16, 16);
            }
            let mut fr = RECT::default();
            if GetWindowRect(gi.hwnd_focus, &mut fr).is_err() {
                return (16, 16);
            }
            let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            let x = fr.left.clamp(vx, (vx + vw - 200).max(vx));
            let below = fr.bottom + 4;
            let y = if below + h <= vy + vh {
                below
            } else {
                (fr.top - h - 4).max(vy)
            };
            (x, y)
        }
    }

    /// 窗口当前是否可见（poll 前台兜底用：只在可见时记日志/收尾）
    pub fn is_visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    /// 【二十四修·动效大瘦身】收窗入口=立即真隐藏（1.5.9 语义；
    /// 上屏停留/退场动画全退役——用户拍板只留平移/尺寸/高亮滑动）。
    pub fn hide(&mut self) {
        self.hide_now();
    }

    /// 轮询 stale 专用：直接收窗（停留钟已退役）。
    pub fn hide_stale(&mut self) {
        self.hide();
    }

    /// 真隐藏：PostMessage 异步
    /// SW_HIDE。失焦/切窗/Deactivate/{隐藏候选}/词框弹窗/cloaked/
    /// 前台他进程等生命周期路径用。
    /// 【二十五修·选重闪帧复活】数字选重后的确认帧（高亮滑到选中项
    /// ~240ms）需要窗短暂在场——二十四修起 hide()=立即收，闪帧
    /// ~10ms 即被收走等于失效。专用短停留：到点异步真隐藏（不恢复
    /// 通用退场停留；新 show 到来会取消本定时器）。
    pub fn hide_later(&mut self, ms: u32) {
        unsafe {
            let _ = SetTimer(self.hwnd, HIDE_LATER_TIMER_ID, ms, None);
        }
    }

    pub fn hide_now(&mut self) {
        // 组段结束：作废「正向打字」单调锁——置 MAX 使下一帧必判
        // 「非增长」→ 新组段首帧自由定位（修单键接单键锁死旧位置）。
        // 粘性位置**保留**：跨组段的位置记忆，新组段首帧锚点暂不可
        // 用时沿用近处而非瞬移屏幕中下（清掉它正是「时不时跳到屏幕
        // 中下方」的病根）。
        // 【位置滑动】收窗即作废位置动效（下个组段首显瞬移新位）
        self.pos_anim = None;
        // 【三十四修】chase 同步作废（与 pos_anim 同生命周期）
        self.chase_target = None;
        self.chase_last = None;
        self.chase_pos = None;
        // 【二十五修·y 锁跨段延续】不再无条件清 y 锁/置首帧自由——
        // 上屏即收语义下每段都走 hide→show，无条件清锁使段间锚 y
        // 锯齿穿透（WPS 偶发抖动根源）。改为只记收窗时刻；首帧自由
        // 与否由 show() 的延续门判定（近距短隔=延续钉住）。焦点切换
        // 走 focus_reset（那边硬清，八修语义保留）。
        self.last_hide_at.set(Some(std::time::Instant::now()));
        // 【拖拽钉住解除】收窗（上屏断段/失焦/翻段）即解除拖拽钉住
        // ——下一组段恢复跟随 caret。
        self.sticky_drag = false;
        // 【绝不同步 ShowWindow】焦点回调（OnSetFocus）里同步 SW_HIDE
        // 与 MSCTF/Chromium 焦点临界区死锁——VSCode 点击冻结事故实锤
        // （栈：OnSetFocus → ShowWindow 永不返回）。改为 PostMessage
        // 排队，焦点回调返回后由消息循环执行隐藏。
        unsafe {
            let _ = PostMessageW(self.hwnd, WM_APP_HIDE_CAND, WPARAM(0), LPARAM(0));
        }
    }

    /// 抑制路径（首帧 35ms 补显/caret 锚点抑制）的收窗：可见中不收
    ///（防补显闪烁）；不可见才真收（幂等清理）。
    pub fn hide_suppress(&mut self) {
        if !self.is_visible() {
            self.hide_now();
        }
    }

    /// 【焦点重置·三十三修】跨焦点（切窗口/切应用）时抹掉候选窗的全部
    /// 位置记忆——用户定稿「换到另一个窗口就是一个新的开始，上一个窗
    /// 口的遗留残留记忆全部抹掉」。hide() 故意保留 sticky_pos（同窗口
    /// 组段间位置连续性），焦点切换时必须清：新窗口锚点全空时若沿用
    /// 旧 sticky，候选会出现在上一个窗口的位置（「从别的窗口过来」
    /// 观感的实体根源）。cloaked_streak 同清（cloak 计数不跨焦点）。
    pub fn focus_reset(&mut self) {
        self.sticky_pos = None;
        self.sticky_drag = false;
        self.pos_anim = None;
        // 【三十四修】chase 同步硬清（焦点切换=全新开始）
        self.chase_target = None;
        self.chase_last = None;
        self.chase_pos = None;
        self.cloaked_streak = 0;
        // 【二十五修】y 锁/首帧自由/收窗时刻一并硬清：焦点切换=全新
        // 开始（跨会话首帧自由 2026-10-09 八的原语义在此兜底——新窗
        // 口新位置不背旧锁/旧累计，反向 4-26px 小位移不钉错位）。
        self.ylock_last_dir.set(0);
        self.ylock_acc.set(0);
        self.show_frame_fresh.set(true);
        self.last_hide_at.set(None);
    }
}

/// 隐藏候选窗的应用层消息（PostMessage 异步隐藏用）
pub const WM_APP_HIDE_CAND: u32 = 0x4948; // "IH"
/// 【六十修】切输入法冲销请求（各检测层→wndproc 消息泵上下文统一
/// 执行——按键回调/WM_INPUT 里跑同步 edit session 会被 TSF 拒）。
pub const WM_APP_IME_SWITCH: u32 = 0x4953; // "IS"

/// 【六十修·一刀】进程内全部候选窗强制隐藏：EnumWindows 按「类名
/// HuFuCandWin2 + 本进程」过滤，每扇投递 WM_APP_HIDE_CAND（handler
/// 内 SW_HIDE+杀钟+退订 rawinput）。不猜是哪扇——当前窗/暂留窗/
/// 漏窗/让渡窗一律清场。切输入法、Deactivate、Activate 扫尸三处调用。
pub fn hide_all_cand_windows() {
    extern "system" fn enum_proc(hwnd: HWND, _l: LPARAM) -> BOOL {
        unsafe {
            let mut cls = [0u16; 32];
            let n = GetClassNameW(hwnd, &mut cls);
            let name = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
            if name.contains("HuFuCand") {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                if pid == std::process::id() {
                    let _ = PostMessageW(hwnd, WM_APP_HIDE_CAND, WPARAM(0), LPARAM(0));
                }
            }
            BOOL(1)
        }
    }
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(0));
    }
}

/// 【六十修】投递冲销请求到候选窗消息泵（无窗/投递失败 → false，
/// 调用方直调兜底）。
pub fn post_ime_switch(shared: &crate::tsf::SharedRef) -> bool {
    let h = shared
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .cand2
        .as_ref()
        .map(|c| c.hwnd);
    match h {
        Some(hwnd) => {
            let ok = unsafe { PostMessageW(hwnd, WM_APP_IME_SWITCH, WPARAM(0), LPARAM(0)) };
            ok.is_ok()
        }
        None => false,
    }
}

/// 【动效】动画 tick 定时器 id（尺寸/位置/高亮滑动共用；FADE 为
/// 历史名沿用）与注释展开延时定时器 id——挂在本窗消息队列，
/// wndproc 0x113 消费。
pub const FADE_TIMER_ID: usize = 0x4846_5550; // 'HuFZ'
pub const EXPAND_TIMER_ID: usize = 0x4846_5551; // 'HuFa'
/// 【六十修·切输入法看门狗】窗可见期 200ms 慢钟 id。
pub const IME_WATCHDOG_TIMER_ID: usize = 0x4846_5553; // 'HuFc'
/// 闪帧收尾（选重确认帧的短停留到点真隐藏）
pub const HIDE_LATER_TIMER_ID: usize = 0x4846_5552; // 'HuFb'
/// 【七十修·动效帧率】动画 tick 周期。原 15ms：SetTimer 实际 ~15.6ms
/// → 动画 ~64fps——60Hz 屏（16.7ms/帧）恰每帧 1 步无感；240Hz 屏
///（4.2ms/帧）每 3.7 帧才 1 步=高刷用户必见跳帧。降至 5ms + 进程
/// 时钟精度 timeBeginPeriod(1)（候选窗创建时一次性，见 new()）→
/// 动画 ~150-200fps，高刷观感对齐；60Hz 无感不退化。动画窗外
/// timer 自动 Kill，常驻开销为零。
pub const FADE_TICK_MS: u32 = 5;
/// 提升系统定时器粒度到 1ms（SetTimer(5) 实际生效的前提）。进程内
/// 一次（OnceLock）；winmm 直连 FFI（免 Cargo feature）。
fn raise_timer_resolution_once() {
    static DONE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    DONE.get_or_init(|| unsafe {
        #[link(name = "winmm")]
        extern "system" {
            fn timeBeginPeriod(ms: u32) -> u32;
        }
        let _ = timeBeginPeriod(1);
    });
}
// ============================================================================
// 【五十三修·动效高频驱动】WM_TIMER 投递颗粒 12~31ms 抖动（消息队列
// 合并 + 闲时投递策略），48ms 入场只剩 3 帧=「一顿一顿」（用户实锤，
// 且明确「不要免滑」）。补一路 winmm timeSetEvent(5ms) 回调 →
// PostMessage(WM_APP_ANIM)：posted 消息不合并、不被闲时策略推迟，
// 实际帧数≈3×。SetTimer 原路保留兜底（winmm 失败/极端宿主），两路
// 都进 fade_tick_shared——步进是纯时间基准，多到的 tick 只重算当前
// 位置（幂等），互不干扰。
//
// 自限：每次起臂刷新 400ms 截止（GetTickCount64）；无再臂回调里
// 自杀（timeKillEvent）+ 清注册表——动画最长高亮滑 ~300ms，驱动
// 最多多活 400ms 即停，常驻开销归零。多窗（双标签双线程）各自
// hwnd 注册、各自线程消费 Post——互不串线。
// ============================================================================
/// 动效高频 tick 消息（wndproc 消费 → fade_tick_shared）。
pub const WM_APP_ANIM: u32 = 0x4941; // "A"
///（截止 GetTickCount64 ms, 注册 hwnd 列表）
static ANIM_BOOST: std::sync::Mutex<(u64, Vec<isize>)> =
    std::sync::Mutex::new((0, Vec::new()));
static ANIM_BOOST_EVENT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetTickCount64() -> u64;
}
#[link(name = "winmm")]
unsafe extern "system" {
    fn timeSetEvent(
        delay: u32,
        resolution: u32,
        cb: Option<unsafe extern "system" fn(u32, u32, usize, usize, usize)>,
        user: usize,
        event_type: u32,
    ) -> u32;
    fn timeKillEvent(id: u32) -> u32;
}

/// winmm 回调（winmm 工作线程）：过期自杀；否则向全部注册窗投递
/// 动效 tick。只做 PostMessage（异步安全），不碰任何窗口状态。
unsafe extern "system" fn anim_boost_cb(_id: u32, _m: u32, _u: usize, _a: usize, _b: usize) {
    let (post, expired) = {
        let mut g = match ANIM_BOOST.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        let now = unsafe { GetTickCount64() };
        if now > g.0 {
            g.1.clear();
            (Vec::new(), true)
        } else {
            (g.1.clone(), false)
        }
    };
    if expired {
        let ev = ANIM_BOOST_EVENT.swap(0, std::sync::atomic::Ordering::Relaxed);
        if ev != 0 {
            unsafe { let _ = timeKillEvent(ev as u32); }
        }
        return;
    }
    for h in post {
        unsafe {
            let _ = PostMessageW(HWND(h as *mut core::ffi::c_void), WM_APP_ANIM, WPARAM(0), LPARAM(0));
        }
    }
}

/// 起臂动效高频驱动：注册本窗 + 刷新截止 + 无事件则启周期回调。
/// 幂等（重复调用=刷新截止）。失败静默退化为纯 SetTimer 路径。
fn anim_boost_arm(hwnd: HWND) {
    raise_timer_resolution_once();
    let mut g = match ANIM_BOOST.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    g.0 = unsafe { GetTickCount64() } + 400;
    let h = hwnd.0 as isize;
    if !g.1.contains(&h) {
        g.1.push(h);
    }
    if ANIM_BOOST_EVENT.load(std::sync::atomic::Ordering::Relaxed) == 0 {
        // TIME_PERIODIC=1；分辨率 1ms（timeBeginPeriod(1) 已提）。
        let id = unsafe { timeSetEvent(FADE_TICK_MS, 1, Some(anim_boost_cb), 0, 1) };
        if id != 0 {
            ANIM_BOOST_EVENT.store(id as usize, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// 动效 tick 起臂统一入口：SetTimer 兜底 + winmm 高频驱动双路。
fn anim_tick_arm(hwnd: HWND) {
    unsafe {
        let _ = SetTimer(hwnd, FADE_TIMER_ID, FADE_TICK_MS, None);
    }
    anim_boost_arm(hwnd);
}
/// 【四十六修·多标签宿主】按「谁的 cand2 拥有本 tick 的 hwnd」选
/// Shared：Win11 记事本等每个标签/窗口独立线程的宿主里，标签 2+
/// 线程的 TIP 实例各有自己的 Shared/候选窗；tick 此前恒读 G_SHARED
///（首线程）→ 标签 2+ 的计时器首 tick 即被误杀（动画永死，「第二
/// 个窗口没动效候选还卡」实锤）。命中本线程 tl_shared() 即用之；
/// 否则回落 G_SHARED（单线程宿主语义不变）。
fn tick_shared_for_hwnd(hwnd: HWND) -> Option<crate::tsf::SharedRef> {
    let owns_tick_hwnd = |s: &crate::tsf::SharedRef| {
        s.lock()
            .unwrap_or_else(|e| e.into_inner())
            .cand2
            .as_ref()
            .is_some_and(|c| c.hwnd == hwnd)
    };
    let g_shared = crate::tsf::G_SHARED.get().map(|x| x.0.clone());
    let t_shared = crate::tsf::tl_shared();
    match (g_shared, t_shared) {
        (Some(a), Some(b)) => {
            if !owns_tick_hwnd(&a) && owns_tick_hwnd(&b) {
                Some(b)
            } else {
                Some(a)
            }
        }
        (None, Some(b)) => Some(b),
        (Some(a), None) => Some(a),
        (None, None) => None,
    }
}
/// 【动效】尺寸/位置/高亮滑动 tick：take cand2+last_show → 尺寸插值
/// 【六十修·三层】订阅/退订全局原始键流（RIDEV_INPUTSINK：不受焦
/// 点、不受 TSF 路由、不被切走方吞键影响——物理按键必到本 wndproc）。
unsafe fn rawinput_listen(hwnd: HWND, on: bool) {
    use windows::Win32::UI::Input::*;
    let dev = RAWINPUTDEVICE {
        usUsagePage: 0x01,
        usUsage: 0x06,
        dwFlags: if on {
            RIDEV_INPUTSINK
        } else {
            RIDEV_REMOVE
        },
        hwndTarget: if on {
            hwnd
        } else {
            HWND(std::ptr::null_mut())
        },
    };
    let r = RegisterRawInputDevices(
        &[dev],
        std::mem::size_of::<RAWINPUTDEVICE>() as u32,
    );
    if let Err(e) = r {
        crate::tsf::trace(&format!(
            "rawinput {} 失败: {e}",
            if on { "订阅" } else { "退订" }
        ));
    }
}

/// 【六十修·三层】原始键流热键识别：Win 按住拍 Space / Ctrl+Shift
/// 成对（任一组合键形态）→ 候选在身即收尸。物理层，路由盲区兜底。
unsafe fn rawinput_switch_detect(lparam: LPARAM, hwnd: HWND) {
    use windows::Win32::UI::Input::*;
    static WIN_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static CTRL_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static SHIFT_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    use std::sync::atomic::Ordering::Relaxed;
    let mut sz = 0u32;
    let _ = GetRawInputData(
        HRAWINPUT(lparam.0 as *mut core::ffi::c_void),
        RID_INPUT,
        None,
        &mut sz,
        std::mem::size_of::<RAWINPUTHEADER>() as u32,
    );
    if sz == 0 || sz as usize > std::mem::size_of::<RAWINPUT>() {
        return;
    }
    let mut buf = RAWINPUT::default();
    let got = GetRawInputData(
        HRAWINPUT(lparam.0 as *mut core::ffi::c_void),
        RID_INPUT,
        Some(&mut buf as *mut RAWINPUT as *mut core::ffi::c_void),
        &mut sz,
        std::mem::size_of::<RAWINPUTHEADER>() as u32,
    );
    if got == u32::MAX || got == 0 {
        return;
    }
    if buf.header.dwType != RIM_TYPEKEYBOARD.0 as u32 {
        return;
    }
    let kb = buf.data.keyboard;
    let vk = kb.VKey;
    let is_up = (kb.Flags & RI_KEY_BREAK as u16) != 0;
    match vk {
        0x5B | 0x5C => {
            WIN_DOWN.store(!is_up, Relaxed);
            return;
        }
        0x11 => {
            CTRL_DOWN.store(!is_up, Relaxed);
        }
        0x10 => {
            SHIFT_DOWN.store(!is_up, Relaxed);
        }
        _ => {}
    }
    let fire = (vk == 0x20 && WIN_DOWN.load(Relaxed))
        || (vk == 0x10 && CTRL_DOWN.load(Relaxed))
        || (vk == 0x11 && SHIFT_DOWN.load(Relaxed));
    if !fire || is_up {
        return;
    }
    if !IsWindowVisible(hwnd).as_bool() {
        return;
    }
    let shared = if crate::tsf::addword_tl_thread() {
        match crate::tsf::tl_shared() {
            Some(s) => s,
            None => return,
        }
    } else {
        match tick_shared_for_hwnd(hwnd) {
            Some(s) => s,
            None => return,
        }
    };
    let in_use = {
        let g = shared.lock().unwrap_or_else(|p| p.into_inner());
        g.composition.is_some() || g.composing || !g.raw_last.is_empty()
    };
    if !in_use {
        return;
    }
    crate::tsf::trace(&format!(
        "rawinput: 切换热键现形(vk=0x{vk:X}) → 收尸（物理层，转消息泵）"
    ));
    if !post_ime_switch(&shared) {
        crate::tsf::ime_switch_abort(&shared);
    }
}

/// 【六十修·切输入法候选残留看门狗】四十五修挂的
/// ITfActiveLanguageProfileNotifySink 只收得见「切回我们」的事件（全
/// 量 trace 实锤：零外源 clsid 事件）——Win+Space 现代切换器切走时不
/// 广播给该 sink，ISV 版 ITfInputProcessorProfileActivationSink 又被
/// AdviseSingleSink 恒拒（0x80040202），事件驱动整条路是盲的；组段存
/// 续期间 TSF 又不调 Deactivate → QQ/32 位应用/资源管理器搜索里
/// Win+Space 切走后候选窗永留（用户实测三宿主一致）。
/// 本钟（候选窗可见期 200ms，wndproc 天然在窗口创建线程=TSF 线程）
/// 轮询 ITfKeystrokeMgr::GetForeground：非我们且窗仍在屏 → 复用
/// ime_switch_abort（冲销组段+收窗+引擎会话清零）。不依赖任何事件，
/// 切法无关（Win+Space/鼠标语言栏/触屏）；事件序上 GetForeground 的
/// 翻转滞后由周期重查吸收（最坏多等一拍）。
unsafe fn ime_watchdog_tick(hwnd: HWND) {
    use windows::Win32::UI::TextServices::ITfKeystrokeMgr;
    if !IsWindowVisible(hwnd).as_bool() {
        // 窗已不可见（竞态：藏窗消息在途）——杀钟免空转
        let _ = KillTimer(hwnd, IME_WATCHDOG_TIMER_ID);
        return;
    }
    // 线程归属路由：小窗线程走 TL 登记，否则按 hwnd 定 Shared
    //（四十六修多标签宿主同款）
    let shared = if crate::tsf::addword_tl_thread() {
        match crate::tsf::tl_shared() {
            Some(s) => s,
            None => return,
        }
    } else {
        match tick_shared_for_hwnd(hwnd) {
            Some(s) => s,
            None => return,
        }
    };
    let tm = shared
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .thread_mgr
        .clone();
    let Some(tm) = tm else { return };
    let Ok(km) = tm.cast::<ITfKeystrokeMgr>() else { return };
    let Ok(fg) = (unsafe { km.GetForeground() }) else { return };
    // 【六十修·诊断轮】节流 2s/次落一行（fg+HKL+可见态）——定位残留
    // 态下 GetForeground 是否翻转（若恒报我们=轮询信号失效，需换源）。
    {
        static LAST: std::sync::Mutex<(u64, [u32; 4])> = std::sync::Mutex::new((0, [0; 4]));
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
        let cur = [fg.data1, fg.data2 as u32, fg.data3 as u32, fg.data4[0] as u32];
        if crate::tsf::trace_on() && (now_ms.saturating_sub(last.0) > 2000 || cur != last.1) {
            let hkl = unsafe {
                let tid = GetWindowThreadProcessId(hwnd, None);
                windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout(tid)
            };
            crate::tsf::trace(&format!(
                "watchdog tick fg={fg:?} hkl={:x} vis={}",
                hkl.0 as u64,
                IsWindowVisible(hwnd).as_bool()
            ));
            *last = (now_ms, cur);
        }
    }
    if fg == crate::CLSID_HUFU_TSF {
        return;
    }
    crate::tsf::trace(&format!(
        "watchdog: 前景 TIP 已易主({fg:?}) → 冲销+收窗（事件盲区兜底，转消息泵）"
    ));
    if !post_ime_switch(&shared) {
        crate::tsf::ime_switch_abort(&shared);
    }
    // ime_switch_abort 内部走 ClearComp→藏窗消息，钟由藏窗执行点杀
}


/// 步进（整帧复渲染外壳）→ 位置插值步进（只 SWP 不重绘——内容按
/// 目标布局早已在缓冲）→ 高亮滑动复渲染 → 放回。全部结束 KillTimer。
unsafe fn fade_tick_shared(hwnd: HWND) {
    // 【动画 tick 线程感知 2026-09-12 十七修】残留窗最终根因：小窗
    // 线程 TL 候选的动画 tick 走到这里 → take g.cand2（主线程窗）
    // → c.show 跨线程 SWP_SHOWWINDOW 把刚 SW_HIDE 的主文档窗复活+
    // 渲染（「、」残留窗、跨线程 D2D 损坏内容）。小窗线程：动画全
    // 跳过（词框候选不需要花式动画——tl_cand_show 首帧已完整渲染）。
    if crate::tsf::addword_tl_thread() {
        // 【动效接上·二十一修】词框候选的动画 tick 完整步进（与主线程
        // 路径同款：拉伸/位置/高亮滑动），操作对象=TL 实例（本线程的窗），
        // 绝不碰 g.cand2（那是主线程窗——跨线程 show=残留窗根因）。
        // 【Shared 实例修正·二十六修】小窗线程 TIP 的 Shared 进不了
        // G_SHARED（只留主线程首激活）——优先取线程局部登记的小窗
        // Shared，last_show/skin 才是词框渲染写的那些。
        let shared = match crate::tsf::tl_shared() {
            Some(s) => s,
            None => {
                let Some(gsh) = crate::tsf::G_SHARED.get() else {
                    return;
                };
                gsh.0.clone()
            }
        };
        let (mut tl, last, skin) = {
            let g = shared.lock().unwrap_or_else(|e| e.into_inner());
            (
                crate::tsf::tl_cand_take(),
                g.last_show.clone(),
                g.skin.clone(),
            )
        };
        let mut anim_done = true;
        if let (Some(c), Some((cands, raw, sel))) = (tl.as_mut(), last) {
            if let Some((f, t, t0)) = c.size_anim {
                let ms = t0.elapsed().as_millis() as u32;
                let cur = size_ease(f, t, ms, c.size_anim_dur.get().max(1));
                let finished = cur == t;
                if finished {
                    c.size_anim = None;
                    c.chrome_override.set(None);
                } else {
                    anim_done = false;
                    c.chrome_override.set(Some(cur));
                }
                c.live_size.set(cur);
                // 【五十九修·形变帧率封顶】中间帧间隔小于「屏幕刷新
                // 帧距」（60Hz=16.7ms / 144Hz=6.9 / 240Hz=4.2，见
                // morph_frame_interval_ms）跳过渲染（完成帧必渲）。
                let render_ok = finished || {
                    let now = std::time::Instant::now();
                    if now.duration_since(c.size_anim_last_render.get()).as_millis()
                        >= morph_frame_interval_ms(hwnd)
                    {
                        c.size_anim_last_render.set(now);
                        true
                    } else {
                        false
                    }
                };
                if c.is_visible() && render_ok {
                    c.internal_rerender = true;
                    let _ = c.show(&cands, &raw, &skin, None, sel);
                    c.internal_rerender = false;
                }
            }
            // 【三十六修·TL 补 chase 步进】chase 臂发在 show() 无线程
            // 区分（TL 实例同样被置 target/清 pos_anim），主 tick 有步进
            // 而本分支没有 → 开 chase 旗标时词框窗冻结在起点、timer 因
            // anim_done 立即被杀永不追到光标。步进逻辑与主线程同款。
            if let Some(tgt) = c.chase_target {
                let now = std::time::Instant::now();
                let dt_ms = c
                    .chase_last
                    .map(|t| now.duration_since(t).as_secs_f32() * 1000.0)
                    .unwrap_or(8.0)
                    .clamp(1.0, 120.0);
                c.chase_last = Some(now);
                let (cx, cy) = c.chase_pos.unwrap_or({
                    let lp = c.live_pos.get();
                    (lp.0 as f32, lp.1 as f32)
                });
                let (dxf, dyf) = (tgt.0 as f32 - cx, tgt.1 as f32 - cy);
                let dist = (dxf * dxf + dyf * dyf).sqrt();
                if dist < 1.0 {
                    c.chase_target = None;
                    c.chase_last = None;
                    c.chase_pos = None;
                    c.live_pos.set(tgt);
                    if c.is_visible() {
                        let _ = SetWindowPos(
                            hwnd,
                            HWND_TOPMOST,
                            tgt.0,
                            tgt.1,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOACTIVATE,
                        );
                    }
                } else {
                    let k = 1.0 - (-dt_ms / 35.0).exp();
                    let mut frac = k;
                    let max_step = 2.5 * dt_ms;
                    if dist * frac > max_step {
                        frac = max_step / dist;
                    }
                    let np = (
                        (cx + dxf * frac).round() as i32,
                        (cy + dyf * frac).round() as i32,
                    );
                    c.chase_pos = Some((cx + dxf * frac, cy + dyf * frac));
                    c.live_pos.set(np);
                    anim_done = false;
                    if c.is_visible() && np != (cx.round() as i32, cy.round() as i32) {
                        let _ = SetWindowPos(
                            hwnd,
                            HWND_TOPMOST,
                            np.0,
                            np.1,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOACTIVATE,
                        );
                    }
                }
            }
            if let Some((f, t, t0, dur, ez)) = c.pos_anim {
                let cur = pos_anim_step(f, t, t0.elapsed().as_millis() as u32, dur, ez);
                if cur == t {
                    c.pos_anim = None;
                    c.live_pos.set(t);
                } else {
                    anim_done = false;
                    c.live_pos.set(cur);
                    if c.is_visible() {
                        // 【三十二修·门控】二十五修口径：trace 关时不白付
                        // format! 分配（动画期每 5ms tick 一次）。
                        if crate::tsf::trace_on() {
                            crate::tsf::trace(&format!("cw2: SWP动画 cur=({},{})", cur.0, cur.1));
                        }
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
            // 【三十六修·TL 补高亮滑动步进】hl 臂发同样无线程区分，
            // 本分支缺步进 → 词框窗选重/换候选时胶囊冻在起点矩形
            //（首 tick anim_done=true 即收钟）。注释「同款」自此属实。
            if c.hl_anim.get().is_some() {
                anim_done = false;
                if c.is_visible() {
                    c.internal_rerender = true;
                    let _ = c.show(&cands, &raw, &skin, None, sel);
                    c.internal_rerender = false;
                }
            }
        }
        if anim_done {
            let _ = KillTimer(hwnd, FADE_TIMER_ID);
        }
        crate::tsf::tl_cand_put_back(tl);
        return;
    }
    // 【四十六修】多标签宿主：按 hwnd 定 Shared（见 helper 注释）
    let Some(shared) = tick_shared_for_hwnd(hwnd) else {
        return;
    };
    let (mut cand2, last, skin, caret) = {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        // 【动效窗口让渡】标记持有中——TSF 线程此刻 is_none() 不新建
        // 第二窗（短等放回），组段收尾挂 pending_cand_hide
        g.cand2_busy = true;
        (g.cand2.take(), g.last_show.clone(), g.skin.clone(), g.caret)
    };
    let mut anim_done = true;
    if let (Some(c), Some((cands, raw, sel))) = (cand2.as_mut(), last) {
        // 【拉伸动效步进】每 tick 以当前插值尺寸整帧重绘：外壳（背景/
        // 边框/阴影）画在插值尺寸上=边缘把边框阴影「拉过去」（延伸
        // 感），内容按目标布局裁在外壳内；完成帧解除覆盖按目标渲染。
        //（P11 曾为 fade+size 并行设本 tick 去重标记——fade 退役后删。）
        if let Some((f, t, t0)) = c.size_anim {
            let ms = t0.elapsed().as_millis() as u32;
            let cur = size_ease(f, t, ms, c.size_anim_dur.get().max(1));
            let finished = cur == t;
            if finished {
                c.size_anim = None;
                c.chrome_override.set(None);
            } else {
                anim_done = false;
                c.chrome_override.set(Some(cur));
            }
            c.live_size.set(cur);
            // 【五十九修·形变帧率封顶】中间帧间隔小于「屏幕刷新帧
            // 距」（morph_frame_interval_ms：60Hz=16.7 / 144=6.9 /
            // 240=4.2ms）跳过渲染（完成帧必渲）——杀 winmm 250fps
            // 驱动下 1-3ms 突发连渲染；高刷屏按其实际刷新率足帧。
            let render_ok = finished || {
                let now = std::time::Instant::now();
                if now.duration_since(c.size_anim_last_render.get()).as_millis()
                    >= morph_frame_interval_ms(hwnd)
                {
                    c.size_anim_last_render.set(now);
                    true
                } else {
                    false
                }
            };
            if c.is_visible() && render_ok {
                c.internal_rerender = true;
                let _ = c.show(&cands, &raw, &skin, caret.as_ref(), sel);
                c.internal_rerender = false;
            }
        }
        // 【三十四修·chase 步进】指数逼近（τ=35ms）+限速（2.5px/ms）：
        // 纯时间基准，dt 自适应——tick 饥饿时步幅自动变大、位置无跳变；
        // 子像素在 chase_pos 积分，<1px 落定收钟。与 pos_anim 互斥。
        if let Some(tgt) = c.chase_target {
            let now = std::time::Instant::now();
            let dt_ms = c
                .chase_last
                .map(|t| now.duration_since(t).as_secs_f32() * 1000.0)
                .unwrap_or(8.0)
                .clamp(1.0, 120.0);
            c.chase_last = Some(now);
            // 以浮点实位积分（chase_pos），live_pos 为其取整镜像。
            let (cx, cy) = c.chase_pos.unwrap_or({
                let lp = c.live_pos.get();
                (lp.0 as f32, lp.1 as f32)
            });
            let (dxf, dyf) = (tgt.0 as f32 - cx, tgt.1 as f32 - cy);
            let dist = (dxf * dxf + dyf * dyf).sqrt();
            if dist < 1.0 {
                c.chase_target = None;
                c.chase_last = None;
                c.chase_pos = None;
                c.live_pos.set(tgt);
                if c.is_visible() {
                    let _ = SetWindowPos(
                        hwnd,
                        HWND_TOPMOST,
                        tgt.0,
                        tgt.1,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
            } else {
                let k = 1.0 - (-dt_ms / 35.0).exp();
                let mut frac = k;
                let max_step = 2.5 * dt_ms;
                if dist * frac > max_step {
                    frac = max_step / dist;
                }
                let nx = cx + dxf * frac;
                let ny = cy + dyf * frac;
                let np = ((nx.round() as i32), (ny.round() as i32));
                c.chase_pos = Some((nx, ny));
                c.live_pos.set(np);
                anim_done = false;
                if c.is_visible() && np != (cx.round() as i32, cy.round() as i32) {
                    if crate::tsf::trace_on() {
                        crate::tsf::trace(&format!(
                            "chase 步: cur=({},{}) target=({},{}) dt={dt_ms:.0}",
                            np.0,
                            np.1,
                            tgt.0,
                            tgt.1
                        ));
                    }
                    let _ = SetWindowPos(
                        hwnd,
                        HWND_TOPMOST,
                        np.0,
                        np.1,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
            }
        }
        // 【位置滑动步进】move-only（内容不变不重绘）：插值坐标推进
        // 窗口跟光标滑动；完成即清。首显起臂在 show() 的 SWP 处。
        if c.chase_target.is_none() {
        if let Some((f, t, t0, dur, ez)) = c.pos_anim {
            let mut cur = pos_anim_step(f, t, t0.elapsed().as_millis() as u32, dur, ez);
            // 【五十八修·末帧并步 2026-09-23】入场滑动收尾「多移一
            // 帧还不流畅」根治：ease-out 立方尾部的亚像素增量经逐帧
            // 取整=尾部 1px 蠕动一帧 + 0px 死帧，且尾巴步在合成器
            //（vsync 采样）上抽签——孤悬的 +1px 悬一整拍=肉眼读作
            // 减速停住后又顿一下（全宿主一致）。修：残距≤2px（两轴）
            // 时并入本步一步落位，动画即清——末步至少 2px（够格当
            // 真实运动步而非抽搐），缓停节奏保留（…4,3 收束式干净
            // 停），蠕动帧/死帧/孤悬拍全消。逐键跟随同通道同受益。
            if (t.0 - cur.0).abs() <= 2 && (t.1 - cur.1).abs() <= 2 {
                cur = t;
            }
            if cur == t {
                c.pos_anim = None;
                c.live_pos.set(t);
                // 【五十八修·终点落位 2026-09-23】完成拍必须把窗送到
                // 精确目标。原实现（含今晨版本）清了动画却不 SWP——
                // 窗停在上一拍位置（差 1-2px），等下一次 show 的 SWP
                // 才纠偏=「收尾多移一帧且不流畅」的真身：一次晚到的
                // 纠偏步（用户从今晨反馈到下午，四轮曲线调参无效的
                // 元凶——顿感从来不是曲线，是缺终拍落位）。
                if c.is_visible() {
                    let _ = SetWindowPos(
                        hwnd,
                        HWND_TOPMOST,
                        t.0,
                        t.1,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
                } else {
                    anim_done = false;
                    let prev = c.live_pos.get();
                    c.live_pos.set(cur);
                    // 死帧去重：坐标与上一拍相同（亚像素增量取整为0）
                    // 则不动窗不落档——尾巴只留真实位移。
                    if c.is_visible() && cur != prev {
                        // 【三十二修·门控】同上：tick 热路径 trace 关不分配。
                        if crate::tsf::trace_on() {
                            crate::tsf::trace(&format!("cw2: SWP动画2 cur=({},{})", cur.0, cur.1));
                        }
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
        }
        // 【高亮滑动步进 2026-10-09】hl_anim 在身：逐 tick 复渲染呈现插值
        // 帧（FADE_TIMER 驱动；完成在渲染内自清 → anim_done 收 timer）。
        if c.hl_anim.get().is_some() {
            anim_done = false;
            if c.is_visible() {
                c.internal_rerender = true;
                let _ = c.show(&cands, &raw, &skin, caret.as_ref(), sel);
                c.internal_rerender = false;
            }
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
                mine.hide_now();
            } else {
                g.cand2 = Some(mine);
            }
        }
        (Some(newer), Some(mut mine)) => {
            mine.hide_now();
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
    // 【动画 tick 线程感知·二十一修】小窗线程：注释展开同样走 TL 实例
    if crate::tsf::addword_tl_thread() {
        // 【Shared 实例修正·二十六修】同 fade tick：优先小窗线程的 Shared
        let shared = match crate::tsf::tl_shared() {
            Some(s) => s,
            None => {
                let Some(gsh) = crate::tsf::G_SHARED.get() else {
                    return;
                };
                gsh.0.clone()
            }
        };
        let (mut tl, last, skin) = {
            let g = shared.lock().unwrap_or_else(|e| e.into_inner());
            (
                crate::tsf::tl_cand_take(),
                g.last_show.clone(),
                g.skin.clone(),
            )
        };
        if let (Some(c), Some((cands, raw, sel))) = (tl.as_mut(), last) {
            if !c.comments_expanded {
                c.comments_expanded = true;
                let _ = KillTimer(hwnd, EXPAND_TIMER_ID);
                // 【锚点修正·二十二修】同 fade tick：anchor=None 钉 sticky
                c.show(&cands, &raw, &skin, None, sel);
            }
        }
        crate::tsf::tl_cand_put_back(tl);
        return;
    }
    // 【四十六修】多标签宿主：按 hwnd 定 Shared（见 helper 注释）
    let Some(shared) = tick_shared_for_hwnd(hwnd) else {
        return;
    };
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

/// 【尺寸动效 2026-09-11】smoothstep 插值：t∈[0,ms] 映射进度
/// p=3t²-2t³（缓起-加速-缓收，「成长感」明确——ease-out 起步即
/// 大位移被实测判「无动画感」），返回 from→to 的即时尺寸。
/// 【三十四修·死代码清理】shadowwin 整链（SHADOW_*、shadow_wndproc、
/// shadowwin_render/show/set_alpha/hide/follow、sd_round_rect、
/// shadow_geo 共 ~370 行）删除：shadowwin_show 全项目零调用 →
/// SHADOW_HWND 恒 None → 7 个调用点纯空锁空转（grep 验证）。链内
/// 的 size_ease 是活函数（动效核心），保留。
/// 【六修·虎娘对齐 2026-09-18】smoothstep（S 曲线慢-快-慢）→ 线性匀速
/// ——虎娘实测恒速小步进（64Hz ~2px/tick），逐键重定目标时匀速续走
/// 无「慢起-加速-减速」脉动=回弹感根除。全部动效（平移/形变/插值
/// tick）共用本函数，一并转线性。
/// 【五十九修·补：形变帧率封顶=屏幕实际刷新率】用户点破硬编码
/// 60fps「高刷屏怎么办」——封顶的本意是「每合成帧至多渲一次，
/// 不白渲」，那么上限就该是候选窗所在显示器的当前刷新率：60Hz
/// 屏 16.7ms/帧、120Hz 8.3、144Hz 6.9、165Hz 6.1、240Hz 4.2。
/// 高刷屏不少帧（五十三修高频驱动的平滑它照吃），低刷屏不白渲
///（超过刷新率的渲染合成器根本不上屏=纯浪费）。取窗口最近显示
/// 器的 ENUM_CURRENT_SETTINGS.dmDisplayFrequency，进程级缓存一
/// 次；查询失败兜底 60Hz。
fn morph_frame_interval_ms(hwnd: HWND) -> u128 {
    use std::sync::Mutex;
    // 2s TTL 缓存（非首查永存）：窗口跨屏迁移（60Hz 外接 ↔ 高刷
    // 主屏）至多 2s 内跟上新屏节奏；查询本身是轻 syscall，2s 一次
    // 可忽略。
    static CACHE: Mutex<(u64, u128)> = Mutex::new((0, 17));
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut g = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if now_ms.saturating_sub(g.0) > 2000 {
        let new = query_refresh_interval_ms(hwnd);
        if new != g.1 && crate::tsf::trace_on() {
            crate::tsf::trace(&format!("cw2: 形变帧率封顶 → {new}ms/帧（跨屏/初查）"));
        }
        *g = (now_ms, new);
    }
    g.1
}

/// 窗口最近显示器当前刷新率的帧距（ms，向上取整）。AppContainer
///（SearchHost/UWP）或任何查询失败 → 兜底 60Hz。5ms winmm tick 是
/// 渲染节拍地板：144Hz 屏阈值 ~7ms → 实际每 2 tick 渲一帧（≈100fps，
/// 不超发合成帧的前提下最平滑；240Hz 阈值 5ms → 每 tick 一帧）。
fn query_refresh_interval_ms(hwnd: HWND) -> u128 {
    unsafe {
        let mut hz: u32 = 60;
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if !mon.is_invalid() {
            let mut mi: MONITORINFOEXW = std::mem::zeroed();
            mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(
                mon,
                &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO,
            )
            .as_bool()
            {
                let mut dm: DEVMODEW = std::mem::zeroed();
                dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
                if EnumDisplaySettingsExW(
                    PCWSTR(mi.szDevice.as_ptr()),
                    ENUM_CURRENT_SETTINGS,
                    &mut dm,
                    ENUM_DISPLAY_SETTINGS_FLAGS(0), // 无附加标志：取当前模式
                )
                .as_bool()
                    && (30..=1000).contains(&dm.dmDisplayFrequency)
                {
                    hz = dm.dmDisplayFrequency;
                }
            }
        }
        (1000.0 / hz as f32).ceil() as u128
    }
}

pub(crate) fn size_ease(from: (i32, i32), to: (i32, i32), t_ms: u32, dur_ms: u32) -> (i32, i32) {
    if dur_ms == 0 || t_ms >= dur_ms {
        return to;
    }
    let x = t_ms as f32 / dur_ms as f32;
    let l = |a: i32, b: i32| a + ((b - a) as f32 * x).round() as i32;
    (l(from.0, to.0), l(from.1, to.1))
}

/// 【五十一修】位置滑动统一步进：曲线 0=线性（size_ease 原口径，
/// 逐键跟随等沿用）；1=入场 ease-out（立方缓出 1-(1-x)³——首帧
/// 即走 ~大头行程，尾段减速归零，无硬刹车感）。时间基准（真实
/// 经耗时），帧距抖动只影响采样点不影响轨迹速度。
pub(crate) fn pos_anim_step(
    from: (i32, i32),
    to: (i32, i32),
    t_ms: u32,
    dur_ms: u32,
    ease: u8,
) -> (i32, i32) {
    if dur_ms == 0 || t_ms >= dur_ms {
        return to;
    }
    if ease == 0 {
        return size_ease(from, to, t_ms, dur_ms);
    }
    if ease == 2 {
        // 【五十八修·入场匀速】用户拍板「一次性滑到要停的位置」：
        // 匀速直线，无减速拖尾——全程等速、到点即停（末步并步保证
        // 最后一步恰好落在目标）。出速观感由时长收紧补偿（见起臂
        // 处 travel*2.2/20..75）。
        let x = t_ms as f32 / dur_ms as f32;
        let l = |a: i32, b: i32| a + ((b - a) as f32 * x).round() as i32;
        return (l(from.0, to.0), l(from.1, to.1));
    }
    if ease == 3 {
        // 【五十八修·入场四次缓出】匀速版实测「一帧一帧」（合成器
        // 60Hz 采样下等速小步=可见阶梯）；三次缓出尾部又蠕动悬帧。
        // 四次缓出 1-(1-x)⁴：50% 时刻已走 94%、60% 走 97.4%——残距
        // ≤2px 并步线在 ~60% 时长即触发，拖尾整段截肢（不存在亚像
        // 素尾帧），前段保留三次版的「弹入」出速观感。时长回五十一
        // 修被认可口径（3.2/28..110）。
        let x = t_ms as f32 / dur_ms as f32;
        let k = 1.0 - (1.0 - x).powi(4);
        let l = |a: i32, b: i32| a + ((b - a) as f32 * k).round() as i32;
        return (l(from.0, to.0), l(from.1, to.1));
    }
    let x = t_ms as f32 / dur_ms as f32;
    let k = 1.0 - (1.0 - x).powi(3);
    let l = |a: i32, b: i32| a + ((b - a) as f32 * k).round() as i32;
    (l(from.0, to.0), l(from.1, to.1))
}
