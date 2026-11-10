//! 晴发文后台工作线程。WinHTTP/DPAPI均为系统API；网络和凭据加密在后台。
//! 服务器切换/退出立即使在途结果失效；旧取文结果不会覆盖新预览。
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::{Duration, Instant};

use gale_core::wenlai::{
    self, FetchParams, HttpMethod, HttpRequest, HttpResponse, HttpTransport, ShelfBook,
    WenlaiAccount, WenlaiArticle, WenlaiClient, WenlaiHttpClient,
};
use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

const MAX_RESPONSE: usize = 5 * 1024 * 1024;
const MAX_CREDENTIAL_FILE: u64 = 128 * 1024;
const MAX_QUEUE: usize = 16;
const DEFAULT_SERVER: &str = "https://qingfawen.fcxxz.com";

fn clear_secret(bytes: &mut [u8]) {
    for byte in bytes {
        unsafe {
            std::ptr::write_volatile(byte, 0);
        }
    }
    std::sync::atomic::compiler_fence(Ordering::SeqCst);
}

#[derive(Clone, Debug)]
pub struct AccountSummary {
    pub domain: String,
    pub username: String,
    pub display_name: String,
    pub user_id: i64,
}

/// 整改2b：在线练习偏好（"自动下一段"开关，默认开）。
/// config.toml 归 gale-core 主配置（本域禁改），此处独立持久化在
/// 数据目录 `online.json`（即 %LOCALAPPDATA%\HuFu\trainer\online.json，
/// 与凭据文件同目录的 sidecar 口径）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnlinePrefs {
    /// 一段结算后 1.5s 按当前随机/顺序来源取下一段
    #[serde(default = "default_auto_next")]
    pub auto_next: bool,
}

fn default_auto_next() -> bool {
    true
}

impl Default for OnlinePrefs {
    fn default() -> Self {
        Self { auto_next: true }
    }
}

impl OnlinePrefs {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() > 4096 {
            return Err("在线偏好文件过大".into());
        }
        std::fs::write(path, bytes).map_err(|e| e.to_string())
    }
}
impl From<&WenlaiAccount> for AccountSummary {
    fn from(a: &WenlaiAccount) -> Self {
        Self {
            domain: a.domain.clone(),
            username: a.username.clone(),
            display_name: a.display_name.clone(),
            user_id: a.user_id,
        }
    }
}

/// 密码只在本次登录中使用，不持久化，也不实现Debug。
pub enum OnlineCommand {
    Login {
        username: String,
        password: String,
    },
    Logout,
    SetServer {
        base_url: String,
    },
    Fetch(FetchParams),
    Categories,
    Bookshelf,
    CurrentUser,
    /// 注册第一步：POST /api/auth/register/sendCode（60s 服务端冷却，
    /// 错误 msg 透传）；成功回 CodeSent，UI 侧启动 60 秒重发冷却。
    RegisterSendCode {
        email: String,
    },
    /// 注册第二步：POST /api/auth/register（core 内成功后自动 login 拿
    /// token），成功按 Login 同型回 Account 并落盘凭据。
    Register {
        username: String,
        password: String,
        email: String,
        code: String,
    },
}

impl Drop for OnlineCommand {
    fn drop(&mut self) {
        // Also covers rejected submissions, superseded logins, and queued work at shutdown.
        if let Self::Login { password, .. } | Self::Register { password, .. } = self {
            unsafe {
                clear_secret(password.as_bytes_mut());
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum OnlinePayload {
    Account(Option<AccountSummary>),
    /// Parameters belong to the accepted result, so failed or superseded fetches
    /// cannot change how the current article is continued.
    Article(WenlaiArticle, FetchParams),
    Categories(Vec<(String, String)>),
    Bookshelf(Vec<ShelfBook>),
    ServerChanged(String),
    LoggedOut,
    /// 注册验证码已发出（UI 侧据此启动 60 秒重发冷却并提示查收邮箱）。
    CodeSent,
    /// 操作已成功，但凭据落盘失败等非致命状态需要明示。
    Warning(String),
}

#[derive(Debug)]
pub struct OnlineResult {
    pub id: u64,
    pub result: Result<OnlinePayload, String>,
}

struct Envelope {
    id: u64,
    epoch: u64,
    command: OnlineCommand,
}
struct WorkerResult {
    epoch: u64,
    is_fetch: bool,
    value: OnlineResult,
}
struct PendingTicket(Arc<AtomicUsize>);
impl Drop for PendingTicket {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct OnlineWorker {
    sender: mpsc::Sender<Envelope>,
    receiver: mpsc::Receiver<WorkerResult>,
    epoch: Arc<AtomicU64>,
    latest_fetch: Arc<AtomicU64>,
    pending: Arc<AtomicUsize>,
    credential_path: PathBuf,
    credential_guard: Arc<Mutex<()>>,
    result_sender: mpsc::Sender<WorkerResult>,
    next_id: u64,
}

impl OnlineWorker {
    pub fn spawn(data_dir: PathBuf, base_url: &str) -> Result<Self, String> {
        Self::spawn_with_transport(data_dir, base_url, WinHttpTransport)
    }

    fn spawn_with_transport<T: HttpTransport + Clone + Send + 'static>(
        data_dir: PathBuf,
        base_url: &str,
        transport: T,
    ) -> Result<Self, String> {
        let server = wenlai::normalize_base_url(base_url)?;
        let (sender, rx) = mpsc::channel::<Envelope>();
        let (tx, receiver) = mpsc::channel::<WorkerResult>();
        let result_sender = tx.clone();
        let epoch = Arc::new(AtomicU64::new(1));
        let latest_fetch = Arc::new(AtomicU64::new(0));
        let pending = Arc::new(AtomicUsize::new(0));
        let credential_path = data_dir.join("online-auth.dpapi");
        let credentials = credential_path.clone();
        let credential_guard = Arc::new(Mutex::new(()));
        let worker_guard = credential_guard.clone();
        let (worker_epoch, worker_fetch, worker_pending) =
            (epoch.clone(), latest_fetch.clone(), pending.clone());
        std::thread::Builder::new()
            .name("hufu-online".into())
            .spawn(move || {
                let mut client = WenlaiHttpClient::new(&server, transport.clone());
                // No network at application start. Restored credentials are checked on first request.
                match load_account(&credentials, &server) {
                    Ok(Some(account)) => {
                        let summary = AccountSummary::from(&account);
                        let restored = client.restore_account(account);
                        if worker_epoch.load(Ordering::Acquire) == 1 {
                            let result = match restored {
                                Ok(()) => OnlinePayload::Account(Some(summary)),
                                Err(e) => OnlinePayload::Warning(e),
                            };
                            let _ = tx.send(WorkerResult {
                                epoch: 1,
                                is_fetch: false,
                                value: OnlineResult {
                                    id: 0,
                                    result: Ok(result),
                                },
                            });
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let _ = tx.send(WorkerResult {
                            epoch: 1,
                            is_fetch: false,
                            value: OnlineResult {
                                id: 0,
                                result: Ok(OnlinePayload::Warning(e)),
                            },
                        });
                    }
                }
                while let Ok(mut env) = rx.recv() {
                    let _pending = PendingTicket(worker_pending.clone());
                    if env.epoch != worker_epoch.load(Ordering::Acquire) {
                        continue;
                    }
                    let is_fetch = matches!(&env.command, OnlineCommand::Fetch(_));
                    if is_fetch && env.id != worker_fetch.load(Ordering::Acquire) {
                        continue;
                    }
                    let mut persist = None;
                    let mut remove_credentials = false;
                    let mut result = match &mut env.command {
                        OnlineCommand::Login { username, password } => {
                            // A failed account switch must not restore the previous account next launch.
                            remove_credentials = true;
                            let r = client
                                .login(username.trim(), password)
                                .map_err(|e| e.to_string());
                            // Drop the password promptly. It is never included in a result/error/log.
                            unsafe {
                                clear_secret(password.as_bytes_mut());
                            }
                            match r {
                                Ok(account) => {
                                    persist = Some(account.clone());
                                    Ok(OnlinePayload::Account(Some(AccountSummary::from(&account))))
                                }
                                Err(e) => Err(e),
                            }
                        }
                        OnlineCommand::Logout => {
                            client.logout();
                            remove_credentials = true;
                            Ok(OnlinePayload::LoggedOut)
                        }
                        OnlineCommand::SetServer { base_url } => {
                            match wenlai::normalize_base_url(base_url) {
                                Ok(server) => {
                                    client = WenlaiHttpClient::new(&server, transport.clone());
                                    remove_credentials = true;
                                    Ok(OnlinePayload::ServerChanged(server))
                                }
                                Err(e) => Err(e),
                            }
                        }
                        OnlineCommand::Fetch(params) => client
                            .fetch(params)
                            .map(|article| OnlinePayload::Article(article, params.clone()))
                            .map_err(|e| e.to_string()),
                        OnlineCommand::Categories => client
                            .categories()
                            .map(OnlinePayload::Categories)
                            .map_err(|e| e.to_string()),
                        OnlineCommand::Bookshelf => {
                            client.bookshelf().map(OnlinePayload::Bookshelf)
                        }
                        OnlineCommand::CurrentUser => client.current_user().map(|_| {
                            OnlinePayload::Account(client.account().map(AccountSummary::from))
                        }),
                        OnlineCommand::RegisterSendCode { email } => client
                            .register_send_code(email.trim())
                            .map(|_| OnlinePayload::CodeSent)
                            .map_err(|e| e.to_string()),
                        OnlineCommand::Register {
                            username,
                            password,
                            email,
                            code,
                        } => {
                            // Mirrors Login: a failed register must not restore the
                            // previous account next launch; the password is dropped
                            // right after use and never included in a result/log.
                            remove_credentials = true;
                            let r = client
                                .register(username, password, email.trim(), code.trim())
                                .map_err(|e| e.to_string());
                            unsafe {
                                clear_secret(password.as_bytes_mut());
                            }
                            match r {
                                Ok(account) => {
                                    persist = Some(account.clone());
                                    Ok(OnlinePayload::Account(Some(AccountSummary::from(&account))))
                                }
                                Err(e) => Err(e),
                            }
                        }
                    };
                    // Credentials must not be written by an invalidated login after logout/server change.
                    if env.epoch != worker_epoch.load(Ordering::Acquire) {
                        continue;
                    }
                    if is_fetch && env.id != worker_fetch.load(Ordering::Acquire) {
                        continue;
                    }
                    if result
                        .as_ref()
                        .err()
                        .map(|e| wenlai::auth_expired(e))
                        .unwrap_or(false)
                    {
                        client.logout();
                        remove_credentials = true;
                        let _ = tx.send(WorkerResult {
                            epoch: env.epoch,
                            is_fetch,
                            value: OnlineResult {
                                id: env.id,
                                result: Ok(OnlinePayload::Account(None)),
                            },
                        });
                    }
                    if remove_credentials {
                        let _guard = worker_guard.lock().unwrap_or_else(|e| e.into_inner());
                        if env.epoch != worker_epoch.load(Ordering::Acquire) {
                            continue;
                        }
                        match invalidate_saved_account(&credentials) {
                            Ok(Some(e)) => {
                                let _ = tx.send(WorkerResult {
                                    epoch: env.epoch,
                                    is_fetch,
                                    value: OnlineResult {
                                        id: env.id,
                                        result: Ok(OnlinePayload::Warning(e)),
                                    },
                                });
                            }
                            Ok(None) => {}
                            Err(e) => {
                                persist = None;
                                result = Err(e);
                            }
                        }
                    }
                    if let Some(account) = persist {
                        let saved = save_account_impl(
                            &credentials,
                            &account,
                            Some((&worker_epoch, env.epoch, &worker_guard)),
                        );
                        if matches!(saved, Ok(false)) {
                            continue;
                        }
                        if let Err(e) = saved {
                            let _ = tx.send(WorkerResult {
                                epoch: env.epoch,
                                is_fetch,
                                value: OnlineResult {
                                    id: env.id,
                                    result: Ok(OnlinePayload::Warning(format!(
                                        "本次已登录，但无法保存登录状态：{e}"
                                    ))),
                                },
                            });
                        }
                        // Logout/server-change/Drop may invalidate while DPAPI or disk I/O runs.
                        // The single worker cannot concurrently commit a newer account here.
                        if env.epoch != worker_epoch.load(Ordering::Acquire) {
                            let _ = remove_account(&credentials);
                            continue;
                        }
                    }
                    let _ = tx.send(WorkerResult {
                        epoch: env.epoch,
                        is_fetch,
                        value: OnlineResult { id: env.id, result },
                    });
                }
            })
            .map_err(|e| format!("无法启动在线取文线程：{e}"))?;
        Ok(Self {
            sender,
            receiver,
            epoch,
            latest_fetch,
            pending,
            credential_path,
            credential_guard,
            result_sender,
            next_id: 1,
        })
    }

    pub fn submit(&mut self, command: OnlineCommand) -> Result<u64, String> {
        if let OnlineCommand::SetServer { base_url } = &command {
            wenlai::normalize_base_url(base_url)?;
        }
        let id = self.next_id;
        let next_id = self
            .next_id
            .checked_add(1)
            .ok_or("请求编号已用尽，请重启跟打器")?;
        let pending_before = self.pending.fetch_add(1, Ordering::AcqRel);
        // Reserve one extra slot so logout can invalidate a full queue immediately.
        if pending_before >= MAX_QUEUE
            && !(pending_before == MAX_QUEUE && matches!(&command, OnlineCommand::Logout))
        {
            self.pending.fetch_sub(1, Ordering::AcqRel);
            return Err("在线操作较多，请等待当前请求完成".into());
        }
        self.next_id = next_id;
        let epoch = if matches!(
            &command,
            OnlineCommand::Login { .. }
                | OnlineCommand::Logout
                | OnlineCommand::SetServer { .. }
                | OnlineCommand::Register { .. }
        ) {
            // Only epoch change, deletion and final rename hold this lock. DPAPI and
            // writing ciphertext to a temporary file happen entirely on the worker.
            let _guard = self
                .credential_guard
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
            let warning = match invalidate_saved_account(&self.credential_path) {
                Ok(w) => w,
                Err(e) => Some(e),
            };
            if let Some(warning) = warning {
                let _ = self.result_sender.send(WorkerResult {
                    epoch,
                    is_fetch: false,
                    value: OnlineResult {
                        id,
                        result: Ok(OnlinePayload::Warning(warning)),
                    },
                });
            }
            epoch
        } else {
            self.epoch.load(Ordering::Acquire)
        };
        if matches!(&command, OnlineCommand::Fetch(_)) {
            self.latest_fetch.store(id, Ordering::Release);
        }
        self.sender
            .send(Envelope { id, epoch, command })
            .map_err(|_| {
                self.pending.fetch_sub(1, Ordering::AcqRel);
                "在线工作线程已退出，请重启跟打器".to_string()
            })?;
        Ok(id)
    }

    pub fn try_recv(&self) -> Option<OnlineResult> {
        while let Ok(result) = self.receiver.try_recv() {
            if result.epoch == self.epoch.load(Ordering::Acquire)
                && (!result.is_fetch
                    || result.value.id == self.latest_fetch.load(Ordering::Acquire))
            {
                return Some(result.value);
            }
        }
        None
    }
    /// Includes queued and executing commands, including invalidated work winding down.
    /// The UI must not infer this count from results: canceled results are suppressed.
    pub fn pending_count(&self) -> usize {
        self.pending.load(Ordering::Acquire)
    }
    pub fn is_busy(&self) -> bool {
        self.pending_count() != 0
    }
}

impl Drop for OnlineWorker {
    fn drop(&mut self) {
        // Invalidate queued work before the sender disconnects. No UI shutdown wait.
        let _guard = self
            .credential_guard
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }
}

struct InternetHandle(Arc<AtomicPtr<c_void>>);
impl InternetHandle {
    fn new(value: *mut c_void, stage: &str) -> Result<Self, String> {
        if value.is_null() {
            Err(format!(
                "{stage}失败（系统错误{}）",
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
            ))
        } else {
            Ok(Self(Arc::new(AtomicPtr::new(value))))
        }
    }
    fn raw(&self) -> *mut c_void {
        self.0.load(Ordering::Acquire)
    }
    fn active(&self) -> Result<*mut c_void, String> {
        let raw = self.raw();
        if raw.is_null() {
            Err("网络请求超过30秒，请稍后重试".into())
        } else {
            Ok(raw)
        }
    }
}
impl Drop for InternetHandle {
    fn drop(&mut self) {
        let raw = self.0.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !raw.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(raw);
            }
        }
    }
}

/// Native WinHTTP cancellation enforces one absolute deadline, including DNS,
/// connect, server headers and trickling response bodies. Closing the request
/// cancels an in-progress synchronous API; the atomic prevents a double close.
struct NetworkDeadline {
    stop: mpsc::Sender<()>,
}
impl NetworkDeadline {
    fn start(request: &InternetHandle, started: Instant) -> Result<Self, String> {
        let remaining = Duration::from_secs(30)
            .checked_sub(started.elapsed())
            .ok_or("网络请求超过30秒，请稍后重试")?;
        let (stop, rx) = mpsc::channel();
        let raw = request.0.clone();
        std::thread::Builder::new()
            .name("hufu-net-deadline".into())
            .spawn(move || {
                if matches!(
                    rx.recv_timeout(remaining),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    let raw = raw.swap(std::ptr::null_mut(), Ordering::AcqRel);
                    if !raw.is_null() {
                        unsafe {
                            let _ = WinHttpCloseHandle(raw);
                        }
                    }
                }
            })
            .map_err(|_| "无法启动网络超时保护，请稍后重试".to_string())?;
        Ok(Self { stop })
    }
}
impl Drop for NetworkDeadline {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

#[derive(Clone, Copy)]
pub struct WinHttpTransport;

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn win_error(stage: &str, e: windows::core::Error) -> String {
    // HRESULT only: never include request headers, passwords, tokens or server bodies.
    format!("{stage}失败（系统代码{:08X}）", e.code().0 as u32)
}

impl HttpTransport for WinHttpTransport {
    fn request(&self, req: &HttpRequest) -> Result<HttpResponse, String> {
        if req.url.len() > 16_384 || req.url.contains(['\0', '\r', '\n']) {
            return Err("请求地址无效或过长".into());
        }
        let rest = req
            .url
            .strip_prefix("https://")
            .ok_or("在线服务必须使用HTTPS")?;
        let slash = rest.find('/').unwrap_or(rest.len());
        let origin = wenlai::normalize_base_url(&format!("https://{}", &rest[..slash]))?;
        let authority = &origin[8..];
        let (host, port) = if authority.starts_with('[') {
            let end = authority.find(']').ok_or("IPv6地址无效")?;
            (
                &authority[1..end],
                authority[end + 1..]
                    .strip_prefix(':')
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(443),
            )
        } else {
            authority
                .rsplit_once(':')
                .map(|(h, p)| (h, p.parse::<u16>().unwrap_or(443)))
                .unwrap_or((authority, 443))
        };
        let object = if slash == rest.len() {
            "/"
        } else {
            &rest[slash..]
        };
        if object.contains('#') || object.contains('\\') {
            return Err("请求路径无效".into());
        }
        let body = req.body.as_deref().unwrap_or("").as_bytes();
        if body.len() > 1024 * 1024 {
            return Err("请求内容过大".into());
        }
        let mut headers = String::from("Accept: application/json\r\n");
        for (name, value) in &req.headers {
            if name.is_empty()
                || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || value.contains(['\r', '\n', '\0'])
            {
                return Err("请求头格式无效".into());
            }
            headers.push_str(name);
            headers.push_str(": ");
            headers.push_str(value);
            headers.push_str("\r\n");
        }
        let started = Instant::now();
        // 代理名与请求头 UA 统一取 gale-core 的 DEFAULT_UA（Chrome 120）：
        // 晴服务端按 UA 甄别客户端，两处不一致可能被拦。
        let agent = wide(gale_core::wenlai::DEFAULT_UA);
        let host = wide(host);
        let object = wide(object);
        let method = wide(if req.method == HttpMethod::Post {
            "POST"
        } else {
            "GET"
        });
        let headers = headers.encode_utf16().collect::<Vec<_>>();
        unsafe {
            let session = InternetHandle::new(
                WinHttpOpen(
                    PCWSTR(agent.as_ptr()),
                    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    0,
                ),
                "初始化网络",
            )?;
            WinHttpSetTimeouts(session.raw(), 5_000, 5_000, 5_000, 15_000)
                .map_err(|e| win_error("设置超时", e))?;
            let connection = InternetHandle::new(
                WinHttpConnect(session.raw(), PCWSTR(host.as_ptr()), port, 0),
                "连接服务器",
            )?;
            let request = InternetHandle::new(
                WinHttpOpenRequest(
                    connection.raw(),
                    PCWSTR(method.as_ptr()),
                    PCWSTR(object.as_ptr()),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    std::ptr::null(),
                    WINHTTP_FLAG_SECURE,
                ),
                "创建请求",
            )?;
            let _deadline = NetworkDeadline::start(&request, started)?;
            // Disable redirects completely; a bearer token/password must never reach another origin.
            WinHttpSetOption(
                Some(request.active()?),
                WINHTTP_OPTION_REDIRECT_POLICY,
                Some(&WINHTTP_OPTION_REDIRECT_POLICY_NEVER.to_ne_bytes()),
            )
            .map_err(|e| win_error("禁止凭据重定向", e))?;
            let decompress = WINHTTP_DECOMPRESSION_FLAG_GZIP | WINHTTP_DECOMPRESSION_FLAG_DEFLATE;
            WinHttpSetOption(
                Some(request.active()?),
                WINHTTP_OPTION_DECOMPRESSION,
                Some(&decompress.to_ne_bytes()),
            )
            .map_err(|e| win_error("设置传输压缩", e))?;
            WinHttpSendRequest(
                request.active()?,
                Some(&headers),
                if body.is_empty() {
                    None
                } else {
                    Some(body.as_ptr().cast())
                },
                body.len() as u32,
                body.len() as u32,
                0,
            )
            .map_err(|e| win_error("发送请求", e))?;
            WinHttpReceiveResponse(request.active()?, std::ptr::null_mut())
                .map_err(|e| win_error("接收响应", e))?;
            let mut status = 0u32;
            let mut status_len = std::mem::size_of::<u32>() as u32;
            WinHttpQueryHeaders(
                request.active()?,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some((&mut status as *mut u32).cast()),
                &mut status_len,
                std::ptr::null_mut(),
            )
            .map_err(|e| win_error("读取响应状态", e))?;
            let mut bytes = Vec::new();
            loop {
                if started.elapsed() >= Duration::from_secs(30) {
                    return Err("网络请求超过30秒，请稍后重试".into());
                }
                let mut chunk = [0u8; 16 * 1024];
                let mut read = 0;
                WinHttpReadData(
                    request.active()?,
                    chunk.as_mut_ptr().cast(),
                    chunk.len() as u32,
                    &mut read,
                )
                .map_err(|e| win_error("读取响应", e))?;
                if read == 0 {
                    break;
                }
                if bytes.len().saturating_add(read as usize) > MAX_RESPONSE {
                    return Err("服务器响应超过5MB，已中止".into());
                }
                bytes.extend_from_slice(&chunk[..read as usize]);
            }
            let body =
                String::from_utf8(bytes).map_err(|_| "服务器响应不是有效UTF-8".to_string())?;
            Ok(HttpResponse {
                status: status as u16,
                body,
            })
        }
    }
}

fn dpapi(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    if data.is_empty() || data.len() > MAX_CREDENTIAL_FILE as usize {
        return Err("登录状态大小无效".into());
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        let result = if encrypt {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        result.map_err(|_| "无法处理本机登录凭据，请重新登录".to_string())?;
        if output.pbData.is_null() {
            return Err("系统未返回登录凭据".into());
        }
        let mut result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        clear_secret(std::slice::from_raw_parts_mut(
            output.pbData,
            output.cbData as usize,
        ));
        let _ = LocalFree(HLOCAL(output.pbData.cast()));
        if result.len() > MAX_CREDENTIAL_FILE as usize {
            clear_secret(&mut result);
            return Err("保存的登录状态过大".into());
        }
        Ok(result)
    }
}

#[cfg(test)]
fn save_account(path: &Path, account: &WenlaiAccount) -> Result<(), String> {
    store_account(path, account)
}

/// 供云同步 profile 导入登录态（JWT）用；不会写明文口令。
pub fn store_account(path: &Path, account: &WenlaiAccount) -> Result<(), String> {
    save_account_impl(path, account, None).map(|_| ())
}

/// 读本机 DPAPI 登录态，供 profile.json 带上会话（跨机免重输密码）。
pub fn export_account(
    data_dir: &Path,
    server: &str,
    explicitly_cleared: bool,
) -> Option<WenlaiAccount> {
    if explicitly_cleared {
        return None;
    }
    load_account(&data_dir.join("online-auth.dpapi"), server)
        .ok()
        .flatten()
}

/// Import only a newer profile for the active server. A durable local logout
/// marker always wins, including when a locked credential file still exists.
pub fn import_synced_account(
    data_dir: &Path,
    server: &str,
    account: &WenlaiAccount,
) -> Result<(), String> {
    let path = data_dir.join("online-auth.dpapi");
    if signed_out_marker(&path)
        .try_exists()
        .map_err(|e| format!("无法核实本机退出状态：{e}"))?
    {
        return Ok(());
    }
    if wenlai::normalize_base_url(&account.domain)? != wenlai::normalize_base_url(server)? {
        return Ok(());
    }
    let remote_time = std::fs::metadata(data_dir.join("profile.json"))
        .and_then(|m| m.modified())
        .ok();
    let local_time = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    if local_time.is_some() && remote_time <= local_time {
        return Ok(());
    }
    store_account(&path, account)
}

fn save_account_impl(
    path: &Path,
    account: &WenlaiAccount,
    gate: Option<(&AtomicU64, u64, &Mutex<()>)>,
) -> Result<bool, String> {
    wenlai::normalize_base_url(&account.domain)?;
    let mut plain = serde_json::to_vec(account).map_err(|_| "无法编码登录状态".to_string())?;
    let protected = dpapi(&plain, true);
    clear_secret(&mut plain);
    let protected = protected?;
    let parent = path.parent().ok_or("登录状态目录无效")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("无法创建登录状态目录：{e}"))?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    {
        use std::io::Write;
        let mut file =
            std::fs::File::create(&temp).map_err(|e| format!("无法保存加密登录状态：{e}"))?;
        if let Err(e) = file.write_all(&protected).and_then(|_| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&temp);
            return Err(format!("无法完整保存加密登录状态：{e}"));
        }
    }
    let _guard = gate.map(|(_, _, lock)| lock.lock().unwrap_or_else(|e| e.into_inner()));
    if gate
        .map(|(epoch, expected, _)| epoch.load(Ordering::Acquire) != expected)
        .unwrap_or(false)
    {
        let _ = std::fs::remove_file(&temp);
        return Ok(false);
    }
    if let Err(e) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("无法替换登录状态：{e}"));
    }
    match std::fs::remove_file(signed_out_marker(path)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("登录状态已加密，但无法清除退出标记；下次启动需重新登录".into()),
    }
    Ok(true)
}

fn load_account(path: &Path, server: &str) -> Result<Option<WenlaiAccount>, String> {
    match std::fs::metadata(signed_out_marker(path)) {
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("无法核实本机退出状态，已阻止恢复账号".into()),
    }
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法读取保存的登录状态，请重新登录".into()),
    };
    if meta.len() > MAX_CREDENTIAL_FILE {
        return Err("保存的登录状态过大，请重新登录".into());
    }
    let encrypted = std::fs::read(path).map_err(|_| "无法读取登录状态".to_string())?;
    let mut plain = dpapi(&encrypted, false)?;
    let account = serde_json::from_slice::<WenlaiAccount>(&plain);
    clear_secret(&mut plain);
    let account = account.map_err(|_| "保存的登录状态损坏，请重新登录".to_string())?;
    if wenlai::normalize_base_url(&account.domain)? != wenlai::normalize_base_url(server)? {
        return Ok(None);
    }
    Ok(Some(account))
}

fn remove_account(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("无法移除本机登录凭据，请检查数据目录权限".into()),
    }
}

fn signed_out_marker(path: &Path) -> PathBuf {
    path.with_extension("signed-out")
}

/// If another process has locked the encrypted file, a durable non-secret marker
/// still prevents restoration. Never announce a completed logout if neither works.
fn invalidate_saved_account(path: &Path) -> Result<Option<String>, String> {
    match remove_account(path) {
        Ok(()) => Ok(None),
        Err(_) => {
            use std::io::Write;
            let blocked = std::fs::File::create(signed_out_marker(path)).and_then(|mut file| {
                file.write_all(b"signed-out\n")?;
                file.sync_all()
            });
            if blocked.is_ok() {
                Ok(Some(
                    "本机凭据文件暂时无法移除，已阻止其恢复；请检查数据目录权限".into(),
                ))
            } else {
                Err("退出登录尚未完成：无法移除或禁用本机凭据。请检查数据目录权限后重试".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_session_clear_wins_even_before_worker_removes_old_dpapi_file() {
        let dir = test_dir("profile-clear");
        let account = WenlaiAccount {
            domain: DEFAULT_SERVER.into(),
            username: "old-user".into(),
            display_name: "old-user".into(),
            user_id: 7,
            jwt_token: "old-test-token".into(),
        };
        store_account(&dir.join("online-auth.dpapi"), &account).unwrap();
        assert!(export_account(&dir, DEFAULT_SERVER, false).is_some());
        assert!(export_account(&dir, DEFAULT_SERVER, true).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn synchronized_account_never_overrides_explicit_local_logout_or_other_domain() {
        let dir = test_dir("profile-import");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("online-auth.dpapi");
        let account = WenlaiAccount {
            domain: DEFAULT_SERVER.into(),
            username: "profile-user".into(),
            display_name: "profile-user".into(),
            user_id: 7,
            jwt_token: "test-profile-token".into(),
        };
        std::fs::write(dir.join("profile.json"), b"profile").unwrap();
        std::fs::write(signed_out_marker(&path), b"signed-out").unwrap();
        import_synced_account(&dir, DEFAULT_SERVER, &account).unwrap();
        assert!(!path.exists());
        std::fs::remove_file(signed_out_marker(&path)).unwrap();
        import_synced_account(&dir, "https://example.com", &account).unwrap();
        assert!(!path.exists());
        import_synced_account(&dir, DEFAULT_SERVER, &account).unwrap();
        assert_eq!(
            load_account(&path, DEFAULT_SERVER)
                .unwrap()
                .unwrap()
                .username,
            "profile-user"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "hufu-online-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    fn drain_until_idle(worker: &OnlineWorker) -> Vec<OnlineResult> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut results = Vec::new();
        loop {
            while let Some(value) = worker.try_recv() {
                results.push(value);
            }
            if !worker.is_busy() {
                break;
            }
            assert!(Instant::now() < deadline, "worker did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
        while let Some(value) = worker.try_recv() {
            results.push(value);
        }
        results
    }
    #[test]
    fn protected_credentials_never_contain_plaintext_and_are_domain_bound() {
        let path = std::env::temp_dir().join(format!(
            "hufu-online-test-{}-{}.dpapi",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let account = WenlaiAccount {
            domain: DEFAULT_SERVER.into(),
            username: "test".into(),
            display_name: "测试".into(),
            user_id: 1,
            jwt_token: "test-secret-token-not-for-log".into(),
        };
        save_account(&path, &account).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes
            .windows(account.jwt_token.len())
            .any(|w| w == account.jwt_token.as_bytes()));
        assert_eq!(
            load_account(&path, DEFAULT_SERVER)
                .unwrap()
                .unwrap()
                .jwt_token,
            account.jwt_token
        );
        assert!(load_account(&path, "https://example.com")
            .unwrap()
            .is_none());
        let mut replacement = account.clone();
        replacement.jwt_token = "replacement-secret".into();
        save_account(&path, &replacement).unwrap();
        assert_eq!(
            load_account(&path, DEFAULT_SERVER)
                .unwrap()
                .unwrap()
                .jwt_token,
            replacement.jwt_token
        );
        remove_account(&path).unwrap();
    }
    #[test]
    fn malformed_or_plaintext_credentials_are_rejected() {
        assert!(dpapi(b"not a dpapi blob", false).is_err());
        let req = HttpRequest {
            method: HttpMethod::Get,
            url: "http://example.com/api/categories".into(),
            headers: vec![],
            body: None,
        };
        assert!(WinHttpTransport
            .request(&req)
            .unwrap_err()
            .contains("HTTPS"));
    }

    #[derive(Clone)]
    struct PausedLogin {
        entered: mpsc::Sender<()>,
        release: Arc<std::sync::Barrier>,
    }
    impl HttpTransport for PausedLogin {
        fn request(&self, _: &HttpRequest) -> Result<HttpResponse, String> {
            self.entered.send(()).unwrap();
            self.release.wait();
            Ok(HttpResponse {
                status: 200,
                body: r#"{"code":200,"data":{"token":"must-not-persist","username":"test"}}"#
                    .into(),
            })
        }
    }

    #[test]
    fn logout_and_server_switch_invalidate_inflight_login_without_restoring_credentials() {
        for change_server in [false, true] {
            let dir = test_dir("superseded");
            let (entered, rx) = mpsc::channel();
            let release = Arc::new(std::sync::Barrier::new(2));
            let transport = PausedLogin {
                entered,
                release: release.clone(),
            };
            let mut worker =
                OnlineWorker::spawn_with_transport(dir.clone(), DEFAULT_SERVER, transport).unwrap();
            worker
                .submit(OnlineCommand::Login {
                    username: "test".into(),
                    password: "one-use-secret".into(),
                })
                .unwrap();
            rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(worker.pending_count(), 1);
            worker
                .submit(if change_server {
                    OnlineCommand::SetServer {
                        base_url: "https://example.com".into(),
                    }
                } else {
                    OnlineCommand::Logout
                })
                .unwrap();
            release.wait();
            let results = drain_until_idle(&worker);
            assert_eq!(worker.pending_count(), 0);
            assert!(!results
                .iter()
                .any(|r| matches!(&r.result, Ok(OnlinePayload::Account(Some(_))))));
            assert!(!dir.join("online-auth.dpapi").exists());
            assert!(results.iter().any(|r| if change_server {
                matches!(&r.result, Ok(OnlinePayload::ServerChanged(_)))
            } else {
                matches!(&r.result, Ok(OnlinePayload::LoggedOut))
            }));
        }
    }

    #[derive(Clone)]
    struct ScriptTransport(Arc<wenlai::ScriptableMockTransport>);
    impl HttpTransport for ScriptTransport {
        fn request(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
            self.0.request(request)
        }
    }

    #[test]
    fn failed_account_switch_cannot_restore_previous_saved_login() {
        let dir = test_dir("failed-login");
        let path = dir.join("online-auth.dpapi");
        save_account(
            &path,
            &WenlaiAccount {
                domain: DEFAULT_SERVER.into(),
                username: "old".into(),
                jwt_token: "old-token".into(),
                ..WenlaiAccount::default()
            },
        )
        .unwrap();
        let transport = Arc::new(wenlai::ScriptableMockTransport::new());
        transport.push(401, r#"{"msg":"账号或密码错误"}"#);
        let mut worker = OnlineWorker::spawn_with_transport(
            dir.clone(),
            DEFAULT_SERVER,
            ScriptTransport(transport),
        )
        .unwrap();
        worker
            .submit(OnlineCommand::Login {
                username: "new".into(),
                password: "bad".into(),
            })
            .unwrap();
        let results = drain_until_idle(&worker);
        assert!(!results
            .iter()
            .any(|r| matches!(&r.result, Ok(OnlinePayload::Account(Some(_))))));
        assert!(results.iter().any(|r| r.result.is_err()));
        assert!(!path.exists());
        drop(worker);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn register_flow_sends_code_then_persists_autologin() {
        let dir = test_dir("register");
        let path = dir.join("online-auth.dpapi");
        let transport = Arc::new(wenlai::ScriptableMockTransport::new());
        transport
            .push(200, r#"{"code":200,"msg":"发送成功","data":null}"#)
            .push(200, r#"{"code":200,"msg":"注册成功","data":{}}"#)
            .push(
                200,
                r#"{"code":200,"data":{"token":"reg-token","userId":9,"username":"新用户","displayName":"新用户"}}"#,
            );
        let mut worker = OnlineWorker::spawn_with_transport(
            dir.clone(),
            DEFAULT_SERVER,
            ScriptTransport(transport),
        )
        .unwrap();
        // 第一步：发码 → CodeSent
        worker
            .submit(OnlineCommand::RegisterSendCode {
                email: " leaf@example.com ".into(),
            })
            .unwrap();
        let results = drain_until_idle(&worker);
        assert!(results
            .iter()
            .any(|r| matches!(&r.result, Ok(OnlinePayload::CodeSent))));
        // 第二步：注册 → 自动登录 → Account(Some) + 凭据落盘
        worker
            .submit(OnlineCommand::Register {
                username: "新用户".into(),
                password: "one-use-secret".into(),
                email: "leaf@example.com".into(),
                code: "482913".into(),
            })
            .unwrap();
        let results = drain_until_idle(&worker);
        assert!(results.iter().any(|r| matches!(
            &r.result,
            Ok(OnlinePayload::Account(Some(a))) if a.username == "新用户" && a.user_id == 9
        )));
        assert!(path.exists());
        drop(worker);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn failed_register_keeps_server_msg_and_no_credentials() {
        let dir = test_dir("register-failed");
        let path = dir.join("online-auth.dpapi");
        let transport = Arc::new(wenlai::ScriptableMockTransport::new());
        transport.push(200, r#"{"code":400,"msg":"用户名已被注册","data":null}"#);
        let mut worker = OnlineWorker::spawn_with_transport(
            dir.clone(),
            DEFAULT_SERVER,
            ScriptTransport(transport),
        )
        .unwrap();
        worker
            .submit(OnlineCommand::Register {
                username: "新用户".into(),
                password: "bad".into(),
                email: "a@b.com".into(),
                code: "000000".into(),
            })
            .unwrap();
        let results = drain_until_idle(&worker);
        assert!(results
            .iter()
            .any(|r| matches!(&r.result, Err(e) if e.contains("用户名已被注册"))));
        assert!(!results
            .iter()
            .any(|r| matches!(&r.result, Ok(OnlinePayload::Account(Some(_))))));
        assert!(!path.exists());
        drop(worker);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn newer_fetch_hides_completed_but_unread_old_article() {
        let transport = Arc::new(wenlai::ScriptableMockTransport::new());
        transport
            .push(200, r#"{"code":200,"data":{"content":"old"}}"#)
            .push(200, r#"{"code":200,"data":{"content":"new"}}"#);
        let mut worker = OnlineWorker::spawn_with_transport(
            test_dir("fetch"),
            DEFAULT_SERVER,
            ScriptTransport(transport),
        )
        .unwrap();
        let old = worker
            .submit(OnlineCommand::Fetch(FetchParams {
                sequential: true,
                book_id: "old-source".into(),
                ..FetchParams::default()
            }))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while worker.is_busy() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let new = worker
            .submit(OnlineCommand::Fetch(FetchParams {
                category: "new-random-source".into(),
                ..FetchParams::default()
            }))
            .unwrap();
        let results = drain_until_idle(&worker);
        assert!(results.iter().all(|r| r.id != old));
        assert!(results.iter().any(|r| r.id == new
            && matches!(&r.result,Ok(OnlinePayload::Article(a, p))
                    if a.text=="new" && p.category=="new-random-source"
                        && !p.sequential && p.book_id.is_empty())));
    }

    #[test]
    fn absolute_network_deadline_closes_native_handle_once() {
        let agent = wide("HuFu-deadline-test");
        let handle = unsafe {
            InternetHandle::new(
                WinHttpOpen(
                    PCWSTR(agent.as_ptr()),
                    WINHTTP_ACCESS_TYPE_NO_PROXY,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    0,
                ),
                "test",
            )
        }
        .unwrap();
        let _deadline =
            NetworkDeadline::start(&handle, Instant::now() - Duration::from_millis(29_950))
                .unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while !handle.raw().is_null() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(handle.active().is_err());
        // Dropping both objects after timer cancellation must not double-close the native handle.
        drop(handle);
    }

    #[test]
    fn invalidated_login_cannot_commit_after_waiting_for_credential_lock() {
        let dir = test_dir("commit-race");
        let path = dir.join("online-auth.dpapi");
        let epoch = Arc::new(AtomicU64::new(1));
        let guard = Arc::new(Mutex::new(()));
        let held = guard.lock().unwrap();
        let (worker_path, worker_epoch, worker_guard) =
            (path.clone(), epoch.clone(), guard.clone());
        let writer = std::thread::spawn(move || {
            save_account_impl(
                &worker_path,
                &WenlaiAccount {
                    domain: DEFAULT_SERVER.into(),
                    jwt_token: "late-login".into(),
                    ..WenlaiAccount::default()
                },
                Some((&worker_epoch, 1, &worker_guard)),
            )
        });
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        let until = Instant::now() + Duration::from_secs(3);
        while !temp.exists() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
        // This models logout taking effect before the prepared ciphertext can commit.
        epoch.store(2, Ordering::Release);
        invalidate_saved_account(&path).unwrap();
        drop(held);
        assert!(!writer.join().unwrap().unwrap());
        assert!(!path.exists());
        assert!(!temp.exists());
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn locked_credential_file_is_blocked_from_restoration_and_new_login_clears_marker() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = test_dir("locked-logout");
        let path = dir.join("online-auth.dpapi");
        let mut account = WenlaiAccount {
            domain: DEFAULT_SERVER.into(),
            jwt_token: "old-token".into(),
            ..WenlaiAccount::default()
        };
        save_account(&path, &account).unwrap();
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert!(invalidate_saved_account(&path).unwrap().is_some());
        assert!(load_account(&path, DEFAULT_SERVER).unwrap().is_none());
        drop(locked);
        account.jwt_token = "new-token".into();
        save_account(&path, &account).unwrap();
        assert!(!signed_out_marker(&path).exists());
        assert_eq!(
            load_account(&path, DEFAULT_SERVER)
                .unwrap()
                .unwrap()
                .jwt_token,
            "new-token"
        );
        remove_account(&path).unwrap();
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn logout_removes_saved_login_before_blocked_worker_or_shutdown_can_delay_it() {
        let dir = test_dir("immediate-logout");
        let path = dir.join("online-auth.dpapi");
        save_account(
            &path,
            &WenlaiAccount {
                domain: DEFAULT_SERVER.into(),
                jwt_token: "old-token".into(),
                ..WenlaiAccount::default()
            },
        )
        .unwrap();
        let (entered, rx) = mpsc::channel();
        let release = Arc::new(std::sync::Barrier::new(2));
        let mut worker = OnlineWorker::spawn_with_transport(
            dir.clone(),
            DEFAULT_SERVER,
            PausedLogin {
                entered,
                release: release.clone(),
            },
        )
        .unwrap();
        worker
            .submit(OnlineCommand::Fetch(FetchParams::default()))
            .unwrap();
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.submit(OnlineCommand::Logout).unwrap();
        assert!(load_account(&path, DEFAULT_SERVER).unwrap().is_none());
        drop(worker);
        release.wait();
        assert!(!path.exists());
        let _ = std::fs::remove_dir(&dir);
    }
}
