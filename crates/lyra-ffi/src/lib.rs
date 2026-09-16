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

mod coach;
mod ipc;
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
    let old = CURRENT_ENGINE.swap(e as usize, Ordering::SeqCst)
        as *mut lyra_engine::Engine;
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

/// Copy the latest viz frame into `out` — the 60 Hz path: short lock,
/// memcpy only, no math. Returns seq; Swift skips redraw when seq is
/// unchanged. Null-safe: returns 0 without touching `out`.
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_viz_frame(
    e: *const lyra_engine::Engine,
    out: *mut lyra_engine::VizFrame,
) -> u64 {
    if e.is_null() || out.is_null() {
        return 0;
    }
    unsafe { &*e }.viz_frame(&mut *out)
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

/// ── Library DB (lyra-store) ─────────────────────────────────────────────
/// Opaque handle. Open once at app start; the library persists across
/// launches — rescan only re-probes mtime-changed files.

/// Open/create the library DB at `path`. Null on failure.
#[no_mangle]
pub extern "C" fn lyra_lib_open(path: *const c_char) -> *mut lyra_store::Library {
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

#[no_mangle]
pub unsafe extern "C" fn lyra_lib_free(l: *mut lyra_store::Library) {
    if !l.is_null() {
        drop(Box::from_raw(l));
    }
}

/// Shutdown + free the live engine. Safe on null; `e` kept for ABI
/// symmetry with the other lyra_engine_* calls.
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_free(e: *mut lyra_engine::Engine) {
    let _g = ENGINE_LOCK.lock().unwrap();
    let cur = CURRENT_ENGINE.swap(0, Ordering::SeqCst) as *mut lyra_engine::Engine;
    let p = if cur.is_null() { e } else { cur };
    if !p.is_null() {
        unsafe { &*p }.shutdown();
        drop(Box::from_raw(p));
    }
}

// ── Remote control ───────────────────────────────────────────────────────
// One global Host; commands route into the engine via EngineSink.

static REMOTE: std::sync::OnceLock<
    Result<std::sync::Arc<lyra_remote::Host>, String>,
> = std::sync::OnceLock::new();

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
                if e.is_playing() { e.pause() } else { e.resume() }
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

static TORRENT: std::sync::OnceLock<
    Result<std::sync::Arc<lyra_torrent::TorrentEngine>, String>,
> = std::sync::OnceLock::new();

pub(crate) fn torrent() -> Result<&'static std::sync::Arc<lyra_torrent::TorrentEngine>, c_int> {
    match TORRENT.get() {
        Some(Ok(e)) => Ok(e),
        Some(Err(_)) => Err(1),
        None => Err(2), // not initialized
    }
}

/// Initialize the torrent session. Idempotent — first call wins. 0 ok.
#[no_mangle]
pub extern "C" fn lyra_torrent_init(download_dir: *const c_char) -> c_int {
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
#[no_mangle]
pub extern "C" fn lyra_torrent_add(spec: *const c_char) -> c_int {
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
        assert_eq!(unsafe { lyra_engine_viz_frame(std::ptr::null(), &mut f) }, 0);
        let seq = unsafe {
            lyra_engine_viz_frame(std::ptr::null_mut(), std::ptr::null_mut())
        };
        assert_eq!(seq, 0);
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
#[no_mangle]
pub extern "C" fn lyra_search_new(data_dir: *const c_char) -> *mut LyraSearch {
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
    ]);
    Box::into_raw(Box::new(LyraSearch { rt, engine }))
}

#[no_mangle]
pub unsafe extern "C" fn lyra_search_free(s: *mut LyraSearch) {
    if !s.is_null() {
        drop(Box::from_raw(s));
    }
}

/// `query_json`: a SearchQuery object ({"text":…,"strict":…,"formats":…})
/// or a bare JSON string → text. Returns SearchResponse JSON
/// {results:[…], provider_errors:[…]} — free with lyra_string_free.
/// Blocks; call off the main thread.
#[no_mangle]
pub unsafe extern "C" fn lyra_search(s: *mut LyraSearch, query_json: *const c_char) -> *mut c_char {
    if s.is_null() {
        return std::ptr::null_mut();
    }
    let raw = unsafe { CStr::from_ptr(query_json) }.to_str().unwrap_or_default();
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
#[no_mangle]
pub unsafe extern "C" fn lyra_search_resolve(
    s: *mut LyraSearch,
    result_json: *const c_char,
) -> *mut c_char {
    if s.is_null() {
        return std::ptr::null_mut();
    }
    let raw = unsafe { CStr::from_ptr(result_json) }.to_str().unwrap_or_default();
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
