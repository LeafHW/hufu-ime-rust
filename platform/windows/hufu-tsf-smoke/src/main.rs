//! hufu-tsf.dll 冒烟测试：
//! [COM 层] LoadLibrary → DllRegisterServer(含语言档案) → DllGetClassObject → CreateInstance
//! [真实激活] ITfInputProcessorProfileMgr::ActivateProfile → msctf 从注册表加载本 DLL
//!            → 我们的 Activate 落标记文件 → TestKeyDown 全链路
//! [引擎链] hufu_test_key 直驱（VK → 管道 → hufu-server 引擎 → consumed）

use windows::core::*;
use windows::Win32::Foundation::{HMODULE, BOOL, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER, IClassFactory};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::TextServices::*;

const CLSID_HUFU: GUID = GUID::from_u128(0x8f5c2a10_3e77_4b9c_a1d4_9e0b7c2f5a88);
const PROFILE_GUID: GUID = GUID::from_u128(0x8f5c2a11_3e77_4b9c_a1d4_9e0b7c2f5a88);

type DllGetClassObjectFn =
    unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut core::ffi::c_void) -> HRESULT;
type DllRegisterServerFn = unsafe extern "system" fn() -> HRESULT;
type TestKeyFn = unsafe extern "system" fn(u32) -> i32;

fn main() {
    // 提权注册模式：hufu-tsf-smoke.exe reg —— 只做 msctf 语言档案注册
    // （ITfInputProcessorProfiles::Register/AddLanguageProfile 写 HKLM，须管理员）
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("reg") {
        unsafe {
            let _ = OleInitialize(None);
            let profiles: ITfInputProcessorProfiles =
                CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)
                    .expect("msctf profiles 不可用");
            let code = |r: windows::core::Result<()>| -> u32 {
                match r {
                    Ok(()) => 0,
                    Err(e) => e.code().0 as u32,
                }
            };
            let hr = code(profiles.Register(&CLSID_HUFU));
            println!("Register → 0x{hr:08X}");
            // 全局分类注册（caps 来源；切换器/系统枚举依赖）：
            // TFCAT_TIP_KEYBOARD + 键盘汇编分类 {34745C63}
            let cat: windows::core::Result<ITfCategoryMgr> =
                CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER);
            match cat {
                Ok(cat) => {
                    const TFCAT_TIP_KEYBOARD: GUID =
                        GUID::from_u128(0x533c5e0e_5ac0_4abd_b6f1_251b82b7be7d);
                    const TFCAT_ASM_KBD: GUID =
                        GUID::from_u128(0x34745c63_b2f0_4784_8b67_5e12c8701a31);
                    let c1 = code(cat.RegisterCategory(&CLSID_HUFU, &TFCAT_TIP_KEYBOARD, &CLSID_HUFU));
                    let c2 = code(cat.RegisterCategory(&CLSID_HUFU, &TFCAT_ASM_KBD, &CLSID_HUFU));
                    println!("RegisterCategory(kbd) → 0x{c1:08X} (asm) → 0x{c2:08X}");
                }
                Err(e) => println!("CategoryMgr 不可用：{e:?}"),
            }
            // 描述必须 NUL 终止（LPCWSTR），否则 msctf 越界读（注册表出现乱码尾巴）
            let desc: Vec<u16> = "HuFu 虎符输入法".encode_utf16().chain([0]).collect();
            let hr2 = code(profiles.AddLanguageProfile(
                &CLSID_HUFU,
                0x0804,
                &PROFILE_GUID,
                &desc,
                &[],
                0,
            ));
            println!("AddLanguageProfile → 0x{hr2:08X}");
            let hr3 = code(profiles.EnableLanguageProfile(
                &CLSID_HUFU,
                0x0804,
                &PROFILE_GUID,
                BOOL(1),
            ));
            println!("EnableLanguageProfile → 0x{hr3:08X}");
            if hr == 0 && hr2 == 0 && hr3 == 0 {
                println!("✓ msctf 档案注册完成");
            }
        }
        return;
    }
    let dll = r"E:\DSH-KF\hufu\platform\windows\target\release\hufu_tsf.dll";
    let wide: Vec<u16> = dll.encode_utf16().chain([0]).collect();
    unsafe {
        let hmod: HMODULE = LoadLibraryW(PCWSTR(wide.as_ptr())).unwrap();
        println!("[1] LoadLibrary ✓");

        let reg: DllRegisterServerFn =
            std::mem::transmute(GetProcAddress(hmod, PCSTR(b"DllRegisterServer\0".as_ptr())).unwrap());
        let hr = reg();
        assert_eq!(hr.0, 0, "DllRegisterServer 失败: 0x{:08X}", hr.0 as u32);
        println!("[2] DllRegisterServer ✓（HKCU CLSID/CTF\\TIP + msctf 语言档案）");

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

        // ── 语言档案注册（安装器职责，在此直测以定位失败点）──
        let marker = std::env::temp_dir().join("hufu-tsf-activated.txt");
        let _ = std::fs::remove_file(&marker);
        {
            // msctf 能否按注册表 CoCreateInstance 我们的 TIP（Register 内部验证路径）
            let direct: windows::core::Result<ITfTextInputProcessor> =
                CoCreateInstance(&CLSID_HUFU, None, CLSCTX_INPROC_SERVER);
            match &direct {
                Ok(_) => println!("    CoCreateInstance(CLSID_HUFU) 直连 ✓"),
                Err(e) => println!("    CoCreateInstance(CLSID_HUFU) 直连失败：{e:?}"),
            }

            let profiles: ITfInputProcessorProfiles =
                CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)
                    .unwrap();
            // 分类注册（ITfCategoryMgr）
            let cat: windows::core::Result<ITfCategoryMgr> =
                CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER);
            if let Ok(cat) = &cat {
                const TFCAT_TIP_KEYBOARD: GUID =
                    GUID::from_u128(0x533c5e0e_5ac0_4abd_b6f1_251b82b7be7d);
                let r = unsafe { cat.RegisterCategory(&CLSID_HUFU, &TFCAT_TIP_KEYBOARD, &CLSID_HUFU) };
                println!("    ITfCategoryMgr::RegisterCategory → {r:?}");
            }
            match unsafe {
                profiles
                    .Register(&CLSID_HUFU)
                    .and_then(|()| {
                        let desc: Vec<u16> = "HuFu 虎符输入法".encode_utf16().collect();
                        profiles.AddLanguageProfile(
                            &CLSID_HUFU,
                            0x0804,
                            &PROFILE_GUID,
                            &desc,
                            &[],
                            0,
                        )
                    })
                    .and_then(|()| {
                        profiles.EnableLanguageProfile(
                            &CLSID_HUFU,
                            0x0804,
                            &PROFILE_GUID,
                            BOOL(1),
                        )
                    })
            } {
                Ok(()) => println!("[6] 语言档案 Register+AddLanguageProfile+Enable ✓"),
                Err(e) => {
                    println!("[6] 语言档案注册失败：{e:?}");
                    println!("    （继续尝试 ActivateProfile，看 msctf 是否已按 CTF\\TIP 键识别）");
                }
            }

            let mgr: ITfInputProcessorProfileMgr = profiles.cast().unwrap();

            // 探测：msctf 能否枚举到我们（HKCU 键是否被读取）
            if let Ok(enum_) = unsafe { mgr.EnumProfiles(0x0804) } {
                let mut seen_hufu = false;
                let mut n = 0;
                loop {
                    let mut profs = [TF_INPUTPROCESSORPROFILE::default(); 4];
                    let mut fetched: u32 = 0;
                    if unsafe { enum_.Next(&mut profs, &mut fetched) }.is_err() || fetched == 0 {
                        break;
                    }
                    for prof in &profs[..fetched as usize] {
                        n += 1;
                        let clsid = format!("{:?}", prof.clsid);
                        let tag = if clsid.to_lowercase().contains("8f5c2a10") {
                            seen_hufu = true;
                            " <-- ours"
                        } else {
                            ""
                        };
                        println!(
                            "    [#{n}] {}{} type={} flags={:#x} caps={:#x} langid={:#x}",
                            &clsid[..clsid.len().min(8)],
                            tag,
                            prof.dwProfileType,
                            prof.dwFlags,
                            prof.dwCaps,
                            prof.langid
                        );
                    }
                }
                if !seen_hufu {
                    println!("    EnumProfiles 共 {n} 项，未含我们的 TIP（HKCU 键未被 msctf 枚举）");
                }
            }

            const TF_PROFILETYPE_INPUTPROCESSOR: u32 = 1;
            let ap = mgr.ActivateProfile(
                TF_PROFILETYPE_INPUTPROCESSOR,
                0x0804,
                &CLSID_HUFU,
                &PROFILE_GUID,
                HKL(std::ptr::null_mut()),
                0,
            );
            match ap {
                Ok(()) => println!("[7] ITfInputProcessorProfileMgr::ActivateProfile ✓"),
                Err(e) => println!("[7] ActivateProfile 失败：{e:?}"),
            }

            if marker.exists() {
                let content = std::fs::read_to_string(&marker).unwrap_or_default();
                println!("[8] 激活标记 ✓（msctf 真实管线加载本 DLL 并调用了 Activate）：{content}");
            } else {
                println!("[8] ⚠ 激活标记未出现（Activate 未被 msctf 调用）");
            }

            // ThreadMgr 激活为客户端，再试全链路按键
            let _tid: u32 = tm.Activate().unwrap();
            let km: ITfKeystrokeMgr = tm.cast().unwrap();
            let eaten = km.TestKeyDown(WPARAM(0x55), LPARAM(1)).unwrap();
            println!("[9] msctf TestKeyDown('u') consumed={} （若 sink 已激活即全链路通）", eaten.as_bool());
            let _ = tm.Deactivate();
        }

        // ── 引擎链直驱：hufu_test_key VK→管道→hufu-server→consumed ──
        let tk: TestKeyFn =
            std::mem::transmute(GetProcAddress(hmod, PCSTR(b"hufu_test_key\0".as_ptr())).unwrap());

        let _ = tk(0x1B);
        let _ = tk(0x1B);
        let u = tk(0x55); // 'u'
        assert_eq!(u, 1, "test_key('u') 应被引擎吃掉");
        println!("[10] hufu_test_key('u') = {u} ✓（DLL→管道→引擎→响应）");

        for vk in [0x4Au32, 0x4B, 0x4C, 0x4D] {
            let r = tk(vk);
            print!("    0x{vk:X}→{r}  ");
        }
        println!("✓");
        let sp = tk(0x20);
        println!("[11] hufu_test_key(space) = {sp}");

        // ── 标点链：空组段直提（Op::Commit 无组段回退）──
        // OEM 逗号 0xBC / 句号 0xBE：TestKeyDown 应预判消费、KeyDown 后引擎提交 ，。
        let c1 = tk(0xBC);
        let p1 = tk(0xBC);
        println!("[11.p1] 标点逗号 test={c1} down={p1}（应 1/1）");
        let p2 = tk(0xBE);
        println!("[11.p2] 标点句号 down={p2}（应 1）");

        // ── 候选窗 v2：D3D11+DComp+D2D+Acrylic accent 四材质各渲染一帧 ──
        type TestCandFn = unsafe extern "system" fn(u32) -> i32;
        let tc: TestCandFn = std::mem::transmute(
            GetProcAddress(hmod, PCSTR(b"hufu_test_candwin2\0".as_ptr())).unwrap(),
        );
        for mode in 0..4u32 {
            let r = unsafe { tc(mode) };
            let name = ["solid", "translucent", "frosted", "glass"][(mode % 4) as usize];
            assert_eq!(r, 1, "candwin2({name}) 渲染应成功");
            println!("[12.{mode}] candwin2 {name} ✓");
        }

        println!("\n=== hufu-tsf 冒烟测试通过 ===");
    }
}
