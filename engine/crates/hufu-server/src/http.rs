//! 极简 HTTP/1.1 服务（仅 localhost 设置界面 + 本地 API，线程每连接）。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub body: Vec<u8>,
    /// 全部请求头（小写键）。【浏览器源加固 2026-09-11】副作用端点
    /// 校验 Origin/Host 用——此前头部解析后被丢弃，路由无从设防。
    pub headers: HashMap<String, String>,
}

impl Request {
    pub fn json(&self) -> serde_json::Value {
        // 容错：剥 UTF-8 BOM（Windows 工具链常见）
        let body = if self.body.starts_with(&[0xEF, 0xBB, 0xBF]) {
            &self.body[3..]
        } else {
            &self.body
        };
        serde_json::from_slice(body).unwrap_or(serde_json::Value::Null)
    }
}

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(v: &serde_json::Value) -> Response {
        Response {
            status: 200,
            content_type: "application/json; charset=utf-8",
            body: serde_json::to_vec(v).unwrap_or_default(),
        }
    }
    pub fn err(status: u16, msg: &str) -> Response {
        Response::json(&serde_json::json!({ "error": msg })).with_status(status)
    }
    pub fn with_status(mut self, status: u16) -> Response {
        self.status = status;
        self
    }
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        // 【SY-02 2026-11-01】云端删除超时返回 202（Accepted，仍在后台执行）
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        403 => "Forbidden",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

/// 【审计 P2-E 2026-11-05】带总量上限的按行读：超限即断连（None）。
/// 防超长请求行/header 单行灌爆内存（read_line 本身无上限）。
fn read_line_bounded(reader: &mut BufReader<TcpStream>, limit: usize) -> Option<String> {
    let mut buf: Vec<u8> = Vec::new();
    reader
        .by_ref()
        .take(limit as u64 + 1)
        .read_until(b'\n', &mut buf)
        .ok()?;
    if buf.len() > limit {
        return None;
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// 【审计 P2-E 2026-11-05】单请求护栏：
/// - 请求行 ≤ 8KB、header ≤ 128 行 × 16KB、body ≤ 16MiB（此前声明多大
///   预分配多大——声明 4GB 即 vec![0u8; 4GB]，本机恶意网页/进程可内存
///   耗尽打崩输入法）；settings_import 全量导出含皮肤实测远小于 16MiB。
/// - 每次底层读 15s 超时（此前慢连接可无限占住每连接一线程）。
/// 任一超限：断连（None），不回 413——攻击者不配得到响应。
fn read_request(stream: &mut TcpStream) -> Option<Request> {
    // 【F5 复审修复 2026-11-05】单次读 15s 上限拦不住 slow-loris（每 ~14s
    // 挤 1 字节可无限占线程）。加总 deadline：整个请求（行+头+体）自首读起
    // 30s 内必须完成，任一阶段越线即断连。每轮把单次读超时收紧为
    // min(15s, 剩余预算)，两头都封死。
    const TOTAL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
    let started = std::time::Instant::now();
    let budget = || TOTAL_DEADLINE.checked_sub(started.elapsed());
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(15)));
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let line = read_line_bounded(&mut reader, 8 * 1024)?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = HashMap::new();
    for _ in 0..128 {
        let _ = stream.set_read_timeout(Some(budget()?));
        let h = read_line_bounded(&mut reader, 16 * 1024)?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    const MAX_BODY: usize = 16 * 1024 * 1024;
    if len > MAX_BODY {
        return None;
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        let _ = stream.set_read_timeout(Some(budget()?));
        reader.read_exact(&mut body).ok()?;
    }
    let (path, query_str) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    let mut query = HashMap::new();
    for kv in query_str.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        query.insert(url_decode(k), url_decode(v));
    }
    Some(Request {
        method,
        path,
        query,
        body,
        headers,
    })
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() + 1 && i + 2 < bytes.len() + 1 => {
                if i + 2 < bytes.len() {
                    if let Ok(b) = u8::from_str_radix(
                        std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"),
                        16,
                    ) {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                }
                out.push(bytes[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn write_response(stream: &mut TcpStream, resp: &Response) {
    // 【F5】写侧同样封时：慢读客户端可阻塞 write_all 无限占线程。
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(15)));
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        resp.status,
        status_text(resp.status),
        resp.content_type,
        resp.body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&resp.body);
    let _ = stream.flush();
}

/// 读 handler panic 的载荷（&str / String），供日志与 500 响应使用。
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// accept 连续错误退避：50、100、200、400、800、1000ms（封顶 1s）。
fn accept_backoff_ms(errors: u32) -> u64 {
    let shift = errors.saturating_sub(1).min(5);
    (50u64 << shift).min(1000)
}

/// 阻塞运行；handler 返回 Response。
/// 【2026-11-02 P2】改为接**已 bind 的 listener**：调用方在端口探测时
/// 一次 bind 成功就把监听器交进来，消除“探测 bind→drop→serve 再 bind”
/// 的 TOCTOU，也避免探测成功但真正 bind 失败时的静默必败。
///
/// 【P1-9 / P2-16 2026-11-04】
/// - accept 错误不再裸 `continue` 空转：计数 + 指数退避（50ms 起、上限
///   1s），成功接一个连接即复位并记日志。Windows 上 listener 被外部
///   干扰时 `incoming()` 会高频返回 Err，旧实现等于忙等烧 CPU。
/// - 每连接的读取与 handler 调用包 `catch_unwind`：handler（尤其 host 锁
///   内的路由）panic 时不再无响应/断连，而是回 500 并继续服务其它连接。
///   Host 互斥锁 panic 后中毒，路由侧已有 `unwrap_or_else(|p| p.into_inner())`
///   兜底，进程不会挂死。
pub fn serve<F>(listener: TcpListener, handler: Arc<F>) -> std::io::Result<()>
where
    F: Fn(&Request) -> Response + Send + Sync + 'static,
{
    if let Ok(addr) = listener.local_addr() {
        eprintln!("HuFu server listening on http://{addr}");
    }
    let mut accept_errors: u32 = 0;
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(s) => {
                if accept_errors > 0 {
                    eprintln!("HTTP accept 恢复正常（此前连续错误 {accept_errors} 次）");
                    accept_errors = 0;
                }
                s
            }
            Err(e) => {
                accept_errors = accept_errors.saturating_add(1);
                let delay_ms = accept_backoff_ms(accept_errors);
                eprintln!(
                    "HTTP accept 错误（连续第 {accept_errors} 次）: {e}；{}ms 后重试",
                    delay_ms
                );
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                continue;
            }
        };
        let handler = handler.clone();
        std::thread::spawn(move || {
            let request = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                read_request(&mut stream)
            }))
            .unwrap_or_else(|payload| {
                eprintln!("HTTP 读取请求 panic: {}", panic_text(&*payload));
                None
            });
            if let Some(req) = request {
                let resp = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    handler(&req)
                }))
                .unwrap_or_else(|payload| {
                    // 不把 panic 细节回给浏览器（可能含路径等），只进日志。
                    eprintln!(
                        "HTTP handler panic（{} {}）: {}",
                        req.method,
                        req.path,
                        panic_text(&*payload)
                    );
                    Response::err(500, "internal error: handler panic")
                });
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    write_response(&mut stream, &resp)
                }));
            }
            let _ = stream.shutdown(std::net::Shutdown::Both);
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::time::Duration;

    fn one_request(addr: &str, path: &str) -> String {
        let mut s = TcpStream::connect(addr).expect("connect");
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        s.write_all(req.as_bytes()).expect("write");
        let mut buf = String::new();
        let _ = s.read_to_string(&mut buf);
        buf
    }

    /// 【P2-16 2026-11-04】handler panic 必须回 500 且服务不中断——
    /// 旧实现 panic 只杀死该连接线程：客户端拿到空响应（fetch 报
    /// "Failed to fetch" / 一直等到超时），设置页表现为「点了没反应」。
    #[test]
    fn handler_panic_returns_500_and_server_survives() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap().to_string();
        let handler = Arc::new(|req: &Request| -> Response {
            if req.path == "/panic" {
                panic!("boom in handler");
            }
            Response::json(&serde_json::json!({"ok": true}))
        });
        std::thread::spawn(move || {
            let _ = serve(listener, handler);
        });
        let panic_resp = one_request(&addr, "/panic");
        assert!(
            panic_resp.starts_with("HTTP/1.1 500"),
            "panic 必须回 500，实际：{panic_resp:?}"
        );
        assert!(
            panic_resp.contains("handler panic"),
            "响应体应说明降级原因：{panic_resp:?}"
        );
        // server 未挂：同一 listener 的后续请求照常
        let ok = one_request(&addr, "/ok");
        assert!(
            ok.starts_with("HTTP/1.1 200") && ok.contains("\"ok\":true"),
            "panic 后服务必须继续：{ok:?}"
        );
    }

    /// 【P1-9 2026-11-04】退避序列有界且递增（旧实现 accept 出错直接
    /// continue 空转，listener 持续报错时等于忙等烧 CPU）。
    #[test]
    fn accept_backoff_is_growing_and_bounded() {
        let seq: Vec<u64> = (1..=8).map(accept_backoff_ms).collect();
        assert_eq!(seq, vec![50, 100, 200, 400, 800, 1000, 1000, 1000]);
        assert_eq!(accept_backoff_ms(0), 50, "计数为 0 也按首档处理");
    }
}
