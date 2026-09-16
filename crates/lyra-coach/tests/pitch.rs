//! Pitch tests: generated sine/chirp at known f0 through MPM + YIN.
//!
//! No audio hardware — synthetic `&[f32]` frames with exact frequencies.

use lyra_coach::{MpmPitchDetector, PitchDetector, YinPitchDetector};

const SR: u32 = 48_000;
const FRAME: usize = 2048;

fn sine(freq: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 * (2.0 * std::f32::consts::PI * freq * i as f32 / SR as f32).sin())
        .collect()
}

fn cents_off(detected: f32, expected: f32) -> f32 {
    1200.0 * (detected / expected).log2().abs()
}

#[test]
fn mpm_tracks_a440_within_a_few_cents() {
    let mut d = MpmPitchDetector::default_48k();
    let est = d.detect(&sine(440.0, FRAME), SR, 0.0).expect("no pitch");
    assert!(cents_off(est.freq_hz, 440.0) < 5.0, "got {}", est.freq_hz);
    assert!((est.midi - 69.0).abs() < 0.1, "got {}", est.midi);
    assert!((0.0..=1.0).contains(&est.clarity));
    assert_eq!(est.t_secs, 0.0);
}

#[test]
fn mpm_tracks_low_e_string() {
    // E2 = 82.41 Hz: the physical worst case (~2 periods per 2048 window).
    let mut d = MpmPitchDetector::default_48k();
    let est = d.detect(&sine(82.41, FRAME), SR, 1.0).expect("no pitch");
    assert!(cents_off(est.freq_hz, 82.41) < 25.0, "got {}", est.freq_hz);
}

#[test]
fn mpm_tracks_high_e_string() {
    // E4 = 329.63 Hz.
    let mut d = MpmPitchDetector::default_48k();
    let est = d.detect(&sine(329.63, FRAME), SR, 0.0).expect("no pitch");
    assert!(cents_off(est.freq_hz, 329.63) < 5.0, "got {}", est.freq_hz);
}

#[test]
fn yin_tracks_a440() {
    let mut d = YinPitchDetector::default_48k();
    let est = d.detect(&sine(440.0, FRAME), SR, 0.0).expect("no pitch");
    assert!(cents_off(est.freq_hz, 440.0) < 10.0, "got {}", est.freq_hz);
}

#[test]
fn silence_returns_none() {
    let mut mpm = MpmPitchDetector::default_48k();
    let mut yin = YinPitchDetector::default_48k();
    let quiet = vec![0.0f32; FRAME];
    assert!(mpm.detect(&quiet, SR, 0.0).is_none());
    assert!(yin.detect(&quiet, SR, 0.0).is_none());
}

#[test]
fn wrong_frame_len_returns_none() {
    let mut d = MpmPitchDetector::default_48k();
    assert!(d.detect(&sine(440.0, 1024), SR, 0.0).is_none());
    assert_eq!(d.frame_len(), FRAME);
}

#[test]
fn mpm_tracks_chirp_frames() {
    // Frequency gliding 200 → 400 Hz across frames: each frame's reading
    // must land near the frame's own center frequency (semitone tolerance).
    let mut d = MpmPitchDetector::default_48k();
    let secs = FRAME as f32 / SR as f32;
    for k in 0..8 {
        let f0 = 200.0 * 2f32.powf(k as f32 / 12.0);
        let frame = sine(f0, FRAME);
        let est = d
            .detect(&frame, SR, k as f64 * secs as f64)
            .unwrap_or_else(|| panic!("no pitch at {f0} Hz"));
        assert!(
            cents_off(est.freq_hz, f0) < 100.0,
            "frame {k}: got {} want {f0}",
            est.freq_hz
        );
    }
}
