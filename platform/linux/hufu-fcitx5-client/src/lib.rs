// SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! 虎符（hufu-ime）fcitx5 前端 · Rust 侧（staticlib）。
//!
//! 架构与 Windows TSF / macOS IMK 一致：本层是薄壳，按键经 Unix socket
//! （`$XDG_RUNTIME_DIR/hufu-ime.sock`，4 字节小端长度 + JSON 帧，与
//! Windows 命名管道同一协议）发给 `hufu-server` 的引擎；回包
//! `{outcome, state}` 经宿主回调驱动组段、候选与上屏。
//!
//! C++ 侧（`hufu-addon/shell/hufu.cpp`）经 `hufu_abi.h` 调用本库；
//! 本 crate 同时产出 rlib 供 mock socket 单测。
//!
//! 按键音效：key/select 回包里的 `outcome.sound`（引擎仅在 `sound.enabled` 时填）
//! 记为待处理 tag，宿主用 `hufu_client_take_sound` 取走后，再经 `hufu_client_sound_fetch`
//! 取回完整 WAV 字节（op `sound`，按 tag 缓存）与当前音量（op `sound_state`，每次现取）——
//! 播放由宿主负责（本层只搬字节）。
//!
//! 所有 `hufu_client_*` 导出都是 `unsafe fn`：调用方须保证指针有效（`hufu_client_new`
//! 的返回值，或各函数注释里明确允许的 NULL）；返回的 `*const c_char` 指向客户端内部缓冲，
//! 下次对同一客户端调用同类函数前有效，宿主须同步拷走。
#![allow(clippy::missing_safety_doc)]

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
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

/// `{"enabled": bool}` 回包 → 1/0；回包缺失或字段不是 bool → -1（未知）。
/// 音效读态/取反共用（引擎不在线时调用方拿到 -1，宿主保持现状）。
fn enabled_flag(resp: Option<serde_json::Value>) -> i32 {
    match resp.and_then(|v| v.get("enabled").and_then(|x| x.as_bool())) {
        Some(true) => 1,
        Some(false) => 0,
        None => -1,
    }
}

/// JSON 深合并：对象递归合并，其余类型整体覆盖（配置补丁用）。
fn merge_json(dst: &mut serde_json::Value, patch: &serde_json::Value) {
    if let (serde_json::Value::Object(d), serde_json::Value::Object(p)) = (&mut *dst, patch) {
        for (k, v) in p {
            match d.get_mut(k) {
                Some(slot) => merge_json(slot, v),
                None => {
                    d.insert(k.clone(), v.clone());
                }
            }
        }
    } else {
        *dst = patch.clone();
    }
}

// ---------------------------------------------------------------------------
// 字反查（纯宿主侧）：按数据目录建「字 → 拼音 / 虎码 / 拆分」索引
// ---------------------------------------------------------------------------

/// 拼音注释（每行 `字\t拼音`；多音写在同一列里，以空格分隔）。
const PINYIN_ANNOTATION: &str = "数据/注释/拼音.注释";
/// 虎码单字表（Rime 词典：`columns:` 给列名，数据行 `字\t码\t权重`）。
const CODE_DICT: &str = "码表/虎码单字/tiger.dict.yaml";
/// 部件拆解（每行 `字\t拆解`；可选——缺文件只是没有拆分列）。
const SPLIT_ANNOTATION: &str = "数据/拆分/虎码.拆分";

/// 取「恰好一个字符」的键；空串、多字符都不是有效键。
fn single_key_char(field: &str) -> Option<char> {
    let mut chars = field.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Some(ch),
        _ => None,
    }
}

/// 读 `字\t值` 两列表（拼音注释与拆分注释同构）。
///
/// 坏行一律跳过（空行、`#` 注释、缺列、键不是单字符、值为空）；多出来的列忽略而不是
/// 判错——实际数据里出现过「值后面多一个尾随空列」的行。同一字出现多次取首行。
fn parse_char_map(text: &str) -> HashMap<char, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(key), Some(value)) = (fields.next(), fields.next()) else {
            continue;
        };
        if let Some(ch) = single_key_char(key) {
            if !value.is_empty() {
                map.entry(ch).or_insert_with(|| value.to_string());
            }
        }
    }
    map
}

/// 读 Rime 词典 `columns:` 里的列名，返回 `(text 列下标, code 列下标)`。
///
/// 列名对不上（没有 `text` 或 `code`）返回 `None`：宁可按「没有码表」降级，也不按
/// 位置硬认列——换了列序的词典会把权重当码显示出去。
fn rime_columns(text: &str) -> Option<(usize, usize)> {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.trim_end_matches('\r').trim() != "columns:" {
            continue;
        }
        let mut names: Vec<String> = Vec::new();
        for item in lines.by_ref() {
            let item = item.trim_end_matches('\r');
            let Some(name) = item.trim().strip_prefix('-') else {
                break; // 列表结束（`...` 或数据行）
            };
            names.push(name.trim().to_string());
        }
        let text_idx = names.iter().position(|n| n == "text")?;
        let code_idx = names.iter().position(|n| n == "code")?;
        return Some((text_idx, code_idx));
    }
    None
}

/// 读 Rime 词典数据行：同字多码按文件序全收（简码在前、全码在后，展示时以 `/` 连接）。
///
/// 头部 YAML 行不含 TAB（或首列不是单字），`...`/`---` 标记与 `#` 注释按「列数不足 /
/// 注释行」自然跳过，故不依赖 `...` 结束标记一定存在。
fn parse_rime_dict(text: &str, text_idx: usize, code_idx: usize) -> HashMap<char, Vec<String>> {
    let need = text_idx.max(code_idx);
    let mut map: HashMap<char, Vec<String>> = HashMap::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() <= need {
            continue;
        }
        let Some(ch) = single_key_char(fields[text_idx]) else {
            continue;
        };
        let code = fields[code_idx];
        if code.is_empty() {
            continue;
        }
        let codes = map.entry(ch).or_default();
        if !codes.iter().any(|c| c == code) {
            codes.push(code.to_string());
        }
    }
    map
}

/// 字反查索引（数据目录由宿主传入；本层不读环境变量）。
///
/// 三份数据各自独立降级：某个文件缺失/读失败只是该列没有数据，不影响其余列。
#[derive(Default)]
struct CharLookup {
    /// 字 → 拼音（注释文件原样，多音以空格分隔）
    pinyin: HashMap<char, String>,
    /// 字 → 虎码（同字多码按文件序）
    codes: HashMap<char, Vec<String>>,
    /// 字 → 部件拆解
    splits: HashMap<char, String>,
}

impl CharLookup {
    /// 按数据根目录（`${XDG_DATA_HOME:-$HOME/.local/share}/hufu`）读三份数据。
    /// 读失败按「该列无数据」处理：不报错、不 panic，由调用方决定是否启用。
    fn load(root: &Path) -> CharLookup {
        let pinyin = std::fs::read_to_string(root.join(PINYIN_ANNOTATION))
            .map(|t| parse_char_map(&t))
            .unwrap_or_default();
        let codes = std::fs::read_to_string(root.join(CODE_DICT))
            .ok()
            .and_then(|t| rime_columns(&t).map(|(ti, ci)| parse_rime_dict(&t, ti, ci)))
            .unwrap_or_default();
        let splits = std::fs::read_to_string(root.join(SPLIT_ANNOTATION))
            .map(|t| parse_char_map(&t))
            .unwrap_or_default();
        CharLookup {
            pinyin,
            codes,
            splits,
        }
    }

    /// 是否值得启用：拼音注释与码表至少一份有数据（只剩拆分列没有意义）。
    fn is_usable(&self) -> bool {
        !self.pinyin.is_empty() || !self.codes.is_empty()
    }

    /// 一个字的三列：`拼音\t虎码[\t拆分]`（缺项为空列；三列全空返回空串）。
    fn row(&self, ch: char) -> String {
        let pinyin = self.pinyin.get(&ch).cloned().unwrap_or_default();
        let code = self
            .codes
            .get(&ch)
            .map(|codes| codes.join("/"))
            .unwrap_or_default();
        let split = self.splits.get(&ch).cloned().unwrap_or_default();
        if pinyin.is_empty() && code.is_empty() && split.is_empty() {
            return String::new();
        }
        let mut row = format!("{pinyin}\t{code}");
        if !split.is_empty() {
            row.push('\t');
            row.push_str(&split);
        }
        row
    }
}

// ---------------------------------------------------------------------------
// 按键音效：key/select 回包的 outcome.sound → op sound 取回完整 WAV
// ---------------------------------------------------------------------------

/// 一类音效的字节与音量（0–100）：首次 fetch 时取回，其后同类 tag 直接用缓存。
struct SoundClip {
    /// 完整 WAV 文件（含 RIFF 头），可直接落盘交给播放器
    bytes: Vec<u8>,
    /// 取回时的引擎音量（`sound.volume`，0–100）
    volume: i32,
}

/// base64 单字符值（标准字母表；非法字符返回 None）。
fn b64_val(b: u8) -> Option<u32> {
    match b {
        b'A'..=b'Z' => Some(u32::from(b - b'A')),
        b'a'..=b'z' => Some(u32::from(b - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(b - b'0') + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// 标准 base64 解码（自足实现，不引依赖；引擎侧的编码同样是自足实现）。
///
/// 非法字符、长度不是 4 的倍数、`=` 不在结尾组或不在末尾，都返回 None——
/// 宁可按「取音效失败」处理，也不把半截字节当 WAV 交给播放器。
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return None;
    }
    let chunks = bytes.len() / 4;
    let mut out = Vec::with_capacity(chunks * 3);
    for (i, chunk) in bytes.chunks(4).enumerate() {
        let last = i + 1 == chunks;
        let mut n: u32 = 0;
        let mut pad = 0u32;
        for (j, &b) in chunk.iter().enumerate() {
            if b == b'=' {
                // 填充只能出现在最后一组的最后 1–2 位
                if !last || j < 2 {
                    return None;
                }
                pad += 1;
                n <<= 6;
                continue;
            }
            if pad > 0 {
                return None; // `=` 之后不允许再有数据
            }
            n = (n << 6) | b64_val(b)?;
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
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
    /// 引擎配置快照（config_get；fcitx5 设置页读写用）
    config: Option<serde_json::Value>,
    /// `hufu_client_config_str` 返回值缓存（C++ 侧同步拷走）
    config_scratch: CString,
    /// 字反查索引（宿主首次触发时才装载；`None` = 未初始化或数据不可用）
    char_lookup: Option<CharLookup>,
    /// `hufu_client_char_lookup` 返回值缓存（C++ 侧同步拷走）
    char_lookup_row: CString,
    /// 待处理音效 tag（key/select 回包的 `outcome.sound`；空 = 无），
    /// 由宿主在每次 key/select 之后用 `take_sound` 取走。
    sound_pending: CString,
    /// `take_sound` 返回值缓存（取走后仍指向有效内存，C++ 侧同步拷走）
    sound_tag_scratch: CString,
    /// tag → 音效片段（首次 fetch 取回；其后同类 tag 用缓存，不再打扰引擎）
    sound_clips: HashMap<String, SoundClip>,
    /// 最近一次成功 fetch 的完整 WAV 字节（`sound_data` 暴露）
    sound_bytes: Vec<u8>,
}

impl HufuClient {
    pub fn new(sock_path: PathBuf, host: HufuHost) -> Self {
        let status = CString::new(format!("未连接（{}）", sock_path.display())).unwrap_or_default();
        HufuClient {
            sock_path,
            stream: None,
            host,
            status,
            scratch: Scratch::default(),
            chinese: true,
            last_back: 0,
            config: None,
            config_scratch: CString::default(),
            char_lookup: None,
            char_lookup_row: CString::default(),
            sound_pending: CString::default(),
            sound_tag_scratch: CString::default(),
            sound_clips: HashMap::new(),
            sound_bytes: Vec::new(),
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

    fn roundtrip(
        s: &mut UnixStream,
        req: &serde_json::Value,
    ) -> std::io::Result<serde_json::Value> {
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

    /// 托盘「重载码表」：当前方案原样重载（改码表/补充语料/符号表后免重启
    /// server 生效，与 Windows 语言栏同款 op）；1=成功，0=失败（含引擎不在线）。
    pub fn reload_schema(&mut self) -> bool {
        self.call(&serde_json::json!({"op": "reload_schema"}))
            .map(|v| v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false))
            .unwrap_or(false)
    }

    /// 托盘「打开方案文件夹」：请引擎打开当前方案码表目录（文件管理器由
    /// server 侧拉起）；1=成功，0=失败（含引擎不在线、方案目录不存在）。
    pub fn open_schema_dir(&mut self) -> bool {
        self.call(&serde_json::json!({"op": "open_schema_dir"}))
            .map(|v| v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false))
            .unwrap_or(false)
    }

    /// 托盘「按键音效」：引擎侧取反并落盘（热生效）；返回**新态**
    /// （1=开 / 0=关 / -1=未知——引擎不在线或回包异常）。
    ///
    /// 成功时清掉音效字节缓存：音量与「哪几类音效存在」都可能刚变过，下一次取用
    /// 重新问引擎（否则要重启客户端才生效）。
    pub fn sound_toggle(&mut self) -> i32 {
        let state = enabled_flag(self.call(&serde_json::json!({"op": "sound_toggle"})));
        if state >= 0 {
            self.sound_clips.clear();
        }
        state
    }

    /// 托盘「按键音效」勾选态：1=开 / 0=关 / -1=未知（引擎不在线或回包异常）。
    pub fn sound_state(&mut self) -> i32 {
        enabled_flag(self.call(&serde_json::json!({"op": "sound_state"})))
    }

    /// 取走待处理音效 tag（key/select 回包的 `outcome.sound`）：取走后清空，
    /// 无待处理时为空串。返回的指针指向客户端内部缓冲，下次调用本函数前有效。
    pub fn take_sound(&mut self) -> *const c_char {
        self.sound_tag_scratch = std::mem::take(&mut self.sound_pending);
        self.sound_tag_scratch.as_ptr()
    }

    /// 取回 tag 的完整 WAV（op `sound`）：成功返回**当前**音量 0–100，并把字节缓存在
    /// 客户端里；失败、未知 tag、音效文件缺失（回包 `data: null`）、引擎不在线都返回 -1。
    ///
    /// 字节按 tag 缓存（音效 wav 是安装期产物，不随设置改动），音量则**每次现取**
    /// （`sound_state`，小回包）：音量是随时可改的设置项，拖一次设置页滑块就该立刻
    /// 生效，不该等到「按键音效」开关翻转才更新。现取失败的短暂窗口沿用上一次已知
    /// 音量，让已在缓存里的音效照常出声（引擎挂起时不该突然静音）。
    /// 缓存的失效点仍是 `sound_toggle`（音效文件集合可能刚变过）。
    pub fn sound_fetch(&mut self, tag: &str) -> i32 {
        if tag.is_empty() {
            return -1;
        }
        if self.sound_clips.contains_key(tag) {
            let live = self.sound_volume();
            if let Some(clip) = self.sound_clips.get_mut(tag) {
                if live >= 0 {
                    clip.volume = live;
                }
                self.sound_bytes = clip.bytes.clone();
                return clip.volume;
            }
        }
        let Some(resp) = self.call(&serde_json::json!({"op": "sound", "tag": tag})) else {
            return -1;
        };
        // 文件缺失时引擎回 `data: null`（volume 仍带）——按取不到处理。
        let Some(data) = resp.get("data").and_then(|v| v.as_str()) else {
            return -1;
        };
        let Some(bytes) = base64_decode(data) else {
            self.set_status("音效 base64 解码失败");
            return -1;
        };
        if bytes.is_empty() {
            self.set_status("音效数据为空");
            return -1;
        }
        let volume = resp
            .get("volume")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .clamp(0, 100) as i32;
        self.sound_bytes = bytes.clone();
        self.sound_clips
            .insert(tag.to_string(), SoundClip { bytes, volume });
        volume
    }

    /// 引擎当前音量（op `sound_state`，0–100）；-1=未知（引擎不在线或回包没有 volume）。
    fn sound_volume(&mut self) -> i32 {
        let Some(resp) = self.call(&serde_json::json!({"op": "sound_state"})) else {
            return -1;
        };
        resp.get("volume")
            .and_then(|v| v.as_i64())
            .map(|v| v.clamp(0, 100) as i32)
            .unwrap_or(-1)
    }

    /// 最近一次成功 `sound_fetch` 的 WAV 字节（从未成功取过时为空切片）。
    pub fn sound_data(&self) -> &[u8] {
        &self.sound_bytes
    }

    /// 拉取引擎配置快照（fcitx5 设置页打开时调用）。
    pub fn refresh_config(&mut self) -> bool {
        let Some(resp) = self.call(&serde_json::json!({"op": "config_get"})) else {
            return false;
        };
        match resp.get("config") {
            Some(c) if !c.is_null() => {
                self.config = Some(c.clone());
                true
            }
            _ => false,
        }
    }

    /// 按 `a.b.c` 路径取配置值（需先 refresh_config）。
    fn config_get(&self, path: &str) -> Option<&serde_json::Value> {
        let mut cur = self.config.as_ref()?;
        for seg in path.split('.') {
            cur = cur.get(seg)?;
        }
        Some(cur)
    }

    /// 配置补丁：拉取当前配置 → 深合并 → config_set → 成功后更新缓存。
    /// （读-改-写：避免部分 JSON 缺省字段被 serde 默认值覆盖。）
    pub fn config_patch(&mut self, patch: &serde_json::Value) -> bool {
        if self.config.is_none() && !self.refresh_config() {
            return false;
        }
        let Some(mut merged) = self.config.clone() else {
            return false;
        };
        merge_json(&mut merged, patch);
        let Some(resp) = self.call(&serde_json::json!({"op": "config_set", "config": merged}))
        else {
            return false;
        };
        if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            self.config = Some(merged);
            true
        } else {
            self.set_status(format!(
                "配置写入失败: {}",
                resp.get("error").and_then(|v| v.as_str()).unwrap_or("未知")
            ));
            false
        }
    }

    /// 配置读取（C++ ABI 用）：bool →（1/0/-1 未知）。
    pub fn config_bool(&self, path: &str) -> i32 {
        match self.config_get(path).and_then(|v| v.as_bool()) {
            Some(true) => 1,
            Some(false) => 0,
            None => -1,
        }
    }

    /// 配置读取：整数（返回 Some）。
    pub fn config_int(&self, path: &str) -> Option<i64> {
        self.config_get(path).and_then(|v| v.as_i64())
    }

    /// 配置读取：字符串（写入 `config_scratch`，返回 C 指针）。
    pub fn config_str_ptr(&mut self, path: &str) -> *const std::ffi::c_char {
        let s = self
            .config_get(path)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        self.config_scratch = CString::new(s).unwrap_or_default();
        self.config_scratch.as_ptr()
    }

    /// 装载字反查索引（宿主传入 `$HUFU_ROOT`；可重复调用，按新目录重载）。
    /// 返回是否可用：拼音注释或码表至少一份读到数据即为可用。
    pub fn char_lookup_init(&mut self, root: &Path) -> bool {
        let lookup = CharLookup::load(root);
        let usable = lookup.is_usable();
        self.char_lookup = if usable { Some(lookup) } else { None };
        usable
    }

    /// 查一个字符：结果写入 `char_lookup_row`（三列 TAB 分隔；无数据为空串）并返回其指针。
    pub fn char_lookup(&mut self, ucs4: u32) -> *const c_char {
        let row = match (char::from_u32(ucs4), self.char_lookup.as_ref()) {
            (Some(ch), Some(lookup)) => lookup.row(ch),
            _ => String::new(),
        };
        self.char_lookup_row = CString::new(row).unwrap_or_default();
        self.char_lookup_row.as_ptr()
    }

    /// 一次按键：返回 `(consumed, back)`；回调同步送达 commit/update。
    ///
    /// `line_end`：1=光标在行尾，0=不在，-1=未知（未知不带该字段）。
    // 参数逐个对应 ABI 与线上的修饰键字段，合成结构体只是搬运，故保留长参数表。
    #[allow(clippy::too_many_arguments)]
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
        self.apply_outcome_response(resp)
    }

    /// 鼠标点击候选（页内下标）：走引擎 `select` op，语义与数字选重一致
    /// （学习、无闪帧）；返回是否消费。回调同步送达 commit/update。
    pub fn select(&mut self, index: usize) -> bool {
        let Some(resp) = self.call(&serde_json::json!({"op": "select", "index": index})) else {
            return false;
        };
        self.apply_outcome_response(resp).0
    }

    /// `{outcome, state}` 回包 → 回调送达 + `(consumed, back)`。
    /// key/select 共用（协议同构）。
    fn apply_outcome_response(&mut self, resp: serde_json::Value) -> (bool, u8) {
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
        // 音效 tag：引擎仅在 sound.enabled 时填；回包里没有就保持待处理态不变
        //（宿主每次 key/select 之后都会 take_sound 取走，不会积压）。
        if let Some(tag) = outcome.sound.as_deref() {
            if !tag.is_empty() {
                self.sound_pending = CString::new(tag).unwrap_or_default();
            }
        }
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
pub unsafe extern "C" fn hufu_client_new(
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
pub unsafe extern "C" fn hufu_client_free(c: *mut HufuClient) {
    if !c.is_null() {
        drop(unsafe { Box::from_raw(c) });
    }
}

/// 一次按键：返回位掩码 `HUFU_KEY_CONSUMED | (back << HUFU_KEY_BACK_SHIFT)`。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_key(
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
    let key = unsafe { CStr::from_ptr(key) }
        .to_string_lossy()
        .into_owned();
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
pub unsafe extern "C" fn hufu_client_reset(c: *mut HufuClient) {
    if !c.is_null() {
        unsafe { &mut *c }.reset();
    }
}

/// 鼠标点击候选：`index` 为页内下标（当前候选窗列表序号）；1=已处理。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_select(c: *mut HufuClient, index: c_int) -> c_int {
    if c.is_null() || index < 0 {
        return 0;
    }
    if unsafe { &mut *c }.select(index as usize) {
        1
    } else {
        0
    }
}

/// 托盘「重载码表」：当前方案原样重载；1=成功（0=失败/引擎不在线，宿主保持现状）。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_reload_schema(c: *mut HufuClient) -> c_int {
    if c.is_null() {
        return 0;
    }
    if unsafe { &mut *c }.reload_schema() {
        1
    } else {
        0
    }
}

/// 托盘「打开方案文件夹」：请引擎打开当前方案码表目录；1=成功（0=失败/引擎不在线）。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_open_schema_dir(c: *mut HufuClient) -> c_int {
    if c.is_null() {
        return 0;
    }
    if unsafe { &mut *c }.open_schema_dir() {
        1
    } else {
        0
    }
}

/// 托盘「按键音效」：引擎侧取反并落盘；返回新态（1=开 / 0=关 / -1=未知）。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_sound_toggle(c: *mut HufuClient) -> c_int {
    if c.is_null() {
        return -1;
    }
    unsafe { &mut *c }.sound_toggle()
}

/// 托盘「按键音效」勾选态：1=开 / 0=关 / -1=未知（引擎不在线）。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_sound_state(c: *mut HufuClient) -> c_int {
    if c.is_null() {
        return -1;
    }
    unsafe { &mut *c }.sound_state()
}

/// 取走待处理音效 tag（key/select 回包里的 `outcome.sound`）：取走后清空，
/// 无待处理时为空串。返回 NUL 结尾指针，指向客户端内部缓冲——**下次对同一客户端
/// 调用本函数前有效**，宿主须同步拷走；`c` 为 NULL 时返回 NULL。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_take_sound(c: *mut HufuClient) -> *const c_char {
    if c.is_null() {
        return std::ptr::null();
    }
    unsafe { &mut *c }.take_sound()
}

/// 取回 tag 的完整 WAV（op `sound`）：成功返回**当前**音量 0–100，并把字节缓存在
/// 客户端里；失败、未知 tag、音效文件缺失（回包 `data: null`）、引擎不在线都返回 -1。
/// 字节按 tag 重复取用缓存（`sound_toggle` 时失效），音量每次现取（即改即生效）。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_sound_fetch(c: *mut HufuClient, tag: *const c_char) -> c_int {
    if c.is_null() || tag.is_null() {
        return -1;
    }
    let tag = unsafe { CStr::from_ptr(tag) }
        .to_string_lossy()
        .into_owned();
    unsafe { &mut *c }.sound_fetch(&tag)
}

/// 最近一次成功 `hufu_client_sound_fetch` 的 WAV 字节（完整文件，含 RIFF 头）。
/// 指针指向客户端内部缓冲，**下次 fetch 或释放客户端前有效**，宿主须同步拷走；
/// 从未成功取过时返回 NULL。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_sound_data(c: *const HufuClient) -> *const u8 {
    if c.is_null() {
        return std::ptr::null();
    }
    let data = unsafe { &*c }.sound_data();
    if data.is_empty() {
        std::ptr::null()
    } else {
        data.as_ptr()
    }
}

/// 最近一次成功 `hufu_client_sound_fetch` 的字节数（0=没有）。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_sound_size(c: *const HufuClient) -> c_int {
    if c.is_null() {
        return 0;
    }
    unsafe { &*c }.sound_data().len() as c_int
}

/// 拉取引擎配置（fcitx5 设置页打开时调用）：1=成功。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_config_refresh(c: *mut HufuClient) -> c_int {
    if c.is_null() {
        return 0;
    }
    if unsafe { &mut *c }.refresh_config() {
        1
    } else {
        0
    }
}

/// 配置补丁（JSON 对象字符串，深合并后写回引擎）：1=成功。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_config_patch(
    c: *mut HufuClient,
    json: *const c_char,
) -> c_int {
    if c.is_null() || json.is_null() {
        return 0;
    }
    let s = unsafe { CStr::from_ptr(json) }.to_string_lossy();
    let Ok(patch) = serde_json::from_str::<serde_json::Value>(&s) else {
        return 0;
    };
    if unsafe { &mut *c }.config_patch(&patch) {
        1
    } else {
        0
    }
}

/// 配置读取 bool：1/0，-1=未知（未拉取或路径不存在）。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_config_bool(
    c: *const HufuClient,
    path: *const c_char,
) -> c_int {
    if c.is_null() || path.is_null() {
        return -1;
    }
    let p = unsafe { CStr::from_ptr(path) }.to_string_lossy();
    unsafe { &*c }.config_bool(&p)
}

/// 配置读取整数：1=成功（写 `*out`），0=未知/失败。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_config_int(
    c: *const HufuClient,
    path: *const c_char,
    out: *mut i64,
) -> c_int {
    if c.is_null() || path.is_null() || out.is_null() {
        return 0;
    }
    let p = unsafe { CStr::from_ptr(path) }.to_string_lossy();
    match unsafe { &*c }.config_int(&p) {
        Some(v) => {
            unsafe { *out = v };
            1
        }
        None => 0,
    }
}

/// 配置读取字符串：返回 NUL 结尾指针（空串=未知；下次调用前有效）。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_config_str(
    c: *mut HufuClient,
    path: *const c_char,
) -> *const c_char {
    if c.is_null() || path.is_null() {
        return std::ptr::null();
    }
    let p = unsafe { CStr::from_ptr(path) }
        .to_string_lossy()
        .into_owned();
    unsafe { &mut *c }.config_str_ptr(&p)
}

#[no_mangle]
pub unsafe extern "C" fn hufu_client_focus(c: *mut HufuClient) {
    if !c.is_null() {
        unsafe { &mut *c }.focus();
    }
}

/// ping：1=引擎可达。设置页/排障用。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_ping(c: *mut HufuClient) -> c_int {
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
pub unsafe extern "C" fn hufu_client_status(c: *const HufuClient) -> *const c_char {
    if c.is_null() {
        return std::ptr::null();
    }
    unsafe { &*c }.status().as_ptr()
}

/// 最近中英态（subMode）：1=中。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_chinese(c: *const HufuClient) -> c_int {
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
pub unsafe extern "C" fn hufu_client_last_back(c: *const HufuClient) -> c_int {
    if c.is_null() {
        return 0;
    }
    unsafe { &*c }.last_back as c_int
}

/// 装载字反查索引：`data_dir` 为数据根目录（`${XDG_DATA_HOME:-$HOME/.local/share}/hufu`，
/// 由宿主解析后传入——本层不读环境变量）。1=可用（拼音注释或码表至少一份读到数据），
/// 0=不可用（目录/文件缺失、参数非法）；可重复调用（按新目录重载，失败即清空索引）。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_char_lookup_init(
    c: *mut HufuClient,
    data_dir: *const c_char,
) -> c_int {
    if c.is_null() || data_dir.is_null() {
        return 0;
    }
    let dir = unsafe { CStr::from_ptr(data_dir) }
        .to_string_lossy()
        .into_owned();
    if dir.is_empty() {
        return 0;
    }
    if unsafe { &mut *c }.char_lookup_init(Path::new(&dir)) {
        1
    } else {
        0
    }
}

/// 查一个字符的「拼音\t虎码[\t拆分]」：缺项为空列，整字无数据为空串。
/// 返回 NUL 结尾指针，指向客户端内部缓冲——**下次对同一客户端调用本函数前有效**，
/// 宿主须同步拷走；未初始化（或 `ucs4` 不是有效字符）时返回空串。
#[no_mangle]
pub unsafe extern "C" fn hufu_client_char_lookup(c: *mut HufuClient, ucs4: u32) -> *const c_char {
    if c.is_null() {
        return std::ptr::null();
    }
    unsafe { &mut *c }.char_lookup(ucs4)
}

// ---------------------------------------------------------------------------
// 单测（mock Unix socket server）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::Mutex;

    /// 一次 update 回调的字段：(preedit, raw, candidates, commit_texts, selected, chinese)
    type UpdateRecord = (String, String, Vec<String>, Vec<String>, usize, bool);

    /// 每个测试独立的回调捕获（并行测试互不干扰）。
    #[derive(Default)]
    struct Capture {
        commits: Vec<String>,
        updates: Vec<UpdateRecord>,
    }

    fn cap_mut<'a>(user: *mut c_void) -> &'a mut Capture {
        unsafe { &mut *(user as *mut Capture) }
    }

    unsafe extern "C" fn on_commit(user: *mut c_void, text: *const c_char) {
        let s = unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned();
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
        let raw = unsafe { CStr::from_ptr(raw) }
            .to_string_lossy()
            .into_owned();
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
    fn mock_server(
        path: PathBuf,
        responses: Vec<serde_json::Value>,
    ) -> std::thread::JoinHandle<()> {
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
        assert_eq!(cap.updates[0].2, vec!["的".to_string(), "得".to_string()]);
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
    fn select_candidate_commits() {
        let mut cap = Box::new(Capture::default());
        let cap_ptr: *mut Capture = &mut *cap;
        let path = test_sock("select");
        let handle = mock_server(
            path.clone(),
            vec![serde_json::json!({
                "outcome": {
                    "consumed": true,
                    "commit": "衣",
                    "state": {
                        "raw": "", "preedit": "", "candidates": [],
                        "page": 0, "page_count": 0, "mode": "Normal", "chinese": true,
                        "full_shape": false, "ascii_punct": false
                    }
                },
                "state": {"raw": "", "candidates": [], "page": 0, "page_count": 0,
                          "mode": "Normal", "chinese": true,
                          "full_shape": false, "ascii_punct": false}
            })],
        );
        let mut c = client_for(path, cap_ptr);
        assert!(c.select(0), "select 应返回已处理");
        assert_eq!(cap.commits.as_slice(), ["衣"]);
        assert_eq!(cap.updates.len(), 1);
        handle.join().unwrap();
    }

    /// mock server（带请求捕获）：返回 (句柄, 收到的请求列表)。
    fn mock_server_capture(
        path: PathBuf,
        responses: Vec<serde_json::Value>,
    ) -> (
        std::thread::JoinHandle<()>,
        std::sync::Arc<Mutex<Vec<serde_json::Value>>>,
    ) {
        let listener = UnixListener::bind(&path).expect("bind");
        let captured = std::sync::Arc::new(Mutex::new(Vec::new()));
        let cap2 = captured.clone();
        let handle = std::thread::spawn(move || {
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
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf) {
                    cap2.lock().unwrap().push(v);
                }
                let body = serde_json::to_vec(&resp).unwrap();
                let _ = s.write_all(&(body.len() as u32).to_le_bytes());
                let _ = s.write_all(&body);
            }
            let _ = std::fs::remove_file(&path);
        });
        (handle, captured)
    }

    #[test]
    fn config_roundtrip_merge_and_get() {
        let mut cap = Box::new(Capture::default());
        let cap_ptr: *mut Capture = &mut *cap;
        let path = test_sock("config");
        let (handle, captured) = mock_server_capture(
            path.clone(),
            vec![
                serde_json::json!({
                    "config": {
                        "candidates": {"page_size": 4, "show_split": true},
                        "input": {"alphabet": "abc", "enter_clear": false},
                        "sound": {"enabled": false, "volume": 50}
                    }
                }),
                serde_json::json!({"ok": true}),
            ],
        );
        let mut c = client_for(path, cap_ptr);
        assert!(c.refresh_config(), "拉取配置");
        assert_eq!(c.config_int("candidates.page_size"), Some(4));
        assert_eq!(c.config_bool("candidates.show_split"), 1);
        assert_eq!(c.config_bool("不存在的路径"), -1);

        // 补丁：改 page_size/sound，其余字段必须保留（深合并）
        assert!(c.config_patch(&serde_json::json!({
            "candidates": {"page_size": 6},
            "sound": {"enabled": true},
        })));
        assert_eq!(c.config_int("candidates.page_size"), Some(6), "缓存更新");
        let reqs = captured.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        let posted = &reqs[1]["config"];
        assert_eq!(posted["candidates"]["page_size"], 6);
        assert_eq!(
            posted["candidates"]["show_split"], true,
            "未提及字段须保留（读-改-写）"
        );
        assert_eq!(posted["input"]["alphabet"], "abc");
        assert_eq!(posted["sound"]["enabled"], true);
        drop(reqs);
        assert_eq!(c.config_bool("sound.enabled"), 1);
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

    #[test]
    fn reload_schema_and_open_dir_requests() {
        let mut cap = Box::new(Capture::default());
        let cap_ptr: *mut Capture = &mut *cap;
        let path = test_sock("menu-ops");
        let (handle, captured) = mock_server_capture(
            path.clone(),
            vec![
                serde_json::json!({"ok": true, "current": "虎整句"}),
                serde_json::json!({"ok": true, "path": "/home/u/.local/share/hufu/码表/虎整句"}),
                serde_json::json!({"ok": false, "path": "/不存在"}),
            ],
        );
        let mut c = client_for(path, cap_ptr);
        assert!(c.reload_schema(), "重载码表：引擎回 ok=true 即成功");
        assert!(c.open_schema_dir(), "打开方案文件夹：ok=true 即成功");
        assert!(!c.open_schema_dir(), "ok=false（方案目录不存在）按失败处理");
        let reqs = captured.lock().unwrap();
        assert_eq!(reqs.len(), 3);
        assert_eq!(reqs[0]["op"], "reload_schema");
        assert_eq!(reqs[1]["op"], "open_schema_dir");
        assert_eq!(reqs[2]["op"], "open_schema_dir");
        drop(reqs);
        handle.join().unwrap();
    }

    #[test]
    fn sound_toggle_and_state() {
        let mut cap = Box::new(Capture::default());
        let cap_ptr: *mut Capture = &mut *cap;
        let path = test_sock("sound");
        let (handle, captured) = mock_server_capture(
            path.clone(),
            vec![
                serde_json::json!({"enabled": false, "volume": 50}),
                serde_json::json!({"enabled": true}),
                serde_json::json!({"enabled": true, "volume": 50}),
                serde_json::json!({}),
            ],
        );
        let mut c = client_for(path, cap_ptr);
        assert_eq!(c.sound_state(), 0, "默认关");
        assert_eq!(c.sound_toggle(), 1, "取反返回新态（开）");
        assert_eq!(c.sound_state(), 1, "再读=开");
        assert_eq!(c.sound_state(), -1, "回包缺 enabled 字段=未知");
        let reqs = captured.lock().unwrap();
        assert_eq!(reqs.len(), 4);
        assert_eq!(reqs[0]["op"], "sound_state");
        assert_eq!(reqs[1]["op"], "sound_toggle");
        assert_eq!(reqs[2]["op"], "sound_state");
        assert_eq!(reqs[3]["op"], "sound_state");
        drop(reqs);
        handle.join().unwrap();
    }

    #[test]
    fn menu_ops_offline_semantics() {
        let mut cap = Box::new(Capture::default());
        let cap_ptr: *mut Capture = &mut *cap;
        let path = test_sock("menu-offline");
        let mut c = client_for(path, cap_ptr);
        // 引擎不在线：动作类（1/0）回 0，状态类回 -1（未知）——宿主保持现状、不崩。
        assert!(!c.reload_schema());
        assert!(!c.open_schema_dir());
        assert_eq!(c.sound_toggle(), -1);
        assert_eq!(c.sound_state(), -1);
    }

    /// 标准 base64 编码（测试侧的独立实现：给 mock 造真实回包，不与被测解码器共用代码）。
    fn b64(data: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            out.push(T[(n >> 18) as usize & 63] as char);
            out.push(T[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                T[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                T[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    /// base64 解码：对已知向量（外部可核对的编码）与非法输入的行为。
    #[test]
    fn base64_decode_known_vectors_and_bad_input() {
        assert_eq!(base64_decode("UklGRg==").unwrap(), b"RIFF");
        assert_eq!(base64_decode("V0FWRQ==").unwrap(), b"WAVE");
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("AAECAwQ=").unwrap(), vec![0, 1, 2, 3, 4]);
        assert_eq!(base64_decode("+/8=").unwrap(), vec![0xfb, 0xff]);
        // 非法输入一律 None（宁可不播，也不把半截字节当 WAV）。
        assert!(base64_decode("").is_none(), "空串");
        assert!(base64_decode("AAA").is_none(), "长度不是 4 的倍数");
        assert!(base64_decode("AA*A").is_none(), "非法字符");
        assert!(base64_decode("A=AA").is_none(), "填充不在末尾");
        assert!(base64_decode("AA=A").is_none(), "填充后还有数据");
        assert!(base64_decode("====").is_none(), "整组填充");
    }

    /// 音效通道：回包含 `outcome.sound` ⇒ `take_sound` 得到 tag，且只取一次。
    #[test]
    fn sound_tag_taken_once() {
        let mut cap = Box::new(Capture::default());
        let cap_ptr: *mut Capture = &mut *cap;
        let path = test_sock("sound-tag");
        let (handle, captured) = mock_server_capture(
            path.clone(),
            vec![
                serde_json::json!({
                    "outcome": {"consumed": true, "sound": "key"},
                    "state": {"raw": "u", "candidates": [], "page": 0, "page_count": 0,
                              "mode": "Normal", "chinese": true}
                }),
                serde_json::json!({
                    "outcome": {"consumed": true},
                    "state": {"raw": "", "candidates": [], "page": 0, "page_count": 0,
                              "mode": "Normal", "chinese": true}
                }),
            ],
        );
        let mut c = client_for(path, cap_ptr);
        let cptr: *mut HufuClient = &mut c;
        assert_eq!(
            cstr_of(unsafe { hufu_client_take_sound(cptr) }),
            "",
            "还没按键"
        );
        c.key("u", false, false, false, false, false, -1);
        assert_eq!(
            cstr_of(unsafe { hufu_client_take_sound(cptr) }),
            "key",
            "回包里的 tag"
        );
        assert_eq!(
            cstr_of(unsafe { hufu_client_take_sound(cptr) }),
            "",
            "取走即清空"
        );
        // 第二个回包没有 sound 字段：待处理保持为空，不会把上一个 tag 再送一遍。
        c.key("i", false, false, false, false, false, -1);
        assert_eq!(cstr_of(unsafe { hufu_client_take_sound(cptr) }), "");
        let reqs = captured.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0]["op"], "key");
        drop(reqs);
        handle.join().unwrap();
    }

    /// 音效通道：`sound_fetch` 成功返回音量，字节与 base64 原文逐字节一致；
    /// 同 tag 重复取用时字节走缓存，音量**每次现取**（改设置即刻生效，无需翻转开关）。
    #[test]
    fn sound_fetch_returns_volume_and_bytes() {
        // 假 WAV：RIFF 头 + 一段含 0x00 与高位字节的负载（验证解码不截断二进制）。
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&[0x24, 0x00, 0x00, 0x00]);
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&[0x10, 0x00, 0x00, 0x00]);
        wav.extend(0u8..=63);
        let encoded = b64(&wav);

        let mut cap = Box::new(Capture::default());
        let cap_ptr: *mut Capture = &mut *cap;
        let path = test_sock("sound-fetch");
        let (handle, captured) = mock_server_capture(
            path.clone(),
            vec![
                serde_json::json!({"data": encoded, "volume": 42}),
                // 第二次的音量：与首次不同，用来证明「现取」而不是回放旧缓存。
                serde_json::json!({"enabled": true, "volume": 77}),
            ],
        );
        let mut c = client_for(path, cap_ptr);
        let cptr: *mut HufuClient = &mut c;
        assert_eq!(
            unsafe { hufu_client_sound_fetch(cptr, c"key".as_ptr()) },
            42,
            "成功返回引擎音量"
        );
        let size = unsafe { hufu_client_sound_size(cptr) };
        assert_eq!(size as usize, wav.len(), "长度=WAV 字节数");
        let data = unsafe { hufu_client_sound_data(cptr) };
        assert!(!data.is_null());
        let got = unsafe { std::slice::from_raw_parts(data, size as usize) };
        assert_eq!(got, wav.as_slice(), "字节与 base64 原文一致");
        // 同 tag 第二次：字节走缓存（不再发 sound op），音量另取一次 sound_state。
        assert_eq!(
            unsafe { hufu_client_sound_fetch(cptr, c"key".as_ptr()) },
            77,
            "音量随引擎现值生效，不必等开关翻转"
        );
        let size2 = unsafe { hufu_client_sound_size(cptr) };
        let data2 = unsafe { hufu_client_sound_data(cptr) };
        assert_eq!(size2, size, "缓存命中的字节数不变");
        let got2 = unsafe { std::slice::from_raw_parts(data2, size2 as usize) };
        assert_eq!(got2, wav.as_slice(), "缓存命中的字节仍是同一份 WAV");
        let reqs = captured.lock().unwrap();
        assert_eq!(reqs.len(), 2, "第二次只补一个音量小回包");
        assert_eq!(reqs[0]["op"], "sound");
        assert_eq!(reqs[0]["tag"], "key");
        assert_eq!(reqs[1]["op"], "sound_state");
        drop(reqs);
        handle.join().unwrap();
    }

    /// 音效通道：音量现取失败（引擎挂起/回包缺 volume）时沿用上一次已知音量，
    /// 已在缓存里的音效照常出声——引擎的短暂沉默不该变成突然静音。
    #[test]
    fn sound_volume_falls_back_when_state_unavailable() {
        let wav = b"RIFF\x24\x00\x00\x00WAVEfmt \x10\x00\x00\x00".to_vec();
        let encoded = b64(&wav);
        let mut cap = Box::new(Capture::default());
        let cap_ptr: *mut Capture = &mut *cap;
        let path = test_sock("sound-volume-fallback");
        let (handle, _captured) = mock_server_capture(
            path.clone(),
            vec![
                serde_json::json!({"data": encoded, "volume": 33}),
                serde_json::json!({"enabled": true}), // 回包没有 volume
            ],
        );
        let mut c = client_for(path, cap_ptr);
        let cptr: *mut HufuClient = &mut c;
        assert_eq!(
            unsafe { hufu_client_sound_fetch(cptr, c"key".as_ptr()) },
            33
        );
        assert_eq!(
            unsafe { hufu_client_sound_fetch(cptr, c"key".as_ptr()) },
            33,
            "取不到现值就沿用上次音量"
        );
        assert_eq!(unsafe { hufu_client_sound_size(cptr) } as usize, wav.len());
        handle.join().unwrap();
    }

    /// 音效通道的失败面：`data:null`（音效文件缺失）/ 未知 tag / 空 tag / 引擎不在线
    /// 都返回 -1 且不产出字节；`c==NULL` 的导出返回 NULL/0，不崩。
    #[test]
    fn sound_fetch_failures_are_silent() {
        let mut cap = Box::new(Capture::default());
        let cap_ptr: *mut Capture = &mut *cap;
        let path = test_sock("sound-fail");
        let (handle, captured) = mock_server_capture(
            path.clone(),
            vec![
                serde_json::json!({"data": null, "volume": 50}), // 音效文件缺失
                serde_json::json!({"error": "未知音效"}),        // 引擎白名单外的 tag
            ],
        );
        let mut c = client_for(path, cap_ptr);
        let cptr: *mut HufuClient = &mut c;
        assert_eq!(
            unsafe { hufu_client_sound_fetch(cptr, c"key".as_ptr()) },
            -1,
            "data:null"
        );
        assert_eq!(unsafe { hufu_client_sound_size(cptr) }, 0, "失败不产出字节");
        assert!(unsafe { hufu_client_sound_data(cptr) }.is_null());
        assert_eq!(
            unsafe { hufu_client_sound_fetch(cptr, c"bogus".as_ptr()) },
            -1,
            "未知 tag"
        );
        assert_eq!(
            unsafe { hufu_client_sound_fetch(cptr, c"".as_ptr()) },
            -1,
            "空 tag"
        );
        let reqs = captured.lock().unwrap();
        assert_eq!(reqs.len(), 2, "空 tag 不发请求");
        drop(reqs);
        handle.join().unwrap();

        // 引擎不在线：同样 -1，不 panic。
        let mut off = client_for(test_sock("sound-offline"), cap_ptr);
        let optr: *mut HufuClient = &mut off;
        assert_eq!(
            unsafe { hufu_client_sound_fetch(optr, c"key".as_ptr()) },
            -1
        );
        assert_eq!(unsafe { hufu_client_sound_size(optr) }, 0);
        assert_eq!(cstr_of(unsafe { hufu_client_take_sound(optr) }), "");

        // NULL 句柄：导出按约定早退。
        assert!(unsafe { hufu_client_take_sound(std::ptr::null_mut()) }.is_null());
        assert!(unsafe { hufu_client_sound_data(std::ptr::null()) }.is_null());
        assert_eq!(unsafe { hufu_client_sound_size(std::ptr::null()) }, 0);
        assert_eq!(
            unsafe { hufu_client_sound_fetch(std::ptr::null_mut(), c"key".as_ptr()) },
            -1
        );
    }

    /// 测试用数据根目录（按 `$HUFU_ROOT` 布局造小样本；Drop 时整树删除）。
    struct TempDataDir(PathBuf);

    impl TempDataDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "hufu-char-lookup-{}-{}",
                std::process::id(),
                name
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("建临时数据目录");
            TempDataDir(dir)
        }

        /// 写一份样本数据（相对路径按 `$HUFU_ROOT` 布局，如 `数据/注释/拼音.注释`）。
        fn write(&self, rel: &str, content: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().expect("父目录")).expect("建父目录");
            std::fs::write(&path, content).expect("写样本数据");
        }
    }

    impl Drop for TempDataDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 一个不连引擎的客户端（字反查不碰 socket）。
    fn offline_client(name: &str) -> HufuClient {
        HufuClient::new(
            test_sock(name),
            HufuHost {
                user: std::ptr::null_mut(),
                commit: None,
                update: None,
            },
        )
    }

    /// 取 C 指针里的字符串（`hufu_client_char_lookup` 的返回）。
    fn cstr_of(p: *const c_char) -> String {
        if p.is_null() {
            return String::new();
        }
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }

    /// 正常数据：三列齐全（拼音原样、同字多码以 `/` 连接、拆分追加为第三列）。
    #[test]
    fn char_lookup_reads_pinyin_codes_and_split() {
        let dir = TempDataDir::new("ok");
        // 拼音注释带 CRLF（随包资源如此）与一行「尾随空列」的坏行。
        dir.write(
            PINYIN_ANNOTATION,
            "中\tzhōng zhòng\r\n的\tdí dì de\r\n𱊳\tlěi\t\r\n坏行没有制表符\r\n多字键\tcuò\r\n",
        );
        dir.write(
            CODE_DICT,
            "\nname: tiger\nsort: by_weight\ncolumns:\n  - text\n  - code\n  - weight\n...\n\n\
             中\td\t900\n中\tdgs\t900\n的\tu\t800\n的\tuni\t800\n# 注释行\n坏行\n",
        );
        dir.write(SPLIT_ANNOTATION, "中\t口丨\r\n");

        let mut c = offline_client("lookup-ok");
        assert!(c.char_lookup_init(&dir.0), "三份数据齐全应可用");
        assert_eq!(
            cstr_of(c.char_lookup('中' as u32)),
            "zhōng zhòng\td/dgs\t口丨",
            "拼音原样 + 同字多码 `/` 连接 + 拆分第三列"
        );
        assert_eq!(cstr_of(c.char_lookup('的' as u32)), "dí dì de\tu/uni");
        assert_eq!(
            cstr_of(c.char_lookup('𱊳' as u32)),
            "lěi\t",
            "尾随空列的行仍可用"
        );
        assert_eq!(cstr_of(c.char_lookup('多' as u32)), "", "多字键的行被跳过");
        assert_eq!(
            cstr_of(c.char_lookup('龘' as u32)),
            "",
            "三份数据都没有的字"
        );
    }

    /// 缺文件：目录不存在 / 只有拼音注释（码表缺）都要优雅降级，不 panic。
    #[test]
    fn char_lookup_missing_files_degrade() {
        let missing =
            std::env::temp_dir().join(format!("hufu-char-lookup-{}-none", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);
        let mut c = offline_client("lookup-missing");
        assert!(!c.char_lookup_init(&missing), "目录不存在应判不可用");
        assert_eq!(cstr_of(c.char_lookup('中' as u32)), "", "未装载时返回空串");

        let dir = TempDataDir::new("pinyin-only");
        dir.write(PINYIN_ANNOTATION, "中\tzhōng\n");
        assert!(c.char_lookup_init(&dir.0), "拼音注释在即算可用");
        assert_eq!(
            cstr_of(c.char_lookup('中' as u32)),
            "zhōng\t",
            "码表缺失：虎码列留空"
        );
        // 重复初始化按新目录重载（同一客户端换数据目录）。
        assert!(!c.char_lookup_init(&missing), "重载到坏目录应清空索引");
        assert_eq!(cstr_of(c.char_lookup('中' as u32)), "");
    }

    /// 缺列/异常行：`columns` 列名不对时整表按「无码表」降级；坏行不影响其余数据。
    #[test]
    fn char_lookup_bad_columns_and_rows() {
        let dir = TempDataDir::new("bad");
        dir.write(PINYIN_ANNOTATION, "中\tzhōng\n");
        // 列名对不上（word/stroke）⇒ 不按位置硬认，码表整体不可用。
        dir.write(
            CODE_DICT,
            "name: tiger\ncolumns:\n  - word\n  - stroke\n...\n中\td\n",
        );
        let mut c = offline_client("lookup-bad-cols");
        assert!(c.char_lookup_init(&dir.0), "拼音注释可用 ⇒ 索引仍可用");
        assert_eq!(
            cstr_of(c.char_lookup('中' as u32)),
            "zhōng\t",
            "码表列名不对 ⇒ 无虎码列"
        );

        // 列序不同（code 在 text 之前）：按列名下标的真实位置取，不认死列位。
        dir.write(
            CODE_DICT,
            "name: tiger\ncolumns:\n  - code\n  - text\n...\ndgs\t中\n",
        );
        assert!(c.char_lookup_init(&dir.0));
        assert_eq!(
            cstr_of(c.char_lookup('中' as u32)),
            "zhōng\tdgs",
            "按 columns 声明的列位取字与码"
        );
    }
}
