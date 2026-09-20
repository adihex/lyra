//! N+1 regression tests for the SQLite layer.
//!
//! [`Library::query_count`] counts every statement issued (see
//! `src/query_counts.rs`). Each test resets the counter, runs one
//! representative flow, and asserts a *bound* independent of row count:
//! if a future change fans a logical read out into per-row queries,
//! the bound breaks and the regression is caught here, not in prod.
//!
//! Bounds are deliberately tight (exact counts, not generous ceilings)
//! so silent extra queries cannot creep in unnoticed.

mod common;

use lyra_core::{AudioFormat, LibraryTrack};
use lyra_store::Library;

fn track(i: usize) -> LibraryTrack {
    LibraryTrack {
        path: format!("/lib/track-{i:03}.flac"),
        title: Some(format!("Track {i}")),
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

fn seeded(n: usize) -> Library {
    let lib = Library::open_memory().unwrap();
    for i in 0..n {
        lib.upsert_track(&track(i), 1, 1).unwrap();
    }
    lib.reset_query_count();
    lib
}

/// Listing is one statement no matter how many rows exist.
/// (An N+1 here would look like 1 + N per-track follow-ups.)
#[test]
fn all_tracks_is_single_query() {
    for n in [1, 10, 50] {
        let lib = seeded(n);
        let rows = lib.all_tracks().unwrap();
        assert_eq!(rows.len(), n);
        assert_eq!(
            lib.query_count(),
            1,
            "all_tracks issued {} statements for {n} rows",
            lib.query_count()
        );
    }
}

/// Search is at most two statements (FTS, plus the LIKE fallback when
/// the query is not valid FTS syntax) — never one per row.
#[test]
fn search_is_bounded() {
    let lib = seeded(25);
    let rows = lib.search("track").unwrap();
    assert!(!rows.is_empty());
    assert!(
        lib.query_count() <= 2,
        "search issued {} statements",
        lib.query_count()
    );

    // Punctuation forces the LIKE fallback path: still bounded.
    lib.reset_query_count();
    let _ = lib.search("(((unbalanced").unwrap();
    assert!(
        lib.query_count() <= 2,
        "fallback search issued {} statements",
        lib.query_count()
    );
}

/// Point lookups cost exactly one statement each.
#[test]
fn point_lookups_are_single_queries() {
    let lib = seeded(10);

    lib.reset_query_count();
    assert!(!lib.needs_scan("/lib/track-000.flac", 1).unwrap());
    assert_eq!(lib.query_count(), 1);

    lib.reset_query_count();
    assert_eq!(lib.track_audio_hash("/lib/track-000.flac").unwrap(), None);
    assert_eq!(lib.query_count(), 1);

    lib.reset_query_count();
    lib.set_setting("k", "v").unwrap();
    assert_eq!(lib.query_count(), 1);
    lib.reset_query_count();
    assert_eq!(lib.get_setting("k").unwrap().as_deref(), Some("v"));
    assert_eq!(lib.query_count(), 1);
}

/// Writes scale with the write, not with table size: bulk upsert of N
/// tracks is exactly N statements (no per-write re-scan of the table).
#[test]
fn bulk_upsert_scales_with_writes_only() {
    let lib = Library::open_memory().unwrap();
    lib.reset_query_count();
    for i in 0..20 {
        lib.upsert_track(&track(i), 1, 1).unwrap();
    }
    assert_eq!(lib.query_count(), 20);
}

/// File-backed libraries count identically; the DB lives in a unique
/// dir so counting is never polluted by another test's files.
#[test]
fn file_backed_counts_match_memory() {
    let dir = common::unique_temp_dir("lyra-store-qc");
    let lib = Library::open(&dir.join("lib.db")).unwrap();
    for i in 0..5 {
        lib.upsert_track(&track(i), 1, 1).unwrap();
    }
    lib.reset_query_count();
    assert_eq!(lib.all_tracks().unwrap().len(), 5);
    assert_eq!(lib.query_count(), 1);
    std::fs::remove_dir_all(&dir).ok();
}

/// `prune_missing` documents its cost honestly: 1 scan, 1 DELETE prepare,
/// then 1 execution per stale row. The bound is linear in *stale* rows, not in table size —
/// pruning 2 strays from a 30-row table must not cost 30 deletes.
#[test]
fn prune_cost_is_bounded_by_stale_rows() {
    let lib = seeded(30);
    let alive: Vec<String> = (0..28).map(|i| format!("/lib/track-{i:03}.flac")).collect();
    let removed = lib.prune_missing(&alive).unwrap();
    assert_eq!(removed, 2);
    // 1 scan + 1 DELETE prepare + 2 DELETE executions. The bound grows
    // with *stale* rows only — pruning 2 strays from a 30-row table
    // must not cost 30 deletes.
    assert_eq!(
        lib.query_count(),
        4,
        "prune issued {} statements for 2 stale rows",
        lib.query_count()
    );
}
