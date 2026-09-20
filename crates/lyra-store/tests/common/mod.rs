//! Shared test-isolation helpers for lyra-store integration tests.
//!
//! Every test that touches the filesystem gets a unique directory
//! (`unique_temp_dir`) and every test that touches SQLite gets its own
//! `Library` (in-memory by default, or file-backed inside its unique
//! dir). Nothing is shared between tests: no fixed paths, no global
//! database, no fixture that one test can mutate under another.
//!
//! Only `std` is used — no extra dev-dependencies required.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

/// A fresh, empty directory unique to this call across threads *and*
/// processes (pid + timestamp + monotonic sequence). The caller owns
/// cleanup; tests remove their dir at the end so repeat-run sweeps
/// start clean even after a crash leaves a previous dir behind.
pub fn unique_temp_dir(prefix: &str) -> PathBuf {
    let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}-{seq}", std::process::id()));
    // A stale dir from a killed run must not leak rows into this test.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create unique test dir");
    dir
}
