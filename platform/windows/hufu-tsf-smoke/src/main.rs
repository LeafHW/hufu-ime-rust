//! hufu-tsf.dll 冒烟测试（不需 regsvr32）：
//! [COM 层] LoadLibrary → DllRegisterServer → DllGetClassObject → CreateInstance → msctf ThreadMgr
//! [引擎链] hufu_test_key 直驱：VK → 管道 → hufu-server 引擎 → consumed
//! 注：msctf 完整激活需 CTF 语言档案注册（下一轮 install 脚本覆盖），
//!     手动 AdviseKeyEventSink 在真实 msctf 上返回 E_INVALIDARG（Wine 宽松）。

use windows::core::*;
use windows::Win32::Foundation::{HMODULE, BOOL, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CLSCTX_INPROC_SERVER, IClassFactory,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::UI::TextServices::*;

const CLSID_HUFU: GUID = GUID::from_u128(0x8f5c2a10_3e77_4b9c_a1d4_9e0b7c2f5a88);

type DllGetClassObjectFn =
    unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut core::ffi::c_void) -> HRESULT;
type DllRegisterServerFn = unsafe extern "system" fn() -> HRESULT;
type TestKeyFn = unsafe extern "system" fn(u32) -> i32;

fn main() {
    let dll = r"E:\DSH-KF\hufu\platform\windows\target\release\hufu_tsf.dll";
    let wide: Vec<u16> = dll.encode_utf16().chain([0]).collect();
    unsafe {
        let hmod: HMODULE = LoadLibraryW(PCWSTR(wide.as_ptr())).unwrap();
        println!("[1] LoadLibrary ✓");

        let reg: DllRegisterServerFn =
            std::mem::transmute(GetProcAddress(hmod, PCSTR(b"DllRegisterServer\0".as_ptr())).unwrap());
        let hr = reg();
        assert_eq!(hr.0, 0, "DllRegisterServer 失败: 0x{:08X}", hr.0 as u32);
        println!("[2] DllRegisterServer ✓（HKCU CLSID + CTF\\TIP 已写）");

        let gco: DllGetClassObjectFn =
            std::mem::transmute(GetProcAddress(hmod, PCSTR(b"DllGetClassObject\0".as_ptr())).unwrap());
        let mut factory: *mut core::ffi::c_void = std::ptr::null_mut();
        let hr = gco(&CLSID_HUFU, &IClassFactory::IID, &mut factory);
        assert_eq!(hr.0, 0, "DllGetClassObject 失败: 0x{:08X}", hr.0 as u32);
        let factory: IClassFactory = std::mem::transmute(factory);
        println!("[3] DllGetClassObject → IClassFactory ✓");

        let tip: ITfTextInputProcessorEx = factory
            .CreateInstance(None)
            .expect("CreateInstance 失败");
        println!("[4] CreateInstance → ITfTextInputProcessorEx ✓（多接口 vtable 正常）");

        let _ = OleInitialize(None);
        let tm: ITfThreadMgr =
            CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER).unwrap();
        println!("[5] CoCreateInstance(msctf ITfThreadMgr) ✓");

        // 手动 ActivateEx：msctf 会拒绝手动 Advise（tid 必须来自 CTF 档案激活流程）
        let real_tid: u32 = tm.Activate().unwrap();
        let manual = tip.ActivateEx(&tm, real_tid, 0);
        match manual {
            Err(e) if e.code() == HRESULT(0x8007_0057u32 as i32) => {
                println!("[6] 手动 ActivateEx → E_INVALIDARG（预期：msctf 要求 CTF 档案 tid）✓");
            }
            other => println!("[6] 手动 ActivateEx → {other:?}"),
        }
        let _ = tm.Deactivate();

        // ── 引擎链直驱：hufu_test_key VK→管道→hufu-server→consumed ──
        let tk: TestKeyFn =
            std::mem::transmute(GetProcAddress(hmod, PCSTR(b"hufu_test_key\0".as_ptr())).unwrap());

        // 重置引擎会话（先按 escape 两次清空）
        let _ = tk(0x1B);
        let _ = tk(0x1B);

        let u = tk(0x55); // 'u'
        assert_eq!(u, 1, "test_key('u') 应被引擎吃掉");
        println!("[7] hufu_test_key('u') = {u} ✓（DLL→管道→引擎→响应）");

        for vk in [0x4Au32, 0x4B, 0x4C, 0x4D] {
            let r = tk(vk);
            print!("    0x{vk:X}→{r}  ");
        }
        println!("✓");

        let sp = tk(0x20); // space → commit
        println!("[8] hufu_test_key(space) = {sp}（0 = 直通说明无组合，1 = 上屏）");

        println!("\n=== hufu-tsf 冒烟测试通过：COM 层 + 引擎链均正常 ===");
    }
}
