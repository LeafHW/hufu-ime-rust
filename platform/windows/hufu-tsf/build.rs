//! 嵌入虎符图标资源（Win+Space 输入法切换器 / 语言栏从 DLL 资源取图标，
//! 注册表 LanguageProfile\...\Icon = "<DLL全路径>,0"）。
//! 工具链 windows-gnu 且无 windres：assets/hufu_rsrc.o 是脚本预生成的
//! COFF 目标文件（单 .rsrc 段 + 段符号 + ADDR32 重定位，与 windres 产物同构），
//! 直接经 rustc-link-arg 交给 ld 链入。

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rerun-if-env-changed=HUFU_TSF_REGISTRATION_DLL");
    if let Ok(name) = std::env::var("HUFU_TSF_REGISTRATION_DLL") {
        assert!(name == "hufu_tsf.dll", "registration facade must be hufu_tsf.dll");
        println!("cargo:rustc-env=HUFU_TSF_REGISTRATION_DLL={name}");
    }
    // MSVC consumes the architecture-independent .res format. Do not feed the
    // checked-in AMD64 COFF object to an ARM64 linker.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let rc = out.join("hufu.rc");
        let res = out.join("hufu.res");
        // Match the installer branding; assets/hufu.ico is a legacy white mark.
        let icon = manifest.join("../setup/icons/虎符.ico");
        std::fs::write(&rc, format!("1 ICON \"{}\"\n", icon.display().to_string().replace('\\', "\\\\"))).unwrap();
        let compiler = std::env::var_os("HUFU_RC").unwrap_or_else(|| "rc.exe".into());
        let status = std::process::Command::new(compiler)
            .arg("/nologo").arg("/c65001").arg("/fo").arg(&res).arg(&rc)
            .status().expect("Windows SDK rc.exe is required for MSVC resources");
        assert!(status.success(), "rc.exe failed: {status}");
        println!("cargo:rustc-link-arg={}", res.display());
        println!("cargo:rerun-if-changed=../setup/icons/虎符.ico");
        println!("cargo:rerun-if-env-changed=HUFU_RC");
        return;
    }
    // 按目标架构选资源 COFF：x64 用 windres 同构产物；i686（32 位
    // 宿主支持，Pain 打器等）用 hufu_rsrc32.o——由 x64 版字节转换而得
    // （machine 0x8664→0x14C、重定位 AMD64_ADDR64NB(3)→I386_DIR32NB(7)；
    // .rsrc 图标目录树与 DIB 数据本身架构无关）。
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let name = if arch == "x86" {
        "hufu_rsrc32.o"
    } else if arch == "x86_64" {
        "hufu_rsrc.o"
    } else {
        panic!("unsupported GNU resource architecture: {arch}")
    };
    let obj = manifest.join("assets").join(name);
    assert!(obj.exists(), "缺少 assets/{name}（图标资源目标文件）");
    println!("cargo:rustc-link-arg={}", obj.to_string_lossy());
    println!("cargo:rerun-if-changed=assets/hufu_rsrc.o");
    println!("cargo:rerun-if-changed=assets/hufu_rsrc32.o");
}
