//! Query-counting hook for the SQLite layer (N+1 regression detection).
//!
//! Every statement `Library` issues against rusqlite goes through
//! [`Library::note_query`] (one call per `execute` / `query_row` /
//! `prepare` at the issue site in `lib.rs`). Tests reset the counter,
//! run a representative flow, and assert a bound — so a future change
//! that fans one logical read out into per-row queries (the classic N+1)
//! fails loudly instead of silently slowing scans and searches.
//!
//! Counts are per-`Library` instance (an `AtomicU64`, so `&self`
//! methods can bump it just like rusqlite's own `&self` API), never
//! global: parallel tests with isolated `Library` values cannot pollute
//! each other's counts. What is counted is *statements issued*, not
//! rows touched — a full-table `SELECT` returning 10k rows is 1 query.

use super::Library;
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic statement counter. `Default` starts at zero.
#[derive(Debug, Default)]
pub struct QueryCounter {
    count: AtomicU64,
}

impl QueryCounter {
    pub fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
        }
    }

    /// Record one issued statement.
    pub fn inc(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Statements issued since creation / last [`QueryCounter::reset`].
    pub fn get(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Zero the counter (test setup between flow phases).
    pub fn reset(&self) {
        self.count.store(0, Ordering::Relaxed);
    }
}

impl Library {
    /// Statements issued on this instance since creation / last reset.
    pub fn query_count(&self) -> u64 {
        self.queries.get()
    }

    /// Zero this instance's statement counter.
    pub fn reset_query_count(&self) {
        self.queries.reset();
    }

    /// Hook called at every SQL issue site in `lib.rs`.
    pub(crate) fn note_query(&self) {
        self.queries.inc();
    }
}
