//! Live pitch path: classical detectors now, neural fallback later.
//!
//! Default: [`MpmPitchDetector`] (McLeod MPM) with [`YinPitchDetector`]
//! (YIN) as the alternate, both over the permissive `pitch-detection`
//! crate (MIT/Apache). Hops are 5–10 ms at 48 kHz (256–512 samples);
//! analysis windows are ~2–4 periods minimum (2048 samples ≈ 43 ms covers
//! low E at 82 Hz), so pitch always confirms *after* the onset fires —
//! the judge timestamps by onset and validates pitch async, exactly like
//! dino-shred's queue architecture and the latency note in
//! `docs/research/pluggable-engines.md` §2.
//!
//! [`SwiftF0Detector`] (SwiftF0 via `ort`, MIT model) is the robust second
//! opinion for distorted/noisy input, behind the off-by-default `onnx`
//! feature. aubio/pYIN stay paper references (GPL — never linked).

use pitch_detection::detector::PitchDetector as ClassicalDetector;

/// A pitch reading stamped to the stream clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PitchEstimate {
    /// Fundamental frequency in Hz.
    pub freq_hz: f32,
    /// Fractional MIDI note number (`69 + 12·log2(f/440)`).
    pub midi: f32,
    /// Detector confidence in 0–1 (MPM/YIN "clarity").
    pub clarity: f32,
    /// Stream-clock seconds of the analyzed frame's first sample.
    pub t_secs: f64,
}

/// Anything that turns a mono f32 frame into a pitch reading.
///
/// Implementations own their scratch buffers; `detect` performs no
/// allocation (the wrapped `pitch-detection` detectors preallocate a
/// `BufferPool` at construction).
pub trait PitchDetector {
    /// Analyze exactly [`PitchDetector::frame_len`] samples.
    /// Returns `None` for silence/unclear frames.
    fn detect(&mut self, frame: &[f32], sample_rate: u32, t_secs: f64) -> Option<PitchEstimate>;
    /// Required frame length in samples.
    fn frame_len(&self) -> usize;
}

/// Convert a confident Hz reading into a stamped estimate.
///
/// The wrapped detector already applies its clarity gate before returning
/// `Some` — this only stamps the reading. Reported clarity is clamped to
/// 0–1: MPM clarity is a true 0–1 peak ratio, while this `pitch-detection`
/// version's YIN clarity (`1 − threshold + peak/threshold`) can exceed
/// those bounds by construction, so treat it as detector-relative.
fn estimate(freq_hz: f32, clarity: f32, t_secs: f64) -> Option<PitchEstimate> {
    // NaN compares Greater to nothing — partial_cmp keeps the
    // not-positive (including NaN) rejection explicit.
    if freq_hz.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    Some(PitchEstimate {
        freq_hz,
        midi: 69.0 + 12.0 * (freq_hz / 440.0).log2(),
        clarity: clarity.clamp(0.0, 1.0),
        t_secs,
    })
}

/// McLeod Pitch Method — the default detector.
///
/// Fast autocorrelation with normalized square difference + parabolic
/// peak interpolation. Good latency/robustness trade for clean guitar.
pub struct MpmPitchDetector {
    inner: pitch_detection::detector::mcleod::McLeodDetector<f32>,
    frame_len: usize,
    power_threshold: f32,
    clarity_threshold: f32,
}

impl MpmPitchDetector {
    /// `frame_len`: analysis window (2048 @48k ≈ 43 ms covers low E).
    /// Thresholds mirror the `pitch-detection` doc example (power 5.0,
    /// clarity 0.7); loosen clarity toward 0.5 for dirty input.
    pub fn new(frame_len: usize, power_threshold: f32, clarity_threshold: f32) -> Self {
        MpmPitchDetector {
            inner: pitch_detection::detector::mcleod::McLeodDetector::new(frame_len, frame_len / 2),
            frame_len,
            power_threshold,
            clarity_threshold,
        }
    }

    /// Sane default: 2048-sample window, power 5.0, clarity 0.7.
    pub fn default_48k() -> Self {
        MpmPitchDetector::new(2048, 5.0, 0.7)
    }
}

impl PitchDetector for MpmPitchDetector {
    fn detect(&mut self, frame: &[f32], sample_rate: u32, t_secs: f64) -> Option<PitchEstimate> {
        if frame.len() != self.frame_len {
            return None;
        }
        let pitch = self.inner.get_pitch(
            frame,
            sample_rate as usize,
            self.power_threshold,
            self.clarity_threshold,
        )?;
        estimate(pitch.frequency, pitch.clarity, t_secs)
    }

    fn frame_len(&self) -> usize {
        self.frame_len
    }
}

/// YIN — the alternate classical detector.
///
/// Cumulative-mean-normalized difference function; slightly heavier than
/// MPM, often more stable on low strings with strong overtones.
pub struct YinPitchDetector {
    inner: pitch_detection::detector::yin::YINDetector<f32>,
    frame_len: usize,
    power_threshold: f32,
    clarity_threshold: f32,
}

impl YinPitchDetector {
    pub fn new(frame_len: usize, power_threshold: f32, clarity_threshold: f32) -> Self {
        YinPitchDetector {
            inner: pitch_detection::detector::yin::YINDetector::new(frame_len, frame_len / 2),
            frame_len,
            power_threshold,
            clarity_threshold,
        }
    }

    pub fn default_48k() -> Self {
        YinPitchDetector::new(2048, 5.0, 0.7)
    }
}

impl PitchDetector for YinPitchDetector {
    fn detect(&mut self, frame: &[f32], sample_rate: u32, t_secs: f64) -> Option<PitchEstimate> {
        if frame.len() != self.frame_len {
            return None;
        }
        let pitch = self.inner.get_pitch(
            frame,
            sample_rate as usize,
            self.power_threshold,
            self.clarity_threshold,
        )?;
        estimate(pitch.frequency, pitch.clarity, t_secs)
    }

    fn frame_len(&self) -> usize {
        self.frame_len
    }
}

/// SwiftF0 (96k-param MIT net, 16 kHz, G1–C7) via `ort` — the robust
/// second opinion for distorted/noisy input where MPM/YIN degrades.
/// Requires `--features onnx`.
///
/// Status: seam stub. `open()` loads the ONNX artifact; `detect()`
/// currently returns `None` until the 16 kHz mel front-end + output
/// argmax land (tracked work, not silent failure — the session falls
/// back to the classical detector meanwhile).
#[cfg(feature = "onnx")]
pub struct SwiftF0Detector {
    session: ort::session::Session,
    frame_len: usize,
}

#[cfg(feature = "onnx")]
impl SwiftF0Detector {
    /// Load a SwiftF0 ONNX artifact (downloaded to the Lyra models dir,
    /// SHA256-pinned, per BLUEPRINT — never bundled).
    pub fn open(model_path: &std::path::Path, frame_len: usize) -> ort::Result<Self> {
        let session = ort::session::Session::builder()?.commit_from_file(model_path)?;
        Ok(SwiftF0Detector { session, frame_len })
    }
}

#[cfg(feature = "onnx")]
impl PitchDetector for SwiftF0Detector {
    fn detect(&mut self, _frame: &[f32], _sample_rate: u32, _t_secs: f64) -> Option<PitchEstimate> {
        let _ = &self.session;
        // Follow-up (tracked, not silent — the session falls back to the
        // classical detector meanwhile): 16 kHz resample + mel front-end,
        // session.run, argmax → Hz. See the `SwiftF0Detector` status note above.
        None
    }

    fn frame_len(&self) -> usize {
        self.frame_len
    }
}
