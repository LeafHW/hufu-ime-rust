//! i686 windows-gnu 链接补丁：DLL 入口 stub。
//! mingw 世界里 `_DllEntryPoint@12` 由 libmingw32.a 的 dll_entry.o 提供
//! （转发 DllMainCRTStartup）。本 i686 组合用的 llvm-mingw libmingw32.a
//! 无此符号（其 DLL 启动走别的路径），而 Rust std 侧 crt 链接序列仍引用
//! 它——在此手工提供等价转发，保持与 binutils libmingw32 一致的行为。
//! 仅 i686 gnu 编译；x86_64（有原生 libmingw32）不编入。

#![cfg(all(target_arch = "x86", target_env = "gnu"))]

use core::ffi::c_int;

unsafe extern "system" {
    fn DllMainCRTStartup(
        hinst: *mut core::ffi::c_void,
        reason: u32,
        reserved: *mut core::ffi::c_void,
    ) -> c_int;
}

#[no_mangle]
pub unsafe extern "system" fn DllEntryPoint(
    hinst: *mut core::ffi::c_void,
    reason: u32,
    reserved: *mut core::ffi::c_void,
) -> c_int {
    DllMainCRTStartup(hinst, reason, reserved)
}
