//! Session tests: synthetic audio blocks end-to-end (onset → judge →
//! follower), calibration flow, and the MIDI judging path.

use lyra_coach::{
    compute_offset, BeatGrid, CoachEvent, ExpectedEvent, Grade, Session, SessionConfig,
};

const SR: u32 = 48_000;
const HOP: usize = 256;

fn burst_secs(secs: f64) -> Vec<f32> {
    let n = (secs * SR as f64) as usize;
    (0..n)
        .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / SR as f32).sin())
        .collect()
}

fn session_at(chart: Vec<ExpectedEvent>) -> Session {
    Session::new(chart, SessionConfig::default())
}

fn collect(events: impl Iterator<Item = CoachEvent>) -> Vec<CoachEvent> {
    events.collect()
}

#[test]
fn chugs_on_the_grid_judge_perfect() {
    // 100 BPM grid at t=1.0: chugs exactly on beats 1..3 judge Perfect.
    let chart: Vec<ExpectedEvent> = (0..4)
        .map(|n| ExpectedEvent::chug(1.0 + n as f64 * 0.6))
        .collect();
    let mut s = session_at(chart);
    // Prime: silence up to the first beat (lets the sweep expire beat 0
    // only after its window + margin pass — sweep at block end).
    let mut t = 0.0;
    let silence = vec![0.0f32; SR as usize]; // 1 s of silence
    for chunk in silence.chunks(HOP) {
        for _ in s.push_samples(chunk, t) {}
        t += chunk.len() as f64 / SR as f64;
    }
    // Now chug on beats 1, 2, 3 (t=1.6, 2.2, 2.8).
    for beat in 1..4 {
        let bt = 1.0 + beat as f64 * 0.6;
        while t < bt {
            let n = ((bt - t) * SR as f64) as usize;
            let n = n.clamp(1, HOP);
            let q = vec![0.0f32; n];
            for _ in s.push_samples(&q, t) {}
            t += n as f64 / SR as f64;
        }
        let b = burst_secs(0.03);
        let mut verdicts = Vec::new();
        for chunk in b.chunks(HOP) {
            for ev in s.push_samples(chunk, t) {
                if let CoachEvent::Verdict(v) = ev {
                    verdicts.push(v);
                }
            }
            t += chunk.len() as f64 / SR as f64;
        }
        // Decay so the gate re-arms for the next chug.
        let q = vec![0.0f32; 4 * HOP];
        for _ in s.push_samples(&q, t) {}
        t += q.len() as f64 / SR as f64;
        assert_eq!(verdicts.len(), 1, "beat {beat}");
        assert_eq!(verdicts[0].grade, Grade::Perfect, "beat {beat}");
        assert_eq!(verdicts[0].expected_index, beat);
    }
    assert_eq!(s.streak(), 3);
    assert_eq!(s.accuracy(), Some(0.75)); // beat 0 swept as a miss
    assert_eq!(s.chart_position(), 4);
}

#[test]
fn missed_beats_sweep_and_reset_streak() {
    let chart: Vec<ExpectedEvent> = (0..3)
        .map(|n| ExpectedEvent::chug(n as f64 * 0.5))
        .collect();
    let mut s = session_at(chart);
    let mut t = 0.0;
    let mut misses = 0;
    // 1.5 s of silence: all three beats (0, 0.5, 1.0) expire past
    // window-close + sweep margin.
    let silence = vec![0.0f32; (1.5 * SR as f64) as usize];
    for chunk in silence.chunks(HOP) {
        for ev in s.push_samples(chunk, t) {
            if matches!(ev, CoachEvent::Verdict(v) if v.grade == Grade::Miss) {
                misses += 1;
            }
        }
        t += chunk.len() as f64 / SR as f64;
    }
    assert_eq!(misses, 3);
    assert_eq!(s.count(Grade::Miss), 3);
    assert_eq!(s.streak(), 0);
}

#[test]
fn calibration_measures_a_constant_delay() {
    // Player + detector 50 ms late against a 100 BPM grid: calibration
    // must recover ~50 ms, which then judges the same input Perfect.
    let mut s = session_at(vec![ExpectedEvent::chug(100.0)]);
    s.start_calibration(BeatGrid::fixed(0.0, 100.0));
    assert!(s.calibrating());
    let mut t = 0.0;
    let mut verdicts = 0;
    // 24 hits, each 50 ms late (discard 4 → 20 usable = complete).
    for k in 0..24 {
        let bt = k as f64 * 0.6 + 0.050;
        while t < bt {
            let n = ((bt - t) * SR as f64) as usize;
            let n = n.clamp(1, HOP);
            let q = vec![0.0f32; n];
            for _ in s.push_samples(&q, t) {}
            t += n as f64 / SR as f64;
        }
        let b = burst_secs(0.03);
        for chunk in b.chunks(HOP) {
            for ev in s.push_samples(chunk, t) {
                if matches!(ev, CoachEvent::Verdict(_)) {
                    verdicts += 1;
                }
            }
            t += chunk.len() as f64 / SR as f64;
        }
        // Decay to re-arm.
        let q = vec![0.0f32; 4 * HOP];
        for _ in s.push_samples(&q, t) {}
        t += q.len() as f64 / SR as f64;
    }
    assert_eq!(verdicts, 0); // calibrating: onsets measured, never judged
    assert!(s.calibration_hits() >= 20);
    let (offset, mad) = s.complete_calibration().unwrap();
    assert!(!s.calibrating());
    assert!((offset - 0.050).abs() < 0.010, "offset={offset}");
    assert!(mad < 0.010, "mad={mad}");
    // The recovered offset is live: the same 50 ms-late input now judges
    // Perfect against the session chart.
    assert!((s.latency_offset() - offset).abs() < 1e-12);
}

#[test]
fn compute_offset_rejects_outliers() {
    // 20 hits at +50 ms, one wild miss at +400 ms (extra hit) and the
    // median must survive.
    let mut errors = vec![0.050; 20];
    errors.push(0.400);
    errors.push(-0.350);
    let (offset, mad) = compute_offset(&errors);
    assert!((offset - 0.050).abs() < 1e-9, "offset={offset}");
    assert!(mad < 1e-9, "mad={mad}");
}

#[test]
fn midi_note_on_judges_exact_pitch() {
    let chart = vec![ExpectedEvent::note(1.0, 69.0)];
    let mut s = session_at(chart);
    let v = s.midi_note_on(69, 1.005); // 5 ms late, exact pitch
    assert_eq!(v.grade, Grade::Perfect);
    assert_eq!(v.pitch_target, Some(69.0));
    assert_eq!(v.pitch_detected, Some(69.0));
    assert_eq!(v.pitch_conf, 1.0);
    assert_eq!(s.streak(), 1);
    assert_eq!(s.chart_position(), 1);
    let staged = collect(s.drain_staged());
    assert!(staged.iter().any(|e| matches!(
        e,
        CoachEvent::Position {
            matched: 0,
            next_t: None
        }
    )));
}
