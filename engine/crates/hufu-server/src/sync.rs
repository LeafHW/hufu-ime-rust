//! 【云同步 v3 2026-10-30 全案重做】WebDAV 后端：多机同步**整个方案文件夹**
//! （码表\<方案>\ 下全部文件）+ 配置设置。服务器 = 用户自备 WebDAV
//!（坚果云等），虎符官方不设服务器、不经手任何用户数据。
//!
//! 核心算法【mtime-LWW，用户定案】：
//! - 一切仲裁按**文件修改时间**：云端新 → 覆盖本地（mtime 回拨防乒乓）；
//!   本地新 → 8 秒防抖后上传。不猜语义、不做行合并——文件什么样就传
//!   什么样，最后改的赢。
//! - 增量：首次整夹上/下，之后逐文件比对 mt，只传有改动的文件。
//! - 删除传播：本地文件相对上次收敛无改动而云端清单已无 = 对端删的
//!   → 本地移入回收站后删除；换码表 = 旧删新传，其他机器跟上。
//! - 回收站：被覆盖/删除的文件先挪 数据\同步回收站\<方案>\，每方案
//!   留 20 份/30 天，管理页可还原（还原即本地新改动，随同步上传）。
//!
//! 云端布局（WebDAV 根相对）：
//! - hufu-sync/manifest.json —— 清单元数据（设备表/各方案文件表/config）
//! - hufu-sync/files/<key> —— 文件本体；key = SHA256("方案/文件名")
//!   前 32 hex（纯 ASCII 路径，规避中文路径兼容问题）
//!
//! 触发：启动延迟 20s / 周期 2min / 码表监视器通知（防抖 8s）/ 手动。
//! 失败静默退避记 last_error——**红线：绝不触碰/阻塞打字路径**。
//!
//! 【加密口令移除 2026-10-30 用户拍板】v3 早期可选的 AES-GCM 端到端
//! 加密已整体移除（含 UI），云端存明文——数据只经用户自己的 WebDAV
//! 网盘，信任边界=网盘账号本身。
//!
//! 凭据存放：数据\同步.json（明文，与本机 config.json 同信任级；WebDAV
//! 用应用密码非登录密码）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::host::Host;

/// manifest 格式版本（v3=分文件 mtime 档；v1/v2 单包档已废弃，拉到旧包
/// 会提示升级——避免旧逻辑回写脏数据）
const MF_VERSION: u64 = 3;
const MF_PATH: &str = "hufu-sync/manifest.json";
const FILES_DIR: &str = "hufu-sync/files";
/// 单文件上限（码表 MB 级；防异常巨物打爆坚果云配额）
const MAX_FILE: u64 = 32 * 1024 * 1024;
/// manifest 上限
const MAX_MF: usize = 512 * 1024;
/// 周期同步间隔【2min 2026-10-30 用户定案，原 5min】
const PERIOD: Duration = Duration::from_secs(120);
/// 启动延迟（让引擎/模型先就绪）
const START_DELAY: Duration = Duration::from_secs(20);
/// 变化通知防抖（合并连发）
const DEBOUNCE: Duration = Duration::from_secs(8);
/// 回收站：每方案保留份数
const TRASH_KEEP: usize = 20;
/// 回收站：保留天数
const TRASH_DAYS: u64 = 30;
/// 整轮备份保留份数（数据\同步备份\）
const BACKUP_KEEP: usize = 10;
/// 设备表活跃窗口（超时视为离线设备）
const DEVICE_STALE_SECS: u64 = 30 * 24 * 3600;
/// 「保持已存值」哨兵（UI 留空不覆盖已存密钥/口令）
const KEEP: &str = "__keep__";
const DEFAULT_URL: &str = "https://dav.jianguoyun.com/dav/";

/// 不同步的 config 键。
/// 【2026-10-30/31 用户拍板两轮】皮肤选择（skin/skin_kind，含自带
/// JSON 皮如墨岩）**纳入同步**——重装/新机要恢复皮肤选择；.ssf 皮
/// 肤文件本体不在同步面（对机没装该文件时 heal 回退默认皮，装回
/// 文件即恢复）。整句模型设置（sentence 整节）不同步——模型路径
/// 每机不同，与导出/导入主权闸同边界。
const CONFIG_EXCLUDED: &[&str] = &[
    "appearance.font_family", // 字体装没装每机不同
    "general.autostart",      // 开机自启 = 机器属性
    "general.export_key",     // 本机录制的快捷键
    "sentence",               // 整句模型设置（模型路径机器相关）
];

// ─────────────────────────── 全局状态 ───────────────────────────

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncSettings {
    pub url: String,
    pub user: String,
    /// WebDAV 应用密码（明文存本机，同 config.json 信任级）
    pub pass: String,
    pub device: String,
    /// 总开关：false=停止同步（凭据保留，云端不动）
    pub enabled: bool,
    /// 勾选参与同步的方案名（码表\ 下的文件夹名）
    pub selected: Vec<String>,
    /// 上次收敛时各勾选方案的文件 mtime 表（毫秒）：方案→文件→mt。
    /// 「本地有无新改动」= 现文件 mt 是否大于此值（同步写盘会回拨到
    /// 云端 mt，故同步写盘不会谎报新改动）。
    pub local_snap: BTreeMap<String, BTreeMap<String, u64>>,
    /// 上次收敛时可同步配置快照（config 三方归并 base）
    pub last_config: Option<Value>,
    pub last_sync: u64,
    pub last_error: Option<String>,
    // 兼容旧档（v1/v2 遗留字段，读入后忽略）
    pub snapshots: Option<BTreeMap<String, String>>,
}

static TX: OnceLock<Sender<&'static str>> = OnceLock::new();
static SHARED: OnceLock<Arc<Mutex<Host>>> = OnceLock::new();
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
/// 内存真源（spawn 时从 数据\同步.json 载入；save/persist 同步写回）
static SETTINGS: Mutex<Option<SyncSettings>> = Mutex::new(None);
static PHASE: Mutex<&'static str> = Mutex::new("idle");

/// 同步进度（供 UI 进度条；文件级，字节级由各传输函数累计）。
/// active=true 期间 cur/total 单调递增；结束后 active=false 但计数
/// 保留到下一轮开始——UI 可展示「上一轮 ↑N 个 / X MB」。
#[derive(Default, Clone, serde::Serialize)]
struct SyncProgress {
    active: bool,
    cur: u32,
    total: u32,
    cur_name: String,
    up_files: u32,
    down_files: u32,
    up_bytes: u64,
    down_bytes: u64,
}
static PROGRESS: Mutex<SyncProgress> = Mutex::new(SyncProgress {
    active: false,
    cur: 0,
    total: 0,
    cur_name: String::new(),
    up_files: 0,
    down_files: 0,
    up_bytes: 0,
    down_bytes: 0,
});

/// 进度游标：开始一个文件的传输（cur_name 立即可见，防止长时间无反馈）。
fn prog_begin_file(name: &str) {
    let mut p = PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
    p.cur_name = name.to_string();
}

/// 进度累计：完成一次上传/下载（is_up 区分方向）。
fn prog_done_file(is_up: bool, bytes: u64) {
    let mut p = PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
    p.cur += 1;
    if p.cur > p.total {
        p.total = p.cur; // 预演低估时兜底，条不至于超界
    }
    if is_up {
        p.up_files += 1;
        p.up_bytes += bytes;
    } else {
        p.down_files += 1;
        p.down_bytes += bytes;
    }
    p.cur_name.clear();
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("同步.json")
}

fn load_settings(data_dir: &Path) -> SyncSettings {
    std::fs::read_to_string(settings_path(data_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn persist(st: &SyncSettings) -> std::io::Result<()> {
    let Some(dir) = DATA_DIR.get() else {
        return Ok(());
    };
    let p = settings_path(dir);
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    // 保存前备份（凭据唯一存放处，写坏可回滚）
    if p.is_file() {
        let _ = std::fs::copy(&p, p.with_extension("json.bak"));
    }
    std::fs::write(&p, serde_json::to_vec_pretty(st).unwrap_or_default())
}

fn set_phase(p: &'static str) {
    *PHASE.lock().unwrap_or_else(|e| e.into_inner()) = p;
}

fn default_device() -> String {
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "PC".into());
    let mut r = [0u8; 2];
    let _ = getrandom::getrandom(&mut r);
    format!("{host}-{}", hex(&r))
}

// ─────────────────────────── 对外 API ───────────────────────────

/// 启动同步线程（main 在引擎就绪后调用一次）。
pub fn spawn(shared: Arc<Mutex<Host>>, data_dir: PathBuf) {
    let (tx, rx) = std::sync::mpsc::channel::<&'static str>();
    let _ = TX.set(tx);
    let _ = SHARED.set(shared);
    *SETTINGS.lock().unwrap_or_else(|e| e.into_inner()) = Some(load_settings(&data_dir));
    let _ = DATA_DIR.set(data_dir);
    let _ = std::thread::Builder::new()
        .name("hufu-sync".into())
        .spawn(move || worker(rx));
}

/// 触发一次同步（码表监视器/设置保存/手动按钮）。未配置/未启动=无害 no-op。
pub fn notify(reason: &'static str) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(reason);
    }
}

/// GET /api/sync/status（不含任何密钥）。
pub fn status_value() -> Value {
    let s = SETTINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default();
    let phase = *PHASE.lock().unwrap_or_else(|e| e.into_inner());
    let prog = PROGRESS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let configured = !s.url.trim().is_empty() && !s.user.trim().is_empty() && !s.pass.is_empty();
    json!({
        "enabled": configured && s.enabled,
        "configured": configured,
        "stopped": configured && !s.enabled,
        "url": s.url, "user": s.user, "device": s.device,
        "selected": s.selected,
        "last_sync": s.last_sync, "last_error": s.last_error,
        "phase": phase,
        "progress": prog,
    })
}

/// GET /api/sync/schemes：本机方案清单 + 勾选态 + 云端有无（供勾选 UI）。
/// 网络只读 manifest，失败时云端部分置空（UI 仍可用本地部分）。
pub fn schemes_overview() -> Value {
    let s = SETTINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default();
    let mut local: Vec<Value> = Vec::new();
    if let Some(root) = local_schema_root() {
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                let p = e.path();
                if !p.is_dir() {
                    continue;
                }
                let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let n_files = std::fs::read_dir(&p)
                    .map(|rd| rd.flatten().filter(|x| x.path().is_file()).count())
                    .unwrap_or(0);
                local.push(json!({
                    "name": name,
                    "local": true,
                    "files": n_files,
                    "selected": s.selected.iter().any(|x| x == name),
                }));
            }
        }
    }
    // 云端清单（只读；未配置/网络失败=空数组）
    let mut cloud_names: Vec<String> = Vec::new();
    if !s.url.trim().is_empty() && !s.user.trim().is_empty() && !s.pass.is_empty() {
        let agent = build_agent();
        if let Ok(Some(mf)) = fetch_manifest(&agent, &s) {
            if let Some(m) = mf.get("schemes").and_then(|v| v.as_object()) {
                cloud_names = m.keys().cloned().collect();
            }
        }
    }
    // 【云端标记 2026-10-30 用户需求】本地+云端都有的方案也要标 ☁——
    // 勾选列表按云端清单逐项补 cloud 标记，不只标「本机没有的」。
    for v in local.iter_mut() {
        let is_cloud = v["name"].as_str().map(|n| cloud_names.iter().any(|c| c == n)).unwrap_or(false);
        v["cloud"] = json!(is_cloud);
    }
    for name in &cloud_names {
        if !local.iter().any(|v| v["name"].as_str() == Some(name.as_str())) {
            local.push(json!({"name": name, "local": false, "files": 0, "cloud": true,
                "selected": s.selected.iter().any(|x| x == name)}));
        }
    }
    json!({"ok": true, "schemes": local})
}

/// POST /api/sync/select {selected: [方案名...]}：勾选保存（空=全不选）。
/// 保存后立即触发同步（新勾方案首拉/首传）。凭据齐备时自动启用
///（升级旧档：enabled 新字段缺省 false，用户勾选=明确要同步）。
pub fn save_selection(selected: &[String]) -> Result<(), String> {
    let mut guard = SETTINGS.lock().unwrap_or_else(|e| e.into_inner());
    let st = guard.get_or_insert_with(SyncSettings::default);
    st.selected = selected.to_vec();
    let ready =
        !st.url.trim().is_empty() && !st.user.trim().is_empty() && !st.pass.is_empty();
    if ready && !st.selected.is_empty() {
        st.enabled = true;
    }
    persist(st).map_err(|e| format!("写入同步配置失败：{e}"))?;
    drop(guard);
    notify("勾选");
    Ok(())
}

/// POST /api/sync/enable {enabled}：停止/恢复同步（凭据与云端不动）。
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let mut guard = SETTINGS.lock().unwrap_or_else(|e| e.into_inner());
    let st = guard.get_or_insert_with(SyncSettings::default);
    st.enabled = enabled;
    persist(st).map_err(|e| format!("写入同步配置失败：{e}"))?;
    drop(guard);
    if enabled {
        notify("恢复");
    }
    Ok(())
}

/// POST /api/sync/save。pass 传 `__keep__` = 保持已存值。
/// 【加密口令移除 2026-10-30 用户拍板】云端只存明文（数据仍只经
/// 用户自己的 WebDAV 网盘）；旧档案里的 passphrase 字段自然失效。
pub fn save_settings(
    url: &str,
    user: &str,
    pass: &str,
    device: &str,
) -> Result<(), String> {
    if DATA_DIR.get().is_none() {
        return Err("同步模块未初始化".into());
    }
    let mut guard = SETTINGS.lock().unwrap_or_else(|e| e.into_inner());
    let st = guard.get_or_insert_with(SyncSettings::default);
    // 误清空护栏（畸形请求防洗凭据）
    let was_enabled = !st.user.is_empty() && !st.pass.is_empty();
    if was_enabled && pass != KEEP && pass.is_empty() && user.trim().is_empty() {
        return Err("已拒绝：该请求会清空已保存的同步账号（疑似误操作）；如确要重置请用「退出帐户」".into());
    }
    st.url = if url.trim().is_empty() {
        DEFAULT_URL.into()
    } else {
        url.trim().to_string()
    };
    st.user = user.trim().to_string();
    if pass != KEEP {
        st.pass = pass.to_string();
    }
    st.device = if device.trim().is_empty() {
        default_device()
    } else {
        device.trim().to_string()
    };
    // 首次配置成功启用
    if !st.user.is_empty() && !st.pass.is_empty() && !st.url.is_empty() {
        st.enabled = true;
    }
    //【重装洗值修复 2026-10-31】保存同步设置 = 本机宣告「以云端为准拉齐
    // 一次」。清空 config 归并 base（last_config）：否则重装/换机后本机
    // 是出厂默认值，base 还是旧机器快照 → 默认值≠base 被误判「本地改
    // 过」→ 回洗云端（实锤案例：diff_hl 勾选被重装机的默认 False 洗掉，
    // 两台机全变关）。清 base 后归并走 local==base → 云端全量采纳。
    // 日常只改设备名等再保存也无害：云端即最后收敛共识，采纳无损失。
    st.last_config = None;
    persist(st).map_err(|e| format!("写入同步配置失败：{e}"))?;
    let ready = !st.url.is_empty() && !st.user.is_empty() && !st.pass.is_empty();
    drop(guard);
    if ready {
        notify("设置");
    }
    Ok(())
}

/// POST /api/sync/test（网络 IO 在 HTTP 线程锁外调用）。
pub fn test_connection() -> Value {
    let s = SETTINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default();
    if s.url.trim().is_empty() || s.user.trim().is_empty() || s.pass.is_empty() {
        return json!({"ok": false, "msg": "请先填写并保存服务器地址/账号/应用密码"});
    }
    let agent = build_agent();
    match fetch_manifest(&agent, &s) {
        Ok(None) => json!({"ok": true, "exists": false,
            "msg": "连接成功：云端还没有同步数据（首次同步将创建）"}),
        Ok(Some(mf)) => {
            let schemes = mf
                .get("schemes")
                .and_then(|v| v.as_object())
                .map(|m| m.len())
                .unwrap_or(0);
            json!({"ok": true, "exists": true,
                "remote_device": mf.get("device").and_then(|v| v.as_str()).unwrap_or("?"),
                "remote_ts": mf.get("ts").and_then(|v| v.as_u64()).unwrap_or(0),
                "schemes": schemes})
        }
        Err(e) => json!({"ok": false, "msg": e}),
    }
}

/// GET /api/sync/cloud：云端概览（设备表/方案→文件清单/大小/mt/上传者）。
/// 只读，不写任何本地数据。
pub fn cloud_summary() -> Value {
    let s = SETTINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default();
    if s.url.trim().is_empty() || s.user.trim().is_empty() || s.pass.is_empty() {
        return json!({"ok": false, "msg": "尚未配置同步账号"});
    }
    let agent = build_agent();
    let mf = match fetch_manifest(&agent, &s) {
        Ok(None) => return json!({"ok": true, "exists": false}),
        Ok(Some(m)) => m,
        Err(e) => return json!({"ok": false, "msg": e}),
    };
    let cutoff = now_secs().saturating_sub(DEVICE_STALE_SECS);
    let devices: Value = mf
        .get("devices")
        .and_then(|v| v.as_object())
        .map(|m| {
            let mut out = serde_json::Map::new();
            for (k, ts) in m {
                if ts.as_u64().unwrap_or(0) > cutoff {
                    out.insert(k.clone(), ts.clone());
                }
            }
            Value::Object(out)
        })
        .unwrap_or(json!({}));
    json!({
        "ok": true, "exists": true,
        "last_device": mf.get("device").and_then(|v| v.as_str()).unwrap_or("?"),
        "last_ts": mf.get("ts").and_then(|v| v.as_u64()).unwrap_or(0),
        "devices": devices,
        "schemes": mf.get("schemes").cloned().unwrap_or(json!({})),
    })
}

/// POST /api/sync/cloud/delete {scheme?}：删云端。带 scheme=只删该方案
///（files DELETE + manifest 条目移除）；不带=整库删除。**本地文件一律
/// 不动**；整库删除同时停止同步（防下轮自动回传）。
pub fn delete_cloud(scheme: Option<&str>) -> Result<String, String> {
    let s = SETTINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default();
    if s.url.trim().is_empty() || s.user.trim().is_empty() || s.pass.is_empty() {
        return Err("尚未配置同步账号".into());
    }
    let agent = build_agent();
    match scheme {
        Some(name) => {
            let mut mf = match fetch_manifest(&agent, &s)? {
                Some(m) => m,
                None => return Ok("云端本来就没有同步数据".into()),
            };
            let entry = mf
                .pointer_mut(&format!("/schemes/{}", json_escape_key(name)))
                .map(|e| e.take());
            match entry {
                Some(Value::Object(files)) => {
                    for (_, f) in &files {
                        if let Some(k) = f.get("key").and_then(|v| v.as_str()) {
                            webdav_file_del(&agent, &s, k)?;
                        }
                    }
                    prune_manifest(&mut mf, &s);
                    put_manifest(&agent, &s, &mf)?;
                    Ok(format!("云端方案「{name}」已删除（本机文件未动）"))
                }
                _ => Ok(format!("云端没有方案「{name}」的同步数据")),
            }
        }
        None => {
            // 整库：删 manifest + files 目录重建空目录标志（DELETE 目录在
            // 坚果云不可靠，改为逐文件删 + manifest 置空）
            match fetch_manifest(&agent, &s)? {
                Some(mut mf) => {
                    if let Some(schemes) = mf.get_mut("schemes").and_then(|v| v.as_object_mut()) {
                        for (_, entry) in schemes.iter() {
                            if let Some(files) = entry.get("files").and_then(|v| v.as_object()) {
                                for (_, f) in files {
                                    if let Some(k) = f.get("key").and_then(|v| v.as_str()) {
                                        let _ = webdav_file_del(&agent, &s, k);
                                    }
                                }
                            }
                        }
                    }
                    mf["schemes"] = json!({});
                    mf["config"] = Value::Null;
                    prune_manifest(&mut mf, &s);
                    let _ = put_manifest(&agent, &s, &mf);
                }
                None => {}
            }
            let _ = webdav_del(&agent, &s, MF_PATH);
            // 停止同步 + 清本地快照（凭据保留，想继续用重新勾选启用）
            let mut guard = SETTINGS.lock().unwrap_or_else(|e| e.into_inner());
            let st = guard.get_or_insert_with(SyncSettings::default);
            st.enabled = false;
            st.local_snap.clear();
            st.last_config = None;
            st.last_sync = 0;
            st.last_error = None;
            persist(st).map_err(|e| format!("写入同步配置失败：{e}"))?;
            Ok("云端同步数据已删除，同步已停止（本机文件未动）".into())
        }
    }
}

/// POST /api/sync/logout：退出帐户——清凭据并停用。云端保留。
pub fn logout() -> Result<String, String> {
    let mut guard = SETTINGS.lock().unwrap_or_else(|e| e.into_inner());
    let st = guard.get_or_insert_with(SyncSettings::default);
    st.user = String::new();
    st.pass = String::new();
    st.enabled = false;
    st.local_snap.clear();
    st.last_config = None;
    persist(st).map_err(|e| format!("写入同步配置失败：{e}"))?;
    Ok("已退出同步帐户（云端数据保留；本机文件未动）".into())
}

/// GET /api/sync/trash：回收站列表（方案→文件名→份数+最近时间）。
pub fn trash_list() -> Value {
    let Some(dir) = DATA_DIR.get() else {
        return json!({"ok": false});
    };
    let root = dir.join("同步回收站");
    let mut schemes = serde_json::Map::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            let sd = e.path();
            if !sd.is_dir() {
                continue;
            }
            let Some(name) = sd.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let mut files: Vec<Value> = std::fs::read_dir(&sd)
                .map(|rd| {
                    rd.flatten()
                        .filter(|x| x.path().is_file())
                        .filter_map(|x| {
                            let fname = x.file_name().into_string().ok()?;
                            // 格式 <epoch_ms>-<原名>
                            let orig = fname.split_once('-').map(|(_, r)| r.to_string())?;
                            let ts: u64 = fname.split('-').next()?.parse().ok()?;
                            let mt = x
                                .metadata()
                                .ok()
                                .and_then(|m| m.modified().ok())
                                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0);
                            Some(json!({"trash": fname, "name": orig,
                                "saved_ms": ts, "size": x.metadata().ok().map(|m| m.len()).unwrap_or(0),
                                "mt": mt}))
                        })
                        .collect()
                })
                .unwrap_or_default();
            files.sort_by(|a, b| b["saved_ms"].as_u64().cmp(&a["saved_ms"].as_u64()));
            if !files.is_empty() {
                schemes.insert(name.to_string(), Value::Array(files));
            }
        }
    }
    json!({"ok": true, "root": root, "schemes": Value::Object(schemes)})
}

/// POST /api/sync/trash/restore {scheme, trash}：还原一份回收站文件到
/// 对应方案目录（mtime=现在 → 视为本地新改动，随下轮同步上传传播）。
pub fn trash_restore(scheme: &str, trash: &str) -> Result<String, String> {
    // 路径安全：名字不得含路径分隔
    if scheme.contains(['/', '\\']) || trash.contains(['/', '\\']) {
        return Err("非法名称".into());
    }
    let Some(dir) = DATA_DIR.get() else {
        return Err("同步模块未初始化".into());
    };
    let src = dir.join("同步回收站").join(scheme).join(trash);
    if !src.is_file() {
        return Err("回收站里没有这份文件".into());
    }
    let Some(orig) = trash.split_once('-').map(|(_, r)| r) else {
        return Err("回收站文件名格式不对".into());
    };
    let root = local_schema_root().ok_or("找不到码表目录")?;
    let dst_dir = root.join(scheme);
    std::fs::create_dir_all(&dst_dir).map_err(|e| format!("建目录失败：{e}"))?;
    let dst = dst_dir.join(orig);
    std::fs::copy(&src, &dst).map_err(|e| format!("还原失败：{e}"))?;
    // mtime=现在：作为本地新改动参与下轮仲裁
    touch(&dst);
    notify("回收站还原");
    Ok(format!("已还原 {scheme}\\{orig}（将作为本机新改动同步）"))
}

/// POST /api/sync/trash/clear：清空回收站。
pub fn trash_clear() -> Result<String, String> {
    let Some(dir) = DATA_DIR.get() else {
        return Err("同步模块未初始化".into());
    };
    let root = dir.join("同步回收站");
    let n = count_files(&root);
    let _ = std::fs::remove_dir_all(&root);
    Ok(format!("已清空回收站（{n} 个文件）"))
}

/// GET /api/sync/backup：本地整轮备份摘要。
pub fn backup_summary() -> Value {
    let Some(dir) = DATA_DIR.get() else {
        return json!({"ok": false});
    };
    let bd = dir.join("同步备份");
    let mut rounds: Vec<String> = std::fs::read_dir(&bd)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    rounds.sort();
    rounds.reverse();
    rounds.truncate(BACKUP_KEEP);
    json!({"ok": true, "path": bd, "rounds": rounds})
}

/// GET /api/sync/tutorial：图文教程 PNG。
pub fn tutorial_image() -> Option<Vec<u8>> {
    let dir = DATA_DIR.get()?.join("同步教程.png");
    if let Ok(b) = std::fs::read(&dir) {
        return Some(b);
    }
    let desktop = std::env::var("USERPROFILE")
        .ok()
        .map(|h| std::path::PathBuf::from(h).join("Desktop").join("同步教程.png"))?;
    std::fs::read(desktop).ok()
}

// ─────────────────────────── 同步线程 ───────────────────────────

fn worker(rx: Receiver<&'static str>) {
    std::thread::sleep(START_DELAY);
    let _ = std::panic::catch_unwind(|| run_sync("启动"));
    loop {
        let reason = match rx.recv_timeout(PERIOD) {
            // 手动=立即；其余（码表变化/设置）防抖合并
            Ok(r) if r == "手动" => r,
            Ok(r) => {
                std::thread::sleep(DEBOUNCE);
                while rx.try_recv().is_ok() {}
                r
            }
            Err(_) => "周期",
        };
        let _ = std::panic::catch_unwind(|| run_sync(reason));
    }
}

fn run_sync(reason: &'static str) {
    let s = SETTINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default();
    if s.url.trim().is_empty() || s.user.trim().is_empty() || s.pass.is_empty() {
        return;
    }
    if !s.enabled {
        return;
    }
    set_phase("syncing");
    {
        // 新一轮：进度归零、激活（total 由 do_sync 预演填充）
        let mut p = PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
        *p = SyncProgress { active: true, ..Default::default() };
    }
    let r = do_sync(&s);
    {
        let mut p = PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
        p.active = false;
        p.cur = p.total; // 完成态条拉满（含预演误差）
        p.cur_name.clear();
    }
    let mut guard = SETTINGS.lock().unwrap_or_else(|e| e.into_inner());
    let st = guard.get_or_insert_with(SyncSettings::default);
    match r {
        Ok(msg) => {
            st.last_sync = now_secs();
            st.last_error = None;
            eprintln!("云同步[{reason}]：{msg}");
        }
        Err(e) => {
            st.last_error = Some(e.clone());
            eprintln!("云同步[{reason}]失败：{e}");
        }
    }
    let _ = persist(st);
    drop(guard);
    set_phase("idle");
}

/// 一轮同步：拉 manifest → 逐方案逐文件 mtime 仲裁 → config 三方归并
/// → 回写 manifest（设备活跃 + 文件表 + config）。
fn do_sync(s: &SyncSettings) -> Result<String, String> {
    let agent = build_agent();
    let mut mf = fetch_manifest(&agent, s)?.unwrap_or_else(empty_manifest);

    let root = local_schema_root().ok_or("找不到码表目录")?;
    let mut uploaded = 0usize;
    let mut downloaded = 0usize;
    let mut deleted_local = 0usize;
    let snap = s.local_snap.clone();

    for name in &s.selected {
        let dir = root.join(name);
        let r_entry = mf
            .pointer(&format!("/schemes/{}", json_escape_key(name)))
            .cloned();
        if !dir.is_dir() {
            // 本机没装但勾选了：云端有 → 整夹拉取落地
            if let Some(entry) = r_entry {
                let n = entry
                    .get("files")
                    .and_then(|v| v.as_object())
                    .map(|m| m.len())
                    .unwrap_or(0);
                {
                    let mut p = PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
                    p.total += n as u32;
                }
                prog_begin_file(&format!("{name}（整套下载）"));
                let got = pull_scheme_all(&agent, s, &root, name, &entry)?;
                prog_done_file(false, got as u64);
                downloaded += 1;
            }
            continue;
        }
        // 本地文件表 {名: (mt, path)}
        let local_files = scan_dir(&dir)?;
        let r_files = r_entry
            .as_ref()
            .and_then(|e| e.get("files"))
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let snap_files = snap.get(name);

        //【洗劫事故 2026-10-30 根因】旧版两处连环坑：push_file 用
        // pointer_mut 访问尚不存在的 files/<名> 键 → None 静默丢条目
        //（文件上云、清单没有）；主循环又用「云端已有条目」重建 new_entry
        // 整覆写 files → 清单被清空。下一轮把本地文件全判成「对端删的」
        // 洗进回收站。现改：①清单就地增删（绝不整覆写）；②删除传播加
        // 保险——云端清单为空而本地有文件时【绝不删本地】（整案清空
        // 只能由管理页「删除云端」触发，同步层不做整夹清空传播）。
        ensure_scheme_entry(&mut mf, name);
        let empty_remote = r_files.is_empty();

        // 预演：本轮该方案可能要传的文件数（mt 级粗判，只增 total
        // 不影响执行；误差只影响进度条走满的时机，不影响正确性）
        {
            let mut p = PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
            for (fname, rv) in &r_files {
                let rmt = rv.get("mt").and_then(|v| v.as_u64()).unwrap_or(0);
                let lmt = local_files
                    .get(fname)
                    .and_then(|q| file_mtime_ms(q).ok())
                    .unwrap_or(0);
                if local_files.get(fname).is_none() || rmt > lmt {
                    p.total += 1;
                }
            }
            for fname in local_files.keys() {
                if !r_files.contains_key(fname) {
                    p.total += 1; // 本地多出：上传或对端删，都要动一下
                }
            }
        }

        // 仲裁：云端文件（拉新/跳过；条目留在清单里）
        for (fname, rv) in &r_files {
            let rmt = rv.get("mt").and_then(|v| v.as_u64()).unwrap_or(0);
            let key = rv.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let lp = local_files.get(fname);
            let lmt = lp
                .and_then(|p| file_mtime_ms(p).ok())
                .unwrap_or(0);
            if lp.is_none() || rmt >= lmt {
                // 云端新（或同刻）：下载覆盖（内容相同会跳过写盘）
                if lp.is_none() || file_hash(lp.unwrap())? != remote_hash_hint(rv) {
                    prog_begin_file(&format!("{name}/{fname}"));
                    let n = pull_file(&agent, s, &dir, fname, key, rmt)?;
                    prog_done_file(false, n as u64);
                    downloaded += 1;
                }
            } else {
                // 本地新：上传（push_file 就地更新清单条目）
                prog_begin_file(&format!("{name}/{fname}"));
                let n = push_file(&agent, s, &mut mf, name, fname, lp.unwrap())?;
                prog_done_file(true, n as u64);
                uploaded += 1;
            }
        }
        // 本地多出的文件（云端清单没有）
        for (fname, lp) in &local_files {
            if r_files.contains_key(fname) {
                continue;
            }
            let lmt = file_mtime_ms(lp)?;
            let snap_mt = snap_files.and_then(|m| m.get(fname)).copied();
            let is_new = snap_mt.is_none() || lmt > snap_mt.unwrap();
            if is_new || empty_remote {
                // 本地新文件（含换码表的新主码表；或云端清单不完整时
                // 的保险闸补传）→ 上传
                prog_begin_file(&format!("{name}/{fname}"));
                let n = push_file(&agent, s, &mut mf, name, fname, lp)?;
                prog_done_file(true, n as u64);
                uploaded += 1;
            } else {
                // 云端清单非空而该文件不在其中 = 对端真删了这个文件
                //（换码表等）→ 回收站后删本地
                trash_file(&dir, fname)?;
                deleted_local += 1;
            }
        }
        // 元信息就地更新（files 已由 push/pull/仲裁就地维护，不覆写）
        if let Some(e) = mf
            .get_mut("schemes")
            .and_then(|v| v.as_object_mut())
            .and_then(|m| m.get_mut(name.as_str()))
        {
            e["by"] = json!(s.device);
            e["ts"] = json!(now_secs());
        }
    }

    //【自带皮肤参数同步 2026-10-31】皮肤目录文件面（mtime-LWW 同款，
    // 哈希比对免空转）。下载落地后 skin_ver 推进——DLL poll 立即重拉。
    {
        let (su, pulled) = sync_builtin_skins(&agent, s, &mut mf)?;
        uploaded += su;
        if pulled {
            if let Some(shared) = SHARED.get() {
                let mut h = shared.lock().unwrap_or_else(|e| e.into_inner());
                h.skin_ver += 1;
            }
        }
    }

    // config：三方归并（base=上次收敛快照）。远端项变了且本机没动 →
    // 采纳；本机变了 → 保留；都动 → 本机胜。
    let mut cfg_changed = false;
    if let Some(rcfg) = mf.get("config").cloned() {
        if rcfg.is_object() {
            let Some(shared) = SHARED.get() else {
                return Err("同步模块未初始化".into());
            };
            let mut h = shared.lock().unwrap_or_else(|p| p.into_inner());
            let full =
                serde_json::to_value(&h.engine.config).map_err(|e| e.to_string())?;
            let mut local_san = full.clone();
            sanitize_config(&mut local_san);
            let base = s.last_config.clone().unwrap_or_else(|| local_san.clone());
            let merged = three_way_merge(&full, &local_san, &base, &rcfg);
            if merged != full {
                let data_dir = DATA_DIR.get().expect("sync 未初始化").clone();
                backup_config(&data_dir);
                let cfg: hufu_config::Config = serde_json::from_value(merged)
                    .map_err(|e| format!("归并配置无法解析：{e}"))?;
                let (need_sentence, teardown) = h
                    .apply_config(cfg)
                    .map_err(|e| format!("应用配置失败：{e}"))?;
                if need_sentence {
                    crate::reload_sentence_bg(teardown, true);
                }
                cfg_changed = true;
            }
        }
    }
    // 回写 config（消毒后的本机当前值）
    {
        let Some(shared) = SHARED.get() else {
            return Err("同步模块未初始化".into());
        };
        let h = shared.lock().unwrap_or_else(|p| p.into_inner());
        let mut cfg = serde_json::to_value(&h.engine.config).map_err(|e| e.to_string())?;
        sanitize_config(&mut cfg);
        mf["config"] = cfg;
    }

    // 设备表 + 落盘 manifest
    prune_manifest(&mut mf, s);
    put_manifest(&agent, s, &mf)?;

    // 更新本地收敛快照（各勾选方案当前文件 mt 表）
    let mut new_snap: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    for name in &s.selected {
        let dir = root.join(name);
        if dir.is_dir() {
            let mut m: BTreeMap<String, u64> = BTreeMap::new();
            for (fname, p) in scan_dir(&dir)? {
                if let Ok(mt) = file_mtime_ms(&p) {
                    m.insert(fname, mt);
                }
            }
            new_snap.insert(name.clone(), m);
        }
    }
    let mut guard = SETTINGS.lock().unwrap_or_else(|e| e.into_inner());
    let st = guard.get_or_insert_with(SyncSettings::default);
    st.local_snap = new_snap;
    let mut cfg = mf.get("config").cloned().unwrap_or(Value::Null);
    if cfg.is_null() {
        st.last_config = None;
    } else {
        sanitize_config(&mut cfg);
        st.last_config = Some(cfg);
    }
    drop(guard);
    if let Some(d) = DATA_DIR.get() {
        prune_backups(d);
    }

    Ok(format!(
        "↑{uploaded} ↓{downloaded} 删{deleted_local} 方案{} 配置{}",
        s.selected.len(),
        if cfg_changed { "有更新" } else { "无变化" }
    ))
}

// ─────────────────────────── 仲裁辅助 ───────────────────────────

/// 本机码表根（Host 锁内短取）。
fn local_schema_root() -> Option<PathBuf> {
    let shared = SHARED.get()?;
    let h = shared.lock().ok()?;
    let data_dir = DATA_DIR.get()?;
    Some(hufu_engine::Engine::resolve_data_sub(
        data_dir,
        &h.engine.config.schema.dir,
    ))
}

/// 扫描目录：{文件名: 路径}（只一层，跳过子目录——方案目录约定扁平）。
fn scan_dir(dir: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut out = BTreeMap::new();
    let rd = std::fs::read_dir(dir).map_err(|e| format!("读目录失败：{e}"))?;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_file() {
            if let Some(n) = p.file_name().and_then(|x| x.to_str()) {
                out.insert(n.to_string(), p);
            }
        }
    }
    Ok(out)
}

fn file_mtime_ms(p: &Path) -> Result<u64, String> {
    let md = std::fs::metadata(p).map_err(|e| format!("读文件属性失败：{e}"))?;
    let t = md.modified().map_err(|e| format!("读修改时间失败：{e}"))?;
    Ok(t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0))
}

/// 把文件 mtime 设为现在（还原用）。
fn touch(p: &Path) {
    if let Ok(f) = std::fs::File::options().write(true).open(p) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

/// 把文件 mtime 回拨为云端 mt（同步写盘 ≠ 用户操作，防乒乓）。
fn set_mtime(p: &Path, ms: u64) {
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms);
    if let Ok(f) = std::fs::File::options().write(true).open(p) {
        let _ = f.set_modified(t);
    }
}

/// 文件内容的 SHA256（前 32 hex，仲裁时省下载：哈希相同不写盘）。
fn file_hash(p: &Path) -> Result<String, String> {
    let b = std::fs::read(p).map_err(|e| format!("读文件失败：{e}"))?;
    Ok(hash_bytes(&b))
}

fn hash_bytes(b: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b);
    let d = h.finalize();
    hex(&d[..16]).to_string()
}

/// 云端文件 key：SHA256("方案/文件名") 前 32 hex（ASCII 路径）。
fn file_key(scheme: &str, fname: &str) -> String {
    hash_bytes(format!("{scheme}/{fname}").as_bytes())
}

/// manifest 文件条目里的哈希提示（有则比对省下载；无则视为不同）。
fn remote_hash_hint(rv: &Value) -> String {
    rv.get("h").and_then(|v| v.as_str()).unwrap_or("").to_string()
}

/// 回收站：把方案目录里的文件挪进去（保原名+时间戳前缀）。
fn trash_file(dir: &Path, fname: &str) -> Result<(), String> {
    let Some(data_dir) = DATA_DIR.get() else {
        return Ok(());
    };
    let src = dir.join(fname);
    if !src.is_file() {
        return Ok(());
    }
    let scheme = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("未知方案");
    let td = data_dir.join("同步回收站").join(scheme);
    std::fs::create_dir_all(&td).map_err(|e| format!("建回收站目录失败：{e}"))?;
    let dst = td.join(format!("{}-{fname}", now_ms()));
    // 同名快还原冲突：直接覆盖旧回收件
    let _ = std::fs::copy(&src, &dst);
    std::fs::remove_file(&src).map_err(|e| format!("删除本地文件失败：{e}"))?;
    prune_trash(&td);
    Ok(())
}

/// 回收站瘦身：每方案留 TRASH_KEEP 份、TRASH_DAYS 天。
fn prune_trash(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<(u64, std::fs::DirEntry)> = rd
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let ts = e
                .file_name()
                .into_string()
                .ok()?
                .split('-')
                .next()?
                .parse()
                .ok()?;
            Some((ts, e))
        })
        .collect();
    items.sort_by(|a, b| b.0.cmp(&a.0));
    let cutoff_ms = now_ms().saturating_sub(TRASH_DAYS * 24 * 3600 * 1000);
    for (i, (ts, e)) in items.into_iter().enumerate() {
        if i >= TRASH_KEEP || ts < cutoff_ms {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn count_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().filter(|e| e.path().is_file()).count())
        .unwrap_or(0)
}

// ─────────────────────────── 传输层 ───────────────────────────

fn empty_manifest() -> Value {
    json!({"v": MF_VERSION, "device": "", "ts": 0,
        "config": Value::Null,
        "devices": {}, "schemes": {}, "skins": {}})
}

/// JSON 对象键转义（方案名含 / 或 . 时 pointer 路径安全）。
fn json_escape_key(k: &str) -> String {
    // serde_json::pointer 用 ~1 表 /、~0 表 ~
    k.replace('~', "~0").replace('/', "~1")
}

fn ensure_scheme_entry(mf: &mut Value, name: &str) {
    if !mf.get("schemes").is_some_and(|v| v.is_object()) {
        mf["schemes"] = json!({});
    }
    let path = format!("/schemes/{}", json_escape_key(name));
    if mf.pointer(&path).is_none() {
        if let Some(m) = mf["schemes"].as_object_mut() {
            m.insert(name.to_string(), json!({"by": "", "ts": 0, "files": {}}));
        }
    }
}

/// manifest 里设备表刷新本机活跃时刻 + 清超窗设备。
fn prune_manifest(mf: &mut Value, s: &SyncSettings) {
    if !mf.get("devices").is_some_and(|v| v.is_object()) {
        mf["devices"] = json!({});
    }
    let cutoff = now_secs().saturating_sub(DEVICE_STALE_SECS);
    if let Some(m) = mf["devices"].as_object_mut() {
        m.retain(|_, ts| ts.as_u64().unwrap_or(0) > cutoff);
        m.insert(s.device.clone(), json!(now_secs()));
    }
    mf["device"] = json!(s.device);
    mf["ts"] = json!(now_secs());
    mf["v"] = json!(MF_VERSION);
}

/// 上传单文件：PUT hufu-sync/files/<key>（口令开启时 AES-GCM），
/// manifest 条目更新（mt/sz/h/key/n）。
fn push_file(
    agent: &ureq::Agent,
    s: &SyncSettings,
    mf: &mut Value,
    scheme: &str,
    fname: &str,
    path: &Path,
) -> Result<usize, String> {
    let raw = std::fs::read(path).map_err(|e| format!("读「{scheme}/{fname}」失败：{e}"))?;
    if raw.len() as u64 > MAX_FILE {
        return Err(format!(
            "「{scheme}/{fname}」 {} MB 超限（32 MB）——已中止",
            raw.len() / 1024 / 1024
        ));
    }
    let key = file_key(scheme, fname);
    let body = raw.clone();
    let url = url_join(&s.url, &format!("{FILES_DIR}/{key}"));
    match auth_req(
        agent.request("PUT", &url).set("Content-Type", "application/octet-stream"),
        &s.user,
        &s.pass,
    )
    .send_bytes(&body)
    {
        Ok(_) => {}
        Err(ureq::Error::Status(409, _)) => {
            mkcol(agent, s)?;
            // 重试一次
            auth_req(
                agent.request("PUT", &url).set("Content-Type", "application/octet-stream"),
                &s.user,
                &s.pass,
            )
            .send_bytes(&body)
            .map_err(|e| format!("上传「{scheme}/{fname}」失败：{e}"))?;
        }
        Err(ureq::Error::Status(401, _)) => {
            return Err("认证失败：账号或应用密码不对".into());
        }
        Err(e) => return Err(format!("上传「{scheme}/{fname}」失败：{e}")),
    }
    let mt = file_mtime_ms(path)?;
    ensure_scheme_entry(mf, scheme);
    //【洗劫事故坑】不能用 pointer_mut 走 files/<名>——键不存在时返回
    // None、条目静默丢失（文件上云、清单没有 → 下轮本地全被判对端删
    // 除）。改为索引访问直写。
    let entry = json!({
        "mt": mt, "sz": raw.len(), "h": hash_bytes(&raw),
        "key": key,
    });
    let wrote = mf
        .get_mut("schemes")
        .and_then(|v| v.as_object_mut())
        .and_then(|m| m.get_mut(scheme))
        .and_then(|e| e.get_mut("files"))
        .and_then(|v| v.as_object_mut())
        .map(|files| files.insert(fname.to_string(), entry).is_some() || true)
        .unwrap_or(false);
    if !wrote {
        return Err(format!("同步清单结构损坏（{scheme}）——已中止本轮"));
    }
    Ok(raw.len())
}

/// 下载单文件：GET → 备份旧版 → 写盘 → mtime 回拨。
fn pull_file(
    agent: &ureq::Agent,
    s: &SyncSettings,
    dir: &Path,
    fname: &str,
    key: &str,
    rmt: u64,
) -> Result<usize, String> {
    let url = url_join(&s.url, &format!("{FILES_DIR}/{key}"));
    let mut body = Vec::new();
    match auth_req(agent.request("GET", &url), &s.user, &s.pass).call() {
        Ok(r) => {
            r.into_reader()
                .take(MAX_FILE)
                .read_to_end(&mut body)
                .map_err(|e| format!("下载「{fname}」失败：{e}"))?;
        }
        Err(ureq::Error::Status(404, _)) => return Ok(0), // 清单与文件竞态：跳过
        Err(ureq::Error::Status(401, _)) => {
            return Err("认证失败：账号或应用密码不对".into());
        }
        Err(e) => return Err(format!("下载「{fname}」失败：{e}")),
    }
    let dst = dir.join(fname);
    if dst.is_file() {
        backup_file_before_write(&dst);
    }
    std::fs::write(&dst, &body).map_err(|e| format!("写入「{fname}」失败：{e}"))?;
    set_mtime(&dst, rmt);
    Ok(body.len())
}

/// 整夹拉取（本机未装该方案）：建目录 + 逐文件下载（mtime 回拨）。
fn pull_scheme_all(
    agent: &ureq::Agent,
    s: &SyncSettings,
    root: &Path,
    scheme: &str,
    entry: &Value,
) -> Result<usize, String> {
    let dir = root.join(scheme);
    std::fs::create_dir_all(&dir).map_err(|e| format!("建方案目录失败：{e}"))?;
    let files = entry.get("files").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    let mut total = 0usize;
    for (fname, rv) in &files {
        let key = rv.get("key").and_then(|v| v.as_str()).unwrap_or("");
        let rmt = rv.get("mt").and_then(|v| v.as_u64()).unwrap_or(0);
        total += pull_file(agent, s, &dir, fname, key, rmt)?;
    }
    Ok(total)
}

// ─────────────── 自带皮肤参数同步【2026-10-31 用户拍板】 ───────────────
// JSON 自带皮（hufu-*.json，安装包 20 款）的调参写进皮肤文件本体，
// 该目录不在码表同步面。现把皮肤目录作为独立文件面纳入：manifest
// 顶层 "skins" 记 {mt,sz,h,key}，mtime-LWW 与方案文件同款仲裁；键空间
// 用 file_key("builtin-skins", 文件名) 隔离（不与方案名撞键）。
// 内容哈希比对：哈希相同只对齐 mtime 不传文件——首轮 20 款皮不产生
// 「内容相同来回空转」流量。只新增/覆盖、不做删除传播（自带皮随安装
// 包存在，无「删」语义）。.ssf 皮肤文件与其 overrides 仍不同步（对机
// 没装皮肤文件时同步参数无意义）。

/// 扫描皮肤目录下的自带皮（hufu-*.json；自定义皮不带此前缀，不同步）。
fn scan_builtin_skins(dir: &Path) -> BTreeMap<String, PathBuf> {
    let mut m = BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let Some(n) = p.file_name().and_then(|x| x.to_str()) else {
                continue;
            };
            if n.starts_with("hufu-") && n.ends_with(".json") {
                m.insert(n.to_string(), p);
            }
        }
    }
    m
}

/// 上传单个皮肤文件（哈希相同时只对齐 manifest.mt，不传内容）。
fn push_skin_file(
    agent: &ureq::Agent,
    s: &SyncSettings,
    mf: &mut Value,
    name: &str,
    path: &Path,
) -> Result<(), String> {
    let raw = std::fs::read(path).map_err(|e| format!("读皮肤「{name}」失败：{e}"))?;
    if raw.len() as u64 > MAX_FILE {
        return Err(format!("皮肤「{name}」超限（32 MB）——已中止本轮"));
    }
    let mt = file_mtime_ms(path)?;
    let h = hash_bytes(&raw);
    let key = file_key("builtin-skins", name);
    // 哈希未变（只是 mtime 被安装/拷贝刷新）：对齐时刻即可，免传
    let cur = mf
        .get("skins")
        .and_then(|v| v.get(name))
        .cloned()
        .unwrap_or(Value::Null);
    if cur.get("h").and_then(|x| x.as_str()) == Some(h.as_str())
        && cur.get("sz").and_then(|x| x.as_u64()) == Some(raw.len() as u64)
    {
        if let Some(m) = mf["skins"].as_object_mut() {
            m.insert(name.to_string(), json!({"mt": mt, "sz": raw.len(), "h": h, "key": key}));
        }
        return Ok(());
    }
    let url = url_join(&s.url, &format!("{FILES_DIR}/{key}"));
    match auth_req(
        agent.request("PUT", &url).set("Content-Type", "application/octet-stream"),
        &s.user,
        &s.pass,
    )
    .send_bytes(&raw)
    {
        Ok(_) => {}
        Err(ureq::Error::Status(409, _)) => {
            mkcol(agent, s)?;
            auth_req(
                agent.request("PUT", &url).set("Content-Type", "application/octet-stream"),
                &s.user,
                &s.pass,
            )
            .send_bytes(&raw)
            .map_err(|e| format!("上传皮肤「{name}」失败：{e}"))?;
        }
        Err(ureq::Error::Status(401, _)) => {
            return Err("认证失败：账号或应用密码不对".into());
        }
        Err(e) => return Err(format!("上传皮肤「{name}」失败：{e}")),
    }
    // 索引直写（洗劫铁律：不 pointer_mut）
    let wrote = mf
        .get_mut("skins")
        .and_then(|v| v.as_object_mut())
        .map(|m| m.insert(name.to_string(), json!({"mt": mt, "sz": raw.len(), "h": h, "key": key})).is_some() || true)
        .unwrap_or(false);
    if !wrote {
        return Err("同步清单结构损坏（skins）——已中止本轮".into());
    }
    Ok(())
}

/// 自带皮肤面同步。返回 (上传文件数, 是否有下载落地)。
/// 下载落地后调用方需 bump skin_ver（DLL 立即重拉生效）。
fn sync_builtin_skins(
    agent: &ureq::Agent,
    s: &SyncSettings,
    mf: &mut Value,
) -> Result<(usize, bool), String> {
    let Some(data_dir) = DATA_DIR.get() else {
        return Ok((0, false));
    };
    let dir = data_dir.join("皮肤");
    let local = scan_builtin_skins(&dir);
    if !mf.get("skins").is_some_and(|v| v.is_object()) {
        mf["skins"] = json!({});
    }
    let remote: BTreeMap<String, (u64, String, String)> = mf["skins"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        (
                            v.get("mt").and_then(|x| x.as_u64()).unwrap_or(0),
                            v.get("key").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                            v.get("h").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        ),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let mut uploaded = 0usize;
    let mut pulled = false;
    for (name, p) in &local {
        let lmt = file_mtime_ms(p).unwrap_or(0);
        // 读一次内容复用：出厂判定（语义比较）+ 云端哈希比对
        let bytes = std::fs::read(p).ok();
        let disk_val = bytes.as_ref().and_then(|b| serde_json::from_slice::<Value>(b).ok());
        //【重装原版覆盖防线 2026-10-31】本地皮肤==内嵌出厂版（未调过）
        // 且云端有记录 → 无条件以云端为准：重装机的原版文件 mtime 天然
        // 晚于旧机调参后的文件，纯「最后修改的赢」会拿原版洗掉云端调参
        //（实锤：旧机竖排沉雾险些被重装机横排原版覆盖）。未调过的本地
        // 版不含用户意图，不参与仲裁。
        let is_factory = disk_val
            .as_ref()
            .map(|v| crate::host::official_skin_unchanged(name, v))
            .unwrap_or(false);
        let local_hash = bytes.as_ref().map(|b| hash_bytes(b));
        match remote.get(name) {
            Some((rmt, key, rh)) if is_factory => {
                // 云端为准（内容相同只回拨 mtime，免下载）
                if local_hash.as_deref() == Some(rh.as_str()) {
                    set_mtime(p, *rmt);
                } else {
                    pull_file(agent, s, &dir, name, key, *rmt)?;
                    pulled = true;
                }
            }
            Some((rmt, key, rh)) if *rmt > lmt => {
                // 云端新：哈希相同只回拨 mtime 对齐（内容没变免下载）
                if local_hash.as_deref() == Some(rh.as_str()) {
                    set_mtime(p, *rmt);
                } else {
                    pull_file(agent, s, &dir, name, key, *rmt)?;
                    pulled = true;
                }
            }
            Some((rmt, _, _)) if lmt > *rmt => {
                push_skin_file(agent, s, mf, name, p)?;
                uploaded += 1;
            }
            Some(_) => {} // 同刻：不动
            None => {
                push_skin_file(agent, s, mf, name, p)?;
                uploaded += 1;
            }
        }
    }
    // 云端有、本地无：落地（旧版安装包缺皮时补齐；不做删除方向）
    for (name, (rmt, key, _)) in &remote {
        if !local.contains_key(name) {
            let _ = std::fs::create_dir_all(&dir);
            pull_file(agent, s, &dir, name, key, *rmt)?;
            pulled = true;
        }
    }
    Ok((uploaded, pulled))
}

/// 写盘前备份到 数据\同步备份\ts-<秒>\（整轮备份，保 10 份）。
fn backup_file_before_write(src: &Path) {
    let Some(data_dir) = DATA_DIR.get() else {
        return;
    };
    let scheme = src
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("方案");
    let bd = data_dir.join("同步备份").join(format!("ts-{}", now_secs()));
    let _ = std::fs::create_dir_all(&bd);
    if let Some(name) = src.file_name().and_then(|n| n.to_str()) {
        let _ = std::fs::copy(src, bd.join(format!("{scheme}-{name}")));
    }
}

fn backup_config(data_dir: &Path) {
    let src = data_dir.join("config.json");
    if src.is_file() {
        let bd = data_dir.join("同步备份").join(format!("ts-{}", now_secs()));
        let _ = std::fs::create_dir_all(&bd);
        let _ = std::fs::copy(&src, bd.join("config.json"));
    }
}

/// 备份目录瘦身（按名倒序保留 BACKUP_KEEP 份）。
fn prune_backups(data_dir: &Path) {
    let bd = data_dir.join("同步备份");
    let mut rounds: Vec<PathBuf> = std::fs::read_dir(&bd)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    rounds.sort();
    while rounds.len() > BACKUP_KEEP {
        let p = rounds.remove(0);
        let _ = std::fs::remove_dir_all(p);
    }
}

// ─────────────────────────── manifest 存取 ───────────────────────────

fn fetch_manifest(agent: &ureq::Agent, s: &SyncSettings) -> Result<Option<Value>, String> {
    match webdav_get_body(agent, s, MF_PATH, MAX_MF as u64) {
        Ok(None) => Ok(None),
        Ok(Some(b)) => {
            let mf: Value =
                serde_json::from_slice(&b).map_err(|_| "云端清单损坏（非 JSON）".to_string())?;
            let v = mf.get("v").and_then(|v| v.as_u64()).unwrap_or(0);
            if v != MF_VERSION {
                return Err(format!(
                    "云端同步数据版本 v{v} 不支持（本机只认 v{MF_VERSION}，两台电脑都装最新版虎符即可）"
                ));
            }
            Ok(Some(mf))
        }
        Err(e) => Err(e),
    }
}

fn put_manifest(agent: &ureq::Agent, s: &SyncSettings, mf: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(mf).map_err(|e| e.to_string())?;
    webdav_put_body(agent, s, MF_PATH, &body, "application/json")
}

// ─────────────────────────── config 归并 ───────────────────────────

/// config 消毒：剔除不同步键（就地深删）。
pub fn sanitize_config(cfg: &mut Value) {
    for path in CONFIG_EXCLUDED {
        let mut segs: Vec<&str> = path.split('.').collect();
        let last = segs.pop().unwrap_or("");
        // 顶层键直接删；嵌套键走 pointer
        if segs.is_empty() {
            if let Some(m) = cfg.as_object_mut() {
                m.remove(last);
            }
        } else if let Some(obj) = cfg.pointer_mut(&segs_to_pointer(&segs)) {
            if let Some(m) = obj.as_object_mut() {
                m.remove(last);
            }
        }
    }
}

fn segs_to_pointer(segs: &[&str]) -> String {
    if segs.is_empty() {
        "/".into()
    } else {
        format!("/{}", segs.join("/"))
    }
}

/// 三方归并：full（本机全量）× remote → merged。base=上次同步快照。
fn three_way_merge(full: &Value, local_san: &Value, base: &Value, remote: &Value) -> Value {
    let mut out = full.clone();
    overlay_remote(&mut out, local_san, base, remote);
    out
}

/// 递归：远端项变了（≠base）且本机没动（local==base）→ 采纳远端。
fn overlay_remote(out: &mut Value, local: &Value, base: &Value, remote: &Value) {
    match (out, local, base, remote) {
        (Value::Object(o), Value::Object(l), Value::Object(b), Value::Object(r)) => {
            for (k, rv) in r {
                let lv = l.get(k);
                let bv = b.get(k);
                let same_as_base = lv == bv;
                if let Some(ov) = o.get_mut(k) {
                    let both_obj = ov.is_object() && rv.is_object();
                    if same_as_base && both_obj {
                        // 本机没动且双方都是对象：递归细粒度采纳
                        overlay_remote(ov, lv.unwrap_or(&Value::Null), bv.unwrap_or(&Value::Null), rv);
                    } else if same_as_base {
                        // 本机没动：整段采纳远端
                        *ov = rv.clone();
                    } else if both_obj {
                        // 本机动了：只在对象层递归（叶子保留本机）
                        overlay_remote(ov, lv.unwrap_or(&Value::Null), bv.unwrap_or(&Value::Null), rv);
                    }
                    // 本机动了且非对象：保留本机（什么都不做）
                } else if same_as_base {
                    // 本机没有该键且没动过：采纳远端
                    o.insert(k.clone(), rv.clone());
                }
            }
        }
        _ => {}
    }
}

// ─────────────────────────── WebDAV ───────────────────────────

/// 标准 Base64（Authorization 头用）。
fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

fn auth_req(r: ureq::Request, user: &str, pass: &str) -> ureq::Request {
    let cred = b64(format!("{user}:{pass}").as_bytes());
    r.set("Authorization", &format!("Basic {cred}"))
}

fn build_agent() -> ureq::Agent {
    let b = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout(Duration::from_secs(30));
    #[cfg(windows)]
    let b = match ureq::native_tls::TlsConnector::new() {
        Ok(c) => b.tls_connector(std::sync::Arc::new(c)),
        Err(_) => b,
    };
    b.build()
}

fn url_join(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path)
}

fn mkcol(agent: &ureq::Agent, s: &SyncSettings) -> Result<(), String> {
    for d in ["hufu-sync", "hufu-sync/files"] {
        let col = url_join(&s.url, d);
        let _ = auth_req(agent.request("MKCOL", &col), &s.user, &s.pass).call();
    }
    Ok(())
}

fn webdav_get_body(
    agent: &ureq::Agent,
    s: &SyncSettings,
    path: &str,
    limit: u64,
) -> Result<Option<Vec<u8>>, String> {
    let url = url_join(&s.url, path);
    match auth_req(agent.request("GET", &url), &s.user, &s.pass).call() {
        Ok(r) => {
            let mut buf = Vec::new();
            r.into_reader()
                .take(limit * 2)
                .read_to_end(&mut buf)
                .map_err(|e| format!("下载失败：{e}"))?;
            if buf.len() as u64 > limit {
                return Err(format!("云端资源超限（{} KB）", buf.len() / 1024));
            }
            Ok(Some(buf))
        }
        Err(ureq::Error::Status(404, _)) | Err(ureq::Error::Status(409, _)) => Ok(None),
        Err(ureq::Error::Status(401, _)) => {
            Err("认证失败：账号或应用密码不对".into())
        }
        Err(e) => Err(format!("网络失败：{e}")),
    }
}

fn webdav_put_body(
    agent: &ureq::Agent,
    s: &SyncSettings,
    path: &str,
    body: &[u8],
    ctype: &str,
) -> Result<(), String> {
    let put = |u: &str| {
        auth_req(
            agent.request("PUT", u).set("Content-Type", ctype),
            &s.user,
            &s.pass,
        )
        .send_bytes(body)
    };
    let url = url_join(&s.url, path);
    match put(&url) {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(409, _)) => {
            mkcol(agent, s)?;
            put(&url).map(|_| ()).map_err(|e| format!("上传失败：{e}"))
        }
        Err(ureq::Error::Status(401, _)) => {
            Err("认证失败：账号或应用密码不对".into())
        }
        Err(e) => Err(format!("上传失败：{e}")),
    }
}

fn webdav_file_del(agent: &ureq::Agent, s: &SyncSettings, key: &str) -> Result<(), String> {
    let url = url_join(&s.url, &format!("{FILES_DIR}/{key}"));
    match auth_req(agent.request("DELETE", &url), &s.user, &s.pass).call() {
        Ok(_) | Err(ureq::Error::Status(404, _)) => Ok(()),
        Err(ureq::Error::Status(401, _)) => {
            Err("认证失败：账号或应用密码不对".into())
        }
        Err(e) => Err(format!("删除失败：{e}")),
    }
}

fn webdav_del(agent: &ureq::Agent, s: &SyncSettings, path: &str) -> Result<(), String> {
    let url = url_join(&s.url, path);
    match auth_req(agent.request("DELETE", &url), &s.user, &s.pass).call() {
        Ok(_) | Err(ureq::Error::Status(404, _)) => Ok(()),
        Err(ureq::Error::Status(401, _)) => {
            Err("认证失败：账号或应用密码不对".into())
        }
        Err(e) => Err(format!("删除失败：{e}")),
    }
}

// ─────────────────────────── hex ───────────────────────────

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

// ─────────────────────────── 测试 ───────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 文件 key：稳定、纯 ASCII、同方案不同文件不同 key。
    #[test]
    fn file_key_stable_and_ascii() {
        let a = file_key("茉莉", "主码表.txt");
        let b = file_key("茉莉", "用户调整.txt");
        let a2 = file_key("茉莉", "主码表.txt");
        assert_eq!(a, a2);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(a.len(), 32);
    }

    /// 消毒：排除键剔除，其余保留（皮肤选择与 schema 纳入同步；模型设置被剔）。
    #[test]
    fn config_sanitize_exclusions() {
        let mut cfg = json!({
            "schema": {"current": "A"},
            "appearance": {"skin": "hufu-moyan", "skin_kind": "json", "font_size": 18},
            "sentence": {"enabled": true, "ngram_path": "模型/x.bin"},
            "general": {"autostart": true, "export_key": "ctrl+x"},
            "candidates": {"page_size": 4, "diff_hl": true}
        });
        sanitize_config(&mut cfg);
        // 皮肤选择（自带皮重装必在）与当前方案、其他偏好保留（同步到其他机器）
        assert_eq!(cfg["appearance"]["skin"], "hufu-moyan");
        assert_eq!(cfg["appearance"]["skin_kind"], "json");
        assert_eq!(cfg["schema"]["current"], "A");
        assert_eq!(cfg["candidates"]["diff_hl"], true);
        // 整句模型设置（机器相关）与机器本地字段被剔除
        assert!(cfg.get("sentence").is_none());
        assert!(cfg["general"].get("autostart").is_none());
        assert!(cfg["general"].get("export_key").is_none());
        assert!(cfg["appearance"].get("font_family").is_none());
    }

    /// 三方归并：本机没动→采纳远端；本机动了→保留本机。
    #[test]
    fn three_way_basic() {
        let full = json!({"a": 1, "b": 2, "font_family": "宋体"});
        let mut local_san = full.clone();
        sanitize_config(&mut local_san); // font_family 被剔除
        let base = json!({"a": 1, "b": 1});
        let remote = json!({"a": 9, "b": 5});
        let merged = three_way_merge(&full, &local_san, &base, &remote);
        // a 本机没动（local==base）→ 采纳远端
        assert_eq!(merged["a"], 9);
        // b 本机动了（2≠1）→ 保留本机
        assert_eq!(merged["b"], 2);
        // 排除键不被远端冲
        assert_eq!(merged["font_family"], "宋体");
    }

    /// manifest 键转义：方案名含 / 时 pointer 安全。
    #[test]
    fn pointer_escape_slash() {
        let mut mf = empty_manifest();
        ensure_scheme_entry(&mut mf, "a/b");
        assert!(mf.pointer("/schemes/a~1b").is_some());
    }

    /// 空清单 roundtrip。
    #[test]
    fn manifest_roundtrip() {
        let mf = empty_manifest();
        let s = serde_json::to_string(&mf).unwrap();
        let back: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back["v"], MF_VERSION);
    }
}
