//! VizTap → LyraVizFrame contract tests — headless: the tap is pure DSP,
//! no device needed. Cases per docs/VIZ-CONTRACT.md § tests.

use lyra_engine::VizTap;
use std::f32::consts::PI;

const RATE: f32 = 48_000.0;
const BLOCK: usize = 2048; // stereo frames per push (~43 ms)

fn stereo(frames: usize, f: impl FnMut(usize) -> [f32; 2]) -> Vec<f32> {
    (0..frames).flat_map(f).collect()
}

fn sine(frames: usize, freq: f32, amp: f32) -> Vec<f32> {
    stereo(frames, |i| {
        let s = (2.0 * PI * freq * i as f32 / RATE).sin() * amp;
        [s, s]
    })
}

fn push_blocks(tap: &mut VizTap, pcm: &[f32]) {
    for chunk in pcm.chunks(BLOCK * 2) {
        tap.push(chunk);
    }
}

#[test]
fn sine_440_lands_in_expected_band() {
    let mut tap = VizTap::new(RATE);
    push_blocks(&mut tap, &sine(BLOCK * 8, 440.0, 0.5));
    let f = tap.frame();
    let peak = f
        .bands
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(i, _)| i)
        .unwrap();
    // 64 log bands 20..20k: b = 64·ln(440/20)/ln(1000) ≈ 28.6
    assert!((26..=32).contains(&peak), "440 Hz peaked at band {peak}");
    assert!(f.bands[peak] > 0.4, "band value {}", f.bands[peak]);
    assert!(f.bass < f.bands[peak], "bass should stay low on a 440 sine");
    assert!(f.seq > 0);
}

#[test]
fn silence_yields_all_zeros() {
    let mut tap = VizTap::new(RATE);
    push_blocks(&mut tap, &vec![0.0; BLOCK * 8 * 2]);
    let f = tap.frame();
    assert!(f.bands.iter().all(|&v| v == 0.0));
    assert!(f.wave_l.iter().chain(&f.wave_r).all(|&v| v == 0.0));
    assert_eq!(f.peak, [0.0; 2]);
    assert_eq!(f.rms, [0.0; 2]);
    assert_eq!(f.bass, 0.0);
    assert_eq!(f.beat, 0.0);
    assert_eq!(f.level, 0.0);
    assert_eq!(f.clip, 0);
    assert!(f.seq > 0);
}

#[test]
fn white_noise_is_broadband() {
    let mut tap = VizTap::new(RATE);
    let mut s = 0x9E3779B97F4A7C15u64;
    let noise = stereo(BLOCK * 24, |_| {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let v = ((s >> 40) as f32 / (1u32 << 24) as f32 - 0.5) * 0.6;
        [v, v]
    });
    push_blocks(&mut tap, &noise);
    let f = tap.frame();
    let min = f.bands.iter().copied().fold(1f32, f32::min);
    assert!(
        f.bands.iter().all(|&v| v > 0.05),
        "band floor dipped to {min}"
    );
    assert!(f.level > 0.3, "noise should read loud, got {}", f.level);
}

#[test]
fn antiphase_gives_opposite_wave_rings() {
    let mut tap = VizTap::new(RATE);
    let pcm = stereo(BLOCK * 4, |i| {
        let s = (2.0 * PI * 300.0 * i as f32 / RATE).sin() * 0.5;
        [s, -s]
    });
    push_blocks(&mut tap, &pcm);
    let f = tap.frame();
    let amp = f.wave_l.iter().copied().fold(0f32, |a, v| a.max(v.abs()));
    assert!(amp > 0.2, "wave ring should carry the signal, max {amp}");
    let cancel = f
        .wave_l
        .iter()
        .zip(&f.wave_r)
        .map(|(l, r)| (l + r).abs())
        .fold(0f32, f32::max);
    assert!(
        cancel < 1e-5,
        "antiphase rings should cancel, residual {cancel}"
    );
    let diff = f
        .wave_l
        .iter()
        .zip(&f.wave_r)
        .map(|(l, r)| (l - r).abs())
        .fold(0f32, f32::max);
    assert!(diff > 0.4, "rings should differ visibly, max diff {diff}");
}

#[test]
fn impulse_spikes_beat_then_decays() {
    let mut tap = VizTap::new(RATE);
    let silence = vec![0.0f32; BLOCK * 2];
    for _ in 0..8 {
        tap.push(&silence);
    }
    let mut hit = vec![0.0f32; BLOCK * 2];
    hit[0] = 1.0;
    tap.push(&hit);
    let p0 = tap.frame().beat;
    assert!(p0 > 0.5, "impulse should spike beat, got {p0}");
    let mut mid = 0f32;
    for _ in 0..2 {
        tap.push(&silence);
        mid = tap.frame().beat;
    }
    assert!(mid < p0, "beat should decay ({p0} -> {mid})");
    for _ in 0..10 {
        tap.push(&silence);
    }
    assert!(tap.frame().beat < 0.05, "beat should reach ~0 in ~150ms+");
}

#[test]
fn wave_ring_is_fixed_capacity_newest_last() {
    let mut tap = VizTap::new(RATE);
    // stride at 48k ≈ 4 — 8000 frames ≫ 256·stride; ring must hold the tail
    let pcm = stereo(8000, |i| {
        let v = ((i % 97) as f32 - 48.0) * 0.01;
        [v, v]
    });
    push_blocks(&mut tap, &pcm);
    let f = tap.frame();
    // kept when (i+1)%stride==0, stride=4 → i = 4k+3; ring holds the last
    // 256 of 2000 kept → k = 1744..=1999, i = 6979..=7999.
    let val = |i: usize| ((i % 97) as f32 - 48.0) * 0.01;
    assert!((f.wave_l[255] - val(7999)).abs() < 1e-6, "newest last");
    assert!((f.wave_l[254] - val(7995)).abs() < 1e-6, "ordering");
    assert!(
        (f.wave_l[0] - val(6979)).abs() < 1e-6,
        "oldest at front — evicted"
    );
}

#[test]
fn clip_bits_set_and_hold() {
    let mut tap = VizTap::new(RATE);
    tap.push(&vec![1.0f32; BLOCK * 2]); // all-ones → both channels clip
    let f = tap.frame();
    assert_eq!(f.clip, 0b11, "all-ones should clip L+R");
    // sticky: still set a few blocks later, clears after the ~1.5s hold
    tap.push(&vec![0.0f32; BLOCK * 2]);
    assert!(tap.frame().clip != 0, "clip should be sticky briefly");
    for _ in 0..60 {
        tap.push(&vec![0.0f32; BLOCK * 2]);
    }
    assert_eq!(tap.frame().clip, 0, "clip hold should expire");
}

#[test]
fn edge_inputs_stay_finite() {
    for (name, pcm) in [
        ("empty", vec![]),
        ("ones", vec![1.0; BLOCK * 2]),
        ("denormals", vec![1e-38; BLOCK * 4]),
        ("huge", vec![1e30; BLOCK * 2]),
        ("nan", vec![f32::NAN; BLOCK * 2]),
        ("mixed", {
            let mut v = vec![f32::INFINITY; BLOCK];
            v.extend(vec![f32::NEG_INFINITY; BLOCK]);
            v
        }),
    ] {
        let mut tap = VizTap::new(RATE);
        tap.push(&pcm);
        tap.push(&pcm);
        let f = tap.frame();
        let all_finite = f
            .bands
            .iter()
            .chain(&f.wave_l)
            .chain(&f.wave_r)
            .chain(&f.peak)
            .chain(&f.rms)
            .chain([&f.bass, &f.beat, &f.level])
            .all(|v| v.is_finite());
        assert!(all_finite, "{name} produced non-finite frame: {:?}", f.peak);
    }
}

#[test]
fn seq_is_monotonic() {
    let mut tap = VizTap::new(RATE);
    let pcm = sine(BLOCK, 440.0, 0.3);
    let mut last = 0;
    for _ in 0..5 {
        tap.push(&pcm);
        let s = tap.frame().seq;
        assert!(s > last);
        last = s;
    }
}
