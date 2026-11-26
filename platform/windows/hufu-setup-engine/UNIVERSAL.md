# Universal EXE maintenance engine

`package-architecture.json` schema 1 selects `system_architecture` (`x64` or `arm64`), `version`, and `files` (`path`, `machine`, `sha256`, `role`). Native OS detection dynamically resolves IsWow64Process2, falling back to GetNativeSystemInfo on Windows 10 1607. Process bitness is never the OS selector.

Roles: `tsf-native`, `tsf-x86`, `registration-native`, `registration-x86`, `tsf-native-dependency`, `tsf-resource`, `render-helper`, `probe-x64`, `program`. ARM64 requires a single ARM64X facade, ARM64 and x64 implementation dependencies, and a separate x64 probe. Resources use machine `data`. Only settings, WebView2Loader, and trainer may be x64 programs in an ARM64 payload; the backend and maintenance engine must be native. System payloads and executable programs remain flat. Data in the program role may use safe relative subdirectories; traversal, devices, aliases and symlink/reparse components are rejected.

Commands:

- `architecture [--log file]`: print native OS JSON.
- `preflight --src directory [--log file]`: validate all manifest entries, hashes and PE machine values; run native, x86 and (on ARM64) x64 read-only DLL probes. No registration or service launch. Inno must invoke this before stopping or replacing existing programs.
- `install [--no-hklm] [--user-only]`: original user coordinates machine helper and current-user configuration. User-only does not enable or launch services. Standard users need an existing validated machine generation.
- `install-elevated`, `rollback-machine`, `uninstall-machine`, `rollback-uninstall-machine`: machine-only helpers. Never run the entire user workflow under alternate administrator credentials.
- `uninstall [--no-hklm]`: disable the caller's profile, remove machine registration in a separate helper if authorized, then remove only the caller's own registration/list/startup values. User files remain.

The immutable directory is `%WINDIR%\SystemIME\HuFu\generations\<version>-<manifest SHA256 first 12>`. All non-program entries are copied together, hash checked, granted package read/execute access, and probed before either COM registry view is changed. The 64-bit COM view points at the single ARM64X facade on ARM64; the 32-bit view points at x86. Shared profile registration is performed once, using the native tool. A fresh `inspect-profile` process must succeed.

Staging order (1.9.9.11): verify all copied bytes in the temporary directory, rename it to the final immutable generation, then run loader probes at that final path. Do not run an x86/x64 executable inside the temporary directory before renaming it: on Windows ARM64, the directory may remain temporarily in use after the emulated process exits (observed error 5 on rename). Registration still begins only after every final-path probe succeeds. An existing or rejected published generation is never overwritten; errors include the operation and path. The explicitly invoked `generation_stage_keeps_source_and_final_payload_consistent` integration test exercises the production staging function using a built payload and a private temporary root, without registration.

Profile readback (1.9.9.12): perform registration once, then start up to 21 independent, read-only `inspect-profile` processes, with 500 ms between observations. Exit 2 permits another observation; timeout, launch failure or another exit code fails immediately. Each observer has a 5-second process limit. Exhaustion remains a failure and enters the existing recovery path. The previous generation's restoration uses the same verification policy. This is a bounded product policy, not a Windows guarantee that all consumers refresh within that interval. Three real unregister/register cycles on this ARM64 host showed stale observations for roughly 0.54–0.58 seconds; the full EXE then passed on its second observation.

Helper stdout/stderr are captured together in an exclusive create-new, delete-on-close file, with at most 64 KiB included in diagnostics. Registration and user-enable success output, all profile observations, and helper failures are written to the engine log. `inspect-profile` reports the operation and numeric HRESULT. Capture failure is itself a failure; a helper is never left running while recovery starts.

Machine snapshots contain only this product's COM, TIP and category membership keys in explicit views. The protected rollback journal is atomically replaced and records prepared/applied/restored phases. A pending prepared operation is restored before the next machine operation. Helpers confirm child termination before restoring registration. Probe/registration timeout is failure; the coordinating process waits for a machine helper's own recovery instead of killing it midway. A user-stage failure requests machine recovery through UAC again; rejected recovery is reported as incomplete, never as restored. The package files are restored by Inno separately.

Uninstall retains all immutable generations, including the formerly active one: hosts and other users may still hold them. Automatic multi-user collection and a global atomic transaction across every user's hive are outside this implementation. Rollback of language lists touches only HuFu values/slots and refuses to replace a slot claimed by another IME. It does not rewrite whole language lists or change another user's configuration.

Validation is split: portable package tests run with `cargo test --lib`; Windows engine tests include a disposable `HKCU\Software\HuFuSetupEngineIsolatedTests\<pid>` registry substitute. They do not call production registration APIs. These checks are not evidence of real installation/UAC, reboot, multi-user behavior or application input acceptance.

## Inno compensation boundary

`snapshot-inno --snapshot <absolute-new-JSON-path>` captures exactly the current user's `Software\Microsoft\Windows\CurrentVersion\Uninstall\{2F8A5D41-7B63-4C3E-9E15-A8D4B0C6F293}_is1` in both explicit registry views, including absence, subkeys, raw value types and bytes. The file is create-new, never silently replaced. `restore-inno --snapshot <same-path>` checks the saved TokenUser SID against the caller before restoring that fixed key. The JSON cannot choose another key or HKLM; the Uninstall parent and other products are never captured or replaced. Restore failures are nonzero and retain the snapshot. Inno owns the separate program/uninstaller file backup and decides when to invoke compensation.

An isolated Windows regression test checks REG_BINARY/EXPAND_SZ/MULTI_SZ/DWORD/QWORD/NONE round trips, subkeys, absent state, a mismatched SID, and an adjacent fictitious product sentinel. It runs through the disposable test hive mapping, not the production uninstall key.

## Development verification (2026-11-26)

On the Windows ARM64 development host, after enabling the manifest dispatch and Inno snapshot commands:

- `cargo test -p hufu-setup-engine --bin hufu-setup-engine --target aarch64-pc-windows-msvc`: 15 passed, 1 ignored.
- The same command for `x86_64-pc-windows-msvc`, running under x64 emulation: 15 passed, 1 ignored.
- Linux `cargo test --manifest-path platform/windows/hufu-setup-engine/Cargo.toml --lib`: 8 passed, including a directory-symlink escape rejection.

The ignored entry is the deliberate 30-second child fixture; the timeout test actually invokes it in a separate process, terminates it on a 2-second limit, and confirms it has exited before returning. Windows tests cover package rejection, isolated own-key rollback, preservation of another IME's concurrent list change, Inno raw-value/SID boundaries, and existing BackupDiff behavior. The only compiler warning is the existing unused `create_url_shortcut` helper.

No production installation, production COM/TIP registration, actual product uninstall-key restoration, UAC prompt, server startup or active user-data migration was run for this verification. The TSF bridge gate and final packaging/preflight evidence are maintained by their corresponding build steps; they are not implied by these engine unit tests.

Final Inno review additionally forwards the parent's explicit engine log path to every elevated machine/recovery helper. Each helper appends a phase marker and checks log access before registration changes; an alternate administrator unable to write the selected log gets a nonzero result before mutation. The ARM64 engine suite after this change passed 16 tests (plus the deliberately ignored child fixture), including preservation of prior log evidence and rejection of inaccessible/relative log destinations. The earlier x64 15-test result predates this final log-path change; production build validation should use the final source.
