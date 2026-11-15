//! Short atomic profile transactions shared with the server's WebDAV worker.
use gale_core::{config::Config, wenlai::WenlaiAccount};
use serde_json::{json, Value};
use std::{io::{Read, Write}, path::Path, time::{SystemTime, UNIX_EPOCH}};

const MAX_PROFILE: u64 = 1024 * 1024;

pub struct State {
    local_identity: Value,
    local_config: Value,
    /// 本机这份配置所对应的云档内容时刻：load 时记录、每次成功写入后刷新。
    /// 冲突判定必须用它作为基线，不能用 config.toml 的 mtime（见 save 内注释）。
    base_ms: u64,
}

pub struct Loaded {
    pub state: State,
    pub config: Config,
    pub user: String,
    pub password: String,
    pub account: Option<WenlaiAccount>,
    pub warning: Option<String>,
}

fn read(path: &Path) -> Result<Option<Value>, String> {
    let mut raw = Vec::new();
    match std::fs::File::open(path) {
        Ok(f) => f.take(MAX_PROFILE + 1).read_to_end(&mut raw).map_err(|e| format!("读取跟打器同步配置失败：{e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取跟打器同步配置失败：{e}")),
    };
    if raw.len() as u64 > MAX_PROFILE { return Err("跟打器同步配置超过 1 MiB".into()); }
    // 【审计 C05】档被写坏时先尝试恢复链（.recover / .bak / .bak-<pid>）再报错。
    let v: Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(_) => match gale_core::score::read_with_recovery(path) {
            Some(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "跟打器同步配置 JSON 损坏".to_string())?,
            None => return Err("跟打器同步配置 JSON 损坏".into()),
        },
    };
    if v["version"].as_u64() != Some(1) { return Err("跟打器同步配置版本不支持".into()); }
    if v.get("online_user").is_some_and(|x| !x.is_string()) || v.get("online_password").is_some_and(|x| !x.is_string()) {
        return Err("跟打器同步账号格式无效".into());
    }
    config(&v)?;
    if !v["online_account"].is_null() {
        serde_json::from_value::<WenlaiAccount>(v["online_account"].clone()).map_err(|_| "跟打器同步账号损坏".to_string())?;
    }
    Ok(Some(v))
}

fn config(v: &Value) -> Result<Config, String> {
    let text = v["config_toml"].as_str().filter(|s| !s.trim().is_empty()).ok_or("跟打器同步配置缺少设置")?;
    if text.len() > 256 * 1024 { return Err("跟打器同步设置超过 256 KiB".into()); }
    Config::from_toml_text(text).map_err(|e| format!("跟打器同步设置损坏：{e}"))
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn mtime_ms(path: &Path) -> u64 {
    mtime(path)
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// 云档的“内容时刻”。下载器会把 profile.json 的文件 mtime 设成远端时间，
// 但云端偶尔会下发内容很旧的档（例如另一台设备很久以前上传的版本）。
// 判断新旧必须看档内 updated_at_ms，不能只看文件 mtime。
fn profile_stamp_ms(v: &Value, path: &Path) -> u64 {
    v.get("updated_at_ms").and_then(Value::as_u64)
        .or_else(|| v.get("updated_at").and_then(Value::as_u64).map(|s| s.saturating_mul(1000)))
        .unwrap_or_else(|| mtime_ms(path))
}

pub fn load(dir: &Path) -> Result<Loaded, String> {
    let cfg_path = dir.join("config.toml");
    // Capture absence/mtime BEFORE load_or_create creates a fresh default file.
    let local_time = mtime(&cfg_path);
    let local_ms = mtime_ms(&cfg_path);
    let path = dir.join("profile.json");
    let (profile, warning) = match read(&path) {
        Ok(v) => (v, None),
        Err(e) => (None, Some(format!("已跳过同步配置，使用本机设置：{e}"))),
    };
    let mut cfg = Config::load_or_create(&cfg_path).map_err(|e| e.to_string())?;
    // 本机配置对应的云档版本：不论随后采用云档还是保留本机设置，这次 load
    // 都算"已看过"该版本，保存时以它为合并基线（审计 C01）。
    let profile_stamp = profile.as_ref().map(|v| profile_stamp_ms(v, &path)).unwrap_or(0);
    if let Some(v) = &profile {
        // 旧云档不能覆盖本机较新的设置：用档内 updated_at_ms 与 config.toml
        // 的 mtime 比较，而不能只看 profile.json 的文件时间（下载器会把
        // 文件 mtime 设成远端时间，内容可能仍旧）。
        let profile_time = profile_stamp_ms(v, &path);
        if local_time.is_none() || profile_time >= local_ms {
            let mut from_profile = config(v)?;
            // 练字记录只增不减：云档可以带来别的设备打过的字数，但不能把
            // 本机已累计的日/总量覆盖小（其余设置仍按原 LWW 语义）。
            from_profile.merge_practice_max(&cfg);
            // 【全面排查 2026-11-16】窗口几何属于本机显示器布局，不随云档走
            // （与运行期 merge_config 的 win_ 前缀本地优先同语义）：云档较新
            // 被整份采纳时保留本机五键，多机同步重启后窗口不再跳到另一台
            // 机器的位置/尺寸。
            from_profile.win_x = cfg.win_x;
            from_profile.win_y = cfg.win_y;
            from_profile.win_w = cfg.win_w;
            from_profile.win_h = cfg.win_h;
            from_profile.win_position_physical = cfg.win_position_physical;
            cfg = from_profile;
            cfg.save(&cfg_path).map_err(|e| e.to_string())?;
        } else {
            cfg.merge_practice_max(&config(v)?);
        }
    }
    let v = profile.as_ref().unwrap_or(&Value::Null);
    // 审计 C04：旧版本把密码/令牌写进过同步档，这里一律不认——读出来就当没有，
    // 下一次保存会把它们从文件里抹掉（新版本也从不写）。
    let account = if v["online_account"].is_null() { None } else {
        let mut account: WenlaiAccount = serde_json::from_value(v["online_account"].clone()).map_err(|_| "跟打器同步账号损坏".to_string())?;
        account.jwt_token.clear();
        Some(account)
    };
    let identity_account = without_token(v["online_account"].clone());
    Ok(Loaded {
        user: v["online_user"].as_str().unwrap_or("").to_string(),
        password: String::new(),
        account,
        warning,
        state: State { local_identity: json!([v["online_user"].as_str().unwrap_or(""), "", identity_account]), local_config: serde_json::to_value(&cfg).map_err(|e| e.to_string())?, base_ms: profile_stamp },
        config: cfg,
    })
}

/// 审计 C04：令牌等效于云端登录凭据。同步档随用户自己的 WebDAV 离开本机，
/// 界面也承诺「密码不保存」，所以落档的账号里令牌永远留空。
fn without_token(mut account: Value) -> Value {
    if let Some(map) = account.as_object_mut() {
        if map.contains_key("jwt_token") {
            map.insert("jwt_token".into(), Value::String(String::new()));
        }
    }
    account
}

fn merge_config(base: &Value, local: &Value, remote: &Value) -> Result<Config, String> {
    let mut merged = remote.clone();
    let object = merged.as_object_mut().ok_or("跟打器设置格式无效")?;
    for (key, value) in local.as_object().ok_or("跟打器设置格式无效")? {
        // Geometry is local to a monitor layout. Local edits win conflicts;
        // untouched fields retain changes downloaded while the app stayed open.
        if key.starts_with("win_") || base.get(key) != Some(value) {
            object.insert(key.clone(), value.clone());
        }
    }
    let mut merged: Config = serde_json::from_value(merged).map_err(|e| format!("合并跟打器配置失败：{e}"))?;
    // 练字统计按日期取 max：所有设备都在本地累加，云端旧档不能把字数改小。
    let local_cfg: Config = serde_json::from_value(local.clone()).map_err(|e| format!("合并跟打器配置失败：{e}"))?;
    merged.merge_practice_max(&local_cfg);
    Ok(merged)
}

impl State {
    /// 保存云同步档。`_password` 只为兼容调用方保留——密码绝不落档（审计 C04）。
    pub fn save(&mut self, dir: &Path, cfg: &Config, user: &str, _password: &str, account: Option<WenlaiAccount>, identity_changed: bool) -> Result<Option<Config>, String> {
        use std::os::windows::fs::OpenOptionsExt;
        let path = dir.join("profile.json");
        let _lock = std::fs::OpenOptions::new().read(true).write(true).create(true).share_mode(0)
            .open(path.with_extension("lock")).map_err(|e| format!("云同步正在写入跟打器配置，请稍后重试：{e}"))?;
        let remote = read(&path)?;
        let local = serde_json::to_value(cfg).map_err(|e| e.to_string())?;
        // 冲突基线 = 本机配置对应的云档版本（base_ms），不能再用 config.toml
        // 的 mtime：设置写入器先写 config.toml 再进这里，mtime 恒等于"现在"，
        // 运行期下载到的新云档会被误判成旧档而整份覆盖（审计 C01，已复现）。
        let remote_stamp = remote.as_ref().map(|v| profile_stamp_ms(v, &path)).unwrap_or(0);
        let remote_advanced = remote.is_some() && remote_stamp > self.base_ms;
        let merged = match &remote {
            Some(v) if remote_advanced =>
                merge_config(&self.local_config, &local, &serde_json::to_value(config(v)?).map_err(|e| e.to_string())?)?,
            Some(v) => {
                let mut fresh = cfg.clone();
                fresh.merge_practice_max(&config(v)?);
                fresh
            }
            None => cfg.clone(),
        };
        // 云档带来的字段必须回传界面：界面仍拿着旧值时，下一次保存（任意一次
        // 偏好改动都会触发）会把刚合并进来的远端设置重新覆盖掉。
        let adopted = match serde_json::to_value(&merged) {
            Ok(value) if value != local => Some(merged.clone()),
            _ => None,
        };
        // Export can be temporarily unavailable while DPAPI is being replaced.
        // Only an explicit account transition may clear the saved account.
        let account = match account {
            Some(a) => without_token(serde_json::to_value(a).map_err(|e| e.to_string())?),
            None if identity_changed => Value::Null,
            None => self.local_identity[2].clone(),
        };
        // 审计 C04/I08：同步档会经用户自己的 WebDAV 离开本机，界面也承诺
        // 「密码不保存」——密码只活在本进程内存里，落档一律留空。
        let local_identity = json!([user, "", account]);
        let identity = if !identity_changed && local_identity == self.local_identity {
            remote.as_ref().map(|r| json!([r["online_user"], "", without_token(r["online_account"].clone())])).unwrap_or_else(|| local_identity.clone())
        } else { local_identity.clone() };
        let mut value = json!({"version":1, "config_toml":merged.to_toml_text(),
            "online_user":identity[0], "online_password":"", "online_account":without_token(identity[2].clone())});
        let mut old = remote.clone().unwrap_or(Value::Null);
        if let Some(o) = old.as_object_mut() { o.remove("updated_at"); o.remove("updated_at_ms"); }
        merged.save(&dir.join("config.toml")).map_err(|e| format!("保存跟打器设置失败：{e}"))?;
        if value != old {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
            value["updated_at"] = json!(now.as_secs());
            value["updated_at_ms"] = json!(now.as_millis() as u64);
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let tmp = path.with_extension(format!("app-{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
            let result = (|| {
                let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|e| e.to_string())?;
                file.write_all(value.to_string().as_bytes()).and_then(|_| file.sync_all()).map_err(|e| e.to_string())?;
                drop(file);
                std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
            })();
            let _ = std::fs::remove_file(&tmp);
            result.map_err(|e| format!("保存跟打器同步配置失败：{e}"))?;
            self.base_ms = value.get("updated_at_ms").and_then(Value::as_u64).unwrap_or(remote_stamp);
        } else {
            // 内容没变：云档仍是同一个版本，本机与它一致，基线就地固定。
            self.base_ms = remote_stamp;
        }
        self.local_identity = local_identity;
        // 基线内容 = 已经写进云档的那一份（远端改动被采用时就是 adopted）。
        self.local_config = serde_json::to_value(&merged).map_err(|e| e.to_string())?;
        Ok(adopted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dir() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("trainer-profile-{}-{}", std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&path).unwrap(); path
    }
    fn fixture(cfg: &Config, user: &str) -> Value {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        json!({"version":1,"updated_at":now/1000,"updated_at_ms":now,"config_toml":cfg.to_toml_text(),
            "online_user":user,"online_password":format!("password-{user}"),
            "online_account":{"domain":cfg.wenlai_domain,"username":user,"display_name":user,"user_id":1,"jwt_token":format!("token-{user}")}})
    }
    fn put(dir: &Path, value: &Value) { std::fs::write(dir.join("profile.json"), value.to_string()).unwrap(); }
    #[test]
    fn first_launch_imports_profile_before_default_file_can_shadow_it() {
        let dir = dir(); let mut cfg = Config::default(); cfg.font_size = 42.;
        put(&dir, &fixture(&cfg, "a"));
        let loaded = load(&dir).unwrap();
        assert_eq!(loaded.config.font_size, 42.);
        assert_eq!(loaded.user, "a"); assert_eq!(loaded.account.unwrap().username, "a");
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn damaged_optional_profile_keeps_valid_local_config_and_reports_warning() {
        let dir = dir(); let mut cfg = Config::default(); cfg.font_size = 42.;
        cfg.save(&dir.join("config.toml")).unwrap();
        std::fs::write(dir.join("profile.json"), b"truncated").unwrap();
        let loaded = load(&dir).unwrap();
        assert_eq!(loaded.config.font_size, 42.);
        assert!(loaded.warning.unwrap().contains("已跳过"));
        assert_eq!(std::fs::read(dir.join("profile.json")).unwrap(), b"truncated");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn newer_cloud_profile_keeps_local_window_geometry() {
        // 【全面排查 2026-11-16】云档较新被整份采纳时，窗口几何五键保留本机
        // 值（几何是本机显示器布局的属性，与运行期 merge_config 的 win_
        // 本地优先同语义）；其余设置仍按云档 LWW 采纳。
        let dir = dir();
        let mut local = Config::default();
        local.win_x = -777; local.win_y = 88; local.win_w = 1234; local.win_h = 567;
        local.win_position_physical = true;
        local.save(&dir.join("config.toml")).unwrap();
        let mut remote = Config::default();
        remote.win_x = 1; remote.win_y = 2; remote.win_w = 3200; remote.win_h = 2000;
        remote.win_position_physical = false;
        remote.font_size = 42.;
        put(&dir, &fixture(&remote, "a"));
        let loaded = load(&dir).unwrap();
        assert_eq!(loaded.config.win_x, -777);
        assert_eq!(loaded.config.win_y, 88);
        assert_eq!(loaded.config.win_w, 1234);
        assert_eq!(loaded.config.win_h, 567);
        assert!(loaded.config.win_position_physical);
        assert_eq!(loaded.config.font_size, 42., "非几何设置仍按云档");
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn unchanged_save_scrubs_legacy_credentials_then_keeps_profile_bytes_stable() {
        let dir = dir(); let cfg = Config::default(); put(&dir, &fixture(&cfg, "a"));
        let mut loaded = load(&dir).unwrap();
        // 旧档里带着密码/令牌：第一次保存必须把它们抹掉（审计 C04），账号身份保留。
        loaded.state.save(&dir, &loaded.config, "a", "password-a", None, false).unwrap();
        let value = read(&dir.join("profile.json")).unwrap().unwrap();
        assert_eq!(value["online_password"], "");
        assert_eq!(value["online_account"]["jwt_token"], "");
        assert_eq!(value["online_account"]["username"], "a");
        // 稳态：内容没变就不再重写，文件与时间戳都保持。
        let before = std::fs::read(dir.join("profile.json")).unwrap();
        loaded.state.save(&dir, &loaded.config, "a", "password-a", None, false).unwrap();
        assert_eq!(std::fs::read(dir.join("profile.json")).unwrap(), before);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn credentials_never_reach_the_cloud_profile() {
        let dir = dir();
        let mut loaded = load(&dir).unwrap();
        let account: WenlaiAccount = serde_json::from_value(json!({
            "domain":"d","username":"u","display_name":"u","user_id":7,"jwt_token":"secret-token"})).unwrap();
        loaded.state.save(&dir, &loaded.config, "u", "hunter2", Some(account), true).unwrap();
        let text = std::fs::read_to_string(dir.join("profile.json")).unwrap();
        assert!(!text.contains("hunter2"), "密码不得落档");
        assert!(!text.contains("secret-token"), "令牌不得落档");
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["online_user"], "u");
        assert_eq!(value["online_account"]["username"], "u");
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn download_while_running_merges_untouched_settings_and_account_without_erasing_local_edits() {
        let dir = dir(); let cfg = Config::default(); put(&dir, &fixture(&cfg, "a"));
        let mut loaded = load(&dir).unwrap();
        let mut remote = cfg.clone(); remote.font_size = 40.; remote.theme = "remote-theme".into();
        put(&dir, &fixture(&remote, "b"));
        let mut local = loaded.config.clone(); local.hint_font_size = 20.; local.win_x = -1800;
        let adopted = loaded.state.save(&dir, &local, "a", "password-a", loaded.account, false).unwrap();
        let value = read(&dir.join("profile.json")).unwrap().unwrap(); let merged = config(&value).unwrap();
        assert_eq!(merged.font_size, 40.); assert_eq!(merged.theme, "remote-theme");
        assert_eq!(merged.hint_font_size, 20.); assert_eq!(merged.win_x, -1800);
        // 审计 C04：合并不会把远端档里的密码/令牌带进来（落档一律留空）。
        assert_eq!(value["online_password"], "");
        assert_eq!(value["online_account"]["jwt_token"], "");
        assert_eq!(value["online_account"]["username"], "b");
        // 界面必须采用回传的合并结果（main.rs adopt_synced_config）；之后从这份
        // 配置再保存一次，不得把远端字段顶回去。
        let adopted = adopted.expect("合并结果必须回传界面");
        loaded.state.save(&dir, &adopted, "a", "password-a", None, false).unwrap();
        assert_eq!(config(&read(&dir.join("profile.json")).unwrap().unwrap()).unwrap().font_size, 40.);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn explicit_logout_clears_password_and_session_in_cloud_profile() {
        let dir = dir(); let cfg = Config::default(); put(&dir, &fixture(&cfg, "a"));
        let mut loaded = load(&dir).unwrap();
        loaded.state.save(&dir, &cfg, "", "", None, true).unwrap();
        let value = read(&dir.join("profile.json")).unwrap().unwrap();
        assert_eq!(value["online_user"], ""); assert_eq!(value["online_password"], "");
        assert!(value["online_account"].is_null());
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn concurrent_writer_lock_reports_failure_without_replacing_profile() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = dir(); let cfg = Config::default(); put(&dir, &fixture(&cfg, "a"));
        let mut loaded = load(&dir).unwrap(); let before = std::fs::read(dir.join("profile.json")).unwrap();
        let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).share_mode(0).open(dir.join("profile.lock")).unwrap();
        assert!(loaded.state.save(&dir, &cfg, "", "", None, true).unwrap_err().contains("稍后重试"));
        assert_eq!(std::fs::read(dir.join("profile.json")).unwrap(), before);
        drop(lock); let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn stale_cloud_profile_cannot_roll_back_newer_local_settings() {
        let dir = dir();
        let mut cfg = Config::default();
        cfg.font_size = 48.;
        cfg.save(&dir.join("config.toml")).unwrap();
        // 内容很旧、但文件时间很新（下载器写完就是这种情况）。
        let mut old = cfg.clone();
        old.font_size = 40.;
        old.theme = "old-theme".into();
        let mut v = fixture(&old, "a");
        v["updated_at"] = json!(1);
        v["updated_at_ms"] = json!(1000);
        put(&dir, &v);
        let loaded = load(&dir).unwrap();
        assert_eq!(loaded.config.font_size, 48., "旧云档不得覆盖本机较新的设置");
        assert_ne!(loaded.config.theme, "old-theme");
        let mut state = loaded.state;
        state.save(&dir, &loaded.config, "a", "password-a", None, false).unwrap();
        let value = read(&dir.join("profile.json")).unwrap().unwrap();
        let merged = config(&value).unwrap();
        assert_eq!(merged.font_size, 48.);
        assert_ne!(merged.theme, "old-theme");
        let _ = std::fs::remove_dir_all(dir);
    }
    // 审计 C01 回归：设置写入器先写 config.toml（刷新本机 mtime）再进 save()，
    // 旧实现用 mtime 当基线，运行期下载到的新云档被当成旧档整份覆盖。
    #[test]
    fn newer_cloud_values_survive_the_config_mtime_refreshed_by_the_settings_writer() {
        let dir = dir();
        let mut cfg = Config::default();
        cfg.font_size = 36.;
        cfg.theme = "default".into();
        cfg.save(&dir.join("config.toml")).unwrap();
        let mut loaded = load(&dir).unwrap();
        // 另一台设备上传了较新的同步档：字号 72 + 青瓷主题。
        let mut remote = Config::default();
        remote.font_size = 72.;
        remote.theme = "青瓷".into();
        put(&dir, &fixture(&remote, "a"));
        // 用户只改了词提字号，且 config.toml 在云档之后落盘（就是写入器的顺序）。
        let mut local = loaded.config.clone();
        local.hint_font_size = 26.;
        local.save(&dir.join("config.toml")).unwrap();
        let adopted = loaded.state.save(&dir, &local, "a", "password-a", None, false).unwrap();
        let merged = config(&read(&dir.join("profile.json")).unwrap().unwrap()).unwrap();
        assert_eq!(merged.font_size, 72., "远端较新的字号不得被本机旧值覆盖");
        assert_eq!(merged.theme, "青瓷");
        assert_eq!(merged.hint_font_size, 26., "本机这次的修改必须保留");
        let adopted = adopted.expect("合并结果必须回传界面");
        assert_eq!(adopted.font_size, 72.);
        assert_eq!(adopted.hint_font_size, 26.);
        // 界面采用合并结果后再保存一次，不能把远端字段顶回去。
        assert!(loaded.state.save(&dir, &adopted, "a", "password-a", None, false).unwrap().is_none());
        let again = config(&read(&dir.join("profile.json")).unwrap().unwrap()).unwrap();
        assert_eq!(again.font_size, 72.);
        assert_eq!(again.theme, "青瓷");
        assert_eq!(again.hint_font_size, 26.);
        let _ = std::fs::remove_dir_all(dir);
    }
}
