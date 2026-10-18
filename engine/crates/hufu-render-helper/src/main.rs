//! 【SSF 实时框渲染小助手 2026-10-16】
//!
//! 用法：`hufu-render-helper.exe <输出.bmp> <hufu_tsf.dll 路径>`
//! 环境变量（server 侧已设）：HUFU_SKIN_DUMP_V2=1、HUFU_TSF_SMOKE=1、
//! HUFU_SKIN_DUMP_ID=<皮肤id>（可选：渲染指定皮肤）。
//!
//! 退出码：0=成功（BMP 已写出）；1=参数/加载失败；2=dump 调用失败。
//! DLL 内部读 HUFU_SKIN_DUMP 环境变量定位输出——本助手把 argv[1] 写
//! 进去，两处单一事实源不冲突。

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: hufu-render-helper <out.bmp> <hufu_tsf.dll>");
        std::process::exit(1);
    }
    std::env::set_var("HUFU_SKIN_DUMP", &args[1]);
    let dll = args[2].clone();
    let dll_bytes = dll.clone().into_bytes();
    let dll_ptr = dll_bytes.as_ptr();
    unsafe {
        let h = windows_sys::LoadLibraryA(dll_ptr);        if h.is_null() {
            eprintln!("LoadLibrary failed: {}", dll);
            std::process::exit(1);
        }
        let proc = windows_sys::GetProcAddress(h, b"hufu_test_skin_dump\0".as_ptr());
        if proc.is_null() {
            eprintln!("GetProcAddress(hufu_test_skin_dump) failed");
            std::process::exit(1);
        }
        // dump 生成+落盘在 DLL 内同步完成（含 0ms hold）——返回即
        // BMP 已写盘，无需再等（早前 900ms 是对 DLL 内部时序的误判）。
        let dump: unsafe extern "system" fn(u32) -> i32 = std::mem::transmute(proc);
        let r = dump(0);
        windows_sys::FreeLibrary(h);
        if r == 0 {
            std::process::exit(2);
        }
    }
    std::process::exit(0);
}

/// 手写最小 P/Invoke（零依赖 crate：windows/windows-sys 都不必引）。
mod windows_sys {
    #[allow(non_snake_case)]
    pub type HMODULE = *mut core::ffi::c_void;
    pub type FARPROC = *mut core::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn LoadLibraryA(name: *const u8) -> HMODULE;
        pub fn GetProcAddress(module: HMODULE, name: *const u8) -> FARPROC;
        pub fn FreeLibrary(module: HMODULE) -> i32;
    }
}
