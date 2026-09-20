//! Test-isolation proofs for the SQLite layer.
//!
//! Each test owns its database: in-memory `Library` values for pure
//! relational tests, file-backed ones inside [`unique_temp_dir`] for
//! anything touching the artwork cache. Run with the default parallel
//! harness — if any state leaked between tests, these fail.

mod common;

use common::unique_temp_dir;
use lyra_core::{AudioFormat, LibraryTrack};
use lyra_store::Library;

fn track(path: &str) -> LibraryTrack {
    LibraryTrack {
        path: path.into(),
        title: Some("Title".into()),
        artist: Some("Artist".into()),
        album: Some("Album".into()),
        album_artist: None,
        genre: None,
        year: None,
        track_number: None,
        duration_secs: None,
        format: AudioFormat::Flac,
        codec: "flac".into(),
        sample_rate: None,
        channels: None,
        bits_per_sample: None,
        artwork_hash: None,
    }
}

/// Two file-backed libraries in sibling unique dirs share nothing.
#[test]
fn file_backed_libraries_are_isolated() {
    let dir_a = unique_temp_dir("lyra-store-iso-a");
    let dir_b = unique_temp_dir("lyra-store-iso-b");
    let a = Library::open(&dir_a.join("lib.db")).unwrap();
    let b = Library::open(&dir_b.join("lib.db")).unwrap();

    a.upsert_track(&track("/shared/path.flac"), 1, 1).unwrap();
    // Same path, different library: independent rows.
    assert!(b.needs_scan("/shared/path.flac", 1).unwrap());
    assert_eq!(b.all_tracks().unwrap().len(), 0);
    assert_eq!(a.all_tracks().unwrap().len(), 1);

    b.set_setting("cursor", "a").unwrap();
    assert_eq!(a.get_setting("cursor").unwrap(), None);

    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}

/// In-memory libraries on parallel threads never observe each other.
#[test]
fn memory_libraries_are_thread_isolated() {
    let handles: Vec<_> = (0..8)
        .map(|i| {
            std::thread::spawn(move || {
                let lib = Library::open_memory().unwrap();
                let path = format!("/thread/{i}.flac");
                lib.upsert_track(&track(&path), 1, 1).unwrap();
                // Sees exactly its own row, even racing 7 siblings.
                let rows = lib.all_tracks().unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].path, path);
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}

/// The helper itself never hands out the same dir twice.
#[test]
fn unique_temp_dirs_never_collide() {
    let dirs: Vec<_> = (0..50)
        .map(|_| unique_temp_dir("lyra-store-uniq"))
        .collect();
    let mut seen = std::collections::HashSet::new();
    for d in &dirs {
        assert!(d.is_dir());
        assert!(seen.insert(d.clone()), "duplicate temp dir: {d:?}");
    }
    for d in &dirs {
        std::fs::remove_dir_all(d).ok();
    }
}
