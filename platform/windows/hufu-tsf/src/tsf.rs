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

/// 【滚轮缩放候选框】进程级 SharedRef 锚点：candwin2 的窗口过程是静态
/// 函数拿不到 TSF 实例，WM_MOUSEWHEEL 里经此重入（取 cand2 + 上帧渲染
/// 参数立即重绘，不等皮肤缓存过期）。Activate 时 Set 一次。
/// Shared 含 HWND 等裸指针（非 Send），静态存储需显式声明跨线程安全
/// ——访问全程持 Mutex，实际串行。
pub struct GShared(pub SharedRef);
unsafe impl Send for GShared {}
unsafe impl Sync for GShared {}
pub static G_SHARED: std::sync::OnceLock<GShared> = std::sync::OnceLock::new();

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
    /// 皮肤上次拉取时刻（2.5s 自动过期：打字中改皮肤也能热生效）
    pub skin_loaded_at: std::time::Instant,
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
    /// 【Shift 状态跟踪 2026-09-06】TestDown/KeyDown 见 0x10 置 true、
    /// keyup 置 false。32 位应用 KeyDown 时刻 GetKeyState(VK_SHIFT)
    /// 偶发读不到按下（Shift+6 出不了 ……——引擎收到 shift=false 数字
    /// 被当选重），vk_to_name 判 shift 用「GetKeyState<0 || 本位」双保险。
    pub shift_down: bool,
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
    /// 【上屏跟随重查】CommitAndRepreedit（自动上屏+继续组句）后置位：
    /// 懒布局宿主（跟打器类）上屏帧 GetTextExt 常返回旧行框（组段跨
    /// 软换行时候选框滞留上一行）。60ms 布局稳定后由 CARET_TIMER 强制
    /// 重查 caret 并移动候选窗到最新插入点——「每自动上屏一次就跟随，
    /// 哪怕候选里还有内容」。
    pub caret_recheck_due: bool,
    /// 【行尾检测】最近一帧 caret 逼近前台窗口右缘（软换行边界）：
    /// 下一键的引擎请求带上（提前上屏确认 2 键→1 键，组段缩短更勤，
    /// 跨行滞留窗口随之更小）。无 caret/窗口查询失败时保持 false。
    pub line_end: bool,
    /// 【滚轮缩放候选框】最近一次 show 的渲染参数（候选/编码/选中）：
    /// WM_MOUSEWHEEL 改字号后用它立即重绘（无键事件触发 update_ui）。
    pub last_show: Option<(Vec<(String, String)>, String, usize)>,
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
            skin_loaded_at: std::time::Instant::now(), // skin=null 首拉兜底
            delay_show_ms: 0,
            raw_last: String::new(),
            raw_changed_at: None,
            caret: None,
            chinese: true,
            composing: false,
            shift_pending: false,
            shift_down: false,
            suppress_pending: false,
            modekey_last: None,
            preedit_last: String::new(),
            tm_sink_cookie: 0,
            cand_sig_last: String::new(),
            caret_recheck_due: false,
            line_end: false,
            last_show: None,
        }
    }

    fn load_skin(&mut self) {
        // 皮肤过期三通道：①首次 ②raw 空（断段）③拉取后超 2.5s——
        // 【打字中热更新】③是关键：旧逻辑只在 raw 空时置 stale，打字
        // 期间（raw 非空）改皮肤（透明度/颜色）永远用缓存——「设置页
        // 预览变了、实际候选窗不变」的病根。2.5s 自动过期让改皮肤最
        // 多 2.5 秒后下一帧生效，代价是每 2.5s 一次 ~100µs 管道往返。
        if self.skin_loaded_at.elapsed() > std::time::Duration::from_millis(2500) {
            self.skin_stale = true;
        }
        if self.skin.is_null() || self.skin_stale {
            if let Some(v) = ipc::call(&serde_json::json!({"op": "skin"})) {
                self.delay_show_ms = v
                    .get("delay_show_ms")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0) as u32;
                self.skin = v;
                self.skin_stale = false;
                self.skin_loaded_at = std::time::Instant::now();
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
        // 【滚轮缩放候选框】进程级锚点：candwin2 窗口过程静态重入用
        let _ = G_SHARED.set(GShared(self.shared.clone()));
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
    fn OnSetFocus(&self, fforeground: BOOL) -> Result<()> {
        // 【焦点残留】本进程键盘 sink 失去前台（切到别的应用打字、
        // 开始菜单/UWP 宿主关闭）：立刻收起本进程候选窗，否则独立
        // TOPMOST 窗会在新前台里残留成「第二个候选框」。server 会话
        // 不动——切回原宿主时组段还在，可继续。poll_tick 另有前台
        // 判据兜底（防个别宿主不走此回调）。
        if !fforeground.as_bool() {
            let mut g = self.shared.lock().unwrap();
            if let Some(c) = g.cand2.as_mut() {
                c.hide();
            }
            if let Some(c) = g.cand3.as_mut() {
                c.hide();
            }
        }
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
        // 【交互守卫】鼠标悬停在候选窗上 = 用户正在拖拽/右键固定。
        // 点击候选窗会让宿主连发 docmgr 焦点事件（实测点击即触发
        // OnSetFocus）——此刻绝不能清组段/隐藏候选窗（表现为「点击
        // 后候选框消失、拖拽刚按住就断」）。真失焦时鼠标不在候选窗
        // 上，正常走清理路径。
        {
            let g = self.shared.lock().unwrap();
            if g.cand2.as_ref().map(|c| c.is_mouse_over()).unwrap_or(false) {
                trace("OnSetFocus: 鼠标在候选窗上——交互中，跳过清理");
                return Ok(());
            }
        }
        let (composing, preedit) = {
            let g = self.shared.lock().unwrap();
            (g.composing, g.preedit_last.clone())
        };
        trace(&format!(
            "OnSetFocus: composing={composing} preedit='{}' prev={:?}",
            preedit,
            pdimprevfocus.is_some()
        ));
        // 1) 旧文档上冲销提交：显式 prev ctx；【焦点回调绝不排队异步
        //    session】Chromium 系应用在点击/焦点切换期持内部锁，异步
        //    edit session 的排队回调需要宿主 UI 线程泵消息执行——VSCode
        //    实测死锁冻结（主进程「未响应」，冻结取证：所有线程栈无
        //    hufu 帧、trace 止于 grant(异步档)，DLL 无阻塞调用但宿主
        //    已锁死）。同步档被拒（E_UNEXPECTED/TF_E_SYNCHRONOUS）即
        //    放弃提交——切焦点丢半个未成词，不可拿宿主冻结换。
        if composing && !preedit.is_empty() {
            let prev_ctx = pdimprevfocus.and_then(|d| unsafe { d.GetTop().ok() });
            trace("foc: A 取ctx");
            if let Some(ctx) = prev_ctx {
                let _ = run_session_sync_only(&self.shared, Op::Commit(preedit.clone()), ctx);
            }
        }
        trace("foc: B commit完");
        // 2) 引擎会话清零：focus 上报挪到工作线程（fire-and-forget）。
        //    【VSCode 冻结事故】此前在焦点回调里同步 ipc::call——server
        //    端 dispatch 持全局 Host 锁，任何长操作（如切方案重装整句）
        //    排队期间该请求悬死；客户端 read_exact 无超时 → UI 线程永久
        //    冻结（实测「点击候选框应用未响应」）。focus 响应本就无需
        //    读取，丢弃安全。焦点切换到下次按键至少隔数百 ms，管道
        //    ms 级延迟不构成竞态。
        std::thread::spawn(|| {
            let _ = ipc::call(&serde_json::json!({ "op": "focus" }));
        });
        trace("foc: C spawn完");
        {
            let mut g = self.shared.lock().unwrap();
            trace("foc: D 拿锁");
            g.composition = None; // 死组段句柄必须丢，后续走全新 StartPreedit
            g.composing = false;
            g.raw_last.clear();
            g.preedit_last.clear();
            g.skin_stale = true; // 新焦点重新拉皮肤（也许用户刚改）
            if let Some(c) = g.cand2.as_mut() {
                c.hide();
            }
            if let Some(c) = g.cand3.as_mut() {
                c.hide();
            }
            trace("foc: E hide完");
            if let Some(c) = g.cand.take() {
                c.hide();
            }
            // 【UWP 失焦收候选】沉浸式宿主（Store/UWP/搜索框）的候选由
            // server 代画或宿主 UIElement 画——本地 cand2/cand3 关不到
            // 它。失焦即收尾代画通道（用户定稿：UWP 失焦/失光标就关
            // 候选）。fire-and-forget：焦点回调绝不等待管道。
            if g.cand_ui_active {
                let host_draws = g.cand_ui_host_draws;
                let ui_id = g.cand_ui_id;
                g.cand_ui_active = false;
                g.cand_ui_host_draws = false;
                g.cand_ui = None;
                drop(g);
                if host_draws {
                    let mgr = self.shared.lock().unwrap().thread_mgr.as_ref().and_then(|tm| {
                        tm.cast::<windows::Win32::UI::TextServices::ITfUIElementMgr>().ok()
                    });
                    if let Some(m) = &mgr {
                        let _ = unsafe { m.EndUIElement(ui_id) };
                    }
                } else {
                    std::thread::spawn(|| {
                        let _ = ipc::call(&serde_json::json!({ "op": "cand_hide" }));
                    });
                }
            }
        }
        trace("foc: F 返回前");
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
        let mode_key = match vk_to_name(wparam, false) {
            Some((n, sh, ct, al)) => n == "capslock" || (ct && !sh && !al && n == "space"),
            None => false,
        };
        // ── Shift 单击判定（Test 层闭环）──
        if wparam == 0x10 {
            if !up {
                // TestDown/KeyDown：只记 pending，不吞（物理 Shift 由应用照常处理）
                let mut g = self.shared.lock().unwrap();
                g.shift_pending = true;
                g.shift_down = true;
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
                g.shift_down = false;
                f
            };
            if fire {
                if let Some((consumed, _commit, _back, state, _sound, _vol)) =
                    ipc::key_request("shift", false, false, false, false)
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
            let (chinese, composing, hint) = {
                let g = self.shared.lock().unwrap();
                (g.chinese, g.composing, g.shift_down)
            };
            let Some((name, shift, ctrl, alt)) = vk_to_name(wparam, hint) else {
                return BOOL(0);
            };
            let _ = alt;
            // Ctrl+M 切方案 / Ctrl+Space 切中英：先声明按键，真实处理在 KeyDown
            // 由引擎定夺（未启用时引擎不吞，KeyDown 返回直通）。
            if ctrl && !shift && !alt && (name == "m" || name == "space") {
                return BOOL(1);
            }
            if ctrl {
                // 【候选调频/删词 2026-09-06】Ctrl+数字（含 Shift 形态）在
                // 编码态由引擎处理（置顶/删除），预吞；空态放行（应用
                // 快捷键语义保留）
                let is_digit = name.len() == 1
                    && name.chars().all(|c| c.is_ascii_digit());
                if is_digit && composing {
                    return BOOL(1);
                }
                return BOOL(0); // 其余组合键直通（Ctrl+Shift+V 剪贴板在 KeyDown 处理）
            }
            let will = chinese
                && match name.as_str() {
                    // 编码中：可打印键与控制键都可能被吞
                    _ if composing => true,
                    // 空闲态数字放行：跟打器类宿主信任 TestDown 的吞键结果
                    // （TestDown TRUE + KeyDown eat=0 的组合数字蒸发，keys-5008
                    // 实证：test/key vk=0x33 eat=0 三连但窗口无字）；组段中
                    // 数字保持吞（选重/锁键由引擎处理不受影响）。
                    // 【Shift+数字=符号 2026-09-06】Shift 按着的数字是符号
                    // （……！＠＃），必须预吞——信任 TestDown 的 32 位宿主
                    // 否则自己上屏 ^ 之类的 US shift 形态（Shift+6 实测）。
                    n if n.len() == 1 => {
                        let plain_digit = n.chars().all(|c| c.is_ascii_digit());
                        !plain_digit || shift
                    }
                    // 空闲：编码字母/分号/引号会起段
                    "space" | "enter" | "escape" | "backspace" | "tab" => false,
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
        let hint = self.shared.lock().unwrap().shift_down;
        let Some((name, shift, ctrl, alt)) = vk_to_name(wparam, hint) else {
            return BOOL(0);
        };
        let (name, m_shift, m_ctrl, m_alt) = match name.as_str() {
            "shift" | "ctrl" | "alt" => (name, false, false, false),
            _ => (name, shift, ctrl, alt),
        };
        // 行尾瞬态（query_caret 每帧刷新）：组段逼近窗口右缘时本键
        // 的提前上屏确认放宽（engine need 2→1）
        let line_end = self.shared.lock().unwrap().line_end;
        let Some((consumed, commit, back, state, sound, sound_vol)) =
            ipc::key_request(&name, m_shift, m_ctrl, m_alt, line_end)
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
                crate::sound::play(&tag, sound_vol);
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
    let Some((name, shift, ctrl, alt)) = vk_to_name(vk as usize, false) else {
        eprintln!("hufu-tsf: test_key vk={vk} 无映射");
        return 0;
    };
    let _ = (shift, ctrl, alt);
    let r = ipc::key_request(&name, false, false, false, false);
    match r {
        Some((consumed, _commit, _back, _state, _sound, _vol)) => {
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
/// vk → (基础键名, shift, ctrl, alt)。shift = GetKeyState 按下 ||
/// hint（本 DLL 跟踪的 TestDown/KeyDown 状态——32 位宿主 KeyDown 时刻
/// GetKeyState 偶发失准，双保险）。
fn vk_to_name(vk: usize, hint: bool) -> Option<(String, bool, bool, bool)> {
    unsafe {
        let shift = GetKeyState(VK_SHIFT.0 as i32) < 0 || hint;
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
                // 【功能词拦截 2026-09-06】{加词}：不上屏，弹加词小窗
                //（词+码 → server /api/user_word/add）；{加权}：不上屏，
                // 弹加权小窗（/jq → server 反查最优码提权）；{隐藏候选}：
                // 不上屏，只收起候选窗。三者都先把组段清空结束（preedit
                // 里的 /jc 之类不落文档）。
                if text == "{加词}" || text == "{加权}" || text == "{隐藏候选}" {
                    if let Some(comp) = g.composition.clone() {
                        if let Ok(range) = (unsafe { comp.GetRange() }) {
                            let empty: Vec<u16> = Vec::new();
                            let _ = unsafe { range.SetText(ec, 0, &empty) };
                            let _ = unsafe { comp.EndComposition(ec) };
                        }
                    }
                    g.composition = None;
                    if text == "{加词}" {
                        drop(g);
                        crate::addword::open();
                    } else if text == "{加权}" {
                        drop(g);
                        crate::addword::open_weight();
                    } else {
                        if let Some(c) = g.cand2.as_mut() {
                            c.hide();
                        }
                        if let Some(c) = g.cand3.as_mut() {
                            c.hide();
                        }
                        if let Some(mut c) = g.cand.take() {
                            c.hide();
                        }
                    }
                    return Ok(());
                }
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
                // 【功能词拦截 2026-09-06b】提前上屏路径（pending 收口+
                // 新编码并存）也会带出 {加权}/{加词}——不落文档：弹窗，
                // 剩余 preedit 继续组句（与 Op::Commit 拦截同语义）。
                if commit_text == "{加词}" || commit_text == "{加权}"
                    || commit_text == "{隐藏候选}"
                {
                    // 先处理剩余 preedit / 候选窗（用 g），最后才 drop 弹窗
                    if !preedit.is_empty() {
                        let cc: ITfContextComposition = ctx.cast()?;
                        let range: ITfRange = selection_range(&ctx, ec)?;
                        let sink: ITfCompositionSink = CompSinkObj.into();
                        let comp: ITfComposition =
                            unsafe { cc.StartComposition(ec, &range, &sink)? };
                        let crange: ITfRange = unsafe { comp.GetRange()? };
                        let wstr2: Vec<u16> = preedit.encode_utf16().collect();
                        unsafe { crange.SetText(ec, 0, &wstr2)? };
                        let _ = set_selection_at_end(&ctx, ec, &crange);
                        g.composition = Some(comp);
                        query_caret(&mut g, &ctx, ec);
                    } else {
                        g.composition = None;
                    }
                    if commit_text == "{隐藏候选}" {
                        if let Some(c) = g.cand2.as_mut() {
                            c.hide();
                        }
                        if let Some(c) = g.cand3.as_mut() {
                            c.hide();
                        }
                        if let Some(mut c) = g.cand.take() {
                            c.hide();
                        }
                    } else {
                        let weighted = commit_text == "{加权}";
                        drop(g);
                        crate::tsf::trace("CommitAndRepreedit 拦截功能词（提前上屏路径）");
                        if weighted {
                            crate::addword::open_weight();
                        } else {
                            crate::addword::open();
                        }
                    }
                    return Ok(());
                }
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
    // 候选窗锚点按宿主分化：
    // - 跟随宿主（晴跟打Pro/虎魄跟打器，整句长编码场景）：锚 END
    //   （光标处）。长编码段光标持续前进，窗钉在段首会越离越远
    //   （实测 w 涨到 800px 仍 x 恒定）；虎魄后报同款病，并入。
    // - 其余宿主：锚 START（组段起始）——编码期间位置恒定，逐键
    //   右移的「跳」由此消除；首帧错位另由稳定期抑制。
    let anchor = if host_follow_caret() {
        TF_ANCHOR_END
    } else {
        TF_ANCHOR_START
    };
    if unsafe { caret.Collapse(ec, anchor) }.is_err() {
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
    // 【行尾检测】caret 右缘距前台窗口右缘 < 56px（≈2-3 个全角字 +
    // 滚动条余量，二者同为屏幕物理像素可直接比）→ 软换行边界将至。
    // 页面视图/分栏等行宽 < 窗口宽的宿主检测不到（不触发，无害）；
    // 误触发时提前上屏的仍是同一置信前缀，语义安全。
    g.line_end = unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            false
        } else {
            let mut wr = RECT::default();
            if GetWindowRect(fg, &mut wr).is_ok() {
                wr.right - rect.right < 56 && rect.right <= wr.right
            } else {
                false
            }
        }
    };
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
        // 【皮肤首键】inline 判定依赖皮肤——首键无皮肤时先拉取（原在
        // op 决策之后，会让 inline_preedit=false 的皮肤首键先建组段）。
        if g.skin.is_null() {
            g.load_skin();
        }
        let raw = state.get("raw").and_then(|v| v.as_str()).unwrap_or("");
        let mut preedit = state.get("preedit").and_then(|v| v.as_str()).unwrap_or("");
        // 【2026-09-05 接线】皮肤 layout.inline_preedit（设置页「编码内联
        // 到应用」）：false 时编码不进应用文本流（组段不建、preedit 视为
        // 空），编码只在候选窗编码行显示；true（默认/无皮肤）保持原行为。
        let inline_on = g
            .skin
            .pointer("/skin/layout/inline_preedit")
            .or_else(|| g.skin.get("layout").and_then(|l| l.get("inline_preedit")))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if !inline_on {
            preedit = "";
        }
        if raw.is_empty() {
            g.skin_stale = true;
        }
        // raw 变化 → 记时刻（候选延时显示用）
        if raw != g.raw_last {
            g.raw_last = raw.to_string();
            g.raw_changed_at = Some(std::time::Instant::now());
        }
        // 【首帧稳定期（仅异步布局宿主，35ms 精确补显版）】立即显示
        // 会首帧旧行框跳变（撤抑制实测回归）；被动等轮询补显则慢
        // 122ms（138ms 首键延迟的实锤主耗）。折中：抑制 35ms（≈1
        // 帧布局稳定下限）+ 一次性定时器到点主动补显——总延迟
        // ~55ms 且不跳。
        let first_frame_unstable = host_async_layout()
            && !raw.is_empty()
            && raw.len() <= 1
            && g.raw_changed_at
                .is_some_and(|t| t.elapsed().as_millis() < 35);
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
            // 【上屏跟随】懒布局宿主上屏帧 caret 常是旧行框——布置 60ms
            // 重查（见 caret_recheck_due 注释）。
            g.caret_recheck_due = true;
            arm_caret_recheck_timer();
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
    // 【2026-09-05 内联去重】inline_preedit 开启且编码已内联在应用组段里
    // 时，候选框不再重复显示编码行（反查/命令模式例外——aux 提示保留）。
    let inline_dup = {
        let on = g
            .skin
            .pointer("/skin/layout/inline_preedit")
            .or_else(|| g.skin.get("layout").and_then(|l| l.get("inline_preedit")))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        on && aux.is_empty() && !raw_state.is_empty()
    };
    let show_code = show_code && !inline_dup;
    // 编码行内容：显示编码→raw；关闭时仅在反查/命令等辅助提示下保留一行。
    // 【2026-09-05 反查提示】aux 非空（反查/命令模式）且 raw 非空时拼成
    // 「〔反查〕 ni」——全程提示当前在反查态（此前进入后提示即消失，
    // 编码行只剩拼音，用户看不出自己在反查）。样式=当前皮肤编码行。
    let raw = if !aux.is_empty() && !raw_state.is_empty() {
        format!("{aux} {raw_state}")
    } else if show_code {
        raw_state.clone()
    } else {
        aux.clone()
    };
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

    // 【性能】皮肤拉取只在「无皮肤（首键）」时走按键路径——首次必须
    // 拉否则无皮肤可渲染。此后 2.5s 过期拉取全部挪到 poll_tick 的
    // 断段分支（raw 空）：改皮肤在下一组段生效，键路径零管道往返
    //（「响应速度变慢」的修复——过期拉取曾在每键路径上同步等管道）。
    if g.skin.is_null() {
        g.load_skin();
    }
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
        // 置补显标记 + 【精确一次性定时器】35ms 后主动补显（不等
        // 110ms 轮询周期——「首键候选慢半拍」的 122ms 主耗曾在此；
        // 立即显示又会首帧旧行框跳变，35ms≈1 帧布局稳定下限）。
        g.suppress_pending = true;
        arm_first_frame_timer();
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
        // 【首帧锚点缺失抑制 2026-09-06】WPS 类宿主组段首帧 GetTextExt
        // 未就绪（caret=None，布局异步）——此前 anchor 缺失时 show 退
        // 「焦点窗口内左下」错位显示（用户实测：首键候选跳位，第二帧
        // 才跳回光标处）。改为本帧不显示 + 35ms 精确补显 timer 重查
        // 锚点后就位——首键慢一拍（35ms≈1 帧）但位置直接正确不跳。
        // SearchHost 豁免：其 GetTextExt 常态失败，焦点窗定位候选是
        // 刚需；用户固定（pinned）时无视锚点，不抑制。
        let pinned_now = crate::candwin2::CAND_PINNED.lock().unwrap().is_some();
        if g.caret.is_none() && !pinned_now && !host_is_searchhost() {
            if let Some(c) = g.cand2.as_mut() {
                c.hide();
            }
            g.suppress_pending = true;
            arm_first_frame_timer();
            return Ok(());
        }
        match g.cand2.as_mut() {
            Some(c) => c.show(&cands, &raw, &skin, caret.as_ref(), sel),
            None => {}
        }
        // 【滚轮缩放候选框】缓存渲染参数：WM_MOUSEWHEEL 改字号后
        // 免键事件立即重绘
        g.last_show = Some((cands.clone(), raw.clone(), sel));
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

/// 焦点回调专用：只请求同步档 edit session，被拒即失败（不排队异步）。
/// 异步 session 的回调需要宿主 UI 线程泵消息，Chromium 系应用在焦点
/// 切换期持内部锁 → 排队即死锁（VSCode 点击候选框冻结事故）。
fn run_session_sync_only(shared: &SharedRef, op: Op, ctx: ITfContext) -> Result<()> {
    let client_id = shared.lock().unwrap().client_id;
    let session: ITfEditSession = EditSession {
        shared: shared.clone(),
        op,
        ctx_override: Some(ctx.clone()),
    }
    .into();
    unsafe {
        match ctx.RequestEditSession(client_id, &session, TF_CONTEXT_EDIT_CONTEXT_FLAGS(0x6)) {
            Ok(_) => {
                trace("focus-commit: 同步档受理");
                Ok(())
            }
            Err(e) => {
                trace(&format!("focus-commit: 同步被拒 0x{:08X}，放弃提交", e.code().0 as u32));
                Err(e)
            }
        }
    }
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
    // 宿主顶层窗（前台窗——代画显示时宿主必为前台）：交给 server 自守
    //（宿主窗不可见时 server 自收代画窗——开始菜单残留的根治）
    let host_hwnd = unsafe { GetForegroundWindow() };
    let host_hwnd = if host_hwnd.0.is_null() { 0 } else { host_hwnd.0 as isize };
    pipe_cand_push(cands, raw, sel, x, y, &skin, host_hwnd);
}

/// server 代画：pipe 推送候选帧（含皮肤，server 按皮肤渲染）
fn pipe_cand_push(
    cands: &[(String, String)],
    raw: &str,
    sel: usize,
    x: i32,
    y: i32,
    skin: &serde_json::Value,
    host_hwnd: isize,
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
        "host_hwnd": host_hwnd,
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
// 机制：message-only 窗口 + SetTimer(140ms)。WM_TIMER 与按键回调
// 同线程派发（宿主 UI 线程），窗口操作无跨线程亲和问题；tick 拉一次
// state（本地管道 ~1ms），候选签名（text 序+selected）变化才走
// update_ui 全量刷新。timer 于首次 update_ui 时武装，进程生命周期
// 内常开（空编码 tick 直接短路，开销可忽略）。
// ═══════════════════════════════════════════════════════════════════

use std::sync::atomic::{AtomicIsize, Ordering as AtomicOrdering};

use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, KillTimer,
    RegisterClassW, SetTimer, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
};

const POLL_TIMER_ID: usize = 0x4846_5546; // 'HuFU'
const POLL_MS: u32 = 110;
/// 首帧稳定期一次性补显定时器（35ms 精确到点，不等轮询周期）
const FIRST_TIMER_ID: usize = 0x4846_5547; // 'HuFV'
const FIRST_FRAME_MS: u32 = 35;
/// 上屏跟随重查（一次性）：60ms ≈ 2-3 帧后宿主懒布局已稳定
const CARET_TIMER_ID: usize = 0x4846_5548; // 'HuFW'
const CARET_RECHECK_MS: u32 = 60;

/// 武装首帧补显定时器（update_ui 的 suppress 分支调用；与 poll 窗口
/// 同线程——TSF 回调线程，SetTimer 亲和无虞。重复调用同 id=重置）。
fn arm_first_frame_timer() {
    let h = POLL_HWND.load(AtomicOrdering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = SetTimer(
                HWND(h as *mut _),
                FIRST_TIMER_ID,
                FIRST_FRAME_MS,
                None,
            );
        }
    }
}

/// 武装上屏跟随重查定时器（update_ui 的 CommitAndRepreedit 分支调用；
/// 与 poll 窗同线程。重复调用同 id=重置，幂等）。
fn arm_caret_recheck_timer() {
    let h = POLL_HWND.load(AtomicOrdering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = SetTimer(
                HWND(h as *mut _),
                CARET_TIMER_ID,
                CARET_RECHECK_MS,
                None,
            );
        }
    }
}

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
        if id == FIRST_TIMER_ID {
            // 首帧补显到点：一次性触发即撤（防空转）
            unsafe {
                let _ = KillTimer(hwnd, FIRST_TIMER_ID);
            }
            poll_tick();
            return LRESULT(0);
        }
        if id == CARET_TIMER_ID {
            // 上屏跟随重查到点：一次性。拉当前引擎态强制走一遍 update_ui
            //（同文本 SetPreedit 重跑组段会话 → query_caret 双查此时拿到
            // 稳定后的新行框 → 候选窗以最新插入点重新定位——懒布局宿主
            // 上屏帧的旧行框滞留由此消除）。
            unsafe {
                let _ = KillTimer(hwnd, CARET_TIMER_ID);
            }
            let shared = POLL_SHARED
                .lock()
                .unwrap()
                .as_ref()
                .map(|p| p.0.clone());
            if let Some(shared) = shared {
                {
                    let mut g = shared.lock().unwrap();
                    g.caret_recheck_due = false;
                }
                if let Some(state) = crate::ipc::state_request() {
                    let raw_empty = state
                        .get("raw")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .is_empty();
                    if !raw_empty {
                        // 候选签名不参与判断（内容没变也要移动位置）
                        let mut g = shared.lock().unwrap();
                        g.cand_sig_last = String::new();
                        drop(g);
                        let _ = update_ui(shared, String::new(), state);
                    }
                }
            }
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
                diag_note("poll: 轮询窗已武装（140ms）");
            }
        }
    }
}

/// 前台窗口进程是否与本 DLL 宿主同应用族（同一安装目录）。
/// WPS 多进程架构：编辑在 wpspdf.exe、前台窗属于 wps.exe——两者
/// exe 同目录。同族不当「他进程」（poll 残留兜底不收窗）。
fn fg_same_app_dir(pid: u32) -> bool {
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
                .is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return false;
        }
        let fg_dir = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]))
            .parent()
            .map(|p| p.to_path_buf());
        let my_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));
        match (fg_dir, my_dir) {
            (Some(a), Some(b)) => a == b,
            _ => false,
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
    // 【残留兜底】前台窗口属于别的进程（宿主失焦：切到别的应用打字、
    // 开始菜单/UWP 宿主关闭）→ 收起本进程候选窗并跳过本帧刷新。
    // 组段会话不动（切回原宿主 poll 恢复显示）；正常显示期前台必然
    // 是本进程宿主（候选窗自身不抢焦点）。OnSetFocus(false) 是同步
    // 路径，这里是 140ms 兜底——个别宿主关闭时键盘 sink 回调不触发。
    unsafe {
        let fg = GetForegroundWindow();
        if !fg.0.is_null() {
            let mut pid = 0u32;
            GetWindowThreadProcessId(fg, Some(&mut pid));
            // 【UWP 豁免】Store 等 UWP 的可见窗口属于框架进程
            // ApplicationFrameHost（pid≠内容进程）——前台判据会把
            // 正在打字的 UWP 误判成「他进程」→ 候选闪现即被收
            //（用户实测 Store 候选只闪一下）。SearchHost 例外：它
            // 的前台窗属于自己（打字时 pid==我，不触发）；开始菜单
            // 关闭后前台离开才需要兜底收残留——不能豁免。
            // 【同应用族豁免 2026-09-06】WPS 多进程架构：编辑上下文
            // 在 wpspdf.exe（本 DLL 宿主），前台窗口属于主进程
            // wps.exe（pid≠我、非 UWP）——原判据把正在打字的 WPS
            // 误判成「他进程」，40ms 轮询反复收窗：候选闪现一下就
            // 消失（空格上字正常）。同应用族=前台进程 exe 与本进程
            // exe 同目录（WPS 全家同目录），不当他进程收窗。
            if pid != std::process::id()
                && !(host_is_packaged() && !host_is_searchhost())
                && !fg_same_app_dir(pid)
            {
                let mut g = shared.lock().unwrap();
                let mut any_visible = false;
                if let Some(c) = g.cand2.as_mut() {
                    any_visible |= c.is_visible();
                    c.hide();
                }
                if let Some(c) = g.cand3.as_mut() {
                    any_visible |= c.is_visible();
                    c.hide();
                }
                // 沉浸式宿主两通道也收尾（UWP/搜索框走 UIElement 或
                // server 代画——只藏 cand2 不够，残留正是缺这段）：
                if g.cand_ui_active {
                    if g.cand_ui_host_draws {
                        let mgr = g.thread_mgr.as_ref().and_then(|tm| {
                            tm.cast::<windows::Win32::UI::TextServices::ITfUIElementMgr>().ok()
                        });
                        let id = g.cand_ui_id;
                        if let Some(mgr) = &mgr {
                            let _ = unsafe { mgr.EndUIElement(id) };
                            diag_note("poll: 前台他进程 → EndUIElement（残留兜底）");
                        }
                    } else {
                        let _ = crate::ipc::call(&serde_json::json!({"op": "cand_hide"}));
                        diag_note("poll: 前台他进程 → srv cand_hide（残留兜底）");
                    }
                    g.cand_ui_active = false;
                }
                if any_visible {
                    diag_note(&format!(
                        "poll: 前台他进程(pid={pid}) → 收起候选窗（残留兜底）"
                    ));
                }
                return;
            }
        }
    }
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
            // 【皮肤热更新】断段时拉新皮肤（2.5s 过期检查在 load_skin
            // 内）：键路径不再做管道往返（性能），改皮肤下一组段生效。
            if !g.skin.is_null() {
                g.load_skin();
            }
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

/// 跟随光标宿主（晴跟打器类）：整句长编码场景，候选窗需随光标前进
/// （锚 END）。此类宿主布局同步、GetTextExt 稳定，跟随不会产生跳动。
/// 其余宿主（含虎魄跟打器）锚组段起点——段内位置恒定。
fn host_follow_caret() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .map(|n| n.contains("晴") || n.contains("虎魄"))
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
