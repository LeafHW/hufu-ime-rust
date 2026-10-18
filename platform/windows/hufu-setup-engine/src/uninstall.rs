//! uninstall 子命令：uninstall.ps1（206 行）+ tools.ps1 尾巴契约（PFRO 登记 /
//! HKLM 残留校验 exit=4 / ProgramData 重试）的逐行血律翻译。
//! 顺序铁律（CONTRACTS.md）：列表纯减法 → 杀 server+ctfmon → smoke unreg →
//! Run 键 → HKCU 清 → lnk → legacy → HKLM 清 → SystemIME（锁则改名 HuFu.oldN）
//! → PFRO → 校验 → 启 ctfmon → 数据保留指引文案。
//! 本文件同时承载 postboot.rs / purge.rs 复用的文件系统小件（属性归一化、
//! -Force 删除、递归收集）。

use crate::common::{
    in_admin_group, install_dir, is_admin, localappdata, programdata, sysdir_x64, sysdir_x86,
    temp_dir, tip_str, Out, ASM_KBD_CAT, CLSID, TFCAT_KBD, TIP_CATS,
};
use crate::procop::{
    kill_by_name, spawn_detached, spawn_runas_wait, spawn_wait_hidden, SpawnResult,
};
use crate::regop;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 语言列表真存储根键（uninstall.ps1 L42 / L57）
const UP_ROOT: &str = r"Control Panel\International\User Profile";

pub fn run_uninstall(out: &mut Out, no_hklm: bool) -> i32 {
    // ── uninstall.ps1 L15-21：$isAdmin / $inAdminGroup / $hklm = $isAdmin ──
    let admin = is_admin();
    let hklm = admin;

    // ── uninstall.ps1 L22-29：自提权——非 admin 且非 -NoHKLM 且在管理员组 →
    //    以 runas 重启自身跑完整卸载（"uninstall" 无参），等待并透传子退出码 ──
    //    【--log 转发 2026-10-14】裸引擎调用（非 iss 链路）时把引擎日志路径
    //    一并带给提权子进程——否则提权段输出无处落（iss 链路 ShellExec 已
    //    自带 --log，转发后同为该文件，双保险）。
    if !admin && !no_hklm && in_admin_group() {
        let exe =
            std::env::current_exe().unwrap_or_else(|_| PathBuf::from("hufu-setup-engine.exe"));
        let args = match &out.logfile {
            Some(p) => format!("uninstall --log \"{}\"", p.display()),
            None => "uninstall".to_string(),
        };
        return match spawn_runas_wait(&exe, &args, 600_000) {
            SpawnResult::Exit(code) => code,
            // PS 语义：Start-Process -Wait 之后无参 exit（=0）；
            // UAC 拒（Fail）/超时（Timeout）没有子退出码，按同款 0 处理
            _ => 0,
        };
    }

    // ── uninstall.ps1 L31：$inst = 脚本所在目录（引擎形态 = exe 所在目录）──
    let inst = install_dir();
    // ── uninstall.ps1 L38：$tipStr = "0804:$CLSID$PROFILE" ──
    let tip = tip_str();
    // ── uninstall.ps1 L39：切换器装配表基键 AssemblyItem\0x00000804\{键盘分类} ──
    let asm = format!(
        r"Software\Microsoft\CTF\SortOrder\AssemblyItem\0x00000804\{}",
        ASM_KBD_CAT
    );

    // ═ 1) 语言列表移除——纯减法（uninstall.ps1 L33-68）══════════════════
    // ── uninstall.ps1 L40-48：① 列表真存储——zh* 语言键下删自己的值
    //    （幂等：无值=跳过；他人条目零触碰）──
    for lk in regop::enum_subkeys("HKCU", UP_ROOT) {
        if !lk.to_lowercase().starts_with("zh") {
            continue; // PSChildName -like 'zh*'（PS -like 不区分大小写）
        }
        let child = format!(r"{}\{}", UP_ROOT, lk);
        if regop::enum_value_names("HKCU", &child)
            .iter()
            .any(|v| v == &tip)
        {
            regop::delete_value("HKCU", &child, &tip);
            out.line("OK 列表真存储 -虎符（纯减法：他人条目零触碰）");
        }
    }

    // ── uninstall.ps1 L49-53：② 切换器装配表——按 CLSID 认领删自己的槽 ──
    let mut slots_gone = 0usize;
    for sub in asm_owned_slots(&asm) {
        regop::delete_tree("HKCU", &format!(r"{}\{}", asm, sub));
        slots_gone += 1; // PS 计数与 Remove-Item 成败无关，照抄
    }

    // ── uninstall.ps1 L54-61：③ 末态回读核验（事实源=注册表；残留必须为 0）──
    let left_up = up_keys_with_tip(&tip);
    let left_asm = asm_owned_slots(&asm).len();

    // ── uninstall.ps1 L62-65：install.log 落一行减法报告（格式串逐字节）──
    out.install_log(&format!(
        "uninstall 减法移除: 删槽={} 残留（真存储={}，装配表={}）——应全为 0",
        slots_gone, left_up, left_asm
    ));

    // ── uninstall.ps1 L66-68：残留 ⚠ 提示（黄字文案照抄）──
    if left_up > 0 || left_asm > 0 {
        out.line(&format!(
            "⚠ 虎符条目残留（真存储 {} / 装配表 {}），详见 install.log",
            left_up, left_asm
        ));
    }

    // ═ 0-moved) uninstall.ps1 L70-73：【防档案重建】列表动刀完毕才杀服务 ─═
    kill_by_name("hufu-server.exe");
    kill_by_name("ctfmon.exe");
    std::thread::sleep(Duration::from_secs(1));

    // ── uninstall.ps1 L75-79：msctf 原生档案注销（仅完整卸载（hklm）时做；
    //    每用户卸载不动 msctf 档案，防无提权重装后输入法失忆）──
    if hklm {
        let smoke = inst.join("hufu-tsf-smoke.exe");
        if smoke.is_file() {
            let _ = spawn_wait_hidden(&smoke, "unreg", 60_000);
        }
    }

    // ── uninstall.ps1 L82：3) 删自启 Run 键 ──
    regop::delete_value(
        "HKCU",
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        "HuFu",
    );

    // ═ 4) uninstall.ps1 L84-91：HKCU 注册表清理 ════════════════════════
    // ── uninstall.ps1 L85：HKCU TIP 树 ──
    regop::delete_tree("HKCU", &format!(r"Software\Microsoft\CTF\TIP\{}", CLSID));
    // ── uninstall.ps1 L86：HKCU Classes CLSID ──
    regop::delete_tree("HKCU", &format!(r"Software\Classes\CLSID\{}", CLSID));
    // 32 位 COM HKCU 双视图（lead 契约清单项；uninstall.ps1 HKCU 段无对应行，
    // 只删自有 CLSID 键，他人零触碰）
    regop::delete_tree(
        "HKCU",
        &format!(r"Software\Classes\WOW6432Node\CLSID\{}", CLSID),
    );
    // ── uninstall.ps1 L87：Software\HuFu（InstallDir 记录，原地安装模式）──
    regop::delete_tree("HKCU", r"Software\HuFu");
    // ── uninstall.ps1 L88-91：切换器装配表残余槽再扫（按 CLSID 认领）──
    for sub in asm_owned_slots(&asm) {
        regop::delete_tree("HKCU", &format!(r"{}\{}", asm, sub));
    }

    // ── uninstall.ps1 L92-95：开始菜单快捷方式（精确名 + *HuFu* 通配双保险）──
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let programs = PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs");
        remove_forced(&programs.join("HuFu 虎符输入法设置.lnk"));
        if let Ok(rd) = std::fs::read_dir(&programs) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if name.contains("hufu") {
                    remove_forced(&e.path()); // Remove-Item -Force -Recurse（文件/目录通吃）
                }
            }
        }
    }

    // ── uninstall.ps1 L96-102：【早期版本残留】%LOCALAPPDATA%\HuFu ──
    let legacy_local = localappdata().join("HuFu");
    if legacy_local.exists() {
        remove_forced(&legacy_local);
        if legacy_local.exists() {
            out.line("· %LOCALAPPDATA%\\HuFu 部分被占用，重启后再跑一次卸载可清");
        } else {
            out.line("已清早期版本数据: %LOCALAPPDATA%\\HuFu");
        }
    }

    // ═ 5) uninstall.ps1 L104-147：HKLM 清理（提权时；普通用户跳过——无害残留）═
    if hklm {
        // ── uninstall.ps1 L106-110：HKLM 主体 + 32 位 COM 双视图两处 ──
        regop::delete_tree("HKLM", &format!(r"SOFTWARE\Microsoft\CTF\TIP\{}", CLSID));
        regop::delete_tree("HKLM", &format!(r"SOFTWARE\Classes\CLSID\{}", CLSID));
        regop::delete_tree(
            "HKLM",
            &format!(r"SOFTWARE\WOW6432Node\Microsoft\CTF\TIP\{}", CLSID),
        );
        regop::delete_tree(
            "HKLM",
            &format!(r"SOFTWARE\Classes\WOW6432Node\CLSID\{}", CLSID),
        );

        // ── uninstall.ps1 L112-123：全局分类库条目（8 项 TIP 分类 + TFCAT_KBD
        //    共 9 GUID；Item\{CLSID} 整树删除连带覆盖 Item\{CLSID}\{g}）──
        for g in TIP_CATS.iter().copied().chain(std::iter::once(TFCAT_KBD)) {
            regop::delete_tree(
                "HKLM",
                &format!(r"SOFTWARE\Microsoft\CTF\Category\Category\{}\{}", g, CLSID),
            );
        }
        regop::delete_tree(
            "HKLM",
            &format!(r"SOFTWARE\Microsoft\CTF\Category\Item\{}", CLSID),
        );

        // ── uninstall.ps1 L124-139：【腾位链清除】SystemIME 两处 HuFu* 全清
        //    （属性归一化后整删；仍占用 → 整目录改名腾位 HuFu.oldN）──
        for base in sys_bases() {
            if let Ok(rd) = std::fs::read_dir(&base) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    if name.starts_with("hufu") {
                        // -like 'HuFu*'：HuFu 本体 + 全部 .oldN 历史
                        remove_forced(&e.path());
                    }
                }
            }
            let sysdir = base.join("HuFu");
            if sysdir.exists() {
                let mut n = 1u32;
                while base.join(format!("HuFu.old{}", n)).exists() {
                    n += 1;
                }
                let _ = std::fs::rename(&sysdir, base.join(format!("HuFu.old{}", n)));
                out.line(&format!(
                    "· DLL 被占用，SystemIME 已腾位 HuFu.old{}（重启后自动可删）",
                    n
                ));
            }
        }

        // ── uninstall.ps1 L140-146 + tools.ps1 L484-500：ProgramData\HuFu
        //    （诊断画像 load-*/act-*.txt；先递归归一化只读属性，
        //    重试 2 次隔 400ms）──
        let pdf_hu = programdata().join("HuFu");
        if pdf_hu.exists() {
            for attempt in 1..=3u32 {
                clear_readonly_recursive(&pdf_hu);
                let r = if pdf_hu.is_dir() {
                    std::fs::remove_dir_all(&pdf_hu)
                } else {
                    std::fs::remove_file(&pdf_hu)
                };
                match r {
                    Ok(()) => break,
                    Err(e) => {
                        out.line(&format!("⚠ ProgramData 删除第 {} 次失败: {}", attempt, e));
                        std::thread::sleep(Duration::from_millis(400));
                    }
                }
            }
        }
    }

    // ── uninstall.ps1 L148-149：临时安装/提权日志（%TEMP%\hufu-*.log）──
    if let Ok(rd) = std::fs::read_dir(temp_dir()) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.starts_with("hufu-") && name.ends_with(".log") {
                remove_forced(&e.path());
            }
        }
    }

    // ── uninstall.ps1 L151-154：6) HKCU TIP 二次复查（ctfmon 已杀，防外部重建）──
    if regop::key_exists("HKCU", &format!(r"Software\Microsoft\CTF\TIP\{}", CLSID)) {
        regop::delete_tree("HKCU", &format!(r"Software\Microsoft\CTF\TIP\{}", CLSID));
    }

    // ═ 8) uninstall.ps1 L161-175：【腾位残留二扫】能删则删（仅 hklm）════
    if hklm {
        let bases = sys_bases();
        for base in &bases {
            if let Ok(rd) = std::fs::read_dir(base) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    if name.starts_with("hufu.old") {
                        remove_forced(&e.path()); // -like 'HuFu.old*'
                    }
                }
            }
        }
        // ── uninstall.ps1 L173-174：x64 SystemIME 剩余占用项计数报告 ──
        let mut left_n = 0usize;
        if let Ok(rd) = std::fs::read_dir(&bases[0]) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if name.starts_with("hufu") {
                    left_n += 1; // -Filter 'HuFu*'（文件+目录）
                }
            }
        }
        if left_n > 0 {
            out.line(&format!(
                "· SystemIME 剩 {} 个占用项（重启后可删/下次安装自动清）",
                left_n
            ));
        }
    }

    // ═ tools.ps1 L418-436：PFRO 延迟删除登记（仅 hklm；ctfmon 拉起前做）══
    if hklm {
        let mut targets: Vec<PathBuf> = Vec::new();
        for base in sys_bases() {
            if !base.exists() {
                continue;
            }
            // tools.ps1 L422-428：HuFu.old* 目录内全部文件（PFRO 不删目录，只登记文件）
            if let Ok(rd) = std::fs::read_dir(&base) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    if name.starts_with("hufu.old") && e.path().is_dir() {
                        collect_files_recursive(&e.path(), &mut targets);
                    }
                }
            }
            // tools.ps1 L429-433：HuFu\*.old* 文件（顶层，-File）
            if let Ok(rd) = std::fs::read_dir(base.join("HuFu")) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    if name.contains(".old") && e.path().is_file() {
                        targets.push(e.path());
                    }
                }
            }
        }
        // tools.ps1 L435：Select-Object -Unique（保序去重）
        let mut seen = std::collections::HashSet::new();
        targets.retain(|t| seen.insert(t.clone()));
        // tools.ps1 L436：收集计数日志（格式逐字节）
        out.line(&format!(
            "PFRO: 收集待登记 {} 个文件（HuFu.old* 目录内文件 + HuFu\\*.old*）",
            targets.len()
        ));
        if targets.is_empty() {
            // tools.ps1 L463
            out.line("PFRO: 无待登记目标（SystemIME 已净）");
        } else {
            // tools.ps1 L440-441：逐个 MoveFileExW 延迟删除登记
            //（登记内核与表内校验/手写兜底属 pfro.rs 职责）
            for t in &targets {
                crate::pfro::schedule_delete(t);
            }
        }
    }

    // ═ tools.ps1 L466-482：HKLM 残留校验（四键；任一残留 → exit=4）══════
    let mut hklm_leftover = 0usize;
    if hklm {
        let tip_key = format!(r"SOFTWARE\Microsoft\CTF\TIP\{}", CLSID);
        let clsid_key = format!(r"SOFTWARE\Classes\CLSID\{}", CLSID);
        let wow_tip_key = format!(r"SOFTWARE\WOW6432Node\Microsoft\CTF\TIP\{}", CLSID);
        let wow_clsid_key = format!(r"SOFTWARE\Classes\WOW6432Node\CLSID\{}", CLSID);
        let checks = [
            (tip_key.clone(), format!("HKLM:\\{}", tip_key)),
            (clsid_key.clone(), format!("HKLM:\\{}", clsid_key)),
            (wow_tip_key.clone(), format!("HKLM:\\{}", wow_tip_key)),
            (wow_clsid_key.clone(), format!("HKLM:\\{}", wow_clsid_key)),
        ];
        for (reg_path, disp) in &checks {
            let has = regop::key_exists("HKLM", reg_path);
            if has {
                hklm_leftover += 1;
            }
            let (state, mark) = if has {
                ("残留!", "⚠")
            } else {
                ("已净", "·")
            };
            // tools.ps1 L481："HKLM 校验: $state  $c"（状态与路径间两个空格）
            out.line(&format!("{} HKLM 校验: {}  {}", mark, state, disp));
        }
    }

    // ═ 9) uninstall.ps1 L177-178：拉起 ctfmon（系统输入法框架恢复）═══════
    // 注：ps1 原文无条件执行；按 lead 契约指令仅完整卸载（hklm）模式照做。
    if hklm {
        spawn_detached("ctfmon.exe", "");
    }

    // ═ uninstall.ps1 L180-187：收尾文案 ═════════════════════════════════
    out.line("");
    out.line("OK 卸载完成（注册表+SystemIME+ProgramData+早期残留已清；输入法已移除）");
    if !hklm {
        out.line("  · 本次为每用户模式（-NoHKLM）：机器级注册未动。");
        out.line("    完整卸载请直接运行 卸载.bat（自动提权）。");
    }
    out.line("  · 腾位项（.oldN）重启后自动可删；下次安装也会自动清理");
    out.line("  · 无需重启/注销；已开应用里的输入法随应用关闭即消失");

    // ═ uninstall.ps1 L188-206：【数据保留指引】安装目录不删，去留由用户定 ═
    if !inst.as_os_str().is_empty() && inst.exists() {
        out.line("");
        out.line(&format!(
            "  数据已保留（未删除任何文件），安装目录：{}",
            inst.display()
        ));
        let mut data_tips: Vec<String> = Vec::new();
        // ── uninstall.ps1 L193：数据\config.json ──
        if inst.join(r"数据\config.json").exists() {
            data_tips.push("数据\\config.json（配置：方案/皮肤/习惯设置）".to_string());
        }
        // ── uninstall.ps1 L194-195：码表\*\用户调整.txt 递归计数 ──
        let mut mb_files = Vec::new();
        collect_files_recursive(&inst.join("码表"), &mut mb_files);
        let adj_n = mb_files
            .iter()
            .filter(|p| {
                p.file_name()
                    .map(|f| f.to_string_lossy() == "用户调整.txt")
                    .unwrap_or(false)
            })
            .count();
        if adj_n > 0 {
            data_tips.push(format!(
                "码表\\*\\用户调整.txt（词频学习记录，{} 个方案）",
                adj_n
            ));
        }
        // ── uninstall.ps1 L196-199：码表导出\ 递归文件计数 ──
        let exp_dir = inst.join("码表导出");
        if exp_dir.exists() {
            let mut exp_files = Vec::new();
            collect_files_recursive(&exp_dir, &mut exp_files);
            let exp_n = exp_files.len();
            if exp_n > 0 {
                data_tips.push(format!("码表导出\\（导出的码表快照，{} 个文件）", exp_n));
            }
        }
        // ── uninstall.ps1 L200-204 ──
        if !data_tips.is_empty() {
            out.line("  可存档的数据：");
            for t in &data_tips {
                out.line(&format!("    · {}", t));
            }
            out.line("  （重装同版后再放回原位即可恢复）");
        }
        // ── uninstall.ps1 L205 ──
        out.line(&format!(
            "  确认不要了，手动删整个文件夹即彻底清理：{}",
            inst.display()
        ));
    }

    // ═ tools.ps1 L502-509：退出码（HKLM 残留 → 4，否则 0）═══════════════
    if hklm_leftover > 0 {
        4
    } else {
        0
    }
}

// ═══════════ 模块内共享小件（postboot.rs / purge.rs 复用）═══════════════

/// SystemIME 两基目录（$env:SystemRoot\SystemIME、$env:SystemRoot\SysWOW64\SystemIME）。
/// sysdir_x64/x86 为常量绝对路径，parent 必在（明确不可达 unwrap）。
pub(crate) fn sys_bases() -> [PathBuf; 2] {
    [
        sysdir_x64().parent().unwrap().to_path_buf(),
        sysdir_x86().parent().unwrap().to_path_buf(),
    ]
}

/// uninstall.ps1 L55-60：zh* 语言键中仍含 tip 值的计数（末态回读）
fn up_keys_with_tip(tip: &str) -> usize {
    let mut n = 0usize;
    for lk in regop::enum_subkeys("HKCU", UP_ROOT) {
        if !lk.to_lowercase().starts_with("zh") {
            continue;
        }
        let child = format!(r"{}\{}", UP_ROOT, lk);
        if regop::enum_value_names("HKCU", &child)
            .iter()
            .any(|v| v == tip)
        {
            n += 1;
        }
    }
    n
}

/// uninstall.ps1 L51-53 / L61 / L89-91：装配表中 CLSID 归属我们的槽名列表
fn asm_owned_slots(asm: &str) -> Vec<String> {
    regop::enum_subkeys("HKCU", asm)
        .into_iter()
        .filter(|sub| {
            let child = format!(r"{}\{}", asm, sub);
            regop::get_str("HKCU", &child, "CLSID").as_deref() == Some(CLSID)
        })
        .collect()
}

/// 单项属性归一化（PS `$_.Attributes = 'Normal'` 的 std 等价：清只读位）
pub(crate) fn clear_readonly_self(p: &Path) {
    if let Ok(md) = std::fs::metadata(p) {
        let mut perm = md.permissions();
        if perm.readonly() {
            perm.set_readonly(false);
            let _ = std::fs::set_permissions(p, perm);
        }
    }
}

/// 递归归一化（含根本体；PS Get-ChildItem -Recurse -Force | % Attributes='Normal'）
pub(crate) fn clear_readonly_recursive(p: &Path) {
    clear_readonly_self(p);
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            clear_readonly_recursive(&e.path());
        }
    }
}

/// Remove-Item -Recurse -Force 等价：先归一化再整删（文件/目录通吃）
pub(crate) fn remove_forced(p: &Path) -> bool {
    if p.is_dir() {
        clear_readonly_recursive(p);
        std::fs::remove_dir_all(p).is_ok()
    } else if p.is_file() {
        clear_readonly_self(p);
        std::fs::remove_file(p).is_ok()
    } else {
        !p.exists()
    }
}

/// 单文件 Remove-Item -Force 等价
pub(crate) fn remove_file_forced(p: &Path) -> bool {
    if p.is_file() {
        clear_readonly_self(p);
        std::fs::remove_file(p).is_ok()
    } else {
        false
    }
}

/// 递归收集全部文件（PFRO 登记、卸载数据指引计数、purge/postboot 共用）
pub(crate) fn collect_files_recursive(p: &Path, files: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let ep = e.path();
            if ep.is_dir() {
                collect_files_recursive(&ep, files);
            } else {
                files.push(ep);
            }
        }
    }
}
