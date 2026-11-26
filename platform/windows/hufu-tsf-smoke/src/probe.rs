//! Read-only loader probe. Never register/activate TSF or contact the server.
use windows::core::*;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::Com::IClassFactory;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::TextServices::ITfTextInputProcessorEx;

fn export_names(bytes: &[u8]) -> std::result::Result<(u16, Vec<String>), String> {
    let u16_at = |n: usize| bytes.get(n..n + 2).map(|b| u16::from_le_bytes(b.try_into().unwrap())).ok_or("truncated PE");
    let u32_at = |n: usize| bytes.get(n..n + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).ok_or("truncated PE");
    if bytes.get(..2) != Some(b"MZ") { return Err("not an MZ image".into()); }
    let pe = u32_at(0x3c)? as usize;
    if bytes.get(pe..pe + 4) != Some(b"PE\0\0") { return Err("not a PE image".into()); }
    let machine = u16_at(pe + 4)?;
    let sections = u16_at(pe + 6)? as usize;
    let optional_size = u16_at(pe + 20)? as usize;
    let opt = pe + 24;
    let directories = match u16_at(opt)? { 0x10b => opt + 96, 0x20b => opt + 112, _ => return Err("invalid optional header".into()) };
    let section_table = opt + optional_size;
    let rva_to_file = |rva: u32| -> std::result::Result<usize, String> {
        for i in 0..sections {
            let s = section_table + i * 40;
            let va = u32_at(s + 12)?;
            let raw_size = u32_at(s + 16)?;
            if rva >= va && rva - va < raw_size { return Ok((u32_at(s + 20)? + rva - va) as usize); }
        }
        Err(format!("unmapped export RVA {rva:x}"))
    };
    let exports = rva_to_file(u32_at(directories)?)?;
    let count = u32_at(exports + 24)? as usize;
    if count == 0 || count > 4096 { return Err("invalid export count".into()); }
    let names = rva_to_file(u32_at(exports + 32)?)?;
    let mut result = Vec::with_capacity(count);
    for i in 0..count {
        let offset = rva_to_file(u32_at(names + i * 4)?)?;
        let tail = bytes.get(offset..).ok_or("export name outside image")?;
        let length = tail.iter().position(|b| *b == 0).ok_or("unterminated export name")?;
        result.push(std::str::from_utf8(&tail[..length]).map_err(|_| "non-UTF8 export")?.to_owned());
    }
    Ok((machine, result))
}

pub fn run(path: &str) -> std::result::Result<(), String> {
    std::env::set_var("HUFU_TSF_PROBE_ONLY", "1");
    let absolute = std::path::absolute(path).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&absolute).map_err(|e| e.to_string())?;
    let (machine, names) = export_names(&bytes)?;
    let wide: Vec<u16> = absolute.as_os_str().to_string_lossy().encode_utf16().chain([0]).collect();
    unsafe {
        let module = LoadLibraryExW(PCWSTR(wide.as_ptr()), None, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS).map_err(|e| format!("LoadLibraryEx: {e}"))?;
        // A short-lived process owns the module, so every error path safely ends
        // in process teardown instead of unloading a live COM vtable.
        for name in &names {
            let c = std::ffi::CString::new(name.as_str()).map_err(|e| e.to_string())?;
            GetProcAddress(module, PCSTR(c.as_ptr().cast())).ok_or_else(|| format!("unresolved export {name}"))?;
        }
        let get = GetProcAddress(module, s!("DllGetClassObject")).ok_or("missing DllGetClassObject")?;
        let mut implementation = HMODULE::default();
        GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, PCWSTR(get as *const () as *const u16), &mut implementation).map_err(|e| e.to_string())?;
        let mut module_path = [0u16; 32768];
        let n = GetModuleFileNameW(implementation, &mut module_path) as usize;
        if n == 0 || n >= module_path.len() { return Err("cannot resolve implementation module".into()); }
        let implementation_path = String::from_utf16_lossy(&module_path[..n]);
        let get: super::DllGetClassObjectFn = std::mem::transmute(get);
        let mut raw = std::ptr::null_mut();
        get(&super::CLSID_HUFU, &IClassFactory::IID, &mut raw).ok().map_err(|e| format!("DllGetClassObject: {e}"))?;
        if raw.is_null() { return Err("class factory returned null".into()); }
        let factory = IClassFactory::from_raw(raw);
        let tip: ITfTextInputProcessorEx = factory.CreateInstance(None).map_err(|e| format!("CreateInstance: {e}"))?;
        let path_fn = GetProcAddress(module, s!("hufu_probe_registration_path")).ok_or("missing registration path probe")?;
        let path_fn: unsafe extern "system" fn(*mut u16, u32) -> u32 = std::mem::transmute(path_fn);
        let mut registration_path = [0u16; 32768];
        let n = path_fn(registration_path.as_mut_ptr(), registration_path.len() as u32) as usize;
        if n == 0 || n > registration_path.len() { return Err("registration path unavailable".into()); }
        let registration_path = String::from_utf16_lossy(&registration_path[..n - 1]);
        let requested = absolute.canonicalize().map_err(|e| e.to_string())?;
        let registered = std::path::Path::new(&registration_path).canonicalize().map_err(|e| format!("registration target missing: {e}"))?;
        if !requested.as_os_str().to_string_lossy().eq_ignore_ascii_case(&registered.as_os_str().to_string_lossy()) {
            return Err(format!("registration target differs from requested facade: {registration_path}"));
        }
        drop(tip);
        drop(factory);
        println!("probe_status=ok");
        println!("host_arch={}", std::env::consts::ARCH);
        println!("image_machine=0x{machine:04X}");
        println!("exports_resolved={}", names.len());
        println!("implementation={implementation_path}");
        println!("registration={registration_path}");
        println!("registry_writes=0\nprofile_activation=0\nserver_calls=0");
    }
    Ok(())
}
