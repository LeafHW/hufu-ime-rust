//! purge-data 子命令：tools.ps1 Invoke-PurgeDataCmd（L553-591）血律翻译。
//! 数据铁律唯一例外：只有本子命令显式删除 码表/数据/模型/码表导出/升级备份。
//! 契约：恒 0。

use crate::common::{install_dir, Out};
use crate::uninstall::{clear_readonly_recursive, remove_file_forced};
use std::time::Duration;

pub fn run_purge_data(out: &mut Out) -> i32 {
    // tools.ps1 L553-557：-InstDir 缺失分支引擎形态不可达（InstDir=exe 所在
    // 目录恒有值），不触发；按契约仍恒 0。
    let inst = install_dir();

    // tools.ps1 L558：五个数据目录（顺序照抄）
    let dir_names = ["码表", "数据", "模型", "码表导出", "升级备份"];
    let mut dirs_gone = 0usize;
    let mut dirs_fail = 0usize;
    let mut files_gone = 0usize;
    let mut files_fail = 0usize;
    let mut self_skipped = false;

    // tools.ps1 L561-575：逐目录删除（先递归归一化属性；2 次 × 400ms 重试）
    for n in dir_names {
        let p = inst.join(n);
        if !p.exists() {
            continue;
        }
        let mut ok = false;
        for _attempt in 1..=2 {
            clear_readonly_recursive(&p);
            let r = if p.is_dir() {
                std::fs::remove_dir_all(&p)
            } else {
                std::fs::remove_file(&p)
            };
            match r {
                Ok(()) => {
                    ok = true;
                    break;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(400)),
            }
        }
        if ok {
            dirs_gone += 1;
        } else {
            dirs_fail += 1;
            out.line(&format!("⚠ PurgeData: 目录未能删除: {}", p.display()));
        }
    }

    // tools.ps1 L577-586：根下 install.log + setup-*.log
    if let Ok(rd) = std::fs::read_dir(&inst) {
        for e in rd.flatten() {
            if !e.path().is_file() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_lowercase();
            let is_log =
                name == "install.log" || (name.starts_with("setup-") && name.ends_with(".log"));
            if !is_log {
                continue;
            }
            // tools.ps1 L580-581：活动日志自身不删（此刻正被追加写入）；
            // $f.FullName -ieq $script:LogPath → 全路径不区分大小写比对
            if let Some(lp) = out.logfile.as_ref() {
                if e.path().to_string_lossy().to_lowercase() == lp.to_string_lossy().to_lowercase()
                {
                    self_skipped = true;
                    continue;
                }
            }
            if remove_file_forced(&e.path()) {
                files_gone += 1;
            } else {
                files_fail += 1;
            }
        }
    }

    // tools.ps1 L587-589：RESULT 总结行（格式逐字节）
    let note = if self_skipped {
        "；活动日志自身保留（iss 可传 -Log 指向它处以全删）"
    } else {
        ""
    };
    out.line(&format!(
        "RESULT: PurgeData 目录删 {} 剩 {}；日志删 {} 剩 {}{}；exit=0",
        dirs_gone, dirs_fail, files_gone, files_fail, note
    ));
    0
}
