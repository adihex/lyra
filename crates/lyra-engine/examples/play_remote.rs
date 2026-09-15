//! Play a remote file over SSH through the real output device.
//!   cargo run -p lyra-engine --example play_remote -- jiopc /path/song.flac
//! Exercises the product path: ssh dd blocks -> CachingSource ->
//! MediaSource -> symphonia -> SRC/EQ/limiter -> ring -> device.

use lyra_engine::Engine;
use lyra_fs::{ByteSource, CachingSource, SshExecFile};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    let host = std::env::args().nth(1).expect("usage: play_remote <host> <path>");
    let path = std::env::args().nth(2).expect("usage: play_remote <host> <path>");
    let ext = path.rsplit('.').next().map(str::to_owned);

    let remote = SshExecFile::open(&host, &path).expect("ssh open failed");
    eprintln!(
        "opened ssh://{host}{path} — {} bytes",
        remote.len()
    );
    let src = CachingSource::wrap(remote);
    let src: Arc<dyn lyra_fs::ByteSource> = src;

    let engine = Engine::new().expect("engine init failed");
    engine.set_volume(0.6);
    engine.play(src, ext.as_deref());
    eprintln!("playing…");

    // Wait for stream start, then until playback ends (5 min cap).
    let start = Instant::now();
    while !engine.is_playing() && start.elapsed() < Duration::from_secs(15) {
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut last_pos = -1.0f32;
    let mut stalled = Instant::now();
    while engine.is_playing() && start.elapsed() < Duration::from_secs(300) {
        std::thread::sleep(Duration::from_millis(250));
        let pos = engine.position_secs();
        if pos > last_pos {
            last_pos = pos;
            stalled = Instant::now();
        } else if stalled.elapsed() > Duration::from_secs(10) {
            eprintln!("position stalled at {pos:.2}s — network read starving?");
            break;
        }
    }
    eprintln!("done — final position {:.2}s", engine.position_secs());
    engine.shutdown();
}
