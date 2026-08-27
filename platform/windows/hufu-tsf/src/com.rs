//! COM 基础设施：类厂与注册。

use windows::Win32::Foundation::{HMODULE, WIN32_ERROR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IClassFactory, IClassFactory_Impl,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleExW};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::TextServices::{
    CLSID_TF_InputProcessorProfiles, ITfInputProcessorProfiles,
};
use windows_core::*;

/// 类厂（#[implement] 生成 IUnknown/IClassFactory vtable）。
#[implement(IClassFactory)]
pub struct HuFuClassFactory;

impl IClassFactory_Impl for HuFuClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Option<&IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        if let Some(_outer) = punkouter {
            return Err(Error::from(HRESULT(-2147221231))); // CLASS_E_NOAGGREGATION
        }
        if riid.is_null() || ppvobject.is_null() {
            return Err(Error::from(HRESULT(-2147467261))); // E_POINTER
        }
        let service: IUnknown = crate::tsf::HuFuTs::new().into();
        unsafe { service.query(riid, ppvobject).ok() }
    }

    fn LockServer(&self, _flock: windows::Win32::Foundation::BOOL) -> Result<()> {
        Ok(())
    }
}

const CLSID_STR: &str = "{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}";

/// 语言档案 GUID（CLSID 末位 +1 派生）。
pub const PROFILE_GUID: GUID = GUID::from_u128(0x8f5c2a11_3e77_4b9c_a1d4_9e0b7c2f5a88);
const PROFILE_GUID_STR: &str = "{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}";

/// 本 DLL 绝对路径（按地址反查模块）。
fn self_path() -> String {
    unsafe {
        let mut hmod = HMODULE::default();
        let flag_from_addr = 4u32; // GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
        let flag_keep_ref = 2u32; // GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT
        let ok = GetModuleHandleExW(
            flag_from_addr | flag_keep_ref,
            PCWSTR((self_path as *const ()).cast()),
            &mut hmod,
        );
        if ok.is_err() {
            return "hufu-tsf.dll".to_string();
        }
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(hmod, &mut buf);
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

fn reg_set(subkey: &str, name: Option<&str>, value: &str) -> WIN32_ERROR {
    unsafe {
        let mut hkey = HKEY::default();
        let sk: Vec<u16> = subkey.encode_utf16().chain([0]).collect();
        let r = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(sk.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        );
        if r != WIN32_ERROR(0) {
            return r;
        }
        let (_vn, ptr) = match name {
            Some(n) => {
                let v: Vec<u16> = n.encode_utf16().chain([0]).collect();
                let p = v.as_ptr();
                (v, p)
            }
            None => (Vec::new(), std::ptr::null()),
        };
        let val: Vec<u16> = value.encode_utf16().chain([0]).collect();
        let val_bytes: Vec<u8> = val.iter().flat_map(|c| c.to_le_bytes()).collect();
        let r = RegSetValueExW(
            hkey,
            PCWSTR(ptr),
            0,
            REG_SZ,
            Some(&val_bytes),
        );
        let _ = RegCloseKey(hkey);
        r
    }
}

fn reg_del_tree(subkey: &str) -> WIN32_ERROR {
    unsafe {
        let sk: Vec<u16> = subkey.encode_utf16().chain([0]).collect();
        RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(sk.as_ptr()))
    }
}

/// CLSID + TSF TIP 注册（HKCU，无需管理员）。
pub fn register_server() -> HRESULT {
    let clsid_key = format!(r"Software\Classes\CLSID\{CLSID_STR}");
    let ips_key = format!(r"Software\Classes\CLSID\{CLSID_STR}\InprocServer32");
    let hr = reg_set(&clsid_key, None, "HuFu TSF Service");
    if hr != WIN32_ERROR(0) {
        return HRESULT((hr.0 | 0x8007_0000) as i32);
    }
    let _ = reg_set(&ips_key, None, &self_path());
    let _ = reg_set(&ips_key, Some("ThreadingModel"), "Apartment");
    let tip = r"Software\Microsoft\CTF\TIP";
    let tip_root = format!(r"{tip}\{CLSID_STR}");
    let _ = reg_set(&tip_root, None, "HuFu 输入法");
    let _ = reg_set(&format!(r"{tip_root}\Description"), None, "HuFu 虎符输入法（虎码）");
    // 微拼/MS Sample 实测布局：Category 两层子键（纯存在性，值留空）
    const TFCAT_TIP_KEYBOARD: &str = "{533C5E0E-5AC0-4ABD-B6F1-251B82B7BE7D}";
    let _ = reg_set(
        &format!(r"{tip_root}\Category\Category\{TFCAT_TIP_KEYBOARD}\{CLSID_STR}"),
        None,
        "",
    );
    let _ = reg_set(
        &format!(r"{tip_root}\Category\Item\{CLSID_STR}\{TFCAT_TIP_KEYBOARD}"),
        None,
        "",
    );
    // 语言档案（msctf AddLanguageProfile 最终也写这里；预写便于 per-user 识别）
    let lp = format!(r"{tip_root}\LanguageProfile\0x00000804\{PROFILE_GUID_STR}");
    let _ = reg_set(&lp, None, "HuFu 虎符输入法");
    let _ = reg_set(&lp, Some("Enable"), "1");
    let _ = reg_set(&lp, Some("Icon"), "hufu-server.exe,0");
    HRESULT(0)
}

/// 语言档案 GUID（供安装器使用；DLL 内不再做 COM 调用——
/// 真实安装器模式：安装 EXE 在 COM 初始化后调 ITfInputProcessorProfiles）。
pub fn register_profile() {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let need_uninit = hr.is_ok();
        let res: Result<()> = (|| {
            let profiles: ITfInputProcessorProfiles =
                CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
            profiles.Register(&crate::CLSID_HUFU_TSF)?;
            let clsid = crate::CLSID_HUFU_TSF;
            let langid = 0x0804u16; // zh-CN
            let desc: Vec<u16> = "HuFu 虎符输入法".encode_utf16().collect();
            profiles.AddLanguageProfile(
                &clsid,
                langid,
                &PROFILE_GUID,
                &desc,
                &[],
                0,
            )?;
            profiles.EnableLanguageProfile(
                &clsid,
                langid,
                &PROFILE_GUID,
                windows::Win32::Foundation::BOOL(1),
            )?;
            Ok(())
        })();
        if let Err(e) = res {
            eprintln!("hufu-tsf: 语言档案注册失败 {e:?}");
        }
        if need_uninit {
            CoUninitialize();
        }
    }
}

pub fn unregister_server() -> HRESULT {
    let _ = reg_del_tree(&format!(r"Software\Classes\CLSID\{CLSID_STR}"));
    let _ = reg_del_tree(&format!(r"Software\Microsoft\CTF\TIP\{CLSID_STR}"));
    HRESULT(0)
}
