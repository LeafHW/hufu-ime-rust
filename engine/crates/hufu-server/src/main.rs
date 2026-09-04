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
    let host = match Host::new(&data_dir) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("引擎初始化失败: {e}");
            std::process::exit(1);
        }
    };
    {
        // 【性能插桩】main 侧总戳（与 host.rs 的 Host::new 打点配套）
        use std::io::Write;
        let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(r"C:\ProgramData\HuFu\diag\startup-trace.txt")
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
        std::thread::Builder::new()
            .name("hufu-reverse-warm".into())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let mut h = shared_bg.lock().unwrap();
                h.engine.ensure_reverse();
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
                let _ = std::fs::remove_file(std::env::current_dir().unwrap_or_default().join("server.pid"));
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
/// 'static 句柄——main 里登记，API 里取用。
static HOST_HANDLE: std::sync::OnceLock<std::sync::Arc<std::sync::Mutex<Host>>> =
    std::sync::OnceLock::new();

/// 整句模型后台装载（启动与 /jq 加权后共用）：短锁取装载计划
///（resupplement=true 时先重读 补充语料.txt 刷新内存快照——补充语料
/// 只在模型装载时生效，写入后必须重载模型），不持锁载 ngram
///（page cache 热时 ~2s），载完短锁热挂。期间管道/设置页/打字照常
///（旧模型继续服务）。
fn spawn_sentence_reload(
    shared: std::sync::Arc<std::sync::Mutex<Host>>,
    resupplement: bool,
) {
    std::thread::Builder::new()
        .name("hufu-ngram-load".into())
        .spawn(move || {
            let t0 = std::time::Instant::now();
            let plan = {
                let mut h = shared.lock().unwrap();
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
                return;
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
                    let mut h = shared.lock().unwrap();
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
            // 方案列表 = 码表目录的子目录名（含可读名字则更佳，先给目录名）
            let dir = host.data_dir.join(&host.engine.config.schema.dir);
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
        ("POST", "/api/config") => {
            let v = req.json();
            let cfg: hufu_config::Config = match serde_json::from_value(v) {
                Ok(c) => c,
                Err(e) => return Response::err(400, &format!("配置无效: {e}")),
            };
            match host.apply_config(cfg) {
                Ok(()) => Response::json(&serde_json::json!({"ok": true})),
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
                    host.setup_sentence();
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
            let dir = host
                .data_dir
                .join(&host.engine.config.schema.dir)
                .join(&name);
            if !dir.is_dir() {
                return Response::err(404, &format!("方案目录不存在: {name}"));
            }
            let _ = std::process::Command::new("explorer").arg(&dir).spawn();
            Response::json(&serde_json::json!({"ok": true, "path": dir}))
        }
        ("POST", "/api/shutdown") => {
            std::process::exit(0);
        }
        _ => Response::err(404, "not found"),
    }
}
