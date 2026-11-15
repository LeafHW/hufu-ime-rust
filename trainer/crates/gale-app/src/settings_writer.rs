//! Coalesced settings persistence. UI actions only replace the pending value;
//! filesystem flushes, DPAPI and cloud-profile transactions stay off the UI thread.

use crate::{online, profile};
use gale_core::config::Config;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const SAVE_DELAY: Duration = Duration::from_millis(180);

#[derive(Clone)]
struct Job {
    sequence: u64,
    config: Config,
    user: String,
    password: String,
    account_cleared: bool,
    identity_changed: bool,
}

#[derive(Default)]
struct Pending {
    queued: u64,
    completed: u64,
    next: Option<Job>,
    failed: Option<Job>,
    error: Option<String>,
    force: bool,
    stopping: bool,
}

pub struct SettingsWriter {
    pending: Arc<(Mutex<Pending>, Condvar)>,
    /// Ok(Some(cfg)) = 云同步档带来的设置已合并，界面必须改用这份配置。
    results: mpsc::Receiver<Result<Option<Config>, String>>,
    worker: Option<JoinHandle<()>>,
}

impl SettingsWriter {
    pub fn new(dir: PathBuf, mut state: profile::State, wake_hwnd: isize) -> Self {
        let pending = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let worker_pending = Arc::clone(&pending);
        let (send, results) = mpsc::channel();
        let worker = thread::spawn(move || loop {
            let (lock, changed) = &*worker_pending;
            let job = {
                let mut p = lock.lock().unwrap_or_else(|e| e.into_inner());
                while p.next.is_none() && !p.stopping {
                    p = changed.wait(p).unwrap_or_else(|e| e.into_inner());
                }
                if p.next.is_none() && p.stopping {
                    break;
                }
                let mut revision = p.queued;
                let mut deadline = Instant::now() + SAVE_DELAY;
                while !p.force && !p.stopping {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    let (next, _) = changed.wait_timeout(p, remaining)
                        .unwrap_or_else(|e| e.into_inner());
                    p = next;
                    if revision != p.queued {
                        revision = p.queued;
                        deadline = Instant::now() + SAVE_DELAY;
                    }
                }
                p.force = false;
                p.next.take().expect("a queued settings value is present")
            };
            let result = save(&dir, &mut state, &job);
            {
                let mut p = lock.lock().unwrap_or_else(|e| e.into_inner());
                p.completed = job.sequence;
                p.error = result.as_ref().err().cloned();
                if result.is_err() {
                    // A later ordinary edit must still publish a failed explicit
                    // login/logout transition, using the later identity values.
                    if let Some(next) = &mut p.next {
                        next.identity_changed |= job.identity_changed;
                    } else {
                        p.failed = Some(job);
                    }
                } else {
                    p.failed = None;
                }
                changed.notify_all();
            }
            let _ = send.send(result);
            if wake_hwnd != 0 {
                use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
                use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_NULL};
                unsafe {
                    let _ = PostMessageW(HWND(wake_hwnd as *mut _), WM_NULL, WPARAM(0), LPARAM(0));
                }
            }
        });
        Self { pending, results, worker: Some(worker) }
    }

    /// Replaces the unsaved draft without waiting for disk or the profile lock.
    pub fn queue(
        &self,
        config: Config,
        user: String,
        password: String,
        account_cleared: bool,
        identity_changed: bool,
    ) {
        let (lock, changed) = &*self.pending;
        let mut p = lock.lock().unwrap_or_else(|e| e.into_inner());
        let identity_changed = identity_changed
            || p.next.as_ref().is_some_and(|j| j.identity_changed)
            || p.failed.as_ref().is_some_and(|j| j.identity_changed);
        p.failed = None;
        p.queued += 1;
        p.next = Some(Job {
            sequence: p.queued,
            config,
            user,
            password,
            account_cleared,
            identity_changed,
        });
        changed.notify_one();
    }

    pub fn try_result(&self) -> Option<Result<Option<Config>, String>> {
        self.results.try_recv().ok()
    }

    /// Forces the final draft to disk. A previous failed write is retried once.
    /// 【审计 C06】关闭/退出时等待保存必须有上限：云端或磁盘卡住时宁可放弃
    /// 等待并报错，也不能让窗口一直"正在保存"关不掉。
    pub fn flush(&self) -> Result<(), String> {
        self.flush_within(std::time::Duration::from_secs(8))
    }

    pub fn flush_within(&self, timeout: std::time::Duration) -> Result<(), String> {
        let (lock, changed) = &*self.pending;
        let mut p = lock.lock().unwrap_or_else(|e| e.into_inner());
        if p.next.is_none() && p.completed == p.queued {
            if let Some(mut retry) = p.failed.take() {
                p.queued += 1;
                retry.sequence = p.queued;
                p.next = Some(retry);
            } else {
                return p.error.clone().map_or(Ok(()), Err);
            }
        }
        let target = p.queued;
        p.force = true;
        changed.notify_one();
        let deadline = std::time::Instant::now() + timeout;
        while p.completed < target {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err("保存超时：设置还没写完，已放弃等待（下次启动会重试）".into());
            }
            let (guard, _) = changed
                .wait_timeout(p, left)
                .unwrap_or_else(|e| e.into_inner());
            p = guard;
        }
        match &p.error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

impl Drop for SettingsWriter {
    fn drop(&mut self) {
        let _ = self.flush();
        let (lock, changed) = &*self.pending;
        lock.lock().unwrap_or_else(|e| e.into_inner()).stopping = true;
        changed.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn save(dir: &std::path::Path, state: &mut profile::State, job: &Job) -> Result<Option<Config>, String> {
    // Local preferences remain durable even if cloud-profile export is locked
    // or malformed. Both operations run on this single background writer.
    job.config.save(&dir.join("config.toml"))
        .map_err(|e| format!("保存本机跟打器设置失败：{e}"))?;
    let account = online::export_account(dir, &job.config.wenlai_domain, job.account_cleared);
    state.save(dir, &job.config, &job.user, &job.password, account, job.identity_changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let dir = std::env::temp_dir().join(format!("trainer-settings-writer-{}-{unique}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }
    fn read_profile(dir: &TestDir) -> Value {
        serde_json::from_slice(&std::fs::read(dir.0.join("profile.json")).unwrap()).unwrap()
    }

    #[test]
    fn flush_saves_latest_draft_after_a_burst() {
        let dir = TestDir::new();
        let loaded = profile::load(&dir.0).unwrap();
        let writer = SettingsWriter::new(dir.0.clone(), loaded.state, 0);
        for n in 11..=100 {
            let config = Config { font_size: n as f32, ..Default::default() };
            writer.queue(config, String::new(), String::new(), false, false);
        }
        writer.flush().unwrap();
        let config = Config::load_or_create(&dir.0.join("config.toml")).unwrap();
        assert_eq!(config.font_size, 100.);
        let cloud = read_profile(&dir);
        assert_eq!(Config::from_toml_text(cloud["config_toml"].as_str().unwrap()).unwrap().font_size, 100.);
    }

    #[test]
    fn coalesced_ordinary_edit_keeps_explicit_logout() {
        let dir = TestDir::new();
        let config = Config::default();
        let cloud = json!({"version":1, "config_toml":config.to_toml_text(),
            "online_user":"fixture", "online_password":"fixture", "online_account":{
                "domain":config.wenlai_domain, "username":"fixture", "display_name":"fixture",
                "user_id":1, "jwt_token":"fixture-token"}});
        std::fs::write(dir.0.join("profile.json"), cloud.to_string()).unwrap();
        let loaded = profile::load(&dir.0).unwrap();
        let writer = SettingsWriter::new(dir.0.clone(), loaded.state, 0);
        writer.queue(config.clone(), String::new(), String::new(), true, true);
        writer.queue(Config { font_size: 72., ..config }, String::new(), String::new(), true, false);
        writer.flush().unwrap();
        let cloud = read_profile(&dir);
        assert_eq!(cloud["online_user"], "");
        assert_eq!(cloud["online_password"], "");
        assert!(cloud["online_account"].is_null());
        assert_eq!(Config::load_or_create(&dir.0.join("config.toml")).unwrap().font_size, 72.);
    }

    #[test]
    fn locked_cloud_reports_failure_but_flush_can_retry_without_losing_local_edit() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = TestDir::new();
        let loaded = profile::load(&dir.0).unwrap();
        let writer = SettingsWriter::new(dir.0.clone(), loaded.state, 0);
        let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).share_mode(0)
            .open(dir.0.join("profile.lock")).unwrap();
        writer.queue(Config { font_size: 63., ..Default::default() }, String::new(), String::new(), true, true);
        assert!(writer.flush().unwrap_err().contains("稍后重试"));
        assert_eq!(Config::load_or_create(&dir.0.join("config.toml")).unwrap().font_size, 63.);
        drop(lock);
        writer.flush().unwrap();
        let cloud = read_profile(&dir);
        assert_eq!(Config::from_toml_text(cloud["config_toml"].as_str().unwrap()).unwrap().font_size, 63.);
        assert!(cloud["online_account"].is_null());
    }
}
