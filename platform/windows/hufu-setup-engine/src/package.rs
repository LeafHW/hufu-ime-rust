//! Strict flat payload validation, shared by preflight and generation deployment.
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::HashSet, fs, path::Path};

pub const MANIFEST: &str = "package-architecture.json";
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub system_architecture: String,
    pub version: String,
    pub files: Vec<Entry>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: String,
    pub machine: String,
    pub sha256: String,
    pub role: String,
}
pub struct Package {
    pub manifest: Manifest,
    pub generation: String,
    pub bytes: Vec<u8>,
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn pe_machine(bytes: &[u8]) -> Result<u16, String> {
    if bytes.get(..2) != Some(b"MZ") {
        return Err("missing MZ".into());
    }
    let off = bytes.get(60..64).ok_or("truncated DOS header")?;
    let n = u32::from_le_bytes(off.try_into().unwrap()) as usize;
    if bytes.get(n..n.saturating_add(4)) != Some(b"PE\0\0") {
        return Err("missing PE".into());
    }
    Ok(u16::from_le_bytes(
        bytes
            .get(n + 4..n + 6)
            .ok_or("truncated PE")?
            .try_into()
            .unwrap(),
    ))
}

fn validate_path(path: &str, subdirectories: bool) -> Result<(), String> {
    if !subdirectories && path.contains(['/', '\\']) {
        return Err("system/program executable payload must be flat".into());
    }
    for part in path.split(['/', '\\']) {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part
                .chars()
                .any(|c| c.is_control() || ":*?\"<>|~".contains(c))
        {
            return Err(format!("unsafe path: {path}"));
        }
        let stem = part.split('.').next().unwrap().to_ascii_uppercase();
        if [
            "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM¹", "COM²", "COM³", "LPT¹",
            "LPT²", "LPT³", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
            "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ]
        .contains(&stem.as_str())
        {
            return Err("reserved filename".into());
        }
    }
    Ok(())
}
fn reject_link(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Err("payload symlink forbidden".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err("payload reparse point forbidden".into());
        }
    }
    Ok(())
}
impl Package {
    pub fn load(root: &Path, architecture: &str) -> Result<Self, String> {
        Self::load_mode(root, architecture, false)
    }
    pub fn load_system(root: &Path, architecture: &str) -> Result<Self, String> {
        Self::load_mode(root, architecture, true)
    }
    fn load_mode(root: &Path, architecture: &str, system: bool) -> Result<Self, String> {
        reject_link(root)?;
        reject_link(&root.join(MANIFEST))?;
        let bytes = fs::read(root.join(MANIFEST)).map_err(|e| format!("manifest: {e}"))?;
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if manifest.schema != 1 || manifest.system_architecture != architecture {
            return Err("unsupported schema or OS architecture mismatch".into());
        }
        if manifest.version.is_empty()
            || !manifest
                .version
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
        {
            return Err("invalid version".into());
        }
        let mut paths = HashSet::new();
        for f in &manifest.files {
            validate_path(&f.path, f.role == "program" && f.machine == "data")?;
            if !paths.insert(f.path.replace('\\', "/").to_lowercase()) {
                return Err(format!("duplicate path: {}", f.path));
            }
            if f.machine == "data"
                && ["exe", "dll"].contains(
                    &f.path
                        .rsplit('.')
                        .next()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .as_str(),
                )
            {
                return Err("PE payload cannot be labelled data".into());
            }
            let expected = match f.role.as_str() {
                "tsf-native" => {
                    if architecture == "arm64" {
                        "arm64x"
                    } else {
                        "x64"
                    }
                }
                "registration-native" | "render-helper" => architecture,
                "tsf-x86" | "registration-x86" => "x86",
                "probe-x64" => "x64",
                "tsf-resource" => "data",
                "tsf-native-dependency" if f.machine == "arm64" || f.machine == "x64" => &f.machine,
                "program"
                    if f.machine == architecture
                        || f.machine == "data"
                        || (architecture == "arm64"
                            && f.machine == "x64"
                            && [
                                "hufu-settings.exe",
                                "webview2loader.dll",
                                "hufu-trainer.exe",
                            ]
                            .contains(&f.path.to_ascii_lowercase().as_str())) =>
                {
                    &f.machine
                }
                _ => return Err(format!("invalid role/machine: {}", f.path)),
            };
            if f.machine != expected {
                return Err(format!("wrong machine for {}", f.path));
            }
            if !system || f.role != "program" {
                Self::validate_file(root, f)?;
            }
        }
        for role in [
            "tsf-native",
            "tsf-x86",
            "registration-native",
            "registration-x86",
        ] {
            if manifest.files.iter().filter(|f| f.role == role).count() != 1 {
                return Err(format!("expected exactly one {role}"));
            }
        }
        for (required, role) in [
            ("hufu-server.exe", "program"),
            ("hufu-setup-engine.exe", "program"),
            ("hufu-settings.exe", "program"),
            ("WebView2Loader.dll", "program"),
            ("hufu-trainer.exe", "program"),
            ("TigerClaw.Sentence.Native.dll", "program"),
            ("hufu_tsf.dll", "tsf-native"),
            ("hufu_tsf32.dll", "tsf-x86"),
            ("hufu-tsf-smoke.exe", "registration-native"),
            ("hufu-tsf-smoke32.exe", "registration-x86"),
            ("hufu-render-helper.exe", "render-helper"),
            ("图标.ico", "tsf-resource"),
        ] {
            if !manifest
                .files
                .iter()
                .any(|f| f.path.eq_ignore_ascii_case(required) && f.role == role)
            {
                return Err(format!("missing required component: {required} ({role})"));
            }
        }
        for role in ["render-helper", "tsf-resource"] {
            if manifest.files.iter().filter(|f| f.role == role).count() != 1 {
                return Err(format!("expected exactly one {role}"));
            }
        }
        if architecture == "arm64" {
            if manifest
                .files
                .iter()
                .filter(|f| f.role == "probe-x64")
                .count()
                != 1
            {
                return Err("ARM64 requires exactly one x64 probe".into());
            }
            if !manifest.files.iter().any(|f| {
                f.role == "probe-x64" && f.path.eq_ignore_ascii_case("hufu-tsf-smoke-x64.exe")
            }) {
                return Err("missing named x64 facade probe".into());
            }
            for prefix in ["hufu_tsf_arm64_", "hufu_tsf_x64_"] {
                if manifest
                    .files
                    .iter()
                    .filter(|f| {
                        f.role == "tsf-native-dependency"
                            && f.path.to_lowercase().starts_with(prefix)
                            && f.path.to_lowercase().ends_with(".dll")
                    })
                    .count()
                    != 1
                {
                    return Err(format!("expected one implementation: {prefix}"));
                }
            }
            for machine in ["arm64", "x64"] {
                if !manifest
                    .files
                    .iter()
                    .any(|f| f.role == "tsf-native-dependency" && f.machine == machine)
                {
                    return Err(format!("missing {machine} facade dependency"));
                }
            }
        }
        let generation = format!("{}-{}", manifest.version, &digest(&bytes)[..12]);
        Ok(Self {
            manifest,
            generation,
            bytes,
        })
    }
    pub fn entry(&self, role: &str) -> &Entry {
        self.manifest
            .files
            .iter()
            .find(|f| f.role == role)
            .expect("validated role")
    }
    pub fn validate_file(root: &Path, f: &Entry) -> Result<(), String> {
        let mut path = root.to_path_buf();
        reject_link(&path)?;
        for segment in f.path.split(['/', '\\']) {
            path.push(segment);
            reject_link(&path)?;
        }
        let meta = fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", f.path))?;
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err("payload must be regular files".into());
        }
        let data = fs::read(path).map_err(|e| e.to_string())?;
        if f.sha256.len() != 64 || digest(&data) != f.sha256.to_ascii_lowercase() {
            return Err(format!("SHA256 mismatch: {}", f.path));
        }
        if f.machine != "data" {
            let expected = match f.machine.as_str() {
                "x86" => 0x14c,
                "x64" => 0x8664,
                "arm64" | "arm64x" => 0xaa64,
                _ => return Err("unknown machine".into()),
            };
            if pe_machine(&data)? != expected {
                return Err(format!("PE machine mismatch: {}", f.path));
            }
            // ARM64X dual-view validity is additionally checked by the x64/native runtime probes.
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture {
        root: std::path::PathBuf,
        manifest: serde_json::Value,
    }
    impl Fixture {
        fn new(arch: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "hufu-package-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir(&root).unwrap();
            let mut files = vec![];
            let native = if arch == "arm64" { "arm64x" } else { "x64" };
            let mut entries = vec![
                ("hufu_tsf.dll", native, "tsf-native"),
                ("hufu_tsf32.dll", "x86", "tsf-x86"),
                ("hufu-tsf-smoke.exe", arch, "registration-native"),
                ("hufu-tsf-smoke32.exe", "x86", "registration-x86"),
                ("hufu-server.exe", arch, "program"),
                ("hufu-setup-engine.exe", arch, "program"),
                ("hufu-settings.exe", arch, "program"),
                ("WebView2Loader.dll", arch, "program"),
                ("hufu-trainer.exe", arch, "program"),
                ("TigerClaw.Sentence.Native.dll", arch, "program"),
                ("hufu-render-helper.exe", arch, "render-helper"),
                ("图标.ico", "data", "tsf-resource"),
            ];
            if arch == "arm64" {
                entries.extend([
                    ("hufu_tsf_arm64_test.dll", "arm64", "tsf-native-dependency"),
                    ("hufu_tsf_x64_test.dll", "x64", "tsf-native-dependency"),
                    ("hufu-tsf-smoke-x64.exe", "x64", "probe-x64"),
                ]);
            }
            for (name, machine, role) in entries {
                let mut bytes = vec![0u8; 256];
                if machine != "data" {
                    bytes[..2].copy_from_slice(b"MZ");
                    bytes[60..64].copy_from_slice(&128u32.to_le_bytes());
                    bytes[128..132].copy_from_slice(b"PE\0\0");
                    let m: u16 = match machine {
                        "x86" => 0x14c,
                        "x64" => 0x8664,
                        _ => 0xaa64,
                    };
                    bytes[132..134].copy_from_slice(&m.to_le_bytes());
                }
                fs::write(root.join(name), &bytes).unwrap();
                files.push(
                    json!({"path":name,"machine":machine,"role":role,"sha256":digest(&bytes)}),
                );
            }
            let manifest =
                json!({"schema":1,"system_architecture":arch,"version":"1.2.3","files":files});
            let f = Self { root, manifest };
            f.save();
            f
        }
        fn save(&self) {
            fs::write(
                self.root.join(MANIFEST),
                serde_json::to_vec(&self.manifest).unwrap(),
            )
            .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn hashes_and_native_architecture_are_admission_gates() {
        let f = Fixture::new("x64");
        assert!(Package::load(&f.root, "x64").is_ok());
        assert!(Package::load(&f.root, "arm64").is_err());
        fs::write(f.root.join("hufu_tsf.dll"), b"tampered").unwrap();
        assert!(Package::load(&f.root, "x64")
            .unwrap_err_text()
            .contains("SHA256"));
    }
    #[test]
    fn arm_requires_both_impl_views_and_x64_probe() {
        let mut f = Fixture::new("arm64");
        assert!(Package::load(&f.root, "arm64").is_ok());
        f.manifest["files"]
            .as_array_mut()
            .unwrap()
            .retain(|x| x["role"] != "probe-x64");
        f.save();
        assert!(Package::load(&f.root, "arm64").is_err());
    }
    #[test]
    fn rejects_windows_aliases_traversal_and_duplicate_case() {
        for path in [
            "../escape.dll",
            "C:evil.dll",
            "CON.dll",
            "file.dll.",
            "HUFU_TSF.DLL",
            "dir\\evil.dll",
        ] {
            let mut f = Fixture::new("x64");
            f.manifest["files"][1]["path"] = path.into();
            f.save();
            assert!(Package::load(&f.root, "x64").is_err(), "{path}");
        }
    }
    #[test]
    fn generation_changes_with_manifest_and_system_requires_all_nonprogram_files() {
        let mut f = Fixture::new("x64");
        let old = Package::load(&f.root, "x64").unwrap().generation;
        f.manifest["version"] = "1.2.4".into();
        f.save();
        assert_ne!(old, Package::load(&f.root, "x64").unwrap().generation);
        fs::remove_file(f.root.join("hufu-server.exe")).unwrap();
        assert!(Package::load(&f.root, "x64").is_err());
        assert!(Package::load_system(&f.root, "x64").is_ok());
        fs::remove_file(f.root.join("图标.ico")).unwrap();
        assert!(Package::load_system(&f.root, "x64").is_err());
    }
    #[test]
    fn corrupted_pe_is_rejected_even_with_matching_hash() {
        let mut f = Fixture::new("x64");
        let bytes = b"not a PE";
        fs::write(f.root.join("hufu_tsf.dll"), bytes).unwrap();
        f.manifest["files"][0]["sha256"] = digest(bytes).into();
        f.save();
        assert!(Package::load(&f.root, "x64").is_err());
    }

    #[test]
    fn arm_emulation_is_limited_to_three_program_names() {
        let mut f = Fixture::new("arm64");
        let mut bytes = fs::read(f.root.join("hufu-settings.exe")).unwrap();
        bytes[132..134].copy_from_slice(&0x8664u16.to_le_bytes());
        fs::write(f.root.join("hufu-settings.exe"), &bytes).unwrap();
        for e in f.manifest["files"].as_array_mut().unwrap() {
            if e["path"] == "hufu-settings.exe" {
                e["machine"] = "x64".into();
                e["sha256"] = digest(&bytes).into();
            }
        }
        f.save();
        assert!(Package::load(&f.root, "arm64").is_ok());
        for e in f.manifest["files"].as_array_mut().unwrap() {
            if e["path"] == "hufu-server.exe" {
                e["machine"] = "x64".into();
            }
        }
        f.save();
        assert!(Package::load(&f.root, "arm64").is_err());
    }
    #[test]
    fn program_data_subdirectories_are_safe_and_hashed() {
        let mut f = Fixture::new("x64");
        fs::create_dir_all(f.root.join("licenses/trainer")).unwrap();
        fs::write(f.root.join("licenses/trainer/LICENSE"), b"license").unwrap();
        f.manifest["files"].as_array_mut().unwrap().push(json!({"path":"licenses/trainer/LICENSE","machine":"data","role":"program","sha256":digest(b"license")}));
        f.save();
        assert!(Package::load(&f.root, "x64").is_ok());
        fs::write(f.root.join("licenses/trainer/LICENSE"), b"corrupted").unwrap();
        assert!(Package::load(&f.root, "x64").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn package_data_cannot_escape_via_directory_symlink() {
        let mut f = Fixture::new("x64");
        std::os::unix::fs::symlink(&f.root, f.root.join("linked")).unwrap();
        f.manifest["files"].as_array_mut().unwrap().push(json!({"path":"linked/图标.ico","machine":"data","role":"program","sha256":digest(&fs::read(f.root.join("图标.ico")).unwrap())}));
        f.save();
        assert!(Package::load(&f.root, "x64").is_err());
    }
    trait ErrorText {
        fn unwrap_err_text(self) -> String;
    }
    impl ErrorText for Result<Package, String> {
        fn unwrap_err_text(self) -> String {
            match self {
                Err(e) => e,
                Ok(_) => panic!("expected rejection"),
            }
        }
    }
}
