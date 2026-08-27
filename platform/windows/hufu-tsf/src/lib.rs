//! hufu-tsf —— HuFu 输入法 Windows TSF 前端（纯 Rust COM DLL）。
//!
//! 架构（同小狼毫）：本 DLL 是薄壳——按键事件通过命名管道发给 hufu-server
//! 引擎，取回 {consumed, commit, state} 后操作 TSF 组段并绘制候选窗。

mod candwin;
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
