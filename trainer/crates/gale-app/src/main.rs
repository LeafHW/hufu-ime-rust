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
    wenlai::{Difficulty, FetchParams, WenlaiArticle},
};
use gale_win::{
    d2d::Renderer,
    edit::NativeEdit,
    input_host::{CommandKey, HostConfig, HostEvent, InputHost},
    window::{MainWindow, MouseEv},
};
use modals::{Action, Modal, Model};
use online::{OnlineCommand, OnlinePayload, OnlineWorker};
use session::{Phase, Session};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::*,
};
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
    theme_generation: u64,
    rule_generation: u64,
    rule_busy: bool,
    qq_busy: bool,
    exit: bool,
    online_pending: usize,
    online_logged_in: bool,
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
        dir,
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
        theme_generation: 0,
        rule_generation: 0,
        rule_busy: false,
        qq_busy: false,
        exit: false,
        online_pending: 0,
        online_logged_in: false,
    };
    app.sync_rule_model();
    if app.cfg.theme == "跟随虎符" {
        app.fetch_skin();
    }
    app.apply_style();
    app.host.show();
    app.host.focus();
    app.analyze();
    gale_win::window::request_frame();
    let mut timer = false;
    let mut last_draw = Instant::now() - Duration::from_secs(1);
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
            for ev in events {
                match ev {
                    HostEvent::Key { vk } => app.sess.on_key(vk, now),
                    HostEvent::Commit(s) => app
                        .sess
                        .feed_input(gale_core::compare::InputEvent::Commit { chars: s }, now),
                    HostEvent::Backspace => app
                        .sess
                        .feed_input(gale_core::compare::InputEvent::Backspace, now),
                    HostEvent::Composing(c) => app.sess.set_composing(c),
                    HostEvent::Preedit(p) => app.sess.set_preedit(p),
                    HostEvent::Cancelled => app.sess.cancel_composition(),
                }
            }
        }
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
        gale_win::window::request_frame();
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
        let (bw, fs, _) = ui::body_metrics(&self.sess, &self.cfg, lw);
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
        let text = self.sess.current_segment().text.clone();
        let hints = self.sess.rules.word_hints && !self.model.race;
        let single = self.sess.source == session::SessionSource::Single || self.cfg.hint_style == 1;
        let known_version = if self.model.words.is_empty() {
            String::new()
        } else {
            self.model.schema_ver.clone()
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
                                if v["words_unchanged"] != true {
                                    self.model.words = serde_json::from_value(v["words"].clone())
                                        .unwrap_or_default();
                                }
                                let chars: Vec<char> =
                                    self.sess.current_segment().text.chars().collect();
                                self.model.words.retain(|word| {
                                    word.len > 0
                                        && word.start <= chars.len()
                                        && word.len <= chars.len() - word.start
                                        && chars[word.start..word.start + word.len]
                                            .iter()
                                            .copied()
                                            .eq(word.text.chars())
                                        && word.code.as_ref().is_none_or(|c| c.len() <= 32)
                                        && word.tail.len() <= 16
                                });
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
                    }
                    self.analyze();
                }
                Bg::Chats(v) => {
                    self.qq_busy = false;
                    self.chats = v;
                    self.model.groups = self.chats.iter().map(|c| c.title.clone()).collect();
                    self.model.bound = None;
                    if self.chats.is_empty() {
                        self.toast("未发现 QQ 会话，请打开 QQ 后重试");
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
        let mut s = format!(
            "《{}》 整场赛报\n{}段 / {}字 / {:0.2}秒 / 总速度{:0.2}\n",
            self.sess.title,
            self.sess.results.iter().flatten().count(),
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
        if confirmed
            && self.sess.phase == Phase::Typing
            && matches!(a, Action::LoadFile | Action::ReadGroup)
        {
            self.sess.jump(self.sess.index);
            self.reset_host();
        }
        if !confirmed
            && self.sess.phase == Phase::Typing
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
                    | Action::Exit
            )
        {
            self.model.confirm = "本段还未完成。继续将中止当前练习，本段不会作为完成成绩。".into();
            self.model.pending = Some(a);
            self.open(Modal::Confirm);
            return;
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
                    self.sess.load_single(gale_core::single::SingleSession {
                        title: title.into(),
                        group_size: 10,
                        items: gale_core::single::parse_items(body, if i == 4 { 2 } else { 1 }),
                        group_index: 0,
                        seed: 0,
                    });
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
                self.cfg.font_size = (self.cfg.font_size + d as f32).clamp(24., 64.);
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
                    let tx = self.tx.clone();
                    let hwnd = self.win.hwnd;
                    std::thread::spawn(move || {
                        unsafe {
                            let _ = windows::Win32::System::Com::CoInitializeEx(
                                None,
                                windows::Win32::System::Com::COINIT_MULTITHREADED,
                            );
                        }
                        let mut chats = Vec::new();
                        if let Some(p) = qq_bridge::main_panel() {
                            for n in uia_bridge::list_sessions(p.hwnd) {
                                chats.push(qq_bridge::ChatWindow {
                                    title: n,
                                    hwnd: p.hwnd,
                                });
                            }
                        }
                        if chats.is_empty() {
                            chats = qq_bridge::list_chat_windows();
                        }
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
            Action::OnlineLoad => {
                if let Some(a) = self.article.clone() {
                    if a.text.chars().count() > 100000 {
                        self.toast("在线正文超过100000字，请重新取较短文段");
                        return;
                    }
                    self.sess.load_group(gale_core::segment::ParsedGroupText {
                        title: a.title,
                        difficulty: Some(a.difficulty.label().into()),
                        segments: vec![gale_core::segment::Segment {
                            mark: a.seg_mark,
                            text: a.text,
                        }],
                    });
                    self.reset_host();
                    self.open(Modal::None);
                    self.snapshot();
                } else {
                    self.toast("请先取得并预览文章");
                }
            }
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
