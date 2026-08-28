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
    let obj = manifest.join("assets").join("hufu_rsrc.o");
    assert!(obj.exists(), "缺少 assets/hufu_rsrc.o（图标资源目标文件）");
    println!("cargo:rustc-link-arg={}", obj.to_string_lossy());
    println!("cargo:rerun-if-changed=assets/hufu_rsrc.o");
}
