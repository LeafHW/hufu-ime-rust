//! TSF 文本服务：按键 → 管道引擎 → 组段/上屏 + 候选窗。

use crate::candwin::CandidateWindow;
use crate::candwin2::CandidateWindowV2;
use crate::ipc;
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::*;
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
        }
    }

    fn load_skin(&mut self) {
        // 编码会话开始（raw 空）时重新拉取皮肤 —— 设置界面改皮肤后，
        // 下一次打字即生效（近热更新）
        if self.skin.is_null() || self.skin_stale {
            if let Some(v) = ipc::call(&serde_json::json!({"op": "skin"})) {
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
#[implement(ITfTextInputProcessor, ITfTextInputProcessorEx, ITfKeyEventSink)]
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
        }
        let mut g = self.shared.lock().unwrap();
        g.thread_mgr = Some(tm);
        g.client_id = tid;
        // 激活标记（冒烟测试读取：证明 msctf 真实激活管线走到了这里）
        let marker = std::env::temp_dir().join("hufu-tsf-activated.txt");
        let _ = std::fs::write(&marker, format!("tid={tid} t={:?}\n", std::time::SystemTime::now()));
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        let mut g = self.shared.lock().unwrap();
        if let Some(tm) = g.thread_mgr.clone() {
            if let Ok(km) = tm.cast::<ITfKeystrokeMgr>() {
                unsafe {
                    let _ = km.UnadviseKeyEventSink(g.client_id);
                }
            }
        }
        g.composition = None;
        if let Some(c) = g.cand2.take() {
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
        Ok(BOOL(0))
    }

    fn OnPreservedKey(&self, _pic: Option<&ITfContext>, _rguid: *const GUID) -> Result<BOOL> {
        Ok(BOOL(0))
    }
}

impl HuFuTs_Impl {
    /// 键分派：VK → 名称+修饰 → 管道引擎 → 更新组段与候选窗。
    fn dispatch(&self, wparam: usize, test_only: bool) -> BOOL {
        let Some((name, shift, ctrl, alt)) = vk_to_name(wparam) else {
            return BOOL(0);
        };
        let (name, m_shift, m_ctrl, m_alt) = match name.as_str() {
            "shift" | "ctrl" | "alt" => (name, false, false, false),
            _ => (name, shift, ctrl, alt),
        };
        let Some((consumed, commit, state)) = ipc::key_request(&name, m_shift, m_ctrl, m_alt)
        else {
            return BOOL(0);
        };
        if !consumed {
            return BOOL(0);
        }
        if !test_only {
            let _ = update_ui(self.shared.clone(), commit, state);
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
        Some((consumed, _commit, _state)) => {
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
    End,
}

#[implement(ITfEditSession)]
struct EditSession {
    shared: SharedRef,
    op: Op,
}

impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        let mut g = self.shared.lock().unwrap();
        let ctx = g
            .focus_context()
            .ok_or_else(|| Error::from(HRESULT(-2147467259)))?;
        match &self.op {
            Op::StartPreedit(text) => {
                let ins: ITfInsertAtSelection = ctx.cast()?;
                let wstr: Vec<u16> = text.encode_utf16().collect();
                let range: ITfRange =
                    unsafe { ins.InsertTextAtSelection(ec, INSERT_TEXT_AT_SELECTION_FLAGS(0), &wstr)? };
                let cc: ITfContextComposition = ctx.cast()?;
                let sink: ITfCompositionSink = CompSinkObj.into();
                let comp: ITfComposition = unsafe { cc.StartComposition(ec, &range, &sink)? };
                g.composition = Some(comp);
                Ok(())
            }
            Op::SetPreedit(text) => {
                let comp = g
                    .composition
                    .clone()
                    .ok_or_else(|| Error::from(HRESULT(-2147467259)))?;
                let range: ITfRange = unsafe { comp.GetRange()? };
                let wstr: Vec<u16> = text.encode_utf16().collect();
                unsafe { range.SetText(ec, 0, &wstr)? };
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
                }
                g.composition = None;
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
    let mut g = shared.lock().unwrap();
    let raw = state.get("raw").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let preedit = state
        .get("preedit")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // 编码会话结束 → 皮肤缓存过期（下次会话重新拉取）
    if raw.is_empty() {
        g.skin_stale = true;
    }
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

    if g.focus_context().is_none() {
        return Ok(());
    }

    // 1) 组段
    if raw.is_empty() && preedit.is_empty() {
        if !commit.is_empty() || g.composition.is_some() {
            run_session(&shared, Op::Commit(commit.clone()))?;
        }
    } else if g.composition.is_none() {
        run_session(&shared, Op::StartPreedit(preedit.clone()))?;
    } else {
        run_session(&shared, Op::SetPreedit(preedit.clone()))?;
    }

    // 2) 候选窗（v2 优先，初始化失败回退 v1）
    g.load_skin();
    if cands.is_empty() {
        if let Some(c) = g.cand2.as_ref() {
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
        }
        let skin = g.skin.clone();
        match g.cand2.as_mut() {
            Some(c) => c.show(&cands, &raw, &skin),
            None => {}
        }
        if g.cand2_dead {
            if g.cand.is_none() {
                g.cand = Some(CandidateWindow::new());
            }
            if let Some(c) = g.cand.as_ref() {
                c.show(&cands, &raw, &g.skin);
            }
        }
    } else {
        if g.cand.is_none() {
            g.cand = Some(CandidateWindow::new());
        }
        if let Some(c) = g.cand.as_ref() {
            c.show(&cands, &raw, &g.skin);
        }
    }
    Ok(())
}

fn run_session(shared: &SharedRef, op: Op) -> Result<()> {
    let (ctx, client_id) = {
        let g = shared.lock().unwrap();
        (
            g.focus_context()
                .ok_or_else(|| Error::from(HRESULT(-2147467259)))?,
            g.client_id,
        )
    };
    let session: ITfEditSession = EditSession {
        shared: shared.clone(),
        op,
    }
    .into();
    unsafe {
        // TF_ES_READWRITE | TF_ES_SYNC = 3
        let _grant = ctx.RequestEditSession(client_id, &session, TF_CONTEXT_EDIT_CONTEXT_FLAGS(3))?;
    }
    Ok(())
}
