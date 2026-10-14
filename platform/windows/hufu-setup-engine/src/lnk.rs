//! 开始菜单 URL 型快捷方式（.lnk 指向 http://127.0.0.1:4390/）。
//! 血律对照：install.ps1 L498-508——WScript.Shell CreateShortcut：
//! TargetPath='http://127.0.0.1:4390/'、IconLocation=图标.ico、Save()。
//! URL 作 SetPath 合法（WScript.Shell 同款：路径槽直接放 http 地址）。
//! 失败只返回 false（PS 侧 try/catch 打「快捷方式失败，可忽略」继续）。

use std::path::Path;

use windows::core::{Interface, GUID, PCWSTR};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::IShellLinkW;

/// CLSID_ShellLink {00021401-0000-0000-C000-000000000046}（windows 0.58 未导出
/// 该常量，按 SDK 头文件 guid 手写；IShellLinkW 官方 coclass）。
const CLSID_SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_C000_000000000046);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_path(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// WScript.Shell CreateShortcut + TargetPath=url + IconLocation=icon + Save 的
/// COM 直译。STA 初始化；RPC_E_CHANGED_MODE（线程已被别的模型初始化，如 UI
/// 线程 MTA）时照用不退出——只有自己 init 成功才 CoUninitialize。
pub fn create_url_shortcut(lnk_path: &Path, url: &str, icon: &Path) -> bool {
    unsafe {
        let mut co_owned = false;
        // 0.58 的 CoInitializeEx 直接返回 HRESULT（非 Result）
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if hr == RPC_E_CHANGED_MODE {
            // 已被别的模型初始化（如 UI 线程 MTA），借宿继续、退出时不拆
        } else if hr.is_ok() {
            co_owned = true; // 含 S_FALSE（同模型重复 init）：配对退出
        } else {
            return false;
        }
        let ok = (|| -> windows::core::Result<()> {
            let link: IShellLinkW =
                CoCreateInstance(&CLSID_SHELL_LINK, None, CLSCTX_INPROC_SERVER)?;
            let url_w = wide(url);
            link.SetPath(PCWSTR::from_raw(url_w.as_ptr()))?;
            let icon_w = wide_path(icon);
            link.SetIconLocation(PCWSTR::from_raw(icon_w.as_ptr()), 0)?; // $lnk.IconLocation=$icon（纯路径→索引 0）
            let pf = link.cast::<IPersistFile>()?;
            let lnk_w = wide_path(lnk_path);
            pf.Save(PCWSTR::from_raw(lnk_w.as_ptr()), false)?;
            Ok(())
        })()
        .is_ok();
        if co_owned {
            CoUninitialize(); // 不拆别人的初始化（借宿模式不退）
        }
        ok
    }
}
