//! 沉浸式宿主（开始菜单搜索等 SystemApps）候选显示：
//! `ITfCandidateListUIElement`——不自绘窗口，把候选列表交给宿主渲染。
//!
//! 病根（2026-08-29 实测）：自绘窗口（DComp 直通窗与 v1 混合窗均试）
//! 在打包宿主进程里会被 DWM 以 DWM_CLOAKED_SHELL 整体隐身，坐标、
//! 可见标志全正常但肉眼不可见。微软拼音/weasel 在搜索框里的候选
//! 走的是 TSF UIElement 通道（宿主自绘），本模块实现同款协议。

use std::cell::RefCell;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::UI::TextServices::*;

/// UIElement 的候选状态（宿主经 COM 接口读取）
pub struct CandUiState {
    pub items: Vec<String>,
    pub selected: usize,
    pub shown: bool,
    pub doc: Option<ITfDocumentMgr>,
}

/// 宿主查询计数（诊断：宿主是否真的在拉取候选）
pub static HOST_QUERIES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn q(note: &str) {
    use std::sync::atomic::Ordering;
    let n = HOST_QUERIES.fetch_add(1, Ordering::SeqCst) + 1;
    if n <= 60 {
        crate::tsf::diag_note(&format!("uiel q#{n} {note}"));
    }
}

#[implement(ITfUIElement, ITfCandidateListUIElement)]
pub struct HuFuCandElement {
    pub state: RefCell<CandUiState>,
}

impl HuFuCandElement {
    pub fn new() -> HuFuCandElement {
        HuFuCandElement {
            state: RefCell::new(CandUiState {
                items: Vec::new(),
                selected: 0,
                shown: false,
                doc: None,
            }),
        }
    }
}

impl ITfUIElement_Impl for HuFuCandElement_Impl {
    fn GetDescription(&self) -> Result<BSTR> {
        q("desc");
        Ok(BSTR::from("HuFu 虎符候选"))
    }

    fn GetGUID(&self) -> Result<GUID> {
        // 与 DLL 其他 GUID 同源风格（随机分配，仅作元素标识）
        Ok(GUID::from_u128(0x8f5c2a20_3e77_4b9c_a1d4_9e0b7c2f5a88))
    }

    fn Show(&self, bshow: BOOL) -> Result<()> {
        q(&format!("SHOW(bshow={})", bshow.0));
        self.state.borrow_mut().shown = bshow.as_bool();
        Ok(())
    }

    fn IsShown(&self) -> Result<BOOL> {
        q("IsShown");
        Ok(BOOL(self.state.borrow().shown as i32))
    }
}

impl ITfCandidateListUIElement_Impl for HuFuCandElement_Impl {
    fn GetUpdatedFlags(&self) -> Result<u32> {
        q("flags");
        // 帧间可变项全量声明：数量/选中/字符串/页
        Ok(TF_CLUIE_COUNT | TF_CLUIE_SELECTION | TF_CLUIE_STRING | TF_CLUIE_PAGEINDEX)
    }

    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        q("docmgr");
        self.state
            .borrow()
            .doc
            .clone()
            .ok_or_else(|| Error::from_hresult(E_FAIL))
    }

    fn GetCount(&self) -> Result<u32> {
        q("count");
        Ok(self.state.borrow().items.len() as u32)
    }

    fn GetSelection(&self) -> Result<u32> {
        q("selection");
        Ok(self.state.borrow().selected as u32)
    }

    fn GetString(&self, uindex: u32) -> Result<BSTR> {
        q("string");
        self.state
            .borrow()
            .items
            .get(uindex as usize)
            .map(|s| BSTR::from(s.as_str()))
            .ok_or_else(|| Error::from_hresult(E_FAIL))
    }

    fn GetPageIndex(&self, pindex: *mut u32, usize_: u32, pupagecnt: *mut u32) -> Result<()> {
        // 单页模型：一页装下全部候选
        unsafe {
            if !pupagecnt.is_null() {
                *pupagecnt = 1;
            }
            if !pindex.is_null() && usize_ >= 1 {
                *pindex = 0;
            }
        }
        Ok(())
    }

    fn SetPageIndex(&self, _pindex: *const u32, _upagecnt: u32) -> Result<()> {
        Ok(())
    }

    fn GetCurrentPage(&self) -> Result<u32> {
        Ok(0)
    }
}
