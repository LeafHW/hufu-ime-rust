//! 【组段下划线·CUAS 支持 2026-09-14】TSF display attribute 提供方。
//!
//! 用户反馈：64 位 TSF-aware 应用（QQ 等）编码显示在应用内且有下划
//! 线（应用自绘）；32 位应用/UWP/开始菜单（CUAS 代理渲染）没有线
//! ——CUAS 依赖 IME 的 ITfDisplayAttributeProvider + 组段 range 上的
//! GUID_PROP_ATTRIBUTE 属性（VT_I4=attribute GUID 的 atom）。此前虎
//! 符两侧都没提供 → CUAS 无从得知编码态样式 → 不画线。
//!
//! 本模块补齐三件套：
//! 1. ITfDisplayAttributeProvider 挂在 TIP（HuFuTs）上：枚出一个
//!    attribute（ATTR_GUID，TF_ATTR_INPUT=输入中——线的具体样式由
//!    CUAS/应用按语义渲染，与 64 位应用观感对齐）。
//! 2. 注册表登记 DisplayAttribute 键（msctf 发现 IME attribute 用，
//!    com.rs 调 register/unregister）。
//! 3. tsf.rs 组段 SetText 后把 atom SetValue 到组段 range；上屏/断段
//!    前 Clear（防下划线残留到已提交文本）。

use windows::core::{implement, GUID, Result};
use windows::Win32::Foundation::{BOOL, E_INVALIDARG, E_POINTER};
use windows::Win32::UI::TextServices::{
    ITfDisplayAttributeInfo, ITfDisplayAttributeInfo_Impl, ITfDisplayAttributeProvider_Impl,
    IEnumTfDisplayAttributeInfo, IEnumTfDisplayAttributeInfo_Impl, TF_ATTR_INPUT, TF_DA_COLOR,
    TF_DA_LINESTYLE, TF_DISPLAYATTRIBUTE,
};
use windows_core::BSTR;

/// 编码态 display attribute GUID（CLSID 尾段 a10→a12 派生，稳定不复用）。
pub const ATTR_GUID: GUID = GUID::from_u128(0x8f5c2a12_3e77_4b9c_a1d4_9e0b7c2f5a88);

/// 编码态样式：TF_ATTR_INPUT（正在输入/未转换）。文字/背景/线不加色
/// ——渲染方（CUAS/应用）按 INPUT 语义画下划线（虚线或实线）。
fn attr_style() -> TF_DISPLAYATTRIBUTE {
    TF_DISPLAYATTRIBUTE {
        crText: TF_DA_COLOR::default(),
        crBk: TF_DA_COLOR::default(),
        lsStyle: TF_DA_LINESTYLE(0), // TF_LS_NONE：样式交由 bAttr 语义
        fBoldLine: BOOL::default(),
        crLine: TF_DA_COLOR::default(),
        bAttr: TF_ATTR_INPUT,
    }
}

/// 单条 attribute 信息对象。
#[implement(ITfDisplayAttributeInfo)]
pub struct HuFuAttrInfo;

impl ITfDisplayAttributeInfo_Impl for HuFuAttrInfo_Impl {
    fn GetGUID(&self) -> Result<GUID> {
        Ok(ATTR_GUID)
    }
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("HuFu 编码输入"))
    }
    fn GetAttributeInfo(&self, pda: *mut TF_DISPLAYATTRIBUTE) -> Result<()> {
        if pda.is_null() {
            return Err(windows::core::Error::from_hresult(E_POINTER));
        }
        unsafe { *pda = attr_style() };
        Ok(())
    }
    fn SetAttributeInfo(&self, _pda: *const TF_DISPLAYATTRIBUTE) -> Result<()> {
        // IME 拥有样式；宿主侧设置忽略（标准 IME 行为）
        Ok(())
    }
    fn Reset(&self) -> Result<()> {
        Ok(())
    }
}

/// 单元素枚举器（Clone/Next/Reset/Skip 标准语义）。
#[implement(IEnumTfDisplayAttributeInfo)]
pub struct HuFuAttrEnum {
    /// 已消费个数（0=未出，1=已出）
    pos: std::cell::Cell<u32>,
}

impl HuFuAttrEnum {
    pub fn new() -> HuFuAttrEnum {
        HuFuAttrEnum {
            pos: std::cell::Cell::new(0),
        }
    }
}

impl Default for HuFuAttrEnum {
    fn default() -> Self {
        Self::new()
    }
}

impl IEnumTfDisplayAttributeInfo_Impl for HuFuAttrEnum_Impl {
    fn Clone(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        let e = HuFuAttrEnum::new();
        e.pos.set(self.pos.get());
        Ok(e.into())
    }
    fn Next(
        &self,
        ulcount: u32,
        rginfo: *mut Option<ITfDisplayAttributeInfo>,
        pcfetched: *mut u32,
    ) -> Result<()> {
        let mut fetched = 0u32;
        if !rginfo.is_null() {
            for i in 0..ulcount as usize {
                if self.pos.get() >= 1 {
                    break;
                }
                let info: ITfDisplayAttributeInfo = HuFuAttrInfo.into();
                unsafe { *rginfo.add(i) = Some(info) };
                fetched += 1;
                self.pos.set(self.pos.get() + 1);
            }
        }
        if !pcfetched.is_null() {
            unsafe { *pcfetched = fetched };
        }
        Ok(())
    }
    fn Reset(&self) -> Result<()> {
        self.pos.set(0);
        Ok(())
    }
    fn Skip(&self, ulcount: u32) -> Result<()> {
        self.pos.set((self.pos.get() + ulcount).min(1));
        Ok(())
    }
}

/// TIP 侧 provider 实现（挂 crate::tsf::HuFuTs——其 #[implement] 列表
/// 已含 ITfDisplayAttributeProvider，trait impl 在此跨模块落位）。
impl ITfDisplayAttributeProvider_Impl for crate::tsf::HuFuTs_Impl {
    fn EnumDisplayAttributeInfo(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(HuFuAttrEnum::new().into())
    }
    fn GetDisplayAttributeInfo(&self, guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
        unsafe {
            if guid.is_null() || *guid != ATTR_GUID {
                return Err(windows::core::Error::from_hresult(E_INVALIDARG));
            }
        }
        Ok(HuFuAttrInfo.into())
    }
}

/// 组段 range 上标记编码态属性（EditSession 内调用：SetText 之后）。
/// atom 由 CategoryMgr 按 GUID 注册（进程内稳定）。失败仅记日志——
/// 下划线是增强显示，不阻塞组段主流程。
pub unsafe fn mark_range_input(ec: u32, ctx: &windows::Win32::UI::TextServices::ITfContext, range: &windows::Win32::UI::TextServices::ITfRange) {
    use windows::Win32::System::Com::CoCreateInstance;
    use windows::Win32::UI::TextServices::{
        CLSID_TF_CategoryMgr, GUID_PROP_ATTRIBUTE, ITfCategoryMgr, ITfProperty,
    };
    let run = || -> Result<()> {
        let prop: ITfProperty = unsafe { ctx.GetProperty(&GUID_PROP_ATTRIBUTE)? };
        let cat: ITfCategoryMgr =
            unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, windows::Win32::System::Com::CLSCTX_INPROC_SERVER)? };
        let atom = unsafe { cat.RegisterGUID(&ATTR_GUID)? };
        unsafe { prop.SetValue(ec, range, &windows::core::VARIANT::from(atom as i32))? };
        Ok(())
    };
    if let Err(e) = run() {
        crate::tsf::trace(&format!("dispattr: mark 失败 {e:?}"));
    }
}

/// 清组段 range 的编码态属性（上屏/断段前调用——防下划线残留到已
/// 提交文本）。属性本来不存在时 Clear 返回错误——吞掉。
pub unsafe fn unmark_range(ec: u32, ctx: &windows::Win32::UI::TextServices::ITfContext, range: &windows::Win32::UI::TextServices::ITfRange) {
    use windows::Win32::UI::TextServices::{GUID_PROP_ATTRIBUTE, ITfProperty};
    let run = || -> Result<()> {
        let prop: ITfProperty = unsafe { ctx.GetProperty(&GUID_PROP_ATTRIBUTE)? };
        let _ = unsafe { prop.Clear(ec, range) };
        Ok(())
    };
    let _ = run();
}

// Interface 引入仅为类型可用性（rust 格式化占位）
#[allow(unused_imports)]
use windows::Win32::UI::TextServices::ITfDisplayAttributeProvider as _ProviderIf;
const _: Option<_ProviderIf> = None;
