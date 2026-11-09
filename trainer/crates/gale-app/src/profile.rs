//! Short atomic profile transactions shared with the server's WebDAV worker.
use gale_core::{config::Config, wenlai::WenlaiAccount};
use serde_json::{json, Value};
use std::{io::{Read, Write}, path::Path, time::{SystemTime, UNIX_EPOCH}};

const MAX_PROFILE: u64 = 1024 * 1024;

pub struct State {
    local_identity: Value,
    local_config: Value,
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
    let v: Value = serde_json::from_slice(&raw).map_err(|_| "跟打器同步配置 JSON 损坏".to_string())?;
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

pub fn load(dir: &Path) -> Result<Loaded, String> {
    let cfg_path = dir.join("config.toml");
    // Capture absence/mtime BEFORE load_or_create creates a fresh default file.
    let local_time = mtime(&cfg_path);
    let path = dir.join("profile.json");
    let (profile, warning) = match read(&path) {
        Ok(v) => (v, None),
        Err(e) => (None, Some(format!("已跳过同步配置，使用本机设置：{e}"))),
    };
    let mut cfg = Config::load_or_create(&cfg_path).map_err(|e| e.to_string())?;
    if let Some(v) = &profile {
        if local_time.is_none() || mtime(&path).is_some_and(|t| Some(t) >= local_time) {
            cfg = config(v)?;
            cfg.save(&cfg_path).map_err(|e| e.to_string())?;
        }
    }
    let v = profile.as_ref().unwrap_or(&Value::Null);
    let account = if v["online_account"].is_null() { None } else {
        Some(serde_json::from_value(v["online_account"].clone()).map_err(|_| "跟打器同步账号损坏".to_string())?)
    };
    Ok(Loaded {
        user: v["online_user"].as_str().unwrap_or("").to_string(),
        password: v["online_password"].as_str().unwrap_or("").to_string(),
        account,
        warning,
        state: State { local_identity: json!([v["online_user"].as_str().unwrap_or(""), v["online_password"].as_str().unwrap_or(""), v["online_account"]]), local_config: serde_json::to_value(&cfg).map_err(|e| e.to_string())? },
        config: cfg,
    })
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
    serde_json::from_value(merged).map_err(|e| format!("合并跟打器配置失败：{e}"))
}

impl State {
    pub fn save(&mut self, dir: &Path, cfg: &Config, user: &str, password: &str, account: Option<WenlaiAccount>, identity_changed: bool) -> Result<(), String> {
        use std::os::windows::fs::OpenOptionsExt;
        let path = dir.join("profile.json");
        let _lock = std::fs::OpenOptions::new().read(true).write(true).create(true).share_mode(0)
            .open(path.with_extension("lock")).map_err(|e| format!("云同步正在写入跟打器配置，请稍后重试：{e}"))?;
        let remote = read(&path)?;
        let local = serde_json::to_value(cfg).map_err(|e| e.to_string())?;
        let merged = match &remote {
            Some(v) => merge_config(&self.local_config, &local, &serde_json::to_value(config(v)?).map_err(|e| e.to_string())?)?,
            None => cfg.clone(),
        };
        // Export can be temporarily unavailable while DPAPI is being replaced.
        // Only an explicit account transition may clear the saved account.
        let account = match account {
            Some(a) => serde_json::to_value(a).map_err(|e| e.to_string())?,
            None if identity_changed => Value::Null,
            None => self.local_identity[2].clone(),
        };
        let local_identity = json!([user, password, account]);
        let identity = if !identity_changed && local_identity == self.local_identity {
            remote.as_ref().map(|r| json!([r["online_user"], r["online_password"], r["online_account"]])).unwrap_or_else(|| local_identity.clone())
        } else { local_identity.clone() };
        let mut value = json!({"version":1, "config_toml":merged.to_toml_text(),
            "online_user":identity[0], "online_password":identity[1], "online_account":identity[2]});
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
        }
        self.local_identity = local_identity;
        self.local_config = local;
        Ok(())
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
        json!({"version":1,"updated_at":1,"config_toml":cfg.to_toml_text(),
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
    fn unchanged_save_keeps_profile_timestamp_and_preserves_temporarily_unavailable_account() {
        let dir = dir(); let cfg = Config::default(); put(&dir, &fixture(&cfg, "a"));
        let before = std::fs::read(dir.join("profile.json")).unwrap();
        let mut loaded = load(&dir).unwrap();
        loaded.state.save(&dir, &loaded.config, "a", "password-a", None, false).unwrap();
        assert_eq!(std::fs::read(dir.join("profile.json")).unwrap(), before);
        assert_eq!(read(&dir.join("profile.json")).unwrap().unwrap()["online_account"]["jwt_token"], "token-a");
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn download_while_running_merges_untouched_settings_and_account_without_erasing_local_edits() {
        let dir = dir(); let cfg = Config::default(); put(&dir, &fixture(&cfg, "a"));
        let mut loaded = load(&dir).unwrap();
        let mut remote = cfg.clone(); remote.font_size = 40.; remote.theme = "remote-theme".into();
        put(&dir, &fixture(&remote, "b"));
        let mut local = loaded.config.clone(); local.hint_font_size = 20.; local.win_x = -1800;
        loaded.state.save(&dir, &local, "a", "password-a", loaded.account, false).unwrap();
        let value = read(&dir.join("profile.json")).unwrap().unwrap(); let merged = config(&value).unwrap();
        assert_eq!(merged.font_size, 40.); assert_eq!(merged.theme, "remote-theme");
        assert_eq!(merged.hint_font_size, 20.); assert_eq!(merged.win_x, -1800);
        assert_eq!(value["online_account"]["jwt_token"], "token-b");
        // A second save from the still-open UI must not revert the remote fields.
        loaded.state.save(&dir, &local, "a", "password-a", None, false).unwrap();
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
}
