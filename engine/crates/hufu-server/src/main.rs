//! hufu-server —— 守护进程：引擎宿主 + HTTP 设置界面 + 前端 IPC。
//!
//! 用法：hufu-server [--data <目录>] [--port <端口>]
//! 默认数据目录：./hufu-data（或环境变量 HUFU_DATA）；默认端口 4390。

mod host;
mod http;

use host::{parse_key, Host};
use http::{Request, Response};
use std::path::PathBuf;
use std::sync::Mutex;

const INDEX_HTML: &str = include_str!("../../../../settings-ui/index.html");

fn main() {
    let mut args = std::env::args().skip(1);
    let mut data_dir = std::env::var("HUFU_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("hufu-data"));
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
    let host = Mutex::new(host);
    let addr = format!("127.0.0.1:{port}");

    let handler = move |req: &Request| -> Response { route(&host, req) };
    if let Err(e) = http::serve(&addr, std::sync::Arc::new(handler)) {
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
                Err(_) => {
                    let s = hufu_skin::Skin::default();
                    Response::json(&serde_json::to_value(&s).unwrap())
                }
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
