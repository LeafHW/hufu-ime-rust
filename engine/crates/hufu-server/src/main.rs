//! hufu-server —— 守护进程：引擎宿主 + HTTP 设置界面 + 前端 IPC。
//!
//! 用法：hufu-server [--data <目录>] [--port <端口>]
//! 默认数据目录：./hufu-data（或环境变量 HUFU_DATA）；默认端口 4390。

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

fn main() {
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
    let shared = Arc::new(Mutex::new(host));
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
    // （词典模式），整句能力稍后自动就位——修「装完要等好久才能
    // 正常打字」：管道不再被模型加载阻塞。
    {
        let shared_bg = shared.clone();
        std::thread::Builder::new()
            .name("hufu-ngram-load".into())
            .spawn(move || {
                let t0 = std::time::Instant::now();
                let plan = {
                    let h = shared_bg.lock().unwrap();
                    h.sentence_load_plan()
                };
                let Some((path, dict, supplement, weights)) = plan else {
                    return;
                };
                match hufu_sentence::SentenceEngine::load(&path, dict, &supplement, weights) {
                    Ok(dec) => {
                        let mut h = shared_bg.lock().unwrap();
                        // 装载期间用户可能切方案/关整句：只在仍满足
                        // 门控时挂载，否则弃用本次结果
                        if h.engine.config.schema.current.contains("整句")
                            && h.engine.config.sentence.enabled
                        {
                            h.engine.set_sentence_decoder(Some(std::sync::Arc::new(dec)));
                            eprintln!(
                                "整句引擎已加载（后台 {:.1}s）: {}",
                                t0.elapsed().as_secs_f32(),
                                path.display()
                            );
                        }
                    }
                    Err(e) => eprintln!("整句模型后台加载失败: {e}"),
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
                        std::process::Command::new(exe).arg(&app_arg).spawn().is_ok()
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
                Ok(s) => Response::json(&serde_json::to_value(&s).unwrap()),
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
        ("POST", "/api/user_word/add") => {
            let v = req.json();
            let code = v.get("code").and_then(|x| x.as_str()).unwrap_or("").trim();
            let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").trim();
            if code.is_empty() || text.is_empty() {
                return Response::err(400, "编码与词不能为空");
            }
            let file = host.engine.schema.dir.join("用户词.txt");
            let line = format!("{code}\t{text}\n");
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
        ("POST", "/api/user_word/remove") => {
            let v = req.json();
            let code = v.get("code").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            let file = host.engine.schema.dir.join("用户词.txt");
            if file.exists() {
                let content = match std::fs::read_to_string(&file) {
                    Ok(c) => c,
                    Err(e) => return Response::err(500, &format!("读取失败: {e}")),
                };
                let kept: String = content
                    .lines()
                    .filter(|l| {
                        let mut it = l.split('\t');
                        let c = it.next().unwrap_or("").trim();
                        let t = it.next().unwrap_or("").trim();
                        !(t == text && c == code)
                    })
                    .map(|l| format!("{l}\n"))
                    .collect();
                if let Err(e) = std::fs::write(&file, kept.as_bytes()) {
                    return Response::err(500, &format!("写入失败: {e}"));
                }
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
        ("POST", "/api/shutdown") => {
            std::process::exit(0);
        }
        _ => Response::err(404, "not found"),
    }
}
