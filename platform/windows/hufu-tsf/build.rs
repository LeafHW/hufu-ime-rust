//! 嵌入虎爪图标资源（Win+Space 输入法切换器 / 语言栏从 DLL 资源取图标，
//! 注册表 LanguageProfile\...\Icon = "<DLL全路径>,0"）。
//! 工具链 windows-gnu 且无 windres：assets/hufu_rsrc.o 是脚本预生成的
//! COFF 目标文件（单 .rsrc 段 + 段符号 + ADDR32 重定位，与 windres 产物同构），
//! 直接经 rustc-link-arg 交给 ld 链入。

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    // 按目标架构选资源 COFF：x64 用 windres 同构产物；i686（32 位
    // 宿主支持，Pain 打器等）用 hufu_rsrc32.o——由 x64 版字节转换而得
    // （machine 0x8664→0x14C、重定位 AMD64_ADDR64NB(3)→I386_DIR32NB(7)；
    // .rsrc 图标目录树与 DIB 数据本身架构无关）。
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let name = if arch == "x86" { "hufu_rsrc32.o" } else { "hufu_rsrc.o" };
    let obj = manifest.join("assets").join(name);
    assert!(obj.exists(), "缺少 assets/{name}（图标资源目标文件）");
    println!("cargo:rustc-link-arg={}", obj.to_string_lossy());
    println!("cargo:rerun-if-changed=assets/hufu_rsrc.o");
    println!("cargo:rerun-if-changed=assets/hufu_rsrc32.o");
}
