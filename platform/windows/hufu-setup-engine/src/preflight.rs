//! Read-only package admission. No registration or service launch.
use crate::{
    common::Out,
    package::{Package, MANIFEST},
    procop::{self, SpawnResult},
};
use std::path::Path;
pub fn architecture() -> Result<&'static str, String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetModuleHandleW(name: *const u16) -> isize;
        fn GetProcAddress(module: isize, name: *const u8) -> *const std::ffi::c_void;
    }
    let name: Vec<u16> = "kernel32.dll".encode_utf16().chain(Some(0)).collect();
    let function = unsafe {
        GetProcAddress(
            GetModuleHandleW(name.as_ptr()),
            b"IsWow64Process2\0".as_ptr(),
        )
    };
    if !function.is_null() {
        type Query = unsafe extern "system" fn(isize, *mut u16, *mut u16) -> i32;
        let query: Query = unsafe { std::mem::transmute(function) };
        let (mut process, mut native) = (0u16, 0u16);
        if unsafe { query(GetCurrentProcess(), &mut process, &mut native) } == 0 {
            return Err("IsWow64Process2 failed".into());
        }
        return match native {
            0xaa64 => Ok("arm64"),
            0x8664 => Ok("x64"),
            _ => Err(format!("unsupported native machine {native:04x}")),
        };
    }
    // Windows 10 1607 has no IsWow64Process2. This reports the OS, not process bitness.
    use windows::Win32::System::SystemInformation::{GetNativeSystemInfo, SYSTEM_INFO};
    let mut info = SYSTEM_INFO::default();
    unsafe {
        GetNativeSystemInfo(&mut info);
    }
    match unsafe { info.Anonymous.Anonymous.wProcessorArchitecture.0 } {
        12 => Ok("arm64"),
        9 => Ok("x64"),
        other => Err(format!("unsupported native architecture {other}")),
    }
}
pub fn probe(root: &Path, p: &Package) -> Result<(), String> {
    for (tool_role, dll_role) in [
        ("registration-native", "tsf-native"),
        ("registration-x86", "tsf-x86"),
    ] {
        let tool = root.join(&p.entry(tool_role).path);
        let dll = root.join(&p.entry(dll_role).path);
        match procop::spawn_wait_hidden(&tool, &format!("probe-dll \"{}\"", dll.display()), 60_000)
        {
            SpawnResult::Exit(0) => {}
            other => return Err(format!("{tool_role} probe failed: {other:?}")),
        }
    }
    if let Some(tool) = p.manifest.files.iter().find(|f| f.role == "probe-x64") {
        let dll = root.join(&p.entry("tsf-native").path);
        if procop::spawn_wait_hidden(
            &root.join(&tool.path),
            &format!("probe-dll \"{}\"", dll.display()),
            60_000,
        ) != SpawnResult::Exit(0)
        {
            return Err("x64 facade probe failed".into());
        }
    }
    Ok(())
}
pub fn run(out: &mut Out, root: &Path) -> i32 {
    let result = (|| {
        let arch = architecture()?;
        let legacy = match std::fs::symlink_metadata(root.join(MANIFEST)) {
            Ok(_) => false,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            Err(e) => return Err(e.to_string()),
        };
        if legacy {
            if arch != "x64" {
                return Err("legacy payload without architecture manifest is x64-only".into());
            }
            for (name, machine) in [
                ("hufu_tsf.dll", 0x8664),
                ("hufu_tsf32.dll", 0x14c),
                ("hufu-tsf-smoke.exe", 0x8664),
            ] {
                let b = std::fs::read(root.join(name)).map_err(|e| format!("{name}: {e}"))?;
                if crate::package::pe_machine(&b)? != machine {
                    return Err(format!("legacy PE mismatch: {name}"));
                }
            }
            out.line("OK legacy x64 payload (no generation/hash contract)");
            return Ok(());
        }
        let p = Package::load(root, arch)?;
        probe(root, &p)?;
        out.line(&format!("OK payload {} {}", arch, p.generation));
        Ok(())
    })();
    match result {
        Ok(()) => 0,
        Err(e) => {
            out.line(&format!("Preflight failed: {e}"));
            1
        }
    }
}
