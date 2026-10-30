//! AppKit's native terminate: does not produce Tao/Tauri ExitRequested.
//! Add only its preventable delegate callback, retaining Tao's full delegate.
use std::ffi::{c_char, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::OnceLock;

static REQUEST_QUIT: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

extern "C" {
    fn mixyard_install_termination_guard(callback: extern "C" fn()) -> *const c_char;
}

extern "C" fn request_quit() {
    // Never unwind across Objective-C. Failure remains fail-closed: AppKit
    // still receives NSTerminateCancel and no process/runtime is stopped.
    if catch_unwind(AssertUnwindSafe(|| {
        if let Some(callback) = REQUEST_QUIT.get() {
            callback();
        }
    }))
    .is_err()
    {
        eprintln!("Mixyard could not enqueue the native termination draft guard");
    }
}

pub fn install(callback: impl Fn() + Send + Sync + 'static) -> Result<(), String> {
    REQUEST_QUIT
        .set(Box::new(callback))
        .map_err(|_| "AppKit termination guard already initialized".to_string())?;
    // The SDK-typed shim validates main-thread/delegate/method ownership. It
    // returns only NULL or a process-lifetime static error string.
    let error = unsafe { mixyard_install_termination_guard(request_quit) };
    if error.is_null() {
        Ok(())
    } else {
        Err(unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned())
    }
}
