//! 注册表基础层（CONTRACTS.md regop.rs 契约，winreg 0.55）。
//! 血律对照：install.ps1 Set-Reg/Set-RegHKLM/icacls —— 具体为 L48-66 的
//! Set-Reg / Set-RegDWord / Set-RegHKLM（Test-Path + New-Item -Force +
//! SetValue('')/Set-ItemProperty），删除侧对齐 Remove-Item / Test-Path 语义。
//! - root 只取 "HKCU"/"HKLM"（不区分大小写）；path 为不带 hive 前缀的子键路径。
//! - name=="(default)" → winreg 默认值键名 ""（PS `-eq '(default)'` 大小写不敏感，
//!   此处 eq_ignore_ascii_case 同语义）。
//! - 失败一律 false / None / 空 Vec，绝不 panic（$ErrorActionPreference='Continue'）。

use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_WRITE};
use winreg::RegKey;

/// hive 预定义句柄："HKCU"→HKEY_CURRENT_USER，"HKLM"→HKEY_LOCAL_MACHINE，其余 None。
fn root_key(root: &str) -> Option<RegKey> {
    match root.to_ascii_uppercase().as_str() {
        "HKCU" => Some(RegKey::predef(HKEY_CURRENT_USER)),
        "HKLM" => Some(RegKey::predef(HKEY_LOCAL_MACHINE)),
        _ => None,
    }
}

/// 值名映射："(default)"（不区分大小写）→ 默认值键名 ""；其余原样。
fn vname(name: &str) -> &str {
    if name.eq_ignore_ascii_case("(default)") {
        ""
    } else {
        name
    }
}

/// 写 REG_SZ（PS Set-Reg / Set-RegHKLM：键不存在则 New-Item -Force 先建，
/// "(default)" 走 SetValue('')，普通名走 Set-ItemProperty -Type String）。
pub fn set_str(root: &str, path: &str, name: &str, val: &str) -> bool {
    let hk = match root_key(root) {
        Some(h) => h,
        None => return false,
    };
    // create_subkey：键存在=以 KEY_ALL_ACCESS 打开；不存在=连同缺失父键一并创建。
    // 正是 Test-Path + New-Item -Force + 可写句柄三步的合取。
    let (k, _) = match hk.create_subkey(path) {
        Ok(t) => t,
        Err(_) => return false,
    };
    k.set_value(vname(name), &val).is_ok()
}

/// 写 REG_DWORD（PS Set-RegDWord：Set-ItemProperty -Type DWord；键不存在先建）。
pub fn set_dword(root: &str, path: &str, name: &str, val: u32) -> bool {
    let hk = match root_key(root) {
        Some(h) => h,
        None => return false,
    };
    let (k, _) = match hk.create_subkey(path) {
        Ok(t) => t,
        Err(_) => return false,
    };
    k.set_value(vname(name), &val).is_ok()
}

/// 读字符串值；"(default)" 读默认值。键不存在 / 值不存在 / 类型非字符串类
/// （REG_SZ/REG_EXPAND_SZ/REG_MULTI_SZ 之外）→ None。
pub fn get_str(root: &str, path: &str, name: &str) -> Option<String> {
    let hk = root_key(root)?;
    let k = hk.open_subkey(path).ok()?;
    k.get_value::<String, _>(vname(name)).ok()
}

/// 键是否存在（Test-Path 语义；path 为空 = hive 自身，winreg 返回自身句柄 → true）。
pub fn key_exists(root: &str, path: &str) -> bool {
    let hk = match root_key(root) {
        Some(h) => h,
        None => return false,
    };
    hk.open_subkey(path).is_ok()
}

/// 递归创建到叶子键（New-Item -Force 逐级）。winreg 的 create_subkey 本就
/// 连建缺失父键，仍按逐级走：任何一级失败即 false。path 为空 = hive 自身 → true。
pub fn ensure_key(root: &str, path: &str) -> bool {
    let hk = match root_key(root) {
        Some(h) => h,
        None => return false,
    };
    let mut cur = String::new();
    for seg in path.split('\\') {
        if seg.is_empty() {
            continue;
        }
        if !cur.is_empty() {
            cur.push('\\');
        }
        cur.push_str(seg);
        if hk.create_subkey(&cur).is_err() {
            return false;
        }
    }
    true
}

/// 递归删键（Remove-Item -Recurse 语义）：先枚举子键深度优先删空，再删自身；
/// 键不存在 → true（PS 侧 Test-Path 守卫后再删的等价语义）。path 为空无意义 → false。
pub fn delete_tree(root: &str, path: &str) -> bool {
    let hk = match root_key(root) {
        Some(h) => h,
        None => return false,
    };
    if path.is_empty() {
        return false;
    }
    if !key_exists(root, path) {
        return true;
    }
    delete_tree_inner(&hk, path)
}

/// delete_tree 递归体：枚举子键 → 逐个深删 → 此时自身已无子键，删自身。
/// 子键句柄在枚举收集完即释放，避免持着句柄删自身带来的"延迟删除"中间态。
fn delete_tree_inner(hk: &RegKey, path: &str) -> bool {
    let children: Vec<String> = match hk.open_subkey(path) {
        Ok(k) => k
            .enum_keys()
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    for name in children {
        if !delete_tree_inner(hk, &format!("{}\\{}", path, name)) {
            return false;
        }
    }
    hk.delete_subkey(path).is_ok()
}

/// 删单个值（Remove-ItemProperty 语义；值或键不存在 = 失败 false）。
/// "(default)" 删默认值。写句柄用 KEY_WRITE（含 KEY_SET_VALUE）。
pub fn delete_value(root: &str, path: &str, name: &str) -> bool {
    let hk = match root_key(root) {
        Some(h) => h,
        None => return false,
    };
    match hk.open_subkey_with_flags(path, KEY_WRITE) {
        Ok(k) => k.delete_value(vname(name)).is_ok(),
        Err(_) => false,
    }
}

/// 枚举直接子键名；键不存在或枚举中途失败 → 空 Vec（契约：失败=空）。
pub fn enum_subkeys(root: &str, path: &str) -> Vec<String> {
    let hk = match root_key(root) {
        Some(h) => h,
        None => return Vec::new(),
    };
    let k = match hk.open_subkey(path) {
        Ok(k) => k,
        Err(_) => return Vec::new(),
    };
    k.enum_keys()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default()
}

/// 枚举值名；键不存在或枚举中途失败 → 空 Vec。默认值（若已设）按 winreg 原生
/// 表示返回空串 ""（RegEnumValueW 对默认值的原生名字）。
pub fn enum_value_names(root: &str, path: &str) -> Vec<String> {
    let hk = match root_key(root) {
        Some(h) => h,
        None => return Vec::new(),
    };
    let k = match hk.open_subkey(path) {
        Ok(k) => k,
        Err(_) => return Vec::new(),
    };
    k.enum_values()
        .map(|r| r.map(|(name, _)| name))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default()
}
