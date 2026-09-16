//! Judgment — port of `dino_shred/rhythm/judge.py` (ADR 0004 §4).
//!
//! `error = onset_t − offset − expected_t(nearest)`, graded
//! Perfect ±30 ms / Good ±60 ms / OK ±100 ms. One onset claims at most one
//! expected event; `sweep_misses()` trails `now` by a safety margin so an
//! onset still sitting in the event queue can claim its note.
//!
//! Generalizations over the Python original (all additive):
//! - The grid is any sorted [`ExpectedEvent`] list (click grid, beat table,
//!   or song map), not just a fixed-tempo conductor.
//! - Each expected event carries a [`NotePolicy`] (graded / advisory / ghost)
//!   and an optional pitch target; each [`Judgment`] carries
//!   `pitch_target`, `pitch_detected`, `pitch_conf`, and `feedback_mode`
//!   per the pedagogy measurement schema.
//! - Running error stats use Welford online mean/variance instead of a
//!   growing `errors` list (same mean/sd, no allocation in the RT path).
//! - Claim/consumed bookkeeping is a pre-sized bit vector, not a `set`.
//!
//! Scoring rules: graded notes score fully (hit extends the streak, miss
//! resets it). Advisory notes are judged and counted but never touch the
//! streak or accuracy (practice reps). Ghost notes are optional by
//! definition: hitting one yields [`Grade::Ghost`], missing one is silent.

/// Judgment windows (half-widths, seconds). Negative error = early.
pub const PERFECT_WINDOW: f64 = 0.030;
pub const GOOD_WINDOW: f64 = 0.060;
pub const OK_WINDOW: f64 = 0.100;

/// Float tie-breaker on window edges (mirrors Python `_EPS`).
const EPS: f64 = 1e-12;

/// Per-hit grade.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Grade {
    /// Within ±30 ms.
    Perfect,
    /// Within ±60 ms.
    Good,
    /// Within ±100 ms.
    Ok,
    /// A scored note whose claim window passed with no onset.
    Miss,
    /// An onset matching nothing claimable (off-grid extra).
    OffGrid,
    /// An onset matching a ghost-policy note (allowed, unscored).
    Ghost,
}

impl Grade {
    /// Counts toward accuracy (a scored attempt, hit or missed).
    pub fn is_scored(self) -> bool {
        matches!(self, Grade::Perfect | Grade::Good | Grade::Ok | Grade::Miss)
    }

    /// A hit of any kind (extends nothing by itself — policy decides).
    pub fn is_hit(self) -> bool {
        matches!(
            self,
            Grade::Perfect | Grade::Good | Grade::Ok | Grade::Ghost
        )
    }
}

/// Per-note scoring policy, read from the expected event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NotePolicy {
    /// Full scoring: hits extend the streak, misses reset it and count
    /// toward accuracy.
    Graded,
    /// Judged and counted, but never touches streak or accuracy
    /// (technique reps inside a scored run).
    Advisory,
    /// Optional extra (V1 backbeat ghost chugs): hits report
    /// [`Grade::Ghost`], misses are silent.
    Ghost,
}

/// How much judgment the player saw when this event was produced
/// (pedagogy measurement schema §9 — hit-rate under different feedback
/// modes is not comparable).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FeedbackMode {
    /// Immediate category + ms readout.
    Full,
    /// Category only.
    Coarse,
    /// Verdict withheld until phrase end.
    EndOfPhrase,
    /// Silent run (metronome-withdrawal / map-off test).
    Silent,
}

/// One chart note the player is expected to play.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpectedEvent {
    /// Target time in stream-clock seconds (grid time, pre-offset).
    pub t_secs: f64,
    /// Pitch target as fractional MIDI (`None` = timing-only, V1 chugs).
    pub midi: Option<f32>,
    /// Scoring policy for this note.
    pub policy: NotePolicy,
}

impl ExpectedEvent {
    /// Timing-only graded note (V1 chug / click-grid beat).
    pub fn chug(t_secs: f64) -> Self {
        ExpectedEvent {
            t_secs,
            midi: None,
            policy: NotePolicy::Graded,
        }
    }

    /// Pitched graded note.
    pub fn note(t_secs: f64, midi: f32) -> Self {
        ExpectedEvent {
            t_secs,
            midi: Some(midi),
            policy: NotePolicy::Graded,
        }
    }
}

/// One detected hit offered to the judge (onset + optional pitch).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectedHit {
    /// Onset time in stream-clock seconds (post detector, pre-offset).
    pub t_secs: f64,
    /// Detected pitch as fractional MIDI, if the pitch path confirmed one.
    pub midi: Option<f32>,
    /// Pitch confidence 0–1 (MPM/YIN clarity; 1.0 for MIDI input).
    pub clarity: f32,
}

impl DetectedHit {
    /// Timing-only hit (pitch path pending or inapplicable).
    pub fn onset(t_secs: f64) -> Self {
        DetectedHit {
            t_secs,
            midi: None,
            clarity: 0.0,
        }
    }
}

/// The verdict for one hit or one missed note.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Judgment {
    pub grade: Grade,
    /// Index into the judge's expected list (Python `beat_n` generalized).
    pub expected_index: usize,
    /// Signed seconds, early negative. `None` for [`Grade::Miss`].
    pub error_s: Option<f64>,
    /// Pitch target from the expected event (`None` = timing-only).
    pub pitch_target: Option<f32>,
    /// Detected pitch, if any.
    pub pitch_detected: Option<f32>,
    /// Pitch confidence 0–1.
    pub pitch_conf: f32,
    /// Feedback mode in force when judged.
    pub feedback_mode: FeedbackMode,
}

/// The judge. Owns the chart, the calibration offset, and all running stats.
pub struct Judge {
    expected: Vec<ExpectedEvent>,
    offset_s: f64,
    first_index: usize,
    max_window_s: f64,
    sweep_margin_s: f64,
    feedback_mode: FeedbackMode,
    streak: u32,
    best_streak: u32,
    counts: [u64; 6],
    consumed: Vec<bool>,
    next_sweep: usize,
    // Welford online stats over scored-hit errors.
    err_n: u64,
    err_mean: f64,
    err_m2: f64,
    // Accuracy over graded notes only.
    graded_hits: u64,
    graded_total: u64,
}

impl Judge {
    /// Build a judge over a time-sorted chart. Allocates the consumed
    /// bitmap once, here — `judge_hit` / `sweep_misses` never allocate.
    pub fn new(
        expected: Vec<ExpectedEvent>,
        offset_s: f64,
        first_index: usize,
        max_window_s: f64,
        sweep_margin_s: f64,
    ) -> Self {
        let n = expected.len();
        Judge {
            expected,
            offset_s,
            first_index: first_index.min(n),
            max_window_s,
            sweep_margin_s,
            feedback_mode: FeedbackMode::Full,
            streak: 0,
            best_streak: 0,
            counts: [0; 6],
            consumed: vec![false; n],
            next_sweep: first_index.min(n),
            err_n: 0,
            err_mean: 0.0,
            err_m2: 0.0,
            graded_hits: 0,
            graded_total: 0,
        }
    }

    /// Judge over a fixed-tempo click grid: graded timing-only beats
    /// `beat 0..count` from `t0` at `bpm` (the `make_judge` shape).
    pub fn click_grid(t0: f64, bpm: f64, count: usize) -> Self {
        let period = 60.0 / bpm;
        Judge::new(
            (0..count)
                .map(|n| ExpectedEvent::chug(t0 + n as f64 * period))
                .collect(),
            0.0,
            0,
            OK_WINDOW,
            0.05,
        )
    }

    pub fn set_offset(&mut self, offset_s: f64) {
        self.offset_s = offset_s;
    }

    pub fn set_feedback_mode(&mut self, mode: FeedbackMode) {
        self.feedback_mode = mode;
    }

    pub fn feedback_mode(&self) -> FeedbackMode {
        self.feedback_mode
    }

    /// Nearest expected index to grid time `t` (binary search).
    fn nearest(&self, t: f64) -> Option<usize> {
        if self.expected.is_empty() {
            return None;
        }
        match self.expected.binary_search_by(|e| e.t_secs.total_cmp(&t)) {
            Ok(i) => Some(i),
            Err(i) => {
                if i == 0 {
                    Some(0)
                } else if i >= self.expected.len() {
                    Some(self.expected.len() - 1)
                } else {
                    let lo = self.expected[i - 1].t_secs;
                    let hi = self.expected[i].t_secs;
                    Some(if t - lo <= hi - t { i - 1 } else { i })
                }
            }
        }
    }

    fn grade_for_error(error: f64) -> Grade {
        let a = error.abs();
        if a <= PERFECT_WINDOW + EPS {
            Grade::Perfect
        } else if a <= GOOD_WINDOW + EPS {
            Grade::Good
        } else {
            Grade::Ok
        }
    }

    fn count_idx(grade: Grade) -> usize {
        match grade {
            Grade::Perfect => 0,
            Grade::Good => 1,
            Grade::Ok => 2,
            Grade::Miss => 3,
            Grade::OffGrid => 4,
            Grade::Ghost => 5,
        }
    }

    fn observe_scored_hit(&mut self, error: f64) {
        // Welford.
        self.err_n += 1;
        let d = error - self.err_mean;
        self.err_mean += d / self.err_n as f64;
        self.err_m2 += d * (error - self.err_mean);
    }

    fn verdict(
        &self,
        grade: Grade,
        index: usize,
        error: Option<f64>,
        hit: Option<DetectedHit>,
    ) -> Judgment {
        let ev = &self.expected[index];
        Judgment {
            grade,
            expected_index: index,
            error_s: error,
            pitch_target: ev.midi,
            pitch_detected: hit.and_then(|h| h.midi),
            pitch_conf: hit.map(|h| h.clarity).unwrap_or(0.0),
            feedback_mode: self.feedback_mode,
        }
    }

    /// Judge one detected hit. Port of `Judge.judge_onset`.
    pub fn judge_hit(&mut self, hit: DetectedHit) -> Judgment {
        let t = hit.t_secs - self.offset_s;
        let Some(n) = self.nearest(t) else {
            // Empty chart: everything is off-grid.
            return Judgment {
                grade: Grade::OffGrid,
                expected_index: 0,
                error_s: None,
                pitch_target: None,
                pitch_detected: hit.midi,
                pitch_conf: hit.clarity,
                feedback_mode: self.feedback_mode,
            };
        };
        let error = t - self.expected[n].t_secs;
        let claimable = n >= self.first_index
            && n >= self.next_sweep
            && !self.consumed[n]
            && error.abs() <= self.max_window_s + EPS;
        if !claimable {
            self.counts[Self::count_idx(Grade::OffGrid)] += 1;
            return self.verdict(Grade::OffGrid, n, Some(error), Some(hit));
        }

        self.consumed[n] = true;
        match self.expected[n].policy {
            NotePolicy::Ghost => {
                self.counts[Self::count_idx(Grade::Ghost)] += 1;
                self.verdict(Grade::Ghost, n, Some(error), Some(hit))
            }
            NotePolicy::Advisory => {
                let grade = Self::grade_for_error(error);
                self.counts[Self::count_idx(grade)] += 1;
                self.observe_scored_hit(error);
                self.verdict(grade, n, Some(error), Some(hit))
            }
            NotePolicy::Graded => {
                let grade = Self::grade_for_error(error);
                self.counts[Self::count_idx(grade)] += 1;
                self.observe_scored_hit(error);
                self.graded_hits += 1;
                self.graded_total += 1;
                self.streak += 1;
                self.best_streak = self.best_streak.max(self.streak);
                self.verdict(grade, n, Some(error), Some(hit))
            }
        }
    }

    /// Mark expected notes whose claim window fully passed with no hit.
    /// Port of `Judge.sweep_misses` (same margin semantics).
    ///
    /// Convenience wrapper (allocates the miss list — fine on the game/UI
    /// thread). The RT session path uses [`Judge::sweep_misses_into`].
    pub fn sweep_misses(&mut self, now: f64) -> Vec<Judgment> {
        let mut misses = Vec::new();
        self.sweep_into(now, &mut misses);
        misses
    }

    /// Allocation-free sweep for the RT path: misses are pushed into the
    /// caller's list (pre-sized), oldest first.
    pub fn sweep_into(&mut self, now: f64, out: &mut Vec<Judgment>) {
        let t = now - self.offset_s - self.sweep_margin_s;
        while self.next_sweep < self.expected.len()
            && self.expected[self.next_sweep].t_secs + self.max_window_s < t
        {
            let n = self.next_sweep;
            self.next_sweep += 1;
            if self.consumed[n] {
                self.consumed[n] = false;
                continue;
            }
            match self.expected[n].policy {
                NotePolicy::Ghost => {
                    // Optional by definition: silent.
                }
                NotePolicy::Advisory => {
                    self.counts[Self::count_idx(Grade::Miss)] += 1;
                    out.push(self.verdict(Grade::Miss, n, None, None));
                }
                NotePolicy::Graded => {
                    self.counts[Self::count_idx(Grade::Miss)] += 1;
                    self.graded_total += 1;
                    self.streak = 0;
                    out.push(self.verdict(Grade::Miss, n, None, None));
                }
            }
        }
    }

    // --- running stats ---

    pub fn streak(&self) -> u32 {
        self.streak
    }

    /// Best (max) streak so far — the combo high-water mark.
    pub fn best_streak(&self) -> u32 {
        self.best_streak
    }

    pub fn count(&self, grade: Grade) -> u64 {
        self.counts[Self::count_idx(grade)]
    }

    /// Graded hit-rate in 0–1, or `None` before the first graded attempt.
    pub fn accuracy(&self) -> Option<f64> {
        if self.graded_total == 0 {
            return None;
        }
        Some(self.graded_hits as f64 / self.graded_total as f64)
    }

    /// Mean signed error in seconds (early negative), scored hits only.
    pub fn mean_error(&self) -> Option<f64> {
        if self.err_n == 0 {
            return None;
        }
        Some(self.err_mean)
    }

    /// Sample std dev of signed error, scored hits only.
    pub fn error_sd(&self) -> Option<f64> {
        if self.err_n < 2 {
            return None;
        }
        Some((self.err_m2 / (self.err_n - 1) as f64).sqrt())
    }

    /// Next unswept index (follower seam: chart position floor).
    pub fn next_sweep(&self) -> usize {
        self.next_sweep
    }

    pub fn expected(&self) -> &[ExpectedEvent] {
        &self.expected
    }
}
