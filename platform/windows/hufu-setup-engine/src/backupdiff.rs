//! backupdiff.rs —— `backup-diff` 子命令：数据文件「差异备份 + 拷入」。
//!
//! 血律对照：`hufu-setup-tools.ps1` L227-348 的 `Invoke-BackupDiffCmd`
//!（exe 安装器 ssPostInstall 阶段唯一的 PowerShell 子进程，2026-10-21
//! 为「exe 安装包完全 0 PowerShell」补的 Rust 等价实现）。行为逐条对齐：
//!
//! - 参数校验：manifest 缺失=2、SrcRoot 无效=5、缺 Dst/Backup=1；
//!   目标/备份根不存在则创建。
//! - 码表区铁闸（L254-279）：目标目录「码表\ 有内容」或「数据\ 在场」
//!   ⇒ 清单里 `码表\*` 全剔除（零写入/零补缺/零复活）。
//! - 逐行：`#`/空行跳过；`!` 前缀=仅当目标缺失才落盘；`/`→`\`。
//! - 目标已存在：`!` 行=跳过（已存在）；源缺失=失败；内容相同（PS 用
//!   SHA256，此处按字节流比对，结论等价）=跳过且保 mtime；不同=先备份
//!   旧件到 `备份根\rel`，再覆盖。
//! - 目标不存在：源缺失=失败；否则直接拷入。
//! - 失败数 >0 → 退出码 5；RESULT 汇总行字段与 PS 版同序。

use crate::common::Out;
use std::fs;
use std::io::Read;
use std::path::Path;
// 【lint 2026-10-26】PathBuf 仅测试消费。
#[cfg(test)]
use std::path::PathBuf;

/// 参数容器（来自 main.rs 的子命令解析）。
pub struct Args<'a> {
    pub manifest: Option<&'a str>,
    pub src: Option<&'a str>,
    pub dst: Option<&'a str>,
    pub backup: Option<&'a str>,
}

pub fn run_backup_diff(out: &mut Out, a: Args<'_>) -> i32 {
    // ── 参数校验（L228-244）──
    let Some(manifest) = a.manifest else {
        out.line("RESULT: manifest 缺失:  (exit=2)");
        return 2;
    };
    let mp = Path::new(manifest);
    if !mp.is_file() {
        out.line(&format!("RESULT: manifest 缺失: {manifest} (exit=2)"));
        return 2;
    }
    let Some(src_root) = a.src else {
        out.line("RESULT: SrcRoot 无效:  (exit=5)");
        return 5;
    };
    let sp = Path::new(src_root);
    if !sp.is_dir() {
        out.line(&format!("RESULT: SrcRoot 无效: {src_root} (exit=5)"));
        return 5;
    }
    let (Some(dst_root), Some(backup_root)) = (a.dst, a.backup) else {
        out.line("RESULT: 需要 -DstRoot 与 -BackupRoot (exit=1)");
        return 1;
    };
    let dp = Path::new(dst_root);
    let bp = Path::new(backup_root);
    for d in [dp, bp] {
        if !d.exists() {
            if let Err(e) = fs::create_dir_all(d) {
                out.line(&format!("RESULT: 目录创建失败: {} : {e} (exit=1)", d.display()));
                return 1;
            }
        }
    }

    // ── 读清单（UTF-8，容忍 BOM；L249-252）──
    let text = match fs::read(mp) {
        Ok(b) => String::from_utf8_lossy(b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&b))
            .into_owned(),
        Err(e) => {
            out.line(&format!("RESULT: manifest 读取失败: {manifest} ({e})"));
            return 2;
        }
    };
    let mut lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();

    // ── 码表区铁闸（L254-279）──
    let dict_root = dp.join("码表");
    let dict_occupied = dir_has_entries(&dict_root);
    let user_zone = dict_occupied || dp.join("数据").exists();
    if user_zone {
        let before = lines.len();
        lines.retain(|raw| {
            let l = raw.trim();
            let l = l.strip_prefix('!').map(str::trim).unwrap_or(l);
            !is_dict_zone_path(l)
        });
        out.line(&format!(
            "BackupDiff: 用户码表区在场（码表\\非空/数据\\在场 之一）——码表\\ 全域冻结，剔除清单 {} 项（零写入/零补缺/零复活）",
            before - lines.len()
        ));
    }

    // ── 逐行处理（L281-340）──
    let (mut copied, mut skip_same, mut skip_exists, mut backed, mut failed) = (0u32, 0u32, 0u32, 0u32, 0u32);
    let mut fail_list: Vec<String> = Vec::new();
    for raw in &lines {
        let mut line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut only_missing = false;
        if let Some(rest) = line.strip_prefix('!') {
            only_missing = true;
            line = rest.trim();
            if line.is_empty() {
                continue;
            }
        }
        let rel = line.replace('/', "\\");
        let src_f = sp.join(&rel);
        let dst_f = dp.join(&rel);

        if dst_f.exists() {
            if only_missing {
                skip_exists += 1;
                out.line(&format!("BackupDiff: 跳过（! 目标已在）: {rel}"));
                continue;
            }
            if !src_f.is_file() {
                failed += 1;
                fail_list.push(format!("{rel}（源缺失）"));
                out.line(&format!("BackupDiff: 源缺失: {}", src_f.display()));
                continue;
            }
            if same_content(&src_f, &dst_f) {
                skip_same += 1;
                out.line(&format!("BackupDiff: 跳过（内容相同，保 mtime）: {rel}"));
                continue;
            }
            let bdst = bp.join(&rel);
            let step = (|| -> std::io::Result<()> {
                ensure_parent(&bdst)?;
                fs::copy(&dst_f, &bdst)?;
                Ok(())
            })();
            if let Err(e) = step {
                failed += 1;
                fail_list.push(format!("{rel}（备份/覆盖失败）"));
                out.line(&format!("BackupDiff: 备份/覆盖失败: {rel} : {e}"));
                continue;
            }
            backed += 1;
            if let Err(e) = fs::copy(&src_f, &dst_f) {
                failed += 1;
                fail_list.push(format!("{rel}（备份/覆盖失败）"));
                out.line(&format!("BackupDiff: 备份/覆盖失败: {rel} : {e}"));
                continue;
            }
            copied += 1;
            out.line(&format!(
                "BackupDiff: 已备份+覆盖: {rel} → 备份 {}",
                bdst.display()
            ));
        } else {
            if !src_f.is_file() {
                failed += 1;
                fail_list.push(format!("{rel}（源缺失）"));
                out.line(&format!("BackupDiff: 源缺失: {}", src_f.display()));
                continue;
            }
            let step = (|| -> std::io::Result<()> {
                ensure_parent(&dst_f)?;
                fs::copy(&src_f, &dst_f)?;
                Ok(())
            })();
            match step {
                Ok(_) => {
                    copied += 1;
                    out.line(&format!("BackupDiff: 拷入: {rel}"));
                }
                Err(e) => {
                    failed += 1;
                    fail_list.push(format!("{rel}（拷入失败）"));
                    out.line(&format!("BackupDiff: 拷入失败: {rel} : {e}"));
                }
            }
        }
    }

    // ── 汇总（L342-347）──
    let rc = if failed > 0 { 5 } else { 0 };
    let mut summary = format!(
        "BackupDiff 拷入={copied} 跳过={}（相同{skip_same}/已存在{skip_exists}）已备份={backed} 失败={failed} exit={rc}",
        skip_same + skip_exists
    );
    if !fail_list.is_empty() {
        summary.push_str(&format!(" 失败清单: {}", fail_list.join("; ")));
    }
    out.line(&format!("RESULT: {summary}"));
    rc
}

/// 目录存在且有子项（PS：`@(Get-ChildItem -Force).Count -gt 0`）。
fn dir_has_entries(p: &Path) -> bool {
    p.is_dir() && fs::read_dir(p).map(|mut it| it.next().is_some()).unwrap_or(false)
}

/// `码表\*` 判定（PS `-like '码表\*'`：不区分大小写，仅前缀）。
fn is_dict_zone_path(rel: &str) -> bool {
    let r = rel.replace('/', "\\").to_lowercase();
    r.starts_with("码表\\")
}

/// 目标已存在时的「内容相同」判定（PS 用 SHA256 相等；逐字节等价）。
fn same_content(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (fs::metadata(a), fs::metadata(b)) else {
        return false;
    };
    if ma.len() != mb.len() {
        return false;
    }
    let (Ok(mut fa), Ok(mut fb)) = (fs::File::open(a), fs::File::open(b)) else {
        return false;
    };
    let mut ba = vec![0u8; 64 * 1024];
    let mut bb = vec![0u8; 64 * 1024];
    loop {
        let na = match fa.read(&mut ba) {
            Ok(n) => n,
            Err(_) => return false,
        };
        let nb = match fb.read(&mut bb) {
            Ok(n) => n,
            Err(_) => return false,
        };
        if na != nb {
            return false;
        }
        if na == 0 {
            return true;
        }
        if ba[..na] != bb[..nb] {
            return false;
        }
    }
}

/// 建父目录（PS `Ensure-DirForFile`）。
fn ensure_parent(p: &Path) -> std::io::Result<()> {
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hufu-bd-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn copies_new_backs_up_changed_and_skips_same() {
        let root = tmpdir("basic");
        let src = root.join("pkg");
        let dst = root.join("dst");
        let bak = root.join("bak");
        fs::create_dir_all(src.join("数据")).unwrap();
        fs::create_dir_all(dst.join("数据")).unwrap();
        fs::write(src.join("数据/a.txt"), b"new-a").unwrap();
        fs::write(src.join("数据/b.txt"), b"same").unwrap();
        fs::write(dst.join("数据/b.txt"), b"same").unwrap();
        let manifest = root.join("list.txt");
        fs::write(&manifest, "数据/a.txt\r\n数据/b.txt\r\n").unwrap();
        let mut out = Out::new(None);
        let rc = run_backup_diff(
            &mut out,
            Args {
                manifest: manifest.to_str(),
                src: src.to_str(),
                dst: dst.to_str(),
                backup: bak.to_str(),
            },
        );
        assert_eq!(rc, 0, "全部成功应 exit=0");
        assert_eq!(fs::read(dst.join("数据/a.txt")).unwrap(), b"new-a");
        assert_eq!(fs::read(dst.join("数据/b.txt")).unwrap(), b"same");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn backups_overwritten_file_and_flags_missing_source() {
        let root = tmpdir("overwrite");
        let src = root.join("pkg");
        let dst = root.join("dst");
        let bak = root.join("bak");
        fs::create_dir_all(src.join("数据")).unwrap();
        fs::create_dir_all(dst.join("数据")).unwrap();
        fs::write(src.join("数据/c.txt"), b"v2").unwrap();
        fs::write(dst.join("数据/c.txt"), b"v1").unwrap();
        let manifest = root.join("list.txt");
        fs::write(&manifest, "数据/c.txt\r\n数据/nope.txt\r\n").unwrap();
        let mut out = Out::new(None);
        let rc = run_backup_diff(
            &mut out,
            Args {
                manifest: manifest.to_str(),
                src: src.to_str(),
                dst: dst.to_str(),
                backup: bak.to_str(),
            },
        );
        assert_eq!(rc, 5, "有源缺失 → exit=5");
        assert_eq!(fs::read(dst.join("数据/c.txt")).unwrap(), b"v2");
        assert_eq!(fs::read(bak.join("数据/c.txt")).unwrap(), b"v1", "旧件进备份");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dict_zone_is_frozen_when_user_zone_present() {
        let root = tmpdir("freeze");
        let src = root.join("pkg");
        let dst = root.join("dst");
        let bak = root.join("bak");
        fs::create_dir_all(src.join("码表")).unwrap();
        fs::create_dir_all(dst.join("码表")).unwrap();
        fs::write(src.join("码表/虎码.txt"), b"new").unwrap();
        fs::write(dst.join("码表/用户调整.txt"), b"keep").unwrap();
        let manifest = root.join("list.txt");
        fs::write(&manifest, "码表/虎码.txt\r\n数据/config.json\r\n").unwrap();
        fs::create_dir_all(src.join("数据")).unwrap();
        fs::write(src.join("数据/config.json"), b"{}").unwrap();
        let mut out = Out::new(None);
        let rc = run_backup_diff(
            &mut out,
            Args {
                manifest: manifest.to_str(),
                src: src.to_str(),
                dst: dst.to_str(),
                backup: bak.to_str(),
            },
        );
        assert_eq!(rc, 0);
        assert!(!dst.join("码表/虎码.txt").exists(), "码表区必须冻结");
        assert!(dst.join("数据/config.json").exists(), "非码表区照常落盘");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn only_missing_prefix_skips_existing_target() {
        let root = tmpdir("onlymissing");
        let src = root.join("pkg");
        let dst = root.join("dst");
        let bak = root.join("bak");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("x.bin"), b"new").unwrap();
        fs::write(dst.join("x.bin"), b"old").unwrap();
        let manifest = root.join("list.txt");
        fs::write(&manifest, "!x.bin\r\n").unwrap();
        let mut out = Out::new(None);
        let rc = run_backup_diff(
            &mut out,
            Args {
                manifest: manifest.to_str(),
                src: src.to_str(),
                dst: dst.to_str(),
                backup: bak.to_str(),
            },
        );
        assert_eq!(rc, 0);
        assert_eq!(fs::read(dst.join("x.bin")).unwrap(), b"old", "! 行不得覆盖");
        let _ = fs::remove_dir_all(&root);
    }
}
