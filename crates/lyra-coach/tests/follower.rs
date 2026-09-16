//! Follower tests: windowed monotonic alignment + wait-for-me gating.

use lyra_coach::Follower;

fn grid() -> Follower {
    Follower::new((0..8).map(|n| n as f64 * 0.5).collect())
}

#[test]
fn tracks_hits_monotonically() {
    let mut f = grid();
    assert_eq!(f.position(), 0);
    assert_eq!(f.observe(0.02), Some(0));
    assert_eq!(f.position(), 1);
    assert_eq!(f.observe(0.52), Some(1));
    assert_eq!(f.position(), 2);
}

#[test]
fn skips_to_nearest_within_window() {
    let mut f = grid();
    // Player skipped beat 1 and hit beat 2: the window reaches ahead.
    assert_eq!(f.observe(1.01), Some(2));
    assert_eq!(f.position(), 3);
}

#[test]
fn never_matches_backward_by_default() {
    let mut f = grid();
    f.observe(0.02);
    f.observe(0.52);
    // A late duplicate of beat 0 arrives after pos advanced: no match
    // (strictly monotonic — the judge already consumed that beat).
    assert_eq!(f.observe(0.03), None);
    assert_eq!(f.position(), 2);
}

#[test]
fn far_off_grid_hits_match_nothing() {
    let mut f = grid();
    assert_eq!(f.observe(0.30), None); // 0.2 from either neighbor
    assert_eq!(f.position(), 0);
}

#[test]
fn resync_after_seek() {
    let mut f = grid();
    f.observe(0.02);
    f.resync(2.0); // user jumped to 2.0 s: beats 0..=4 consumed
    assert_eq!(f.position(), 5);
    assert_eq!(f.next_event_t(), Some(2.5));
    // And backward seeks work too.
    f.resync(0.0);
    assert_eq!(f.position(), 1); // beat 0 at 0.0 <= 0.0
}

#[test]
fn wait_for_me_holds_the_song() {
    let mut f = grid();
    f.set_wait_for_me(true, 0.25);
    // Song may run to beat 0 + grace, then holds.
    assert_eq!(f.gate_chart_time(0.1), 0.1);
    assert_eq!(f.gate_chart_time(1.0), 0.25);
    // Player arrives: the gate releases to the next event.
    f.observe(0.02);
    assert_eq!(f.gate_chart_time(1.0), 0.75);
    assert_eq!(f.gate_chart_time(0.6), 0.6);
}

#[test]
fn normal_mode_gate_is_identity() {
    let f = grid();
    assert_eq!(f.gate_chart_time(99.0), 99.0);
}

#[test]
fn finished_chart_matches_nothing_and_gates_open() {
    let mut f = Follower::new(vec![0.0, 0.5]);
    f.set_wait_for_me(true, 0.25);
    f.observe(0.01);
    f.observe(0.51);
    assert!(f.is_finished());
    assert_eq!(f.observe(1.01), None);
    assert_eq!(f.gate_chart_time(99.0), 99.0);
    assert_eq!(f.next_event_t(), None);
}

#[test]
fn loose_match_counter_flags_drift() {
    let mut f = grid();
    f.observe(0.02); // tight
    assert_eq!(f.loose_matches(), 0);
    f.observe(0.60); // 100 ms off beat 1: matches, but loose
    assert_eq!(f.loose_matches(), 1);
}
