//! Unix socket 回归电池（Linux/macOS）：协议帧 + 键流 + 方案 + 音效 + HTTP。
//!
//! 用法（先起一个专用 hufu-server，避免污染日常安装）：
//! ```sh
//! XDG_RUNTIME_DIR=/tmp/hufu-battery engine/target/release/hufu-server \
//!     --data _tmp/battery/数据 --port 4393 &
//! XDG_RUNTIME_DIR=/tmp/hufu-battery \
//!     cargo run --release -p hufu-cli --example socketbattery
//! ```
//! 断言全部从引擎实时回包推导（不硬编码候选文本），换码表也能跑。

#[cfg(unix)]
fn main() {
    imp::run();
}

#[cfg(not(unix))]
fn main() {
    eprintln!("socketbattery 仅支持 Unix（Linux/macOS）；Windows 用 pipe-*.ps1 电池。");
}

#[cfg(unix)]
mod imp {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;

    struct Ctx {
        sock: PathBuf,
        port: u16,
        pass: u32,
        fail: u32,
    }

    impl Ctx {
        fn check(&mut self, name: &str, ok: bool, detail: &str) {
            if ok {
                self.pass += 1;
                println!("  ✓ {name}");
            } else {
                self.fail += 1;
                println!("  ✗ {name}  —— {detail}");
            }
        }

        fn call(&mut self, req: serde_json::Value) -> serde_json::Value {
            let body = serde_json::to_vec(&req).expect("序列化");
            let mut s = UnixStream::connect(&self.sock).expect("连接 socket（server 起了吗？）");
            s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            s.write_all(&(body.len() as u32).to_le_bytes()).unwrap();
            s.write_all(&body).unwrap();
            let mut head = [0u8; 4];
            s.read_exact(&mut head).unwrap();
            let n = u32::from_le_bytes(head) as usize;
            let mut buf = vec![0u8; n];
            s.read_exact(&mut buf).unwrap();
            serde_json::from_slice(&buf).expect("响应 JSON")
        }

        /// 一次按键，返回 outcome。
        fn key(&mut self, k: &str) -> serde_json::Value {
            let r = self.call(serde_json::json!({"op": "key", "key": k}));
            r.get("outcome").cloned().unwrap_or(serde_json::Value::Null)
        }

        fn raw(&mut self) -> String {
            let r = self.call(serde_json::json!({"op": "state"}));
            r["state"]["raw"].as_str().unwrap_or("").to_string()
        }

        fn candidates(&mut self) -> Vec<String> {
            let r = self.call(serde_json::json!({"op": "state"}));
            r["state"]["candidates"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c["text"].as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        }

        fn reset(&mut self) {
            let _ = self.call(serde_json::json!({"op": "reset"}));
        }

        fn http(&self, path: &str) -> (u16, String) {
            let mut s = TcpStream::connect(("127.0.0.1", self.port)).expect("HTTP 连接");
            let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
            s.write_all(req.as_bytes()).unwrap();
            let mut buf = String::new();
            let _ = s.read_to_string(&mut buf);
            let status = buf
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let body = buf.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
            (status, body)
        }
    }

    pub fn run() {
        let sock = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("hufu-ime.sock");
        let port: u16 = std::env::var("HUFU_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4393);
        let mut c = Ctx {
            sock: sock.clone(),
            port,
            pass: 0,
            fail: 0,
        };
        println!("socket={} port={port}", sock.display());

        // ── 协议 ──
        let r = c.call(serde_json::json!({"op": "ping"}));
        c.check("1. ping", r["ok"].as_bool() == Some(true), &r.to_string());
        let r = c.call(serde_json::json!({"op": "不存在"}));
        c.check("2. 未知 op 报错", r.get("error").is_some(), &r.to_string());

        // ── 键流：编码 → 候选 → 数字选重上屏 ──
        c.reset();
        let o = c.key("u");
        let cands = c.candidates();
        c.check(
            "3. u 出候选且 consumed",
            o["consumed"].as_bool() == Some(true) && !cands.is_empty(),
            &format!("consumed={} cands={cands:?}", o["consumed"]),
        );
        let first = cands.first().cloned().unwrap_or_default();
        c.reset();
        let _ = c.key("u");
        let o = c.key("1");
        c.check(
            "4. 数字选重上屏首选",
            o["commit"].as_str() == Some(first.as_str()),
            &format!("commit={:?} 期望={first}", o["commit"]),
        );

        // ── 键流：次选键（;）──
        c.reset();
        let _ = c.key("u");
        let cands = c.candidates();
        if cands.len() >= 2 {
            let second = cands[1].clone();
            c.reset();
            let _ = c.key("u");
            let o = c.key(";");
            c.check(
                "5. ; 次选上屏第 2 候选",
                o["commit"].as_str() == Some(second.as_str()),
                &format!("commit={:?} 期望={second}", o["commit"]),
            );
        } else {
            c.check("5. ; 次选（数据无重码，跳过）", true, "");
        }

        // ── 键流：退格 / Esc ──
        c.reset();
        let _ = c.key("t");
        let _ = c.key("i");
        let _ = c.key("backspace");
        let raw = c.raw();
        c.check("6. 退格回删编码", raw == "t", &raw);
        let _ = c.key("escape");
        let raw = c.raw();
        c.check("7. Esc 清空编码", raw.is_empty(), &raw);

        // ── 顶屏：第 5 键触发前串上屏 ──
        c.reset();
        let mut committed: Option<String> = None;
        for k in ["t", "u", "j", "a", "g"] {
            let o = c.key(k);
            if let Some(t) = o["commit"].as_str() {
                if !t.is_empty() {
                    committed = Some(t.to_string());
                    break;
                }
            }
        }
        c.check(
            "8. 第 5 键顶屏上屏",
            committed.is_some(),
            "5 键内无 commit（数据可能是整句？）",
        );
        c.reset();

        // ── 空格上屏 ──
        let _ = c.key("u");
        let o = c.key("space");
        c.check(
            "9. 空格上屏当前首选",
            o["commit"].as_str().is_some_and(|s| !s.is_empty()),
            &o.to_string(),
        );

        // ── 方案：列表 / 切换 ──
        c.reset();
        let r = c.call(serde_json::json!({"op": "schemas"}));
        let names: Vec<String> = r["schemas"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let cur = r["current"].as_str().unwrap_or("").to_string();
        c.check(
            "10. 方案列表非空且含当前",
            !names.is_empty() && names.contains(&cur),
            &format!("{names:?} current={cur}"),
        );
        if names.len() >= 2 {
            let other = names.iter().find(|n| **n != cur).cloned().unwrap();
            let r = c.call(serde_json::json!({"op": "set_schema", "name": other}));
            c.check(
                "11. 切换方案生效",
                r["current"].as_str() == Some(other.as_str()),
                &r.to_string(),
            );
            let r = c.call(serde_json::json!({"op": "set_schema", "name": cur}));
            c.check(
                "12. 切回原方案",
                r["current"].as_str() == Some(cur.as_str()),
                &r.to_string(),
            );
        }

        // ── 音效开关（读态 → 翻转 → 还原）──
        let r = c.call(serde_json::json!({"op": "sound_state"}));
        let before = r["enabled"].as_bool().unwrap_or(false);
        let r = c.call(serde_json::json!({"op": "sound_toggle"}));
        let flipped = r["enabled"].as_bool().unwrap_or(before);
        c.check(
            "13. 音效开关翻转",
            flipped != before,
            &format!("before={before} after={flipped}"),
        );
        let _ = c.call(serde_json::json!({"op": "sound_toggle"})); // 还原

        // ── 翻页（默认 -/=/[/] 等由配置决定；引擎消费）──
        c.reset();
        let _ = c.key("t");
        let _ = c.key("i");
        let page0_first = c.candidates().first().cloned().unwrap_or_default();
        let _ = c.key("=");
        let st = c.call(serde_json::json!({"op": "state"}));
        let page1 = st["state"]["page"].as_u64().unwrap_or(0);
        let page1_first = c.candidates().first().cloned().unwrap_or_default();
        c.check(
            "19. = 翻到第 2 页且候选变化",
            page1 == 1 && !page1_first.is_empty() && page1_first != page0_first,
            &format!("page={page1} first={page1_first} (原 {page0_first})"),
        );
        let _ = c.key("-");
        let st = c.call(serde_json::json!({"op": "state"}));
        c.check(
            "20. - 翻回第 1 页",
            st["state"]["page"].as_u64() == Some(0),
            &st["state"]["page"].to_string(),
        );

        // ── 三选键（'）──
        c.reset();
        let _ = c.key("t");
        let _ = c.key("i");
        let cands = c.candidates();
        if cands.len() >= 3 {
            let third = cands[2].clone();
            c.reset();
            let _ = c.key("t");
            let _ = c.key("i");
            let o = c.key("'");
            c.check(
                "21. ' 三选上屏第 3 候选",
                o["commit"].as_str() == Some(third.as_str()),
                &format!("commit={:?} 期望={third}", o["commit"]),
            );
        } else {
            c.check("21. ' 三选（数据候选不足，跳过）", true, "");
        }
        c.reset();

        // ── Linux 策略：引擎不自带中英切换（英文输入交给 fcitx5 键盘布局）──
        let (_, cfg_body) = c.http("/api/config");
        let shift_off = serde_json::from_str::<serde_json::Value>(&cfg_body)
            .ok()
            .and_then(|v| v["general"]["shift_switch"].as_bool())
            .map(|v| !v)
            .unwrap_or(false);
        if shift_off {
            let _ = c.call(serde_json::json!({"op": "reset"}));
            let before = c.call(serde_json::json!({"op": "state"}))["state"]["chinese"]
                .as_bool()
                .unwrap_or(true);
            let _ = c.key("shift");
            let _ = c.key("capslock");
            let after = c.call(serde_json::json!({"op": "state"}))["state"]["chinese"]
                .as_bool()
                .unwrap_or(true);
            c.check(
                "22. Shift/Caps 不切中英（Linux 策略）",
                before == after,
                &format!("chinese {before} → {after}"),
            );
        } else {
            c.check("22. Shift/Caps 不切中英（配置未关闭，跳过）", true, "");
        }

        // ── 鼠标点击候选（select op：页内下标，语义同数字选重）──
        c.reset();
        let _ = c.key("t");
        let _ = c.key("i");
        let cands = c.candidates();
        if !cands.is_empty() {
            let pick = cands[0].clone();
            let r = c.call(serde_json::json!({"op": "select", "index": 0}));
            c.check(
                "23. select 点击候选上屏（页内下标）",
                r["outcome"]["commit"].as_str() == Some(pick.as_str()),
                &format!("commit={:?} 期望={pick}", r["outcome"]["commit"]),
            );
        } else {
            c.check("23. select（数据无候选，跳过）", true, "");
        }
        c.reset();

        // ── 焦点/reset 幂等 ──
        let _ = c.key("t");
        let _ = c.call(serde_json::json!({"op": "focus"}));
        let raw = c.raw();
        c.check("14. focus 清态", raw.is_empty(), &raw);
        let _ = c.call(serde_json::json!({"op": "reset"}));
        let raw = c.raw();
        c.check("15. reset 幂等", raw.is_empty(), &raw);

        // ── HTTP：设置页 / state / schemas ──
        let (code, body) = c.http("/api/state");
        c.check("16. GET /api/state 200", code == 200 && body.contains("raw"), &body[..body.len().min(80)]);
        let (code, body) = c.http("/api/schemas");
        c.check(
            "17. GET /api/schemas 200",
            code == 200 && body.contains("\"schemas\""),
            &body[..body.len().min(80)],
        );
        let (code, body) = c.http("/");
        c.check(
            "18. GET / 设置页",
            code == 200 && body.to_lowercase().contains("<!doctype") || body.contains("<html"),
            &format!("{code} {}B", body.len()),
        );

        println!("\n结果: {} PASS / {} FAIL", c.pass, c.fail);
        std::process::exit(if c.fail == 0 { 0 } else { 1 });
    }
}
