//! 【P1 云同步 2026-10-28】WebDAV 后端：多机同步「用户调整.txt（各方案）
//! + 配置设置（剔皮肤挂件/机器本地字段）」。服务器 = 用户自备 WebDAV
//! （坚果云等），虎符官方不设服务器、不经手任何用户数据。
//!
//! 同步算法（一次 run_sync = 先拉后推）：
//! 1. GET 云端同步包（404=云端为空）；解析（加密包先解密）；
//! 2. merge_local：远端 → 本机合并（Host 锁内，与用户操作同写纪律）：
//!    - 用户调整 = 行并集去重（远端行序在前、本机独有行追加在后）——
//!      同键操作后写的行胜，回放语义（最新覆盖）天然收敛；
//!    - config = 三方归并（base=上次同步快照 last_config）：远端某项
//!      变了且本机没动 → 采纳远端；本机变了 → 保留本机（不会被云端
//!      旧值冲回）；两边都动同一项 → 本机胜 → apply_config；
//!    - 合并写盘不主动 reload：2Hz 码表监视器 ~1s 自动轻量重载（打字
//!      无感），config 走 apply_config 自带的后台重建；
//!    - 每轮首次写盘前自动备份到 数据\同步备份\ts-<秒>\（保留 10 份）；
//! 3. collect_local 收集本机全量 → build_package → 内容无变化跳过上传
//!    （data 部分规范哈希比对，坚果云流量友好）→ PUT。
//!
//! 触发：启动延迟 20s / 周期 5min / 码表监视器通知（防抖 8s 合并连发，
//! "手动"不防抖）/ 设置保存。全部在独立线程，网络全超时（连 8s/读 30s），
//! 失败静默退避记入 last_error——**红线：绝不触碰/阻塞打字路径**。
//!
//! 加密（可选口令，强烈建议）：PBKDF2-HMAC-SHA256(300k) → AES-256-GCM，
//! 云端只见密文；口令两台机必须一致。口令为空=明文包（UI 有警示）。
//!
//! 已知边界：本机没有的方案**不落盘**远端调整文件——目录里只有一份
//! 用户调整.txt 会被 Schema::load「最大文件胜选」当成主码表（schema.rs
//! 有注释实证）；换新机先装方案目录再同步。
//!
//! 凭据存放：数据\同步.json（明文，与本机 config.json/用户调整同信任
//! 级；WebDAV 用应用密码非登录密码，泄露面已收窄）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::host::Host;

/// 同步包格式版本（v=1：schemas+config 单包）
const PKG_VERSION: u64 = 1;
/// 云端路径（相对 WebDAV 根）
const PKG_PATH: &str = "hufu-sync/sync.json";
/// 口令派生轮数（PBKDF2-HMAC-SHA256）
const KDF_ROUNDS: u32 = 300_000;
/// 同步包上限（防用户调整异常膨胀打爆坚果云配额）
const MAX_PKG: usize = 2 * 1024 * 1024;
/// 周期同步间隔
const PERIOD: Duration = Duration::from_secs(300);
/// 启动延迟（让引擎/模型先就绪）
const START_DELAY: Duration = Duration::from_secs(20);
/// 变化通知防抖（合并连发）
const DEBOUNCE: Duration = Duration::from_secs(8);
/// 备份保留份数
const BACKUP_KEEP: usize = 10;
/// 「保持已存值」哨兵（UI 留空不覆盖已存密钥/口令）
const KEEP: &str = "__keep__";
/// 【默认地址 2026-10-29】服务器地址留空=坚果云官方 WebDAV（UI 地址框
/// 的 placeholder 一直是它；此前值为空保存会被判「未配置」，用户被迫
/// 手填一次才生效——现在 save 端同样归一，空=默认）。
const DEFAULT_URL: &str = "https://dav.jianguoyun.com/dav/";

/// 不同步的 config 键（皮肤挂件/机器本地字段/每机方案选择）。
/// 排除即不同步也不被远端覆盖（overlay 只写远端携带的键）。
const CONFIG_EXCLUDED: &[&str] = &[
    "schema", // 当前方案/目录 = 每机自己的选择
    "appearance.skin",
    "appearance.skin_kind",
    "appearance.font_family", // 字体装没装每机不同
    "general.autostart",      // 开机自启 = 机器属性
    "general.export_key",     // 本机录制的快捷键
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
    /// 加密口令（空 = 明文包）
    pub passphrase: String,
    /// 上次成功同步时刻（epoch 秒）
    pub last_sync: u64,
    /// 上次成功同步时的可同步配置快照（三方归并 base；None=首次）
    pub last_config: Option<Value>,
    /// 各方案「用户调整.txt」上次收敛快照（重置检测的 base：
    /// 有快照而本机空了=本机导出合并过；云端没了=对端导出合并过）
    #[serde(default)]
    pub snapshots: std::collections::BTreeMap<String, String>,
    pub last_error: Option<String>,
}

static TX: OnceLock<Sender<&'static str>> = OnceLock::new();
static SHARED: OnceLock<Arc<Mutex<Host>>> = OnceLock::new();
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
/// 内存真源（spawn 时从 数据\同步.json 载入；save/persist 同步写回）
static SETTINGS: Mutex<Option<SyncSettings>> = Mutex::new(None);
static PHASE: Mutex<&'static str> = Mutex::new("idle");

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
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
    // 【保存前备份 2026-10-29】同步.json 落盘前把旧版复制为 同步.json.bak
    //（凭据唯一存放处，误清空/写坏时可一键回滚——今晚真机探针事故实证
    // 此前无任何护栏：一次畸形请求即可洗掉全部凭据）。
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
    json!({
        "enabled": !s.url.trim().is_empty() && !s.user.trim().is_empty() && !s.pass.is_empty(),
        "url": s.url, "user": s.user, "device": s.device,
        "passphrase_set": !s.passphrase.is_empty(),
        "last_sync": s.last_sync, "last_error": s.last_error,
        "phase": phase,
    })
}

/// POST /api/sync/save。pass/passphrase 传 `__keep__` = 保持已存值
/// （UI 密钥框留空防误清）；passphrase 显式空串 = 停用加密。
pub fn save_settings(
    url: &str,
    user: &str,
    pass: &str,
    device: &str,
    passphrase: &str,
) -> Result<(), String> {
    if DATA_DIR.get().is_none() {
        return Err("同步模块未初始化".into());
    }
    let mut guard = SETTINGS.lock().unwrap_or_else(|e| e.into_inner());
    let st = guard.get_or_insert_with(SyncSettings::default);
    // 【误清空护栏 2026-10-29】此前已启用（user+pass 齐备）而本次请求会把
    // pass 以字面空串覆盖（非 __keep__）且 user 也为空 ＝ 疑似事故性整清
    //（坏客户端/引号丢失的畸形请求——UI 正常路径密钥类永远带 __keep__）。
    // 拒绝并保留原值；确要重置：删除 数据\同步.json 或改用别的账号覆盖。
    let was_enabled = !st.user.is_empty() && !st.pass.is_empty();
    if was_enabled && pass != KEEP && pass.is_empty() && user.trim().is_empty() {
        return Err("已拒绝：该请求会清空已保存的同步账号（疑似误操作）；如确要重置，请删除 安装目录\\数据\\同步.json 后重新配置".into());
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
    if passphrase != KEEP {
        st.passphrase = passphrase.to_string();
    }
    persist(st).map_err(|e| format!("写入同步配置失败：{e}"))?;
    let ready =
        !st.url.is_empty() && !st.user.is_empty() && !st.pass.is_empty();
    drop(guard);
    if ready {
        notify("设置");
    }
    Ok(())
}

/// POST /api/sync/test（HTTP 线程锁外调用，含网络 IO）。
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
    match webdav_get(&agent, &s) {
        Ok(None) => json!({"ok": true, "exists": false,
            "msg": "连接成功：云端还没有同步包（首次同步将创建）"}),
        Ok(Some(body)) => match parse_package(&body, &s.passphrase) {
            Ok(pkg) => {
                let schemas = pkg
                    .pointer("/data/schemas")
                    .and_then(|v| v.as_object())
                    .map(|m| m.len());
                json!({"ok": true, "exists": true,
                    "remote_device": pkg.get("device").and_then(|v| v.as_str()).unwrap_or("?"),
                    "remote_ts": pkg.get("ts").and_then(|v| v.as_u64()).unwrap_or(0),
                    "schemas": schemas})
            }
            Err(e) => json!({"ok": false, "msg": e}),
        },
        Err(e) => json!({"ok": false, "msg": e}),
    }
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

/// 将「本机未装方案」的远端条目并入待上传数据（键已存在则不动——
/// 装好方案后本地合并结果优先）。云端数据不被未装方案的机器挤掉。
fn union_carried(local: &mut Value, carried: &Value) {
    let Some(cm) = carried.as_object() else { return };
    if cm.is_empty() {
        return;
    }
    if let Some(obj) = local.get_mut("schemas").and_then(|v| v.as_object_mut()) {
        for (k, v) in cm {
            obj.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
}

/// GET /api/sync/tutorial：图文教程 PNG（优先 数据\同步教程.png——随
/// 安装分发；兜底当前用户桌面，方便未放数据目录时也能打开）。
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

fn run_sync(reason: &'static str) {
    let s = SETTINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default();
    if s.url.trim().is_empty() || s.user.trim().is_empty() || s.pass.is_empty() {
        return;
    }
    set_phase("syncing");
    let r = do_sync(&s);
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

fn do_sync(s: &SyncSettings) -> Result<String, String> {
    let agent = build_agent();
    // 1. 拉
    let remote_pkg = match webdav_get(&agent, s)? {
        Some(b) => Some(parse_package(&b, &s.passphrase)?),
        None => None,
    };
    // 2. 合并进本机（carried=本机未装方案而原样保留的云端条目）
    let (mut merged, carried) = if let Some(pkg) = &remote_pkg {
        let data = pkg.get("data").cloned().ok_or("同步包缺 data 字段")?;
        merge_local(&data)?
    } else {
        (false, Value::Object(serde_json::Map::new()))
    };
    // 3. 收集本机 → 未装方案的云端条目并回（不丢）→ 无变化跳过上传
    let mut local = collect_local()?;
    union_carried(&mut local, &carried);
    let lh = data_hash(&local);
    if let Some(pkg) = &remote_pkg {
        if !merged && data_hash(pkg.get("data").unwrap_or(&Value::Null)) == lh {
            refresh_merge_base(&local);
            snapshot_after_sync();
            return Ok("云端与本机一致，无需上传".into());
        }
    }
    let n = local
        .pointer("/schemas")
        .and_then(|v| v.as_object())
        .map(|m| m.len())
        .unwrap_or(0);
    let body = build_package(&local, &s.device, &s.passphrase)?;
    if body.len() > MAX_PKG {
        return Err(format!(
            "同步包 {} KB 超限（2 MB）——用户调整异常膨胀，已中止上传",
            body.len() / 1024
        ));
    }
    // 4. 推
    webdav_put(&agent, s, &body)?;
    refresh_merge_base(&local);
    snapshot_after_sync();
    Ok(format!(
        "已上传 {n} 个方案的用户调整 + 配置（{} B）",
        body.len()
    ))
}

/// 同步收敛后刷新三方归并 base（= 本次收敛后的本机可同步配置快照）。
fn refresh_merge_base(local: &Value) {
    let mut guard = SETTINGS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(st) = guard.as_mut() {
        st.last_config = local.pointer("/config").cloned();
    }
}

/// 同步收敛后记录各方案「用户调整.txt」收敛快照（下轮合并的 base，
/// 也是重置检测的参照）。只存非空内容。
fn snapshot_after_sync() {
    let Some(shared) = SHARED.get() else { return };
    let h = shared.lock().unwrap_or_else(|p| p.into_inner());
    let root = schema_root(&h);
    let mut snap = std::collections::BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if let Ok(c) = std::fs::read_to_string(p.join("用户调整.txt")) {
                if !c.trim().is_empty() {
                    snap.insert(name.to_string(), c);
                }
            }
        }
    }
    let mut guard = SETTINGS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(st) = guard.as_mut() {
        st.snapshots = snap;
        let _ = persist(st);
    }
}

// ─────────────────────── 本机收集 / 合并 ───────────────────────

/// 码表根（码表\）：与码表监视器同源解析（resolve_data_sub）。
fn schema_root(h: &Host) -> PathBuf {
    let data_dir = DATA_DIR.get().expect("sync 未初始化");
    hufu_engine::Engine::resolve_data_sub(data_dir, &h.engine.config.schema.dir)
}

/// Host 锁内收集：各方案 用户调整.txt 全文 + 消毒后的 config。
fn collect_local() -> Result<Value, String> {
    let shared = SHARED.get().ok_or("同步模块未初始化")?;
    let h = shared.lock().unwrap_or_else(|p| p.into_inner());
    let root = schema_root(&h);
    let mut schemas = serde_json::Map::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let adj = p.join("用户调整.txt");
            if adj.is_file() {
                if let Ok(c) = std::fs::read_to_string(&adj) {
                    if !c.trim().is_empty() {
                        schemas.insert(name.to_string(), Value::String(c));
                    }
                }
            }
        }
    }
    let mut cfg = serde_json::to_value(&h.engine.config).map_err(|e| e.to_string())?;
    sanitize_config(&mut cfg);
    Ok(json!({"schemas": Value::Object(schemas), "config": cfg}))
}

/// Host 锁内合并远端 → 本机（写盘纪律与用户操作一致）。
/// 返回 (是否有落盘变化, 本机未装方案而需原样透传的远端条目)。
fn merge_local(remote: &Value) -> Result<(bool, Value), String> {
    let shared = SHARED.get().ok_or("同步模块未初始化")?;
    let mut h = shared.lock().unwrap_or_else(|p| p.into_inner());
    let data_dir = DATA_DIR.get().expect("sync 未初始化").clone();
    let root = schema_root(&h);
    let mut changed = false;
    let mut backup: Option<PathBuf> = None;
    let mut carried = serde_json::Map::new();

    // 用户调整：名字全集 = 远端方案 ∪ 本地方案目录（只写本机已有目录，
    // 孤儿方案防主码表误选；未装方案的远端条目 carried 透传）。
    // 重置语义（导出合并/手动清空后开始新记录）：有收敛快照 base 而
    // 一侧空了 = 该侧重置 → 另一侧只保留 base 之后的新增，不拉回旧记录。
    if let Some(schemas) = remote.pointer("/schemas").and_then(|v| v.as_object()) {
        let snapshots = SETTINGS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|s| s.snapshots.clone())
            .unwrap_or_default();
        let mut names: Vec<String> = schemas.keys().cloned().collect();
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                if let Some(n) = e.path().file_name().and_then(|x| x.to_str()) {
                    if !names.iter().any(|x| x == n) {
                        names.push(n.to_string());
                    }
                }
            }
        }
        for name in names {
            let dir = root.join(&name);
            if !dir.is_dir() {
                // 本机未装该方案：云端条目原样透传保留（防挤掉）
                if let Some(rv) = schemas.get(&name) {
                    carried.insert(name.clone(), rv.clone());
                }
                continue;
            }
            let p = dir.join("用户调整.txt");
            // 读失败=中止本轮（不误判为重置）；不存在=None
            let lc = match std::fs::read_to_string(&p) {
                Ok(c) => Some(c),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(format!("读「{name}」用户调整失败：{e}")),
            };
            let rc = schemas.get(&name).and_then(|v| v.as_str());
            let base = snapshots.get(&name).map(|s| s.as_str());
            // 【三态语义 2026-10-29】None=保持现状（不写盘）；Some(空串)
            // =明确清空。洗词事故回归锁：云端没有该方案条目且无 base 时
            // 本机内容必须原样保留（随后 collect 会把它推上云补齐），
            // 绝不能落成空文件。
            if let Some(target) = merge_adjust_v2(rc, lc.as_deref(), base) {
                let current = lc.clone().unwrap_or_default();
                if target != current {
                    let bd = ensure_backup(&data_dir, &mut backup);
                    if p.is_file() {
                        let _ = std::fs::copy(&p, bd.join(format!("{name}-用户调整.txt")));
                    }
                    std::fs::write(&p, target)
                        .map_err(|e| format!("写入「{name}」用户调整失败：{e}"))?;
                    changed = true;
                }
            }
        }
    }

    // config：三方归并（base=上次同步快照）。远端变了且本机没动 → 采纳
    // 远端；本机变了 → 保留本机（修复：远端旧值不得冲回本机新设置）；
    // 两边都动同一项 → 本机胜。排除字段天然不参与（远端已消毒）。
    if let Some(rcfg) = remote.get("config") {
        if rcfg.is_object() {
            let full =
                serde_json::to_value(&h.engine.config).map_err(|e| e.to_string())?;
            let mut local_san = full.clone();
            sanitize_config(&mut local_san);
            // base 缺失（首次同步）＝以本机当前态为 base：远端与本机不同
            // 且本机自同步以来未动的键采纳远端，本机独有改动天然保留。
            let base = last_config_snapshot().unwrap_or_else(|| local_san.clone());
            let merged = three_way_merge(&full, &local_san, &base, rcfg);
            if merged != full {
                let cfg_path = data_dir.join("config.json");
                if cfg_path.is_file() {
                    let bd = ensure_backup(&data_dir, &mut backup);
                    let _ = std::fs::copy(&cfg_path, bd.join("config.json"));
                }
                let cfg: hufu_config::Config = serde_json::from_value(merged)
                    .map_err(|e| format!("归并配置无法解析：{e}"))?;
                let (need_sentence, teardown) = h
                    .apply_config(cfg)
                    .map_err(|e| format!("应用配置失败：{e}"))?;
                if need_sentence {
                    // 锁内调用安全：reload_sentence_bg 用 HOST_HANDLE，
                    // 重锁在其内部线程（与 /api/config 路由同路径）
                    crate::reload_sentence_bg(teardown, true);
                }
                changed = true;
            }
        }
    }

    if backup.is_some() {
        prune_backups(&data_dir);
    }
    Ok((changed, Value::Object(carried)))
}

/// a 的行序中剔除 b 里出现过的行（CRLF 容忍、保序去重、跳过空行）。
fn diff_lines(a: &str, b: &str) -> String {
    let bs: std::collections::BTreeSet<&str> =
        b.lines().map(|x| x.trim_end_matches('\r')).collect();
    let mut seen = std::collections::BTreeSet::new();
    let mut out: Vec<&str> = Vec::new();
    for line in a.lines() {
        let t = line.trim_end_matches('\r');
        if !t.is_empty() && !bs.contains(t) && seen.insert(t) {
            out.push(t);
        }
    }
    out.join("\n")
}

/// 带收敛快照 base 的三态合并（导出合并/清空 = 重置语义）：
/// - 无 base（该方案首次参与同步）：云端没有 → **保持本机现状**
///   （本机内容随 collect 上传补齐云端——洗词事故 2026-10-29 的修复：
///   旧版此处返回 None 被调用方拍平成空串写盘，云端只要缺条目就把
///   本机有词的文件洗空，再被 collect 推空、恶性循环）；云端有 →
///   并集；
/// - 有 base 且本机空了（本机导出合并清空了日志）→ 只留远端新增
///   （远端 − base），旧记录不再拉回本地；无新增=维持空（本就空，
///   无写盘必要）；
/// - 有 base 且云端没了该方案（对端导出合并）→ 只留本机新增；
///   无新增=明确清空本机旧记录（Some("")）；
/// - 双方都空 → 维持现状；双方都在 → 并集。
/// remote/local 传 None=云端没有该方案 / 本地文件缺失或空。
/// 返回 Some=应写入的内容（空串=明确清空）；**None=保持现状不写盘**。
fn merge_adjust_v2(remote: Option<&str>, local: Option<&str>, base: Option<&str>) -> Option<String> {
    let has = |s: Option<&str>| s.is_some_and(|x| !x.trim().is_empty());
    if !has(base) {
        return match (has(remote), has(local)) {
            // 云端没有该方案：保持本机现状（本机内容 collect 时上传补齐）
            (false, _) => None,
            (true, false) => Some(remote.unwrap().to_string()),
            (true, true) => Some(merge_adjust(remote.unwrap(), local.unwrap())),
        };
    }
    match (has(remote), has(local)) {
        (false, false) => None,
        (true, false) => {
            // 本机重置：远端只留 base 之后的新增；无新增=维持空（不写盘）
            let m = diff_lines(remote.unwrap(), base.unwrap());
            if m.is_empty() { None } else { Some(m) }
        }
        (false, true) => {
            // 远端重置：本机只留 base 之后的新增；无新增=明确清空旧记录
            let m = diff_lines(local.unwrap(), base.unwrap());
            Some(if m.is_empty() { String::new() } else { m })
        }
        (true, true) => Some(merge_adjust(remote.unwrap(), local.unwrap())),
    }
}

/// 用户调整合并（【语义合并 2026-10-30】恶性洗词修复，替代纯行并集）。
///
/// 事故链（真机实锤，码表方案「茉莉」）：A 机加词上传 → B 机文件
/// 并集后，B 机本地的 `{整序}` 快照组（整序=清空重建、清单外即删，
/// user.rs set_order）恰好排在跨机加词行**之后** → 回放时快照重建
/// 把对方加的词挤出候选（词不显示=每 5 分钟洗回）；且 B 机右键删词
/// 落盘 `{删除}词`+整套快照重写（{删除}/{整序}/{添加}pN 三行）上传
/// → 并集把同 (码,词) 的 {添加} 与 {删除} **双行共存**，回放「后写
/// 赢」→ 删除行在后者永远杀词（重新添加也死循环）。
///
/// 合并语义改为与回放一致的**键终态**：
/// - 同 (码,词) 的 {添加}/{删除}/{置顶} 只保留拼接流（远端在前、
///   本机在后）中**最后出现**的操作行——中间态行全部淘汰。删词
///   传播（终态=删除）与加词传播（终态=添加）都与单机语义对齐；
/// - 同码多份 `{整序}` 快照组只保留最后一份（早份重建后必被晚份
///   覆盖，只留终态）；
/// - **快照后置**：同码无 pN 的 {添加} 行（跨机单行加词）一律排到
///   该码最后一份快照组之后——回放序=快照重建后再加词，对方机器
///   加的词不再被孤儿淘汰（本修复的断根点）；
/// - {加权} 行按整行去重（同词可多轮调权，保留最后值）；
/// - 其余行维持旧行为：远端行序在前、本机独有行追加在后。
fn merge_adjust(remote: &str, local: &str) -> String {
    /// 行解析：(标记, 码, 词, 有无 pN)。非标记行=(原始行,空,空,false)。
    fn parse(l: &str) -> (&str, &str, &str, bool) {
        for tag in ["{添加}", "{删除}", "{置顶}", "{加权}"] {
            if let Some(rest) = l.strip_prefix(tag) {
                let mut it = rest.split('\t');
                let code = it.next().unwrap_or("");
                let word = it.next().unwrap_or("");
                let has_pn = it.next().is_some_and(|p| p.starts_with('p'));
                return (tag, code, word, has_pn);
            }
        }
        if let Some(code) = l.strip_prefix("{整序}") {
            return ("{整序}", code, "", false);
        }
        ("", "", "", false)
    }

    // 1. 拼接行流（远端在前本机在后），保序、trim \r、跳过空行。
    //    【键行不去重】{添加}/{删除}/{置顶}/{加权}/{整序} 行保留两侧
    //    全部出现——local 的「重新添加」行可能与 remote 历史行**字节
    //    相同**（本案实锤），整行去重会把它吃掉、让墓碑 {删除} 行
    //    错成终态。键行靠第 2 步终态过滤自然收敛，位置=时序。
    let mut lines: Vec<&str> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for l in remote.lines().chain(local.lines()) {
        let l = l.trim_end_matches('\r');
        if l.trim().is_empty() {
            continue;
        }
        let (tag, code, word, _) = parse(l);
        let is_key_line = (!code.is_empty() && !word.is_empty())
            || (tag == "{整序}" && !code.is_empty());
        if !is_key_line && !seen.insert(l) {
            continue;
        }
        lines.push(l);
    }

    // 2. 键终态：同 (码,词) 的 {添加}/{删除}/{置顶}/{加权} 只留最后出现者
    //    （加权多轮调权同键收敛，顺带去重）
    let mut last_op_at: HashMap<(&str, &str), usize> = HashMap::new();
    for (i, l) in lines.iter().enumerate() {
        let (tag, code, word, _) = parse(l);
        if !code.is_empty() && !word.is_empty() {
            last_op_at.insert((code, word), i);
        }
    }
    // 3. {整序} 快照：同码只留最后一份
    let mut last_order_at: HashMap<&str, usize> = HashMap::new();
    for (i, l) in lines.iter().enumerate() {
        let (tag, code, _, _) = parse(l);
        if tag == "{整序}" && !code.is_empty() {
            last_order_at.insert(code, i);
        }
    }
    // 4. 第一遍输出：淘汰中间态行
    let kept: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter(|(i, l)| {
            let (tag, code, word, _) = parse(l);
            if !code.is_empty() && !word.is_empty() {
                return last_op_at.get(&(code, word)) == Some(i);
            }
            if tag == "{整序}" && !code.is_empty() {
                return last_order_at.get(code) == Some(i);
            }
            true
        })
        .map(|(_, l)| *l)
        .collect();

    // 5. 快照后置：有 {整序} 快照的码，其**全部**无 pN {添加} 行
    //    （跨机单行加词，可能出现在快照之前——本案实锤形态）一律
    //    抽出、插到该码最后一行（快照组尾）之后——回放序=快照重建
    //    后再加词，对方机器加的词不再被孤儿淘汰。
    let out: Vec<&str>;
    {
        let snap_codes: HashSet<&str> = last_order_at.keys().copied().collect();
        // 抽出所有需后置的行（保序）
        let mut moved: Vec<&str> = Vec::new();
        let mut shrunk: Vec<&str> = Vec::with_capacity(kept.len());
        for l in &kept {
            let (tag, code, _, pn) = parse(l);
            if tag == "{添加}" && snap_codes.contains(code) && !pn {
                moved.push(l);
            } else {
                shrunk.push(l);
            }
        }
        if !moved.is_empty() {
            // 每码在 shrunk 中最后出现位置（{整序}/{添加}/{删除}/
            // {置顶}/{加权} 同码行都算锚点）
            let mut last_at: HashMap<&str, usize> = HashMap::new();
            for (i, l) in shrunk.iter().enumerate() {
                let (tag, code, _, _) = parse(l);
                if !code.is_empty()
                    && (tag == "{整序}"
                        || tag == "{添加}"
                        || tag == "{删除}"
                        || tag == "{置顶}"
                        || tag == "{加权}")
                {
                    last_at.insert(code, i);
                }
            }
            // 从后往前插（保前面的索引不漂移）：同码 moved 行按序追加
            let mut insert_after: HashMap<&str, Vec<&str>> = HashMap::new();
            for l in moved {
                let (_, code, _, _) = parse(l);
                insert_after.entry(code).or_default().push(l);
            }
            let mut anchors: Vec<(usize, &str)> =
                last_at.iter().map(|(c, i)| (*i, *c)).collect();
            anchors.sort_unstable_by(|a, b| b.0.cmp(&a.0)); // 倒序插
            for (at, code) in anchors {
                if let Some(rows) = insert_after.remove(code) {
                    let mut idx = at + 1;
                    for r in rows {
                        shrunk.insert(idx, r);
                        idx += 1;
                    }
                }
            }
            out = shrunk;
        } else {
            out = kept;
        }
    }

    if out.is_empty() {
        String::new()
    } else {
        let mut s = out.join("\n");
        s.push('\n');
        s
    }
}

/// 每轮首个备份：数据\同步备份\ts-<epoch>\（返回已建目录路径）。
fn ensure_backup<'a>(data_dir: &Path, slot: &'a mut Option<PathBuf>) -> &'a Path {
    slot.get_or_insert_with(|| {
        let d = data_dir.join("同步备份").join(format!("ts-{}", now_secs()));
        let _ = std::fs::create_dir_all(&d);
        d
    })
}

/// 备份目录按名倒序保留最近 BACKUP_KEEP 份（ts- 前缀等宽，字符串序=时间序）。
fn prune_backups(data_dir: &Path) {
    let root = data_dir.join("同步备份");
    let dirs = match std::fs::read_dir(&root) {
        Ok(d) => d,
        Err(_) => return,
    };
    let mut names: Vec<String> = dirs
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("ts-"))
        .collect();
    names.sort();
    let excess = names.len().saturating_sub(BACKUP_KEEP);
    for n in &names[..excess] {
        let _ = std::fs::remove_dir_all(root.join(n));
    }
}

// ───────────────────────── config 消毒/覆盖 ─────────────────────────

/// 删除 v 中 path 指到的键（"a.b" 两级）。
fn remove_path(v: &mut Value, path: &str) {
    let mut segs: Vec<&str> = path.split('.').collect();
    let Some(last) = segs.pop() else { return };
    let mut cur = v;
    for s in &segs {
        match cur.get_mut(*s) {
            Some(n) if n.is_object() => cur = n,
            _ => return,
        }
    }
    if let Some(o) = cur.as_object_mut() {
        o.remove(last);
    }
}

/// 剔除不同步键（上行前调用；overlay 端同表保障下行不受覆盖）。
fn sanitize_config(v: &mut Value) {
    for p in CONFIG_EXCLUDED {
        remove_path(v, p);
    }
}

/// 深覆盖：src 只含可同步键 → dst 排除键天然保留；数组/标量整体替换。
///（仅测试用：合并路径已改走 three_way_merge）
#[cfg(test)]
fn overlay(dst: &mut Value, src: &Value) {
    if let (Some(d), Some(s)) = (dst.as_object_mut(), src.as_object()) {
        for (k, v) in s {
            match d.get_mut(k) {
                Some(slot) if slot.is_object() && v.is_object() => overlay(slot, v),
                _ => {
                    d.insert(k.clone(), v.clone());
                }
            }
        }
    }
}

// ─────────────────── config 三方归并 ───────────────────

/// 读设置里的上次同步配置快照（三方归并 base）。
fn last_config_snapshot() -> Option<Value> {
    SETTINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|s| s.last_config.clone())
}

/// 收集对象的全部叶子路径（"a.b.c" → 值；数组视为叶子整体）。
fn cfg_leaves(prefix: &str, v: &Value, out: &mut Vec<(String, Value)>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                cfg_leaves(&format!("{prefix}.{k}"), x, out);
            }
        }
        _ => out.push((prefix.to_string(), v.clone())),
    }
}

fn get_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |cur, k| cur.get(k))
}

fn set_path(v: &mut Value, path: &str, val: Value) {
    let segs: Vec<&str> = path.split('.').collect();
    let mut cur = v;
    for s in &segs[..segs.len() - 1] {
        if !cur.get(s).is_some_and(|x| x.is_object()) {
            // 路径缺段（远端来自更新版本的配置）：补空对象再下钻
            if let Some(o) = cur.as_object_mut() {
                o.insert((*s).to_string(), Value::Object(Default::default()));
            }
        }
        cur = match cur.get_mut(s) {
            Some(c) => c,
            None => return,
        };
    }
    if let Some(o) = cur.as_object_mut() {
        o.insert(segs[segs.len() - 1].to_string(), val);
    }
}

/// 三方归并（配置）：full=本机完整配置（未消毒）；local_san=本机消毒快照；
/// base=上次同步时双方一致的消毒快照；remote=云端消毒快照。
/// 逐叶子：本机已等于远端→跳过；远端==base（没动）→本机新值保留；
/// 远端动了且本机没动（==base 或 base 缺该键）→采纳远端；两边都动→本机胜。
fn three_way_merge(full: &Value, local_san: &Value, base: &Value, remote: &Value) -> Value {
    let mut out = full.clone();
    let mut leaves = Vec::new();
    cfg_leaves("", remote, &mut leaves);
    for (raw_path, rval) in leaves {
        let path = raw_path.strip_prefix('.').unwrap_or(&raw_path);
        let lval = get_path(local_san, path);
        let bval = get_path(base, path);
        if Some(&rval) == lval {
            continue;
        }
        if Some(&rval) == bval {
            continue;
        }
        match (lval, bval) {
            (l, b) if l == b || b.is_none() => set_path(&mut out, path, rval),
            _ => {}
        }
    }
    out
}

// ─────────────────────────── 同步包 ───────────────────────────

/// data 部分规范哈希（BTreeMap 序确定 → 同内容同哈希）。
fn data_hash(d: &Value) -> String {
    hex(&Sha256::digest(
        &serde_json::to_vec(d).unwrap_or_default(),
    ))
}

/// 构包：明文（口令空）或 AES-256-GCM 密文。
fn build_package(data: &Value, device: &str, passphrase: &str) -> Result<Vec<u8>, String> {
    let mut pkg = json!({"v": PKG_VERSION, "device": device, "ts": now_secs()});
    if passphrase.is_empty() {
        pkg["enc"] = Value::Null;
        pkg["data"] = data.clone();
    } else {
        let mut salt = [0u8; 16];
        let mut nonce = [0u8; 12];
        getrandom::getrandom(&mut salt).map_err(|e| format!("取随机数失败：{e}"))?;
        getrandom::getrandom(&mut nonce).map_err(|e| format!("取随机数失败：{e}"))?;
        let key = derive_key(passphrase.as_bytes(), &salt, KDF_ROUNDS);
        let pt = serde_json::to_vec(data).map_err(|e| e.to_string())?;
        let ct = aes_ops(&key, &nonce, &pt, true).ok_or("加密失败")?;
        pkg["enc"] = json!({"salt": hex(&salt), "nonce": hex(&nonce), "iter": KDF_ROUNDS});
        pkg["data"] = Value::String(hex(&ct));
    }
    serde_json::to_vec(&pkg).map_err(|e| e.to_string())
}

/// 解包：版本校验 + 解密（enc 包把 data 原地替换为解密后的对象）。
fn parse_package(body: &[u8], passphrase: &str) -> Result<Value, String> {
    let mut pkg: Value =
        serde_json::from_slice(body).map_err(|_| "云端同步包损坏（非 JSON）".to_string())?;
    let v = pkg.get("v").and_then(|v| v.as_u64()).unwrap_or(0);
    if v != PKG_VERSION {
        return Err(format!("云端同步包版本 v{v} 不支持（本机只认 v{PKG_VERSION}），请升级虎符"));
    }
    let enc = pkg.get("enc").cloned().unwrap_or(Value::Null);
    if enc.is_null() {
        return Ok(pkg);
    }
    if passphrase.is_empty() {
        return Err("云端是加密同步包：请先在本机填写同一加密口令".into());
    }
    let hex_field = |v: &Value| -> Option<Vec<u8>> { v.as_str().and_then(unhex) };
    let salt = hex_field(&enc["salt"]).ok_or("加密参数（salt）损坏")?;
    let nonce = hex_field(&enc["nonce"]).ok_or("加密参数（nonce）损坏")?;
    let iter = enc["iter"].as_u64().unwrap_or(KDF_ROUNDS as u64) as u32;
    let ct = hex_field(&pkg["data"]).ok_or("密文损坏")?;
    let key = derive_key(passphrase.as_bytes(), &salt, iter);
    let pt = aes_ops(&key, &nonce, &ct, false).ok_or("解密失败：加密口令不匹配（或数据损坏）")?;
    let data: Value = serde_json::from_slice(&pt).map_err(|_| "解密后数据损坏".to_string())?;
    pkg["data"] = data;
    Ok(pkg)
}

// ─────────────────────────── 加密件 ───────────────────────────

fn derive_key(pp: &[u8], salt: &[u8], iter: u32) -> [u8; 32] {
    let mut k = [0u8; 32];
    pbkdf2::pbkdf2::<hmac::Hmac<Sha256>>(pp, salt, iter, &mut k);
    k
}

/// AES-256-GCM 加/解密（enc=true 加密）。
fn aes_ops(key: &[u8; 32], nonce: &[u8], data: &[u8], enc: bool) -> Option<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit};
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(key).ok()?;
    let n = aes_gcm::Nonce::from_slice(nonce);
    if enc {
        cipher.encrypt(n, data).ok()
    } else {
        cipher.decrypt(n, data).ok()
    }
}

// ─────────────────────────── WebDAV ───────────────────────────

/// 标准 Base64（编码方向；仅 Authorization 头用，不引依赖）。
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

/// HTTP Basic 认证头（ureq 无内联助手/特性门无关，标准头最稳）。
fn auth_req(r: ureq::Request, user: &str, pass: &str) -> ureq::Request {
    let cred = b64(format!("{user}:{pass}").as_bytes());
    r.set("Authorization", &format!("Basic {cred}"))
}

fn build_agent() -> ureq::Agent {
    let b = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout(Duration::from_secs(30));
    // 【TLS 坑 2026-10-28】ureq 的 native-tls 特性 ≠ 自动生效：不开
    // rustls("tls") 特性时默认 connector 是 NoTlsConfig，https 直接报
    // "no TLS backend is configured"——必须在 AgentBuilder 上显式挂
    // native_tls connector（ureq 已 re-export，无需直接依赖 native-tls）。
    #[cfg(windows)]
    let b = match ureq::native_tls::TlsConnector::new() {
        Ok(c) => b.tls_connector(std::sync::Arc::new(c)),
        Err(_) => b, // schannel 初始化失败（几乎不可能）：退化为无 TLS
    };
    // unix 走 rustls("tls" 特性)，是默认 connector，无需配置。
    b.build()
}

fn url_join(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path)
}

/// GET 云端包：Ok(None)=404 云端为空；读体限流（防异常大响应）。
fn webdav_get(agent: &ureq::Agent, s: &SyncSettings) -> Result<Option<Vec<u8>>, String> {
    let url = url_join(&s.url, PKG_PATH);
    match auth_req(agent.request("GET", &url), &s.user, &s.pass).call() {
        Ok(r) => {
            let mut buf = Vec::new();
            r.into_reader()
                .take((MAX_PKG * 2) as u64)
                .read_to_end(&mut buf)
                .map_err(|e| format!("下载失败：{e}"))?;
            if buf.len() > MAX_PKG {
                return Err(format!("云端同步包超限（{} KB > 2 MB）", buf.len() / 1024));
            }
            Ok(Some(buf))
        }
        // 404=云端为空；409=父集合 hufu-sync/ 不存在（坚果云口径），
        // 同样视为云端为空——首次 PUT 会先 MKCOL 建目录再传。
        Err(ureq::Error::Status(404, _)) | Err(ureq::Error::Status(409, _)) => Ok(None),
        Err(ureq::Error::Status(401, _)) => {
            Err("认证失败：账号或应用密码不对".into())
        }
        Err(e) => Err(format!("网络失败：{e}")),
    }
}

/// PUT 云端包：409（父集合不存在）→ MKCOL 建目录重试一次。
fn webdav_put(agent: &ureq::Agent, s: &SyncSettings, body: &[u8]) -> Result<(), String> {
    let put = |u: &str| {
        auth_req(
            agent
                .request("PUT", u)
                .set("Content-Type", "application/json"),
            &s.user,
            &s.pass,
        )
        .send_bytes(body)
    };
    let url = url_join(&s.url, PKG_PATH);
    match put(&url) {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(409, _)) => {
            let col = url_join(&s.url, "hufu-sync");
            let _ = auth_req(agent.request("MKCOL", &col), &s.user, &s.pass).call();
            put(&url).map(|_| ()).map_err(|e| format!("上传失败：{e}"))
        }
        Err(ureq::Error::Status(401, _)) => {
            Err("认证失败：账号或应用密码不对".into())
        }
        Err(e) => Err(format!("上传失败：{e}")),
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

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let b = s.as_bytes();
    let mut v = Vec::with_capacity(b.len() / 2);
    for i in (0..b.len()).step_by(2) {
        let hi = (b[i] as char).to_digit(16)?;
        let lo = (b[i + 1] as char).to_digit(16)?;
        v.push((hi * 16 + lo) as u8);
    }
    Some(v)
}

// ─────────────────────────── 测试 ───────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write as _};

    /// 【误清空护栏 + 备份 2026-10-29】畸形整清请求（user 空 + pass 字面
    /// 空串覆盖）必须被拒绝且原凭据保留；正常路径（__keep__/换账号）不受
    /// 影响；persist 落盘前自动留 同步.json.bak。
    #[test]
    fn save_settings_rejects_accidental_wipe_and_backs_up() {
        let dir = std::env::temp_dir().join(format!("hufu-sync-wipe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _ = DATA_DIR.set(dir.clone());
        *SETTINGS.lock().unwrap_or_else(|e| e.into_inner()) = Some(SyncSettings {
            url: "https://dav.example/dav/".into(),
            user: "u@example.com".into(),
            pass: "secret".into(),
            ..Default::default()
        });
        // 畸形整清：拒绝，原值分毫不动
        let r = save_settings("", "", "", "", "");
        assert!(r.is_err(), "整清请求必须被拒绝");
        let s = SETTINGS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap();
        assert_eq!((s.user.as_str(), s.pass.as_str()), ("u@example.com", "secret"));
        // 正常保存：pass/__keep__ 保留、url 空归默认、持久化 + 备份
        save_settings("", "u@example.com", KEEP, "", KEEP).unwrap();
        let s2 = SETTINGS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap();
        assert_eq!(s2.url, DEFAULT_URL);
        assert_eq!(s2.pass, "secret");
        assert!(dir.join("同步.json").is_file());
        // 第二次正常保存 → 备份滚动为上一版（首次保存无旧版可备）
        save_settings("", "u2@example.com", KEEP, "自定义设备", KEEP).unwrap();
        assert!(dir.join("同步.json.bak").is_file(), "非首版落盘前必须留备份");
        let bak: SyncSettings = serde_json::from_str(
            &std::fs::read_to_string(dir.join("同步.json.bak")).unwrap(),
        ).unwrap();
        assert_eq!(bak.user, "u@example.com", "备份应保存上一版");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_adjust_union_dedup_local_last() {
        // 并集去重，远端序在前、本机独有在后
        let m = merge_adjust("{置顶}a\t一\n{删除}b\t二\n", "{删除}b\t二\n{添加}c\t三\n");
        assert_eq!(m, "{置顶}a\t一\n{删除}b\t二\n{添加}c\t三\n");
        // 【键终态 2026-10-30】同键不同操作只留拼接流最后出现者（与
        // 回放「后写赢」一致——旧行并集保留双行是恶性洗词的帮凶，
        // 见 merge_adjust 文档注释）；本机删除在后=删除终态传播
        let m2 = merge_adjust("{添加}a\t一\n", "{删除}a\t一\n");
        assert_eq!(m2, "{删除}a\t一\n");
        // 反向：本机添加在后=加词传播（跨机重新加词不再被墓碑杀）
        let m2b = merge_adjust("{删除}a\t一\n", "{添加}a\t一\n");
        assert_eq!(m2b, "{添加}a\t一\n");
        assert_eq!(merge_adjust("", ""), "");
        assert_eq!(merge_adjust("x\n", "x\n"), "x\n");
        // 空行剔除 / CRLF 容错
        let m3 = merge_adjust("\r\n{删除}b\t二\r\n", "\n");
        assert_eq!(m3, "{删除}b\t二\n");
    }

    /// 【恶性洗词 2026-10-30 真机实锤场景回归】码表方案「茉莉」：
    /// A 机加词 qp 名字（远端）+ B 机右键删词落盘三件套（{删除}+
    /// {整序}+{添加}qp 锭 p1，本机）+ B 机用户又重新加词（本机尾行）。
    /// 期望：键终态=最后添加（词活）、墓碑 {删除} 行淘汰、
    /// 快照只留最后一份、跨机无 pN 加词行后置到快照之后
    /// （回放=快照重建后再加 → 名字不再被孤儿淘汰）。
    #[test]
    fn merge_adjust_moli_wash_regression() {
        let remote = "\
{添加}tjpt\t生日礼物\tp3\n\
{添加}qp\t名字\n\
{删除}qp\t名字\n\
{整序}qp\n\
{添加}qp\t锭\tp1\n\
{整序}qp\n\
{添加}qp\t锭\tp1\n";
        let local = "{添加}qp\t名字\n";
        let m = merge_adjust(remote, local);
        let lines: Vec<&str> = m.lines().collect();
        // 墓碑淘汰：同键终态=最后的 {添加}，{删除} 行不存在
        assert!(!lines.contains(&"{删除}qp\t名字"), "墓碑行应被淘汰: {m}");
        // 快照去重：{整序}qp 只一份
        assert_eq!(lines.iter().filter(|l| **l == "{整序}qp").count(), 1);
        // 无 pN 加词行后置：名字行在 {整序}qp 与 {添加}qp 锭 p1 之后
        let i_snap = lines.iter().position(|l| *l == "{整序}qp").unwrap();
        let i_ding = lines.iter().position(|l| *l == "{添加}qp\t锭\tp1").unwrap();
        let i_mz = lines.iter().position(|l| *l == "{添加}qp\t名字").unwrap();
        assert!(i_snap < i_ding && i_ding < i_mz, "回放序应为 快照→锭p1→名字: {m}");
        // 词行仍随全局序保留
        assert_eq!(lines[0], "{添加}tjpt\t生日礼物\tp3");
    }

    /// 快照后置的另一半：跨机加词行出现在快照**之前**（A 机文件序）
    /// 也必须被抽到快照之后——这是主凶路径（B 机快照经并集追加在
    /// 尾部，把 A 机传来的加词行压在前面=回放被孤儿淘汰）。
    #[test]
    fn merge_adjust_addword_before_snapshot_moves_after() {
        let remote = "{添加}qp\t名字\n{添加}nx\t习惯\n";
        let local = "{整序}qp\n{添加}qp\t锭\tp1\n";
        let m = merge_adjust(remote, local);
        let lines: Vec<&str> = m.lines().collect();
        let i_snap = lines.iter().position(|l| *l == "{整序}qp").unwrap();
        let i_mz = lines.iter().position(|l| *l == "{添加}qp\t名字").unwrap();
        assert!(i_snap < i_mz, "快照前的加词行必须后置到快照后: {m}");
        // 无快照码不受影响
        assert_eq!(lines.iter().position(|l| *l == "{添加}nx\t习惯").unwrap(), 0);
    }

    #[test]
    fn config_sanitize_overlay_exclusion() {
        let local = json!({
            "schema": {"current": "A", "dir": "码表"},
            "general": {"autostart": true, "export_key": "ctrl+alt+e", "shift_switch": true},
            "appearance": {"skin": "hufu-moyan", "skin_kind": "json", "font_family": "", "font_size": 17.6, "anim": true},
            "sentence": {"enabled": true}
        });
        let mut remote_full = local.clone();
        sanitize_config(&mut remote_full);
        assert!(remote_full.get("schema").is_none());
        assert!(remote_full["appearance"].get("skin").is_none());
        assert!(remote_full["appearance"].get("skin_kind").is_none());
        assert!(remote_full["appearance"].get("font_family").is_none());
        assert!(remote_full["general"].get("autostart").is_none());
        assert!(remote_full["general"].get("export_key").is_none());
        assert_eq!(remote_full["general"]["shift_switch"], true);
        assert_eq!(remote_full["appearance"]["font_size"], 17.6);
        // overlay：远端键覆盖，本机排除键保留
        let mut merged = local.clone();
        overlay(&mut merged, &json!({"general": {"shift_switch": false}, "appearance": {"font_size": 20.0}}));
        assert_eq!(merged["general"]["shift_switch"], false);
        assert_eq!(merged["appearance"]["font_size"], 20.0);
        assert_eq!(merged["appearance"]["skin"], "hufu-moyan");
        assert_eq!(merged["schema"]["current"], "A");
    }

    #[test]
    fn config_three_way_merge_rules() {
        let full = json!({
            "general": {"esc_undo": false},
            "candidates": {"page_size": 5},
            "appearance": {"skin": "keepme", "anim": true}
        });
        let mut san = full.clone();
        sanitize_config(&mut san);
        // base = 上次同步双方一致态
        let base = json!({
            "general": {"esc_undo": false},
            "candidates": {"page_size": 5},
            "appearance": {"anim": true}
        });
        // 本机改了 anim=false、page_size=7；远端改了 esc_undo=true（page_size 是 base 旧值）
        let mut local_full = full.clone();
        local_full["appearance"]["anim"] = json!(false);
        local_full["candidates"]["page_size"] = json!(7);
        let mut local_san = local_full.clone();
        sanitize_config(&mut local_san);
        let remote = json!({
            "general": {"esc_undo": true},
            "candidates": {"page_size": 5},
            "appearance": {"anim": true}
        });
        let merged = three_way_merge(&local_full, &local_san, &base, &remote);
        assert_eq!(merged["general"]["esc_undo"], true, "远端变了本机没动→采纳");
        assert_eq!(merged["candidates"]["page_size"], 7, "本机变了远端没动→保留");
        assert_eq!(merged["appearance"]["anim"], false, "本机变了→保留");
        assert_eq!(merged["appearance"]["skin"], "keepme", "排除字段原样");

        // 两边都改同一键 → 本机胜
        let base2 = json!({"candidates": {"page_size": 5}});
        let remote2 = json!({"candidates": {"page_size": 9}});
        let local2 = json!({"candidates": {"page_size": 7}});
        let m2 = three_way_merge(&local2, &local2, &base2, &remote2);
        assert_eq!(m2["candidates"]["page_size"], 7, "两边都动→本机胜");

        // 云端旧值不得冲回本机新设置（回归锁：修复前的缺陷场景）
        // 远端 anim=true==base（远端没动），本机已改 false → 保持 false
        assert_eq!(merged["appearance"]["anim"], false);

        // 首次同步（base=本机当前态）：远端不同且本机没动 → 全采纳远端
        let remote3 = json!({"general": {"esc_undo": true}, "candidates": {"page_size": 9}});
        let m3 = three_way_merge(&full, &san, &san, &remote3);
        assert_eq!(m3["general"]["esc_undo"], true);
        assert_eq!(m3["candidates"]["page_size"], 9);
    }

    #[test]
    fn union_carried_keeps_remote_only_schemas() {
        // 本机只有 B（A 未装）；carried 有 A → 并回后 A 保留、B 不动
        let mut local = json!({"schemas": {"B": "{添加}b\t乙"}, "config": {}});
        let carried = json!({"A": "{置顶}a\t甲"});
        union_carried(&mut local, &carried);
        assert_eq!(local["schemas"]["A"], "{置顶}a\t甲", "未装方案的云端条目必须透传保留");
        assert_eq!(local["schemas"]["B"], "{添加}b\t乙");
        // 键已存在（本地装好并已合并）→ 本地结果优先，不被云端旧值覆盖
        let mut local2 = json!({"schemas": {"A": "本地合并结果"}});
        union_carried(&mut local2, &carried);
        assert_eq!(local2["schemas"]["A"], "本地合并结果");
        // 空 carried 无副作用
        let mut local3 = json!({"schemas": {}});
        union_carried(&mut local3, &json!({}));
        assert!(local3["schemas"].as_object().unwrap().is_empty());
    }

    #[test]
    fn merge_adjust_v2_reset_semantics() {
        let base = "{置顶}mj\t明显\n{删除}cu\t错词";
        let remote_more = "{置顶}mj\t明显\n{删除}cu\t错词\n{添加}xx\t新词";
        // 首次（无 base）：并集
        assert_eq!(
            merge_adjust_v2(Some(remote_more), Some(base), None).unwrap(),
            merge_adjust(remote_more, base)
        );
        // 本机重置（导出合并清空）：只留远端新增，旧记录不拉回
        assert_eq!(
            merge_adjust_v2(Some(remote_more), None, Some(base)).unwrap(),
            "{添加}xx\t新词"
        );
        // 本机重置且远端无新增 → 维持空
        assert_eq!(merge_adjust_v2(Some(base), None, Some(base)), None);
        // 远端重置（云端没了该方案）：本机只留自己新增
        assert_eq!(
            merge_adjust_v2(None, Some(remote_more), Some(base)).unwrap(),
            "{添加}xx\t新词"
        );
        // 双方都清 → 维持空
        assert_eq!(merge_adjust_v2(None, None, Some(base)), None);
        // 双方都在 → 并集
        assert_eq!(
            merge_adjust_v2(Some(remote_more), Some(base), Some(base)).unwrap(),
            merge_adjust(remote_more, base)
        );
        // 空/空白字符串视为 None
        assert_eq!(merge_adjust_v2(Some("  \n"), None, Some(base)), None);
        // 【洗词事故回归锁 2026-10-29】云端没有该方案 + 无 base +
        // 本机有内容 → 保持现状（None=不写盘），绝不能写空
        assert_eq!(merge_adjust_v2(None, Some("{添加}kmmr\t只看见贼吃肉"), None), None);
        // 远端重置且本机无新增 → 明确清空（Some("")，区别于保持现状）
        assert_eq!(merge_adjust_v2(None, Some(base), Some(base)), Some(String::new()));
        // diff_lines：保序、CRLF 容忍、去重
        assert_eq!(diff_lines("a\r\nb\na\nc", "b"), "a\nc");
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(hex(&[0x00, 0xff, 0x10]), "00ff10");
        assert_eq!(unhex("00ff10").unwrap(), vec![0x00, 0xff, 0x10]);
        assert!(unhex("abc").is_none());
        assert!(unhex("zz").is_none());
    }

    /// 内存 WebDAV 测试服（GET/PUT/MKCOL/PROPFIND 最小集，Connection: close）。
    fn mini_webdav() -> (String, Arc<Mutex<HashMap<String, Vec<u8>>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let store = Arc::new(Mutex::new(HashMap::<String, Vec<u8>>::new()));
        let st = store.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let store = st.clone();
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let Ok(reader) = stream.try_clone() else { return };
                    let mut reader = BufReader::new(reader);
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    let mut it = line.split_whitespace();
                    let method = it.next().unwrap_or("").to_string();
                    let target = it.next().unwrap_or("/").to_string();
                    let mut cl = 0usize;
                    loop {
                        let mut h = String::new();
                        if reader.read_line(&mut h).is_err() {
                            break;
                        }
                        if h.trim().is_empty() {
                            break;
                        }
                        if let Some((k, v)) = h.split_once(':') {
                            if k.trim().eq_ignore_ascii_case("content-length") {
                                cl = v.trim().parse().unwrap_or(0);
                            }
                        }
                    }
                    let mut body = vec![0u8; cl];
                    if cl > 0 && reader.read_exact(&mut body).is_err() {
                        return;
                    }
                    let (code, rb): (&str, Vec<u8>) = match method.as_str() {
                        "GET" => store
                            .lock()
                            .unwrap()
                            .get(&target)
                            .map(|b| ("200", b.clone()))
                            .unwrap_or(("404", vec![])),
                        "PUT" => {
                            store.lock().unwrap().insert(target, body);
                            ("201", vec![])
                        }
                        "MKCOL" | "PROPFIND" => ("201", vec![]),
                        _ => ("405", vec![]),
                    };
                    let head = format!(
                        "HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        rb.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&rb);
                    let _ = stream.flush();
                });
            }
        });
        (format!("http://{addr}"), store)
    }

    #[test]
    fn package_and_webdav_roundtrip() {
        let (url, store) = mini_webdav();
        let s = SyncSettings {
            url,
            user: "u".into(),
            pass: "p".into(),
            device: "T1".into(),
            passphrase: "sec".into(),
            ..Default::default()
        };
        let agent = build_agent();
        // 云端为空 → None
        assert!(webdav_get(&agent, &s).unwrap().is_none());
        // 加密包上传 → 云端只见密文（store 里搜不到明文）
        let data = json!({"schemas": {"A": "{置顶}aa\t一\n"}, "config": {"general": {"shift_switch": true}}});
        let body = build_package(&data, "T1", "sec").unwrap();
        webdav_put(&agent, &s, &body).unwrap();
        {
            let st = store.lock().unwrap();
            let raw = st
                .values()
                .find(|b| !b.is_empty())
                .expect("云端应有同步包");
            let raw_s = String::from_utf8_lossy(raw);
            assert!(!raw_s.contains("{置顶}"), "明文词条不得出现在密文包里");
            assert!(!raw_s.contains("shift_switch"), "明文配置键不得出现在密文包里");
        }
        // 取回 + 正确口令解密 → data 等值
        let got = webdav_get(&agent, &s).unwrap().unwrap();
        let pkg = parse_package(&got, "sec").unwrap();
        assert_eq!(pkg["data"], data);
        assert_eq!(pkg["device"], "T1");
        // 错误口令 → 明确报错
        assert!(parse_package(&got, "wrong").is_err());
        // 明文包 + 空口令
        let plain = build_package(&data, "T1", "").unwrap();
        let pkg2 = parse_package(&plain, "").unwrap();
        assert_eq!(pkg2["data"], data);
        // 版本不符拒绝
        let bad = json!({"v": 99, "data": {}});
        assert!(parse_package(&serde_json::to_vec(&bad).unwrap(), "").is_err());
    }
}
