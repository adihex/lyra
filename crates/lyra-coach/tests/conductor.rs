//! Conductor tests — port of `tests/test_conductor.py` (same expectations).
//!
//! Plus table-grid checks for the V1→V2 hinge (ADR 0004 consequence):
//! the same lookup semantics over an extracted `beats[]` array.

use lyra_coach::BeatGrid;

#[test]
fn beat_times_at_100_bpm() {
    let c = BeatGrid::fixed(100.0, 100.0);
    assert_eq!(c.period(), 0.6);
    assert_eq!(c.beat_time(0), 100.0);
    assert_eq!(c.beat_time(5), 103.0);
}

#[test]
fn nearest_beat_rounds_correctly() {
    let c = BeatGrid::fixed(0.0, 100.0);
    assert_eq!(c.nearest_beat(0.29), 0);
    assert_eq!(c.nearest_beat(0.31), 1);
    assert_eq!(c.nearest_beat(2.95), 5);
    assert_eq!(c.nearest_beat(-0.2), 0);
}

#[test]
fn beats_in_window_half_open() {
    let c = BeatGrid::fixed(10.0, 120.0); // period 0.5
    assert_eq!(c.beats_in(10.0, 11.0), 0..2); // 11.0 excluded
    assert_eq!(c.beats_in(10.9, 11.6), 2..4);
    assert!(c.beats_in(10.2, 10.4).is_empty());
}

// --- BeatTable: same semantics over extracted data ---

fn table_100bpm() -> BeatGrid {
    BeatGrid::table((0..8).map(|n| 100.0 + n as f64 * 0.6).collect())
}

#[test]
fn table_beat_time_matches_fixed_grid() {
    let c = table_100bpm();
    assert_eq!(c.beat_time(0), 100.0);
    assert_eq!(c.beat_time(5), 103.0);
    assert!((c.period() - 0.6).abs() < 1e-12);
}

#[test]
fn table_nearest_beat_matches_fixed_grid() {
    let c = table_100bpm();
    assert_eq!(c.nearest_beat(100.29), 0);
    assert_eq!(c.nearest_beat(100.31), 1);
    assert_eq!(c.nearest_beat(102.95), 5);
}

#[test]
fn table_beats_in_half_open() {
    let c = BeatGrid::table(vec![10.0, 10.5, 11.0, 11.5]);
    assert_eq!(c.beats_in(10.0, 11.0), 0..2);
    assert_eq!(c.beats_in(10.9, 11.6), 2..4);
    assert!(c.beats_in(10.2, 10.4).is_empty());
}
