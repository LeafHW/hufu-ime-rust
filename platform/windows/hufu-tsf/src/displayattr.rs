//! 【组段下划线·CUAS 支持 2026-09-14 / 类别注册补课 2026-10-09】
//! TSF display attribute 提供方。
//!
//! 用户反馈：64 位 TSF-aware 应用（QQ/记事本等）编码显示在应用内且
//! CUAS 默认画下划线；32 位应用（跟打器 Pain）/UWP/开始菜单（XAML/
//! CUAS 严格按属性渲染）没有线——这些宿主依赖 IME 的
//! ITfDisplayAttributeProvider + 组段 range 上的 GUID_PROP_ATTRIBUTE
//! 属性（VT_I4=attribute GUID 的 atom）。不提供 → 无从得知编码态样式
//! → 不画线。
//!
//! 【首轮失败的根因 2026-10-09】provider 实现了、属性也打了，但 msctf
//! 解析 GUID→TIP 走的是**类别管理器**：官方文档（Providing Display
//! Attributes）第一步就是
//!   ITfCategoryMgr::RegisterCategory(TIP CLSID,
//!       GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER, TIP CLSID)
//! ——此前从来没注册过这个类别，msctf 找不到 provider 拥有者 → 宿主
//! 拿不到样式 → 连默认下划线都不画（QQ 一并失效的机制）。本机微拼
//! 无 DisplayAttribute 注册表键也有线，旁证机制在类别而非注册表键。
//!
//! 三件套（齐了）：
//! 1. 类别注册：DllRegisterServer（com.rs，装机提权侧）+ TIP Activate
//!    （运行时每进程兜底，幂等）双路 RegisterCategory。
//! 2. ITfDisplayAttributeProvider 挂在 TIP（HuFuTs）上：枚出一个
//!    attribute（ATTR_GUID，TF_ATTR_INPUT + TF_LS_SOLID 实线下划线
//!    ——宿主按 lsStyle 画线，NONE=不画）。
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
        // 【实线定稿 2026-10-09】实证（像素级）：链路全通（宿主每次
        // 按键都查询 provider、mark 零失败）而 TF_LS_NONE 时 XAML/CUAS
        // 宿主一根线都不画——宿主按 lsStyle 画线，「按 INPUT 语义自画」
        // 是想当然（首轮 QQ 失效同因）。TF_LS_SOLID+默认线色（宿主用
        // 文字色）=微拼同款观感。crText/crBk 不动（不改字色/底色）。
        lsStyle: TF_DA_LINESTYLE(1), // TF_LS_SOLID：实线下划线
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
        // 【破案探针 2026-10-09】msctf 找到 provider 的直接证据：枚举被
        // 调一次记一次（diag 摘要，低频不刷屏）。
        crate::tsf::diag_note("dispattr: provider 枚举被查询");
        Ok(HuFuAttrEnum::new().into())
    }

    fn GetDisplayAttributeInfo(&self, guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
        unsafe {
            if guid.is_null() || *guid != ATTR_GUID {
                return Err(windows::core::Error::from_hresult(E_INVALIDARG));
            }
        }
        crate::tsf::diag_note("dispattr: provider 按GUID被查询");
        Ok(HuFuAttrInfo.into())
    }
}

/// 【类别注册补课 2026-10-09】RegisterCategory(TIP, DISPLAYATTRIBUTEPROVIDER,
/// TIP)——msctf 据此知道「谁拥有 display attribute provider」，宿主查询
/// GUID 时才能 CoCreate 到本 TIP。幂等；失败仅日志（增强显示不阻塞）。
/// DllRegisterServer（装机提权）与 TIP Activate（每进程兜底）都调。
pub fn register_provider_category() {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::TextServices::{CLSID_TF_CategoryMgr, ITfCategoryMgr};

    unsafe {
        let init_hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let need_uninit = init_hr.is_ok();
        let run = || -> Result<()> {
            let cat: ITfCategoryMgr =
                CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
            cat.RegisterCategory(
                &crate::CLSID_HUFU_TSF,
                &windows::Win32::UI::TextServices::GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
                &crate::CLSID_HUFU_TSF,
            )?;
            Ok(())
        };
        match run() {
            Ok(()) => crate::tsf::diag_note("dispattr: 类别注册 OK"),
            Err(e) => crate::tsf::diag_note(&format!("dispattr: 类别注册失败 {e:?}")),
        }
        if need_uninit {
            CoUninitialize();
        }
    }
}

/// 卸载侧对称清理（失败忽略——类别残留无害但求干净）。
pub fn unregister_provider_category() {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::TextServices::{CLSID_TF_CategoryMgr, ITfCategoryMgr};

    unsafe {
        let init_hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let need_uninit = init_hr.is_ok();
        let run = || -> Result<()> {
            let cat: ITfCategoryMgr =
                CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
            cat.UnregisterCategory(
                &crate::CLSID_HUFU_TSF,
                &windows::Win32::UI::TextServices::GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
                &crate::CLSID_HUFU_TSF,
            )?;
            Ok(())
        };
        let _ = run();
        if need_uninit {
            CoUninitialize();
        }
    }
}

/// 组段 range 上标记编码态属性（EditSession 内调用：SetText 之后）。
/// atom 由 CategoryMgr 按 GUID 注册（进程内稳定）。失败仅记日志——
/// 下划线是增强显示，不阻塞组段主流程。
pub unsafe fn mark_range_input(
    ec: u32,
    ctx: &windows::Win32::UI::TextServices::ITfContext,
    range: &windows::Win32::UI::TextServices::ITfRange,
) {
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
pub unsafe fn unmark_range(
    ec: u32,
    ctx: &windows::Win32::UI::TextServices::ITfContext,
    range: &windows::Win32::UI::TextServices::ITfRange,
) {
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
