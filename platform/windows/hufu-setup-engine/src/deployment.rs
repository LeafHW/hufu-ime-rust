//! Manifest installations use immutable SystemIME generations and scoped registry rollback.
use crate::{
    acl,
    common::{self, Out, CLSID, PROFILE, TIP_CATS},
    package::{Package, MANIFEST},
    preflight,
    procop::{self, SpawnResult},
    registry_tx::{self, Snapshot},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use winreg::enums::*;

type Result<T> = std::result::Result<T, String>;
fn ioe(e: std::io::Error) -> String {
    e.to_string()
}
fn system_root() -> Result<PathBuf> {
    // GetWindowsDirectoryW, not a caller-controlled environment variable.
    #[link(name = "kernel32")]
    extern "system" {
        fn GetWindowsDirectoryW(buffer: *mut u16, size: u32) -> u32;
    }
    let mut b = vec![0u16; 32768];
    let n = unsafe { GetWindowsDirectoryW(b.as_mut_ptr(), b.len() as u32) } as usize;
    if n == 0 || n >= b.len() {
        return Err("GetWindowsDirectoryW failed".into());
    }
    Ok(
        PathBuf::from(String::from_utf16(&b[..n]).map_err(|e| e.to_string())?)
            .join("SystemIME")
            .join("HuFu")
            .join("generations"),
    )
}

fn maintenance_lock() -> Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    let base = system_root()?;
    fs::create_dir_all(&base).map_err(ioe)?;
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(base.join("maintenance.lock"))
        .map_err(|e| format!("another maintenance operation is running: {e}"))
}

fn exec(tool: &Path, args: &str) -> Result<()> {
    run_helper(tool, args).map(|_| ())
}
fn run_helper(tool: &Path, args: &str) -> Result<String> {
    let (status, output) = procop::spawn_diagnostic(tool, args, 60_000);
    match status {
        SpawnResult::Exit(0) => Ok(output),
        r => Err(format!("{} {args}: {r:?}\n{}", tool.display(), output.trim())),
    }
}
fn exec_logged(out: &mut Out, tool: &Path, args: &str) -> Result<()> {
    let output = run_helper(tool, args)?;
    out.line(&format!("helper {} {args}: Exit(0)\n{}", tool.display(), output.trim()));
    Ok(())
}
// TSF can return a stale catalog even to a new process immediately after a
// successful RegisterProfile. As in WhiteTiger's maintenance policy, give
// fresh read-only observers a bounded window; never repeat the mutation.
fn poll_profile(
    mut inspect: impl FnMut() -> (SpawnResult, String),
    mut pause: impl FnMut(),
) -> Result<String> {
    let mut observations = String::new();
    for attempt in 1..=21 {
        let (status, output) = inspect();
        observations.push_str(&format!("profile inspection {attempt}/21: {status:?}\n{}\n", output.trim()));
        match status {
            SpawnResult::Exit(0) => return Ok(observations),
            SpawnResult::Exit(2) if attempt < 21 => pause(),
            _ => return Err(format!("TSF profile verification failed\n{observations}")),
        }
    }
    unreachable!()
}
fn await_profile(tool: &Path) -> Result<String> {
    poll_profile(
        || procop::spawn_diagnostic(tool, "inspect-profile", 5_000),
        || std::thread::sleep(std::time::Duration::from_millis(500)),
    )
}
fn native_tool(root: &Path, p: &Package) -> PathBuf {
    root.join(&p.entry("registration-native").path)
}
fn validate_files(root: &Path, p: &Package) -> Result<()> {
    let q = Package::load_system(root, &p.manifest.system_architecture)?;
    if q.generation != p.generation {
        return Err("generation manifest mismatch".into());
    }
    Ok(())
}
fn validate_root(root: &Path, p: &Package) -> Result<()> {
    validate_files(root, p)?;
    preflight::probe(root, p)
}
fn stage(src: &Path, p: &Package) -> Result<PathBuf> {
    let base = system_root()?;
    stage_at(src, p, &base)
}
fn stage_at(src: &Path, p: &Package, base: &Path) -> Result<PathBuf> {
    fs::create_dir_all(base).map_err(|e| format!("create generation root {}: {e}", base.display()))?;
    if !acl::grant_all_app_packages(base.parent().ok_or("invalid SystemIME root")?, true)
        || !acl::grant_all_app_packages(base, true)
    {
        return Err("SystemIME ACL failed".into());
    }
    let dest = base.join(&p.generation);
    if dest.exists() {
        validate_root(&dest, p)?;
        return Ok(dest);
    }
    let temp = base.join(format!(".stage-{}-{}", p.generation, std::process::id()));
    // Never reuse a partially staged directory.
    fs::create_dir(&temp).map_err(|e| format!("create staging directory {}: {e}", temp.display()))?;
    let result = (|| {
        for f in &p.manifest.files {
            if f.role != "program" {
                fs::copy(src.join(&f.path), temp.join(&f.path)).map_err(|e|
                    format!("copy {} -> {}: {e}", src.join(&f.path).display(), temp.join(&f.path).display()))?;
            }
        }
        fs::write(temp.join(MANIFEST), &p.bytes).map_err(|e|
            format!("write staging manifest {}: {e}", temp.join(MANIFEST).display()))?;
        if !acl::grant_all_app_packages(&temp, true) {
            return Err("generation ACL failed".into());
        }
        for entry in &p.manifest.files {
            if entry.role != "program"
                && !acl::grant_all_app_packages(&temp.join(&entry.path), false)
            {
                return Err(format!("payload ACL failed: {}", entry.path));
            }
        }
        // Do not execute emulated x86/x64 probes inside the temporary directory.
        // On Windows ARM64 the directory can remain in use briefly after those
        // processes exit, making the immediately following rename fail with 5.
        // Verify bytes first, publish the immutable path, then run all loader
        // checks there. COM/TIP registration only follows a successful stage().
        validate_files(&temp, p).map_err(|e| format!("validate staging {}: {e}", temp.display()))?;
        fs::rename(&temp, &dest).map_err(|e|
            format!("publish generation {} -> {}: {e}", temp.display(), dest.display()))?;
        // Retain a published but rejected generation for diagnosis; never switch
        // COM or remove an existing generation when any loader check fails.
        validate_root(&dest, p).map_err(|e| format!("validate published generation {}: {e}", dest.display()))?;
        Ok(dest)
    })();
    if let Err(error) = result {
        if let Err(cleanup) = fs::remove_dir_all(&temp) {
            if cleanup.kind() != std::io::ErrorKind::NotFound {
                return Err(format!("{error}; staging cleanup failed {}: {cleanup}", temp.display()));
            }
        }
        return Err(error);
    }
    result
}
#[derive(Serialize, Deserialize)]
struct Journal {
    generation: String,
    phase: String,
    action: String,
    registry: Snapshot,
}
fn journal_path() -> Result<PathBuf> {
    Ok(system_root()?.join("rollback.json"))
}
fn save_journal(j: &Journal) -> Result<()> {
    let path = journal_path()?;
    let tmp = path.with_extension("new");
    let bytes = serde_json::to_vec(j).map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut f = fs::File::create(&tmp).map_err(ioe)?;
    f.write_all(&bytes).map_err(ioe)?;
    f.sync_all().map_err(ioe)?;
    drop(f);
    // Atomic durable replacement, including post-registration phase updates.
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    let from: Vec<u16> = tmp.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 1 | 8) } == 0 {
        return Err(ioe(std::io::Error::last_os_error()));
    }
    Ok(())
}
fn write_registration(machine: bool, root: &Path, p: &Package) -> Result<()> {
    registry_tx::set_com(
        machine,
        KEY_WOW64_64KEY,
        &root.join(&p.entry("tsf-native").path),
    )
    .map_err(ioe)?;
    registry_tx::set_com(
        machine,
        KEY_WOW64_32KEY,
        &root.join(&p.entry("tsf-x86").path),
    )
    .map_err(ioe)?;
    let hive = registry_tx::hive(machine);
    // Only our TIP subtree and category membership; never rewrite global lists.
    let tip = registry_tx::tip_path();
    let (k, _) = hive
        .create_subkey_with_flags(&tip, KEY_READ | KEY_WRITE | KEY_WOW64_64KEY)
        .map_err(ioe)?;
    k.create_subkey("Description")
        .map_err(ioe)?
        .0
        .set_value("", &"HuFu 虎符输入法（虎码）")
        .map_err(ioe)?;
    for c in TIP_CATS {
        k.create_subkey(format!(r"Category\Category\{}\{}", c, CLSID))
            .map_err(ioe)?;
        k.create_subkey(format!(r"Category\Item\{}\{}", CLSID, c))
            .map_err(ioe)?;
        if machine {
            for path in [
                format!(r"SOFTWARE\Microsoft\CTF\Category\Category\{}\{}", c, CLSID),
                format!(r"SOFTWARE\Microsoft\CTF\Category\Item\{}\{}", CLSID, c),
            ] {
                hive.create_subkey_with_flags(path, KEY_READ | KEY_WRITE | KEY_WOW64_64KEY)
                    .map_err(ioe)?;
            }
        }
    }
    let lp = k
        .create_subkey(format!(r"LanguageProfile\0x00000804\{}", PROFILE))
        .map_err(ioe)?
        .0;
    lp.set_value("Description", &"HuFu 虎符输入法")
        .map_err(ioe)?;
    lp.set_value("Display Description", &"HuFu 虎符输入法")
        .map_err(ioe)?;
    lp.set_value("Enable", &1u32).map_err(ioe)?;
    lp.set_value(
        "IconFile",
        &root
            .join(&p.entry("tsf-native").path)
            .to_string_lossy()
            .as_ref(),
    )
    .map_err(ioe)?;
    lp.set_value("IconIndex", &0u32).map_err(ioe)?;
    Ok(())
}
pub fn machine_install(out: &mut Out) -> i32 {
    let result = (|| {
        if !common::is_admin() {
            return Err("machine phase requires elevation".into());
        }
        let _lock = maintenance_lock()?;
        recover_pending_machine()?;
        let src = common::install_dir();
        let p = Package::load(&src, preflight::architecture()?)?;
        out.line(&format!("machine stage: prepare {}", p.generation));
        let root = stage(&src, &p)?;
        out.line("machine stage: final directory and loader checks passed; preparing registration");
        let previous = Snapshot::capture(true).map_err(|e| format!("snapshot system registration: {e}"))?;
        save_journal(&Journal {
            generation: p.generation.clone(),
            phase: "prepared".into(),
            action: "install".into(),
            registry: previous,
        })?;
        let result: Result<()> = (|| {
            write_registration(true, &root, &p)?;
            exec_logged(out,
                &native_tool(&root, &p),
                &format!(
                    "reg \"{}\"",
                    root.join(&p.entry("tsf-native").path).display()
                ),
            )?;
            out.line(&await_profile(&native_tool(&root, &p))?);
            mark_journal("applied")?;
            Ok(())
        })();
        if let Err(error) = result {
            let recovery = restore_machine(&p.generation, false);
            return Err(format!(
                "{error}; rollback: {}",
                recovery
                    .err()
                    .unwrap_or_else(|| "restored owned registry".into())
            ));
        }
        out.line(&format!("OK machine generation {}", p.generation));
        Ok(())
    })();
    report(out, result)
}
fn mark_journal(phase: &str) -> Result<()> {
    let mut j: Journal = serde_json::from_slice(&fs::read(journal_path()?).map_err(ioe)?)
        .map_err(|e| e.to_string())?;
    j.phase = phase.into();
    save_journal(&j)
}
fn recover_pending_machine() -> Result<()> {
    let path = journal_path()?;
    if !path.exists() {
        return Ok(());
    }
    let j: Journal =
        serde_json::from_slice(&fs::read(path).map_err(ioe)?).map_err(|e| e.to_string())?;
    if j.phase == "prepared" {
        restore_machine(&j.generation, false)?;
    }
    Ok(())
}

fn restore_machine(generation: &str, check_active: bool) -> Result<()> {
    let data = fs::read(journal_path()?).map_err(ioe)?;
    let mut j: Journal = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    if !j.registry.is_machine() {
        return Err("rollback hive mismatch".into());
    }
    if j.generation != generation {
        return Err("rollback generation mismatch; newer transaction retained".into());
    }
    if check_active && j.action == "install" {
        let active = registry_tx::com_dll(true, KEY_WOW64_64KEY).map_err(ioe)?;
        if active.parent() != Some(system_root()?.join(generation).as_path()) {
            return Err("active generation changed; rollback refused".into());
        }
    }
    j.registry.restore().map_err(ioe)?;
    // Refresh the shared profile through the previous generation's native API tool.
    // Always replay the exact snapshot afterwards, including if the API failed.
    if let Ok(old_dll) = registry_tx::com_dll(true, KEY_WOW64_64KEY) {
        if let Some(old_root) = old_dll.parent() {
            if old_root.starts_with(system_root()?) && old_root.join(MANIFEST).is_file() {
                let old = Package::load_system(old_root, preflight::architecture()?)?;
                let refresh = exec(
                    &native_tool(old_root, &old),
                    &format!("reg \"{}\"", old_dll.display()),
                );
                j.registry.restore().map_err(ioe)?;
                refresh?;
                await_profile(&native_tool(old_root, &old))?;
            }
        }
    }
    j.phase = "restored".into();
    save_journal(&j)
}
pub fn rollback_machine(out: &mut Out) -> i32 {
    report(
        out,
        (|| {
            if !common::is_admin() {
                return Err("rollback requires elevation".into());
            }
            let _lock = maintenance_lock()?;
            let p = Package::load(&common::install_dir(), preflight::architecture()?)?;
            restore_machine(&p.generation, true)
        })(),
    )
}
fn elevated(command: &str, parent_log: Option<&Path>) -> Result<()> {
    let log = parent_log
        .map(PathBuf::from)
        .unwrap_or_else(common::elevated_log);
    let arguments = format!("{command} --log \"{}\"", log.display());
    let exe = std::env::current_exe().map_err(ioe)?;
    let r = if common::is_admin() {
        procop::spawn_wait_hidden(&exe, &arguments, u32::MAX)
    } else {
        procop::spawn_runas_wait(&exe, &arguments, u32::MAX)
    };
    if r == SpawnResult::Exit(0) {
        Ok(())
    } else {
        Err(format!("{command}: {r:?}; machine phase log: {} (helper refuses mutation if this log cannot be opened)", log.display()))
    }
}
fn report(out: &mut Out, result: Result<()>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(e) => {
            out.line(&format!("ERROR: {e}"));
            1
        }
    }
}

pub fn install(out: &mut Out, no_hklm: bool, user_only: bool) -> i32 {
    let parent_log = out.logfile.clone();
    let mut machine_changed = false;
    let result = (|| {
        let src = common::install_dir();
        let p = Package::load(&src, preflight::architecture()?)?;
        preflight::probe(&src, &p)?;
        let needs_machine =
            !no_hklm && !user_only && (common::is_admin() || common::in_admin_group());
        if needs_machine {
            elevated("install-elevated", parent_log.as_deref())?;
            machine_changed = true;
        }
        // Standard users attach to a fully validated machine generation. Never point
        // HKCU at a private facade or a missing SystemIME DLL after UAC cancellation.
        let active_dll = registry_tx::com_dll(true, KEY_WOW64_64KEY).map_err(ioe)?;
        let root = active_dll.parent().ok_or("invalid machine registration")?;
        if !root.starts_with(system_root()?) {
            return Err("machine base is legacy; administrator must upgrade it first".into());
        }
        let base = Package::load_system(root, preflight::architecture()?)?;
        validate_root(root, &base)?;
        if active_dll != root.join(&base.entry("tsf-native").path) {
            return Err("machine native COM does not point at the facade".into());
        }
        if registry_tx::com_dll(true, KEY_WOW64_32KEY).map_err(ioe)?
            != root.join(&base.entry("tsf-x86").path)
        {
            return Err("machine x86 registration does not match generation".into());
        }
        let previous = Snapshot::capture(false).map_err(ioe)?;
        let lists = registry_tx::UserList::capture().map_err(ioe)?;
        let user_result: Result<()> = (|| {
            write_registration(false, root, &base)?;
            if !user_only {
                exec_logged(out, &native_tool(root, &base), "enable")?;
            }
            let key = registry_tx::hive(false)
                .create_subkey_with_flags(r"Software\HuFu", KEY_READ | KEY_WRITE | KEY_WOW64_64KEY)
                .map_err(ioe)?
                .0;
            key.set_value("InstallDir", &src.to_string_lossy().as_ref())
                .map_err(ioe)?;
            Ok(())
        })();
        if let Err(e) = user_result {
            let user_restore = previous.restore().and_then(|_| lists.restore());
            return Err(format!("{e}; user rollback: {:?}", user_restore));
        }
        if user_only {
            out.line("OK current-user registration only");
            return Ok(());
        }
        // Startup and shortcuts are post-registration conveniences; no list reordering.
        if !crate::regop::set_str(
            "HKCU",
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            "HuFu",
            &format!("\"{}\"", src.join("hufu-server.exe").display()),
        ) {
            out.line("Warning: startup entry could not be written");
        }
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let programs = PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs");
            for (file, title) in [
                ("hufu-settings.exe", "HuFu 虎符输入法设置.lnk"),
                ("hufu-trainer.exe", "HuFu 虎符跟打器.lnk"),
            ] {
                if src.join(file).exists()
                    && !crate::lnk::create_app_shortcut(
                        &programs.join(title),
                        &src.join(file),
                        "",
                        &src,
                        &src.join("图标.ico"),
                    )
                {
                    out.line("Warning: shortcut creation failed");
                }
            }
        }
        let server = src.join("hufu-server.exe");
        if common::is_admin() {
            procop::spawn_detached("explorer.exe", &format!("\"{}\"", server.display()));
        } else {
            procop::spawn_detached(&server.to_string_lossy(), "");
        }
        out.line(&format!(
            "OK install; active TSF generation {} (user data retained)",
            base.generation
        ));
        Ok(())
    })();
    let result = match result {
        Err(e) if machine_changed => Err(format!(
            "{e}; machine rollback: {:?}",
            elevated("rollback-machine", parent_log.as_deref())
        )),
        other => other,
    };
    report(out, result)
}

pub fn uninstall_machine(out: &mut Out) -> i32 {
    let result = (|| {
        if !common::is_admin() {
            return Err("machine removal requires elevation".into());
        }
        let _lock = maintenance_lock()?;
        recover_pending_machine()?;
        let dll = registry_tx::com_dll(true, KEY_WOW64_64KEY).map_err(ioe)?;
        let root = dll.parent().ok_or("invalid machine DLL")?;
        if !root.starts_with(system_root()?) {
            return Err("not a managed generation".into());
        }
        let p = Package::load_system(root, preflight::architecture()?)?;
        let previous = Snapshot::capture(true).map_err(ioe)?;
        save_journal(&Journal {
            generation: p.generation.clone(),
            phase: "prepared".into(),
            action: "uninstall".into(),
            registry: previous,
        })?;
        let remove: Result<()> = (|| {
            exec(&native_tool(root, &p), "unreg")?;
            Snapshot::remove_owned(true).map_err(ioe)?;
            mark_journal("applied")?;
            Ok(())
        })();
        if let Err(e) = remove {
            return Err(format!(
                "{e}; rollback: {:?}",
                restore_machine(&p.generation, false)
            ));
        }
        // Retain immutable generations: existing hosts and other users can still hold
        // them. A later explicit cleanup may collect them after ownership checks.
        Ok(())
    })();
    report(out, result)
}
pub fn rollback_uninstall_machine(out: &mut Out) -> i32 {
    let result = (|| {
        if !common::is_admin() {
            return Err("rollback requires elevation".into());
        }
        let _lock = maintenance_lock()?;
        let j: Journal = serde_json::from_slice(&fs::read(journal_path()?).map_err(ioe)?)
            .map_err(|e| e.to_string())?;
        if j.action != "uninstall" || j.phase != "applied" {
            return Err("no completed uninstall transaction to restore".into());
        }
        match registry_tx::com_dll(true, KEY_WOW64_64KEY) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => {
                return Err("machine registration changed since uninstall; recovery refused".into())
            }
        }
        restore_machine(&j.generation, false)
    })();
    report(out, result)
}

pub fn uninstall(out: &mut Out, no_hklm: bool) -> i32 {
    let parent_log = out.logfile.clone();
    let result = (|| {
        let src = common::install_dir();
        let p = Package::load(&src, preflight::architecture()?)?;
        let previous = Snapshot::capture(false).map_err(ioe)?;
        let lists = registry_tx::UserList::capture().map_err(ioe)?;
        let values = registry_tx::UserValues::capture().map_err(ioe)?;
        let mut machine_changed = false;
        let removal: Result<()> = (|| {
            // Disable only this caller's profile, before removing shared registration.
            exec(&native_tool(&src, &p), "disable")?;
            if !no_hklm && (common::is_admin() || common::in_admin_group()) {
                elevated("uninstall-machine", parent_log.as_deref())?;
                machine_changed = true;
            }
            registry_tx::UserList::clear().map_err(ioe)?;
            Snapshot::remove_owned(false).map_err(ioe)?;
            values.clear().map_err(ioe)?;
            Ok(())
        })();
        if let Err(e) = removal {
            let machine_recovery = if machine_changed {
                elevated("rollback-uninstall-machine", parent_log.as_deref())
            } else {
                Ok(())
            };
            let user_recovery = previous
                .restore()
                .and_then(|_| lists.restore())
                .and_then(|_| values.restore());
            return Err(format!(
                "{e}; machine rollback: {machine_recovery:?}; user rollback: {user_recovery:?}"
            ));
        }
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let programs = PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs");
            for title in ["HuFu 虎符输入法设置.lnk", "HuFu 虎符跟打器.lnk"] {
                let _ = fs::remove_file(programs.join(title));
            }
        }
        out.line("OK unregistered; user data and loaded TSF generations retained");
        Ok(())
    })();
    report(out, result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_readback_retries_only_observation_until_visible() {
        let mut calls = 0;
        let mut pauses = 0;
        let result = poll_profile(|| {
            calls += 1;
            (SpawnResult::Exit(if calls == 4 { 0 } else { 2 }), format!("observation={calls}"))
        }, || pauses += 1).unwrap();
        assert_eq!((calls, pauses), (4, 3));
        assert!(result.contains("observation=1") && result.contains("observation=4"));
    }

    #[test]
    fn profile_readback_is_bounded_and_preserves_diagnostics() {
        let mut calls = 0;
        let mut pauses = 0;
        let result = poll_profile(|| {
            calls += 1;
            (SpawnResult::Exit(2), "hresult=0x80004005".into())
        }, || pauses += 1).unwrap_err();
        assert_eq!((calls, pauses), (21, 20));
        assert!(result.contains("21/21") && result.contains("hresult=0x80004005"));
    }

    #[test]
    fn profile_readback_does_not_retry_process_failure_or_timeout() {
        for failure in [SpawnResult::Fail, SpawnResult::Timeout, SpawnResult::Exit(101)] {
            let result = poll_profile(|| (failure, "process error".into()), || panic!("must not retry"));
            assert!(result.unwrap_err().contains("process error"));
        }
    }

    fn remove_test_tree(path: &Path) {
        // Windows emulation may retain the last probe directory briefly. This
        // delay belongs only to disposal of this test's private artifacts.
        for _ in 0..50 {
            if !path.exists() || fs::remove_dir_all(path).is_ok() { return; }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("test artifact still held after probe exit: {}", path.display());
    }

    #[test]
    #[ignore = "requires HUFU_STAGE_TEST_PAYLOAD pointing to a built native payload"]
    fn generation_stage_keeps_source_and_final_payload_consistent() {
        let src = PathBuf::from(std::env::var_os("HUFU_STAGE_TEST_PAYLOAD").expect("explicit test payload required"));
        let arch = preflight::architecture().unwrap();
        let p = Package::load(&src, arch).unwrap();
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let sandbox = std::env::temp_dir().join(format!("hufu-stage-test-{}-{unique}", std::process::id()));
        let base = sandbox.join("HuFu/generations");
        eprintln!("isolated staging root: {}", sandbox.display());
        let dest = stage_at(&src, &p, &base).expect("production staging must publish a validated generation");
        assert_eq!(dest, base.join(&p.generation));
        validate_root(&dest, &p).unwrap();
        assert_eq!(stage_at(&src, &p, &base).unwrap(), dest, "same generation must be reusable");
        assert_eq!(Package::load(&src, arch).unwrap().bytes, p.bytes, "source payload unchanged");
        remove_test_tree(&sandbox);
    }
}
