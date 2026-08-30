//! 虎爪 llama.cpp 原生打分器动态加载（tcs_* FFI）。
//! 实测 81ms/2 候选（纯 Rust 1173ms 的 1/14）；判序三案全对
//! （句首/句中拼串/成语）。llama.cpp MIT + Qwen Apache，可分发。

use std::ffi::c_void;
use std::path::Path;

use windows_sys::Win32::Foundation::FreeLibrary;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

type CreateFn = unsafe extern "C" fn(*const u8, *mut *mut c_void) -> i32;
type ScoreFn = unsafe extern "C" fn(*mut c_void, *const *const u8, i32, *mut f64) -> i32;
type DestroyFn = unsafe extern "C" fn(*mut c_void);

pub struct NativeScorer {
    handle: *mut c_void,
    _module: windows_sys::Win32::Foundation::HMODULE,
    score_fn: ScoreFn,
    destroy_fn: DestroyFn,
    #[allow(dead_code)]
    model_path: std::path::PathBuf,
}

fn cstr(s: &str) -> Vec<u8> {
    s.as_bytes().iter().copied().chain([0u8]).collect()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0u16]).collect()
}

unsafe fn sym<T>(
    h: windows_sys::Win32::Foundation::HMODULE,
    name: &str,
) -> Option<T> {
    let n = cstr(name);
    let p = GetProcAddress(h, n.as_ptr());
    if p.is_none() {
        return None;
    }
    Some(std::mem::transmute_copy(&p))
}

impl NativeScorer {
    /// 尝试加载 dll（exe 同目录 / 指定目录）并建 scorer。
    /// 任一步失败返回 None（调用方回落纯 Rust 路径）。
    /// 注意必须 LoadLibraryW：安装路径含中文（ANSI A 版会把 UTF-8
    /// 字节按代码页误解 → 126 找不到模块）。
    pub fn try_new(extra_dirs: &[&Path], model: &Path) -> Option<NativeScorer> {
        let dll_name = "TigerClaw.Sentence.Native.dll";
        let mut candidates: Vec<std::path::PathBuf> = vec![];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() {
                candidates.push(d.join(dll_name));
            }
        }
        for d in extra_dirs {
            candidates.push(d.join(dll_name));
        }
        let path = candidates.into_iter().find(|p| p.exists())?;
        let pstr = path.to_string_lossy().into_owned();
        unsafe {
            let w = wide(&pstr);
            let h = LoadLibraryW(w.as_ptr());
            if h.is_null() {
                return None;
            }
            let create: CreateFn = sym(h, "tcs_create_from_file")?;
            let score_fn: ScoreFn = sym(h, "tcs_score")?;
            let destroy_fn: DestroyFn = sym(h, "tcs_destroy")?;
            // tcs_create_from_file 收 UTF-8（接口契约），路径含中文没问题
            let m = cstr(&model.to_string_lossy());
            let mut handle: *mut c_void = std::ptr::null_mut();
            if create(m.as_ptr(), &mut handle) != 0 || handle.is_null() {
                FreeLibrary(h);
                return None;
            }
            Some(NativeScorer {
                handle,
                _module: h,
                score_fn,
                destroy_fn,
                model_path: model.to_path_buf(),
            })
        }
    }

    /// 打分：ctx 非空时拼串（「ctx+候选」整串概率，实测句首/句中判序均对）。
    pub fn score(&self, ctx: &str, cands: &[String]) -> Vec<f64> {
        let joined: Vec<String> = if ctx.is_empty() {
            cands.to_vec()
        } else {
            cands.iter().map(|c| format!("{ctx}{c}")).collect()
        };
        let owned: Vec<Vec<u8>> = joined.iter().map(|c| cstr(c)).collect();
        let ptrs: Vec<*const u8> = owned.iter().map(|v| v.as_ptr()).collect();
        let mut scores = vec![f64::NEG_INFINITY; cands.len()];
        unsafe {
            (self.score_fn)(
                self.handle,
                ptrs.as_ptr(),
                cands.len() as i32,
                scores.as_mut_ptr(),
            );
        }
        scores
    }
}

impl Drop for NativeScorer {
    fn drop(&mut self) {
        unsafe {
            (self.destroy_fn)(self.handle);
            FreeLibrary(self._module);
        }
    }
}
