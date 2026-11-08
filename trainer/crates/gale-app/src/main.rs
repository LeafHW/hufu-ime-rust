//! 虎符跟打：真实 RichEdit/TSF 输入，原生 D2D 界面，后台联网。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod modals;
mod online;
mod qq_bridge;
mod secondary_panels;
mod session;
mod skin;
mod ui;
mod uia_bridge;
use gale_core::{
    config::Config,
    library::Library,
    rules::{CleaningPreset, CleaningRule, TrainingMode},
    score::{format_score_report, ScoreStore},
    segment::parse_group_text,
    single::SingleDrillConfig,
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
    collections::{BTreeMap, HashMap, VecDeque},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, VK_CONTROL};
use windows::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, GetLastError, BOOL, HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::*,
};

/// 整改13：server `/api/trainer/analyze?include_full=true` 返回的全篇理论
/// 码长分析（`full_analysis` 字段）。字段名与 server 端严格一致——视觉代理
/// 的分析页模态按此消费；任务14（词提全文显示）同源复用 entries。
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct FullAnalysis {
    #[serde(default)]
    pub entries: Vec<FullAnalysisEntry>,
    #[serde(default)]
    pub total_code_len: u64,
    #[serde(default)]
    pub avg_code_len_per_char: f64,
    #[serde(default)]
    pub select_count: u64,
}

/// 分析词条：word=词面、code=编码（生词无码时 None）、code_len=码长、rank=词频名次
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct FullAnalysisEntry {
    #[serde(default)]
    pub word: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub code_len: u64,
    #[serde(default)]
    pub rank: u64,
}

/// 整改12：热力图 VK 口径（任务指定区段）。wParam 低 16 位里的修饰/扫描码
/// 高位标志已由 `& 0xFF` 剥离；小键盘（0x60-0x6F）、Tab、OEM ` 与重映射
/// 0xE5 不计（比 stats 的 is_stat_key 窄一档，与热力图需求对齐）。
fn heatmap_vk(vk: u32) -> Option<u8> {
    match vk & 0xFF {
        0x08 | 0x0D | 0x20 => Some(vk as u8),
        0x30..=0x39 | 0x41..=0x5A => Some(vk as u8),
        0xBA..=0xBF | 0xDB..=0xDE => Some(vk as u8),
        _ => None,
    }
}

/// 私有标记通道（整改2/5/8/13）：modals.rs（禁改域）暂无对应 Action 变体，
/// 借用 `Action::Theme(String)` 载荷携带 `\u{e000}` 前缀命令。真实主题名
/// 永不含 U+E000，不会与外观页主题切换冲突；modals.rs 落正式变体（如
/// Action::OnlineRegister / ShowFullAnalysis / SingleShuffle / SingleSegStep）
/// 后，把发射点换绑即可，main.rs 这边的解码可原样退役。
const AUX_PREFIX: char = '\u{e000}';
const AUX_REGISTER: &str = "online_register";
const AUX_AUTO_NEXT: &str = "online_auto_next";
const AUX_SINGLE_SHUFFLE: &str = "single_shuffle";
const AUX_SINGLE_SEG_PLUS: &str = "single_seg+5";
const AUX_SINGLE_SEG_MINUS: &str = "single_seg-5";
const AUX_SHOW_FULL: &str = "show_full_analysis";
const AUX_SHOW_HEATMAP: &str = "show_heatmap";

enum AuxAction {
    OnlineRegister,
    OnlineAutoNext,
    SingleShuffle,
    SingleSeg(i32),
    ShowFullAnalysis,
    ShowHeatmap,
}

fn aux_action(a: &Action) -> Option<AuxAction> {
    let Action::Theme(s) = a else {
        return None;
    };
    let cmd = s.strip_prefix(AUX_PREFIX)?;
    Some(match cmd {
        AUX_REGISTER => AuxAction::OnlineRegister,
        AUX_AUTO_NEXT => AuxAction::OnlineAutoNext,
        AUX_SINGLE_SHUFFLE => AuxAction::SingleShuffle,
        AUX_SINGLE_SEG_PLUS => AuxAction::SingleSeg(5),
        AUX_SINGLE_SEG_MINUS => AuxAction::SingleSeg(-5),
        AUX_SHOW_FULL => AuxAction::ShowFullAnalysis,
        AUX_SHOW_HEATMAP => AuxAction::ShowHeatmap,
        _ => return None,
    })
}

pub(crate) fn aux_marker(cmd: &str) -> Action {
    Action::Theme(format!("{AUX_PREFIX}{cmd}"))
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
    if done
        && matches!(modal, Modal::None | Modal::Result)
        && matches!(command, CommandKey::Enter | CommandKey::Space)
    {
        Some(Action::Next)
    } else {
        None
    }
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
        let handle = match CreateMutexW(
            None,
            true,
            windows::core::w!("HuFu-Trainer-SingleInstance"),
        ) {
            Ok(h) => h,
            Err(_) => return false, // 创建失败不拦启动（宁可双开也不打不开）
        };
        // 判据：CreateMutexW 后 GetLastError==ERROR_ALREADY_EXISTS 且探测未接管
        //（WAIT_ABANDONED = 旧实例持锁僵死，本实例接管放行——与 window.rs 同解）。
        let wait: WAIT_EVENT = WaitForSingleObject(handle, 0);
        let already = GetLastError() == ERROR_ALREADY_EXISTS
            && wait.0 != 0
            && wait.0 != 128; /* WAIT_ABANDONED */
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
        if let Ok(hwnd) = FindWindowExW(None, None, windows::core::w!("HuFuTrainerWindow"), None)
        {
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
    Analyze(u64, Result<serde_json::Value, String>),
    Chats(Vec<qq_bridge::ChatWindow>),
    Article(u64, Option<usize>, u64, Result<String, String>),
    File(u64, u64, Result<(String, String), String>),
    Saved(Result<(), String>),
    ScoreSaved(Result<(), String>),
    Skin(u64, Result<gale_core::theme::Theme, String>),
    RuleTest(u64, Result<gale_core::rules::CleaningResult, String>),
    RuleApply(u64, u64, Result<Session, String>),
    /// 整改13：全篇理论分析（None=旧版服务无 full_analysis 字段，静默降级）
    FullAnalysis(u64, Option<FullAnalysis>),
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
    library: Library,
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
    draft_revision: u64,
    pending_draft: Option<(String, String)>,
    edit_epoch: u64,
    hits: Vec<modals::Hit>,
    anim: ui::Animation,
    clock: Instant,
    online: Option<OnlineWorker>,
    article: Option<WenlaiArticle>,
    shelf: Vec<gale_core::wenlai::ShelfBook>,
    chats: Vec<qq_bridge::ChatWindow>,
    tx: mpsc::Sender<Bg>,
    rx: mpsc::Receiver<Bg>,
    generation: u64,
    analyzing: bool,
    analyzed: u64,
    /// 整改14：整篇文章的词提（全文坐标，一次分析缓存；换段本地切片，
    /// 不再打到哪段才请求哪段）。full_words_ver=生成它的 schema 版本；
    /// full_words_of=生成时的全文快照（文本变化自动失效缓存）。
    full_words: Vec<modals::HintWord>,
    full_words_ver: String,
    full_words_of: String,
    /// 本次 analyze 是否按整篇请求（false=超长回退单段，响应为段内坐标）。
    last_analyze_whole: bool,
    theme_generation: u64,
    rule_generation: u64,
    rule_busy: bool,
    qq_busy: bool,
    exit: bool,
    online_pending: usize,
    online_logged_in: bool,
    /// P2：CTF 输入通道自检失败（启动时探测一次）——状态栏持续警示标记。
    tsf_broken: bool,
    /// 整改2b：自动下一段开关（online.json）+ 结算后自动取文的到期时刻
    online_auto_next: bool,
    auto_next_at: Option<u64>,
    /// 整改2a：当前会话正文是否来自在线文（自动下一段的判据）
    online_article_current: bool,
    /// 整改6：Ctrl+滚轮字号防抖落盘时刻（800ms）
    font_save_due: Option<u64>,
    /// 整改7：QQ 会话列表上次扫描时刻（屏开期间每 10s 刷新）
    qq_scan_at: u64,
    /// 整改8：单字练习配置（single.json：乱序 + 段长）
    single_cfg: SingleDrillConfig,
    /// 整改13：已请求过全篇分析的文章指纹（同文重打不重复请求）
    full_fetch_hash: Option<String>,
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
    if single_instance_guard() {
        return Ok(());
    }
    if gale_win::window::activate_existing_instance().is_some() {
        return Ok(());
    }
    let _com =
        gale_win::window::ComSta::new().map_err(|e| format!("无法初始化STA/TSF输入环境：{e}"))?;
    gale_win::window::enable_per_monitor_dpi_awareness();
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut cfg = Config::load_or_create(&dir.join("config.toml")).map_err(|e| e.to_string())?;
    // Native child editors require the opaque redirection surface in this build.
    cfg.material = gale_core::config::Material::Solid;
    let sc = gale_win::window::dpi_scale();
    let work = unsafe {
        let mut r = windows::Win32::Foundation::RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut r as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        r
    };
    let ww = ((cfg.win_w.max(800) as f32 * sc) as i32).min((work.right - work.left - 20).max(600));
    let hh = ((cfg.win_h.max(600) as f32 * sc) as i32).min((work.bottom - work.top - 20).max(440));
    let win = MainWindow::create("虎符跟打 · 静室", ww, hh).map_err(|e| e.to_string())?;
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
    if sess.current_segment().text.is_empty() {
        sess.load_book("欢迎来到静室","天青色等烟雨，而我在等你。\n文字落下，心也安静下来。\n按 F2 载入文字，按 F7 选择外观。按下第一个键开始计时。");
    }
    if sess.frozen_rules.is_none() {
        let _ = sess.set_rules(cfg.session_rules.clone());
    }
    let library = Library::open(&dir.join("文库")).map_err(|e| e.to_string())?;
    if let Some(path) = cli_value("--article") {
        let (name, text) = read_article_file(&path)?;
        sess.load_book(&name, &text);
    }
    let mut model = Model::default();
    model.rule_save_preset = true;
    model.hints = sess.rules.word_hints;
    model.race = sess.rules.mode == TrainingMode::Race;
    model.quote_relaxed = sess.rules.quotes.enabled;
    model.reduced_motion = gale_win::edit::reduced_motion() || cfg.motion_level == 0;
    model.status = "按 F2 载文 · F3 重打 · F5 赛报".into();
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
    model.books = library.books().iter().map(|b| b.name.clone()).collect();
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
        library,
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
        draft_revision: 0,
        pending_draft: None,
        edit_epoch: u64::MAX,
        hits: Vec::new(),
        anim: ui::Animation::default(),
        clock: Instant::now(),
        online,
        article: None,
        shelf: Vec::new(),
        chats: Vec::new(),
        tx,
        rx,
        generation: 1,
        analyzing: false,
        analyzed: 0,
        full_words: Vec::new(),
        full_words_ver: String::new(),
        full_words_of: String::new(),
        last_analyze_whole: false,
        theme_generation: 0,
        rule_generation: 0,
        rule_busy: false,
        qq_busy: false,
        exit: false,
        online_pending: 0,
        online_logged_in: false,
        tsf_broken: !host_ctf_ok,
        online_auto_next: OnlinePrefs::load(&dir.join("online.json")).auto_next,
        auto_next_at: None,
        online_article_current: false,
        font_save_due: None,
        qq_scan_at: 0,
        single_cfg: SingleDrillConfig::load(&dir.join("single.json")),
        full_fetch_hash: None,
    };
    app.model
        .fields
        .insert("online_auto_next".into(), app.online_auto_next.to_string());
    app.sync_single_fields();
    app.sync_rule_model();
    if app.cfg.theme == "跟随虎符" {
        app.fetch_skin();
    }
    app.apply_style();
    app.host.show();
    app.host.focus();
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
    let mut status_at = 0u64;
    let mut was_focused = true;
    while !app.exit {
        if !gale_win::window::pump_messages_once() {
            break;
        }
        let now = app.ms();
        let seconds = app.clock.elapsed().as_secs_f64();
        if app.capture_fields() {
            gale_win::window::request_frame();
        }
        app.collect_background();
        app.collect_online();
        app.pump_persistence();
        let focused = unsafe { GetForegroundWindow() == app.win.hwnd() };
        if focused && !was_focused && app.sess.phase == Phase::Idle {
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
        if accept_host {
            // P1-6：Composing(false) 与 Commit 同批 = 提交清空（不置 protect）；
            // 只有 Composing(false) = 退格删空/Esc 取消（置 protect）。
            let batch_has_commit = events
                .iter()
                .any(|e| matches!(e, HostEvent::Commit(_)));
            for ev in events {
                match ev {
                    HostEvent::Key { vk } => app.sess.on_key(vk, now),
                    HostEvent::Commit(s) => app
                        .sess
                        .feed_input(gale_core::compare::InputEvent::Commit { chars: s }, now),
                    HostEvent::Backspace => app
                        .sess
                        .feed_input(gale_core::compare::InputEvent::Backspace, now),
                    HostEvent::Composing(c) => {
                        app.sess.set_composing_from_host(c, batch_has_commit)
                    }
                    HostEvent::Preedit(p) => app.sess.set_preedit(p),
                    HostEvent::Cancelled => app.sess.cancel_composition(),
                }
            }
        }
        // P1-3 选重/标顶（任务 2）：每帧 poll 之后立即 drain 20ms 配对结果
        //（input_host 顺序契约：必须在 poll 之后），Ding→标顶/Select→选重/
        // Unknown 忽略；模态打开时只清队列不记账。
        if accept_host {
            for cause in gale_win::input_host::drain_commit_causes() {
                app.sess.on_commit_cause(cause);
            }
        } else {
            let _ = gale_win::input_host::drain_commit_causes();
        }
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
        let paused = (!focused || app.model.modal != Modal::None) && !app.model.race;
        app.sess.set_paused(paused, now);
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
        for cmd in gale_win::input_host::drain_cmds() {
            let a = match cmd {
                CommandKey::Escape => {
                    if app.model.modal != Modal::None {
                        Some(Action::Close)
                    } else {
                        app.sess.stop_at(now);
                        None
                    }
                }
                CommandKey::F1 | CommandKey::F7 | CommandKey::F10 => {
                    Some(Action::Open(Modal::Appearance))
                }
                CommandKey::F2 => Some(Action::Open(Modal::Load)),
                CommandKey::F3 => Some(Action::Restart),
                CommandKey::F4 => Some(Action::ReadGroup),
                CommandKey::F5 => Some(Action::Open(Modal::Report)),
                CommandKey::F6 => Some(Action::Open(Modal::Groups)),
                CommandKey::F8 => Some(Action::Open(Modal::Online)),
                CommandKey::F9 => Some(Action::Open(Modal::Rules)),
                CommandKey::LoadClipboard | CommandKey::AltE => Some(Action::LoadClipboard),
                CommandKey::CopyResult => Some(if app.model.modal == Modal::Report {
                    Action::CopyRace
                } else {
                    Action::CopyScore
                }),
                CommandKey::SendResult => Some(if app.model.modal == Modal::Report {
                    Action::CopyRace
                } else {
                    Action::SendScore
                }),
                CommandKey::ExportResult => Some(Action::ExportImage),
                CommandKey::RetryResult => {
                    (app.model.modal == Modal::Result).then_some(Action::Restart)
                }
                CommandKey::CtrlLeft => Some(Action::Previous),
                CommandKey::CtrlRight => Some(Action::Next),
                CommandKey::Enter => {
                    if app.model.modal == Modal::Load {
                        Some(Action::LoadText)
                    } else {
                        completed_segment_action(
                            cmd,
                            app.sess.phase == Phase::Done,
                            app.model.modal,
                        )
                    }
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
                app.pending_scores.push_back((id, record));
                app.open(Modal::Result);
                app.snapshot();
                // 整改2b：在线练习（正文来自文来）结算后 1.5s 自动取下一段；
                // 赛制（竞赛段）结算不自动跳，等发文确认。
                if app.online_auto_next
                    && app.online_logged_in
                    && app.online_article_current
                    && app.article.is_some()
                    && app.sess.rules.mode != TrainingMode::Race
                {
                    app.auto_next_at = Some(now + 1500);
                }
            }
        }
        // 整改2b：自动下一段到期触发。仍在成绩单上（用户未接管：未回车进赛报/
        // 未开其他面板）才发相邻段请求；取回的正文经 collect_online 自动载入。
        if let Some(at) = app.auto_next_at {
            if now >= at {
                app.auto_next_at = None;
                if app.online_auto_next
                    && app.online_logged_in
                    && app.online_article_current
                    && app.article.is_some()
                    && app.model.modal == Modal::Result
                    && app.sess.phase == Phase::Done
                    && app.sess.rules.mode != TrainingMode::Race
                {
                    app.act(Action::OnlineFetch(1), false);
                }
            }
        }
        // 整改7：QQ 读取屏（F6 群屏 / F2 第三 tab）打开期间每 10s 刷新会话列表。
        let qq_screen = app.model.modal == Modal::Groups
            || (app.model.modal == Modal::Load && app.model.load_tab == 2);
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
            (app.model.modal == Modal::Report
                || (app.model.modal == Modal::Result && app.sess.phase == Phase::Done))
                .then_some(app.win.hwnd),
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
                        app.host.focus();
                    }
                }
                MouseEv::Wheel { delta, .. } => {
                    // 整改6：Ctrl+滚轮 = 实时字号（10..=60，滚轮向上放大），
                    // 不滚面板；800ms 防抖后统一落盘（见主循环 font_save_due）。
                    if unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0 {
                        let step = (delta / 120).clamp(-1, 1);
                        if step != 0 {
                            app.cfg.font_size =
                                (app.cfg.font_size + step as f32).clamp(10., 60.);
                            app.apply_style();
                            app.font_save_due = Some(app.ms() + 800);
                            gale_win::window::request_frame();
                        }
                        continue;
                    }
                    let (_, _, _, ph) = modals::panel(
                        app.model.modal,
                        0.,
                        app.win.client_size().1 as f32 / gale_win::window::dpi_scale(),
                    );
                    let max = if matches!(app.model.modal, Modal::Result | Modal::Report) {
                        (650. - ph).max(0.)
                    } else {
                        (ph - app.win.client_size().1 as f32 / gale_win::window::dpi_scale() + 48.)
                            .max(0.)
                    };
                    let has_list = matches!(
                        app.model.modal,
                        Modal::Rules | Modal::Groups | Modal::Report
                    ) || (app.model.modal == Modal::Load && app.model.load_tab == 1);
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
                            Modal::Groups => app.model.groups.len().saturating_sub(5),
                            Modal::Load => app.model.books.len().saturating_sub(5),
                            Modal::Rules => app.model.rules.len().saturating_sub(6),
                            Modal::Report => app.sess.total_segments().saturating_sub(6),
                            _ => 0,
                        };
                        app.model.scroll =
                            (app.model.scroll - (delta as f32 / 120.) * 2.).clamp(0., cap as f32);
                    }
                    gale_win::window::request_frame();
                }
                _ => {}
            }
        }
        let size = app.win.client_size();
        if size != last_size {
            last_size = size;
            gale_win::window::request_frame();
        }
        if app.sess.phase == Phase::Idle && now - status_at >= 5000 {
            status_at = now;
            app.analyzed = 0;
            app.analyze();
        }
        if app.sess.phase == Phase::Typing && now - status_at >= 250 {
            status_at = now;
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
            || app.analyzing
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
            gale_win::window::wait_idle(if need_timer { Some(50) } else { Some(500) });
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
        self.cfg.session_rules = self.sess.rules.clone();
        let mut saved = self.cfg.clone();
        if self.model.rule_save_preset {
            self.saved_cleaning = self.cfg.cleaning_preset.clone();
        } else {
            saved.cleaning_preset = self.saved_cleaning.clone();
        }
        if let Err(e) = saved.save(&self.dir.join("config.toml")) {
            self.toast(format!("配置保存失败：{e}"));
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
        self.generation = self.generation.wrapping_add(1);
        self.analyze();
        self.fetch_full_analysis();
        gale_win::window::request_frame();
    }
    /// 整改2a：在线文章直达跟打区（预览流程退役的唯一载入口）。
    /// 打字中（规则已冻结）或成绩积压时降级为待读草稿，不抢正文。
    fn load_online_article(&mut self) {
        let Some(a) = self.article.clone() else {
            self.toast("请先取文");
            return;
        };
        if a.text.chars().count() > 100000 {
            self.toast("在线正文超过100000字，请重新取较短文段");
            return;
        }
        if self.sess.frozen_rules.is_some() || self.pending_scores.len() >= 32 {
            self.defer_draft("在线文章".into(), a.text);
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
        self.reset_host();
        self.open(Modal::None);
        self.snapshot();
    }
    /// 整改8：单字配置同步到 model.fields（modals.rs 侧控件读这里显示状态）
    fn sync_single_fields(&mut self) {
        self.model
            .fields
            .insert("single_shuffle".into(), self.single_cfg.shuffle.to_string());
        self.model
            .fields
            .insert("single_seg".into(), self.single_cfg.segment_chars.to_string());
    }
    /// 整改13：文章载入后（在线/本地全文均可）后台请求全篇理论分析。
    /// 同一文章（指纹一致）不重复请求；server 旧版无 full_analysis 字段=
    /// 无此功能，静默降级（Bg::FullAnalysis(None) 丢弃）。
    fn fetch_full_analysis(&mut self) {
        let full: String = self
            .sess
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if full.trim().is_empty() {
            return;
        }
        let hash = gale_core::rules::fingerprint(full.as_bytes());
        if self.full_fetch_hash.as_deref() == Some(&hash) {
            return;
        }
        self.full_fetch_hash = Some(hash);
        let g = self.generation;
        let single = self.sess.source == SessionSource::Single;
        let tx = self.tx.clone();
        let hwnd = self.win.hwnd;
        std::thread::spawn(move || {
            let body = serde_json::json!({ "text": full, "single": single });
            // 失败静默：local_json 报错或旧版服务无 full_analysis 字段都归 None
            let v = local_json(local_port(), "/api/trainer/analyze?include_full=true", Some(body))
                .ok()
                .and_then(|v| v.get("full_analysis").cloned());
            let parsed = v
                .filter(|v| !v.is_null())
                .and_then(|v| serde_json::from_value::<FullAnalysis>(v).ok())
                .filter(|f| !f.entries.is_empty());
            let _ = tx.send(Bg::FullAnalysis(g, parsed));
            let _ = wake(hwnd);
        });
    }
    /// 整改13：把 full_analysis 移交给展示模态的触发点。modals.rs 尚无
    /// Modal::FullAnalysis 变体（禁改域，待视觉代理/Y2 添加），当前以提示
    /// 收尾；变体就位后此函数改为 self.open(Modal::FullAnalysis) 即接通。
    fn show_full_analysis(&mut self) {
        match &self.sess.full_analysis {
            Some(f) if !f.entries.is_empty() => {
                // 整改13收口：分析页模态（Y2 域已就绪）直接打开，不再只提示。
                self.model.modal = Modal::FullAnalysis;
                self.model.modal_epoch += 1;
                let _ = f;
            }
            Some(_) => self.toast("理论分析无词条"),
            None => self.toast("当前服务未提供全文理论分析（需新版虎符服务）"),
        }
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

    /// 整改9收口：词提四色循环选色。白名单外键拒绝（防伪造 aux 串）；
    /// 当前色不在色板内（主题派生色）则跳到首色。即时生效+落盘。
    fn cycle_hint_color(&mut self, key: &str) -> bool {
        const SWATCH: [&str; 12] = [
            "e7b75c", "63c08d", "7fb0e8", "e08a6a", "c88fd8", "8ad0c0", "e0d06a", "8a9ae0",
            "e0746a", "a8d46a", "6ad4d4", "f0ede6",
        ];
        if !matches!(
            key,
            "词提编码色" | "词提角标色" | "词提下划线色" | "词提底色"
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
    /// 私有标记动作执行（aux_action 解码后）
    fn run_aux(&mut self, aux: AuxAction) {
        match aux {
            AuxAction::OnlineRegister => {
                // 整改2c：注册/账号入口（默认站首页；URL 实查结论见 wenlai.rs）
                let url = gale_core::wenlai::register_url_for(&self.cfg.wenlai_domain);
                let wide: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
                unsafe {
                    let r = windows::Win32::UI::Shell::ShellExecuteW(
                        self.win.hwnd(),
                        windows::core::w!("open"),
                        windows::core::PCWSTR(wide.as_ptr()),
                        None,
                        None,
                        SW_SHOWNORMAL,
                    );
                    if r.0 as isize <= 32 {
                        self.toast("无法打开注册页面，请检查默认浏览器");
                    }
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
            AuxAction::SingleShuffle => {
                self.single_cfg.shuffle = !self.single_cfg.shuffle;
                self.save_single_cfg();
            }
            AuxAction::SingleSeg(d) => {
                let c = self.single_cfg.clamped();
                self.single_cfg.segment_chars =
                    (c.segment_chars as i32 + d).clamp(
                        gale_core::single::SINGLE_SEGMENT_MIN as i32,
                        gale_core::single::SINGLE_SEGMENT_MAX as i32,
                    ) as u16;
                self.save_single_cfg();
            }
            AuxAction::ShowFullAnalysis => self.show_full_analysis(),
            AuxAction::ShowHeatmap => self.show_heatmap(),
        }
    }
    fn save_single_cfg(&mut self) {
        if let Err(e) = self.single_cfg.save(&self.dir.join("single.json")) {
            self.toast(format!("单字配置保存失败：{e}"));
        }
        self.sync_single_fields();
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
        // Old controls must not overwrite a newly loaded draft before draw()
        // rebuilds the controls for the updated model.
        if self.edit_epoch != self.model.modal_epoch {
            return false;
        }
        let mut changed = false;
        for (k, e) in &self.edits {
            let text = e.text();
            if self.model.field(k) != text {
                changed = true;
                if k == "load_text" {
                    self.draft_revision = self.draft_revision.wrapping_add(1);
                }
                self.model.fields.insert(k.clone(), text);
            }
        }
        changed
    }
    fn open(&mut self, m: Modal) {
        self.capture_fields();
        self.model.open(m);
        gale_win::input_host::set_result_key_target(
            matches!(m, Modal::Result | Modal::Report).then_some(self.win.hwnd),
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
        }
        let now = self.ms();
        for ev in self.host.poll() {
            match ev {
                HostEvent::Commit(chars) => self
                    .sess
                    .feed_input(gale_core::compare::InputEvent::Commit { chars }, now),
                HostEvent::Backspace => self
                    .sess
                    .feed_input(gale_core::compare::InputEvent::Backspace, now),
                HostEvent::Composing(c) => self.sess.set_composing(c),
                HostEvent::Preedit(p) => self.sess.set_preedit(p),
                HostEvent::Cancelled => self.sess.cancel_composition(),
                HostEvent::Key { vk } => self.sess.on_key(vk, now),
            }
        }
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
        let (_, _, _, ph) = modals::panel(self.model.modal, lw, lh);
        self.model.panel_scroll = self.model.panel_scroll.clamp(
            0.,
            if matches!(self.model.modal, Modal::Result | Modal::Report) {
                (650. - ph).max(0.)
            } else {
                (ph - (lh - 48.).min(ph)).max(0.)
            },
        );
        let (bw, fs, lheight) = ui::body_metrics(&self.sess, &self.cfg, lw);
        let key = (self.generation, fs.to_bits(), bw.to_bits());
        if self.layout_key != key {
            self.layout = self
                .renderer
                .wrap_text(&self.sess.current_segment().text, fs, bw);
            self.layout_key = key;
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
        if let Err(e) = self.renderer.draw_frame(&f) {
            self.toast(format!("绘制失败：{e}"));
        }
        if self.model.modal == Modal::None {
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
                    12.,
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
    }
    fn analyze(&mut self) {
        if self.analyzing || self.analyzed == self.generation {
            return;
        }
        self.analyzing = true;
        let tx = self.tx.clone();
        let hwnd = self.win.hwnd;
        let generation = self.generation;
        // 整改14：词提一次性分析整篇文章（全文坐标）；换段只做本地切片，
        // 不再“打到哪段才请求哪段”。段间直接拼接（无分隔符），
        // start=全文累计字符偏移。超长文（>9500 字）会撞 server 10000 字
        // 上限，退回单段请求（段内坐标，refill 不适用——清全文缓存）。
        let full_text: String = self
            .sess
            .segments
            .iter()
            .flat_map(|s| s.text.chars())
            .collect();
        let whole_book = full_text.chars().count() <= 9500;
        let text = if whole_book {
            full_text
        } else {
            self.sess.current_segment().text.clone()
        };
        if !whole_book && !self.full_words.is_empty() {
            self.full_words.clear();
            self.full_words_ver.clear();
            self.full_words_of.clear();
        }
        self.last_analyze_whole = whole_book;
        let hints = gale_core::rules::hints_effective(self.sess.rules.word_hints, self.model.race);
        let single = self.sess.source == session::SessionSource::Single || self.cfg.hint_style == 1;
        // 缓存仅当：有词提数据 且 schema 版本未变 且 正文未变（换书/清洗自动失效）。
        let known_version = if self.full_words.is_empty()
            || self.full_words_of != text
            || self.full_words_ver.is_empty()
        {
            String::new()
        } else {
            self.full_words_ver.clone()
        };
        std::thread::spawn(move || {
            let port = local_port();
            let r = local_json(port, "/api/trainer/status", None).and_then(|status| {
                if !hints || status["hint_unavailable_reason"].is_string() {
                    return Ok(status);
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
                    Some(serde_json::json!({"text":text,"single":single})),
                )
            });
            let _ = tx.send(Bg::Analyze(generation, r));
            wake(hwnd);
        });
    }
    fn collect_background(&mut self) {
        while let Ok(r) = self.rx.try_recv() {
            match r {
                Bg::Analyze(g, r) => {
                    self.analyzing = false;
                    self.analyzed = g;
                    if g == self.generation {
                        match r {
                            Ok(v) => {
                                self.model.schema = v["schema"].as_str().unwrap_or("").into();
                                self.model.schema_ver =
                                    v["schema_ver"].as_str().unwrap_or("").into();
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
                                                && word
                                                    .code
                                                    .as_ref()
                                                    .is_none_or(|c| c.len() <= 32)
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
                                }
                                if self.last_analyze_whole {
                                    self.refill_segment_words();
                                }
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
                                self.model.words.clear();
                                self.model.status = format!("词提暂不可用 · {e}");
                            }
                        }
                        // CTF 自检失败的持续警示：任何状态文案都带上前缀（任务 7）
                        if self.tsf_broken {
                            self.model.status = format!(
                                "TSF 输入法可能不可用 · {}",
                                self.model.status
                            );
                        }
                    }
                    self.analyze();
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
                        self.toast("未检测到 QQ：请打开并登录 QQ 后重试");
                    }
                }
                Bg::FullAnalysis(g, v) => {
                    // 整改13：旧版服务无 full_analysis 字段（None）=无此功能，静默
                    if g == self.generation {
                        self.sess.full_analysis = v;
                        self.snapshot_dirty = true;
                    }
                }
                Bg::Article(g, bound, revision, r) => {
                    self.capture_fields();
                    self.qq_busy = false;
                    match r {
                        Ok(t) => {
                            if g == self.generation
                                && revision == self.draft_revision
                                && bound == self.model.bound
                                && self.sess.frozen_rules.is_none()
                                && self.pending_scores.len() < 32
                            {
                                self.model.fields.insert("load_text".into(), t);
                                self.load_text();
                            } else {
                                self.defer_draft("QQ待读群文".into(), t);
                            }
                        }
                        Err(e) => self.toast(e),
                    }
                }
                Bg::File(g, revision, r) => {
                    self.capture_fields();
                    match r {
                        Ok((name, t)) => {
                            if g != self.generation
                                || revision != self.draft_revision
                                || self.sess.frozen_rules.is_some()
                                || self.pending_scores.len() >= 32
                            {
                                self.defer_draft(name, t);
                                continue;
                            }
                            self.model.fields.insert("load_text".into(), t);
                            self.load_text();
                            if self.sess.title == "自定义文字"
                                || self.sess.title == "群文"
                                || self.sess.title.is_empty()
                            {
                                self.sess.title = name;
                                self.snapshot();
                            }
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
                        self.toast(format!("保存失败：{e}"));
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
                                self.theme = t;
                                self.apply_style();
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
            OnlineCommand::Login { .. } | OnlineCommand::Logout | OnlineCommand::SetServer { .. }
        );
        match self
            .online
            .as_mut()
            .ok_or_else(|| "在线模块初始化失败，请重新应用服务地址".to_string())
            .and_then(|w| w.submit(c))
        {
            Ok(_) => {
                if identity_change {
                    self.online_logged_in = false;
                    self.model.online_authenticated = false;
                    self.model.online_expired = true;
                    self.model.online_login_expanded = true;
                    self.model.modal_epoch += 1;
                    self.article = None;
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
                    self.online_logged_in = true;
                    self.model.online_authenticated = true;
                    self.model.online_expired = false;
                    self.model.online_login_expanded = false;
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
                    self.online_logged_in = false;
                    self.shelf.clear();
                    self.model.online_shelf.clear();
                    self.model.online_status = "登录已过期，请重新登录；当前预览保留".into();
                }
                Ok(OnlinePayload::Article(a)) => {
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
                    // 整改2a：去预览——取到正文直接载入跟打区，第一段起打
                    //（成绩单期间由自动下一段触发时，载入本身就会关闭成绩单）。
                    // 开打中（规则已冻结）或成绩积压时降级为待读草稿，不抢正文。
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
                    self.model.online_article = None;
                    self.model.online_authenticated = false;
                    self.model.online_account.clear();
                    self.cfg.wenlai_domain = s.clone();
                    self.model.fields.insert("server".into(), s);
                    self.model.online_status = "服务已切换，请重新登录".into();
                    self.save_config();
                }
                Ok(OnlinePayload::LoggedOut) => {
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
                Ok(OnlinePayload::Warning(e)) | Err(e) => {
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
    fn defer_draft(&mut self, name: String, text: String) {
        self.model.pending_source = name.clone();
        self.pending_draft = Some((name, text));
        self.toast("新文字已读取，当前草稿保留；F2中选择显示待读文字");
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
        if text.contains("-----第") {
            self.sess.load_group(parse_group_text(&text));
        } else {
            self.sess.load_book("自定义文字", &text);
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
    fn score_text(&self) -> Option<String> {
        if self.sess.phase != Phase::Done || !self.sess.recorded {
            return None;
        }
        self.sess
            .results
            .get(self.sess.current_index())
            .and_then(Option::as_ref)
            .map(format_score_report)
    }
    fn race_text(&self) -> String {
        let a = self.sess.aggregate();
        // 段数取 a.segments（<5 硬门口径，与整场聚合一致——原 results 计数
        // 会把 Esc 弃打的短段也算进去，与下面的字数/速度口径打架）。
        let mut s = format!(
            "《{}》 整场赛报\n{}段 / {}字 / {:0.2}秒 / 总速度{:0.2}\n",
            self.sess.title,
            a.segments,
            a.chars,
            a.time_ms as f64 / 1000.,
            a.speed
        );
        for r in self.sess.results.iter().flatten() {
            s.push_str(&format_score_report(r));
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
        // 整改9收口：词提四色循环选色（aux 前缀带参数，不进 AuxAction 枚举）。
        if let Action::Theme(s) = &a {
            if let Some(key) =
                s.strip_prefix(AUX_PREFIX).and_then(|c| c.strip_prefix("hint_color:"))
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
        if self.pending_scores.len() >= 32
            && matches!(
                a,
                Action::Previous
                    | Action::Restart
                    | Action::Next
                    | Action::ToggleRace
                    | Action::LoadText
                    | Action::LoadClipboard
                    | Action::LoadFile
                    | Action::LoadBook(_)
                    | Action::Practice(_)
                    | Action::OnlineLoadOnly
                    | Action::OnlineLoad
                    | Action::ReadGroup
            )
        {
            self.toast("成绩正在保存，请稍候再开始下一段");
            return;
        }
        // 整改5：F1-F4 跳段/取文不再弹"是否中断打文"确认——直接执行。
        // 打字中触发载文件/读群文时，先把未完成段按中断处理（jump 复位本段），
        // 与原确认后路径的语义一致；Esc 停表语义不变（主循环 Escape 分支）。
        if self.sess.phase == Phase::Typing
            && matches!(a, Action::LoadFile | Action::ReadGroup)
        {
            self.sess.jump(self.sess.index);
            self.reset_host();
        }
        match a {
            Action::None => {}
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
            Action::Practice(i) => {
                if let Some((title, body)) = gale_core::single::builtin_tables().get(i).copied() {
                    self.sess.jump(self.sess.index);
                    let mut rules = self.sess.rules.clone();
                    rules.mode = TrainingMode::Practice;
                    rules.word_hints = true;
                    let _ = self.sess.set_rules(rules);
                    // 整改8：段长/乱序走 single.json（shuffle 默认开、段长默认 20）。
                    // 乱序开时种子取时钟毫秒——每次进入练习都是新排列（重打本组
                    // 由 SingleSession.seed 固定复现）；关时 seed=0 保持原序。
                    let seed = if self.single_cfg.shuffle {
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64
                    } else {
                        0
                    };
                    self.sess.load_single(self.single_cfg.build_session(
                        title,
                        body,
                        if i == 4 { 2 } else { 1 },
                        seed,
                    ));
                    self.online_article_current = false;
                    self.model.race = false;
                    self.model.hints = true;
                    self.reset_host();
                    self.open(Modal::None);
                    self.snapshot();
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
                self.generation = self.generation.wrapping_add(1);
                self.model.words.clear();
                self.analyze();
                self.save_config();
            }
            Action::AppearanceReset => {
                let d = Config::default();
                self.cfg.font_size = d.font_size;
                self.cfg.line_gap = d.line_gap;
                self.cfg.hint_style = d.hint_style;
                self.cfg.motion_level = d.motion_level;
                self.cfg.theme = "墨玉".into();
                self.cfg.color_overrides.clear();
                self.cfg.smooth_caret = true;
                self.cfg.shake_on_wrong = true;
                self.model.reduced_motion = gale_win::edit::reduced_motion();
                self.theme = ui::theme("墨玉");
                self.apply_style();
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
                if self.sess.index + 1 >= self.sess.total_segments() {
                    self.open(Modal::Report);
                } else {
                    self.sess.next_segment();
                    self.reset_host();
                    self.open(Modal::None);
                }
            }
            Action::ToggleRace => {
                if self.sess.frozen_rules.is_some() {
                    self.sess.jump(self.sess.index);
                }
                let mut r = self.sess.rules.clone();
                r.mode = if r.mode == TrainingMode::Race {
                    TrainingMode::Practice
                } else {
                    TrainingMode::Race
                };
                match self.sess.set_rules(r) {
                    Ok(()) => {
                        self.model.race = self.sess.rules.mode == TrainingMode::Race;
                        self.model.hints = self.sess.rules.word_hints;
                        self.model.quote_relaxed = self.sess.rules.quotes.enabled;
                        self.reset_host();
                        self.open(Modal::None);
                        self.save_config();
                    }
                    Err(e) => self.toast(e),
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
                    self.model.fields.insert("load_text".into(), t);
                    if self.model.modal == Modal::Load {
                        self.model.modal_epoch += 1;
                    } else {
                        self.load_text();
                    }
                }
                None => self.toast("剪贴板为空或不可用"),
            },
            Action::LoadText => self.load_text(),
            Action::AcceptDraft => {
                if let Some((_, text)) = self.pending_draft.take() {
                    self.open(Modal::Load);
                    self.model.load_tab = 0;
                    self.model.fields.insert("load_text".into(), text);
                    self.model.pending_source.clear();
                    self.model.modal_epoch += 1;
                }
            }
            Action::LoadTab(tab) => {
                self.model.load_tab = tab;
                self.model.modal_epoch += 1;
            }
            Action::LoadEdit => {
                self.model.load_editing = !self.model.load_editing;
                self.model.modal_epoch += 1;
            }
            Action::LoadBook(i) => {
                if let Some(b) = self.library.books().get(i) {
                    let name = b.name.clone();
                    let text = b.raw_text.clone();
                    self.sess.load_book(&name, &text);
                    self.online_article_current = false;
                    self.reset_host();
                    self.open(Modal::None);
                    self.snapshot();
                }
            }
            Action::LoadFile => {
                if let Some(p) = file_dialog(self.win.hwnd, false, "txt") {
                    let tx = self.tx.clone();
                    let hwnd = self.win.hwnd;
                    let g = self.generation;
                    let revision = self.draft_revision;
                    std::thread::spawn(move || {
                        let r = read_article_file(&p);
                        let _ = tx.send(Bg::File(g, revision, r));
                        wake(hwnd);
                    });
                }
            }
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
                self.save_config();
            }
            Action::Material(mat) => {
                self.cfg.material = mat;
                self.apply_style();
                self.save_config();
            }
            Action::Font(d) => {
                // 整改6：外观页字号 +/- 与 Ctrl+滚轮统一口径（10..=60）
                self.cfg.font_size = (self.cfg.font_size + d as f32).clamp(10., 60.);
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
                let s = self.race_text();
                self.copy(&s);
            }
            Action::SendScore => {
                if let Some(s) = self.score_text() {
                    self.copy(&s);
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
                    std::thread::spawn(move || {
                        unsafe {
                            let _ = windows::Win32::System::Com::CoInitializeEx(
                                None,
                                windows::Win32::System::Com::COINIT_MULTITHREADED,
                            );
                        }
                        // 整改7：主面板会话列表 ∪ 独立聊天窗，按标题去重（主面板优先）
                        let chats = qq_bridge::list_sessions_merged();
                        let _ = tx.send(Bg::Chats(chats));
                        wake(hwnd);
                    });
                }
            }
            Action::SelectGroup(i) => {
                if i < self.chats.len() {
                    self.model.bound = Some(i);
                    self.toast("已选择会话。请在 QQ 手动打开该群后读取");
                }
            }
            Action::ReadGroup => {
                if self.qq_busy {
                    self.toast("QQ 正在读取，请稍候");
                } else if let Some(c) = self.model.bound.and_then(|i| self.chats.get(i)).cloned() {
                    self.qq_busy = true;
                    let tx = self.tx.clone();
                    let hwnd = self.win.hwnd;
                    let g = self.generation;
                    let bound = self.model.bound;
                    let revision = self.draft_revision;
                    std::thread::spawn(move || {
                        unsafe {
                            let _ = windows::Win32::System::Com::CoInitializeEx(
                                None,
                                windows::Win32::System::Com::COINIT_MULTITHREADED,
                            );
                        }
                        let r = uia_bridge::read_current_article(c.hwnd, &c.title);
                        let _ = tx.send(Bg::Article(g, bound, revision, r));
                        wake(hwnd);
                    });
                } else {
                    self.toast("先按 F6 扫描并选择 QQ 会话，或手动复制群文");
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
fn read_article_file(p: &Path) -> Result<(String, String), String> {
    let meta = std::fs::metadata(p).map_err(|e| e.to_string())?;
    if meta.len() > 2_000_000 {
        return Err("文本文件超过 2 MB".into());
    }
    let b = std::fs::read(p).map_err(|e| e.to_string())?;
    let s = if b.starts_with(&[0xFF, 0xFE]) {
        if b.len() % 2 != 0 {
            return Err("UTF-16文本字节长度无效".into());
        }
        String::from_utf16(
            &b[2..]
                .chunks_exact(2)
                .map(|v| u16::from_le_bytes([v[0], v[1]]))
                .collect::<Vec<_>>(),
        )
        .map_err(|_| "UTF-16 文本无效")?
    } else {
        String::from_utf8(b).map_err(|_| "请将文本另存为 UTF-8 或 UTF-16 LE 编码")?
    };
    Ok((
        p.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        s.trim_start_matches('\u{feff}').to_string(),
    ))
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
        return Err("词提响应过长".into());
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
            413 => "文章超过词提服务字数上限，请缩短段落".into(),
            429 => "词提服务正在处理其他请求，请稍后重试".into(),
            _ => format!("虎符服务返回HTTP {code}"),
        }));
    }
    serde_json::from_slice(&b[sep + 4..]).map_err(|_| "词提数据无效".into())
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    #[test]
    fn completed_round_continues_only_from_result_or_typing_view() {
        let panels = [
            Modal::None,
            Modal::Result,
            Modal::Load,
            Modal::Groups,
            Modal::Appearance,
            Modal::Report,
            Modal::Online,
            Modal::Rules,
            Modal::Confirm,
        ];
        for panel in panels {
            for key in [CommandKey::Enter, CommandKey::Space] {
                assert_eq!(
                    completed_segment_action(key, true, panel),
                    if matches!(panel, Modal::None | Modal::Result) {
                        Some(Action::Next)
                    } else {
                        None
                    },
                    "panel={panel:?} key={key:?}"
                );
                assert_eq!(completed_segment_action(key, false, panel), None);
            }
        }
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
