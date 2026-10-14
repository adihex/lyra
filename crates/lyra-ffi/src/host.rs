//! Shared runtime for Lyra hosts that aren't the SwiftUI shell —
//! `lyrad` (headless) and `lyra-ui` (GTK) both boot engine + store +
//! torrents + IPC through this so the wire-visible behavior (`lyra` CLI
//! surface, published state shape, queue semantics) is identical no
//! matter which host is running.
//!
//! Everything here runs on the host's main thread; the FFI calls are the
//! same `extern "C"` functions the app invokes from Swift.

use serde_json::{json, Value};
use std::borrow::Cow;
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

/// Strictly parse the host's torrent track ids, rejecting signs, extra
/// components and overflow before looking up cached session metadata.
fn torrent_track_id(uri: &str) -> Option<(usize, usize)> {
    let (id, index) = uri.strip_prefix("torrent://")?.split_once('/')?;
    let number = |s: &str| {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse::<usize>().ok()
    };
    Some((number(id)?, number(index)?))
}

/// Filename-derived metadata only: never open a file or probe a torrent stream
/// on the host/UI thread. Scanned library metadata remains authoritative.
fn torrent_track_metadata(
    uri: &str,
    file: &str,
    torrent_name: Option<&str>,
) -> lyra_core::LibraryTrack {
    let path = Path::new(file);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file)
        .trim();
    let digits = stem.bytes().take_while(u8::is_ascii_digit).count();
    let suffix = &stem[digits..];
    let title = suffix.trim_start_matches(['.', '-', '_', ' ', '\t']);
    let numbered = digits > 0 && title.len() < suffix.len() && !title.is_empty();
    let track_number = numbered.then(|| stem[..digits].parse().ok()).flatten();
    let album = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        .or_else(|| {
            torrent_name.filter(|name| Some(*name) != path.file_name().and_then(|s| s.to_str()))
        })
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let artist = album
        .and_then(|s| s.split_once(" - "))
        .or_else(|| torrent_name.and_then(|s| s.split_once(" - ")))
        .map(|(artist, _)| artist.trim())
        .filter(|s| !s.is_empty());
    let codec = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    lyra_core::LibraryTrack {
        path: uri.to_owned(),
        title: Some(if numbered { title } else { stem }.to_owned()),
        artist: artist.map(str::to_owned),
        album: album.map(str::to_owned),
        album_artist: artist.map(str::to_owned),
        genre: None,
        year: None,
        track_number,
        duration_secs: None,
        format: lyra_core::AudioFormat::from_extension(&codec),
        codec,
        sample_rate: None,
        channels: None,
        bits_per_sample: None,
        artwork_hash: None,
    }
}

/// SQLite ledger only: negative-cache rows and legacy empty hashes are not art.
fn cached_metadata_artwork(
    lib: &lyra_store::Library,
    artist: &str,
    album: &str,
    title: &str,
) -> Option<String> {
    let key = crate::art::metadata_key(artist, album, title);
    let (state, hash, _) = lib.art_fetch_row(&key).ok().flatten()?;
    hash.filter(|h| state == "ok" && !h.is_empty())
}

fn track_json(t: &lyra_core::LibraryTrack) -> Value {
    json!({"id": t.path, "path": t.path, "title": t.title,
           "artist": t.artist, "album": t.album,
           "duration": t.duration_secs, "codec": t.codec})
}

fn publication_signature(
    current: Option<&str>,
    queue_len: usize,
    queue_index: Option<usize>,
    track: &Option<Value>,
) -> String {
    json!([current, queue_len, queue_index, track]).to_string()
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
    /// Set whenever `tracks` is reloaded — shells poll it to invalidate
    /// their own row caches (e.g. an IPC-driven `library.scan` finishing
    /// mid-session, not just a UI-initiated one).
    dirty: bool,
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
            dirty: true, // first load: shells should build rows once
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

    /// Consume the library-invalidated flag.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn reload(&mut self) {
        if let Some(lib) = &self.lib {
            self.tracks = lib.all_tracks().unwrap_or_default();
        }
        self.dirty = true;
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

    /// Publish the app-shaped now-playing snapshot when track metadata or
    /// queue length/index changes — the CLI's `lyra state` reads this.
    pub fn publish_state_if_changed(&mut self) {
        let cur_idx = self.queue_index();
        let current = self.current_track();
        let duration = current
            .as_ref()
            .and_then(|t| t.duration_secs)
            .unwrap_or(0.0);
        let track = current.as_deref().map(track_json);
        drop(current);
        let sig =
            publication_signature(self.current.as_deref(), self.tracks.len(), cur_idx, &track);
        if sig == self.last_sig {
            return;
        }
        self.last_sig = sig;

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
            "duration": duration,
            "queue": {"index": cur_idx.unwrap_or(0), "length": self.tracks.len(),
                      "items": items},
            "playlist_revision": self.tracks.len(),
            "device": "default",
        });
        if let Ok(s) = CString::new(state.to_string()) {
            unsafe { crate::ipc::lyra_ipc_publish_state(s.as_ptr()) };
        }
    }

    /// Resolve the current path using the same metadata as library/model rows.
    pub fn current_track(&self) -> Option<Cow<'_, lyra_core::LibraryTrack>> {
        self.track_for_path(self.current.as_deref()?)
    }

    /// Prefer scanned tags; unscanned torrent tracks use cached session filenames
    /// and the SQLite artwork ledger. Does not wait, fetch, or probe audio bytes.
    /// Unknown paths (including unresolved torrent files) return `None`.
    pub fn track_for_path(&self, path: &str) -> Option<Cow<'_, lyra_core::LibraryTrack>> {
        if let Some(track) = self.tracks.iter().find(|t| t.path == path) {
            return Some(Cow::Borrowed(track));
        }
        let (id, index) = torrent_track_id(path)?;
        let torrent = crate::torrent().ok()?;
        let file = torrent
            .files(id)
            .ok()?
            .into_iter()
            .find(|f| f.index == index)?;
        let name = torrent
            .list()
            .into_iter()
            .find(|(i, _)| *i == id)
            .map(|(_, name)| name);
        let mut track = torrent_track_metadata(path, &file.path, name.as_deref());
        track.artwork_hash = self.lib.as_ref().and_then(|lib| {
            cached_metadata_artwork(
                lib,
                track.artist.as_deref().unwrap_or(""),
                track.album.as_deref().unwrap_or(""),
                track.title.as_deref().unwrap_or(""),
            )
        });
        Some(Cow::Owned(track))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torrent_ids_are_strict_nonnegative_indices() {
        assert_eq!(torrent_track_id("torrent://0/3"), Some((0, 3)));
        assert_eq!(torrent_track_id("torrent://12/0"), Some((12, 0)));
        for uri in [
            "torrent://-1/3",
            "torrent://0/-3",
            "torrent://+1/3",
            "torrent://0/+3",
            "torrent:///3",
            "torrent://0/",
            "torrent://0/3/4",
            "torrent://0/3?x=1",
            "torrent://0/ 3",
            "torrent://999999999999999999999999999999/3",
            "torrent://0/999999999999999999999999999999",
            "/music/song.flac",
        ] {
            assert_eq!(torrent_track_id(uri), None, "{uri}");
        }
    }

    #[test]
    fn simple_man_uses_cached_filename_and_parent_album() {
        let t = torrent_track_metadata(
            "torrent://0/3",
            "Lynyrd Skynyrd - Pronounced/04. Simple Man.flac",
            Some("Lynyrd Skynyrd - Discography"),
        );
        assert_eq!(t.path, "torrent://0/3");
        assert_eq!(t.title.as_deref(), Some("Simple Man"));
        assert_eq!(t.artist.as_deref(), Some("Lynyrd Skynyrd"));
        assert_eq!(t.album.as_deref(), Some("Lynyrd Skynyrd - Pronounced"));
        assert_eq!(t.track_number, Some(4));
        assert_eq!(t.format, lyra_core::AudioFormat::Flac);
        assert_eq!(t.codec, "flac");
        assert_eq!(t.duration_secs, None);
        assert_eq!(t.sample_rate, None);
        assert_eq!(t.artwork_hash, None);
        let published = track_json(&t);
        assert_eq!(published["id"], "torrent://0/3");
        assert_eq!(published["title"], "Simple Man");
        assert!(published["duration"].is_null());
    }

    #[test]
    fn root_file_uses_torrent_album_and_normalizes_extension() {
        let t = torrent_track_metadata("torrent://2/0", "01 - Étoile.FLAC", Some("Artist - Album"));
        assert_eq!(t.title.as_deref(), Some("Étoile"));
        assert_eq!(t.album.as_deref(), Some("Artist - Album"));
        assert_eq!(t.artist.as_deref(), Some("Artist"));
        assert_eq!(t.codec, "flac");
        assert_eq!(t.format, lyra_core::AudioFormat::Flac);
    }

    #[test]
    fn preserve_unnumbered_and_all_numeric_titles() {
        for title in ["Simple Man", "2112", "04.", "3Doors Down", "Étoile"] {
            let t = torrent_track_metadata("torrent://0/0", &format!("{title}.mp3"), None);
            assert_eq!(t.title.as_deref(), Some(title));
            assert_eq!(t.track_number, None);
            assert_eq!(t.album, None);
            assert_eq!(t.artist, None);
        }
    }

    #[test]
    fn missing_artist_can_use_torrent_name_without_inventing_tags() {
        let t = torrent_track_metadata(
            "torrent://0/0",
            "CD1/01 Song.unknown",
            Some("Artist - Album"),
        );
        assert_eq!(t.album.as_deref(), Some("CD1"));
        assert_eq!(t.artist.as_deref(), Some("Artist"));
        assert_eq!(t.format, lyra_core::AudioFormat::Unknown);
        let single = torrent_track_metadata("torrent://0/0", "Song.flac", Some("Song.flac"));
        assert_eq!(single.album, None);
        assert_eq!(single.artist, None);
    }

    #[test]
    fn metadata_artwork_matches_ffi_and_survives_reopen() {
        let dir = std::env::temp_dir().join(format!(
            "lyra-metadata-art-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("library.db");
        let cases = [
            (
                "Lynyrd Skynyrd",
                "Lynyrd Skynyrd - Pronounced",
                "Simple Man",
                "lynyrd skynyrd\u{1f}lynyrd skynyrd - pronounced",
                "album-hash",
            ),
            (
                "Aerosmith",
                "",
                "Dream On",
                "aerosmith\u{1f}rec:dream on",
                "recording-hash",
            ),
        ];
        {
            let mut lib = lyra_store::Library::open(&db).unwrap();
            assert!(lib.all_tracks().unwrap().is_empty());
            for (artist, album, title, key, hash) in cases {
                assert_eq!(crate::art::metadata_key(artist, album, title), key);
                lib.art_fetch_record(key, None, "ok", Some(200), None, Some(hash))
                    .unwrap();
                let query = CString::new(
                    json!({
                        "artist": artist, "album": album, "title": title
                    })
                    .to_string(),
                )
                .unwrap();
                let result = unsafe {
                    take_string(crate::art::lyra_art_fetch_meta(&mut lib, query.as_ptr()))
                }
                .unwrap();
                let result: Value = serde_json::from_str(&result).unwrap();
                assert_eq!(result["state"], "not_due");
                assert_eq!(result["hash"], hash);
            }
        }
        {
            let lib = lyra_store::Library::open(&db).unwrap();
            let t = torrent_track_metadata(
                "torrent://0/3",
                "Lynyrd Skynyrd - Pronounced/04. Simple Man.flac",
                Some("Lynyrd Skynyrd - Discography"),
            );
            assert_eq!(
                cached_metadata_artwork(
                    &lib,
                    t.artist.as_deref().unwrap(),
                    t.album.as_deref().unwrap(),
                    t.title.as_deref().unwrap(),
                )
                .as_deref(),
                Some("album-hash")
            );
            for (artist, album, title, key, hash) in cases {
                assert_eq!(
                    cached_metadata_artwork(&lib, artist, album, title).as_deref(),
                    Some(hash)
                );
                for (state, hash) in [
                    ("not_found", Some("false-art")),
                    ("error", Some("false-art")),
                    ("ok", Some("")),
                    ("ok", None),
                ] {
                    lib.art_fetch_record(key, None, state, None, None, hash)
                        .unwrap();
                    assert_eq!(cached_metadata_artwork(&lib, artist, album, title), None);
                }
            }
            assert_eq!(
                cached_metadata_artwork(&lib, "Missing", "Album", "Song"),
                None
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn publication_changes_when_metadata_or_duration_arrives() {
        let mut t = torrent_track_metadata("torrent://0/3", "04. Simple Man.flac", None);
        let signature = |track| publication_signature(Some("torrent://0/3"), 0, None, &track);
        let missing = signature(None);
        let known = signature(Some(track_json(&t)));
        assert_ne!(missing, known);
        assert_eq!(known, signature(Some(track_json(&t))));
        t.duration_secs = Some(357.0);
        assert_ne!(known, signature(Some(track_json(&t))));
        assert_eq!(track_json(&t)["duration"], 357.0);
    }
}
