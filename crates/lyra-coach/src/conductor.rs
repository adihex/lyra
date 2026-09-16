//! The beat grid as pure functions of the stream clock (ADR 0004).
//!
//! Port of `dino_shred/rhythm/conductor.py`. Nothing here is ever advanced
//! or accumulated: every quantity derives fresh from a timestamp, so there
//! is no error to accumulate and nothing to drift.
//!
//! Two grid shapes (the V1→V2 hinge from ADR 0004):
//! - [`BeatGrid::FixedTempo`] — `beat_time(n) = t0 + n·period` (live click).
//! - [`BeatGrid::BeatTable`] — an extracted `beats[]` array (song map from
//!   `beat_this`); `nearest_beat` is a binary search (`np.searchsorted`
//!   semantics). Game/judge code is unchanged between the two.

use std::ops::Range;

/// Drift-free beat grid: the single authoritative music clock.
#[derive(Clone, Debug)]
pub enum BeatGrid {
    /// Live metronome grid: `beat_time(n) = t0 + n * (60/bpm)`.
    FixedTempo { t0: f64, bpm: f64 },
    /// Extracted grid (offline beat tracker): sorted beat times in seconds.
    BeatTable { beats: Vec<f64> },
}

impl BeatGrid {
    /// Fixed-tempo grid anchored at stream-clock time `t0`.
    pub fn fixed(t0: f64, bpm: f64) -> Self {
        BeatGrid::FixedTempo { t0, bpm }
    }

    /// Extracted grid. `beats` must be sorted ascending (not checked).
    pub fn table(beats: Vec<f64>) -> Self {
        BeatGrid::BeatTable { beats }
    }

    /// Beat period in seconds. For a table this is the *mean* interval
    /// (a convenience for spawners/HUD, never for judging).
    pub fn period(&self) -> f64 {
        match self {
            BeatGrid::FixedTempo { bpm, .. } => 60.0 / bpm,
            BeatGrid::BeatTable { beats } => {
                if beats.len() < 2 {
                    return 0.5;
                }
                (beats[beats.len() - 1] - beats[0]) / (beats.len() - 1) as f64
            }
        }
    }

    /// Number of beats in a table grid (fixed-tempo grids are unbounded).
    pub fn len(&self) -> Option<usize> {
        match self {
            BeatGrid::FixedTempo { .. } => None,
            BeatGrid::BeatTable { beats } => Some(beats.len()),
        }
    }

    /// Stream-clock time of beat `n`.
    ///
    /// Fixed tempo: exact. Table: `beats[n]`; out-of-range indices
    /// extrapolate with the edge interval (mirrors the Python V2 hinge
    /// where the grid is data, never accumulated state).
    pub fn beat_time(&self, n: i64) -> f64 {
        match self {
            BeatGrid::FixedTempo { t0, bpm } => t0 + n as f64 * (60.0 / bpm),
            BeatGrid::BeatTable { beats } => {
                let len = beats.len() as i64;
                if len == 0 {
                    return f64::NAN;
                }
                if n >= 0 && n < len {
                    return beats[n as usize];
                }
                if len == 1 {
                    return beats[0] + n as f64 * 0.5;
                }
                if n < 0 {
                    let dt = beats[1] - beats[0];
                    return beats[0] + n as f64 * dt;
                }
                let dt = beats[beats.len() - 1] - beats[beats.len() - 2];
                beats[beats.len() - 1] + (n - (len - 1)) as f64 * dt
            }
        }
    }

    /// Index of the beat nearest to stream-clock time `t`.
    ///
    /// Fixed tempo: `round((t - t0) / period)` (Python `round()` =
    /// banker's rounding; the difference only matters at exact
    /// half-period ties, where we round half away from zero — never hit
    /// in practice since onsets never land exactly on a tie).
    /// Table: lower-bound binary search with edge comparison, i.e. the
    /// index a `np.searchsorted`-based nearest lookup would return.
    pub fn nearest_beat(&self, t: f64) -> i64 {
        match self {
            BeatGrid::FixedTempo { t0, bpm } => {
                let period = 60.0 / bpm;
                ((t - t0) / period).round() as i64
            }
            BeatGrid::BeatTable { beats } => {
                if beats.is_empty() {
                    return 0;
                }
                match beats.binary_search_by(|b| b.total_cmp(&t)) {
                    Ok(i) => i as i64,
                    Err(i) => {
                        if i == 0 {
                            0
                        } else if i >= beats.len() {
                            beats.len() as i64 - 1
                        } else {
                            let lo = beats[i - 1];
                            let hi = beats[i];
                            if t - lo <= hi - t {
                                i as i64 - 1
                            } else {
                                i as i64
                            }
                        }
                    }
                }
            }
        }
    }

    /// All beat indices `n` with `t_start <= beat_time(n) < t_end`.
    ///
    /// Fixed tempo: exact ceil arithmetic (port of the Python impl).
    /// Table: binary-searched sub-range over the stored beats.
    pub fn beats_in(&self, t_start: f64, t_end: f64) -> Range<i64> {
        match self {
            BeatGrid::FixedTempo { t0, bpm } => {
                let period = 60.0 / bpm;
                let first = ((t_start - t0) / period).ceil() as i64;
                let stop = ((t_end - t0) / period).ceil() as i64;
                first..stop.max(first)
            }
            BeatGrid::BeatTable { beats } => {
                let first = beats.partition_point(|b| *b < t_start) as i64;
                let stop = beats.partition_point(|b| *b < t_end) as i64;
                first..stop.max(first)
            }
        }
    }
}
