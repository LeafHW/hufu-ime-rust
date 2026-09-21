//! hufu-tsf —— HuFu 输入法 Windows TSF 前端（纯 Rust COM DLL）。
//!
//! 架构（同小狼毫）：本 DLL 是薄壳——按键事件通过命名管道发给 hufu-server
//! 引擎，取回 {consumed, commit, state} 后操作 TSF 组段并绘制候选窗。

mod addword;
// 【死模块移除 2026-09-11】candwin.rs（v1 候选窗）与 candwin3.rs 同理：
// CandidateWindow::new 无调用点（g.cand 恒 None），v2（DComp 直通）+
// server 代画双通道定稿后 v1 只剩死分支。整模块删除（git 可回溯）。
mod candwin2;
// 【特效退役 2026-09-22】commitfx（上屏特效:飘散/落印/像素链/学习引
// 擎）与 fxprobe（落印诊断）整模块删除——用户拍板「删掉所有特效相
// 关」。tsf.rs 的触发点/Shared 字段/尾锚实测同步清理。git 可回溯。
// 【死模块移除 2026-09-13 三十四修】canduielement（ITfCandidateListUIElement
// 宿主代画协议）整模块删除：HuFuCandElement 全库无构造点——UIElement
// 代画通道被 server 代画「用户定稿」架空（ui_element_show 无条件 false），
// tsf.rs 的 cand_ui/cand_ui_id/cand_ui_host_draws 三字段与全部 if 分支
// 为死分支（cand_ui_active 仍在用已保留）。git 历史可回溯。
mod com;
// i686 windows-gnu 交叉链接补丁：llvm libmingw32 无 _DllEntryPoint@12，
// 由本模块 stub 提供（转发 DllMainCRTStartup）。x86_64 不编入。
#[cfg(all(target_arch = "x86", target_env = "gnu"))]
mod dll_entry_x86;
// 【下划线回归·类别注册补课 2026-10-09】displayattr 修复回归：首轮
// 失败根因=没注册 GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER 类别，msctf
// 解析不到 provider → 宿主连默认线都不画。现注册类别（装机+Activate
// 双路），QQ/记事本走默认线不受扰，32 位/UWP/开始菜单按属性画线。
mod displayattr;
mod ipc;
// 语言栏品牌按钮（「虎」牌）+ 中/英模式 compartment 同步——Activate
// 时安装（tsf.rs L312-321 实际调用链），非死代码。
mod langbar;
mod sound;
mod tsf;

use windows_core::*;

/// HuFu TSF 服务 CLSID：{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}
pub const CLSID_HUFU_TSF: GUID = GUID::from_u128(0x8f5c2a10_3e77_4b9c_a1d4_9e0b7c2f5a88);

#[no_mangle]
extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut core::ffi::c_void,
) -> HRESULT {
    use windows::Win32::System::Com::IClassFactory;
    // 加载画像（每进程一笔，ProgramData\HuFu\diag\load-<pid>.txt）：
    // 区分「宿主没加载 DLL」vs「加载了但未激活」（UWP/搜索框问题分层定位）
    // 【三十六修】OnceLock 每进程一次——旧实现每次 COM 激活都
    // create_dir_all+覆写盘（COM 激活路径的同步磁盘 IO）。
    static LOAD_MARK: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    LOAD_MARK.get_or_init(|| {
        let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
        let _ = std::fs::write(
            format!(r"C:\ProgramData\HuFu\diag\load-{}.txt", std::process::id()),
            format!(
                "load dll={} t={:?}\n",
                com::self_path_for_diag(),
                std::time::SystemTime::now()
            ),
        );
    });
    if ppv.is_null() {
        return HRESULT(-2147467261); // E_POINTER
    }
    unsafe { *ppv = std::ptr::null_mut() };
    if rclsid.is_null() || riid.is_null() {
        return HRESULT(-2147467261);
    }
    if unsafe { *rclsid } != CLSID_HUFU_TSF {
        return HRESULT(-2147467263); // CLASS_E_CLASSNOTAVAILABLE
    }
    let factory: IClassFactory = com::HuFuClassFactory.into();
    unsafe { factory.query(riid, ppv) }
}

#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    HRESULT(1) // S_FALSE：常驻
}

#[no_mangle]
extern "system" fn DllRegisterServer() -> HRESULT {
    com::register_server()
}

#[no_mangle]
extern "system" fn DllUnregisterServer() -> HRESULT {
    com::unregister_server()
}

/// 测试钩子：绕过 msctf 直接驱动「VK → 管道 → hufu-server 引擎」链。
/// 返回 1 = 引擎吃掉该键，0 = 直通/管道失败。仅供 hufu-tsf-smoke 使用。
#[no_mangle]
extern "system" fn hufu_test_key(vk: u32) -> i32 {
    tsf::test_key(vk)
}

/// 测试钩子：重置引擎会话（冒烟前置；真实应用里的 Shift 会把全局会话
/// 切成英文态污染断言）。
#[no_mangle]
extern "system" fn hufu_test_reset() -> i32 {
    i32::from(crate::ipc::reset_session())
}

/// 测试钩子：DLL→server 管道键往返微基准。n 次真实 key 请求（reset→
/// 编码增长→reset），返回平均每键耗时（µs）。回归资产：轮询分级/
/// 连接复用等 ipc 改动的量化依据。
#[no_mangle]
extern "system" fn hufu_test_key_burst(n: u32) -> i32 {
    let n = n.clamp(1, 512) as usize;
    let _ = crate::ipc::reset_session();
    let keys = ["u", "e", "y", "i", "h", "x", "m", "f", "t", "d"];
    // 预热一轮（首键建连/服务器锁热身）
    let _ = crate::ipc::key_request("u", false, false, false, false, None);
    let _ = crate::ipc::reset_session();
    let t0 = std::time::Instant::now();
    for i in 0..n {
        // 每 8 键 reset：避免编码无限增长把基准变成「长句解码测试」
        //（10 键循环拼出的串在整句方案下解码代价逐键暴涨，测不出 ipc）
        if i > 0 && i % 8 == 0 {
            let _ = crate::ipc::reset_session();
        }
        let k = keys[i % keys.len()];
        let _ = crate::ipc::key_request(k, false, false, false, false, None);
    }
    let us = t0.elapsed().as_micros() as f64 / n as f64;
    eprintln!("key-burst: {n} 键 avg {us:.0}µs/键");
    // 返回毫秒×10（i32 精度够；0 表示 <100µs）
    (us / 100.0) as i32
}

/// 测试钩子：按服务器当前皮肤渲染典型候选内容，回读像素落盘 BMP
/// （%TEMP%\hufu-pad.bmp）供视觉/数值检查内边距。返回 1=成功。
#[no_mangle]
extern "system" fn hufu_test_pad_dump() -> i32 {
    let Some(resp) = crate::ipc::call(&serde_json::json!({"op": "skin"})) else {
        eprintln!("pad-dump: skin op 失败");
        return 0;
    };
    let mut skin = resp.get("skin").cloned().unwrap_or(serde_json::Value::Null);
    // 候选数可用 HUFU_PAD_N 控制（默认 5；10=验证第 10 序号显示 0）
    let n = std::env::var("HUFU_PAD_N")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(5)
        .clamp(1, 10);
    // 【皮肤自省增强 2026-09-09】HUFU_PAD_RAW=编码串（长编码溢出验证，
    // 默认 uu）；HUFU_PAD_FONT=字号覆盖（最大号场景）；HUFU_PAD_CMT=1
    // 候选带注释（注释列宽度参与验证）。
    let raw_str = std::env::var("HUFU_PAD_RAW").unwrap_or_else(|_| "uu".into());
    let font_ovr = std::env::var("HUFU_PAD_FONT")
        .ok()
        .and_then(|v| v.parse::<f64>().ok());
    let with_cmt = std::env::var("HUFU_PAD_CMT").ok().as_deref() == Some("1");
    // HUFU_PAD_V=1：强制竖排（溢出修复验证用——当前配置横排）
    let force_v = std::env::var("HUFU_PAD_V").ok().as_deref() == Some("1");
    let words = [
        "你好", "世界", "吗", "呢", "吧", "的", "了", "是", "在", "有",
    ];
    let cmt_src = [
        "ni hao", "shijie", "shaoyong", "ne", "ba", "de", "le", "shi", "zai", "you",
    ];
    let cands: Vec<(String, String)> = words[..n]
        .iter()
        .enumerate()
        .map(|(i, w)| {
            (
                w.to_string(),
                if with_cmt && i < 4 {
                    cmt_src[i].to_string()
                } else {
                    String::new()
                },
            )
        })
        .collect();
    if force_v || font_ovr.is_some() {
        // 皮肤 JSON 顶层即 layout（无 "skin" 包裹层——server 响应
        // {"skin": <Skin>}，Skin 直含 colors/layout/material）
        if let Some(l) = skin.get_mut("layout").and_then(|l| l.as_object_mut()) {
            if let Some(fp) = font_ovr {
                l.insert("font_point".into(), serde_json::json!(fp));
            }
            if force_v {
                l.insert("horizontal".into(), serde_json::json!(false));
            }
        }
    }
    // 【全档 DPI 扫描皮肤控制 2026-09-11】外部用户 100%~500% 全档验
    // 证用：强制材质/阴影参数/横竖排——不继承 server 当前皮肤（用户
    // 实测中皮肤随时在变，扫档必须控变量）。全部环境变量门控，真机
    // 打字路径零影响。HUFU_PAD_GEO=1 时另落 %TEMP%\hufu-pad-geo.txt
    //（候选窗/玻璃阴影窗屏幕矩形，供居中外扩断言）。
    let kind_ovr = std::env::var("HUFU_PAD_KIND")
        .ok()
        .filter(|s| !s.is_empty());
    let sh_r = std::env::var("HUFU_PAD_SHADOW_R")
        .ok()
        .and_then(|v| v.parse::<f64>().ok());
    let off_x = std::env::var("HUFU_PAD_OFF_X")
        .ok()
        .and_then(|v| v.parse::<f64>().ok());
    let off_y = std::env::var("HUFU_PAD_OFF_Y")
        .ok()
        .and_then(|v| v.parse::<f64>().ok());
    let force_h = std::env::var("HUFU_PAD_LAYOUT_H").ok().as_deref() == Some("1");
    if kind_ovr.is_some() || sh_r.is_some() || off_x.is_some() || off_y.is_some() || force_h {
        if let Some(l) = skin.get_mut("layout").and_then(|l| l.as_object_mut()) {
            if let Some(r) = sh_r {
                l.insert("shadow_radius".into(), serde_json::json!(r));
            }
            if let Some(v) = off_x {
                l.insert("shadow_offset_x".into(), serde_json::json!(v));
            }
            if let Some(v) = off_y {
                l.insert("shadow_offset_y".into(), serde_json::json!(v));
            }
            if force_h {
                l.insert("horizontal".into(), serde_json::json!(true));
            }
        }
        if let Some(k) = &kind_ovr {
            if let Some(m) = skin.get_mut("material").and_then(|m| m.as_object_mut()) {
                m.insert("kind".into(), serde_json::json!(k));
            }
        }
    }
    let Some(mut w) = crate::candwin2::CandidateWindowV2::new() else {
        eprintln!("pad-dump: 候选窗初始化失败");
        return 0;
    };
    w.readback = true;
    w.show(
        &cands,
        &raw_str,
        &skin,
        Some(&windows::Win32::Foundation::RECT {
            left: 120,
            top: 120,
            right: 120,
            bottom: 144,
        }),
        0,
    );
    std::thread::sleep(std::time::Duration::from_millis(80));
    let px = w.last_pixels.take();
    let (wq, hq) = w.last_size;
    w.readback = false;
    // 【取证模式 2026-09-08】HUFU_PAD_HOLD=<ms>：渲染帧保持显示指定时长
    // （不立即 hide），供外部截屏做实机白边取证；窗口位置 (100,100)。
    let hold = std::env::var("HUFU_PAD_HOLD")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if hold > 0 {
        std::thread::sleep(std::time::Duration::from_millis(hold));
    } else {
        w.hide();
    }
    let Some(px) = px else {
        eprintln!("pad-dump: 回读失败");
        return 0;
    };
    // BMP（32bpp 自底向上）
    let mut bmp: Vec<u8> = Vec::with_capacity(54 + px.len());
    let sz = 54 + px.len() as u32;
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&sz.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(wq as i32).to_le_bytes());
    bmp.extend_from_slice(&(hq as i32).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&(px.len() as u32).to_le_bytes());
    bmp.extend_from_slice(&2835u32.to_le_bytes());
    bmp.extend_from_slice(&2835u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    let stride = (wq as usize) * 4;
    for y in (0..hq as usize).rev() {
        bmp.extend_from_slice(&px[y * stride..(y + 1) * stride]);
    }
    let path = std::env::temp_dir().join("hufu-pad.bmp");
    match std::fs::write(&path, &bmp) {
        Ok(()) => {
            eprintln!(
                "pad-dump: {} {wq}x{hq} → {}",
                path.display(),
                path.display()
            );
            1
        }
        Err(e) => {
            eprintln!("pad-dump: 写盘失败 {e}");
            0
        }
    }
}

/// 测试钩子：驱动候选窗 v2（D3D11+DComp+D2D）完整渲染一帧。
/// 返回 1 = 管线全通（设备/链/渲染/Present），0 = 初始化或渲染失败。
#[no_mangle]
extern "system" fn hufu_test_candwin2(mode: u32) -> i32 {
    use crate::candwin2::CandidateWindowV2;
    let Some(mut w) = CandidateWindowV2::new() else {
        eprintln!("candwin2: 初始化失败（回退 v1 路径可用）");
        return 0;
    };
    // 皮肤：优先从引擎取，失败用默认 translucent 样例
    let mut skin = crate::ipc::call(&serde_json::json!({"op": "skin"})).unwrap_or_else(|| {
        serde_json::json!({
            "skin": {
                "colors": {
                    "back_color": "#202022E6",
                    "border_color": "#FFFFFF26",
                    "text_color": "#E8E8EAFF",
                    "candidate_text_color": "#E8E8EAFF",
                    "comment_text_color": "#9A9AA0FF",
                    "label_color": "#C9C9C9FF",
                    "hilited_candidate_back_color": "#404046FF",
                    "hilited_candidate_text_color": "#FFFFFFFF",
                    "hilited_candidate_label_color": "#FFD75EFF"
                },
                "layout": { "font_point": 17.6, "corner_radius": 8.0,
                            "hilited_corner_radius": 6.0, "border_width": 1.0,
                            "margin_x": 10.0, "margin_y": 8.0, "line_spacing": 6.0 },
                "material": { "kind": "translucent" }
            }
        })
    });
    // mode: 0=solid 1=translucent —— 材质轮测（毛玻璃已退役）
    let kind = match mode % 2 {
        0 => "solid",
        _ => "translucent",
    };
    if let Some(s) = skin.get_mut("skin").and_then(|s| s.as_object_mut()) {
        if let Some(m) = s.get_mut("material").and_then(|m| m.as_object_mut()) {
            m.insert("kind".into(), serde_json::json!(kind));
        }
    }
    let cands = vec![
        ("你好".to_string(), "ni hao".to_string()),
        ("您好".to_string(), "".to_string()),
        ("拟好".to_string(), "少用".to_string()),
    ];
    w.show(
        &cands,
        "nih",
        &skin,
        Some(&windows::Win32::Foundation::RECT {
            left: 120,
            top: 120,
            right: 120,
            bottom: 144,
        }),
        0,
    );
    std::thread::sleep(std::time::Duration::from_millis(400));
    w.hide();
    eprintln!("candwin2: {kind} 材质渲染+隐藏完成");
    1
}

#[no_mangle]
extern "system" fn hufu_test_sound_burst() -> i32 {
    for i in 0..16u32 {
        crate::sound::play("key", 70);
        if i % 2 == 0 {
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }
    // 等全部播放线程收尾（最深队列 4×175ms + 余量）
    std::thread::sleep(std::time::Duration::from_millis(1200));
    1
}

/// 测试钩子：尺寸动效（变宽变窄）×缩放比例 100%~500% 矩阵（HUFU_FAKE_DPI
/// 伪造高 DPI）。每档：窄窗稳态 → 换宽内容（delta>24px）→ size_anim 起臂
/// → tick 推进完成 → 渲染宽随比例放大。返回位掩码 bit_i=第 i 档通过，
/// 0x1FF=9 档全通（100/125/150/175/200/250/300/400/500%）。
#[no_mangle]
extern "system" fn hufu_test_anim_scales() -> i32 {
    use crate::candwin2::CandidateWindowV2;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, IsWindowVisible, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };

    const SCALES: [f64; 9] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0];
    let skin = serde_json::json!({
        "skin": {
            "colors": {
                "back_color": "#FFFFFFE6", "border_color": "#00000060",
                "preedit_back_color": "#000000D0",
                "text_color": "#FFFFFFFF", "candidate_text_color": "#FFFFFFFF",
                "comment_text_color": "#FFDDDDFF", "label_color": "#FFFFFFCC",
                "hilited_candidate_back_color": "#CC0000FF",
                "hilited_candidate_text_color": "#FFFFFFFF"
            },
            "layout": { "font_point": 17.6, "corner_radius": 8.0,
                        "hilited_corner_radius": 6.0, "border_width": 1.0,
                        "margin_x": 10.0, "margin_y": 8.0, "line_spacing": 6.0,
                        "size_morph": 1.0 },
            "material": { "kind": "solid" }
        }
    });
    let cands = vec![
        ("你好".to_string(), "ni hao".to_string()),
        ("您好".to_string(), "".to_string()),
        ("拟好".to_string(), "少用".to_string()),
    ];
    // 【二十四修改写】入场已退役——改测「变宽变窄」尺寸动效：
    let narrow = vec![
        ("你好".to_string(), "ni".to_string()),
        ("您好".to_string(), String::new()),
    ];
    let wide = cands.clone(); // 带长注释（ni hao …）
    let anchor = RECT {
        left: 160,
        top: 160,
        right: 160,
        bottom: 184,
    };
    let Some(gsh) = crate::tsf::G_SHARED.get() else {
        return 0;
    };
    let shared = gsh.0.clone();
    let saved = {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        g.cand2.take()
    };
    let mut mask = 0u32;
    let mut base_w = 0i32;
    for (i, sc) in SCALES.iter().enumerate() {
        // 伪造 DPI（与阴影取证同旋钮）——env 进程内生效
        std::env::set_var("HUFU_FAKE_DPI", format!("{}", (sc * 96.0) as i32));
        let Some(w) = CandidateWindowV2::new() else {
            continue;
        };
        let hwnd = w.hwnd;
        {
            let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
            g.cand2 = Some(w);
            g.skin = skin.clone();
            // 稳态窄窗：首显直接全尺寸（无入场动效）
            g.last_show = Some((narrow.clone(), "nih".to_string(), 0));
            if let Some(c) = g.cand2.as_mut() {
                c.show(&narrow, "nih", &skin, Some(&anchor), 0);
            }
        }
        // 窄稳态收敛：等窗真正可见。首档 DWrite/D2D 冷启动时首次
        // ensure_swapchain 可能失败（show 早退窗不可见）→ 宽帧会走
        // 首显直达路径不起臂——可见前重试窄显（≤3 次）。
        for attempt in 0..3u32 {
            let t_st = std::time::Instant::now();
            while t_st.elapsed().as_millis() < 300 {
                unsafe {
                    let mut msg = MSG::default();
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&mut msg);
                        DispatchMessageW(&mut msg);
                    }
                }
                if unsafe { IsWindowVisible(hwnd).as_bool() } {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(4));
            }
            if unsafe { IsWindowVisible(hwnd).as_bool() } {
                break;
            }
            let _ = attempt;
            // 冷启动重试：再窄显一次（下次 ensure_swapchain 已热）
            {
                let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                g.last_show = Some((narrow.clone(), "nih".to_string(), 0));
                if let Some(c) = g.cand2.as_mut() {
                    c.show(&narrow, "nih", &skin, Some(&anchor), 0);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(30)); // 可见后再稳一拍
        // 变宽起臂：换宽内容（阈值 24px 以上 → size_anim 武装）
        {
            let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
            g.last_show = Some((wide.clone(), "nih".to_string(), 0));
            if let Some(c) = g.cand2.as_mut() {
                c.show(&wide, "nih", &skin, Some(&anchor), 0);
            }
        }

        // 起臂判定：变宽动画已武装
        let armed = {
            let g = shared.lock().unwrap_or_else(|e| e.into_inner());
            g.cand2
                .as_ref()
                .map(|c| c.size_anim.is_some())
                .unwrap_or(false)
        };
        // 推进 tick 至完成（15ms×N，留 3×余量；90ms 默认 + 高 DPI 渲染）
        let t0 = std::time::Instant::now();
        let mut done = false;
        let mut vis = false;
        let mut fin_w = 0i32;
        while t0.elapsed().as_millis() < 1200 {
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            {
                let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                g.last_key_at = Some(std::time::Instant::now());
            }
            let (anim_on, cw2) = {
                let g = shared.lock().unwrap_or_else(|e| e.into_inner());
                (
                    g.cand2
                        .as_ref()
                        .map(|c| c.size_anim.is_some())
                        .unwrap_or(false),
                    g.cand2
                        .as_ref()
                        .map(|c| c.content_size.get())
                        .unwrap_or((0, 0)),
                )
            };
            vis |= unsafe { IsWindowVisible(hwnd).as_bool() };
            fin_w = fin_w.max(cw2.0);
            if armed && !anim_on {
                done = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
        if i == 0 {
            base_w = fin_w;
        }
        // 通过口径：起臂 + 推进完成 + 可见 + 物理宽随档位走（≥基准，
        // 175% 以上允许 1.6×——高 DPI 留白同乘）
        let scaled_ok = base_w <= 0 || fin_w as f64 >= base_w as f64 * 0.95;
        let ok = armed && done && vis && scaled_ok;
        if ok {
            mask |= 1 << i;
        }
        eprintln!(
            "anim-scale {:3}%: armed={armed} done={done} vis={vis} w={fin_w}（base={base_w}）{}",
            (sc * 100.0) as i32,
            if ok { "✓" } else { "✗" }
        );
        // 清理本档窗口
        {
            let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
            let mine = g.cand2.take();
            drop(mine);
        }
    }
    std::env::remove_var("HUFU_FAKE_DPI");
    {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        g.cand2 = saved;
    }
    eprintln!("anim-scales: mask={mask:09b}（0x1FF=9 档全通）");
    mask as i32
}

/// 测试钩子：拉伸动效圆角取证（慢速 sim 200%+速度）。流程：窄窗稳态 →
/// size_ms 拉到 700ms → 宽内容触发拉伸 → 中途（≈45%）与完成各抓一帧
/// 屏幕像素，比对「角内 3px（圆角应为阴影/桌面）vs 面板内部 vs 边缘」：
/// 角内≈面板色 = 直角残片。返回 bit0=中途帧圆角 OK，bit1=完成帧圆角 OK
/// （3=通过）；eprintln 输出采样值供人工核对。
#[no_mangle]
extern "system" fn hufu_test_stretch_corner() -> i32 {
    use crate::candwin2::CandidateWindowV2;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetWindowRect, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };

    let skin = serde_json::json!({
        "skin": {
            "colors": {
                "back_color": "#FFFFFFE6", "border_color": "#00000060",
                "preedit_back_color": "#000000D0",
                "text_color": "#FFFFFFFF", "candidate_text_color": "#FFFFFFFF",
                "comment_text_color": "#FFDDDDFF", "label_color": "#FFFFFFCC",
                "hilited_candidate_back_color": "#CC0000FF",
                "hilited_candidate_text_color": "#FFFFFFFF"
            },
            "layout": { "font_point": 17.6, "corner_radius": 20.0,
                        "hilited_corner_radius": 6.0, "border_width": 1.0,
                        "margin_x": 10.0, "margin_y": 8.0, "line_spacing": 6.0,
                        "shadow_radius": 10.0 },
            "material": { "kind": "solid" }
        }
    });
    let narrow = vec![
        ("你好".to_string(), "".to_string()),
        ("您好".to_string(), "".to_string()),
    ];
    let wide = vec![
        ("你好你好你好你好你好".to_string(), "".to_string()),
        ("您好您好您好您好您好".to_string(), "".to_string()),
        ("拟好拟好拟好拟好拟好".to_string(), "".to_string()),
    ];
    let anchor = RECT {
        left: 200,
        top: 200,
        right: 200,
        bottom: 224,
    };
    let Some(gsh) = crate::tsf::G_SHARED.get() else {
        return 0;
    };
    let shared = gsh.0.clone();
    let saved = {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        g.cand2.take()
    };
    let Some(w) = CandidateWindowV2::new() else {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        g.cand2 = saved;
        return 0;
    };
    let hwnd = w.hwnd;
    let pump = || unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    };
    // 稳态窄窗
    {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        g.cand2 = Some(w);
        g.last_show = Some((narrow.clone(), "ni".to_string(), 0));
        g.skin = skin.clone();
        if let Some(c) = g.cand2.as_mut() {
            c.last_hide_at.set(None);
            c.size_ms = 1; // 稳态阶段动效近零（show 会以皮肤 layout.size_ms 再覆盖）
            c.show(&narrow, "ni", &skin, Some(&anchor), 0);
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(120));
    pump();
    // 抓帧辅助：窗口矩形 → 三采样点（角内3 / 内部 / 边缘），返回 BGR
    let grab = |tag: &str| -> Option<((i32, i32, i32), RECT)> {
        unsafe {
            let mut wr = RECT::default();
            if GetWindowRect(hwnd, &mut wr).is_err() {
                return None;
            }
            let sm = 24i32; // shadow_m 物理（σ=6, 3σ+6≈24）
            let cx = wr.right - sm; // 面板右上角
            let cy = wr.top + sm;
            let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
            if w < sm * 2 + 40 || h < sm * 2 + 40 {
                return None;
            }
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let hdc = GetDC(None);
            let mdc = CreateCompatibleDC(hdc);
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let hbmp =
                CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();
            if hbmp.is_invalid() {
                let _ = DeleteDC(mdc);
                ReleaseDC(None, hdc);
                return None;
            }
            let old = windows::Win32::Graphics::Gdi::SelectObject(mdc, hbmp);
            let _ = BitBlt(mdc, 0, 0, w, h, hdc, wr.left, wr.top, SRCCOPY);
            let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
            let got = GetDIBits(
                mdc,
                hbmp,
                0,
                h as u32,
                Some(buf.as_mut_ptr().cast()),
                &mut bmi,
                DIB_RGB_COLORS,
            );
            let _ = windows::Win32::Graphics::Gdi::SelectObject(mdc, old);
            let _ = DeleteObject(hbmp);
            let _ = DeleteDC(mdc);
            ReleaseDC(None, hdc);
            if got == 0 {
                return None;
            }
            let px = |gx: i32, gy: i32| -> (i32, i32, i32) {
                let lx = (gx - wr.left).clamp(0, w - 1) as usize;
                let ly = (gy - wr.top).clamp(0, h - 1) as usize;
                let o = (ly * w as usize + lx) * 4;
                (buf[o + 2] as i32, buf[o + 1] as i32, buf[o] as i32) // BGR→RGB
            };
            let corner = px(cx - 3, cy + 3);
            let inside = px(cx - 14, cy + 14);
            let edge = px(cx - 14, cy + 2);
            // 取证落盘：BMP（54B 头 + BGRA 当 BGRX 用）直接目检
            {
                use std::io::Write;
                let mut bmp: Vec<u8> = Vec::with_capacity(54 + buf.len());
                let stride = (w as usize * 4) as u32;
                bmp.extend_from_slice(b"BM");
                bmp.extend_from_slice(&(54u32 + stride * h as u32).to_le_bytes());
                bmp.extend_from_slice(&[0u8; 4]);
                bmp.extend_from_slice(&54u32.to_le_bytes());
                bmp.extend_from_slice(&40u32.to_le_bytes());
                bmp.extend_from_slice(&(w as i32).to_le_bytes());
                bmp.extend_from_slice(&(h as i32).to_le_bytes());
                bmp.extend_from_slice(&1u16.to_le_bytes());
                bmp.extend_from_slice(&32u16.to_le_bytes());
                bmp.extend_from_slice(&[0u8; 24]);
                bmp.extend_from_slice(&buf);
                if let Ok(mut f) = std::fs::File::create(
                    std::env::temp_dir().join(format!("hufu-stretch-{tag}.bmp")),
                ) {
                    let _ = f.write_all(&bmp);
                }
            }
            eprintln!(
                "stretch-corner[{tag}] win={w}x{h} 角内3=({},{},{}) 内部=({},{},{}) 边缘=({},{},{})",
                corner.0, corner.1, corner.2, inside.0, inside.1, inside.2, edge.0, edge.1, edge.2
            );
            Some(((corner.0, inside.0, edge.0), wr))
        }
    };
    // 慢速拉伸（sim 200%+ 观感）：宽帧皮肤 layout.size_ms=700——show()
    // 每帧以皮肤值覆盖 size_ms，直接改字段无效（首版取证即因此跑成 90ms）
    let skin_slow = {
        let mut s = skin.clone();
        s["skin"]["layout"]["size_ms"] = serde_json::json!(700);
        s
    };
    {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        g.last_show = Some((wide.clone(), "nih".to_string(), 0));
        g.skin = skin_slow.clone();
        if let Some(c) = g.cand2.as_mut() {
            c.show(&wide, "nih", &skin_slow, Some(&anchor), 0);
        }
    }
    let mut mask = 0u32;
    // 中途帧（≈45%）：角内应明显亮于面板内部（圆角让位给阴影/桌面）
    let mut mid = None;
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_millis() < 900 {
        pump();
        {
            let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
            g.last_key_at = Some(std::time::Instant::now());
        }
        if t0.elapsed().as_millis() >= 300 && mid.is_none() {
            mid = grab("mid");
        }
        let anim_on = {
            let g = shared.lock().unwrap_or_else(|e| e.into_inner());
            g.cand2
                .as_ref()
                .map(|c| c.size_anim.is_some())
                .unwrap_or(false)
        };
        if !anim_on && mid.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(6));
    }
    std::thread::sleep(std::time::Duration::from_millis(60));
    pump();
    let fin = grab("fin");
    // 判定：角内R − 内部R ≥ 22 为圆角（面板R≈暗）；< 22 且边缘≈内部 → 直角
    if let Some(((cr, ir, er), _)) = mid {
        if cr - ir >= 22 || (er - ir).abs() > 22 {
            mask |= 1;
        }
        eprintln!(
            "stretch-corner[mid] 判定: 角内-内部={} 边缘-内部={} → {}",
            cr - ir,
            er - ir,
            if mask & 1 != 0 { "圆角" } else { "直角!" }
        );
    }
    if let Some(((cr, ir, er), _)) = fin {
        if cr - ir >= 22 || (er - ir).abs() > 22 {
            mask |= 2;
        }
        eprintln!(
            "stretch-corner[fin] 判定: 角内-内部={} 边缘-内部={} → {}",
            cr - ir,
            er - ir,
            if mask & 2 != 0 { "圆角" } else { "直角!" }
        );
    }
    // 清理
    {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        let mine = g.cand2.take();
        drop(mine);
        g.cand2 = saved;
    }
    eprintln!("stretch-corner: mask={mask:02b}（3=两帧皆圆角）");
    mask as i32
}

/// 皮肤热更新 E2E：同一窗口连续两帧不同皮肤 → 屏幕捕获像素必须显著变化。
/// 返回 1 = 变化检出（渲染管线吃到了新皮肤值）；0 = 两帧几乎一样（热更新失效）。
#[no_mangle]
extern "system" fn hufu_test_skin_hot() -> i32 {
    use crate::candwin2::CandidateWindowV2;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let Some(mut w) = CandidateWindowV2::new() else {
        eprintln!("skin-hot: candwin2 初始化失败");
        return 0;
    };
    // 基础皮肤来自引擎（结构/字体真实），只覆盖颜色做 A/B
    let mut base = crate::ipc::call(&serde_json::json!({"op": "skin"})).unwrap_or_else(
        || serde_json::json!({"skin": {"colors": {}, "layout": {}, "material": {"kind": "solid"}}}),
    );
    // 强制 solid + 不透明底色：排除 accent 语义干扰，纯看颜色渲染。
    // 【基线钉死】master/hilite/shadow/border 四个透明度也一并锁 1.0
    // ——皮肤热数据（用户滑条设置）会随「服务器当前皮肤」混进基线，
    // 曾因墨岩 hilite_alpha=0.7 把胶囊压暗致断言假红。
    if let Some(s) = base.get_mut("skin").and_then(|s| s.as_object_mut()) {
        if let Some(m) = s.get_mut("material").and_then(|m| m.as_object_mut()) {
            m.insert("kind".into(), serde_json::json!("solid"));
            m.insert("master_alpha".into(), serde_json::json!(1.0));
            m.insert("hilite_alpha".into(), serde_json::json!(1.0));
            m.insert("shadow_alpha".into(), serde_json::json!(1.0));
            m.insert("border_alpha".into(), serde_json::json!(1.0));
        }
    }
    let set_colors = |sk: &mut serde_json::Value, back: &str, hilight: &str, hitext: &str| {
        // 显式锁定竖排，隔离当前皮肤（可能是横排预设）对测试基线的污染
        if let Some(l) = sk
            .pointer_mut("/skin/layout")
            .and_then(|l| l.as_object_mut())
        {
            l.insert("horizontal".into(), serde_json::json!(false));
        }
        if let Some(s) = sk.get_mut("skin").and_then(|s| s.as_object_mut()) {
            if let Some(c) = s.get_mut("colors").and_then(|c| c.as_object_mut()) {
                c.insert("back_color".into(), serde_json::json!(back));
                c.insert(
                    "hilited_candidate_back_color".into(),
                    serde_json::json!(hilight),
                );
                c.insert(
                    "hilited_candidate_text_color".into(),
                    serde_json::json!(hitext),
                );
            }
        }
    };
    let mut skin_a = base.clone();
    set_colors(&mut skin_a, "#101014FF", "#3050A0FF", "#FFFFFFFF"); // 深底·蓝高亮
    let mut skin_b = base.clone();
    set_colors(&mut skin_b, "#F5F0E6FF", "#C03030FF", "#101010FF"); // 浅底·红高亮
    // 首候选行 y：阴影边距下有平移——胶囊检查用竖带扫描（见②）
    // 行内水平扫描范围：避开序号列，覆盖胶囊主体
    let margin_probe = |w: usize| -> std::ops::Range<usize> {
        let s = (w * 15 / 100).max(20);
        let e = (w * 70 / 100).min(w.saturating_sub(4));
        s..e.max(s + 1)
    };

    let cands = vec![
        ("你好".to_string(), "ni hao".to_string()),
        ("您好".to_string(), "".to_string()),
        ("拟好".to_string(), "".to_string()),
        ("腻好".to_string(), "".to_string()),
        ("逆耗".to_string(), "".to_string()),
    ];
    let capture = |w: &CandidateWindowV2| -> Option<Vec<u8>> {
        unsafe {
            let mut rc = windows::Win32::Foundation::RECT::default();
            if GetWindowRect(w.hwnd, &mut rc).is_err() {
                return None;
            }
            let wd = (rc.right - rc.left).max(1) as i32;
            let ht = (rc.bottom - rc.top).max(1) as i32;
            let hdc_screen = GetDC(None);
            let hdc_mem = CreateCompatibleDC(hdc_screen);
            let mut bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: wd,
                    biHeight: -ht, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let hb = match CreateDIBSection(hdc_screen, &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(h) => h,
                Err(_) => {
                    let _ = DeleteDC(hdc_mem);
                    ReleaseDC(None, hdc_screen);
                    return None;
                }
            };
            if bits.is_null() {
                let _ = DeleteObject(hb);
                let _ = DeleteDC(hdc_mem);
                ReleaseDC(None, hdc_screen);
                return None;
            }
            let _ = windows::Win32::Graphics::Gdi::SelectObject(hdc_mem, hb);
            // DComp/NOREDIRECTIONBITMAP 窗口对 PrintWindow 免疫，BitBlt 屏幕坐标捕获
            let _ = BitBlt(hdc_mem, 0, 0, wd, ht, hdc_screen, rc.left, rc.top, SRCCOPY);
            let n = (wd * ht * 4) as usize;
            let mut buf = vec![0u8; n];
            let mut copied = 0usize;
            if GetDIBits(
                hdc_mem,
                hb,
                0,
                ht as u32,
                Some(buf.as_mut_ptr() as *mut std::ffi::c_void),
                &mut bi,
                DIB_RGB_COLORS,
            ) != 0
            {
                copied = n;
            }
            let _ = DeleteObject(hb);
            let _ = DeleteDC(hdc_mem);
            ReleaseDC(None, hdc_screen);
            if copied == n { Some(buf) } else { None }
        }
    };

    // 帧 A → 捕获；帧 B（同一窗口实例，模拟词边界热换肤）→ 捕获
    w.show(
        &cands,
        "nih",
        &skin_a,
        Some(&windows::Win32::Foundation::RECT {
            left: 120,
            top: 120,
            right: 120,
            bottom: 144,
        }),
        0,
    );
    std::thread::sleep(std::time::Duration::from_millis(250));
    let cap_a = capture(&w);
    w.show(
        &cands,
        "nih",
        &skin_b,
        Some(&windows::Win32::Foundation::RECT {
            left: 120,
            top: 120,
            right: 120,
            bottom: 144,
        }),
        0,
    );
    std::thread::sleep(std::time::Duration::from_millis(250));
    let cap_b = capture(&w);
    w.hide();
    let (Some(a), Some(b)) = (cap_a, cap_b) else {
        eprintln!("skin-hot: 屏幕捕获失败");
        return 0;
    };
    let px = a.len() / 4;
    let diff_ratio = |x: &[u8], y: &[u8]| -> f64 {
        let n = x.len().min(y.len()) / 4;
        let mut d = 0usize;
        for i in 0..n {
            let dv = (x[i * 4] as i32 - y[i * 4] as i32).abs()
                + (x[i * 4 + 1] as i32 - y[i * 4 + 1] as i32).abs()
                + (x[i * 4 + 2] as i32 - y[i * 4 + 2] as i32).abs();
            if dv > 48 {
                d += 1;
            }
        }
        d as f64 / n.max(1) as f64
    };
    let ratio = diff_ratio(&a, &b);
    eprintln!("skin-hot: 颜色 A/B {px}px 差异 {:.1}%", ratio * 100.0);
    if ratio <= 0.05 {
        return 0;
    }

    // ── 材质回读断言（D2D 位图；屏幕 BitBlt 对 DComp 窗口不可靠）──
    // 稳定可断言：① 圆角四角真透明 ② 高亮胶囊颜色精确 ③ 文本像素存在
    let mut skin_f = base.clone();
    set_colors(&mut skin_f, "#101014FF", "#3050A0FF", "#FFFFFFFF");
    if let Some(s) = skin_f.get_mut("skin").and_then(|s| s.as_object_mut()) {
        if let Some(m) = s.get_mut("material").and_then(|m| m.as_object_mut()) {
            m.insert("kind".into(), serde_json::json!("frosted"));
            m.insert("tint".into(), serde_json::json!("#2C3E50D8"));
            m.insert("opacity".into(), serde_json::json!(1.0));
        }
    }
    w.readback = true;
    w.show(
        &cands,
        "nih",
        &skin_f,
        Some(&windows::Win32::Foundation::RECT {
            left: 120,
            top: 120,
            right: 120,
            bottom: 144,
        }),
        0,
    );
    let mut rc_f = windows::Win32::Foundation::RECT::default();
    let _ = unsafe { GetWindowRect(w.hwnd, &mut rc_f) };
    let (fw, fh) = (rc_f.right - rc_f.left, rc_f.bottom - rc_f.top);
    let Some(f_px) = w.last_pixels.take() else {
        eprintln!("skin-hot: frosted 回读失败");
        return 0;
    };
    {
        // 【no-shrink 适配】用 last_size（内容尺寸）而非窗口 rect
        let (wq, hq) = (
            (w.last_size.0.max(1)) as usize,
            (w.last_size.1.max(1)) as usize,
        );
        let px = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * wq + x) * 4;
            [f_px[i], f_px[i + 1], f_px[i + 2], f_px[i + 3]]
        };
        // ① 圆角：四角 alpha≈0（窗口真透明圆角，非「补直角」）
        for (x, y) in [(1usize, 1usize), (wq - 2, 1), (1, hq - 2), (wq - 2, hq - 2)] {
            let c = px(x, y);
            if c[3] > 30 {
                eprintln!("skin-hot: 圆角失效（{x},{y} a={}）", c[3]);
                return 0;
            }
        }
        // ② 高亮胶囊色精确（首行高亮 #3050A0）：竖带扫描——阴影边距使
        // 首行 y 整体平移，带状扫描对边距鲁棒
        let mut pill_hit = 0usize;
        let y_lo = (fh as usize * 12 / 100).max(8);
        let y_hi = (fh as usize * 32 / 100).min(hq.saturating_sub(2));
        for y in y_lo..y_hi.max(y_lo + 1) {
            for gx in margin_probe(wq) {
                let c = px(gx, y);
                if c[2] >= 40 && c[2] <= 60 && c[0] >= 145 && c[0] <= 175 && c[3] > 200 {
                    pill_hit += 1;
                }
            }
        }
        // ③ 文本像素存在（R 通道亮像素）+ 上下留白对称性（光学居中诊断）
        let mut text_px = 0usize;
        let mut top_bright = usize::MAX;
        let mut bot_bright = 0usize;
        for y in 0..hq {
            for x in 0..wq {
                if px(x, y)[2] > 180 {
                    text_px += 1;
                    if y < top_bright {
                        top_bright = y;
                    }
                    if y > bot_bright {
                        bot_bright = y;
                    }
                }
            }
        }
        let gap_top = top_bright as i32;
        let gap_bot = (hq as i32 - 1) - bot_bright as i32;
        let dy_dbg = w.last_dy.take().unwrap_or(f32::NAN);
        // ④ 投影存在：外边距环内半透明像素（阴影渲染真值——阴影曾经
        //    整个没画过，此断言防再死回归）
        let mut sh_px = 0usize;
        for y in 0..hq {
            for x in 0..wq {
                let a = px(x, y)[3];
                if a > 8 && a < 200 {
                    sh_px += 1;
                }
            }
        }
        eprintln!(
            "skin-hot: 回读 {wq}x{hq} 四角透明✓ 胶囊命中 {pill_hit} 亮像素 {text_px} 阴影像素 {sh_px} 留白上{gap_top}/下{gap_bot} dy={dy_dbg:.1}"
        );
        if sh_px < 400 {
            eprintln!("skin-hot: 投影未渲染（阴影像素 {sh_px}）");
            return 0;
        }
        if pill_hit < 6 {
            eprintln!("skin-hot: 高亮胶囊颜色不符（内边距/颜色回归）");
            return 0;
        }
        if text_px < 50 {
            eprintln!("skin-hot: 候选文本缺失");
            return 0;
        }
    }
    w.readback = false;
    eprintln!("skin-hot: 材质回读断言 ✓（圆角透明/胶囊色/文本）");

    // ── 横排：同一候选集下窗口必须变宽变矮（5 候选几何上必然分离）──
    let mut skin_h = skin_f.clone();
    if let Some(s) = skin_h.get_mut("skin").and_then(|s| s.as_object_mut()) {
        if let Some(l) = s.get_mut("layout").and_then(|l| l.as_object_mut()) {
            l.insert("horizontal".into(), serde_json::json!(true));
        }
    }
    w.show(
        &cands,
        "nih",
        &skin_h,
        Some(&windows::Win32::Foundation::RECT {
            left: 120,
            top: 120,
            right: 120,
            bottom: 144,
        }),
        0,
    );
    // 轮询等待尺寸真正变化上屏（SetWindowPos 异步，固定 sleep 有竞态）。
    // 【no-shrink 适配】窗口 rect 高度不收缩——判定与计量都用
    // content_size（内容实际尺寸）
    let mut rc_h = windows::Win32::Foundation::RECT::default();
    let mut settled = false;
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = unsafe { GetWindowRect(w.hwnd, &mut rc_h) };
        let (cw, chh) = w.content_size.get();
        if cw > fw + 15 && chh < fh - 8 {
            settled = true;
            break;
        }
    }
    // 横排留白回读（用户皮肤即横排；窗口可能比内容先到，再等一帧）
    w.readback = true;
    w.show(
        &cands,
        "nih",
        &skin_h,
        Some(&windows::Win32::Foundation::RECT {
            left: 120,
            top: 120,
            right: 120,
            bottom: 144,
        }),
        0,
    );
    std::thread::sleep(std::time::Duration::from_millis(60));
    // 【no-shrink 适配】窗口 rect 可能有透明余量——回读尺寸必须用
    // last_size（内容实际渲染尺寸），否则索引越界
    let (wq, hq) = (
        (w.last_size.0.max(1)) as usize,
        (w.last_size.1.max(1)) as usize,
    );
    if let Some(h_px) = w.last_pixels.take() {
        let mut top_b = usize::MAX;
        let mut bot_b = 0usize;
        for y in 0..hq {
            for x in 0..wq {
                let i = (y * wq + x) * 4;
                if h_px[i + 2] > 180 {
                    if y < top_b {
                        top_b = y;
                    }
                    if y > bot_b {
                        bot_b = y;
                    }
                }
            }
        }
        if top_b != usize::MAX {
            eprintln!(
                "skin-hot: 横排留白 上{} / 下{}（差 {}）",
                top_b,
                (hq - 1) - bot_b,
                ((hq - 1) - bot_b) as i32 - top_b as i32
            );
        }
    }
    w.readback = false;
    w.hide();
    let hw = (rc_h.right - rc_h.left).max(1);
    let hh = (rc_h.bottom - rc_h.top).max(1);
    eprintln!("skin-hot: 横排窗口 {hw}x{hh}（竖排 {fw}x{fh}）");
    if !settled {
        return 0; // 横排未生效
    }
    1
}
