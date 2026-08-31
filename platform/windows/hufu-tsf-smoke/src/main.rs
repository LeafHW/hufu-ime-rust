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
    let args: Vec<String> = std::env::args().collect();
    // 提权注销模式：hufu-tsf-smoke.exe unreg —— 卸载器专用，
    // 从 msctf 原生库移除语言档案（注册表项由卸载脚本删）。
    if args.get(1).map(|s| s.as_str()) == Some("unreg") {
        unsafe {
            let _ = OleInitialize(None);
            let mgr: ITfInputProcessorProfileMgr =
                CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)
                    .expect("msctf profiles 不可用");
            match unsafe { mgr.UnregisterProfile(&CLSID_HUFU, 0x0804, &PROFILE_GUID, 0) } {
                Ok(()) => println!("✓ msctf 档案已注销"),
                Err(e) => println!("注销返回：{e:?}（未登记也算成功）"),
            }
        }
        return;
    }
    // 提权注册模式：hufu-tsf-smoke.exe reg [图标路径] —— 安装器专用，
    // 只做 msctf 注册（RegisterProfile 带图标 + 分类 + 激活）。须管理员；
    // 图标路径默认 DLL 自身（内嵌虎符资源）。
    if args.get(1).map(|s| s.as_str()) == Some("reg") {
        let icon_path = args
            .get(2)
            .cloned()
            .or_else(|| std::env::var("HUFU_ICON_FILE").ok())
            .unwrap_or_else(|| {
                "E:\\DSH-KF\\hufu\\platform\\windows\\target\\release\\hufu_tsf.dll".into()
            });
        unsafe {
            let _ = OleInitialize(None);
            let mgr: ITfInputProcessorProfileMgr =
                CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)
                    .expect("msctf profiles 不可用");
            let code = |r: windows::core::Result<()>| -> u32 {
                match r {
                    Ok(()) => 0,
                    Err(e) => e.code().0 as u32,
                }
            };
            let hr = code(mgr.RegisterProfile(
                &CLSID_HUFU,
                0x0804,
                &PROFILE_GUID,
                &"HuFu 虎符输入法".encode_utf16().collect::<Vec<_>>(),
                &icon_path.encode_utf16().collect::<Vec<_>>(),
                0,
                HKL(std::ptr::null_mut()),
                0,
                BOOL(1),
                0,
            ));
            println!("RegisterProfile(带图标 {icon_path}) → 0x{hr:08X}");
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
            const TF_PROFILETYPE_INPUTPROCESSOR: u32 = 1;
            match mgr.ActivateProfile(
                TF_PROFILETYPE_INPUTPROCESSOR,
                0x0804,
                &CLSID_HUFU,
                &PROFILE_GUID,
                HKL(std::ptr::null_mut()),
                0,
            ) {
                Ok(()) => println!("✓ 激活成功，安装注册完成"),
                Err(e) => {
                    println!("✗ ActivateProfile 失败：{e:?}");
                    std::process::exit(1);
                }
            }
        }
        return;
    }
    let dll = std::env::var("HUFU_TSF_DLL").unwrap_or_else(|_| {
        // 优先 smoke exe 同目录的 DLL（安装态），否则开发构建兜底。
        // （旧版直接取工程绝对路径，安装机上会误注册/误测开发 DLL。）
        let beside = std::env::current_exe()
            .ok()
            .and_then(|p| {
                let d = p.parent()?.join("hufu_tsf.dll");
                d.exists().then_some(d)
            })
            .map(|p| p.to_string_lossy().into_owned());
        beside.unwrap_or_else(|| {
            r"E:\DSH-KF\hufu\platform\windows\target\release\hufu_tsf.dll".into()
        })
    });
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
                // 真正的 TFCAT_TIP_KEYBOARD 是 34745C63（此前误用 533C5E0E）
                const TFCAT_TIP_KEYBOARD: GUID =
                    GUID::from_u128(0x34745c63_b2f0_4784_8b67_5e12c8701a31);
                let r = unsafe { cat.RegisterCategory(&CLSID_HUFU, &TFCAT_TIP_KEYBOARD, &CLSID_HUFU) };
                println!("    ITfCategoryMgr::RegisterCategory → {r:?}");
            }
            match unsafe {
                profiles
                    .Register(&CLSID_HUFU)
                    .and_then(|()| {
                        let desc: Vec<u16> = "HuFu 虎符输入法".encode_utf16().collect();
            // 图标随档案登记进 msctf 原生库（浮层只读这里，
            // 注册表 IconFile/IconIndex 对浮层无效——25H2 实测）。
            // 可用环境变量 HUFU_ICON_FILE 换图标文件做鉴别实验。
                        let icon_path = std::env::var("HUFU_ICON_FILE").unwrap_or_else(|_| {
                            "E:\\DSH-KF\\hufu\\platform\\windows\\target\\release\\hufu_tsf.dll".into()
                        });
                        let icon: Vec<u16> = icon_path
                            .encode_utf16()
                            .chain([0])
                            .collect();
                        profiles.AddLanguageProfile(
                            &CLSID_HUFU,
                            0x0804,
                            &PROFILE_GUID,
                            &desc,
                            &icon,
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
            let mut ours_prof: Option<TF_INPUTPROCESSORPROFILE> = None;
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
                        if !tag.is_empty() {
                            ours_prof = Some(*prof);
                        }
                    }
                }
                if !seen_hufu {
                    println!("    EnumProfiles 共 {n} 项，未含我们的 TIP（HKCU 键未被 msctf 枚举）");
                }
            }

            // 重登记：档案已存在时 AddLanguageProfile 的图标参数会被静默忽略，
            // 浮层只读 msctf 原生库 → 先卸载 → 带图标重注册（AddLanguageProfile
            // 的 pchIconFile/uIconIndex 参数即 msctf 原生库的图标来源）。
            if ours_prof.is_some() {
                let icon_path = std::env::var("HUFU_ICON_FILE").unwrap_or_else(|_| {
                    "E:\\DSH-KF\\hufu\\platform\\windows\\target\\release\\hufu_tsf.dll".into()
                });
                let icon: Vec<u16> = icon_path.encode_utf16().chain([0]).collect();
                let desc: Vec<u16> = "HuFu 虎符输入法".encode_utf16().collect();
                let _ = unsafe { mgr.UnregisterProfile(&CLSID_HUFU, 0x0804, &PROFILE_GUID, 0) };
                match unsafe {
                    mgr.RegisterProfile(
                        &CLSID_HUFU,
                        0x0804,
                        &PROFILE_GUID,
                        &desc,
                        &icon,
                        0,
                        HKL(std::ptr::null_mut()),
                        0,
                        BOOL(1),
                        0,
                    )
                } {
                    Ok(()) => println!("    卸载→RegisterProfile(带图标 {icon_path}) ✓"),
                    Err(e) => println!("    卸载→RegisterProfile 失败：{e:?}"),
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
        // 前置重置：真实应用的 Shift 会把全局会话切成英文态污染断言
        type ResetFn = unsafe extern "system" fn() -> i32;
        if let Some(p) = GetProcAddress(hmod, PCSTR(b"hufu_test_reset\0".as_ptr())) {
            let rst: ResetFn = std::mem::transmute(p);
            let r = unsafe { rst() };
            println!("[pre] hufu_test_reset = {r}（会话归零回中文态）");
        }

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

        // ── 音效池化连打：16 连击（4 句柄排队深度压力）不得崩/死锁 ──
        type SndBurstFn = unsafe extern "system" fn() -> i32;
        let sb: SndBurstFn = std::mem::transmute(
            GetProcAddress(hmod, PCSTR(b"hufu_test_sound_burst\0".as_ptr())).unwrap(),
        );
        let r = unsafe { sb() };
        assert_eq!(r, 1, "音效连打应全部顺利完成");
        println!("[14] 音效池化连打（含排队压力）✓");

        // ── 管道键往返微基准（ipc 改动的量化回归点）──
        type KeyBurstFn = unsafe extern "system" fn(u32) -> i32;
        if let Some(p) = GetProcAddress(hmod, PCSTR(b"hufu_test_key_burst\0".as_ptr())) {
            let kb: KeyBurstFn = std::mem::transmute(p);
            let _ = unsafe { kb(200) };
        }

        // ── 皮肤热更新 E2E：同窗两帧不同皮肤，屏幕像素必须显著变化 ──
        // （刻意排在音效之后：曾因 UAF 在音频线程收尾时崩 D3D 初始化，作回归哨兵）
        type SkinHotFn = unsafe extern "system" fn() -> i32;
        let sh: SkinHotFn = std::mem::transmute(
            GetProcAddress(hmod, PCSTR(b"hufu_test_skin_hot\0".as_ptr())).unwrap(),
        );
        let r = unsafe { sh() };
        assert_eq!(r, 1, "皮肤 A/B 两帧像素应显著不同（热更新失效）");
        println!("[13] 皮肤热更新像素 E2E ✓");

        // ── [16] 当前皮肤（crystal 横排）内边距视觉落盘 ──
        type PadDumpFn = unsafe extern "system" fn() -> i32;
        if let Some(p) = GetProcAddress(hmod, PCSTR(b"hufu_test_pad_dump\0".as_ptr())) {
            let pd: PadDumpFn = std::mem::transmute(p);
            let r = unsafe { pd() };
            println!("[16] 当前皮肤内边距落盘 = {r}（%TEMP%\\hufu-pad.bmp）");
        }

        // ── [15] 横排真路径 E2E：设置皮肤(毛玻璃横排)→真实按键→窗口必须变宽扁 ──
        {
            use std::io::{Read, Write};
            use std::net::TcpStream;
            let http = |method: &str, path: &str, body: &str| -> String {
                let mut s = TcpStream::connect("127.0.0.1:4390").unwrap();
                let req = format!(
                    "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                s.write_all(req.as_bytes()).unwrap();
                let mut buf = String::new();
                s.read_to_string(&mut buf).unwrap();
                buf
            };
            let body_of = |resp: &str| -> String {
                resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string()
            };
            // 保存当前皮肤 JSON（稍后还原）
            let cur = body_of(&http("GET", "/api/skins", ""));
            let cur_id = cur
                .split("\"current\":\"")
                .nth(1)
                .and_then(|t| t.split('"').next())
                .unwrap_or("hufu-default")
                .to_string();
            let backup = body_of(&http("GET", &format!("/api/skin?id={cur_id}"), ""));
            // 切到「毛玻璃横排」预设（用户操作路径：选皮肤→保存）
            let frost = body_of(&http("GET", "/api/skin?id=hufu-frost-h", ""));
            let sv = http("POST", "/api/skin", &frost);
            assert!(sv.contains("\"ok\":true"), "皮肤切换保存失败: {sv}");
            // 真实渲染验证：pad_dump 按服务器**当前皮肤**真实渲染并落盘 BMP，
            // 量 BMP 头的宽高——这才是「切换后渲染尺寸」的真值。
            //（旧版用 test_key+FindWindow 量窗口，但 test_key 只走引擎管道
            // 从不渲染，量到的实为上一测试残留窗口——current 恰为横排时
            // 假通过，默认皮肤改竖排后现出原形。）
            let pd = {
                type P = unsafe extern "system" fn() -> i32;
                let p = GetProcAddress(hmod, PCSTR(b"hufu_test_pad_dump\0".as_ptr())).unwrap();
                unsafe { std::mem::transmute::<_, P>(p) }
            };
            let bmp_dims = || -> (i32, i32) {
                let mut b = std::fs::read(
                    std::env::temp_dir().join("hufu-pad.bmp"),
                )
                .expect("pad bmp 应已生成");
                let w = i32::from_le_bytes([b[18], b[19], b[20], b[21]]);
                let h = i32::from_le_bytes([b[22], b[23], b[24], b[25]]);
                b.clear();
                (w, h.abs())
            };
            assert_eq!(unsafe { pd() }, 1, "横排 pad_dump 渲染失败");
            let (w, h) = bmp_dims();
            // 先还原原皮肤再断言：断言失败也不能把当前皮肤留在横排预设上
            let _ = http("POST", "/api/skin", &backup);
            assert!(w > h, "横排应宽扁（{w}x{h}）——皮肤未生效");
            println!("[15] 横排真路径：切换皮肤→真实渲染 → {w}x{h} ✓（已还原 {cur_id}）");
        }

        println!("\n=== hufu-tsf 冒烟测试通过 ===");
    }
}
