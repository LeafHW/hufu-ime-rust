//! Read-only host projection. Original windows remain independently owned.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[cfg(test)]
use std::rc::Rc;
use std::{
    cell::RefCell,
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};
use windows::core::AgileReference;
use windows::{
    core::{implement, Interface, Result, BSTR, GUID},
    Win32::{
        Foundation::{BOOL, E_INVALIDARG, E_NOTIMPL},
        UI::TextServices::*,
    },
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Menu {
    texts: Vec<String>,
    pages: Vec<u32>,
    selected: u32,
    page: u32,
}
impl Menu {
    fn valid(&self) -> bool {
        !self.texts.is_empty()
            && self.texts.len() <= 65536
            && self.pages.first() == Some(&0)
            && self.pages.windows(2).all(|p| p[0] < p[1])
            && self
                .pages
                .last()
                .is_some_and(|p| (*p as usize) < self.texts.len())
            && (self.page as usize) < self.pages.len()
            && (self.selected as usize) < self.texts.len()
            && self.selected >= self.pages[self.page as usize]
            && self.selected
                < self
                    .pages
                    .get(self.page as usize + 1)
                    .copied()
                    .unwrap_or(self.texts.len() as u32)
            && self.texts.iter().all(|s| !s.is_empty())
    }
    fn flags(&self, old: &Self) -> u32 {
        (if self.texts != old.texts {
            TF_CLUIE_STRING
        } else {
            0
        }) | (if self.texts.len() != old.texts.len() {
            TF_CLUIE_COUNT
        } else {
            0
        }) | (if self.selected != old.selected {
            TF_CLUIE_SELECTION
        } else {
            0
        }) | (if self.pages != old.pages {
            TF_CLUIE_PAGEINDEX
        } else {
            0
        }) | (if self.page != old.page {
            TF_CLUIE_CURRENTPAGE
        } else {
            0
        })
    }
}
const ALL: u32 = TF_CLUIE_DOCUMENTMGR
    | TF_CLUIE_COUNT
    | TF_CLUIE_SELECTION
    | TF_CLUIE_STRING
    | TF_CLUIE_PAGEINDEX
    | TF_CLUIE_CURRENTPAGE;
#[derive(Clone)]
struct Snapshot {
    menu: Menu,
    flags: u32,
}
// windows-rs 0.58 implements IAgileObject by default. Keep snapshot storage
// thread-safe, and retain the document via COM's agile-reference marshaler.
struct SnapshotCell(RwLock<Snapshot>);
impl SnapshotCell {
    fn new(value: Snapshot) -> Self {
        Self(RwLock::new(value))
    }
    fn borrow(&self) -> RwLockReadGuard<'_, Snapshot> {
        self.0.read().unwrap_or_else(|e| e.into_inner())
    }
    fn borrow_mut(&self) -> RwLockWriteGuard<'_, Snapshot> {
        self.0.write().unwrap_or_else(|e| e.into_inner())
    }
}
#[implement(ITfCandidateListUIElement)]
struct Candidate {
    data: Arc<SnapshotCell>,
    doc: Option<AgileReference<ITfDocumentMgr>>,
    thread: u32,
}
impl ITfUIElement_Impl for Candidate_Impl {
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("HuFu candidates"))
    }
    fn GetGUID(&self) -> Result<GUID> {
        Ok(GUID::from_u128(0x00d4ea5d_6ccb_48ed_a84e_011d50e68121))
    }
    // FALSE means the host may render. This first bridge deliberately keeps both windows.
    fn Show(&self, _: BOOL) -> Result<()> {
        Ok(())
    }
    fn IsShown(&self) -> Result<BOOL> {
        Ok(BOOL(
            crate::tsf::candidate_window_visible(self.thread) as i32
        ))
    }
}
impl ITfCandidateListUIElement_Impl for Candidate_Impl {
    fn GetUpdatedFlags(&self) -> Result<u32> {
        Ok(self.data.borrow().flags)
    }
    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        self.doc
            .as_ref()
            .ok_or_else(|| windows::core::Error::from(E_INVALIDARG))?
            .resolve()
    }
    fn GetCount(&self) -> Result<u32> {
        Ok(self.data.borrow().menu.texts.len() as u32)
    }
    fn GetSelection(&self) -> Result<u32> {
        Ok(self.data.borrow().menu.selected)
    }
    fn GetString(&self, i: u32) -> Result<BSTR> {
        self.data
            .borrow()
            .menu
            .texts
            .get(i as usize)
            .map(|s| BSTR::from(s.as_str()))
            .ok_or(E_INVALIDARG.into())
    }
    fn GetPageIndex(&self, p: *mut u32, n: u32, count: *mut u32) -> Result<()> {
        if count.is_null() || (n > 0 && p.is_null()) {
            return Err(E_INVALIDARG.into());
        }
        let s = self.data.borrow();
        let pages = &s.menu.pages;
        unsafe {
            *count = pages.len() as u32;
        }
        if n == 0 {
            return Ok(());
        }
        if (n as usize) < pages.len() {
            return Err(E_INVALIDARG.into());
        }
        unsafe {
            std::ptr::copy_nonoverlapping(pages.as_ptr(), p, pages.len());
        }
        Ok(())
    }
    fn SetPageIndex(&self, _: *const u32, _: u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetCurrentPage(&self) -> Result<u32> {
        Ok(self.data.borrow().menu.page)
    }
}
struct Active {
    mgr: ITfUIElementMgr,
    id: u32,
    _object: ITfUIElement,
    data: Arc<SnapshotCell>,
}
#[derive(Default)]
struct Bridge {
    generation: u64,
    context: Option<ITfContext>,
    token: String,
    instance: String,
    version: u64,
    retired: Vec<String>,
    active: Option<Active>,
}
thread_local! { static BRIDGE:RefCell<Bridge> = RefCell::new(Bridge::default()); }
fn end(a: Option<Active>) {
    if let Some(a) = a {
        unsafe {
            let _ = a.mgr.EndUIElement(a.id);
        }
    }
}
pub fn reset() {
    let a = BRIDGE.with(|b| {
        let mut b = b.borrow_mut();
        b.generation += 1;
        b.context = None;
        b.token.clear();
        b.instance.clear();
        b.version = 0;
        b.retired.clear();
        b.active.take()
    });
    end(a);
}
pub fn prepare(ctx: Option<&ITfContext>) {
    let same = BRIDGE.with(|b| b.borrow().context.as_ref() == ctx);
    if same {
        return;
    }
    reset();
    if let Some(ctx) = ctx {
        BRIDGE.with(|b| {
            let mut b = b.borrow_mut();
            b.context = Some(ctx.clone());
            b.token = format!(
                "{}-{}-{}-{}",
                std::process::id(),
                unsafe { windows::Win32::System::Threading::GetCurrentThreadId() },
                b.generation,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );
        });
    }
}
pub fn request() -> Value {
    BRIDGE.with(|b|{let b=b.borrow();json!({"protocol":1,"context":b.token,"instance":b.instance,"version":if b.active.is_some(){b.version}else{0}})})
}
thread_local! {
    static PUBLISHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PENDING: RefCell<Option<Value>> = const { RefCell::new(None) };
}
pub fn publish(state: &Value) {
    if PUBLISHING.with(|b| b.replace(true)) {
        PENDING.with(|p| *p.borrow_mut() = Some(state.clone()));
        return;
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            PUBLISHING.with(|b| b.set(false));
        }
    }
    let _guard = Guard;
    publish_inner(state);
    loop {
        let next = PENDING.with(|p| p.borrow_mut().take());
        match next {
            Some(s) => publish_inner(&s),
            None => break,
        }
    }
}
fn publish_inner(state: &Value) {
    let Some(ext) = state.get("candidate_ui") else {
        clear_menu();
        return;
    };
    let (token, gen, ctx, old_instance, old_version) = BRIDGE.with(|b| {
        let b = b.borrow();
        (
            b.token.clone(),
            b.generation,
            b.context.clone(),
            b.instance.clone(),
            b.version,
        )
    });
    if token.is_empty() || ext["context"].as_str() != Some(token.as_str()) {
        return;
    }
    if ext["protocol"].as_u64() != Some(1)
        || serde_json::to_vec(ext).map_or(true, |x| x.len() > 1 << 20)
    {
        clear_menu();
        return;
    }
    let Some(instance) = ext["instance"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() < 128)
    else {
        clear_menu();
        return;
    };
    let Some(version) = ext["version"].as_u64() else {
        clear_menu();
        return;
    };
    if instance == old_instance && version < old_version {
        return;
    }
    let Some(value) = ext.get("menu") else {
        if instance != old_instance || version != old_version {
            clear_menu();
        }
        return;
    };
    let rejected = BRIDGE.with(|b| {
        let mut b = b.borrow_mut();
        if b.retired.iter().any(|s| s == instance) {
            return true;
        }
        if !b.instance.is_empty() && b.instance != instance {
            if b.retired.len() >= 16 {
                return true;
            }
            let old = b.instance.clone();
            b.retired.push(old);
        }
        b.instance = instance.into();
        b.version = version;
        false
    });
    if rejected {
        return;
    }
    if value.is_null() {
        clear_menu();
        return;
    }
    let Ok(menu) = serde_json::from_value::<Menu>(value.clone()) else {
        clear_menu();
        return;
    };
    if !menu.valid() {
        clear_menu();
        return;
    }
    let Some((doc, mgr)) = target(ctx.as_ref()) else {
        return;
    };
    let doc = match doc.as_ref().map(AgileReference::new).transpose() {
        Ok(doc) => doc,
        Err(_) => {
            clear_menu();
            return;
        }
    };
    let action = BRIDGE.with(|b| {
        let mut b = b.borrow_mut();
        if b.generation != gen {
            return None;
        }
        b.instance = instance.into();
        b.version = version;
        if let Some(a) = b.active.as_ref() {
            let flags = menu.flags(&a.data.borrow().menu);
            *a.data.borrow_mut() = Snapshot {
                menu: menu.clone(),
                flags,
            };
            Some((Some((a.mgr.clone(), a.id, flags)), None))
        } else {
            let data = Arc::new(SnapshotCell::new(Snapshot { menu, flags: ALL }));
            let candidate: ITfCandidateListUIElement = Candidate {
                thread: unsafe { windows::Win32::System::Threading::GetCurrentThreadId() },
                data: data.clone(),
                doc,
            }
            .into();
            let obj: ITfUIElement = candidate.into();
            Some((None, Some((obj, data))))
        }
    });
    match action {
        Some((Some((mgr, id, flags)), _)) => {
            if flags != 0 {
                unsafe {
                    let _ = mgr.UpdateUIElement(id);
                }
            }
        }
        Some((_, Some((obj, data)))) => {
            let mut show = BOOL(1);
            let mut id = 0;
            if unsafe { mgr.BeginUIElement(&obj, &mut show, &mut id) }.is_ok() {
                let mut pending = Some(Active {
                    mgr: mgr.clone(),
                    id,
                    _object: obj,
                    data,
                });
                BRIDGE.with(|b| {
                    let mut b = b.borrow_mut();
                    if b.generation == gen && b.active.is_none() {
                        b.active = pending.take();
                    }
                });
                end(pending);
            }
        }
        _ => {}
    }
}
fn clear_menu() {
    let a = BRIDGE.with(|b| {
        let mut b = b.borrow_mut();
        b.active.take()
    });
    end(a);
}

fn target(ctx: Option<&ITfContext>) -> Option<(Option<ITfDocumentMgr>, ITfUIElementMgr)> {
    #[cfg(test)]
    if let Some(mgr) = tests::MANAGER.with(|m| m.borrow().clone()) {
        return Some((None, mgr));
    }
    let ctx = ctx?;
    let tm = crate::tsf::thread_tm()?;
    let doc = unsafe { ctx.GetDocumentMgr().ok()? };
    if unsafe { tm.GetFocus().ok().and_then(|d| d.GetTop().ok()) }.as_ref() != Some(ctx) {
        reset();
        return None;
    }
    Some((Some(doc), tm.cast().ok()?))
}
#[cfg(test)]
mod tests {
    use super::*;
    thread_local! { pub static MANAGER:RefCell<Option<ITfUIElementMgr>>=const{RefCell::new(None)}; }
    #[derive(Default)]
    struct Log {
        events: Vec<String>,
        retained: Option<ITfCandidateListUIElement>,
        reenter: bool,
        reset_begin: bool,
        reset_update: bool,
    }
    #[implement(ITfUIElementMgr)]
    struct Manager {
        log: Rc<RefCell<Log>>,
    }
    impl ITfUIElementMgr_Impl for Manager_Impl {
        fn BeginUIElement(
            &self,
            e: Option<&ITfUIElement>,
            show: *mut BOOL,
            id: *mut u32,
        ) -> Result<()> {
            let c: ITfCandidateListUIElement = e.unwrap().cast()?;
            unsafe {
                assert_eq!(c.GetUpdatedFlags()?, ALL);
                *show = BOOL(0);
                *id = 17;
            }
            let (reenter, reset_begin) = {
                let mut l = self.log.borrow_mut();
                l.events.push("begin".into());
                l.retained = Some(c);
                (
                    std::mem::take(&mut l.reenter),
                    std::mem::take(&mut l.reset_begin),
                )
            };
            if reenter {
                publish(&frame(2, 1));
            }
            if reset_begin {
                reset();
            }
            Ok(())
        }
        fn UpdateUIElement(&self, _: u32) -> Result<()> {
            let reset_update = {
                let mut l = self.log.borrow_mut();
                l.events.push("update".into());
                std::mem::take(&mut l.reset_update)
            };
            if reset_update {
                reset();
            }
            Ok(())
        }
        fn EndUIElement(&self, _: u32) -> Result<()> {
            self.log.borrow_mut().events.push("end".into());
            let c = self.log.borrow().retained.clone().unwrap();
            unsafe {
                assert!(c.GetCount()? > 0);
                let _ = c.GetString(0)?;
            }
            Ok(())
        }
        fn GetUIElement(&self, _: u32) -> Result<ITfUIElement> {
            Ok(self.log.borrow().retained.clone().unwrap().into())
        }
        fn EnumUIElements(&self) -> Result<IEnumTfUIElements> {
            Err(E_NOTIMPL.into())
        }
    }
    fn frame(version: u64, page: u32) -> Value {
        json!({"candidate_ui":{"protocol":1,"context":"test","instance":"server-a","version":version,
        "menu":{"texts":["甲","乙","𠀀é"],"pages":[0,2],"selected":if page==0{1}else{2},"page":page}}})
    }
    fn token() {
        BRIDGE.with(|b| b.borrow_mut().token = "test".into());
    }
    #[test]
    fn actual_document_reference_survives_owner_drop() {
        use windows::Win32::System::Com::*;
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            {
                let tm: ITfThreadMgr =
                    CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER).unwrap();
                tm.Activate().unwrap();
                let doc = tm.CreateDocumentMgr().unwrap();
                let data = Arc::new(SnapshotCell::new(Snapshot {
                    menu: Menu {
                        texts: vec!["字".into()],
                        pages: vec![0],
                        selected: 0,
                        page: 0,
                    },
                    flags: ALL,
                }));
                let candidate: ITfCandidateListUIElement = Candidate {
                    thread: unsafe { windows::Win32::System::Threading::GetCurrentThreadId() },
                    data,
                    doc: Some(AgileReference::new(&doc).unwrap()),
                }
                .into();
                assert_eq!(candidate.GetDocumentMgr().unwrap(), doc);
                drop(doc);
                assert!(candidate.GetDocumentMgr().is_ok());
                drop(candidate);
                tm.Deactivate().unwrap();
            }
            CoUninitialize();
        }
    }

    #[test]
    fn actual_com_pain_order_and_lifecycle() {
        unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            )
            .ok()
            .unwrap();
        }
        reset();
        token();
        let log = Rc::new(RefCell::new(Log::default()));
        let mgr: ITfUIElementMgr = Manager { log: log.clone() }.into();
        MANAGER.with(|m| *m.borrow_mut() = Some(mgr));
        publish(&frame(1, 0));
        let c = log.borrow().retained.clone().unwrap();
        let agile = AgileReference::new(&c).unwrap();
        std::thread::spawn(move || unsafe {
            use windows::Win32::System::Com::*;
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
            {
                let proxy = agile.resolve().unwrap();
                assert_eq!(proxy.GetCount().unwrap(), 3);
                assert_eq!(proxy.GetString(2).unwrap().to_string(), "𠀀é");
            }
            CoUninitialize();
        })
        .join()
        .unwrap();
        unsafe {
            assert_eq!(c.GetCount().unwrap(), 3);
            assert_eq!(c.GetSelection().unwrap(), 1);
            let mut count = 0;
            (c.vtable().GetPageIndex)(c.as_raw(), std::ptr::null_mut(), 0, &mut count)
                .ok()
                .unwrap();
            assert_eq!(count, 2);
            let mut pages = [0; 2];
            c.GetPageIndex(&mut pages, &mut count).unwrap();
            assert_eq!(pages, [0, 2]);
            assert!(c.GetPageIndex(&mut pages[..1], &mut count).is_err());
            assert!(
                (c.vtable().GetPageIndex)(c.as_raw(), std::ptr::null_mut(), 1, &mut count)
                    .ok()
                    .is_err()
            );
            assert!(c.GetPageIndex(&mut pages, std::ptr::null_mut()).is_err());
            assert!(c.GetString(3).is_err());
            assert_eq!(c.GetString(2).unwrap().to_string(), "𠀀é");
            assert!(c.cast::<ITfCandidateListUIElementBehavior>().is_err());
            c.Show(false).unwrap();
        }
        publish(&frame(2, 1));
        unsafe {
            assert_eq!(c.GetCurrentPage().unwrap(), 1);
            assert_eq!(c.GetSelection().unwrap(), 2);
            assert_eq!(
                c.GetUpdatedFlags().unwrap(),
                TF_CLUIE_SELECTION | TF_CLUIE_CURRENTPAGE
            );
        }
        publish(&frame(1, 0));
        let mut same = frame(2, 1);
        same["candidate_ui"].as_object_mut().unwrap().remove("menu");
        publish(&same);
        assert_eq!(log.borrow().events, vec!["begin", "update"]);
        let mut foreign = frame(3, 0);
        foreign["candidate_ui"]["context"] = json!("other");
        publish(&foreign);
        assert_eq!(log.borrow().events.len(), 2);
        reset();
        unsafe {
            assert_eq!(c.GetString(2).unwrap().to_string(), "𠀀é");
        }
        assert_eq!(log.borrow().events.last().unwrap(), "end");
        token();
        log.borrow_mut().reenter = true;
        publish(&frame(1, 0));
        assert_eq!(&log.borrow().events[3..], &["begin", "update"]);
        log.borrow_mut().reset_update = true;
        publish(&frame(3, 0));
        assert_eq!(log.borrow().events.last().unwrap(), "end");
        token();
        log.borrow_mut().reset_begin = true;
        publish(&frame(1, 0));
        assert_eq!(log.borrow().events.last().unwrap(), "end");
        assert!(BRIDGE.with(|b| b.borrow().active.is_none()));
        token();
        publish(&frame(1, 0));
        publish(&json!({}));
        assert_eq!(log.borrow().events.last().unwrap(), "end");
        publish(&frame(2, 0));
        let mut bad = frame(3, 0);
        bad["candidate_ui"]["menu"]["pages"] = json!([1, 2]);
        publish(&bad);
        assert_eq!(log.borrow().events.last().unwrap(), "end");
        publish(&frame(4, 0));
        bad = frame(5, 0);
        bad["candidate_ui"]["menu"]["texts"][0] = json!("x".repeat(1 << 20));
        publish(&bad);
        assert_eq!(log.borrow().events.last().unwrap(), "end");
        reset();
        token();
        publish(&frame(10, 0));
        let mut changed = frame(11, 0);
        changed["candidate_ui"]["menu"]["texts"] = json!(["新", "乙", "𠀀é", "末"]);
        changed["candidate_ui"]["menu"]["pages"] = json!([0, 2, 3]);
        publish(&changed);
        let current = log.borrow().retained.clone().unwrap();
        unsafe {
            assert_eq!(
                current.GetUpdatedFlags().unwrap(),
                TF_CLUIE_STRING | TF_CLUIE_COUNT | TF_CLUIE_PAGEINDEX
            );
        }
        let mut restarted = frame(1, 1);
        restarted["candidate_ui"]["instance"] = json!("server-b");
        publish(&restarted);
        publish(&frame(100, 0)); // retired server cannot resurrect even with a larger version
        unsafe {
            assert_eq!(current.GetCurrentPage().unwrap(), 1);
        }
        let mut empty = restarted.clone();
        empty["candidate_ui"]["version"] = json!(2);
        empty["candidate_ui"]["menu"] = Value::Null;
        publish(&empty);
        publish(&restarted);
        assert!(BRIDGE.with(|b| b.borrow().active.is_none()));
        reset();
        MANAGER.with(|m| m.borrow_mut().take());
        unsafe {
            windows::Win32::System::Com::CoUninitialize();
        }
    }
}
// Opt-in isolated probe exports: never included in normal release builds.
#[cfg(feature = "candidate-probe")]
mod probe {
    use super::*;
    thread_local! { static DATA:RefCell<Option<Arc<SnapshotCell>>>=const{RefCell::new(None)};
    static WINDOW:RefCell<Option<crate::candwin2::CandidateWindowV2>>=const{RefCell::new(None)}; }
    fn menu(page: u32) -> Menu {
        Menu {
            texts: vec![
                "first".into(),
                "second".into(),
                "remaining".into(),
                "𠀀é".into(),
            ],
            pages: vec![0, 2],
            selected: if page == 0 { 0 } else { 3 },
            page,
        }
    }
    #[no_mangle]
    pub unsafe extern "C" fn CreateTestCandidate(
        page: u32,
        out: *mut *mut std::ffi::c_void,
    ) -> i32 {
        if out.is_null() || page > 1 {
            return E_INVALIDARG.0;
        }
        let data = Arc::new(SnapshotCell::new(Snapshot {
            menu: menu(page),
            flags: ALL,
        }));
        let c: ITfCandidateListUIElement = Candidate {
            thread: windows::Win32::System::Threading::GetCurrentThreadId(),
            data: data.clone(),
            doc: None,
        }
        .into();
        DATA.with(|d| *d.borrow_mut() = Some(data));
        *out = c.into_raw();
        0
    }
    #[no_mangle]
    pub extern "C" fn UpdateTestCandidate(page: u32) -> i32 {
        if page > 1 {
            return E_INVALIDARG.0;
        }
        DATA.with(|d| {
            let d = d.borrow();
            let Some(data) = d.as_ref() else {
                return -1;
            };
            let m = menu(page);
            let flags = m.flags(&data.borrow().menu);
            *data.borrow_mut() = Snapshot { menu: m, flags };
            0
        })
    }
    #[no_mangle]
    pub extern "C" fn ProbeWindow(show: i32) -> i32 {
        WINDOW.with(|w|{
            let mut w=w.borrow_mut();
            if show<0{return w.as_ref().is_some_and(|w|w.is_visible()) as i32;}
            if show==0{
                if let Some(mut window)=w.take(){window.hide();unsafe{let _=windows::Win32::UI::WindowsAndMessaging::DestroyWindow(window.hwnd);}}
                DATA.with(|d|d.borrow_mut().take());return 1;
            }
            if w.is_none(){*w=crate::candwin2::CandidateWindowV2::new();}
            let Some(w)=w.as_mut() else{return 0;};
            w.show(&[("虎符原候选窗".into(),String::new())],"a",&json!({"skin":{"material":{"kind":"solid"},"colors":{"back_color":"#202022FF","candidate_text_color":"#FFFFFFFF"}}}),
                Some(&windows::Win32::Foundation::RECT{left:500,top:100,right:500,bottom:125}),0);
            w.is_visible() as i32
        })
    }
}
