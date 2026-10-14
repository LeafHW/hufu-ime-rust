//! postboot 子命令：tools.ps1 Invoke-PostBootCmd（L512-551）血律翻译。
//! 契约：恒静默恒 0（hufu-postboot.ps1 薄壳语义；main.rs 同款强制 0），
//! 所有异常吞掉，绝不非零退出。

use crate::common::{install_dir, Out};
use crate::uninstall::{remove_file_forced, remove_forced, sys_bases};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn run_postboot(out: &mut Out) -> i32 {
    // tools.ps1 L512-516：-InstDir 缺失分支在引擎形态不可达（InstDir=exe 所在
    // 目录恒有值），不触发；按契约仍恒 0。
    let inst = install_dir();

    // tools.ps1 L517-530：安装目录 *.old* 文件 5 轮 × 1s 重试删
    //（每轮枚举→删→数剩余，剩 0 早停）
    let mut deleted = 0usize;
    let mut remain = 0usize;
    for _attempt in 1..=5 {
        let olds = old_star_files(&inst); // -Filter '*.old*' -File
        if olds.is_empty() {
            break;
        }
        for f in olds {
            // Remove-Item -Force -ErrorAction Stop + catch{}：失败静默吞
            if remove_file_forced(&f) {
                deleted += 1;
            }
        }
        remain = old_star_files(&inst).len();
        if remain == 0 {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }

    // tools.ps1 L531-548：SystemIME 尽力删（普通权限失败静默；不打扰开机）
    let mut sys_del = 0usize;
    let mut sys_remain = 0usize;
    for base in sys_bases() {
        if !base.exists() {
            continue;
        }
        let mut items: Vec<PathBuf> = Vec::new();
        // tools.ps1 L535-536：-like 'HuFu.old*'（目录/文件均可命中）
        if let Ok(rd) = std::fs::read_dir(&base) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if name.starts_with("hufu.old") {
                    items.push(e.path());
                }
            }
        }
        // tools.ps1 L537：HuFu\*.old* 文件（-Filter '*.old*' -File）
        if let Ok(rd) = std::fs::read_dir(base.join("HuFu")) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if name.contains(".old") && e.path().is_file() {
                    items.push(e.path());
                }
            }
        }
        // tools.ps1 L538-547：目录先递归归一化属性再 -Recurse -Force 删；
        // 成功 sysDel++，失败 sysRemain++（catch 分支）
        for it in items {
            if remove_forced(&it) {
                sys_del += 1;
            } else {
                sys_remain += 1;
            }
        }
    }

    // tools.ps1 L549：RESULT 总结行（格式逐字节）
    out.line(&format!(
        "RESULT: PostBoot 安装目录 .old* 删 {} 剩 {}；SystemIME 尽力删 {} 剩 {}；exit=0",
        deleted, remain, sys_del, sys_remain
    ));
    0
}

/// tools.ps1 L519 / L527：目录顶层 *.old* 文件（-Filter '*.old*' -File，
/// PS Filter 不区分大小写 → 小写化后按 contains(".old") 匹配）
fn old_star_files(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.contains(".old") && e.path().is_file() {
                v.push(e.path());
            }
        }
    }
    v
}
