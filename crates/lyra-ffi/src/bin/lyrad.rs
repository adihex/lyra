//! `lyrad` — the headless Lyra host: the app minus the UI.
//!
//! Boots the same pieces the SwiftUI shell wires up via FFI — playback
//! engine (cpal compat path), library store, the `lyra-ipc` control socket
//! the `lyra`/`lyra-mcp` clients talk to, the torrent session, and an
//! optional LAN remote — then plays the VM's role in the loop via
//! [`lyra_ffi::host::Host`]: drains UI-bound ops the dispatcher routes up
//! (track.play/next/prev/queue.play, library.reload) and publishes
//! now-playing state back.
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

use lyra_ffi::host::{self, Host, HostArgs};
use lyra_ipc::paths;
use std::path::PathBuf;
use std::time::Duration;

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

fn parse_args() -> (HostArgs, Option<PathBuf>) {
    let mut a = HostArgs {
        data_dir: host::default_data_dir(),
        socket: paths::default_socket_path(),
        remote_port: None,
    };
    let mut play = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let Some(v) = it.next() else { usage() };
        match arg.as_str() {
            "--data-dir" => a.data_dir = PathBuf::from(v),
            "--socket" => a.socket = PathBuf::from(v),
            "--remote-port" => a.remote_port = Some(v.parse().unwrap_or_else(|_| usage())),
            "--play" => play = Some(PathBuf::from(v)),
            _ => usage(),
        }
    }
    (a, play)
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lyra=info".into()),
        )
        .init();
    let (args, play) = parse_args();

    let mut host = match Host::boot(&args) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("lyrad: {e}");
            std::process::exit(1);
        }
    };
    if let Some(p) = play {
        host.play_path(&p.display().to_string());
    }

    loop {
        host.drain_commands();
        host.publish_state_if_changed();
        std::thread::sleep(Duration::from_millis(150));
    }
}
