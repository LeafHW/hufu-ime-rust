//! hufu-setup-engine —— install.ps1/uninstall.ps1 血律的 Rust 编译版。
//! 子命令：install / install-elevated / uninstall / postboot / purge-data /
//!        backup-diff（数据文件差异备份+拷入：原 hufu-setup-tools.ps1
//!        -BackupDiff，2026-10-21 为「exe 安装包完全 0 PowerShell」移植）
//! 公共参数：--log <path>（引擎日志；install-elevated 默认 %TEMP%\hufu-install-elevated.log）

// 【重启闪终端修复 2026-11-05】GUI 子系统：RunOnce(HuFuPostBoot) 由
// userinit 在登录时直接执行——console 子系统会被分配新控制台，终端
// 窗口闪现一下（postboot 幂等秒退）。旧 ps1 轨的 -WindowStyle Hidden
// 同样闪黑框，是历史已知症状；Rust 化时漏设子系统，此修复补齐。
// 输出全部走 --log 文件；从终端手动调试时 main 开头 AttachConsole(-1)
// 挂回父控制台，println 照常工作。
#![cfg_attr(windows, windows_subsystem = "windows")]

mod acl;
mod backupdiff;
mod common;
mod install;
mod lnk;
mod pfro;
mod postboot;
mod procop;
mod purge;
mod regop;
mod uninstall;

use common::Out;
use std::path::PathBuf;

fn main() {
    // 挂回父控制台（ATTACH_PARENT_PROCESS=-1）：终端手动跑时输出照常；
    // 双击/RunOnce 启动（无控制台父进程）返回失败，忽略即可。
    #[cfg(windows)]
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn AttachConsole(pid: isize) -> i32;
        }
        AttachConsole(-1);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: hufu-setup-engine <install|install-elevated|uninstall|postboot|purge-data|backup-diff> [--no-hklm] [--user-only] [--log <path>] [--manifest <f> --src <d> --dst <d> --backup <d>]");
        std::process::exit(1);
    }
    let cmd = args[0].clone();
    let mut no_hklm = false;
    let mut user_only = false;
    let mut log: Option<PathBuf> = None;
    let mut manifest: Option<String> = None;
    let mut src_root: Option<String> = None;
    let mut dst_root: Option<String> = None;
    let mut backup_root: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--no-hklm" => no_hklm = true,
            "--user-only" => user_only = true,
            "--log" | "--manifest" | "--src" | "--dst" | "--backup" => {
                if i + 1 < args.len() {
                    let v = args[i + 1].clone();
                    match args[i].as_str() {
                        "--log" => log = Some(PathBuf::from(&v)),
                        "--manifest" => manifest = Some(v),
                        "--src" => src_root = Some(v),
                        "--dst" => dst_root = Some(v),
                        _ => backup_root = Some(v),
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    // 提权子阶段默认落 %TEMP%\hufu-install-elevated.log（install.ps1 L347-349 契约：
    // 父进程读回该文件逐行回显）
    if cmd == "install-elevated" && log.is_none() {
        log = Some(common::elevated_log());
    }
    let mut out = Out::new(log);
    let code = match cmd.as_str() {
        "install" => install::run_install(&mut out, no_hklm, user_only),
        "install-elevated" => install::run_phase_elevated(&mut out),
        "uninstall" => uninstall::run_uninstall(&mut out, no_hklm),
        "postboot" => {
            // 铁律：恒静默恒 0（hufu-postboot.ps1 薄壳契约）
            let rc = postboot::run_postboot(&mut out);
            if rc != 0 {
                0
            } else {
                0
            }
        }
        "purge-data" => purge::run_purge_data(&mut out),
        // 【0 PowerShell 2026-10-21】iss 里最后一处 PowerShell 子进程
        //（hufu-setup-tools.ps1 -BackupDiff）的 Rust 等价实现
        "backup-diff" => backupdiff::run_backup_diff(
            &mut out,
            backupdiff::Args {
                manifest: manifest.as_deref(),
                src: src_root.as_deref(),
                dst: dst_root.as_deref(),
                backup: backup_root.as_deref(),
            },
        ),
        // iss 专用微子命令（rust 轨替换 iss 里最后的两处 powershell 探针）
        "server-alive" => {
            if procop::proc_running("hufu-server.exe") {
                0
            } else {
                1
            }
        }
        "clear-override" => {
            // iss L810-826 等价：仅当默认输入法覆盖值含虎符 profile 才删（R16 血律：
            // 他人值零触碰）
            const OV_KEY: &str = r"Control Panel\International\User Profile";
            match regop::get_str("HKCU", OV_KEY, "InputMethodOverride") {
                Some(v) if v.contains("8F5C2A11") => {
                    let _ = regop::delete_value("HKCU", OV_KEY, "InputMethodOverride");
                }
                _ => {}
            }
            0
        }
        _ => {
            eprintln!("未知子命令: {}", cmd);
            1
        }
    };
    std::process::exit(code);
}
