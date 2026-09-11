//! TSF 文本服务：按键 → 管道引擎 → 组段/上屏 + 候选窗。

use crate::candwin2::CandidateWindowV2;
use crate::ipc;
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::*;
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

// 【重入死锁防护 2026-09-09】DoEditSession 全程持 shared 锁并跨越
// SetText/GetTextExt 等宿主回调；宿主在回调链内泵消息+焦点变化会
// 同线程再触发 OnSetFocus——其 lock(shared) 重入 std::sync::Mutex
// = 死锁（WPS/多窗口切换「打不出字」多人实录）。守卫：edit session
// 期间焦点事件延迟到 session 出口回放（同线程，同套间，安全）。
thread_local! {
    static IN_EDIT_SESSION: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static DEFERRED_FOCUS: std::cell::RefCell<
        Option<(Option<ITfDocumentMgr>, Option<ITfDocumentMgr>)>,
    > = const { std::cell::RefCell::new(None) };
}
fn edit_session_enter() {
    IN_EDIT_SESSION.with(|d| d.set(d.get() + 1));
}
fn edit_session_exit() {
    IN_EDIT_SESSION.with(|d| d.set(d.get() - 1));
}
fn in_edit_session() -> bool {
    IN_EDIT_SESSION.with(|d| d.get() > 0)
}
fn take_deferred_focus() -> Option<(Option<ITfDocumentMgr>, Option<ITfDocumentMgr>)> {
    DEFERRED_FOCUS.with(|d| d.borrow_mut().take())
}

/// 线程共享状态（文本服务 / 按键接收 / 编辑会话共用）。
pub struct Shared {
    pub thread_mgr: Option<ITfThreadMgr>,
    pub client_id: u32,
    pub composition: Option<ITfComposition>,
    /// v2（DComp+Acrylic）初始化失败 → 回退 v1
    pub cand2: Option<CandidateWindowV2>,
    pub cand2_dead: bool,
    /// 【动效窗口让渡 2026-09-11】动效/展开 tick 正持有 cand2 锁外渲染
    /// ——此间 TSF 线程 is_none() 不许新建第二窗（否则双窗同屏=延伸
    /// 部分重叠感），组段收尾 take() 落空时挂 pending_cand_hide 由
    /// tick 放回时代为隐藏（否则旧窗漏藏=残留阴影，概率性时序 bug）。
    pub cand2_busy: bool,
    pub pending_cand_hide: bool,
    /// v3（普通分层窗，打包宿主 SearchHost/UWP 专用——DComp 直通窗
    /// 被 DWM cloak，普通分层窗考古验证在 UWP 可见可跟光标）
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
    /// 【焦点代际 2026-09-09】每次 OnSetFocus（真实焦点变化）+1——
    /// 在途编辑会话执行时比对，不符即丢弃（详见 EditSession.epoch）。
    pub focus_epoch: u64,
    /// 皮肤上次拉取时刻（2.5s 自动过期：打字中改皮肤也能热生效）
    pub skin_loaded_at: std::time::Instant,
    /// 【皮肤版本 2026-09-08】server 每次保存皮肤 +1；poll 比对不一致
    /// 即强制重拉（绕 2.5s 缓存）——设置页连续调参时实机预览即时
    /// 生效（用户实测「反应慢」根因：连续拖动内缓存未过期，弹的
    /// 还是旧参数窗）。
    pub skin_ver_last: u64,
    /// 皮肤刚重拉、待按新皮肤重绘一次（poll 消费即清——签名未变
    /// 时的预览刷新通道；绝非常驻标志，防每 40ms 白绘）。
    pub skin_repaint: bool,
    /// 【皮肤去重推送 2026-09-11】server 代画通道已推送过的皮肤版本
    ///（u64::MAX=从未/失效→下次必带 skin）。旧实现每帧 clone 整份
    /// 皮肤 JSON 随管道全量推送（沉浸宿主逐键帧，KB 级浪费）；现仅
    /// 换肤/首推/推送失败重推时携带，server 端缓存上帧皮肤。
    pub srv_skin_ver_pushed: u64,
    /// 【方案变更兜底 2026-09-11】poll 见过的当前方案名——server state
    /// 常带 current_schema，比对变化即置皮肤失效（entrance_anim 等
    /// 方案相关字段即时跟随；覆盖设置页/langbar 等非键路径切方案）。
    pub last_schema_seen: String,
    /// 【焦点风暴去抖 2026-09-08】上次「无组段」OnSetFocus 处理时刻——
    /// QQ 类宿主 40ms 内连发 8 次焦点事件（trace 实锤），无组段时重复
    /// 处理全是空操作（spawn+管道+锁白白与打字路径竞争），200ms 去抖。
    pub focus_idle_at: Option<std::time::Instant>,
    /// 【反查退格 2026-09-11】引擎处于反查/命令模式但无组段（仅 aux
    /// 提示态）：退格=退出模式（引擎 consumed）。TestDown 须据此声明
    /// 吞键——否则宿主直接吃掉退格删前字、反查态悬空（WPS 实测）。
    /// update_ui 按引擎 state 每帧刷新；焦点切换清零。
    pub aux_active: bool,
    /// 【打字期 poll 静默 2026-09-08】最近一次按键时刻——poll_tick 在
    /// 500ms 活跃窗口内直接跳过（键路径自会刷新 UI；poll 在打字中
    /// 只有抢管道/抢锁的副作用）。对齐虎爪「打字时零后台」行为。
    pub last_key_at: Option<std::time::Instant>,
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
    /// 【WPS caret 收敛 2026-09-07】WPS 族（et 表格实测）组段首显前
    /// 做插入点收敛检测：每轮记录 GetTextExt rect，连续两轮相同视为
    /// 布局稳定才首显；上限 300ms 兜底强制显示。此前固定 35ms 补显
    /// 远不够表格布局刷新（首键抑制了，第二键 raw=2 直接以陈旧
    /// caret 显示 → 依旧跳）。
    pub wps_caret_prev: Option<RECT>,
    pub wps_settle_start: Option<std::time::Instant>,
    /// 本组段候选窗是否已完成首显（raw 变空时重置）——WPS 收敛检测
    /// 只管首显，首显后正常跟随。
    pub cand_shown_this_segment: bool,
    /// 【失焦残留拦截 2026-09-07】OnSetFocus 后 focus op 异步清 server
    /// session，窗口期内补显 timer/轮询可能拿着失焦前的 raw（如 'd'）
    /// 在新焦点重建组段——用户实测 WPS 表格 A1 打 d 点 B2，A1/B2 双双
    /// 出现字母 d。失焦时记录 raw 快照 + 300ms 有效期：期间 update_ui/
    /// poll 见到同码 state 直接忽略（server 清后自然消失）；不同码=
    /// 用户已在新焦点开始新输入，放行。
    pub stale_raw: String,
    pub stale_raw_until: Option<std::time::Instant>,
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
    /// 【锚组段起点·补显强制重查 2026-09-08】非跟随宿主段内跳过
    /// query_caret（布局锁减压）——但组段首帧查到的常是旧行框（懒
    /// 布局），首帧抑制 35ms 补显时必须强制重查一次拿稳定值，否则
    /// 候选窗按旧行框显示到下一段（用户实测「候选框乱跳」）。置位
    /// 于 arm_first_frame_timer 各抑制点，query_caret 消费即清。
    pub caret_force: bool,
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
            cand2_busy: false,
            pending_cand_hide: false,
            cand_ui: None,
            cand_ui_id: 0,
            cand_ui_active: false,
            cand_ui_host_draws: false,
            skin: serde_json::Value::Null,
            skin_stale: true,
            focus_epoch: 0,
            skin_loaded_at: std::time::Instant::now(), // skin=null 首拉兜底
            skin_ver_last: 0,
            skin_repaint: false,
            srv_skin_ver_pushed: u64::MAX,
            last_schema_seen: String::new(),
            focus_idle_at: None,
            last_key_at: None,
            delay_show_ms: 0,
            raw_last: String::new(),
            raw_changed_at: None,
            caret: None,
            chinese: true,
            composing: false,
            shift_pending: false,
            shift_down: false,
            aux_active: false,
            suppress_pending: false,
            wps_caret_prev: None,
            wps_settle_start: None,
            cand_shown_this_segment: false,
            stale_raw: String::new(),
            stale_raw_until: None,
            modekey_last: None,
            preedit_last: String::new(),
            tm_sink_cookie: 0,
            cand_sig_last: String::new(),
            caret_recheck_due: false,
            caret_force: false,
            line_end: false,
            last_show: None,
        }
    }

    fn load_skin(&mut self) {
        // 皮肤过期四通道：①首次 ②raw 空（断段）③拉取后超 2.5s
        // ④server 皮肤版本变化（poll 比对 skin_ver，强制绕过时限——
        // 【打字中热更新】③是关键：旧逻辑只在 raw 空时置 stale，打字
        // 期间（raw 非空）改皮肤（透明度/颜色）永远用缓存——「设置页
        // 预览变了、实际候选窗不变」的病根。2.5s 自动过期让改皮肤最
        // 多 2.5 秒后下一帧生效，代价是每 2.5s 一次 ~100µs 管道往返。
        if self.skin_loaded_at.elapsed() > std::time::Duration::from_millis(2500) {
            self.skin_stale = true;
        }
        if self.skin.is_null() || self.skin_stale {
            if let Some(v) = ipc::call(&serde_json::json!({"op": "skin"})) {
                self.delay_show_ms =
                    v.get("delay_show_ms").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                self.skin = v;
                self.skin_stale = false;
                self.skin_loaded_at = std::time::Instant::now();
            }
        }
    }

    /// 【皮肤版本失效 2026-09-08】poll 检测 server 皮肤版本变化：直接
    /// 强制重拉（不重置 loaded_at 的路径之外另走）——连续调参时
    /// 2.5s 时限未到也立即拿到新皮肤。
    fn load_skin_forced(&mut self) {
        self.skin_stale = true;
        self.load_skin();
    }

    /// 焦点上下文（当前文档顶层）。
    fn focus_context(&self) -> Option<ITfContext> {
        let tm = self.thread_mgr.as_ref()?;
        let doc: ITfDocumentMgr = unsafe { tm.GetFocus().ok()? };
        unsafe { doc.GetTop().ok() }
    }
}

/// 焦点上下文的视图窗（ITfContextView::GetWnd）：宿主进程自己的窗。
/// 【右键菜单 owner / 打包宿主候选窗 owner 2026-09-11】weasel 生产
/// 路线：菜单与候选窗挂在该窗下（同进程合法 owner；沉浸宿主里
/// owned 窗不进 DWM cloak 名单——「UWP 候选窗隐身」的正解）。
pub fn focus_view_hwnd() -> Option<isize> {
    let Some(g) = G_SHARED.get() else {
        return None;
    };
    let shared = g.0.clone();
    let ctx = {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        g.focus_context()
    }?;
    unsafe {
        let view: ITfContextView = ctx.GetActiveView().ok()?;
        let hwnd = view.GetWnd().ok()?;
        if hwnd.0.is_null() {
            None
        } else {
            Some(hwnd.0 as isize)
        }
    }
}

/// ── 文本服务（同时实现按键接收器：msctf 要求 fforeground sink 支持
///    ITfTextInputProcessor，因此 TIP 对象自身实现 ITfKeyEventSink）──
#[implement(
    ITfTextInputProcessor,
    ITfTextInputProcessorEx,
    ITfKeyEventSink,
    ITfThreadMgrEventSink
)]
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
                        self.shared
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .tm_sink_cookie = cookie;
                    }
                }
            }
            // 输入法默认「开+中文」：部分应用读 OPENCLOSE 档位决定是否走 IME，
            // 不设会表现为「先按一下 Shift 才能打中文」
            if let Ok(cm) = tm.cast::<ITfCompartmentMgr>() {
                if let Ok(comp) = unsafe { cm.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE) }
                {
                    let v = VARIANT::from(1i32);
                    let _ = unsafe { comp.SetValue(tid, &v) };
                }
            }
        }
        let mut g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
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
        let _ = std::fs::write(
            &marker,
            format!("tid={tid} t={:?}\n", std::time::SystemTime::now()),
        );
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
        let mut g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
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
        // 【动效窗口让渡】tick 持有中 → 挂 pending 由 tick 放回时代为
        // 隐藏（否则旧窗漏藏=残留阴影）
        if let Some(mut c) = g.cand2.take() {
            c.hide();
        } else if g.cand2_busy {
            g.pending_cand_hide = true;
        }
        // 【回归病根】ctfmon 重启等场景进程内本实例会被再次 Activate：
        // cand2_dead 若不清，重激活后所有显示分支被跳过、落入 v1 隐身窗
        // → 搜索框候选彻底消失（实测 notes 只有老会话记录）
        g.cand2_dead = false;
        // 沉浸式宿主：两通道各自收尾
        if g.cand_ui_active {
            if g.cand_ui_host_draws {
                let mgr = g.thread_mgr.as_ref().and_then(|tm| {
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
        // 【2026-09-11 poll 预武装】焦点事件到达即武装停顿期轮询：
        // 打包宿主（开始菜单/UWP）里若无真实键流，update_ui 从未执行
        // → poll 永不武装 → 外部管道键/设置页改状态后候选窗永不刷新。
        // poll_arm 幂等（POLL_HWND 判重），110ms 空转查询由前台判据
        // 兜底，无键宿主零负担。
        poll_arm(&self.shared);
        if !fforeground.as_bool() {
            let mut g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(c) = g.cand2.as_mut() {
                c.hide();
            }
        }
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        _pic: Option<&ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        // 诊断：按键是否进入键盘钩（搜索框等特殊宿主排查）
        keys_log(&format!(
            "test vk={:#x} t={:?}",
            wparam.0,
            std::time::SystemTime::now()
        ));
        Ok(self.dispatch(wparam.0, true, false))
    }

    fn OnTestKeyUp(
        &self,
        _pic: Option<&ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        keys_log(&format!("testup vk={:#x}", wparam.0));
        Ok(self.dispatch(wparam.0, true, true))
    }

    fn OnKeyDown(
        &self,
        _pic: Option<&ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        // 诊断：真实按键事件（附 dispatch 结论与管道错误码）
        let r = self.dispatch(wparam.0, false, false);
        keys_log(&format!(
            "key vk={:#x} eat={} perr={} t={:?}",
            wparam.0,
            r.0,
            crate::ipc::LAST_PIPE_ERR.load(std::sync::atomic::Ordering::SeqCst),
            std::time::SystemTime::now()
        ));
        Ok(r)
    }

    fn OnKeyUp(&self, _pic: Option<&ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        // 键音「松开即停」：截断当前正在响的键音（打字机手感）
        crate::sound::key_up();
        keys_log(&format!("keyup vk={:#x}", wparam.0));
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
        // 【重入死锁防护】见 thread_local 声明处注释：edit session 内
        // 触发的焦点事件延迟回放，避免同线程锁重入。
        if in_edit_session() {
            DEFERRED_FOCUS
                .with(|d| *d.borrow_mut() = Some((pdimfocus.cloned(), pdimprevfocus.cloned())));
            return Ok(());
        }
        handle_set_focus(&self.shared, pdimfocus, pdimprevfocus)
    }

    fn OnPushContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        Ok(())
    }

    fn OnPopContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        Ok(())
    }
}

/// OnSetFocus 主体（从 trait 方法抽出，供 edit session 出口回放共用）。
/// 【焦点模式同步 2026-09-11】focus worker 线程回写的中英态
/// （0=无待同步 1=中文 2=英文）——Shared 含 COM 原始指针不可跨
/// 线程移动，经此原子中转；下一次 dispatch（UI 线程）懒应用。
static CHINESE_SYNC: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// 懒应用待同步的中英态（dispatch 入口调用，UI 线程安全）。
fn apply_chinese_sync(shared: &SharedRef) {
    let v = CHINESE_SYNC.swap(0, std::sync::atomic::Ordering::AcqRel);
    if v == 0 {
        return;
    }
    let zh = v == 1;
    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
    if g.chinese != zh {
        g.chinese = zh;
        crate::langbar::set_mode(zh);
        trace(&format!("焦点同步: 本地中英态回填 chinese={zh}"));
    }
}

fn handle_set_focus(
    shared: &SharedRef,
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
        let g = shared.lock().unwrap_or_else(|e| e.into_inner());
        if g.cand2.as_ref().map(|c| c.is_mouse_over()).unwrap_or(false) {
            trace("OnSetFocus: 鼠标在候选窗上——交互中，跳过清理");
            return Ok(());
        }
    }
    let (composing, preedit) = {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        // 【焦点代际 2026-09-09】真实焦点事件到达即 +1：此前请求的
        // 在途编辑会话（异步档排队中）执行时代际不符即被丢弃——
        // 否则会把组段/文字写进旧焦点文档（切窗竞态残余）。
        g.focus_epoch += 1;
        (g.composing, g.preedit_last.clone())
    };
    // 【焦点风暴去抖 2026-09-08】QQ 实测 40ms 内连发 8 次
    // OnSetFocus（宿主内部焦点抖动）——每次 spawn 线程+focus
    // 管道+抢锁，与打字键路径竞争 → 打字卡顿（用户实测「打着
    // 打着会卡」，trace 实锤风暴时段）。无组段（composing=
    // false 且 preedit 空）时这些全是幂等空操作：200ms 内已
    // 处理过一次的直接跳过。有组段永不去抖（冲销必须执行）。
    if !composing && preedit.is_empty() {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        let now = std::time::Instant::now();
        let skip = g
            .focus_idle_at
            .is_some_and(|t| now.duration_since(t).as_millis() < 200);
        if !skip {
            g.focus_idle_at = Some(now);
        }
        drop(g);
        if skip {
            trace("OnSetFocus: 无组段+200ms 内已处理 → 风暴去抖跳过");
            return Ok(());
        }
    }
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
        let raw_last = {
            let g = shared.lock().unwrap_or_else(|e| e.into_inner());
            g.raw_last.clone()
        };
        // 【失焦编码冲销 2026-09-07】兜底提交只保「中文形态」preedit
        //（整句/成词显示文本）；编码字母形态（preedit==raw，顶功
        // 进行中）原样 commit 会把字母写进旧单元格（WPS 表格实测：
        // A1 打 d 点 B2，A1 出字母 d）——改为冲销组段（清空不写）。
        let is_code_form = !raw_last.is_empty() && preedit == raw_last;
        let prev_ctx = pdimprevfocus.and_then(|d| unsafe { d.GetTop().ok() });
        trace("foc: A 取ctx");
        if let Some(ctx) = prev_ctx {
            let payload = if is_code_form {
                String::new()
            } else {
                preedit.clone()
            };
            let _ = run_session_sync_only(shared, Op::Commit(payload), ctx);
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
    // 【焦点模式同步 2026-09-11】中英切换是 server 全局会话态，
    // 其他进程（如 QQ）切走后本进程缓存 g.chinese 即失同步——
    // 焦点切换是拉齐的唯一时机。Shared 含 COM 原始指针不可跨
    // 线程 → worker 经 CHINESE_SYNC 原子中转，下一次 dispatch
    // （UI 线程）懒应用（焦点到首键间隔数百 ms，管道 ms 级）。
    std::thread::spawn(|| {
        if let Some(resp) = ipc::call(&serde_json::json!({ "op": "focus" })) {
            let zh = resp
                .pointer("/state/chinese")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            CHINESE_SYNC.store(if zh { 1 } else { 2 }, std::sync::atomic::Ordering::Release);
        }
    });
    trace("foc: C spawn完");
    {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        trace("foc: D 拿锁");
        g.composition = None; // 死组段句柄必须丢，后续走全新 StartPreedit
        g.composing = false;
        // 失焦残留拦截：focus op 异步清 server session，窗口期内
        // 补显 timer/轮询拿旧 raw 在新焦点重建组段——记快照拦同码。
        if !g.raw_last.is_empty() {
            g.stale_raw = g.raw_last.clone();
            g.stale_raw_until = Some(std::time::Instant::now());
        } else {
            g.stale_raw.clear();
            g.stale_raw_until = None;
        }
        g.raw_last.clear();
        g.preedit_last.clear();
        g.aux_active = false; // 【反查退格】焦点切换：server 会话已清，aux 态作废
        g.skin_stale = true; // 新焦点重新拉皮肤（也许用户刚改）
        if let Some(c) = g.cand2.as_mut() {
            c.hide();
        }
        trace("foc: E hide完");
        // 【UWP 失焦收候选】沉浸式宿主（Store/UWP/搜索框）的候选由
        // server 代画或宿主 UIElement 画——本地 cand2 关不到
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
                let mgr = shared
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .thread_mgr
                    .as_ref()
                    .and_then(|tm| {
                        tm.cast::<windows::Win32::UI::TextServices::ITfUIElementMgr>()
                            .ok()
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

/// 轨迹日志（诊断 UI 线程卡死）：追加到 %TEMP%\hufu-tsf-trace.log。
/// 环境变量 HUFU_TRACE=0 可关。多进程各写各行（带进程名）。
/// 【高速化 2026-09-08】旧版每条 trace 都 env::var + current_exe +
/// open/close 文件——日志累积 83MB（约百万条，每键 10+ 条）意味着
/// 键路径上每秒几百次磁盘 I/O（trace 本身成了卡顿放大器）。现：
/// 开关/exe 名 OnceLock 缓存；句柄进程级常驻（append 打开一次）；
/// 超 8MB 自动轮转（.old 覆盖）。
pub fn trace(msg: &str) {
    use std::io::Write;
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    static EXE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    static FILE: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);
    if !*ON.get_or_init(|| std::env::var("HUFU_TRACE").as_deref() != Ok("0")) {
        return;
    }
    let exe = EXE.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_default()
    });
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut g = FILE.lock().unwrap_or_else(|p| p.into_inner());
    let reopen = |_g: &mut Option<std::fs::File>| -> Option<std::fs::File> {
        let path = std::env::temp_dir().join("hufu-tsf-trace.log");
        // 轮转：超 8MB 归档 .old（旧 .old 覆盖）
        if let Ok(md) = std::fs::metadata(&path) {
            if md.len() > 8 * 1024 * 1024 {
                let _ = std::fs::rename(&path, path.with_extension("log.old"));
            }
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    };
    if g.is_none() {
        *g = reopen(&mut None);
    }
    if let Some(f) = g.as_mut() {
        if writeln!(f, "[{t}] {exe}: {msg}").is_err() {
            *g = reopen(&mut None);
            if let Some(f) = g.as_mut() {
                let _ = writeln!(f, "[{t}] {exe}: {msg}");
            }
        }
    }
}

/// 跨容器诊断日志（AppContainer 宿主如 SearchHost 写不了 %TEMP%，
/// 统一落 C:\ProgramData\HuFu\diag\notes-<pid>.txt——该目录已授
/// Everyone + 全应用包写权限）。
/// 【常驻句柄 + 轮转 2026-09-11】旧实现每条 open/close 且无上限——
/// 残留兜底路径每次 poll 命中都写，宿主异常日可涨到 MB 级。改常驻
/// 句柄 + 8MB 轮转（对齐 trace 的教训：日志别成卡顿放大器）。
pub fn diag_note(msg: &str) {
    use std::io::Write;
    static FILE: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);
    let mut g = FILE.lock().unwrap_or_else(|p| p.into_inner());
    let path = format!(r"C:\ProgramData\HuFu\diag\notes-{}.txt", std::process::id());
    if g.is_none() {
        let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
        if let Ok(md) = std::fs::metadata(&path) {
            if md.len() > 8 * 1024 * 1024 {
                let _ = std::fs::rename(&path, format!("{path}.old"));
            }
        }
        *g = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
    }
    if let Some(f) = g.as_mut() {
        if writeln!(f, "{msg}").is_err() {
            *g = None; // 下次重开（句柄失效）
        }
    }
}

/// 键事件诊断日志（keys-<pid>.txt：vk/dispatch 结论/管道错误码）。
/// 【开关化 2026-09-11】旧实现每键 2-3 次无条件 open/append，无开关
/// 无轮转——keys-*.txt 日增 MB 级（对照 trace 83MB 的教训）。现默认
/// 关：HUFU_KEYS=1 开启（排障时设）；开启时常驻句柄 + 8MB 轮转。
pub fn keys_log(line: &str) {
    use std::io::Write;
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    static FILE: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);
    if !*ON.get_or_init(|| std::env::var("HUFU_KEYS").as_deref() == Ok("1")) {
        return;
    }
    let mut g = FILE.lock().unwrap_or_else(|p| p.into_inner());
    let path = format!(r"C:\ProgramData\HuFu\diag\keys-{}.txt", std::process::id());
    if g.is_none() {
        let _ = std::fs::create_dir_all(r"C:\ProgramData\HuFu\diag");
        if let Ok(md) = std::fs::metadata(&path) {
            if md.len() > 8 * 1024 * 1024 {
                let _ = std::fs::rename(&path, format!("{path}.old"));
            }
        }
        *g = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
    }
    if let Some(f) = g.as_mut() {
        if writeln!(f, "{line}").is_err() {
            *g = None;
        }
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
        // 【焦点模式同步 2026-09-11】应用 focus worker 经原子中转回写
        // 的中英态（见 handle_set_focus），在任何本地预判前拉齐缓存。
        apply_chinese_sync(&self.shared);
        // 模式键（无组合歧义）：CapsLock / Ctrl+Space（按着 Ctrl 的 space，
        // 含 TestKeyUp 时刻——跟打器 space 只在 testup 可见且此时 Ctrl 仍按）。
        let mode_key = match vk_to_name(wparam, false) {
            Some((n, sh, ct, al)) => n == "capslock" || (ct && !sh && !al && n == "space"),
            None => false,
        };
        // ── Shift 单击判定（Test 层闭环）──
        if wparam == 0x10 {
            if !up {
                // TestDown/KeyDown：只记 pending，不吞（物理 Shift 由应用照常处理）
                let mut g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
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
                let mut g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
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
                        let mut g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
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
                self.shared
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .shift_pending = false;
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
                let g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
                matches!(&g.modekey_last, Some((vk, t))
                    if *vk == wparam && t.elapsed().as_millis() < 80)
            };
            if dup {
                return BOOL(0);
            }
            self.shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .modekey_last = Some((wparam, std::time::Instant::now()));
            // fallthrough：走通用路径发 server + 完整响应处理
        }
        // TestKeyDown：本地预判（缓存引擎态），不碰管道——
        // 否则同一键会被引擎处理两次（Test + Down 各一次）。
        // （Shift 的 TestUp 触发与模式键直发已豁免：fallthrough 发 server。）
        if test_only && !mode_key && wparam != 0x10 {
            let (chinese, composing, hint) = {
                let g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
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
                let is_digit = name.len() == 1 && name.chars().all(|c| c.is_ascii_digit());
                if is_digit && composing {
                    return BOOL(1);
                }
                return BOOL(0); // 其余组合键直通（Ctrl+Shift+V 剪贴板在 KeyDown 处理）
            }
            let g_aux = self
                .shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .aux_active;
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
                    // 【反查退格 2026-09-11】反查/命令已进入但无组段
                    //（仅 aux 提示态）时，退格=退出模式（引擎 consumed）
                    // ——TestDown 须声明吞键，否则宿主直接吃掉退格删前
                    // 字、反查态悬空（WPS 实测：窗口残留+后续编码不进
                    // 反查）。有组段的反查（打字中）走 composing 分支。
                    "backspace" => g_aux,
                    // 空闲：编码字母/分号/引号会起段
                    "space" | "enter" | "escape" | "tab" => false,
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
        let hint = self
            .shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .shift_down;
        let Some((name, shift, ctrl, alt)) = vk_to_name(wparam, hint) else {
            return BOOL(0);
        };
        let (name, m_shift, m_ctrl, m_alt) = match name.as_str() {
            "shift" | "ctrl" | "alt" => (name, false, false, false),
            _ => (name, shift, ctrl, alt),
        };
        // 行尾瞬态（query_caret 每帧刷新）：组段逼近窗口右缘时本键
        // 的提前上屏确认放宽（engine need 2→1）
        let line_end = self
            .shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .line_end;
        // 【打字期 poll 静默】记最近按键时刻（poll 500ms 窗口内跳过）
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last_key_at = Some(std::time::Instant::now());
        let Some((consumed, commit, back, state, sound, sound_vol)) =
            ipc::key_request(&name, m_shift, m_ctrl, m_alt, line_end)
        else {
            return BOOL(0);
        };
        // 【换方案即失效皮肤 2026-09-11】Ctrl+M 换方案后 entrance_anim
        // 等方案相关字段变化——键路径不拉皮肤（性能），不失效则缓存
        // 沿用到断段+2.5s（实测：单字切回整句后无入场动效，切窗才
        // 恢复）。server 在键响应 state 里带 schema_changed 标记。
        if state
            .get("schema_changed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            self.shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .skin_stale = true;
        }
        trace(&format!("pipe back consumed={consumed}"));
        if !consumed {
            // 【中英失同步自愈 2026-09-11】直通键也回填模式缓存：
            // server 全局会话可能被其他进程切走（QQ 切英文→切 WPS，
            // WPS 本地 g.chinese 仍 true → TestDown 预判错报吞键 →
            // 信任 TestDown 的宿主（WPS）字符蒸发「连字母都没有」，
            // 且旧代码 consumed=false 提前返回永不回填→不自愈）。
            if !test_only {
                let zh = state
                    .get("chinese")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let mut g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
                if g.chinese != zh {
                    g.chinese = zh;
                    crate::langbar::set_mode(zh);
                    trace(&format!("自愈: 本地中英态回填 chinese={zh}"));
                }
            }
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
            if consumed {
                1
            } else {
                0
            }
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
            0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk as u32 | 32).unwrap_or(' ').to_string(),
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
    /// 【焦点代际 2026-09-09】请求时的 focus_epoch：异步档（0xA）会话
    /// 排队执行可能晚于焦点切换——ctx_override 是请求时的旧文档，
    /// 执行时若代际已变，继续写=文字/组段落进旧应用（切窗「打不出
    /// 字/候选漂移」残余竞态）。执行前比对，不符即丢弃本会话。
    epoch: u64,
    /// 【结果回传 2026-09-11】RequestEditSession 的返回值只反映「受理」
    /// 不反映执行——旧实现 run_session 把受理当成功上抛（状态假阳
    /// 性：上屏实际失败但调用方当成功清了本地状态）。回调把执行结果
    /// 写进槽位；同步档（0x6 回调内联）受理后即可读真实结果。
    /// None = 调用方不关心（焦点冲销等一次性路径）。
    result_slot: Option<std::sync::Arc<std::sync::Mutex<Option<bool>>>>,
}

impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        trace("DoEditSession enter");
        // 【重入死锁防护 2026-09-09】标记本线程进入 edit session：
        // 期间宿主回调链内触发的 OnSetFocus 延迟（见 OnSetFocus 处），
        // 出口回放（此刻 shared 锁空闲、焦点状态已稳定）。
        edit_session_enter();
        let r = self.do_edit_session(ec);
        edit_session_exit();
        if let Some((f, p)) = take_deferred_focus() {
            trace("DoEditSession: 回放延迟焦点事件（session 内触发）");
            let _ = handle_set_focus(&self.shared, f.as_ref(), p.as_ref());
        }
        // 执行结果落槽（run_session 同步档受理后读取上抛）
        if let Some(slot) = &self.result_slot {
            if let Ok(mut s) = slot.lock() {
                *s = Some(r.is_ok());
            }
        }
        trace(&format!("DoEditSession exit ok={}", r.is_ok()));
        r
    }
}

impl EditSession_Impl {
    fn do_edit_session(&self, ec: u32) -> Result<()> {
        let mut g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        // 【焦点代际守卫 2026-09-09】请求→执行之间焦点已变（异步档排队/
        // 宿主消息泵延迟）：本会话的目标文档是旧焦点——丢弃，防止组段
        // 句柄/文字写进旧应用（g.composition 被指向旧文档后，下一键
        // SetText 打不进新应用=「打不出字」，候选锚新光标=「漂移」）。
        if g.focus_epoch != self.epoch {
            trace("edit session: 焦点代际不符——丢弃旧文档会话");
            return Err(Error::from(HRESULT(-2147467259)));
        }
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
                let sink: ITfCompositionSink = CompSinkObj {
                    shared: self.shared.clone(),
                }
                .into();
                let comp: ITfComposition = match unsafe { cc.StartComposition(ec, &range, &sink) } {
                    Ok(c) => c,
                    Err(e) => {
                        trace(&format!(
                            "SP: StartComposition err 0x{:08X}",
                            e.code().0 as u32
                        ));
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
                // 【组段漂移自愈 2026-09-09】选区已离段（用户点了别处，
                // 宿主未按契约终止组段）——弃旧段在当前光标处重组段，
                // 否则本键的预编辑写进旧位置（「首键不在光标处」）。
                if composition_drifted(&g, &ctx, ec) {
                    trace("SetPreedit: 选区离段——重组段跟随光标");
                    if let Some(comp) = g.composition.clone() {
                        end_comp_clear(ec, &comp);
                    }
                    g.composition = None;
                    drop(g);
                    return start_preedit_on(&ctx, &self.shared, ec, &text);
                }
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
                // 【段内免 SetSelection 2026-09-08】每键 SetSelection 触发
                // 宿主选区通知链（跟打器 UI 线程上又一逐键负担——虎爪
                // 对照流畅，我们卡：段内逐键压宿主的点全部剔除）。组段
                // range 自锚定，选区只在 StartPreedit 建段时设一次；
                // Commit/上屏路径不受影响（EndComposition 后宿主按组段
                // 末尾放置插入点）。
                // 【打包宿主例外 2026-09-11】Win11 记事本/UWP 文本栈的
                // 插入符只按 selection 画——段内不更新选区=光标竖线
                // 滞留段首、编码向右生长（用户实测「光标一直在左边，
                // 上屏一次才到最右」）。打包宿主里逐键把选区折叠到段
                // 末（跟打器非打包应用，性能豁免不受影响）。
                if host_is_packaged() || focus_is_uwp_shell() {
                    let _ = set_selection_at_end(&ctx, ec, &range);
                }
                // 【实时光标跟随 2026-09-12 用户拍板·全局】所有应用：
                // 光标一动窗就动，候选窗最左=实时光标最右。三处配合：
                // ①锚点源优先系统插入符（GUITHREADINFO——系统实时维
                // 护、零 TSF 回调、无布局锁/旧行框问题，虎魄跟打器亦
                // 覆盖）；查不到再回落 GetTextExt（query_caret）。
                // ②show() 锚 x=光标右缘（rect.right）。③删 x 单调锁/
                // y 行高锁（顶功上屏 raw 等长但光标已跳——锁导致窗
                // 不跟；旧行框问题由系统插入符从根上绕开）。
                if host_follow_caret() || g.caret.is_none() || g.caret_force {
                    if let Some(r) = gui_caret_fallback() {
                        g.caret_force = false;
                        g.caret = Some(r);
                        g.line_end = unsafe {
                            let fg = GetForegroundWindow();
                            if fg.0.is_null() {
                                false
                            } else {
                                let mut wr = RECT::default();
                                if GetWindowRect(fg, &mut wr).is_ok() {
                                    wr.right - r.right < 56 && r.right <= wr.right
                                } else {
                                    false
                                }
                            }
                        };
                    } else {
                        query_caret(&mut g, &ctx, ec);
                    }
                }
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
                    }
                    return Ok(());
                }
                // 【组段漂移自愈 2026-09-09】上屏前同款检测：选区离段
                //（用户点击别处、宿主未终止组段）时旧段锚在旧位——弃段
                // 走下方「无组段直插」在光标处落字，否则整词上到旧位置。
                if composition_drifted(&g, &ctx, ec) {
                    trace("Commit: 选区离段——弃旧段改光标直插");
                    if let Some(comp) = g.composition.clone() {
                        end_comp_clear(ec, &comp);
                    }
                    g.composition = None;
                }
                if let Some(comp) = g.composition.clone() {
                    let range: ITfRange = unsafe { comp.GetRange()? };
                    let wstr: Vec<u16> = text.encode_utf16().collect();
                    // 【空格清屏修复 2026-09-09】组段 SetText 失败（焦点
                    // 已切/组段被宿主终止）原实现直接 ? 上抛：server 端
                    // session 已清（空格已消费）而文本没落进文档——候选
                    // 有字但屏清了（多人实录，切窗恢复）。自愈：终止死组
                    // 段后在当前 context 直插保字（照 SetPreedit 自愈先例）。
                    let set_ok = unsafe { range.SetText(ec, 0, &wstr).is_ok() };
                    if set_ok {
                        let _ = unsafe { comp.EndComposition(ec) };
                        // 提交后选区放到已提交文本之后
                        let _ = set_selection_at_end(&ctx, ec, &range);
                    } else {
                        trace("Commit: 死组段 SetText 失败——自愈直插保字");
                        let _ = unsafe { comp.EndComposition(ec) };
                        if !text.is_empty() {
                            if let Ok(r2) = selection_range(&ctx, ec) {
                                let _ = unsafe { r2.SetText(ec, 0, &wstr) };
                                let _ = set_selection_at_end(&ctx, ec, &r2);
                            }
                        }
                    }
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
                if commit_text == "{加词}" || commit_text == "{加权}" || commit_text == "{隐藏候选}"
                {
                    // 先处理剩余 preedit / 候选窗（用 g），最后才 drop 弹窗
                    if !preedit.is_empty() {
                        let cc: ITfContextComposition = ctx.cast()?;
                        let range: ITfRange = selection_range(&ctx, ec)?;
                        let sink: ITfCompositionSink = CompSinkObj {
                            shared: self.shared.clone(),
                        }
                        .into();
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
                // 【空格清屏修复】与 Op::Commit 同款自愈：死组段 SetText
                // 失败时直插保字，不让已消费的 commit 丢失。
                // 【组段漂移自愈 2026-09-09】选区离段同款：弃段走直插，
                // 重开组段（第 2 步）本就读当前选区=剩余编码落在光标处。
                if composition_drifted(&g, &ctx, ec) {
                    trace("C&R: 选区离段——弃旧段改光标直插");
                    if let Some(comp) = g.composition.clone() {
                        end_comp_clear(ec, &comp);
                    }
                    g.composition = None;
                }
                if let Some(comp) = g.composition.clone() {
                    let range: ITfRange = unsafe { comp.GetRange()? };
                    let wstr: Vec<u16> = commit_text.encode_utf16().collect();
                    let set_ok = unsafe { range.SetText(ec, 0, &wstr).is_ok() };
                    if set_ok {
                        let _ = unsafe { comp.EndComposition(ec) };
                        let _ = set_selection_at_end(&ctx, ec, &range);
                    } else {
                        trace("C&R: 死组段 SetText 失败——自愈直插保字");
                        let _ = unsafe { comp.EndComposition(ec) };
                        if !commit_text.is_empty() {
                            if let Ok(r2) = selection_range(&ctx, ec) {
                                let _ = unsafe { r2.SetText(ec, 0, &wstr) };
                                let _ = set_selection_at_end(&ctx, ec, &r2);
                            }
                        }
                    }
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
                let sink: ITfCompositionSink = CompSinkObj {
                    shared: self.shared.clone(),
                }
                .into();
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
                    // 有活动组段先结束（清空段内文本——裸 EndComposition
                    // 会把旧 preedit 留在文档，Ctrl+Shift+V 粘贴时编码
                    // 字母落地文档旧位置）
                    if let Some(comp) = g.composition.clone() {
                        end_comp_clear(ec, &comp);
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
                    // 有活动组段先结束（正常不该发生，防御；清空段内
                    // 文本——同 Op::Insert 注记）
                    if let Some(comp) = g.composition.clone() {
                        end_comp_clear(ec, &comp);
                    }
                    g.composition = None;
                }
                let range: ITfRange = selection_range(&ctx, ec)?;
                for _ in 0..*n {
                    let mut moved: i32 = 0;
                    let hr = unsafe { range.ShiftStart(ec, -1, &mut moved, std::ptr::null_mut()) };
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
                    end_comp_clear(ec, &comp);
                }
                g.composition = None;
                Ok(())
            }
        }
    }
}

/// 【弃段清文本 2026-09-11】结束组段前先清空段内文本，再
/// EndComposition。裸 EndComposition 会把组段里的旧预编辑（编码
/// 字母）残留在文档旧位置——跨进程焦点竞态实测 A 侧 'uj' 残留的
/// 机制根因；Op::End 此前就是「清空再结束」（正确样板），全部弃段
/// 路径（漂移自愈 ×3、Insert/DeleteBack 防御 ×2）统一收敛到这。
/// 清空失败（宿主只读等）也无害：EndComposition 照发，残留风险
/// 至少不高于旧实现。
fn end_comp_clear(ec: u32, comp: &ITfComposition) {
    unsafe {
        if let Ok(range) = comp.GetRange() {
            let empty: Vec<u16> = Vec::new();
            let _ = range.SetText(ec, 0, &empty);
        }
        let _ = comp.EndComposition(ec);
    }
}

/// 在指定上下文当前选区新开组段并写入预编辑（StartPreedit 主体 + 死组段自愈复用）。
fn start_preedit_on(ctx: &ITfContext, shared: &SharedRef, ec: u32, text: &str) -> Result<()> {
    let cc: ITfContextComposition = ctx.cast()?;
    let range: ITfRange = selection_range(ctx, ec)?;
    let sink: ITfCompositionSink = CompSinkObj {
        shared: shared.clone(),
    }
    .into();
    let comp: ITfComposition = unsafe { cc.StartComposition(ec, &range, &sink)? };
    let crange: ITfRange = unsafe { comp.GetRange()? };
    let wstr: Vec<u16> = text.encode_utf16().collect();
    unsafe { crange.SetText(ec, 0, &wstr)? };
    let _ = set_selection_at_end(ctx, ec, &crange);
    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
    g.composition = Some(comp);
    query_caret(&mut g, ctx, ec);
    Ok(())
}

/// 把选区放到指定范围末尾（折叠），保证后续插入点跟随。
fn set_selection_at_end(ctx: &ITfContext, ec: u32, range: &ITfRange) -> Result<()> {
    let r: ITfRange = unsafe { range.Clone()? };
    unsafe { r.Collapse(ec, TF_ANCHOR_END)? };
    let mut sel = [TF_SELECTION {
        range: core::mem::ManuallyDrop::new(Some(r)),
        style: TF_SELECTIONSTYLE {
            ase: TF_AE_NONE,
            fInterimChar: BOOL(0),
        },
    }];
    let hr = unsafe { ctx.SetSelection(ec, &sel) };
    // 【COM 泄漏修复 2026-09-09】ManuallyDrop 包裹的克隆 ITfRange 引用
    // 计数永不释放——每建段/上屏泄漏一个 COM 对象（全天打字数千次）。
    // SetSelection 无论成败都接管不了所有权（COM 数组约定：调用方
    // 保留所有权），必须手动 drop。
    unsafe { core::mem::ManuallyDrop::drop(&mut sel[0].range) };
    hr?;
    Ok(())
}

/// 【组段漂移检测 2026-09-09】当前选区是否已离开组段范围——用户点击
/// 别处而宿主未按 TSF 契约终止组段（OnCompositionTerminated 空实现收
/// 不到/收了也不清句柄）时，组段仍活且锚在旧位：继续 SetText/上屏全
/// 部落在旧位置而非光标处（实测「选候选后首键不在光标处」）。
/// 判定=选区起点在段 [start,end] 之外（两端含）；正常打字选区恒在
/// 段内（建段时设过一次选区、跟随宿主自行走到段尾，皆在界内）。
/// GetSelection/CompareStart 是纯文档锚点查询（无布局无通知链），
/// 逐键一次的代价远低于已剔除的 GetTextExt/SetSelection。
fn composition_drifted(g: &Shared, ctx: &ITfContext, ec: u32) -> bool {
    let Some(comp) = g.composition.as_ref() else {
        return false;
    };
    let Ok(range) = (unsafe { comp.GetRange() }) else {
        return true; // 句柄已死，交上层重开/直插
    };
    let Ok(sel) = selection_range(ctx, ec) else {
        return false; // 查不到选区：保守不动
    };
    let (before, after) = unsafe {
        (
            sel.CompareStart(ec, &range, TF_ANCHOR_START),
            sel.CompareStart(ec, &range, TF_ANCHOR_END),
        )
    };
    let (Ok(before), Ok(after)) = (before, after) else {
        return false; // 比较失败：保守不动
    };
    before < 0 || after > 0
}

/// 当前选区（= 光标插入点）克隆出的范围，作为组段起点。
fn selection_range(ctx: &ITfContext, ec: u32) -> Result<ITfRange> {
    let mut sel = [TF_SELECTION::default()];
    let mut fetched: u32 = 0;
    unsafe {
        // 【ulCount=TF_DEFAULT_SELECTION 2026-09-11 回滚】审计建议改
        // 实数 1（担心 WPS 对 -1 校验失败）——实测 WinForms/EDIT 宿主
        // 传 1 反而 GetSelection 失败（TF_E_NOSELECTION 类），组段
        // 建不起来=打字全不上屏（真机 jav+空格 edit 零字实录）。
        // u32::MAX 是 v1.5.1 全量装机验证过的写法，以实测为准回滚；
        // WPS 兼容顾虑无实锤，若将来 WPS 出问题再按宿主名单分派。
        ctx.GetSelection(ec, u32::MAX, &mut sel, &mut fetched)?;
    }
    if fetched == 0 {
        return Err(Error::from(HRESULT(-2147467259)));
    }
    let r = unsafe { core::mem::ManuallyDrop::take(&mut sel[0].range) };
    // 【panic 修复 2026-09-09】宿主返回 fetched>0 但 range=NULL（病态
    // 宿主/竞态）时原 expect 在 DoEditSession（COM 回调）内 panic
    // =宿主进程崩溃。改为 Err 走上层重试/降级。
    r.ok_or_else(|| Error::from(HRESULT(-2147467259)))
}

/// 组段内文本的屏幕矩形（插入点跟随）。
/// 量的是**组段末尾折叠后的零宽范围**=光标点本身，不是整段矩形——
/// 整段矩形的左缘是组段起点、宽度随打字膨胀、换行时上下跳行，
/// 拿它当锚点正是候选框水平/垂直抖动的病根。
fn query_caret(g: &mut Shared, ctx: &ITfContext, ec: u32) {
    // 【旧锚点快照 2026-09-11】旧实现开头即 g.caret=None，末尾失败
    // 分支却注释「保留旧 caret」——实际锚点已丢（候选窗闪回兜底位）。
    // 先快照，失败时真恢复。
    let prev_caret = g.caret;
    let prev_line_end = g.line_end;
    g.caret = None;
    g.caret_force = false; // 消费即清（补显强制重查一次性）
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
    // 候选窗锚点按宿主分化（【默认跟随光标 2026-09-11】用户拍板）：
    // - 默认（全部常规宿主+晴跟打Pro）：锚 END（最新光标处）——候选
    //   框最左=最新光标，逐键前进由位置滑动动效平滑化。
    // - 虎魄跟打器：锚 START（组段起始）+段内零查询——其布局锁下
    //   每键 GetTextExt×2（强迫懒布局收敛）卡秒级（2026-09-08 实测
    //   5.1s，卡点在 SetPreedit 会话内 GetTextExt）。
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
            // 【撤销 64px 高度过滤 2026-09-08】跟打器大字号行高 ~135px、
            // 竖线 caret 宽 2px——此前误判为「整行框」全量丢弃，导致
            // 跟打器锚点永远缺失、候选框被抑制（QQ 行高正常不受影响，
            // 用户实测「QQ 里有、跟打器里没有」）。高行高宿主 caret
            // 本就如此，bottom=行底即正确视觉位置；只保留几何退化
            //（零/反转）判据。
            let degenerate = rect.bottom <= rect.top
                || rect.right < rect.left
                || (rect.left == 0 && rect.top == 0 && rect.right == 0 && rect.bottom == 0);
            if !degenerate {
                last_ok = Some(rect);
            }
        }
    }
    let Some(rect) = last_ok else {
        // 两次均失败/退化：恢复快照的旧锚点（比丢锚点稳；旧实现因
        // 开头清 None 实际没保住——见函数头注记）
        g.caret = prev_caret;
        g.line_end = prev_line_end;
        trace("qc: GetTextExt 两次均失败/退化，沿用旧锚点");
        return;
    };
    // GetTextExt 返回屏幕坐标（MSDN）——不再做客户区→屏幕转换
    trace(&format!(
        "qc: raw=({},{},{},{})",
        rect.left, rect.top, rect.right, rect.bottom
    ));
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
/// 【终止回调实装 2026-09-11】此前 OnCompositionTerminated 空实现——
/// 宿主单方面终止组段后 g.composition 悬挂死句柄，全靠漂移检测 +
/// SetText 失败自愈兜底。现在回调里 try_lock 清引用（抢不到锁说明
/// 正在 edit session 内持锁——自愈链会处理，不死锁）。
#[implement(ITfCompositionSink)]
struct CompSinkObj {
    shared: SharedRef,
}

impl ITfCompositionSink_Impl for CompSinkObj_Impl {
    fn OnCompositionTerminated(
        &self,
        _ecwrite: u32,
        _pcomposition: Option<&ITfComposition>,
    ) -> Result<()> {
        if let Ok(mut g) = self.shared.try_lock() {
            if g.composition.is_some() {
                trace("OnCompositionTerminated: 宿主终止组段——清悬挂句柄");
                g.composition = None;
                // 注：Chromium 系宿主在按下候选窗时杀组段+blur 编辑器
                //（caret 消失）——曾试过合成点击焦点自愈，实测反噬
                //（合成点击再触发焦点清理把候选窗藏掉），已按用户拍板
                // 移除：拖动不要求光标存活，用户自己点回，位置由
                // CAND_PINNED 锁定（见 candwin2 0x202 pin 双保险）。
            }
        }
        Ok(())
    }
}

/// 引擎结果 → 组段与候选窗更新。
fn update_ui(shared: SharedRef, commit: String, state: serde_json::Value) -> Result<()> {
    // 停顿期轮询武装（幂等；进程内一次）+ 记录本次展示签名
    poll_arm(&shared);
    {
        let mut g0 = shared.lock().unwrap_or_else(|e| e.into_inner());
        g0.cand_sig_last = state_sig(&state);
    }
    // 派生要做的组段操作（不持锁调用 run_session——其回调会再拿锁）
    let (op, has_ctx, suppress_win) = {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        // 【皮肤首键】inline 判定依赖皮肤——首键无皮肤时先拉取（原在
        // op 决策之后，会让 inline_preedit=false 的皮肤首键先建组段）。
        if g.skin.is_null() {
            g.load_skin();
        }
        let raw = state.get("raw").and_then(|v| v.as_str()).unwrap_or("");
        // 【失焦残留拦截】focus op 异步清 session 的窗口期内，补显
        // timer/轮询拿失焦前的同码 raw 在新焦点重建组段（B2 冒出
        // 残留字母）——300ms 内同码直接忽略；不同码=新输入放行。
        if !raw.is_empty()
            && !g.stale_raw.is_empty()
            && raw == g.stale_raw
            && g.stale_raw_until
                .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(300))
        {
            return Ok(());
        }
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
            // 断段：下一组段的候选首显重新走 WPS 收敛检测
            g.cand_shown_this_segment = false;
            g.wps_caret_prev = None;
            g.wps_settle_start = None;
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
        // 【实机预览豁免】预览态（state 带 preview_anchor）不走首帧/
        // 延时抑制——锚点即位置无跳变风险，立即显示。
        let is_preview = state.get("preview_anchor").is_some();
        let suppress = !is_preview
            && (first_frame_unstable
                || (g.delay_show_ms > 0
                    && !raw.is_empty()
                    && g.raw_changed_at
                        .is_some_and(|t| (t.elapsed().as_millis() as u32) < g.delay_show_ms)));
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
            // 重查（见 caret_recheck_due 注释）。锚组段起点宿主不需要：
            // CommitAndRepreet 新组段 StartPreedit 会话自会查首帧锚点。
            if host_follow_caret() {
                g.caret_recheck_due = true;
                arm_caret_recheck_timer();
            }
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
        let mut g2 = shared.lock().unwrap_or_else(|e| e.into_inner());
        let zh = state
            .get("chinese")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if g2.chinese != zh {
            g2.chinese = zh;
            // 语言栏「中/A」：Shift 切换也走这里回填图标/文字
            crate::langbar::set_mode(zh);
        }
        g2.composing = !state
            .get("raw")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty();
        // 【反查退格 2026-09-11】aux 态（反查/命令已进入、无组段）标记：
        // TestDown 据此声明吞退格（退出模式而非漏给宿主删前字）。
        // 引擎 state 每帧刷新；退出模式/上屏后 aux 空 → false。
        g2.aux_active = !state
            .get("aux")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty();
        g2.preedit_last = state
            .get("preedit")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
    }

    // 候选窗（v2 优先，初始化失败回退 v1）
    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
    let show_code = state
        .get("show_code")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let raw_state = state
        .get("raw")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let aux = state
        .get("aux")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // 【2026-09-05 内联去重】inline_preedit 开启且编码已内联在应用组段里
    // 时，候选框不再重复显示编码行。
    // 【2026-09-06 反查内联跟随】反查模式 aux 非空不再豁免：内联开着时
    // 拼音同样内嵌在应用组段（g2.preedit=raw），候选框只留〔反查〕提示
    // 不重复拼音；内联关着时候选框承担编码显示，仍拼「〔反查〕 ni」。
    let inline_on = g
        .skin
        .pointer("/skin/layout/inline_preedit")
        .or_else(|| g.skin.get("layout").and_then(|l| l.get("inline_preedit")))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let inline_dup = inline_on && !raw_state.is_empty();
    let show_code = show_code && !inline_dup;
    // 编码行内容：显示编码→raw；关闭时仅在反查等辅助提示下保留一行。
    // 【2026-09-05 反查提示】aux 非空（反查）且 raw 非空时拼成
    // 「〔反查〕 ni」——全程提示当前在反查态（此前进入后提示即消失，
    // 编码行只剩拼音，用户看不出自己在反查）。样式=当前皮肤编码行。
    // 【2026-09-06 内联跟随】内联开（拼音已在应用组段）→ 候选框只留
    // aux 提示不拼 raw；内联关 → 维持拼合显示。
    let raw = if !aux.is_empty() && !raw_state.is_empty() {
        if inline_on {
            aux.clone()
        } else {
            format!("{aux} {raw_state}")
        }
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
                        c.get("text")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        c.get("comment")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
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
    trace(&format!(
        "cands={} raw='{}' cand2={} dead={}",
        cands.len(),
        raw,
        g.cand2.is_some(),
        g.cand2_dead
    ));

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
        if g.cand_ui_active {
            drop(g);
            ui_element_hide(&shared);
            return Ok(());
        }
    } else if suppress_win {
        trace("分支: suppress（35ms 补显）");
        // 候选延时窗口内：快速输入防闪烁，先不显示。
        // 置补显标记 + 【精确一次性定时器】35ms 后主动补显（不等
        // 110ms 轮询周期——「首键候选慢半拍」的 122ms 主耗曾在此；
        // 立即显示又会首帧旧行框跳变，35ms≈1 帧布局稳定下限）。
        g.suppress_pending = true;
        g.caret_force = true; // 补显时强制重查锚点（旧行框矫正）
        arm_first_frame_timer();
        if let Some(c) = g.cand2.as_mut() {
            c.hide();
        }
    } else if (host_is_packaged() || focus_is_uwp_shell()) && !g.cand2_dead {
        trace("OWNED分支: 进入");
        // 【打包宿主 2026-09-11 三次尝试·ULW owned 窗】DComp 直通窗在
        // 沙盒里 D3D 初始化卡死（二次实测），本轮改用 owned 分层窗 +
        // 软件呈现（WARP D3D + D2D 离屏 + GDI UpdateLayeredWindow——
        // weasel 同思路），new_owned 内设备初始化已线程化（2.5s 超时
        // 守护，卡死只卡孤儿线程）。失败/超时/被 cloak → 下方 caret-
        // follow server 代画兜底（位置也跟光标）。
        if g.cand2.is_none() && g.cand2_busy {
            for _ in 0..25 {
                drop(g);
                std::thread::sleep(std::time::Duration::from_millis(2));
                g = shared.lock().unwrap_or_else(|e| e.into_inner());
                if g.cand2.is_some() || !g.cand2_busy {
                    break;
                }
            }
        }
        if g.cand2.is_none() {
            // 【死锁修复 2026-09-11】focus_view_hwnd() 内部要拿同一把
            // Shared 锁——持锁调用=同线程 Mutex 重入=TSF 线程卡死
            //（前两轮「OWNED 进入后无下文」的真凶，与 D3D/沙盒无关）。
            // 先放锁取 owner，再重新拿锁并复查（期间他人可能已建窗）。
            drop(g);
            let owner = focus_view_hwnd();
            g = shared.lock().unwrap_or_else(|e| e.into_inner());
            if g.cand2.is_none() {
                match CandidateWindowV2::new_owned(
                    owner.map(|h| windows::Win32::Foundation::HWND(h as *mut _)),
                ) {
                    Some(v2) => g.cand2 = Some(v2),
                    None => g.cand2_dead = true,
                }
                trace(&format!(
                    "cand2(owned-ulw) init ok={} owner={:#x} dead={}",
                    g.cand2.is_some(),
                    owner.unwrap_or(0),
                    g.cand2_dead
                ));
                diag_note(&format!(
                    "cand2(owned-ulw) init ok={} owner={:#x} dead={}",
                    g.cand2.is_some(),
                    owner.unwrap_or(0),
                    g.cand2_dead
                ));
            }
        }
        if g.cand2_dead {
            // owned 窗建不出（WARP 失败/超时等）→ server 代画兜底
            //（位置同链跟光标：SearchHost 兜不到才退 (12,12)）
            let caret_pos = g
                .caret
                .map(|r| (r.left, r.bottom + 4))
                .or_else(|| gui_caret_fallback().map(|r| (r.left, r.bottom + 4)));
            let (x, y) = if host_is_searchhost() {
                caret_pos.unwrap_or((12, 12))
            } else {
                caret_pos.unwrap_or((100, 100))
            };
            let raw_c = raw.clone();
            drop(g);
            diag_note("打包宿主 owned(ULW) 建窗失败 → server 代画");
            ui_element_show(&shared, &cands, &raw_c, sel, x, y);
            shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .suppress_pending = false;
            return Ok(());
        }
        // ↓ 与常规宿主同一条显示路径（共用锚点链/抑制逻辑/show）
        let skin = g.skin.clone();
        let preview_anchor = state.get("preview_anchor").cloned();
        let caret = preview_anchor
            .as_ref()
            .and_then(|a| {
                let x = a.get("x").and_then(|v| v.as_i64())? as i32;
                let y = a.get("y").and_then(|v| v.as_i64())? as i32;
                Some(RECT {
                    left: x,
                    top: y,
                    right: x,
                    bottom: y,
                })
            })
            .or(g.caret)
            .or_else(gui_caret_fallback)
            // 【owned 锚点 2026-09-11】打包宿主 GetTextExt 常态失败、
            // XAML 自绘光标无系统插入符 → 退 owner 窗矩形：候选窗贴
            // 宿主输入窗下缘（开始菜单=搜索框正下方）。
            .or_else(|| {
                g.cand2.as_ref().and_then(|c| {
                    let o = c.owner_hwnd();
                    if o == 0 {
                        return None;
                    }
                    let mut r = RECT::default();
                    unsafe { GetWindowRect(windows::Win32::Foundation::HWND(o as *mut _), &mut r) };
                    if r.right > r.left && r.bottom > r.top {
                        Some(r)
                    } else {
                        None
                    }
                })
            });
        let is_preview = preview_anchor.is_some();
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
            g.cand2_dead = true;
            let (x, y) = if sh {
                // 【用户定稿】开始菜单：候选固定屏幕左上角（cloak 兜底）
                (12, 12)
            } else {
                caret.map(|r| (r.left, r.bottom + 4)).unwrap_or((100, 100))
            };
            let raw_c = raw.clone();
            drop(g);
            diag_note("cw2 owned 仍被 cloaked → server 代画兜底");
            ui_element_show(&shared, &cands, &raw_c, sel, x, y);
            shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .suppress_pending = false;
            return Ok(());
        }
        // SearchHost 豁免 caret 抑制（同常规分支语义）；显示。抑制判据
        // 用「算完兜底链的 caret」——owned 锚点在身就不抑制（防
        // caret=None 死循环，反查首帧同款教训）。
        let pinned_now = crate::candwin2::CAND_PINNED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        let _ = pinned_now;
        let suppress_caret = caret.is_none() && !is_preview && !sh;
        if suppress_caret {
            if let Some(c) = g.cand2.as_mut() {
                c.hide();
            }
            g.suppress_pending = true;
            g.caret_force = true;
            arm_first_frame_timer();
            return Ok(());
        }
        let content_empty = cands.is_empty() && raw.is_empty();
        if !content_empty {
            if let Some(c) = g.cand2.as_mut() {
                c.show(&cands, &raw, &skin, caret.as_ref(), sel);
            }
            g.last_show = Some((cands.clone(), raw.clone(), sel));
        }
        g.cand_shown_this_segment = true;
        g.suppress_pending = false;
    } else if g.cand_ui_active {
        // server 代画续帧：位置与首帧同链（SearchHost 也跟系统插入符，
        // 兜不到才退 (12,12)——2026-09-11 从固定左上角升级）。
        let caret_pos = g
            .caret
            .map(|r| (r.left, r.bottom + 4))
            .or_else(|| gui_caret_fallback().map(|r| (r.left, r.bottom + 4)));
        let (x, y) = if host_is_searchhost() {
            caret_pos.unwrap_or((12, 12))
        } else {
            caret_pos.unwrap_or((100, 100))
        };
        let raw_c = raw.clone();
        drop(g);
        ui_element_show(&shared, &cands, &raw_c, sel, x, y);
        shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .suppress_pending = false;
    } else if !g.cand2_dead {
        // 【动效窗口让渡】动效/展开 tick 持有 cand2 锁外渲染中——窗口
        // 存在只是暂时不在槽里：短等放回后复用，严禁此刻新建第二窗
        //（双窗同屏=延伸区重叠+阴影残留）
        if g.cand2.is_none() && g.cand2_busy {
            for _ in 0..25 {
                drop(g);
                std::thread::sleep(std::time::Duration::from_millis(2));
                g = shared.lock().unwrap_or_else(|e| e.into_inner());
                if g.cand2.is_some() || !g.cand2_busy {
                    break;
                }
            }
        }
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
        // 【实机预览锚点 2026-09-08】设置页预览（server state 带
        // preview_anchor=设置窗中心）时候选窗弹在那里，不用陈旧
        // 光标——预览不依赖真实 caret（可能 None 或屏幕任意处）。
        let preview_anchor = state.get("preview_anchor").cloned();
        let caret = preview_anchor
            .as_ref()
            .and_then(|a| {
                let x = a.get("x").and_then(|v| v.as_i64())? as i32;
                let y = a.get("y").and_then(|v| v.as_i64())? as i32;
                Some(RECT {
                    left: x,
                    top: y,
                    right: x,
                    bottom: y,
                })
            })
            .or(g.caret)
            // 【反查首帧锚点 2026-09-11】无组段帧 caret 恒 None——退
            // 系统插入符（GUITHREADINFO hCaret）：反查提示窗直接落在
            // 真实输入位置，打出首字母后组段锚点接管（位置滑动过渡）。
            .or_else(gui_caret_fallback);
        let is_preview = preview_anchor.is_some();
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
            shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .suppress_pending = false;
            return Ok(());
        }
        // 【首帧锚点缺失抑制 2026-09-06】WPS 类宿主组段首帧 GetTextExt
        // 未就绪（caret=None，布局异步）——此前 anchor 缺失时 show 退
        // 「焦点窗口内左下」错位显示（用户实测：首键候选跳位，第二帧
        // 才跳回光标处）。改为本帧不显示 + 35ms 精确补显 timer 重查
        // 锚点后就位——首键慢一拍（35ms≈1 帧）但位置直接正确不跳。
        // SearchHost 豁免：其 GetTextExt 常态失败，焦点窗定位候选是
        // 刚需；用户固定（pinned）时无视锚点，不抑制。
        let pinned_now = crate::candwin2::CAND_PINNED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        // 【WPS caret 收敛 2026-09-07】固定 35ms 补显不够 WPS 表格布局
        // 稳定：首键帧抑制了，但第二键（raw=2）不再命中 wps_first_key，
        // 直接以陈旧 caret 显示 → 依旧跳。改为按「组段首显」管理：WPS
        // 族组段候选窗首次显示前做插入点收敛检测——每 35ms 重查一轮，
        // 连续两轮 GetTextExt rect 相同（布局已稳）才首显；300ms 上限
        // 兜底强制显示（防极端慢布局永不显示）。首显后正常跟随。
        let wps_settle = host_is_wps() && !g.cand_shown_this_segment;
        let wps_stable = g.wps_caret_prev.is_some_and(|p| g.caret == Some(p));
        let wps_deadline = g
            .wps_settle_start
            .is_some_and(|t| t.elapsed() > std::time::Duration::from_millis(300));
        // 【反查首帧修复 2026-09-11】无组段帧（反查/命令模式刚进入：仅
        // aux 提示行、编码空、应用文本流无内容）豁免 caret 抑制链——
        // query_caret 需要组段，这类帧 caret 恒 None，抑制+35ms 补显
        // 重查永远查不出（死循环），实测 WPS/Typora 里按 ` 后反查窗
        // 口不出现，打出第一个字母才有。等待无意义：直接显示，锚点
        // 用旧 caret（上一段落点）→ 无则系统插入符（GUITHREADINFO）
        // → 再无则 candwin2 焦点窗兜底。
        let compless = g.composition.is_none();
        let suppress = !compless
            && (g.caret.is_none() || (wps_settle && !(wps_stable || wps_deadline)))
            && !pinned_now
            && !host_is_searchhost()
            && !is_preview; // 实机预览：锚点即位置，不走 caret 抑制链
        if suppress {
            if let Some(c) = g.cand2.as_mut() {
                c.hide();
            }
            g.wps_caret_prev = g.caret;
            if g.wps_settle_start.is_none() {
                g.wps_settle_start = Some(std::time::Instant::now());
            }
            g.suppress_pending = true;
            g.caret_force = true; // 补显时强制重查锚点（旧行框矫正）
            arm_first_frame_timer();
            return Ok(());
        }
        // 【退场淡出修复 2026-09-11】空帧到此不再下落 show：此前直落
        // 2186 用空数据再 show 一次并把 last_show 覆盖成空——渐隐
        // tick 复渲染空内容=无东西可淡，「退出无动画」的根因。
        let content_empty = cands.is_empty() && raw.is_empty();
        if !content_empty {
            match g.cand2.as_mut() {
                Some(c) => c.show(&cands, &raw, &skin, caret.as_ref(), sel),
                None => {}
            }
        }
        // 【滚轮缩放候选框】缓存渲染参数：WM_MOUSEWHEEL 改字号后
        // 免键事件立即重绘（空帧保留旧值供退场渐隐复渲染）
        if !content_empty {
            g.last_show = Some((cands.clone(), raw.clone(), sel));
        }
        // 显示完成：清除首帧抑制补显标记
        g.cand_shown_this_segment = true;
        g.wps_caret_prev = None;
        g.wps_settle_start = None;
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
    let (target, client_id, epoch) = {
        let g = shared.lock().unwrap_or_else(|e| e.into_inner());
        let target = match ctx {
            Some(c) => c,
            None => g
                .focus_context()
                .ok_or_else(|| Error::from(HRESULT(-2147467259)))?,
        };
        (target, g.client_id, g.focus_epoch)
    };
    // 结果槽：同步档回调内联执行，受理返回时槽已填——把真实执行
    // 结果上抛（旧实现受理=成功的假阳性，见 struct 注记）；异步档
    // 无法同步等待（回调要本线程泵消息），如实按「已排队」返回。
    let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let session: ITfEditSession = EditSession {
        shared: shared.clone(),
        op,
        ctx_override: Some(target.clone()),
        epoch,
        result_slot: Some(slot.clone()),
    }
    .into();
    unsafe {
        let mut granted = None;
        for flags in [0x6u32, 0xA] {
            match target.RequestEditSession(
                client_id,
                &session,
                TF_CONTEXT_EDIT_CONTEXT_FLAGS(flags),
            ) {
                Ok(h) => {
                    trace(&format!(
                        "session grant = 0x{:08X} (flags={flags})",
                        h.0 as u32
                    ));
                    granted = Some((h, flags));
                    break;
                }
                Err(e) => {
                    trace(&format!(
                        "session denied flags={flags} err=0x{:08X}",
                        e.code().0 as u32
                    ));
                }
            }
        }
        let Some((_, flags)) = granted else {
            return Err(Error::from(HRESULT(-2147467259)));
        };
        if flags == 0x6 {
            // 同步档：读回调真实结果
            if let Ok(s) = slot.lock() {
                if let Some(ok) = *s {
                    if !ok {
                        return Err(Error::from(HRESULT(-2147467259)));
                    }
                }
            }
        } else {
            trace("run_session: 异步档已排队（结果不可同步等待）");
        }
    }
    Ok(())
}

/// 焦点回调专用：只请求同步档 edit session，被拒即失败（不排队异步）。
/// 异步 session 的回调需要宿主 UI 线程泵消息，Chromium 系应用在焦点
/// 切换期持内部锁 → 排队即死锁（VSCode 点击候选框冻结事故）。
fn run_session_sync_only(shared: &SharedRef, op: Op, ctx: ITfContext) -> Result<()> {
    let (client_id, epoch) = {
        let g = shared.lock().unwrap_or_else(|e| e.into_inner());
        (g.client_id, g.focus_epoch)
    };
    let session: ITfEditSession = EditSession {
        shared: shared.clone(),
        op,
        ctx_override: Some(ctx.clone()),
        epoch,
        result_slot: None,
    }
    .into();
    unsafe {
        match ctx.RequestEditSession(client_id, &session, TF_CONTEXT_EDIT_CONTEXT_FLAGS(0x6)) {
            Ok(_) => {
                trace("focus-commit: 同步档受理");
                Ok(())
            }
            Err(e) => {
                trace(&format!(
                    "focus-commit: 同步被拒 0x{:08X}，放弃提交",
                    e.code().0 as u32
                ));
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

/// 【开始菜单=explorer 承载 2026-09-11】Win11 部分版本开始菜单搜索的
/// 输入焦点窗在 Explorer.EXE 的 ApplicationFrameWindow（UWP 壳）里，
/// 不在 SearchHost/SystemApps——按 exe 路径判不出「打包」。此时视同
/// 打包宿主：owned 窗（动效瞬跳，DComp 中间帧直角根除）+ 逐键选区
/// 跟随（组段光标冻结同修）。判定=本进程 explorer 且当前线程焦点窗
/// 的顶层类是 UWP 壳窗（文件管理器重命名=CabinetWClass 不受影响）。
/// GetFocus/GetClassName 均为无锁 user32 调用——持 G_SHARED 锁处调用
/// 无重入风险（focus_view_hwnd 有锁，勿用它）。
fn focus_is_uwp_shell() -> bool {
    static EXE_IS_EXPLORER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*EXE_IS_EXPLORER.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
            .map(|s| s.eq_ignore_ascii_case("explorer.exe"))
            .unwrap_or(false)
    }) {
        return false;
    }
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetAncestor, GetClassNameW, GA_ROOT,
        };
        let f = GetFocus();
        if f.is_invalid() {
            return false;
        }
        let root = GetAncestor(f, GA_ROOT);
        if root.is_invalid() {
            return false;
        }
        let mut buf = [0u16; 64];
        let n = GetClassNameW(root, &mut buf);
        if n <= 0 {
            return false;
        }
        let cls = String::from_utf16_lossy(&buf[..n as usize]);
        cls == "ApplicationFrameWindow" || cls == "Windows.UI.Core.CoreWindow"
    }
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
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_default();
        exe.eq_ignore_ascii_case("SearchHost.exe")
    })
}

/// 宿主是否 WPS 全家（wps 文字 / et 表格 / wpp 演示 / wpspdf）。
/// 【表格首键跳位 2026-09-06】et.exe 表格里 GetTextExt 首键返回
/// 陈旧 caret（上一单元格位置，Some 非 None）——原「首帧锚点缺失
/// 抑制」只盖 caret=None，表格首键候选先显示旧位、第二帧跳回光标
/// （用户实测首键跳、第二键才跟随）。WPS 族组段首键帧（raw 单键）
/// 一律延迟一帧 + 35ms 补显，等表格布局把 caret 刷稳。
fn host_is_wps() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| {
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_default();
        let l = exe.to_lowercase();
        l == "et.exe" || l == "wps.exe" || l == "wpp.exe" || l == "wpspdf.exe"
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
    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
    g.cand_ui_active = true;
    g.cand_ui_host_draws = false;
    // 【皮肤去重推送 2026-09-11】仅换肤/首推/上次失败时携带 skin
    //（server 端缓存上帧皮肤），其余帧省掉整份 JSON 的 clone+序列化
    //+管道字节——沉浸宿主逐键帧是无谓的 KB 级重复开销。
    let need_skin = g.srv_skin_ver_pushed != g.skin_ver_last;
    let skin = if need_skin {
        g.skin.clone()
    } else {
        serde_json::Value::Null
    };
    drop(g);
    // 宿主顶层窗（前台窗——代画显示时宿主必为前台）：交给 server 自守
    //（宿主窗不可见时 server 自收代画窗——开始菜单残留的根治）
    let host_hwnd = unsafe { GetForegroundWindow() };
    let host_hwnd = if host_hwnd.0.is_null() {
        0
    } else {
        host_hwnd.0 as isize
    };
    let ok = pipe_cand_push(cands, raw, sel, x, y, &skin, host_hwnd);
    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
    if ok {
        if need_skin {
            g.srv_skin_ver_pushed = g.skin_ver_last;
        }
    } else {
        // 推送失败（server 重启/管道断）：皮肤缓存失效，下帧重推
        g.srv_skin_ver_pushed = u64::MAX;
    }
}

/// server 代画：pipe 推送候选帧（皮肤仅换肤时携带，server 按皮肤渲染；
/// 返回是否推送成功——失败时调用方标记下帧重推皮肤）
fn pipe_cand_push(
    cands: &[(String, String)],
    raw: &str,
    sel: usize,
    x: i32,
    y: i32,
    skin: &serde_json::Value,
    host_hwnd: isize,
) -> bool {
    let items: Vec<serde_json::Value> = cands
        .iter()
        .map(|(t, c)| serde_json::json!({"text": t, "comment": c}))
        .collect();
    let r = crate::ipc::call(&serde_json::json!({
        "op": "cand",
        "items": items,
        "raw": raw,
        "selected": sel,
        "x": x,
        "y": y,
        "skin": skin,
        "host_hwnd": host_hwnd,
    }));
    // 【诊断降噪 2026-09-11】旧实现每帧 diag_note（沉浸宿主逐键帧
    // 写盘）——只在出错时落一条。
    if r.is_none() {
        diag_note(&format!(
            "srv push FAIL n={} err={:?}",
            cands.len(),
            crate::ipc::LAST_PIPE_ERR.load(std::sync::atomic::Ordering::SeqCst)
        ));
    }
    r.is_some()
}

/// 结束沉浸式候选（编码结束/失焦）：两通道各自收尾
fn ui_element_hide(shared: &SharedRef) {
    use windows::Win32::UI::TextServices::ITfUIElementMgr;
    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
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
    CreateWindowExW, DefWindowProcW, GetForegroundWindow, GetGUIThreadInfo, GetWindowRect,
    GetWindowThreadProcessId, KillTimer, RegisterClassW, SetTimer, GUITHREADINFO, HWND_MESSAGE,
    WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
};

/// 【反查首帧锚点 2026-09-11】无组段帧（反查/命令模式刚进入：仅 aux
/// 提示、应用文本流里什么都没有）的插入点兜底：前台线程 GUITHREADINFO
/// 的系统插入符（hCaret/rcCaret 客户区坐标→屏幕）。这是不依赖组段的
/// 唯一光标来源——query_caret 需要组段（GetTextExt），首帧查不出。
fn gui_caret_fallback() -> Option<RECT> {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return None;
        }
        let tid = GetWindowThreadProcessId(fg, None);
        let mut gi = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(tid, &mut gi).is_ok() && !gi.hwndCaret.0.is_null() {
            let w = (gi.rcCaret.right - gi.rcCaret.left).max(2);
            let mut pt = POINT {
                x: gi.rcCaret.left,
                y: gi.rcCaret.bottom,
            };
            if windows::Win32::Graphics::Gdi::ClientToScreen(gi.hwndCaret, &mut pt).as_bool() {
                return Some(RECT {
                    left: pt.x,
                    top: pt.y,
                    right: pt.x + w,
                    bottom: pt.y,
                });
            }
        }
        None
    }
}

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
            let _ = SetTimer(HWND(h as *mut _), FIRST_TIMER_ID, FIRST_FRAME_MS, None);
        }
    }
}

/// 武装上屏跟随重查定时器（update_ui 的 CommitAndRepreedit 分支调用；
/// 与 poll 窗同线程。重复调用同 id=重置，幂等）。
fn arm_caret_recheck_timer() {
    let h = POLL_HWND.load(AtomicOrdering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = SetTimer(HWND(h as *mut _), CARET_TIMER_ID, CARET_RECHECK_MS, None);
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
    // 【2026-09-11 签名扩展】中英态/辅助提示纳入签名：外部改模式
    //（设置页、管道 toggle）无按键时，原签名（候选+选中）不变 →
    // poll 去重跳过 update_ui → 语言栏指示牌永不刷新（实测定格）。
    let zh = state
        .pointer("/chinese")
        .or_else(|| state.pointer("/state/chinese"))
        .and_then(|v| v.as_bool());
    let aux = state.get("aux").and_then(|v| v.as_str()).unwrap_or("");
    format!(
        "{}|{}|{}|{}",
        texts.join("\u{1}"),
        sel,
        zh.map(|b| b as u8).unwrap_or(2),
        aux
    )
}

extern "system" fn poll_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
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
            let shared = POLL_SHARED.lock().unwrap().as_ref().map(|p| p.0.clone());
            if let Some(shared) = shared {
                {
                    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
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
                        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
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
    *POLL_SHARED.lock().unwrap_or_else(|e| e.into_inner()) = Some(PollShared(shared.clone()));
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
fn fg_exe_lower(pid: u32) -> Option<String> {
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return None;
        };
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
                .is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]).to_lowercase())
    }
}

/// 【双候选窗修复 2026-09-11】前台进程是否 UWP 框架族（打包应用/
/// ApplicationFrameHost/搜索宿主）。原「打包宿主一律豁免他进程收窗」
/// 太宽：notepad 打完字切到普通应用（Typora 等），notepad 的 owned
/// 窗既收不到失焦回调（UWP 焦点通知不可靠）也不被 poll 收——残留
/// 第二个候选窗且内容还跟着引擎刷新（用户实测「两个候选框都生效，
/// 分别在两个主体里」）。收紧：豁免仅当前台进程仍是 UWP 框架族
/// （Store 打字时前台=ApplicationFrameHost，pid 永远≠内容进程，
/// 不豁免会把正在打字的候选误收——原 Store 只闪一下的教训）；
/// 前台是普通桌面应用=真离开了，照收残留。
fn fg_is_uwp_frame(pid: u32) -> bool {
    match fg_exe_lower(pid) {
        Some(p) => {
            p.contains("\\windowsapps\\")
                || p.contains("\\systemapps\\")
                || p.ends_with("\\applicationframehost.exe")
        }
        None => true, // 查不到（权限/竞态）——保守豁免，不误收
    }
}

fn fg_same_app_dir(pid: u32) -> bool {
    let Some(fg_path) = fg_exe_lower(pid) else {
        return false;
    };
    let fg_dir = std::path::PathBuf::from(fg_path)
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

fn poll_tick() {
    // 重入保护（update_ui 过程中不会再泵本消息，双保险）
    if POLL_IN_TICK.swap(1, AtomicOrdering::Relaxed) != 0 {
        return;
    }
    let _guard = scopeguard_release();
    // 【打字期静默】500ms 内有按键 → 键路径在活跃，poll 只会抢管道
    // /抢锁（键请求被队头阻塞的温床）。跳过本拍。
    // 【回归修复 2026-09-08】首帧抑制的 35ms 补显走 poll 路径——
    // 打字期全静默把补显也饿死（用户实测「没有候选框」：连打期间
    // suppress_pending 永不消费）。豁免：待补显/上屏重查/皮肤重绘
    // 在身时 poll 必须跑（跑一拍消费掉标志后恢复静默）。
    {
        let sp = POLL_SHARED.lock().unwrap().as_ref().map(|p| p.0.clone());
        if let Some(s) = sp {
            let g = s.lock().unwrap_or_else(|e| e.into_inner());
            let busy = g.last_key_at.is_some_and(|t| t.elapsed().as_millis() < 500);
            let owes = g.suppress_pending || g.caret_recheck_due || g.skin_repaint;
            drop(g);
            if busy && !owes {
                return;
            }
        }
    }
    let n = POLL_TICKS.fetch_add(1, AtomicOrdering::Relaxed);
    if n < 3 {
        diag_note(&format!("poll: tick #{} 开始", n + 1));
    }
    // 【语言栏缓存 2026-09-11】方案清单/音效快照在 poll 线程刷新
    //（5s 节流）——右键菜单 msctf 回调里零管道。
    crate::langbar::refresh_schemas_cache();
    let shared = POLL_SHARED.lock().unwrap().as_ref().map(|p| p.0.clone());
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
                && !(host_is_packaged() && !host_is_searchhost() && fg_is_uwp_frame(pid))
                && !fg_same_app_dir(pid)
            {
                let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                let mut any_visible = false;
                if let Some(c) = g.cand2.as_mut() {
                    any_visible |= c.is_visible();
                    c.hide();
                }
                // 沉浸式宿主两通道也收尾（UWP/搜索框走 UIElement 或
                // server 代画——只藏 cand2 不够，残留正是缺这段）：
                // 【锁外 IPC 2026-09-11】EndUIElement（COM，宿主回调）
                // 与 ipc::call（同步管道，server 卡住最坏秒级）都在持
                // shared 锁内做——server 一卡，键路径抢锁失败=宿主 UI
                // 卡死（对照 ui_element_show 先 drop(g) 的正确姿势）。
                // 先摘参数、放锁、锁外执行。
                let ui_active = g.cand_ui_active;
                let ui_host_draws = g.cand_ui_host_draws;
                let ui_id = g.cand_ui_id;
                let mgr = if ui_active && ui_host_draws {
                    g.thread_mgr.as_ref().and_then(|tm| {
                        tm.cast::<windows::Win32::UI::TextServices::ITfUIElementMgr>()
                            .ok()
                    })
                } else {
                    None
                };
                g.cand_ui_active = false;
                drop(g);
                if ui_active {
                    if ui_host_draws {
                        if let Some(mgr) = &mgr {
                            let _ = unsafe { mgr.EndUIElement(ui_id) };
                            diag_note("poll: 前台他进程 → EndUIElement（残留兜底）");
                        }
                    } else {
                        let _ = crate::ipc::call(&serde_json::json!({"op": "cand_hide"}));
                        diag_note("poll: 前台他进程 → srv cand_hide（残留兜底）");
                    }
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
    let Some(state) = ipc::state_request() else {
        return;
    };
    let raw_empty = state
        .get("raw")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .is_empty();
    let sig = state_sig(&state);
    let mut need_show = false;
    {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        // 【皮肤版本失效 2026-09-08】server 保存过皮肤（版本号变化）
        // → 强制重拉绕 2.5s 时限：设置页连续调参时实机预览即时生效。
        // 放 raw_empty 判定前——预览流程 reset（断段）与打 w 之间的
        // 任意一拍都会走到这里。skin_repaint 置位：签名未变时也按
        // 新皮肤重绘一次（一次性，下方消费清除）。
        let sv = state.get("skin_ver").and_then(|v| v.as_u64()).unwrap_or(0);
        if sv != g.skin_ver_last {
            g.skin_ver_last = sv;
            if !g.skin.is_null() {
                g.load_skin_forced();
                g.skin_repaint = true;
            }
        }
        // 【方案变更兜底 2026-09-11】state 常带 current_schema：比对变
        // 化即置皮肤失效——entrance_anim 等方案相关字段即时跟随
        //（覆盖设置页/langbar 切方案；Ctrl+M 键路径另有即时标记）。
        if let Some(cs) = state.get("current_schema").and_then(|v| v.as_str()) {
            if !cs.is_empty() && cs != g.last_schema_seen {
                g.last_schema_seen = cs.to_string();
                if !g.skin.is_null() {
                    g.skin_stale = true;
                }
            }
        }
        if raw_empty {
            g.cand_sig_last = String::new();
            g.suppress_pending = false;
            g.cand_shown_this_segment = false;
            g.caret_force = false; // 断段：补显强制重查标志失效
            g.wps_caret_prev = None;
            g.wps_settle_start = None;
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
            // 【实机预览重绘·仅限皮肤刚变化 2026-09-08】签名未变且
            // 皮肤刚重拉（skin_repaint 一次性标志）时按上帧渲染参数
            // 重绘一次。此前为无条件——候选窗显示期间 poll 每 40ms
            // 全帧重绘（每秒 25 次白绘），打字卡顿实锤（用户实测
            // 「打着打着会卡」）。
            if g.skin_repaint && g.cand_shown_this_segment {
                g.skin_repaint = false;
                let last = g.last_show.take();
                let skin = g.skin.clone();
                let caret = g.caret;
                let anchor = state.get("preview_anchor").cloned();
                if let (Some(c), Some((cands, raw2, sel))) = (g.cand2.as_mut(), last) {
                    let anchor_rect = anchor.and_then(|a| {
                        let x = a.get("x").and_then(|v| v.as_i64())? as i32;
                        let y = a.get("y").and_then(|v| v.as_i64())? as i32;
                        Some(RECT {
                            left: x,
                            top: y,
                            right: x,
                            bottom: y,
                        })
                    });
                    let caret2 = anchor_rect.or(caret);
                    c.show(&cands, &raw2, &skin, caret2.as_ref(), sel);
                    g.last_show = Some((cands, raw2, sel));
                }
            }
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
/// 其余宿主（含虎魄跟打器）锚组段起点——段内位置恒定且零 GetTextExt
///（虎魄 2026-09-08 移出：逐键布局查询×2 在跟打器上卡秒级，见
/// query_caret 注释）。
fn host_follow_caret() -> bool {
    // 【恒 true 2026-09-12 用户拍板】所有应用实时跟随最新光标。旧
    // 「虎魄」名单（锚组段起点防 GetTextExt 布局锁卡 5.1s）退役：
    // 实时光标实现在名单为 true 时优先系统插入符（GUITHREADINFO，
    // 零 TSF 回调不进布局锁）——虎魄跟打器（真宿主名「虎魄跟打器
    // .exe」，此前 TigerClaw 判定是错靶）也走此轻量路径，卡顿前提
    // 不复存在。查不到系统插入符才回落 GetTextExt。
    true
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
