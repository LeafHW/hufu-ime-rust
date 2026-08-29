//! TSF 文本服务：按键 → 管道引擎 → 组段/上屏 + 候选窗。

use crate::candwin::CandidateWindow;
use crate::candwin2::CandidateWindowV2;
use crate::ipc;
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::MapWindowPoints;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::TextServices::*;
use windows_core::*;

type SharedRef = Arc<Mutex<Shared>>;

/// 线程共享状态（文本服务 / 按键接收 / 编辑会话共用）。
pub struct Shared {
    pub thread_mgr: Option<ITfThreadMgr>,
    pub client_id: u32,
    pub composition: Option<ITfComposition>,
    /// v2（DComp+Acrylic）初始化失败 → 回退 v1
    pub cand2: Option<CandidateWindowV2>,
    pub cand2_dead: bool,
    pub cand: Option<CandidateWindow>,
    pub skin: serde_json::Value,
    /// 会话结束后重新拉皮肤
    pub skin_stale: bool,
    /// 候选延时显示（candidates.delay_show_ms）：raw 变更后该毫秒内抑制候选窗（防闪烁）
    pub delay_show_ms: u32,
    /// 上次 raw（变化检测）
    pub raw_last: String,
    /// raw 最近一次变化时刻
    pub raw_changed_at: Option<std::time::Instant>,
    /// 插入点（屏幕坐标，GetTextExt 实测）
    pub caret: Option<RECT>,
    /// 缓存引擎态：中文模式 / 编码中（TestKeyDown 本地预判用，免双发引擎）
    pub chinese: bool,
    pub composing: bool,
    /// 最近一次 preedit（失焦冲销用）
    pub preedit_last: String,
    /// 线程焦点事件 sink cookie（Deactivate 反注册用）
    pub tm_sink_cookie: u32,
}

impl Shared {
    fn new() -> Shared {
        Shared {
            thread_mgr: None,
            client_id: 0,
            composition: None,
            cand2: None,
            cand2_dead: false,
            cand: None,
            skin: serde_json::Value::Null,
            skin_stale: true,
            delay_show_ms: 0,
            raw_last: String::new(),
            raw_changed_at: None,
            caret: None,
            chinese: true,
            composing: false,
            preedit_last: String::new(),
            tm_sink_cookie: 0,
        }
    }

    fn load_skin(&mut self) {
        // 编码会话开始（raw 空）时重新拉取皮肤 —— 设置界面改皮肤后，
        // 下一次打字即生效（近热更新）
        if self.skin.is_null() || self.skin_stale {
            if let Some(v) = ipc::call(&serde_json::json!({"op": "skin"})) {
                self.delay_show_ms = v
                    .get("delay_show_ms")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0) as u32;
                self.skin = v;
                self.skin_stale = false;
            }
        }
    }

    /// 焦点上下文（当前文档顶层）。
    fn focus_context(&self) -> Option<ITfContext> {
        let tm = self.thread_mgr.as_ref()?;
        let doc: ITfDocumentMgr = unsafe { tm.GetFocus().ok()? };
        unsafe { doc.GetTop().ok() }
    }
}

/// ── 文本服务（同时实现按键接收器：msctf 要求 fforeground sink 支持
///    ITfTextInputProcessor，因此 TIP 对象自身实现 ITfKeyEventSink）──
#[implement(ITfTextInputProcessor, ITfTextInputProcessorEx, ITfKeyEventSink, ITfThreadMgrEventSink)]
pub struct HuFuTs {
    shared: SharedRef,
}

impl HuFuTs {
    pub fn new() -> HuFuTs {
        HuFuTs {
            shared: Arc::new(Mutex::new(Shared::new())),
        }
    }
}

impl ITfTextInputProcessor_Impl for HuFuTs_Impl {
    fn Activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> Result<()> {
        let tm = ptim
            .cloned()
            .ok_or_else(|| Error::from(HRESULT(-2147467259)))?;
        let km: ITfKeystrokeMgr = tm.cast()?;
        // 把「自己」注册为前景按键接收器
        let sink: ITfKeyEventSink = unsafe { self.cast()? };
        unsafe {
            km.AdviseKeyEventSink(tid, &sink, BOOL(1))?;
            // 文档焦点事件：失焦冲销会话+关候选窗（修「切窗后候选不关/回不来」）
            {
                let tm_sink: ITfThreadMgrEventSink = unsafe { self.cast()? };
                if let Ok(src) = tm.cast::<ITfSource>() {
                    let unk: IUnknown = tm_sink.cast()?;
                    if let Ok(cookie) =
                        unsafe { src.AdviseSink(&ITfThreadMgrEventSink::IID, Some(&unk)) }
                    {
                        self.shared.lock().unwrap().tm_sink_cookie = cookie;
                    }
                }
            }
            // 输入法默认「开+中文」：部分应用读 OPENCLOSE 档位决定是否走 IME，
            // 不设会表现为「先按一下 Shift 才能打中文」
            if let Ok(cm) = tm.cast::<ITfCompartmentMgr>() {
                if let Ok(comp) = unsafe { cm.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE) } {
                    let v = VARIANT::from(1i32);
                    let _ = unsafe { comp.SetValue(tid, &v) };
                }
            }
        }
        let mut g = self.shared.lock().unwrap();
        g.thread_mgr = Some(tm);
        g.client_id = tid;
        // （语言栏按钮已下线：Win11 桌面语言栏是可拖动浮动条而非任务栏
        // 常驻，且小尺寸渲染差——用户实测否决。输入指示「中」改由
        // DLL 内嵌图标资源承担（build.rs + assets/hufu_rsrc.o）。
        // langbar.rs 源码保留，备将来做中/英态切换。）
        // 激活标记（冒烟测试读取：证明 msctf 真实激活管线走到了这里）
        let marker = std::env::temp_dir().join("hufu-tsf-activated.txt");
        let _ = std::fs::write(&marker, format!("tid={tid} t={:?}\n", std::time::SystemTime::now()));
        // 上报激活：托盘图标「仅虎符激活时显示」
        let _ = crate::ipc::call(&serde_json::json!({"op": "ime", "active": true}));
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        // 上报失活（托盘侧 700ms 防抖后隐藏图标）
        let _ = crate::ipc::call(&serde_json::json!({"op": "ime", "active": false}));
        let mut g = self.shared.lock().unwrap();
        if let Some(tm) = g.thread_mgr.clone() {
            if let Ok(km) = tm.cast::<ITfKeystrokeMgr>() {
                unsafe {
                    let _ = km.UnadviseKeyEventSink(g.client_id);
                }
            }
            if let Ok(src) = tm.cast::<ITfSource>() {
                let cookie = g.tm_sink_cookie;
                if cookie != 0 {
                    unsafe {
                        let _ = src.UnadviseSink(cookie);
                    }
                    g.tm_sink_cookie = 0;
                }
            }
        }
        g.composition = None;
        if let Some(mut c) = g.cand2.take() {
            c.hide();
        }
        g.cand = None;
        g.thread_mgr = None;
        g.client_id = 0;
        Ok(())
    }
}

impl ITfTextInputProcessorEx_Impl for HuFuTs_Impl {
    fn ActivateEx(&self, ptim: Option<&ITfThreadMgr>, tid: u32, _flags: u32) -> Result<()> {
        self.Activate(ptim, tid)
    }
}

impl ITfKeyEventSink_Impl for HuFuTs_Impl {
    fn OnSetFocus(&self, _fforeground: BOOL) -> Result<()> {
        Ok(())
    }

    fn OnTestKeyDown(&self, _pic: Option<&ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        Ok(self.dispatch(wparam.0, true))
    }

    fn OnTestKeyUp(&self, _pic: Option<&ITfContext>, _wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        Ok(BOOL(0))
    }

    fn OnKeyDown(&self, _pic: Option<&ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        Ok(self.dispatch(wparam.0, false))
    }

    fn OnKeyUp(&self, _pic: Option<&ITfContext>, _wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        // 键音「松开即停」：截断当前正在响的键音（打字机手感）
        crate::sound::key_up();
        Ok(BOOL(0))
    }

    fn OnPreservedKey(&self, _pic: Option<&ITfContext>, _rguid: *const GUID) -> Result<BOOL> {
        Ok(BOOL(0))
    }
}

impl ITfThreadMgrEventSink_Impl for HuFuTs_Impl {
    fn OnInitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }

    fn OnUninitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }

    /// 文档焦点变化（切应用 / 切输入框 / 失焦为 None）。
    /// 统一策略：
    /// 1. 有未上屏内容 → 在「旧」文档上下文里提交（显式 ctx，防写错应用）。
    ///    焦点切换期应用持有文档锁，同步授权会被拒（trace 实证 TF_E_SYNCHRONOUS
    ///    0x80040209，全为 Chromium 系应用）→ run_session 内置多档回退，
    ///    并把提交挪到工作线程异步落账，绝不阻塞焦点回调。
    /// 2. 引擎会话清零 + 本地组段句柄必须丢弃 ——
    ///    切窗时系统已终止组段，留着死句柄会让后续 SetText 全失败，
    ///    表现为「回来后有候选但中文永远上不了屏、只有字母能直通」
    fn OnSetFocus(
        &self,
        pdimfocus: Option<&ITfDocumentMgr>,
        pdimprevfocus: Option<&ITfDocumentMgr>,
    ) -> Result<()> {
        let _ = pdimfocus;
        let (composing, preedit) = {
            let g = self.shared.lock().unwrap();
            (g.composing, g.preedit_last.clone())
        };
        trace(&format!(
            "OnSetFocus: composing={composing} preedit='{}' prev={:?}",
            preedit,
            pdimprevfocus.is_some()
        ));
        // 1) 旧文档上冲销提交：显式 prev ctx；ASYNCDONTCARE 授权拿不到同步就排队，
        //    焦点回调里绝不因 TF_E_SYNCHRONOUS 丢文本（此前 Chromium 系应用实证）
        if composing && !preedit.is_empty() {
            let prev_ctx = pdimprevfocus.and_then(|d| unsafe { d.GetTop().ok() });
            if let Some(ctx) = prev_ctx {
                let _ = run_session(&self.shared, Op::Commit(preedit.clone()), Some(ctx));
            }
        }
        // 2) 引擎会话清零（服务端 focus op 清空缓冲；本地缓存全部复位）
        let _ = ipc::call(&serde_json::json!({ "op": "focus" }));
        {
            let mut g = self.shared.lock().unwrap();
            g.composition = None; // 死组段句柄必须丢，后续走全新 StartPreedit
            g.composing = false;
            g.raw_last.clear();
            g.preedit_last.clear();
            g.skin_stale = true; // 新焦点重新拉皮肤（也许用户刚改）
            if let Some(c) = g.cand2.as_mut() {
                c.hide();
            }
            if let Some(c) = g.cand.take() {
                c.hide();
            }
        }
        Ok(())
    }

    fn OnPushContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        Ok(())
    }

    fn OnPopContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        Ok(())
    }
}

/// 轨迹日志（诊断 UI 线程卡死）：追加到 %TEMP%\hufu-tsf-trace.log。
/// 环境变量 HUFU_TRACE=0 可关。多进程各写各行（带进程名）。
pub fn trace(msg: &str) {
    use std::io::Write;
    if std::env::var("HUFU_TRACE").as_deref() == Ok("0") {
        return;
    }
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = std::env::temp_dir().join("hufu-tsf-trace.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{t}] {exe}: {msg}");
    }
}

impl HuFuTs_Impl {
    /// 键分派：VK → 名称+修饰 → 管道引擎 → 更新组段与候选窗。
    fn dispatch(&self, wparam: usize, test_only: bool) -> BOOL {
        // TestKeyDown：本地预判（缓存引擎态），不碰管道——
        // 否则同一键会被引擎处理两次（Test + Down 各一次）
        if test_only {
            let (chinese, composing) = {
                let g = self.shared.lock().unwrap();
                (g.chinese, g.composing)
            };
            let Some((name, shift, ctrl, alt)) = vk_to_name(wparam) else {
                return BOOL(0);
            };
            let _ = (shift, alt);
            // Ctrl+M 切方案 / Ctrl+Space 切中英：先声明按键，真实处理在 KeyDown
            // 由引擎定夺（未启用时引擎不吞，KeyDown 返回直通）。
            if ctrl && !shift && !alt && (name == "m" || name == "space") {
                return BOOL(1);
            }
            if ctrl {
                return BOOL(0); // 其余组合键直通（Ctrl+Shift+V 剪贴板在 KeyDown 处理）
            }
            let will = chinese
                && match name.as_str() {
                    // 编码中：可打印键与控制键都可能被吞
                    _ if composing => true,
                    // 空闲：编码字母/分号/引号会起段
                    "space" | "enter" | "escape" | "backspace" | "tab" => false,
                    n if n.len() == 1 => true, // 单字符（字母/数字/标点）
                    _ => false,
                };
            return BOOL(will as i32);
        }
        trace(&format!("dispatch vk=0x{wparam:X}"));
        // Ctrl+Shift+V：剪贴板上屏（配置+白名单由 server 判定）
        if wparam == 0x56 {
            unsafe {
                let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                let alt = GetKeyState(VK_MENU.0 as i32) < 0;
                if ctrl && shift && !alt {
                    return self.paste_clipboard(test_only);
                }
            }
        }
        let Some((name, shift, ctrl, alt)) = vk_to_name(wparam) else {
            return BOOL(0);
        };
        let (name, m_shift, m_ctrl, m_alt) = match name.as_str() {
            "shift" | "ctrl" | "alt" => (name, false, false, false),
            _ => (name, shift, ctrl, alt),
        };
        let Some((consumed, commit, state, sound)) =
            ipc::key_request(&name, m_shift, m_ctrl, m_alt)
        else {
            return BOOL(0);
        };
        trace(&format!("pipe back consumed={consumed}"));
        if !consumed {
            return BOOL(0);
        }
        if !test_only {
            if let Some(tag) = sound {
                crate::sound::play(&tag);
            }
            trace("before update_ui");
            let _ = update_ui(self.shared.clone(), commit, state);
            trace("after update_ui");
        }
        BOOL(1)
    }

    /// Ctrl+Shift+V 剪贴板上屏：管道取文本（server 校验配置/白名单），
    /// 有文本则插入光标处并吞键。
    fn paste_clipboard(&self, test_only: bool) -> BOOL {
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let Some(text) = ipc::clipboard_request(&exe) else {
            return BOOL(0);
        };
        if text.is_empty() {
            return BOOL(0); // 未启用/白名单拒绝/剪贴板空 → 交给系统 Ctrl+Shift+V
        }
        if !test_only {
            let _ = run_session(&self.shared, Op::Insert(text), None);
        }
        BOOL(1)
    }
}

/// 冒烟测试直驱：vk_to_name + 管道引擎往返（不经 msctf/组段/候选窗）。
pub fn test_key(vk: u32) -> i32 {
    let Some((name, shift, ctrl, alt)) = vk_to_name(vk as usize) else {
        eprintln!("hufu-tsf: test_key vk={vk} 无映射");
        return 0;
    };
    let _ = (shift, ctrl, alt);
    let r = ipc::key_request(&name, false, false, false);
    match r {
        Some((consumed, _commit, _state, _sound)) => {
            eprintln!("hufu-tsf: test_key '{name}' → consumed={consumed}");
            if consumed { 1 } else { 0 }
        }
        None => {
            eprintln!("hufu-tsf: test_key '{name}' → 管道失败");
            0
        }
    }
}

/// VK → (引擎键名, shift, ctrl, alt)。不可识别返回 None（直通）。
fn vk_to_name(vk: usize) -> Option<(String, bool, bool, bool)> {
    unsafe {
        let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
        let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
        let alt = GetKeyState(VK_MENU.0 as i32) < 0;
        let name = match vk {
            0x20 => "space".to_string(),
            0x0D => "enter".to_string(),
            0x08 => "backspace".to_string(),
            0x09 => "tab".to_string(),
            0x1B => "escape".to_string(),
            0x14 => "capslock".to_string(),
            0x10 => "shift".to_string(),
            0x11 => "ctrl".to_string(),
            0x12 => "alt".to_string(),
            0x25 => "left".to_string(),
            0x26 => "up".to_string(),
            0x27 => "right".to_string(),
            0x28 => "down".to_string(),
            0x21 => "pageup".to_string(),
            0x22 => "pagedown".to_string(),
            0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk as u32 | 32)
                .unwrap_or(' ')
                .to_string(),
            0xBA => ";".to_string(),
            0xBB => "=".to_string(),
            0xBC => ",".to_string(),
            0xBD => "-".to_string(),
            0xBE => ".".to_string(),
            0xBF => "/".to_string(),
            0xC0 => "`".to_string(),
            0xDE => "'".to_string(),
            _ => return None,
        };
        Some((name, shift, ctrl, alt))
    }
}

/// ── 编辑会话 ──────────────────────────────────────────────
enum Op {
    StartPreedit(String),
    SetPreedit(String),
    Commit(String),
    /// 提前上屏：先提交前缀（结束当前组段），再开新组段继续显示剩余
    CommitAndRepreedit(String, String),
    /// 无组段直接插入文本（剪贴板上屏）
    Insert(String),
    End,
}

#[implement(ITfEditSession)]
struct EditSession {
    shared: SharedRef,
    op: Op,
    /// 显式目标上下文（失焦冲销 = 旧文档；None = 当前焦点）。
    ctx_override: Option<ITfContext>,
}

impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        trace("DoEditSession enter");
        let r = self.do_edit_session(ec);
        trace(&format!("DoEditSession exit ok={}", r.is_ok()));
        r
    }
}

impl EditSession_Impl {
    fn do_edit_session(&self, ec: u32) -> Result<()> {
        let mut g = self.shared.lock().unwrap();
        let ctx = match self.ctx_override.clone() {
            Some(c) => c,
            None => g
                .focus_context()
                .ok_or_else(|| Error::from(HRESULT(-2147467259)))?,
        };
        match &self.op {
            Op::StartPreedit(text) => {
                // 标准 IME 流程：选区范围 → StartComposition → 组段内 SetText。
                // （InsertTextAtSelection 在真实应用上下文会报 TF_E_SYNCHRONOUS）
                let cc: ITfContextComposition = ctx.cast()?;
                trace("SP: cast ok");
                let range: ITfRange = selection_range(&ctx, ec)?;
                trace("SP: GetSelection ok");
                let sink: ITfCompositionSink = CompSinkObj.into();
                let comp: ITfComposition = match unsafe { cc.StartComposition(ec, &range, &sink) } {
                    Ok(c) => c,
                    Err(e) => {
                        trace(&format!("SP: StartComposition err 0x{:08X}", e.code().0 as u32));
                        return Err(e);
                    }
                };
                trace("SP: StartComposition ok");
                let crange: ITfRange = unsafe { comp.GetRange()? };
                let wstr: Vec<u16> = text.encode_utf16().collect();
                unsafe { crange.SetText(ec, 0, &wstr)? };
                trace("SP: SetText ok");
                // 选区跟随到组段末尾（否则下次插入点停在开头）
                let _ = set_selection_at_end(&ctx, ec, &crange);
                g.composition = Some(comp);
                query_caret(&mut g, &ctx, ec);
                Ok(())
            }
            Op::SetPreedit(text) => {
                let comp = g
                    .composition
                    .clone()
                    .ok_or_else(|| Error::from(HRESULT(-2147467259)))?;
                let range: ITfRange = unsafe { comp.GetRange()? };
                let wstr: Vec<u16> = text.encode_utf16().collect();
                if unsafe { range.SetText(ec, 0, &wstr) }.is_err() {
                    // 自愈：组段已被应用/焦点切换单方面终止（SetText 失败）——
                    // 弃死句柄，在当前上下文全新 StartComposition
                    // （此前表现为「有候选但中文永远上不了屏」）
                    trace("SetPreedit: 死组段，自愈重开");
                    let _ = unsafe { comp.EndComposition(ec) };
                    g.composition = None;
                    drop(g);
                    return start_preedit_on(&ctx, &self.shared, ec, text);
                }
                let _ = set_selection_at_end(&ctx, ec, &range);
                query_caret(&mut g, &ctx, ec);
                Ok(())
            }
            Op::Commit(text) => {
                if let Some(comp) = g.composition.clone() {
                    let range: ITfRange = unsafe { comp.GetRange()? };
                    let wstr: Vec<u16> = text.encode_utf16().collect();
                    unsafe {
                        range.SetText(ec, 0, &wstr)?;
                        comp.EndComposition(ec)?;
                    }
                    // 提交后选区放到已提交文本之后
                    let _ = set_selection_at_end(&ctx, ec, &range);
                } else if !text.is_empty() {
                    // 无活动组段的直接提交（如开头标点「，」）：
                    // 在当前选区（插入点）处插入文本，等价于 StartPreedit 的定位流程但不开组段
                    let range = selection_range(&ctx, ec)?;
                    let wstr: Vec<u16> = text.encode_utf16().collect();
                    unsafe {
                        range.SetText(ec, 0, &wstr)?;
                    }
                    let _ = set_selection_at_end(&ctx, ec, &range);
                }
                g.composition = None;
                Ok(())
            }
            Op::CommitAndRepreedit(commit_text, preedit) => {
                // 1) 提交前缀：组段文本置为 commit → EndComposition 落地
                if let Some(comp) = g.composition.clone() {
                    let range: ITfRange = unsafe { comp.GetRange()? };
                    let wstr: Vec<u16> = commit_text.encode_utf16().collect();
                    unsafe {
                        range.SetText(ec, 0, &wstr)?;
                        comp.EndComposition(ec)?;
                    }
                    let _ = set_selection_at_end(&ctx, ec, &range);
                } else if !commit_text.is_empty() {
                    let range = selection_range(&ctx, ec)?;
                    let wstr: Vec<u16> = commit_text.encode_utf16().collect();
                    unsafe {
                        range.SetText(ec, 0, &wstr)?;
                    }
                    let _ = set_selection_at_end(&ctx, ec, &range);
                }
                g.composition = None;
                // 2) 重开组段显示剩余预编辑
                let cc: ITfContextComposition = ctx.cast()?;
                let range: ITfRange = selection_range(&ctx, ec)?;
                let sink: ITfCompositionSink = CompSinkObj.into();
                let comp: ITfComposition = unsafe { cc.StartComposition(ec, &range, &sink)? };
                let crange: ITfRange = unsafe { comp.GetRange()? };
                let wstr2: Vec<u16> = preedit.encode_utf16().collect();
                unsafe { crange.SetText(ec, 0, &wstr2)? };
                let _ = set_selection_at_end(&ctx, ec, &crange);
                g.composition = Some(comp);
                query_caret(&mut g, &ctx, ec);
                Ok(())
            }
            Op::Insert(text) => {
                // 无组段：光标处直接插入（剪贴板上屏）
                if g.composition.is_some() {
                    // 有活动组段先结束
                    if let Some(comp) = g.composition.clone() {
                        unsafe {
                            let _ = comp.EndComposition(ec);
                        }
                    }
                    g.composition = None;
                }
                let ins: ITfInsertAtSelection = ctx.cast()?;
                let wstr: Vec<u16> = text.encode_utf16().collect();
                unsafe {
                    ins.InsertTextAtSelection(ec, INSERT_TEXT_AT_SELECTION_FLAGS(0), &wstr)?;
                }
                Ok(())
            }
            Op::End => {
                if let Some(comp) = g.composition.clone() {
                    let range: ITfRange = unsafe { comp.GetRange()? };
                    let empty: Vec<u16> = Vec::new();
                    unsafe {
                        let _ = range.SetText(ec, 0, &empty);
                        let _ = comp.EndComposition(ec);
                    }
                }
                g.composition = None;
                Ok(())
            }
        }
    }
}

/// 在指定上下文当前选区新开组段并写入预编辑（StartPreedit 主体 + 死组段自愈复用）。
fn start_preedit_on(ctx: &ITfContext, shared: &SharedRef, ec: u32, text: &str) -> Result<()> {
    let cc: ITfContextComposition = ctx.cast()?;
    let range: ITfRange = selection_range(ctx, ec)?;
    let sink: ITfCompositionSink = CompSinkObj.into();
    let comp: ITfComposition = unsafe { cc.StartComposition(ec, &range, &sink)? };
    let crange: ITfRange = unsafe { comp.GetRange()? };
    let wstr: Vec<u16> = text.encode_utf16().collect();
    unsafe { crange.SetText(ec, 0, &wstr)? };
    let _ = set_selection_at_end(ctx, ec, &crange);
    let mut g = shared.lock().unwrap();
    g.composition = Some(comp);
    query_caret(&mut g, ctx, ec);
    Ok(())
}

/// 把选区放到指定范围末尾（折叠），保证后续插入点跟随。
fn set_selection_at_end(ctx: &ITfContext, ec: u32, range: &ITfRange) -> Result<()> {
    let r: ITfRange = unsafe { range.Clone()? };
    unsafe { r.Collapse(ec, TF_ANCHOR_END)? };
    let sel = [TF_SELECTION {
        range: core::mem::ManuallyDrop::new(Some(r)),
        style: TF_SELECTIONSTYLE {
            ase: TF_AE_NONE,
            fInterimChar: BOOL(0),
        },
    }];
    unsafe { ctx.SetSelection(ec, &sel)? };
    Ok(())
}

/// 当前选区（= 光标插入点）克隆出的范围，作为组段起点。
fn selection_range(ctx: &ITfContext, ec: u32) -> Result<ITfRange> {
    let mut sel = [TF_SELECTION::default()];
    let mut fetched: u32 = 0;
    unsafe {
        ctx.GetSelection(ec, u32::MAX, &mut sel, &mut fetched)?;
    }
    if fetched == 0 {
        return Err(Error::from(HRESULT(-2147467259)));
    }
    let r = unsafe { core::mem::ManuallyDrop::take(&mut sel[0].range) };
    Ok(r.expect("GetSelection 未返回 range"))
}

/// 组段内文本的屏幕矩形（插入点跟随）。
/// 量的是**组段末尾折叠后的零宽范围**=光标点本身，不是整段矩形——
/// 整段矩形的左缘是组段起点、宽度随打字膨胀、换行时上下跳行，
/// 拿它当锚点正是候选框水平/垂直抖动的病根。
fn query_caret(g: &mut Shared, ctx: &ITfContext, ec: u32) {
    g.caret = None;
    let Some(comp) = g.composition.clone() else {
        trace("qc: 无组段");
        return;
    };
    let Ok(range) = (unsafe { comp.GetRange() }) else {
        trace("qc: GetRange 失败");
        return;
    };
    let Ok(caret) = (unsafe { range.Clone() }) else {
        trace("qc: Clone 失败");
        return;
    };
    if unsafe { caret.Collapse(ec, TF_ANCHOR_END) }.is_err() {
        trace("qc: Collapse 失败");
        return;
    };
    let Ok(view) = (unsafe { ctx.GetActiveView() }) else {
        trace("qc: GetActiveView 失败");
        return;
    };
    let mut rect = RECT::default();
    let mut clipped = BOOL(0);
    // 双查取末次：部分应用（如跟打器）文本布局异步——按键后第一次
    // 查询常返回旧布局（前一位置），第二次才反映新光标。锚点在旧/新
    // 之间交替正是候选窗「中间→下面→中间」跳动的病根。连查两次取
    // 末次非退化结果，迫使懒布局在本次渲染前完成。
    let mut last_ok: Option<RECT> = None;
    for _ in 0..2 {
        rect = RECT::default();
        if unsafe { view.GetTextExt(ec, &caret, &mut rect, &mut clipped) }.is_ok() {
            let degenerate = rect.bottom <= rect.top
                || rect.right < rect.left
                || (rect.left == 0 && rect.top == 0 && rect.right == 0 && rect.bottom == 0);
            if !degenerate {
                last_ok = Some(rect);
            }
        }
    }
    let Some(rect) = last_ok else {
        trace("qc: GetTextExt 两次均失败/退化");
        return;
    };
    // GetTextExt 返回屏幕坐标（MSDN）——不再做客户区→屏幕转换
    trace(&format!("qc: raw=({},{},{},{})", rect.left, rect.top, rect.right, rect.bottom));
    g.caret = Some(RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    });
}

/// 空组段接收器。
#[implement(ITfCompositionSink)]
struct CompSinkObj;

impl ITfCompositionSink_Impl for CompSinkObj_Impl {
    fn OnCompositionTerminated(&self, _ecwrite: u32, _pcomposition: Option<&ITfComposition>) -> Result<()> {
        Ok(())
    }
}

/// 引擎结果 → 组段与候选窗更新。
fn update_ui(shared: SharedRef, commit: String, state: serde_json::Value) -> Result<()> {
    // 派生要做的组段操作（不持锁调用 run_session——其回调会再拿锁）
    let (op, has_ctx, suppress_win) = {
        let mut g = shared.lock().unwrap();
        let raw = state.get("raw").and_then(|v| v.as_str()).unwrap_or("");
        let preedit = state.get("preedit").and_then(|v| v.as_str()).unwrap_or("");
        if raw.is_empty() {
            g.skin_stale = true;
        }
        // raw 变化 → 记时刻（候选延时显示用）
        if raw != g.raw_last {
            g.raw_last = raw.to_string();
            g.raw_changed_at = Some(std::time::Instant::now());
        }
        let suppress = g.delay_show_ms > 0
            && !raw.is_empty()
            && g
                .raw_changed_at
                .is_some_and(|t| (t.elapsed().as_millis() as u32) < g.delay_show_ms);
        if g.focus_context().is_none() {
            return Ok(());
        }
        let op = if raw.is_empty() && preedit.is_empty() {
            if !commit.is_empty() || g.composition.is_some() {
                Some(Op::Commit(commit.clone()))
            } else {
                None
            }
        } else if !commit.is_empty() {
            // 提前上屏：提交前缀 + 继续组句（此前该分支丢失中途上屏文本）
            Some(Op::CommitAndRepreedit(commit.clone(), preedit.to_string()))
        } else if g.composition.is_none() {
            Some(Op::StartPreedit(preedit.to_string()))
        } else {
            Some(Op::SetPreedit(preedit.to_string()))
        };
        (op, true, suppress)
    };
    let _ = has_ctx;
    // 组段（锁已释放；DoEditSession 内部自行加锁）
    if let Some(op) = op {
        trace("run_session begin");
        // 授权被拒不中断：引擎态照常缓存 + 候选窗照常更新（应用持锁的瞬态窗口期，
        // 下一键会重试补齐组段），否则整条 UI 链被跳过加剧失步
        let r = run_session(&shared, op, None);
        trace("run_session end");
        if r.is_err() {
            trace("run_session err（组段编辑未落地，继续 UI 更新）");
        }
    }

    // 缓存引擎态（TestKeyDown 预判 + 失焦冲销）
    {
        let mut g2 = shared.lock().unwrap();
        g2.chinese = state.get("chinese").and_then(|v| v.as_bool()).unwrap_or(true);
        g2.composing = !state.get("raw").and_then(|v| v.as_str()).unwrap_or("").is_empty();
        g2.preedit_last = state
            .get("preedit")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
    }

    // 候选窗（v2 优先，初始化失败回退 v1）
    let mut g = shared.lock().unwrap();
    let show_code = state.get("show_code").and_then(|v| v.as_bool()).unwrap_or(true);
    let raw_state = state.get("raw").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let aux = state.get("aux").and_then(|v| v.as_str()).unwrap_or("").to_string();
    // 编码行内容：显示编码→raw；关闭时仅在反查/命令等辅助提示下保留一行
    let raw = if show_code { raw_state.clone() } else { aux.clone() };
    let cands: Vec<(String, String)> = state
        .get("candidates")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|c| {
                    (
                        c.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        c.get("comment").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let sel = state.get("selected").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    // 反查/命令模式刚进入（编码空、候选空）时，aux 作首行提示立即显示
    let raw = if raw.is_empty() && cands.is_empty() && !aux.is_empty() {
        aux.clone()
    } else {
        raw
    };
    trace(&format!("cands={} raw='{}' cand2={} dead={}", cands.len(), raw, g.cand2.is_some(), g.cand2_dead));

    g.load_skin();
    if cands.is_empty() && raw.is_empty() {
        if let Some(c) = g.cand2.as_mut() {
            c.hide();
        }
        if let Some(c) = g.cand.take() {
            c.hide();
        }
    } else if suppress_win {
        // 候选延时窗口内：快速输入防闪烁，先不显示
        if let Some(c) = g.cand2.as_mut() {
            c.hide();
        }
        if let Some(c) = g.cand.take() {
            c.hide();
        }
    } else if !g.cand2_dead {
        if g.cand2.is_none() {
            match CandidateWindowV2::new() {
                Some(v2) => g.cand2 = Some(v2),
                None => g.cand2_dead = true,
            }
            trace(&format!("cand2 init dead={}", g.cand2_dead));
        }
        let skin = g.skin.clone();
        let caret = g.caret;
        match g.cand2.as_mut() {
            Some(c) => c.show(&cands, &raw, &skin, caret.as_ref(), sel),
            None => {}
        }
        if g.cand2_dead {
            if g.cand.is_none() {
                g.cand = Some(CandidateWindow::new());
            }
            if let Some(c) = g.cand.as_ref() {
                c.show(&cands, &raw, &g.skin, caret.as_ref(), sel);
            }
        }
    } else {
        if g.cand.is_none() {
            g.cand = Some(CandidateWindow::new());
        }
        let caret = g.caret;
        if let Some(c) = g.cand.as_ref() {
            c.show(&cands, &raw, &g.skin, caret.as_ref(), sel);
        }
    }
    Ok(())
}

/// 申请编辑会话。ctx=Some 显式目标（失焦冲销=旧文档）；None=当前焦点。
/// 授权旗标：首选 TF_ES_SYNC|TF_ES_READWRITE(0x6)——按键汇内 msctf 同步受理并
/// 授带写锁的 cookie（IME 规范路径）。此前首选 0xA(ASYNCDONTCARE)：重启后实测
/// 被按异步排队，Chromium 异步会话只授只读锁 → StartComposition 0x80040201
/// （TS_E_SYNCHRONOUS）打字不上屏、无组段无锚点。0x6 在非按键场景被拒
/// （0x80040209）时再退 0xA 纯异步（读态操作/冲销尽力而为）。
fn run_session(shared: &SharedRef, op: Op, ctx: Option<ITfContext>) -> Result<()> {
    let (target, client_id) = {
        let g = shared.lock().unwrap();
        let target = match ctx {
            Some(c) => c,
            None => g
                .focus_context()
                .ok_or_else(|| Error::from(HRESULT(-2147467259)))?,
        };
        (target, g.client_id)
    };
    let session: ITfEditSession = EditSession {
        shared: shared.clone(),
        op,
        ctx_override: Some(target.clone()),
    }
    .into();
    unsafe {
        let mut granted = None;
        for flags in [0x6u32, 0xA] {
            match target.RequestEditSession(client_id, &session, TF_CONTEXT_EDIT_CONTEXT_FLAGS(flags)) {
                Ok(h) => {
                    trace(&format!("session grant = 0x{:08X} (flags={flags})", h.0 as u32));
                    granted = Some(h);
                    break;
                }
                Err(e) => {
                    trace(&format!("session denied flags={flags} err=0x{:08X}", e.code().0 as u32));
                }
            }
        }
        if granted.is_none() {
            return Err(Error::from(HRESULT(-2147467259)));
        }
    }
    Ok(())
}
