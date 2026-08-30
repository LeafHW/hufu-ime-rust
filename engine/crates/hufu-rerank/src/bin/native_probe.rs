//! 虎爪 llama.cpp 原生打分器 FFI 探针（运行时 LoadLibrary 动态加载）。
//! 用法：native-probe <Native.dll 完整路径> <model.gguf> <cand1> <cand2> [...]

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};

type CreateFn = unsafe extern "C" fn(*const u8, *mut *mut c_void) -> i32;
type ScoreFn =
    unsafe extern "C" fn(*mut c_void, *const *const u8, i32, *mut f64) -> i32;
type DestroyFn = unsafe extern "C" fn(*mut c_void);

fn cstr(s: &str) -> Vec<u8> {
    s.as_bytes().iter().copied().chain([0u8]).collect()
}

fn sym<T>(h: HMODULE, name: &[u8]) -> Option<T> {
    let n = cstr(std::str::from_utf8(name).ok()?);
    let p = unsafe { GetProcAddress(h, n.as_ptr() as *const u8) };
    if p.is_none() {
        return None;
    }
    Some(unsafe { std::mem::transmute_copy(&p) })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("用法: native-probe <Native.dll> <model.gguf> <cand1> <cand2> [...]");
        std::process::exit(1);
    }
    let dll_path = &args[1];
    let model = &args[2];
    let cands = &args[3..];
    unsafe {
        let d = cstr(dll_path);
        let h = LoadLibraryA(d.as_ptr());
        if h.is_null() {
            eprintln!("LoadLibrary 失败: {}", std::io::Error::last_os_error());
            std::process::exit(2);
        }
        let create: CreateFn = sym(h, b"tcs_create_from_file").expect("无 tcs_create_from_file");
        let score: ScoreFn = sym(h, b"tcs_score").expect("无 tcs_score");
        let destroy: DestroyFn = sym(h, b"tcs_destroy").expect("无 tcs_destroy");

        let m = cstr(model);
        let mut scorer: *mut c_void = std::ptr::null_mut();
        let t0 = std::time::Instant::now();
        let r = create(m.as_ptr(), &mut scorer);
        println!("create → rc={r} {}ms", t0.elapsed().as_millis());
        if r != 0 || scorer.is_null() {
            eprintln!("加载失败");
            std::process::exit(3);
        }
        // 两轮：预热轮 + 计时轮
        for round in 0..2 {
            let owned: Vec<Vec<u8>> = cands.iter().map(|c| cstr(c)).collect();
            let ptrs: Vec<*const u8> = owned.iter().map(|v| v.as_ptr()).collect();
            let mut scores = vec![0f64; cands.len()];
            let t1 = std::time::Instant::now();
            let rc = score(scorer, ptrs.as_ptr(), cands.len() as i32, scores.as_mut_ptr());
            let el = t1.elapsed().as_millis();
            if round == 1 {
                println!("score → rc={rc} {el}ms/{}候选（第 2 轮，热）", cands.len());
                for (c, s) in cands.iter().zip(&scores) {
                    println!("  {c} = {s:.6}");
                }
            } else {
                println!("预热轮 rc={rc} {el}ms");
            }
        }
        destroy(scorer);
        FreeLibrary(h);
    }
}
