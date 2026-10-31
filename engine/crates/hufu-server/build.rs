//! Embed the built `settings-app/dist` into `hufu-server` so `/next` works
//! without Node on the release machine. The dist directory is committed
//! (`settings-app/dist`) and rebuilt before packaging.
//!
//! Generates `$OUT_DIR/next_assets.rs` with `NEXT_INDEX` + a lookup table of
//! `/assets/...` and public files. Missing dist is not a build error: `/next`
//! then returns 404 and the old `/` page keeps working.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}

fn collect(root: &Path, dir: &Path, code: &mut String) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, code);
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if rel == "index.html" {
            continue;
        }
        let route = format!("/{rel}");
        let abs = path.canonicalize().unwrap_or(path);
        println!("cargo:rerun-if-changed={}", abs.display());
        code.push_str(&format!(
            "    (r#\"{route}\"#, include_bytes!(r#\"{}\"#), \"{}\"),\n",
            abs.display(),
            content_type(&abs)
        ));
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let dist = manifest.join("../../../settings-app/dist");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    println!("cargo:rerun-if-changed={}", dist.display());

    let mut code = String::new();
    let index = dist.join("index.html");
    if index.is_file() {
        println!("cargo:rerun-if-changed={}", index.display());
        code.push_str(&format!(
            "pub const NEXT_INDEX: &str = include_str!(r#\"{}\"#);\n",
            index.display()
        ));
        code.push_str("pub static NEXT_ASSETS: &[(&str, &[u8], &'static str)] = &[\n");
        collect(&dist, &dist, &mut code);
        code.push_str("];\n");
    } else {
        code.push_str("pub const NEXT_INDEX: &str = \"\";\n");
        code.push_str("pub static NEXT_ASSETS: &[(&str, &[u8], &'static str)] = &[];\n");
    }

    let path = out.join("next_assets.rs");
    if let Err(err) = fs::write(&path, code) {
        panic!("failed to write {}: {err}", path.display());
    }
}
