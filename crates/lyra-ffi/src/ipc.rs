//! Live IPC bridge — implements lyra-ipc's `Dispatcher` over the real
//! engine, store, and torrent session. The app starts it via
//! `lyra_ipc_start`; `lyra` CLI and `lyra-mcp` then drive the running app.
//!
//! Split of labor:
//! - **Direct Rust-side**: transport, volume, EQ, spectrum, library
//!   search/stats/scan, torrent.add — these act on Engine/Store/TorrentEngine.
//! - **Drained by the VM**: track.play, queue.*, next/prev, shuffle/repeat —
//!   playback identity lives in the Swift view model, so these land in a
//!   command slot the app drains on its poll and executes with full UI
//!   coherence. `lyra_ipc_publish_state` is the reverse edge: the VM pushes
//!   now-playing/queue/mode JSON so `state.get` reflects the real UI state.

use lyra_ipc::paths;
use lyra_ipc::protocol::ErrorCode;
use lyra_ipc::{ApiError, DispatchCtx, Dispatcher, Server, ServerHandle};
use serde_json::{json, Value};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use crate::torrent;

fn engine() -> Option<&'static lyra_engine::Engine> {
    let p = crate::lyra_engine_current();
    if p.is_null() {
        None
    } else {
        Some(unsafe { &*p })
    }
}

fn bump(rev: &AtomicI64) -> i64 {
    rev.fetch_add(1, Ordering::SeqCst) + 1
}

fn need_engine() -> Result<&'static lyra_engine::Engine, ApiError> {
    engine().ok_or_else(|| ApiError::new(ErrorCode::NotRunning, "engine not initialized"))
}

/// State shared between the Server's dispatcher and the publish/drain FFI
/// — kept in its own Arc so Server can own the dispatcher by value.
pub(crate) struct Shared {
    revision: AtomicI64,
    /// VM-published UI state: {track, queue, shuffle, repeat,
    /// playlist_revision, duration, speed}. Read on every snapshot.
    published: RwLock<Value>,
    /// UI-bound ops drained by `lyra_ipc_drain_commands`.
    commands: Mutex<Vec<Value>>,
}

/// The live backend. `store` is a second Library connection on the app's
/// sqlite — WAL tolerates the extra writer.
pub struct LiveDispatcher {
    shared: std::sync::Arc<Shared>,
    store: Mutex<Option<lyra_store::Library>>,
}

impl LiveDispatcher {
    /// `Shared` stays crate-internal — hosts come up through `lyra_ipc_start`,
    /// which builds the pair together, so `new` doesn't leak the type.
    pub(crate) fn new(db_path: PathBuf, shared: std::sync::Arc<Shared>) -> Self {
        let store = lyra_store::Library::open(&db_path)
            .map_err(|e| tracing::warn!("ipc: store open {db_path:?}: {e}"))
            .ok();
        Self {
            shared,
            store: Mutex::new(store),
        }
    }

    fn with_store<T>(
        &self,
        f: impl FnOnce(&lyra_store::Library) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let g = self.store.lock().unwrap();
        let lib = g
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::NotRunning, "library store unavailable"))?;
        f(lib)
    }

    fn ui_command(&self, op: &str, params: Value) -> Value {
        self.shared
            .commands
            .lock()
            .unwrap()
            .push(json!({"op": op, "params": params}));
        json!({"accepted": true, "routed": "ui"})
    }
}

fn track_json(t: &lyra_core::LibraryTrack) -> Value {
    json!({
        "id": t.path,
        "path": t.path,
        "title": t.title,
        "artist": t.artist,
        "album": t.album,
        "album_artist": t.album_artist,
        "duration": t.duration_secs,
        "format": format!("{:?}", t.format).to_lowercase(),
        "codec": t.codec,
        "sample_rate": t.sample_rate,
        "bits": t.bits_per_sample,
    })
}

impl Dispatcher for LiveDispatcher {
    fn snapshot(&self) -> Value {
        let pub_ = self.shared.published.read().unwrap().clone();
        let (playing, position, volume, bands) = match engine() {
            Some(e) => {
                let mut bands = [0f32; 32];
                e.viz_bands(&mut bands);
                (
                    e.is_playing(),
                    e.position_secs() as f64,
                    e.volume() as f64,
                    bands.to_vec(),
                )
            }
            None => (false, 0.0, 1.0, vec![0.0; 32]),
        };
        let eq = engine()
            .map(|e| {
                e.eq_specs()
                    .iter()
                    .map(|s| match s {
                        Some(b) => json!({"freq": b.freq_hz, "q": b.q, "gain_db": b.gain_db}),
                        None => Value::Null,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let state = if playing {
            "playing"
        } else if engine().map(|e| e.can_resume()).unwrap_or(false) {
            "paused"
        } else {
            "stopped"
        };
        json!({
            "version": 2,
            "revision": self.shared.revision.load(Ordering::SeqCst),
            "playlist_revision": pub_.get("playlist_revision").cloned().unwrap_or(json!(1)),
            "state": state,
            "position": position,
            "duration": pub_.get("duration").cloned().unwrap_or(Value::Null),
            "seekable": true,
            "volume": volume,
            "speed": pub_.get("speed").cloned().unwrap_or(json!(1.0)),
            "shuffle": pub_.get("shuffle").cloned().unwrap_or(json!(false)),
            "repeat": pub_.get("repeat").cloned().unwrap_or(json!("off")),
            "track": pub_.get("track").cloned().unwrap_or(Value::Null),
            "queue": pub_.get("queue").cloned().unwrap_or(json!({"index":0,"length":0})),
            "eq": {"bands": eq, "preamp": 0.0},
            "viz": {"bands": bands},
            "audio": {"device": pub_.get("device").cloned().unwrap_or(json!("default"))},
            "stream_error": pub_.get("stream_error").cloned().unwrap_or(Value::Null),
        })
    }

    fn call(&self, ctx: &DispatchCtx, method: &str, params: &Value) -> Result<Value, ApiError> {
        let rev = &self.shared.revision;
        match method {
            "play" => {
                need_engine()?.resume();
                bump(rev);
                Ok(json!({"state": "playing"}))
            }
            "pause" => {
                need_engine()?.pause();
                bump(rev);
                Ok(json!({"state": "paused"}))
            }
            "toggle" => {
                let e = need_engine()?;
                if e.is_playing() {
                    e.pause()
                } else {
                    e.resume()
                }
                bump(rev);
                Ok(json!({"state": if e.is_playing() { "playing" } else { "paused" }}))
            }
            "stop" => {
                need_engine()?.stop();
                bump(rev);
                Ok(json!({"state": "stopped"}))
            }
            // Ops the VM executes via the command drain — the library table
            // IS the queue, so these need the app's track list.
            "next" | "prev" | "track.play" | "queue.play" => {
                Ok(self.ui_command(method, params.clone()))
            }
            // Not bridged yet — no separate queue model / URL source /
            // device switch / speed control in the app today.
            "queue.enqueue" | "queue.remove" | "queue.move" | "queue.clear" | "track.queue"
            | "url.load" | "device.set" | "speed" | "shuffle" | "repeat" => Err(
                ApiError::invalid_param(format!("{method} not bridged in this build")),
            ),
            "seek.absolute" => {
                let pos = params
                    .get("position_s")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| ApiError::invalid_param("seek.absolute needs position_s"))?;
                need_engine()?.seek(pos);
                bump(rev);
                Ok(json!({"position": pos}))
            }
            "seek.relative" => {
                let d = params
                    .get("delta_s")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| ApiError::invalid_param("seek.relative needs delta_s"))?;
                let e = need_engine()?;
                e.seek((e.position_secs() as f64 + d).max(0.0));
                bump(rev);
                Ok(json!({"position": e.position_secs()}))
            }
            "volume" | "volume.set" => {
                let v = params
                    .get("volume")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| ApiError::invalid_param("volume.set needs volume"))?;
                need_engine()?.set_volume(v as f32);
                bump(rev);
                Ok(json!({"volume": v}))
            }
            "eq.get" => {
                let specs = need_engine()?.eq_specs();
                Ok(json!({"bands": specs.iter().map(|s| match s {
                    Some(b) => json!({"freq": b.freq_hz, "q": b.q, "gain_db": b.gain_db}),
                    None => Value::Null,
                }).collect::<Vec<_>>()}))
            }
            "eq.set" => {
                let e = need_engine()?;
                let bands = params
                    .get("bands")
                    .and_then(Value::as_array)
                    .ok_or_else(|| ApiError::invalid_param("eq.set needs bands[]"))?;
                let mut specs = e.eq_specs();
                for (i, g) in bands.iter().enumerate().take(specs.len()) {
                    let gain = g.as_f64().unwrap_or(0.0) as f32;
                    let mut spec = specs[i].unwrap_or(lyra_engine::BandSpec {
                        freq_hz: 0.0,
                        q: 1.0,
                        gain_db: 0.0,
                        peaking: true,
                    });
                    spec.gain_db = gain;
                    e.set_band(i, spec);
                    specs[i] = Some(spec);
                }
                bump(rev);
                Ok(json!({"bands": bands.len()}))
            }
            "eq.band.set" => {
                let i = params
                    .get("band")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| ApiError::invalid_param("eq.band.set needs band"))?
                    as usize;
                let gain = params
                    .get("gain_db")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| ApiError::invalid_param("eq.band.set needs gain_db"))?
                    as f32;
                let e = need_engine()?;
                let specs = e.eq_specs();
                if i >= specs.len() {
                    return Err(ApiError::invalid_param(format!("band {i} out of range")));
                }
                let mut spec = specs[i].unwrap_or(lyra_engine::BandSpec {
                    freq_hz: 0.0,
                    q: 1.0,
                    gain_db: 0.0,
                    peaking: true,
                });
                spec.gain_db = gain;
                e.set_band(i, spec);
                bump(rev);
                Ok(json!({"band": i, "gain_db": gain}))
            }
            "spectrum.get" => {
                let e = need_engine()?;
                let mut bands = [0f32; 32];
                e.viz_bands(&mut bands);
                Ok(json!({"bands": bands}))
            }
            "queue.list" => {
                let pub_ = self.shared.published.read().unwrap();
                Ok(json!({"queue": pub_.get("queue").cloned().unwrap_or(json!([]))}))
            }

            "library.search" => {
                let q = params.get("q").and_then(Value::as_str).unwrap_or("");
                let limit = params.get("limit").and_then(Value::as_i64).unwrap_or(50) as usize;
                let hits = self.with_store(|lib| {
                    lib.search(q)
                        .map(|v| {
                            v.into_iter()
                                .take(limit)
                                .map(|t| track_json(&t))
                                .collect::<Vec<_>>()
                        })
                        .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))
                })?;
                Ok(json!({"results": hits}))
            }
            "library.stats" => self.with_store(|lib| {
                let tracks = lib
                    .all_tracks()
                    .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;
                let sources = lib
                    .sources()
                    .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;
                Ok(json!({"tracks": tracks.len(), "sources": sources.len()}))
            }),
            "library.scan" => {
                // Async op — the job worker calls this off the conn loop.
                let path = params
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ApiError::invalid_param("library.scan needs path"))?;
                let stats = self.with_store(|lib| {
                    lib.sync_dir(std::path::Path::new(path))
                        .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))
                })?;
                ctx.emit("library.scan", json!({"state": "succeeded", "path": path}));
                // The app re-reads its table on this UI command.
                self.ui_command("library.reload", json!({}));
                Ok(json!({"walked": stats.walked, "probed": stats.probed, "pruned": stats.pruned}))
            }
            "torrent.add" => {
                let spec = params
                    .get("magnet")
                    .or_else(|| params.get("path"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| ApiError::invalid_param("torrent.add needs magnet or path"))?;
                match torrent() {
                    Ok(e) => match e.add(spec) {
                        Ok(id) => {
                            self.ui_command("torrent.added", json!({"id": id}));
                            Ok(json!({"id": id}))
                        }
                        Err(e) => Err(ApiError::new(ErrorCode::Internal, format!("add: {e}"))),
                    },
                    Err(_) => Err(ApiError::new(ErrorCode::NotRunning, "torrent engine off")),
                }
            }
            "device.list" => Ok(json!({
                "devices": ["default"],
                "current": "default",
            })),
            "lyrics.get" => Err(ApiError::not_found("no lyrics store yet")),
            "plugin.call" => Err(ApiError::not_found("no plugins installed")),
            _ => Err(ApiError::new(
                ErrorCode::UnknownMethod,
                format!("unhandled op {method}"),
            )),
        }
    }
}

// ── FFI surface ──────────────────────────────────────────────────────────

static SHARED: OnceLock<std::sync::Arc<Shared>> = OnceLock::new();
static SERVER: Mutex<Option<ServerHandle>> = Mutex::new(None);

/// Start the IPC server. `db_path` = the app's library.db; `sock_dir` = the
/// directory to bind control.sock in (app container's Application Support
/// dir — sandboxed apps can't write the group-container path, but the CLI's
/// candidate list already looks inside the container). 0 ok.
///
/// # Safety
/// `db_path` and `sock_dir` must be non-null and point to valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn lyra_ipc_start(
    db_path: *const c_char,
    sock_dir: *const c_char,
) -> std::os::raw::c_int {
    crate::init_logging();
    let db = match unsafe { CStr::from_ptr(db_path) }.to_str() {
        Ok(p) => PathBuf::from(p),
        Err(_) => return 2,
    };
    let dir = match unsafe { CStr::from_ptr(sock_dir) }.to_str() {
        Ok(p) => PathBuf::from(p),
        Err(_) => return 2,
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::error!("ipc: mkdir {dir:?}: {e}");
        return 3;
    }
    let shared = std::sync::Arc::new(Shared {
        revision: AtomicI64::new(1),
        published: RwLock::new(json!({})),
        commands: Mutex::new(Vec::new()),
    });
    let _ = SHARED.set(std::sync::Arc::clone(&shared));
    match Server::new(LiveDispatcher::new(db, shared)).serve_on_path(&dir.join(paths::SOCKET_NAME))
    {
        Ok(h) => {
            *SERVER.lock().unwrap() = Some(h);
            tracing::info!("ipc: listening at {}/{}", dir.display(), paths::SOCKET_NAME);
            0
        }
        Err(e) => {
            tracing::error!("ipc: bind: {e}");
            4
        }
    }
}

/// Stop the IPC server and unbind the socket.
///
/// # Safety
/// Always safe to call; a no-op when the server was never started.
#[no_mangle]
pub unsafe extern "C" fn lyra_ipc_stop() {
    if let Some(h) = SERVER.lock().unwrap().take() {
        h.shutdown();
    }
}

/// VM → dispatcher: publish {track, queue, shuffle, repeat, speed, duration,
/// playlist_revision, device, stream_error}. Merged into every snapshot.
///
/// # Safety
/// `json_str` must be non-null and point to a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn lyra_ipc_publish_state(json_str: *const c_char) -> std::os::raw::c_int {
    let Some(d) = SHARED.get() else { return 1 };
    let s = match unsafe { CStr::from_ptr(json_str) }.to_str() {
        Ok(s) => s,
        Err(_) => return 2,
    };
    match serde_json::from_str::<Value>(s) {
        Ok(v) => {
            *d.published.write().unwrap() = v;
            0
        }
        Err(_) => 3,
    }
}

/// VM ← dispatcher: drain pending UI commands as a JSON array. The app
/// polls this and executes queue/track ops with full UI coherence.
///
/// # Safety
/// Always safe to call; returns null when the server never started or no commands are pending.
/// Free a non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_ipc_drain_commands() -> *mut c_char {
    let Some(d) = SHARED.get() else {
        return std::ptr::null_mut();
    };
    let cmds: Vec<Value> = std::mem::take(&mut *d.commands.lock().unwrap());
    if cmds.is_empty() {
        return std::ptr::null_mut();
    }
    CString::new(Value::Array(cmds).to_string())
        .unwrap()
        .into_raw()
}
