//! lyra-ffi: C ABI surface for the SwiftUI shell.
//!
//! Convention: strings out are heap-allocated — Swift must call
//! lyra_string_free. Errors are JSON strings, not panics: FFI boundaries
//! never unwind (panic=unwind is set in the workspace profile as a belt,
//! but the API contract is always Result→JSON).

use std::ffi::{c_char, c_int, CStr, CString};
use std::path::PathBuf;
use std::sync::Once;

static INIT: Once = Once::new();

fn init_logging() {
    INIT.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "lyra=info".into()),
            )
            .try_init();
    });
}

/// Static — do not free.
#[no_mangle]
pub extern "C" fn lyra_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// Probe an audio file: format + stream info + tags as one JSON object.
/// Caller frees with lyra_string_free. Returns null on null path.
#[no_mangle]
pub extern "C" fn lyra_probe(path: *const c_char) -> *mut c_char {
    init_logging();
    let path = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => return std::ptr::null_mut(),
    };

    let result = serde_json::json!({
        "format": lyra_formats::probe(&path),
        "stream": lyra_formats::stream_info(&path).ok(),
        "tags": lyra_formats::read_tags(&path).ok(),
    });
    CString::new(result.to_string()).unwrap_or_default().into_raw()
}

/// Frees strings returned by lyra_*.
///
/// # Safety
/// `s` must be a pointer previously returned by a lyra_* function, or null.
#[no_mangle]
pub unsafe extern "C" fn lyra_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

/// Start the remote-control server on `port` in a background runtime.
/// Returns 0 on success, 1 if the port is taken, 2 for other failures.
/// NOTE: pairing/transport-encryption per BLUEPRINT.md land before this is
/// safe to expose — scaffold binds the skeleton only.
#[no_mangle]
pub extern "C" fn lyra_remote_start(port: u16) -> c_int {
    init_logging();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(_) => return 2,
        };
        rt.block_on(async move {
            match lyra_remote::serve(port).await {
                Ok(()) => 0,
                Err(_) => 1,
            }
        })
    });
    0
}
