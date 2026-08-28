//! hufu-tsf —— HuFu 输入法 Windows TSF 前端（纯 Rust COM DLL）。
//!
//! 架构（同小狼毫）：本 DLL 是薄壳——按键事件通过命名管道发给 hufu-server
//! 引擎，取回 {consumed, commit, state} 后操作 TSF 组段并绘制候选窗。

mod candwin;
mod candwin2;
mod com;
mod ipc;
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
    w.show(&cands, "nih", &skin, None, 0);
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
        crate::sound::play("key");
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
    // 强制 solid + 不透明底色：排除 accent 语义干扰，纯看颜色渲染
    if let Some(s) = base.get_mut("skin").and_then(|s| s.as_object_mut()) {
        if let Some(m) = s.get_mut("material").and_then(|m| m.as_object_mut()) {
            m.insert("kind".into(), serde_json::json!("solid"));
        }
    }
    let set_colors = |sk: &mut serde_json::Value, back: &str, hilight: &str, hitext: &str| {
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

    let cands = vec![
        ("你好".to_string(), "ni hao".to_string()),
        ("您好".to_string(), "".to_string()),
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
    w.show(&cands, "nih", &skin_a, None, 0);
    std::thread::sleep(std::time::Duration::from_millis(250));
    let cap_a = capture(&w);
    w.show(&cands, "nih", &skin_b, None, 0);
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

    // ── 材质可见性：竖排 solid vs 竖排 frosted（自绘磨砂层必须改变外观）──
    let mut skin_f = base.clone();
    set_colors(&mut skin_f, "#101014FF", "#3050A0FF", "#FFFFFFFF");
    if let Some(s) = skin_f.get_mut("skin").and_then(|s| s.as_object_mut()) {
        if let Some(m) = s.get_mut("material").and_then(|m| m.as_object_mut()) {
            m.insert("kind".into(), serde_json::json!("frosted"));
            m.insert("tint".into(), serde_json::json!("#2C3E50D8"));
            m.insert("noise".into(), serde_json::json!(45.0));
        }
    }
    w.show(&cands, "nih", &skin_f, None, 0);
    std::thread::sleep(std::time::Duration::from_millis(250));
    let cap_f = capture(&w);
    w.hide();
    let Some(f) = cap_f else {
        eprintln!("skin-hot: frosted 捕获失败");
        return 0;
    };
    let fr = diff_ratio(&a, &f);
    eprintln!("skin-hot: solid vs frosted 差异 {:.1}%（磨砂层可见性）", fr * 100.0);
    if fr <= 0.03 {
        return 0;
    }

    // ── 横排：窗口宽高比必须翻转（w > h）──
    let mut skin_h = skin_f.clone();
    if let Some(s) = skin_h.get_mut("skin").and_then(|s| s.as_object_mut()) {
        if let Some(l) = s.get_mut("layout").and_then(|l| l.as_object_mut()) {
            l.insert("horizontal".into(), serde_json::json!(true));
        }
    }
    w.show(&cands, "nih", &skin_h, None, 0);
    std::thread::sleep(std::time::Duration::from_millis(250));
    let mut rc_h = windows::Win32::Foundation::RECT::default();
    let _ = unsafe { GetWindowRect(w.hwnd, &mut rc_h) };
    let cap_h = capture(&w);
    w.hide();
    let hw = (rc_h.right - rc_h.left).max(1);
    let hh = (rc_h.bottom - rc_h.top).max(1);
    eprintln!("skin-hot: 横排窗口 {hw}x{hh}");
    if hw <= hh {
        return 0; // 横排未生效
    }
    if let Some(h) = cap_h {
        let hr = diff_ratio(&f, &h);
        eprintln!("skin-hot: 竖排 vs 横排 frosted 差异 {:.1}%", hr * 100.0);
        if hr <= 0.05 {
            return 0;
        }
    }
    1
}
