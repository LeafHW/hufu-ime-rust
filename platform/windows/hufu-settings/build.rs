use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    // OUT_DIR = target/<triple>/<profile>/build/<pkg>-<hash>/out
    // ancestors: [out, pkg-hash, build, profile, triple, target, ...]
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("unexpected OUT_DIR layout")
        .to_path_buf();

    let src = manifest_dir.join("vendor").join("WebView2Loader.dll");
    println!("cargo:rerun-if-changed={}", src.display());

    if !src.is_file() {
        println!(
            "cargo:warning=vendor/WebView2Loader.dll is missing; the host will fail to load WebView2"
        );
        return;
    }

    let dst = profile_dir.join("WebView2Loader.dll");
    match fs::copy(&src, &dst) {
        Ok(_) => println!("cargo:warning=copied WebView2Loader.dll -> {}", dst.display()),
        Err(err) => println!(
            "cargo:warning=failed to copy WebView2Loader.dll to {}: {err}",
            dst.display()
        ),
    }
}
