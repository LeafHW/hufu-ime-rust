//! 虎符（hufu-ime）fcitx5 前端 · Rust 侧（staticlib）。
//!
//! 架构与 Windows TSF / macOS IMK 一致：本层是薄壳，按键经 Unix socket
//! （`$XDG_RUNTIME_DIR/hufu-ime.sock`，4 字节小端长度 + JSON 帧，与
//! Windows 命名管道同一协议）发给 `hufu-server` 的引擎；回包
//! `{outcome, state}` 经宿主回调驱动组段、候选与上屏。
//!
//! C++ 侧（`hufu-addon/shell/hufu.cpp`）经 `hufu_abi.h` 调用本库；
//! 本 crate 同时产出 rlib 供 mock socket 单测。
#![allow(clippy::missing_safety_doc)]

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use hufu_types::{KeyOutcome, SessionState};

/// 帧上限（与 hufu-server `pipe.rs` 的 BUF 一致）。
const BUF: usize = 1 << 20;
/// 单次请求读超时：引擎按键与 state 都是毫秒级；超时按断线处理并重连。
const READ_TIMEOUT: Duration = Duration::from_secs(3);

/// 返回值位：已消费（宿主不应再处理该键）。
pub const HUFU_KEY_CONSUMED: c_int = 0x1;
/// 返回值位：`back`（提交前需回删的已上屏字符数）左移位数。
pub const HUFU_KEY_BACK_SHIFT: u32 = 8;

/// 宿主回调表（C++ 薄壳实现；函数指针可为 NULL）。
///
/// - `commit`：立即上屏文本（UTF-8，NUL 结尾）。
/// - `update`：UI 快照——preedit + raw（UTF-8；raw 空且候选非空=选重闪帧，
///   壳应直接清窗）+ 候选文本/注释/实际上屏文本三个平行数组（各 NUL 结尾；
///   `count==0` 时必须清除候选列表）+ 高亮索引（页内 0 起）+ aux 提示 +
///   中英态（1=中）。回调期指针有效，C++ 侧须同步拷走。
///   上屏文本数组用于「顶字」：`显示=>输出` 覆盖时与显示文本不同。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HufuHost {
    pub user: *mut c_void,
    pub commit: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
    pub update: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *const c_char,
            *const *const c_char,
            *const *const c_char,
            *const *const c_char,
            c_int,
            c_int,
            *const c_char,
            c_int,
        ),
    >,
}

/// 回调期存活的字符串与指针（仅在 `deliver` 内构建/使用）。
#[derive(Default)]
struct Scratch {
    preedit: CString,
    raw: CString,
    aux: CString,
    texts: Vec<CString>,
    comments: Vec<CString>,
    /// 候选的实际上屏文本（commit_override 优先；顶字用）
    commits: Vec<CString>,
    text_ptrs: Vec<*const c_char>,
    comment_ptrs: Vec<*const c_char>,
    commit_ptrs: Vec<*const c_char>,
}

/// 前置声明：默认 socket 路径（`$XDG_RUNTIME_DIR/hufu-ime.sock`，回退 `/tmp`）。
pub fn default_socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("hufu-ime.sock")
}

/// 单会话引擎客户端（惰性连接 + 断线重连）。
pub struct HufuClient {
    sock_path: PathBuf,
    stream: Option<UnixStream>,
    host: HufuHost,
    status: CString,
    scratch: Scratch,
    /// 最近一次 update 的中英态（subMode 用；回调已同步送达 C++）。
    pub chinese: bool,
    /// 最近一次按键的回删数（commit 回调发生在 `key` 返回前，C++ 侧
    /// 经 `hufu_client_last_back` 读取以先回删再上屏）。
    last_back: u8,
}

impl HufuClient {
    pub fn new(sock_path: PathBuf, host: HufuHost) -> Self {
        let status = CString::new(format!("未连接（{}）", sock_path.display()))
            .unwrap_or_default();
        HufuClient {
            sock_path,
            stream: None,
            host,
            status,
            scratch: Scratch::default(),
            chinese: true,
            last_back: 0,
        }
    }

    fn set_status(&mut self, s: impl Into<String>) {
        self.status = CString::new(s.into()).unwrap_or_default();
    }

    pub fn status(&self) -> &CStr {
        &self.status
    }

    /// 惰性连接（已连接则直接返回 true）。
    fn ensure_conn(&mut self) -> bool {
        if self.stream.is_some() {
            return true;
        }
        match UnixStream::connect(&self.sock_path) {
            Ok(s) => {
                let _ = s.set_read_timeout(Some(READ_TIMEOUT));
                self.stream = Some(s);
                true
            }
            Err(e) => {
                self.set_status(format!("连接失败: {e}"));
                false
            }
        }
    }

    /// 一次请求/响应；失败返回 None（断线时清空连接，下次自动重连）。
    fn call(&mut self, req: &serde_json::Value) -> Option<serde_json::Value> {
        if !self.ensure_conn() {
            return None;
        }
        let s = self.stream.as_mut()?;
        match Self::roundtrip(s, req) {
            Ok(v) => Some(v),
            Err(e) => {
                self.stream = None;
                self.set_status(format!("请求失败（已断开）: {e}"));
                None
            }
        }
    }

    fn roundtrip(s: &mut UnixStream, req: &serde_json::Value) -> std::io::Result<serde_json::Value> {
        let body = serde_json::to_vec(req)?;
        if body.len() > BUF {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "请求帧过大",
            ));
        }
        s.write_all(&(body.len() as u32).to_le_bytes())?;
        s.write_all(&body)?;
        let mut head = [0u8; 4];
        s.read_exact(&mut head)?;
        let n = u32::from_le_bytes(head) as usize;
        if n == 0 || n > BUF {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "响应帧长度非法",
            ));
        }
        let mut buf = vec![0u8; n];
        s.read_exact(&mut buf)?;
        serde_json::from_slice(&buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// ping：1=通。
    pub fn ping(&mut self) -> bool {
        self.call(&serde_json::json!({"op": "ping"}))
            .map(|v| v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false))
            .unwrap_or(false)
    }

    /// 一次按键：返回 `(consumed, back)`；回调同步送达 commit/update。
    ///
    /// `line_end`：1=光标在行尾，0=不在，-1=未知（未知不带该字段）。
    pub fn key(
        &mut self,
        key: &str,
        shift: bool,
        ctrl: bool,
        alt: bool,
        meta: bool,
        caps: bool,
        line_end: i32,
    ) -> (bool, u8) {
        let mut req = serde_json::json!({
            "op": "key",
            "key": key,
            "modifiers": {
                "shift": shift, "ctrl": ctrl, "alt": alt,
                "meta": meta, "caps": caps,
            },
        });
        if line_end >= 0 {
            req["line_end"] = serde_json::json!(line_end == 1);
        }
        let Some(resp) = self.call(&req) else {
            return (false, 0);
        };
        let Some(outcome_v) = resp.get("outcome") else {
            return (false, 0);
        };
        let Ok(outcome) = serde_json::from_value::<KeyOutcome>(outcome_v.clone()) else {
            self.set_status("outcome 解析失败");
            return (false, 0);
        };
        // 顶层 state 与 outcome.state 同值（选重闪帧在 outcome.state 里，
        // pipe.rs 已把顶层 state 也置为它）——优先 outcome.state。
        let state = outcome
            .state
            .clone()
            .or_else(|| {
                resp.get("state")
                    .and_then(|v| serde_json::from_value::<SessionState>(v.clone()).ok())
            })
            .unwrap_or_default();
        self.last_back = outcome.back;
        if outcome.consumed || outcome.state.is_some() {
            self.deliver(&outcome, &state);
        }
        (outcome.consumed, outcome.back)
    }

    /// 重置/焦点切换（activate/deactivate）：清引擎会话并同步清 UI。
    pub fn reset(&mut self) {
        self.simple_state_op("reset");
    }

    pub fn focus(&mut self) {
        self.simple_state_op("focus");
    }

    fn simple_state_op(&mut self, op: &str) {
        let Some(resp) = self.call(&serde_json::json!({"op": op})) else {
            return;
        };
        let state = resp
            .get("state")
            .and_then(|v| serde_json::from_value::<SessionState>(v.clone()).ok())
            .unwrap_or_default();
        self.last_back = 0;
        let outcome = KeyOutcome {
            consumed: true,
            ..Default::default()
        };
        self.deliver(&outcome, &state);
    }

    /// 送达 commit + update 回调（C++ 侧同步拷贝）。
    fn deliver(&mut self, outcome: &KeyOutcome, state: &SessionState) {
        if let Some(cb) = self.host.commit {
            if let Some(text) = outcome.commit.as_deref() {
                // 功能词指令不算上屏内容（与 DLL/pipe 同口径）
                if !text.is_empty() && text != "{加词}" && text != "{隐藏候选}" {
                    if let Ok(cs) = CString::new(text) {
                        unsafe { cb(self.host.user, cs.as_ptr()) };
                    }
                }
            }
        }
        // preedit：引擎给了展示串就用它，否则退回 raw（macOS 前端同款）
        let preedit = if state.preedit.is_empty() {
            state.raw.as_str()
        } else {
            state.preedit.as_str()
        };
        self.scratch.preedit = CString::new(preedit).unwrap_or_default();
        self.scratch.raw = CString::new(state.raw.as_str()).unwrap_or_default();
        self.scratch.aux = CString::new(state.aux.as_str()).unwrap_or_default();
        self.scratch.texts = state
            .candidates
            .iter()
            .map(|c| CString::new(c.text.as_str()).unwrap_or_default())
            .collect();
        self.scratch.comments = state
            .candidates
            .iter()
            .map(|c| CString::new(c.comment.as_str()).unwrap_or_default())
            .collect();
        self.scratch.commits = state
            .candidates
            .iter()
            .map(|c| CString::new(c.commit_text()).unwrap_or_default())
            .collect();
        self.scratch.text_ptrs = self.scratch.texts.iter().map(|c| c.as_ptr()).collect();
        self.scratch.comment_ptrs = self.scratch.comments.iter().map(|c| c.as_ptr()).collect();
        self.scratch.commit_ptrs = self.scratch.commits.iter().map(|c| c.as_ptr()).collect();
        self.chinese = state.chinese;
        if let Some(cb) = self.host.update {
            unsafe {
                cb(
                    self.host.user,
                    self.scratch.preedit.as_ptr(),
                    self.scratch.raw.as_ptr(),
                    self.scratch.text_ptrs.as_ptr(),
                    self.scratch.comment_ptrs.as_ptr(),
                    self.scratch.commit_ptrs.as_ptr(),
                    self.scratch.texts.len() as c_int,
                    state.selected as c_int,
                    self.scratch.aux.as_ptr(),
                    if state.chinese { 1 } else { 0 },
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// C ABI（与 shell/hufu_abi.h 一一对应）
// ---------------------------------------------------------------------------

/// 创建客户端；`sock_path` 为 NULL/空串时用默认路径。宿主回调表可 NULL。
/// 引擎未运行时也返回非 NULL（惰性连接，首次按键失败即透传）。
#[no_mangle]
pub extern "C" fn hufu_client_new(
    sock_path: *const c_char,
    host: *const HufuHost,
) -> *mut HufuClient {
    let path = if sock_path.is_null() {
        default_socket_path()
    } else {
        let s = unsafe { CStr::from_ptr(sock_path) }.to_string_lossy();
        if s.is_empty() {
            default_socket_path()
        } else {
            PathBuf::from(s.as_ref())
        }
    };
    let host = if host.is_null() {
        HufuHost {
            user: std::ptr::null_mut(),
            commit: None,
            update: None,
        }
    } else {
        unsafe { *host }
    };
    Box::into_raw(Box::new(HufuClient::new(path, host)))
}

#[no_mangle]
pub extern "C" fn hufu_client_free(c: *mut HufuClient) {
    if !c.is_null() {
        drop(unsafe { Box::from_raw(c) });
    }
}

/// 一次按键：返回位掩码 `HUFU_KEY_CONSUMED | (back << HUFU_KEY_BACK_SHIFT)`。
#[no_mangle]
pub extern "C" fn hufu_client_key(
    c: *mut HufuClient,
    key: *const c_char,
    shift: c_int,
    ctrl: c_int,
    alt: c_int,
    meta: c_int,
    caps: c_int,
    line_end: c_int,
) -> c_int {
    if c.is_null() || key.is_null() {
        return 0;
    }
    let client = unsafe { &mut *c };
    let key = unsafe { CStr::from_ptr(key) }.to_string_lossy().into_owned();
    let (consumed, back) = client.key(
        &key,
        shift != 0,
        ctrl != 0,
        alt != 0,
        meta != 0,
        caps != 0,
        line_end,
    );
    let mut ret = 0;
    if consumed {
        ret |= HUFU_KEY_CONSUMED;
    }
    ret | ((back as c_int) << HUFU_KEY_BACK_SHIFT)
}

#[no_mangle]
pub extern "C" fn hufu_client_reset(c: *mut HufuClient) {
    if !c.is_null() {
        unsafe { &mut *c }.reset();
    }
}

#[no_mangle]
pub extern "C" fn hufu_client_focus(c: *mut HufuClient) {
    if !c.is_null() {
        unsafe { &mut *c }.focus();
    }
}

/// ping：1=引擎可达。设置页/排障用。
#[no_mangle]
pub extern "C" fn hufu_client_ping(c: *mut HufuClient) -> c_int {
    if c.is_null() {
        return 0;
    }
    if unsafe { &mut *c }.ping() {
        1
    } else {
        0
    }
}

/// 最近状态串（诊断；UTF-8，NUL 结尾）。
#[no_mangle]
pub extern "C" fn hufu_client_status(c: *const HufuClient) -> *const c_char {
    if c.is_null() {
        return std::ptr::null();
    }
    unsafe { &*c }.status().as_ptr()
}

/// 最近中英态（subMode）：1=中。
#[no_mangle]
pub extern "C" fn hufu_client_chinese(c: *const HufuClient) -> c_int {
    if c.is_null() {
        return 1;
    }
    if unsafe { &*c }.chinese {
        1
    } else {
        0
    }
}

/// 最近一次按键的回删数（commit 回调发生在 `key` 返回前——C++ 侧在
/// commit 回调里读它，先回删已上屏字符再上屏新文本）。
#[no_mangle]
pub extern "C" fn hufu_client_last_back(c: *const HufuClient) -> c_int {
    if c.is_null() {
        return 0;
    }
    unsafe { &*c }.last_back as c_int
}

// ---------------------------------------------------------------------------
// 单测（mock Unix socket server）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// 每个测试独立的回调捕获（并行测试互不干扰）。
    #[derive(Default)]
    struct Capture {
        commits: Vec<String>,
        /// (preedit, raw, candidates, commit_texts, selected, chinese)
        updates: Vec<(String, String, Vec<String>, Vec<String>, usize, bool)>,
    }

    fn cap_mut<'a>(user: *mut c_void) -> &'a mut Capture {
        unsafe { &mut *(user as *mut Capture) }
    }

    unsafe extern "C" fn on_commit(user: *mut c_void, text: *const c_char) {
        let s = unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned();
        cap_mut(user).commits.push(s);
    }

    unsafe extern "C" fn on_update(
        user: *mut c_void,
        preedit: *const c_char,
        raw: *const c_char,
        texts: *const *const c_char,
        _comments: *const *const c_char,
        commits: *const *const c_char,
        count: c_int,
        selected: c_int,
        _aux: *const c_char,
        chinese: c_int,
    ) {
        let pre = unsafe { CStr::from_ptr(preedit) }
            .to_string_lossy()
            .into_owned();
        let raw = unsafe { CStr::from_ptr(raw) }.to_string_lossy().into_owned();
        let mut cands = Vec::new();
        let mut commits_v = Vec::new();
        for i in 0..count.max(0) as isize {
            let p = unsafe { *texts.offset(i) };
            cands.push(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned());
            let cp = unsafe { *commits.offset(i) };
            commits_v.push(unsafe { CStr::from_ptr(cp) }.to_string_lossy().into_owned());
        }
        cap_mut(user).updates.push((
            pre,
            raw,
            cands,
            commits_v,
            selected.max(0) as usize,
            chinese == 1,
        ));
    }

    fn test_sock(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "hufu-client-test-{}-{}.sock",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    /// mock server：把一串预置响应依次回给一个连接。
    /// 注意 listener 必须在测试线程同步 bind——放进子线程会与客户端
    /// connect 竞态（ECONNREFUSED → 测试静默失败/挂起）。
    fn mock_server(path: PathBuf, responses: Vec<serde_json::Value>) -> std::thread::JoinHandle<()> {
        let listener = UnixListener::bind(&path).expect("bind");
        std::thread::spawn(move || {
            let Ok((mut s, _)) = listener.accept() else {
                return;
            };
            for resp in responses {
                let mut head = [0u8; 4];
                if s.read_exact(&mut head).is_err() {
                    return;
                }
                let n = u32::from_le_bytes(head) as usize;
                let mut buf = vec![0u8; n];
                if s.read_exact(&mut buf).is_err() {
                    return;
                }
                let body = serde_json::to_vec(&resp).unwrap();
                let _ = s.write_all(&(body.len() as u32).to_le_bytes());
                let _ = s.write_all(&body);
            }
            let _ = std::fs::remove_file(&path);
        })
    }

    fn client_for(path: PathBuf, cap: *mut Capture) -> HufuClient {
        HufuClient::new(
            path,
            HufuHost {
                user: cap as *mut c_void,
                commit: Some(on_commit),
                update: Some(on_update),
            },
        )
    }

    #[test]
    fn key_commit_update_and_back() {
        let mut cap = Box::new(Capture::default());
        let cap_ptr: *mut Capture = &mut *cap;
        let path = test_sock("key");
        let handle = mock_server(
            path.clone(),
            vec![serde_json::json!({
                "outcome": {
                    "consumed": true,
                    "commit": "的",
                    "back": 1,
                    "state": {
                        "raw": "u",
                        "preedit": "u",
                        "candidates": [
                            {"text": "的", "code": "u", "source": {"kind": "dict"}},
                            {"text": "得", "code": "u", "source": {"kind": "dict"}}
                        ],
                        "page": 0, "page_count": 1, "selected": 1,
                        "mode": "Normal", "chinese": true,
                        "full_shape": false, "ascii_punct": false
                    }
                },
                "state": {"raw": "u", "candidates": [], "page": 0, "page_count": 0,
                          "mode": "Normal", "chinese": true}
            })],
        );
        let mut c = client_for(path, cap_ptr);
        let (consumed, back) = c.key("u", false, false, false, false, false, -1);
        assert!(consumed);
        assert_eq!(back, 1);
        assert_eq!(cap.commits.as_slice(), ["的"]);
        assert_eq!(cap.updates.len(), 1);
        assert_eq!(cap.updates[0].0, "u");
        assert_eq!(cap.updates[0].1, "u");
        assert_eq!(
            cap.updates[0].2,
            vec!["的".to_string(), "得".to_string()]
        );
        assert_eq!(
            cap.updates[0].3,
            vec!["的".to_string(), "得".to_string()],
            "上屏文本数组（无 commit_override 时=显示文本）"
        );
        assert_eq!(cap.updates[0].4, 1);
        assert!(cap.updates[0].5);
        handle.join().unwrap();
    }

    #[test]
    fn passthrough_no_commit() {
        let mut cap = Box::new(Capture::default());
        let cap_ptr: *mut Capture = &mut *cap;
        let path = test_sock("pass");
        let handle = mock_server(
            path.clone(),
            vec![serde_json::json!({
                "outcome": {"consumed": false},
                "state": {"raw": "", "candidates": [], "page": 0, "page_count": 0,
                          "mode": "Normal", "chinese": true}
            })],
        );
        let mut c = client_for(path, cap_ptr);
        let (consumed, back) = c.key("x", false, false, false, false, false, -1);
        assert!(!consumed);
        assert_eq!(back, 0);
        assert!(cap.commits.is_empty());
        assert!(cap.updates.is_empty());
        handle.join().unwrap();
    }

    #[test]
    fn no_server_passthrough() {
        let mut cap = Box::new(Capture::default());
        let cap_ptr: *mut Capture = &mut *cap;
        let path = test_sock("offline");
        let mut c = client_for(path, cap_ptr);
        let (consumed, back) = c.key("a", false, false, false, false, false, -1);
        assert!(!consumed);
        assert_eq!(back, 0);
        assert!(!c.ping());
    }

    #[test]
    fn flash_frame_raw_empty_with_candidates() {
        // 数字/; 选重上屏：引擎回闪帧（raw/preedit 空 + 旧候选 + 高亮）。
        // 壳据此清窗；此处验证 raw 通道确实送达空串。
        let mut cap = Box::new(Capture::default());
        let cap_ptr: *mut Capture = &mut *cap;
        let path = test_sock("flash");
        let handle = mock_server(
            path.clone(),
            vec![serde_json::json!({
                "outcome": {
                    "consumed": true,
                    "commit": "的",
                    "state": {
                        "raw": "",
                        "preedit": "",
                        "candidates": [
                            {"text": "的", "code": "u", "source": {"kind": "dict"}},
                            {"text": "得", "code": "u", "source": {"kind": "dict"}}
                        ],
                        "page": 0, "page_count": 1, "selected": 0,
                        "mode": "Normal", "chinese": true,
                        "full_shape": false, "ascii_punct": false
                    }
                }
            })],
        );
        let mut c = client_for(path, cap_ptr);
        let (consumed, _) = c.key("1", false, false, false, false, false, -1);
        assert!(consumed);
        assert_eq!(cap.commits.as_slice(), ["的"]);
        assert_eq!(cap.updates.len(), 1);
        assert_eq!(cap.updates[0].0, ""); // preedit
        assert_eq!(cap.updates[0].1, ""); // raw —— 壳以「raw 空 + 有候选」判闪帧
        assert_eq!(cap.updates[0].2.len(), 2);
        handle.join().unwrap();
    }

    #[test]
    fn reset_clears_panel() {
        let mut cap = Box::new(Capture::default());
        let cap_ptr: *mut Capture = &mut *cap;
        let path = test_sock("reset");
        let handle = mock_server(
            path.clone(),
            vec![serde_json::json!({
                "state": {"raw": "", "preedit": "", "candidates": [],
                          "page": 0, "page_count": 0, "mode": "Normal", "chinese": true}
            })],
        );
        let mut c = client_for(path, cap_ptr);
        c.reset();
        assert_eq!(cap.updates.len(), 1);
        assert_eq!(cap.updates[0].0, "");
        assert_eq!(cap.updates[0].1, "");
        assert!(cap.updates[0].2.is_empty());
        handle.join().unwrap();
    }
}
