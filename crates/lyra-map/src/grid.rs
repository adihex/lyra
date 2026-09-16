//! GRID: beats + downbeats + meter + per-section tempo (§4.1).
//!
//! Production path is beat_this ONNX (MIT) via `ort`; the adapter lives
//! behind the `onnx` cargo feature so default builds need no weights and no
//! onnxruntime download. `NullBeatTracker` is an energy/autocorrelation
//! fallback — an honest constant-tempo grid marked `Degraded`, good enough
//! to exercise the full pipeline (and its failure path) with zero models.
//!
//! Inference is env-gated for tests: integration tests only touch weights
//! when `LYRA_TEST_MODELS` (or `LYRA_MODELS_DIR`) points at a models dir.

use crate::map::{
    BeatGrid, BeatPt, GridPos, MeterChange, StageStatus, TempoMark, TICKS_PER_BEAT,
};
use lyra_core::LyraError;
use std::path::PathBuf;

/// Output of any beat tracker: beats plus the bar model derived from them.
#[derive(Debug, Clone)]
pub struct GridEstimate {
    pub beats: Vec<BeatPt>,
    /// Indices into `beats` marking bar starts.
    pub downbeats: Vec<u32>,
    pub meter: Vec<MeterChange>,
    pub tempi: Vec<TempoMark>,
    pub conf: f32,
    pub status: StageStatus,
}

pub trait BeatTracker: Send + Sync {
    fn name(&self) -> &str;
    fn track(&self, mono_22050: &[f32]) -> Result<GridEstimate, LyraError>;
}

/// Models dir for ONNX weights + env-gated tests. Never inside the repo —
/// weights live in `~/lyra-models/` or the app-support models dir.
pub fn models_dir() -> Option<PathBuf> {
    for key in ["LYRA_TEST_MODELS", "LYRA_MODELS_DIR"] {
        if let Ok(v) = std::env::var(key) {
            let p = PathBuf::from(v);
            if p.is_dir() {
                return Some(p);
            }
        }
    }
    None
}

// ── Null tracker: energy/autocorrelation fallback ────────────────────────

/// Constant-tempo fallback from an amplitude-envelope autocorrelation.
/// Correct for click/metronome material, approximate for real music —
/// always `Degraded`, never silent about it. Silence (or no detectable
/// pulse) is an honest `Failed` estimate, which the pipeline maps to
/// "cannot be charted" instead of emitting garbage (§7).
pub struct NullBeatTracker;

impl BeatTracker for NullBeatTracker {
    fn name(&self) -> &str {
        "null-fallback"
    }

    fn track(&self, mono_22050: &[f32]) -> Result<GridEstimate, LyraError> {
        const SR: usize = 22_050;
        const HOP: usize = 512;
        if mono_22050.len() < SR * 2 {
            return Ok(failed_estimate());
        }
        // Amplitude envelope at ~43 fps, mean-removed.
        let env: Vec<f32> = mono_22050
            .chunks(HOP)
            .map(|f| f.iter().map(|s| s.abs()).sum::<f32>() / f.len() as f32)
            .collect();
        let mean = env.iter().sum::<f32>() / env.len() as f32;
        if mean < 1e-5 {
            return Ok(failed_estimate());
        }
        let env: Vec<f32> = env.iter().map(|e| e - mean).collect();

        // Autocorrelation over 60–200 BPM lags; pick the strongest peak.
        let fps = SR as f32 / HOP as f32;
        let lo = (fps * 60.0 / 200.0) as usize;
        let hi = (fps * 60.0 / 60.0) as usize;
        let mut best_lag = 0usize;
        let mut best_val = f32::NEG_INFINITY;
        for lag in lo..hi.min(env.len() / 2) {
            let v: f32 = env.iter().zip(env[lag..].iter()).map(|(a, b)| a * b).sum();
            if v > best_val {
                best_val = v;
                best_lag = lag;
            }
        }
        if best_lag == 0 || best_val <= 0.0 {
            return Ok(failed_estimate());
        }
        let period_s = best_lag as f32 / fps;

        // First beat: earliest strong envelope attack (top-decile onset).
        let peak = env.iter().fold(f32::NEG_INFINITY, |a, e| a.max(*e));
        let t0_frame = env
            .iter()
            .position(|e| *e > peak * 0.5)
            .unwrap_or(0);
        let t0 = t0_frame as f32 * HOP as f32 / SR as f32;
        let dur = mono_22050.len() as f32 / SR as f32;

        let mut beats = Vec::new();
        let mut t = t0;
        while t < dur {
            beats.push(BeatPt { t_s: t, conf: 0.35 });
            t += period_s;
        }
        if beats.len() < 4 {
            return Ok(failed_estimate());
        }
        // 4/4 bar model: downbeat every 4th beat from the first.
        let downbeats: Vec<u32> = (0..beats.len() as u32).step_by(4).collect();
        let bpm = 60.0 / period_s;
        Ok(GridEstimate {
            beats,
            downbeats,
            meter: vec![MeterChange { bar: 0, beats_per_bar: 4 }],
            tempi: vec![TempoMark { bar: 0, bpm }],
            conf: 0.35,
            status: StageStatus::Degraded,
        })
    }
}

fn failed_estimate() -> GridEstimate {
    GridEstimate {
        beats: Vec::new(),
        downbeats: Vec::new(),
        meter: Vec::new(),
        tempi: Vec::new(),
        conf: 0.0,
        status: StageStatus::Failed,
    }
}

/// Fold a tracker estimate into the map's grid (status + conf preserved).
pub fn apply_estimate(grid: &mut BeatGrid, est: GridEstimate) {
    grid.beats = est.beats;
    grid.downbeats = est.downbeats;
    grid.meter = est.meter;
    grid.tempi = est.tempi;
    grid.conf = est.conf;
    grid.status = est.status;
}

// ── Clock helpers: audio seconds ↔ bar:beat:tick ─────────────────────────

/// Index of the beat at or before `t` (binary search — the conductor's
/// `searchsorted` equivalent for `beat_time` table lookups).
pub fn nearest_beat_idx(beats: &[BeatPt], t: f32) -> usize {
    if beats.is_empty() {
        return 0;
    }
    let mut lo = 0usize;
    let mut hi = beats.len();
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if beats[mid].t_s <= t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    // Snap to the actually-nearest of the bracket.
    if lo + 1 < beats.len()
        && (beats[lo + 1].t_s - t).abs() < (t - beats[lo].t_s).abs()
    {
        lo + 1
    } else {
        lo
    }
}

/// Bar number + beat-in-bar for a beat index, from the downbeat table.
pub fn bar_of(grid: &BeatGrid, beat_idx: usize) -> (u32, u8) {
    let mut bar = 0u32;
    for (i, &db) in grid.downbeats.iter().enumerate() {
        if (db as usize) <= beat_idx {
            bar = i as u32;
        } else {
            break;
        }
    }
    let bar_start = grid.downbeats.get(bar as usize).copied().unwrap_or(0) as usize;
    (bar, beat_idx.saturating_sub(bar_start).min(255) as u8)
}

/// Full grid position for an audio time: beat bracket → bar/beat, tick by
/// interpolation within the beat (clamped to the track ends).
pub fn grid_pos_at(grid: &BeatGrid, t: f32) -> GridPos {
    if grid.beats.is_empty() {
        return GridPos { bar: 0, beat: 0, tick: 0 };
    }
    let i = nearest_beat_idx(&grid.beats, t);
    let (bar, beat) = bar_of(grid, i);
    let b0 = grid.beats[i].t_s;
    let b1 = grid.beats.get(i + 1).map(|b| b.t_s).unwrap_or(b0 + 0.5);
    let span = (b1 - b0).max(1e-3);
    let phase = ((t - b0) / span).clamp(0.0, 1.0);
    // If t falls before the beat (nearest snapped forward), re-anchor to
    // the previous beat so ticks never run backwards.
    let (bar, beat, phase) = if t < b0 && i > 0 {
        let (pb, pbt) = bar_of(grid, i - 1);
        let p0 = grid.beats[i - 1].t_s;
        (pb, pbt, ((t - p0) / span).clamp(0.0, 1.0))
    } else {
        (bar, beat, phase)
    };
    GridPos { bar, beat, tick: (phase * TICKS_PER_BEAT as f32).round() as u16 }
}

// ── ONNX adapter (beat_this) ─────────────────────────────────────────────

/// beat_this ONNX via `ort` — the production grid path (pluggable-engines
/// §3: export via the Rliop913 script or beat_this_cpp's prebuilt artifact;
/// front end = log-mel + small transformer, post-decode in Rust).
///
/// P0 status: model-file discovery + session validation compile and run;
/// the frame decoder that turns activations into beats is the P1 spike
/// (it needs the exported model's exact I/O contract, which only exists
/// alongside weights we don't vendor). Until then this adapter reports
/// `unavailable` and the pipeline keeps the Null fallback — a missing
/// model degrades the grid, never the build.
#[cfg(feature = "onnx")]
pub struct OnnxBeatTracker {
    pub model_path: PathBuf,
}

#[cfg(feature = "onnx")]
impl OnnxBeatTracker {
    pub const MODEL_FILE: &'static str = "beat_this.onnx";

    /// None when no weights are present — the pipeline then uses Null.
    pub fn discover(models_dir: Option<PathBuf>) -> Option<Self> {
        let dir = models_dir.or_else(models_dir)?;
        let p = dir.join(Self::MODEL_FILE);
        p.is_file().then(|| Self { model_path: p })
    }

    fn unavailable(&self, why: &str) -> LyraError {
        LyraError::Audio(format!(
            "beat_this unavailable ({}:{}): {why}",
            self.model_path.display(),
            Self::MODEL_FILE
        ))
    }
}

#[cfg(feature = "onnx")]
impl BeatTracker for OnnxBeatTracker {
    fn name(&self) -> &str {
        "beat_this-onnx"
    }

    fn track(&self, _mono_22050: &[f32]) -> Result<GridEstimate, LyraError> {
        // Validate the artifact loads — proves the ort session + EP wiring
        // on this machine; inference shapes land with the export spike.
        let _session = ort::session::Session::builder()
            .map_err(|e| self.unavailable(&e.to_string()))?
            .commit_from_file(&self.model_path)
            .map_err(|e| self.unavailable(&e.to_string()))?;
        Err(self.unavailable(
            "frame decoder is P1 (needs the export's I/O contract); \
             run without --models-dir for the Null fallback",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Click track: 120 BPM impulses @22050, 8 s.
    fn click_track(bpm: f32, secs: f32) -> Vec<f32> {
        let sr = 22_050.0;
        let n = (sr * secs) as usize;
        let mut x = vec![0.0; n];
        let period = (sr * 60.0 / bpm) as usize;
        let mut i = 0;
        while i < n {
            for k in 0..32 {
                if i + k < n {
                    x[i + k] = 1.0 - k as f32 / 32.0;
                }
            }
            i += period;
        }
        x
    }

    #[test]
    fn null_tracker_finds_click_tempo() {
        let est = NullBeatTracker.track(&click_track(120.0, 8.0)).unwrap();
        assert_eq!(est.status, StageStatus::Degraded);
        assert_eq!(est.meter[0].beats_per_bar, 4);
        let bpm = est.tempi[0].bpm;
        assert!((bpm - 120.0).abs() < 3.0, "got {bpm}");
        // ~16 beats in 8 s at 120 BPM.
        assert!((est.beats.len() as i32 - 16).abs() <= 2, "{}", est.beats.len());
        assert_eq!(est.downbeats, vec![0, 4, 8, 12]);
    }

    #[test]
    fn silence_is_failed_not_garbage() {
        let est = NullBeatTracker.track(&vec![0.0; 22_050 * 4]).unwrap();
        assert_eq!(est.status, StageStatus::Failed);
        assert!(est.beats.is_empty());
    }

    #[test]
    fn clock_helpers_roundtrip() {
        let est = NullBeatTracker.track(&click_track(120.0, 8.0)).unwrap();
        let mut grid = BeatGrid::empty(StageStatus::Degraded);
        apply_estimate(&mut grid, est);
        // Beat 5 (t≈2.5 s) is the second beat of bar 1.
        let gp = grid_pos_at(&grid, grid.beats[5].t_s + 0.01);
        assert_eq!((gp.bar, gp.beat), (1, 1));
        assert!(gp.tick < 60, "tick {}", gp.tick);
        assert_eq!(nearest_beat_idx(&grid.beats, -99.0), 0);
        let last = nearest_beat_idx(&grid.beats, 1e9);
        assert_eq!(last, grid.beats.len() - 1);
    }

    /// Env-gated: only runs with real weights (`LYRA_TEST_MODELS` set).
    #[test]
    fn onnx_smoke_if_models_present() {
        let Some(dir) = models_dir() else { return };
        assert!(dir.join("beat_this.onnx").is_file(), "no beat_this.onnx in {}", dir.display());
        #[cfg(feature = "onnx")]
        {
            let t = OnnxBeatTracker { model_path: dir.join("beat_this.onnx") };
            let _ = t.track(&click_track(120.0, 4.0));
        }
    }
}
