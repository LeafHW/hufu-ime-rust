//! hufu-setup-engine —— install.ps1/uninstall.ps1 血律的 Rust 编译版。
//! 子命令：install / install-elevated / uninstall / postboot / purge-data
//! 公共参数：--log <path>（引擎日志；install-elevated 默认 %TEMP%\hufu-install-elevated.log）

mod acl;
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: hufu-setup-engine <install|install-elevated|uninstall|postboot|purge-data> [--no-hklm] [--user-only] [--log <path>]");
        std::process::exit(1);
    }
    let cmd = args[0].clone();
    let mut no_hklm = false;
    let mut user_only = false;
    let mut log: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--no-hklm" => no_hklm = true,
            "--user-only" => user_only = true,
            "--log" => {
                if i + 1 < args.len() {
                    log = Some(PathBuf::from(&args[i + 1]));
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
