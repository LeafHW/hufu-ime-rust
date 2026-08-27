//! COM 基础设施：类厂与注册。

use windows::Win32::Foundation::WIN32_ERROR;
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::System::Registry::*;
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
const SERVER_PATH: &str = "hufu-tsf.dll";

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
    let _ = reg_set(&ips_key, None, SERVER_PATH);
    let _ = reg_set(&ips_key, Some("ThreadingModel"), "Apartment");
    let tip = r"Software\Microsoft\CTF\TIP";
    let _ = reg_set(&format!(r"{tip}\{CLSID_STR}"), None, "HuFu 输入法");
    let _ = reg_set(&format!(r"{tip}\{CLSID_STR}\Description"), None, "HuFu 虎符输入法（虎码）");
    let _ = reg_set(
        &format!(r"{tip}\{CLSID_STR}\Category"),
        Some("Item"),
        "{533C5E0E-5AC0-4ABD-B6F1-251B82B7BE7D}", // GUID_TFCAT_TIP_KEYBOARD
    );
    let _ = reg_set(
        &format!(r"{tip}\{CLSID_STR}\Profiles\{CLSID_STR}"),
        Some("Description"),
        "HuFu 虎符输入法",
    );
    HRESULT(0)
}

pub fn unregister_server() -> HRESULT {
    let _ = reg_del_tree(&format!(r"Software\Classes\CLSID\{CLSID_STR}"));
    let _ = reg_del_tree(&format!(r"Software\Microsoft\CTF\TIP\{CLSID_STR}"));
    HRESULT(0)
}
