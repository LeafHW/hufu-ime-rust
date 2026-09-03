//! hufu-tsf —— HuFu 输入法 Windows TSF 前端（纯 Rust COM DLL）。
//!
//! 架构（同小狼毫）：本 DLL 是薄壳——按键事件通过命名管道发给 hufu-server
//! 引擎，取回 {consumed, commit, state} 后操作 TSF 组段并绘制候选窗。

mod candwin;
mod candwin2;
mod candwin3;
mod canduielement;
mod com;
// i686 windows-gnu 交叉链接补丁：llvm libmingw32 无 _DllEntryPoint@12，
// 由本模块 stub 提供（转发 DllMainCRTStartup）。x86_64 不编入。
#[cfg(all(target_arch = "x86", target_env = "gnu"))]
mod dll_entry_x86;
mod ipc;
// 语言栏按钮已下线（Win11 桌面语言栏为可拖浮动条，用户实测否决）；
// langbar.rs 源码保留，备将来做中/英态切换时复用。
#[allow(unused)]
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
    let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
    let _ = std::fs::write(
        format!(r"C:\ProgramData\HuFu\diag\load-{}.txt", std::process::id()),
        format!(
            "load dll={} t={:?}\n",
            com::self_path_for_diag(),
            std::time::SystemTime::now()
        ),
    );
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
    let _ = crate::ipc::key_request("u", false, false, false, false);
    let _ = crate::ipc::reset_session();
    let t0 = std::time::Instant::now();
    for i in 0..n {
        // 每 8 键 reset：避免编码无限增长把基准变成「长句解码测试」
        //（10 键循环拼出的串在整句方案下解码代价逐键暴涨，测不出 ipc）
        if i > 0 && i % 8 == 0 {
            let _ = crate::ipc::reset_session();
        }
        let k = keys[i % keys.len()];
        let _ = crate::ipc::key_request(k, false, false, false, false);
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
    let skin = resp.get("skin").cloned().unwrap_or(serde_json::Value::Null);
    let Some(mut w) = crate::candwin2::CandidateWindowV2::new() else {
        eprintln!("pad-dump: 候选窗初始化失败");
        return 0;
    };
    let cands = vec![
        ("你好".to_string(), String::new()),
        ("世界".to_string(), String::new()),
        ("吗".to_string(), String::new()),
        ("呢".to_string(), String::new()),
        ("吧".to_string(), String::new()),
    ];
    w.readback = true;
    w.show(&cands, "uu", &skin, Some(&windows::Win32::Foundation::RECT { left: 120, top: 120, right: 120, bottom: 144 }), 0);
    std::thread::sleep(std::time::Duration::from_millis(80));
    let px = w.last_pixels.take();
    let (wq, hq) = w.last_size;
    w.readback = false;
    w.hide();
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
            eprintln!("pad-dump: {} {wq}x{hq} → {}", path.display(), path.display());
            1
        }
        Err(e) => {
            eprintln!("pad-dump: 写盘失败 {e}");
            0
        }
    }
}

/// 测试钩子：驱动候选窗 v2（D3D11+DComp+D2D+Acrylic accent）完整渲染一帧。
/// 返回 1 = 管线全通（设备/链/渲染/Present），0 = 初始化或渲染失败。
#[no_mangle]
extern "system" fn hufu_test_candwin2(mode: u32) -> i32 {
    use crate::candwin2::CandidateWindowV2;
    let Some(mut w) = CandidateWindowV2::new() else {
        eprintln!("candwin2: 初始化失败（回退 v1 路径可用）");
        return 0;
    };
    // 皮肤：优先从引擎取（材质 accent 用得上），失败用默认 frosted 样例
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
                "material": { "kind": "frosted", "tint": "#1C1C1ECC" }
            }
        })
    });
    // mode: 0=solid 1=translucent 2=frosted(acrylic) 3=glass —— 轮一遍材质
    let kind = match mode % 4 {
        0 => "solid",
        1 => "translucent",
        2 => "frosted",
        _ => "glass",
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
    w.show(&cands, "nih", &skin, Some(&windows::Win32::Foundation::RECT { left: 120, top: 120, right: 120, bottom: 144 }), 0);
    std::thread::sleep(std::time::Duration::from_millis(400));
    w.hide();
    eprintln!("candwin2: {kind} 材质渲染+隐藏完成");
    1
}

/// 音效池化播放练习：16 次急速连击（0/15ms 间隔），压排队深度；
/// 任何一次崩溃/死锁返回 0（崩溃使进程直接退出）。
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

/// 皮肤热更新 E2E：同一窗口连续两帧不同皮肤 → 屏幕捕获像素必须显著变化。
/// 返回 1 = 变化检出（渲染管线吃到了新皮肤值）；0 = 两帧几乎一样（热更新失效）。
#[no_mangle]
extern "system" fn hufu_test_skin_hot() -> i32 {
    use crate::candwin2::CandidateWindowV2;
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetDIBits,
        ReleaseDC, SRCCOPY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let Some(mut w) = CandidateWindowV2::new() else {
        eprintln!("skin-hot: candwin2 初始化失败");
        return 0;
    };
    // 基础皮肤来自引擎（结构/字体真实），只覆盖颜色做 A/B
    let mut base = crate::ipc::call(&serde_json::json!({"op": "skin"})).unwrap_or_else(|| {
        serde_json::json!({"skin": {"colors": {}, "layout": {}, "material": {"kind": "solid"}}})
    });
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
                c.insert("hilited_candidate_back_color".into(), serde_json::json!(hilight));
                c.insert("hilited_candidate_text_color".into(), serde_json::json!(hitext));
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
            BitBlt(hdc_mem, 0, 0, wd, ht, hdc_screen, rc.left, rc.top, SRCCOPY);
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
    w.show(&cands, "nih", &skin_a, Some(&windows::Win32::Foundation::RECT { left: 120, top: 120, right: 120, bottom: 144 }), 0);
    std::thread::sleep(std::time::Duration::from_millis(250));
    let cap_a = capture(&w);
    w.show(&cands, "nih", &skin_b, Some(&windows::Win32::Foundation::RECT { left: 120, top: 120, right: 120, bottom: 144 }), 0);
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
    w.show(&cands, "nih", &skin_f, Some(&windows::Win32::Foundation::RECT { left: 120, top: 120, right: 120, bottom: 144 }), 0);
    let mut rc_f = windows::Win32::Foundation::RECT::default();
    let _ = unsafe { GetWindowRect(w.hwnd, &mut rc_f) };
    let (fw, fh) = (rc_f.right - rc_f.left, rc_f.bottom - rc_f.top);
    let Some(f_px) = w.last_pixels.take() else {
        eprintln!("skin-hot: frosted 回读失败");
        return 0;
    };
    {
        let (wq, hq) = (fw as usize, fh as usize);
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
            for gx in (margin_probe(wq)) {
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
    w.show(&cands, "nih", &skin_h, Some(&windows::Win32::Foundation::RECT { left: 120, top: 120, right: 120, bottom: 144 }), 0);
    // 轮询等待尺寸真正变化上屏（SetWindowPos 异步，固定 sleep 有竞态）
    let mut rc_h = windows::Win32::Foundation::RECT::default();
    let mut settled = false;
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = unsafe { GetWindowRect(w.hwnd, &mut rc_h) };
        let (cw, chh) = (rc_h.right - rc_h.left, rc_h.bottom - rc_h.top);
        if cw > fw + 15 && chh < fh - 8 {
            settled = true;
            break;
        }
    }
    // 横排留白回读（用户皮肤即横排；窗口可能比内容先到，再等一帧）
    w.readback = true;
    w.show(&cands, "nih", &skin_h, Some(&windows::Win32::Foundation::RECT { left: 120, top: 120, right: 120, bottom: 144 }), 0);
    std::thread::sleep(std::time::Duration::from_millis(60));
    let (wq, hq) = (
        ((rc_h.right - rc_h.left).max(1)) as usize,
        ((rc_h.bottom - rc_h.top).max(1)) as usize,
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
