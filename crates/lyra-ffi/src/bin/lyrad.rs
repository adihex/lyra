//! `lyrad` — the headless Lyra host: the app minus the UI.
//!
//! Boots the same pieces the SwiftUI shell wires up via FFI — playback
//! engine (cpal compat path), library store, the `lyra-ipc` control socket
//! the `lyra`/`lyra-mcp` clients talk to, the torrent session, and an
//! optional LAN remote — then plays the VM's role in the loop: drains
//! UI-bound ops the dispatcher routes up (track.play/next/prev/queue.play,
//! library.reload) and publishes now-playing state back.
//!
//! This is the supported way to run Lyra on Linux, and the smokeable host
//! for headless CI on macOS. The .app remains macOS-only; everything here
//! is the portable Rust core.
//!
//! ```sh
//! cargo run -p lyra-ffi --bin lyrad            # engine + IPC + store
//! lyrad --remote-port 9600 --play song.flac    # + LAN remote + autoplay
//! lyra status | lyra play | lyra scan ~/Music  # from another shell
//! ```

use lyra_ipc::paths;
use serde_json::{json, Value};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::time::Duration;

struct Args {
    /// Data dir for library.db, artwork cache, torrent downloads.
    data_dir: PathBuf,
    /// IPC socket path (default: platform well-known — see lyra-ipc::paths).
    socket: PathBuf,
    /// LAN remote port; the Noise/SPAKE2 listener stays off without it.
    remote_port: Option<u16>,
    /// File to play immediately after boot.
    play: Option<PathBuf>,
}

fn default_data_dir() -> PathBuf {
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

fn usage() -> ! {
    eprintln!(
        "usage: lyrad [--data-dir DIR] [--socket PATH] [--remote-port PORT] [--play FILE]\n\
         \n\
         \t--data-dir DIR     library.db/artwork/torrents live here\n\
         \t                   (default: $XDG_DATA_HOME/lyra or ~/.local/share/lyra)\n\
         \t--socket PATH      IPC socket (default: $XDG_RUNTIME_DIR/lyra/control.sock\n\
         \t                   on Linux; group container on macOS; LYRA_SOCKET wins)\n\
         \t--remote-port PORT serve the LAN remote (SPAKE2 + Noise) on PORT\n\
         \t--play FILE        play FILE after boot"
    );
    std::process::exit(2)
}

fn parse_args() -> Args {
    let mut a = Args {
        data_dir: default_data_dir(),
        socket: paths::default_socket_path(),
        remote_port: None,
        play: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let Some(v) = it.next() else { usage() };
        match arg.as_str() {
            "--data-dir" => a.data_dir = PathBuf::from(v),
            "--socket" => a.socket = PathBuf::from(v),
            "--remote-port" => a.remote_port = Some(v.parse().unwrap_or_else(|_| usage())),
            "--play" => a.play = Some(PathBuf::from(v)),
            _ => usage(),
        }
    }
    a
}

/// CString arg for the FFI calls; the layer takes &CStr-style raw pointers.
fn c(s: &Path) -> CString {
    CString::new(s.to_string_lossy().as_bytes()).unwrap_or_default()
}

/// Drained ops can only play what the engine can open — a local path. The
/// dispatcher's track id IS the path (see ipc.rs `track_json`).
fn play_path(path: &str, current: &mut Option<String>) {
    let e = lyra_ffi::lyra_engine_current();
    if e.is_null() {
        tracing::warn!("track.play: no engine (no output device?)");
        return;
    }
    if !Path::new(path).exists() {
        tracing::warn!("track.play: not a file: {path}");
        return;
    }
    let rc = c(Path::new(path));
    if unsafe { lyra_ffi::lyra_engine_play_file(e, rc.as_ptr()) } == 0 {
        *current = Some(path.to_string());
        tracing::info!("playing {path}");
    } else {
        tracing::warn!("track.play rejected: {path}");
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lyra=info".into()),
        )
        .init();
    let args = parse_args();

    if let Err(e) = std::fs::create_dir_all(&args.data_dir) {
        eprintln!("lyrad: data dir {}: {e}", args.data_dir.display());
        std::process::exit(1);
    }
    let db = args.data_dir.join("library.db");
    let downloads = args.data_dir.join("torrents");
    let sock_dir = args.socket.parent().unwrap_or(Path::new("/")).to_path_buf();

    // Engine on the compat path — on Linux cpal lands on ALSA. A headless
    // box may have no device at all: serve IPC anyway (library ops still
    // work; transport ops answer NotRunning), same as the app's posture.
    let engine = lyra_ffi::lyra_engine_new();
    if engine.is_null() {
        tracing::warn!("no output device — serving library/IPC without audio");
    }
    if unsafe { lyra_ffi::lyra_torrent_init(c(&downloads).as_ptr()) } != 0 {
        tracing::warn!("torrent engine unavailable");
    }
    let rc = unsafe { lyra_ffi::ipc::lyra_ipc_start(c(&db).as_ptr(), c(&sock_dir).as_ptr()) };
    if rc != 0 {
        eprintln!("lyrad: ipc bind at {} failed ({rc})", args.socket.display());
        std::process::exit(rc);
    }
    eprintln!("lyrad: socket {}", args.socket.display());
    eprintln!("lyrad: library {}", db.display());

    if let Some(port) = args.remote_port {
        if engine.is_null() {
            tracing::warn!("--remote-port ignored: remote needs the engine");
        } else {
            let key = c(&args.data_dir.join("remote-key.bin"));
            if unsafe { lyra_ffi::lyra_remote_init(engine, key.as_ptr()) } == 0
                && lyra_ffi::lyra_remote_start(port) == 0
            {
                eprintln!("lyrad: remote on :{port}");
            } else {
                tracing::warn!("remote init/start failed");
            }
        }
    }

    let mut current: Option<String> = None;
    if let Some(p) = &args.play {
        play_path(&p.display().to_string(), &mut current);
    }

    // The VM's job, headless: ordered queue = the library table, drained
    // UI ops executed against it, published state on change.
    let mut tracks: Vec<lyra_core::LibraryTrack> = Vec::new();
    let store = lyra_store::Library::open(&db).ok();
    let reload = |tracks: &mut Vec<lyra_core::LibraryTrack>| {
        if let Some(lib) = &store {
            *tracks = lib.all_tracks().unwrap_or_default();
        }
    };
    reload(&mut tracks);

    let mut last_sig = String::new();
    loop {
        let raw = unsafe { lyra_ffi::ipc::lyra_ipc_drain_commands() };
        if !raw.is_null() {
            let cmds: Vec<Value> = unsafe {
                let s = std::ffi::CStr::from_ptr(raw).to_string_lossy().into_owned();
                lyra_ffi::lyra_string_free(raw);
                serde_json::from_str(&s).unwrap_or_default()
            };
            for cmd in cmds {
                let op = cmd.get("op").and_then(Value::as_str).unwrap_or("");
                let params = cmd.get("params").cloned().unwrap_or(json!({}));
                let idx = current
                    .as_ref()
                    .and_then(|p| tracks.iter().position(|t| &t.path == p));
                match op {
                    "track.play" => {
                        if let Some(id) = params.get("track_id").and_then(Value::as_str) {
                            play_path(id, &mut current);
                        }
                    }
                    "queue.play" => {
                        if let Some(i) = params.get("index").and_then(Value::as_u64) {
                            if let Some(t) = tracks.get(i as usize) {
                                play_path(&t.path.clone(), &mut current);
                            }
                        }
                    }
                    "next" | "prev" => {
                        if !tracks.is_empty() {
                            let i = match (idx, op) {
                                (Some(i), "next") => (i + 1) % tracks.len(),
                                (Some(i), _) => i.saturating_sub(1).min(tracks.len() - 1),
                                (None, _) => 0,
                            };
                            play_path(&tracks[i].path.clone(), &mut current);
                        }
                    }
                    "library.reload" => reload(&mut tracks),
                    "torrent.added" => {
                        let id = params.get("id").and_then(|v| v.as_i64()).unwrap_or(-1);
                        tracing::info!("torrent {id} added — library.scan the dir to index it");
                    }
                    _ => tracing::debug!("unhandled ui op {op}"),
                }
            }
        }

        let track = current.as_deref().and_then(|p| {
            tracks.iter().find(|t| t.path == p).map(|t| {
                json!({"id": t.path, "path": t.path, "title": t.title,
                       "artist": t.artist, "album": t.album,
                       "duration": t.duration_secs, "codec": t.codec})
            })
        });
        let cur_idx = current
            .as_ref()
            .and_then(|p| tracks.iter().position(|t| &t.path == p));
        let sig = format!(
            "{}|{}|{}",
            current.as_deref().unwrap_or(""),
            tracks.len(),
            cur_idx.map(|i| i as i64).unwrap_or(-1)
        );
        if sig != last_sig {
            last_sig = sig;
            let items: Vec<Value> = tracks
                .iter()
                .take(500)
                .map(|t| {
                    json!({"id": t.path, "title": t.title, "artist": t.artist,
                           "album": t.album, "duration": t.duration_secs})
                })
                .collect();
            let state = json!({
                "track": track,
                "duration": current.as_deref()
                    .and_then(|p| tracks.iter().find(|t| t.path == p))
                    .and_then(|t| t.duration_secs).unwrap_or(0.0),
                "queue": {"index": cur_idx.unwrap_or(0), "length": tracks.len(),
                          "items": items},
                "playlist_revision": tracks.len(),
                "device": "default",
            });
            if let Ok(s) = CString::new(state.to_string()) {
                unsafe { lyra_ffi::ipc::lyra_ipc_publish_state(s.as_ptr()) };
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}
