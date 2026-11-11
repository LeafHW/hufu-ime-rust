//! 虎符跟打：真实 RichEdit/TSF 输入，原生 D2D 界面，后台联网。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod analysis;
mod history;
mod keyboard;
mod modals;
mod online;
mod profile;
mod qq_bridge;
mod secondary_panels;
mod session;
mod skin;
mod ui;
mod uia_bridge;
use gale_core::{
    config::{Config, HintStrategy},
    rules::{CleaningPreset, CleaningRule, TrainingMode},
    score::ScoreStore,
    wenlai::{Difficulty, FetchParams, WenlaiArticle},
};
use gale_win::{
    d2d::Renderer,
    edit::NativeEdit,
    input_host::{CommandKey, HostConfig, HostEvent, InputHost},
    window::{MainWindow, MouseEv},
};
use modals::{Action, Modal, Model};
use online::{OnlineCommand, OnlinePayload, OnlinePrefs, OnlineWorker};
use session::{Phase, Session, SessionSource};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, VK_CONTROL};
use windows::Win32::{
    Foundation::{GetLastError, BOOL, ERROR_ALREADY_EXISTS, HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::*,
};

pub use analysis::{FullAnalysis, FullAnalysisEntry};

/// Full physical keyboard capture is independent of speed statistics.

fn heatmap_vk(vk: u32) -> Option<u8> {
    match vk & 0xff {
        0 | 0xe5 | 0xff => None,
        v => Some(v as u8),
    }
}

/// 私有标记通道（整改2/5/8/13）：modals.rs（禁改域）暂无对应 Action 变体，
/// 借用 `Action::Theme(String)` 载荷携带 `\u{e000}` 前缀命令。真实主题名
/// 永不含 U+E000，不会与外观页主题切换冲突；modals.rs 落正式变体（如
/// Action::OnlineRegister / ShowFullAnalysis）
/// 后，把发射点换绑即可，main.rs 这边的解码可原样退役。
const AUX_PREFIX: char = '\u{e000}';
const AUX_REGISTER: &str = "online_register";
const AUX_REGISTER_CODE: &str = "online_reg_code";
const AUX_REGISTER_SUBMIT: &str = "online_reg_submit";
const AUX_AUTO_NEXT: &str = "online_auto_next";
const AUX_SHOW_FULL: &str = "show_full_analysis";
const AUX_SHOW_HEATMAP: &str = "show_heatmap";

enum AuxAction {
    /// 登录卡 ↔ 注册卡切换（原整改2c"开站点注册页"已由应用内注册卡取代）。
    OnlineRegister,
    /// 注册卡"发送验证码"（RegisterSendCode，60s 客户端冷却）。
    OnlineRegisterCode,
    /// 注册卡"注册并登录"（Register，worker 内成功后自动登录）。
    OnlineRegisterSubmit,
    OnlineAutoNext,
    ShowFullAnalysis,
    ShowHeatmap,
    AnalysisAuto4,
    AnalysisFont(u8),
    AnalysisView,
    AnalysisCopy,
    AnalysisRetry,
    Preference(String),
    HistoryToggle,
    HistoryOpen(usize),
    HistoryStep(i32),
}

fn aux_action(a: &Action) -> Option<AuxAction> {
    let Action::Theme(s) = a else {
        return None;
    };
    let cmd = s.strip_prefix(AUX_PREFIX)?;
    Some(match cmd {
        AUX_REGISTER => AuxAction::OnlineRegister,
        AUX_REGISTER_CODE => AuxAction::OnlineRegisterCode,
        AUX_REGISTER_SUBMIT => AuxAction::OnlineRegisterSubmit,
        AUX_AUTO_NEXT => AuxAction::OnlineAutoNext,
        AUX_SHOW_FULL => AuxAction::ShowFullAnalysis,
        AUX_SHOW_HEATMAP => AuxAction::ShowHeatmap,
        "analysis_auto4" => AuxAction::AnalysisAuto4,
        "analysis_view" => AuxAction::AnalysisView,
        "analysis_copy" => AuxAction::AnalysisCopy,
        "analysis_retry" => AuxAction::AnalysisRetry,
        _ if cmd.starts_with("analysis_font:") => AuxAction::AnalysisFont(
            cmd.strip_prefix("analysis_font:")?
                .parse::<u8>()
                .ok()?
                .clamp(12, 36),
        ),
        "history_toggle" => AuxAction::HistoryToggle,
        _ if cmd.starts_with("history_open:") => {
            AuxAction::HistoryOpen(cmd.strip_prefix("history_open:")?.parse().ok()?)
        }
        _ if cmd.starts_with("history_step:") => {
            AuxAction::HistoryStep(cmd.strip_prefix("history_step:")?.parse().ok()?)
        }
        _ if cmd.starts_with("pref_") => AuxAction::Preference(cmd.to_string()),
        _ => return None,
    })
}

pub(crate) fn aux_marker(cmd: &str) -> Action {
    Action::Theme(format!("{AUX_PREFIX}{cmd}"))
}
const PREFERENCE_COLORS: &[&str] = &[
    "跟打区背景色",
    "对打未打底色",
    "打对字色",
    "对打打对底色",
    "打错字色",
    "对打打错底色",
    "词提编码色",
    "词提角标色",
    "词提一码色",
    "词提简码色",
    "词提二字三码色",
    "词提二字全码色",
    "词提三字全码色",
    "词提四字全码色",
    "词提下划线色",
];
const SCORE_FIELD_LABELS: &[(&str, &str)] = &[
    ("segment", ""),
    ("speed", "速度"),
    ("accuracy", "键准"),
    ("code_len", "码长"),
    ("word_rate", "词率"),
    ("select", "选重"),
    ("retype", "重打"),
    ("chars", "字数"),
    ("time", "用时"),
    ("keys", "总键数"),
    ("backspace", "退格"),
    ("wrong", "错字"),
    ("signature", ""),
];
fn canonical_score_fields(fields: &str) -> String {
    let mut unique = Vec::new();
    for key in fields.split(',') {
        let key = match key.trim() {
            "accuracy" => "key_accuracy",
            "select" => "select_count",
            "retype" => "retype_count",
            "backspace" => "backspaces",
            other => other,
        };
        if !key.is_empty() && !unique.contains(&key) {
            unique.push(key);
        }
    }
    gale_core::scorecard::upgrade_standard_fields(&unique.join(","))
}
fn configured_template(cfg: &Config) -> String {
    if !cfg.score_template.trim().is_empty() {
        return gale_core::scorecard::upgrade_builtin_template(&cfg.score_template);
    }
    let fields = canonical_score_fields(&cfg.score_fields);
    if fields == gale_core::scorecard::STANDARD_SCORE_FIELDS {
        return gale_core::scorecard::preset("standard").template.into();
    }
    let parts: Vec<_> = fields
        .split(',')
        .filter(|key| key.trim() != "character_accuracy")
        .filter_map(|key| {
            let label = SCORE_FIELD_LABELS
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| *v)
                .or_else(|| {
                    gale_core::scorecard::SCORE_FIELDS
                        .iter()
                        .find(|(k, _)| *k == key)
                        .map(|(_, v)| v.split('（').next().unwrap_or(v))
                })?;
            Some(format!(
                "{label}{{{key}}}{}",
                if matches!(key, "accuracy" | "character_accuracy" | "key_accuracy") {
                    "%"
                } else {
                    ""
                }
            ))
        })
        .collect();
    if parts.is_empty() {
        gale_core::scorecard::preset("standard").template.into()
    } else {
        parts.join("  ")
    }
}

/// Which score-report preset the saved configuration corresponds to. The old
/// build kept this only in `Model.fields`, so reopening F1 highlighted nothing
/// and the settings looked unsaved.
fn current_score_preset(cfg: &Config) -> &'static str {
    let template = cfg.score_template.trim();
    if template.is_empty() {
        return if canonical_score_fields(&cfg.score_fields)
            == gale_core::scorecard::STANDARD_SCORE_FIELDS
        {
            "standard"
        } else {
            "custom"
        };
    }
    gale_core::scorecard::builtin_preset_id(template).unwrap_or("custom")
}

pub fn data_dir() -> PathBuf {
    if let Some(v) =
        cli_value("--data").or_else(|| std::env::var_os("HUFU_TRAINER_DATA").map(PathBuf::from))
    {
        return v;
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
        .join("HuFu")
        .join("trainer")
}
fn cli_value(key: &str) -> Option<PathBuf> {
    let args: Vec<_> = std::env::args_os().collect();
    args.windows(2)
        .find(|a| a[0] == key)
        .map(|a| PathBuf::from(&a[1]))
}
fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
/// Every analysis request uses the same scalar offsets for round boundaries.
/// Empty segments do not add duplicate or out-of-range breaks.
fn segment_analysis_breaks(segments: &[gale_core::segment::Segment]) -> Vec<usize> {
    let total: usize = segments
        .iter()
        .map(|segment| segment.text.chars().count())
        .sum();
    let (mut offset, mut breaks) = (0, Vec::new());
    for segment in segments.iter().take(segments.len().saturating_sub(1)) {
        offset += segment.text.chars().count();
        if offset > 0 && offset < total && breaks.last().copied() != Some(offset) {
            breaks.push(offset);
        }
    }
    breaks
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct RequiredSpan {
    start: usize,
    len: usize,
}

fn analysis_request_key(
    text: &str,
    breaks: &[usize],
    single: bool,
    schema_ver: &str,
    auto4: bool,
    strategy: HintStrategy,
    required_spans: &[RequiredSpan],
    dynamic_boundary: Option<usize>,
    route_mode: &str,
) -> String {
    format!(
        "{}:{breaks:?}:{single}:{schema_ver}:{auto4}:{}:{required_spans:?}:{dynamic_boundary:?}:{route_mode}",
        gale_core::rules::fingerprint(text.as_bytes()), strategy.as_str(),
    )
}

fn validate_route_response(
    value: &serde_json::Value,
    strategy: HintStrategy,
    required_spans: &[RequiredSpan],
    dynamic_boundary: Option<usize>,
    route_mode: &str,
) -> Result<(), String> {
    let echoed: Vec<RequiredSpan> = serde_json::from_value(value["required_spans"].clone())
        .map_err(|_| "当前虎符服务不支持新词提规则，请更新服务后重试".to_string())?;
    if value["route_rules"].as_str() != Some("quiettype-v1")
        || value["strategy"].as_str() != Some(strategy.as_str())
        || value["route_mode"].as_str() != Some(route_mode)
        || echoed != required_spans
        || match dynamic_boundary {
            Some(boundary) => value["dynamic_boundary"].as_u64() != Some(boundary as u64),
            None => !value["dynamic_boundary"].is_null(),
        }
    {
        return Err("词提路由规则与请求不一致，请更新虎符服务后重试".into());
    }
    Ok(())
}

fn fetch_theory(
    text: &str,
    single: bool,
    auto4: bool,
    breaks: &[usize],
    strategy: HintStrategy,
) -> Result<FullAnalysis, String> {
    if text.chars().count() > 10000 {
        return Err("全文分析最多支持10000字，请载入较短文章".into());
    }
    let value = local_json(
        local_port(),
        "/api/trainer/analyze",
        Some(serde_json::json!({
            "text": text, "single": single, "theory_auto4": auto4, "breaks": breaks,
            "strategy": strategy.as_str(), "required_spans": [],
            "dynamic_boundary": null, "route_mode": "theory",
        })),
    )?;
    validate_route_response(&value, strategy, &[], None, "theory")?;
    if value["theory_auto4"].as_bool() != Some(auto4) {
        return Err("当前虎符服务不支持理论分析，请更新服务后重试".into());
    }
    full_analysis_from_response(text, &value, auto4)
}

fn full_analysis_from_response(
    text: &str,
    value: &serde_json::Value,
    auto4: bool,
) -> Result<FullAnalysis, String> {
    let mut fa = FullAnalysis {
        entries: analysis::response_entries(text, &value)?,
        schema: value["schema"].as_str().unwrap_or("当前词库").into(),
        schema_ver: value["schema_ver"].as_str().unwrap_or("").into(),
        max_code_length: value["max_code_length"].as_u64().unwrap_or(4) as usize,
        ..Default::default()
    };
    let summary = analysis::summarize(&fa, auto4);
    fa.total_code_len = summary.known_keys;
    fa.avg_code_len_per_char = summary.avg_code_len;
    fa.select_count = summary.select_count;
    Ok(fa)
}

/// Match the reference's longest correct prefix, including relaxed quotes.
/// A wrong character stops progress even when later characters match.
fn dynamic_prefix_boundary(sess: &Session) -> usize {
    sess.comparer
        .typed
        .iter()
        .copied()
        .zip(sess.current_segment().text.chars())
        .take(sess.comparer.pos)
        .take_while(|(typed, expected)| sess.comparer.quote_rules.equivalent(*typed, *expected))
        .count()
}

/// Coalesce rapid commits into a bounded window rather than waiting for an
/// input pause. The latest boundary replaces the pending one without moving
/// its deadline; a response in flight is followed by analyze() on completion.
fn dynamic_hint_refresh_due(
    boundary: Option<usize>,
    seen: &mut Option<usize>,
    due: &mut Option<u64>,
    now: u64,
) -> bool {
    if boundary != *seen {
        *seen = boundary;
        due.get_or_insert(now.saturating_add(80));
    }
    if due.is_some_and(|at| now >= at) {
        *due = None;
        true
    } else {
        false
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SchemaRevision {
    epoch: u64,
    version: String,
    unavailable_reason: Option<String>,
}
impl SchemaRevision {
    fn accepts(&self, dispatched: &Self, returned_version: &str) -> bool {
        self == dispatched && self.version == returned_version
    }
}

#[derive(Debug, Default)]
struct RetryBackoff {
    failures: u8,
    retry_at: u64,
}
impl RetryBackoff {
    fn ready(&self, now: u64) -> bool {
        now >= self.retry_at
    }
    fn fail(&mut self, now: u64) {
        self.failures = self.failures.saturating_add(1);
        let delay = (400u64 << self.failures.min(4)).min(4000);
        self.retry_at = now.saturating_add(delay);
    }
    fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StatusRequest {
    id: u64,
    epoch: u64,
}
#[derive(Debug, Default)]
struct LiveSchemaStatus {
    revision: SchemaRevision,
    snapshot: Option<serde_json::Value>,
    next_poll_at: u64,
    sequence: u64,
    in_flight: Option<StatusRequest>,
    retry: RetryBackoff,
    /// A result from a different revision must wait for a fresh status sample.
    awaiting_status: bool,
}
enum StatusOutcome {
    Ignored,
    Failed(String),
    Ready { changed: bool },
}
impl LiveSchemaStatus {
    fn begin(&mut self, now: u64) -> Option<StatusRequest> {
        if self.in_flight.is_some() || now < self.next_poll_at {
            return None;
        }
        self.sequence = self.sequence.wrapping_add(1);
        let request = StatusRequest {
            id: self.sequence,
            epoch: self.revision.epoch,
        };
        self.in_flight = Some(request);
        Some(request)
    }
    fn complete(
        &mut self,
        request: StatusRequest,
        now: u64,
        result: Result<serde_json::Value, String>,
    ) -> StatusOutcome {
        if self.in_flight != Some(request) {
            return StatusOutcome::Ignored;
        }
        self.in_flight = None;
        if request.epoch != self.revision.epoch {
            self.next_poll_at = now;
            return StatusOutcome::Ignored;
        }
        let result = result.and_then(|value| {
            // Older services can return a transient Host try_lock failure as
            // HTTP 200. It is not evidence that the dictionary disappeared.
            if value["schema_ver"].as_str() == Some("")
                && value["hint_unavailable_reason"].as_str() == Some("输入引擎忙，请稍后重试")
            {
                Err("输入引擎忙，请稍后重试".into())
            } else if value["schema_ver"].is_string() {
                Ok(value)
            } else {
                Err("虎符服务未返回词库版本，请更新服务后重试".into())
            }
        });
        match result {
            Ok(value) => {
                let version = value["schema_ver"].as_str().unwrap().to_string();
                let unavailable_reason = value["hint_unavailable_reason"]
                    .as_str()
                    .map(str::to_string);
                let changed = self.snapshot.is_none()
                    || version != self.revision.version
                    || unavailable_reason != self.revision.unavailable_reason;
                if changed {
                    self.revision.epoch = self.revision.epoch.wrapping_add(1);
                    self.revision.version = version;
                    self.revision.unavailable_reason = unavailable_reason;
                }
                self.snapshot = Some(value);
                self.awaiting_status = false;
                self.retry.reset();
                self.next_poll_at = now.saturating_add(400);
                StatusOutcome::Ready { changed }
            }
            Err(error) => {
                self.retry.fail(now);
                self.next_poll_at = self.retry.retry_at;
                StatusOutcome::Failed(error)
            }
        }
    }
    fn request_fresh(&mut self, now: u64) {
        self.awaiting_status = true;
        self.next_poll_at = self.next_poll_at.min(now);
    }
    fn ready(&self) -> bool {
        self.snapshot.is_some() && !self.awaiting_status
    }
    fn wait_ms(&self, now: u64) -> u32 {
        if self.in_flight.is_some() {
            500
        } else {
            self.next_poll_at.saturating_sub(now).clamp(1, 500) as u32
        }
    }
}

/// Only derived views change when a dictionary revision changes. In particular,
/// the active round, its input host and its frozen scoring rules stay intact.
fn clear_live_hint_views(model: &mut Model, sess: &mut Session) {
    model.words.clear();
    sess.full_analysis = None;
    model.fields.remove("analysis_error");
    model.fields.remove("analysis_failed_key");
}
/// P2 今日汇总（任务 6b）：从磁盘成绩库（统计/records.jsonl；load_valid 过
/// <5 硬门，与聚合口径一致）算今天的字数。ui.rs 的今日卡片读 cfg.today_chars
///（该文件禁改），数据由 main 侧在启动时 + 每次成绩保存成功后注入。
fn today_chars_from_dir(dir: &Path) -> u64 {
    let today = gale_core::score::unix_to_date(unix_secs());
    gale_core::score::ScoreStore::open(&dir.join("统计"))
        .map(|store| gale_core::report::day_summary(&store.load_valid(), &today).chars)
        .unwrap_or(0)
}
fn completed_segment_action(command: CommandKey, done: bool, modal: Modal) -> Option<Action> {
    if done && modal == Modal::None && matches!(command, CommandKey::Enter | CommandKey::Space) {
        Some(Action::Next)
    } else {
        None
    }
}

/// Esc dismisses panels only; the active round and its clock stay untouched.
fn escape_action(modal: Modal, ledger_open: bool, analysis_dock: bool) -> Option<Action> {
    if modal != Modal::None {
        Some(Action::Close)
    } else if ledger_open {
        Some(Action::ToggleLedger)
    } else if analysis_dock {
        Some(Action::ToggleAnalysis)
    } else {
        None
    }
}

/// Physical keys precede their document changes in the host queue. Resume at
/// that boundary so the first key and its commit are included, while delayed
/// IME notifications and simply returning to the window keep the clock paused.
#[derive(Default)]
struct DispatchedInput {
    // The host emits one cause per non-ASCII commit, including rejected ones.
    accepted_commits: Vec<bool>,
}
impl DispatchedInput {
    fn apply_commit_causes(
        self,
        sess: &mut Session,
        causes: Vec<gale_win::input_host::CommitCause>,
    ) {
        for (accepted, cause) in self.accepted_commits.into_iter().zip(causes) {
            if accepted {
                sess.on_commit_cause(cause);
            }
        }
    }
}

fn dispatch_host_events(
    sess: &mut Session,
    events: Vec<HostEvent>,
    can_resume: bool,
    now: u64,
) -> DispatchedInput {
    let mut dispatched = DispatchedInput::default();
    let batch_has_commit = events.iter().any(|e| matches!(e, HostEvent::Commit(_)));
    let mut pause_after_flush = false;
    for ev in events {
        match ev {
            HostEvent::Key { vk } => {
                if can_resume && gale_core::stats::StatsEngine::is_stat_key(vk) {
                    sess.set_paused(false, now);
                    pause_after_flush = false;
                }
                sess.on_key(vk, now);
            }
            HostEvent::ResumeKey { .. } => {
                if can_resume {
                    sess.set_paused(false, now);
                    pause_after_flush = false;
                }
            }
            // The final document diff is appended after physical events. Keep
            // this last commit, then pause even if focus returned in this batch.
            HostEvent::FocusLost => pause_after_flush = true,
            HostEvent::CandidateSelection { vk, .. } => sess.on_candidate_selection(vk, now),
            HostEvent::Commit(chars) => {
                let has_cause = !chars.is_ascii();
                let previous_pos = sess.comparer.pos;
                sess.feed_input(gale_core::compare::InputEvent::Commit { chars }, now);
                if has_cause {
                    dispatched
                        .accepted_commits
                        .push(sess.comparer.pos > previous_pos);
                }
            }
            HostEvent::Backspace => sess.feed_input(gale_core::compare::InputEvent::Backspace, now),
            HostEvent::Composing(c) => sess.set_composing_from_host(c, batch_has_commit),
            HostEvent::Preedit(p) => sess.set_preedit(p),
            HostEvent::Cancelled => sess.cancel_composition(),
        }
    }
    if pause_after_flush {
        sess.set_paused(true, now);
    }
    dispatched
}

/// P0 单实例守卫（任务 3）：命名互斥体 `HuFu-Trainer-SingleInstance` 为主判据。
/// 返回 true = 已有活实例并已唤醒其窗口，调用方应退出（exit 0）。
/// 正常路径握住互斥体句柄到进程退出（std::mem::forget 防 CloseHandle；OS 进程
/// 结束自动回收）。window.rs 的 activate_existing_instance（旧版互斥体名）仍
/// 作为上一代实例的兜底在其后调用。
fn single_instance_guard() -> bool {
    use windows::Win32::Foundation::WAIT_EVENT;
    use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
    unsafe {
        let handle =
            match CreateMutexW(None, true, windows::core::w!("HuFu-Trainer-SingleInstance")) {
                Ok(h) => h,
                Err(_) => return false, // 创建失败不拦启动（宁可双开也不打不开）
            };
        // 判据：CreateMutexW 后 GetLastError==ERROR_ALREADY_EXISTS 且探测未接管
        //（WAIT_ABANDONED = 旧实例持锁僵死，本实例接管放行——与 window.rs 同解）。
        let wait: WAIT_EVENT = WaitForSingleObject(handle, 0);
        let already = GetLastError() == ERROR_ALREADY_EXISTS && wait.0 != 0 && wait.0 != 128; /* WAIT_ABANDONED */
        if already {
            if let Some(hwnd) = find_existing_hufu_window() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
            }
            return true;
        }
        // 唯一实例：句柄随进程生命周期（windows 0.58 HANDLE 为 Copy 无 Drop，
        // 不 CloseHandle；进程退出时 OS 自动回收互斥体）。
        let _ = handle;
        false
    }
}
/// 找已运行的虎符跟打主窗口：先按主窗口类名（window.rs 只读常量，gale-win
/// 禁改故此处硬拷贝字面量），找不到再 EnumWindows 按标题模糊匹配（旧版兜底）。
fn find_existing_hufu_window() -> Option<HWND> {
    unsafe {
        if let Ok(hwnd) = FindWindowExW(None, None, windows::core::w!("HuFuTrainerWindow"), None) {
            return Some(hwnd);
        }
        let mut found: Vec<HWND> = Vec::new();
        let ctx = LPARAM(&mut found as *mut Vec<HWND> as isize);
        let _ = EnumWindows(Some(collect_hufu_windows), ctx);
        found.into_iter().next()
    }
}
unsafe extern "system" fn collect_hufu_windows(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam.0 as *mut Vec<HWND>);
    let mut buf = [0u16; 96];
    let n = GetWindowTextW(hwnd, &mut buf);
    let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
    if title.contains("虎符跟打") && out.is_empty() {
        out.push(hwnd);
    }
    BOOL(1)
}
pub fn timestamp_ms() -> u64 {
    unix_secs().saturating_mul(1000)
}
fn wake(hwnd: isize) {
    unsafe {
        let _ = PostMessageW(HWND(hwnd as *mut _), WM_NULL, WPARAM(0), LPARAM(0));
    }
}
enum Bg {
    TrainerStatus(StatusRequest, Result<serde_json::Value, String>),
    Analyze(
        u64,
        String,
        SchemaRevision,
        Result<serde_json::Value, String>,
    ),
    Chats(Vec<qq_bridge::ChatWindow>),
    Article(Result<String, String>),
    Saved(Result<(), String>),
    ScoreSaved(Result<(), String>),
    Skin(u64, Result<gale_core::theme::Theme, String>),
    RuleTest(u64, Result<gale_core::rules::CleaningResult, String>),
    RuleApply(u64, u64, Result<Session, String>),
    /// Full-article theory response with an explicit, retryable error.
    FullAnalysis(String, SchemaRevision, Result<FullAnalysis, String>),
    /// QQ 发送完成（在工作线程里等确认后回传；主线程收到后才切回跟打器）。
    Sent {
        label: String,
        result: Result<(), String>,
    },
}
enum Persist {
    Snapshot(Vec<u8>),
    Score(u64, gale_core::score::ScoreRecord),
    Stop(mpsc::Sender<Result<(), String>>),
}
fn append_score_once(
    store: &ScoreStore,
    r: &gale_core::score::ScoreRecord,
    attempt_start: &mut Option<u64>,
) -> std::io::Result<()> {
    if attempt_start.is_none() {
        *attempt_start = Some(match std::fs::metadata(store.path()) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e),
        });
    }
    use std::io::{Seek, SeekFrom};
    let bytes = serde_json::to_vec(r).map_err(std::io::Error::other)?;
    match std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(store.path())
    {
        Ok(mut file) => {
            let len = file.metadata()?.len();
            let size = len.min(bytes.len() as u64 + 2) as usize;
            file.seek(SeekFrom::End(-(size as i64)))?;
            let mut tail = Vec::with_capacity(size);
            file.read_to_end(&mut tail)?;
            while tail.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
                tail.pop();
            }
            if len > attempt_start.unwrap_or(len) && tail.ends_with(&bytes) {
                return file.sync_data();
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    store.append(r)
}
fn persistence(dir: PathBuf, tx: mpsc::Sender<Bg>, hwnd: isize) -> mpsc::SyncSender<Persist> {
    let (send, recv) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut last_attempt = None;
        let mut attempt_start = None;
        let (mut score_error, mut snapshot_error): (Option<String>, Option<String>) = (None, None);
        while let Ok(job) = recv.recv() {
            match job {
                Persist::Score(id, r) => {
                    if last_attempt != Some(id) {
                        attempt_start = None;
                    }
                    last_attempt = Some(id);
                    let result = ScoreStore::open(&dir.join("统计"))
                        .and_then(|s| append_score_once(&s, &r, &mut attempt_start))
                        .map_err(|e| e.to_string());
                    score_error = result.as_ref().err().cloned();
                    let _ = tx.send(Bg::ScoreSaved(result));
                    wake(hwnd);
                }
                Persist::Snapshot(bytes) => {
                    let result =
                        gale_core::score::durable_replace(&dir.join("session.jsonl"), &bytes)
                            .map_err(|e| e.to_string());
                    snapshot_error = result.as_ref().err().cloned();
                    if result.is_err() {
                        let _ = tx.send(Bg::Saved(result));
                        wake(hwnd);
                    }
                }
                Persist::Stop(ack) => {
                    let _ = ack.send(match score_error.or(snapshot_error) {
                        Some(e) => Err(e),
                        None => Ok(()),
                    });
                    break;
                }
            }
        }
    });
    send
}

struct App {
    win: MainWindow,
    renderer: Renderer,
    host: InputHost,
    cfg: Config,
    saved_cleaning: CleaningPreset,
    sess: Session,
    model: Model,
    theme: gale_core::theme::Theme,
    dir: PathBuf,
    persist: mpsc::SyncSender<Persist>,
    pending_scores: VecDeque<(u64, gale_core::score::ScoreRecord)>,
    pending_snapshot: Option<Vec<u8>>,
    score_inflight: bool,
    score_next: u64,
    score_retry_ms: u64,
    snapshot_dirty: bool,
    last_snapshot_ms: u64,
    layout_key: (u64, u32, u32),
    layout: Vec<(usize, String, Vec<f32>)>,
    edits: BTreeMap<String, NativeEdit>,
    font_picker: Option<gale_win::font_picker::NativeFontPicker>,
    edit_epoch: u64,
    hits: Vec<modals::Hit>,
    /// 最近一次鼠标位置（逻辑坐标），用于悬停高亮。
    hover: Option<(f32, f32)>,
    /// Last time a snapshot-save error toast was shown (rate limit).
    save_toast_at: u64,
    anim: ui::Animation,
    clock: Instant,
    online: Option<OnlineWorker>,
    article: Option<WenlaiArticle>,
    /// Last accepted online source; random mode must survive advancing a round.
    article_fetch: Option<FetchParams>,
    shelf: Vec<gale_core::wenlai::ShelfBook>,
    chats: Vec<qq_bridge::ChatWindow>,
    tx: mpsc::Sender<Bg>,
    rx: mpsc::Receiver<Bg>,
    generation: u64,
    analyzing: bool,
    analyzed: u64,
    analyzed_key: String,
    live_schema: LiveSchemaStatus,
    display_retry: RetryBackoff,
    /// 整改14：整篇文章的词提（全文坐标，一次分析缓存；换段本地切片，
    /// 不再打到哪段才请求哪段）。full_words_ver=生成它的 schema 版本；
    /// full_words_of/full_words_breaks=生成时的全文与段落快照（变化自动失效缓存）。
    full_words: Vec<modals::HintWord>,
    full_words_ver: String,
    full_words_of: String,
    full_words_breaks: Vec<usize>,
    full_words_strategy: HintStrategy,
    full_words_required_spans: Vec<RequiredSpan>,
    full_words_auto4: bool,
    full_words_dynamic_boundary: Option<usize>,
    dynamic_boundary_seen: Option<usize>,
    dynamic_hint_due: Option<u64>,
    /// Recent dynamic boundaries, capped at four small responses and invalidated
    /// whenever the independent status poll observes a new schema revision.
    route_cache: VecDeque<(String, serde_json::Value)>,
    /// 本次 analyze 是否按整篇请求（false=超长回退单段，响应为段内坐标）。
    last_analyze_whole: bool,
    theme_generation: u64,
    rule_generation: u64,
    rule_busy: bool,
    qq_busy: bool,
    /// QQ 后台发送中：同一时间只允许一条，避免抢焦点/重复发。
    send_busy: bool,
    exit: bool,
    online_pending: usize,
    online_logged_in: bool,
    /// P2：CTF 输入通道自检失败（启动时探测一次）——状态栏持续警示标记。
    tsf_broken: bool,
    /// 整改2b：自动下一段开关（online.json）+ 结算后自动取文的到期时刻
    online_auto_next: bool,
    auto_next_at: Option<u64>,
    /// 注册卡"发送验证码"重发冷却截止时刻（ms，0=空闲；主循环每秒折算
    /// model.online_code_wait 供注册卡按钮显示剩余秒）。
    reg_code_until: u64,
    /// 整改2a：当前会话正文是否来自在线文（自动下一段的判据）
    online_article_current: bool,
    /// 整改6：Ctrl+滚轮字号防抖落盘时刻（800ms）
    font_save_due: Option<u64>,
    /// 整改7：QQ 会话列表上次扫描时刻（屏开期间每 10s 刷新）
    qq_scan_at: u64,
    /// Last successful analysis key: text, source mode, schema and auto4 option.
    full_fetch_hash: Option<String>,
    full_analyzing: bool,
    full_retry: RetryBackoff,
    /// 云同步的晴账号（profile.json，随虎符云同步走）。
    sync_online_user: String,
    sync_online_password: String,
    /// 本次登录提交的明文口令：登录成功后才写入 profile，避免存错密码。
    pending_login_password: Option<(u64, String)>,
    sync_profile: profile::State,
    sync_identity_dirty: bool,
    sync_account_cleared: bool,
}
fn main() {
    if let Err(e) = run() {
        unsafe {
            let msg: Vec<u16> = format!("虎符跟打遇到问题：{e}\0").encode_utf16().collect();
            MessageBoxW(
                None,
                windows::core::PCWSTR(msg.as_ptr()),
                windows::core::w!("虎符跟打"),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}
fn run() -> Result<(), String> {
    // P0 单实例（任务 3）：新命名互斥体主判据；旧版实例由 window.rs 兜底探测。
    // 开发/验收时可用 HUFU_TRAINER_TEST_MUTEX=1 跳过单实例守卫（配合 --data 独立数据目录）。
    // 仅调试构建接受该变量；发行构建恒为 false，与原来的单实例行为一致。
    #[cfg(debug_assertions)]
    let test_instance = std::env::var_os("HUFU_TRAINER_TEST_MUTEX").is_some();
    #[cfg(not(debug_assertions))]
    let test_instance = false;
    if !test_instance {
        if single_instance_guard() {
            return Ok(());
        }
        if gale_win::window::activate_existing_instance().is_some() {
            return Ok(());
        }
    }
    let _com =
        gale_win::window::ComSta::new().map_err(|e| format!("无法初始化STA/TSF输入环境：{e}"))?;
    gale_win::window::enable_per_monitor_dpi_awareness();
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let loaded = profile::load(&dir)?;
    let mut startup_warning = loaded.warning;
    let mut cfg = loaded.config;
    // Old profiles may still contain the retired mode setting. Active rounds
    // now share ordinary typing rules; saved scores keep their original rules.
    cfg.session_rules.mode = TrainingMode::Practice;
    cfg.session_rules.pause_on_blur = true;
    let sync_online_user = loaded.user;
    let sync_online_password = loaded.password;
    if let Some(account) = &loaded.account {
        if let Err(e) = crate::online::import_synced_account(&dir, &cfg.wenlai_domain, account) {
            let warning = format!("同步账号暂未导入，本机跟打可正常使用：{e}");
            startup_warning = Some(match startup_warning {
                Some(previous) => format!("{previous}；{warning}"),
                None => warning,
            });
        }
    }
    // Native child editors require the opaque redirection surface in this build.
    cfg.material = gale_core::config::Material::Solid;
    cfg.score_fields = canonical_score_fields(&cfg.score_fields);
    let win = MainWindow::create("虎符跟打 · 静室", 800, 600).map_err(|e| e.to_string())?;
    win.restore_bounds(
        cfg.win_x,
        cfg.win_y,
        cfg.win_w,
        cfg.win_h,
        cfg.win_position_physical,
    );
    win.show_and_focus();
    let (cw, ch) = win.client_size();
    let renderer =
        Renderer::new(win.hwnd, cw.max(1) as u32, ch.max(1) as u32).map_err(|e| e.to_string())?;
    let host = InputHost::create(&HostConfig {
        parent: win.hwnd,
        x: 40,
        y: ch - 120,
        w: cw - 80,
        h: 42,
    })
    .map_err(|e| e.to_string())?;
    // P2 输入诊断（任务 7）：CTF 不可用 → 持续状态栏警示（见 App.draw 后重申）；
    // 诊断日志开启 → 启动 toast 提示。
    let host_ctf_ok = host.ctf_ok();
    let mut sess = Session::restore_snapshot(&dir.join("session.jsonl"))
        .unwrap_or_else(|_| Session::new(cfg.seg_chars));
    if sess.frozen_rules.is_none() {
        let _ = sess.set_rules(cfg.session_rules.clone());
    } else {
        sess.rules.mode = TrainingMode::Practice;
        sess.rules.pause_on_blur = true;
    }
    let history = ScoreStore::open(&dir.join("统计"))
        .map(|store| store.load_all())
        .unwrap_or_default();
    sess.seed_retype_history(history.iter());
    let mut model = Model::default();
    model.history = history;
    model.rule_save_preset = true;
    model.hints = sess.rules.word_hints;
    model.quote_relaxed = sess.rules.quotes.enabled;
    model.reduced_motion = gale_win::edit::reduced_motion() || cfg.motion_level == 0;
    model.status = "Ctrl+V 载文 · F3 重打 · F4 群载文 · F5 选群".into();
    model.online_status = "未登录 · 登录后浏览在线书架".into();
    model.online_title = "在线书架".into();
    model
        .fields
        .insert("server".into(), cfg.wenlai_domain.clone());
    model
        .fields
        .insert("username".into(), cfg.wenlai_username.clone());
    model.fields.insert("online_chars".into(), "200".into());
    model
        .fields
        .insert("rule_test".into(), "“天青色”　……\u{feff}".into());
    let mut theme = ui::theme(&cfg.theme);
    for (key, value) in &cfg.color_overrides {
        if let Some(c) = gale_core::theme::Color::from_hex_no_hash(value) {
            theme.set(key, c);
        }
    }
    let (tx, rx) = mpsc::channel();
    let online = OnlineWorker::spawn(dir.clone(), &cfg.wenlai_domain).ok();
    let persist = persistence(dir.clone(), tx.clone(), win.hwnd);
    let mut app = App {
        win,
        renderer,
        host,
        saved_cleaning: cfg.cleaning_preset.clone(),
        cfg,
        sess,
        model,
        theme,
        dir: dir.clone(),
        persist,
        pending_scores: VecDeque::new(),
        pending_snapshot: None,
        score_inflight: false,
        score_next: 1,
        score_retry_ms: 0,
        snapshot_dirty: false,
        last_snapshot_ms: 0,
        layout_key: (0, 0, 0),
        layout: Vec::new(),
        edits: BTreeMap::new(),
        font_picker: None,
        edit_epoch: u64::MAX,
        hits: Vec::new(),
        hover: None,
        save_toast_at: 0,
        anim: ui::Animation::default(),
        clock: Instant::now(),
        online,
        article: None,
        article_fetch: None,
        shelf: Vec::new(),
        chats: Vec::new(),
        tx,
        rx,
        generation: 1,
        analyzing: false,
        analyzed: 0,
        analyzed_key: String::new(),
        live_schema: LiveSchemaStatus::default(),
        display_retry: RetryBackoff::default(),
        full_words: Vec::new(),
        full_words_ver: String::new(),
        full_words_of: String::new(),
        full_words_breaks: Vec::new(),
        full_words_strategy: HintStrategy::default(),
        full_words_required_spans: Vec::new(),
        full_words_auto4: true,
        full_words_dynamic_boundary: None,
        dynamic_boundary_seen: None,
        dynamic_hint_due: None,
        route_cache: VecDeque::new(),
        last_analyze_whole: false,
        theme_generation: 0,
        rule_generation: 0,
        rule_busy: false,
        qq_busy: false,
        send_busy: false,
        exit: false,
        online_pending: 0,
        online_logged_in: false,
        tsf_broken: !host_ctf_ok,
        online_auto_next: OnlinePrefs::load(&dir.join("online.json")).auto_next,
        auto_next_at: None,
        reg_code_until: 0,
        online_article_current: false,
        font_save_due: None,
        qq_scan_at: 0,
        full_fetch_hash: None,
        full_analyzing: false,
        full_retry: RetryBackoff::default(),
        sync_online_user: sync_online_user.clone(),
        sync_online_password: sync_online_password.clone(),
        pending_login_password: None,
        sync_profile: loaded.state,
        sync_identity_dirty: false,
        sync_account_cleared: false,
    };
    app.model
        .fields
        .insert("online_auto_next".into(), app.online_auto_next.to_string());
    app.sync_preferences(true);
    app.sync_rule_model();
    if let Some(warning) = startup_warning {
        app.toast(warning);
    }
    if !app.sync_online_user.is_empty() {
        app.model
            .fields
            .insert("username".into(), app.sync_online_user.clone());
    }
    if !app.sync_online_password.is_empty() {
        app.model
            .fields
            .insert("password".into(), app.sync_online_password.clone());
        if let Some(e) = app.edits.get("password") {
            e.set_text(&app.sync_online_password);
        }
    }
    if app.cfg.theme == "跟随虎符" {
        app.fetch_skin();
    }
    app.apply_style();
    app.host.show();
    app.host.focus();
    app.host.hide_caret();
    app.analyze();
    // P2 输入诊断（任务 7）：诊断日志开启 → 启动提示；CTF 不可用 →
    // 持续状态栏警示（覆盖默认提示语；analyze 失败文案前缀重申，见 collect_background）。
    if gale_win::input_host::diagnostic_enabled() {
        app.toast("输入诊断日志已开启");
    }
    if app.tsf_broken {
        app.model.status = "TSF 输入法可能不可用，正文将无法接收中文输入".into();
        app.toast("TSF 输入法可能不可用，正文将无法接收中文输入");
    }
    // P2 今日汇总（任务 6b）：启动时从磁盘成绩库刷新今日字数。
    app.cfg.today_chars = today_chars_from_dir(&app.dir);
    gale_win::window::request_frame();
    let mut timer = false;
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    // P1-6：gale-win 事件泵不透传 keyup——帧级 GetKeyState 边沿观测 VK_BACK
    // 弹起，通知 session 清 protect 窗（帧粒度；真实退格按压远长于一帧）。
    let mut bs_down_prev = false;
    let mut last_size = (0, 0);
    let mut stats_draw_at = 0u64;
    let mut was_focused = true;
    while !app.exit {
        if !gale_win::window::pump_messages_once() {
            break;
        }
        let now = app.ms();
        app.sess.update_clock(now);
        let seconds = app.clock.elapsed().as_secs_f64();
        // 注册卡"发送验证码"60s 冷却：每秒折算剩余秒刷新按钮禁用态显示。
        let code_wait = app
            .reg_code_until
            .checked_sub(now)
            .map(|ms| ((ms + 999) / 1000) as u16)
            .unwrap_or(0);
        if code_wait == 0 {
            app.reg_code_until = 0;
        }
        if app.model.online_code_wait != code_wait {
            app.model.online_code_wait = code_wait;
            gale_win::window::request_frame();
        }
        if app.capture_fields() {
            gale_win::window::request_frame();
        }
        app.collect_background();
        app.poll_trainer_status(now);
        if app.display_retry.retry_at != 0 && app.display_retry.ready(now) {
            app.analyze();
        }
        if app.model.analysis_dock && app.sess.full_analysis.is_none() {
            app.fetch_full_analysis();
        }
        app.collect_online();
        app.pump_persistence();
        let focused = unsafe { GetForegroundWindow() == app.win.hwnd() };
        if focused && !was_focused && (app.sess.phase == Phase::Idle || app.model.analysis_dock) {
            app.analyzed = 0;
            app.analyze();
        }
        let accept_host = app.model.modal == Modal::None && (focused || was_focused);
        was_focused = focused;
        app.online_pending = app.online.as_ref().map(|o| o.pending_count()).unwrap_or(0);
        app.model.online_busy = app.online_pending > 0;
        let oldpos = app.sess.comparer.pos;
        let oldwrong = app.sess.comparer.wrong;
        let events = app.host.poll();
        if !events.is_empty() {
            app.snapshot_dirty = true;
        }
        let dispatched = if accept_host {
            // P1-6：Composing(false) 与 Commit 同批 = 提交清空（不置 protect）；
            // 只有 Composing(false) = 退格删空/Esc 取消（置 protect）。
            dispatch_host_events(&mut app.sess, events, focused, now)
        } else {
            DispatchedInput::default()
        };
        // P1-3 选重/标顶（任务 2）：每帧 poll 之后立即 drain 20ms 配对结果
        //（input_host 顺序契约：必须在 poll 之后），Ding→标顶/Select→选重/
        // Unknown 忽略；模态打开时只清队列不记账。
        dispatched.apply_commit_causes(&mut app.sess, gale_win::input_host::drain_commit_causes());
        // 整改12：每帧取走击键时间戳队列（poll 之后、选重 20ms 配对已消费本帧
        // 击键），按热力图口径计入本段 VK 直方图；模态打开时只清队列不记账。
        for (vk, _) in gale_win::input_host::drain_key_times(1024) {
            if accept_host {
                if let Some(k) = heatmap_vk(vk) {
                    *app.sess.vk_histogram.entry(k).or_insert(0) += 1;
                }
            }
        }
        // P1-6：VK_BACK 弹起边沿（帧粒度观测，见 bs_down_prev 注释）
        let bs_down = unsafe { GetKeyState(0x08) } as u16 & 0x8000 != 0;
        if bs_down_prev && !bs_down {
            app.sess.on_backspace_up();
        }
        bs_down_prev = bs_down;
        if !focused || app.model.modal != Modal::None {
            app.sess.set_paused(true, now);
        }
        let paused = app.sess.is_paused();
        if paused != app.model.paused {
            app.model.paused = paused;
            gale_win::window::request_frame();
        }
        if app.sess.comparer.pos != oldpos || app.sess.comparer.wrong != oldwrong {
            app.anim.last_input = seconds;
            if app.sess.comparer.wrong > oldwrong {
                app.anim.wrong_at = seconds;
            } else {
                app.anim.correct_at = seconds;
            }
            gale_win::window::request_frame();
        }
        app.refresh_dynamic_hints(now);
        for cmd in gale_win::input_host::drain_cmds() {
            let a = match cmd {
                CommandKey::Escape => escape_action(
                    app.model.modal,
                    app.model.ledger_open,
                    app.model.analysis_dock,
                ),
                CommandKey::F1 => Some(Action::Open(Modal::Preferences)),
                CommandKey::F7 => Some(Action::OpenAppearanceSettings),
                CommandKey::F10 => Some(Action::ToggleAnalysis),
                CommandKey::F3 => Some(Action::Restart),
                CommandKey::F4 => Some(Action::ReadGroup),
                CommandKey::F5 => Some(Action::Open(Modal::Groups)),
                CommandKey::F8 => Some(Action::Open(Modal::Online)),
                CommandKey::F9 => Some(Action::Open(Modal::Rules)),
                // F2 发送当前文章；本地载入只保留剪贴板，F6 不绑定功能。
                CommandKey::F2 => Some(Action::PostArticle),
                CommandKey::F6 | CommandKey::RetryResult => None,
                CommandKey::LoadClipboard | CommandKey::AltE => Some(Action::LoadClipboard),
                CommandKey::CopyResult => Some(Action::CopyScore),
                CommandKey::SendResult => Some(Action::SendScore),
                CommandKey::ExportResult => Some(Action::ExportImage),
                CommandKey::CtrlLeft => Some(Action::Previous),
                CommandKey::CtrlRight => Some(Action::Next),
                CommandKey::Enter => {
                    completed_segment_action(cmd, app.sess.phase == Phase::Done, app.model.modal)
                }
                CommandKey::Space => {
                    completed_segment_action(cmd, app.sess.phase == Phase::Done, app.model.modal)
                }
            };
            if let Some(a) = a {
                app.act(a, false);
            }
        }
        if app.sess.phase == Phase::Done && !app.sess.recorded {
            if let Some(record) = app.sess.finalize_current_at(now, unix_secs()) {
                app.model.latest = Some(record.clone());
                let id = app.score_next;
                app.score_next += 1;
                app.model.history.push(record.clone());
                app.model.history_scroll = 0;
                app.pending_scores.push_back((id, record.clone()));
                if app.cfg.score_auto_copy {
                    let text = app.format_record(&record);
                    match unsafe { gale_win::clipboard_bridge::try_write_text(&text) } {
                        Ok(()) => app.toast("本段已记录 · 成绩已自动复制"),
                        Err(e) => app.toast(format!("本段已记录，自动复制失败：{e}")),
                    }
                } else {
                    app.toast("本段已记录 · Enter 下一段 / F3 重打");
                }
                // 【群三件套 2026-11-09】单段结算弹窗退役；绑定群且开自动发
                // 成绩时，打完一段把晴兼容成绩单直接发到群。
                if app.cfg.auto_send_score && app.model.bound.is_some() && record.can_send_score() {
                    // Send the round just settled, even while a history row is selected.
                    app.send_score_to_group(&record);
                }
                app.snapshot();
                // 整改2b：在线练习（正文来自文来）结算后 1.5s 自动取下一段；
                if app.online_auto_next
                    && app.online_logged_in
                    && app.online_article_current
                    && app.article.is_some()
                {
                    app.auto_next_at = Some(now + 1500);
                }
            }
        }
        // 整改2b：自动下一段到期触发。仍在成绩单上（用户未接管：未回车进赛报/
        // 未开其他面板）才按当前随机/顺序来源取文；collect_online 自动载入。
        if let Some(at) = app.auto_next_at {
            if now >= at {
                app.auto_next_at = None;
                if app.online_auto_next
                    && app.online_logged_in
                    && app.online_article_current
                    && app.article.is_some()
                    && matches!(app.model.modal, Modal::None)
                    && app.sess.phase == Phase::Done
                {
                    app.act(Action::OnlineFetch(1), false);
                }
            }
        }
        // 选群面板打开期间每 10s 刷新会话列表。
        let qq_screen = app.model.modal == Modal::Groups;
        if qq_screen && !app.qq_busy && now.saturating_sub(app.qq_scan_at) >= 10_000 {
            app.qq_scan_at = now;
            app.act(Action::ScanGroups, false);
        }
        // 整改6：Ctrl+滚轮字号防抖 800ms 后落盘。
        if let Some(due) = app.font_save_due {
            if now >= due {
                app.font_save_due = None;
                app.save_config();
            }
        }
        gale_win::input_host::set_round_done(app.sess.phase == Phase::Done);
        gale_win::input_host::set_result_key_target(
            (app.model.ledger_open && app.model.selected_history.is_some()).then_some(app.win.hwnd),
        );
        for e in gale_win::window::drain_mouse() {
            match e {
                MouseEv::LDown { x, y } => {
                    let xx = x as f32;
                    let yy = y as f32;
                    if let Some(a) = modals::hit(&app.hits, xx, yy) {
                        if a == Action::None {
                            continue;
                        }
                        app.capture_fields();
                        app.act(a, false);
                    } else if yy < 55. {
                        unsafe {
                            let _ = ReleaseCapture();
                            let _ = SendMessageW(
                                app.win.hwnd(),
                                WM_NCLBUTTONDOWN,
                                WPARAM(2),
                                LPARAM(0),
                            );
                        }
                    } else if app.model.modal == Modal::None {
                        if app.model.ledger_open || app.model.analysis_dock {
                            app.model.ledger_open = false;
                            app.model.analysis_dock = false;
                            gale_win::window::request_frame();
                        }
                        app.host.focus();
                        app.host.hide_caret();
                    }
                }
                MouseEv::Move { x, y } => {
                    let pos = Some((x as f32, y as f32));
                    if app.hover != pos {
                        app.hover = pos;
                        gale_win::window::request_frame();
                    }
                }
                MouseEv::LUp { .. } => {}
                MouseEv::Wheel { delta, x, y } => {
                    let sc = gale_win::window::dpi_scale();
                    let (cw, ch) = app.win.client_size();
                    let (lw, lh) = (cw as f32 / sc, ch as f32 / sc);
                    let mx = x as f32 / sc;
                    let my = y as f32 / sc;
                    // 理论分析停靠面板：指针在面板内时滚分析内容。
                    if app.model.analysis_dock {
                        let (ax, ay, aw, ah) = modals::analysis_dock_rect(lw, lh);
                        if mx >= ax && mx <= ax + aw && my >= ay && my <= ay + ah {
                            let max = modals::analysis_scroll_max(&app.sess, &app.model, lw, lh);
                            app.model.analysis_dock_scroll = (app.model.analysis_dock_scroll
                                - delta as f32 / 120. * 64.)
                                .clamp(0., max);
                            gale_win::window::request_frame();
                            continue;
                        }
                    }
                    // 成绩面板：指针在面板内时滚列表。
                    if app.model.ledger_open {
                        let (px, py, pw, ph) = history::panel_rect_for(&app.model, lw, lh);
                        if mx >= px && mx <= px + pw && my >= py && my <= py + ph {
                            let max = history::scroll_max(&app.model, lh);
                            app.model.ledger_scroll = (app.model.ledger_scroll
                                - delta as f32 / 120. * 64.)
                                .clamp(0., max);
                            gale_win::window::request_frame();
                            continue;
                        }
                    }
                    // Ctrl+滚轮 = 实时字号（10..=100，滚轮向上放大），不滚面板。
                    if unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0 {
                        let step = (delta / 120).clamp(-1, 1);
                        if step != 0 {
                            app.cfg.font_size = (app.cfg.font_size + step as f32).clamp(10., 100.);
                            app.apply_style();
                            app.font_save_due = Some(app.ms() + 800);
                            gale_win::window::request_frame();
                        }
                        continue;
                    }
                    let (_, _, _, ph) = modals::panel_for_model(&app.model, lw, lh);
                    let max = (ph - lh + 48.).max(0.);
                    let has_list = matches!(app.model.modal, Modal::Rules | Modal::Groups);
                    if app.model.modal != Modal::None
                        && max > 0.
                        && !(has_list
                            && ((delta < 0 && app.model.panel_scroll >= max)
                                || (delta > 0 && app.model.scroll > 0.)))
                    {
                        app.model.panel_scroll =
                            (app.model.panel_scroll - delta as f32 / 120. * 64.).clamp(0., max);
                    } else {
                        let cap = match app.model.modal {
                            Modal::Groups => app.model.groups.len().saturating_sub(7),
                            Modal::Rules => app.model.rules.len().saturating_sub(6),
                            _ => 0,
                        };
                        app.model.scroll =
                            (app.model.scroll - (delta as f32 / 120.) * 2.).clamp(0., cap as f32);
                    }
                    gale_win::window::request_frame();
                }
            }
        }
        let size = app.win.client_size();
        if size != last_size {
            last_size = size;
            gale_win::window::request_frame();
        }
        if app.sess.phase == Phase::Typing && !app.model.analysis_dock && now - stats_draw_at >= 250
        {
            stats_draw_at = now;
            gale_win::window::request_frame();
        }
        let animate = !app.model.reduced_motion && app.anim.active(seconds);
        let requested = gale_win::window::take_frame_request();
        if requested || animate {
            if last_draw.elapsed() >= Duration::from_millis(16) || requested {
                app.draw();
                last_draw = Instant::now();
            }
        }

        if app.snapshot_dirty && now.saturating_sub(app.last_snapshot_ms) >= 2000 {
            app.snapshot();
        }
        app.pump_persistence();
        let need_timer = animate
            || app.sess.phase == Phase::Typing
            || app.online_pending > 0
            || app.reg_code_until > 0
            || app.analyzing
            || app.dynamic_hint_due.is_some()
            || app.qq_busy
            || app.font_save_due.is_some()
            || app.auto_next_at.is_some()
            || !app.pending_scores.is_empty()
            || app.pending_snapshot.is_some()
            || app
                .model
                .toast
                .as_ref()
                .map(|(_, at)| seconds - at < 3.7)
                .unwrap_or(false);
        if need_timer && !timer {
            gale_win::window::start_frame_timer(app.win.hwnd);
            timer = true;
        } else if !need_timer && timer {
            gale_win::window::stop_frame_timer(app.win.hwnd);
            timer = false;
        }
        if !gale_win::window::has_frame_request() {
            let wait = if need_timer { 50 } else { 500 };
            gale_win::window::wait_idle(Some(wait.min(app.live_schema.wait_ms(now))));
        }
    }
    app.capture_fields();
    app.snapshot();
    app.flush_persistence();
    app.save_config();
    let (ack, done) = mpsc::channel();
    let _ = app.persist.send(Persist::Stop(ack));
    let saved = done
        .recv()
        .map_err(|_| "成绩保存线程未确认退出".to_string())?;
    gale_win::window::clear_size_rebuild_callback();
    gale_win::window::unregister_size_fill_renderer();
    saved.map_err(|e| format!("退出时有数据未保存：{e}"))?;
    Ok(())
}
impl App {
    fn fetch_skin(&mut self) {
        let tx = self.tx.clone();
        let hwnd = self.win.hwnd;
        let g = self.theme_generation;
        std::thread::spawn(move || {
            let _ = tx.send(Bg::Skin(g, skin::fetch(local_port())));
            wake(hwnd);
        });
    }
    fn ms(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }
    fn toast(&mut self, s: impl Into<String>) {
        let n = self.clock.elapsed().as_secs_f64();
        self.model.toast(s, n);
        gale_win::window::request_frame();
    }
    fn save_config(&mut self) {
        // 用户手调过的窗口大小/位置随配置落盘；下次启动原样恢复。
        if let Some((x, y, w, h)) = self.win.window_rect() {
            let sc = gale_win::window::dpi_scale();
            if sc > 0. {
                self.cfg.win_x = x;
                self.cfg.win_y = y;
                self.cfg.win_position_physical = true;
                self.cfg.win_w = (w as f32 / sc).round() as i32;
                self.cfg.win_h = (h as f32 / sc).round() as i32;
            }
        }
        self.cfg.session_rules = self.sess.rules.clone();
        let mut saved = self.cfg.clone();
        if self.model.rule_save_preset {
            self.saved_cleaning = self.cfg.cleaning_preset.clone();
        } else {
            saved.cleaning_preset = self.saved_cleaning.clone();
        }
        self.write_sync_profile(&saved);
    }

    /// 把跟打器配置与晴账号写成云同步用的 profile.json（明文，走用户自己的 WebDAV）。
    fn write_sync_profile(&mut self, cfg: &Config) {
        // Optional sync failure must not discard a local settings change.
        if let Err(e) = cfg.save(&self.dir.join("config.toml")) {
            self.toast(format!("保存本机跟打器设置失败：{e}"));
            return;
        }
        let account =
            crate::online::export_account(&self.dir, &cfg.wenlai_domain, self.sync_account_cleared);
        match self.sync_profile.save(
            &self.dir,
            cfg,
            &self.sync_online_user,
            &self.sync_online_password,
            account,
            self.sync_identity_dirty,
        ) {
            Ok(()) => self.sync_identity_dirty = false,
            Err(e) => self.toast(e),
        }
    }
    fn snapshot_bytes(&self) -> Option<Vec<u8>> {
        self.sess.snapshot_bytes_bounded().ok()
    }
    fn snapshot(&mut self) {
        self.last_snapshot_ms = self.ms();
        if let Some(bytes) = self.snapshot_bytes() {
            self.pending_snapshot = Some(bytes);
            self.snapshot_dirty = false;
            self.last_snapshot_ms = self.ms();
        } else {
            self.toast("恢复快照超过限制，文章仍保留在当前窗口");
        }
    }
    fn pump_persistence(&mut self) {
        if !self.score_inflight && self.ms() >= self.score_retry_ms {
            if let Some((id, r)) = self.pending_scores.front() {
                match self.persist.try_send(Persist::Score(*id, r.clone())) {
                    Ok(()) => self.score_inflight = true,
                    Err(mpsc::TrySendError::Full(_)) => {}
                    Err(_) => {
                        self.score_retry_ms = self.ms() + 2000;
                        self.toast("成绩保存线程不可用，请复制当前成绩");
                    }
                }
            }
        }
        // The session checkpoint is independently writable even when the score
        // directory is blocked. Both jobs still use the same ordered worker.
        if let Some(bytes) = self.pending_snapshot.take() {
            match self.persist.try_send(Persist::Snapshot(bytes)) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(Persist::Snapshot(b))) => {
                    self.pending_snapshot = Some(b)
                }
                Err(_) => self.toast("恢复快照保存线程不可用"),
            }
        }
    }
    fn flush_persistence(&mut self) {
        while let Some((id, r)) = self.pending_scores.pop_front() {
            let _ = self.persist.send(Persist::Score(id, r));
        }
        if let Some(b) = self.pending_snapshot.take() {
            let _ = self.persist.send(Persist::Snapshot(b));
        }
    }
    fn reset_host(&mut self) {
        self.snapshot_dirty = true;
        self.model.latest = None;
        self.host.reset_text();
        self.anim.caret = None;
        self.model.words.clear();
        // Switching segments used to clear the prefetched whole-book hints and
        // wait for another service round-trip. Re-slice the cached words first;
        // The independent status poll refreshes the schema in the background.
        self.refill_from_full_words();
        self.generation = self.generation.wrapping_add(1);
        self.full_retry.reset();
        self.analyze();
        gale_win::window::request_frame();
    }
    /// Re-slice `full_words` for the current segment when the cache still covers
    /// the whole article under the active schema. Returns true when it applied.
    fn refill_from_full_words(&mut self) -> bool {
        if !self.last_analyze_whole
            || self.full_words.is_empty()
            || self.full_words_of != self.analysis_text()
            || self.full_words_breaks != self.analysis_breaks()
            || self.full_words_strategy != self.cfg.hint_strategy
            || self.full_words_required_spans != self.analysis_required_spans()
            || !self.full_words_auto4
            || self.full_words_dynamic_boundary != self.analysis_dynamic_boundary()
            || self.full_words_ver != self.model.schema_ver
        {
            return false;
        }
        self.refill_segment_words();
        true
    }
    /// 整改2a：在线文章直达跟打区（预览流程退役的唯一载入口）。
    /// The online worker filters superseded requests before reaching here.
    fn load_online_article(&mut self) {
        let Some(a) = self.article.clone() else {
            self.toast("请先取文");
            return;
        };
        if a.text.chars().count() > 100000 {
            self.toast("在线正文超过100000字，请重新取较短文段");
            return;
        }
        self.sess.load_group(gale_core::segment::ParsedGroupText {
            title: a.title,
            // P2 难度带分值（任务 6d）：文来 API 的 difficulty_score（0=无分值）
            // 拼成 "普 1.84" 风格——DifficultyTag::parse 在 session.load_group 内
            // 解析，结算进成绩单 难度普(1.84)。
            difficulty: Some(if a.difficulty_score > 0. {
                format!("{} {}", a.difficulty.label(), a.difficulty_score)
            } else {
                a.difficulty.label().to_string()
            }),
            segments: vec![gale_core::segment::Segment {
                mark: a.seg_mark,
                text: a.text,
            }],
        });
        self.online_article_current = true;
        self.model.ledger_open = false;
        self.model.selected_history = None;
        self.reset_host();
        self.open(Modal::None);
        self.snapshot();
        // 选了群：在线取文只要进了跟打区就自动发到群里。
        if self.model.bound.is_some() {
            self.act(Action::PostArticle, false);
        }
    }
    fn sync_preferences(&mut self, editors: bool) {
        for (key, value) in [
            ("hint_show_code", self.cfg.hint_show_code),
            ("hint_show_rank", self.cfg.hint_show_rank),
            ("hint_color_words", self.cfg.hint_color_words),
            ("hint_rank_colors", self.cfg.hint_rank_colors),
            ("hint_dynamic", self.cfg.hint_dynamic),
            ("hint_underline", self.cfg.hint_underline),
            ("hint_word_bg", self.cfg.hint_word_bg),
            ("score_auto_copy", self.cfg.score_auto_copy),
            ("auto_send_score", self.cfg.auto_send_score),
        ] {
            self.model.fields.insert(key.into(), value.to_string());
        }
        self.model.fields.insert(
            "hint_strategy".into(),
            self.cfg.hint_strategy.as_str().into(),
        );
        self.model.fields.insert(
            "hint_settings_locked".into(),
            self.sess.frozen_rules.is_some().to_string(),
        );
        self.model.fields.insert(
            "hint_font_size".into(),
            format!("{:.0}", self.cfg.hint_font_size),
        );
        self.model
            .fields
            .insert("score_fields".into(), self.cfg.score_fields.clone());
        self.model.fields.insert(
            "score_preset".into(),
            current_score_preset(&self.cfg).into(),
        );
        for key in PREFERENCE_COLORS
            .iter()
            .chain(ui::HINT_RANK_COLOR_KEYS.iter())
        {
            let c = ui::setting_color(&self.theme, key);
            self.model.fields.insert(
                format!("color_{key}"),
                format!(
                    "{:02x}{:02x}{:02x}",
                    (c[0] * 255.).round() as u8,
                    (c[1] * 255.).round() as u8,
                    (c[2] * 255.).round() as u8
                ),
            );
        }
        if editors {
            self.model
                .fields
                .insert("score_template".into(), configured_template(&self.cfg));
            modals::preferences::sync_color_editor(&mut self.model);
        }
    }
    fn preference_action(&mut self, command: &str) {
        self.cfg.score_fields = canonical_score_fields(&self.cfg.score_fields);
        let parts: Vec<_> = command.split(':').collect();
        let route_change = matches!(
            parts.as_slice(),
            ["pref_strategy", _] | ["pref_toggle", "hint_dynamic"]
        );
        if route_change && self.sess.frozen_rules.is_some() {
            self.toast("本轮规则已冻结，请先重打再修改词提策略");
            return;
        }
        match parts.as_slice() {
            ["pref_strategy", strategy] => {
                self.cfg.hint_strategy = HintStrategy::from_str_lossy(strategy);
            }
            ["pref_toggle", key] => {
                let field = match *key {
                    "hint_show_code" => &mut self.cfg.hint_show_code,
                    "hint_show_rank" => &mut self.cfg.hint_show_rank,
                    "hint_color_words" => &mut self.cfg.hint_color_words,
                    "hint_rank_colors" => &mut self.cfg.hint_rank_colors,
                    "hint_dynamic" => &mut self.cfg.hint_dynamic,
                    "hint_underline" => &mut self.cfg.hint_underline,
                    "hint_word_bg" => &mut self.cfg.hint_word_bg,
                    "score_auto_copy" => &mut self.cfg.score_auto_copy,
                    "auto_send_score" => &mut self.cfg.auto_send_score,
                    _ => return,
                };
                *field = !*field;
            }
            ["pref_step", "hint_font_size", delta] => {
                if let Ok(delta) = delta.parse::<f32>() {
                    self.cfg.hint_font_size = (self.cfg.hint_font_size + delta).clamp(14., 28.);
                }
            }
            ["pref_tab", tab] => {
                if let Ok(tab) = tab.parse::<u8>() {
                    self.sync_preferences(false);
                    modals::preferences::select_page(&mut self.model, tab);
                }
                return;
            }
            ["pref_hint_view", view @ ("settings" | "colors")] => {
                self.sync_preferences(false);
                modals::preferences::select_hint_view(&mut self.model, view);
                return;
            }
            ["pref_color_choose", key] if modals::preferences::color_allowed(&self.model, key) => {
                self.model
                    .fields
                    .insert("pref_color_key".into(), (*key).into());
                let c = ui::setting_color(&self.theme, key);
                let current = (
                    (c[0] * 255.).round() as u8,
                    (c[1] * 255.).round() as u8,
                    (c[2] * 255.).round() as u8,
                );
                let Some((r, g, b)) = gale_win::color_picker::choose_color(self.win.hwnd, current)
                else {
                    return;
                };
                self.theme.set(key, gale_core::theme::Color(r, g, b, 255));
                self.cfg
                    .color_overrides
                    .insert((*key).into(), format!("{r:02x}{g:02x}{b:02x}"));
                self.apply_style();
            }
            ["pref_score_preset", id] => {
                match *id {
                    "standard" => {
                        self.cfg.score_template.clear();
                        self.cfg.score_fields = gale_core::scorecard::STANDARD_SCORE_FIELDS.into();
                        self.toast("已恢复标准成绩单：速度、键准、击键、码长");
                    }
                    "compact" | "classic" | "detailed" => {
                        self.cfg.score_template = gale_core::scorecard::preset(id).template.into();
                        self.toast("已切换成绩单预设");
                    }
                    "custom" => {}
                    _ => return,
                }
                self.model
                    .fields
                    .insert("score_preset".into(), (*id).into());
                self.model
                    .fields
                    .insert("score_template".into(), configured_template(&self.cfg));
                self.model.modal_epoch += 1;
                if *id == "custom" {
                    // Free editing is a view state, not a saved config value;
                    // returning early keeps the highlight until the user saves
                    // or picks a preset, instead of being re-inferred away.
                    return;
                }
            }
            ["pref_save_template"] => {
                self.cfg.score_template = self.model.field("score_template");
                self.toast("成绩单模板已保存");
            }
            ["pref_field", key] => {
                if !SCORE_FIELD_LABELS
                    .iter()
                    .chain(gale_core::scorecard::SCORE_FIELDS.iter())
                    .any(|(k, _)| k == key)
                {
                    return;
                }
                let mut fields: Vec<_> = self
                    .cfg
                    .score_fields
                    .split(',')
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
                    .collect();
                if fields.iter().any(|v| v == key) {
                    fields.retain(|v| v != key);
                } else {
                    fields.push((*key).into());
                }
                self.cfg.score_fields = fields.join(",");
                let cleared_template = !self.cfg.score_template.trim().is_empty();
                self.cfg.score_template.clear();
                if cleared_template {
                    self.toast("已切回字段模式 · 自定义模板已清除");
                }
                self.model
                    .fields
                    .insert("score_template".into(), configured_template(&self.cfg));
                self.model
                    .fields
                    .insert("score_preset".into(), "standard".into());
                self.model.modal_epoch += 1;
            }
            _ => return,
        }
        if route_change {
            self.generation = self.generation.wrapping_add(1);
            self.analyzed = 0;
            self.full_words.clear();
            self.full_words_ver.clear();
            self.model.words.clear();
            self.sess.full_analysis = None;
            self.full_fetch_hash = None;
            self.full_retry.reset();
            self.model.fields.remove("analysis_failed_key");
            self.route_cache.clear();
            self.analyze();
        }
        self.sync_preferences(false);
        modals::preferences::sync_color_editor(&mut self.model);
        self.save_config();
    }
    fn format_record(&self, record: &gale_core::score::ScoreRecord) -> String {
        gale_core::scorecard::format_score_template(
            record,
            &configured_template(&self.cfg),
            &self.cfg.score_signature,
        )
    }
    fn analysis_text(&self) -> String {
        self.sess.segments.iter().map(|s| s.text.as_str()).collect()
    }
    fn analysis_breaks(&self) -> Vec<usize> {
        segment_analysis_breaks(&self.sess.segments)
    }
    fn analysis_auto4(&self) -> bool {
        self.model.field("analysis_auto4") != "false"
    }
    fn analysis_required_spans(&self) -> Vec<RequiredSpan> {
        Vec::new()
    }
    fn analysis_dynamic_boundary(&self) -> Option<usize> {
        self.cfg.hint_dynamic.then(|| {
            let base: usize = self.sess.segments[..self.sess.current_index()]
                .iter()
                .map(|segment| segment.text.chars().count())
                .sum();
            base + dynamic_prefix_boundary(&self.sess)
        })
    }
    fn refresh_dynamic_hints(&mut self, now: u64) {
        let boundary = self.analysis_dynamic_boundary();
        if dynamic_hint_refresh_due(
            boundary,
            &mut self.dynamic_boundary_seen,
            &mut self.dynamic_hint_due,
            now,
        ) {
            self.analyze();
        }
    }
    fn hint_analysis_input(&self) -> (String, Vec<usize>, Vec<RequiredSpan>, Option<usize>, bool) {
        let text = self.analysis_text();
        if text.chars().count() <= 9500 {
            (
                text,
                self.analysis_breaks(),
                self.analysis_required_spans(),
                self.analysis_dynamic_boundary(),
                true,
            )
        } else {
            (
                self.sess.current_segment().text.clone(),
                Vec::new(),
                Vec::new(),
                self.cfg
                    .hint_dynamic
                    .then(|| dynamic_prefix_boundary(&self.sess)),
                false,
            )
        }
    }
    fn hint_analysis_key(&self) -> String {
        let (text, breaks, required_spans, dynamic_boundary, _) = self.hint_analysis_input();
        analysis_request_key(
            &text,
            &breaks,
            self.sess.source == SessionSource::Single,
            &self.model.schema_ver,
            true, // Both recovered display wrappers use four-code first candidates.
            self.cfg.hint_strategy,
            &required_spans,
            dynamic_boundary,
            "display",
        )
    }
    fn analysis_key(&self) -> String {
        analysis_request_key(
            &self.analysis_text(),
            &self.analysis_breaks(),
            self.sess.source == SessionSource::Single,
            &self.model.schema_ver,
            self.analysis_auto4(),
            self.cfg.hint_strategy,
            &[],
            None,
            "theory",
        )
    }
    /// Serialize with ordinary hints. Failed requests remain retryable and only
    /// a successful result is cached; generation changes alone never poison it.
    fn fetch_full_analysis(&mut self) {
        if !self.model.analysis_dock
            || self.analyzing
            || self.full_analyzing
            || !self.live_schema.ready()
        {
            return;
        }
        if let Some(reason) = &self.live_schema.revision.unavailable_reason {
            self.model
                .fields
                .insert("analysis_loading".into(), "false".into());
            self.model
                .fields
                .insert("analysis_error".into(), reason.clone());
            return;
        }
        let full = self.analysis_text();
        if full.trim().is_empty() {
            return;
        }
        let key = self.analysis_key();
        if self.model.field("analysis_failed_key") != key {
            self.full_retry.reset();
        }
        if !self.full_retry.ready(self.ms()) {
            return;
        }
        if self.full_fetch_hash.as_deref() == Some(&key) && self.sess.full_analysis.is_some() {
            return;
        }
        self.full_analyzing = true;
        self.sess.full_analysis = None;
        self.model
            .fields
            .insert("analysis_loading".into(), "true".into());
        self.model.fields.remove("analysis_error");
        let single = self.sess.source == SessionSource::Single;
        let auto4 = self.analysis_auto4();
        let breaks = self.analysis_breaks();
        let strategy = self.cfg.hint_strategy;
        let revision = self.live_schema.revision.clone();
        let tx = self.tx.clone();
        let hwnd = self.win.hwnd;
        std::thread::spawn(move || {
            let result = fetch_theory(&full, single, auto4, &breaks, strategy);
            let _ = tx.send(Bg::FullAnalysis(key, revision, result));
            wake(hwnd);
        });
    }
    fn toggle_analysis_dock(&mut self) {
        if self.model.analysis_dock {
            self.model.analysis_dock = false;
            gale_win::window::request_frame();
            return;
        }
        self.model.modal = Modal::None;
        self.model.ledger_open = false;
        self.model.selected_history = None;
        self.model.analysis_dock = true;
        self.model.analysis_dock_scroll = 0.;
        self.full_retry.reset();
        self.analyzed = 0;
        self.analyze();
        self.fetch_full_analysis();
        gale_win::window::request_frame();
    }

    /// 整改12收口：打开当前段落键盘热力图模态（数据=最近结算段
    /// last_vk_histogram；尚无记录时提示而非弹空图）。
    fn show_heatmap(&mut self) {
        if self.sess.last_vk_histogram.is_empty() {
            self.toast("还没有按键记录——打完一段结算后可看热力图");
            return;
        }
        self.model.modal = Modal::Heatmap;
        self.model.modal_epoch += 1;
    }

    /// 词提颜色循环选色。白名单外键拒绝（防伪造 aux 串）；
    /// 当前色不在色板内（主题派生色）则跳到首色。即时生效+落盘。
    fn cycle_hint_color(&mut self, key: &str) -> bool {
        const SWATCH: [&str; 12] = [
            "e7b75c", "63c08d", "7fb0e8", "e08a6a", "c88fd8", "8ad0c0", "e0d06a", "8a9ae0",
            "e0746a", "a8d46a", "6ad4d4", "f0ede6",
        ];
        if !matches!(
            key,
            "词提编码色"
                | "词提角标色"
                | "词提下划线色"
                | "词提底色"
                | "词提一码色"
                | "词提简码色"
                | "词提二字三码色"
                | "词提二字全码色"
                | "词提三字全码色"
                | "词提四字全码色"
                | "多字词一色"
                | "多字词二色"
                | "多字词三色"
        ) {
            return false;
        }
        let cur = self.theme.get(key);
        let cur_hex = format!("{:02x}{:02x}{:02x}", cur.0, cur.1, cur.2);
        let next = match SWATCH.iter().position(|s| *s == cur_hex) {
            Some(i) => SWATCH[(i + 1) % SWATCH.len()],
            None => SWATCH[0],
        };
        let Some(c) = gale_core::theme::Color::from_hex_no_hash(next) else {
            return false;
        };
        self.theme.set(key, c);
        self.cfg.color_overrides.insert(key.into(), next.into());
        self.apply_style();
        self.save_config();
        true
    }

    /// 整改14：把全文词提（full_words，全文坐标）切成当前段子集（段内坐标）
    /// 填给渲染层——换段时零网络延迟，全文词提一次到位。
    fn refill_segment_words(&mut self) {
        let idx = self.sess.current_index();
        let base: usize = self.sess.segments[..idx]
            .iter()
            .map(|s| s.text.chars().count())
            .sum();
        let n = self.sess.current_segment().text.chars().count();
        let mut v: Vec<modals::HintWord> = self
            .full_words
            .iter()
            .filter(|w| w.start >= base && w.len > 0 && w.start + w.len <= base + n)
            .map(|w| {
                let mut c = w.clone();
                c.start -= base;
                c
            })
            .collect();
        v.sort_by_key(|w| w.start);
        self.model.words = v;
    }
    fn select_history(&mut self, index: usize) {
        if index >= self.model.history.len() {
            return;
        }
        self.auto_next_at = None;
        self.model.modal = Modal::None;
        self.model.analysis_dock = false;
        self.model.ledger_open = true;
        self.model.selected_history = Some(index);
        let sc = gale_win::window::dpi_scale();
        let (_, h) = self.win.client_size();
        let lh = h as f32 / sc;
        let offset = crate::history::record_offset(&self.model, index);
        self.model.ledger_scroll = offset.min(crate::history::scroll_max(&self.model, lh));
        gale_win::window::request_frame();
    }

    /// 私有标记动作执行（aux_action 解码后）
    fn run_aux(&mut self, aux: AuxAction) {
        match aux {
            AuxAction::Preference(command) => self.preference_action(&command),
            AuxAction::HistoryToggle => {
                self.auto_next_at = None;
                self.model.analysis_dock = false;
                self.model.ledger_open = !self.model.ledger_open;
                if !self.model.ledger_open {
                    self.model.selected_history = None;
                }
            }
            AuxAction::HistoryStep(delta) => {
                let sc = gale_win::window::dpi_scale();
                let (_, h) = self.win.client_size();
                let lh = h as f32 / sc;
                let cap = crate::history::scroll_max(&self.model, lh);
                self.model.ledger_scroll = (self.model.ledger_scroll + delta as f32).clamp(0., cap);
            }
            AuxAction::HistoryOpen(index) => {
                self.select_history(index);
            }
            AuxAction::OnlineRegister => {
                // 登录卡 ↔ 注册卡切换（原整改2c开站点入口退役：晴站注册
                // API 已实装 /api/auth/register*，见 wenlai.rs 与 online.rs）。
                self.model.online_register_mode = !self.model.online_register_mode;
                self.model.modal_epoch += 1; // 编辑控件集不同，强制重建
                self.model.online_status = if self.model.online_register_mode {
                    "填写用户名、密码与邮箱验证码注册".into()
                } else {
                    "登录后收起此页，可按分类、难度取文。".into()
                };
            }
            AuxAction::OnlineRegisterCode => {
                let now = self.ms();
                if self.reg_code_until > now {
                    self.toast("验证码已发送，请稍候再试");
                    return;
                }
                let email = self.model.field("reg_email").trim().to_string();
                if email.is_empty() || !email.contains('@') {
                    self.toast("请输入正确的邮箱地址");
                    return;
                }
                self.submit_online(OnlineCommand::RegisterSendCode { email });
            }
            AuxAction::OnlineRegisterSubmit => {
                let username = self.model.field("username");
                let password = self.model.field("password");
                let email = self.model.field("reg_email").trim().to_string();
                let code = self.model.field("reg_code").trim().to_string();
                if username.trim().is_empty() {
                    self.toast("请输入用户名");
                    return;
                }
                if password.is_empty() {
                    self.toast("请输入密码");
                    return;
                }
                if email.is_empty() || !email.contains('@') {
                    self.toast("请输入正确的邮箱地址");
                    return;
                }
                if code.is_empty() {
                    self.toast("请输入邮箱验证码");
                    return;
                }
                self.cfg.wenlai_username = username.clone();
                self.submit_online(OnlineCommand::Register {
                    username,
                    password,
                    email,
                    code,
                });
                // 与 OnlineLogin 同口径：密码提交后立即从输入框清除。
                self.model.fields.insert("password".into(), String::new());
                if let Some(e) = self.edits.get("password") {
                    e.set_text("");
                }
            }
            AuxAction::OnlineAutoNext => {
                self.online_auto_next = !self.online_auto_next;
                let prefs = OnlinePrefs {
                    auto_next: self.online_auto_next,
                };
                if let Err(e) = prefs.save(&self.dir.join("online.json")) {
                    self.toast(format!("在线偏好保存失败：{e}"));
                }
                self.model
                    .fields
                    .insert("online_auto_next".into(), self.online_auto_next.to_string());
                if !self.online_auto_next {
                    self.auto_next_at = None;
                }
            }
            AuxAction::ShowFullAnalysis => self.toggle_analysis_dock(),
            AuxAction::ShowHeatmap => self.show_heatmap(),
            AuxAction::AnalysisAuto4 => {
                let next = !self.analysis_auto4();
                self.model
                    .fields
                    .insert("analysis_auto4".into(), next.to_string());
                self.full_retry.reset();
                self.full_fetch_hash = None;
                self.sess.full_analysis = None;
                self.fetch_full_analysis();
            }
            AuxAction::AnalysisFont(size) => {
                self.model
                    .fields
                    .insert("analysis_font".into(), size.to_string());
            }
            AuxAction::AnalysisView => {
                let next = if self.model.field("analysis_view") == "heatmap" {
                    "article"
                } else {
                    "heatmap"
                };
                self.model
                    .fields
                    .insert("analysis_view".into(), next.into());
                self.model.analysis_dock_scroll = 0.;
            }
            AuxAction::AnalysisRetry => {
                self.full_retry.reset();
                self.full_fetch_hash = None;
                self.fetch_full_analysis();
            }
            AuxAction::AnalysisCopy => {
                self.draw();
                let sc = gale_win::window::dpi_scale();
                let (w, h) = self.win.client_size();
                let (x, y, pw, ph) = modals::analysis_dock_rect(w as f32 / sc, h as f32 / sc);
                let crop = Some((
                    (x * sc) as u32,
                    (y * sc) as u32,
                    (pw * sc) as u32,
                    (ph * sc) as u32,
                ));
                match self.renderer.copy_bmp_to_clipboard(crop) {
                    Ok(()) => self.toast("当前分析图片已复制"),
                    Err(error) => self.toast(error),
                }
            }
        }
    }
    /// 发文/发成绩会激活 QQ；发送结束（成功或失败）都切回跟打器。
    fn focus_trainer(&mut self) {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{BringWindowToTop, SetForegroundWindow};
            let hwnd = windows::Win32::Foundation::HWND(self.win.hwnd as *mut core::ffi::c_void);
            let _ = SetForegroundWindow(hwnd);
            let _ = BringWindowToTop(hwnd);
        }
        self.host.focus();
        self.host.hide_caret();
        gale_win::window::request_frame();
    }

    /// 后台发送 QQ：先让最后一帧画完（300ms），工作线程粘贴+回车并等
    /// 消息确认，完成后通过 Bg::Sent 回主线程再切回跟打器。绝不阻塞输入/结算。
    fn spawn_send(&mut self, chat: qq_bridge::ChatWindow, text: String, ok_label: String) {
        if self.send_busy {
            self.toast("正在发送上一条，请稍候");
            return;
        }
        self.send_busy = true;
        self.toast(format!("正在发送到「{}」…", chat.title));
        let tx = self.tx.clone();
        let hwnd = self.win.hwnd;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let result = qq_bridge::send_message_enhanced(&chat, &text).map_err(|e| e.to_string());
            let _ = tx.send(Bg::Sent {
                label: ok_label,
                result,
            });
            wake(hwnd);
        });
    }

    fn apply_style(&mut self) {
        let bg = ui::rgba(self.theme.get("窗体背景色"));
        let fg = ui::rgba(self.theme.get("窗体字体色"));
        let native = NativeEdit {
            hwnd: self.host.hwnd,
        };
        native.style(bg, bg, 1.);
        let _ = fg;
        std::mem::forget(native);
        self.renderer.set_resize_fill(bg);
        let _ = gale_win::window::apply_material(&gale_win::window::MaterialParams {
            hwnd: self.win.hwnd,
            material: gale_core::config::Material::Solid,
            tint_rgb: (
                (bg[0] * 255.) as u8,
                (bg[1] * 255.) as u8,
                (bg[2] * 255.) as u8,
            ),
            opacity: self.cfg.opacity,
        });
    }
    fn capture_fields(&mut self) -> bool {
        let font_changed =
            if let Some(family) = self.font_picker.as_mut().and_then(|p| p.take_selection()) {
                self.cfg.font_family = family;
                self.save_config();
                true
            } else {
                false
            };
        // Old controls must not overwrite a newly loaded draft before draw()
        // rebuilds the controls for the updated model.
        if self.edit_epoch != self.model.modal_epoch {
            return font_changed;
        }
        let mut changed = font_changed;
        for (k, e) in &self.edits {
            let text = e.text();
            if self.model.field(k) != text {
                changed = true;
                self.model.fields.insert(k.clone(), text);
            }
        }
        changed
    }
    fn open(&mut self, m: Modal) {
        self.capture_fields();
        if m == Modal::Preferences {
            self.sync_preferences(true);
        }
        if m != Modal::None {
            self.model.ledger_open = false;
            self.model.analysis_dock = false;
        }
        self.model.open(m);
        gale_win::input_host::set_result_key_target(
            (self.model.ledger_open && self.model.selected_history.is_some())
                .then_some(self.win.hwnd),
        );
        gale_win::window::set_focus_relay(m == Modal::None);
        if m != Modal::None {
            self.host.hide();
            unsafe {
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(self.win.hwnd());
            }
        } else {
            self.edits.clear();
            self.host.show();
            self.host.focus();
            self.host.hide_caret();
        }
        let now = self.ms();
        let dispatched = dispatch_host_events(&mut self.sess, self.host.poll(), false, now);
        dispatched.apply_commit_causes(&mut self.sess, gale_win::input_host::drain_commit_causes());
        if m != Modal::None {
            self.sess.set_paused(true, now);
        }
        self.model.paused = self.sess.is_paused();
        gale_win::window::request_frame();
    }
    fn sync_rule_model(&mut self) {
        self.model.rules = self
            .cfg
            .cleaning_preset
            .rules
            .iter()
            .map(|r| {
                (
                    r.name.clone(),
                    r.enabled,
                    !r.safe,
                    r.pattern.clone(),
                    r.replacement.clone(),
                )
            })
            .collect();
        let i = self
            .model
            .rule_selected
            .min(self.cfg.cleaning_preset.rules.len().saturating_sub(1));
        self.model.rule_selected = i;
        if let Some(r) = self.cfg.cleaning_preset.rules.get(i) {
            self.model.fields.insert("rule_name".into(), r.name.clone());
            self.model
                .fields
                .insert("rule_pattern".into(), r.pattern.clone());
            self.model
                .fields
                .insert("rule_replacement".into(), r.replacement.clone());
        } else {
            for n in ["rule_name", "rule_pattern", "rule_replacement"] {
                self.model.fields.insert(n.into(), String::new());
            }
        }
        self.edit_epoch = u64::MAX;
    }
    fn draw(&mut self) {
        if self.renderer.fonts.set_body_family(&self.cfg.font_family) {
            self.layout_key = (u64::MAX, 0, 0);
        }
        let (w, h) = self.win.client_size();
        if w <= 0 || h <= 0 {
            return;
        }
        if self.renderer.width != w as f32 || self.renderer.height != h as f32 {
            self.renderer.resize_lossy(w as u32, h as u32);
        }
        let sc = gale_win::window::dpi_scale();
        let (lw, lh) = (w as f32 / sc, h as f32 / sc);
        let now = self.clock.elapsed().as_secs_f64();
        let (_, _, _, ph) = modals::panel_for_model(&self.model, lw, lh);
        self.model.panel_scroll = self
            .model
            .panel_scroll
            .clamp(0., (ph - (lh - 48.).min(ph)).max(0.));
        let (bw, fs, base_lh) = ui::body_metrics(&self.sess, &self.cfg, lw);
        let hints_visible =
            self.model.hints && (self.cfg.hint_show_code || self.cfg.hint_show_rank);
        let lheight = ui::line_height_for_anchor(&self.cfg, fs, base_lh, hints_visible);
        let key = (self.generation, fs.to_bits(), bw.to_bits());
        if self.layout_key != key {
            self.layout = self
                .renderer
                .wrap_text(&self.sess.current_segment().text, fs, bw);
            self.layout_key = key;
        }
        if self.model.hints {
            let boundaries: Vec<_> = self
                .model
                .words
                .iter()
                .filter(|word| word.code.is_some() && analysis::is_multi_word(&word.text))
                .map(|word| (word.start, word.len))
                .collect();
            if let Some(lines) = gale_win::d2d::rewrap_words_to_width(&self.layout, &boundaries, bw)
            {
                self.layout = lines;
            }
        }
        let mut f = ui::build(
            &self.sess,
            &self.cfg,
            &self.theme,
            &self.model,
            lw,
            lh,
            self.ms(),
            now,
            &mut self.anim,
            &mut self.hits,
            &self.layout,
        );
        modals::draw(
            &mut f,
            &mut self.hits,
            &self.model,
            &self.sess,
            &self.cfg,
            &self.theme,
            lw,
            lh,
            now,
        );
        if let Some((mx, my)) = self.hover {
            if let Some(a) = modals::hit(&self.hits, mx, my) {
                if a != Action::None {
                    if let Some(h) = self.hits.iter().rev().find(|h| {
                        h.action == a && mx >= h.x && mx < h.x + h.w && my >= h.y && my < h.y + h.h
                    }) {
                        if h.w <= 920. && h.h <= 56. && h.w > 12. {
                            let palette = ui::tokens(&self.theme);
                            let dark = palette.bg[0] * 0.2126
                                + palette.bg[1] * 0.7152
                                + palette.bg[2] * 0.0722
                                < 0.5;
                            let tint = if dark {
                                [1., 1., 1., 0.065]
                            } else {
                                [0., 0., 0., 0.055]
                            };
                            let radius = (h.h * 0.25).clamp(3., 8.);
                            ui::round(
                                &mut f,
                                h.x + 1.,
                                h.y + 1.,
                                (h.w - 2.).max(0.),
                                (h.h - 2.).max(0.),
                                radius,
                                tint,
                                None,
                            );
                        }
                    }
                }
            }
        }
        if let Err(e) = self.renderer.draw_frame(&f) {
            self.toast(format!("绘制失败：{e}"));
        }
        if self.model.modal == Modal::None {
            gale_win::window::set_focus_relay(true);
            // Keep a visible, focused native TSF host at the painted caret.
            // It has a real client surface; text itself is drawn by DirectWrite.
            let (cx, cy) = self.anim.caret.unwrap_or((32., 270.));
            self.host.place(
                (cx.clamp(8., lw - 8.) * sc) as i32,
                ((cy + fs).clamp(60., lh - 50.) * sc) as i32,
                2,
                2,
            );
            // P2 组字窗/候选窗锚点（任务 8）：把绘制光标坐标 + 当前行行高
            //（DIP→物理像素 ×DPI）每帧下发给 IME 锚点（内部 3px 去抖）；
            // NativeEdit 2×2 贴层保留不动（双层：光标载体 + IME 锚点）。
            // 光标矩形顶 = 行顶 + 5（ui.rs caret 绘制几何）。
            let dpi = (sc * 96.).round() as u32;
            let _ = self.host.set_anchor_from_caret(
                cx.clamp(8., lw - 8.) * sc,
                (cy + 5.).clamp(60., lh - 50.) * sc,
                (lheight * sc).round() as i32,
                dpi,
            );
            self.host.show();
            self.host.hide_caret();
        }
        let specs = modals::fields(&self.model, lw, lh);
        if self.edit_epoch != self.model.modal_epoch {
            self.edits.clear();
            for z in &specs {
                match NativeEdit::new(self.win.hwnd, z.multi, z.password) {
                    Ok(e) => {
                        e.set_text(&self.model.field(z.id));
                        self.edits.insert(z.id.into(), e);
                    }
                    Err(e) => self.toast(format!("编辑框不可用：{e}")),
                }
            }
            if self.font_picker.is_none()
                && self.model.modal == Modal::Preferences
                && self.model.field("pref_tab") == "2"
            {
                match gale_win::font_picker::NativeFontPicker::new(
                    self.win.hwnd,
                    &self.cfg.font_family,
                ) {
                    Ok(picker) => self.font_picker = Some(picker),
                    Err(error) => self.toast(error),
                }
            }
            self.edit_epoch = self.model.modal_epoch;
            if let Some(z) = specs.first() {
                if let Some(e) = self.edits.get(z.id) {
                    e.focus();
                }
            }
        }
        for z in specs {
            if let Some(e) = self.edits.get(z.id) {
                e.style(
                    ui::rgba(self.theme.get("次级背景色")),
                    ui::rgba(self.theme.get("窗体字体色")),
                    16.,
                );
                let top = z.y.max(8.);
                let bottom = (z.y + z.h).min(lh - 8.);
                if bottom <= top || z.h <= 0. {
                    e.hide();
                } else {
                    e.place(z.x, top, z.w, bottom - top);
                }
            }
        }
        let picker_rect = modals::font_picker(&self.model, lw, lh);
        let picker_error = if let Some(picker) = &mut self.font_picker {
            if let Some(z) = picker_rect {
                let error = picker.set_current(&self.cfg.font_family).err();
                picker.place(z.x, z.y, z.w, z.h);
                error
            } else {
                picker.hide();
                None
            }
        } else {
            None
        };
        if let Some(error) = picker_error {
            self.toast(error);
        }
    }
    fn poll_trainer_status(&mut self, now: u64) {
        let Some(request) = self.live_schema.begin(now) else {
            return;
        };
        let tx = self.tx.clone();
        let hwnd = self.win.hwnd;
        std::thread::spawn(move || {
            let result = local_json(local_port(), "/api/trainer/status", None);
            let _ = tx.send(Bg::TrainerStatus(request, result));
            wake(hwnd);
        });
    }
    fn invalidate_live_hints(&mut self) {
        self.analyzed = 0;
        self.analyzed_key.clear();
        self.full_words.clear();
        self.full_words_ver.clear();
        self.full_words_of.clear();
        self.full_words_breaks.clear();
        self.full_words_required_spans.clear();
        self.full_words_dynamic_boundary = None;
        self.route_cache.clear();
        self.full_fetch_hash = None;
        self.display_retry.reset();
        self.full_retry.reset();
        clear_live_hint_views(&mut self.model, &mut self.sess);
        self.layout_key = (u64::MAX, 0, 0);
        // Keep both single-flight locks: old requests must finish before the
        // latest revision is dispatched, even after several rapid switches.
        gale_win::window::request_frame();
    }
    fn apply_trainer_status(&mut self) {
        let Some(status) = self.live_schema.snapshot.as_ref() else {
            return;
        };
        if let Some(keys) = status["selection_keys"].as_array() {
            let keys: Vec<_> = keys
                .iter()
                .filter_map(|value| {
                    value
                        .as_u64()
                        .filter(|value| *value <= 255)
                        .map(|v| v as u32)
                })
                .collect();
            gale_win::input_host::set_selection_keys(&keys);
        }
        self.model.schema = status["schema"].as_str().unwrap_or("").into();
        self.model.schema_ver = self.live_schema.revision.version.clone();
        self.model.max_code_length = status["max_code_length"].as_u64().unwrap_or(4) as usize;
        if let Some(reason) = status["hint_unavailable_reason"].as_str() {
            self.model.status = reason.into();
        } else if self.model.status.starts_with("词库状态暂不可用") {
            self.model.status = format!(
                "{} · {}",
                self.model.schema,
                if self.model.words.is_empty() {
                    "服务已连接"
                } else {
                    "词提已就绪"
                },
            );
        }
    }
    fn analyze(&mut self) {
        if self.analyzing
            || self.full_analyzing
            || !self.live_schema.ready()
            || !self.display_retry.ready(self.ms())
            || (self.analyzed == self.generation && self.analyzed_key == self.hint_analysis_key())
        {
            return;
        }
        self.analyzing = true;
        let tx = self.tx.clone();
        let hwnd = self.win.hwnd;
        let generation = self.generation;
        let revision = self.live_schema.revision.clone();
        let status = self.live_schema.snapshot.clone().unwrap();
        // 整改14：词提一次性分析整篇文章（全文坐标）；换段只做本地切片，
        // 不再“打到哪段才请求哪段”。段间直接拼接（无分隔符），
        // start=全文累计字符偏移。超长文（>9500 字）会撞 server 10000 字
        // 上限，退回单段请求（段内坐标，refill 不适用——清全文缓存）。
        let (text, breaks, required_spans, dynamic_boundary, whole_book) =
            self.hint_analysis_input();
        let strategy = self.cfg.hint_strategy;
        // The display rule is fixed; the F10 switch changes only the theoretical
        // four-code model and must not reroute the article during practice.
        let auto4 = true;
        let request_key = self.hint_analysis_key();
        let cached_route = self
            .route_cache
            .iter()
            .find(|(key, _)| key == &request_key)
            .map(|(_, value)| value.clone());
        self.dynamic_boundary_seen = self.analysis_dynamic_boundary();
        self.dynamic_hint_due = None;
        if !whole_book && !self.full_words.is_empty() {
            self.full_words.clear();
            self.full_words_ver.clear();
            self.full_words_of.clear();
            self.full_words_breaks.clear();
        }
        self.last_analyze_whole = whole_book;
        let hints = self.sess.rules.word_hints;
        if !hints || status["hint_unavailable_reason"].is_string() {
            self.analyzing = false;
            self.analyzed = generation;
            self.analyzed_key = request_key;
            self.display_retry.reset();
            self.model.words.clear();
            return;
        }
        let single = self.sess.source == session::SessionSource::Single;
        // 缓存仅当：有词提数据 且 schema 版本未变 且 正文未变（换书/清洗自动失效）。
        let known_version = if self.full_words.is_empty()
            || self.full_words_of != text
            || self.full_words_breaks != breaks
            || self.full_words_strategy != strategy
            || self.full_words_required_spans != required_spans
            || self.full_words_auto4 != auto4
            || self.full_words_dynamic_boundary != dynamic_boundary
            || self.full_words_ver.is_empty()
        {
            String::new()
        } else {
            self.full_words_ver.clone()
        };
        std::thread::spawn(move || {
            let port = local_port();
            let r = (|| {
                if let Some(cached) = cached_route {
                    if cached["schema_ver"].as_str().is_some_and(|version| {
                        !version.is_empty() && Some(version) == status["schema_ver"].as_str()
                    }) {
                        return Ok(cached);
                    }
                }
                if !known_version.is_empty()
                    && status["schema_ver"].as_str() == Some(&known_version)
                {
                    let mut cached = status;
                    cached["words_unchanged"] = true.into();
                    return Ok(cached);
                }
                local_json(
                    port,
                    "/api/trainer/analyze",
                    Some(serde_json::json!({
                        "text":text,"single":single,"breaks":breaks,"theory_auto4":auto4,
                        "strategy":strategy.as_str(),"required_spans":required_spans,
                        "dynamic_boundary":dynamic_boundary,
                        "route_mode":"display",
                    })),
                )
                .and_then(|value| {
                    validate_route_response(
                        &value,
                        strategy,
                        &required_spans,
                        dynamic_boundary,
                        "display",
                    )?;
                    if value["theory_auto4"].as_bool() != Some(auto4) {
                        return Err("当前虎符服务不支持新词提规则，请更新服务后重试".into());
                    }
                    analysis::response_entries(&text, &value)?;
                    Ok(value)
                })
            })();
            let _ = tx.send(Bg::Analyze(generation, request_key, revision, r));
            wake(hwnd);
        });
    }
    fn collect_background(&mut self) {
        while let Ok(r) = self.rx.try_recv() {
            match r {
                Bg::TrainerStatus(request, result) => {
                    let now = self.ms();
                    match self.live_schema.complete(request, now, result) {
                        StatusOutcome::Ignored => {}
                        StatusOutcome::Failed(error) => {
                            self.model.status = format!("词库状态暂不可用 · 将自动重试 · {error}");
                            gale_win::window::request_frame();
                        }
                        StatusOutcome::Ready { changed } => {
                            self.apply_trainer_status();
                            if changed {
                                self.invalidate_live_hints();
                            }
                            self.analyze();
                            self.fetch_full_analysis();
                        }
                    }
                }
                Bg::Analyze(g, request_key, revision, r) => {
                    self.analyzing = false;
                    let current_revision =
                        g == self.generation && revision == self.live_schema.revision;
                    // A transport/protocol failure still applies to this
                    // service revision after another commit changes the key.
                    // Otherwise rapid typing could bypass failure backoff.
                    let failure_recorded = current_revision && r.is_err();
                    if failure_recorded {
                        let now = self.ms();
                        self.display_retry.fail(now);
                    }
                    if current_revision && request_key == self.hint_analysis_key() {
                        let r = r.and_then(|value| {
                            if self
                                .live_schema
                                .revision
                                .accepts(&revision, value["schema_ver"].as_str().unwrap_or(""))
                            {
                                Ok(value)
                            } else {
                                let now = self.ms();
                                self.live_schema.request_fresh(now);
                                Err("词库版本已变化，正在重新获取词提".into())
                            }
                        });
                        match r {
                            Ok(v) => {
                                self.analyzed = g;
                                self.analyzed_key = request_key.clone();
                                self.display_retry.reset();
                                if self.cfg.hint_dynamic
                                    && v["words"].is_array()
                                    && serde_json::to_vec(&v)
                                        .is_ok_and(|bytes| bytes.len() <= 2_000_000)
                                {
                                    let key = self.hint_analysis_key();
                                    self.route_cache.retain(|(cached, _)| cached != &key);
                                    self.route_cache.push_back((key, v.clone()));
                                    while self.route_cache.len() > 4 {
                                        self.route_cache.pop_front();
                                    }
                                }
                                // 整改14：words 是全文坐标；缓存命中（words_unchanged）
                                // 沿用 full_words，仅重切当前段。超长回退
                                // （last_analyze_whole=false）时响应为段内坐标，
                                // 走旧段内校验直填，不动全文缓存。
                                if !self.last_analyze_whole {
                                    let chars: Vec<char> =
                                        self.sess.current_segment().text.chars().collect();
                                    let parsed: Vec<modals::HintWord> =
                                        serde_json::from_value(v["words"].clone())
                                            .unwrap_or_default();
                                    self.model.words = parsed
                                        .into_iter()
                                        .filter(|word| {
                                            word.len > 0
                                                && word.start <= chars.len()
                                                && word.len <= chars.len() - word.start
                                                && chars[word.start..word.start + word.len]
                                                    .iter()
                                                    .copied()
                                                    .eq(word.text.chars())
                                                && word.code.as_ref().is_none_or(|c| c.len() <= 32)
                                                && word.tail.len() <= 16
                                        })
                                        .collect();
                                } else if v["words_unchanged"] != true {
                                    let parsed: Vec<modals::HintWord> =
                                        serde_json::from_value(v["words"].clone())
                                            .unwrap_or_default();
                                    // 全文校验：词面与正文逐字对齐（防分词漂移）。
                                    let full: Vec<char> = self
                                        .sess
                                        .segments
                                        .iter()
                                        .flat_map(|s| s.text.chars())
                                        .collect();
                                    self.full_words = parsed
                                        .into_iter()
                                        .filter(|word| {
                                            word.len > 0
                                                && word.start <= full.len()
                                                && word.len <= full.len() - word.start
                                                && full[word.start..word.start + word.len]
                                                    .iter()
                                                    .copied()
                                                    .eq(word.text.chars())
                                                && word.code.as_ref().is_none_or(|c| c.len() <= 32)
                                                && word.tail.len() <= 16
                                        })
                                        .collect();
                                    self.full_words_ver = self.model.schema_ver.clone();
                                    self.full_words_of = full.into_iter().collect();
                                    self.full_words_breaks = self.analysis_breaks();
                                    self.full_words_strategy = self.cfg.hint_strategy;
                                    self.full_words_required_spans = self.analysis_required_spans();
                                    self.full_words_auto4 = true;
                                    self.full_words_dynamic_boundary =
                                        self.analysis_dynamic_boundary();
                                }
                                if self.last_analyze_whole {
                                    self.refill_segment_words();
                                }
                                self.layout_key = (u64::MAX, 0, 0);
                                self.model.status = format!(
                                    "{} · {}",
                                    self.model.schema,
                                    if self.model.words.is_empty() {
                                        "服务已连接"
                                    } else {
                                        "词提已就绪"
                                    }
                                );
                                if let Some(reason) = v["hint_unavailable_reason"].as_str() {
                                    self.model.status = reason.into();
                                }
                            }
                            Err(e) => {
                                if !failure_recorded {
                                    let now = self.ms();
                                    self.display_retry.fail(now);
                                }
                                // Keep showing the prefetched whole-book hints
                                // when the service is temporarily unavailable.
                                if self.refill_from_full_words() {
                                    self.model.status =
                                        format!("词提请求失败 · 已用本地缓存 · {e}");
                                } else {
                                    self.model.words.clear();
                                    self.model.status = format!("词提暂不可用 · {e}");
                                }
                            }
                        }
                        // CTF 自检失败的持续警示：任何状态文案都带上前缀（任务 7）
                        if self.tsf_broken {
                            self.model.status =
                                format!("TSF 输入法可能不可用 · {}", self.model.status);
                        }
                    }
                    self.fetch_full_analysis();
                    self.analyze();
                    gale_win::window::request_frame();
                }
                Bg::Chats(v) => {
                    self.qq_busy = false;
                    // 整改7：刷新不丢选择——按标题把旧 bound 重绑到新列表位置；
                    // 标题消失（窗口关闭/会话移除）才解绑。
                    let prev_title = self
                        .model
                        .bound
                        .and_then(|i| self.chats.get(i))
                        .map(|c| c.title.clone());
                    self.chats = v;
                    self.model.groups = self.chats.iter().map(|c| c.title.clone()).collect();
                    self.model.bound = prev_title
                        .as_ref()
                        .and_then(|t| self.chats.iter().position(|c| &c.title == t));
                    if self.chats.is_empty() {
                        let panel = qq_bridge::main_panel().is_some();
                        let msg = if panel {
                            "暂时读不到 QQ 群聊列表，请打开群聊窗口后刷新"
                        } else {
                            "未检测到 QQ：请打开并登录 QQ 后重试"
                        };
                        self.model.status = msg.into();
                        self.toast(msg);
                    }
                }
                Bg::Sent { label, result } => {
                    self.send_busy = false;
                    // 工作线程确认 QQ 已收下消息后才回到这里切前台。
                    self.focus_trainer();
                    match result {
                        Ok(()) => self.toast(label),
                        Err(e) => self.toast(e),
                    }
                }
                Bg::FullAnalysis(key, revision, result) => {
                    self.full_analyzing = false;
                    self.model
                        .fields
                        .insert("analysis_loading".into(), "false".into());
                    if revision == self.live_schema.revision && key == self.analysis_key() {
                        let result = result.and_then(|analysis| {
                            if self
                                .live_schema
                                .revision
                                .accepts(&revision, &analysis.schema_ver)
                            {
                                Ok(analysis)
                            } else {
                                let now = self.ms();
                                self.live_schema.request_fresh(now);
                                Err("词库版本已变化，正在重新获取理论分析".into())
                            }
                        });
                        match result {
                            Ok(fa) => {
                                self.sess.full_analysis = Some(fa);
                                self.full_fetch_hash = Some(self.analysis_key());
                                self.full_retry.reset();
                                self.model.fields.remove("analysis_error");
                            }
                            Err(error) => {
                                self.model.fields.insert("analysis_failed_key".into(), key);
                                let now = self.ms();
                                self.full_retry.fail(now);
                                self.model.fields.insert("analysis_error".into(), error);
                            }
                        }
                    } else {
                        self.full_retry.reset();
                        self.fetch_full_analysis();
                    }
                    self.analyze();
                    gale_win::window::request_frame();
                }
                Bg::Article(r) => {
                    self.capture_fields();
                    self.qq_busy = false;
                    match r {
                        Ok(t) => {
                            // F4 读到就立刻载入，不再做待读草稿二次确认。
                            self.model.fields.insert("load_text".into(), t);
                            self.load_text();
                        }
                        Err(e) => self.toast(e),
                    }
                }
                Bg::ScoreSaved(r) => {
                    self.score_inflight = false;
                    match r {
                        Ok(()) => {
                            self.pending_scores.pop_front();
                            self.score_retry_ms = 0;
                            // P2 今日汇总（任务 6b）：成绩落盘后刷新今日字数
                            //（ui.rs 今日卡片读 cfg.today_chars，数据由这里注入）。
                            self.cfg.today_chars = today_chars_from_dir(&self.dir);
                        }
                        Err(e) => {
                            self.score_retry_ms = self.ms() + 2000;
                            self.snapshot_dirty = true;
                            self.toast(format!("成绩保存失败，保留待重试：{e}"));
                        }
                    }
                }
                Bg::Saved(r) => {
                    if let Err(e) = r {
                        self.snapshot_dirty = true;
                        // A locked/denied data dir otherwise re-toasts every
                        // few seconds forever; keep the error visible but calm.
                        if self.ms().saturating_sub(self.save_toast_at) > 30_000 {
                            self.save_toast_at = self.ms();
                            self.toast(format!("保存失败：{e}"));
                        }
                    }
                }
                Bg::RuleTest(g, r) => {
                    self.rule_busy = false;
                    if g == self.rule_generation && self.model.modal == Modal::Rules {
                        match r {
                            Ok(r) => {
                                self.model.rule_test_source = r.raw_text;
                                self.model.rule_preview = r.cleaned_text;
                                self.model.rule_hits = r.hits.iter().map(|(_, n)| n).sum();
                                self.model.rule_error.clear();
                            }
                            Err(e) => self.model.rule_error = e,
                        }
                    }
                }
                Bg::RuleApply(g, rule, r) => {
                    self.rule_busy = false;
                    if g != self.generation
                        || rule != self.rule_generation
                        || self.sess.frozen_rules.is_some()
                    {
                        self.toast("文章已变更，清洗结果已丢弃");
                    } else {
                        match r {
                            Ok(sess) => {
                                self.sess = sess;
                                self.reset_host();
                                self.toast("已应用清洗；原文保留，可随时还原");
                                self.snapshot();
                            }
                            Err(e) => self.toast(e),
                        }
                    }
                }
                Bg::Skin(g, r) => {
                    if g == self.theme_generation && self.cfg.theme == "跟随虎符" {
                        match r {
                            Ok(t) => {
                                self.capture_fields();
                                let key = self.model.field("pref_color_key");
                                let draft = self.model.field("pref_color_hex");
                                let pristine =
                                    draft.trim().trim_start_matches('#').eq_ignore_ascii_case(
                                        &self.model.field(&format!("color_{key}")),
                                    );
                                self.theme = t;
                                for (key, value) in &self.cfg.color_overrides {
                                    if let Some(color) =
                                        gale_core::theme::Color::from_hex_no_hash(value)
                                    {
                                        self.theme.set(key, color);
                                    }
                                }
                                self.apply_style();
                                self.sync_preferences(false);
                                if pristine {
                                    modals::preferences::sync_color_editor(&mut self.model);
                                    self.model.modal_epoch += 1;
                                }
                            }
                            Err(e) => self.toast(e),
                        }
                    }
                }
            }
            gale_win::window::request_frame();
        }
    }
    fn submit_online(&mut self, c: OnlineCommand) {
        if let OnlineCommand::Login { username, password } = &c {
            if username.trim().is_empty() || password.is_empty() {
                self.toast("请输入用户名和密码");
                return;
            }
            let entered = gale_core::wenlai::normalize_base_url(&self.model.field("server"));
            let active = gale_core::wenlai::normalize_base_url(&self.cfg.wenlai_domain);
            if entered.is_err() || entered != active {
                self.toast("请先应用服务地址，再登录此服务器");
                return;
            }
        }
        if matches!(&c, OnlineCommand::Fetch(_) | OnlineCommand::Bookshelf)
            && !self.online_logged_in
        {
            self.toast("请先登录晴发文账号");
            return;
        }
        if self.online.is_none() {
            if let OnlineCommand::SetServer { base_url } = &c {
                match OnlineWorker::spawn(self.dir.clone(), base_url) {
                    Ok(w) => self.online = Some(w),
                    Err(e) => {
                        self.toast(e);
                        return;
                    }
                }
            }
        }
        let identity_change = matches!(
            &c,
            OnlineCommand::Login { .. }
                | OnlineCommand::Logout
                | OnlineCommand::SetServer { .. }
                | OnlineCommand::Register { .. }
        );
        let identity_password = match &c {
            OnlineCommand::Login { password, .. } | OnlineCommand::Register { password, .. } => {
                Some(password.clone())
            }
            _ => None,
        };
        let fetching = matches!(&c, OnlineCommand::Fetch(_));
        match self
            .online
            .as_mut()
            .ok_or_else(|| "在线模块初始化失败，请重新应用服务地址".to_string())
            .and_then(|w| w.submit(c))
        {
            Ok(id) => {
                if fetching {
                    // A manual fetch (including shelf selection) takes over the
                    // pending automatic advance instead of queuing a second one.
                    self.auto_next_at = None;
                }
                if identity_change {
                    self.pending_login_password = identity_password.map(|p| (id, p));
                    self.online_logged_in = false;
                    self.model.online_authenticated = false;
                    self.model.online_expired = true;
                    self.model.online_login_expanded = true;
                    self.model.modal_epoch += 1;
                    self.article = None;
                    self.article_fetch = None;
                    self.shelf.clear();
                    self.model.online_preview.clear();
                    self.model.online_shelf.clear();
                    self.model.online_categories.clear();
                    self.model.online_category = 0;
                    self.model.online_title = "在线书架".into();
                }
                self.online_pending = self.online.as_ref().map(|w| w.pending_count()).unwrap_or(0);
                self.model.online_busy = self.online_pending > 0;
                self.model.online_status = if identity_change {
                    "正在更新登录状态…"
                } else {
                    "正在请求…（当前文字保留）"
                }
                .into();
                gale_win::window::request_frame();
            }
            Err(e) => self.toast(e),
        }
    }
    fn collect_online(&mut self) {
        loop {
            let r = self.online.as_ref().and_then(|o| o.try_recv());
            let Some(r) = r else {
                break;
            };
            if self.model.modal == Modal::Online {
                self.capture_fields();
            }
            match r.result {
                Ok(OnlinePayload::Account(Some(a))) => {
                    self.sync_account_cleared = false;
                    self.sync_online_user = a.username.clone();
                    if let Some((_, p)) = self
                        .pending_login_password
                        .take()
                        .filter(|(id, _)| *id == r.id)
                    {
                        self.sync_online_password = p;
                    }
                    self.sync_identity_dirty = r.id != 0;
                    self.write_sync_profile(&self.cfg.clone());
                    self.online_logged_in = true;
                    self.model.online_authenticated = true;
                    self.model.online_expired = false;
                    self.model.online_login_expanded = false;
                    self.model.online_register_mode = false;
                    self.model.online_account = a.display_name.clone();
                    if self.model.modal == Modal::Online {
                        self.model.modal_epoch += 1;
                    }
                    self.model.online_status = format!(
                        "{} · {}",
                        a.display_name,
                        if r.id == 0 {
                            "已恢复登录，取文时验证"
                        } else {
                            "已登录"
                        }
                    );
                    if self.model.modal == Modal::Online {
                        self.submit_online(OnlineCommand::Categories);
                        self.submit_online(OnlineCommand::Bookshelf);
                    }
                }
                Ok(OnlinePayload::Account(None)) => {
                    self.sync_account_cleared = true;
                    self.pending_login_password = None;
                    self.sync_online_password.clear();
                    self.sync_identity_dirty = true;
                    self.write_sync_profile(&self.cfg.clone());
                    self.online_logged_in = false;
                    self.shelf.clear();
                    self.model.online_shelf.clear();
                    self.model.online_status = "登录已过期，请重新登录；当前预览保留".into();
                }
                Ok(OnlinePayload::Article(a, params)) => {
                    // Validate before committing either the article or its
                    // continuation source; rejection must preserve both.
                    if a.text.chars().count() > 100000 {
                        let message = "在线正文超过100000字，请重新取较短文段";
                        self.model.online_status = message.into();
                        self.toast(message);
                        continue;
                    }
                    self.model.online_title = a.title.clone();
                    self.model.online_preview = a.text.clone();
                    self.model.online_status = format!(
                        "{} 字 · {}",
                        a.text.chars().count(),
                        if a.seg_mark.is_empty() {
                            "无段标"
                        } else {
                            &a.seg_mark
                        }
                    );
                    self.model.online_article = Some(a.clone());
                    self.article = Some(a);
                    self.article_fetch = Some(params);
                    // 整改2a：去预览——取到正文直接载入跟打区，第一段起打
                    //（成绩单期间由自动下一段触发时，载入本身就会关闭成绩单）。
                    self.load_online_article();
                }
                Ok(OnlinePayload::Categories(c)) => {
                    let selected = self
                        .model
                        .online_categories
                        .get(self.model.online_category)
                        .map(|(id, _)| id.clone());
                    self.model.online_categories =
                        std::iter::once((String::new(), "全部分类".into()))
                            .chain(c)
                            .collect();
                    self.model.online_category = selected
                        .and_then(|id| {
                            self.model
                                .online_categories
                                .iter()
                                .position(|(key, _)| *key == id)
                        })
                        .unwrap_or(0);
                }
                Ok(OnlinePayload::Bookshelf(v)) => {
                    self.model.online_status = if v.is_empty() {
                        "已登录 · 书架为空，可随机取文".into()
                    } else {
                        format!("已登录 · 书架 {} 本", v.len())
                    };
                    self.model.online_shelf = v
                        .iter()
                        .map(|s| {
                            format!(
                                "{} · {}",
                                s.title,
                                if s.sort_num > 0 {
                                    format!("第{}段", s.sort_num)
                                } else {
                                    "未开始".into()
                                }
                            )
                        })
                        .collect();
                    self.shelf = v;
                }
                Ok(OnlinePayload::ServerChanged(s)) => {
                    self.sync_account_cleared = true;
                    self.pending_login_password = None;
                    self.sync_online_user.clear();
                    self.sync_online_password.clear();
                    self.sync_identity_dirty = true;
                    self.model.online_article = None;
                    self.model.online_authenticated = false;
                    self.model.online_account.clear();
                    self.cfg.wenlai_domain = s.clone();
                    self.model.fields.insert("server".into(), s);
                    self.model.online_status = "服务已切换，请重新登录".into();
                    self.save_config();
                }
                Ok(OnlinePayload::LoggedOut) => {
                    self.sync_account_cleared = true;
                    self.pending_login_password = None;
                    self.sync_online_user.clear();
                    self.sync_online_password.clear();
                    self.sync_identity_dirty = true;
                    self.write_sync_profile(&self.cfg.clone());
                    self.online_logged_in = false;
                    self.model.online_authenticated = false;
                    self.model.online_expired = false;
                    self.model.online_account.clear();
                    self.model.online_article = None;
                    if self.model.modal == Modal::Online {
                        self.model.modal_epoch += 1;
                    }
                    self.model.online_status = "已退出登录".into();
                }
                Ok(OnlinePayload::CodeSent) => {
                    // 验证码已发出：启动 60 秒重发冷却（主循环每秒刷新剩余秒）。
                    self.reg_code_until = self.ms() + 60_000;
                    self.model.online_status = "验证码已发送，请查收邮箱".into();
                    self.toast("验证码已发送，请查收邮箱");
                }
                Err(e) => {
                    if self
                        .pending_login_password
                        .as_ref()
                        .is_some_and(|(id, _)| *id == r.id)
                    {
                        self.sync_account_cleared = true;
                        self.pending_login_password = None;
                        self.sync_online_password.clear();
                        self.sync_identity_dirty = true;
                        self.write_sync_profile(&self.cfg.clone());
                    }
                    self.model.online_status = e.clone();
                    self.toast(e);
                }
                Ok(OnlinePayload::Warning(e)) => {
                    self.model.online_status = e.clone();
                    self.toast(e);
                }
            }
            gale_win::window::request_frame();
        }
        let busy = self.online.as_ref().map(|o| o.is_busy()).unwrap_or(false);
        self.online_pending = self.online.as_ref().map(|o| o.pending_count()).unwrap_or(0);
        if busy != self.model.online_busy {
            self.model.online_busy = busy;
            gale_win::window::request_frame();
        }
    }
    fn load_text(&mut self) {
        let text = self.model.field("load_text");
        if text.trim().is_empty() {
            self.toast("没有可载入的正文");
            return;
        }
        if text.chars().count() > 100000 {
            self.toast("文章超过 100000 字，请分段载入");
            return;
        }
        self.sess.load_text(&text);
        self.online_article_current = false;
        self.reset_host();
        self.open(Modal::None);
        gale_win::input_host::diag_log(&format!(
            "load_text complete: {} chars",
            self.sess.current_segment().text.chars().count()
        ));
        self.snapshot();
    }
    fn score_record(&self) -> Option<&gale_core::score::ScoreRecord> {
        if self.model.ledger_open {
            if let Some(record) = self
                .model
                .selected_history
                .and_then(|index| self.model.history.get(index))
            {
                return Some(record);
            }
        }
        if self.sess.phase != Phase::Done || !self.sess.recorded {
            return None;
        }
        self.sess
            .results
            .get(self.sess.current_index())
            .and_then(Option::as_ref)
    }
    fn score_text(&self) -> Option<String> {
        self.score_record().map(|record| self.format_record(record))
    }
    fn sendable_score_text(&self) -> Option<String> {
        self.score_record()
            .filter(|record| record.can_send_score())
            .map(|record| self.format_record(record))
    }
    fn send_score_to_group(&mut self, record: &gale_core::score::ScoreRecord) {
        if !record.can_send_score() {
            self.toast("成绩只有打完且最后一字正确才能发送");
            return;
        }
        match self.model.bound.and_then(|i| self.chats.get(i).cloned()) {
            Some(chat) => {
                let text = self.format_record(record);
                let label = format!("成绩已发送到「{}」", chat.title);
                self.spawn_send(chat, text, label);
            }
            None => {
                self.toast("未绑定群，请先选择");
                self.open(Modal::Groups);
            }
        }
    }
    fn race_text(&self) -> String {
        let a = self.sess.aggregate();
        // 段数取 a.segments（<5 硬门口径，与整场聚合一致——原 results 计数
        // 会把中断的短段也算进去，与下面的字数/速度口径打架）。
        let mut s = format!(
            "《{}》 整场赛报\n{}段 / {}字 / {:0.2}秒 / 总速度{:0.2}\n",
            self.sess.title,
            a.segments,
            a.chars,
            a.time_ms as f64 / 1000.,
            a.speed
        );
        for r in self.sess.results.iter().flatten() {
            s.push_str(&self.format_record(r));
            s.push('\n');
        }
        s
    }
    fn copy(&mut self, s: &str) {
        match unsafe { gale_win::clipboard_bridge::try_write_text(s) } {
            Ok(()) => self.toast("已复制，请确认目标群后手动粘贴发送"),
            Err(e) => self.toast(e),
        }
    }
    fn act(&mut self, a: Action, confirmed: bool) {
        self.capture_fields();
        // 私有标记通道（整改2/5/8/13）：modals.rs 暂无正式变体的动作在这里
        // 先行解码执行，不再走后面的公共分支（真实 Theme 动作不受影响）。
        // 词提颜色循环选色（aux 前缀带参数，不进 AuxAction 枚举）。
        if let Action::Theme(s) = &a {
            if let Some(key) = s
                .strip_prefix(AUX_PREFIX)
                .and_then(|c| c.strip_prefix("hint_color:"))
            {
                if self.cycle_hint_color(key) {
                    gale_win::window::request_frame();
                    return;
                }
            }
        }
        if let Some(aux) = aux_action(&a) {
            self.run_aux(aux);
            gale_win::window::request_frame();
            return;
        }
        if gale_win::input_host::diagnostics_enabled() {
            gale_win::input_host::diag_log(&format!(
                "action={a:?} confirmed={confirmed} phase={:?} modal={:?} load_chars={}",
                self.sess.phase,
                self.model.modal,
                self.model.field("load_text").chars().count()
            ));
        }
        if matches!(
            a,
            Action::RuleSelect(_)
                | Action::RuleToggle(_)
                | Action::RuleUp
                | Action::RuleDown
                | Action::RuleAdd
                | Action::RuleDelete
                | Action::RuleSave
                | Action::RuleImport
                | Action::RuleContentToggle
                | Action::RuleReset
        ) {
            self.rule_generation += 1;
            self.model.rule_preview.clear();
        }
        match a {
            Action::None => {}
            Action::ToggleLedger => {
                self.model.analysis_dock = false;
                self.model.modal = Modal::None;
                self.model.ledger_open = !self.model.ledger_open;
                if self.model.ledger_open {
                    let sc = gale_win::window::dpi_scale();
                    let (_, h) = self.win.client_size();
                    let lh = h as f32 / sc;
                    let cap = crate::history::scroll_max(&self.model, lh);
                    self.model.ledger_scroll = self.model.ledger_scroll.clamp(0., cap);
                } else {
                    self.model.selected_history = None;
                }
                gale_win::window::request_frame();
            }
            Action::ToggleAnalysis => self.toggle_analysis_dock(),
            Action::OpenAppearanceSettings => {
                self.model.fields.insert("pref_tab".into(), "2".into());
                self.open(Modal::Preferences);
            }
            Action::Open(m) => {
                if self.model.modal == m {
                    self.open(Modal::None);
                } else {
                    self.open(m);
                    if m == Modal::Groups {
                        self.act(Action::ScanGroups, true);
                    }
                    if m == Modal::Online && self.online_logged_in {
                        self.submit_online(OnlineCommand::Categories);
                        self.submit_online(OnlineCommand::Bookshelf);
                    }
                    if m == Modal::Rules {
                        if self.cfg.cleaning_preset.rules.is_empty() {
                            self.cfg.cleaning_preset = CleaningPreset::builtin();
                        }
                        self.sync_rule_model();
                    }
                }
            }
            Action::SelectHistory(index) => {
                if self.model.ledger_open && self.model.selected_history == Some(index) {
                    self.model.selected_history = None;
                    gale_win::window::request_frame();
                } else {
                    self.select_history(index);
                }
            }
            Action::OnlineAccount => {
                self.model.online_login_expanded = !self.model.online_login_expanded;
                self.model.modal_epoch += 1;
            }
            Action::OnlineDifficultyTo(i) => self.model.online_difficulty = i.min(5),
            Action::OnlineLoadOnly => self.act(Action::OnlineLoad, confirmed),
            Action::RuleScopeToggle => {
                self.model.rule_save_preset = !self.model.rule_save_preset;
                if self.model.rule_save_preset {
                    self.save_config();
                }
            }
            Action::RuleContentToggle => {
                if let Some(r) = self
                    .cfg
                    .cleaning_preset
                    .rules
                    .get_mut(self.model.rule_selected)
                {
                    r.safe = !r.safe;
                }
                let i = self.model.rule_selected;
                if let (Some(rule), Some(view)) = (
                    self.cfg.cleaning_preset.rules.get(i),
                    self.model.rules.get_mut(i),
                ) {
                    view.2 = !rule.safe;
                }
                self.save_config();
            }
            Action::RuleReset => {
                if !confirmed {
                    self.model.confirm =
                        "恢复内置清洗规则？当前规则将被替换，可先导出保留。".into();
                    self.model.pending = Some(Action::RuleReset);
                    self.open(Modal::Confirm);
                    return;
                }
                self.cfg.cleaning_preset = CleaningPreset::builtin();
                self.model.rule_selected = 0;
                self.sync_rule_model();
                self.save_config();
                self.open(Modal::Rules);
            }
            Action::QuoteClass(i) => {
                let mut rules = self.sess.rules.clone();
                let value = match i {
                    0 => &mut rules.quotes.double,
                    1 => &mut rules.quotes.single,
                    2 => &mut rules.quotes.corner,
                    _ => &mut rules.quotes.double_corner,
                };
                *value = !*value;
                match self.sess.set_rules(rules) {
                    Ok(()) => self.save_config(),
                    Err(e) => self.toast(e),
                }
            }
            Action::Accent(value) => {
                if let Some(color) = gale_core::theme::Color::from_hex_no_hash(&value) {
                    self.cfg.color_overrides.insert("光标色".into(), value);
                    self.theme.set("光标色", color);
                    self.save_config();
                }
            }
            Action::LineHeight(delta) => {
                self.cfg.line_gap = (self.cfg.line_gap + delta as f32 * 0.02).clamp(0.4, 1.2);
                self.save_config();
            }
            Action::MotionLevel(level) => {
                self.cfg.motion_level = level.min(3);
                self.model.reduced_motion = level == 0 || gale_win::edit::reduced_motion();
                self.cfg.smooth_caret = level > 0;
                self.cfg.shake_on_wrong = level >= 2;
                self.cfg.pop_on_correct = level >= 2;
                self.save_config();
            }
            Action::HintStyle(style) => {
                self.cfg.hint_style = style.min(2);
                self.full_words.clear();
                self.full_words_ver.clear();
                self.generation = self.generation.wrapping_add(1);
                self.model.words.clear();
                self.analyze();
                self.save_config();
            }
            Action::AppearanceReset => {
                let d = Config::default();
                self.cfg.font_size = d.font_size;
                self.cfg.font_family = d.font_family;
                self.cfg.line_gap = d.line_gap;
                self.cfg.hint_style = d.hint_style;
                self.cfg.motion_level = d.motion_level;
                self.cfg.theme = "墨玉".into();
                self.cfg
                    .color_overrides
                    .retain(|key, _| key.starts_with("词提"));
                self.cfg.smooth_caret = true;
                self.cfg.shake_on_wrong = true;
                self.model.reduced_motion = gale_win::edit::reduced_motion();
                self.theme = ui::theme("墨玉");
                for (key, value) in &self.cfg.color_overrides {
                    if let Some(color) = gale_core::theme::Color::from_hex_no_hash(value) {
                        self.theme.set(key, color);
                    }
                }
                self.apply_style();
                self.sync_preferences(true);
                self.model.modal_epoch += 1;
                self.save_config();
            }
            Action::Close => self.open(Modal::None),
            Action::Exit => self.exit = true,
            Action::Minimize => unsafe {
                let _ = ShowWindow(self.win.hwnd(), SW_MINIMIZE);
            },
            Action::Maximize => unsafe {
                let cmd = if IsZoomed(self.win.hwnd()).as_bool() {
                    SW_RESTORE
                } else {
                    SW_MAXIMIZE
                };
                let _ = ShowWindow(self.win.hwnd(), cmd);
            },
            Action::ConfirmNo => {
                self.model.pending = None;
                self.open(Modal::None);
            }
            Action::ConfirmYes => {
                let p = self.model.pending.take();
                if let Some(p) = p {
                    self.act(p, true);
                }
                if self.model.modal == Modal::Confirm {
                    self.open(Modal::None);
                }
            }
            Action::Previous => {
                if self.sess.index > 0 {
                    self.sess.jump(self.sess.index - 1);
                    self.reset_host();
                    self.open(Modal::None);
                }
            }
            Action::Restart => {
                self.sess.jump(self.sess.index);
                self.reset_host();
                self.open(Modal::None);
            }
            Action::Next => {
                if self.online_article_current && self.article.is_some() {
                    self.auto_next_at = None;
                    self.act(Action::OnlineFetch(1), false);
                } else if self.sess.index + 1 >= self.sess.total_segments() {
                    if self.model.history.is_empty() {
                        self.toast("本轮已完成");
                    } else {
                        self.select_history(self.model.history.len() - 1);
                    }
                } else {
                    self.sess.next_segment();
                    self.reset_host();
                    self.open(Modal::None);
                }
            }
            Action::ToggleHints => {
                let mut r = self.sess.rules.clone();
                r.word_hints = !r.word_hints;
                match self.sess.set_rules(r) {
                    Ok(()) => {
                        self.model.hints = self.sess.rules.word_hints;
                        self.generation = self.generation.wrapping_add(1);
                        self.model.words.clear();
                        self.analyze();
                        self.save_config();
                    }
                    Err(e) => self.toast(e),
                }
            }
            Action::LoadClipboard => match unsafe { gale_win::clipboard_bridge::read_text() } {
                Some(t) => {
                    // 【减法 2026-11-09】载文窗退役：剪贴板内容直接开打。
                    self.model.fields.insert("load_text".into(), t);
                    self.load_text();
                }
                None => self.toast("剪贴板为空或不可用"),
            },
            Action::Theme(n) => {
                self.theme_generation += 1;
                self.cfg.theme = n.clone();
                if n == "跟随虎符" {
                    self.fetch_skin();
                } else {
                    self.theme = ui::theme(&n);
                    for (key, value) in &self.cfg.color_overrides {
                        if let Some(c) = gale_core::theme::Color::from_hex_no_hash(value) {
                            self.theme.set(key, c);
                        }
                    }
                }
                self.apply_style();
                self.sync_preferences(true);
                self.model.modal_epoch += 1;
                self.save_config();
            }
            Action::Material(mat) => {
                self.cfg.material = mat;
                self.apply_style();
                self.save_config();
            }
            Action::Font(d) => {
                // 外观页字号 +/- 与 Ctrl+滚轮统一口径（10..=100）。
                self.cfg.font_size = (self.cfg.font_size + d as f32).clamp(10., 100.);
                self.save_config();
            }
            Action::FontDefault => {
                self.cfg.font_family.clear();
                self.save_config();
            }
            Action::Motion => {
                self.model.reduced_motion = if gale_win::edit::reduced_motion() {
                    true
                } else {
                    !self.model.reduced_motion
                };
                self.cfg.smooth_caret = !self.model.reduced_motion;
                self.cfg.shake_on_wrong = !self.model.reduced_motion;
                self.save_config();
            }
            Action::Folder => {
                let path: Vec<u16> = self
                    .dir
                    .to_string_lossy()
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                unsafe {
                    let r = windows::Win32::UI::Shell::ShellExecuteW(
                        self.win.hwnd(),
                        windows::core::w!("open"),
                        windows::core::PCWSTR(path.as_ptr()),
                        None,
                        None,
                        SW_SHOWNORMAL,
                    );
                    if r.0 as isize <= 32 {
                        self.toast("无法打开数据目录");
                    }
                }
            }
            Action::CopyScore => {
                if let Some(s) = self.score_text() {
                    self.copy(&s);
                } else {
                    self.toast("还没有本段成绩");
                }
            }
            Action::CopyRace => {
                if self.sess.aggregate().segments == 0 {
                    self.toast("当前没有可复制的赛报");
                } else {
                    let s = self.race_text();
                    self.copy(&s);
                }
            }
            Action::SendScore => {
                if let Some(s) = self.sendable_score_text() {
                    self.copy(&s);
                } else {
                    self.toast("成绩只有打完且最后一字正确才能发送");
                }
            }
            Action::ToggleAutoSend => {
                self.cfg.auto_send_score = !self.cfg.auto_send_score;
                self.save_config();
                self.toast(if self.cfg.auto_send_score {
                    "打完自动发成绩：开"
                } else {
                    "打完自动发成绩：关（仅复制）"
                });
            }
            Action::SendScoreToGroup => {
                if let Some(record) = self.score_record().cloned() {
                    self.send_score_to_group(&record);
                } else {
                    self.toast("成绩只有打完且最后一字正确才能发送");
                }
            }
            Action::PostArticle => {
                if self.sess.current_segment().text.trim().is_empty() {
                    self.toast("请先载入文章，再发送到群");
                    return;
                }
                match self.model.bound.and_then(|i| self.chats.get(i).cloned()) {
                    Some(chat) => {
                        let seg = self.sess.current_segment();
                        let chars = seg.text.chars().count();
                        let mark = if seg.mark.is_empty() {
                            (self.sess.index + 1).to_string()
                        } else {
                            seg.mark.clone()
                        };
                        // 发文格式（跟打码生态通用）：[难度]标题 [字数]\n正文\n-----第N段-晴发文
                        let title = crate::ui::elide(&self.sess.title, 24);
                        let text = format!(
                            "[跟打]{} [{}字]\n{}\n-----第{}段-晴发文",
                            title, chars, seg.text, mark
                        );
                        let label = format!("第{mark}段已发文到「{}」", chat.title);
                        self.spawn_send(chat, text, label);
                    }
                    None => {
                        self.toast("未绑定群，请先选择");
                        self.open(Modal::Groups);
                    }
                }
            }
            Action::ExportImage => {
                if let Some(p) = file_dialog(self.win.hwnd, true, "bmp") {
                    match self.renderer.save_bmp(&p) {
                        Ok(()) => self.toast(format!("图片已导出：{}", p.display())),
                        Err(e) => self.toast(e),
                    }
                }
            }
            Action::ScanGroups => {
                if !self.qq_busy {
                    self.qq_busy = true;
                    // 整改7：QQ 读取屏扫描节流锚点（开屏 10s 后自动刷新用）
                    self.qq_scan_at = self.ms();
                    // Run on the UI thread: QQ's Chromium accessibility tree is
                    // only fully materialised for an STA client with a message
                    // pump. A bare worker thread sees just the D3D child.
                    let chats = qq_bridge::list_sessions_merged();
                    let _ = self.tx.send(Bg::Chats(chats));
                    gale_win::window::request_frame();
                }
            }
            Action::SelectGroup(i) => {
                if self.model.bound == Some(i) {
                    // 再点一次已选群 = 取消选择（用户要求能取消群选）。
                    self.model.bound = None;
                    self.toast("已取消选择群聊");
                    gale_win::window::request_frame();
                    return;
                }
                if i < self.chats.len() {
                    self.model.bound = Some(i);
                    // 【群三件套】选群即切换：能自动切就切，切不动再让用户点。
                    let chat = self.chats[i].clone();
                    if qq_bridge::focus_chat(&chat) {
                        self.toast(format!(
                            "已切到「{}」· F4 读文 / 打完自动发成绩",
                            chat.title
                        ));
                    } else {
                        self.toast(format!(
                            "已绑定「{}」· 请在 QQ 点开该群后 F4 读文",
                            chat.title
                        ));
                    }
                }
            }
            Action::ReadGroup => {
                if self.qq_busy {
                    self.toast("QQ 正在读取，请稍候");
                } else if let Some(c) = self.model.bound.and_then(|i| self.chats.get(i)).cloned() {
                    self.qq_busy = true;
                    // Same apartment rule as ScanGroups: Chromium's UIA tree is
                    // only complete for the UI STA thread, not for an MTA worker.
                    let r = uia_bridge::read_current_article(c.hwnd, &c.title);
                    let _ = self.tx.send(Bg::Article(r));
                    gale_win::window::request_frame();
                } else {
                    // 【减法 2026-11-09】没绑群就直接弹选群面板（用户拍板：
                    // F4 群载文、没选群就弹出选群）。
                    self.toast("未绑定群，请选择要跟打的群");
                    self.open(Modal::Groups);
                }
            }
            Action::OnlineLogin => {
                let username = self.model.field("username");
                let password = self.model.field("password");
                self.cfg.wenlai_username = username.clone();
                self.submit_online(OnlineCommand::Login { username, password });
                self.model.fields.insert("password".into(), String::new());
                if let Some(e) = self.edits.get("password") {
                    e.set_text("");
                }
            }
            Action::OnlineLogout => self.submit_online(OnlineCommand::Logout),
            Action::OnlineServer => {
                let base_url = self.model.field("server");
                self.submit_online(OnlineCommand::SetServer { base_url });
            }
            Action::ShelfPage => {
                self.model.shelf_offset = (self.model.shelf_offset + 1) % self.shelf.len().max(1)
            }
            Action::OnlineCategory(i) => self.model.online_category = i,
            Action::OnlineDifficulty => {
                self.model.online_difficulty = (self.model.online_difficulty + 1) % 6
            }
            Action::OnlineStrict => self.model.online_strict = !self.model.online_strict,
            Action::OnlineBooks => self.submit_online(OnlineCommand::Bookshelf),
            Action::OnlineFetch(d) => {
                let chars = match self.model.field("online_chars").trim().parse::<usize>() {
                    Ok(n) if (1..=100000).contains(&n) => n,
                    _ => {
                        self.toast("目标字数请输入1至100000之间的整数");
                        return;
                    }
                };
                let p = if d != 0 {
                    match &self.article {
                        Some(a) if d > 0 => self
                            .article_fetch
                            .as_ref()
                            .map(|source| source.next_params(a, chars, self.model.online_strict))
                            .unwrap_or_else(|| {
                                a.adjacent_params(true, chars, self.model.online_strict)
                            }),
                        Some(a) => a.adjacent_params(d > 0, chars, self.model.online_strict),
                        None => {
                            self.toast("先随机取得一段文章");
                            return;
                        }
                    }
                } else {
                    FetchParams {
                        chars,
                        strict_length: self.model.online_strict,
                        difficulty: Difficulty::from_level(self.model.online_difficulty as i64 + 1)
                            .unwrap_or(Difficulty::Pu),
                        category: self
                            .model
                            .online_categories
                            .get(self.model.online_category)
                            .map(|(id, _)| id.clone())
                            .unwrap_or_default(),
                        ..FetchParams::default()
                    }
                };
                self.submit_online(OnlineCommand::Fetch(p));
            }
            Action::OnlineShelf(i) => {
                let chars = match self.model.field("online_chars").trim().parse::<usize>() {
                    Ok(n) if (1..=100000).contains(&n) => n,
                    _ => {
                        self.toast("目标字数请输入1至100000之间的整数");
                        return;
                    }
                };
                if let Some(b) = self.shelf.get(i) {
                    self.submit_online(OnlineCommand::Fetch(FetchParams {
                        book_id: b.book_id.clone(),
                        use_shelf: true,
                        sequential: b.mode == "SEQUENTIAL",
                        chars,
                        strict_length: self.model.online_strict,
                        difficulty: Difficulty::from_level(self.model.online_difficulty as i64 + 1)
                            .unwrap_or(Difficulty::Pu),
                        ..FetchParams::default()
                    }));
                }
            }
            Action::OnlineLoad => self.load_online_article(),
            Action::RuleSelect(i) => {
                self.model.rule_selected = i;
                self.sync_rule_model();
            }
            Action::RuleToggle(i) => {
                if let Some(r) = self.cfg.cleaning_preset.rules.get_mut(i) {
                    r.enabled = !r.enabled;
                }
                if let (Some(rule), Some(view)) = (
                    self.cfg.cleaning_preset.rules.get(i),
                    self.model.rules.get_mut(i),
                ) {
                    view.1 = rule.enabled;
                }
                self.save_config();
            }
            Action::RuleUp | Action::RuleDown => {
                let i = self.model.rule_selected;
                let j = if a == Action::RuleUp {
                    i.saturating_sub(1)
                } else {
                    (i + 1).min(self.cfg.cleaning_preset.rules.len().saturating_sub(1))
                };
                if i < self.cfg.cleaning_preset.rules.len() {
                    self.cfg.cleaning_preset.rules.swap(i, j);
                    self.model.rule_selected = j;
                    self.sync_rule_model();
                    self.save_config();
                }
            }
            Action::RuleAdd => {
                self.cfg.cleaning_preset.rules.push(CleaningRule {
                    id: format!("custom-{}", unix_secs()),
                    name: "自定义规则".into(),
                    pattern: String::new(),
                    replacement: String::new(),
                    enabled: false,
                    safe: false,
                    builtin: false,
                });
                self.model.rule_selected = self.cfg.cleaning_preset.rules.len() - 1;
                self.sync_rule_model();
            }
            Action::RuleDelete => {
                let i = self.model.rule_selected;
                if i < self.cfg.cleaning_preset.rules.len() {
                    self.cfg.cleaning_preset.rules.remove(i);
                    self.sync_rule_model();
                    self.save_config();
                }
            }
            Action::RuleSave => {
                let i = self.model.rule_selected;
                let mut p = self.cfg.cleaning_preset.clone();
                if let Some(r) = p.rules.get_mut(i) {
                    r.name = self.model.field("rule_name");
                    let pattern = self.model.field("rule_pattern");
                    let replacement = self.model.field("rule_replacement");
                    if r.pattern != pattern || r.replacement != replacement {
                        r.safe = false;
                        r.builtin = false;
                    }
                    r.pattern = pattern;
                    r.replacement = replacement;
                }
                match p.validate() {
                    Ok(()) => {
                        self.cfg.cleaning_preset = p;
                        self.sync_rule_model();
                        self.save_config();
                        self.toast("规则已保存");
                    }
                    Err(e) => {
                        self.model.rule_error = e.clone();
                        self.toast(e)
                    }
                }
            }
            Action::RuleTest => {
                if self.rule_busy {
                    self.toast("规则正在处理，请稍候");
                    return;
                }
                let mut p = self.cfg.cleaning_preset.clone();
                if let Some(r) = p.rules.get_mut(self.model.rule_selected) {
                    r.pattern = self.model.field("rule_pattern");
                    r.replacement = self.model.field("rule_replacement");
                }
                let text = self.model.field("rule_test");
                self.rule_generation += 1;
                let g = self.rule_generation;
                self.rule_busy = true;
                let tx = self.tx.clone();
                let hwnd = self.win.hwnd;
                std::thread::spawn(move || {
                    let r = gale_core::rules::clean_body(&text, &p, TrainingMode::Practice);
                    let _ = tx.send(Bg::RuleTest(g, r));
                    wake(hwnd);
                });
            }
            Action::RuleApply => {
                if self.rule_busy {
                    self.toast("规则正在处理，请稍候");
                    return;
                }
                if self.sess.frozen_rules.is_some() {
                    self.toast("开打后不能修改正文，请先重打");
                    return;
                }
                self.rule_busy = true;
                let mut sess = self.sess.clone();
                let p = self.cfg.cleaning_preset.clone();
                let g = self.generation;
                let rule = self.rule_generation;
                let tx = self.tx.clone();
                let hwnd = self.win.hwnd;
                std::thread::spawn(move || {
                    let r = sess.apply_cleaning(&p).map(|()| sess);
                    let _ = tx.send(Bg::RuleApply(g, rule, r));
                    wake(hwnd);
                });
            }
            Action::RuleRestore => match self.sess.restore_raw() {
                Ok(()) => {
                    self.reset_host();
                    self.toast("已还原原文");
                }
                Err(e) => self.toast(e),
            },
            Action::RuleImport => {
                if let Some(p) = file_dialog(self.win.hwnd, false, "json") {
                    match std::fs::metadata(&p)
                        .map_err(|e| e.to_string())
                        .and_then(|m| {
                            if m.len() > 131072 {
                                Err("规则文件超过 128 KiB".into())
                            } else {
                                std::fs::read_to_string(&p).map_err(|e| e.to_string())
                            }
                        })
                        .and_then(|s| CleaningPreset::import_json(&s))
                    {
                        Ok(p) => {
                            self.cfg.cleaning_preset = p;
                            self.sync_rule_model();
                            self.save_config();
                        }
                        Err(e) => self.toast(e),
                    }
                }
            }
            Action::RuleExport => {
                if let Some(p) = file_dialog(self.win.hwnd, true, "json") {
                    match self
                        .cfg
                        .cleaning_preset
                        .export_json()
                        .and_then(|s| std::fs::write(p, s).map_err(|e| e.to_string()))
                    {
                        Ok(()) => self.toast("规则已导出"),
                        Err(e) => self.toast(e),
                    }
                }
            }
            Action::QuoteToggle => {
                let mut r = self.sess.rules.clone();
                {
                    r.quotes.enabled = !r.quotes.enabled;
                    match self.sess.set_rules(r) {
                        Ok(()) => {
                            self.model.quote_relaxed = self.sess.rules.quotes.enabled;
                            self.save_config();
                        }
                        Err(e) => self.toast(e),
                    }
                }
            }
        }
        gale_win::window::request_frame();
    }
}
fn file_dialog(hwnd: isize, save: bool, ext: &str) -> Option<PathBuf> {
    use windows::Win32::UI::Controls::Dialogs::*;
    unsafe {
        let mut buffer = vec![0u16; 32768];
        let filter: Vec<u16> = format!("{} 文件\0*.{}\0所有文件\0*.*\0\0", ext.to_uppercase(), ext)
            .encode_utf16()
            .collect();
        let suffix: Vec<u16> = ext.encode_utf16().chain(Some(0)).collect();
        let mut ofn = OPENFILENAMEW::default();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = HWND(hwnd as *mut _);
        ofn.lpstrFile = windows::core::PWSTR(buffer.as_mut_ptr());
        ofn.nMaxFile = buffer.len() as u32;
        ofn.lpstrFilter = windows::core::PCWSTR(filter.as_ptr());
        ofn.lpstrDefExt = windows::core::PCWSTR(suffix.as_ptr());
        ofn.Flags = OFN_EXPLORER
            | OFN_NOCHANGEDIR
            | if save {
                OFN_OVERWRITEPROMPT
            } else {
                OFN_FILEMUSTEXIST
            };
        let ok = if save {
            GetSaveFileNameW(&mut ofn)
        } else {
            GetOpenFileNameW(&mut ofn)
        };
        if ok.as_bool() {
            let len = buffer.iter().position(|c| *c == 0).unwrap_or(0);
            Some(PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
        } else {
            None
        }
    }
}
fn local_port() -> u16 {
    let path = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("HuFu/http_port.txt");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(4390)
}
fn local_json(
    port: u16,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let mut s = std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_secs(2),
    )
    .map_err(|_| "虎符服务未连接".to_string())?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(4)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let data = body.map(|v| v.to_string()).unwrap_or_default();
    let method = if data.is_empty() { "GET" } else { "POST" };
    let req=format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",data.len());
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut b = Vec::new();
    s.take(4_000_001)
        .read_to_end(&mut b)
        .map_err(|e| e.to_string())?;
    if b.len() > 4_000_000 {
        return Err("服务响应过长".into());
    }
    let sep = b
        .windows(4)
        .position(|v| v == b"\r\n\r\n")
        .ok_or("服务响应无效")?;
    let head = String::from_utf8_lossy(&b[..sep]);
    let code = head
        .lines()
        .next()
        .and_then(|s| s.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    if code != 200 {
        if code == 404 {
            return Err("当前虎符服务尚不支持跟打接口，请升级虎符".into());
        }
        let detail = serde_json::from_slice::<serde_json::Value>(&b[sep + 4..])
            .ok()
            .and_then(|v| {
                v["error"]
                    .as_str()
                    .or_else(|| v["message"].as_str())
                    .map(str::to_string)
            });
        return Err(detail.unwrap_or_else(|| match code {
            403 => "虎符服务拒绝连接，请检查本地服务设置".into(),
            413 => "文字超过服务字数上限，请缩短段落".into(),
            429 => "服务正在处理其他请求，请稍后重试".into(),
            _ => format!("虎符服务返回HTTP {code}"),
        }));
    }
    serde_json::from_slice(&b[sep + 4..]).map_err(|_| "服务数据无效".into())
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn saved_score_presets_keep_core_metrics_and_custom_templates_stay_free() {
        let mut cfg = Config::default();
        assert_eq!(current_score_preset(&cfg), "standard");
        let record = fixture();
        for id in ["standard", "compact"] {
            cfg.score_template = if id == "standard" {
                String::new()
            } else {
                gale_core::scorecard::preset(id).template.into()
            };
            let loaded = Config::from_toml_text(&cfg.to_toml_text()).unwrap();
            assert_eq!(current_score_preset(&loaded), id);
            let text = gale_core::scorecard::format_score_template(
                &record,
                &configured_template(&loaded),
                "虎符",
            );
            for label in ["速度", "键准", "击键", "码长"] {
                assert!(text.contains(label), "{id}: {text}");
            }
        }
        cfg.score_fields = "segment,speed,key_accuracy,code_len,word_rate,select_count,retype_count,chars,time,keys,backspaces,wrong,signature".into();
        cfg.score_template.clear();
        assert_eq!(current_score_preset(&cfg), "standard");
        assert!(configured_template(&cfg).contains("击键{strokes}"));
        cfg.score_template = "我的成绩：  {speed}\n  {keys}".into();
        let loaded = Config::from_toml_text(&cfg.to_toml_text()).unwrap();
        assert_eq!(current_score_preset(&loaded), "custom");
        assert_eq!(configured_template(&loaded), cfg.score_template);
    }

    fn live_status_value(version: &str, unavailable: Option<&str>) -> serde_json::Value {
        let mut value = serde_json::json!({"schema_ver":version,"schema":"测试方案"});
        if let Some(reason) = unavailable {
            value["hint_unavailable_reason"] = reason.into();
        }
        value
    }

    fn accept_live_status(
        live: &mut LiveSchemaStatus,
        now: u64,
        version: &str,
        unavailable: Option<&str>,
    ) -> bool {
        let request = live.begin(now).expect("status request is due");
        match live.complete(request, now, Ok(live_status_value(version, unavailable))) {
            StatusOutcome::Ready { changed } => changed,
            _ => panic!("valid status must be accepted"),
        }
    }

    #[test]
    fn live_schema_revision_rejects_old_analysis_after_a_b_a_and_coalesces_latest() {
        let mut live = LiveSchemaStatus::default();
        assert!(accept_live_status(&mut live, 0, "A", None));
        let old_a_request = live.revision.clone();
        // The analysis lane stays busy while status polling observes changes.
        assert!(accept_live_status(&mut live, 400, "B", None));
        assert!(accept_live_status(&mut live, 800, "C", None));
        assert!(accept_live_status(&mut live, 1200, "A", None));
        assert!(!live.revision.accepts(&old_a_request, "A"));
        let latest_request = live.revision.clone();
        assert!(live.revision.accepts(&latest_request, "A"));
        assert!(!live.revision.accepts(&latest_request, "B"));
        assert_eq!(
            live.revision.version, "A",
            "analyze replies cannot advance status"
        );
        assert!(!accept_live_status(&mut live, 1600, "A", None));
        assert_eq!(
            live.revision, latest_request,
            "unchanged polls do not reroute static hints"
        );
    }

    #[test]
    fn live_schema_poll_is_single_flight_and_old_status_cannot_release_a_new_request() {
        let mut live = LiveSchemaStatus::default();
        let first = live.begin(0).unwrap();
        assert!(live.begin(4000).is_none(), "slow polls never overlap");
        assert!(matches!(
            live.complete(first, 0, Ok(live_status_value("A", None))),
            StatusOutcome::Ready { changed: true }
        ));
        assert!(live.begin(399).is_none());
        let second = live.begin(400).unwrap();
        assert!(matches!(
            live.complete(first, 401, Ok(live_status_value("old", None))),
            StatusOutcome::Ignored
        ));
        assert_eq!(live.in_flight, Some(second));
        assert_eq!(live.revision.version, "A");
        assert!(matches!(
            live.complete(second, 402, Ok(live_status_value("B", None))),
            StatusOutcome::Ready { changed: true }
        ));
        assert_eq!(live.revision.version, "B");
    }

    #[test]
    fn live_schema_unavailable_changes_and_recovery_invalidate_even_empty_versions() {
        let mut live = LiveSchemaStatus::default();
        assert!(accept_live_status(&mut live, 0, "A", None));
        let old_a = live.revision.clone();
        assert!(accept_live_status(&mut live, 400, "", Some("源目录已删除")));
        let deleted = live.revision.clone();
        assert!(!live.revision.accepts(&old_a, "A"));
        assert!(accept_live_status(&mut live, 800, "", Some("码表格式损坏")));
        assert_ne!(
            live.revision, deleted,
            "same empty version still tracks the failure reason"
        );
        assert!(accept_live_status(&mut live, 1200, "A", None));
        assert!(!live.revision.accepts(&old_a, "A"));
        assert!(live.revision.unavailable_reason.is_none());
    }

    #[test]
    fn live_schema_failures_back_off_with_a_limit_and_recover_automatically() {
        let mut live = LiveSchemaStatus::default();
        let mut now = 0;
        for delay in [800, 1600, 3200, 4000, 4000, 4000] {
            let request = live.begin(now).unwrap();
            assert!(live.begin(now + 1).is_none());
            assert!(matches!(
                live.complete(request, now, Err("服务未连接".into())),
                StatusOutcome::Failed(_)
            ));
            assert_eq!(live.next_poll_at, now + delay);
            assert!(live.begin(now + delay - 1).is_none());
            now += delay;
        }
        assert!(accept_live_status(&mut live, now, "recovered", None));
        assert_eq!(live.next_poll_at, now + 400);
        assert_eq!(live.retry.failures, 0);
        assert!(live.ready());
        live.request_fresh(now + 100);
        assert!(
            !live.ready(),
            "mismatched analyze responses wait for authoritative status"
        );
        assert_eq!(live.next_poll_at, now + 100);
        assert!(!accept_live_status(&mut live, now + 100, "recovered", None));
        assert!(live.ready());
    }

    #[test]
    fn live_schema_transient_busy_preserves_the_last_revision_and_cached_status() {
        let mut live = LiveSchemaStatus::default();
        assert!(accept_live_status(&mut live, 0, "A", None));
        let before_revision = live.revision.clone();
        let before_snapshot = live.snapshot.clone();
        let request = live.begin(400).unwrap();
        assert!(matches!(
            live.complete(
                request,
                400,
                Ok(live_status_value("", Some("输入引擎忙，请稍后重试"))),
            ),
            StatusOutcome::Failed(error) if error == "输入引擎忙，请稍后重试"
        ));
        assert_eq!(live.revision, before_revision);
        assert_eq!(live.snapshot, before_snapshot);
        assert!(live.ready(), "the last usable status remains available");
        assert!(live.revision.accepts(&before_revision, "A"));
        assert_eq!(live.next_poll_at, 1200);
        assert!(live.begin(1199).is_none());
        assert!(!accept_live_status(&mut live, 1200, "A", None));
        assert_eq!(live.revision, before_revision);
        assert_eq!(live.retry.failures, 0);

        // Similar text is not a transient-status exemption. Real unavailable
        // sources must still clear old views, including during recovery.
        assert!(accept_live_status(
            &mut live,
            1600,
            "",
            Some("输入引擎忙，请稍后重试：码表来源损坏"),
        ));
        assert!(!live.revision.accepts(&before_revision, "A"));
        assert!(accept_live_status(&mut live, 2000, "A", None));
        assert!(live.revision.unavailable_reason.is_none());
    }

    #[test]
    fn live_schema_view_invalidation_preserves_typing_composition_and_scores() {
        let mut sess = Session::new(100);
        sess.load_book("不中断练习", "甲乙丙丁戊己");
        sess.feed_input(
            gale_core::compare::InputEvent::Commit {
                chars: "甲乙".into(),
            },
            10,
        );
        sess.set_composing(true);
        sess.set_preedit("bing".into());
        sess.done_chars = 123;
        sess.done_keys = 456;
        sess.full_analysis = Some(FullAnalysis::default());
        sess.vk_histogram.insert(0x41, 3);
        let before = serde_json::to_value(&sess).unwrap();
        let mut model = Model::default();
        model.words.push(modals::HintWord {
            text: "甲乙".into(),
            len: 2,
            ..Default::default()
        });
        model
            .fields
            .insert("analysis_error".into(), "old error".into());
        clear_live_hint_views(&mut model, &mut sess);
        assert_eq!(serde_json::to_value(&sess).unwrap(), before);
        assert_eq!(sess.vk_histogram.get(&0x41), Some(&3));
        assert!(sess.composing);
        assert_eq!(sess.preedit, "bing");
        assert!(sess.full_analysis.is_none());
        assert!(model.words.is_empty());
        assert!(model.field("analysis_error").is_empty());
    }

    #[test]
    fn route_capability_echo_rejects_ignored_strategy_spans_or_dynamic_boundary() {
        let spans = vec![RequiredSpan { start: 1, len: 2 }];
        let expected = serde_json::json!({
            "route_rules":"quiettype-v1", "route_mode":"display", "strategy":"phrases",
            "required_spans":[{"start":1,"len":2}], "dynamic_boundary":3,
        });
        assert!(validate_route_response(
            &expected,
            HintStrategy::Phrases,
            &spans,
            Some(3),
            "display"
        )
        .is_ok());
        for (field, wrong) in [
            ("route_rules", serde_json::json!("old")),
            ("route_mode", serde_json::json!("theory")),
            ("strategy", serde_json::json!("shortest")),
            ("required_spans", serde_json::json!([])),
            ("dynamic_boundary", serde_json::json!(2)),
        ] {
            let mut response = expected.clone();
            response[field] = wrong;
            assert!(
                validate_route_response(
                    &response,
                    HintStrategy::Phrases,
                    &spans,
                    Some(3),
                    "display"
                )
                .is_err(),
                "{field}"
            );
        }
        assert!(validate_route_response(
            &serde_json::json!({}),
            HintStrategy::Shortest,
            &[],
            None,
            "theory"
        )
        .is_err());
    }

    #[test]
    fn route_cache_keys_separate_display_progress_strategy_and_theory() {
        let key = |strategy, boundary, mode| {
            analysis_request_key(
                "甲😀乙",
                &[2],
                false,
                "schema",
                true,
                strategy,
                &[],
                boundary,
                mode,
            )
        };
        assert_ne!(
            key(HintStrategy::Shortest, None, "display"),
            key(HintStrategy::Phrases, None, "display")
        );
        assert_ne!(
            key(HintStrategy::Shortest, Some(1), "display"),
            key(HintStrategy::Shortest, Some(2), "display")
        );
        assert_ne!(
            key(HintStrategy::Shortest, None, "display"),
            key(HintStrategy::Shortest, None, "theory")
        );
        let locked = analysis_request_key(
            "甲😀乙",
            &[2],
            false,
            "schema",
            true,
            HintStrategy::Shortest,
            &[RequiredSpan { start: 0, len: 2 }],
            None,
            "display",
        );
        assert_ne!(locked, key(HintStrategy::Shortest, None, "display"));
    }

    #[test]
    fn dynamic_refresh_keeps_a_bounded_window_during_continuous_input() {
        let mut seen = Some(0);
        let mut due = None;
        let mut dispatched = Vec::new();
        // At 16ms per commit there is never an 80ms input pause. Requests
        // still dispatch at the latest boundary in each bounded window.
        for step in 0..18 {
            let boundary = Some(step + 1);
            let now = step as u64 * 16;
            if dynamic_hint_refresh_due(boundary, &mut seen, &mut due, now) {
                dispatched.push((now, seen));
            }
        }
        assert_eq!(
            dispatched,
            [(80, Some(6)), (176, Some(12)), (272, Some(18))]
        );
        assert!(due.is_none());
        assert!(!dynamic_hint_refresh_due(
            Some(18),
            &mut seen,
            &mut due,
            300
        ));

        // Backspacing all input is an ordinary changed boundary, not a
        // disabled dynamic mode. It dispatches Some(0) to restore base words.
        assert!(!dynamic_hint_refresh_due(Some(0), &mut seen, &mut due, 320));
        assert_eq!(due, Some(400));
        assert!(dynamic_hint_refresh_due(Some(0), &mut seen, &mut due, 400));
        assert_eq!(seen, Some(0));
        assert!(due.is_none());
    }

    #[test]
    fn dynamic_progress_uses_only_the_correct_prefix_and_recovers_on_backspace() {
        use gale_core::compare::InputEvent;
        let mut sess = Session::new(100);
        sess.load_book("动态", "“甲😀乙”");
        sess.feed_input(
            InputEvent::Commit {
                chars: "\"甲😀错”".into(),
            },
            10,
        );
        assert_eq!(sess.comparer.pos, 5);
        assert_eq!(
            dynamic_prefix_boundary(&sess),
            3,
            "a later matching quote cannot skip the wrong character"
        );
        // The comparer has reached Done; use its input stream directly to
        // exercise the same prefix calculation during correction/recovery.
        sess.comparer.feed(&InputEvent::Backspace, 20);
        sess.comparer.feed(&InputEvent::Backspace, 30);
        assert_eq!(dynamic_prefix_boundary(&sess), 3);
        sess.comparer.feed(&InputEvent::Backspace, 40);
        assert_eq!(dynamic_prefix_boundary(&sess), 2, "non-BMP is one scalar");
        sess.comparer.quote_rules.enabled = false;
        assert_eq!(
            dynamic_prefix_boundary(&sess),
            0,
            "quote compatibility follows the current round"
        );
        sess.comparer.quote_rules.enabled = true;
        assert_eq!(dynamic_prefix_boundary(&sess), 2);
        sess.comparer.feed(&InputEvent::Backspace, 50);
        sess.comparer.feed(&InputEvent::Backspace, 60);
        assert_eq!(dynamic_prefix_boundary(&sess), 0);
    }

    #[test]
    fn analysis_boundaries_and_keys_follow_rounds_in_unicode_scalars() {
        let segments = |texts: &[&str]| {
            texts
                .iter()
                .map(|text| gale_core::segment::Segment {
                    mark: String::new(),
                    text: (*text).into(),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            segment_analysis_breaks(&segments(&["", "𠮷甲", "", "乙", ""])),
            [2]
        );
        assert!(segment_analysis_breaks(&segments(&["", "", ""])).is_empty());
        assert!(segment_analysis_breaks(&segments(&["𠮷甲乙"])).is_empty());
        let joined = "𠮷甲乙";
        let split_a = segment_analysis_breaks(&segments(&["𠮷", "甲乙"]));
        let split_b = segment_analysis_breaks(&segments(&["𠮷甲", "乙"]));
        let key = |breaks: &[usize]| {
            analysis_request_key(
                joined,
                breaks,
                false,
                "schema",
                true,
                HintStrategy::Shortest,
                &[],
                None,
                "theory",
            )
        };
        assert_ne!(
            key(&split_a),
            key(&split_b),
            "same text with different rounds needs separate theory cache"
        );
        assert_ne!(key(&split_a), key(&[]));
        assert_eq!(key(&split_a), key(&[1]));
    }
    #[test]
    fn completed_round_continues_only_from_typing_view() {
        let panels = [
            Modal::None,
            Modal::Groups,
            Modal::Online,
            Modal::Rules,
            Modal::Confirm,
            Modal::More,
        ];
        for panel in panels {
            for key in [CommandKey::Enter, CommandKey::Space] {
                assert_eq!(
                    completed_segment_action(key, true, panel),
                    (panel == Modal::None).then_some(Action::Next),
                    "panel={panel:?} key={key:?}"
                );
                assert_eq!(completed_segment_action(key, false, panel), None);
            }
        }
    }

    #[test]
    fn escape_only_dismisses_panels_and_leaves_the_round_running() {
        assert_eq!(
            escape_action(Modal::Preferences, true, true),
            Some(Action::Close)
        );
        assert_eq!(
            escape_action(Modal::None, true, true),
            Some(Action::ToggleLedger)
        );
        assert_eq!(
            escape_action(Modal::None, false, true),
            Some(Action::ToggleAnalysis)
        );
        let mut sess = Session::new(100);
        sess.load_book("Esc", "甲乙");
        dispatch_host_events(
            &mut sess,
            vec![HostEvent::Key { vk: 0x41 }, HostEvent::Commit("甲".into())],
            true,
            100,
        );
        assert_eq!(escape_action(Modal::None, false, false), None);
        assert_eq!(sess.phase, Phase::Typing);
        assert_eq!(sess.elapsed_ms(600), 500);
        assert!(!sess.is_paused());
        assert!(!sess.recorded);
        assert!(sess.finished_ms.is_none());
        dispatch_host_events(
            &mut sess,
            vec![HostEvent::Key { vk: 0x42 }, HostEvent::Commit("乙".into())],
            true,
            1100,
        );
        assert_eq!(sess.phase, Phase::Done);
        assert_eq!(sess.elapsed_ms(1100), 1000);
    }

    #[test]
    fn refocus_waits_for_a_physical_typing_key_before_processing_its_commit() {
        let mut sess = Session::new(100);
        sess.load_book("焦点", "甲乙丙");
        dispatch_host_events(
            &mut sess,
            vec![HostEvent::Key { vk: 0x41 }, HostEvent::Commit("甲".into())],
            true,
            100,
        );
        sess.set_paused(true, 200);
        dispatch_host_events(&mut sess, vec![], true, 1000);
        dispatch_host_events(
            &mut sess,
            vec![HostEvent::Commit("错".into()), HostEvent::Backspace],
            true,
            1100,
        );
        dispatch_host_events(
            &mut sess,
            vec![
                HostEvent::Key { vk: 0x10 },
                HostEvent::Preedit("b".into()),
                HostEvent::Composing(true),
            ],
            true,
            1200,
        );
        assert_eq!(escape_action(Modal::None, false, false), None);
        assert!(sess.is_paused());
        assert_eq!(sess.elapsed_ms(1900), 100);
        assert_eq!(sess.typed_len(), 1);
        assert_eq!(sess.stats.keys, 1);
        dispatch_host_events(
            &mut sess,
            vec![
                HostEvent::Composing(false),
                HostEvent::Key { vk: 0x42 },
                HostEvent::Commit("乙".into()),
            ],
            true,
            2000,
        );
        assert!(!sess.is_paused());
        assert_eq!(sess.elapsed_ms(2000), 100);
        assert_eq!(sess.typed_len(), 2);
        assert_eq!(sess.stats.keys, 2);
        dispatch_host_events(
            &mut sess,
            vec![HostEvent::Key { vk: 0x43 }, HostEvent::Commit("丙".into())],
            true,
            2100,
        );
        assert_eq!(sess.phase, Phase::Done);
        assert_eq!(sess.elapsed_ms(9000), 200);
        assert_eq!(sess.stats.keys, 3);
    }

    #[test]
    fn background_and_modal_keys_cannot_resume_the_clock() {
        let mut sess = Session::new(100);
        sess.load_book("焦点", "甲乙");
        dispatch_host_events(
            &mut sess,
            vec![HostEvent::Key { vk: 0x41 }, HostEvent::Commit("甲".into())],
            true,
            100,
        );
        sess.set_paused(true, 200);
        dispatch_host_events(
            &mut sess,
            vec![HostEvent::Key { vk: 0x42 }, HostEvent::Commit("乙".into())],
            false,
            1200,
        );
        assert!(sess.is_paused());
        assert_eq!(sess.phase, Phase::Typing);
        assert_eq!(sess.typed_len(), 1);
        assert_eq!(sess.stats.keys, 1);
        assert_eq!(sess.elapsed_ms(1900), 100);
    }

    #[test]
    fn focus_loss_flushes_the_last_commit_before_latching_the_pause() {
        let mut sess = Session::new(100);
        sess.load_book("焦点", "甲乙丙");
        dispatch_host_events(&mut sess, vec![HostEvent::Key { vk: 0x41 }], true, 100);
        dispatch_host_events(
            &mut sess,
            vec![HostEvent::FocusLost, HostEvent::Commit("甲".into())],
            false,
            200,
        );
        assert_eq!(sess.typed_len(), 1, "the IME blur flush is kept once");
        assert!(sess.is_paused());
        dispatch_host_events(&mut sess, vec![HostEvent::Commit("错".into())], true, 1000);
        assert_eq!(sess.typed_len(), 1, "a later notification cannot resume");
        dispatch_host_events(
            &mut sess,
            vec![
                HostEvent::Key { vk: 0x42 },
                HostEvent::FocusLost,
                HostEvent::Commit("乙".into()),
            ],
            true,
            1100,
        );
        assert!(
            sess.is_paused(),
            "a quick refocus does not undo deactivation"
        );
        assert_eq!(sess.typed_len(), 2);
        dispatch_host_events(
            &mut sess,
            vec![
                HostEvent::FocusLost,
                HostEvent::Key { vk: 0x43 },
                HostEvent::Commit("丙".into()),
            ],
            true,
            2100,
        );
        assert!(!sess.is_paused(), "only a key after deactivation resumes");
        assert_eq!(sess.phase, Phase::Done);
        assert_eq!(sess.elapsed_ms(9000), 100);
        assert_eq!(sess.stats.keys, 3);
    }

    #[test]
    fn paused_commit_causes_are_discarded_but_blur_flush_causes_are_kept() {
        use gale_win::input_host::CommitCause;
        let mut sess = Session::new(100);
        sess.load_book("归因", "甲乙丙");
        dispatch_host_events(&mut sess, vec![HostEvent::Key { vk: 0x41 }], true, 100);
        let flushed = dispatch_host_events(
            &mut sess,
            vec![HostEvent::FocusLost, HostEvent::Commit("甲".into())],
            false,
            200,
        );
        flushed.apply_commit_causes(&mut sess, vec![CommitCause::Select]);
        assert!(sess.is_paused());
        assert_eq!(
            sess.stats.select_count, 1,
            "the accepted blur flush keeps its cause"
        );
        let delayed =
            dispatch_host_events(&mut sess, vec![HostEvent::Commit("错".into())], true, 1200);
        delayed.apply_commit_causes(&mut sess, vec![CommitCause::Select]);
        assert_eq!(sess.typed_len(), 1);
        assert_eq!(
            sess.stats.select_count, 1,
            "a rejected commit cannot change statistics"
        );
        let resumed = dispatch_host_events(
            &mut sess,
            vec![HostEvent::Key { vk: 0x42 }, HostEvent::Commit("乙".into())],
            true,
            2200,
        );
        resumed.apply_commit_causes(&mut sess, vec![CommitCause::Select]);
        assert_eq!(sess.stats.select_count, 2);
    }

    #[test]
    fn undo_redo_and_special_selection_keys_resume_without_inventing_stat_keys() {
        let mut sess = Session::new(100);
        sess.load_book("编辑", "甲乙");
        dispatch_host_events(
            &mut sess,
            vec![HostEvent::Key { vk: 0x41 }, HostEvent::Commit("甲".into())],
            true,
            100,
        );
        sess.set_paused(true, 200);
        dispatch_host_events(
            &mut sess,
            vec![HostEvent::ResumeKey { vk: 0x5a }, HostEvent::Backspace],
            true,
            1200,
        );
        assert!(!sess.is_paused());
        assert_eq!(sess.typed_len(), 0);
        assert_eq!(
            sess.stats.keys, 1,
            "Ctrl+Z is still excluded from typing keys"
        );
        assert_eq!(sess.elapsed_ms(1300), 200);
        sess.set_paused(true, 1300);
        dispatch_host_events(
            &mut sess,
            vec![
                HostEvent::ResumeKey { vk: 0x59 },
                HostEvent::Commit("甲".into()),
            ],
            true,
            2300,
        );
        assert_eq!(sess.typed_len(), 1);
        assert_eq!(sess.stats.keys, 1);
        sess.set_paused(true, 2400);
        dispatch_host_events(
            &mut sess,
            vec![HostEvent::CandidateSelection { rank: 3, vk: None }],
            true,
            3000,
        );
        assert!(
            sess.is_paused(),
            "a candidate notification is not a fresh key"
        );
        dispatch_host_events(
            &mut sess,
            vec![
                HostEvent::ResumeKey { vk: 0x14 },
                HostEvent::CandidateSelection {
                    rank: 3,
                    vk: Some(0x14),
                },
                HostEvent::Commit("乙".into()),
            ],
            true,
            3400,
        );
        assert_eq!(sess.phase, Phase::Done);
        assert_eq!(
            sess.stats.keys, 2,
            "the proven selection key is counted once"
        );
        assert_eq!(sess.stats.select_count, 1);
        assert_eq!(sess.elapsed_ms(9000), 300);
    }

    fn fixture() -> gale_core::score::ScoreRecord {
        let mut s = Session::new(100);
        s.load_book("test", "甲");
        s.on_key(65, 10);
        s.feed_input(
            gale_core::compare::InputEvent::Commit {
                chars: "甲".into()
            },
            30,
        );
        s.finalize_current_at(30, 123).unwrap()
    }
    #[test]
    fn locked_score_retry_is_durable_without_duplicate_or_losing_equal_rounds() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!(
            "hufu-save-retry-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = ScoreStore::open(&dir.join("统计")).unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = persistence(dir.clone(), tx, 0);
        let r = fixture();
        let send = |id| {
            worker.send(Persist::Score(id, r.clone())).unwrap();
            match rx.recv_timeout(Duration::from_secs(3)).unwrap() {
                Bg::ScoreSaved(v) => v,
                _ => panic!("missing score acknowledgement"),
            }
        };
        assert!(send(1).is_ok());
        assert!(send(1).is_ok());
        assert_eq!(store.load_all().len(), 1);
        assert!(send(2).is_ok());
        assert_eq!(store.load_all().len(), 2);
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(store.path())
            .unwrap();
        assert!(send(3).is_err());
        drop(lock);
        assert!(send(3).is_ok());
        assert_eq!(store.load_all().len(), 3);
        let (ack, done) = mpsc::channel();
        worker.send(Persist::Stop(ack)).unwrap();
        assert!(done.recv().unwrap().is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn shutdown_reports_a_real_score_write_failure() {
        let dir = std::env::temp_dir().join(format!(
            "hufu-save-error-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("统计/records.jsonl")).unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = persistence(dir.clone(), tx, 0);
        worker.send(Persist::Score(1, fixture())).unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            Bg::ScoreSaved(Err(_))
        ));
        let (ack, done) = mpsc::channel();
        worker.send(Persist::Stop(ack)).unwrap();
        assert!(done.recv().unwrap().is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn session_snapshot_persists_while_score_directory_is_unwritable() {
        let dir = std::env::temp_dir().join(format!(
            "hufu-snapshot-score-failure-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // An ordinary file at the required directory makes score writes fail
        // deterministically without changing user ACLs or requiring elevation.
        std::fs::write(dir.join("统计"), b"blocked").unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = persistence(dir.clone(), tx, 0);
        worker.send(Persist::Score(1, fixture())).unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            Bg::ScoreSaved(Err(_))
        ));
        let mut session = Session::new(100);
        session.load_book("可恢复文章", "甲乙丙丁");
        let bytes = session.snapshot_bytes_bounded().unwrap();
        worker.try_send(Persist::Snapshot(bytes.clone())).unwrap();
        let (ack, done) = mpsc::channel();
        worker.send(Persist::Stop(ack)).unwrap();
        // Snapshot success cannot mask the outstanding failed score.
        assert!(done.recv_timeout(Duration::from_secs(3)).unwrap().is_err());
        assert_eq!(std::fs::read(dir.join("session.jsonl")).unwrap(), bytes);
        let restored = Session::restore_snapshot(&dir.join("session.jsonl")).unwrap();
        assert_eq!(restored.current_segment().text, "甲乙丙丁");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn local_api_404_keeps_upgrade_guidance_over_generic_json_detail() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            connection
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0u8; 2048];
            assert!(connection.read(&mut request).unwrap() > 0);
            let body = r#"{"error":"not found"}"#;
            let response = format!(
                "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            connection.write_all(response.as_bytes()).unwrap();
        });
        let error = local_json(port, "/api/trainer/status", None).unwrap_err();
        server.join().unwrap();
        assert_eq!(error, "当前虎符服务尚不支持跟打接口，请升级虎符");
    }
}
