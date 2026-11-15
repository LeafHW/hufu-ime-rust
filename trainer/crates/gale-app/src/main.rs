//! 虎符跟打：真实 RichEdit/TSF 输入，原生 D2D 界面，后台联网。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod analysis;
mod ctxmenu;
mod history;
mod keyboard;
mod modals;
mod online;
mod overlay;
mod profile;
mod qq_bridge;
mod secondary_panels;
mod session;
mod settings_writer;
mod sta_worker;
mod skin;
mod ui;
mod uia_bridge;
use gale_core::{
    config::{Config, DisplayMode, HintStrategy},
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
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetFocus, GetKeyState, ReleaseCapture, VK_CONTROL, VK_LBUTTON,
};
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

/// 私有标记通道（整改2/5/8/13）：modals.rs（禁改域）暂无对应 Action 变体。
/// 借用 `Action::Theme(String)` 载荷携带 `\u{e000}` 前缀命令。真实主题名
/// 永不占用 U+E000，不会与外观页主题切换冲突；modals.rs 落正式变体
/// （Action::OnlineRegister / ShowFullAnalysis）后，把发射点换绑即可，main.rs 这边的解码可原样退役。
const AUX_PREFIX: char = '\u{e000}';
const AUX_REGISTER: &str = "online_register";
const AUX_REGISTER_CODE: &str = "online_reg_code";
const AUX_REGISTER_SUBMIT: &str = "online_reg_submit";
const AUX_AUTO_NEXT: &str = "online_auto_next";
const AUX_SHOW_FULL: &str = "show_full_analysis";
const AUX_SHOW_HEATMAP: &str = "show_heatmap";

/// 独立二级窗比面板大一圈的留白（左右 / 上下）。面板在窗内仍按
/// `modals::panel_for_model` 的居中位落位，等于窗口四周一圈柔和边框。
const PAGE_MARGIN_X: f32 = 16.;
const PAGE_MARGIN_Y: f32 = 24.;

/// 新打开的二级页始终完整收进工作区；只有用户实际拖动后才允许出屏。
/// 不能依据窗口是否已创建或 modal_shift 是否非零判断：缓存窗会复用，
/// 首次收边本身也会把 modal_shift 校到非零。
#[derive(Default)]
struct OverlayPagePlacement {
    user_dragged: bool,
}

impl OverlayPagePlacement {
    fn reset(&mut self) {
        self.user_dragged = false;
    }

    fn note_drag(&mut self, dx: f32, dy: f32) {
        if dx.abs().max(dy.abs()) >= 0.5 {
            self.user_dragged = true;
        }
    }

    fn fit(&self, main: isize, model: &Model, w: f32, h: f32) -> (f32, f32, f32, f32) {
        let (px, py, pw, ph) = modals::panel_for_model_free(model, w, h);
        let fit = if self.user_dragged { overlay::fit_loose } else { overlay::fit_to_work };
        fit(
            main,
            px - PAGE_MARGIN_X,
            py - PAGE_MARGIN_Y,
            pw + PAGE_MARGIN_X * 2.,
            ph + PAGE_MARGIN_Y * 2.,
        )
    }
}

/// 二级页是否由独立顶层窗口（overlay）承载；与 `App::draw` 里的
/// `overlay_page` 必须一致：原生控件宿主、滚轮上限、
/// 绘制虚拟尺寸都按这一份判定走。
/// 诊断开关：GALE_HIT_DEBUG=1 时在独立窗里把每个命中矩形描成红框，
/// 用于肉眼核对「看得见的位置」与「点得到的位置」是否一致。
fn hit_debug_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("GALE_HIT_DEBUG").is_some())
}

fn page_in_overlay(m: Modal) -> bool {
    // 诊断开关：GALE_NO_OVERLAY=1 时所有二级页退回主窗绘制（对照原生控件可见性）
    if std::env::var_os("GALE_NO_OVERLAY").is_some() {
        return false;
    }
    matches!(
        m,
        Modal::Groups
            | Modal::Preferences
            | Modal::Confirm
            | Modal::Heatmap
            | Modal::More
            | Modal::Online
            | Modal::Rules
            | Modal::Analysis
    )
}

fn draw_hover(frame: &mut gale_win::d2d::Frame, hits: &[modals::Hit], hover: Option<(f32, f32)>, theme: &gale_core::theme::Theme) {
        if let Some((mx, my)) = hover {
            if let Some(a) = modals::hit(hits, mx, my) {
                if a != Action::None {
                    if let Some(h) = hits.iter().rev().find(|h| {
                        h.action == a && mx >= h.x && mx < h.x + h.w && my >= h.y && my < h.y + h.h
                    }) {
                        if h.w <= 920. && h.h <= 56. && h.w > 12. {
                            let palette = ui::tokens(theme);
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
                                frame,
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
}

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
    /// 【快捷键 2026-11-13】录制某个动作的新组合键。
    ShortcutRecord(String),
    /// 恢复默认快捷键。
    ShortcutReset,
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
        "pref_shortcut:reset" => AuxAction::ShortcutReset,
        _ if cmd.starts_with("pref_shortcut:record:") => AuxAction::ShortcutRecord(
            cmd.strip_prefix("pref_shortcut:record:")?.to_string(),
        ),
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

/// 【快捷键 2026-11-13】动作 id → 命令键（与 `SHORTCUT_DEFAULTS` 一一对应）。
fn shortcut_command(id: &str) -> Option<CommandKey> {
    Some(match id {
        "settings" => CommandKey::F1,
        "post" => CommandKey::F2,
        "appearance" => CommandKey::F7,
        "analysis" => CommandKey::F10,
        "restart" => CommandKey::F3,
        "read_group" => CommandKey::F4,
        "groups" => CommandKey::F5,
        "online" => CommandKey::F8,
        "rules" => CommandKey::F9,
        "minimal" => CommandKey::F11,
        "toggle_mode" => CommandKey::F12,
        "menu" => CommandKey::MenuKey,
        "prev_segment" => CommandKey::CtrlLeft,
        "next_segment" => CommandKey::CtrlRight,
        "online_prev" => CommandKey::CtrlO,
        "online_next" => CommandKey::CtrlP,
        "load_clipboard_alt" => CommandKey::AltE,
        _ => return None,
    })
}
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
                        .map(|(_, v)| v.split('·').next().unwrap_or(v))
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
/// 【词提诊断 2026-11-12】排查期打点：append 进数据/trainer-debug.log。
/// 只用标准文件 API（零解释器铁律），低频事件直写。
/// 发布版置 ENABLED=false（零 IO 零开销）；排障时改为 true 即复用。
fn dbg_hint(line: String) {
    const ENABLED: bool = false;
    if !ENABLED {
        return;
    }
    use std::io::Write;
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("数据").join("trainer-debug.log")))
        .unwrap_or_else(|| PathBuf::from("trainer-debug.log"));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{ms}] {line}");
    }
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
        .map_err(|_| "当前虎符服务不支持新词提规则，请更新虎符服务后重试".to_string())?;
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
        return Err("全文分析最多支持 10000 字，请载入较短文本".into());
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
    /// 【换段提速 2026-11-12】排队型被拒（"正在分析另一篇文章"）不是
    /// 故障：旧文分析几百毫秒内完成，固定 150ms 快重试，不吃指数退避。
    fn soft_fail(&mut self, now: u64) {
        self.retry_at = now.saturating_add(150);
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
#[derive(Debug)]
struct LiveSchemaStatus {
    revision: SchemaRevision,
    snapshot: Option<serde_json::Value>,
    next_poll_at: u64,
    sequence: u64,
    in_flight: Option<StatusRequest>,
    retry: RetryBackoff,
    /// A result from a different revision must wait for a fresh status sample.
    awaiting_status: bool,
    /// 【轮询降频 2026-11-14】被动轮询间隔（complete 成功后 next_poll_at=now+此值）。
    /// 词提关闭或无文章 5s；有文但挂起（Idle/Done/暂停）2s；打字中 400ms。
    /// request_fresh 的立即轮询不受此限。默认 400 保持旧语义。
    interval_ms: u64,
}
impl Default for LiveSchemaStatus {
    fn default() -> Self {
        Self {
            revision: SchemaRevision::default(),
            snapshot: None,
            next_poll_at: 0,
            sequence: 0,
            in_flight: None,
            retry: RetryBackoff::default(),
            awaiting_status: false,
            interval_ms: 400,
        }
    }
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
                self.next_poll_at = now.saturating_add(self.interval_ms.max(1));
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
/// 【词提防闪烁 2026-11-12】词库 revision 微变（打字调频让 schema_ver 持续变化
/// 时保留旧词提显示（stale-while-revalidate）：analyze 重拉到达后原子替换；
/// 仅当词库不可用（preserve=false）才硬清空，避免显示与词库脱节的死数据。
fn retire_hint_views(model: &mut Model, sess: &mut Session, preserve: bool) {
    dbg_hint(format!(
        "retire preserve={preserve} words={} full_analysis={}",
        model.words.len(),
        sess.full_analysis.is_some()
    ));
    if !preserve {
        model.words.clear();
        sess.full_analysis = None;
    }
    model.fields.remove("analysis_error");
    model.fields.remove("analysis_failed_key");
}
/// P2 今日汇总（任务 6b）：从磁盘成绩库（统计/records.jsonl；load_valid（
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

/// While typing, input_host routes Enter/Space here only when they are not
/// legitimate target input, so an unfinished round receives a clear hint.
fn round_stuck_fallback(modal: Modal, sess: &Session) -> Option<Action> {
    (modal == Modal::None && sess.phase == Phase::Typing).then_some(Action::RoundStuckHint)
}

/// Esc dismisses panels only; the active round and its clock stay untouched.
fn escape_action(
    modal: Modal,
    ledger_open: bool,
    analysis_dock: bool,
    menu_open: bool,
) -> Option<Action> {
    if modal != Modal::None {
        Some(Action::Close)
    } else if menu_open {
        Some(Action::CloseMenu)
    } else if ledger_open {
        Some(Action::ToggleLedger)
    } else if analysis_dock {
        Some(Action::ToggleAnalysis)
    } else {
        None
    }
}

/// 【对照模式 2026-11-12】拖动分栏把手：指针 y 映射文章区占比（0.15..=0.97）。
/// 只在几何有效时改配置并请求重绘；松手后由主循环防抖落盘。
fn drag_split_ratio(app: &mut App, pointer_y: f32) {
    let sc = gale_win::window::dpi_scale();
    let (cw, ch) = app.win.client_size();
    let (lw, lh) = (cw as f32 / sc, ch as f32 / sc);
    let single = app.sess.source == SessionSource::Single;
    let (top, bottom) = ui::body_region(&app.cfg, &app.model, lw, lh, single);
    let span = (bottom - top).max(80.);
    // 2026-11-13 用户拍板：不设人为限制，拖到哪里算哪里（几何层 input_min 兜物理一行）。
    let ratio = ((pointer_y - top) / span).clamp(0.15, 0.97);
    if (app.cfg.split_ratio - ratio).abs() > 0.001 {
        app.cfg.split_ratio = ratio;
        app.layout_save_due = Some(app.ms() + 350);
        gale_win::window::request_frame();
    }
}

/// 成绩面板上沿把手（逻辑坐标）；面板收起或模态打开时没有。
fn ledger_grip(app: &App) -> Option<(f32, f32, f32, f32)> {
    let sc = gale_win::window::dpi_scale();
    let (cw, ch) = app.win.client_size();
    let (lw, lh) = (cw as f32 / sc, ch as f32 / sc);
    crate::history::resize_grip_rect(&app.cfg, &app.model, lw, lh)
}

/// 【成绩面板 2026-11-13】拖动上沿：指针 y → 面板高度（记忆到配置，防抖落盘）。
fn drag_ledger_height(app: &mut App, pointer_y: f32) {
    let sc = gale_win::window::dpi_scale();
    let (cw, ch) = app.win.client_size();
    let (lw, lh) = (cw as f32 / sc, ch as f32 / sc);
    let bottom = lh - crate::history::bottom_inset(&app.cfg);
    let max_h = (lh - 140.).max(160.);
    let height = (bottom - pointer_y + 2.).clamp(120., max_h);
    if (app.cfg.ledger_panel_h - height).abs() > 0.5 {
        app.cfg.ledger_panel_h = height;
        app.model.ledger_scroll = app
            .model
            .ledger_scroll
            .clamp(0., crate::history::scroll_max(&app.cfg, &app.model, lh));
        app.layout_save_due = Some(app.ms() + 350);
        gale_win::window::request_frame();
    }
}

/// Physical keys precede their document changes in the host queue. Resume at
/// that boundary so the first key and its commit are included, while delayed
/// IME notifications and simply returning to the window keep the clock paused.
#[derive(Default)]
struct DispatchedInput {
    // The host emits one cause per non-ASCII commit, including rejected ones.
    accepted_commits: Vec<bool>,
    /// 本批实际上屏字符数（练字记录用；退格为 0）。
    typed_chars: usize,
    /// 【超文长 2026-11-14】超过文章长度被 comparer 忽略的字符数
    ///（打完仍继续打：正文已满，静默丢弃 → 借 shake 给出可感知反馈）。
    overlong_chars: usize,
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
                // 【超文长 2026-11-14】comparer 只消费到文长为止；文章已满
                //（含 Done 后继续打）时整批静默丢弃——记数供主循环 shake 反馈。
                let cnt = chars.chars().count();
                let live = !sess.is_paused() && !sess.comparer.states.is_empty();
                let typed = sess.feed_input(gale_core::compare::InputEvent::Commit { chars }, now);
                dispatched.typed_chars = dispatched.typed_chars.saturating_add(typed);
                if live && typed < cnt {
                    dispatched.overlong_chars += cnt - typed;
                }
                if has_cause {
                    dispatched
                        .accepted_commits
                        .push(sess.comparer.pos > previous_pos);
                }
            }
            HostEvent::Backspace => {
                sess.feed_input(gale_core::compare::InputEvent::Backspace, now);
            }
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
/// 结束自动回收）。window.rs 的 activate_existing_instance（旧版互斥体名）
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

/// FNV-1a 文本哈希：对照模式输入行缓存键（长度相同但内容变了的退格/回改也能失效）。
fn text_hash(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
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
    /// 【下一个字编码】single=true 单字码表响应。
    CharCodes(
        u64,
        String,
        String,
        usize,
        bool,
        SchemaRevision,
        Result<serde_json::Value, String>,
    ),
    Chats(Vec<qq_bridge::ChatWindow>),
    ChatFocused(String, bool),
    Article(u64, isize, String, Result<String, String>),
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
        result: Result<qq_bridge::SendStatus, String>,
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
    /// 【词提重排缓存 2026-11-14】基准布局（rewrap 前）——词边界变化时
    /// 从基准重排，避免在已重排布局上叠加二阶重排。
    layout_base: Vec<(usize, String, Vec<f32>)>,
    /// 【词提重排缓存 2026-11-14】词边界指纹（FNV over (start,len)），
    /// u64::MAX = 需重算。词提重排原来每帧重跑 O(正文)，现在只在
    /// 布局重建或词边界变化时执行。
    rewrap_words_key: u64,
    edits: BTreeMap<String, NativeEdit>,
    font_picker: Option<gale_win::font_picker::NativeFontPicker>,
    edit_epoch: u64,
    hits: Vec<modals::Hit>,
    /// 最近一次鼠标位置（逻辑坐标），用于悬停高亮。
    hover: Option<(f32, f32)>,
    /// 【鼠标自动隐藏 2026-11-13】上一帧光标屏幕坐标（轮询判断“鼠标动了”）。
    last_cursor_pos: (i32, i32),
    /// 期望隐藏光标：键入置 true、鼠标活动/focus 丢失置 false。
    /// 在主循环里每帧强制执行（输入法可能在隐藏期间把光标重新显示）。
    cursor_want_hidden: bool,
    /// 【对照模式 2026-11-12】用户已打字符的换行缓存（Renderer::wrap_text 实测）。
    input_lines: Vec<(usize, String, Vec<f32>)>,
    input_layout_key: (u64, u64, u64, u64),
    /// 【视图菜单】上一帧菜单矩形（点击外部关闭用）。
    menu_rect: Option<(f32, f32, f32, f32)>,
    /// 【拖动】左键按下待判定（位置+时刻）：移动超阈值即转窗口拖动。
    press: Option<(f32, f32, u64)>,
    /// 【双击】上次单击（位置+时刻）：双击非控件区=最大化/还原。
    last_click: Option<(f32, f32, u64)>,
    /// 【对照模式】正在拖分栏把手。
    split_drag: bool,
    /// 【成绩面板】正在拖上沿调高度。
    ledger_drag: bool,
    /// 【二级页拖动 2026-11-14】抓住标题栏时的 (抓取点 x,y, 起始 shift x,y)。
    // 【拖动正反馈 2026-11-14】六元组 = (gx, gy, sx, sy, ogx, ogy)：
    // 抓取点坐标、抓取时 modal_shift、抓取时 overlay 窗原点（主窗客户 DIP）。
    // overlay 页转发坐标以窗口当前位置为基准，必须连同抓取时窗原点一起记，
    // Move 时才能把「窗口自位移」从手位移里扣掉（详见 Move 分支注释）。
    modal_drag: Option<(f32, f32, f32, f32, f32, f32)>,
    /// 【独立二级页】理论分析弹窗的顶层窗口（面板可拖出主窗）。
    overlay: Option<overlay::Overlay>,
    /// 当前 overlay 承载的页面；页面切换或关闭时销毁重建。
    overlay_key: Option<Modal>,
    /// 二级页 overlay 的实际窗口尺寸（DIP，已按屏幕工作区收边）；
    /// 绘制虚拟尺寸、原生控件裁剪与滚轮上限都用它，和窗口保持一致。
    overlay_frame: Option<(f32, f32)>,
    overlay_placement: OverlayPagePlacement,
    /// 【ESC 兜底 2026-11-14】原生编辑框/下拉有焦点时 WM_KEYDOWN 会被子窗吞掉。
    /// 主循环用 GetAsyncKeyState 边沿检测补一枪，保证任何二级页 Esc 都能关。
    esc_was_down: bool,
    /// 分栏比例等布局改动落盘防抖截止时刻。
    layout_save_due: Option<u64>,
    /// 练字记录有未落盘增量；到点后与 save_config 一起写 config.toml + profile.json。
    practice_dirty: bool,
    practice_flush_due: Option<u64>,
    /// Last time a snapshot-save error toast was shown (rate limit).
    save_toast_at: u64,
    anim: ui::Animation,
    clock: Instant,
    online: Option<OnlineWorker>,
    article: Option<WenlaiArticle>,
    /// 已看过的在线文章历史；上一段回到上一份，而不是重新随机。
    article_history: Vec<WenlaiArticle>,
    article_history_pos: usize,
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
    /// 【下一个字编码】单字码表缓存控制（数据在 model.char_codes）。
    /// char_codes_of=策略|分析文本；char_codes_ver=生成时的 schema 版本。
    char_codes_of: String,
    char_codes_ver: String,
    char_codes_pending: bool,
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
    settings_writer: settings_writer::SettingsWriter,
    qq_worker: Result<sta_worker::StaWorker, String>,
    qq_focus_epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
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
    // 审计 C04/I08：同步档不再保存密码/令牌（界面承诺「密码不保存」），跨机登录态
    // 因此无法随档迁移——本机登录态仍由 DPAPI 的 online-auth.dpapi 负责恢复。
    let resumed_account = loaded.account.as_ref().filter(|account| !account.jwt_token.is_empty());
    if let Some(account) = resumed_account {
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
    model.online_status = "未登录 · 登录后浏览在线书籍".into();
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
    let theme = ui::theme_with_overrides(&cfg.theme, &cfg.color_overrides);
    let (tx, rx) = mpsc::channel();
    let online = OnlineWorker::spawn(dir.clone(), &cfg.wenlai_domain).ok();
    let persist = persistence(dir.clone(), tx.clone(), win.hwnd);
    let settings_writer = settings_writer::SettingsWriter::new(dir.clone(), loaded.state, win.hwnd);
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
        layout_base: Vec::new(),
        rewrap_words_key: u64::MAX,
        edits: BTreeMap::new(),
        font_picker: None,
        edit_epoch: u64::MAX,
        hits: Vec::new(),
        hover: None,
        last_cursor_pos: (-1, -1),
        cursor_want_hidden: false,
        input_lines: Vec::new(),
        input_layout_key: (0, 0, 0, 0),
        menu_rect: None,
        press: None,
        last_click: None,
        split_drag: false,
        ledger_drag: false,
        modal_drag: None,
        overlay: None,
        overlay_key: None,
        overlay_frame: None,
        overlay_placement: OverlayPagePlacement::default(),
        esc_was_down: false,
        layout_save_due: None,
        practice_dirty: false,
        practice_flush_due: None,
        save_toast_at: 0,
        anim: ui::Animation::default(),
        clock: Instant::now(),
        online,
        article: None,
        article_history: Vec::new(),
        article_history_pos: 0,
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
        char_codes_of: String::new(),
        char_codes_ver: String::new(),
        char_codes_pending: false,
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
        settings_writer,
        qq_worker: sta_worker::StaWorker::new(),
        qq_focus_epoch: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        sync_identity_dirty: false,
        sync_account_cleared: false,
    };
    app.model
        .fields
        .insert("online_auto_next".into(), app.online_auto_next.to_string());
    app.sync_preferences(true);
    app.sync_rule_model();
    app.apply_shortcuts();
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
    // 【视图体系】窗口置顶随配置恢复（右键菜单/设置页可切换）。
    if app.cfg.always_on_top {
        gale_win::window::set_topmost(app.win.hwnd(), true);
    }
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
    // 【降帧 2026-11-14】双速帧计时器：动画期 16ms 全速；打字静止期等慢速
    // 消费者（250ms 统计/秒级 deadline/toast 消隐）只需 50ms—— Typing 态
    // 挂机 CPU 从 ~1 核空转降到 ~1/3。输入消息本身会立刻唤醒泵并重启快档。
    let mut timer_fast = false;
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    // P1-6：gale-win 事件泵不透传 keyup——帧级 GetKeyState 边沿观测 VK_BACK
    // 弹起，通知 session 清 protect 窗（帧粒度；真实退格按压远长于一帧）。
    let mut bs_down_prev = false;
    let mut last_size = (0, 0);
    let mut stats_draw_at = 0u64;
    let mut was_focused = true;
    // 【空闲诊断 2026-11-14】同上：GALE_IDLE_DEBUG=1 时每秒记录 need_timer 分量。
    let idle_dbg = std::env::var("GALE_IDLE_DEBUG").is_ok();
    let mut idle_dbg_at = 0u64;
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
        if (app.model.analysis_dock || app.model.modal == Modal::Analysis)
            && app.sess.full_analysis.is_none()
        {
            app.fetch_full_analysis();
        }
        app.collect_online();
        app.pump_persistence();
        while let Some(result) = app.settings_writer.try_result() {
            match result {
                Ok(Some(synced)) => app.adopt_synced_config(synced),
                Ok(None) => {}
                Err(error) => app.toast(error),
            }
        }
        let main_focused = unsafe { GetForegroundWindow() } == app.win.hwnd();
        let overlay_focused = app.overlay.as_ref().is_some_and(|o| unsafe {
            GetForegroundWindow() == o.hwnd()
        });
        let focused = main_focused || overlay_focused;
        if main_focused && !was_focused && (app.sess.phase == Phase::Idle || app.model.analysis_dock) {
            app.analyzed = 0;
            app.analyze();
        }
        // Esc 吞键门：有可关面板时跟打器才拦截 Esc；无面板时放行给
        // RichEdit/TSF（虎符空态 Esc 撤回在未组字时工作，见 input_host）。
        gale_win::input_host::set_esc_panel_open(
            app.model.modal != Modal::None
                || app.model.ledger_open
                || app.model.analysis_dock
                || app.model.menu.is_some(),
        );
        // 【ESC 兜底 2026-11-14】字体下拉、编辑框等原生子窗有焦点时，Esc 被吞。
        // WM_KEYDOWN 被子窗吞掉，主窗的输入宿主收不到，二级页就关不掉。
        // 焦点在子窗、且没有正在编辑的数字快捷键录入时，用全局键态补一枪。
        let esc_now = unsafe { GetAsyncKeyState(0x1B) } < 0;
        let native_focus = unsafe { GetFocus() } != app.win.hwnd();
        if app.model.modal != Modal::None
            && focused
            && native_focus
            && esc_now
            && !app.esc_was_down
            && app.model.field("pref_num_field").is_empty()
            && app.model.field("pref_recording").is_empty()
        {
            // 【ESC 兜底排查 2026-11-14】插桩：兜底触发时打点（含前台/焦点状态）。
            gale_win::input_host::diag_log(&format!(
                "esc fallback fire: fg_main={main_focused} fg_overlay={overlay_focused} native_focus={native_focus}"
            ));
            app.act(Action::Close, false);
        }
        app.esc_was_down = esc_now;
        let accept_host = app.model.modal == Modal::None && (focused || was_focused);
        was_focused = focused;
        if !focused {
            app.cursor_want_hidden = false;
            gale_win::window::set_cursor_hidden(false);
        }
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
        if accept_host && dispatched.typed_chars > 0 {
            app.record_practice(dispatched.typed_chars);
        }
        // 【超文长 2026-11-14】文章已满仍提交：正文全绿却无任何反馈，容易
        // 以为没打上而继续敲——复用打错字的 shake 通道给出可感知反馈
        //（ui.rs shake_on_wrong/reduced_motion 门禁照常生效）。
        if accept_host && dispatched.overlong_chars > 0 {
            app.anim.last_input = seconds;
            app.anim.wrong_at = seconds;
            gale_win::window::request_frame();
        }
        // P1-3 选重/标顶（任务 2）：每帧 poll 之后立即 drain 20ms 配对结果
        //（input_host 顺序契约：必须在 poll 之后），Ding→标顶/Select→选重/
        // Unknown 忽略；模态打开时只清队列不记账。
        dispatched.apply_commit_causes(&mut app.sess, gale_win::input_host::drain_commit_causes());
        // 整改12：每帧取走击键时间戳队列（poll 之后、选重 20ms 配对已消费本帧
        // 击键），按热力图口径计入本段 VK 直方图；模态打开时只清队列不记账。
        let key_times = gale_win::input_host::drain_key_times(1024);
        // 【鼠标自动隐藏 2026-11-13】窗口内一有键入就隐藏光标；鼠标一动恢复。
        // 位置用 GetCursorPos 查询：指针常悬在 RichEdit 子窗上，主窗口收不到
        // WM_MOUSEMOVE，不能依赖 hover 事件。
        let cursor_in = gale_win::window::cursor_inside_client(app.win.hwnd());
        if accept_host && !key_times.is_empty() && cursor_in {
            // 【2026-11-13 修】这里必须真正调用 set_cursor_hidden(true)：只置
            // want 标志、指望下一帧 enforce 去隐藏是错的——enforce 的语义是
            // 「已隐藏但被系统重新显示时再压一次」，它看到 CURSOR_HIDDEN 还是
            // false 就直接返回，于是光标从头到尾没被隐藏过（用户实测「完全不
            // 消失」）。置 want 的同时立刻隐藏，之后每帧 enforce 兜外部重显示。
            app.cursor_want_hidden = true;
            gale_win::window::set_cursor_hidden(true);
        }
        // 轮询光标位置：任何位移（含子窗上的移动）都立即恢复显示。
        let cursor_now = gale_win::window::cursor_screen_pos();
        if cursor_now != app.last_cursor_pos {
            app.last_cursor_pos = cursor_now;
            app.cursor_want_hidden = false;
            if gale_win::window::cursor_hidden() {
                gale_win::window::set_cursor_hidden(false);
            }
        }
        // 想要隐藏期间每帧守一次：输入法候选窗/子控件若把光标重新显示出来
        // （用户实测会出现「按一下消失、再按一下又出来」），这一帧立刻再压下去。
        if app.cursor_want_hidden {
            gale_win::window::enforce_cursor_hidden();
        }
        for (vk, _) in key_times {
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
            // 开始打字自动收起视图菜单（菜单不挡操作）。
            if app.model.menu.is_some() && app.sess.comparer.pos != oldpos {
                app.model.menu = None;
            }
            gale_win::window::request_frame();
        }
        app.refresh_dynamic_hints(now);
        app.handle_shortcut_recording();
        for cmd in gale_win::input_host::drain_cmds() {
            // 【审计 I02】Esc 的目标必须在清菜单之前按原始状态判定：先清再判会
            // 让菜单那层的信息丢失，一次 Esc 连底部面板一起关掉（要求两下才关）。
            let escape = if cmd == CommandKey::Escape {
                escape_action(
                    app.model.modal,
                    app.model.ledger_open,
                    app.model.analysis_dock,
                    app.model.menu.is_some(),
                )
            } else {
                None
            };
            // 其它命令键一律先收起视图菜单；菜单键本身负责开菜单。
            if cmd != CommandKey::MenuKey {
                app.model.menu = None;
            }
            let a = match cmd {
                CommandKey::Escape => escape,
                CommandKey::F1 => Some(Action::Open(Modal::Preferences)),
                CommandKey::F7 => Some(Action::OpenAppearanceSettings),
                CommandKey::F10 => Some(Action::ToggleAnalysis),
                CommandKey::F3 => Some(Action::Restart),
                CommandKey::F4 => Some(Action::ReadGroup),
                CommandKey::F5 => Some(Action::Open(Modal::Groups)),
                CommandKey::F8 => Some(Action::Open(Modal::Online)),
                CommandKey::F9 => Some(Action::Open(Modal::Rules)),
                // 视图体系：F11 极简模式、F12 字贴/对照互换、菜单键开视图菜单。
                CommandKey::F11 => Some(Action::ToggleMinimal),
                CommandKey::F12 => Some(Action::ToggleDisplayMode),
                CommandKey::MenuKey => {
                    let sc = gale_win::window::dpi_scale();
                    let (cw, ch) = app.win.client_size();
                    let (lw, lh) = (cw as f32 / sc, ch as f32 / sc);
                    let (mx, my) = app
                        .anim
                        .caret
                        .map(|(cx, cy)| (cx + 24., cy + 12.))
                        .unwrap_or((lw / 2., lh / 3.));
                    Some(Action::OpenMenuAt(
                        mx.clamp(8., (lw - 40.).max(8.)),
                        my.clamp(8., (lh - 40.).max(8.)),
                    ))
                }
                // F2 发送当前文章；本地载入只保留剪贴板，F6 不绑定功能。
                CommandKey::F2 => Some(Action::PostArticle),
                CommandKey::F6 | CommandKey::RetryResult => None,
                CommandKey::LoadClipboard | CommandKey::AltE => Some(Action::LoadClipboard),
                CommandKey::CopyResult => Some(Action::CopyScore),
                CommandKey::SendResult => Some(Action::SendScore),
                CommandKey::ExportResult => Some(Action::ExportImage),
                CommandKey::CtrlLeft => Some(Action::Previous),
                CommandKey::CtrlRight => Some(Action::Next),
                CommandKey::CtrlO => Some(Action::OnlineFetch(-1)),
                CommandKey::CtrlP => Some(Action::OnlineFetch(1)),
                CommandKey::Enter => {
                    completed_segment_action(cmd, app.sess.phase == Phase::Done, app.model.modal)
                        .or(round_stuck_fallback(app.model.modal, &app.sess))
                }
                CommandKey::Space => {
                    completed_segment_action(cmd, app.sess.phase == Phase::Done, app.model.modal)
                        .or(round_stuck_fallback(app.model.modal, &app.sess))
                }
            };
            if let Some(a) = a {
                app.act(a, false);
            }
        }
        // 【数值直输 2026-11-13】外观页数值编辑态：偏好弹窗一关即取消。
        // 编辑期间数字/退格、回车/Esc 由 input_host 队列消费（其余键已被吞）。
        if !app.model.field("pref_num_field").is_empty() {
            if app.model.modal != Modal::Preferences {
                app.model.fields.insert("pref_num_field".into(), String::new());
                gale_win::input_host::set_num_edit(false);
            } else {
                for vk in gale_win::input_host::drain_num_keys() {
                    app.handle_num_key(vk);
                }
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
        // 【对照模式 2026-11-12】拖动分栏后防抖落盘比例。
        if let Some(due) = app.layout_save_due {
            if now >= due {
                app.layout_save_due = None;
                app.save_config();
            }
        }
        // 练字记录：攒满 60 秒再落盘，避免每字一次 profile 云同步写。
        if app.practice_dirty && app.practice_flush_due.is_some_and(|due| now >= due) {
            app.practice_flush_due = None;
            app.practice_dirty = false;
            app.save_config();
        }
        gale_win::input_host::set_round_done(app.sess.phase == Phase::Done);
        // Group bodies preserve LF: Enter must reach RichEdit exactly when the
        // next target is a line break. ASCII spaces keep the article policy.
        gale_win::input_host::set_round_typing(
            app.sess.phase == Phase::Typing,
            app.sess.comparer.next_target_char() == Some('\n'),
            app.sess.comparer.text.contains(' '),
        );
        gale_win::input_host::set_result_key_target(
            (app.model.ledger_open && app.model.selected_history.is_some()).then_some(app.win.hwnd),
        );
        for e in gale_win::window::drain_mouse() {
            match e {
                MouseEv::LDown { x, y } => {
                    app.cursor_want_hidden = false;
                    gale_win::window::set_cursor_hidden(false);
                    let xx = x as f32;
                    let yy = y as f32;
                    // 命中探针（GALE_HIT_DEBUG=1）：记录每次点击落到哪个命中框，
                    // 用于把「视觉位置」与「生效位置」逐条对照。
                    if hit_debug_enabled() {
                        let mut probe = None;
                        for (i, h) in app.hits.iter().enumerate().rev() {
                            if xx >= h.x && xx < h.x + h.w && yy >= h.y && yy < h.y + h.h {
                                probe = Some((i, h));
                                break;
                            }
                        }
                        match probe {
                            Some((i, h)) => gale_win::input_host::diag_log(&format!(
                                "HITPROBE x={xx:.1} y={yy:.1} modal={:?} idx={i} rect=({:.1},{:.1},{:.1},{:.1}) action={:?}",
                                app.model.modal, h.x, h.y, h.w, h.h, h.action
                            )),
                            None => gale_win::input_host::diag_log(&format!(
                                "HITPROBE x={xx:.1} y={yy:.1} modal={:?} idx=none action=None",
                                app.model.modal
                            )),
                        }
                    }
                    // ① 视图菜单：点菜单项执行，点外面只收起菜单（不误触底层控件）。
                    if app.model.menu.is_some() {
                        let inside = app.menu_rect.is_some_and(|(mx, my, mw, mh)| {
                            xx >= mx && xx <= mx + mw && yy >= my && yy <= my + mh
                        });
                        if inside {
                            if let Some(a) = modals::hit(&app.hits, xx, yy) {
                                if a != Action::None {
                                    app.capture_fields();
                                    // 字号/比例的步进按钮允许连点，菜单保持打开；
                                    // 其余开关与模式切换执行后收起菜单，让变化看得见。
                                    let keep_open =
                                        matches!(a, Action::StepFont(_) | Action::StepSplit(_));
                                    app.act(a, false);
                                    if !keep_open {
                                        app.model.menu = None;
                                    }
                                    gale_win::window::request_frame();
                                }
                            }
                        } else {
                            app.model.menu = None;
                            gale_win::window::request_frame();
                        }
                        continue;
                    }
                    // ② 对照模式分栏把手：按住拖动改文章/输入区比例。
                    if let Some((dx, dy, dw, dh)) = app.anim.split_divider {
                        if xx >= dx && xx <= dx + dw && yy >= dy && yy <= dy + dh {
                            app.split_drag = true;
                            app.press = None;
                            gale_win::window::request_frame();
                            continue;
                        }
                    }
                    // ②b 成绩面板上沿：按住拖动可把面板拖高拖低。
                    if let Some((gx, gy, gw, gh)) = ledger_grip(&app) {
                        if xx >= gx && xx <= gx + gw && yy >= gy && yy <= gy + gh {
                            app.ledger_drag = true;
                            app.press = None;
                            gale_win::window::request_frame();
                            continue;
                        }
                    }
                    // ②c 二级页拖动带。overlay 页=整个独立窗（含透明留白圈）；
                    // 主窗内模态=仅标题带。按住非控件处拖动整块面板。
                    if app.model.modal != Modal::None {
                        // 【命中 2026-11-14】这里原来直接拿 client_size()（物理像素）
                        // 去算面板矩形，而命中表/Action 全是 DIP；125% 下标题带会
                        // 判错（点开关被当拖动、点 × 被当拖动）。统一换成 DIP。
                        let (cw, ch) = app.win.client_size();
                        let sc = gale_win::window::dpi_scale().max(0.01);
                        let (cw, ch) = (cw as f32 / sc, ch as f32 / sc);
                        let (mx, my, mw, mh) = modals::panel_for_model(&app.model, cw, ch);
                        // 【看不见的区域 2026-11-14】overlay 页的可拖区=整个独立窗
                        // 矩形（origin_in_main + 窗口 DIP 尺寸），不再只有 PANEL_HEAD
                        // 标题带：此前按住面板正文空白或透明留白圈会漏到分支④，
                        // 表现为「按住浮动窗空白处，后面主窗被拖走/失焦」——独立
                        // 窗视觉边界与交互边界不一致。RichEdit 子窗的点击不经
                        // overlay_proc，文本选择/输入不受影响；控件命中仍走③。
                        let in_overlay_page =
                            page_in_overlay(app.model.modal) && app.overlay.is_some();
                        let (inside, bx, by) = if in_overlay_page {
                            let ov = app.overlay.as_ref().unwrap();
                            let (ox, oy) = ov.origin_in_main();
                            let (fw, fh) = app.overlay_frame.unwrap_or((mw, mh));
                            (xx >= ox && xx <= ox + fw && yy >= oy && yy <= oy + fh, ox, oy)
                        } else {
                            (
                                xx >= mx
                                    && xx <= mx + mw
                                    && yy >= my
                                    && yy <= my + modals::PANEL_HEAD,
                                mx,
                                my,
                            )
                        };
                        if inside {
                            let on_control = modals::hit(&app.hits, xx, yy)
                                .is_some_and(|a| a != Action::None);
                            if !on_control {
                                // 【拖动正反馈 2026-11-14】overlay 页必须连同抓取时的
                                // 窗原点一起记（日志/回溯用）；Move 的手位移已改纯
                                // 屏幕增量（x−gx），不再消费 og。
                                let (ogx, ogy) = if page_in_overlay(app.model.modal) {
                                    app.overlay
                                        .as_ref()
                                        .map(|ov| ov.origin_in_main())
                                        .unwrap_or((0., 0.))
                                } else {
                                    (0., 0.)
                                };
                                app.modal_drag = Some((
                                    xx,
                                    yy,
                                    app.model.modal_shift.0,
                                    app.model.modal_shift.1,
                                    ogx,
                                    ogy,
                                ));
                                app.press = None;
                                // 【拖动排查 2026-11-14b】拖动启动插桩：确认 LDown
                                // 真正进入 modal_drag 分支（与 DRAGMOVE/SETRECT 配合
                                // 三点定位链路断在哪一环）。
                                gale_win::input_host::diag_log(&format!(
                                    "DRAGSTART xx={xx:.0} yy={yy:.0} band=({bx:.0},{by:.0}) shift0=({:.0},{:.0}) og=({ogx:.0},{ogy:.0})",
                                    app.model.modal_shift.0, app.model.modal_shift.1
                                ));
                                gale_win::window::request_frame();
                                continue;
                            }
                        }
                    }
                    // ③ 控件命中（工具栏/统计/底栏/菜单/窗口按钮）。
                    if gale_win::input_host::diagnostics_enabled() {
                        let near = app
                            .hits
                            .iter()
                            .rev()
                            .find(|h| {
                                xx >= h.x && xx <= h.x + h.w && yy >= h.y && yy <= h.y + h.h
                            })
                            .map(|h| {
                                format!(
                                    "({:.0},{:.0},{:.0},{:.0}) {:?}",
                                    h.x, h.y, h.w, h.h, h.action
                                )
                            })
                            .unwrap_or_else(|| "-".to_string());
                        gale_win::input_host::diag_log(&format!(
                            "CLICK xx={xx:.1} yy={yy:.1} modal={:?} overlay={} hit={}",
                            app.model.modal,
                            app.overlay_frame.is_some(),
                            near
                        ));
                    }
                    if let Some(a) = modals::hit(&app.hits, xx, yy) {
                        if a == Action::None {
                            continue;
                        }
                        app.capture_fields();
                        app.press = None;
                        app.act(a, false);
                        continue;
                    }
                    // 【理论分析整页 2026-11-15】整页浮窗打开时，点主窗空白＝关闭它
                    // （浮窗本体在独立窗口里，这条只兜「点面板以外任何空处」）。
                    if app.model.modal == Modal::Analysis {
                        app.model.open(Modal::None);
                        gale_win::window::request_frame();
                        continue;
                    }
                    // ④ 空白/正文：单击=聚焦，按住拖动=移动窗口（任意模式、任意处）。
                    app.capture_fields();
                    app.press = Some((xx, yy, app.ms()));
                    // 【2026-11-13】成绩面板点正文不再收起（常驻，直到手动收起/Esc）；
                    // 理论分析停靠面板仍保留「点正文收起」的旧行为。
                    if app.model.analysis_dock {
                        app.model.analysis_dock = false;
                        gale_win::window::request_frame();
                    }
                    app.host.focus();
                    app.host.hide_caret();
                }
                MouseEv::Move { x, y } => {
                    // 【2026-11-13 修】子窗/输入法窗口会产生「指针没动」的
                    // WM_MOUSEMOVE，不能一收到就恢复光标（否则每键隐藏后立刻被
                    // 抬回来，表现为完全看不见效果）。只有坐标真的变了才恢复。
                    let pos = Some((x as f32, y as f32));
                    if app.hover != pos {
                        app.hover = pos;
                        app.model.pointer = pos;
                        gale_win::window::request_frame();
                        app.cursor_want_hidden = false;
                        gale_win::window::set_cursor_hidden(false);
                    }
                    if let Some((gx, gy, sx, sy, _ogx, _ogy)) = app.modal_drag {
                        // 【拖动自愈 2026-11-14】物理左键已不在按住状态却还有
                        // modal_drag（捕获被其它窗口夺走 / LBUTTONUP 丢失 /
                        // 拖动中模态被 Esc 关闭）时立即终止拖动，防止随后的
                        // 纯 hover 移动继续拖面板。GetKeyState 读本线程队列
                        // 状态，与转发链路同线程，可靠。
                        let held =
                            unsafe { GetKeyState(VK_LBUTTON.0 as i32) } < 0;
                        if !held {
                            app.modal_drag = None;
                            gale_win::window::request_frame();
                            continue;
                        }
                        // 【单位 2026-11-14】panel_base 期望 DIP（与 1763 行 LBUTTONDOWN
                        // 分支同口径）；这里原来直接拿物理像素 client_size()，125% 下
                        // 拖动夹取范围错 1.25 倍，面板会被夹到错误位置。
                        let (cw, ch) = app.win.client_size();
                        let sc = gale_win::window::dpi_scale().max(0.01);
                        let (cw, ch) = (cw as f32 / sc, ch as f32 / sc);
                        let (bx, by, bw, _) = modals::panel_base(&app.model, cw, ch);
                        // 【拖动跟手 2026-11-14】overlay 转发的坐标经
                        // ClientToScreen(overlay) → 主窗客户区换算后已是
                        // 「屏幕锚定」的绝对坐标：光标不动、窗口自移时 x 不变
                        // （窗口自位移在换算里被加上又减去，天然抵消）。
                        // 因此手位移 = 当前光标 − 抓取光标（x−gx），不能再减
                        // (gx−ogx)/(窗自移)：那会把已应用的面板移动再扣一遍，
                        // 迭代不动点 s*=ΔM/2+shift0 —— 表现为窗口恒半速跟手
                        // 且帧间抖动（18:40 用户实测「不跟鼠标」）。
                        // 旧「runaway 2×加速」只在转发坐标为窗口相对坐标的
                        // 时代存在；改绝对坐标后该补偿已是重复扣减。
                        let hand_dx = x as f32 - gx;
                        let hand_dy = y as f32 - gy;
                        // 【拖动范围 2026-11-14】overlay 页不按主窗钳制：顶层窗
                        // 可游遍整个屏幕工作区（fit_to_work 收边），否则 800 高
                        // 面板 py∈[16,h-70] 与「须伸到主窗顶以上」矛盾 → 垂直
                        // 死锁。主窗内模态保持旧钳制（留 160 宽 + 标题栏可抓回）。
                        // 位移求解统一走 overlay::drag_shift_absolute（绝对坐标
                        // 语义，overlay.rs 有回归单测钉住半速/纹丝不动两类 bug）。
                        let (ax, ay) = overlay::drag_shift_absolute(
                            (sx, sy),
                            (gx, gy),
                            (x as f32, y as f32),
                        );
                        debug_assert_eq!((ax, ay), (sx + hand_dx, sy + hand_dy));
                        let (nx, ny) = if page_in_overlay(app.model.modal) {
                            (ax, ay)
                        } else {
                            (
                                ax.clamp(120. - bw - bx, cw - 120. - bx),
                                ay.clamp(16. - by, (ch - 70.).max(16.) - by),
                            )
                        };
                        app.model.modal_shift = (nx, ny);
                        // 【拖动流畅 2026-11-14】拖动中不等渲染帧：输入事件里直接
                        // 把 overlay 窗口几何推到位。窗口移动是纯 SetWindowPos
                        // （DComp 内容随窗口整体平移，无需重绘内容），125Hz 输入
                        // 率下窗口跟手。此前位置只在 draw() 里更新，更新率被渲染
                        // 耗时拖住（实测 SETRECT 22 次 vs DRAGMOVE 41 次）→ 视觉
                        // 卡顿（用户实测「拖动不流畅」）。
                        if page_in_overlay(app.model.modal) {
                            app.overlay_placement.note_drag(hand_dx, hand_dy);
                            app.reposition_overlay_now();
                        }
                        // 【拖动排查 2026-11-14b】Move 链路插桩：拖动期间记录
                        // 手位移与目标 shift（每 move 一行，仅在诊断开启时写）。
                        gale_win::input_host::diag_log(&format!(
                            "DRAGMOVE x={x} y={y} hand=({hand_dx:.0},{hand_dy:.0}) shift=({nx:.0},{ny:.0})"
                        ));
                        gale_win::window::request_frame();
                    } else if app.split_drag {
                        drag_split_ratio(&mut app, y as f32);
                    } else if app.ledger_drag {
                        drag_ledger_height(&mut app, y as f32);
                    } else if let Some((px, py, _)) = app.press {
                        let held = unsafe { GetKeyState(VK_LBUTTON.0 as i32) } < 0;
                        let (mx, my) = (x as f32, y as f32);
                        if held && (mx - px).abs() + (my - py).abs() > 4. {
                            // 【任意处拖动 2026-11-12】移动超过 4DIP 才转窗口拖动，
                            // 未移动的单击仍是聚焦/收起浮层（沿用原语义）。
                            app.press = None;
                            gale_win::window::set_nc_drag_active(true);
                            unsafe {
                                let _ = ReleaseCapture();
                                let _ = SendMessageW(
                                    app.win.hwnd(),
                                    WM_NCLBUTTONDOWN,
                                    WPARAM(2),
                                    LPARAM(0),
                                );
                            }
                            gale_win::window::set_nc_drag_active(false);
                            app.host.reset_anchor();
                            gale_win::window::request_frame();
                        }
                    }
                }
                MouseEv::Leave => {
                    // 指针离开窗口：恢复光标并清 hover（隐藏只在窗口内有效）。
                    // 进入子窗（打字热区）也会触发 WM_MOUSELEAVE，先用
                    // GetCursorPos 确认真的在客户区外，否则不动隐藏状态。
                    if !gale_win::window::cursor_inside_client(app.win.hwnd()) {
                        app.cursor_want_hidden = false;
                        gale_win::window::set_cursor_hidden(false);
                    }
                    if app.hover.take().is_some() {
                        app.model.pointer = None;
                        gale_win::window::request_frame();
                    }
                }
                MouseEv::LUp { x, y } => {
                    if app.modal_drag.take().is_some() {
                        gale_win::window::request_frame();
                        continue;
                    }
                    if app.split_drag {
                        app.split_drag = false;
                        app.save_config();
                        app.host.reset_anchor();
                        gale_win::window::request_frame();
                        continue;
                    }
                    if app.ledger_drag {
                        app.ledger_drag = false;
                        app.save_config();
                        app.host.reset_anchor();
                        gale_win::window::request_frame();
                        continue;
                    }
                    let Some((_px, _py, _at)) = app.press.take() else {
                        continue;
                    };
                    let (xx, yy) = (x as f32, y as f32);
                    let now = app.ms();
                    let double = app.last_click.is_some_and(|(lx, ly, lt)| {
                        now.saturating_sub(lt) <= 450
                            && (xx - lx).abs() < 4.
                            && (yy - ly).abs() < 4.
                    });
                    // 标题带双击 = 最大化/还原（极简模式没有标题栏，用顶部静默带）。
                    let title_like = if app.cfg.minimal { yy <= 44. } else { yy < 55. };
                    if double && title_like {
                        app.last_click = None;
                        app.act(Action::Maximize, false);
                    } else {
                        app.last_click = Some((xx, yy, now));
                    }
                }
                MouseEv::RDown { x, y } => {
                    app.cursor_want_hidden = false;
                    gale_win::window::set_cursor_hidden(false);
                    // 模态面板内不抢右键；正文区右键打开窗口内自绘「视图」菜单
                    //（系统右键菜单通道仍被输入宿主全拦，防作弊语义不变）。
                    if app.model.modal != Modal::None {
                        continue;
                    }
                    app.model.menu = Some(modals::CtxMenu {
                        x: x as f32,
                        y: y as f32,
                    });
                    gale_win::window::request_frame();
                }
                MouseEv::Wheel { delta, x, y } => {
                    app.cursor_want_hidden = false;
                    gale_win::window::set_cursor_hidden(false);
                    let sc = gale_win::window::dpi_scale();
                    let (cw, ch) = app.win.client_size();
                    let (lw, lh) = (cw as f32 / sc, ch as f32 / sc);
                    // 【滚轮坐标 2026-11-14】x/y 来自 window.rs lx()，已是主窗客户区 DIP，
                    // 这里再除一次 scale 会在非 100% 缩放下把停靠/成绩面板命中域缩小。
                    let mx = x as f32;
                    let my = y as f32;
                    // 理论分析停靠面板：指针在面板内时滚分析内容。
                    if app.model.analysis_dock {
                        let (ax, ay, aw, ah) = modals::analysis_dock_rect_for(&app.cfg, lw, lh);
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
                        let (px, py, pw, ph) = history::panel_rect_for(&app.cfg, &app.model, lw, lh);
                        if mx >= px && mx <= px + pw && my >= py && my <= py + ph {
                            let max = history::scroll_max(&app.cfg, &app.model, lh);
                            app.model.ledger_scroll = (app.model.ledger_scroll
                                - delta as f32 / 120. * 64.)
                                .clamp(0., max);
                            gale_win::window::request_frame();
                            continue;
                        }
                    }
                    // 理论分析弹窗：内容一页放不下时滚分析正文。
                    if app.model.modal == Modal::Analysis {
                        let (vw, vh) = if page_in_overlay(app.model.modal) {
                            app.overlay_frame.unwrap_or((lw, lh))
                        } else {
                            (lw, lh)
                        };
                        if let Some((_fs, total, body_h)) = modals::analysis_modal_fit_metrics(
                            &app.sess, &app.model, vw, vh,
                        ) {
                            let max = (total - body_h).max(0.);
                            if max > 0. {
                                app.model.analysis_dock_scroll = (app.model.analysis_dock_scroll
                                    - delta as f32 / 120. * 64.)
                                    .clamp(0., max);
                                gale_win::window::request_frame();
                                continue;
                            }
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
                    // 二级页在独立窗里：可视高度是窗口高度（小屏会被工作区收边），
                    // 不是主窗高度，否则 Rules 这类高面板永远滚不动。
                    let view_h = if page_in_overlay(app.model.modal) {
                        app.overlay_frame.map(|f| f.1).unwrap_or(lh)
                    } else {
                        lh
                    };
                    let max = modals::panel_scroll_max(&app.model, lw, lh, view_h);
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
                            Modal::Groups => {
                                let (vw, vh) = if page_in_overlay(app.model.modal) {
                                    app.overlay_frame.unwrap_or((lw, lh))
                                } else { (lw, lh) };
                                let (_, _, _, ph) = modals::panel_for_model(&app.model, vw, vh);
                                app.model.groups.len().saturating_sub(modals::groups_visible_rows(ph))
                            },
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
        // 【动效 2026-11-15】出帧只看有没有正在跑的动画：调度侧（smooth_caret /
        // scroll_offset）已经按 reduced_motion 与 smooth_caret 决定跑不跑，这里再拿
        // reduced_motion 卡一次会把「已经启动的动画」的后续帧全掐掉，结果只剩首帧。
        let animate = app.anim.active(seconds);
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
        // 【空闲诊断 2026-11-14】GALE_IDLE_DEBUG=1 时每秒一行，列出 need_timer 各
        // 分量——排查挂机 CPU 时定位哪个源一直活着（写入输入诊断.log，需同时
        // 开哨兵或 GALE_INPUT_LOG）。
        if idle_dbg && now.saturating_sub(idle_dbg_at) >= 1000 {
            idle_dbg_at = now;
            gale_win::input_host::diag_log(&format!(
                "idle need_timer={} animate={} typing={} online={} reg={} analyzing={} \
                 full_analyzing={} dyn_hint={} qq={} font_save={} auto_next={} scores={} \
                 snap={} toast={}",
                need_timer,
                animate,
                app.sess.phase == Phase::Typing,
                app.online_pending > 0,
                app.reg_code_until > 0,
                app.analyzing,
                app.full_analyzing,
                app.dynamic_hint_due.is_some(),
                app.qq_busy,
                app.font_save_due.is_some(),
                app.auto_next_at.is_some(),
                !app.pending_scores.is_empty(),
                app.pending_snapshot.is_some(),
                app.model
                    .toast
                    .as_ref()
                    .map(|(_, at)| seconds - at < 3.7)
                    .unwrap_or(false),
            ));
        }
        // 【降帧 2026-11-14】动画期 16ms；其余（打字静止/秒级 deadline/toast）50ms。
        let want_fast = animate;
        if need_timer && (!timer || timer_fast != want_fast) {
            gale_win::window::start_frame_timer(app.win.hwnd, if want_fast { 16 } else { 50 });
            timer = true;
            timer_fast = want_fast;
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
    let settings_saved = app.settings_writer.flush();
    let (ack, done) = mpsc::channel();
    let _ = app.persist.send(Persist::Stop(ack));
    let saved = done
        .recv()
        .map_err(|_| "成绩保存线程未确认退出".to_string())?;
    gale_win::window::clear_size_rebuild_callback();
    gale_win::window::unregister_size_fill_renderer();
    saved.map_err(|e| format!("退出时有数据未保存：{e}"))?;
    settings_saved.map_err(|e| format!("退出时有设置未保存：{e}"))?;
    Ok(())
}
/// 在线取文难度：`level` 为设置页下标（0..=5 固定六级、6=随机）。
/// 随机档每次取文从六级里抽一级；`seed` 只用亚秒纳秒当熵源，够用且无依赖。
fn online_difficulty_for(level: i32, seed: u32) -> Difficulty {
    let level = if level >= 6 {
        (seed % 6) as i64 + 1
    } else {
        level.max(0) as i64 + 1
    };
    Difficulty::from_level(level).unwrap_or(Difficulty::Pu)
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
    /// 练字记录：按本地日期累计**实际上屏**字符数（含错字重打，不扣退格）。
    /// 不逐字写盘：标记 dirty，主循环 60 秒后与正常设置保存一起落盘。
    fn record_practice(&mut self, chars: usize) {
        if chars == 0 {
            return;
        }
        let today = gale_core::score::unix_to_date(unix_secs());
        if self.cfg.practice_record(&today, chars as u64) {
            self.practice_dirty = true;
            if self.practice_flush_due.is_none() {
                self.practice_flush_due = Some(self.ms() + 60_000);
            }
        }
    }
    /// 在线取文难度：选「随机」（index 6）时每次从六级里抽一级。
    fn online_fetch_difficulty(&self) -> Difficulty {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        online_difficulty_for(self.model.online_difficulty as i32, seed)
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

    /// 【审计 C01】云档带来的设置已被合并进 profile.json：界面必须改用合并
    /// 结果，否则任意一次后续偏好保存都会拿旧界面值把远端改动覆盖回去。
    fn adopt_synced_config(&mut self, synced: Config) {
        self.cfg = synced;
        self.model.reduced_motion = gale_win::edit::reduced_motion();
        self.theme = ui::theme_with_overrides(&self.cfg.theme, &self.cfg.color_overrides);
        self.apply_style();
        self.invalidate_live_hints(false);
        self.analyze();
        self.toast("已合并云同步设置");
    }

    /// 【长截图 2026-11-15】整篇装不进一屏时，用固定可读字号分页渲染，再竖向拼成
    /// 一张长图写进剪贴板。返回 None 表示一屏装得下，交给原单屏截图路径。
    fn copy_analysis_long_image(&mut self) -> Option<Result<(), String>> {
        let fa = self.sess.full_analysis.clone()?;
        let sc = gale_win::window::dpi_scale();
        let (cw, ch) = self.win.client_size();
        let (lw, lh) = (cw as f32 / sc, ch as f32 / sc);
        let (x, y, pw, ph) = modals::panel_for_model(&self.model, lw, lh);
        let (_, body_y, body_w, body_h) = modals::analysis_modal_body_geom(x, y, pw, ph);
        let manual = self.model.fields.contains_key("analysis_font_manual");
        let cap_fs = if manual {
            modals::analysis_font(&self.model)
        } else {
            26.0
        };
        let slices = modals::analysis_page_slices(&fa, &self.model, body_w, cap_fs, body_h);
        let total = modals::analysis_total_at(&fa, &self.model, body_w, cap_fs);
        if slices.len() <= 1 {
            return None;
        }
        let saved_font = self.model.fields.get("analysis_font").cloned();
        let saved_manual = self.model.fields.get("analysis_font_manual").cloned();
        let saved_scroll = self.model.analysis_dock_scroll;
        self.model
            .fields
            .insert("analysis_font".into(), format!("{cap_fs}"));
        self.model
            .fields
            .insert("analysis_font_manual".into(), "true".into());
        let mut parts: Vec<Vec<u8>> = Vec::new();
        let mut failure: Option<String> = None;
        for page in 0..slices.len() {
            // 面板滚动到底后会夹在 total-body_h：末页名义起点可能比实际滚动量更大，
            // 直接把差值补进截图偏移，否则末页会把上一页的尾巴再画一遍（重影）。
            let start = slices[page].0.max(0.);
            let actual = start.min((total - body_h).max(0.));
            let delta = (start - actual).max(0.);
            self.model.analysis_dock_scroll = actual;
            self.draw();
            if let Some(overlay) = self.overlay.as_mut() {
                overlay.retain_export_frame(true);
            }
            self.draw();
            let slice = slices[page].1.max(1.);
            // 【长截图 2026-11-15】第一片连面板头部（标题 + 码长/字数/选重/缺字/理论键
            // 摘要行 + 工具条）一起截，否则长图里只剩文章本体，看不出统计口径。
            let (crop_top, crop_h) = if page == 0 {
                let head = (body_y - y - 8.).max(0.);
                (PAGE_MARGIN_Y + 8. + delta, head + slice)
            } else {
                (PAGE_MARGIN_Y + (body_y - y) + delta, slice)
            };
            // 横向取整块面板（不是 body 的 24px 内缩），否则标题/字号/工具条左右被裁。
            let crop = Some((
                (PAGE_MARGIN_X * sc) as u32,
                (crop_top * sc) as u32,
                (pw * sc) as u32,
                (crop_h * sc) as u32,
            ));
            let shot = match self.overlay.as_mut() {
                Some(overlay) => overlay.export_dib(crop),
                None => Err("独立窗未就绪，无法生成长图".to_string()),
            };
            match shot {
                Ok(part) => parts.push(part),
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        self.model.analysis_dock_scroll = saved_scroll;
        match saved_font {
            Some(v) => self.model.fields.insert("analysis_font".into(), v),
            None => self.model.fields.remove("analysis_font"),
        };
        match saved_manual {
            Some(v) => self.model.fields.insert("analysis_font_manual".into(), v),
            None => self.model.fields.remove("analysis_font_manual"),
        };
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.retain_export_frame(false);
        }
        let owner = self
            .overlay
            .as_ref()
            .map(|ov| ov.hwnd_isize())
            .unwrap_or_else(|| self.win.hwnd().0 as isize);
        let result = match failure {
            Some(error) => Err(error),
            None => gale_win::clipboard_bridge::write_long_dib(owner, &parts),
        };
        Some(result)
    }

    /// 把跟打器配置与晴账号写成云同步用的 profile.json（明文，走用户自己的 WebDAV）。
    fn write_sync_profile(&mut self, cfg: &Config) {
        self.settings_writer.queue(
            cfg.clone(),
            self.sync_online_user.clone(),
            self.sync_online_password.clone(),
            self.sync_account_cleared,
            self.sync_identity_dirty,
        );
        // The worker retains this transition until it has saved successfully,
        // including when ordinary edits supersede this queued draft.
        self.sync_identity_dirty = false;
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
        self.anim.caret_target = None;
        self.model.words.clear();
        // 【换段提速 2026-11-12】旧段请求即使回来也会因 generation 变化被
        // 丢弃——不必等它：立即解除单飞标志，让新段请求马上发出，
        // 否则换段要干等旧请求在途数秒（着色延迟主因）。
        self.analyzing = false;
        dbg_hint("reset_host_clear (换段/重置)".into());
        // Switching segments used to clear the prefetched whole-book hints and
        // wait for another service round-trip. Re-slice the cached words first;
        // The independent status poll refreshes the schema in the background.
        let hit = self.refill_from_full_words();
        dbg_hint(format!("reset_host refill_hit={hit} words={}", self.model.words.len()));
        self.generation = self.generation.wrapping_add(1);
        self.full_retry.reset();
        // 【换段提速】新段是新语境：旧段攒下的深退避不惩罚新段首请求。
        self.display_retry.reset();
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
            // 【词提诊断 2026-11-12】记录缓存未命中的具体守卫，定位换段空窗。
            dbg_hint(format!(
                "refill_miss whole={} empty={} text_eq={} breaks_eq={} strat_eq={} spans_eq={} auto4={} dyn_eq={} ver_eq={}",
                self.last_analyze_whole,
                self.full_words.is_empty(),
                self.full_words_of == self.analysis_text(),
                self.full_words_breaks == self.analysis_breaks(),
                self.full_words_strategy == self.cfg.hint_strategy,
                self.full_words_required_spans == self.analysis_required_spans(),
                self.full_words_auto4,
                self.full_words_dynamic_boundary == self.analysis_dynamic_boundary(),
                self.full_words_ver == self.model.schema_ver,
            ));
            return false;
        }
        self.refill_segment_words();
        true
    }
    /// 整改2a：在线文章直达跟打区（预览流程退役的唯一载入口）。
    /// The online worker filters superseded requests before reaching here.
    /// 回到历史里第 idx 篇；成功返回 true。上一段/下一段用它，
    /// 不再每次重新随机（用户 2026-11-14 要求「上一段要回到刚才那篇」）。
    fn show_history_article(&mut self, idx: usize) -> bool {
        let Some(a) = self.article_history.get(idx).cloned() else {
            return false;
        };
        self.article_history_pos = idx;
        self.model.online_title = a.title.clone();
        self.model.online_preview = a.text.clone();
        self.model.online_status = format!("{} 字· {}", a.text.chars().count(), a.seg_mark);
        self.model.online_article = Some(a.clone());
        self.article = Some(a);
        self.load_online_article(false);
        true
    }

    fn load_online_article(&mut self, _confirmed: bool) {
        // Explicit load commands always replace the text immediately.
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
        if self.sess.normalize_count > 0 {
            let (a, b) = self.sess.normalize_chars;
            // 【五轮】把规范化引起的字符数变化亮出来（省略号收敛/空白删除/CRLF），
            // 载文"少字"直接可解释，不再让用户猜。
            let delta = if a != b { format!("，总字数 {a}→{b}") } else { String::new() };
            self.toast(&format!(
                "已规范 {} 处标点引号{delta}",
                self.sess.normalize_count
            ));
        }
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
            ("hint_rank_corner", self.cfg.hint_rank_corner),
            ("hint_color_words", self.cfg.hint_color_words),
            ("hint_rank_colors", self.cfg.hint_rank_colors),
            ("hint_dynamic", self.cfg.hint_dynamic),
            ("hint_underline", self.cfg.hint_underline),
            ("hint_word_bg", self.cfg.hint_word_bg),
            ("score_auto_copy", self.cfg.score_auto_copy),
            ("auto_send_score", self.cfg.auto_send_score),
            // 【视图体系 2026-11-12】F1 设置页「界面与显示」用同一 fields 通道。
            ("minimal", self.cfg.minimal),
            ("minimal_hud", self.cfg.minimal_hud),
            ("always_on_top", self.cfg.always_on_top),
            ("hint_rank_bold", self.cfg.hint_rank_bold),
            ("show_preedit", self.cfg.show_preedit),
        ] {
            self.model.fields.insert(key.into(), value.to_string());
        }
        self.model
            .fields
            .insert("display_mode".into(), self.cfg.display_mode.as_str().into());
        self.model
            .fields
            .insert("split_ratio".into(), format!("{:.0}", self.cfg.split_ratio * 100.));
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
        self.model.fields.insert(
            "hint_rank_offset".into(),
            format!("{:.0}", self.cfg.hint_rank_offset),
        );
        for (key, value) in [
            ("hint_code_offset", self.cfg.hint_code_offset),
            ("preedit_font_size", self.cfg.preedit_font_size),
            ("preedit_offset", self.cfg.preedit_offset),
        ] {
            self.model.fields.insert(key.into(), format!("{value:.0}"));
        }
        self.model
            .fields
            .insert("score_fields".into(), self.cfg.score_fields.clone());
        self.model.fields.insert(
            "score_preset".into(),
            current_score_preset(&self.cfg).into(),
        );
        // 颜色键以设置页三份清单为准（含「未打文字色」与词提/选重配色）。
        for key in modals::preferences::all_color_keys() {
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
        // 【数值直输】任何其它偏好操作先退出数值编辑态（防编辑器悬空）。
        if !self.model.field("pref_num_field").is_empty()
            && !command.starts_with("pref_num_edit")
        {
            self.model.fields.insert("pref_num_field".into(), String::new());
            gale_win::input_host::set_num_edit(false);
        }
        match parts.as_slice() {
            ["pref_strategy", strategy] => {
                self.cfg.hint_strategy = HintStrategy::from_str_lossy(strategy);
            }
            ["pref_toggle", key] => {
                let field = match *key {
                    "hint_show_code" => &mut self.cfg.hint_show_code,
                    "hint_show_rank" => &mut self.cfg.hint_show_rank,
                    "hint_rank_corner" => &mut self.cfg.hint_rank_corner,
                    "hint_color_words" => &mut self.cfg.hint_color_words,
                    "hint_rank_colors" => &mut self.cfg.hint_rank_colors,
                    "hint_dynamic" => &mut self.cfg.hint_dynamic,
                    "hint_underline" => &mut self.cfg.hint_underline,
                    "hint_word_bg" => &mut self.cfg.hint_word_bg,
                    "score_auto_copy" => &mut self.cfg.score_auto_copy,
                    "auto_send_score" => &mut self.cfg.auto_send_score,
                    // 【视图体系 2026-11-12】界面与显示页
                    "minimal" => &mut self.cfg.minimal,
                    "minimal_hud" => &mut self.cfg.minimal_hud,
                    "always_on_top" => &mut self.cfg.always_on_top,
                    "hint_rank_bold" => &mut self.cfg.hint_rank_bold,
                    "show_preedit" => &mut self.cfg.show_preedit,
                    _ => return,
                };
                *field = !*field;
                if *key == "always_on_top" {
                    gale_win::window::set_topmost(self.win.hwnd(), self.cfg.always_on_top);
                }
                if *key == "minimal" && self.cfg.minimal {
                    self.model.ledger_open = false;
                    self.model.selected_history = None;
                    self.model.analysis_dock = false;
                }
                self.anim.caret = None;
        self.anim.caret_target = None;
            }
            ["pref_mode", mode] => {
                self.cfg.display_mode = DisplayMode::from_str_lossy(mode);
                self.anim.caret = None;
        self.anim.caret_target = None;
                self.layout_key = (u64::MAX, 0, 0);
                self.input_layout_key = (0, 0, 0, 0);
            }
            // 【数值直输 2026-11-13】点外观页数值进入编辑：缓冲从当前值起。
            // Enter 提交（clamp 后写配置）、Esc 取消；其它偏好点击先退出编辑。
            ["pref_num_edit", field @ ("font" | "line" | "letter" | "hint_font" | "hint_code_offset" | "hint_rank_offset" | "preedit_font" | "preedit_offset")] => {
                let init = match *field {
                    "font" => format!("{:.0}", self.cfg.font_size),
                    "line" => format!("{:.2}", 1. + self.cfg.line_gap),
                    "hint_font" => format!("{:.0}", self.cfg.hint_font_size),
                    "hint_code_offset" => format!("{:.0}", self.cfg.hint_code_offset),
                    "hint_rank_offset" => format!("{:.0}", self.cfg.hint_rank_offset),
                    "preedit_font" => format!("{:.0}", self.cfg.preedit_font_size),
                    "preedit_offset" => format!("{:.0}", self.cfg.preedit_offset),
                    _ => format!("{:.0}", self.cfg.letter_spacing),
                };
                self.model.fields.insert("pref_num_field".into(), (*field).into());
                self.model.fields.insert("pref_num_buf".into(), init);
                self.model.fields.insert("pref_num_fresh".into(), "true".into());
                gale_win::input_host::set_num_edit(true);
            }
            ["pref_step", "split_ratio", delta] => {
                if let Ok(delta) = delta.parse::<f32>() {
                    self.cfg.split_ratio = (self.cfg.split_ratio + delta * 0.02).clamp(0.15, 0.97);
                }
            }
            ["pref_step", "hint_font_size", delta] => {
                if let Ok(delta) = delta.parse::<f32>() {
                    // 七轮：上限 28→100（用户不设人为上限）；下限 14→10
                    self.cfg.hint_font_size = (self.cfg.hint_font_size + delta).clamp(10., 100.);
                }
            }
            // 【2026-11-13】字间距 / 选重数字高低（界面与显示页）
            ["pref_step", "letter_spacing", delta] => {
                if let Ok(delta) = delta.parse::<f32>() {
                    self.cfg.letter_spacing = (self.cfg.letter_spacing + delta).clamp(0., 12.);
                    self.layout_key = (u64::MAX, 0, 0);
                    self.input_layout_key = (0, 0, 0, 0);
                }
            }
            ["pref_step", "hint_rank_offset", delta] => {
                if let Ok(delta) = delta.parse::<f32>() {
                    self.cfg.hint_rank_offset = (self.cfg.hint_rank_offset + delta).clamp(-100., 100.);
                }
            }
            ["pref_step", key @ ("hint_code_offset" | "preedit_font_size" | "preedit_offset"), delta] => {
                if let Ok(delta) = delta.parse::<f32>() {
                    let (value, min, max) = match *key {
                        "hint_code_offset" => (&mut self.cfg.hint_code_offset, -100., 100.),
                        "preedit_font_size" => (&mut self.cfg.preedit_font_size, 10., 100.),
                        _ => (&mut self.cfg.preedit_offset, -100., 100.),
                    };
                    *value = (*value + delta).clamp(min, max);
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
    /// 【数值直输 2026-11-13】数值编辑态按键：数字/退格编辑缓冲，Enter 提交、Esc 取消。
    fn handle_num_key(&mut self, vk: u32) {
        let field = self.model.field("pref_num_field");
        if field.is_empty() {
            return;
        }
        if vk == 0x1B {
            self.model.fields.insert("pref_num_field".into(), String::new());
            gale_win::input_host::set_num_edit(false);
            return;
        }
        if vk == 0x0D {
            if let Ok(v) = self.model.field("pref_num_buf").parse::<f32>() {
                match field.as_str() {
                    "font" => self.cfg.font_size = v.clamp(10., 100.),
                    "hint_font" => self.cfg.hint_font_size = v.clamp(10., 100.),
                    "hint_code_offset" => self.cfg.hint_code_offset = v.clamp(-100., 100.),
                    "hint_rank_offset" => self.cfg.hint_rank_offset = v.clamp(-100., 100.),
                    "preedit_font" => self.cfg.preedit_font_size = v.clamp(10., 100.),
                    "preedit_offset" => self.cfg.preedit_offset = v.clamp(-100., 100.),
                    // 直输的是「行距倍数」：存回 line_gap 需减 1（0.10..=3.00 倍）。
                    "line" => self.cfg.line_gap = (v - 1.).clamp(-0.90, 2.0),
                    _ => {
                        self.cfg.letter_spacing = v.clamp(0., 12.);
                        self.layout_key = (u64::MAX, 0, 0);
                        self.input_layout_key = (0, 0, 0, 0);
                    }
                }
                self.save_config();
                self.sync_preferences(false);
            }
            self.model.fields.insert("pref_num_field".into(), String::new());
            gale_win::input_host::set_num_edit(false);
            return;
        }
        let mut buf = self.model.field("pref_num_buf");
        let fresh = self.model.field("pref_num_fresh") == "true";
        // The input host queues A only after capturing Ctrl+A at keydown.
        // Ctrl may already be released by the time this queue is consumed.
        if vk == 0x41 {
            self.model.fields.insert("pref_num_fresh".into(), "true".into());
            return;
        }
        if fresh {
            buf.clear();
            self.model.fields.insert("pref_num_fresh".into(), "false".into());
        }
        if vk == 0x08 {
            buf.pop();
        } else if buf.len() < 6 {
            let ch = match vk {
                0x30..=0x39 => (b'0' + (vk - 0x30) as u8) as char,
                0x60..=0x69 => (b'0' + (vk - 0x60) as u8) as char,
                0xBD | 0x6D if buf.is_empty() => '-',
                0xBE | 0x6E => {
                    if buf.contains('.') {
                        return;
                    }
                    '.'
                }
                _ => return,
            };
            buf.push(ch);
        }
        self.model.fields.insert("pref_num_buf".into(), buf);
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
        if !(self.model.analysis_dock || self.model.modal == Modal::Analysis)
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
    /// 【2026-11-15】理论分析不再占下半屏停靠卡：点一次开整页面板，
    /// 再点一次（或点窗口空白处）关掉。
    fn toggle_analysis_dock(&mut self) {
        if self.model.modal == Modal::Analysis {
            self.open(Modal::None);
            return;
        }
        self.open(Modal::None);
        self.model.menu = None;
        self.model.ledger_open = false;
        self.model.selected_history = None;
        self.model.analysis_dock = false;
        self.open(Modal::Analysis);
        self.full_retry.reset();
        self.analyzed = 0;
        self.analyze();
        self.fetch_full_analysis();
        gale_win::window::request_frame();
    }

    fn fit_analysis_view(&mut self, w: f32, h: f32) {
        if self.model.analysis_dock
            && self.model.modal == Modal::None
            && !modals::analysis_dock_fits(&self.model, &self.cfg, w, h)
        {
            // Expand once; do not collapse the user's window when it is resized
            // again or when they switch back from the keyboard to the article.
            self.open(Modal::Analysis);
        }
    }

    /// 整改12收口：打开当前段落键盘热力图模态（数据=最近结算段
    /// last_vk_histogram；尚无记录时提示而非弹空图）。
    /// 【热力图入口恢复 2026-11-14】原入口在单段结算弹窗（2026-11-09 退役），
    /// 现由右键菜单「键位热力图」chip 进入；走 model.open 以重置滚动/居中。
    fn show_heatmap(&mut self) {
        if self.sess.last_vk_histogram.is_empty() {
            self.toast("还没有按键记录——打完一段结算后可看热力图");
            return;
        }
        self.open(Modal::Heatmap);
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
        self.model.ledger_scroll = offset.min(crate::history::scroll_max(&self.cfg, &self.model, lh));
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
                let cap = crate::history::scroll_max(&self.cfg, &self.model, lh);
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
                match prefs.save(&self.dir.join("online.json")) {
                    Ok(()) => self.model.mark_saved(unix_now()),
                    Err(e) => {
                        self.model.mark_save_failed(&e.to_string());
                        self.toast(format!("在线偏好保存失败：{e}"));
                    }
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
            // 【2026-11-15】手动点过字号就固定用户值，不再按画布自动铺满。
            self.model
            .fields
            .insert("analysis_font_manual".into(), "true".into());
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
                self.model.toast = None;
                // 【截图复制 2026-11-15】用户要「整篇文章」而不是当前一屏：
                // 底部停靠面板只排一屏（其余靠滚动），而弹窗会把整篇缩排进一页，
                // 所以先升成弹窗再截图——粘出去就是整篇，屏幕上也看得见全篇。
                if self.model.analysis_dock && self.model.modal != Modal::Analysis {
                    self.model.analysis_dock = false;
                    self.model.modal = Modal::Analysis;
                }
                // 【长截图 2026-11-15】长文自动缩字号会小到看不清，改成「固定可读
                // 字号 + 分页渲染 + 竖向拼成一张长图」，粘出去就是整篇长图。
                if let Some(copied) = self.copy_analysis_long_image() {
                    match copied {
                        Ok(()) => self.toast("整篇分析长图已复制"),
                        Err(error) => self.toast(error),
                    }
                    return;
                }
                self.draw();
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.retain_export_frame(true);
                    // Capture only this requested frame. The normal typing and
                    // hover path keeps frame replay disabled.
                    self.draw();
                }
                let sc = gale_win::window::dpi_scale();
                let (cw, ch) = self.win.client_size();
                let (lw, lh) = (cw as f32 / sc, ch as f32 / sc);
                // 【二级页拖动 2026-11-14】弹窗模式下必须裁弹窗自己的矩形。
                // 以前一律裁底部停靠面板，弹窗里点「复制图片」会复制到错误区域。
                let (x, y, pw, ph) = if self.model.modal == Modal::Analysis {
                    modals::panel_for_model(&self.model, lw, lh)
                } else {
                    modals::analysis_dock_rect_for(&self.cfg, lw, lh)
                };
                let crop = Some((
                    (x * sc) as u32,
                    (y * sc) as u32,
                    (pw * sc) as u32,
                    (ph * sc) as u32,
                ));
                // The dock is painted by its own renderer. The main surface
                // deliberately excludes it, so copying there exports article
                // background even when the button was hit correctly.
                let copied = if let Some(overlay) = self.overlay.as_mut().filter(|_| {
                    self.model.analysis_dock || page_in_overlay(self.model.modal)
                }) {
                    let local_crop = if page_in_overlay(self.model.modal) {
                        let (fw, fh) = self.overlay_frame.unwrap_or((lw, lh));
                        let (_, _, pw, ph) = modals::panel_base(&self.model, fw, fh);
                        Some((
                            (PAGE_MARGIN_X * sc) as u32,
                            (PAGE_MARGIN_Y * sc) as u32,
                            (pw * sc) as u32,
                            (ph * sc) as u32,
                        ))
                    } else {
                        None
                    };
                    overlay.copy_bmp_to_clipboard(local_crop)
                } else {
                    self.renderer.copy_bmp_to_clipboard(crop)
                };
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.retain_export_frame(false);
                }
                match copied {
                    Ok(()) => self.toast("当前分析图片已复制"),
                    Err(error) => self.toast(error),
                }
            }
            AuxAction::ShortcutRecord(id) => {
                self.model
                    .fields
                    .insert("pref_recording".into(), id);
                gale_win::input_host::begin_shortcut_recording();
                self.toast("按下新的组合键（Esc 取消）");
                gale_win::window::request_frame();
            }
            AuxAction::ShortcutReset => {
                gale_win::input_host::cancel_shortcut_recording();
                self.model.fields.remove("pref_recording");
                self.cfg.shortcuts.clear();
                self.save_config();
                self.apply_shortcuts();
                self.toast("快捷键已恢复默认");
                gale_win::window::request_frame();
            }
        }
    }

    /// 把当前配置解析成 (组合键, 命令) 表装进输入宿主。
    fn apply_shortcuts(&mut self) {
        let mut entries = Vec::new();
        for (id, _, _) in gale_core::config::SHORTCUT_DEFAULTS {
            if let Some(command) = shortcut_command(id) {
                let chord = self.cfg.shortcut(id);
                if !chord.is_empty() {
                    entries.push((chord, command));
                }
            }
        }
        gale_win::input_host::set_shortcuts(&entries);
    }

    /// 录制收尾：取到组合键就写入配置，取消/失焦就清掉录制态。
    fn handle_shortcut_recording(&mut self) {
        let recording = self.model.field("pref_recording");
        if let Some(chord) = gale_win::input_host::take_recorded_shortcut() {
            if !recording.is_empty() {
                self.assign_shortcut(&recording, &chord);
            }
            self.model.fields.remove("pref_recording");
            gale_win::window::request_frame();
            return;
        }
        if recording.is_empty() {
            return;
        }
        if !gale_win::input_host::shortcut_recording_active()
            || self.model.modal != Modal::Preferences
        {
            gale_win::input_host::cancel_shortcut_recording();
            self.model.fields.remove("pref_recording");
            gale_win::window::request_frame();
        }
    }

    /// 校验并保存一个新快捷键（F1–F24 或含 Ctrl/Alt；拒绝固定键与重复）。
    fn assign_shortcut(&mut self, id: &str, chord: &str) {
        let Some(spec) = gale_win::input_host::parse_chord(chord) else {
            self.toast("这个组合键无法识别，换一个试试");
            return;
        };
        let f_key = (0x70..=0x87).contains(&spec.vk);
        if !(spec.ctrl || spec.alt || f_key) {
            self.toast("快捷键需要 F1–F24 或包含 Ctrl / Alt");
            return;
        }
        if gale_win::input_host::shortcut_reserved(chord) {
            self.toast("这个键是打字/编辑固定键，不能占用");
            return;
        }
        for (other, label, _) in gale_core::config::SHORTCUT_DEFAULTS {
            if *other == id {
                continue;
            }
            if self.cfg.shortcut(other).eq_ignore_ascii_case(chord) {
                self.toast(format!("「{label}」已占用 {chord}，先改那一个"));
                return;
            }
        }
        self.cfg.shortcuts.insert(id.to_string(), chord.to_string());
        self.save_config();
        self.apply_shortcuts();
        self.toast(format!("快捷键已设为 {chord}"));
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
        self.toast(format!("正在发送到「{}」", chat.title));
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
        // 2026-11-13：整体界面底色「跟打区背景色」（升格），旧值「窗体背景色」向后兼容。
        let bg = ui::rgba(ui::interface_bg(&self.theme));
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
        // 打开模态面板时收起视图菜单（关闭后不残影复现）。
        self.model.menu = None;
        if m == Modal::Preferences {
            self.sync_preferences(true);
        }
        if m != Modal::None {
            self.model.ledger_open = false;
            self.model.analysis_dock = false;
        }
        self.model.open(m);
        self.modal_drag = None;
        self.overlay_placement.reset();
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
    /// 【拖动流畅 2026-11-14】拖动中不等渲染帧：输入事件（Move）里直接把
    /// overlay 窗口几何推到位。窗口移动是纯 SetWindowPos——DComp 内容随窗口
    /// 整体平移，无需重绘内容——125Hz 输入率下窗口跟手。此前位置只在
    /// draw() 里更新，更新率被渲染耗时拖住（SETRECT 22 次 vs DRAGMOVE 41 次）
    /// → 用户实测「拖动不流畅」。与 draw() 的帧定位共用实际拖动状态，
    /// set_rect 幂等（same=true 静默），两路叠加不冲突。
    fn reposition_overlay_now(&mut self) {
        if !page_in_overlay(self.model.modal) || self.overlay.is_none() {
            return;
        }
        let (cw, ch) = self.win.client_size();
        let sc = gale_win::window::dpi_scale().max(0.01);
        let (lw, lh) = (cw as f32 / sc, ch as f32 / sc);
        let (fx, fy, fw, fh) = self.overlay_placement.fit(self.win.hwnd, &self.model, lw, lh);
        if let Some(ov) = self.overlay.as_mut() {
            if ov.set_rect(fx, fy, fw, fh, sc).is_ok() {
                self.overlay_frame = Some((fw, fh));
            }
        }
    }

    fn draw(&mut self) {
        if self.renderer.set_body_family(&self.cfg.font_family) {
            self.layout_key = (u64::MAX, 0, 0);
        }
        // 【overlay 字体 2026-11-14】独立窗 Renderer 有自己的 FontKit，
        // 与主窗互不相通：每帧同步 cfg.font_family（新 overlay 首帧即收敛，
        // 字体选择器换字体即刻生效）。
        if let Some(ov) = self.overlay.as_mut() {
            ov.set_body_family(&self.cfg.font_family);
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
        self.fit_analysis_view(lw, lh);
        let now = self.clock.elapsed().as_secs_f64();
        // 二级页画在独立窗里：滚动上限按窗口高度（可能被工作区收边）算。
        let view_h = if page_in_overlay(self.model.modal) {
            self.overlay_frame.map(|f| f.1).unwrap_or(lh)
        } else {
            lh
        };
        self.model.panel_scroll = self.model.panel_scroll.clamp(
            0., modals::panel_scroll_max(&self.model, lw, lh, view_h),
        );
        let (bw, fs, _base_lh) = ui::body_metrics(&self.sess, &self.cfg, lw);
        let key = (self.generation, fs.to_bits(), bw.to_bits());
        let layout_rebuilt = self.layout_key != key;
        if layout_rebuilt {
            self.layout_base = self
                .renderer
                .wrap_text(&self.sess.current_segment().text, fs, bw);
            self.layout_key = key;
            self.rewrap_words_key = u64::MAX;
        }
        // 【词提重排缓存 2026-11-14】词提重排原来每帧重跑 O(正文)；
        // 现在用词边界指纹（FNV over (start,len)）做键，只在布局重建
        // 或词边界变化时从基准 layout_base 重排（避免在已重排布局上
        // 叠加二阶重排）。词提关闭时恢复基准布局，保持口径统一。
        if self.model.hints {
            let mut wh: u64 = 0xcbf2_9ce4_8422_2325;
            for word in &self.model.words {
                if word.code.is_some() && analysis::is_multi_word(&word.text) {
                    wh = (wh
                        ^ (word.start as u64).rotate_left(13)
                        ^ (word.len as u64).rotate_left(29))
                        .wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            if self.rewrap_words_key != wh {
                let boundaries: Vec<_> = self
                    .model
                    .words
                    .iter()
                    .filter(|word| word.code.is_some() && analysis::is_multi_word(&word.text))
                    .map(|word| (word.start, word.len))
                    .collect();
                self.layout =
                    match gale_win::d2d::rewrap_words_to_width(&self.layout_base, &boundaries, bw)
                    {
                        Some(lines) => lines,
                        None => self.layout_base.clone(),
                    };
                self.rewrap_words_key = wh;
            }
        } else if layout_rebuilt || self.rewrap_words_key != u64::MAX {
            self.layout = self.layout_base.clone();
            self.rewrap_words_key = u64::MAX;
        }
        // 【对照模式 2026-11-12】输入卡文字：用真实 DirectWrite 宽度换行，
        // 缓存键 = 已打字符哈希 + 字号 + 正文宽 + 段号（退格/回改都会变）。
        if self.cfg.display_mode.is_split() {
            let typed: String = self.sess.comparer.typed.iter().collect();
            let key = (
                text_hash(&typed),
                fs.to_bits() as u64,
                bw.to_bits() as u64,
                self.sess.index as u64,
            );
            if key != self.input_layout_key {
                self.input_lines = if typed.is_empty() {
                    Vec::new()
                } else {
                    self.renderer.wrap_text(&typed, fs, bw)
                };
                self.input_layout_key = key;
            }
        }
        self.model.pointer = self.hover;
        let now_ms = self.ms();
        // 【独立二级窗 2026-11-14】理论分析 dock 由独立顶层窗口承载。
        // 先建窗口再建主帧，这样主帧可以跳过 dock 绘制；overlay 直接用
        // 主帧坐标偏移渲染 dock 区域，窗口可被拖到主窗之外。
        let dock = if self.model.analysis_dock && self.model.modal == Modal::None {
            Some(modals::analysis_dock_rect_for(&self.cfg, lw, lh))
        } else {
            None
        };
        let overlay_page = if page_in_overlay(self.model.modal) {
            Some(self.model.modal)
        } else {
            None
        };
        let overlay_key = if dock.is_some() {
            Some(Modal::Analysis)
        } else {
            overlay_page
        };
        if self.overlay_key != overlay_key {
            // 【拖动排查 2026-11-14】overlay 生命周期插桩：谁在什么时刻拆/建窗口。
            gale_win::input_host::diag_log(&format!(
                "overlay lifecycle: destroy old={:?} new={:?} modal={:?} drag={}",
                self.overlay_key.is_some(),
                overlay_key.is_some(),
                self.model.modal,
                self.modal_drag.is_some()
            ));
            // Keep the GPU device, font collection and text caches warm across
            // F1/Esc and page switches. Closing a panel hides its one owned
            // window; it must not recreate Direct3D/DirectWrite on every click.
            if let Some(overlay) = self.overlay.as_ref() {
                overlay.show(false);
            }
            self.overlay_frame = None;
            self.overlay_key = overlay_key;
            self.modal_drag = None;
            self.overlay_placement.reset();
        }
        if self.overlay.is_none() {
            self.overlay_frame = None;
            if let Some((dx, dy, dw, dh)) = dock {
                let sc = gale_win::window::dpi_scale();
                self.overlay = overlay::Overlay::create(self.win.hwnd, dx, dy, dw, dh, sc).ok();
            } else if overlay_page.is_some() {
                // 【拖动范围 2026-11-14】overlay 定位用免主窗钳制变体：顶层窗
                // 可整体游进主窗上方/侧边的屏幕区域，收敛交给 fit_to_work。
                let sc = gale_win::window::dpi_scale();
                // 【独立窗 2026-11-14 返工】窗口仍比面板大一圈（16/24），
                // 但这圈现在是**完全透明**（overlay 帧不再画整窗遮罩），
                // 视觉上就是「只有卡片、没有黑边」；保留这一圈是为了
                // panel_for_model 的面板几何与主窗内完全一致（不再二选一）。
                let (fx, fy, fw, fh) = self.overlay_placement.fit(self.win.hwnd, &self.model, lw, lh);
                let o = overlay::Overlay::create(self.win.hwnd, fx, fy, fw, fh, sc).ok();
                // 【拖动排查 2026-11-14】建窗插桩（含失败：err 也打点；含 hwnd 供对照）。
                gale_win::input_host::diag_log(&format!(
                    "overlay lifecycle: create at ({fx:.0},{fy:.0}) {fw:.0}x{fh:.0} hwnd=0x{:x} ok={}",
                    o.as_ref().map(|x| x.hwnd().0 as usize).unwrap_or(0),
                    o.is_some()
                ));
                if o.is_some() {
                    self.overlay_frame = Some((fw, fh));
                }
                self.overlay = o;
            }
        }
        // 【命中与尺寸 2026-11-14】窗口只在 key 变化时创建；但面板高度会随内容
        // 变化（设置页切标签、规则页增删规则…）。窗口若比卡片大，多出来的透明圈
        // 会吃掉边缘点击，表现为「UI 与点击对不上」。每帧把窗口尺寸校到卡片尺寸。
        if overlay_page.is_some() {
            // 首次/重开和后续帧共用同一定位规则。缓存窗已存在不代表用户拖过；
            // 只有实际手动位移才启用 loose，松手后继续保留可拖出屏幕的状态。
            let (fx, fy, fw, fh) = self.overlay_placement.fit(self.win.hwnd, &self.model, lw, lh);
            if let Some(ov) = self.overlay.as_mut() {
                let sc = gale_win::window::dpi_scale();
                if ov.set_rect(fx, fy, fw, fh, sc).is_ok() {
                    self.overlay_frame = Some((fw, fh));
                    // 【拖动范围 2026-11-14】停靠吸附：fit 收边后把 modal_shift
                    // 对齐到实际窗位（拖动中不吸，保持「手位移=去掉窗自位移」
                    // 的绝对映射；吸附幂等，防止边缘停靠累积出大片死区）。
                    if self.modal_drag.is_none() {
                        let (bx2, by2, _, _) = modals::panel_base(&self.model, lw, lh);
                        let want_x = fx + PAGE_MARGIN_X - bx2;
                        let want_y = fy + PAGE_MARGIN_Y - by2;
                        if (self.model.modal_shift.0 - want_x).abs() >= 0.5
                            || (self.model.modal_shift.1 - want_y).abs() >= 0.5
                        {
                            self.model.modal_shift = (want_x, want_y);
                        }
                    }
                }
            }
        }
        self.model.skip_analysis_dock = dock.is_some() && self.overlay.is_some();
        let mut f = ui::build_with_input(
            &self.sess,
            &self.cfg,
            &self.theme,
            &self.model,
            lw,
            lh,
            now_ms,
            now,
            &mut self.anim,
            &mut self.hits,
            &self.layout,
            &self.input_lines,
        );
        self.model.skip_analysis_dock = false;
        let main_hit_count = self.hits.len();
        let page_overlay = overlay_page.is_some() && self.overlay.is_some();
        if let (true, Some(ov)) = (page_overlay, self.overlay.as_mut()) {
            ov.set_body_family(&self.cfg.font_family);
            // 绘制虚拟尺寸 = 实际窗口尺寸（收边后），否则面板会把页脚画到窗外。
            let (fw, fh) = self.overlay_frame.unwrap_or_else(|| {
                let (_, _, pw, ph) = modals::panel_for_model(&self.model, lw, lh);
                (pw + PAGE_MARGIN_X * 2., ph + PAGE_MARGIN_Y * 2.)
            });
            let mut mf = gale_win::d2d::Frame::new(fw, fh);
            let mut mh = Vec::new();
            modals::draw(
                &mut mf,
                &mut mh,
                &self.model,
                &self.sess,
                &self.cfg,
                &self.theme,
                fw,
                fh,
                now,
                // 独立窗：整圈留白保持透明，不画 0.72 遮罩（用户说的「大黑边」）
                false,
                // 【拖动分割 2026-11-14】帧内面板钉死在留白角：窗口定位（上方
                // set_rect/fit_to_work）已含 modal_shift，帧内再叠一次会把面板
                // 画到窗外被裁（「分割/消失」），× 按钮也会跟着飞出命中域。
                Some((PAGE_MARGIN_X, PAGE_MARGIN_Y)),
            );
            // 命中框可视化（GALE_HIT_DEBUG=1）：把每个命中的矩形描出来，
            // 用于肉眼核对「看得见的位置」与「点得到的位置」是否一致。
            if crate::hit_debug_enabled() {
                for h in &mh {
                    let c = [1.0, 0.15, 0.15, 1.0];
                    for (x, y, w, hh) in [
                        (h.x, h.y, h.w, 1.0),
                        (h.x, h.y + h.h - 1.0, h.w, 1.0),
                        (h.x, h.y, 1.0, h.h),
                        (h.x + h.w - 1.0, h.y, 1.0, h.h),
                    ] {
                        mf.push(gale_win::d2d::DrawCmd::Rect {
                            x,
                            y,
                            w,
                            h: hh,
                            color: c,
                        });
                    }
                }
            }
            let (ox, oy) = ov.origin_in_main();
            draw_hover(&mut mf, &mh, self.hover.map(|(x, y)| (x - ox, y - oy)), &self.theme);
            for h in &mut mh {
                h.x += ox;
                h.y += oy;
            }
            self.hits.extend(mh);
            let _ = ov.present(&mf);
            ov.show(true);
        } else if let (Some((dx, dy, dw, dh)), Some(ov)) = (dock, self.overlay.as_mut()) {
            ov.set_body_family(&self.cfg.font_family);
            // 【dock 跟随 2026-11-14】停靠面板的独立窗此前只在建窗时定位一次：
            // 主窗移动/缩放后 overlay 停在旧屏幕位置，面板脱钩成幽灵。每帧
            // set_rect 到当前 dock 矩形（same 早退，几乎零成本），与面板页
            // 每帧校准（上方 overlay_page 分支）同口径。
            let sc = gale_win::window::dpi_scale();
            let _ = ov.set_rect(dx, dy, dw, dh, sc);
            let mut mf = gale_win::d2d::Frame::new(lw, lh);
            let mut mh = Vec::new();
            modals::draw_analysis_dock_for(
                &mut mf,
                &mut mh,
                &self.model,
                &self.sess,
                &self.theme,
                &self.cfg,
                lw,
                lh,
            );
            modals::draw_toast(&mut mf, &self.model, &self.theme, (dx, dy, dw, dh), now);
            let (ox, oy) = ov.origin_in_main();
            let (sx, sy) = (ox - dx, oy - dy);
            draw_hover(&mut mf, &mh, self.hover.map(|(x, y)| (x - sx, y - sy)), &self.theme);
            for h in &mut mh {
                h.x += sx;
                h.y += sy;
            }
            self.hits.extend(mh);
            let _ = ov.present_region(&mf, dx, dy);
            ov.show(true);
        } else {
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
                true,
                None,
            );
        }
        if !page_overlay {
            // A dock appends overlay hits to the main window's list. A modal
            // drawn in the main window replaces that list, so its old length
            // must never be used to slice the fallback modal's hit targets.
            let hover_hits = if dock.is_some() && self.overlay.is_some() {
                &self.hits[..main_hit_count]
            } else {
                &self.hits
            };
            draw_hover(&mut f, hover_hits, self.hover, &self.theme);
        }
        // 【视图菜单 2026-11-12】菜单画在所有面板与 hover 之上；矩形回存供点击外部关闭。
        self.menu_rect = None;
        if self.model.modal == Modal::None && self.model.menu.is_some() {
            self.menu_rect = ctxmenu::draw(
                &mut f,
                &mut self.hits,
                &self.cfg,
                &self.model,
                &self.theme,
                lw,
                lh,
            );
        }
        // 光标提示：任意处可拖动（四向箭头）/ 分栏与成绩面板把手（上下）/ 控件上保持箭头。
        let over_divider = self.anim.split_divider.is_some_and(|(dx, dy, dw, dh)| {
            self.hover
                .is_some_and(|(hx, hy)| hx >= dx && hx <= dx + dw && hy >= dy && hy <= dy + dh)
        });
        let over_grip = {
            let sc = gale_win::window::dpi_scale();
            let (cw, ch) = self.win.client_size();
            let (lw, lh) = (cw as f32 / sc, ch as f32 / sc);
            crate::history::resize_grip_rect(&self.cfg, &self.model, lw, lh).is_some_and(
                |(gx, gy, gw, gh)| {
                    self.hover
                        .is_some_and(|(hx, hy)| hx >= gx && hx <= gx + gw && hy >= gy && hy <= gy + gh)
                },
            )
        };
        let hint = if self.model.menu.is_some() {
            gale_win::window::CursorHint::Arrow
        } else if self.split_drag || self.ledger_drag || over_divider || over_grip {
            gale_win::window::CursorHint::ResizeNS
        } else {
            gale_win::window::CursorHint::Arrow
        };
        gale_win::window::set_cursor_hint(hint);
        if let Err(e) = self.renderer.draw_frame(&f) {
            self.toast(format!("绘制失败：{e}"));
        }
        if self.model.modal == Modal::None {
            gale_win::window::set_focus_relay(true);
            // Keep a visible, focused native TSF host at the painted caret.
            // It has a real client surface; text itself is drawn by DirectWrite.
            let (cx, cy) = self
                .anim
                .caret_target
                .or(self.anim.caret)
                .unwrap_or((32., 270.));
            let host_line_h = self.anim.input_line_h.max(1.);
            let min_y = if self.cfg.minimal { 2. } else { 60. };
            let max_y = (lh - if self.cfg.minimal { 2. } else { 50. }).max(min_y);
            if gale_win::input_host::diagnostics_enabled() {
                gale_win::input_host::diag_log(&format!(
                    "HOST place cx={cx:.2} cy={cy:.2} fs={fs:.2} sc={sc:.3} line_h={host_line_h:.2} px={:.1} py={:.1}",
                    (cx.clamp(8., (lw - 8.).max(8.)) * sc),
                    ((cy + fs).clamp(min_y, max_y) * sc)
                ));
            }
            self.host.place(
                (cx.clamp(8., (lw - 8.).max(8.)) * sc) as i32,
                ((cy + fs).clamp(min_y, max_y) * sc) as i32,
                2,
                2,
            );
            // P2 组字窗/候选窗锚点（任务 8）：把绘制光标坐标 + 当前行行高
            //（DIP→物理像素 ×DPI）每帧下发给 IME 锚点（内部 3px 去抖）；
            // NativeEdit 2×2 贴层保留不动（双层：光标载体 + IME 锚点）。
            // 光标矩形顶 = 行顶 + 5（ui.rs caret 绘制几何）。
            let dpi = (sc * 96.).round() as u32;
            let _ = self.host.set_anchor_from_caret(
                cx.clamp(8., (lw - 8.).max(8.)) * sc,
                (cy + 5.).clamp(min_y, max_y) * sc,
                (host_line_h * sc).round() as i32,
                dpi,
            );
            self.host.show();
            self.host.hide_caret();
        }
        // 二级页迁到独立窗后，原生编辑框、字体下拉必须挂到 overlay 上，
        // 否则它们仍留在主窗，面板一拖就与控件脱节（Online/Rules 的
        // 服务器地址、规则编辑框同理）。
        let native_in_overlay = matches!(
            overlay_page,
            Some(Modal::Preferences) | Some(Modal::Online) | Some(Modal::Rules)
        ) && self.overlay.is_some();        let (nlw, nlh) = if native_in_overlay {
            // 与绘制窗口同一虚拟尺寸：收边后控件随之下移，不会落到窗外。
            self.overlay_frame.unwrap_or_else(|| {
                let (_, _, pw, ph) = modals::panel_for_model(&self.model, lw, lh);
                (pw + PAGE_MARGIN_X * 2., ph + PAGE_MARGIN_Y * 2.)
            })
        } else {
            (lw, lh)
        };
        let native_parent = if native_in_overlay {
            self.overlay.as_ref().map(|o| o.hwnd().0 as isize).unwrap_or(self.win.hwnd)
        } else {
            self.win.hwnd
        };
        let specs = modals::fields(
            &self.model,
            nlw,
            nlh,
            // 【拖动分割 2026-11-14】overlay 模式原生控件与绘制同源钉在留白角。
            native_in_overlay.then_some((PAGE_MARGIN_X, PAGE_MARGIN_Y)),
        );
        // Native child windows sit above DirectComposition. Retire fields as
        // soon as their page disappears, including direct view transitions
        // that did not rebuild the modal epoch.
        self.edits.retain(|id, _| specs.iter().any(|field| field.id == id));
        if self.edit_epoch != self.model.modal_epoch {
            self.edits.clear();
            for z in &specs {
                match NativeEdit::new(native_parent, z.multi, z.password) {
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
                    native_parent,
                    &self.cfg.font_family,
                ) {
                    Ok(mut picker) => {
                        // 【审计 V04】深色面板上的原生下拉框必须跟着暗色主题走，
                        // 否则会露出一块亮白框，和面板底色割裂。
                        picker.set_dark(
                            gale_core::theme::relative_luminance(self.theme.get("窗体背景色"))
                                < 0.35,
                        );
                        self.font_picker = Some(picker);
                    }
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
                let bottom = (z.y + z.h).min(nlh - 8.);
                if bottom <= top || z.h <= 0. {
                    e.hide();
                } else {
                    e.place(z.x, top, z.w, bottom - top);
                }
            }
        }
        let picker_rect = modals::font_picker(
            &self.model,
            nlw,
            nlh,
            native_in_overlay.then_some((PAGE_MARGIN_X, PAGE_MARGIN_Y)),
        );
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
        // 【轮询降频 2026-11-14】词提关闭或无文章=没人消费 schema → 5s；有文但挂起
        // （Idle/Done/暂停，无打字在推进）→ 2s；打字中保持 400ms（调频/schema_ver
        // 微变需及时跟进）。demand 路径（request_fresh）不受间隔限制仍立即轮询。
        let hint_consumer = self.model.hints
            && self
                .sess
                .segments
                .get(self.sess.index)
                .is_some_and(|s| !s.text.is_empty());
        self.live_schema.interval_ms = if !hint_consumer {
            5000
        } else if self.sess.idle_hint() {
            2000
        } else {
            400
        };
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
    fn invalidate_live_hints(&mut self, preserve_views: bool) {
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
        retire_hint_views(&mut self.model, &mut self.sess, preserve_views);
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
            // 【可观测性 2026-11-13】词提被快照门限整段拒绝时，底部状态行会被
            // 省略号截断，用户只会觉得「不出词提」。首次/变化时弹一次 toast。
            if self.model.field("hint_unavailable_reason") != reason {
                self.model
                    .fields
                    .insert("hint_unavailable_reason".into(), reason.into());
                let hint = if reason.contains("用户词或调整过多") {
                    format!("{reason}（可在当前方案里精简用户调整.txt 或用户码表后重试）")
                } else {
                    reason.to_string()
                };
                self.toast(hint);
            }
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
        // 【下一个字编码 2026-11-12】单字码表与词提同源触发，但独立守卫/缓存：
        // 词提关闭或本条分析被去重挡下时仍保持可用。
        let (cc_text, _, _, _, cc_whole) = self.hint_analysis_input();
        self.fetch_char_codes(&cc_text, cc_whole);
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
            dbg_hint(format!(
                "send_abort hints={} snapshot_reason={:?} words_was={}",
                hints,
                status["hint_unavailable_reason"].as_str(),
                self.model.words.len()
            ));
            self.analyzing = false;
            self.analyzed = generation;
            self.analyzed_key = request_key;
            self.display_retry.reset();
            // 【词提防闪烁 2026-11-12】仅用户显式关闭词提(!hints)才清空显示；
            // 快照临时不可用(引擎忙)时保留旧词提，状态轮询恢复后自动替换。
            if !hints {
                self.model.words.clear();
            }
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
                    dbg_hint(format!(
                        "analyze_ok words={}",
                        value["words"].as_array().map(|a| a.len()).unwrap_or(0),
                    ));
                    Ok(value)
                })
            })();
            let _ = tx.send(Bg::Analyze(generation, request_key, revision, r));
            wake(hwnd);
        });
    }
    /// 【下一个字编码 2026-11-12】单字码表后台请求：single=true 显示路由，
    /// 响应与请求文本逐字对齐。键=策略|文本；缓存到 model.char_codes。
    fn fetch_char_codes(&mut self, text: &str, whole_book: bool) {
        let key = format!("{}|{}", self.cfg.hint_strategy.as_str(), text);
        if self.char_codes_pending
            || (self.char_codes_of == key
                && self.char_codes_ver == self.model.schema_ver
                && self.model.char_codes_whole == whole_book
                && (whole_book || self.model.char_codes_seg == self.sess.index))
        {
            return;
        }
        self.char_codes_pending = true;
        let tx = self.tx.clone();
        let hwnd = self.win.hwnd;
        let generation = self.generation;
        let revision = self.live_schema.revision.clone();
        let request = text.to_string();
        let strategy = self.cfg.hint_strategy.as_str().to_string();
        let seg = self.sess.index;
        std::thread::spawn(move || {
            let r = local_json(
                local_port(),
                "/api/trainer/analyze",
                Some(serde_json::json!({
                    "text": request, "single": true, "theory_auto4": true,
                    "breaks": [], "strategy": strategy, "required_spans": [],
                    "dynamic_boundary": null, "route_mode": "display",
                })),
            );
            let _ = tx.send(Bg::CharCodes(generation, key, request, seg, whole_book, revision, r));
            wake(hwnd);
        });
    }
    fn collect_background(&mut self) {
        while let Ok(r) = self.rx.try_recv() {
            match r {
                Bg::CharCodes(g, key, text, seg, whole, revision, result) => {
                    self.char_codes_pending = false;
                    let (cur_text, _, _, _, cur_whole) = self.hint_analysis_input();
                    let current = g == self.generation
                        && revision == self.live_schema.revision
                        && whole == cur_whole
                        && key == format!("{}|{}", self.cfg.hint_strategy.as_str(), cur_text)
                        && (whole || seg == self.sess.index);
                    if !current {
                        self.fetch_char_codes(&cur_text, cur_whole);
                    } else if let Ok(value) = result {
                        if self.live_schema.revision.accepts(
                            &revision,
                            value["schema_ver"].as_str().unwrap_or(""),
                        ) {
                            if let Ok(codes) = analysis::response_char_codes(&text, &value) {
                                self.char_codes_of = key;
                                self.char_codes_ver =
                                    value["schema_ver"].as_str().unwrap_or("").to_string();
                                self.model.char_codes = codes;
                                self.model.char_codes_whole = whole;
                                self.model.char_codes_seg = seg;
                                gale_win::window::request_frame();
                            }
                        }
                    }
                }
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
                                // 【词提防闪烁 2026-11-12】打字调频让 schema_ver
                                // 持续微变；词库仍可用时保留旧词提（后台重拉
                                // 到达后原子替换），只有词库不可用才清空显示。
                                let preserve =
                                    self.live_schema.revision.unavailable_reason.is_none();
                                self.invalidate_live_hints(preserve);
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
                        // 排队被拒（另一篇在途）走快重试；真故障走指数退避。
                        if r.as_ref().is_err_and(|e| e.contains("另一篇")) {
                            self.display_retry.soft_fail(now);
                        } else {
                            self.display_retry.fail(now);
                        }
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
                                dbg_hint(format!("analyze_err ({e})"));
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
                                    // 【词提防闪烁 2026-11-12 日志定案】引擎忙/
                                    // 单飞冲突是瞬时态：请求失败不得清空正在显示
                                    // 的词提（清空=用户可见闪烁），保留旧值等重试
                                    // 成功后整帧替换。words 本就对应当前段切片，
                                    // 下一轮成功响应必然覆盖。
                                    self.model.status = format!("词提请求失败 · 重试中 · {e}");
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
                Bg::ChatFocused(title, success) => {
                    if !self.model.bound.and_then(|i| self.chats.get(i)).is_some_and(|c| c.title == title) {
                        continue;
                    }
                    self.toast(if success {
                        format!("已切到「{title}」（F4 读文 / 打完自动发成绩）")
                    } else {
                        format!("已绑定「{title}」· 请在 QQ 点开该群后 F4 读文")
                    });
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
                        Ok(qq_bridge::SendStatus::Confirmed) => self.toast(label),
                        // 审计 C03：没有回执就不能报「已发送」，如实说明需人工确认。
                        Ok(qq_bridge::SendStatus::Unconfirmed) => {
                            self.toast("已粘贴并回车，但读不到QQ回执：群聊确认是否发出")
                        }
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
                Bg::Article(generation, hwnd, title, r) => {
                    self.capture_fields();
                    self.qq_busy = false;
                    if self.generation != generation || !self.model.bound.and_then(|i| self.chats.get(i))
                        .is_some_and(|c| c.hwnd == hwnd && c.title == title) {
                        continue;
                    }
                    match r {
                        Ok(t) => {
                            // F4 读到就立刻载入，不再做待读草稿二次确认。
                            self.model.fields.insert("load_text".into(), t);
                            self.load_text(false);
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
                    // 【审计 I06】保存结果在弹层里留成状态行，不再只是一次转瞬即逝的 toast。
                    match &r {
                        Ok(()) => self.model.mark_saved(unix_now()),
                        Err(e) => self.model.mark_save_failed(&e.to_string()),
                    }
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
                                ui::apply_overrides(&mut self.theme, &self.cfg.color_overrides);
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
                    "正在更新登录状态"
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
                    self.article = Some(a.clone());
                    // 新取到的文进入历史尾；从历史中间再取新文时截断前向分支。
                    self.article_history.truncate(self.article_history_pos + 1);
                    self.article_history.push(a);
                    if self.article_history.len() > 50 {
                        self.article_history.remove(0);
                    }
                    self.article_history_pos = self.article_history.len().saturating_sub(1);
                    self.article_fetch = Some(params);
                    // 整改2a：去预览——取到正文直接载入跟打区，第一段起打
                    //（成绩单期间由自动下一段触发时，载入本身就会关闭成绩单；
                    //  打字中守卫见 load_online_article 头【载文守卫 2026-11-14】）。
                    self.load_online_article(false);
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
    fn load_text(&mut self, _confirmed: bool) {
        // Explicit load commands always replace the text immediately.
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
        if self.sess.normalize_count > 0 {
            let (a, b) = self.sess.normalize_chars;
            // 【五轮】载文少字透明化：规范化（省略号收敛、空白删除/CRLF）带来的
            // 字符数变化直接写进 toast，用户一眼看出少了几个、为何少"。
            let delta = if a != b { format!("，总字数 {a}→{b}") } else { String::new() };
            self.toast(&format!("已规范 {} 处标点引号{delta}", self.sess.normalize_count));
        }
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
                self.model.menu = None;
                self.model.ledger_open = !self.model.ledger_open;
                if self.model.ledger_open {
                    let sc = gale_win::window::dpi_scale();
                    let (_, h) = self.win.client_size();
                    let lh = h as f32 / sc;
                    let cap = crate::history::scroll_max(&self.cfg, &self.model, lh);
                    self.model.ledger_scroll = self.model.ledger_scroll.clamp(0., cap);
                } else {
                    self.model.selected_history = None;
                }
                gale_win::window::request_frame();
            }
            Action::ToggleAnalysis => self.toggle_analysis_dock(),
            // ── 【视图体系 2026-11-12】 ────────────────────────────────
            Action::SetDisplayMode(mode) => {
                if self.cfg.display_mode != mode {
                    self.cfg.display_mode = mode;
                    self.anim.caret = None;
        self.anim.caret_target = None;
                    self.layout_key = (u64::MAX, 0, 0);
                    self.input_layout_key = (0, 0, 0, 0);
                    self.save_config();
                    self.toast(format!(
                        "已切换到{}模式{}",
                        mode.label(),
                        if mode.is_split() { " · 可拖动中间把手调比例" } else { "" }
                    ));
                    gale_win::window::request_frame();
                }
            }
            Action::ToggleDisplayMode => {
                let next = if self.cfg.display_mode.is_split() {
                    DisplayMode::Copybook
                } else {
                    DisplayMode::Split
                };
                self.act(Action::SetDisplayMode(next), confirmed);
            }
            Action::ToggleMinimal => {
                self.cfg.minimal = !self.cfg.minimal;
                if self.cfg.minimal {
                    self.model.ledger_open = false;
                    self.model.selected_history = None;
                    self.model.analysis_dock = false;
                }
                self.anim.caret = None;
        self.anim.caret_target = None;
                self.save_config();
                self.toast(if self.cfg.minimal {
                    "极简模式 · F11 退出 · 右键打开视图菜单"
                } else {
                    "已退出极简模式"
                });
                gale_win::window::request_frame();
            }
            Action::ToggleMinimalHud => {
                self.cfg.minimal_hud = !self.cfg.minimal_hud;
                self.save_config();
                self.toast(if self.cfg.minimal_hud {
                    "极简数据条 · 开"
                } else {
                    "极简数据条 · 关"
                });
                gale_win::window::request_frame();
            }
            Action::ToggleTopmost => {
                self.cfg.always_on_top = !self.cfg.always_on_top;
                gale_win::window::set_topmost(self.win.hwnd(), self.cfg.always_on_top);
                self.save_config();
                self.toast(if self.cfg.always_on_top {
                    "窗口已置顶"
                } else {
                    "已取消置顶"
                });
                gale_win::window::request_frame();
            }
            Action::OpenMenuAt(x, y) => {
                self.model.menu = Some(modals::CtxMenu { x, y });
                gale_win::window::request_frame();
            }
            Action::CloseMenu => {
                self.model.menu = None;
                gale_win::window::request_frame();
            }
            Action::StepFont(delta) => {
                self.cfg.font_size = (self.cfg.font_size + delta as f32).clamp(10., 100.);
                self.apply_style();
                self.font_save_due = Some(self.ms() + 800);
                gale_win::window::request_frame();
            }
            Action::StepSplit(delta) => {
                self.cfg.split_ratio =
                    (self.cfg.split_ratio + delta as f32 * 0.02).clamp(0.15, 0.97);
                self.layout_save_due = Some(self.ms() + 600);
                gale_win::window::request_frame();
            }
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
            Action::OnlineDifficultyTo(i) => self.model.online_difficulty = i.min(6),
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
                // 行距步进范围与渲染口径 (0.10..=3.00)、配置解析 clamp(-0.90, 2.0) 对齐。
                // 负行距，行高低于字号（紧凑排版，2026-11-13 四轮用户要求最低 0.1）。
                self.cfg.line_gap = (self.cfg.line_gap + delta as f32 * 0.02).clamp(-0.90, 2.0);
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
                // 【审计 I05】恢复默认外观必须把字间距一起还原，否则旧的自定义字距
                // 会在「已恢复默认」之后继续生效，与按钮文案不符。
                self.cfg.letter_spacing = d.letter_spacing;
                self.cfg.theme = "墨玉".into();
                self.cfg
                    .color_overrides
                    .retain(|key, _| key.starts_with("词提"));
                self.cfg.smooth_caret = true;
                self.cfg.shake_on_wrong = true;
                self.model.reduced_motion = gale_win::edit::reduced_motion();
                self.theme = ui::theme_with_overrides("墨玉", &self.cfg.color_overrides);
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
            Action::RoundStuckHint => {
                // 【末字 2026-11-14】打完但末字有误：phase 永留 Typing 是结算
                // 规则（末字必须 Correct），这里把「为什么 Enter 不跳段」说破。
                let c = &self.sess.comparer;
                if c.is_done() {
                    self.toast("末字有误：改对末字（或退格重打）后自动进入下一段");
                } else {
                    let remaining = c.states.len().saturating_sub(c.pos);
                    self.toast(format!(
                        "还剩 {remaining} 字未打 · Enter 在打完后才跳段（F3 重打）"
                    ));
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
                    // 【减法 2026-11-09】载文窗退役：剪贴板内容直接开打；
                    // 打字中的丢弃守卫统一在 load_text 里（【载文守卫 2026-11-14】）。
                    self.model.fields.insert("load_text".into(), t);
                    self.load_text(false);
                }
                None => self.toast("剪贴板为空或不可用"),
            },
            Action::ProceedLoad => self.load_text(true),
            Action::Theme(n) => {
                self.theme_generation += 1;
                self.cfg.theme = n.clone();
                if n == "跟随虎符" {
                    self.fetch_skin();
                } else {
                    self.theme = ui::theme_with_overrides(&n, &self.cfg.color_overrides);
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
                            "[跟打]{} [{}字]\n{}\n-----第{}段 · 晴发文-----",
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
                    let tx = self.tx.clone();
                    let hwnd = self.win.hwnd;
                    let job = Box::new(move || {
                        let _ = tx.send(Bg::Chats(qq_bridge::list_sessions_merged()));
                        wake(hwnd);
                    });
                    let result = self.qq_worker.as_ref().map_err(Clone::clone)
                        .and_then(|worker| worker.submit(job));
                    if let Err(error) = result {
                        self.qq_busy = false;
                        self.toast(error);
                    }
                    gale_win::window::request_frame();
                }
            }
            Action::SelectGroup(i) => {
                let epoch = self.qq_focus_epoch.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
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
                    self.toast(format!("已绑定「{}」", chat.title));
                    let tx = self.tx.clone();
                    let hwnd = self.win.hwnd;
                    let focus_epoch = self.qq_focus_epoch.clone();
                    let job = Box::new(move || {
                        if focus_epoch.load(std::sync::atomic::Ordering::Relaxed) != epoch {
                            return;
                        }
                        let success = qq_bridge::focus_chat(&chat);
                        let _ = tx.send(Bg::ChatFocused(chat.title, success));
                        wake(hwnd);
                    });
                    if let Err(error) = self.qq_worker.as_ref().map_err(Clone::clone)
                        .and_then(|worker| worker.submit(job)) {
                        self.toast(error);
                    }
                }
            }
            Action::ReadGroup => {
                if self.qq_busy {
                    self.toast("QQ 正在读取，请稍候");
                } else if let Some(c) = self.model.bound.and_then(|i| self.chats.get(i)).cloned() {
                    self.qq_busy = true;
                    let tx = self.tx.clone();
                    let hwnd = self.win.hwnd;
                    let generation = self.generation;
                    let job = Box::new(move || {
                        let result = uia_bridge::read_current_article(c.hwnd, &c.title);
                        let _ = tx.send(Bg::Article(generation, c.hwnd, c.title, result));
                        wake(hwnd);
                    });
                    if let Err(error) = self.qq_worker.as_ref().map_err(Clone::clone)
                        .and_then(|worker| worker.submit(job)) {
                        self.qq_busy = false;
                        self.toast(error);
                    }
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
                self.model.online_difficulty = (self.model.online_difficulty + 1) % 7
            }
            Action::OnlineStrict => self.model.online_strict = !self.model.online_strict,
            Action::OnlineBooks => self.submit_online(OnlineCommand::Bookshelf),
            Action::OnlineFetch(dir) => {
                // 上一段/下一段先走历史：上一段回到刚才看的那篇，
                // 历史到头或明确要随机时才去全站取新文。
                if dir < 0 {
                    let idx = self.article_history_pos.saturating_sub(1);
                    if self.article_history_pos > 0 && self.show_history_article(idx) {
                        return;
                    }
                } else if dir > 0 {
                    let idx = self.article_history_pos + 1;
                    if idx < self.article_history.len() && self.show_history_article(idx) {
                        return;
                    }
                }
                let chars = match self.model.field("online_chars").trim().parse::<usize>() {
                    Ok(n) if (1..=100000).contains(&n) => n,
                    _ => {
                        self.toast("目标字数请输入1至100000之间的整数");
                        return;
                    }
                };
                // 2026-11-14 用户拍板：上一段/下一段不再绑定当前小说，
                // 一律从全站随机取文；难度选「随机」时每次抽一级。
                let p = FetchParams {
                    chars,
                    strict_length: self.model.online_strict,
                    difficulty: self.online_fetch_difficulty(),
                    category: self
                        .model
                        .online_categories
                        .get(self.model.online_category)
                        .map(|(id, _)| id.clone())
                        .unwrap_or_default(),
                    ..FetchParams::default()
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
                        difficulty: self.online_fetch_difficulty(),
                        ..FetchParams::default()
                    }));
                }
            }
            Action::OnlineLoad => self.load_online_article(confirmed),
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
                        self.model.mark_saved(unix_now());
                        self.toast("规则已保存");
                    }
                    Err(e) => {
                        self.model.mark_save_failed(&e.clone());
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
mod overlay_placement_tests {
    use super::*;
    use windows::Win32::{
        Foundation::{POINT, RECT},
        Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    };

    struct HiddenMain(HWND);
    impl Drop for HiddenMain {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }

    #[test]
    fn cached_native_page_stays_inside_on_first_frame_and_reopen_until_actually_dragged() {
        let _com = gale_win::window::ComSta::new().unwrap();
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        assert!(unsafe {
            GetMonitorInfoW(MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTONEAREST), &mut info)
        }.as_bool());
        let work = info.rcWork;
        // 真实隐藏主窗靠近工作区底边，理想居中的高设置页会有半窗越出屏幕。
        // 不显示/聚焦任何窗口，避免打断正在运行的跟打器验收。
        let main = HiddenMain(unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0), windows::core::w!("STATIC"), windows::core::w!("placement test"),
                WS_POPUP, work.left + 100, work.bottom - 400, 1280, 800,
                None, None, None, None,
            ).unwrap()
        });
        let scale = gale_win::window::refresh_window_dpi(main.0);
        let (w, h) = (1280. / scale, 800. / scale);
        let mut model = Model::default();
        model.open(Modal::Preferences);
        let mut placement = OverlayPagePlacement::default();
        let (x, y, width, height) = placement.fit(main.0.0 as isize, &model, w, h);
        let mut native_page = overlay::Overlay::create(main.0.0 as isize, x, y, width, height, scale).unwrap();
        let native_rect = |page: &overlay::Overlay| {
            let mut rect = RECT::default();
            unsafe { GetWindowRect(page.hwnd(), &mut rect).unwrap(); }
            rect
        };
        let assert_inside = |rect: RECT| {
            assert!(rect.left >= work.left && rect.top >= work.top, "{},{}", rect.left, rect.top);
            assert!(rect.right <= work.right && rect.bottom <= work.bottom,
                "首次/重开页脚必须留在工作区：bottom={} work={}", rect.bottom, work.bottom);
        };
        assert_inside(native_rect(&native_page));
        let (base_x, base_y, _, _) = modals::panel_base(&model, w, h);
        model.modal_shift = (x + PAGE_MARGIN_X - base_x, y + PAGE_MARGIN_Y - base_y);
        assert_ne!(model.modal_shift, (0., 0.), "首次紧钳本身会产生非零偏移");
        placement.note_drag(0., 0.);
        // 重现 create 后立即 set_rect、后续空闲重绘：都必须保持 tight。
        for _ in 0..3 {
            let (x, y, width, height) = placement.fit(main.0.0 as isize, &model, w, h);
            native_page.set_rect(x, y, width, height, scale).unwrap();
            assert_inside(native_rect(&native_page));
        }
        // 只有实际拖动才放宽，允许推出屏幕；页签刷新不抹掉用户拖动位置。
        let dx = (work.right - work.left) as f32 / scale;
        placement.note_drag(dx, 0.);
        model.modal_shift.0 += dx;
        model.modal_epoch += 1;
        let (x, y, width, height) = placement.fit(main.0.0 as isize, &model, w, h);
        native_page.set_rect(x, y, width, height, scale).unwrap();
        let moved = native_rect(&native_page);
        assert!(moved.right > work.right);
        assert!(moved.left < work.right, "推出屏幕后仍须留出可抓回的窗口带");
        // 同一个缓存 HWND 关闭后重开，恢复 tight，不继承上次出屏权限。
        let cached_hwnd = native_page.hwnd();
        native_page.show(false);
        model.open(Modal::Preferences);
        placement.reset();
        let (x, y, width, height) = placement.fit(main.0.0 as isize, &model, w, h);
        native_page.set_rect(x, y, width, height, scale).unwrap();
        assert_eq!(native_page.hwnd(), cached_hwnd);
        assert_inside(native_rect(&native_page));
    }
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
        // 【词提防闪烁 2026-11-12】词库可用（preserve=true）时保留词提与
        // F10 旧视图，仅清错误字段；不可用（preserve=false）才硬清空。
        retire_hint_views(&mut model, &mut sess, true);
        assert_eq!(model.words.len(), 1);
        assert!(sess.full_analysis.is_some());
        assert!(model.field("analysis_error").is_empty());
        retire_hint_views(&mut model, &mut sess, false);
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
        sess.load_book("动画", "“甲😀乙”");
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
            segment_analysis_breaks(&segments(&["", "𠮷甲", "", "乙"])),
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
            escape_action(Modal::Preferences, true, true, false),
            Some(Action::Close)
        );
        // 所有二级页一视同仁：哪怕成绩面板/菜单也开着，Esc 先关当前页。
        for m in [
            Modal::Groups,
            Modal::Online,
            Modal::Rules,
            Modal::Confirm,
            Modal::Heatmap,
            Modal::Analysis,
            Modal::More,
        ] {
            assert_eq!(
                escape_action(m, true, true, true),
                Some(Action::Close),
                "{m:?}"
            );
        }
        assert_eq!(
            escape_action(Modal::None, true, true, false),
            Some(Action::ToggleLedger)
        );
        assert_eq!(
            escape_action(Modal::None, false, true, false),
            Some(Action::ToggleAnalysis)
        );
        // 视图菜单打开时 Esc 先收菜单（不关面板、不动计时）。
        assert_eq!(
            escape_action(Modal::None, false, false, true),
            Some(Action::CloseMenu)
        );
        assert_eq!(
            escape_action(Modal::None, true, true, true),
            Some(Action::CloseMenu)
        );
        let mut sess = Session::new(100);
        sess.load_book("Esc", "甲乙");
        dispatch_host_events(
            &mut sess,
            vec![HostEvent::Key { vk: 0x41 }, HostEvent::Commit("甲".into())],
            true,
            100,
        );
        assert_eq!(escape_action(Modal::None, false, false, false), None);
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

    /// 【超文长 2026-11-14】文章打满后继续提交：comparer 静默忽略，
    /// DispatchedInput.overlong_chars 必须记数（驱动 shake 反馈）。
    #[test]
    fn overlong_commit_after_text_full_is_counted_for_feedback() {
        let mut sess = Session::new(100);
        sess.load_book("超长", "甲乙");
        let d = dispatch_host_events(
            &mut sess,
            vec![HostEvent::Key { vk: 0x41 }, HostEvent::Commit("甲乙".into())],
            true,
            100,
        );
        assert_eq!(d.typed_chars, 2);
        assert_eq!(d.overlong_chars, 0, "恰好打满不计数");
        assert_eq!(sess.phase, Phase::Done);
        // 打完继续打：整批丢弃但必须计数（live 条件=未暂停且有文章）。
        let d2 = dispatch_host_events(
            &mut sess,
            vec![HostEvent::Commit("丙丁".into())],
            true,
            200,
        );
        assert_eq!(d2.typed_chars, 0);
        assert_eq!(d2.overlong_chars, 2, "Done 后的超文长提交要计数");
        // 打字中途一批含超长尾巴：消费 2 计 2。
        let mut s2 = Session::new(100);
        s2.load_book("混合", "甲乙丙丁");
        let d3 = dispatch_host_events(
            &mut s2,
            vec![
                HostEvent::Key { vk: 0x41 },
                HostEvent::Commit("甲乙丙丁戊".into()),
            ],
            true,
            100,
        );
        assert_eq!(d3.typed_chars, 4);
        assert_eq!(d3.overlong_chars, 1, "只计被忽略的超出部分");
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
            vec![HostEvent::Commit("甲".into()), HostEvent::Backspace],
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
        assert_eq!(escape_action(Modal::None, false, false, false), None);
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
            vec![HostEvent::Key { vk: 0x42 }, HostEvent::Commit("甲".into())],
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
        dispatch_host_events(&mut sess, vec![HostEvent::Commit("乙".into())], true, 1000);
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
            dispatch_host_events(&mut sess, vec![HostEvent::Commit("甲".into())], true, 1200);
        delayed.apply_commit_causes(&mut sess, vec![CommitCause::Select]);
        assert_eq!(sess.typed_len(), 1);
        assert_eq!(
            sess.stats.select_count, 1,
            "a rejected commit cannot change statistics"
        );
        let resumed = dispatch_host_events(
            &mut sess,
            vec![HostEvent::Key { vk: 0x42 }, HostEvent::Commit("甲".into())],
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
        session.load_book("可恢复文本", "甲乙丙丁");
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

    #[test]
    fn online_difficulty_random_cycle_covers_all_six_levels() {
        // 固定档：0..=5 逐一映射到六级。
        for level in 0..6 {
            let got = online_difficulty_for(level, 0);
            assert_eq!(got, Difficulty::from_level(level as i64 + 1).unwrap());
        }
        // 随机档：同一 level=6 按 seed 遍历六级，绝不回落成固定某一级。
        let levels: std::collections::BTreeSet<_> = (0..6)
            .map(|seed| online_difficulty_for(6, seed).level())
            .collect();
        assert_eq!(levels.len(), 6);
        // 越界/负数不 panic，退回固定档。
        assert!(online_difficulty_for(-5, 0) == Difficulty::from_level(1).unwrap());
        assert!(online_difficulty_for(9, 5) == Difficulty::from_level(6).unwrap());
    }
}

/// 【审计 I06】保存状态行要显示的时间戳（秒）。
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
