//! lyra-ffi: C ABI surface for the SwiftUI shell.
//!
//! Convention: strings out are heap-allocated — Swift must call
//! lyra_string_free. Errors are JSON strings, not panics: FFI boundaries
//! never unwind (panic=unwind is set in the workspace profile as a belt,
//! but the API contract is always Result→JSON).

use std::ffi::{c_char, c_int, CStr, CString};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Mutex, Once};

/// mimalloc as the Rust core's allocator — measurable RSS reduction for the
/// alloc patterns here (many small blocks + stream buffers) vs the macOS
/// default. One line, real win.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

static INIT: Once = Once::new();

mod art;
mod coach;
mod ipc;
mod map;
mod remote_fs;

pub(crate) fn init_logging() {
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
///
/// # Safety
/// `path` may be null (returns null); otherwise it must point to a valid NUL-terminated C string.
/// Free a non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_probe(path: *const c_char) -> *mut c_char {
    if path.is_null() {
        return std::ptr::null_mut();
    }
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
    CString::new(result.to_string())
        .unwrap_or_default()
        .into_raw()
}

/// Scan a folder recursively → JSON array of LibraryTrack. Caller frees.
/// Synchronous — call from a background thread for big trees.
///
/// # Safety
/// `path` may be null (returns null); otherwise it must point to a valid NUL-terminated C string.
/// Free a non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_scan_dir(path: *const c_char) -> *mut c_char {
    if path.is_null() {
        return std::ptr::null_mut();
    }
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
        drop(unsafe { CString::from_raw(s) });
    }
}

/// ── Playback engine ─────────────────────────────────────────────────────
/// Opaque handle API. `lyra_engine_new` may return null if no output device.
///
/// The engine is hot-swappable: `set_output_mode` builds a replacement,
/// swaps it in, then retires the old one. `lyra_engine_current` always
/// returns the live handle — callers must not cache it across a mode
/// switch. ENGINE_LOCK guards the free vs the remote-command path: the
/// sink holds it while touching the engine, the swap holds it while
/// freeing, so a remote command can never land on a retired engine.
static CURRENT_ENGINE: AtomicUsize = AtomicUsize::new(0);
static CURRENT_MODE: AtomicI32 = AtomicI32::new(0);
static ENGINE_LOCK: Mutex<()> = Mutex::new(());

fn install_engine(e: *mut lyra_engine::Engine) -> *mut lyra_engine::Engine {
    let _g = ENGINE_LOCK.lock().unwrap();
    let old = CURRENT_ENGINE.swap(e as usize, Ordering::SeqCst) as *mut lyra_engine::Engine;
    if !old.is_null() {
        unsafe {
            (&*old).shutdown();
            drop(Box::from_raw(old));
        }
    }
    e
}

/// The live engine handle — re-fetch after any output-mode switch.
#[no_mangle]
pub extern "C" fn lyra_engine_current() -> *mut lyra_engine::Engine {
    CURRENT_ENGINE.load(Ordering::SeqCst) as *mut lyra_engine::Engine
}

/// Create the engine on the compat (cpal) path. Null on failure.
#[no_mangle]
pub extern "C" fn lyra_engine_new() -> *mut lyra_engine::Engine {
    lyra_engine_new_mode(0)
}

/// Create the engine on a chosen output path (0 = Compat, 1 = HAL
/// exclusive). Swaps out any existing engine. Null on failure — a failed
/// build leaves the previous engine running.
#[no_mangle]
pub extern "C" fn lyra_engine_new_mode(mode: c_int) -> *mut lyra_engine::Engine {
    init_logging();
    let m = if mode == 1 {
        lyra_engine::OutputMode::HalExclusive
    } else {
        lyra_engine::OutputMode::Compat
    };
    match lyra_engine::Engine::with_output(m) {
        Ok(e) => {
            CURRENT_MODE.store(mode, Ordering::SeqCst);
            install_engine(Box::into_raw(Box::new(e)))
        }
        Err(e) => {
            tracing::error!("engine init: {e}");
            std::ptr::null_mut()
        }
    }
}

/// Switch the output path at runtime. 0 ok — the new engine is live and
/// the old one retired (playback restarts idle). Nonzero on failure —
/// the previous engine is untouched.
#[no_mangle]
pub extern "C" fn lyra_engine_set_output_mode(mode: c_int) -> c_int {
    if lyra_engine_new_mode(mode).is_null() {
        1
    } else {
        0
    }
}

/// 0 = Compat, 1 = HAL exclusive — the mode the live engine was built with.
#[no_mangle]
pub extern "C" fn lyra_engine_output_mode() -> c_int {
    CURRENT_MODE.load(Ordering::SeqCst)
}

/// Play a local file (block-cached through lyra-fs). Returns 0 if the
/// command was accepted.
///
/// # Safety
/// `e` must be a live engine handle from `lyra_engine_new[_mode]` (null is tolerated, returns 2).
/// `path` must be non-null and point to a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_play_file(
    e: *mut lyra_engine::Engine,
    path: *const c_char,
) -> c_int {
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

/// Pause playback. Null-safe no-op.
///
/// # Safety
/// `e` must be a live engine handle or null (null is a no-op).
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_pause(e: *mut lyra_engine::Engine) {
    if !e.is_null() {
        unsafe { &*e }.pause()
    }
}
/// Resume playback. Null-safe no-op.
///
/// # Safety
/// `e` must be a live engine handle or null (null is a no-op).
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_resume(e: *mut lyra_engine::Engine) {
    if !e.is_null() {
        unsafe { &*e }.resume()
    }
}
/// Stop playback and release the source. Null-safe no-op.
///
/// # Safety
/// `e` must be a live engine handle or null (null is a no-op).
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_stop(e: *mut lyra_engine::Engine) {
    if !e.is_null() {
        unsafe { &*e }.stop()
    }
}
/// Seek to `secs` seconds. Null-safe no-op.
///
/// # Safety
/// `e` must be a live engine handle or null (null is a no-op).
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_seek(e: *mut lyra_engine::Engine, secs: f64) {
    if !e.is_null() {
        unsafe { &*e }.seek(secs)
    }
}
/// Set output volume (0.0–1.0, square-law taper). Null-safe no-op.
///
/// # Safety
/// `e` must be a live engine handle or null (null is a no-op).
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_set_volume(e: *mut lyra_engine::Engine, v: f32) {
    if !e.is_null() {
        unsafe { &*e }.set_volume(v)
    }
}
/// Current playback position in seconds; 0.0 when null.
///
/// # Safety
/// `e` must be a live engine handle or null (null yields 0.0).
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_position(e: *const lyra_engine::Engine) -> f64 {
    if e.is_null() {
        0.0
    } else {
        unsafe { &*e }.position_secs() as f64
    }
}
/// Nonzero while audio is flowing; 0 when null.
///
/// # Safety
/// `e` must be a live engine handle or null (null yields 0).
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_is_playing(e: *const lyra_engine::Engine) -> c_int {
    if e.is_null() {
        0
    } else {
        unsafe { &*e }.is_playing() as c_int
    }
}
/// Nonzero when paused mid-track with position held; 0 when null.
///
/// # Safety
/// `e` must be a live engine handle or null (null yields 0).
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_can_resume(e: *const lyra_engine::Engine) -> c_int {
    if e.is_null() {
        0
    } else {
        unsafe { &*e }.can_resume() as c_int
    }
}

/// Set EQ band params. `peaking` 1 = peaking filter, 0 = low shelf.
///
/// # Safety
/// `e` must be a live engine handle or null (null is a no-op).
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
///
/// # Safety
/// `e` must be a live engine handle or null (null returns null).
/// Free a non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_viz(e: *const lyra_engine::Engine) -> *mut c_char {
    if e.is_null() {
        return std::ptr::null_mut();
    }
    let (bands, peak, clip) = unsafe { &*e }.viz_snapshot();
    let json = serde_json::json!({"bands": bands, "peak": peak, "clip": clip});
    CString::new(json.to_string())
        .unwrap_or_default()
        .into_raw()
}

/// Fill `out` with normalized spectrum bands (0..1). Returns bands written.
/// This is the 60Hz path — no JSON, no alloc.
///
/// # Safety
/// `e` must be a live engine handle or null (null returns 0 without touching `out`).
/// `out` must be null or point to `n` writable `f32` slots.
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_viz_bands(
    e: *const lyra_engine::Engine,
    out: *mut f32,
    n: usize,
) -> usize {
    if e.is_null() || out.is_null() || n == 0 {
        return 0;
    }
    let e = unsafe { &*e };
    let out = unsafe { std::slice::from_raw_parts_mut(out, n) };
    e.viz_bands(out)
}

/// Copy the latest viz frame into `out` — the 60 Hz path: short lock,
/// memcpy only, no math. Returns seq; Swift skips redraw when seq is
/// unchanged. Null-safe: returns 0 without touching `out`.
///
/// # Safety
/// `e` must be a live engine handle or null (null returns 0 without touching `out`).
/// `out` must be null or point to a writable `VizFrame`.
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_viz_frame(
    e: *const lyra_engine::Engine,
    out: *mut lyra_engine::VizFrame,
) -> u64 {
    if e.is_null() || out.is_null() {
        return 0;
    }
    let e = unsafe { &*e };
    let out = unsafe { &mut *out };
    e.viz_frame(out)
}

/// EQ response curve as JSON: {"freqs":[…], "db":[…]} — the drawn curve
/// uses the same biquad coefficients as the audio path. Caller frees.
///
/// # Safety
/// `e` must be a live engine handle or null (null returns null).
/// Free a non-null return with `lyra_string_free`.
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
    CString::new(json.to_string())
        .unwrap_or_default()
        .into_raw()
}

/// ── Library DB (lyra-store) ─────────────────────────────────────────────
/// Opaque handle. Open once at app start; the library persists across
/// launches — rescan only re-probes mtime-changed files.
/// Open/create the library DB at `path`. Null on failure.
///
/// # Safety
/// `path` may be null (returns null); otherwise it must point to a valid NUL-terminated C string.
/// A living handle must be freed exactly once with `lyra_lib_free` and never used afterwards.
#[no_mangle]
pub unsafe extern "C" fn lyra_lib_open(path: *const c_char) -> *mut lyra_store::Library {
    if path.is_null() {
        return std::ptr::null_mut();
    }
    init_logging();
    let path = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => return std::ptr::null_mut(),
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match lyra_store::Library::open(&path) {
        Ok(l) => Box::into_raw(Box::new(l)),
        Err(e) => {
            tracing::error!("lib open: {e}");
            std::ptr::null_mut()
        }
    }
}

/// Incremental sync of a folder into the DB → SyncStats JSON. Caller frees.
///
/// # Safety
/// `l` must be a live library handle from `lyra_lib_open` or null (null returns null).
/// `dir` must be non-null and point to a valid NUL-terminated C string.
/// Free a non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_lib_sync_dir(
    l: *mut lyra_store::Library,
    dir: *const c_char,
) -> *mut c_char {
    if l.is_null() {
        return std::ptr::null_mut();
    }
    let dir = match unsafe { CStr::from_ptr(dir) }.to_str() {
        Ok(p) => PathBuf::from(p),
        Err(_) => return std::ptr::null_mut(),
    };
    match unsafe { &*l }.sync_dir(&dir) {
        Ok(stats) => CString::new(serde_json::json!(stats).to_string())
            .unwrap_or_default()
            .into_raw(),
        Err(e) => {
            tracing::error!("sync_dir: {e}");
            std::ptr::null_mut()
        }
    }
}

/// Sync an explicit list of files (JSON array of paths) → SyncStats JSON.
/// Used by the picker's file selection — unlike sync_dir this never prunes.
///
/// # Safety
/// `l` must be a live library handle from `lyra_lib_open` or null (null returns null).
/// `json` must be non-null and point to a valid NUL-terminated C string.
/// Free a non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_lib_sync_files(
    l: *mut lyra_store::Library,
    json: *const c_char,
) -> *mut c_char {
    if l.is_null() {
        return std::ptr::null_mut();
    }
    let json = unsafe { CStr::from_ptr(json) }.to_str().unwrap_or_default();
    let files: Vec<PathBuf> = serde_json::from_str::<Vec<String>>(json)
        .map(|v| v.into_iter().map(PathBuf::from).collect())
        .unwrap_or_default();
    match unsafe { &*l }.sync_files(&files) {
        Ok(stats) => CString::new(serde_json::json!(stats).to_string())
            .unwrap_or_default()
            .into_raw(),
        Err(e) => {
            tracing::error!("sync_files: {e}");
            std::ptr::null_mut()
        }
    }
}

/// All library rows as JSON. Caller frees.
///
/// # Safety
/// `l` must be a live library handle from `lyra_lib_open` or null (null returns null).
/// Free a non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_lib_tracks(l: *mut lyra_store::Library) -> *mut c_char {
    if l.is_null() {
        return std::ptr::null_mut();
    }
    match unsafe { &*l }.all_tracks() {
        Ok(t) => CString::new(serde_json::json!(t).to_string())
            .unwrap_or_default()
            .into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// FTS search → JSON rows. Caller frees.
///
/// # Safety
/// `l` must be a live library handle from `lyra_lib_open` or null (null returns null).
/// `q` must be non-null and point to a valid NUL-terminated C string.
/// Free a non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_lib_search(
    l: *mut lyra_store::Library,
    q: *const c_char,
) -> *mut c_char {
    if l.is_null() {
        return std::ptr::null_mut();
    }
    let q = unsafe { CStr::from_ptr(q) }.to_str().unwrap_or_default();
    match unsafe { &*l }.search(q) {
        Ok(t) => CString::new(serde_json::json!(t).to_string())
            .unwrap_or_default()
            .into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Close the library connection and free the handle.
///
/// # Safety
/// `l` must be a pointer returned by `lyra_lib_open` (or null), freed at most once and never used afterwards.
#[no_mangle]
pub unsafe extern "C" fn lyra_lib_free(l: *mut lyra_store::Library) {
    if !l.is_null() {
        drop(unsafe { Box::from_raw(l) });
    }
}

/// Shutdown + free the live engine. Safe on null; `e` kept for ABI
/// symmetry with the other lyra_engine_* calls.
///
/// # Safety
/// `e` should be an engine handle (null tolerated); the live engine must not be freed twice — double free is undefined behavior.
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_free(e: *mut lyra_engine::Engine) {
    let _g = ENGINE_LOCK.lock().unwrap();
    let cur = CURRENT_ENGINE.swap(0, Ordering::SeqCst) as *mut lyra_engine::Engine;
    let p = if cur.is_null() { e } else { cur };
    if !p.is_null() {
        unsafe { &*p }.shutdown();
        drop(unsafe { Box::from_raw(p) });
    }
}

// ── Remote control ───────────────────────────────────────────────────────
// One global Host; commands route into the engine via EngineSink.

static REMOTE: std::sync::OnceLock<Result<std::sync::Arc<lyra_remote::Host>, String>> =
    std::sync::OnceLock::new();

/// Resolves through CURRENT_ENGINE under ENGINE_LOCK so remote commands
/// always hit the live engine — output-mode swaps can't strand it.
struct EngineSink;

impl lyra_remote::CommandSink for EngineSink {
    fn handle(&self, cmd: &lyra_core::PlayerCommand) -> serde_json::Value {
        use lyra_core::PlayerCommand as C;
        let _g = ENGINE_LOCK.lock().unwrap();
        let ptr = CURRENT_ENGINE.load(Ordering::SeqCst) as *const lyra_engine::Engine;
        if ptr.is_null() {
            return serde_json::json!({"ok": false, "error": "no engine"});
        }
        let e = unsafe { &*ptr };
        match cmd {
            C::Toggle => {
                if e.is_playing() {
                    e.pause()
                } else {
                    e.resume()
                }
            }
            C::StopAfterCurrent => e.stop(),
            C::Seek { position_secs } => e.seek(*position_secs),
            C::Volume { value } => e.set_volume(*value),
            C::Mute { on } => e.set_volume(if *on { 0.0 } else { 1.0 }),
            _ => return serde_json::json!({"ok": false, "error": "unhandled"}),
        }
        serde_json::json!({
            "ok": true,
            "playing": e.is_playing(),
            "position": e.position_secs(),
        })
    }
}

fn remote() -> Result<&'static std::sync::Arc<lyra_remote::Host>, c_int> {
    match REMOTE.get() {
        Some(Ok(h)) => Ok(h),
        _ => Err(2),
    }
}

/// Create the remote host bound to `e`. `key_path` persists the pinned
/// X25519 identity. Call once at app start. 0 ok, 1 init failed.
///
/// # Safety
/// `e` must be a live engine handle or null (null returns 2).
/// `key_path` must be non-null and point to a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn lyra_remote_init(
    e: *mut lyra_engine::Engine,
    key_path: *const c_char,
) -> c_int {
    if e.is_null() {
        return 2;
    }
    let kp = match unsafe { CStr::from_ptr(key_path) }.to_str() {
        Ok(p) => PathBuf::from(p),
        Err(_) => return 2,
    };
    let res = lyra_remote::Host::new(&kp, std::sync::Arc::new(EngineSink))
        .map(std::sync::Arc::new)
        .map_err(|e| e.to_string());
    let _ = REMOTE.set(res);
    match REMOTE.get() {
        Some(Ok(_)) => 0,
        _ => 1,
    }
}

/// Start the Noise listener on `port` in a background runtime. 0 ok.
#[no_mangle]
pub extern "C" fn lyra_remote_start(port: u16) -> c_int {
    init_logging();
    let host = match remote() {
        Ok(h) => std::sync::Arc::clone(h),
        Err(c) => return c,
    };
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(_) => return 2,
        };
        rt.block_on(async move {
            match lyra_remote::serve(host, port).await {
                Ok(()) => 0,
                Err(_) => 1,
            }
        })
    });
    0
}

/// Open a pairing window → JSON {"code":"123456","fp":"AA BB .."}.
/// Null if remote not initialized. Free with lyra_string_free.
#[no_mangle]
pub extern "C" fn lyra_remote_open_pairing() -> *mut c_char {
    match remote() {
        Ok(h) => {
            let code = h.open_pairing();
            let j = serde_json::json!({"code": code, "fp": h.fingerprint()});
            CString::new(j.to_string()).unwrap().into_raw()
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// Number of pinned devices.
#[no_mangle]
pub extern "C" fn lyra_remote_paired_count() -> c_int {
    remote().map(|h| h.paired_count() as c_int).unwrap_or(-1)
}

/// Paired devices as JSON [{id, name}] — `id` is the pinned-key hash hex
/// used by lyra_remote_revoke. Free with lyra_string_free.
#[no_mangle]
pub extern "C" fn lyra_remote_devices() -> *mut c_char {
    match remote() {
        Ok(h) => {
            let j = serde_json::json!(h
                .devices()
                .iter()
                .map(|(id, name)| serde_json::json!({"id": id, "name": name}))
                .collect::<Vec<_>>());
            CString::new(j.to_string()).unwrap_or_default().into_raw()
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// Remove a paired device by id (hash hex from lyra_remote_devices).
/// 0 revoked, 1 unknown id, 2 remote not initialized.
///
/// # Safety
/// `id` must be non-null and point to a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn lyra_remote_revoke(id: *const c_char) -> c_int {
    let id = match unsafe { CStr::from_ptr(id) }.to_str() {
        Ok(s) => s,
        Err(_) => return 1,
    };
    match remote() {
        Ok(h) => {
            if h.revoke(id) {
                0
            } else {
                1
            }
        }
        Err(c) => c,
    }
}

// ── Torrents ─────────────────────────────────────────────────────────────
// One global rqbit session per app process — download_dir must live inside
// the app container (Swift passes its Application Support path).

static TORRENT: std::sync::OnceLock<Result<std::sync::Arc<lyra_torrent::TorrentEngine>, String>> =
    std::sync::OnceLock::new();

pub(crate) fn torrent() -> Result<&'static std::sync::Arc<lyra_torrent::TorrentEngine>, c_int> {
    match TORRENT.get() {
        Some(Ok(e)) => Ok(e),
        Some(Err(_)) => Err(1),
        None => Err(2), // not initialized
    }
}

/// Initialize the torrent session. Idempotent — first call wins. 0 ok.
///
/// # Safety
/// `download_dir` may be null (returns 2); otherwise it must point to a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn lyra_torrent_init(download_dir: *const c_char) -> c_int {
    if download_dir.is_null() {
        return 2;
    }
    init_logging();
    let dir = match unsafe { CStr::from_ptr(download_dir) }.to_str() {
        Ok(p) => PathBuf::from(p),
        Err(_) => return 2,
    };
    let res = lyra_torrent::TorrentEngine::new(dir)
        .map(std::sync::Arc::new)
        .map_err(|e| e.to_string());
    if let Err(e) = &res {
        tracing::error!("torrent init: {e}");
    }
    let _ = TORRENT.set(res);
    match TORRENT.get() {
        Some(Ok(_)) => 0,
        _ => 1,
    }
}

/// Add a magnet URI or local .torrent path → torrent id (≥0), −1 bad spec,
/// −2 add failure, −3 engine not initialized. Blocks on metadata resolve
/// for magnets — call off the main thread.
///
/// # Safety
/// `spec` may be null (returns -1); otherwise it must point to a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn lyra_torrent_add(spec: *const c_char) -> c_int {
    if spec.is_null() {
        return -1;
    }
    let spec = match unsafe { CStr::from_ptr(spec) }.to_str() {
        Ok(s) if !s.is_empty() => s,
        _ => return -1,
    };
    match torrent() {
        Err(c) => -3 - c,
        Ok(e) => e.add(spec).map(|id| id as c_int).unwrap_or(-2),
    }
}

/// JSON [{index,path,len}] for a resolved torrent. Null on failure.
#[no_mangle]
pub extern "C" fn lyra_torrent_files(id: c_int) -> *mut c_char {
    let e = match torrent() {
        Ok(e) => e,
        Err(_) => return std::ptr::null_mut(),
    };
    match e.files(id as usize) {
        Ok(fs) => {
            let j = serde_json::json!(fs
                .iter()
                .map(|f| serde_json::json!({"index": f.index, "path": f.path, "len": f.len}))
                .collect::<Vec<_>>());
            CString::new(j.to_string()).unwrap().into_raw()
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// Remove torrent `id` from the session. `delete_files != 0` also deletes
/// its downloaded data from disk. 0 ok, 1 unknown torrent, 2+ engine error.
#[no_mangle]
pub extern "C" fn lyra_torrent_remove(id: c_int, delete_files: c_int) -> c_int {
    let e = match torrent() {
        Ok(e) => e,
        Err(c) => return 2 + c,
    };
    match e.remove(id as usize, delete_files != 0) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// JSON [{id,name}] of managed torrents — the session persists across
/// relaunches, so the UI rebuilds its list from this. Null on failure.
#[no_mangle]
pub extern "C" fn lyra_torrent_list() -> *mut c_char {
    let e = match torrent() {
        Ok(e) => e,
        Err(_) => return std::ptr::null_mut(),
    };
    let j = serde_json::json!(e
        .list()
        .iter()
        .map(|(id, name)| serde_json::json!({"id": id, "name": name}))
        .collect::<Vec<_>>());
    CString::new(j.to_string()).unwrap().into_raw()
}

/// JSON [{name,bytes}] of download_dir entries not owned by any managed
/// torrent. Null on failure.
#[no_mangle]
pub extern "C" fn lyra_torrent_orphans() -> *mut c_char {
    let e = match torrent() {
        Ok(e) => e,
        Err(_) => return std::ptr::null_mut(),
    };
    match e.orphans() {
        Ok(os) => {
            let j = serde_json::json!(os
                .iter()
                .map(|(name, bytes)| serde_json::json!({"name": name, "bytes": bytes}))
                .collect::<Vec<_>>());
            CString::new(j.to_string()).unwrap().into_raw()
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// Delete every orphan entry → JSON {removed,bytes}. Null on failure.
#[no_mangle]
pub extern "C" fn lyra_torrent_purge_orphans() -> *mut c_char {
    let e = match torrent() {
        Ok(e) => e,
        Err(_) => return std::ptr::null_mut(),
    };
    match e.purge_orphans() {
        Ok((removed, bytes)) => {
            let j = serde_json::json!({"removed": removed, "bytes": bytes});
            CString::new(j.to_string()).unwrap().into_raw()
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// Session-wide tracker list (fixed at engine build from the ngosang
/// best-of cache + bundled fallback; rqbit has no post-add mutation).
/// JSON {trackers:[…]}. Null when the engine isn't initialized.
#[no_mangle]
pub extern "C" fn lyra_torrent_trackers() -> *mut c_char {
    let e = match torrent() {
        Ok(e) => e,
        Err(_) => return std::ptr::null_mut(),
    };
    let j = serde_json::json!({"trackers": e.session_trackers()});
    CString::new(j.to_string()).unwrap().into_raw()
}

/// JSON stats snapshot {progress_bytes,total_bytes,finished}. Null on failure.
#[no_mangle]
pub extern "C" fn lyra_torrent_stats(id: c_int) -> *mut c_char {
    let e = match torrent() {
        Ok(e) => e,
        Err(_) => return std::ptr::null_mut(),
    };
    match e.stats(id as usize) {
        Ok(j) => CString::new(j.to_string()).unwrap().into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Stream-play file `file_idx` of torrent `id`: pieces fetch on demand in
/// read order, CachingSource absorbs seek latency for the decoder.
/// Returns 0 if the engine accepted the source.
///
/// # Safety
/// `e` must be a live engine handle or null (null returns 2).
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_play_torrent(
    e: *mut lyra_engine::Engine,
    id: c_int,
    file_idx: c_int,
) -> c_int {
    if e.is_null() {
        return 2;
    }
    let eng = match torrent() {
        Ok(e) => e,
        Err(c) => return c,
    };
    let src = match eng.open_file(id as usize, file_idx as usize) {
        Ok(s) => s,
        Err(_) => return 1,
    };
    let ext = eng
        .files(id as usize)
        .ok()
        .and_then(|fs| fs.into_iter().find(|f| f.index == file_idx as usize))
        .and_then(|f| {
            std::path::Path::new(&f.path)
                .extension()
                .and_then(|s| s.to_str())
                .map(String::from)
        });
    let cached = lyra_fs::CachingSource::wrap(src);
    unsafe { &*e }.play(cached, ext.as_deref());
    0
}

/// Probe file `file_idx` of torrent `id` for container metadata → JSON
/// {duration_secs,codec,sample_rate,channels}. Reads only the header
/// region (the probe's reads pull piece 0 on demand). Null on failure —
/// caller keeps the row without duration.
/// Free with lyra_string_free.
#[no_mangle]
pub extern "C" fn lyra_torrent_probe(id: c_int, file_idx: c_int) -> *mut c_char {
    let eng = match torrent() {
        Ok(e) => e,
        Err(_) => return std::ptr::null_mut(),
    };
    let ext = eng
        .files(id as usize)
        .ok()
        .and_then(|fs| fs.into_iter().find(|f| f.index == file_idx as usize))
        .and_then(|f| {
            std::path::Path::new(&f.path)
                .extension()
                .and_then(|s| s.to_str())
                .map(String::from)
        })
        .unwrap_or_default();
    let src = match eng.open_file(id as usize, file_idx as usize) {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };
    let media = lyra_fs::SourceMediaSource::new(lyra_fs::CachingSource::wrap(src));
    let format = lyra_formats::format_from_ext(&ext);
    match lyra_formats::stream_info_media(media, format, &ext) {
        Ok(info) => {
            let j = serde_json::json!({
                "duration_secs": info.duration_secs,
                "codec": info.codec,
                "sample_rate": info.sample_rate,
                "channels": info.channels,
            });
            CString::new(j.to_string()).unwrap().into_raw()
        }
        Err(_) => std::ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// Rust VizFrame must byte-match the C LyraVizFrame in lyra.h —
    /// Swift parses raw field offsets, so a drift here is silent corruption.
    #[test]
    fn viz_frame_abi_layout() {
        type F = lyra_engine::VizFrame;
        assert_eq!(offset_of!(F, bands), 0);
        assert_eq!(offset_of!(F, wave_l), 256);
        assert_eq!(offset_of!(F, wave_r), 1280);
        assert_eq!(offset_of!(F, peak), 2304);
        assert_eq!(offset_of!(F, rms), 2312);
        assert_eq!(offset_of!(F, bass), 2320);
        assert_eq!(offset_of!(F, beat), 2324);
        assert_eq!(offset_of!(F, level), 2328);
        assert_eq!(offset_of!(F, clip), 2332);
        assert_eq!(offset_of!(F, seq), 2336);
        assert_eq!(size_of::<F>(), 2344);
    }

    #[test]
    fn viz_frame_null_safe() {
        let mut f = lyra_engine::VizFrame::default();
        assert_eq!(
            unsafe { lyra_engine_viz_frame(std::ptr::null(), &mut f) },
            0
        );
        let seq = unsafe { lyra_engine_viz_frame(std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(seq, 0);
    }

    /// Live-network smoke: archive.org etree query through the FFI.
    /// Ignored — `cargo test -p lyra-ffi search_live -- --ignored`.
    #[test]
    #[ignore]
    fn search_live_archive_org() {
        let dir = std::env::temp_dir().join(format!("lyra-search-{}", std::process::id()));
        let dc = CString::new(dir.to_str().unwrap()).unwrap();
        let s = unsafe { lyra_search_new(dc.as_ptr()) };
        assert!(!s.is_null());

        // Object form — what the app sends (strict lossless default).
        let q = CString::new(r#"{"text":"grateful dead","strict":true}"#).unwrap();
        let raw = unsafe { lyra_search(s, q.as_ptr()) };
        assert!(!raw.is_null());
        let body = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
        unsafe { lyra_string_free(raw) };
        println!("results: {}", &body[..body.len().min(400)]);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v.get("results").is_some(), "no results key: {body}");
        assert!(
            v["results"]
                .as_array()
                .map(|r| !r.is_empty())
                .unwrap_or(false),
            "empty results: {body}"
        );

        // Bare-text and garbage input must not crash — both return JSON.
        let q = CString::new("not json at all").unwrap();
        let raw = unsafe { lyra_search(s, q.as_ptr()) };
        let body = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
        unsafe { lyra_string_free(raw) };
        assert!(body.contains("error"), "expected error json: {body}");

        unsafe { lyra_search_free(s) };
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Live Torznab smoke: register a Jackett/Prowlarr endpoint, search,
    /// resolve the first row. Endpoint comes from env:
    ///   LYRA_TORZNAB_URL / LYRA_TORZNAB_KEY
    /// `cargo test -p lyra-ffi search_live_torznab -- --ignored`.
    #[test]
    #[ignore]
    fn search_live_torznab() {
        let url = std::env::var("LYRA_TORZNAB_URL").expect("LYRA_TORZNAB_URL not set");
        let key = std::env::var("LYRA_TORZNAB_KEY").unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("lyra-search-{}", std::process::id()));
        let dc = CString::new(dir.to_str().unwrap()).unwrap();
        let s = unsafe { lyra_search_new(dc.as_ptr()) };
        assert!(!s.is_null());

        let ep = serde_json::json!([{"url": url, "apikey": key, "name": "jackett"}]);
        let ec = CString::new(ep.to_string()).unwrap();
        let n = unsafe { lyra_search_sync_torznab(s, ec.as_ptr()) };
        assert_eq!(n, 1, "endpoint registration failed");

        let q = CString::new(r#"{"text":"aerosmith dream on","strict":false}"#).unwrap();
        let raw = unsafe { lyra_search(s, q.as_ptr()) };
        let body = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
        unsafe { lyra_string_free(raw) };
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let rows = v["results"].as_array().cloned().unwrap_or_default();
        println!("results: {} errors: {}", rows.len(), v["provider_errors"]);
        assert!(!rows.is_empty(), "no rows: {body}");
        let torznab = rows
            .iter()
            .find(|r| r["provider"].as_str().unwrap_or("").starts_with("torznab"));
        let row = torznab.unwrap_or(&rows[0]).clone();
        println!("row: {}", serde_json::to_string_pretty(&row).unwrap());

        // Resolve — must yield an addable magnet/url.
        let rc = CString::new(row.to_string()).unwrap();
        let raw = unsafe { lyra_search_resolve(s, rc.as_ptr()) };
        let body = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
        unsafe { lyra_string_free(raw) };
        println!("resolved: {}", &body[..body.len().min(600)]);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v.get("addable").is_some(), "no addable: {body}");

        unsafe { lyra_search_free(s) };
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ── Torrent search (lyra-search) ──────────────────────────────────────
// Lossless-first search over legal indexes (archive.org etree scope,
// Academic Torrents). Opaque handle + private runtime — same block_on
// pattern as TorrentEngine. Calls block; Swift runs them off-main.

pub struct LyraSearch {
    rt: tokio::runtime::Runtime,
    engine: lyra_search::SearchEngine,
}

/// `data_dir` backs provider caches (AT database.xml). Created if
/// missing. Null on failure.
///
/// # Safety
/// `data_dir` may be null (returns null); otherwise it must point to a valid NUL-terminated C string.
/// A living handle must be freed exactly once with `lyra_search_free` and never used afterwards.
#[no_mangle]
pub unsafe extern "C" fn lyra_search_new(data_dir: *const c_char) -> *mut LyraSearch {
    if data_dir.is_null() {
        return std::ptr::null_mut();
    }
    init_logging();
    let dir = match unsafe { CStr::from_ptr(data_dir) }.to_str() {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => return std::ptr::null_mut(),
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return std::ptr::null_mut();
    }
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("search rt: {e}");
            return std::ptr::null_mut();
        }
    };
    let engine = lyra_search::SearchEngine::new(vec![
        std::sync::Arc::new(lyra_search::ArchiveOrgProvider::new()),
        std::sync::Arc::new(lyra_search::AcademicTorrentsProvider::new(dir)),
        std::sync::Arc::new(lyra_search::X1337Provider::new()),
        std::sync::Arc::new(lyra_search::ApibayProvider::new()),
        std::sync::Arc::new(lyra_search::KnabenProvider::new()),
        std::sync::Arc::new(lyra_search::TorrentsCsvProvider::new()),
        std::sync::Arc::new(lyra_search::NyaaProvider::new()),
        std::sync::Arc::new(lyra_search::SolidTorrentsProvider::new()),
    ]);
    Box::into_raw(Box::new(LyraSearch { rt, engine }))
}

/// Replace the External-tier provider set from a JSON array of
/// [{url,apikey,name?}] — user-managed Torznab endpoints (Jackett,
/// Prowlarr). One call applies the whole list, so deletions propagate.
/// Returns the registered count, −1 bad json/null.
///
/// # Safety
/// `s` must be a live search handle or null (null returns -1).
/// `endpoints_json` must be null or a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn lyra_search_sync_torznab(
    s: *mut LyraSearch,
    endpoints_json: *const c_char,
) -> c_int {
    if s.is_null() || endpoints_json.is_null() {
        return -1;
    }
    let raw = unsafe { CStr::from_ptr(endpoints_json) }
        .to_str()
        .unwrap_or_default();
    #[derive(serde::Deserialize)]
    struct Ep {
        url: String,
        apikey: Option<String>,
        name: Option<String>,
    }
    let eps: Vec<Ep> = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return -1,
    };
    let set: Vec<std::sync::Arc<dyn lyra_search::TorrentProvider>> = eps
        .into_iter()
        .filter(|e| e.url.starts_with("http"))
        .map(|e| {
            std::sync::Arc::new(lyra_search::TorznabProvider::new(
                e.url,
                e.apikey.unwrap_or_default(),
                e.name.as_deref(),
            )) as std::sync::Arc<dyn lyra_search::TorrentProvider>
        })
        .collect();
    let n = set.len() as c_int;
    let s = unsafe { &*s };
    s.engine.sync_external(set);
    n
}

/// Shut down the search runtime and free the handle.
///
/// # Safety
/// `s` must be a pointer returned by `lyra_search_new` (or null), freed at most once and never used afterwards.
#[no_mangle]
pub unsafe extern "C" fn lyra_search_free(s: *mut LyraSearch) {
    if !s.is_null() {
        drop(unsafe { Box::from_raw(s) });
    }
}

/// `query_json`: a SearchQuery object ({"text":…,"strict":…,"formats":…})
/// or a bare JSON string → text. Returns SearchResponse JSON
/// {results:[…], provider_errors:[…]} — free with lyra_string_free.
/// Blocks; call off the main thread.
///
/// # Safety
/// `s` must be a live search handle or null (null returns null).
/// `query_json` must be null or a valid NUL-terminated C string.
/// Free a non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_search(s: *mut LyraSearch, query_json: *const c_char) -> *mut c_char {
    if s.is_null() {
        return std::ptr::null_mut();
    }
    let raw = unsafe { CStr::from_ptr(query_json) }
        .to_str()
        .unwrap_or_default();
    let Some(q) = lyra_search::SearchQuery::from_json(raw) else {
        return CString::new(r#"{"error":"bad query json"}"#)
            .unwrap_or_default()
            .into_raw();
    };
    let s = unsafe { &*s };
    let resp = s.rt.block_on(s.engine.search(&q));
    CString::new(serde_json::json!(resp).to_string())
        .unwrap_or_default()
        .into_raw()
}

/// `result_json`: a SearchResult object from a prior lyra_search call.
/// Returns ResolvedTorrent JSON {result, files, addable:{kind:magnet|
/// torrent_url|torrent_b64, …}} or {"error":…}. Blocks; call off-main.
///
/// # Safety
/// `s` must be a live search handle or null (null returns null).
/// `result_json` must be null or a valid NUL-terminated C string.
/// Free a non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_search_resolve(
    s: *mut LyraSearch,
    result_json: *const c_char,
) -> *mut c_char {
    if s.is_null() {
        return std::ptr::null_mut();
    }
    let raw = unsafe { CStr::from_ptr(result_json) }
        .to_str()
        .unwrap_or_default();
    let r: lyra_search::SearchResult = match serde_json::from_str(raw) {
        Ok(r) => r,
        Err(_) => {
            return CString::new(r#"{"error":"bad result json"}"#)
                .unwrap_or_default()
                .into_raw()
        }
    };
    let s = unsafe { &*s };
    match s.rt.block_on(s.engine.resolve(&r)) {
        Ok(res) => CString::new(serde_json::json!(res).to_string())
            .unwrap_or_default()
            .into_raw(),
        Err(e) => CString::new(serde_json::json!({"error": e.to_string()}).to_string())
            .unwrap_or_default()
            .into_raw(),
    }
}

#[cfg(test)]
mod live_new_providers {
    use crate::{lyra_search, lyra_search_free, lyra_search_new, lyra_string_free};
    use std::ffi::{CStr, CString};

    #[test]
    #[ignore]
    fn search_live_new_indexes() {
        let dir = std::env::temp_dir().join(format!("lyra-search-{}", std::process::id()));
        let dc = CString::new(dir.to_str().unwrap()).unwrap();
        let s = unsafe { lyra_search_new(dc.as_ptr()) };
        assert!(!s.is_null());
        let q = CString::new(r#"{"text":"aerosmith dream on","strict":false}"#).unwrap();
        let raw = unsafe { lyra_search(s, q.as_ptr()) };
        let body = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
        unsafe { lyra_string_free(raw) };
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let rows = v["results"].as_array().cloned().unwrap_or_default();
        let mut by: std::collections::BTreeMap<String, usize> = Default::default();
        for r in &rows {
            *by.entry(r["provider"].as_str().unwrap_or("?").to_string())
                .or_default() += 1;
        }
        println!("providers: {:?}", by);
        for e in v["provider_errors"].as_array().cloned().unwrap_or_default() {
            println!(
                "err: {} -> {}",
                e["provider"],
                e["error"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(90)
                    .collect::<String>()
            );
        }
        unsafe { lyra_search_free(s) };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
