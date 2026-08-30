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
    /// v3（普通分层窗，打包宿主 SearchHost/UWP 专用——DComp 直通窗
    /// 被 DWM cloak，普通分层窗考古验证在 UWP 可见可跟光标）
    pub cand3: Option<crate::candwin3::CandWin3>,
    pub cand3_dead: bool,
    pub cand: Option<CandidateWindow>,
    /// 沉浸式宿主（自绘窗被 DWM cloaked）→ 双通道候选：
    /// A. TSF UIElement——BeginUIElement pbShow=TRUE 即宿主愿意代画
    ///    （微软拼音同款，宿主=SearchHost 的搜索框场景）；
    /// B. server 代画——宿主拒绝时由 hufu-server 进程开窗（普通桌面
    ///    进程不受容器隐身限制；沉浸层之下仍可能被压，保底通道）。
    pub cand_ui: Option<windows::Win32::UI::TextServices::ITfCandidateListUIElement>,
    pub cand_ui_id: u32,
    pub cand_ui_active: bool,
    /// true=宿主经 UIElement 画；false=server 代画
    pub cand_ui_host_draws: bool,
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
    /// Shift 单击判定：keydown 置位；期间任何其他键 keydown 视为组合
    /// （打大写/快捷键）清除；keyup 时仍置位才发给 server 切换中英。
    pub shift_pending: bool,
    /// 候选窗首帧抑制后的补显标记：poll 轮询看到本位且 raw 非空时
    /// 无条件刷新（布局稳定后以正确位置显示，消除首帧错位跳变）。
    pub suppress_pending: bool,
    /// 模式键（CapsLock/Ctrl+Space）Test 阶段直发后的去重标记：
    /// 规范宿主 Test→KeyDown 成对，80ms 内同键 Down 跳过防双发。
    pub modekey_last: Option<(usize, std::time::Instant)>,
    /// 最近一次 preedit（失焦冲销用）
    pub preedit_last: String,
    /// 线程焦点事件 sink cookie（Deactivate 反注册用）
    pub tm_sink_cookie: u32,
    /// 最近一次展示的候选签名（text 序 + selected；停顿期轮询比对，
    /// 异步重排换序后主动刷新候选窗）
    pub cand_sig_last: String,
}

impl Shared {
    fn new() -> Shared {
        Shared {
            thread_mgr: None,
            client_id: 0,
            composition: None,
            cand2: None,
            cand2_dead: false,
            cand3: None,
            cand3_dead: false,
            cand: None,
            cand_ui: None,
            cand_ui_id: 0,
            cand_ui_active: false,
            cand_ui_host_draws: false,
            skin: serde_json::Value::Null,
            skin_stale: true,
            delay_show_ms: 0,
            raw_last: String::new(),
            raw_changed_at: None,
            caret: None,
            chinese: true,
            composing: false,
            shift_pending: false,
            suppress_pending: false,
            modekey_last: None,
            preedit_last: String::new(),
            tm_sink_cookie: 0,
            cand_sig_last: String::new(),
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
        // 语言栏「中/A」状态牌（用户需求：任务栏语言区显示中英态，
        // 左键切换、右键设置）。每线程挂自己的项（weasel 模式）；
        // 项数据（图标/文字）读进程全局 CHINESE，模式由 update_ui
        // 每帧从引擎 state 同步。
        if let Some(tm) = g.thread_mgr.clone() {
            if let Ok(lbm) = tm.cast::<ITfLangBarItemMgr>() {
                if crate::langbar::install(&lbm).is_err() {
                    // 多线程重复挂同 GUID 会失败（正常）；留诊断即可
                }
            }
        }
        // 系统输入指示「中/A」：compartment 同步（本线程 + 全局），
        // 推 OPENCLOSE=1 + 转换模式初值（微软拼音/Rime 同路线）
        if let Some(tm) = g.thread_mgr.clone() {
            crate::langbar::install_compartments(&tm, tid);
        }
        // 激活标记（冒烟测试读取：证明 msctf 真实激活管线走到了这里）
        let marker = std::env::temp_dir().join("hufu-tsf-activated.txt");
        let _ = std::fs::write(&marker, format!("tid={tid} t={:?}\n", std::time::SystemTime::now()));
        // 诊断：宿主画像 + 管道探活（排查开始菜单搜索等特殊宿主：
        // AppContainer 的 DLL 加载与管道连通分层定位）——ProgramData\HuFu\diag
        {
            let pipe_ok = crate::ipc::call(&serde_json::json!({"op": "status"})).is_some();
            let perr = crate::ipc::LAST_PIPE_ERR.load(std::sync::atomic::Ordering::SeqCst);
            let line = format!(
                "pid={} pipe={} perr={} t={:?}\n",
                std::process::id(),
                if pipe_ok { "ok" } else { "fail" },
                perr,
                std::time::SystemTime::now()
            );
            let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
            let _ = std::fs::write(
                format!(r"C:\ProgramData\HuFu\diag\act-{}.txt", std::process::id()),
                line,
            );
        }
        // 上报激活：托盘图标「仅虎符激活时显示」
        let _ = crate::ipc::call(&serde_json::json!({"op": "ime", "active": true}));
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        // 上报失活（托盘侧 700ms 防抖后隐藏图标）
        let _ = crate::ipc::call(&serde_json::json!({"op": "ime", "active": false}));
        let mut g = self.shared.lock().unwrap();
        // compartment 监听摘除（语言栏项同理，各自对称）
        crate::langbar::uninstall_compartments();
        if let Some(tm) = g.thread_mgr.clone() {
            // 语言栏项摘除（与本线程 Activate 对称）
            if let Ok(lbm) = tm.cast::<ITfLangBarItemMgr>() {
                crate::langbar::uninstall(&lbm);
            }
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
        if let Some(mut c) = g.cand3.take() {
            c.hide();
        }
        g.cand = None;
        // 【回归病根】ctfmon 重启等场景进程内本实例会被再次 Activate：
        // cand2_dead 若不清，重激活后所有显示分支被跳过、落入 v1 隐身窗
        // → 搜索框候选彻底消失（实测 notes 只有老会话记录）
        g.cand2_dead = false;
        g.cand3_dead = false;
        // 沉浸式宿主：两通道各自收尾
        if g.cand_ui_active {
            if g.cand_ui_host_draws {
                let mgr = g
                    .thread_mgr
                    .as_ref()
                    .and_then(|tm| {
                        tm.cast::<windows::Win32::UI::TextServices::ITfUIElementMgr>()
                            .ok()
                    });
                let id = g.cand_ui_id;
                if let Some(mgr) = &mgr {
                    let _ = unsafe { mgr.EndUIElement(id) };
                    diag_note("uiel: end (deactivate)");
                }
            } else {
                let _ = crate::ipc::call(&serde_json::json!({"op": "cand_hide"}));
                diag_note("srv cand hide (deactivate)");
            }
        }
        g.cand_ui = None;
        g.cand_ui_active = false;
        g.cand_ui_host_draws = false;
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
        // 诊断：按键是否进入键盘钩（搜索框等特殊宿主排查）
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(format!(r"C:\ProgramData\HuFu\diag\keys-{}.txt", std::process::id()))
        {
            let _ = writeln!(f, "test vk={:#x} t={:?}", wparam.0, std::time::SystemTime::now());
        }
        Ok(self.dispatch(wparam.0, true, false))
    }

    fn OnTestKeyUp(&self, _pic: Option<&ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(format!(r"C:\ProgramData\HuFu\diag\keys-{}.txt", std::process::id()))
        {
            let _ = writeln!(f, "testup vk={:#x}", wparam.0);
        }
        Ok(self.dispatch(wparam.0, true, true))
    }

    fn OnKeyDown(&self, _pic: Option<&ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        // 诊断：真实按键事件（附 dispatch 结论与管道错误码）
        let r = self.dispatch(wparam.0, false, false);
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(format!(r"C:\ProgramData\HuFu\diag\keys-{}.txt", std::process::id()))
        {
            let _ = writeln!(
                f,
                "key vk={:#x} eat={} perr={} t={:?}",
                wparam.0,
                r.0,
                crate::ipc::LAST_PIPE_ERR.load(std::sync::atomic::Ordering::SeqCst),
                std::time::SystemTime::now()
            );
        }
        Ok(r)
    }

    fn OnKeyUp(&self, _pic: Option<&ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        // 键音「松开即停」：截断当前正在响的键音（打字机手感）
        crate::sound::key_up();
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(format!(r"C:\ProgramData\HuFu\diag\keys-{}.txt", std::process::id()))
        {
            let _ = writeln!(f, "keyup vk={:#x}", wparam.0);
        }
        Ok(self.dispatch(wparam.0, false, true))
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

/// 跨容器诊断日志（AppContainer 宿主如 SearchHost 写不了 %TEMP%，
/// 统一落 C:\ProgramData\HuFu\diag\notes-<pid>.txt——该目录已授
/// Everyone + 全应用包写权限）。
pub fn diag_note(msg: &str) {
    use std::io::Write;
    let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
    let path = format!(r"C:\ProgramData\HuFu\diag\notes-{}.txt", std::process::id());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{msg}");
    }
}

impl HuFuTs_Impl {
    /// 键分派：VK → 名称+修饰 → 管道引擎 → 更新组段与候选窗。
    ///
    /// 【宿主形态实测 keys-17436】虎魄跟打器只投 OnTestKeyDown/OnTestKeyUp
    /// （test/testup 成对、key/keyup 全无，space 连 TestKeyDown 都不来只
    /// 来 TestKeyUp）——KeyDown/KeyUp 通道在该宿主完全不可依赖。因此：
    /// - Shift 单击判定闭环在 Test 层：TestDown 置 pending、其他键
    ///   TestDown 清除（组合保护）、TestUp 仍存活才发 server 切换。
    /// - CapsLock / Ctrl+Space 模式键：Test 阶段（Down 或 Up）直发
    ///   server，规范宿主的后续成对事件由 80ms 同键去重挡双发。
    fn dispatch(&self, wparam: usize, test_only: bool, up: bool) -> BOOL {
        // 模式键（无组合歧义）：CapsLock / Ctrl+Space（按着 Ctrl 的 space，
        /// 含 TestKeyUp 时刻——跟打器 space 只在 testup 可见且此时 Ctrl 仍按）。
        let mode_key = match vk_to_name(wparam) {
            Some((n, sh, ct, al)) => n == "capslock" || (ct && !sh && !al && n == "space"),
            None => false,
        };
        // ── Shift 单击判定（Test 层闭环）──
        if wparam == 0x10 {
            if !up {
                // TestDown/KeyDown：只记 pending，不吞（物理 Shift 由应用照常处理）
                self.shared.lock().unwrap().shift_pending = true;
                return BOOL(0);
            }
            // TestUp/KeyUp：pending 存活 = 单击切换。直接发 server 并回填
            // 缓存态（不走通用路径——update_ui 的回填被 !test_only 挡住，
            // Test 阶段直发若不回填，g.chinese 停留旧值：英文态下字母
            // TestDown 预判错报 TRUE → 宿主不产字、IME 也不产 → 字符
            // 消失（跟打器「英文打不进」实测）。返回 BOOL(0) 不吞 keyup。
            let fire = {
                let mut g = self.shared.lock().unwrap();
                let f = g.shift_pending;
                g.shift_pending = false;
                f
            };
            if fire {
                if let Some((consumed, _commit, _back, state, _sound)) =
                    ipc::key_request("shift", false, false, false)
                {
                    if consumed {
                        let zh = state
                            .get("chinese")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                        let mut g = self.shared.lock().unwrap();
                        if g.chinese != zh {
                            g.chinese = zh;
                            crate::langbar::set_mode(zh);
                        }
                        g.composing = !state
                            .get("raw")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .is_empty();
                    }
                }
            }
            return BOOL(0);
        } else {
            if !up {
                // 其他键按下：Shift 组合保护（大写/快捷键），取消单击判定
                self.shared.lock().unwrap().shift_pending = false;
            } else {
                // 其余键的 keyup/testup 一律直通——放行字母 keyup 会造成
                // 每键双发（「按一下等于按两下」回归）。
                return BOOL(0);
            }
        }
        // ── 模式键直发 + 去重 ──
        // 只在按下事件直发（TestDown 或 KeyDown）。松开（testup/keyup）
        // 一律直通：实测按住 CapsLock 常 >80ms 去重窗，keyup 再直发会
        // 把切换翻回去——净效果为零（「caps 无效」实测）。
        if mode_key {
            let dup = {
                let g = self.shared.lock().unwrap();
                matches!(&g.modekey_last, Some((vk, t))
                    if *vk == wparam && t.elapsed().as_millis() < 80)
            };
            if dup {
                return BOOL(0);
            }
            self.shared.lock().unwrap().modekey_last =
                Some((wparam, std::time::Instant::now()));
            // fallthrough：走通用路径发 server + 完整响应处理
        }
        // TestKeyDown：本地预判（缓存引擎态），不碰管道——
        // 否则同一键会被引擎处理两次（Test + Down 各一次）。
        // （Shift 的 TestUp 触发与模式键直发已豁免：fallthrough 发 server。）
        if test_only && !mode_key && wparam != 0x10 {
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
        let Some((consumed, commit, back, state, sound)) =
            ipc::key_request(&name, m_shift, m_ctrl, m_alt)
        else {
            return BOOL(0);
        };
        trace(&format!("pipe back consumed={consumed}"));
        if !consumed {
            return BOOL(0);
        }
        if !test_only || mode_key {
            // 模式键豁免 test 挡板：TestDown 直发切换后必须回填
            // g.chinese/语言栏，否则预判用旧值、英文态错报 TRUE，
            // 宿主不产字（「caps 切换后英文打不进」）。切换响应
            // commit 为空、无组段副作用，update_ui 此时只做状态同步。
            if let Some(tag) = sound {
                crate::sound::play(&tag);
            }
            // 回删替换（数字后 1. 再按 . → 。）：先删旧字符再走正常提交
            if back > 0 {
                let _ = run_session(&self.shared, Op::DeleteBack(back as u32), None);
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
        Some((consumed, _commit, _back, _state, _sound)) => {
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
            // 【标点回归】0xDB/0xDC/0xDD 此前缺失 → vk_to_name 返回
            // None → 直通，中文态 [ ] 出不来【】（引擎映射表本就有）
            0xDB => "[".to_string(),
            0xDC => "\\".to_string(),
            0xDD => "]".to_string(),
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
    /// 回删已上屏字符（数字后「1.」再按 . 换「。」：先删旧点再提交句号）
    DeleteBack(u32),
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
            Op::DeleteBack(n) => {
                // 回删已上屏字符（composition 之外）：选区起点向前扩 n
                // 字符后清空该范围。「1.」再按 . → 删半角点 → 提交「。」
                if g.composition.is_some() {
                    // 有活动组段先结束（正常不该发生，防御）
                    if let Some(comp) = g.composition.clone() {
                        unsafe {
                            let _ = comp.EndComposition(ec);
                        }
                    }
                    g.composition = None;
                }
                let range: ITfRange = selection_range(&ctx, ec)?;
                for _ in 0..*n {
                    let mut moved: i32 = 0;
                    let hr = unsafe {
                        range.ShiftStart(ec, -1, &mut moved, std::ptr::null_mut())
                    };
                    if hr.is_err() || moved == 0 {
                        break;
                    }
                }
                let empty: Vec<u16> = Vec::new();
                unsafe { range.SetText(ec, 0, &empty)? };
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
    // 候选窗锚定组段【起始】位置（非光标/末尾）：编码每加一键组段
    // 变长，锚 END 会带着候选窗逐字符右移——打字快时窗口不停挪动
    // （跟打器实测「在光标附近跳」）。锚 START 则整段编码期间位置
    // 恒定，新组段才换地方（微软拼音/搜狗同款行为）。
    if unsafe { caret.Collapse(ec, TF_ANCHOR_START) }.is_err() {
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
    // 停顿期轮询武装（幂等；进程内一次）+ 记录本次展示签名
    poll_arm(&shared);
    {
        let mut g0 = shared.lock().unwrap();
        g0.cand_sig_last = state_sig(&state);
    }
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
        // 【组段首帧稳定期（仅异步布局宿主）】跟打器类宿主文本布局
        // 懒执行：首键 GetTextExt 常返回旧行框（候选窗偏高一行、第二
        // 键跳正——cw2 show y 序列实测 1092→1175 / 1166→1286）。首帧
        // 220ms 内不显示，等第二键或 260ms 轮询在布局稳定后以正确位
        // 置补显，全程零跳变。同步布局宿主（记事本等）不受影响。
        let first_frame_unstable = host_async_layout()
            && !raw.is_empty()
            && raw.len() <= 1
            && g.raw_changed_at
                .is_some_and(|t| t.elapsed().as_millis() < 220);
        let suppress = first_frame_unstable
            || (g.delay_show_ms > 0
                && !raw.is_empty()
                && g
                    .raw_changed_at
                    .is_some_and(|t| (t.elapsed().as_millis() as u32) < g.delay_show_ms));
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

    // 缓存引擎态（TestKeyDown 预判 + 失焦冲销）+ 语言栏中英态同步
    {
        let mut g2 = shared.lock().unwrap();
        let zh = state.get("chinese").and_then(|v| v.as_bool()).unwrap_or(true);
        if g2.chinese != zh {
            g2.chinese = zh;
            // 语言栏「中/A」：Shift 切换也走这里回填图标/文字
            crate::langbar::set_mode(zh);
        }
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
        if let Some(c) = g.cand3.as_mut() {
            c.hide();
        }
        if let Some(c) = g.cand.take() {
            c.hide();
        }
        if g.cand_ui_active {
            drop(g);
            ui_element_hide(&shared);
            return Ok(());
        }
    } else if suppress_win {
        // 候选延时窗口内：快速输入防闪烁，先不显示。
        // 首帧稳定期抑制时置补显标记——260ms 轮询见此标记且 raw
        // 非空则无条件刷新（此时布局已稳、rect 正确）。
        g.suppress_pending = true;
        if let Some(c) = g.cand2.as_mut() {
            c.hide();
        }
        if let Some(c) = g.cand3.as_mut() {
            c.hide();
        }
        if let Some(c) = g.cand.take() {
            c.hide();
        }
    } else if host_is_packaged() {
        // 【打包宿主（SearchHost/UWP/Store）】进程内候选窗死路实锤：
        // DComp 直通窗（candwin2）与普通分层窗（candwin3，v1 考古
        // 路线，ulw=1 上屏成功）均被 DWM 以 DWM_CLOAKED_SHELL 持续
        // 隐身——cloak 与窗口技术无关，是宿主级的。唯一出路=server
        // 进程代画。位置定版（用户拍板）：开始菜单=桌面左上角 (12,12)
        // 固定；其他打包宿主（Store/UWP）=跟光标（实测锚点精确，
        // 从第一帧就跟）。
        let (x, y) = if host_is_searchhost() {
            (12, 12)
        } else {
            g.caret
                .map(|r| (r.left, r.bottom + 4))
                .unwrap_or((100, 100))
        };
        let raw_c = raw.clone();
        drop(g);
        diag_note("打包宿主 → server 代画（开始菜单=左上角，其他=跟光标）");
        ui_element_show(&shared, &cands, &raw_c, sel, x, y);
        shared.lock().unwrap().suppress_pending = false;
    } else if g.cand_ui_active {
        // server 代画续帧（开始菜单=左上角固定；其他打包宿主每帧跟光标）
        let (x, y) = if host_is_searchhost() {
            (12, 12)
        } else {
            g.caret
                .map(|r| (r.left, r.bottom + 4))
                .unwrap_or((100, 100))
        };
        let raw_c = raw.clone();
        drop(g);
        ui_element_show(&shared, &cands, &raw_c, sel, x, y);
        shared.lock().unwrap().suppress_pending = false;
    } else if !g.cand2_dead {
        if g.cand2.is_none() {
            match CandidateWindowV2::new() {
                Some(v2) => g.cand2 = Some(v2),
                None => g.cand2_dead = true,
            }
            diag_note(&format!(
                "cand2 init ok={} dead={}",
                g.cand2.is_some(),
                g.cand2_dead
            ));
        }
        let skin = g.skin.clone();
        let caret = g.caret;
        // DComp 直通窗在 SearchHost（开始菜单搜索）里被 DWM 整体
        // cloaked（显示中但不可见，实测 cloak=2 逐帧持续）；v1 混合窗
        // 同被隐身，自绘路线在该宿主是死路 → 切 server 代画（左上角）。
        // 【其他宿主（含 UWP 如 Store）】建窗首帧 cloak=2 是瞬态
        // （vis=0 未 Show，实测 Store 首帧即切 → 候选跑左上角被用户
        // 否决）：streak>=3 才认「真·持续隐身」，且降级为 server 代画
        // 【跟光标】（观感=正常候选窗），位置不跑左上角。
        let sh = host_is_searchhost();
        let cloaked_dead = g
            .cand2
            .as_ref()
            .map(|c| c.cloaked_streak >= if sh { 1 } else { 3 })
            .unwrap_or(false);
        if cloaked_dead {
            if let Some(mut c) = g.cand2.take() {
                c.hide();
            }
            // 不预置 cand_ui_active——由 ui_element_show 先问宿主
            // （BeginUIElement）再定通道，否则首轮直接落入 server 分支
            g.cand2_dead = true;
            let (x, y) = if sh {
                // 【用户定稿】开始菜单：候选固定屏幕左上角
                (12, 12)
            } else {
                caret.map(|r| (r.left, r.bottom + 4)).unwrap_or((100, 100))
            };
            let raw_c = raw.clone();
            drop(g);
            diag_note(if sh {
                "cw2 连续 cloaked → 切换双通道候选（左上角）"
            } else {
                "cw2 持续 cloaked（非开始菜单）→ server 跟光标代画"
            });
            ui_element_show(&shared, &cands, &raw_c, sel, x, y);
            shared.lock().unwrap().suppress_pending = false;
            return Ok(());
        }
        match g.cand2.as_mut() {
            Some(c) => c.show(&cands, &raw, &skin, caret.as_ref(), sel),
            None => {}
        }
        // 显示完成：清除首帧抑制补显标记
        g.suppress_pending = false;
    } else {
        // 沉浸式锁定态（自绘窗 cloaked）但 UIElement 通道未激活
        // （Deactivate→再 Activate 的状态漂移）：SearchHost 直接
        // server 代画自愈（左上角）；其他宿主复位 dead 下一帧重建自绘
        if host_is_searchhost() {
            let raw_c = raw.clone();
            drop(g);
            diag_note("cand2_dead 漂移自愈 → server 代画（左上角）");
            ui_element_show(&shared, &cands, &raw_c, sel, 12, 12);
        } else {
            g.cand2_dead = false;
        }
        return Ok(());
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

/// 沉浸式宿主候选显示（双通道）：
/// 首帧 BeginUIElement——pbShow=TRUE 即宿主愿意代画（走 UIElement），
/// FALSE 则降级 server 代画（pipe 推送候选+坐标，server 开窗绘制，
/// 其普通桌面进程窗口不受容器隐身限制）。后续帧按通道更新。
/// 宿主是否打包应用（UWP/XAML Island）：exe 位于 WindowsApps 或
/// SystemApps（SearchHost/Store/设置 等）。此类宿主里 DComp 直通窗
/// 被 DWM cloak → 走 candwin3 普通分层窗（考古复活的 v1 路线）。
fn host_is_packaged() -> bool {
    static P: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *P.get_or_init(|| {
        let exe = std::env::current_exe()
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let l = exe.to_lowercase();
        l.contains("\\windowsapps\\") || l.contains("\\systemapps\\")
    })
}

/// 宿主是否开始菜单搜索（SearchHost.exe）。DLL 跑在宿主进程里，
/// current_exe 即宿主路径。开始菜单的 DWM_CLOAKED_SHELL 是逐帧
/// 持续的真隐身（首帧即切、位置=左上角）；其他宿主（含 UWP/Store）
/// 首帧 cloak=2 是建窗瞬态，streak>=3 才算真隐身且位置跟光标。
fn host_is_searchhost() -> bool {
    static SH: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SH.get_or_init(|| {
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| {
                p.file_name()
                    .map(|s| s.to_string_lossy().to_string())
            })
            .unwrap_or_default();
        exe.eq_ignore_ascii_case("SearchHost.exe")
    })
}

fn ui_element_show(
    shared: &SharedRef,
    cands: &[(String, String)],
    raw: &str,
    sel: usize,
    x: i32,
    y: i32,
) {
    // 【用户定稿】沉浸式宿主候选由 server 代画【用户自己的皮肤】
    // （与普通应用同皮肤同竖排观感）：开始菜单=屏幕左上角（调用方
    // 传 12,12），真·持续 cloak 的其他宿主=跟光标（调用方传光标坐
    // 标）。系统自带候选条仅微软自家 IME 可用；自绘窗被
    // DWM_CLOAKED_SHELL 隐身；server topmost 窗已验证像素级可见。
    let mut g = shared.lock().unwrap();
    g.cand_ui_active = true;
    g.cand_ui_host_draws = false;
    let skin = g.skin.clone();
    drop(g);
    pipe_cand_push(cands, raw, sel, x, y, &skin);
}

/// server 代画：pipe 推送候选帧（含皮肤，server 按皮肤渲染）
fn pipe_cand_push(
    cands: &[(String, String)],
    raw: &str,
    sel: usize,
    x: i32,
    y: i32,
    skin: &serde_json::Value,
) {
    let items: Vec<serde_json::Value> = cands
        .iter()
        .map(|(t, c)| serde_json::json!({"text": t, "comment": c}))
        .collect();
    let _ = crate::ipc::call(&serde_json::json!({
        "op": "cand",
        "items": items,
        "raw": raw,
        "selected": sel,
        "x": x,
        "y": y,
        "skin": skin,
    }));
    diag_note(&format!(
        "srv push n={} perr={}",
        cands.len(),
        crate::ipc::LAST_PIPE_ERR.load(std::sync::atomic::Ordering::SeqCst)
    ));
}

/// 结束沉浸式候选（编码结束/失焦）：两通道各自收尾
fn ui_element_hide(shared: &SharedRef) {
    use windows::Win32::UI::TextServices::ITfUIElementMgr;
    let mut g = shared.lock().unwrap();
    if !g.cand_ui_active {
        return;
    }
    if g.cand_ui_host_draws {
        let mgr = g
            .thread_mgr
            .as_ref()
            .and_then(|tm| tm.cast::<ITfUIElementMgr>().ok());
        let id = g.cand_ui_id;
        g.cand_ui = None;
        g.cand_ui_active = false;
        g.cand_ui_host_draws = false;
        drop(g);
        if let Some(mgr) = mgr {
            let _ = unsafe { mgr.EndUIElement(id) };
            diag_note("uiel: end");
        }
        return;
    }
    g.cand_ui_active = false;
    g.cand_ui_host_draws = false;
    drop(g);
    let _ = crate::ipc::call(&serde_json::json!({"op": "cand_hide"}));
}

// ═══════════════════════════════════════════════════════════════════
// 停顿期候选轮询：异步重排换序后，用户不按键也能看到新首选。
//
// 背景：重排在 server 侧 debounce+推理（~0.5-1s）后写入缓存，但 DLL
// 侧候选窗只在 OnKeyDown 时拉取——停顿中模型算完了，眼前的窗还是
// 旧序，按 2 想选旧序第 2 项会上屏新序第 2 项（选错词投诉位）。
//
// 机制：message-only 窗口 + SetTimer(260ms)。WM_TIMER 与按键回调
// 同线程派发（宿主 UI 线程），窗口操作无跨线程亲和问题；tick 拉一次
// state（本地管道 ~1ms），候选签名（text 序+selected）变化才走
// update_ui 全量刷新。timer 于首次 update_ui 时武装，进程生命周期
// 内常开（空编码 tick 直接短路，开销可忽略）。
// ═══════════════════════════════════════════════════════════════════

use std::sync::atomic::{AtomicIsize, Ordering as AtomicOrdering};

use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassW, SetTimer, HWND_MESSAGE, WINDOW_EX_STYLE,
    WINDOW_STYLE, WNDCLASSW,
};

const POLL_TIMER_ID: usize = 0x4846_5546; // 'HuFU'
const POLL_MS: u32 = 260;

static POLL_HWND: AtomicIsize = AtomicIsize::new(0);
// Shared 含 COM 接口指针（NonNull）非 Send/Sync——但 poll 窗口的
// WM_TIMER 只在其创建线程（=TSF 回调线程）派发，poll_tick 与所有
// COM 访问严格同线程；此 wrapper 仅满足 static 的类型约束。
struct PollShared(SharedRef);
unsafe impl Send for PollShared {}
unsafe impl Sync for PollShared {}
static POLL_SHARED: Mutex<Option<PollShared>> = Mutex::new(None);
static POLL_IN_TICK: AtomicIsize = AtomicIsize::new(0);
static POLL_TICKS: AtomicIsize = AtomicIsize::new(0);

fn state_sig(state: &serde_json::Value) -> String {
    let texts: Vec<String> = state
        .get("candidates")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|c| {
                    c.get("text")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    let sel = state.get("selected").and_then(|v| v.as_u64()).unwrap_or(0);
    format!("{}|{}", texts.join("\u{1}"), sel)
}

extern "system" fn poll_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // WM_TIMER=0x0113（此前笔误 273=WM_COMMAND，tick 永不触发）
    if msg == 0x0113 {
        let id = wparam.0 as usize;
        if id == POLL_TIMER_ID {
            poll_tick();
            return LRESULT(0);
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn poll_arm(shared: &SharedRef) {
    if POLL_HWND.load(AtomicOrdering::Relaxed) != 0 {
        return;
    }
    *POLL_SHARED.lock().unwrap() = Some(PollShared(shared.clone()));
    let cls: Vec<u16> = "HuFuPollWnd".encode_utf16().chain([0]).collect();
    let name: Vec<u16> = "hufu-poll".encode_utf16().chain([0]).collect();
    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(poll_wndproc),
            lpszClassName: PCWSTR(cls.as_ptr()),
            hInstance: HINSTANCE(std::ptr::null_mut()),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            // 已注册（重复激活）忽略
        }
        let h = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(cls.as_ptr()),
            PCWSTR(name.as_ptr()),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            HWND_MESSAGE, // message-only：不可见、不收桌面消息
            None,
            HINSTANCE(std::ptr::null_mut()),
            None,
        );
        if let Ok(h) = h {
            if !h.0.is_null() {
                let _ = SetTimer(h, POLL_TIMER_ID, POLL_MS, None);
                POLL_HWND.store(h.0 as isize, AtomicOrdering::Relaxed);
                diag_note("poll: 轮询窗已武装（260ms）");
            }
        }
    }
}

fn poll_tick() {
    // 重入保护（update_ui 过程中不会再泵本消息，双保险）
    if POLL_IN_TICK.swap(1, AtomicOrdering::Relaxed) != 0 {
        return;
    }
    let _guard = scopeguard_release();
    let n = POLL_TICKS.fetch_add(1, AtomicOrdering::Relaxed);
    if n < 3 {
        diag_note(&format!("poll: tick #{} 开始", n + 1));
    }
    let shared = POLL_SHARED
        .lock()
        .unwrap()
        .as_ref()
        .map(|p| p.0.clone());
    let Some(shared) = shared else { return };
    let Some(state) = ipc::state_request() else { return };
    let raw_empty = state
        .get("raw")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .is_empty();
    let sig = state_sig(&state);
    let mut need_show = false;
    {
        let mut g = shared.lock().unwrap();
        if raw_empty {
            g.cand_sig_last = String::new();
            g.suppress_pending = false;
            return;
        }
        // 补显：首帧稳定期抑制过的窗（suppress_pending），轮询时
        // 无条件刷新——此时 220ms 稳定期已过、布局已稳，update_ui
        // 以正确 rect 显示（op 重跑 edit session 顺带重查 caret）。
        need_show = g.suppress_pending;
        if sig == g.cand_sig_last && !need_show {
            return;
        }
        g.suppress_pending = false;
    }
    trace(if need_show {
        "poll: 首帧抑制 → 补显"
    } else {
        "poll: 候选签名变化 → 刷新"
    });
    let _ = update_ui(shared, String::new(), state);
}

/// 跟打器类宿主：文本布局懒/异步——组段首帧 GetTextExt 常返回旧行框
/// （首键候选窗偏高一行、第二键跳正，实测 y 序列 1092→1175 / 1166→1286）。
/// 此类宿主启用「首帧 220ms 稳定期抑制 + 260ms 轮询补显」；同步布局
/// 宿主（记事本等）首帧 rect 本就正确，不受影响。
fn host_async_layout() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .map(|n| n.contains("跟打"))
            .unwrap_or(false)
    })
}

fn scopeguard_release() -> PollGuard {
    PollGuard
}
struct PollGuard;
impl Drop for PollGuard {
    fn drop(&mut self) {
        POLL_IN_TICK.store(0, AtomicOrdering::Relaxed);
    }
}
