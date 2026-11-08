//! Keep synchronous input responsive without promoting unrelated server work.
//!
//! Normal process (base 8) + HIGHEST pipe threads (base 10) preserves the old
//! AboveNormal input base, while default HTTP/watcher/native worker threads
//! remain at 8 and the Rust rerank pool's BELOW_NORMAL threads run at 7.

#[cfg(windows)]
pub const PROCESS_PRIORITY_CLASS: u32 = 0x0000_0020; // NORMAL_PRIORITY_CLASS

/// Called only by dedicated input service threads, never on a caller's UI thread.
#[cfg(windows)]
pub fn input_thread() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }
    // THREAD_PRIORITY_HIGHEST = +2. With Normal process class this is base 10,
    // the same as a normal thread in the previous AboveNormal process class.
    if unsafe { SetThreadPriority(GetCurrentThread(), 2) } == 0 {
        eprintln!("input thread priority failed: {}", std::io::Error::last_os_error());
    }
}

#[cfg(not(windows))]
pub fn input_thread() {}

/// The native scorer's owning worker also yields to interactive work. Internal
/// DLL threads are separate and must not be claimed to inherit this setting.
#[cfg(windows)]
pub fn inference_thread() {
    #[repr(C)]
    struct PowerThrottling { version: u32, control: u32, state: u32 }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
        fn SetThreadInformation(thread: isize, class: i32, info: *const core::ffi::c_void, size: u32) -> i32;
    }
    unsafe {
        let thread = GetCurrentThread();
        if SetThreadPriority(thread, -1) == 0 {
            eprintln!("inference thread priority failed: {}", std::io::Error::last_os_error());
        }
        let qos = PowerThrottling { version: 1, control: 1, state: 1 };
        // ThreadPowerThrottling = 3, EXECUTION_SPEED = 1.
        if SetThreadInformation(thread, 3, &qos as *const _ as *const _, std::mem::size_of_val(&qos) as u32) == 0 {
            eprintln!("inference thread EcoQoS failed: {}", std::io::Error::last_os_error());
        }
    }
}

#[cfg(not(windows))]
pub fn inference_thread() {}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn dedicated_input_thread_gets_relative_priority_two() {
        std::thread::spawn(|| {
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn GetCurrentThread() -> isize;
                fn GetThreadPriority(thread: isize) -> i32;
            }
            super::input_thread();
            assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, 2);
        }).join().unwrap();
    }
}
