//! Owned TCP fixture evidence only. Inspect this process's actual symbol owners.
use std::ffi::c_void;
use std::path::PathBuf;

#[cfg(unix)]
fn owner(_name: &str, address: *const c_void) -> std::io::Result<PathBuf> {
    use std::ffi::{CStr, c_char, c_int};
    #[repr(C)]
    struct Info {
        filename: *const c_char,
        base: *mut c_void,
        symbol: *const c_char,
        symbol_address: *mut c_void,
    }
    #[cfg_attr(target_os = "linux", link(name = "dl"))]
    unsafe extern "C" {
        fn dladdr(address: *const c_void, info: *mut Info) -> c_int;
    }
    let mut info = Info {
        filename: std::ptr::null(),
        base: std::ptr::null_mut(),
        symbol: std::ptr::null(),
        symbol_address: std::ptr::null_mut(),
    };
    // SAFETY: address is an imported function, info is a valid writable Dl_info.
    if unsafe { dladdr(address, &mut info) } == 0 || info.filename.is_null() {
        return Err(std::io::Error::other("dladdr cannot resolve codec symbol"));
    }
    // SAFETY: successful dladdr supplies a process-owned NUL-terminated filename.
    let text = unsafe { CStr::from_ptr(info.filename) }
        .to_str()
        .map_err(std::io::Error::other)?;
    PathBuf::from(text).canonicalize()
}

#[cfg(windows)]
fn owner(name: &str, mut address: *const c_void) -> std::io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleExW(flags: u32, address: *const u16, module: *mut *mut c_void) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, path: *mut u16, capacity: u32) -> u32;
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
    }
    if std::env::var("MIXYARD_CODEC_RUNTIME_KIND").as_deref() == Ok("dylib") {
        // An imported function pointer can refer to the executable's import
        // thunk. Resolve the export in the already-loaded canonical DLL too;
        // dumpbin independently verifies the executable's actual DLL imports.
        let dll = if name.starts_with("ZSTD_") {
            "zstd.dll"
        } else {
            "opus.dll"
        };
        let wide: Vec<_> = dll.encode_utf16().chain(std::iter::once(0)).collect();
        let symbol = std::ffi::CString::new(name).map_err(std::io::Error::other)?;
        // SAFETY: names are NUL-terminated; GetModuleHandleW only looks up an
        // already-loaded module and does not load a new library for this probe.
        let loaded = unsafe { GetModuleHandleW(wide.as_ptr()) };
        if loaded.is_null() {
            return Err(std::io::Error::other("selected codec DLL is not loaded"));
        }
        // SAFETY: loaded is a live process module; symbol is NUL-terminated.
        address = unsafe { GetProcAddress(loaded, symbol.as_ptr().cast()) };
        if address.is_null() {
            return Err(std::io::Error::other("codec export is absent"));
        }
    }
    let mut module = std::ptr::null_mut();
    // FROM_ADDRESS | UNCHANGED_REFCOUNT: the pointer is a code address, not a name.
    // SAFETY: address is an imported function; module is writable; no unload occurs.
    if unsafe { GetModuleHandleExW(6, address.cast(), &mut module) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut buffer = vec![0u16; 32768];
    // SAFETY: module is valid and buffer has exactly the supplied writable capacity.
    let length = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 || length as usize >= buffer.len() {
        return Err(std::io::Error::other(
            "cannot resolve full codec module path",
        ));
    }
    PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize])).canonicalize()
}

pub fn report() -> std::io::Result<()> {
    if std::env::var_os("MIXYARD_CODEC_RUNTIME_PROBE").is_none() {
        return Ok(());
    }
    #[cfg(feature = "system-zstd")]
    {
        unsafe extern "C" {
            fn ZSTD_compress2(
                context: *mut c_void,
                output: *mut c_void,
                capacity: usize,
                input: *const c_void,
                length: usize,
            ) -> usize;
            fn ZSTD_decompressDCtx(
                context: *mut c_void,
                output: *mut c_void,
                capacity: usize,
                input: *const c_void,
                length: usize,
            ) -> usize;
        }
        for (name, address) in [
            (
                "ZSTD_compress2",
                ZSTD_compress2 as *const () as *const c_void,
            ),
            (
                "ZSTD_decompressDCtx",
                ZSTD_decompressDCtx as *const () as *const c_void,
            ),
        ] {
            println!(
                "codec_runtime_symbol={name}\t{}",
                owner(name, address)?.display()
            );
        }
    }
    #[cfg(feature = "system-opus")]
    {
        unsafe extern "C" {
            fn opus_encode_float(
                state: *mut c_void,
                pcm: *const f32,
                frames: i32,
                output: *mut u8,
                capacity: i32,
            ) -> i32;
            fn opus_decode_float(
                state: *mut c_void,
                input: *const u8,
                length: i32,
                output: *mut f32,
                frames: i32,
                fec: i32,
            ) -> i32;
        }
        for (name, address) in [
            (
                "opus_encode_float",
                opus_encode_float as *const () as *const c_void,
            ),
            (
                "opus_decode_float",
                opus_decode_float as *const () as *const c_void,
            ),
        ] {
            println!(
                "codec_runtime_symbol={name}\t{}",
                owner(name, address)?.display()
            );
        }
    }
    Ok(())
}
