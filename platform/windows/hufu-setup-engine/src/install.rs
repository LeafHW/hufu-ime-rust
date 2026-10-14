//! install.rs —— install.ps1（556 行）全量 Rust 翻译（血律：行为等价、文案逐字节）。
//! 对账机制：每个行为段前有 `// install.ps1 L<起>-<止>：<段名>` 注释，验收按此清单逐段核对。
//!
//! PS 语义映射（总则）：
//! - Set-Reg / Set-RegHKLM / Set-RegDWord / Set-ItemProperty → regop::set_str / set_dword
//!   （"(default)" 写键默认值；键不存在先建 = New-Item -Force → 由 regop 保证）。
//! - New-Item（注册表键）→ regop::ensure_key；New-Item（目录）→ std::fs::create_dir_all。
//! - Test-Path（文件/目录）→ Path::exists()；Test-Path（注册表）→ regop::key_exists。
//! - Remove-Item / Copy-Item / Rename-Item → std::fs 小工具函数（模块内私有 fn）。
//! - PS 字符串比较/通配/-Unique 均不区分大小写 → eq_ci / starts_with_ci / push_unique。
//! - $ErrorActionPreference='Continue'：单步失败打 ⚠ 行继续，不 panic、不 unwrap。
//!
//! install.ps1 L12-14（param/$ErrorActionPreference）由 main.rs 子命令与参数解析承担；
//! L16-18（$CLSID/$PROFILE/$TFCAT_KBD）与 L48-66（Set-Reg 三函数）在 common.rs / regop.rs。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::acl;
use crate::common::{self, Out, ASM_KBD_CAT, CLSID, PROFILE, TFCAT_KBD, TIP_CATS};
use crate::lnk;
use crate::procop::{self, SpawnResult};
use crate::regop;

// ───────────────────── 小工具（PS 语义私有复刻） ─────────────────────

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// PS 字符串比较不区分大小写（-eq / -ne / -contains）
fn eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// PS `-like '<prefix>*'` 前缀匹配（不区分大小写；用 get 防非 ASCII 边界切片 panic）
fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.get(..prefix.len())
        .map_or(false, |p| p.eq_ignore_ascii_case(prefix))
}

/// PS 布尔插值渲染（$true → "True"）
fn bool_ps(b: bool) -> &'static str {
    if b {
        "True"
    } else {
        "False"
    }
}

/// Select-Object -Unique（不区分大小写、保序保首见）
fn push_unique(v: &mut Vec<String>, s: String) {
    if !v.iter().any(|x| eq_ci(x, &s)) {
        v.push(s);
    }
}

fn unique_union(a: &[String], b: &[String]) -> Vec<String> {
    let mut r = Vec::new();
    for s in a.iter().chain(b.iter()) {
        push_unique(&mut r, s.clone());
    }
    r
}

/// 目标 DLL 的 .oldN 腾位名（$sysdll.old$n 语义：同目录同名 + .oldN 后缀）
fn with_old_suffix(p: &Path, n: u32) -> PathBuf {
    let name = p
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    p.with_file_name(format!("{}.old{}", name, n))
}

// install.ps1 L88-98 / L112-119：DLL 拷贝（目标被宿主进程加载锁定 → 改名 .oldN 腾位再拷；
// Windows 允许改名已加载 DLL，旧副本留 .oldN 随系统清理）。x64 段播报腾位文案，x86 段无文案。
fn copy_dll_with_old_fallback(out: &mut Out, src: &Path, dst: &Path, announce: bool) {
    if std::fs::copy(src, dst).is_ok() {
        return;
    }
    let mut n: u32 = 1;
    while with_old_suffix(dst, n).exists() {
        n += 1;
    }
    let _ = std::fs::rename(dst, with_old_suffix(dst, n));
    let _ = std::fs::copy(src, dst); // 再失败由调用方 Test-Path 兜底（L99/L221 前置检查）
    if announce {
        out.line(&format!("（旧 DLL 被占用，已腾位为 .old{}）", n));
    }
}

/// 递归清 readonly 属性（PS $_.Attributes='Normal' 语义；连根目录一起清，
/// 补齐 PS Remove-Item -Force 对只读根的删除能力）
fn normalize_tree_attrs(p: &Path) {
    let md = match std::fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(_) => return,
    };
    let mut perms = md.permissions();
    if perms.readonly() {
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(p, perms);
    }
    if md.is_dir() {
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                normalize_tree_attrs(&e.path());
            }
        }
    }
}

/// Remove-Item -Force [-Recurse]：目录递归删、文件直接删
fn remove_entry(p: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(p)
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    }
}

/// Get-ChildItem -Recurse -File | Measure-Object Length -Sum（字节数）
fn dir_file_sum(p: &Path) -> u64 {
    let mut sum: u64 = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let ep = e.path();
            match std::fs::symlink_metadata(&ep) {
                Ok(m) if m.is_dir() => sum += dir_file_sum(&ep),
                Ok(m) => sum += m.len(),
                Err(_) => {}
            }
        }
    }
    sum
}

/// @(Get-ChildItem …).Count（一级条目计数）
fn count_entries(p: &Path) -> usize {
    std::fs::read_dir(p).map(|rd| rd.count()).unwrap_or(0)
}

// ───────────────────── install.ps1 L74-82：Invoke-SmokeReg ─────────────────────
// smoke 的 msctf 原生登记（COM 调 ITfInputProcessorProfiles）在 CTF 服务未就绪时会无限
// 挂起——60 秒超时杀进程按「跳过原生登记」记 0（与无管理员分支同语义）。
fn invoke_smoke_reg(out: &mut Out, reg_arg: &str) -> i32 {
    let smoke_exe = common::install_dir().join("hufu-tsf-smoke.exe");
    if !smoke_exe.exists() {
        return 0; // L76：exe 不存在 = 0
    }
    match procop::spawn_wait_hidden(&smoke_exe, &format!("reg \"{}\"", reg_arg), 60_000) {
        SpawnResult::Exit(c) => c, // L78：WaitForExit(60000) 成功 → ExitCode
        SpawnResult::Timeout => {
            // L79-81：超时杀进程 → 黄字提示 → 按 0 记
            out.line("⚠ msctf 登记超时（60 秒）——已跳过原生登记（注册表已写好，重开应用或重启后一般自愈）");
            0
        }
        SpawnResult::Fail => 0, // 启动失败（PS：Start-Process 报错、$p 为 null，无专有文案）
    }
}

// ───────────────────── install.ps1 L262-323：Install-UserConfig ─────────────────────
// HKCU COM + TIP 键树注册（每用户；-UserOnly 与默认主流程共用，行为不变）。
// PS 恒 return $true（注册失败为 Continue 型，只污染控制台不返回 false）。
fn install_user_config(out: &mut Out) -> bool {
    let inst = common::install_dir();
    let inst_s = path_str(&inst);
    let dll = inst.join("hufu_tsf.dll");
    let dll_s = path_str(&dll);
    let dll32 = inst.join("hufu_tsf32.dll");
    let sysdll = common::sysdll_x64();
    let sysdll32 = common::sysdll_x86();

    // install.ps1 L263-264：1.5) 记录安装目录（DLL 自愈链 / 卸载器读取）
    regop::set_str("HKCU", r"Software\HuFu", "InstallDir", &inst_s);

    // install.ps1 L266-281：2) HKCU COM 注册（解析 HKCU 优先；DLL 路径优先 SystemIME 副本，
    // 未提权安装退回用户目录）
    let dll_reg = if sysdll.exists() {
        path_str(&sysdll)
    } else {
        dll_s.clone()
    };
    let ips_user = format!(r"Software\Classes\CLSID\{}\InprocServer32", CLSID);
    // install.ps1 L272-276：【顺序铁律】regsvr32 必须先跑——DllRegisterServer 会把 HKCU
    // CLSID 默认值覆盖为 DLL 自身路径（安装目录，AppContainer 宿主读不了），之后重写为
    // $dllReg。PS 同步 & 调用无超时，此处按契约给 120 秒。
    let _ = procop::spawn_wait_hidden(
        &PathBuf::from(r"C:\Windows\System32\regsvr32.exe"),
        &format!("/s \"{}\"", dll_s),
        120_000,
    );
    regop::set_str(
        "HKCU",
        &format!(r"Software\Classes\CLSID\{}", CLSID),
        "(default)",
        "HuFu TSF Service",
    );
    regop::set_str("HKCU", &ips_user, "(default)", &dll_reg);
    regop::set_str("HKCU", &ips_user, "ThreadingModel", "Apartment");
    // install.ps1 L280-281：末值双保险（回读 ≠ 目标值 → 再写一遍）
    let final_dll = regop::get_str("HKCU", &ips_user, "(default)");
    if !matches!(&final_dll, Some(v) if eq_ci(v, &dll_reg)) {
        regop::set_str("HKCU", &ips_user, "(default)", &dll_reg);
    }

    // install.ps1 L283-303：HKCU TIP 键树——切换器/搜索框的生命线，不可省。
    // Win+空格切换器只列「带键盘分类」的 TIP；语言档案启用状态与切换器图标也在此树。
    let tip_u = format!(r"Software\Microsoft\CTF\TIP\{}", CLSID);
    regop::set_str("HKCU", &tip_u, "(default)", "HuFu 输入法");
    regop::set_str(
        "HKCU",
        &format!(r"{}\Description", tip_u),
        "(default)",
        "HuFu 虎符输入法（虎码）",
    );
    // install.ps1 L292-293：键盘分类两层键（仅 TFCAT_KBD 一项）
    let _ = regop::ensure_key(
        "HKCU",
        &format!(r"{}\Category\Category\{}\{}", tip_u, TFCAT_KBD, CLSID),
    );
    let _ = regop::ensure_key(
        "HKCU",
        &format!(r"{}\Category\Item\{}\{}", tip_u, CLSID, TFCAT_KBD),
    );
    // install.ps1 L294-297：键盘分类（34745C63）只写 HKLM 全局库——HKCU TIP 树里写键盘分类
    // 键会被 msctf 判非法周期删除（净室实测稳定复现），勿双写。
    // install.ps1 L298-303：LanguageProfile（Enable=DWORD 1 / IconIndex=DWORD 0 /
    // IconFile=$dllReg / Icon="$dllReg,0" 图标双写两制式；DisplayAttribute 键已退役不写）
    let lp_u = format!(r"{}\LanguageProfile\0x00000804\{}", tip_u, PROFILE);
    regop::set_str("HKCU", &lp_u, "(default)", "HuFu 虎符输入法");
    regop::set_dword("HKCU", &lp_u, "Enable", 1);
    regop::set_dword("HKCU", &lp_u, "IconIndex", 0);
    regop::set_str("HKCU", &lp_u, "IconFile", &dll_reg);
    regop::set_str("HKCU", &lp_u, "Icon", &format!("{},0", dll_reg));

    // install.ps1 L308-320：32 位宿主注册（HKCU 的 Wow6432Node 视图；跟打器等 32 位进程）
    if dll32.exists() {
        let dll_reg32 = if sysdll32.exists() {
            path_str(&sysdll32)
        } else {
            path_str(&dll32)
        };
        let wow_u = format!(r"Software\Classes\Wow6432Node\CLSID\{}", CLSID);
        regop::set_str("HKCU", &wow_u, "(default)", "HuFu TSF Service");
        regop::set_str(
            "HKCU",
            &format!(r"{}\InprocServer32", wow_u),
            "(default)",
            &dll_reg32,
        );
        regop::set_str(
            "HKCU",
            &format!(r"{}\InprocServer32", wow_u),
            "ThreadingModel",
            "Apartment",
        );
        out.line(&format!("OK HKCU 32 位视图已注册（DLL → {}）", dll_reg32));
    }
    // install.ps1 L321-322
    out.line(&format!(
        "OK HKCU COM + TIP 键树已注册（DLL → {}）",
        dll_reg
    ));
    true
}

// ───────────────────── install.ps1 L406-416：Get-HuFuUpOthers ─────────────────────
// 语言列表真存储快照（只读！）：HKCU `Control Panel\International\User Profile` 各语言
// 子键里名为 0804:* 且 ≠ 我们 tip_str 的值名集合（去重）。绝不作为任何重写的原料。
fn get_up_others(tip: &str) -> Vec<String> {
    let root = r"Control Panel\International\User Profile";
    let mut r: Vec<String> = Vec::new();
    for lk in regop::enum_subkeys("HKCU", root) {
        for n in regop::enum_value_names("HKCU", &format!(r"{}\{}", root, lk)) {
            if starts_with_ci(&n, "0804:") && !eq_ci(&n, tip) {
                push_unique(&mut r, n);
            }
        }
    }
    r
}

// ───────────────────── install.ps1 L417-426：Get-HuFuAsmOthers ─────────────────────
// 切换器装配表快照（只读！）：HKCU SortOrder\AssemblyItem\0x00000804\{34745C63-…} 各槽位
// 读 CLSID/Profile 值，CLSID ≠ 我们的 → "0804:{CLSID}{Profile}"（去重）。
fn get_asm_others() -> Vec<String> {
    let base = format!(
        r"Software\Microsoft\CTF\SortOrder\AssemblyItem\0x00000804\{}",
        ASM_KBD_CAT
    );
    let mut r: Vec<String> = Vec::new();
    if regop::key_exists("HKCU", &base) {
        for k in regop::enum_subkeys("HKCU", &base) {
            let sub = format!(r"{}\{}", base, k);
            let c = regop::get_str("HKCU", &sub, "CLSID");
            let p = regop::get_str("HKCU", &sub, "Profile");
            if let (Some(c), Some(p)) = (c, p) {
                if !c.is_empty() && !p.is_empty() && !eq_ci(&c, CLSID) {
                    push_unique(&mut r, format!("0804:{}{}", c, p));
                }
            }
        }
    }
    r
}

// ═══════════════════ 主流程：install 子命令（install.ps1 L229-556）═══════════════════

pub fn run_install(out: &mut Out, no_hklm: bool, user_only: bool) -> i32 {
    // install.ps1 L20-39：路径常量（原地安装：$src=$inst=脚本/exe 所在目录；$data L24 定义
    // 后未使用，无行为）。$dll32/$sysdll32 全路径定义（三十四修补，主流程也要用）。
    let inst = common::install_dir();
    let inst_s = path_str(&inst);
    let dll = inst.join("hufu_tsf.dll");
    let dll_s = path_str(&dll);
    let exe = inst.join("hufu-server.exe");
    let exe_s = path_str(&exe);
    let icon = inst.join("图标.ico");
    let icon_s = path_str(&icon);
    let sysdll = common::sysdll_x64();
    let sysdll_s = path_str(&sysdll);

    // install.ps1 L41-46：升级检测——本输入法 TIP 键（HKCU 或 HKLM）已存在 = 升级安装
    // （虎爪保护：升级跳过 ctfmon 杀启；必须在任何注册表写入之前判定）。
    let tip_already = regop::key_exists("HKCU", &format!(r"Software\Microsoft\CTF\TIP\{}", CLSID))
        || regop::key_exists("HKLM", &format!(r"SOFTWARE\Microsoft\CTF\TIP\{}", CLSID));

    // install.ps1 L230-235：权限判定（IsInRole Administrator / whoami /groups 含 S-1-5-32-544）
    let is_admin = common::is_admin();
    let in_admin_group = common::in_admin_group();

    // install.ps1 L237-239：横幅（空行照排）
    out.host_only("");
    out.line("HuFu 虎符输入法 安装");
    out.line(&format!("安装目录（原地运行）: {}", inst_s));

    // install.ps1 L241-246：0) 安装位置安全性检查（-like "$env:TEMP*" 前缀匹配，不分大小写）
    let temp_s = path_str(&common::temp_dir());
    if inst_s.to_lowercase().starts_with(&temp_s.to_lowercase()) {
        out.line("⚠ 当前位于临时目录（会被系统清理，输入法将失效）！");
        out.line("  请把整个文件夹移到稳定位置（如 D:\\HuFu）后重新运行本安装。");
        return 1;
    }

    // install.ps1 L247-252：旧布局检测（%LOCALAPPDATA%\HuFu\hufu-server.exe 存在 → 报体积；
    // $mb 供尾横幅 L553 复用）
    let legacy = common::localappdata().join("HuFu");
    let mut legacy_mb: Option<u64> = None;
    if legacy.join("hufu-server.exe").exists() {
        let mb = (dir_file_sum(&legacy) as f64 / (1024.0 * 1024.0)).round() as u64;
        legacy_mb = Some(mb);
        out.line(&format!(
            "· 检测到旧版安装目录：{}（约 {}MB）",
            path_str(&legacy),
            mb
        ));
        out.line("  本次安装完成后确认输入法正常，即可删除该目录释放空间。");
    }

    // install.ps1 L254-257：1) 文件检查（原地运行：不拷贝；缺件 exit 1）
    if !dll.exists() {
        out.line(&format!("✗ 缺少 {}，请完整解压安装包后重试", dll_s));
        return 1;
    }
    if !exe.exists() {
        out.line(&format!("✗ 缺少 {}，请完整解压安装包后重试", exe_s));
        return 1;
    }
    out.line("OK 文件就位（原地运行，不占用 C 盘额外空间）");

    // install.ps1 L325-332：-UserOnly 分支——只写当前用户 HKCU IME 配置后退出
    // （不动 HKLM/msctf/语言列表/自启/server；多用户机器新用户免提权开通）
    if user_only {
        if !install_user_config(out) {
            return 1;
        }
        out.line("OK -UserOnly 完成：仅当前用户 HKCU 配置已写入（HKLM/msctf/server 未动）");
        return 0;
    }

    // install.ps1 L334：默认主流程依次调用 Install-UserConfig（行为与拆分前一致）
    if !install_user_config(out) {
        return 1;
    }

    // install.ps1 L336-381：3) 提权注册（HKLM + msctf；一次 UAC，日志回流本窗口）
    if !no_hklm {
        if is_admin {
            // install.ps1 L338-344：管理员直跑提权子命令（& 调用同窗口；Rust 侧子进程输出经
            // main.rs 默认 --log=%TEMP%\hufu-install-elevated.log 留痕）。PS $? 判定 → 非 0 即告警。
            let self_exe = std::env::current_exe().unwrap_or_default();
            match procop::spawn_wait_hidden(&self_exe, "install-elevated", 600_000) {
                SpawnResult::Exit(0) => {}
                _ => out.line("⚠ 提权阶段报告失败（msctf 登记可能未完成）"),
            }
        } else if in_admin_group {
            // install.ps1 L345-356：可提权 → 弹 UAC 跑子命令，捕获退出码（不中断主流程），
            // 提权日志按 UTF-8 读回逐行回显（前缀两空格）
            out.line("（弹出 UAC：机器级注册 + msctf 登记，请点「是」）");
            let elog = common::elevated_log();
            let _ = std::fs::remove_file(&elog); // 先删旧日志（PS *> 重定向=覆盖语义）
            let self_exe = std::env::current_exe().unwrap_or_default();
            match procop::spawn_runas_wait(&self_exe, "install-elevated", 600_000) {
                SpawnResult::Exit(0) => {}
                SpawnResult::Exit(c) => {
                    out.line(&format!("⚠ 提权阶段退出码 {}（msctf 登记可能未完成）", c))
                }
                // UAC 被拒/超时：PS 里 $evProc 为 null，退出码插值为空串——同款渲染
                _ => out.line("⚠ 提权阶段退出码 （msctf 登记可能未完成）"),
            }
            if elog.exists() {
                if let Ok(txt) = std::fs::read_to_string(&elog) {
                    for l in txt.lines() {
                        out.line(&format!("  {}", l));
                    }
                }
            }
            // install.ps1 L357-363：【顺序铁律·校验半边】提权段已回写 HKCU→SystemIME；
            // 此处回读双保险。注意：PS「原值」取的是 SetValue 之后的回读（照抄该行为，
            // 标签写「原值」、实为新值）。
            let ips_cu = format!(r"Software\Classes\CLSID\{}\InprocServer32", CLSID);
            if regop::key_exists("HKCU", &ips_cu) {
                let cur = regop::get_str("HKCU", &ips_cu, "(default)");
                if !matches!(&cur, Some(v) if eq_ci(v, &sysdll_s)) {
                    regop::set_str("HKCU", &ips_cu, "(default)", &sysdll_s);
                    let nowv = regop::get_str("HKCU", &ips_cu, "(default)").unwrap_or_default();
                    out.line(&format!("  · HKCU COM 校正 → SystemIME（原值 {}）", nowv));
                }
            }
        } else {
            // install.ps1 L364-369：无管理员权限（标准用户）——文件与语言列表铺好，
            // msctf 登记尽力而为（smoke reg $icon）
            out.line("⚠ 无管理员权限：msctf 输入法注册需机器级写入（TSF 平台限制，");
            out.line("  同类输入法如虎爪同样要求管理员）。文件与语言列表已铺好，");
            out.line("  但输入法要能用，需以管理员身份重跑本安装器完成登记。");
            let _ = invoke_smoke_reg(out, &icon_s);
        }
    } else {
        // install.ps1 L370-381：-NoHKLM 分支——按机器级底座是否已建给出每用户安装说明，
        // msctf 登记尽力而为（smoke reg $icon）
        let has_base = sysdll.exists()
            && regop::key_exists("HKLM", &format!(r"SOFTWARE\Microsoft\CTF\TIP\{}", CLSID));
        if has_base {
            out.line("· 每用户安装（本机已有机器级底座）：功能完整可用。");
        } else {
            out.line("· -NoHKLM 且本机无机器级底座：输入法将不可用。全新机器");
            out.line("  首次安装请不带 -NoHKLM 运行（一次 UAC 完成机器级注册）。");
        }
        let _ = invoke_smoke_reg(out, &icon_s);
    }

    // install.ps1 L383-401：纯注释段（结构根治设计说明，无行为）。
    // install.ps1 L402-428：语言列表快照（装前只读留证，绝不作为重写原料）
    let tip = common::tip_str(); // "0804:{CLSID}{PROFILE}"
    let up_before = get_up_others(&tip);
    let asm_before = get_asm_others();

    // install.ps1 L429-453：① 加入语言列表——调 CTF 原生 API（smoke enable →
    // EnableLanguageProfile，只启用自己的 profile，物理上碰不到其他输入法）。
    // 失败不裸写注册表兜底（裸写条目会被 msctf 一致性校验判非法清除）。
    let smoke_exe = inst.join("hufu-tsf-smoke.exe");
    let mut enable_ok = false;
    if smoke_exe.exists() {
        let tmp_out = common::temp_dir().join("hufu-enable-out.txt");
        let _ = std::fs::remove_file(&tmp_out); // 先删旧输出文件（-RedirectStandardOutput 建新文件）
        let res = procop::spawn_capture(&smoke_exe, "enable", 60_000, &tmp_out);
        std::thread::sleep(Duration::from_millis(500)); // L440
        let det = std::fs::read_to_string(&tmp_out).unwrap_or_default(); // UTF-8 读回
                                                                         // L443-444 判据：输出含成功标记，或（已退出且 exit==0 且输出非空且不含「失败」）
        let hit = det.contains("OK 虎符已加入语言列表")
            || (matches!(res, SpawnResult::Exit(0)) && !det.is_empty() && !det.contains("失败"));
        if hit {
            enable_ok = true;
            out.line("OK 已加入语言列表（CTF 原生 EnableLanguageProfile：他人条目零触碰）");
        } else {
            // install.ps1 L448：install.log 留证（PS 仅在进程已退出取得到 ExitCode 时落此行）
            if let SpawnResult::Exit(c) = res {
                out.install_log(&format!("install enable 失败: exit={} {}", c, det));
            }
            out.line("⚠ 自动加入语言列表失败——请到 设置→时间和语言→语言→中文→选项 手动添加一次「HuFu 虎符输入法」（注册已就绪，添加一次即永久）");
        }
    } else {
        // install.ps1 L451-452：缺 smoke exe
        out.line("⚠ 缺 hufu-tsf-smoke.exe——请到 设置→时间和语言→语言→中文→选项 手动添加一次「HuFu 虎符输入法」");
    }

    // install.ps1 L454-461：② ctfmon 未运行则冷启动（CTF 从注册表真存储重建视图；
    // $ctfStartedEarly 供第 6 段刷新判断复用）
    let mut ctf_started_early = false;
    if !procop::proc_running("ctfmon.exe") {
        procop::spawn_detached("ctfmon.exe", "");
        std::thread::sleep(Duration::from_secs(2));
        ctf_started_early = true;
    }

    // install.ps1 L462-479：③ 末态核验（注册表对注册表回读）+ install.log 留证
    let up_after = get_up_others(&tip);
    let asm_after = get_asm_others();
    let others_total = unique_union(&up_before, &asm_before);
    let gone: Vec<String> = others_total
        .iter()
        .filter(|s| {
            !up_after.iter().any(|x| eq_ci(x.as_str(), s.as_str()))
                && !asm_after.iter().any(|x| eq_ci(x.as_str(), s.as_str()))
        })
        .cloned()
        .collect();
    let mut me_in = false;
    std::thread::sleep(Duration::from_millis(800)); // L469：CTF 落注册表为异步，稍候再读
    let up_root = r"Control Panel\International\User Profile";
    for lk in regop::enum_subkeys("HKCU", up_root) {
        if starts_with_ci(&lk, "zh") {
            // L471：仅 zh* 语言子键里查我们 tip_str 是否在场
            for n in regop::enum_value_names("HKCU", &format!(r"{}\{}", up_root, lk)) {
                if eq_ci(&n, &tip) {
                    me_in = true;
                }
            }
        }
    }
    // install.ps1 L475-479：install.log 格式串逐字段对齐（布尔渲染 True/False）
    let after_total = unique_union(&up_after, &asm_after);
    out.install_log(&format!(
        "install CTF原生启用: enable={} 他人快照={}（真存储{}+装配表{}） 末态他人={} 我们在场={} 丢失=[{}] ctfmon预热={}",
        bool_ps(enable_ok),
        others_total.len(),
        up_before.len(),
        asm_before.len(),
        after_total.len(),
        bool_ps(me_in),
        gone.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(","),
        bool_ps(ctf_started_early)
    ));
    // install.ps1 L480-485：核验结论（告警只为极端并发修改留证）
    if !gone.is_empty() {
        out.line(&format!(
            "⚠ 核验发现既有条目变化（原生启用下不应发生，详见 install.log）：{}",
            gone.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    } else {
        out.line(&format!(
            "OK 语言列表处理完成（他人快照 {} 条全数在场，0 缺失）",
            others_total.len()
        ));
    }

    // install.ps1 L486-492：默认首选输入法——显式设覆盖=虎符（Set-WinDefaultInputMethodOverride
    // = HKCU `Control Panel\International\User Profile` 值 InputMethodOverride）
    if regop::set_str(
        "HKCU",
        r"Control Panel\International\User Profile",
        "InputMethodOverride",
        &tip,
    ) {
        out.line("OK 默认输入法覆盖 = 虎符（重启/新会话默认首选）");
    } else {
        out.line("· 默认输入法覆盖写入失败（不影响列表首位默认）");
    }
    // install.ps1 L493
    out.line("OK 语言列表 + 切换器装配已写入");

    // install.ps1 L495-508：5) 开机自启（Run 键）+ 开始菜单快捷方式
    regop::set_str(
        "HKCU",
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        "HuFu",
        &format!("\"{}\"", exe_s),
    );
    // L498-504：WScript.Shell URL 快捷方式 → %APPDATA%\…\Programs（GetFolderPath('Programs')）
    let lnk_ok = (|| -> Option<bool> {
        let appdata = std::env::var_os("APPDATA")?;
        let sm = PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs");
        let lnk_path = sm.join("HuFu 虎符输入法设置.lnk");
        Some(lnk::create_url_shortcut(
            &lnk_path,
            "http://127.0.0.1:4390/",
            &icon,
        ))
    })()
    .unwrap_or(false);
    if lnk_ok {
        out.line("OK 开机自启 + 开始菜单快捷方式「HuFu 虎符输入法设置」");
    } else {
        out.line("OK 开机自启已设置（开始菜单快捷方式失败，可忽略）");
    }

    // install.ps1 L510-519：6) 启动 server——【铁律】必须普通权限：提权启动的管道会拒绝
    // 普通应用。先杀旧实例 → 500ms → 启动 → 2s。
    procop::kill_by_name("hufu-server.exe");
    std::thread::sleep(Duration::from_millis(500));
    if is_admin {
        // 本安装器自身被提权运行（如右键管理员）：经 explorer.exe 中转降权启动（参数带引号）
        procop::spawn_detached("explorer.exe", &format!("\"{}\"", exe_s));
    } else {
        // 普通权限：火忘启动（CREATE_NO_WINDOW 不等待——server 常驻，绝不能 wait）
        procop::spawn_detached(&exe_s, "");
    }
    std::thread::sleep(Duration::from_secs(2));

    // install.ps1 L520-530：纯注释段（ctfmon 刷新设计说明，无行为）。
    // install.ps1 L531-539：ctfmon 三分支（虎爪保护二阶：升级安装完全跳过刷新；
    // 本安装内已冷启动过也不再重复杀启——每次重启都是一次 msctf 一致性校验风险）
    if ctf_started_early {
        out.line("OK ctfmon 已在本安装内冷启动（语言列表读取前），跳过刷新");
    } else if !tip_already {
        procop::kill_by_name("ctfmon.exe");
        std::thread::sleep(Duration::from_secs(1));
        procop::spawn_detached("ctfmon.exe", "");
    } else {
        out.line("OK 升级安装：跳过 ctfmon 刷新（虎爪保护；新文件随应用重开生效）");
    }

    // install.ps1 L541-556：完成横幅（'=' 行 42 字符、空行照排）
    out.host_only("");
    out.line("==========================================");
    out.line(" 安装完成！");
    out.line("  · Win+空格 切到「HuFu 虎符输入法」");
    out.line("  · 设置：Ctrl+Alt+H（已开则弹到最前）/ Ctrl+Shift+H 呼出到最前 / 开始菜单");
    out.line("     搜「HuFu」或双击「设置.bat」");
    out.line("  · 绿色模式：程序在本目录原地运行，不占 C 盘；");
    out.line("    不要移动/删除本文件夹（输入法依赖它）");
    out.line("  · 卸载：运行「卸载.bat」后把本文件夹整个删除即可");
    out.line("==========================================");
    out.line("  · 无需重启/注销；正在运行的应用重开后才加载新输入法");
    // install.ps1 L552-554：旧版目录提醒（复检存在性 + 开头算好的 $mb）
    if legacy.join("hufu-server.exe").exists() {
        let mb_str = match legacy_mb {
            Some(v) => format!("{}MB", v),
            None => String::new(), // PS：$mb 未定义时插值为空
        };
        out.line(&format!(
            "  · 记得删除旧版目录释放 {}：{}",
            mb_str,
            path_str(&legacy)
        ));
    }
    // install.ps1 L555：$perUserFull 赋值后未再使用（无可见行为，不翻译为代码）。
    // install.ps1 L556
    if !sysdll.exists() {
        out.line("  · 本机未做机器级注册：开始菜单/UWP 暂不可用（下次以管理员运行安装器一次即可）");
    }
    0
}

// ═══════════════════ 提权子阶段：install-elevated 子命令（install.ps1 L85-228）═══════════════════
// 只做 HKLM 注册 + msctf 登记，绝不启动 server。

pub fn run_phase_elevated(out: &mut Out) -> i32 {
    let inst = common::install_dir();
    let dll = inst.join("hufu_tsf.dll"); // L91/L107 源（PSScriptRoot 同目录）
    let dll32 = inst.join("hufu_tsf32.dll");
    let sysdir = common::sysdir_x64();
    let sysdll = common::sysdll_x64();
    let sysdll_s = path_str(&sysdll);

    // install.ps1 L86-87：横幅 + SystemIME 建目录（打包进程可读的系统输入法同款目录）
    out.line("—— 提权阶段：SystemIME 副本（打包进程可读）——");
    let _ = std::fs::create_dir_all(&sysdir);

    // install.ps1 L88-102：拷贝 x64 DLL（被宿主进程加载锁住 → .oldN 腾位 fallback）+ ACL
    copy_dll_with_old_fallback(out, &dll, &sysdll, true);
    // install.ps1 L99：拷贝失败 → throw（终止提权阶段，退出码 1；文案照 throw 消息）
    if !sysdll.exists() {
        out.line(&format!("SystemIME DLL 拷贝失败：{}", sysdll_s));
        return 1;
    }
    // install.ps1 L100-101：icacls（目录 (OI)(CI)RX / 文件 RX；失败不中断）
    let _ = acl::grant_all_app_packages(&sysdir, true);
    let _ = acl::grant_all_app_packages(&sysdll, false);
    // install.ps1 L102
    out.line("OK DLL → SystemIME");

    // install.ps1 L103-129：── 32 位宿主支持（WoW64：跟打器等 32 位进程）──
    // 32 位进程 COM 查找：HKCU（无重定向）失败后回退 HKLM 32 位视图（WOW6432Node）→
    // 命中 SysWOW64\SystemIME 的 32 位 DLL。
    if dll32.exists() {
        let dir32 = common::sysdir_x86();
        let _ = std::fs::create_dir_all(&dir32);
        let sysdll32 = common::sysdll_x86();
        copy_dll_with_old_fallback(out, &dll32, &sysdll32, false); // 此段腾位无播报（ps1 无 Write-Host）
        let _ = acl::grant_all_app_packages(&dir32, true);
        let _ = acl::grant_all_app_packages(&sysdll32, false);
        let wow = format!(r"SOFTWARE\Classes\WOW6432Node\CLSID\{}", CLSID);
        regop::set_str("HKLM", &wow, "(default)", "HuFu TSF Service");
        regop::set_str(
            "HKLM",
            &format!(r"{}\InprocServer32", wow),
            "(default)",
            &path_str(&sysdll32),
        );
        regop::set_str(
            "HKLM",
            &format!(r"{}\InprocServer32", wow),
            "ThreadingModel",
            "Apartment",
        );
        // install.ps1 L126
        out.line("OK 32 位 DLL → SysWOW64（跟打器等 32 位宿主可用）");
    } else {
        // install.ps1 L127-129：黄字提示跳过
        out.line("（未找到 hufu_tsf32.dll，跳过 32 位支持）");
    }

    // install.ps1 L130-150：── 升级清理 1) 历史腾位残留（HuFu.oldN 目录、HuFu\*.old* 文件）──
    // 本次 DLL 已就位；旧的若仍被运行中的应用占用则跳过（catch 静默，下次安装/重启后再清）。
    let ime_bases = [
        PathBuf::from(r"C:\Windows\SystemIME"),
        PathBuf::from(r"C:\Windows\SysWOW64\SystemIME"),
    ];
    for base in &ime_bases {
        if let Ok(rd) = std::fs::read_dir(base) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if starts_with_ci(&name, "HuFu.old") {
                    let full = e.path();
                    normalize_tree_attrs(&full); // 先归一化属性再递归删
                    if remove_entry(&full).is_ok() {
                        out.line(&format!("清理腾位残留: {}", path_str(&full)));
                    }
                }
            }
        }
        let hu = base.join("HuFu");
        if hu.exists() {
            if let Ok(rd) = std::fs::read_dir(&hu) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.to_lowercase().contains(".old") {
                        if remove_entry(&e.path()).is_ok() {
                            out.line(&format!("清理旧副本: {}", name));
                        }
                    }
                }
            }
        }
    }

    // install.ps1 L151-160：── 升级清理 2) 诊断日志（ProgramData\HuFu\diag，升级即清）──
    let diag = common::programdata().join("HuFu").join("diag");
    if diag.exists() {
        let n = count_entries(&diag);
        if let Ok(rd) = std::fs::read_dir(&diag) {
            for e in rd.flatten() {
                let _ = remove_entry(&e.path()); // 尽力删，失败静默
            }
        }
        if n > 0 {
            out.line(&format!("清理诊断日志: {} 个", n));
        }
    }

    // install.ps1 L161-170：── 升级清理 3) 早期版本残留 %LOCALAPPDATA%\HuFu（绿色化前旧版
    // 数据目录；被占用则提示下次再清）──
    let legacy_local = common::localappdata().join("HuFu");
    if legacy_local.exists() {
        normalize_tree_attrs(&legacy_local);
        if remove_entry(&legacy_local).is_ok() {
            out.line("清理早期版本数据: %LOCALAPPDATA%\\HuFu");
        } else {
            out.line("· %LOCALAPPDATA%\\HuFu 被占用，下次安装再清");
        }
    }

    // install.ps1 L171-177：── HKLM 机器级注册：COM InprocServer32 + TIP 描述 ──
    out.line("—— 提权阶段：HKLM 机器级注册 ——");
    let clsid_lm = format!(r"SOFTWARE\Classes\CLSID\{}", CLSID);
    let ips = format!(r"{}\InprocServer32", clsid_lm);
    regop::set_str("HKLM", &clsid_lm, "(default)", "HuFu TSF Service");
    regop::set_str("HKLM", &ips, "(default)", &sysdll_s);
    regop::set_str("HKLM", &ips, "ThreadingModel", "Apartment");
    let tip_lm = format!(r"SOFTWARE\Microsoft\CTF\TIP\{}", CLSID);
    regop::set_str(
        "HKLM",
        &format!(r"{}\Description", tip_lm),
        "(default)",
        "HuFu 虎符输入法（虎码）",
    );

    // install.ps1 L178-199：TIP 全套分类（8 项，虎爪/主流 IME 同款）——双层写入：全局分类库
    // + TIP 树，两层均需齐全（只注册一两个分类时 Win+空格 会跳过本输入法）。
    let lm_cat = r"SOFTWARE\Microsoft\CTF\Category";
    for c in TIP_CATS.iter() {
        let _ = regop::ensure_key(
            "HKLM",
            &format!(r"{}\Category\Category\{}\{}", tip_lm, c, CLSID),
        );
        let _ = regop::ensure_key(
            "HKLM",
            &format!(r"{}\Category\Item\{}\{}", tip_lm, CLSID, c),
        );
        let _ = regop::ensure_key("HKLM", &format!(r"{}\Category\{}\{}", lm_cat, c, CLSID));
        let _ = regop::ensure_key("HKLM", &format!(r"{}\Item\{}\{}", lm_cat, CLSID, c));
    }
    out.line("OK TIP 分类 8 项已齐全（全局库 + TIP 树双层）");

    // install.ps1 L200-206：TIP LanguageProfile 五值（Enable/IconIndex 为 DWORD）
    let lp = format!(r"{}\LanguageProfile\0x00000804\{}", tip_lm, PROFILE);
    regop::set_str("HKLM", &lp, "Description", "HuFu 虎符输入法");
    regop::set_str("HKLM", &lp, "Display Description", "HuFu 虎符输入法");
    regop::set_dword("HKLM", &lp, "Enable", 1);
    regop::set_str("HKLM", &lp, "IconFile", &sysdll_s);
    regop::set_dword("HKLM", &lp, "IconIndex", 0);
    out.line("OK HKLM 机器级已注册（指向 SystemIME）");

    // install.ps1 L207-212：── msctf 原生登记（Invoke-SmokeReg $sysdll，60 秒超时，
    // 退出码捕获传播）──
    out.line("—— 提权阶段：msctf 原生登记 ——");
    let rc_smoke = invoke_smoke_reg(out, &sysdll_s);

    // install.ps1 L213-223：【顺序铁律·回写半边】每用户段先写了 HKCU→安装目录 DLL（当时
    // SystemIME 尚未建立）；此刻 SystemIME 已就位，HKCU COM 必须回写为 SystemIME 路径，
    // 否则打包进程（开始菜单/UWP）按 HKCU 优先解析到用户目录 DLL → 加载失败。
    let ips_cu = format!(r"Software\Classes\CLSID\{}\InprocServer32", CLSID);
    regop::set_str("HKCU", &ips_cu, "(default)", &sysdll_s);
    // install.ps1 L221-223：回读校验（读不到/不等 → ⚠ 带当前值；PS null 插值为空串同款）
    let chk_cu = regop::get_str("HKCU", &ips_cu, "(default)");
    if !matches!(&chk_cu, Some(v) if eq_ci(v, &sysdll_s)) {
        out.line(&format!(
            "⚠ HKCU 回写校验失败：{}",
            chk_cu.unwrap_or_default()
        ));
    } else {
        out.line("OK HKCU COM 已回写 → SystemIME（打包进程可读）");
    }

    // install.ps1 L224
    out.line("提权阶段完成。");

    // install.ps1 L225-227：退出码传播——smoke 登记失败 → ⚠ + 非 0 退出；成功 → 0
    if rc_smoke != 0 {
        out.line(&format!("⚠ msctf 登记退出码 {}", rc_smoke));
        return rc_smoke;
    }
    0
}
