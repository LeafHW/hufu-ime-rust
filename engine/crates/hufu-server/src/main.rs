//! hufu-server —— 守护进程：引擎宿主 + HTTP 设置界面 + 前端 IPC。
//!
//! 用法：hufu-server [--data <目录>] [--port <端口>] [--console]
//! 默认数据目录：./hufu-data（或环境变量 HUFU_DATA）；默认端口 4390。
//!
//! 【GUI 子系统】修复「开机自启弹出终端（重排装载日志）」：HKCU Run 裸路径
//! 启动控制台程序会弹黑窗。改为 windows 子系统后任何拉起方（Run/DLL/
//! explorer 中转）都无窗；开发态从终端启动时 AttachConsole(父进程)
//! 接回 stdout/stderr（首次输出前调用，std 句柄懒初始化可拿到控制台）；
//! --console 强制 AllocConsole（双击 exe 调试用）。
#![cfg_attr(not(feature = "console"), windows_subsystem = "windows")]

mod candwin;
mod host;
mod http;
mod pipe;
#[cfg(windows)]
mod clipboard;
#[cfg(windows)]
mod tray;

use host::{parse_key, Host};
use http::{Request, Response};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const INDEX_HTML: &str = include_str!("../../../../settings-ui/index.html");

/// 开发态接回终端（见文件头注释）。必须在任何 stdout/stderr 输出前调用。
/// 零依赖直声明（与 pipe/tray 同风格）；std 句柄懒初始化，Attach 成功后
/// 首次 println 即可写入父控制台。
#[cfg(windows)]
fn attach_console_for_dev(force: bool, dev_attach: bool) {
    const ATTACH_PARENT_PROCESS: usize = usize::MAX;
    // windows-sys 原型（保持零特性门）
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(dwProcessId: usize) -> i32;
        fn AllocConsole() -> i32;
        fn GetConsoleWindow() -> isize;
    }
    unsafe {
        if GetConsoleWindow() != 0 {
            return; // 已有控制台（终端里 cargo run / --console 二次调用）
        }
        if force {
            let _ = AllocConsole();
        } else if dev_attach {
            // 仅开发显式开关（HUFU_DEV_CONSOLE=1）时接回父控制台；默认零
            // 输出——安装器/Run/DLL 自愈等任何拉起方都静默（用户实测反馈：
            // 安装窗口出现装载日志会吓到人）。
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

fn main() {
    #[cfg(all(windows, not(feature = "console")))]
    {
        let force = std::env::args().any(|a| a == "--console");
        let dev = std::env::var("HUFU_DEV_CONSOLE").map(|v| v == "1").unwrap_or(false);
        attach_console_for_dev(force, dev);
    }
    let mut args = std::env::args().skip(1);
    let mut data_dir = std::env::var("HUFU_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            // 默认：exe 同目录下的「数据」（安装布局 %LOCALAPPDATA%\HuFu\数据）。
            // 不再用相对路径 hufu-data——CWD 不可控（开机自启/explorer 中转启动时
            // CWD 是 system32 等），相对默认会凭空建出错误目录。
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                .map(|d| d.join("数据"))
                .unwrap_or_else(|| PathBuf::from("hufu-data"))
        });
    let mut port: u16 = 4390;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data" => {
                if let Some(v) = args.next() {
                    data_dir = PathBuf::from(v);
                }
            }
            "--port" => {
                if let Some(v) = args.next() {
                    port = v.parse().unwrap_or(4390);
                }
            }
            _ => {}
        }
    }
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        eprintln!("数据目录不可创建: {e}");
        std::process::exit(1);
    }
    // 【单实例互斥 2026-09-11】此前靠 4390 端口 bind 冲突兜底，但管道
    // 线程先于 HTTP 启动 → 双实例并存窗口期（双 pipe、双托盘消息窗）。
    // 命名互斥体进程级兜底：已存在即本进程直接退（老实例继续服务）。
    #[cfg(windows)]
    if sys_win::already_running() {
        eprintln!("hufu-server 已在运行（命名互斥体命中），本实例退出");
        std::process::exit(0);
    }
    // 【DPI 感知 2026-09-11】server 代画候选窗此前按 96-DPI 逻辑像素
    // 当物理像素用：进程默认 DPI-unaware，窗口被系统拉伸模糊、坐标
    // 与 DLL（物理像素）错位。声明 Per-Monitor V2 后按窗口实际 DPI
    // 缩放渲染（candwin.rs render_frame），与 candwin2 同款语义。
    #[cfg(windows)]
    sys_win::declare_per_monitor_dpi();
    let host = match Host::new(&data_dir) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("引擎初始化失败: {e}");
            std::process::exit(1);
        }
    };
    {
        // 【性能插桩】main 侧总戳（与 host.rs 的 Host::new 打点配套）
        // 【轮转 2026-09-11】启动追加了无上限增长——超 4MB 翻转 .old
        use std::io::Write;
        let p = r"C:\ProgramData\HuFu\diag\startup-trace.txt";
        let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
        if std::fs::metadata(p).map(|m| m.len() > 4 << 20).unwrap_or(false) {
            let _ = std::fs::rename(p, r"C:\ProgramData\HuFu\diag\startup-trace.old.txt");
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
        {
            let _ = writeln!(f, "--- Host::new 完成（含 spawn 前全部同步工作）---");
        }
    }
    let shared = Arc::new(Mutex::new(host));
    // 【/jq→补充语料 2026-09-06】weight API 运行时触发整句模型重载
    // 需要 'static 句柄——全局登记。
    let _ = HOST_HANDLE.set(shared.clone());
    let addr = format!("127.0.0.1:{port}");

    // 命名管道（Windows 前端 IPC）独立线程
    #[cfg(windows)]
    {
        let p = shared.clone();
        std::thread::spawn(move || {
            if let Err(e) = pipe::run_pipe(p) {
                eprintln!("命名管道服务退出: {e}");
            }
        });
    }

    // 整句模型后台装载：Host::new 只载词典（秒级），此处线程不持锁
    // 载 ngram（~10s），载完短锁热挂。期间管道/设置页/打字照常
    //（词典模式），整句能力稍后自动就位——修「装完要等好久才能
    // 正常打字」：管道不再被模型加载阻塞。
    spawn_sentence_reload(shared.clone(), false);
    // 【性能】反查表后台预热：启动路径已不载（懒加载省冷启动 ~700ms），
    // 此处稍等片刻（让位 ngram/打字 IO）后装表，用户首按反查前缀
    // （默认 `）前即已就绪。
    {
        let shared_bg = shared.clone();
        let _ = std::thread::Builder::new()
            .name("hufu-reverse-warm".into())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let mut h = shared_bg.lock().unwrap_or_else(|p| p.into_inner());
                h.engine.ensure_reverse();
            });
    }

    // 【拖入模型自动生效 2026-09-10】无模型小包用户事后把模型文件拖进
    // 目录，此前不会自动装载——须切一次方案/重载一次码表才生效（用户
    // 实测确认）。本线程每 5s 探测模型文件（GGUF 重排 + ngram 整句），
    // 「先缺后在且尺寸稳定（拷贝完成：两轮同 size+mtime）」边沿触发
    // 一次 reload_sentence_bg——GGUF 补建重排线程、ngram 后台装载、
    // 整句门控判断一条通道全办（幂等：BUSY 闸门+装载内部门控）。
    // 启动时已在场的文件视为「启动路径已装载」不触发；文件被删除重置
    // 边沿（再次拖入可再触发）；装载失败（文件损坏）不重试，手动重载
    // 码表仍可救。探测开销：每 5s 两次 metadata 读取。
    {
        let shared_w = shared.clone();
        let data_dir_w = data_dir.clone();
        let _ = std::thread::Builder::new()
            .name("hufu-model-watch".into())
            .spawn(move || {
                let stat_of = |p: &std::path::Path| -> Option<(u64, i64)> {
                    let m = p.metadata().ok()?;
                    Some((
                        m.len(),
                        m.modified()
                            .ok()?
                            .duration_since(std::time::UNIX_EPOCH)
                            .ok()?
                            .as_secs() as i64,
                    ))
                };
                let gguf_stat = |data_dir: &std::path::Path| -> Option<(u64, i64)> {
                    let model_dir = hufu_engine::Engine::resolve_data_sub(data_dir, "模型");
                    std::fs::read_dir(&model_dir)
                        .ok()?
                        .filter_map(|e| e.ok())
                        .find(|e| {
                            e.path()
                                .extension()
                                .map(|x| x.eq_ignore_ascii_case("gguf"))
                                .unwrap_or(false)
                        })
                        .and_then(|e| stat_of(&e.path()))
                };
                // 状态机：0=不在 1=首轮见 2=稳定（两轮同参）3=消失
                // prev 永远同步为本轮快照（cur）。
                let step = |prev: &mut Option<(u64, i64)>, cur: Option<(u64, i64)>| -> u8 {
                    let old = prev.take();
                    *prev = cur;
                    match (old, cur) {
                        (None, None) => 0,
                        (None, Some(_)) => 1,
                        (Some(_), None) => 3,
                        (Some(p), Some(c)) => {
                            if p == c { 2 } else { 1 }
                        }
                    }
                };
                let mut prev_gguf: Option<(u64, i64)> = None;
                let mut prev_ngram: Option<(u64, i64)> = None;
                let mut seen_gguf = false; // 已在场/已触发过（防重复）
                let mut seen_ngram = false;
                let mut first_round = true;
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    let (ngram_path, rerank_enabled) = {
                        let h = shared_w.lock().unwrap_or_else(|p| p.into_inner());
                        (
                            hufu_engine::Engine::resolve_data_sub(
                                &data_dir_w,
                                &h.engine.config.sentence.ngram_path,
                            ),
                            h.engine.config.sentence.rerank.enabled,
                        )
                    };
                    let cur_gguf = if rerank_enabled { gguf_stat(&data_dir_w) } else { None };
                    let cur_ngram = stat_of(&ngram_path);
                    let g = step(&mut prev_gguf, cur_gguf);
                    let n = step(&mut prev_ngram, cur_ngram);
                    // 启动首轮在场：视为已装载（启动路径自己会装），不算边沿
                    if first_round {
                        first_round = false;
                        if cur_gguf.is_some() { seen_gguf = true; }
                        if cur_ngram.is_some() { seen_ngram = true; }
                        continue;
                    }
                    if g == 3 { seen_gguf = false; }
                    if n == 3 { seen_ngram = false; }
                    let trig = (g == 2 && !seen_gguf) || (n == 2 && !seen_ngram);
                    if g == 2 { seen_gguf = true; }
                    if n == 2 { seen_ngram = true; }
                    if trig {
                        eprintln!("模型监视：检测到新拖入的模型文件，自动装载（GGUF/整句通道）");
                        reload_sentence_bg(false, false);
                    }
                }
            });
    }

    // Windows 托盘（双击开设置页 / 右键退出）
    #[cfg(windows)]
    {
        use std::sync::mpsc;
        let (quit_tx, quit_rx) = mpsc::channel::<()>();
        let (open_tx, open_rx) = mpsc::channel::<()>();
        tray::spawn(quit_tx, open_tx, Some(shared.clone()));
        let url = format!("http://{addr}/");
        std::thread::spawn(move || {
            // 常驻循环：每次托盘信号都开窗口（旧版一次性线程导致第二次进不去）
            while open_rx.recv().is_ok() {
                // 独立应用窗口（Chromium --app 模式）：有自己的任务栏图标、无地址栏，
                // 观感等同原生窗口。CreateProcess 不查 App Paths，须用完整路径；
                // Edge → Chrome → 默认浏览器三级回退（没装 Edge 的机器用 Chrome
                // 同样得到独立窗口）。窗口已开时再启动会聚焦/新开一窗。
                let pf86 = std::env::var("ProgramFiles(x86)").unwrap_or_default();
                let pf = std::env::var("ProgramFiles").unwrap_or_default();
                let pflocal = std::env::var("LOCALAPPDATA").unwrap_or_default();
                let app_arg = format!("--app={url}");
                // 【用户定稿】900×800 紧凑窗口。Edge/Chrome 单实例驻留时
                // --window-size 会被忽略（参数转发给已有实例）——独立
                // user-data-dir 让设置窗口自成实例，尺寸参数永远生效，
                // 也避免与用户日常浏览器窗口互相干扰。
                let profile = std::env::var("LOCALAPPDATA")
                    .map(|p| format!("{p}\\HuFuSettingsProfile"))
                    .unwrap_or_else(|_| "HuFuSettingsProfile".to_string());
                let size_arg = "--window-size=900,800";
                let extra_args = [
                    format!("--user-data-dir={profile}"),
                    "--no-first-run".to_string(),
                    "--no-default-browser-check".to_string(),
                ];
                let browser = [
                    format!("{pf86}\\Microsoft\\Edge\\Application\\msedge.exe"),
                    format!("{pf}\\Microsoft\\Edge\\Application\\msedge.exe"),
                    format!("{pflocal}\\Google\\Chrome\\Application\\chrome.exe"),
                    format!("{pf}\\Google\\Chrome\\Application\\chrome.exe"),
                    format!("{pf86}\\Google\\Chrome\\Application\\chrome.exe"),
                ]
                .into_iter()
                .find(|p| std::path::Path::new(p).exists());
                let opened = match &browser {
                    Some(exe) => {
                        let mut c = std::process::Command::new(exe);
                        c.arg(&app_arg).arg(size_arg);
                        for a in &extra_args {
                            c.arg(a);
                        }
                        c.spawn().is_ok()
                    }
                    None => false,
                };
                if !opened {
                    let _ = std::process::Command::new("cmd")
                        .args(["/C", "start", "", &url])
                        .spawn();
                }
            }
        });
        std::thread::spawn(move || {
            if quit_rx.recv().is_ok() {
                // 【pid 路径修复 2026-09-11】旧实现用 current_dir()（GUI
                // 子系统 CWD 不可控，基本删不中）。谁都不写 server.pid
                //（历史上只有开发脚本假定它存在）——按脚本期望路径防御
                // 性清理一次。
                let _ = std::fs::remove_file(data_dir.join("server.pid"));
                std::process::exit(0);
            }
        });
    }

    let handler = {
        let shared = shared.clone();
        move |req: &Request| -> Response { route(&shared, req) }
    };
    if let Err(e) = http::serve(&addr, Arc::new(handler)) {
        eprintln!("HTTP 服务失败: {e}");
        std::process::exit(1);
    }
}

/// 【/jq→补充语料 2026-09-06】weight API 运行时触发整句模型重载需要
/// 'static 句柄——main 里登记，全局登记。
static HOST_HANDLE: std::sync::OnceLock<std::sync::Arc<std::sync::Mutex<Host>>> =
    std::sync::OnceLock::new();

/// 【卡死修复 2026-09-11】切方案/重载码表/改配置路径经此入口重建整句：
/// 旧 setup_sentence 在持锁状态同步载 546MB ngram（秒级~10s），期间
/// 全机按键/轮询阻塞。现统一为：独立线程短锁决策——门控不满足
///（切到非整句方案/关整句）立即拆旧模型（便宜）；teardown_old=真
///（方案切换，旧模型词典与新方案不符）也先拆——打字暂时走码表
///（词典模式），后台不持锁装载、载完短锁热挂（与启动路径同款）。
/// 调用方可能正持有 Host 锁（pipe dispatch/HTTP 路由/tray）：函数
/// 体内再入锁会死锁，故先 spawn 井线程、由它等锁释放。
pub fn reload_sentence_bg(teardown_old: bool, resupplement: bool) {
    std::thread::Builder::new()
        .name("hufu-sentence-reload-kick".into())
        .spawn(move || {
            let Some(shared) = HOST_HANDLE.get() else { return };
            {
                let mut h = shared.lock().unwrap_or_else(|p| p.into_inner());
                if teardown_old || h.sentence_load_plan().is_none() {
                    h.engine.set_sentence_decoder(None);
                }
                // 拖入模型补装（原 setup_sentence 尾部语义）：rerank 线程
                // 缺位而 qwen 模型如今在场 → 补建
                h.ensure_rerank_if_late();
            }
            spawn_sentence_reload(shared.clone(), resupplement);
        })
        .ok();
}

/// 整句模型后台装载（启动与 /jq 加权后共用）：短锁取装载计划
///（resupplement=true 时先重读 补充语料.txt 刷新内存快照——补充语料
/// 只在模型装载时生效，写入后必须重载模型），不持锁载 ngram
///（page cache 热时 ~2s），载完短锁热挂。期间管道/设置页/打字照常
///（旧模型继续服务）。
/// 【加载去重 2026-09-11】连续触发（weight API 连续调/快速切方案）
/// 不再各起一个装载线程并发载 N 份 546MB：BUSY 闸门 + PENDING 重跑，
/// 装载串行、最后一次请求语义不丢。
static NGRAM_LOAD_BUSY: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static NGRAM_LOAD_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn spawn_sentence_reload(
    shared: std::sync::Arc<std::sync::Mutex<Host>>,
    resupplement: bool,
) {
    use std::sync::atomic::Ordering;
    if NGRAM_LOAD_BUSY.swap(true, Ordering::SeqCst) {
        // 已有装载在跑：只记「再来一次」（装载完成后补跑，覆盖最新
        // 配置——补跑按重读补充语料语义，保证 weight 类变更生效）
        NGRAM_LOAD_PENDING.store(true, Ordering::SeqCst);
        return;
    }
    std::thread::Builder::new()
        .name("hufu-ngram-load".into())
        .spawn(move || {
            let mut first = true;
            loop {
                let resupplement = if first { resupplement } else { true };
                first = false;
                let t0 = std::time::Instant::now();
                let plan = {
                    let mut h = shared.lock().unwrap_or_else(|p| p.into_inner());
                    if resupplement {
                        let p = h.engine.schema.dir.join("补充语料.txt");
                        match hufu_dict::supplement::Supplement::load(&p) {
                            Ok(s) => h.engine.schema.supplement = s,
                            Err(e) => eprintln!("补充语料重读失败: {e}"),
                        }
                    }
                    h.sentence_load_plan()
                };
                let Some((path, dict, supplement, weights)) = plan else {
                    break;
                };
                // 【性能】mmap 页缓存预热：v5 模型 546MB 惰性映射，首查
                // 缺页逐条读盘。冷启动时并行顺序读整文件填 page cache；
                // 重载时页缓存已热，顺序读很快返回。
                {
                    let p = path.clone();
                    std::thread::Builder::new()
                        .name("hufu-ngram-warm".into())
                        .spawn(move || {
                            let t0 = std::time::Instant::now();
                            if let Ok(mut f) = std::fs::File::open(&p) {
                                use std::io::Read;
                                let mut buf = vec![0u8; 4 << 20];
                                while let Ok(n) = f.read(&mut buf) {
                                    if n == 0 {
                                        break;
                                    }
                                }
                            }
                            eprintln!(
                                "ngram 页缓存预热完成（{:.1}s）",
                                t0.elapsed().as_secs_f32()
                            );
                        })
                        .ok();
                }
                match hufu_sentence::SentenceEngine::load(&path, dict, &supplement, weights) {
                    Ok(dec) => {
                        let mut h = shared.lock().unwrap_or_else(|p| p.into_inner());
                        // 装载期间用户可能切方案/关整句：只在仍满足
                        // 门控时挂载，否则弃用本次结果
                        if h.engine.config.schema.current.contains("整句")
                            && h.engine.config.sentence.enabled
                        {
                            h.engine.set_sentence_decoder(Some(std::sync::Arc::new(dec)));
                            // 【用户词注入 2026-09-06】装载后即注入
                            //（/jc 加词参与整句词图）
                            h.engine.sync_sentence_user_words();
                            eprintln!(
                                "整句引擎已加载（后台 {:.1}s）: {}",
                                t0.elapsed().as_secs_f32(),
                                path.display()
                            );
                        }
                    }
                    Err(e) => eprintln!("整句模型后台加载失败: {e}"),
                }
                if !NGRAM_LOAD_PENDING.swap(false, Ordering::SeqCst) {
                    break;
                }
                eprintln!("整句模型装载：排队中的重载请求，补跑一轮");
            }
            NGRAM_LOAD_BUSY.store(false, Ordering::SeqCst);
        })
        .ok();
}

/// 清 补充语料.txt 同词旧行（`词 权重` 格式；# 注释行/空行保留）。
fn rewrite_supplement_lines(path: &std::path::Path, word: &str) {
    if let Ok(content) = std::fs::read_to_string(path) {
        let kept: Vec<&str> = content
            .lines()
            .filter(|l| {
                let t = l.trim();
                if t.is_empty() || t.starts_with('#') {
                    return true;
                }
                let first = t.split_whitespace().next().unwrap_or("");
                first != word
            })
            .collect();
        let mut s = kept.join("\n");
        if !s.is_empty() {
            s.push('\n');
        }
        let _ = std::fs::write(path, s.as_bytes());
        // 【审计】行数变化落 adj-audit.log（与 用户调整.txt 同款抓现场）
        if let Some(dir) = path.parent() {
            use std::io::Write;
            if let Ok(mut a) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("adj-audit.log"))
            {
                let _ = writeln!(
                    a,
                    "[server supplement] {word} 前={} 后={}",
                    content.lines().count(),
                    kept.len() + 1
                );
            }
        }
    }
}

/// 【格式统一 2026-09-06】清 用户调整.txt 中同码同词旧行（四种标记
/// 行+旧 TSV 词行一起清——写入端只留最新操作，文件不膨胀且回放
/// 语义与追加日志等价）。
fn rewrite_keep_lines(path: &std::path::Path, code: &str, text: &str) {
    if let Ok(content) = std::fs::read_to_string(path) {
        let kept: Vec<&str> = content
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                let body = ["{置顶}", "{添加}", "{删除}", "{加权}"]
                    .iter()
                    .find_map(|m| t.strip_prefix(m))
                    .unwrap_or(t);
                let mut it = body.split('\t');
                let c = it.next().unwrap_or("").trim();
                let w = it.next().unwrap_or("").trim();
                !(c == code && w == text)
            })
            .collect();
        let mut s = kept.join("\n");
        if !s.is_empty() {
            s.push('\n');
        }
        let _ = std::fs::write(path, s.as_bytes());
    }
}

fn route(host: &Mutex<Host>, req: &Request) -> Response {
    // 【浏览器源加固 2026-09-11】所有副作用端点（POST）拒绝跨源网页：
    // 浏览器跨源 POST 必带 Origin 头且值非本站（页面 JS 无法伪造）；
    // 本机原生调用方（设置页同源 fetch、托盘、管道 DLL）要么同源
    // 要么无 Origin。恶意网页 fetch no-cors POST 直达 127.0.0.1:4390
    // 杀 server（/api/shutdown 无鉴权审计项）由此封死；Host 白名单
    // 同时防 DNS rebinding（外域域名解析到 127.0.0.1 的请求 Host
    // 不是本机名）。
    if req.method == "POST" {
        if let Some(origin) = req.headers.get("origin").filter(|o| !o.is_empty()) {
            let local = origin.starts_with("http://127.0.0.1:")
                || origin.starts_with("http://localhost:")
                || origin.starts_with("http://[::1]:");
            if !local {
                return Response::err(403, "跨源请求被拒绝");
            }
        }
        if let Some(h) = req.headers.get("host").filter(|h| !h.is_empty()) {
            let h = h.to_lowercase();
            let local_host = h.starts_with("127.0.0.1")
                || h.starts_with("localhost")
                || h.starts_with("[::1]");
            if !local_host {
                return Response::err(403, "非法 Host");
            }
        }
    }
    let mut host = host.lock().unwrap_or_else(|p| p.into_inner());
    let method = req.method.as_str();
    let path = req.path.as_str();

    match (method, path) {
        ("GET", "/") => Response {
            status: 200,
            content_type: "text/html; charset=utf-8",
            body: INDEX_HTML.as_bytes().to_vec(),
        },
        ("GET", "/api/state") => {
            let state = host.engine.state(&host.session);
            Response::json(&serde_json::json!({
                "state": state,
                "schemas": host.engine.schemas,
                "current_schema": host.engine.config.schema.current,
                "sentence_active": host.engine.sentence_active(),
                // 【无模型小包 2026-09-07】设置页「整句模型」页判断用：
                // ngram 模型文件是否在数据目录（小包默认不带，设置页
                // 显示下载/放置指引；与 sentence_active 分开——后者还
                // 受当前方案是否整句方案影响）。
                "model_present": hufu_engine::Engine::resolve_data_sub(
                    &host.data_dir,
                    &host.engine.config.sentence.ngram_path,
                )
                .exists(),
            }))
        }
        ("POST", "/api/key") => {
            let key = match parse_key(&req.json()) {
                Some(k) => k,
                None => return Response::err(400, "按键描述无效"),
            };
            host.session.line_end_hint = req
                .json()
                .get("line_end")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Response::json(&host.process_key(key))
        }
        ("POST", "/api/reset") => {
            host.session = hufu_engine::Session::new(true);
            let state = host.engine.state(&host.session);
            Response::json(&serde_json::json!({ "state": state }))
        }
        ("GET", "/api/config") => Response::json(&serde_json::to_value(&host.engine.config).unwrap()),
        ("GET", "/api/schemas") => {
            // 方案列表 = 码表目录的子目录名（实时列目录）。
            // 【2026-09-06】码表目录一级布局：优先安装根\码表，回退 数据\码表
            // （resolve_data_sub 与引擎同源判定）。
            let dir =
                hufu_engine::Engine::resolve_data_sub(&host.data_dir, &host.engine.config.schema.dir);
            let mut names: Vec<String> = std::fs::read_dir(&dir)
                .map(|rd| {
                    rd.flatten()
                        // 注意：码表子目录多为 junction，DirEntry::file_type() 对
                        // 链接点返回 reparse（非目录）——用 path().is_dir() 跟随判定
                        .filter(|e| e.path().is_dir())
                        .filter_map(|e| e.file_name().into_string().ok())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            Response::json(&serde_json::json!({ "schemas": names }))
        }
        // 【2026-09-06 大统一】全局资源清单：拼音反查方案（数据\拼音反查\*.txt）
        // 与拆分方案（数据\拆分\*.拆分）——设置页下拉实时数据源。
        ("GET", "/api/assets") => {
            let list = |dir: &str, ext: &str| -> Vec<String> {
                let mut v: Vec<String> = std::fs::read_dir(host.data_dir.join(dir))
                    .map(|rd| {
                        rd.flatten()
                            .filter(|e| e.path().is_file())
                            .filter(|e| {
                                e.path()
                                    .extension()
                                    .and_then(|x| x.to_str())
                                    .map(|x| x == ext)
                                    .unwrap_or(false)
                            })
                            .filter_map(|e| {
                                e.path()
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .map(|s| s.to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                v.sort();
                v
            };
            let reverse = list("拼音反查", "txt");
            let split = list("拆分", "拆分");
            Response::json(&serde_json::json!({ "reverse": reverse, "split": split }))
        }
        ("POST", "/api/config") => {
            let v = req.json();
            let cfg: hufu_config::Config = match serde_json::from_value(v) {
                Ok(c) => c,
                Err(e) => return Response::err(400, &format!("配置无效: {e}")),
            };
            match host.apply_config(cfg) {
                Ok((need_sentence, teardown)) => {
                    if need_sentence {
                        // 【卡死修复】后台重建整句（旧 setup_sentence 持锁
                        // 载 546MB 模型秒级卡全机打字）
                        reload_sentence_bg(teardown, true);
                    }
                    Response::json(&serde_json::json!({"ok": true}))
                }
                Err(e) => Response::err(500, &format!("应用失败: {e}")),
            }
        }
        ("GET", "/api/skins") => {
            let list: Vec<serde_json::Value> = host
                .list_skins()
                .into_iter()
                .map(|(id, name)| serde_json::json!({"id": id, "name": name}))
                .collect();
            Response::json(&serde_json::json!({
                "skins": list,
                "current": host.engine.config.appearance.skin,
            }))
        }
        ("GET", "/api/skin") => {
            let id = req
                .query
                .get("id")
                .cloned()
                .unwrap_or_else(|| host.engine.config.appearance.skin.clone());
            let p = host.skins_dir().join(format!("{id}.json"));
            match hufu_skin::Skin::load(&p) {
                Ok(mut s) => {
                    // 【id 不变量】返回体 id 强制=请求 id（文件名）。皮肤
                    // json 内 id 曾批量写错（gen5 模板 id 未换）——设置页
                    // 按 GET 的 id 回存 POST，若放行错 id 会把 A 皮肤写进
                    // B 文件（墨岩被暮山紫顶掉的事故链）。
                    s.id = id.clone();
                    Response::json(&serde_json::to_value(&s).unwrap())
                }
                Err(e) => {
                    // 皮肤 JSON 有错时明确指认（此前静默回默认皮，用户只见「不生效」）
                    eprintln!("皮肤 {id} 加载失败（回退默认）: {e}");
                    let s = hufu_skin::Skin::default();
                    Response::json(&serde_json::to_value(&s).unwrap())
                }
            }
        }
        ("POST", "/api/skin/select") => {
            // 仅切换当前皮肤：不写皮肤文件（POST /api/skin 是「保存」语义，
            // 误发 {id:...} 会把目标皮肤覆盖成全默认——曾经的静默毁档事故）
            let id = req
                .json()
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                return Response::err(400, "缺少 id");
            }
            let p = host.skins_dir().join(format!("{id}.json"));
            if !p.exists() {
                return Response::err(404, &format!("皮肤 {id} 不存在"));
            }
            host.engine.config.appearance.skin = id.clone();
            let _ = host.engine.config.save(&host.config_path);
            Response::json(&serde_json::json!({"ok": true, "id": id}))
        }
        ("POST", "/api/preview") => {
            // 【实机预览锚点 2026-09-08】设置页报来自己窗口的屏幕坐标
            //（浏览器 window.screenX/outerWidth 可得），2.5s 有效期内
            // pipe state 携带 → DLL 预览候选窗弹在设置窗中心而非陈旧
            // 光标处（用户实测「弹在屏幕中间位置不对」的修复）。
            let v = req.json();
            let x = v.get("x").and_then(|x| x.as_i64()).unwrap_or(0);
            let y = v.get("y").and_then(|x| x.as_i64()).unwrap_or(0);
            host.preview_anchor = Some(((x, y), std::time::Instant::now() + std::time::Duration::from_millis(2500)));
            Response::json(&serde_json::json!({"ok": true}))
        }
        ("POST", "/api/skin/reset") => {
            // 【每皮肤恢复默认 2026-09-08】官方皮肤按 id 恢复各自出厂
            // （内嵌 official-skins 整文件写回）；非官方 id（用户自建）
            // 返回 404，UI 回退统一出厂常量。返回恢复后的皮肤全量 JSON。
            let id = req
                .json()
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                return Response::err(400, "缺少 id");
            }
            match host.reset_official_skin(&id) {
                Some(s) => {
                    let _ = host.engine.config.save(&host.config_path);
                    Response::json(&serde_json::to_value(&s).unwrap())
                }
                None => Response::err(404, &format!("皮肤 {id} 非官方皮肤（无出厂配置）")),
            }
        }
        ("POST", "/api/skin") => {
            let v = req.json();
            let skin: hufu_skin::Skin = match serde_json::from_value(v) {
                Ok(s) => s,
                Err(e) => return Response::err(400, &format!("皮肤无效: {e}")),
            };
            let p = host.skins_dir().join(format!("{}.json", skin.id));
            match skin.save(&p) {
                Ok(()) => {
                    host.engine.config.appearance.skin = skin.id.clone();
                    let _ = host.engine.config.save(&host.config_path);
                    // 皮肤版本 +1：DLL poll 发现变化即强制重拉（绕过
                    // 2.5s 缓存）——设置页调参实机预览即时生效
                    host.skin_ver += 1;
                    Response::json(&serde_json::json!({"ok": true, "id": skin.id}))
                }
                Err(e) => Response::err(500, &format!("保存失败: {e}")),
            }
        }
        ("POST", "/api/weasel_import") => {
            // body: { "id": "...", "colors": { weasel 字段 } }
            let v = req.json();
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("imported");
            match hufu_skin::Skin::from_weasel_colors(id, v.get("colors").unwrap_or(&v)) {
                Some(skin) => Response::json(&serde_json::to_value(&skin).unwrap()),
                None => Response::err(400, "导入失败"),
            }
        }
        ("POST", "/api/sentence_test") => {
            let raw = req
                .json()
                .get("raw")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let dec = host.engine.sentence_decoder().cloned();
            match dec {
                Some(d) => {
                    let cands = d.decode(&raw);
                    let texts: Vec<String> = cands.iter().map(|c| c.text.clone()).collect();
                    Response::json(&serde_json::json!({"candidates": texts}))
                }
                None => Response::err(400, "整句引擎未加载"),
            }
        }
        ("GET", "/api/user_words") => {
            let ud = &host.engine.schema.user_dict;
            let words: Vec<serde_json::Value> = ud
                .entries
                .iter()
                .map(|e| serde_json::json!({"code": e.code, "text": e.text}))
                .collect();
            Response::json(&serde_json::json!({"words": words}))
        }
        ("GET", "/api/code_preview") => {
            // /jc 加词窗「编码框」实时预览：该编码当前最终候选序
            //（码表 + 调整回放 + 用户词含选重位——所见即所得），供
            // 用户参考着填选重位。?code=xxx，限前 10。
            let code = req
                .query
                .get("code")
                .cloned()
                .unwrap_or_default();
            let texts: Vec<String> = host
                .engine
                .schema
                .candidates(&code)
                .iter()
                .take(10)
                .map(|e| e.text.clone())
                .collect();
            Response::json(&serde_json::json!({"texts": texts}))
        }
        ("POST", "/api/user_word/add") => {
            let v = req.json();
            let code = v.get("code").and_then(|x| x.as_str()).unwrap_or("").trim();
            let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").trim();
            // 选重位（/jc 第三框「第 N 选」）：≥1 时词固定第 N 候选
            //（不足 N 个则排最后）；0/缺省=原置顶行为。
            let pos = v.get("pos").and_then(|x| x.as_i64()).unwrap_or(0);
            if code.is_empty() || text.is_empty() {
                return Response::err(400, "编码与词不能为空");
            }
            if !(0..=99).contains(&pos) {
                return Response::err(400, "选重位须在 1-99");
            }
            // 【格式统一 2026-09-06】统一 用户调整.txt：{添加}码\t词[\tpN]。
            // 同码同词旧行全清（含 {删除}——加词=明确想要它，修复加词
            // 被旧删除行屏蔽的问题）后追加。
            let file = host.engine.schema.dir.join("用户调整.txt");
            rewrite_keep_lines(&file, code, text);
            let line = if pos >= 1 {
                format!("{{添加}}{code}\t{text}\tp{pos}\n")
            } else {
                format!("{{添加}}{code}\t{text}\n")
            };
            use std::io::Write;
            let mut f = match std::fs::OpenOptions::new().create(true).append(true).open(&file) {
                Ok(f) => f,
                Err(e) => return Response::err(500, &format!("写入失败: {e}")),
            };
            if let Err(e) = f.write_all(line.as_bytes()) {
                return Response::err(500, &format!("写入失败: {e}"));
            }
            host.engine.reload_user_data();
            Response::json(&serde_json::json!({"ok": true}))
        }
        ("POST", "/api/user_word/weight") => {
            // 【/jq→补充语料 2026-09-06 用户拍板】词+权重 → 写当前方案
            // 的 补充语料.txt：`词 权重`（同词旧行先清）——/jq 的语义
            // 是「提升整句里这个词的概率」，这正是补充语料的职责
            //（ngram 词图注入，奖励 = 9+2·ln(权重/1000)，上限 32）。
            // 补充语料只在模型装载时生效 → 写完后台重载整句引擎
            //（~2s，期间旧模型继续服务）。不再写 用户调整.txt 的
            // {加权} 行（旧行回放兼容保留）。
            let v = req.json();
            let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            let weight = v.get("weight").and_then(|x| x.as_i64()).unwrap_or(1000);
            if text.is_empty() {
                return Response::err(400, "词不能为空");
            }
            if !(1..=1_000_000_000).contains(&weight) {
                return Response::err(400, "权重须为正整数");
            }
            let file = host.engine.schema.dir.join("补充语料.txt");
            rewrite_supplement_lines(&file, &text);
            let line = format!("{text} {weight}\n");
            use std::io::Write;
            let mut f = match std::fs::OpenOptions::new().create(true).append(true).open(&file) {
                Ok(f) => f,
                Err(e) => return Response::err(500, &format!("写入失败: {e}")),
            };
            if let Err(e) = f.write_all(line.as_bytes()) {
                return Response::err(500, &format!("写入失败: {e}"));
            }
            // 后台重载整句模型（刷新补充语料快照→重载 ngram→热挂）
            if let Some(h) = HOST_HANDLE.get() {
                spawn_sentence_reload(h.clone(), true);
            }
            Response::json(&serde_json::json!({"ok": true}))
        }
        ("POST", "/api/user_word/remove") => {
            let v = req.json();
            let code = v.get("code").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            // 硬删用户词条：清同码同词全部行（词行+调整行）
            let file = host.engine.schema.dir.join("用户调整.txt");
            if file.exists() {
                rewrite_keep_lines(&file, &code, &text);
                host.engine.reload_user_data();
            }
            Response::json(&serde_json::json!({"ok": true}))
        }
        ("POST", "/api/candidate/pin") => {
            let v = req.json();
            let code = v.get("code").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            if code.is_empty() || text.is_empty() {
                return Response::err(400, "编码与词不能为空");
            }
            host.engine.adjust_pin(&code, &text);
            host.session.clear();
            Response::json(&serde_json::json!({"ok": true}))
        }
        ("POST", "/api/candidate/hide") => {
            let v = req.json();
            let code = v.get("code").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            if code.is_empty() || text.is_empty() {
                return Response::err(400, "编码与词不能为空");
            }
            host.engine.adjust_hide(&code, &text);
            host.session.clear();
            Response::json(&serde_json::json!({"ok": true}))
        }
        ("GET", "/api/sound") => {
            // 音效预览：?tag=key|select|commit|page → audio/wav
            let tag = req.query.get("tag").cloned().unwrap_or_default();
            let safe = ["key", "select", "commit", "page"];
            if !safe.contains(&tag.as_str()) {
                return Response::err(400, "未知音效");
            }
            let p = host.data_dir.join("音效").join(format!("{tag}.wav"));
            match std::fs::read(&p) {
                Ok(bytes) => Response {
                    status: 200,
                    content_type: "audio/wav",
                    body: bytes,
                },
                Err(_) => Response::err(404, "音效文件不存在"),
            }
        }
        ("GET", "/api/export") => {
            // 全量用户数据快照：配置 + 当前方案用户词 + 调整日志
            let schema_dir = host.engine.schema.dir.clone();
            let read = |name: &str| -> String {
                std::fs::read_to_string(schema_dir.join(name)).unwrap_or_default()
            };
            let stamp = {
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                format!("{}-{secs}", hufu_engine::dynamic::date_string_iso())
            };
            Response::json(&serde_json::json!({
                "schema": host.engine.schema.name,
                "exported_at_unix": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs()).unwrap_or(0),
                "config": host.engine.config,
                "user_words_txt": read("用户词.txt"),
                "adjust_txt": read("用户调整.txt"),
                "stamp": stamp,
            }))
        }
        ("POST", "/api/schema") => {
            let name = req
                .json()
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            match host.engine.switch_schema(&name) {
                Ok(()) => {
                    host.session.clear();
                    // 【卡死修复】旧 setup_sentence 持锁同步载模型（秒级
                    // 卡全机）——改后台重建（先拆旧防跨方案词典串扰）
                    reload_sentence_bg(true, false);
                    let _ = host.engine.config.save(&host.config_path);
                    Response::json(&serde_json::json!({"ok": true, "current": name}))
                }
                Err(e) => Response::err(500, &format!("切换失败: {e}")),
            }
        }
        ("POST", "/api/open_schema_dir") => {
            // body {name?}：缺省=当前方案。打开方案码表目录的资源管理器窗口。
            let name = req
                .json()
                .get("name")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(&host.engine.config.schema.current)
                .to_string();
            let dir = hufu_engine::Engine::resolve_data_sub(
                &host.data_dir,
                &host.engine.config.schema.dir,
            )
            .join(&name);
            if !dir.is_dir() {
                return Response::err(404, &format!("方案目录不存在: {name}"));
            }
            let _ = std::process::Command::new("explorer").arg(&dir).spawn();
            Response::json(&serde_json::json!({"ok": true, "path": dir}))
        }
        ("POST", "/api/export_schema") => {
            // body {name?}：缺省=当前方案。导出用户调整合并后的完整码表
            //（虎爪码表导出同格式）到 数据\码表导出\<方案名> <时间戳>.txt。
            let name = req
                .json()
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            match host.export_schema(if name.is_empty() { None } else { Some(&name) }) {
                Ok((path, n)) => {
                    // 导出即达：explorer 打开导出子文件夹（码表导出\<方案名>\）
                    if let Some(dir) = std::path::Path::new(&path).parent() {
                        if dir.is_dir() {
                            let _ = std::process::Command::new("explorer")
                                .arg(dir)
                                .spawn();
                        }
                    }
                    Response::json(&serde_json::json!({
                        "ok": true, "path": path, "lines": n
                    }))
                }
                Err(e) => Response::err(500, &format!("导出失败: {e}")),
            }
        }
        ("POST", "/api/shutdown") => {
            // 源校验已在 route 入口完成（跨源网页 403）；此处只剩本机
            // 设置页/托盘语义的合法调用
            std::process::exit(0);
        }
        _ => Response::err(404, "not found"),
    }
}

/// Windows 原生小件：单实例互斥 + DPI 感知声明（零依赖 extern）。
#[cfg(windows)]
mod sys_win {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(sa: *const core::ffi::c_void, initial: i32, name: *const u16) -> isize;
        fn GetLastError() -> u32;
        fn SetProcessDpiAwarenessContext(value: *mut core::ffi::c_void) -> i32;
        fn SetProcessDpiAwareness(value: u32) -> i32;
    }
    const ERROR_ALREADY_EXISTS: u32 = 183;

    /// 已有实例在跑（命名互斥体命中）→ true。句柄故意不关：进程存续
    /// 期间互斥体必须持有；退出时系统自动回收。
    pub fn already_running() -> bool {
        let name: Vec<u16> = "HuFu-IME-Server-Single-Instance\0"
            .encode_utf16()
            .collect();
        unsafe {
            let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            // 句柄无效（极端：句柄表满）当「未命中」走老路：端口 bind 兜底
            h != 0 && GetLastError() == ERROR_ALREADY_EXISTS
        }
    }

    /// Per-Monitor V2（Win10 1703+）；旧系统回落 per-monitor v1。
    pub fn declare_per_monitor_dpi() {
        unsafe {
            // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = ((HANDLE)-4)
            let pm_v2: *mut core::ffi::c_void = -4isize as *mut core::ffi::c_void;
            if SetProcessDpiAwarenessContext(pm_v2) == 0 {
                // PROCESS_PER_MONITOR_DPI_AWARE = 2
                let _ = SetProcessDpiAwareness(2);
            }
        }
    }
}
