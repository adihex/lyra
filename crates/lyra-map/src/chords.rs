//! CHORDS: HPCP-style chroma → template HMM + Viterbi (§6, P0).
//!
//! The chordino/NNLS *architecture* (Mauch & Dixon 2010) reimplemented in
//! pure Rust — the plugin code is GPLv2, so this is our own ~200 lines on
//! rustfft: log-frequency chroma + bass chroma → Viterbi over
//! {12 roots × {maj, min}} + N.C. Zero weights, zero license risk.
//! Expected: ~0.75–0.85 segment accuracy, maj/min vocab, on pop/rock.

use crate::grid::grid_pos_at;
use crate::map::{BeatGrid, ChordEvent, ChordQuality, StageStatus, StrumEvent};
use rustfft::{num_complex::Complex, FftPlanner};

const FFT_N: usize = 4096;
const HOP: usize = 2048;
const SR: f32 = 44_100.0;
/// Frames per second of the chroma track.
pub fn chroma_fps() -> f32 {
    SR / HOP as f32
}
/// Cost of changing chord state between frames (log-domain-ish penalty).
const SWITCH_PENALTY: f32 = 0.35;
/// Cosine floor below which N.C. wins the frame.
const NC_SCORE: f32 = 0.5;
/// Runs shorter than this merge into the longer neighbor.
const MIN_SEG_FRAMES: usize = 5;

/// Chord track: segments plus grid-aligned strum onsets.
#[derive(Debug, Clone)]
pub struct ChordTrack {
    pub segments: Vec<ChordEvent>,
    pub strums: Vec<StrumEvent>,
    pub status: StageStatus,
    pub conf: f32,
}

/// Full P0 chord pass over the 44100 stereo bus (uses the left channel).
pub fn transcribe_chords(stereo_44100: &[f32], grid: &BeatGrid) -> ChordTrack {
    let mono: Vec<f32> = stereo_44100
        .as_chunks::<2>()
        .0
        .iter()
        .map(|f| 0.5 * (f[0] + f[1]))
        .collect();
    let (chroma, bass, hop_s) = chroma_track(&mono);
    if chroma.len() < MIN_SEG_FRAMES * 2 {
        return ChordTrack {
            segments: Vec::new(),
            strums: Vec::new(),
            status: StageStatus::Failed,
            conf: 0.0,
        };
    }
    let states = viterbi(&chroma);
    let segments = to_segments(&states, &chroma, &bass, hop_s, grid);
    let strums = detect_strums(&mono, &segments, grid);
    let conf = if segments.is_empty() {
        0.0
    } else {
        segments.iter().map(|s| s.conf).sum::<f32>() / segments.len() as f32
    };
    ChordTrack {
        segments,
        strums,
        status: if conf > 0.0 {
            StageStatus::Ok
        } else {
            StageStatus::Failed
        },
        conf,
    }
}

/// STFT → 12-bin chroma + low-band bass chroma. One Hann-windowed 4096-pt
/// FFT per hop; bins 55 Hz–2 kHz vote into pitch classes by magnitude²,
/// bins 41–262 Hz vote into the bass chroma.
pub fn chroma_track(mono_44100: &[f32]) -> (Vec<[f32; 12]>, Vec<[f32; 12]>, f32) {
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_N);
    let window: Vec<f32> = (0..FFT_N)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT_N as f32).cos())
        .collect();
    let mut buf = vec![Complex { re: 0.0, im: 0.0 }; FFT_N];
    let hop_s = HOP as f32 / SR;
    let mut chroma = Vec::new();
    let mut bass = Vec::new();
    let mut pos = 0usize;
    while pos + FFT_N <= mono_44100.len() {
        for i in 0..FFT_N {
            buf[i] = Complex {
                re: mono_44100[pos + i] * window[i],
                im: 0.0,
            };
        }
        fft.process(&mut buf);
        let mut c = [0f32; 12];
        let mut b = [0f32; 12];
        for (k, v) in buf.iter().enumerate().take(FFT_N / 2) {
            let freq = k as f32 * SR / FFT_N as f32;
            if !(41.0..=2093.0).contains(&freq) {
                continue;
            }
            let mag = v.norm_sqr();
            let midi = (69.0 + 12.0 * (freq / 440.0).log2()).round() as i32;
            let pc = midi.rem_euclid(12) as usize;
            c[pc] += mag;
            if freq < 261.7 {
                b[pc] += mag;
            }
        }
        let n: f32 = c.iter().sum();
        if n > 0.0 {
            for v in c.iter_mut() {
                *v /= n;
            }
        }
        let nb: f32 = b.iter().sum();
        if nb > 0.0 {
            for v in b.iter_mut() {
                *v /= nb;
            }
        }
        chroma.push(c);
        bass.push(b);
        pos += HOP;
    }
    (chroma, bass, hop_s)
}

fn template(root: usize, minor: bool) -> [f32; 12] {
    let mut t = [0.15f32; 12];
    t[root] = 1.0;
    t[(root + if minor { 3 } else { 4 }) % 12] = 0.9;
    t[(root + 7) % 12] = 0.9;
    t
}

fn cosine(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na < 1e-9 || nb < 1e-9 {
        return 0.0;
    }
    dot / (na * nb)
}

/// Viterbi over 25 states (12 maj + 12 min + N.C.). Emission = template
/// cosine; transition = stay-bonus vs. flat switch penalty — the
/// key-aware prior of chordino reduced to its honest P0 core.
pub fn viterbi(chroma: &[[f32; 12]]) -> Vec<usize> {
    const NST: usize = 25;
    let mut templates = [[0f32; 12]; 24];
    for root in 0..12 {
        templates[root * 2] = template(root, false);
        templates[root * 2 + 1] = template(root, true);
    }
    let t = chroma.len();
    let mut emit = vec![[0f32; NST]; t];
    for (i, c) in chroma.iter().enumerate() {
        let silent = c.iter().all(|v| *v == 0.0);
        for s in 0..24 {
            emit[i][s] = if silent {
                0.0
            } else {
                cosine(c, &templates[s])
            };
        }
        emit[i][24] = if silent { 0.9 } else { NC_SCORE };
    }
    let mut dp = vec![[f32::NEG_INFINITY; NST]; t];
    let mut bt = vec![[0u8; NST]; t];
    dp[0] = emit[0];
    for i in 1..t {
        // Best previous state overall (flat switch prior).
        let (best_s, best_v) = (0..NST)
            .map(|s| (s, dp[i - 1][s]))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap();
        for s in 0..NST {
            let stay = dp[i - 1][s];
            let switch = best_v - SWITCH_PENALTY;
            if stay >= switch {
                dp[i][s] = emit[i][s] + stay;
                bt[i][s] = s as u8;
            } else {
                dp[i][s] = emit[i][s] + switch;
                bt[i][s] = best_s as u8;
            }
        }
    }
    let mut states = vec![0usize; t];
    states[t - 1] = (0..NST)
        .max_by(|a, b| dp[t - 1][*a].partial_cmp(&dp[t - 1][*b]).unwrap())
        .unwrap();
    for i in (1..t).rev() {
        states[i - 1] = bt[i][states[i]] as usize;
    }
    states
}

fn state_label(s: usize) -> (Option<u8>, ChordQuality) {
    if s == 24 {
        (None, ChordQuality::Nc)
    } else {
        let root = (s / 2) as u8;
        let q = if s.is_multiple_of(2) {
            ChordQuality::Maj
        } else {
            ChordQuality::Min
        };
        (Some(root), q)
    }
}

fn to_segments(
    states: &[usize],
    chroma: &[[f32; 12]],
    bass: &[[f32; 12]],
    hop_s: f32,
    grid: &BeatGrid,
) -> Vec<ChordEvent> {
    // Collapse runs, then merge sub-minimum runs into the longer neighbor.
    let mut runs: Vec<(usize, usize, usize)> = Vec::new(); // (state, start, end)
    for (i, &s) in states.iter().enumerate() {
        match runs.last_mut() {
            Some(r) if r.0 == s => r.2 = i + 1,
            _ => runs.push((s, i, i + 1)),
        }
    }
    let mut i = 0;
    while i < runs.len() {
        if runs[i].2 - runs[i].1 < MIN_SEG_FRAMES && runs.len() > 1 {
            let other = if i == 0 {
                1
            } else if i + 1 == runs.len()
                || runs[i - 1].2 - runs[i - 1].1 >= runs[i + 1].2 - runs[i + 1].1
            {
                i - 1
            } else {
                i + 1
            };
            if other < i {
                runs[other].2 = runs[i].2;
            } else {
                runs[other].1 = runs[i].1;
            }
            runs.remove(i);
        } else {
            i += 1;
        }
    }

    runs.into_iter()
        .map(|(s, a, b)| {
            let (root, quality) = state_label(s);
            // Segment conf = mean frame margin (best − runner-up cosine).
            let mut conf_sum = 0.0;
            let mut bass_acc = [0f32; 12];
            for f in a..b {
                let mut c0 = 0.0;
                let mut c1 = 0.0;
                for ss in 0..24 {
                    let r = ss / 2;
                    let minor = ss % 2 == 1;
                    let v = cosine(&chroma[f], &template(r, minor));
                    if v > c0 {
                        c1 = c0;
                        c0 = v;
                    } else if v > c1 {
                        c1 = v;
                    }
                }
                conf_sum += (c0 - c1).clamp(0.0, 1.0);
                for pc in 0..12 {
                    bass_acc[pc] += bass[f][pc];
                }
            }
            let n = (b - a) as f32;
            let conf = (conf_sum / n).clamp(0.0, 1.0);
            let bass_pc = bass_acc
                .iter()
                .enumerate()
                .max_by(|x, y| x.1.partial_cmp(y.1).unwrap())
                .map(|(pc, _)| pc as u8);
            let bass = match (root, bass_pc) {
                (Some(r), Some(bpc)) if bpc != r => Some(bpc),
                (r, _) => r,
            };
            // Thin margin between maj/min of the same root → carry the
            // runner-up as an alternate instead of forcing the label.
            let alt = match (root, quality) {
                (Some(_), q @ (ChordQuality::Maj | ChordQuality::Min)) if conf < 0.08 => {
                    Some(if q == ChordQuality::Maj {
                        ChordQuality::Min
                    } else {
                        ChordQuality::Maj
                    })
                }
                _ => None,
            };
            let t0 = a as f32 * hop_s;
            let t1 = b as f32 * hop_s;
            ChordEvent {
                t0,
                t1,
                grid0: grid_pos_at(grid, t0),
                grid1: grid_pos_at(grid, t1),
                root,
                quality,
                bass,
                conf,
                alt,
            }
        })
        .collect()
}

/// Strum onsets: RMS-envelope flux peak-picked, kept when they fall inside
/// (or near) a chord segment, aligned to the grid at detection time.
fn detect_strums(mono_44100: &[f32], segments: &[ChordEvent], grid: &BeatGrid) -> Vec<StrumEvent> {
    const HOP: usize = 512;
    let rms: Vec<f32> = mono_44100
        .chunks(HOP)
        .map(|f| (f.iter().map(|s| s * s).sum::<f32>() / f.len() as f32).sqrt())
        .collect();
    if rms.len() < 8 {
        return Vec::new();
    }
    let flux: Vec<f32> = rms.windows(2).map(|w| (w[1] - w[0]).max(0.0)).collect();
    let mean = flux.iter().sum::<f32>() / flux.len() as f32;
    let std = (flux.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / flux.len() as f32).sqrt();
    let thresh = mean + 0.8 * std;
    let hop_s = HOP as f32 / SR;
    let mut strums = Vec::new();
    // Local-max peak pick, ≥80 ms apart. (wrapping_sub: the first peak is
    // always "far enough" from the virtual peak at t=-∞.)
    let min_gap = (0.08 / hop_s) as usize;
    let mut last = 0usize.wrapping_sub(min_gap);
    for i in 2..flux.len() - 2 {
        if flux[i] < thresh
            || flux[i] < flux[i - 1]
            || flux[i] < flux[i + 1]
            || i.wrapping_sub(last) < min_gap
        {
            continue;
        }
        last = i;
        let t = i as f32 * hop_s;
        let inside = segments
            .iter()
            .any(|s| t >= s.t0 - 0.06 && t <= s.t1 + 0.06);
        if inside {
            strums.push(StrumEvent {
                t,
                grid: grid_pos_at(grid, t),
                conf: (flux[i] / (mean + 3.0 * std + 1e-6)).clamp(0.0, 1.0),
            });
        }
    }
    strums
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render a chord loop: A2+E3+A3+C#4+E4 (A major-ish stack) alternating
    /// with D major voicing, 0.5 s each, 4 s total @44100 mono→stereo.
    fn chord_loop() -> Vec<f32> {
        let sr = 44_100.0;
        let a_maj = [110.0, 164.81, 220.0, 277.18, 329.63];
        let d_maj = [146.83, 220.0, 293.66, 369.99];
        let mut mono = Vec::new();
        for rep in 0..4 {
            let chord = if rep % 2 == 0 { &a_maj[..] } else { &d_maj[..] };
            let n = (sr * 0.5) as usize;
            for i in 0..n {
                let t = i as f32 / sr;
                // Percussive-ish attack + sustain so strums also fire.
                let env = (-t * 4.0).exp() * 0.6 + 0.4;
                let s: f32 = chord
                    .iter()
                    .map(|f| (2.0 * std::f32::consts::PI * f * t).sin())
                    .sum();
                mono.push(env * s / chord.len() as f32 * 0.8);
            }
        }
        let mut stereo = Vec::with_capacity(mono.len() * 2);
        for s in mono {
            stereo.push(s);
            stereo.push(s);
        }
        stereo
    }

    #[test]
    fn viterbi_labels_clean_chords() {
        // Synthetic chroma: 40 frames of A major, 40 of D minor.
        let mut chroma = Vec::new();
        for _ in 0..40 {
            let mut c = [0.02f32; 12];
            for pc in [9, 1, 4] {
                c[pc] = 1.0; // A C# E
            }
            chroma.push(c);
        }
        for _ in 0..40 {
            let mut c = [0.02f32; 12];
            for pc in [2, 5, 9] {
                c[pc] = 1.0; // D F A
            }
            chroma.push(c);
        }
        let states = viterbi(&chroma);
        assert!(states[..40].iter().all(|&s| s == 9 * 2)); // A maj
        assert!(states[40..].iter().all(|&s| s == 2 * 2 + 1)); // D min
    }

    #[test]
    fn end_to_end_chord_loop() {
        let stereo = chord_loop();
        let grid = BeatGrid::empty(StageStatus::Skipped);
        let track = transcribe_chords(&stereo, &grid);
        assert_eq!(track.status, StageStatus::Ok);
        // Expect alternating A / D roots (9 = A, 2 = D).
        let roots: Vec<Option<u8>> = track.segments.iter().map(|s| s.root).collect();
        assert!(roots.contains(&Some(9)), "roots: {roots:?}");
        assert!(roots.contains(&Some(2)), "roots: {roots:?}");
        assert!(!track.strums.is_empty(), "strum onsets should fire");
        assert!(track.conf > 0.1, "conf {}", track.conf);
    }

    #[test]
    fn silence_is_nc_not_a_chord() {
        let stereo = vec![0.0f32; 44_100 * 2 * 2]; // 2 s silence stereo
        let grid = BeatGrid::empty(StageStatus::Skipped);
        let track = transcribe_chords(&stereo, &grid);
        assert!(track.segments.iter().all(|s| s.quality == ChordQuality::Nc));
    }
}
