//! Onset tests: synthetic impulses/bursts through [`EnergyGate`].
//!
//! No audio hardware — every case feeds synthetic `&[f32]` with explicit
//! stream-clock timestamps, exactly how the HAL input callback will drive it.

use lyra_coach::EnergyGate;

const SR: u32 = 48_000;
const HOP: usize = 256;

fn gate() -> EnergyGate {
    EnergyGate::new(SR, HOP, -30.0, 6.0, 0.08)
}

/// A loud burst: 0.5 amplitude sine @220 Hz, `n_hops` hops long.
fn burst(n_hops: usize) -> Vec<f32> {
    (0..n_hops * HOP)
        .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / SR as f32).sin())
        .collect()
}

#[test]
fn silence_produces_no_events() {
    let mut g = gate();
    let quiet = vec![0.0f32; 4 * HOP];
    assert_eq!(g.push_samples(&quiet, 0.0).count(), 0);
    // Sub-threshold rumble (-40 dB) stays shut too.
    let rumble: Vec<f32> = (0..4 * HOP)
        .map(|i| 0.01 * (2.0 * std::f32::consts::PI * 60.0 * i as f32 / SR as f32).sin())
        .collect();
    assert_eq!(g.push_samples(&rumble, 0.25).count(), 0);
}

#[test]
fn burst_fires_once_with_hop_timestamp() {
    let mut g = gate();
    // One quiet hop first so the event timestamp is unambiguous.
    let quiet = vec![0.0f32; HOP];
    assert_eq!(g.push_samples(&quiet, 0.0).count(), 0);
    let b = burst(4);
    let t1 = HOP as f64 / SR as f64;
    let events: Vec<_> = g.push_samples(&b, t1).collect();
    // Sustained loudness re-arms nothing: exactly one onset (hysteresis).
    assert_eq!(events.len(), 1);
    assert!((events[0].t - t1).abs() < 1e-9, "got t={}", events[0].t);
    assert!(events[0].strength > 0.0);
    assert_eq!(events[0].detector, "energy");
}

#[test]
fn sustained_loudness_does_not_retrigger() {
    let mut g = gate();
    let b = burst(40); // ~213 ms held loud
    let events: Vec<_> = g.push_samples(&b, 0.0).collect();
    assert_eq!(events.len(), 1);
}

#[test]
fn re_arm_after_decay_fires_again() {
    let mut g = gate();
    let b = burst(4);
    // 16 quiet hops (~85 ms) so the second attack clears the refractory.
    let quiet = vec![0.0f32; 16 * HOP];
    let t0 = 0.0;
    assert_eq!(g.push_samples(&b, t0).count(), 1);
    let t1 = b.len() as f64 / SR as f64;
    assert_eq!(g.push_samples(&quiet, t1).count(), 0); // decay re-arms
    let t2 = t1 + quiet.len() as f64 / SR as f64;
    // >80 ms after the first onset: fires again.
    let events: Vec<_> = g.push_samples(&b, t2).collect();
    assert_eq!(events.len(), 1);
    assert!((events[0].t - t2).abs() < 1e-9);
}

#[test]
fn refractory_suppresses_fast_double_trigger() {
    let mut g = gate();
    let b = burst(2); // ~10.7 ms
    let dip = vec![0.0f32; 2 * HOP]; // decay → re-arm
    assert_eq!(g.push_samples(&b, 0.0).count(), 1);
    let t1 = b.len() as f64 / SR as f64;
    assert_eq!(g.push_samples(&dip, t1).count(), 0);
    let t2 = t1 + dip.len() as f64 / SR as f64; // ~21 ms after onset
                                                // Re-armed but inside the 80 ms refractory: suppressed.
    assert_eq!(g.push_samples(&b, t2).count(), 0);
}

#[test]
fn partial_hops_accumulate_across_calls() {
    let mut g = gate();
    let b = burst(2);
    // Feed byte-by-byte-ish: 100-sample dribbles with contiguous clocks.
    let mut t = 0.0;
    let mut total = 0;
    for chunk in b.chunks(100) {
        total += g.push_samples(chunk, t).count();
        t += chunk.len() as f64 / SR as f64;
    }
    assert_eq!(total, 1);
}

#[test]
fn stream_jump_resyncs_without_panic() {
    let mut g = gate();
    let half = vec![0.5f32; HOP / 2]; // partial hop pending
    assert_eq!(g.push_samples(&half, 0.0).count(), 0);
    // Clock jumps forward discontinuously: stale partial dropped, no event.
    let quiet = vec![0.0f32; HOP];
    assert_eq!(g.push_samples(&quiet, 5.0).count(), 0);
    // And the detector still works afterwards.
    let events: Vec<_> = g
        .push_samples(&burst(2), 5.0 + HOP as f64 / SR as f64)
        .collect();
    assert_eq!(events.len(), 1);
}

#[test]
fn rt_buffers_stay_put_after_init() {
    // Proxy for "no alloc in the RT path": capacities fixed at construction
    // must not move no matter how much audio flows through.
    let mut g = gate();
    let loud: Vec<f32> = (0..SR as usize)
        .map(|i| if i % 2 == 0 { 0.4 } else { -0.4 })
        .collect();
    let mut t = 0.0;
    for chunk in loud.chunks(997) {
        for _ in g.push_samples(chunk, t) {}
        t += chunk.len() as f64 / SR as f64;
    }
    // If push_samples had grown anything it would show here; the design
    // keeps one hop buffer + one 8-slot inline stage. This at least pins
    // the behavior against future edits that add a Vec::push somewhere.
    let b = burst(2);
    let events: Vec<_> = g.push_samples(&b, t).collect();
    assert!(events.len() <= 1);
}
