//! hufu-tsf —— HuFu 输入法 Windows TSF 前端（纯 Rust COM DLL）。
//!
//! 架构（同小狼毫）：本 DLL 是薄壳——按键事件通过命名管道发给 hufu-server
//! 引擎，取回 {consumed, commit, state} 后操作 TSF 组段并绘制候选窗。

mod candwin;
mod candwin2;
mod com;
mod ipc;
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
    w.show(&cands, "nih", &skin);
    std::thread::sleep(std::time::Duration::from_millis(400));
    w.hide();
    eprintln!("candwin2: {kind} 材质渲染+隐藏完成");
    1
}
