//! lyra-ffi: C ABI surface for the SwiftUI shell.
//!
//! Convention: strings out are heap-allocated — Swift must call
//! lyra_string_free. Errors are JSON strings, not panics: FFI boundaries
//! never unwind (panic=unwind is set in the workspace profile as a belt,
//! but the API contract is always Result→JSON).

use std::ffi::{c_char, c_int, CStr, CString};
use std::path::PathBuf;
use std::sync::Once;

/// mimalloc as the Rust core's allocator — measurable RSS reduction for the
/// alloc patterns here (many small blocks + stream buffers) vs the macOS
/// default. One line, real win.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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

/// Scan a folder recursively → JSON array of LibraryTrack. Caller frees.
/// Synchronous — call from a background thread for big trees.
#[no_mangle]
pub extern "C" fn lyra_scan_dir(path: *const c_char) -> *mut c_char {
    init_logging();
    let path = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => return std::ptr::null_mut(),
    };
    match lyra_formats::scan_dir(&path) {
        Ok(tracks) => CString::new(serde_json::json!(tracks).to_string())
            .unwrap_or_default()
            .into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
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

/// ── Playback engine ─────────────────────────────────────────────────────
/// Opaque handle API. `lyra_engine_new` may return null if no output device.

/// Create the engine (brings up the output stream + worker). Null on failure.
#[no_mangle]
pub extern "C" fn lyra_engine_new() -> *mut lyra_engine::Engine {
    init_logging();
    match lyra_engine::Engine::new() {
        Ok(e) => Box::into_raw(Box::new(e)),
        Err(e) => {
            tracing::error!("engine init: {e}");
            std::ptr::null_mut()
        }
    }
}

/// Play a local file (block-cached through lyra-fs). Returns 0 if the
/// command was accepted.
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_play_file(e: *mut lyra_engine::Engine, path: *const c_char) -> c_int {
    if e.is_null() {
        return 2;
    }
    let path = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(p) => PathBuf::from(p),
        Err(_) => return 2,
    };
    let ext = path.extension().and_then(|s| s.to_str()).map(String::from);
    match lyra_fs::LocalFile::open(&path) {
        Ok(local) => {
            let cached = lyra_fs::CachingSource::wrap(local);
            unsafe { &*e }.play(cached, ext.as_deref());
            0
        }
        Err(_) => 1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn lyra_engine_pause(e: *mut lyra_engine::Engine) {
    if !e.is_null() { unsafe { &*e }.pause() }
}
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_resume(e: *mut lyra_engine::Engine) {
    if !e.is_null() { unsafe { &*e }.resume() }
}
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_stop(e: *mut lyra_engine::Engine) {
    if !e.is_null() { unsafe { &*e }.stop() }
}
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_seek(e: *mut lyra_engine::Engine, secs: f64) {
    if !e.is_null() { unsafe { &*e }.seek(secs) }
}
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_set_volume(e: *mut lyra_engine::Engine, v: f32) {
    if !e.is_null() { unsafe { &*e }.set_volume(v) }
}
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_position(e: *const lyra_engine::Engine) -> f64 {
    if e.is_null() { 0.0 } else { unsafe { &*e }.position_secs() as f64 }
}
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_is_playing(e: *const lyra_engine::Engine) -> c_int {
    if e.is_null() { 0 } else { unsafe { &*e }.is_playing() as c_int }
}
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_can_resume(e: *const lyra_engine::Engine) -> c_int {
    if e.is_null() { 0 } else { unsafe { &*e }.can_resume() as c_int }
}

/// Set EQ band params. `peaking` 1 = peaking filter, 0 = low shelf.
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_set_band(
    e: *mut lyra_engine::Engine,
    band: c_int,
    freq_hz: f32,
    q: f32,
    gain_db: f32,
    peaking: c_int,
) {
    if e.is_null() || band < 0 {
        return;
    }
    unsafe { &*e }.set_band(
        band as usize,
        lyra_engine::BandSpec {
            freq_hz,
            q,
            gain_db,
            peaking: peaking != 0,
        },
    );
}

/// Viz snapshot as JSON: {"bands":[…48], "peak":[l,r] dB, "clip":bool}.
/// Caller frees with lyra_string_free.
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_viz(e: *const lyra_engine::Engine) -> *mut c_char {
    if e.is_null() {
        return std::ptr::null_mut();
    }
    let (bands, peak, clip) = unsafe { &*e }.viz_snapshot();
    let json = serde_json::json!({"bands": bands, "peak": peak, "clip": clip});
    CString::new(json.to_string()).unwrap_or_default().into_raw()
}

/// Fill `out` with normalized spectrum bands (0..1). Returns bands written.
/// This is the 60Hz path — no JSON, no alloc.
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_viz_bands(
    e: *const lyra_engine::Engine,
    out: *mut f32,
    n: usize,
) -> usize {
    if e.is_null() || out.is_null() || n == 0 {
        return 0;
    }
    unsafe { &*e }.viz_bands(std::slice::from_raw_parts_mut(out, n))
}

/// EQ response curve as JSON: {"freqs":[…], "db":[…]} — the drawn curve
/// uses the same biquad coefficients as the audio path. Caller frees.
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_eq_response(e: *const lyra_engine::Engine) -> *mut c_char {
    if e.is_null() {
        return std::ptr::null_mut();
    }
    // Log-spaced 20Hz–20kHz, 200 points.
    let n = 200usize;
    let freqs: Vec<f32> = (0..n)
        .map(|i| 20.0 * (1000f32).powf(i as f32 / (n - 1) as f32))
        .collect();
    let db = unsafe { &*e }.eq_response(&freqs);
    let json = serde_json::json!({"freqs": freqs, "db": db});
    CString::new(json.to_string()).unwrap_or_default().into_raw()
}

/// Shutdown + free. Safe on null.
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_free(e: *mut lyra_engine::Engine) {
    if !e.is_null() {
        unsafe { &*e }.shutdown();
        drop(Box::from_raw(e));
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
