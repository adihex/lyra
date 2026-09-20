//! Session-isolation proofs for lyra-torrent.
//!
//! Each test owns its session directory (unique temp dir per test —
//! never the fixed `lyra-torrent-e2e` path the e2e uses), so parallel
//! engines cannot see each other's torrents, orphans, or tracker
//! caches. Pure-config parsing stays offline; engine tests need only
//! loopback session setup, no swarm peers.

use lyra_torrent::{EngineConfig, TorrentEngine};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

/// Fresh session dir unique across threads and processes.
fn unique_session_dir(prefix: &str) -> PathBuf {
    let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn local_engine(dir: PathBuf) -> TorrentEngine {
    // DHT off (no UDP sockets), OS-assigned listen port (None range):
    // parallel engines never contend for a fixed port.
    TorrentEngine::new_with_config(
        dir,
        EngineConfig {
            disable_dht: true,
            listen_port_range: None,
        },
    )
    .unwrap()
}

/// Two engines in sibling dirs start empty and stay independent:
/// an orphan planted in one is invisible to the other.
#[test]
fn engines_do_not_share_session_state() {
    let dir_a = unique_session_dir("lyra-torrent-iso-a");
    let dir_b = unique_session_dir("lyra-torrent-iso-b");
    let a = local_engine(dir_a.clone());
    let b = local_engine(dir_b.clone());

    assert!(a.list().is_empty());
    assert!(b.list().is_empty());

    std::fs::write(dir_a.join("leftover.bin"), [0u8; 16]).unwrap();
    let orphans_a = a.orphans().unwrap();
    assert!(
        orphans_a.iter().any(|(n, _)| n == "leftover.bin"),
        "engine A must report its own orphan: {orphans_a:?}"
    );
    let orphans_b = b.orphans().unwrap();
    assert!(
        !orphans_b.iter().any(|(n, _)| n == "leftover.bin"),
        "engine B must not see A's orphan: {orphans_b:?}"
    );

    // Reclaiming in B removes nothing; A's leftover survives it.
    let (removed, _) = b.purge_orphans().unwrap();
    assert_eq!(removed, 0);
    assert!(dir_a.join("leftover.bin").exists());

    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}

/// Engines built on parallel threads get independent sessions.
#[test]
fn engines_are_thread_isolated() {
    let handles: Vec<_> = (0..4)
        .map(|i| {
            std::thread::spawn(move || {
                let dir = unique_session_dir(&format!("lyra-torrent-thr-{i}"));
                let engine = local_engine(dir.clone());
                assert!(engine.list().is_empty());
                assert!(engine.orphans().unwrap().is_empty());
                std::fs::remove_dir_all(&dir).ok();
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}
