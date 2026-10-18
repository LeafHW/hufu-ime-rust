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
use windows::Win32::Foundation::{BOOL, COLORREF, E_INVALIDARG, E_POINTER};
use windows::Win32::UI::TextServices::{
    ITfDisplayAttributeInfo, ITfDisplayAttributeInfo_Impl, ITfDisplayAttributeProvider_Impl,
    IEnumTfDisplayAttributeInfo, IEnumTfDisplayAttributeInfo_Impl, TF_ATTR_INPUT, TF_CT_COLORREF,
    TF_DA_COLOR, TF_DA_COLOR_0, TF_DA_LINESTYLE, TF_DISPLAYATTRIBUTE,
};
use windows_core::BSTR;

/// 编码态 display attribute GUID（CLSID 尾段 a10→a12 派生，稳定不复用）。
pub const ATTR_GUID: GUID = GUID::from_u128(0x8f5c2a12_3e77_4b9c_a1d4_9e0b7c2f5a88);

/// 【组字串颜色 2026-10-18 实时化】运行时可变：skin 载荷到达时由
/// note_preedit_color 更新（server 配置 appearance.preedit_color，
/// 设置页改色 → fp 变 → ≤2.5s 重拉 → 宿主每键重查显示属性 →
/// **下一按键即新色，无需重启应用**）。0 = 跟随应用（不带色）。
/// 改默认色只改这里的初始值；正式配置走设置页。
static PREEDIT_COLOR: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// "#RRGGBB" / "#RRGGBBAA" / "RRGGBB" → COLORREF 0x00BBGGRR（A 忽略
/// ——文字不做透明）。空/非法/长度不对 → 0（跟随应用）。
pub fn parse_hex_colorref(s: &str) -> u32 {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 && h.len() != 8 {
        return 0;
    }
    let hex = |c: u8| -> Option<u32> {
        match c {
            b'0'..=b'9' => Some((c - b'0') as u32),
            b'a'..=b'f' => Some((c - b'a' + 10) as u32),
            b'A'..=b'F' => Some((c - b'A' + 10) as u32),
            _ => None,
        }
    };
    let b = h.as_bytes();
    let mut v = [0u32; 6];
    for i in 0..6 {
        v[i] = match hex(b[i]) {
            Some(x) => x,
            None => return 0,
        };
    }
    (v[4] << 4 | v[5]) << 16 | (v[2] << 4 | v[3]) << 8 | (v[0] << 4 | v[1]) // 0x00BBGGRR
}

/// skin 载荷全量到达时调用（tsf.rs load_skin；same:true 微载荷不调
/// =颜色不变沿用）。字段缺省（老 server/老配置）→ 0 回跟随应用。
pub fn note_preedit_color(v: &serde_json::Value) {
    let s = v
        .pointer("/skin/preedit_color")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    PREEDIT_COLOR.store(parse_hex_colorref(s), std::sync::atomic::Ordering::Relaxed);
}

/// 编码态样式：TF_ATTR_INPUT（正在输入/未转换）+ 组字串色（文字与
/// 下划线同色；0=跟随应用不加色）。宿主每键重查——原子值变了下一键
/// 就换色。
fn attr_style() -> TF_DISPLAYATTRIBUTE {
    let c = PREEDIT_COLOR.load(std::sync::atomic::Ordering::Relaxed);
    let color = if c == 0 {
        TF_DA_COLOR::default()
    } else {
        TF_DA_COLOR {
            r#type: TF_CT_COLORREF,
            Anonymous: TF_DA_COLOR_0 { cr: COLORREF(c) },
        }
    };
    TF_DISPLAYATTRIBUTE {
        crText: color,
        crBk: TF_DA_COLOR::default(),
        // 【实线定稿 2026-10-09】实证（像素级）：链路全通（宿主每次
        // 按键都查询 provider、mark 零失败）而 TF_LS_NONE 时 XAML/CUAS
        // 宿主一根线都不画——宿主按 lsStyle 画线，「按 INPUT 语义自画」
        // 是想当然（首轮 QQ 失效同因）。TF_LS_SOLID=微拼同款观感。
        lsStyle: TF_DA_LINESTYLE(1), // TF_LS_SOLID：实线下划线
        fBoldLine: BOOL::default(),
        crLine: color,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 【组字串颜色 2026-10-18】hex→COLORREF（0x00BBGGRR）解析矩阵：
    /// hot pink #FF69B4（R=FF G=69 B=B4）→ 0x00B469FF；非法一律 0
    /// （跟随应用）。AA 尾容忍但忽略（文字不做透明）。
    #[test]
    fn parse_hex_colorref_matrix() {
        assert_eq!(parse_hex_colorref("#FF69B4"), 0x00B4_69FF, "hot pink 带井号");
        assert_eq!(parse_hex_colorref("ff69b4"), 0x00B4_69FF, "小写无井号");
        assert_eq!(parse_hex_colorref("#FF69B4FF"), 0x00B4_69FF, "8 位 AA 忽略");
        assert_eq!(parse_hex_colorref("#202022"), 0x00_22_20_20, "深灰：B=22 G=20 R=20");
        assert_eq!(parse_hex_colorref(""), 0, "空=跟随应用");
        assert_eq!(parse_hex_colorref("#12345"), 0, "长度错");
        assert_eq!(parse_hex_colorref("#GG69B4"), 0, "非 hex");
        assert_eq!(parse_hex_colorref(" #FF69B4 "), 0x00B4_69FF, "首尾空白容忍");
    }

    /// 载荷落地：/skin/preedit_color 缺省（老 server）→ 0；有值 → 生效。
    #[test]
    fn note_preedit_color_from_payload() {
        let mut v = serde_json::json!({"skin": {}});
        note_preedit_color(&v);
        assert_eq!(PREEDIT_COLOR.load(std::sync::atomic::Ordering::Relaxed), 0);
        v["skin"]["preedit_color"] = serde_json::json!("#FF69B4");
        note_preedit_color(&v);
        assert_eq!(PREEDIT_COLOR.load(std::sync::atomic::Ordering::Relaxed), 0x00B4_69FF);
        // 回跟随应用（设置页清空）
        v["skin"]["preedit_color"] = serde_json::json!("");
        note_preedit_color(&v);
        assert_eq!(PREEDIT_COLOR.load(std::sync::atomic::Ordering::Relaxed), 0);
    }
}
