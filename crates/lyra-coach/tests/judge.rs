//! Judge tests — port of `tests/test_judge.py` (same expectations).
//!
//! The click-grid helper builds the `make_judge` shape: fixed 100 BPM grid
//! anchored at t=100 (beats at 100.0, 100.6, …), default windows and margin.
//! Extra cases cover the Rust generalizations: advisory/ghost policies,
//! pitch stamping, accuracy stats.

use lyra_coach::{DetectedHit, ExpectedEvent, Grade, Judge, NotePolicy};

fn make_judge() -> Judge {
    Judge::click_grid(100.0, 100.0, 32)
}

fn onset(t: f64) -> DetectedHit {
    DetectedHit::onset(t)
}

#[test]
fn window_boundaries() {
    // (err, grade) — same table as test_judge.py.
    let cases = [
        (0.0, Grade::Perfect),
        (-0.030, Grade::Perfect),
        (0.031, Grade::Good),
        (-0.060, Grade::Good),
        (0.061, Grade::Ok),
        (-0.100, Grade::Ok),
    ];
    for (err, grade) in cases {
        let mut j = make_judge();
        assert_eq!(j.judge_hit(onset(100.6 + err)).grade, grade, "err={err}");
    }
}

#[test]
fn far_offbeat_is_off_grid_and_keeps_streak() {
    let mut j = make_judge();
    j.judge_hit(onset(100.6));
    assert_eq!(j.streak(), 1);
    let verdict = j.judge_hit(onset(100.6 + 0.25)); // way off any beat
    assert_eq!(verdict.grade, Grade::OffGrid);
    assert_eq!(j.streak(), 1); // off-grid never touches score
}

#[test]
fn one_onset_per_beat() {
    let mut j = make_judge();
    assert_eq!(j.judge_hit(onset(100.59)).grade, Grade::Perfect);
    // beat 1 already consumed
    assert_eq!(j.judge_hit(onset(100.62)).grade, Grade::OffGrid);
}

#[test]
fn calibration_offset_is_subtracted() {
    let mut j = make_judge();
    j.set_offset(0.050); // detector+player measured 50 ms late
    assert_eq!(j.judge_hit(onset(100.6 + 0.050)).grade, Grade::Perfect);
}

#[test]
fn sweep_marks_misses_and_resets_streak() {
    let mut j = make_judge();
    j.judge_hit(onset(100.6)); // hit beat 1
                               // advance past beats 2 and 3 without onsets (+ margin 0.05 + window 0.1)
    let misses = j.sweep_misses(101.8 + 0.16);
    assert_eq!(
        misses.iter().map(|m| m.expected_index).collect::<Vec<_>>(),
        vec![0, 2, 3]
    ); // beat 0 also never hit
    assert!(misses.iter().all(|m| m.grade == Grade::Miss));
    assert_eq!(j.streak(), 0);
    assert_eq!(j.count(Grade::Miss), 3);
}

#[test]
fn sweep_respects_margin_for_inflight_events() {
    let mut j = make_judge();
    // beat 1 window closes at 100.70; sweep margin keeps it claimable until
    // 100.75, so a sweep at 100.72 only expires beat 0 and a late event can
    // still claim beat 1.
    let misses = j.sweep_misses(100.72);
    assert_eq!(
        misses.iter().map(|m| m.expected_index).collect::<Vec<_>>(),
        vec![0]
    );
    assert_eq!(j.judge_hit(onset(100.68)).grade, Grade::Ok);
}

#[test]
fn first_beat_skips_count_in() {
    let expected: Vec<ExpectedEvent> = (0..16)
        .map(|n| ExpectedEvent::chug(100.0 + n as f64 * 0.6))
        .collect();
    let mut j = Judge::new(expected, 0.0, 4, 0.100, 0.05);
    let misses = j.sweep_misses(103.0);
    assert_eq!(
        misses.iter().map(|m| m.expected_index).collect::<Vec<_>>(),
        vec![4]
    ); // beats 0-3 ignored
       // count-in beat not judged
    assert_eq!(j.judge_hit(onset(100.6)).grade, Grade::OffGrid);
}

// --- generalizations beyond the Python original ---

#[test]
fn ghost_notes_score_nothing_and_miss_silently() {
    let expected = vec![
        ExpectedEvent::chug(1.0),
        ExpectedEvent {
            t_secs: 1.5,
            midi: None,
            policy: NotePolicy::Ghost,
        },
        ExpectedEvent::chug(2.0),
    ];
    let mut j = Judge::new(expected, 0.0, 0, 0.100, 0.05);
    assert_eq!(j.judge_hit(onset(1.0)).grade, Grade::Perfect);
    assert_eq!(j.judge_hit(onset(1.5)).grade, Grade::Ghost);
    assert_eq!(j.streak(), 1); // ghost neither extends nor breaks
    assert_eq!(j.accuracy(), Some(1.0)); // ghost excluded from accuracy
    let misses = j.sweep_misses(3.0);
    assert_eq!(misses.len(), 1); // only the graded beat-2 miss; ghost silent
    assert_eq!(misses[0].expected_index, 2);
    assert_eq!(j.streak(), 0);
}

#[test]
fn advisory_notes_judge_but_dont_touch_streak() {
    let expected = vec![
        ExpectedEvent::chug(1.0),
        ExpectedEvent {
            t_secs: 2.0,
            midi: None,
            policy: NotePolicy::Advisory,
        },
    ];
    let mut j = Judge::new(expected, 0.0, 0, 0.100, 0.05);
    j.judge_hit(onset(1.0));
    assert_eq!(j.streak(), 1);
    // Advisory hit is graded...
    assert_eq!(j.judge_hit(onset(2.0)).grade, Grade::Perfect);
    assert_eq!(j.streak(), 1); // ...but never extends the streak
                               // Advisory miss is counted but never resets the streak.
    let mut j = Judge::new(
        vec![
            ExpectedEvent::chug(1.0),
            ExpectedEvent {
                t_secs: 2.0,
                midi: None,
                policy: NotePolicy::Advisory,
            },
        ],
        0.0,
        0,
        0.100,
        0.05,
    );
    j.judge_hit(onset(1.0));
    let misses = j.sweep_misses(3.0);
    assert_eq!(misses.len(), 1);
    assert_eq!(j.streak(), 1);
    assert_eq!(j.accuracy(), Some(1.0));
}

#[test]
fn judgments_carry_pitch_and_feedback_fields() {
    let expected = vec![ExpectedEvent::note(1.0, 69.0)];
    let mut j = Judge::new(expected, 0.0, 0, 0.100, 0.05);
    let v = j.judge_hit(DetectedHit {
        t_secs: 1.01,
        midi: Some(69.1),
        clarity: 0.9,
    });
    assert_eq!(v.grade, Grade::Perfect);
    assert_eq!(v.pitch_target, Some(69.0));
    assert_eq!(v.pitch_detected, Some(69.1));
    assert_eq!(v.pitch_conf, 0.9);
}

#[test]
fn running_stats_track_accuracy_and_bias() {
    let mut j = make_judge();
    assert_eq!(j.accuracy(), None);
    j.judge_hit(onset(100.0 + 0.010)); // perfect, +10 ms
    j.judge_hit(onset(100.6 - 0.050)); // good, -50 ms
    assert_eq!(j.accuracy(), Some(1.0));
    assert_eq!(j.streak(), 2);
    assert_eq!(j.best_streak(), 2);
    let mean = j.mean_error().unwrap();
    assert!((mean - (-0.020)).abs() < 1e-9, "mean={mean}");
    assert!(j.error_sd().unwrap() > 0.0);
    // A miss drops accuracy and resets the streak, not the best.
    j.sweep_misses(101.2 + 0.16);
    assert_eq!(j.accuracy(), Some(2.0 / 3.0));
    assert_eq!(j.streak(), 0);
    assert_eq!(j.best_streak(), 2);
}
