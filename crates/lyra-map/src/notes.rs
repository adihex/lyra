//! NOTES: basic-pitch ONNX (frame/onset/contour @~86 fps) + the
//! note-assembly post-processor — the inference.py port: onset×frame
//! hysteresis, min-note-len, energy gate, bend extraction.
//!
//! Same trait + env-gate pattern as the grid: `OnnxNoteTranscriber` behind
//! the `onnx` feature, `NullNoteTranscriber` (spectral peak-picker + onset
//! gate, low-conf, `Degraded`) so the pipeline runs end-to-end without
//! weights. Mix AND stem runs fuse per-note — agreement is a model-free
//! confidence signal (§5.1); the stem stage itself is P1, so P0 fuses
//! mix-only (agreement 0.6 across the board, honestly).

use crate::map::{Provenance, StageStatus};
use rustfft::{num_complex::Complex, FftPlanner};

/// One detected note, pre-grid (times are raw audio seconds).
#[derive(Debug, Clone)]
pub struct RawNote {
    pub onset_s: f32,
    pub offset_s: f32,
    /// Continuous MIDI (bend-aware center pitch).
    pub midi: f32,
    pub bend_cents: Option<i16>,
    pub onset_strength: f32,
    pub frame_strength: f32,
    /// Composed downstream: onset × frame × source-agreement (§5.1).
    pub conf: f32,
}

/// Transcriber output: notes plus the stage verdict.
#[derive(Debug, Clone)]
pub struct NoteSet {
    pub notes: Vec<RawNote>,
    pub status: StageStatus,
    pub conf: f32,
}

pub trait NoteTranscriber: Send + Sync {
    fn name(&self) -> &str;
    fn transcribe(&self, mono_22050: &[f32]) -> Result<NoteSet, lyra_core::LyraError>;
}

// ── Assembly: the inference.py port ──────────────────────────────────────

/// Thresholds for note assembly (basic-pitch defaults in spirit).
#[derive(Debug, Clone, Copy)]
pub struct AssemblyOpts {
    pub onset_thresh: f32,
    pub frame_thresh: f32,
    /// Minimum note length, in frames.
    pub min_note_frames: usize,
    /// Gap-bridging: frame dropouts this short don't split a note.
    pub max_gap_frames: usize,
    /// Seconds per activation frame (≈86 fps for basic-pitch @22050).
    pub frame_s: f32,
    /// Lowest MIDI pitch in the activation rows.
    pub midi_base: u8,
}

impl Default for AssemblyOpts {
    fn default() -> Self {
        Self {
            onset_thresh: 0.5,
            frame_thresh: 0.3,
            min_note_frames: 5,
            max_gap_frames: 2,
            frame_s: 256.0 / 22_050.0,
            midi_base: 21,
        }
    }
}

/// Assemble notes from frame/onset activations ([T]×[P] row-major) with
/// optional per-pitch contour offsets in semitones (same shape; None skips
/// bend extraction). Pure function — fully unit-testable on synthetic
/// activations, and the exact seam the ONNX adapter feeds.
pub fn assemble_notes(
    frames: &[Vec<f32>],
    onsets: &[Vec<f32>],
    contours: Option<&[Vec<f32>]>,
    opts: &AssemblyOpts,
) -> Vec<RawNote> {
    if frames.is_empty() || frames.len() != onsets.len() {
        return Vec::new();
    }
    let n_pitch = frames[0].len();
    let mut notes = Vec::new();
    for p in 0..n_pitch {
        let mut t = 0usize;
        while t < frames.len() {
            // Onset gate: onset × frame hysteresis opens a note.
            if onsets[t][p] >= opts.onset_thresh && frames[t][p] >= opts.frame_thresh {
                let start = t;
                let mut end = t;
                let mut gap = 0usize;
                let mut u = t + 1;
                while u < frames.len() {
                    if frames[u][p] >= opts.frame_thresh {
                        end = u;
                        gap = 0;
                    } else {
                        gap += 1;
                        if gap > opts.max_gap_frames {
                            break;
                        }
                    }
                    u += 1;
                }
                // Energy gate + min-note-len: weak blips aren't notes.
                let peak_frame = frames[start..=end.min(frames.len() - 1)]
                    .iter()
                    .map(|r| r[p])
                    .fold(0f32, f32::max);
                if end + 1 - start >= opts.min_note_frames && peak_frame >= opts.frame_thresh {
                    let bend = bend_from_contour(contours, start, end, p);
                    let mut midi = opts.midi_base as f32 + p as f32;
                    if let Some(cents) = bend {
                        midi += cents as f32 / 100.0;
                    }
                    notes.push(RawNote {
                        onset_s: start as f32 * opts.frame_s,
                        offset_s: (end + 1) as f32 * opts.frame_s,
                        midi,
                        bend_cents: bend,
                        onset_strength: onsets[start][p],
                        frame_strength: peak_frame,
                        conf: onsets[start][p] * peak_frame,
                    });
                }
                t = u;
            } else {
                t += 1;
            }
        }
    }
    notes.sort_by(|a, b| a.onset_s.partial_cmp(&b.onset_s).unwrap());
    notes
}

/// Bend extraction: median contour offset over the note body; only
/// excursions ≥ ~70¢ count as bends (vibrato-width wobble is not a bend).
fn bend_from_contour(
    contours: Option<&[Vec<f32>]>,
    start: usize,
    end: usize,
    p: usize,
) -> Option<i16> {
    let c = contours?;
    let mut offs: Vec<f32> = (start..=end.min(c.len() - 1)).map(|t| c[t][p]).collect();
    if offs.is_empty() {
        return None;
    }
    offs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = offs[offs.len() / 2];
    let cents = (med * 100.0).round() as i16;
    if cents.abs() >= 70 { Some(cents) } else { None }
}

// ── Fusion: mix × stem agreement ─────────────────────────────────────────

/// Fuse mix-run and stem-run notes. Agreement within ±30 ms + same pitch
/// class → conf × 1.0 (`Fused`); single-source → × 0.6; octave disagreement
/// → × 0.4 (§5.1). P0 has no stem stage, so mix-only maps carry the honest
/// 0.6 single-source factor.
pub fn fuse_notes(
    mix: Vec<RawNote>,
    stem: Vec<RawNote>,
) -> Vec<(RawNote, Provenance)> {
    const TOL: f32 = 0.03;
    let mut used = vec![false; stem.len()];
    let mut out = Vec::with_capacity(mix.len() + stem.len());
    for m in mix {
        let mut best: Option<usize> = None;
        for (i, s) in stem.iter().enumerate() {
            if used[i] {
                continue;
            }
            if (s.onset_s - m.onset_s).abs() <= TOL
                && (s.midi.round() - m.midi.round()).abs() < 6.0
            {
                best = Some(i);
                break;
            }
        }
        match best {
            Some(i) => {
                used[i] = true;
                let s = &stem[i];
                let octave_off =
                    ((s.midi - m.midi).abs() - 12.0).abs() < 1.0 && (s.midi - m.midi).abs() > 6.0;
                let mut fused = m;
                fused.midi = (fused.midi + s.midi) / 2.0;
                fused.onset_strength = fused.onset_strength.max(s.onset_strength);
                fused.frame_strength = fused.frame_strength.max(s.frame_strength);
                if octave_off {
                    fused.conf *= 0.4;
                    out.push((fused, Provenance::Mix));
                } else {
                    fused.conf *= 1.0;
                    out.push((fused, Provenance::Fused));
                }
            }
            None => {
                let mut n = m;
                n.conf *= 0.6;
                out.push((n, Provenance::Mix));
            }
        }
    }
    for (i, s) in stem.into_iter().enumerate() {
        if !used[i] {
            let mut n = s;
            n.conf *= 0.6;
            out.push((n, Provenance::Stem));
        }
    }
    out.sort_by(|a, b| a.0.onset_s.partial_cmp(&b.0.onset_s).unwrap());
    out
}

// ── Null transcriber: spectral peak-picker + onset gate ──────────────────

/// Crude-but-real multi-pitch follower: 2048-pt STFT @256 hop (86 fps, the
/// basic-pitch frame rate), per-frame peak-picking, flux-based onset gating,
/// greedy pitch tracking with hysteresis. Always `Degraded` at conf ~0.3 —
/// a stand-in that exercises assembly/fusion/quantize/tab, not a model.
pub struct NullNoteTranscriber;

impl NoteTranscriber for NullNoteTranscriber {
    fn name(&self) -> &str {
        "null-spectral"
    }

    fn transcribe(&self, mono_22050: &[f32]) -> Result<NoteSet, lyra_core::LyraError> {
        const N: usize = 2048;
        const HOP: usize = 256;
        const SR: f32 = 22_050.0;
        if mono_22050.len() < N * 2 {
            return Ok(NoteSet { notes: Vec::new(), status: StageStatus::Failed, conf: 0.0 });
        }
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(N);
        let window: Vec<f32> = (0..N)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos())
            .collect();
        let mut buf = vec![Complex { re: 0.0, im: 0.0 }; N];
        // Frame magnitudes + spectral flux.
        let mut mags: Vec<Vec<f32>> = Vec::new();
        let mut pos = 0usize;
        while pos + N <= mono_22050.len() {
            for i in 0..N {
                buf[i] = Complex { re: mono_22050[pos + i] * window[i], im: 0.0 };
            }
            fft.process(&mut buf);
            mags.push(buf[..N / 2].iter().map(|v| v.norm()).collect());
            pos += HOP;
        }
        let frame_s = HOP as f32 / SR;
        let flux: Vec<f32> = mags
            .windows(2)
            .map(|w| {
                w[0].iter().zip(w[1].iter()).map(|(a, b)| (b - a).max(0.0)).sum::<f32>()
            })
            .collect();
        let fmean = flux.iter().sum::<f32>() / flux.len().max(1) as f32;
        // Onset-armed frames: local flux maxima above the floor.
        let mut armed = vec![false; mags.len()];
        for i in 1..flux.len().saturating_sub(1) {
            if flux[i] > fmean * 1.5 && flux[i] >= flux[i - 1] && flux[i] >= flux[i + 1] {
                armed[i] = true;
                if i > 0 {
                    armed[i - 1] = true;
                }
                if i + 2 < armed.len() {
                    armed[i + 1] = true;
                }
            }
        }

        #[derive(Clone)]
        struct Active {
            midi: f32,
            start: usize,
            last: usize,
            peak: f32,
            missed: usize,
        }
        let mut active: Vec<Active> = Vec::new();
        let mut notes = Vec::new();
        for (t, mag) in mags.iter().enumerate() {
            let ceiling = mag.iter().fold(0f32, |a, v| a.max(*v));
            let floor = ceiling * 0.18;
            // Local-max peak pick, top-4, guitar range E1..E7.
            let mut peaks: Vec<(f32, f32)> = Vec::new(); // (midi, mag)
            for k in 2..mag.len() - 2 {
                if mag[k] > floor && mag[k] >= mag[k - 1] && mag[k] > mag[k + 1] {
                    let freq = k as f32 * SR / N as f32;
                    if (41.0..2637.0).contains(&freq) {
                        // Parabolic interpolation for sub-bin accuracy.
                        let d = 0.5 * (mag[k - 1] - mag[k + 1])
                            / (mag[k - 1] - 2.0 * mag[k] + mag[k + 1] + 1e-9);
                        let f = (k as f32 + d.clamp(-0.5, 0.5)) * SR / N as f32;
                        peaks.push((69.0 + 12.0 * (f / 440.0).log2(), mag[k]));
                    }
                }
            }
            peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            peaks.truncate(4);
            // Greedy match to active tracks (±60¢); harmonics of an active
            // pitch (octave/fifth within 40¢) don't open new notes.
            // Only tracks that predate this frame are matchable — notes
            // born this frame sit past `n_tracked` and are never indexed.
            let n_tracked = active.len();
            let mut matched = vec![false; n_tracked];
            for (midi, m) in &peaks {
                let mut hit = None;
                for (i, a) in active.iter().enumerate().take(n_tracked) {
                    if !matched[i] && (a.midi - midi).abs() < 0.6 {
                        hit = Some(i);
                        break;
                    }
                }
                match hit {
                    Some(i) => {
                        matched[i] = true;
                        active[i].midi = 0.7 * active[i].midi + 0.3 * midi;
                        active[i].last = t;
                        active[i].missed = 0;
                        active[i].peak = active[i].peak.max(*m);
                    }
                    None => {
                        let harmonic = active.iter().any(|a| {
                            let d = (midi - a.midi).abs();
                            [12.0, 7.0, 19.0, 5.0]
                                .iter()
                                .any(|iv| (d - iv).abs() < 0.4)
                        });
                        if !harmonic && armed[t] {
                            active.push(Active {
                                midi: *midi,
                                start: t,
                                last: t,
                                peak: *m,
                                missed: 0,
                            });
                        }
                    }
                }
            }
            // Age out unmatched tracks (3-frame dropout tolerance).
            // Same-frame newborns sit past `matched` — never penalized.
            let mut i = 0;
            while i < active.len() {
                if i < matched.len() && !matched[i] {
                    active[i].missed += 1;
                }
                if active[i].missed > 3 {
                    let a = active.remove(i);
                    if a.last + 1 - a.start >= 5 {
                        notes.push(RawNote {
                            onset_s: a.start as f32 * frame_s,
                            offset_s: (a.last + 1) as f32 * frame_s,
                            midi: a.midi,
                            bend_cents: None,
                            onset_strength: 0.5,
                            frame_strength: 0.6,
                            conf: 0.3,
                        });
                    }
                } else {
                    i += 1;
                }
            }
        }
        for a in active {
            if a.last + 1 - a.start >= 5 {
                notes.push(RawNote {
                    onset_s: a.start as f32 * frame_s,
                    offset_s: (a.last + 1) as f32 * frame_s,
                    midi: a.midi,
                    bend_cents: None,
                    onset_strength: 0.5,
                    frame_strength: 0.6,
                    conf: 0.3,
                });
            }
        }
        notes.sort_by(|a, b| a.onset_s.partial_cmp(&b.onset_s).unwrap());
        let status = if notes.is_empty() { StageStatus::Failed } else { StageStatus::Degraded };
        let conf = if notes.is_empty() {
            0.0
        } else {
            notes.iter().map(|n| n.conf).sum::<f32>() / notes.len() as f32
        };
        Ok(NoteSet { notes, status, conf })
    }
}

// ── ONNX adapter (basic-pitch) ───────────────────────────────────────────

/// basic-pitch `model.onnx` (Apache-2.0, ships in the wheel) via `ort`.
/// P0 status mirrors the grid adapter: discovery + session validation
/// compile and run; the CQT + harmonic-stacking front end and the exact
/// output-tensor wiring land with weights in P1. Meanwhile `assemble_notes`
/// above is the tested, ready-to-feed post-processor.
#[cfg(feature = "onnx")]
pub struct OnnxNoteTranscriber {
    pub model_path: std::path::PathBuf,
}

#[cfg(feature = "onnx")]
impl OnnxNoteTranscriber {
    pub const MODEL_FILE: &'static str = "basic_pitch.onnx";

    pub fn discover(models_dir: Option<std::path::PathBuf>) -> Option<Self> {
        let dir = models_dir.or_else(crate::grid::models_dir)?;
        let p = dir.join(Self::MODEL_FILE);
        p.is_file().then(|| Self { model_path: p })
    }
}

#[cfg(feature = "onnx")]
impl NoteTranscriber for OnnxNoteTranscriber {
    fn name(&self) -> &str {
        "basic-pitch-onnx"
    }

    fn transcribe(&self, _mono_22050: &[f32]) -> Result<NoteSet, lyra_core::LyraError> {
        let _session = ort::session::Session::builder()
            .map_err(|e| lyra_core::LyraError::Audio(e.to_string()))?
            .commit_from_file(&self.model_path)
            .map_err(|e| lyra_core::LyraError::Audio(e.to_string()))?;
        Err(lyra_core::LyraError::Audio(
            "basic-pitch front end is P1 (needs the CQT/stacking export contract); \
             run without --models-dir for the Null fallback"
                .into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_activations() -> (Vec<Vec<f32>>, Vec<Vec<f32>>, AssemblyOpts) {
        // 30 frames × 4 pitches (base 60): note A on p1 frames 2-12,
        // note B on p3 frames 15-25.
        let (t, p) = (30, 4);
        let mut fr = vec![vec![0.05; p]; t];
        let mut on = vec![vec![0.0; p]; t];
        for f in 2..=12 {
            fr[f][1] = 0.8;
        }
        on[2][1] = 0.9;
        for f in 15..=25 {
            fr[f][3] = 0.7;
        }
        on[15][3] = 0.85;
        // Sub-threshold blip on p0: must not become a note.
        fr[20][0] = 0.2;
        on[20][0] = 0.4;
        let opts = AssemblyOpts { midi_base: 60, ..AssemblyOpts::default() };
        (fr, on, opts)
    }

    #[test]
    fn assembly_finds_notes_rejects_blips() {
        let (fr, on, opts) = synth_activations();
        let notes = assemble_notes(&fr, &on, None, &opts);
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].midi, 61.0);
        assert_eq!(notes[1].midi, 63.0);
        assert!((notes[0].onset_s - 2.0 * opts.frame_s).abs() < 1e-6);
        assert!((notes[0].conf - 0.9 * 0.8).abs() < 1e-5);
    }

    #[test]
    fn assembly_extracts_bends() {
        let (fr, on, opts) = synth_activations();
        let mut ct = vec![vec![0.0; 4]; 30];
        for f in 2..=12 {
            ct[f][1] = 1.2; // +120¢ whole-note bend
        }
        let notes = assemble_notes(&fr, &on, Some(&ct), &opts);
        assert_eq!(notes[0].bend_cents, Some(120));
        assert!((notes[0].midi - 62.2).abs() < 1e-5);
        // Sub-70¢ wobble: vibrato, not a bend.
        for f in 2..=12 {
            ct[f][1] = 0.4;
        }
        let notes = assemble_notes(&fr, &on, Some(&ct), &opts);
        assert_eq!(notes[0].bend_cents, None);
    }

    #[test]
    fn fusion_rewards_agreement() {
        let mix = vec![
            RawNote { onset_s: 1.0, offset_s: 1.4, midi: 69.0, bend_cents: None, onset_strength: 0.9, frame_strength: 0.8, conf: 0.72 },
            RawNote { onset_s: 2.0, offset_s: 2.3, midi: 71.0, bend_cents: None, onset_strength: 0.8, frame_strength: 0.7, conf: 0.56 },
        ];
        let stem = vec![
            RawNote { onset_s: 1.01, offset_s: 1.4, midi: 69.1, bend_cents: None, onset_strength: 0.9, frame_strength: 0.9, conf: 0.81 },
        ];
        let fused = fuse_notes(mix, stem);
        assert_eq!(fused.len(), 2);
        assert_eq!(fused[0].1, Provenance::Fused);
        assert!((fused[0].0.conf - 0.72).abs() < 1e-5);
        assert_eq!(fused[1].1, Provenance::Mix);
        assert!((fused[1].0.conf - 0.56 * 0.6).abs() < 1e-5);
    }

    /// Plucked A2/A3/A4 arpeggio @22050: the Null follower must find
    /// three note-ish events with roughly the right pitches.
    #[test]
    fn null_follower_finds_plucked_notes() {
        let sr = 22_050.0;
        let mut x = vec![0.0; (sr * 2.0) as usize];
        for (start, freq) in [(0.1, 110.0), (0.7, 220.0), (1.3, 440.0)] {
            let n0 = (start * sr) as usize;
            for i in 0..(sr * 0.4) as usize {
                if n0 + i >= x.len() {
                    break;
                }
                let t = i as f32 / sr;
                x[n0 + i] += (-t * 6.0).exp()
                    * (2.0 * std::f32::consts::PI * freq * t).sin()
                    * 0.9;
            }
        }
        let set = NullNoteTranscriber.transcribe(&x).unwrap();
        assert_eq!(set.status, StageStatus::Degraded);
        assert!(set.notes.len() >= 2, "found {:?}", set.notes.len());
        let midis: Vec<f32> = set.notes.iter().map(|n| n.midi).collect();
        for target in [45.0, 57.0, 69.0] {
            assert!(
                midis.iter().any(|m| (m - target).abs() < 1.5),
                "missing ~{target} in {midis:?}"
            );
        }
    }
}
