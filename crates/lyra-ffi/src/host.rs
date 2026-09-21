//! Shared runtime for Lyra hosts that aren't the SwiftUI shell —
//! `lyrad` (headless) and `lyra-ui` (GTK) both boot engine + store +
//! torrents + IPC through this so the wire-visible behavior (`lyra` CLI
//! surface, published state shape, queue semantics) is identical no
//! matter which host is running.
//!
//! Everything here runs on the host's main thread; the FFI calls are the
//! same `extern "C"` functions the app invokes from Swift.

use serde_json::{json, Value};
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};

/// What a host needs at boot. `socket` is the IPC control socket path;
/// `remote_port` starts the LAN remote (SPAKE2 + Noise) when set.
pub struct HostArgs {
    pub data_dir: PathBuf,
    pub socket: PathBuf,
    pub remote_port: Option<u16>,
}

/// Platform data dir — mirrors where the app keeps library.db/artwork:
/// `~/Library/Application Support/Lyra` on macOS, `$XDG_DATA_HOME/lyra`
/// (else `~/.local/share/lyra`) on Linux.
pub fn default_data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join("Library/Application Support/Lyra");
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            return PathBuf::from(xdg).join("lyra");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".local/share/lyra");
        }
    }
    std::env::temp_dir().join("lyra")
}

/// CString arg for the FFI calls; that layer takes CStr-style pointers.
pub fn cstr(s: &Path) -> CString {
    CString::new(s.to_string_lossy().as_bytes()).unwrap_or_default()
}

/// Read and free a `char*` returned by the FFI layer.
///
/// # Safety
/// `p` must be null or a string returned by a `lyra_*` FFI function.
pub unsafe fn take_string(p: *mut std::ffi::c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { crate::lyra_string_free(p) };
    Some(s)
}

/// The live host: ordered queue = the library table, drained UI ops
/// executed against it, state published on change — the VM's role in
/// `ContentView.swift`, without Swift.
pub struct Host {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub socket: PathBuf,
    pub tracks: Vec<lyra_core::LibraryTrack>,
    /// Track id of what's loaded == its path (see `track_json` in ipc.rs).
    pub current: Option<String>,
    /// True when the audio device opened; transport ops warn+no-op otherwise.
    pub engine_ok: bool,
    lib: Option<lyra_store::Library>,
    last_sig: String,
}

impl Host {
    /// mkdir data_dir, boot engine (compat path — cpal/ALSA on Linux),
    /// torrent session, IPC server, optional remote. IPC bind failure is
    /// fatal (returned as Err); a missing output device is not — IPC and
    /// library ops still work, same posture as the app.
    pub fn boot(args: &HostArgs) -> std::io::Result<Host> {
        std::fs::create_dir_all(&args.data_dir)?;
        let db = args.data_dir.join("library.db");
        let downloads = args.data_dir.join("torrents");
        let sock_dir = args
            .socket
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));

        let engine = crate::lyra_engine_new();
        let engine_ok = !engine.is_null();
        if !engine_ok {
            tracing::warn!("no output device — serving library/IPC without audio");
        }
        if unsafe { crate::lyra_torrent_init(cstr(&downloads).as_ptr()) } != 0 {
            tracing::warn!("torrent engine unavailable");
        }
        let rc =
            unsafe { crate::ipc::lyra_ipc_start(cstr(&db).as_ptr(), cstr(&sock_dir).as_ptr()) };
        if rc != 0 {
            return Err(std::io::Error::other(format!(
                "ipc bind at {} failed ({rc})",
                args.socket.display()
            )));
        }
        tracing::info!("socket {}", args.socket.display());
        tracing::info!("library {}", db.display());

        if let Some(port) = args.remote_port {
            if engine_ok {
                let key = cstr(&args.data_dir.join("remote-key.bin"));
                if unsafe { crate::lyra_remote_init(engine, key.as_ptr()) } == 0
                    && crate::lyra_remote_start(port) == 0
                {
                    tracing::info!("remote on :{port}");
                } else {
                    tracing::warn!("remote init/start failed");
                }
            } else {
                tracing::warn!("remote port ignored: remote needs the engine");
            }
        }

        let lib = lyra_store::Library::open(&db).ok();
        let tracks = lib
            .as_ref()
            .and_then(|l| l.all_tracks().ok())
            .unwrap_or_default();
        Ok(Host {
            data_dir: args.data_dir.clone(),
            db_path: db,
            socket: args.socket.clone(),
            lib,
            tracks,
            current: None,
            engine_ok,
            last_sig: String::new(),
        })
    }

    /// Live engine handle — null when no output device exists.
    pub fn engine(&self) -> *mut lyra_engine::Engine {
        crate::lyra_engine_current()
    }

    /// Play a local path through the FFI engine. Drained ops and UI picks
    /// both land here; id == path.
    pub fn play_path(&mut self, path: &str) {
        let e = self.engine();
        if e.is_null() {
            tracing::warn!("play: no engine (no output device?)");
            return;
        }
        if !Path::new(path).exists() {
            tracing::warn!("play: not a file: {path}");
            return;
        }
        let rc = cstr(Path::new(path));
        if unsafe { crate::lyra_engine_play_file(e, rc.as_ptr()) } == 0 {
            self.current = Some(path.to_string());
            tracing::info!("playing {path}");
        } else {
            tracing::warn!("play rejected: {path}");
        }
    }

    pub fn play_index(&mut self, i: usize) {
        if let Some(t) = self.tracks.get(i) {
            let p = t.path.clone();
            self.play_path(&p);
        }
    }

    /// Queue index of the current track, if it's in the library table.
    pub fn queue_index(&self) -> Option<usize> {
        self.current
            .as_ref()
            .and_then(|p| self.tracks.iter().position(|t| t.path == *p))
    }

    /// next wraps; prev clamps — single source for both hosts.
    pub fn step(&mut self, d: i64) {
        if self.tracks.is_empty() {
            return;
        }
        let i = match (self.queue_index(), d) {
            (Some(i), 1) => (i + 1) % self.tracks.len(),
            (Some(i), _) => i.saturating_sub(1).min(self.tracks.len() - 1),
            (None, _) => 0,
        };
        self.play_index(i);
    }

    pub fn reload(&mut self) {
        if let Some(lib) = &self.lib {
            self.tracks = lib.all_tracks().unwrap_or_default();
        }
    }

    /// FTS path — the app's search box semantics.
    pub fn search(&self, q: &str) -> Vec<lyra_core::LibraryTrack> {
        self.lib
            .as_ref()
            .and_then(|l| l.search(q).ok())
            .unwrap_or_default()
    }

    /// Sync a directory into library.db on a fresh connection — safe to
    /// call from a worker thread (rusqlite conns aren't `Send`).
    pub fn sync_dir_blocking(db: &Path, dir: &Path) -> Option<lyra_store::SyncStats> {
        lyra_store::Library::open(db)
            .ok()
            .and_then(|l| l.sync_dir(dir).ok())
    }

    /// Drain UI-bound ops the IPC dispatcher routed up and execute them —
    /// exactly what the app's `execIPC` pump does on its timer.
    pub fn drain_commands(&mut self) {
        let raw = unsafe { crate::ipc::lyra_ipc_drain_commands() };
        let Some(s) = (unsafe { take_string(raw) }) else {
            return;
        };
        let cmds: Vec<Value> = serde_json::from_str(&s).unwrap_or_default();
        for cmd in cmds {
            let op = cmd.get("op").and_then(Value::as_str).unwrap_or("");
            let params = cmd.get("params").cloned().unwrap_or(json!({}));
            match op {
                "track.play" => {
                    if let Some(id) = params.get("track_id").and_then(Value::as_str) {
                        self.play_path(id);
                    }
                }
                "queue.play" => {
                    if let Some(i) = params.get("index").and_then(Value::as_u64) {
                        self.play_index(i as usize);
                    }
                }
                "next" => self.step(1),
                "prev" => self.step(-1),
                "library.reload" => self.reload(),
                "torrent.added" => {
                    let id = params.get("id").and_then(Value::as_i64).unwrap_or(-1);
                    tracing::info!("torrent {id} added — library.scan the dir to index it");
                }
                _ => tracing::debug!("unhandled ui op {op}"),
            }
        }
    }

    /// Publish the app-shaped now-playing snapshot when (track, queue
    /// length, queue index) changed — the CLI's `lyra state` reads this.
    pub fn publish_state_if_changed(&mut self) {
        let cur_idx = self.queue_index();
        let sig = format!(
            "{}|{}|{}",
            self.current.as_deref().unwrap_or(""),
            self.tracks.len(),
            cur_idx.map(|i| i as i64).unwrap_or(-1)
        );
        if sig == self.last_sig {
            return;
        }
        self.last_sig = sig;
        let track = self.current_track().map(|t| {
            json!({"id": t.path, "path": t.path, "title": t.title,
                   "artist": t.artist, "album": t.album,
                   "duration": t.duration_secs, "codec": t.codec})
        });
        let items: Vec<Value> = self
            .tracks
            .iter()
            .take(500)
            .map(|t| {
                json!({"id": t.path, "title": t.title, "artist": t.artist,
                       "album": t.album, "duration": t.duration_secs})
            })
            .collect();
        let state = json!({
            "track": track,
            "duration": self.current_track().and_then(|t| t.duration_secs).unwrap_or(0.0),
            "queue": {"index": cur_idx.unwrap_or(0), "length": self.tracks.len(),
                      "items": items},
            "playlist_revision": self.tracks.len(),
            "device": "default",
        });
        if let Ok(s) = CString::new(state.to_string()) {
            unsafe { crate::ipc::lyra_ipc_publish_state(s.as_ptr()) };
        }
    }

    pub fn current_track(&self) -> Option<&lyra_core::LibraryTrack> {
        self.current
            .as_ref()
            .and_then(|p| self.tracks.iter().find(|t| &t.path == p))
    }

    /// Content-addressed artwork path, same layout the app composes:
    /// `<db dir>/artwork/<h[..2]>/<h>/<size>.jpg`.
    pub fn artwork_path(&self, hash: &str, size: u32) -> PathBuf {
        self.data_dir
            .join("artwork")
            .join(&hash[..2.min(hash.len())])
            .join(hash)
            .join(format!("{size}.jpg"))
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        unsafe { crate::ipc::lyra_ipc_stop() };
    }
}
