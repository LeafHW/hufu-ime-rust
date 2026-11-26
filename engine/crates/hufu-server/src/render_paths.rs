//! Resolve a matching helper/DLL pair from this installation, never another product's
//! hard-coded drive or an arbitrarily newest SystemIME generation.
use std::path::{Component, Path, PathBuf};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct RenderPaths { pub helper: PathBuf, pub dll: PathBuf }

pub fn resolve() -> Option<RenderPaths> {
    // Explicit paired overrides support isolated probes and generation-specific installs.
    match (std::env::var_os("HUFU_RENDER_HELPER"), std::env::var_os("HUFU_RENDER_DLL")) {
        (Some(h), Some(d)) => return existing(PathBuf::from(h), PathBuf::from(d)),
        (None, None) => (),
        _ => return None,
    }
    let exe = std::env::current_exe().ok()?;
    let root = exe.parent()?;
    let manifest = root.join("package-architecture.json");
    if manifest.exists() {
        // A damaged or incompatible new package must not silently select stale legacy DLLs.
        return from_manifest(root, &std::fs::read(manifest).ok()?);
    }
    if let Some(pair) = legacy(root, None) { return Some(pair); }
    #[cfg(windows)]
    if let Some(dll) = registered_dll() {
        if let Some(dir) = dll.parent() {
            // A registered generation is authoritative; do not scan/sort unrelated generations.
            if let Some(pair) = existing(root.join("hufu-render-helper.exe"), dll.clone()) {
                return Some(pair);
            }
            if let Some(pair) = existing(dir.join("hufu-render-helper.exe"), dll.clone()) {
                return Some(pair);
            }
        }
    }
    legacy(root, std::env::var_os("SystemRoot").map(PathBuf::from).as_deref())
}

fn existing(helper: PathBuf, dll: PathBuf) -> Option<RenderPaths> {
    (helper.is_file() && dll.is_file()).then_some(RenderPaths { helper, dll })
}

fn safe_relative(value: &str) -> Option<PathBuf> {
    // Treat backslashes as separators on every build host so tests exercise Windows rules.
    if value.contains(':') { return None; }
    let p = PathBuf::from(value.replace('\\', "/"));
    if p.as_os_str().is_empty() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(p)
}

fn from_manifest(root: &Path, bytes: &[u8]) -> Option<RenderPaths> {
    let m: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if m.get("schema").and_then(|v| v.as_u64()) != Some(1) { return None; }
    let system = m.get("system_architecture")?.as_str()?;
    if !matches!(system, "x64" | "arm64") || m.get("version")?.as_str()?.is_empty() { return None; }
    if (std::env::consts::ARCH == "aarch64" && system != "arm64")
        || (std::env::consts::ARCH == "x86_64" && system != "x64") { return None; }
    let entries = m.get("files")?.as_array()?;
    let get = |role: &str| -> Option<PathBuf> {
        let mut found = entries.iter().filter(|v| v.get("role").and_then(|v| v.as_str()) == Some(role));
        let entry = found.next()?;
        if found.next().is_some() { return None; }
        let machine = entry.get("machine")?.as_str()?;
        let compatible = match (std::env::consts::ARCH, role) {
            ("aarch64", "render-helper") => machine == "arm64",
            ("aarch64", _) => matches!(machine, "arm64" | "arm64x"),
            ("x86_64", "render-helper") => machine == "x64",
            ("x86_64", _) => matches!(machine, "x64" | "arm64x"),
            ("x86", _) => machine == "x86",
            _ => false,
        };
        if !compatible { return None; }
        let p = root.join(safe_relative(entry.get("path")?.as_str()?)?);
        // Reparse points may redirect a relative path outside the verified package.
        let canonical = p.canonicalize().ok()?;
        if !canonical.starts_with(root.canonicalize().ok()?) { return None; }
        let expected = entry.get("sha256")?.as_str()?;
        let bytes = std::fs::read(&p).ok()?;
        if format!("{:x}", Sha256::digest(&bytes)) != expected.to_ascii_lowercase() { return None; }
        Some(p)
    };
    existing(get("render-helper")?, get("tsf-native")?)
}

fn legacy(root: &Path, system_root: Option<&Path>) -> Option<RenderPaths> {
    let mut roots = vec![root.to_path_buf()];
    if let Some(windows) = system_root { roots.push(windows.join("SystemIME").join("HuFu")); }
    for dir in roots {
        for name in ["hufu_tsf_render.dll", "hufu_tsf.dll"] {
            if let Some(p) = existing(dir.join("hufu-render-helper.exe"), dir.join(name)) { return Some(p); }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relative_paths_reject_windows_escapes() {
        for p in ["../old.dll", "..\\old.dll", "C:\\old.dll", "\\\\host\\share", "/old", ""] {
            assert!(safe_relative(p).is_none(), "{p}");
        }
        assert_eq!(safe_relative("tsf\\arm64x\\hufu_tsf.dll"), Some(PathBuf::from("tsf/arm64x/hufu_tsf.dll")));
    }
    #[test]
    fn manifest_role_hash_and_generation_are_authoritative() {
        let root = std::env::temp_dir().join(format!("hufu-render-paths-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(root.join("tsf/generation-a")).unwrap();
        let files = [("render-helper", "helper.exe", b"helper".as_slice()), ("tsf-native", "tsf/generation-a/native.dll", b"dll".as_slice())];
        let entries: Vec<_> = files.iter().map(|(role,path,bytes)| {
            std::fs::write(root.join(path), bytes).unwrap();
            serde_json::json!({"role":role,"path":path,"sha256":format!("{:x}",Sha256::digest(bytes)),"machine":if cfg!(target_arch="aarch64") {"arm64"} else {"x64"}})
        }).collect();
        let mut manifest = serde_json::json!({"schema":1,"system_architecture":if cfg!(target_arch="aarch64") {"arm64"} else {"x64"},"version":"test","files":entries});
        let encoded = serde_json::to_vec(&manifest).unwrap();
        assert_eq!(from_manifest(&root, &encoded).unwrap().dll, root.join("tsf/generation-a/native.dll"));
        for (field, value) in [("schema", serde_json::json!(2)), ("system_architecture", serde_json::json!("invalid")), ("version", serde_json::json!(""))] {
            let mut invalid = manifest.clone();
            invalid[field] = value;
            assert!(from_manifest(&root, &serde_json::to_vec(&invalid).unwrap()).is_none());
        }
        let mut duplicate = manifest.clone();
        duplicate["files"].as_array_mut().unwrap().push(manifest["files"][0].clone());
        assert!(from_manifest(&root, &serde_json::to_vec(&duplicate).unwrap()).is_none());
        std::fs::write(root.join("tsf/generation-a/native.dll"), b"changed").unwrap();
        assert!(from_manifest(&root, &encoded).is_none());
        std::fs::write(root.join("tsf/generation-a/native.dll"), b"dll").unwrap();
        manifest["files"][1]["path"] = serde_json::json!("../old.dll");
        assert!(from_manifest(&root, &serde_json::to_vec(&manifest).unwrap()).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(windows)]
fn registered_dll() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    type HKey = *mut core::ffi::c_void;
    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(key: HKey, subkey: *const u16, options: u32, access: u32, result: *mut HKey) -> i32;
        fn RegQueryValueExW(key: HKey, name: *const u16, reserved: *mut u32, kind: *mut u32, data: *mut u8, bytes: *mut u32) -> i32;
        fn RegCloseKey(key: HKey) -> i32;
    }
    let sub: Vec<u16> = "Software\\Classes\\CLSID\\{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}\\InprocServer32".encode_utf16().chain(Some(0)).collect();
    // Native 64-bit view; both ARM64 and emulated x64 use the ARM64X registration.
    for hive in [0x80000001u32, 0x80000002] {
        unsafe {
            let mut key = std::ptr::null_mut();
            if RegOpenKeyExW(hive as i32 as isize as HKey, sub.as_ptr(), 0, 0x20019 | 0x100, &mut key) != 0 { continue; }
            let mut kind = 0; let mut bytes = 0;
            let rc = RegQueryValueExW(key, std::ptr::null(), std::ptr::null_mut(), &mut kind, std::ptr::null_mut(), &mut bytes);
            if rc != 0 || kind != 1 || bytes == 0 || bytes > 65536 { RegCloseKey(key); continue; }
            let mut value = vec![0u16; (bytes as usize + 1) / 2];
            let rc = RegQueryValueExW(key, std::ptr::null(), std::ptr::null_mut(), &mut kind, value.as_mut_ptr().cast(), &mut bytes);
            RegCloseKey(key);
            if rc == 0 && kind == 1 {
                let end = value.iter().position(|&v| v == 0).unwrap_or(value.len());
                let path = PathBuf::from(std::ffi::OsString::from_wide(&value[..end]));
                if path.is_file() { return Some(path); }
            }
        }
    }
    None
}
