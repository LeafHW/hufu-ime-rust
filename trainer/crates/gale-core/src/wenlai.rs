//! 晴发文系统（文来）客户端 —— jifeng-core wenlai.rs 全量移植（无 ureq，传输抽象为 trait）。
//!
//! 默认服务器：`https://qingfawen.fcxxz.com/`（晴跟打 Pro 同源，config.wenlai_domain）。
//! 统一响应 envelope（两种都可能出现，需兼容）：
//! - 新：`{"code": 200, "msg": "...", "data": {...}}`，code==200 为成功；
//! - 旧：`{"error": 0, "msg": {...或"..."}}`，error==0 为成功，data = msg。
//! 登录后请求带 `Authorization: Bearer <token>`；UA 用 Chrome 120。
//!
//! **离线约定**：HTTP 传输通过 [`HttpTransport`] trait 注入；内置
//! [`ScriptableMockTransport`]（可编排响应序列）供测试/开发。真实接入时在 gale-win 侧
//! 用 winhttp/reqwest 实现同一 trait 即可，本 crate 不新增依赖。
//!
//! **晴杯赛文加密（未实现，如实桩）**：晴杯 `/api/race/submit` 上报需
//! RSA-OAEP-SHA256 + AES-256-GCM 混合加密 envelope + 客户端 RSA-SHA256 签名
//! （_research/typesunny-qq.md §4.3）。vendor 无相应 crate，**不手搓加密**：
//! 载荷构造（明文 JSON，字段对晴 API）已实现为 [`build_race_payload`]，
//! 加密/签名/发送留 TODO 等在线阶段补依赖后接上。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::GaleResult;

/// 难度分级（文来实测枚举：level 1-6，与服务端 DifficultyConstant 一致）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Difficulty {
    Miao, // 淼
    Shui, // 水
    Yi,   // 易
    Pu,   // 普
    Nan,  // 难
    Nve,  // 虐
}

/// 难度标签表（1..=6 → 淼水易普难虐）
const DIFFICULTY_LABELS: [&str; 6] = ["淼", "水", "易", "普", "难", "虐"];

impl Difficulty {
    pub fn label(self) -> &'static str {
        match self {
            Difficulty::Miao => "淼",
            Difficulty::Shui => "水",
            Difficulty::Yi => "易",
            Difficulty::Pu => "普",
            Difficulty::Nan => "难",
            Difficulty::Nve => "虐",
        }
    }
    /// 服务端数字等级（1..=6）
    pub fn level(self) -> u8 {
        match self {
            Difficulty::Miao => 1,
            Difficulty::Shui => 2,
            Difficulty::Yi => 3,
            Difficulty::Pu => 4,
            Difficulty::Nan => 5,
            Difficulty::Nve => 6,
        }
    }
    pub fn from_label(s: &str) -> Option<Self> {
        Some(match s {
            "淼" => Difficulty::Miao,
            "水" => Difficulty::Shui,
            "易" => Difficulty::Yi,
            "普" => Difficulty::Pu,
            "难" => Difficulty::Nan,
            "虐" => Difficulty::Nve,
            _ => return None,
        })
    }
    pub fn from_level(level: i64) -> Option<Self> {
        let idx = usize::try_from(level).ok()?; // level < 0 → None
        if idx >= 1 && idx <= 6 {
            Some(Self::from_label(DIFFICULTY_LABELS[idx - 1])?)
        } else {
            None
        }
    }
}

/// 账号凭据（持久化部分；token 只存 Bearer 串）
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct WenlaiAccount {
    pub domain: String,
    pub username: String,
    pub display_name: String,
    pub user_id: i64,
    pub jwt_token: String,
}

/// 拉文请求参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchParams {
    pub difficulty: Difficulty,
    /// 字数（文来常用 50-500）
    pub chars: usize,
    /// 分类（chuban 等）
    pub category: String,
    /// 上一段 / 随机 / 下一段（Prev/Next 走 /api/texts/adjacent，需 book_id + sort_num）
    pub mode: FetchMode,
    /// adjacent 模式：上一段的 bookId
    #[serde(default)]
    pub book_id: String,
    /// adjacent 模式：上一段的 sortNum
    #[serde(default)]
    pub sort_num: i64,
    /// 精确字数，关闭时交由服务端按自然段选择。
    #[serde(default = "default_true")]
    pub strict_length: bool,
    #[serde(default)]
    pub end_sort_num: i64,
    #[serde(default)]
    pub end_chars: String,
    #[serde(default)]
    pub start_chars: String,
    #[serde(default)]
    pub use_shelf: bool,
    #[serde(default)]
    pub sequential: bool,
}

fn default_true() -> bool {
    true
}

impl Default for FetchParams {
    fn default() -> Self {
        Self {
            difficulty: Difficulty::Pu,
            chars: 300,
            category: String::new(),
            mode: FetchMode::Random,
            book_id: String::new(),
            sort_num: 0,
            strict_length: true,
            end_sort_num: 0,
            end_chars: String::new(),
            start_chars: String::new(),
            use_shelf: false,
            sequential: false,
        }
    }
}

impl std::fmt::Debug for WenlaiAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WenlaiAccount")
            .field("domain", &self.domain)
            .field("username", &self.username)
            .field("display_name", &self.display_name)
            .field("user_id", &self.user_id)
            .field("jwt_token", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FetchMode {
    Random,
    Prev,
    Next,
}

/// 拉到的文章（一段）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WenlaiArticle {
    pub title: String,
    pub difficulty: Difficulty,
    pub text: String,
    /// 段标记 "当前段-总段数"，如 "2-277101"
    pub seg_mark: String,
    /// 难度分（如 1.84；无分数时 0）
    #[serde(default)]
    pub difficulty_score: f64,
    /// 书 ID（字符串化；adjacent 翻段依赖）
    #[serde(default)]
    pub book_id: String,
    #[serde(default)]
    pub sort_num: i64,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub end_sort_num: i64,
    #[serde(default)]
    pub end_chars: String,
    #[serde(default)]
    pub start_chars: String,
}

impl WenlaiArticle {
    /// 源站署名始终保留，缺少段号时不伪造。
    pub fn publish_text(&self) -> String {
        let tail = if self.seg_mark.is_empty() {
            "-----晴发文".into()
        } else {
            format!("-----第{}段-晴发文", self.seg_mark)
        };
        format!(
            "[{}]{} [字数{}]\n{}\n{}",
            self.difficulty.label(),
            self.title.replace(['\r', '\n'], " "),
            self.text.chars().count(),
            self.text,
            tail
        )
    }
    pub fn adjacent_params(&self, next: bool, chars: usize, strict_length: bool) -> FetchParams {
        FetchParams {
            difficulty: self.difficulty,
            chars,
            category: self.category.clone(),
            mode: if next {
                FetchMode::Next
            } else {
                FetchMode::Prev
            },
            book_id: self.book_id.clone(),
            sort_num: self.sort_num,
            strict_length,
            end_sort_num: self.end_sort_num,
            end_chars: self.end_chars.clone(),
            start_chars: self.start_chars.clone(),
            use_shelf: false,
            sequential: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShelfBook {
    pub book_id: String,
    pub title: String,
    pub sort_num: i64,
    pub mode: String,
}

/// 成绩上报载荷 —— 字段名对齐晴发文 leaderboard（复用 score::LeaderboardScore，上报免转换）。
/// `score_text` 为发群用成绩单文本（format_score_report 输出）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WenlaiScorePost {
    pub leaderboard: crate::score::LeaderboardScore,
    pub score_text: String,
}

// ─────────────────────────────────────────────────────────────
// HTTP 传输抽象（std 层面，无 crates.io 依赖）
// ─────────────────────────────────────────────────────────────

/// 一次 HTTP 请求的完整描述（阻塞式；UI 侧放后台线程调用）
#[derive(Clone)]
pub struct HttpRequest {
    pub method: HttpMethod,
    /// 完整 URL（含 query）
    pub url: String,
    /// 头部（Authorization/Content-Type/User-Agent 等）
    pub headers: Vec<(String, String)>,
    /// JSON body（POST 时）
    pub body: Option<String>,
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &"[redacted]")
            .field("body", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

/// HTTP 响应（2xx 才算传输成功；业务码在 envelope 层判）
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// HTTP 传输 trait —— 真实接入时在 gale-win 侧实现（winhttp / reqwest），
/// 测试用 [`ScriptableMockTransport`]。
pub trait HttpTransport {
    fn request(&self, req: &HttpRequest) -> Result<HttpResponse, String>;
}

/// 可编排的 mock 传输：按序弹出预置响应；请求全量记录供断言。
pub struct ScriptableMockTransport {
    /// 预置响应队列（(status, body)）
    queue: std::sync::Mutex<Vec<(u16, String)>>,
    /// 已收到的请求
    requests: std::sync::Mutex<Vec<HttpRequest>>,
}

impl ScriptableMockTransport {
    pub fn new() -> Self {
        Self {
            queue: std::sync::Mutex::new(Vec::new()),
            requests: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// 追加一条预置响应
    pub fn push(&self, status: u16, body: &str) -> &Self {
        self.queue.lock().unwrap().push((status, body.to_string()));
        self
    }

    /// 已收到的请求快照
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Default for ScriptableMockTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpTransport for ScriptableMockTransport {
    fn request(&self, req: &HttpRequest) -> Result<HttpResponse, String> {
        self.requests.lock().unwrap().push(req.clone());
        let mut q = self.queue.lock().unwrap();
        if q.is_empty() {
            Err("mock: 预置响应队列已空".to_string())
        } else {
            let (status, body) = q.remove(0);
            Ok(HttpResponse { status, body })
        }
    }
}

// ─────────────────────────────────────────────────────────────
// 客户端
// ─────────────────────────────────────────────────────────────

/// 晴发文系统客户端接口（接口签名冻结：login/fetch/report_score/account/categories）。
/// 实现：[`WenlaiHttpClient`]（trait 传输注入）、[`MockWenlaiClient`]（固定演示文）。
pub trait WenlaiClient {
    fn login(&mut self, username: &str, password: &str) -> GaleResult<WenlaiAccount>;
    fn fetch(&mut self, params: &FetchParams) -> GaleResult<WenlaiArticle>;
    fn report_score(&mut self, post: &WenlaiScorePost) -> GaleResult<()>;
    fn account(&self) -> Option<&WenlaiAccount>;
    /// GET /api/categories → [(code, name)]；过滤 isActive==false
    fn categories(&mut self) -> GaleResult<Vec<(String, String)>>;
    /// POST /api/dailyWords/report，body `{"count":n,"date":"yyyy-MM-dd"}`
    fn report_daily_words(&mut self, count: usize, date: &str) -> GaleResult<()>;
}

/// 真实协议逻辑的客户端实现：envelope 解析 / 端点构造 / 字段兼容全在这里，
/// 传输由 `T: HttpTransport` 注入（离线测试给 mock，在线给真实传输）。
pub struct WenlaiHttpClient<T: HttpTransport> {
    transport: T,
    base_url: String,
    token: Option<String>,
    account: Option<WenlaiAccount>,
}

/// 默认 UA：Chrome 120（晴跟打 Pro 同款，避免被服务端按非常规客户端拦截）。
/// 全端点统一使用；gale-app 的 WinHttpOpen 代理名也取这一份（online.rs）。
pub const DEFAULT_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

impl<T: HttpTransport> WenlaiHttpClient<T> {
    /// base_url 去掉尾部 `/`（对齐 jifeng；另容忍旧配置以 `/api/get_text` 结尾——裁掉）。
    pub fn new(base_url: &str, transport: T) -> Self {
        let mut base_url = base_url.trim_end_matches('/').to_string();
        if let Some(stripped) = base_url.strip_suffix("/api/get_text") {
            base_url = stripped.to_string();
        }
        Self {
            transport,
            base_url,
            token: None,
            account: None,
        }
    }

    /// 登录态 token（login 后自动持有；也可外部注入恢复会话）
    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }

    pub fn logout(&mut self) {
        self.token = None;
        self.account = None;
    }

    pub fn restore_account(&mut self, account: WenlaiAccount) -> Result<(), String> {
        if normalize_base_url(&account.domain)? != normalize_base_url(&self.base_url)? {
            return Err("账号属于其他服务器，请重新登录".into());
        }
        if account.jwt_token.is_empty() || account.jwt_token.contains(['\r', '\n', '\0']) {
            return Err("保存的登录凭据无效，请重新登录".into());
        }
        self.token = Some(account.jwt_token.clone());
        self.account = Some(account);
        Ok(())
    }

    pub fn current_user(&self) -> Result<(), String> {
        let body = self.get_idempotent("/api/auth/current")?;
        parse_envelope(&body)?;
        Ok(())
    }

    pub fn bookshelf(&self) -> Result<Vec<ShelfBook>, String> {
        let body = self.get_idempotent("/api/userBookConfig/list")?;
        let data = parse_envelope(&body)?;
        let items = data
            .as_array()
            .or_else(|| data.get("books").and_then(Value::as_array))
            .or_else(|| data.get("records").and_then(Value::as_array))
            .or_else(|| data.get("list").and_then(Value::as_array))
            .ok_or("书架响应格式暂不支持，请联系服务端核对接口")?;
        items
            .iter()
            .map(|v| {
                let id = int_field(v, "bookId")
                    .map(|x| x.to_string())
                    .or_else(|| str_field(v, "bookId"))
                    .ok_or("书架响应缺少bookId，请核对接口")?;
                let title = str_field(v, "bookName")
                    .or_else(|| str_field(v, "name"))
                    .ok_or("书架响应缺少书名，请核对接口")?;
                Ok(ShelfBook {
                    book_id: id,
                    title,
                    sort_num: int_field(v, "sortNum").unwrap_or(0),
                    mode: str_field(v, "mode")
                        .or_else(|| str_field(&data, "mode"))
                        .unwrap_or_default(),
                })
            })
            .collect()
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn get(&self, path_and_query: &str) -> Result<String, String> {
        normalize_base_url(&self.base_url)?;
        let req = HttpRequest {
            method: HttpMethod::Get,
            url: format!("{}{}", self.base_url, path_and_query),
            headers: self.auth_headers(),
            body: None,
        };
        self.read_response(self.transport.request(&req))
    }

    /// 幂等 GET（categories / bookshelf / auth-current 等只读端点）：传输层失败
    /// （连接建立/发送失败，即 transport 返回 `Err`）时**重试一次**；HTTP 状态
    /// 错误以 `Ok(HttpResponse)` 进 read_response，不算连接失败、不重试。
    /// fetch_random / fetch_adjacent 服务端可能推进阅读游标，保持单次 `get`
    /// （见各端点旁的豁免注释）。
    fn get_idempotent(&self, path_and_query: &str) -> Result<String, String> {
        normalize_base_url(&self.base_url)?;
        let make_req = || HttpRequest {
            method: HttpMethod::Get,
            url: format!("{}{}", self.base_url, path_and_query),
            headers: self.auth_headers(),
            body: None,
        };
        let result = match self.transport.request(&make_req()) {
            Err(_) => self.transport.request(&make_req()), // 连接类失败重试一次
            ok => ok,
        };
        self.read_response(result)
    }

    fn post_json(&self, path: &str, body: Value) -> Result<String, String> {
        normalize_base_url(&self.base_url)?;
        let req = HttpRequest {
            method: HttpMethod::Post,
            url: format!("{}{}", self.base_url, path),
            headers: self.auth_headers(),
            body: Some(body.to_string()),
        };
        self.read_response(self.transport.request(&req))
    }

    fn auth_headers(&self) -> Vec<(String, String)> {
        let mut h = vec![
            ("User-Agent".to_string(), DEFAULT_UA.to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        if let Some(t) = &self.token {
            h.push(("Authorization".to_string(), format!("Bearer {t}")));
        }
        h
    }

    /// 统一读取响应：2xx 返回文本；HTTP 错误时尽量挖出服务端 msg 一起报错。
    fn read_response(&self, result: Result<HttpResponse, String>) -> Result<String, String> {
        match result {
            Ok(resp) => {
                if (200..300).contains(&resp.status) {
                    Ok(resp.body)
                } else {
                    let msg = serde_json::from_str::<Value>(&resp.body)
                        .ok()
                        .and_then(|v| str_field(&v, "msg").or_else(|| str_field(&v, "message")))
                        .unwrap_or_default();
                    if msg.is_empty() {
                        Err(format!("请求失败 (HTTP {})", resp.status))
                    } else {
                        Err(format!("{} (HTTP {})", msg, resp.status))
                    }
                }
            }
            Err(e) => Err(format!("网络请求失败: {e}")),
        }
    }

    /// GET请求只生成一次，不将随机拉文当幂等重试（服务端可能推进阅读游标）。
    pub fn fetch_random(&self, params: &FetchParams) -> Result<WenlaiArticle, String> {
        validate_fetch(params)?;
        // Current public frontend uses difficulty/useShelf/bookId/sequential. Keep
        // difficultyLevel as a compatible alias for older deployments.
        let mut path = format!("/api/texts/random?difficultyLevel={}&difficulty={}&length={}&strictLength={}&category={}&useShelf={}&sequential={}",
            params.difficulty.level(),params.difficulty.level(),params.chars,params.strict_length,
            encode_query(&params.category),params.use_shelf,params.sequential);
        if !params.book_id.is_empty() {
            path.push_str(&format!("&bookId={}", encode_query(&params.book_id)));
        }
        let body = self.get(&path)?;
        parse_article_data(&parse_envelope(&body)?)
    }

    pub fn fetch_adjacent(
        &self,
        params: &FetchParams,
        next: bool,
    ) -> Result<WenlaiArticle, String> {
        validate_fetch(params)?;
        if params.book_id.is_empty() {
            return Err("没有当前文章游标，请先随机取文".into());
        }
        let mut path = format!("/api/texts/adjacent?bookId={}&sortNum={}&direction={}&category={}&length={}&strictLength={}",
            encode_query(&params.book_id), params.sort_num, if next { "next" } else { "prev" },
            encode_query(&params.category), params.chars, params.strict_length);
        if params.end_sort_num > 0 {
            path.push_str(&format!("&endSortNum={}", params.end_sort_num));
        }
        if !params.end_chars.is_empty() {
            path.push_str(&format!("&endChars={}", encode_query(&params.end_chars)));
        }
        if !params.start_chars.is_empty() {
            path.push_str(&format!(
                "&startChars={}",
                encode_query(&params.start_chars)
            ));
        }
        let body = self.get(&path)?;
        parse_article_data(&parse_envelope(&body)?)
    }
}

impl<T: HttpTransport> WenlaiClient for WenlaiHttpClient<T> {
    /// POST /api/auth/login，body `{"username","password"}`。
    fn login(&mut self, username: &str, password: &str) -> GaleResult<WenlaiAccount> {
        self.logout();
        if username.trim().is_empty() || password.is_empty() {
            return Err(crate::GaleError::Msg("请输入用户名和密码".into()));
        }
        let body = self
            .post_json(
                "/api/auth/login",
                json!({ "username": username, "password": password }),
            )
            .map_err(|e| crate::GaleError::Msg(safe_login_error(&e)))?;
        let data =
            parse_envelope(&body).map_err(|e| crate::GaleError::Msg(safe_login_error(&e)))?;
        let token = str_field(&data, "token").unwrap_or_default();
        if token.is_empty() || token.contains(['\r', '\n', '\0']) {
            return Err(crate::GaleError::Msg(
                "登录响应没有有效凭据，未保存登录状态".into(),
            ));
        }
        let account = WenlaiAccount {
            domain: self.base_url.clone(),
            username: str_field(&data, "username").unwrap_or_else(|| username.to_string()),
            display_name: str_field(&data, "displayName").unwrap_or_else(|| username.to_string()),
            user_id: int_field(&data, "userId").unwrap_or(0),
            jwt_token: token.clone(),
        };
        self.token = if token.is_empty() { None } else { Some(token) };
        self.account = Some(account.clone());
        Ok(account)
    }

    fn fetch(&mut self, params: &FetchParams) -> GaleResult<WenlaiArticle> {
        let a = match params.mode {
            FetchMode::Random => self.fetch_random(params),
            FetchMode::Next => self.fetch_adjacent(params, true),
            FetchMode::Prev => self.fetch_adjacent(params, false),
        }
        .map_err(|e| {
            if auth_expired(&e) {
                self.logout();
            }
            crate::GaleError::Msg(e)
        })?;
        Ok(a)
    }

    /// 晴杯上报（如实桩）：载荷构造见 [`build_race_payload`]；
    /// 加密 envelope（RSA-OAEP+AES-GCM）+ 签名 + 发送 **TODO 在线阶段补依赖后实现**。
    fn report_score(&mut self, _post: &WenlaiScorePost) -> GaleResult<()> {
        Err(crate::GaleError::Msg(
            "晴杯成绩上报需 RSA-OAEP+AES-GCM envelope（vendor 无密码学 crate）；载荷构造见 build_race_payload，接入待在线阶段".into(),
        ))
    }

    fn account(&self) -> Option<&WenlaiAccount> {
        self.account.as_ref()
    }

    fn categories(&mut self) -> GaleResult<Vec<(String, String)>> {
        let body = self
            .get_idempotent("/api/categories")
            .map_err(crate::GaleError::Msg)?;
        let data = parse_envelope(&body).map_err(crate::GaleError::Msg)?;
        let arr = match data.as_array() {
            Some(a) => a,
            None => {
                return Err(crate::GaleError::Msg(
                    "分类数据格式不正确（应为数组）".into(),
                ));
            }
        };
        let mut out = Vec::new();
        for item in arr {
            if item
                .get("isActive")
                .map(|b| b == &json!(false))
                .unwrap_or(false)
            {
                continue;
            }
            let code = ["code", "category", "value", "slug"]
                .iter()
                .find_map(|k| str_field(item, k))
                .unwrap_or_default();
            if code.trim().is_empty() {
                continue;
            }
            let name = ["name", "label", "title"]
                .iter()
                .find_map(|k| str_field(item, k))
                .unwrap_or_else(|| code.clone());
            out.push((code, name));
        }
        Ok(out)
    }

    fn report_daily_words(&mut self, count: usize, date: &str) -> GaleResult<()> {
        let body = self
            .post_json(
                "/api/dailyWords/report",
                json!({ "count": count, "date": date }),
            )
            .map_err(crate::GaleError::Msg)?;
        parse_envelope(&body)
            .map_err(|_| crate::GaleError::Msg("每日字数上报失败".into()))
            .map(|_| ())
    }
}

impl<T: HttpTransport> WenlaiClient for &mut WenlaiHttpClient<T> {
    fn login(&mut self, username: &str, password: &str) -> GaleResult<WenlaiAccount> {
        (**self).login(username, password)
    }
    fn fetch(&mut self, params: &FetchParams) -> GaleResult<WenlaiArticle> {
        (**self).fetch(params)
    }
    fn report_score(&mut self, post: &WenlaiScorePost) -> GaleResult<()> {
        (**self).report_score(post)
    }
    fn account(&self) -> Option<&WenlaiAccount> {
        (**self).account()
    }
    fn categories(&mut self) -> GaleResult<Vec<(String, String)>> {
        (**self).categories()
    }
    fn report_daily_words(&mut self, count: usize, date: &str) -> GaleResult<()> {
        (**self).report_daily_words(count, date)
    }
}

/// 构造晴杯 `/api/race/submit` 明文载荷（字段名对齐晴 API：raceId/speed/timeCost/
/// charCount/keystroke/codeLength/backspaceCount/keyCount/keyAccuracy/wordRate/inputMethod/…）。
/// 加密/签名不在此层（TODO 在线阶段）。
pub fn build_race_payload(
    lb: &crate::score::LeaderboardScore,
    race_id: &str,
    article_id: &str,
) -> Value {
    json!({
        "raceId": race_id,
        "articleId": article_id,
        // 速度：晴杯口径为有效速度（错一罚五）
        "speed": format!("{:.2}", lb.speed),
        "timeCost": lb.time_ms / 1000,
        "charCount": lb.word_count,
        "keystroke": format!("{:.2}", lb.keystroke),
        "codeLength": format!("{:.3}", lb.code_length),
        "backspaceCount": lb.backspaces,
        "keyCount": lb.total_keys,
        "keyAccuracy": format!("{:.2}", lb.key_accuracy),
        "wrongNum": lb.wrong,
        "inputMethod": lb.input_method,
        "mark": lb.mark,
        // wordRate（打词率）与 sessionNonce/clientTs 由调用层补充/在线阶段补
    })
}

// ─────────────────────────────────────────────────────────────
// envelope / 数据解析（jifeng wenlai.rs 移植）
// ─────────────────────────────────────────────────────────────

fn safe_login_error(message: &str) -> String {
    // Never display server text on the credential path. Passwords may be echoed
    // as JSON escapes, Unicode escapes, fragments, or encodings we cannot redact.
    // Keep only the numeric suffix added by our response parser.
    let generic = "登录失败，请检查账号、密码和服务地址";
    for kind in ["HTTP", "code", "error"] {
        let marker = format!(" ({kind} ");
        if let Some((_, suffix)) = message.rsplit_once(&marker) {
            if let Some(number) = suffix.strip_suffix(')') {
                if let Ok(number) = number.parse::<i64>() {
                    return format!("{generic} ({kind} {number})");
                }
            }
        }
    }
    if let Some(detail) = message.strip_prefix("网络请求失败: ") {
        for stage in [
            "初始化网络",
            "连接服务器",
            "创建请求",
            "设置超时",
            "禁止凭据重定向",
            "设置传输压缩",
            "发送请求",
            "接收响应",
            "读取响应状态",
            "读取响应",
        ] {
            if detail.starts_with(&format!("{stage}失败（")) {
                return format!("登录时{stage}失败，请检查网络和服务地址后重试");
            }
        }
        if detail.starts_with("网络请求超过30秒") {
            return "登录请求超过30秒，请检查网络后重试".into();
        }
        return "登录网络请求失败，请检查网络和服务地址后重试".into();
    }
    generic.into()
}

/// 安全取字符串字段（null / 缺失 → None）。
fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .filter(|x| !x.is_null())
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

/// 安全取整数字段（数字或数字字符串）。
fn int_field(v: &Value, key: &str) -> Option<i64> {
    v.get(key).filter(|x| !x.is_null()).and_then(|x| {
        x.as_i64()
            .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
    })
}

/// 解析统一响应 envelope：
/// - `{"code":200,"data":...}` → Ok(data)；
/// - `{"error":0,"msg":{...}}` → Ok(msg)（msg 为对象时）；
/// - 其余 → Err(描述性错误信息，含服务器 msg)。
pub fn parse_envelope(body: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(body.trim())
        .map_err(|_| "服务器返回了非JSON内容，请检查服务器地址或稍后重试".to_string())?;

    // 新格式：{"code":200,"msg":"...","data":...}
    if let Some(code) = v.get("code").and_then(|c| c.as_i64()) {
        if code == 200 {
            return Ok(v.get("data").cloned().unwrap_or(Value::Null));
        }
        let msg = str_field(&v, "msg").unwrap_or_default();
        if msg.is_empty() {
            return Err(format!("服务器返回错误 (code {code})"));
        }
        return Err(format!("{msg} (code {code})"));
    }

    // 旧格式：{"error":0,"msg":{...}}；error==0 为成功，data = msg
    if let Some(err) = v.get("error").and_then(|e| e.as_i64()) {
        if err == 0 {
            return Ok(v.get("msg").cloned().unwrap_or(Value::Null));
        }
        let msg = str_field(&v, "msg").unwrap_or_default();
        if msg.is_empty() {
            return Err(format!("服务器返回错误 (error {err})"));
        }
        return Err(format!("{msg} (error {err})"));
    }
    if v.get("success").and_then(Value::as_bool) == Some(true) {
        return Ok(v.get("data").cloned().unwrap_or(Value::Null));
    }

    // Do not echo arbitrary response bodies: they can contain tokens or account details.
    Err("无法识别的响应格式，请核对服务端接口版本".into())
}

/// 解析文章数据（新旧字段命名都兼容），data 不是对象或正文为空时报错。
pub fn parse_article_data(data: &Value) -> Result<WenlaiArticle, String> {
    if !data.is_object() {
        return Err("API返回的数据字段不是对象".to_string());
    }

    // 标题：新 bookName / 旧 name；'#' 后是段描述，截断
    let mut title = str_field(data, "bookName")
        .or_else(|| str_field(data, "name"))
        .unwrap_or_else(|| "未知标题".to_string());
    if let Some(pos) = title.find('#') {
        title.truncate(pos);
    }
    if title.trim().is_empty() {
        title = "未知标题".to_string();
    }

    let text = str_field(data, "content").unwrap_or_default();
    if text.trim().is_empty() {
        return Err("服务器返回的文章内容为空".to_string());
    }

    let seg_mark = str_field(data, "mark").unwrap_or_default();
    let category = str_field(data, "category").unwrap_or_default();
    let book_id = int_field(data, "bookId")
        .or_else(|| int_field(data, "book_id"))
        .map(|v| v.to_string())
        .unwrap_or_else(|| str_field(data, "bookId").unwrap_or_default());
    let sort_num = int_field(data, "sortNum")
        .or_else(|| int_field(data, "sort_num"))
        .unwrap_or(0);

    let (label, score) = difficulty_of_data(data);
    let difficulty = Difficulty::from_label(&label).unwrap_or(Difficulty::Pu);

    Ok(WenlaiArticle {
        title,
        difficulty,
        text,
        seg_mark,
        difficulty_score: score,
        book_id,
        sort_num,
        category,
        end_sort_num: int_field(data, "endSortNum").unwrap_or(0),
        end_chars: str_field(data, "endChars").unwrap_or_default(),
        start_chars: str_field(data, "startChars").unwrap_or_default(),
    })
}

/// 从文章数据提取 (难度标签, 难度分)：4 级字段兼容（对齐 jifeng）。
fn difficulty_of_data(data: &Value) -> (String, f64) {
    // 1) 显式 difficultyLabel
    if let Some(label) = str_field(data, "difficultyLabel") {
        if !label.trim().is_empty() {
            let score = data
                .get("difficultyScore")
                .and_then(|s| s.as_f64())
                .unwrap_or(0.0);
            return (label, score);
        }
    }
    // 2) 数字等级 difficultyLevel（1..6）
    if let Some(level) = int_field(data, "difficultyLevel") {
        if (1..=6).contains(&level) {
            let score = data
                .get("difficultyScore")
                .and_then(|s| s.as_f64())
                .unwrap_or(0.0);
            return (DIFFICULTY_LABELS[(level - 1) as usize].to_string(), score);
        }
    }
    // 3) 旧格式：difficulty 文本（如 "普(1.84)"，可解析出分数）
    if let Some(text) = str_field(data, "difficulty") {
        let text = text.trim();
        if !text.is_empty() {
            return parse_difficulty_text(text);
        }
    }
    // 4) 旧格式：custom_difficulty 数字等级（无分数）
    if let Some(level) = int_field(data, "custom_difficulty") {
        if (1..=6).contains(&level) {
            return (DIFFICULTY_LABELS[(level - 1) as usize].to_string(), 0.0);
        }
    }
    (String::new(), 0.0)
}

/// 解析 "普(1.84)" / "普" 形式的难度文本。
fn parse_difficulty_text(text: &str) -> (String, f64) {
    let open = match text.find('(') {
        Some(p) => p,
        None => return (text.to_string(), 0.0),
    };
    let close = text[open..].find(')').map(|p| p + open);
    let label = text[..open].trim().to_string();
    let score = close
        .and_then(|c| text[open + 1..c].trim().parse::<f64>().ok())
        .unwrap_or(0.0);
    (
        if label.is_empty() {
            text.to_string()
        } else {
            label
        },
        score,
    )
}

// ─────────────────────────────────────────────────────────────
// 固定 mock（开发演示：不联网、不依赖脚本）
// ─────────────────────────────────────────────────────────────

/// 本地 mock：不联网，返回固定文章（开发/测试用）
pub struct MockWenlaiClient;

impl WenlaiClient for MockWenlaiClient {
    fn login(&mut self, username: &str, _password: &str) -> GaleResult<WenlaiAccount> {
        Ok(WenlaiAccount {
            domain: "mock://localhost".into(),
            username: username.into(),
            display_name: username.into(),
            user_id: 0,
            jwt_token: "mock-token".into(),
        })
    }

    fn fetch(&mut self, params: &FetchParams) -> GaleResult<WenlaiArticle> {
        Ok(WenlaiArticle {
            title: format!("文来mock·{}", params.difficulty.label()),
            difficulty: params.difficulty,
            text: "晴发文系统接口预留：当前为本地演示文段。接入时仅需实现 HttpTransport。".into(),
            seg_mark: "mock-1".into(),
            difficulty_score: 1.0,
            book_id: "0".into(),
            sort_num: 0,
            category: params.category.clone(),
            end_sort_num: 0,
            end_chars: String::new(),
            start_chars: String::new(),
        })
    }

    fn report_score(&mut self, _post: &WenlaiScorePost) -> GaleResult<()> {
        Ok(())
    }

    fn account(&self) -> Option<&WenlaiAccount> {
        None
    }

    fn categories(&mut self) -> GaleResult<Vec<(String, String)>> {
        Ok(vec![("chuban".into(), "出版".into())])
    }

    fn report_daily_words(&mut self, _count: usize, _date: &str) -> GaleResult<()> {
        Ok(())
    }
}

/// HTTPS origin only: bearer tokens never cross domains or redirect to another origin.
/// 整改2c：文来注册/账号入口（跟打器"注册账号"按钮打开）。
/// 实查 2026-11-08：`qingfawen.fcxxz.com/register`、`/login` 均 HTTP 404
/// （晴发文为站长在「用户管理」添加账号的模式，无公开注册页）；
/// `dazi.wenlai.cn/register` 返回 200 但全站 JS 跳转、无表单实证，
/// 且是另一套服务（账号与本站不互通）。按兜底口径打开站点首页，
/// 用户在站内自行走账号入口；确认到真实注册页后替换此常量即可。
pub const REGISTER_URL: &str = "https://qingfawen.fcxxz.com/";

/// 注册/账号入口：配置了合法自定义服务域则开其根，否则默认站。
pub fn register_url_for(domain: &str) -> String {
    normalize_base_url(domain).unwrap_or_else(|_| REGISTER_URL.to_string())
}

pub fn normalize_base_url(value: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches('/');
    let authority = value
        .strip_prefix("https://")
        .ok_or("服务器地址必须使用 https://")?;
    if authority.is_empty()
        || authority.len() > 260
        || authority.contains(['/', '?', '#', '@', '\\'])
        || authority
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || !c.is_ascii())
    {
        return Err("服务器地址只能包含HTTPS域名和可选端口，不能含路径、账号或查询参数".into());
    }
    let (host, port) = if authority.starts_with('[') {
        let end = authority.find(']').ok_or("IPv6服务器地址无效")?;
        authority[1..end]
            .parse::<std::net::Ipv6Addr>()
            .map_err(|_| "IPv6服务器地址无效")?;
        let rest = &authority[end + 1..];
        if !rest.is_empty() && !rest.starts_with(':') {
            return Err("服务器端口无效".into());
        }
        (&authority[..end + 1], rest.strip_prefix(':'))
    } else {
        let (host, port) = authority
            .split_once(':')
            .map(|(a, b)| (a, Some(b)))
            .unwrap_or((authority, None));
        if host.is_empty()
            || !host
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
            || host.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
            })
        {
            return Err("服务器域名无效".into());
        }
        (host, port)
    };
    let port = port
        .map(|s| {
            s.parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or("服务器端口无效")
        })
        .transpose()?;
    Ok(format!(
        "https://{}{}",
        host.to_ascii_lowercase(),
        port.filter(|p| *p != 443)
            .map(|p| format!(":{p}"))
            .unwrap_or_default()
    ))
}

pub fn auth_expired(message: &str) -> bool {
    message.contains("(HTTP 401)")
        || message.contains("(code 401)")
        || message.contains("(error 401)")
}

pub fn encode_query(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn validate_fetch(params: &FetchParams) -> Result<(), String> {
    if !(1..=100_000).contains(&params.chars) {
        return Err("取文字数须为1至100000".into());
    }
    if params.category.len() > 512
        || params.book_id.len() > 256
        || params.end_chars.len() > 2048
        || params.start_chars.len() > 2048
    {
        return Err("文章筛选或游标过长".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn difficulty_roundtrip() {
        for d in [
            Difficulty::Miao,
            Difficulty::Shui,
            Difficulty::Yi,
            Difficulty::Pu,
            Difficulty::Nan,
            Difficulty::Nve,
        ] {
            assert_eq!(Difficulty::from_label(d.label()), Some(d));
            assert_eq!(Difficulty::from_level(d.level() as i64), Some(d));
        }
        assert_eq!(Difficulty::from_label("?"), None);
        assert_eq!(Difficulty::from_level(0), None);
        assert_eq!(Difficulty::from_level(7), None);
        assert_eq!(Difficulty::from_level(-1), None);
    }

    #[test]
    fn mock_client_works() {
        let mut c = MockWenlaiClient;
        c.login("山君", "x").unwrap();
        let a = c
            .fetch(&FetchParams {
                difficulty: Difficulty::Pu,
                chars: 100,
                category: "chuban".into(),
                mode: FetchMode::Random,
                book_id: String::new(),
                sort_num: 0,
                ..FetchParams::default()
            })
            .unwrap();
        assert!(a.title.contains("普"));
        assert!(!c.categories().unwrap().is_empty());
        c.report_daily_words(100, "2026-09-28").unwrap();
        let post = WenlaiScorePost {
            leaderboard: crate::score::LeaderboardScore {
                speed: 191.47,
                raw_speed: 201.77,
                keystroke: 3.57,
                code_length: 1.061,
                key_accuracy: 94.71,
                wrong: 2,
                word_count: 196,
                time_ms: 58_285,
                total_keys: 208,
                backspaces: 6,
                corrections: 3,
                input_method: "五笔".into(),
                mark: "2-277101".into(),
            },
            score_text: String::new(),
        };
        c.report_score(&post).unwrap();
    }

    // ── envelope 解析（jifeng tests 对齐）──

    #[test]
    fn parse_envelope_new_format() {
        let v = parse_envelope(r#"{"code":200,"msg":"ok","data":{"a":1}}"#).unwrap();
        assert_eq!(v["a"], 1);
        // data 缺省 → Null
        let v = parse_envelope(r#"{"code":200}"#).unwrap();
        assert!(v.is_null());
        // 业务错误
        let err = parse_envelope(r#"{"code":401,"msg":"请先登录"}"#).unwrap_err();
        assert!(err.contains("请先登录"), "{err}");
        assert!(err.contains("401"));
    }

    #[test]
    fn parse_envelope_legacy_format() {
        let v = parse_envelope(r#"{"error":0,"msg":{"content":"正文","mark":"1-100"}}"#).unwrap();
        assert_eq!(v["content"], "正文");
        let v = parse_envelope(r#"{"error":0,"msg":"纯文本数据"}"#).unwrap();
        assert_eq!(v, "纯文本数据");
        let err = parse_envelope(r#"{"error":3,"msg":"分类不存在"}"#).unwrap_err();
        assert!(err.contains("分类不存在"), "{err}");
        assert!(err.contains("error 3"));
    }

    #[test]
    fn parse_envelope_bad_input() {
        let err = parse_envelope("<html>Bad Gateway</html>").unwrap_err();
        assert!(err.contains("非JSON内容"), "{err}");
        assert!(!err.contains("<html>Bad Gateway"));
        let err = parse_envelope(r#"{"foo":1}"#).unwrap_err();
        assert!(err.contains("无法识别"), "{err}");
        assert!(parse_envelope("").is_err());
    }

    // ── 文章数据解析（jifeng tests 对齐）──

    #[test]
    fn parse_article_new_format() {
        let data: Value = serde_json::from_str(
            r#"{
                "bookName": "三国演义#第3回",
                "content": "话说天下大势，分久必合。",
                "mark": "2-277101",
                "difficultyLevel": 4,
                "difficultyLabel": "普",
                "difficultyScore": 1.84,
                "bookId": 8899,
                "sortNum": 2,
                "category": "chuban"
            }"#,
        )
        .unwrap();
        let a = parse_article_data(&data).unwrap();
        assert_eq!(a.title, "三国演义"); // '#' 后截断
        assert_eq!(a.text, "话说天下大势，分久必合。");
        assert_eq!(a.seg_mark, "2-277101");
        assert_eq!(a.difficulty, Difficulty::Pu);
        assert!((a.difficulty_score - 1.84).abs() < 1e-9);
        assert_eq!(a.book_id, "8899");
        assert_eq!(a.sort_num, 2);
        assert_eq!(a.category, "chuban");
    }

    #[test]
    fn parse_article_legacy_format() {
        let data: Value = serde_json::from_str(
            r#"{
                "name": "旧格式书名",
                "content": "旧格式正文。",
                "difficulty": "普(1.84)",
                "custom_difficulty": 4,
                "book_id": 42,
                "sort_num": 7
            }"#,
        )
        .unwrap();
        let a = parse_article_data(&data).unwrap();
        assert_eq!(a.title, "旧格式书名");
        assert_eq!(a.difficulty, Difficulty::Pu);
        assert!((a.difficulty_score - 1.84).abs() < 1e-9);
        assert_eq!(a.book_id, "42");
        assert_eq!(a.sort_num, 7);
        assert_eq!(a.category, "");
    }

    #[test]
    fn parse_article_level_only() {
        for (level, expect) in [
            (1, Difficulty::Miao),
            (2, Difficulty::Shui),
            (3, Difficulty::Yi),
            (4, Difficulty::Pu),
            (5, Difficulty::Nan),
            (6, Difficulty::Nve),
        ] {
            let data: Value = serde_json::from_str(&format!(
                r#"{{ "bookName": "t", "content": "c", "difficultyLevel": {level} }}"#
            ))
            .unwrap();
            let a = parse_article_data(&data).unwrap();
            assert_eq!(a.difficulty, expect);
        }
    }

    #[test]
    fn parse_article_errors() {
        assert!(parse_article_data(&json!("字符串")).is_err());
        let data = json!({ "bookName": "t", "content": "  " });
        let err = parse_article_data(&data).unwrap_err();
        assert!(err.contains("内容为空"), "{err}");
    }

    #[test]
    fn difficulty_text_parse() {
        let (l, s) = parse_difficulty_text("普(1.84)");
        assert_eq!(l, "普");
        assert!((s - 1.84).abs() < 1e-9);
        let (l, s) = parse_difficulty_text("难");
        assert_eq!(l, "难");
        assert_eq!(s, 0.0);
    }

    // ── 真实协议客户端（mock 传输注入，jifeng 流程对齐）──

    const LOGIN_RESP: &str = r#"{"code":200,"msg":"ok","data":{"token":"jwt-abc","userId":26,"username":"山君","displayName":"山君"}}"#;
    const RANDOM_RESP: &str = r#"{"code":200,"data":{"bookName":"三国演义","content":"正文。","mark":"2-277101","difficultyLevel":4,"difficultyScore":1.84,"bookId":8899,"sortNum":2,"category":"chuban"}}"#;

    #[test]
    fn client_login_sets_bearer_and_account() {
        let t = ScriptableMockTransport::new();
        t.push(200, LOGIN_RESP);
        let mut c = WenlaiHttpClient::new("https://qingfawen.fcxxz.com/", t);
        let acc = c.login("山君", "pw").unwrap();
        assert_eq!(acc.user_id, 26);
        assert_eq!(acc.jwt_token, "jwt-abc");
        assert!(c.account().is_some());
        // 请求形态断言：POST + JSON body + 正确 URL
        let reqs = c.transport.requests();
        assert_eq!(reqs[0].method, HttpMethod::Post);
        assert!(reqs[0].url.ends_with("/api/auth/login"));
        let body: Value = serde_json::from_str(reqs[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["username"], "山君");
        assert_eq!(body["password"], "pw");
    }

    #[test]
    fn client_fetch_random_and_adjacent() {
        let t = ScriptableMockTransport::new();
        t.push(200, LOGIN_RESP)
            .push(200, RANDOM_RESP)
            .push(200, RANDOM_RESP);
        let mut c = WenlaiHttpClient::new("https://qingfawen.fcxxz.com/", t);
        c.login("山君", "pw").unwrap();
        let params = FetchParams {
            difficulty: Difficulty::Pu,
            chars: 100,
            category: "chuban".into(),
            mode: FetchMode::Random,
            book_id: String::new(),
            sort_num: 0,
            ..FetchParams::default()
        };
        let a = c.fetch(&params).unwrap();
        assert_eq!(a.seg_mark, "2-277101");
        // 第二次：adjacent next（带 Bearer）
        let params2 = FetchParams {
            mode: FetchMode::Next,
            book_id: a.book_id.clone(),
            sort_num: a.sort_num,
            ..params
        };
        let a2 = c.fetch(&params2).unwrap();
        assert_eq!(a2.seg_mark, "2-277101");
        // 请求断言：Bearer 头 + adjacent 参数
        let reqs = c.transport.requests();
        assert_eq!(reqs[1].url, "https://qingfawen.fcxxz.com/api/texts/random?difficultyLevel=4&difficulty=4&length=100&strictLength=true&category=chuban&useShelf=false&sequential=false");
        assert!(reqs[2]
            .url
            .contains("/api/texts/adjacent?bookId=8899&sortNum=2&direction=next"));
        let auth = reqs[2]
            .headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .unwrap();
        assert_eq!(auth.1, "Bearer jwt-abc");
    }

    #[test]
    fn client_base_url_trims_suffix() {
        let t = ScriptableMockTransport::new();
        let c = WenlaiHttpClient::new("https://qingfawen.fcxxz.com///", t);
        assert_eq!(c.base_url(), "https://qingfawen.fcxxz.com");
        let t2 = ScriptableMockTransport::new();
        let c2 = WenlaiHttpClient::new("https://x.com/api/get_text", t2);
        assert_eq!(c2.base_url(), "https://x.com"); // 旧路径裁剪
    }

    #[test]
    fn client_categories_filters_inactive() {
        let t = ScriptableMockTransport::new();
        t.push(
            200,
            r#"{"code":200,"data":[{"code":"chuban","name":"出版"},{"code":"old","name":"旧","isActive":false},{"slug":"wangwen","label":"网文"}]}"#,
        );
        let mut c = WenlaiHttpClient::new("https://x.com", t);
        let cats = c.categories().unwrap();
        assert_eq!(
            cats,
            vec![
                ("chuban".to_string(), "出版".to_string()),
                ("wangwen".to_string(), "网文".to_string())
            ]
        );
    }

    #[test]
    fn client_daily_words_report() {
        let t = ScriptableMockTransport::new();
        t.push(200, r#"{"code":200,"data":null}"#);
        let mut c = WenlaiHttpClient::new("https://x.com", t);
        c.report_daily_words(500, "2026-09-28").unwrap();
        let reqs = c.transport.requests();
        assert!(reqs[0].url.ends_with("/api/dailyWords/report"));
        let body: Value = serde_json::from_str(reqs[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["count"], 500);
        assert_eq!(body["date"], "2026-09-28");
    }

    #[test]
    fn client_http_error_carries_server_msg() {
        let t = ScriptableMockTransport::new();
        t.push(401, r#"{"code":401,"msg":"请先登录"}"#);
        let c = WenlaiHttpClient::new("https://x.com", t);
        let err = c
            .fetch_random(&FetchParams {
                difficulty: Difficulty::Pu,
                chars: 100,
                category: "chuban".into(),
                mode: FetchMode::Random,
                book_id: String::new(),
                sort_num: 0,
                ..FetchParams::default()
            })
            .unwrap_err();
        assert!(err.contains("请先登录"), "{err}");
        assert!(err.contains("401"));
    }

    #[test]
    fn report_score_is_honest_stub() {
        // 加密未实装 → 明确报错（不假实现）；载荷构造在 build_race_payload 测试验证
        let t = ScriptableMockTransport::new();
        let mut c = WenlaiHttpClient::new("https://x.com", t);
        let r = WenlaiClient::report_score(
            &mut c,
            &WenlaiScorePost {
                leaderboard: crate::score::LeaderboardScore {
                    speed: 191.47,
                    raw_speed: 201.77,
                    keystroke: 3.57,
                    code_length: 1.061,
                    key_accuracy: 94.71,
                    wrong: 2,
                    word_count: 196,
                    time_ms: 58_285,
                    total_keys: 208,
                    backspaces: 6,
                    corrections: 3,
                    input_method: "五笔".into(),
                    mark: "2-277101".into(),
                },
                score_text: String::new(),
            },
        );
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("RSA"));
    }

    #[test]
    fn race_payload_field_names_align_qingfawen() {
        let lb = crate::score::LeaderboardScore {
            speed: 191.47,
            raw_speed: 201.77,
            keystroke: 3.57,
            code_length: 1.061,
            key_accuracy: 94.71,
            wrong: 2,
            word_count: 196,
            time_ms: 58_285,
            total_keys: 208,
            backspaces: 6,
            corrections: 3,
            input_method: "五笔".into(),
            mark: "2-277101".into(),
        };
        let p = build_race_payload(&lb, "race-1", "art-9");
        // 晴 API 字段名（_research/typesunny-qq.md §4.3 / RaceAPI.cs:186-222）
        assert_eq!(p["raceId"], "race-1");
        assert_eq!(p["articleId"], "art-9");
        assert_eq!(p["speed"], "191.47");
        assert_eq!(p["charCount"], 196);
        assert_eq!(p["keystroke"], "3.57");
        assert_eq!(p["codeLength"], "1.061");
        assert_eq!(p["keyCount"], 208);
        assert_eq!(p["backspaceCount"], 6);
        assert_eq!(p["keyAccuracy"], "94.71");
        assert_eq!(p["wrongNum"], 2);
        assert_eq!(p["inputMethod"], "五笔");
    }

    #[test]
    fn query_values_cannot_inject_filters_and_preserve_adjacent_cursors() {
        let t = ScriptableMockTransport::new();
        t.push(200, RANDOM_RESP);
        let c = WenlaiHttpClient::new("https://x.com", t);
        let params = FetchParams {
            category: "出版&length=1".into(),
            book_id: "a&b".into(),
            end_sort_num: 12,
            end_chars: "中+&".into(),
            start_chars: "?头".into(),
            strict_length: false,
            ..FetchParams::default()
        };
        c.fetch_adjacent(&params, true).unwrap();
        let req = c.transport.requests().remove(0);
        assert!(req.url.contains("bookId=a%26b"));
        assert!(req.url.contains("strictLength=false"));
        assert!(req.url.contains("category=%E5%87%BA%E7%89%88%26length%3D1"));
        assert!(req
            .url
            .contains("endSortNum=12&endChars=%E4%B8%AD%2B%26&startChars=%3F%E5%A4%B4"));
    }

    #[test]
    fn failed_login_and_expired_auth_clear_previous_account() {
        let t = ScriptableMockTransport::new();
        t.push(200, LOGIN_RESP)
            .push(200, r#"{"code":200,"data":{}}"#)
            .push(200, LOGIN_RESP)
            .push(401, r#"{"msg":"登录过期"}"#);
        let mut c = WenlaiHttpClient::new("https://x.com", t);
        c.login("user", "pw").unwrap();
        assert!(c.login("user", "pw").is_err());
        assert!(c.account().is_none());
        c.login("user", "pw").unwrap();
        assert!(c.fetch(&FetchParams::default()).is_err());
        assert!(c.account().is_none());
        assert!(c.token.is_none());
    }

    #[test]
    fn origin_validation_prevents_credentials_crossing_servers() {
        assert_eq!(
            normalize_base_url(" https://EXAMPLE.com:443/ ").unwrap(),
            "https://example.com"
        );
        assert_eq!(
            normalize_base_url("https://[::1]:8443").unwrap(),
            "https://[::1]:8443"
        );
        for url in [
            "http://x.com",
            "https://user@x.com",
            "https://x.com/api",
            "https://x.com?token=t",
            "https://x.com\\evil",
            "https://x.com:0",
            "https://-x.com",
            "https://x..com",
        ] {
            assert!(normalize_base_url(url).is_err(), "{url}");
        }
        let mut c = WenlaiHttpClient::new("https://x.com", ScriptableMockTransport::new());
        let account = WenlaiAccount {
            domain: "https://y.com".into(),
            jwt_token: "secret".into(),
            ..WenlaiAccount::default()
        };
        assert!(c.restore_account(account).is_err());
        assert!(c.account().is_none());
    }

    #[test]
    fn unknown_segments_are_not_forged_and_secrets_are_redacted() {
        let a = parse_article_data(&json!({"bookName":"书名","content":"正文"})).unwrap();
        assert!(a.publish_text().ends_with("-----晴发文"));
        assert!(!a.publish_text().contains("第1段"));
        let account = WenlaiAccount {
            jwt_token: "secret-token".into(),
            ..WenlaiAccount::default()
        };
        assert!(!format!("{account:?}").contains("secret-token"));
        let req = HttpRequest {
            method: HttpMethod::Post,
            url: "https://x.com".into(),
            headers: vec![("Authorization".into(), "secret-token".into())],
            body: Some("secret-password".into()),
        };
        assert!(!format!("{req:?}").contains("secret-"));
        assert!(!safe_login_error("bad password secret-password").contains("secret-password"));
        assert!(!parse_envelope(r#"{"token":"secret-token"}"#)
            .unwrap_err()
            .contains("secret-token"));
    }

    #[test]
    fn login_errors_hide_raw_and_json_escaped_credentials() {
        let password = "private-\"\\\n密码";
        let echoed = json!({"username":"user", "password":password}).to_string();
        let msg = format!("request body: {echoed}; raw password: {password}");
        let modern = json!({"code":401,"msg":msg}).to_string();
        let legacy = json!({"error":3,"msg":msg}).to_string();
        for (status, body, expected) in [
            (
                401,
                modern.as_str(),
                "登录失败，请检查账号、密码和服务地址 (HTTP 401)",
            ),
            (
                200,
                modern.as_str(),
                "登录失败，请检查账号、密码和服务地址 (code 401)",
            ),
            (
                200,
                legacy.as_str(),
                "登录失败，请检查账号、密码和服务地址 (error 3)",
            ),
            (
                200,
                "private-\\u0022\\\\\\n\\u5bc6\\u7801",
                "登录失败，请检查账号、密码和服务地址",
            ),
        ] {
            let transport = ScriptableMockTransport::new();
            transport.push(status, body);
            let mut client = WenlaiHttpClient::new("https://x.com", transport);
            assert_eq!(
                client.login("user", password).unwrap_err().to_string(),
                expected
            );
            assert!(client.account().is_none());
        }
    }

    #[test]
    fn login_errors_keep_only_fixed_network_stage_and_numeric_status() {
        assert_eq!(
            safe_login_error("网络请求失败: 发送请求失败（系统代码80072EFD） private-secret"),
            "登录时发送请求失败，请检查网络和服务地址后重试"
        );
        assert_eq!(
            safe_login_error("网络请求失败: 网络请求超过30秒，请稍后重试 private-secret"),
            "登录请求超过30秒，请检查网络后重试"
        );
        assert_eq!(
            safe_login_error("网络请求失败: private-secret"),
            "登录网络请求失败，请检查网络和服务地址后重试"
        );
        assert_eq!(
            safe_login_error("private-secret (HTTP forged)"),
            "登录失败，请检查账号、密码和服务地址"
        );
    }

    #[test]
    fn bookshelf_rejects_unknown_shapes_and_keeps_numeric_book_ids() {
        let t = ScriptableMockTransport::new();
        t.push(200,r#"{"code":200,"data":{"books":[{"bookId":42,"bookName":"测试","sortNum":9}],"mode":"SEQUENTIAL"}}"#)
            .push(200,r#"{"code":200,"data":{"something":[]}}"#);
        let c = WenlaiHttpClient::new("https://x.com", t);
        let books = c.bookshelf().unwrap();
        assert_eq!(books[0].book_id, "42");
        assert_eq!(books[0].sort_num, 9);
        assert_eq!(books[0].mode, "SEQUENTIAL");
        assert!(c.bookshelf().is_err());
    }

    #[test]
    fn shelf_selection_uses_random_endpoint_with_explicit_source() {
        let t = ScriptableMockTransport::new();
        t.push(200, RANDOM_RESP);
        let c = WenlaiHttpClient::new("https://x.com", t);
        c.fetch_random(&FetchParams {
            book_id: "42".into(),
            use_shelf: true,
            sequential: true,
            ..FetchParams::default()
        })
        .unwrap();
        let req = c.transport.requests().remove(0);
        assert!(req.url.contains("/api/texts/random?"));
        assert!(req.url.contains("difficulty=4&"));
        assert!(req.url.contains("useShelf=true&sequential=true&bookId=42"));
    }
}
