//! Snapshots are restricted to this product's keys and explicit registry views.
use crate::common::{ASM_KBD_CAT, CLSID, TFCAT_KBD, TIP_CATS};
use serde::{Deserialize, Serialize};
use std::io;
use winreg::{enums::*, RegKey, RegValue};
#[derive(Serialize, Deserialize)]
struct Value {
    name: String,
    kind: u32,
    bytes: Vec<u8>,
}
#[derive(Serialize, Deserialize)]
struct Tree {
    values: Vec<Value>,
    children: Vec<(String, Tree)>,
}
#[derive(Serialize, Deserialize)]
struct Saved {
    path: String,
    view: u32,
    tree: Option<Tree>,
}
#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    machine: bool,
    keys: Vec<Saved>,
}
#[cfg(test)]
thread_local! { static TEST_ROOT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) }; }

pub fn hive(machine: bool) -> RegKey {
    #[cfg(test)]
    if let Some(root) = TEST_ROOT.with(|r| r.borrow().clone()) {
        return RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(format!(
                "{}\\{}",
                root,
                if machine { "Machine" } else { "User" }
            ))
            .unwrap()
            .0;
    }
    RegKey::predef(if machine {
        HKEY_LOCAL_MACHINE
    } else {
        HKEY_CURRENT_USER
    })
}
fn open_optional(key: &RegKey, path: &str, flags: u32) -> io::Result<Option<RegKey>> {
    match key.open_subkey_with_flags(path, flags) {
        Ok(k) => Ok(Some(k)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
pub fn com_path() -> String {
    format!(r"SOFTWARE\Classes\CLSID\{}", CLSID)
}
pub fn tip_path() -> String {
    format!(r"SOFTWARE\Microsoft\CTF\TIP\{}", CLSID)
}
fn owned_paths(machine: bool) -> Vec<String> {
    let mut paths = vec![com_path(), tip_path()];
    if !machine {
        paths.push(r"Software\HuFu".into());
    }
    paths.push(format!(r"SOFTWARE\Microsoft\CTF\Category\Item\{}", CLSID));
    for cat in TIP_CATS.iter().copied().chain([TFCAT_KBD, ASM_KBD_CAT]) {
        let p = format!(
            r"SOFTWARE\Microsoft\CTF\Category\Category\{}\{}",
            cat, CLSID
        );
        if !paths.contains(&p) {
            paths.push(p);
        }
    }
    paths
}
fn read_tree(key: &RegKey) -> io::Result<Tree> {
    let values = key
        .enum_values()
        .map(|r| {
            r.map(|(name, v)| Value {
                name,
                kind: v.vtype as u32,
                bytes: v.bytes,
            })
        })
        .collect::<io::Result<_>>()?;
    let mut children = vec![];
    for name in key.enum_keys() {
        let name = name?;
        children.push((name.clone(), read_tree(&key.open_subkey(&name)?)?));
    }
    Ok(Tree { values, children })
}
fn write_tree(key: &RegKey, tree: &Tree) -> io::Result<()> {
    for v in &tree.values {
        let kind = match v.kind {
            0 => REG_NONE,
            1 => REG_SZ,
            2 => REG_EXPAND_SZ,
            3 => REG_BINARY,
            4 => REG_DWORD,
            5 => REG_DWORD_BIG_ENDIAN,
            6 => REG_LINK,
            7 => REG_MULTI_SZ,
            8 => REG_RESOURCE_LIST,
            9 => REG_FULL_RESOURCE_DESCRIPTOR,
            10 => REG_RESOURCE_REQUIREMENTS_LIST,
            11 => REG_QWORD,
            _ => return Err(io::Error::other("invalid registry type")),
        };
        key.set_raw_value(
            &v.name,
            &RegValue {
                bytes: v.bytes.clone(),
                vtype: kind,
            },
        )?;
    }
    for (name, child) in &tree.children {
        write_tree(&key.create_subkey(name)?.0, child)?;
    }
    Ok(())
}
pub fn remove(machine: bool, path: &str, view: u32) -> io::Result<()> {
    // Open the view at its parent, so recursive deletion never defaults to process bitness.
    let (parent, leaf) = path
        .rsplit_once('\\')
        .ok_or_else(|| io::Error::other("root deletion forbidden"))?;
    match hive(machine).open_subkey_with_flags(parent, KEY_READ | KEY_WRITE | view) {
        Ok(p) => match p.delete_subkey_all(leaf) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
impl Snapshot {
    pub fn is_machine(&self) -> bool {
        self.machine
    }
    pub fn capture(machine: bool) -> io::Result<Self> {
        let mut keys = vec![];
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            for path in owned_paths(machine) {
                let tree = match hive(machine).open_subkey_with_flags(&path, KEY_READ | view) {
                    Ok(k) => Some(read_tree(&k)?),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e),
                };
                keys.push(Saved { path, view, tree });
            }
        }
        Ok(Self { machine, keys })
    }
    pub fn restore(&self) -> io::Result<()> {
        // A persisted journal is never allowed to name keys outside our ownership set.
        let allowed = owned_paths(self.machine);
        for k in &self.keys {
            if !allowed.contains(&k.path) || ![KEY_WOW64_64KEY, KEY_WOW64_32KEY].contains(&k.view) {
                return Err(io::Error::other("invalid rollback scope"));
            }
        }
        for k in &self.keys {
            remove(self.machine, &k.path, k.view)?;
            if let Some(t) = &k.tree {
                write_tree(
                    &hive(self.machine)
                        .create_subkey_with_flags(&k.path, KEY_READ | KEY_WRITE | k.view)?
                        .0,
                    t,
                )?;
            }
        }
        Ok(())
    }
    pub fn remove_owned(machine: bool) -> io::Result<()> {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            for p in owned_paths(machine) {
                remove(machine, &p, view)?;
            }
        }
        Ok(())
    }
}
pub fn set_com(machine: bool, view: u32, dll: &std::path::Path) -> io::Result<()> {
    let (key, _) =
        hive(machine).create_subkey_with_flags(com_path(), KEY_READ | KEY_WRITE | view)?;
    key.set_value("", &"HuFu TSF Service")?;
    let (ips, _) = key.create_subkey("InprocServer32")?;
    ips.set_value("", &dll.to_string_lossy().as_ref())?;
    ips.set_value("ThreadingModel", &"Apartment")?;
    let actual: String = ips.get_value("")?;
    if actual != dll.to_string_lossy() {
        return Err(io::Error::other("COM readback mismatch"));
    }
    Ok(())
}
pub fn com_dll(machine: bool, view: u32) -> io::Result<std::path::PathBuf> {
    let k = hive(machine)
        .open_subkey_with_flags(format!(r"{}\InprocServer32", com_path()), KEY_READ | view)?;
    Ok(std::path::PathBuf::from(k.get_value::<String, _>("")?))
}

const USER_LIST: &str = r"Control Panel\International\User Profile";
fn assembly() -> String {
    format!(
        r"SOFTWARE\Microsoft\CTF\SortOrder\AssemblyItem\0x00000804\{}",
        ASM_KBD_CAT
    )
}
/// Preserve only HuFu entries, never an entire language list or another IME's slot.
pub struct UserList {
    values: Vec<(String, RegValue)>,
    slots: Vec<(String, Tree)>,
}
impl UserList {
    pub fn capture() -> io::Result<Self> {
        let hk = hive(false);
        let mut values = vec![];
        if let Some(k) = open_optional(&hk, USER_LIST, KEY_READ | KEY_WOW64_64KEY)? {
            for lang in k.enum_keys() {
                let lang = lang?;
                let child = k.open_subkey(&lang)?;
                match child.get_raw_value(crate::common::tip_str()) {
                    Ok(v) => values.push((lang, v)),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
        let mut slots = vec![];
        if let Some(k) = open_optional(&hk, &assembly(), KEY_READ | KEY_WOW64_64KEY)? {
            for name in k.enum_keys() {
                let name = name?;
                let child = k.open_subkey(&name)?;
                if child
                    .get_value::<String, _>("CLSID")
                    .is_ok_and(|v| v.eq_ignore_ascii_case(CLSID))
                {
                    slots.push((name, read_tree(&child)?));
                }
            }
        }
        Ok(Self { values, slots })
    }
    pub fn clear() -> io::Result<()> {
        let state = Self::capture()?;
        for (lang, _) in state.values {
            hive(false)
                .open_subkey_with_flags(
                    format!(r"{}\{}", USER_LIST, lang),
                    KEY_WRITE | KEY_WOW64_64KEY,
                )?
                .delete_value(crate::common::tip_str())?;
        }
        for (name, _) in state.slots {
            remove(false, &format!(r"{}\{}", assembly(), name), KEY_WOW64_64KEY)?;
        }
        Ok(())
    }
    pub fn restore(&self) -> io::Result<()> {
        Self::clear()?;
        for (lang, v) in &self.values {
            hive(false)
                .create_subkey_with_flags(
                    format!(r"{}\{}", USER_LIST, lang),
                    KEY_WRITE | KEY_WOW64_64KEY,
                )?
                .0
                .set_raw_value(crate::common::tip_str(), v)?;
        }
        for (name, tree) in &self.slots {
            // A newly occupied slot belonging to another IME must not be overwritten.
            let path = format!(r"{}\{}", assembly(), name);
            if let Ok(k) = hive(false).open_subkey_with_flags(&path, KEY_READ | KEY_WOW64_64KEY) {
                if !k
                    .get_value::<String, _>("CLSID")
                    .is_ok_and(|v| v.eq_ignore_ascii_case(CLSID))
                {
                    return Err(io::Error::other(
                        "rollback slot occupied by another input method",
                    ));
                }
            }
            write_tree(
                &hive(false)
                    .create_subkey_with_flags(path, KEY_READ | KEY_WRITE | KEY_WOW64_64KEY)?
                    .0,
                tree,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rollback_restores_owned_trees_without_rewriting_other_input_methods() {
        let root = format!(
            r"Software\HuFuSetupEngineIsolatedTests\{}",
            std::process::id()
        );
        TEST_ROOT.with(|r| *r.borrow_mut() = Some(root.clone()));
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                TEST_ROOT.with(|r| *r.borrow_mut() = None);
                let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root);
        let hk = hive(true);
        let ours = format!(r"{}\InprocServer32", com_path());
        hk.create_subkey(&ours)
            .unwrap()
            .0
            .set_value("", &"old-generation.dll")
            .unwrap();
        let other = r"SOFTWARE\Classes\CLSID\{11111111-2222-3333-4444-555555555555}";
        hk.create_subkey(other)
            .unwrap()
            .0
            .set_value("sentinel", &"unchanged")
            .unwrap();
        let snap = Snapshot::capture(true).unwrap();
        set_com(
            true,
            KEY_WOW64_64KEY,
            std::path::Path::new("new-generation.dll"),
        )
        .unwrap();
        hk.create_subkey(format!(r"{}\CreatedDuringFailure", tip_path()))
            .unwrap();
        snap.restore().unwrap();
        assert_eq!(
            hk.open_subkey(&ours)
                .unwrap()
                .get_value::<String, _>("")
                .unwrap(),
            "old-generation.dll"
        );
        assert!(hk.open_subkey(tip_path()).is_err());
        assert_eq!(
            hk.open_subkey(other)
                .unwrap()
                .get_value::<String, _>("sentinel")
                .unwrap(),
            "unchanged"
        );
        let mut malicious = snap;
        malicious.keys[0].path = other.into();
        assert!(malicious.restore().is_err());
        assert_eq!(
            hk.open_subkey(other)
                .unwrap()
                .get_value::<String, _>("sentinel")
                .unwrap(),
            "unchanged"
        );
    }
    #[test]
    fn user_list_rollback_keeps_concurrent_other_ime_changes() {
        let root = format!(
            r"Software\HuFuSetupEngineIsolatedTests\{}-user",
            std::process::id()
        );
        TEST_ROOT.with(|r| *r.borrow_mut() = Some(root.clone()));
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                TEST_ROOT.with(|r| *r.borrow_mut() = None);
                let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root);
        let hk = hive(false);
        let lang = hk.create_subkey(format!(r"{}\zh-CN", USER_LIST)).unwrap().0;
        lang.set_value(crate::common::tip_str(), &3u32).unwrap();
        lang.set_value("OtherIME", &1u32).unwrap();
        let slot = format!(r"{}\00000003", assembly());
        hk.create_subkey(&slot)
            .unwrap()
            .0
            .set_value("CLSID", &CLSID)
            .unwrap();
        let before = UserList::capture().unwrap();
        UserList::clear().unwrap();
        lang.set_value("OtherIME", &2u32).unwrap(); // Independent change must survive rollback.
        hk.create_subkey(format!(r"{}\00000005", assembly()))
            .unwrap()
            .0
            .set_value("CLSID", &CLSID)
            .unwrap();
        before.restore().unwrap();
        assert_eq!(lang.get_value::<u32, _>("OtherIME").unwrap(), 2);
        assert_eq!(
            lang.get_value::<u32, _>(crate::common::tip_str()).unwrap(),
            3
        );
        assert!(hk.open_subkey(format!(r"{}\00000005", assembly())).is_err());
        hk.open_subkey_with_flags(&slot, KEY_WRITE)
            .unwrap()
            .set_value("CLSID", &"OtherIME")
            .unwrap();
        assert!(before.restore().is_err());
        assert_eq!(
            hk.open_subkey(&slot)
                .unwrap()
                .get_value::<String, _>("CLSID")
                .unwrap(),
            "OtherIME"
        );
    }
}

/// Exact-value ownership for uninstall conveniences; never snapshot a whole Run key.
pub struct UserValues(Vec<(String, String, Option<RegValue>)>);
impl UserValues {
    pub fn capture() -> io::Result<Self> {
        let mut items = vec![];
        for (path, name) in [
            (r"Software\Microsoft\Windows\CurrentVersion\Run", "HuFu"),
            (
                r"Software\Microsoft\Windows\CurrentVersion\RunOnce",
                "HuFuPostBoot",
            ),
            (USER_LIST, "InputMethodOverride"),
        ] {
            let value = match hive(false).open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY) {
                Ok(k) => match k.get_raw_value(name) {
                    Ok(v) => Some(v),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e),
                },
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            if name == "InputMethodOverride" {
                let own = value.as_ref().is_some_and(|v| {
                    String::from_utf16_lossy(
                        &v.bytes
                            .chunks_exact(2)
                            .map(|x| u16::from_le_bytes([x[0], x[1]]))
                            .collect::<Vec<_>>(),
                    )
                    .contains(crate::common::PROFILE)
                });
                if !own {
                    continue;
                }
            }
            items.push((path.into(), name.into(), value));
        }
        Ok(Self(items))
    }
    pub fn clear(&self) -> io::Result<()> {
        for (path, name, _) in &self.0 {
            match hive(false).open_subkey_with_flags(path, KEY_WRITE | KEY_WOW64_64KEY) {
                Ok(k) => match k.delete_value(name) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                },
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    pub fn restore(&self) -> io::Result<()> {
        self.clear()?;
        for (path, name, value) in &self.0 {
            if let Some(v) = value {
                hive(false)
                    .create_subkey_with_flags(path, KEY_WRITE | KEY_WOW64_64KEY)?
                    .0
                    .set_raw_value(name, v)?;
            }
        }
        Ok(())
    }
}

const INNO_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{2F8A5D41-7B63-4C3E-9E15-A8D4B0C6F293}_is1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InnoSnapshot {
    schema: u32,
    sid: Vec<u8>,
    views: Vec<(u32, Option<Tree>)>,
}
fn current_sid() -> io::Result<Vec<u8>> {
    use windows::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::{GetLengthSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let mut size = 0;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
        if size == 0 {
            let _ = CloseHandle(token);
            return Err(io::Error::last_os_error());
        }
        // Token structures contain pointers; give the buffer pointer alignment.
        let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        let result = GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size,
            &mut size,
        );
        let _ = CloseHandle(token);
        result.map_err(|e| io::Error::other(e.to_string()))?;
        let user = &*(buffer.as_ptr().cast::<TOKEN_USER>());
        let length = GetLengthSid(user.User.Sid) as usize;
        if !(8..=68).contains(&length) {
            return Err(io::Error::other("invalid token SID"));
        }
        Ok(std::slice::from_raw_parts(user.User.Sid.0.cast::<u8>(), length).to_vec())
    }
}
impl InnoSnapshot {
    fn capture(sid: Vec<u8>) -> io::Result<Self> {
        let mut views = vec![];
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            let tree = open_optional(&hive(false), INNO_KEY, KEY_READ | view)?
                .map(|k| read_tree(&k))
                .transpose()?;
            views.push((view, tree));
        }
        Ok(Self {
            schema: 1,
            sid,
            views,
        })
    }
    fn restore(&self, sid: &[u8]) -> io::Result<()> {
        if self.schema != 1
            || self.sid != sid
            || self.views.len() != 2
            || self.views[0].0 != KEY_WOW64_64KEY
            || self.views[1].0 != KEY_WOW64_32KEY
        {
            return Err(io::Error::other(
                "snapshot schema, registry views or caller SID mismatch",
            ));
        }
        fn validate(t: &Tree, depth: usize) -> io::Result<()> {
            if depth > 64
                || t.values
                    .iter()
                    .any(|v| v.name.contains('\0') || v.kind > 11)
            {
                return Err(io::Error::other("invalid snapshot tree"));
            }
            for (name, tree) in &t.children {
                if name.is_empty() || name.contains(['\\', '\0']) {
                    return Err(io::Error::other("snapshot child is not a single key name"));
                }
                validate(tree, depth + 1)?;
            }
            Ok(())
        }
        for (_, tree) in &self.views {
            if let Some(t) = tree {
                validate(t, 0)?;
            }
        }
        // Snapshot the current own key before compensation, so an ordinary I/O failure
        // does not strand a half-restored Inno key. This never includes the parent.
        let current = Self::capture(sid.to_vec())?;
        let apply = |snapshot: &Self| -> io::Result<()> {
            for (view, tree) in &snapshot.views {
                remove(false, INNO_KEY, *view)?;
                if let Some(t) = tree {
                    write_tree(
                        &hive(false)
                            .create_subkey_with_flags(INNO_KEY, KEY_READ | KEY_WRITE | view)?
                            .0,
                        t,
                    )?;
                }
            }
            Ok(())
        };
        if let Err(e) = apply(self) {
            return match apply(&current) { Ok(()) => Err(e), Err(recovery) => Err(io::Error::other(format!("Inno restore failed: {e}; previous Inno state recovery also failed: {recovery}"))) };
        }
        Ok(())
    }
}
pub fn inno_snapshot(path: &std::path::Path, restore: bool) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::other("--snapshot must be an absolute JSON path"));
    }
    let sid = current_sid()?;
    if restore {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > 8 * 1024 * 1024
        {
            return Err(io::Error::other("invalid snapshot file"));
        }
        let snapshot: InnoSnapshot =
            serde_json::from_slice(&std::fs::read(path)?).map_err(io::Error::other)?;
        snapshot.restore(&sid)
    } else {
        use std::io::Write;
        let snapshot = InnoSnapshot::capture(sid)?;
        let bytes = serde_json::to_vec(&snapshot).map_err(io::Error::other)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(&bytes)?;
        file.sync_all()
    }
}

#[cfg(test)]
mod inno_tests {
    use super::*;
    #[test]
    fn inno_snapshot_preserves_raw_types_absence_sid_and_other_product() {
        let root = format!(
            r"Software\HuFuSetupEngineIsolatedTests\{}-inno",
            std::process::id()
        );
        TEST_ROOT.with(|r| *r.borrow_mut() = Some(root.clone()));
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                TEST_ROOT.with(|r| *r.borrow_mut() = None);
                let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root);
        let sid = current_sid().unwrap();
        let missing = InnoSnapshot::capture(sid.clone()).unwrap();
        let hk = hive(false);
        let parent = INNO_KEY.rsplit_once('\\').unwrap().0;
        let neighbor = format!(r"{}\OtherProduct_is1", parent);
        hk.create_subkey(&neighbor)
            .unwrap()
            .0
            .set_value("sentinel", &"keep")
            .unwrap();
        let key = hk.create_subkey(INNO_KEY).unwrap().0;
        for (name, value) in [
            (
                "binary",
                RegValue {
                    vtype: REG_BINARY,
                    bytes: vec![0, 255, 17, 0],
                },
            ),
            (
                "expand",
                RegValue {
                    vtype: REG_EXPAND_SZ,
                    bytes: "%APPDATA%\\HuFu\0"
                        .encode_utf16()
                        .flat_map(u16::to_le_bytes)
                        .collect(),
                },
            ),
            (
                "multi",
                RegValue {
                    vtype: REG_MULTI_SZ,
                    bytes: "alpha\0beta\0\0"
                        .encode_utf16()
                        .flat_map(u16::to_le_bytes)
                        .collect(),
                },
            ),
            (
                "dword",
                RegValue {
                    vtype: REG_DWORD,
                    bytes: 0xfedcba98u32.to_le_bytes().to_vec(),
                },
            ),
            (
                "qword",
                RegValue {
                    vtype: REG_QWORD,
                    bytes: 0xfedcba9876543210u64.to_le_bytes().to_vec(),
                },
            ),
            (
                "none",
                RegValue {
                    vtype: REG_NONE,
                    bytes: vec![0, 8, 0],
                },
            ),
        ] {
            key.set_raw_value(name, &value).unwrap();
        }
        key.create_subkey("Nested")
            .unwrap()
            .0
            .set_value("name", &"虎符")
            .unwrap();
        let original = key.enum_values().collect::<io::Result<Vec<_>>>().unwrap();
        drop(key);
        let encoded = serde_json::to_vec(&InnoSnapshot::capture(sid.clone()).unwrap()).unwrap();
        let snapshot: InnoSnapshot = serde_json::from_slice(&encoded).unwrap();
        remove(false, INNO_KEY, KEY_WOW64_64KEY).unwrap();
        hk.create_subkey(INNO_KEY)
            .unwrap()
            .0
            .set_value("unexpected", &42u32)
            .unwrap();
        assert!(snapshot.restore(&[99]).is_err());
        assert_eq!(
            hk.open_subkey(INNO_KEY)
                .unwrap()
                .get_value::<u32, _>("unexpected")
                .unwrap(),
            42
        );
        snapshot.restore(&sid).unwrap();
        let key = hk.open_subkey(INNO_KEY).unwrap();
        for (name, old) in original {
            let new = key.get_raw_value(&name).unwrap();
            assert_eq!(new.bytes, old.bytes);
            assert_eq!(new.vtype, old.vtype);
        }
        assert!(key.get_raw_value("unexpected").is_err());
        assert_eq!(
            key.open_subkey("Nested")
                .unwrap()
                .get_value::<String, _>("name")
                .unwrap(),
            "虎符"
        );
        drop(key);
        missing.restore(&sid).unwrap();
        assert!(hk.open_subkey(INNO_KEY).is_err());
        assert_eq!(
            hk.open_subkey(neighbor)
                .unwrap()
                .get_value::<String, _>("sentinel")
                .unwrap(),
            "keep"
        );
    }
}
