//! Session — onset + pitch → judge → follower, calibration, UI event stream.
//!
//! One `Session` owns the whole live lane for a chart: audio blocks go in
//! via [`Session::push_samples`], [`CoachEvent`]s come out for the UI lane.
//! Calibration state follows `rhythm/calibrate.py` semantics (median + MAD
//! outlier rejection, first hits discarded, per-device offset persisted by
//! the caller alongside detector/blocksize metadata).

use crate::batch::{BatchDrain, EventBatch};
use crate::conductor::BeatGrid;
use crate::follower::Follower;
use crate::judge::{DetectedHit, ExpectedEvent, FeedbackMode, Grade, Judge, Judgment};
use crate::onset::EnergyGate;
use crate::pitch::{MpmPitchDetector, PitchDetector};

/// Max UI events staged per audio block (onsets + verdicts + positions).
pub const MAX_SESSION_EVENTS: usize = 24;

/// Pitch history kept for the analysis window (samples @48k ≈ 85 ms).
const PITCH_HISTORY: usize = 4096;

/// Calibration defaults from `calibrate.py`: ≥20 hits, drop the first 4.
pub const CAL_MIN_HITS: usize = 20;
pub const CAL_DISCARD: usize = 4;

/// Session configuration. Built once, before any audio flows.
#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub sample_rate: u32,
    /// Initial input-latency estimate in seconds (refined by calibration).
    pub latency_offset_s: f64,
    pub feedback_mode: FeedbackMode,
    pub wait_for_me: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            sample_rate: 48_000,
            latency_offset_s: 0.0,
            feedback_mode: FeedbackMode::Full,
            wait_for_me: false,
        }
    }
}

/// UI-lane event. `Copy` so the RT path stages it without allocation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CoachEvent {
    /// Raw onset (pre-judgment — drives HUD flashes, string anim).
    Onset { t: f64, strength: f32 },
    /// A judged hit or swept miss (carries `pitch_target`, `pitch_conf`,
    /// `feedback_mode` per the measurement schema).
    Verdict(Judgment),
    /// Follower position after an observation.
    Position { matched: usize, next_t: Option<f64> },
    /// Calibration finished; offset now in force.
    CalibrationComplete { offset_s: f64, mad: f64 },
}

/// Latency calibration — port of `CalibrationSession` / `compute_offset`.
pub struct CalibrationSession {
    grid: BeatGrid,
    min_hits: usize,
    discard: usize,
    errors: Vec<f64>,
}

impl CalibrationSession {
    pub fn new(grid: BeatGrid) -> Self {
        CalibrationSession {
            grid,
            min_hits: CAL_MIN_HITS,
            discard: CAL_DISCARD,
            errors: Vec::new(),
        }
    }

    /// Offer one onset time (stream clock, pre-offset).
    pub fn add_hit(&mut self, onset_t: f64) {
        let n = self.grid.nearest_beat(onset_t);
        self.errors.push(onset_t - self.grid.beat_time(n));
    }

    /// Usable hits so far (after the discard window).
    pub fn hits(&self) -> usize {
        self.errors.len().saturating_sub(self.discard)
    }

    pub fn is_complete(&self) -> bool {
        self.hits() >= self.min_hits
    }

    /// `(offset_s, mad)`: median error after MAD rejection. Cold path —
    /// sorting here never runs on the audio thread.
    pub fn result(&self) -> (f64, f64) {
        compute_offset(&self.errors[self.discard.min(self.errors.len())..])
    }
}

/// Median + 3·MAD outlier rejection, then median of the kept errors.
/// Port of `calibrate.compute_offset`.
pub fn compute_offset(errors: &[f64]) -> (f64, f64) {
    if errors.is_empty() {
        return (0.0, 0.0);
    }
    let med = median(errors);
    let mut devs: Vec<f64> = errors.iter().map(|e| (e - med).abs()).collect();
    devs.sort_by(|a, b| a.total_cmp(b));
    let mad = median(&devs);
    let kept: Vec<f64> = if mad > 0.0 {
        errors
            .iter()
            .copied()
            .filter(|e| (e - med).abs() <= 3.0 * mad)
            .collect()
    } else {
        errors.to_vec()
    };
    (median(&kept), mad)
}

fn median(sorted_or_not: &[f64]) -> f64 {
    if sorted_or_not.is_empty() {
        return 0.0;
    }
    let mut v = sorted_or_not.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let m = v.len() / 2;
    if v.len() % 2 == 1 {
        v[m]
    } else {
        0.5 * (v[m - 1] + v[m])
    }
}

/// The live lane.
pub struct Session {
    onset: EnergyGate,
    pitch: MpmPitchDetector,
    pitch_frame: usize,
    /// Pre-block history for the pitch window. Fixed capacity after init.
    history: Vec<f32>,
    /// Scratch for one pitch frame. Fixed capacity after init.
    scratch: Vec<f32>,
    judge: Judge,
    follower: Follower,
    calibration: Option<CalibrationSession>,
    staged: EventBatch<CoachEvent, MAX_SESSION_EVENTS>,
    miss_scratch: Vec<Judgment>,
    sample_rate: f64,
    offset_s: f64,
}

impl Session {
    /// Build a session over a chart. All RT buffers pre-sized here.
    pub fn new(chart: Vec<ExpectedEvent>, config: SessionConfig) -> Self {
        let pitch = MpmPitchDetector::default_48k();
        let pitch_frame = pitch.frame_len();
        let mut scratch = Vec::with_capacity(pitch_frame);
        scratch.resize(pitch_frame, 0.0);
        let mut history = Vec::with_capacity(PITCH_HISTORY + 8192);
        history.clear();
        let mut judge = Judge::new(chart, config.latency_offset_s, 0, 0.100, 0.05);
        judge.set_feedback_mode(config.feedback_mode);
        let mut follower = Follower::from_expected(judge.expected());
        follower.set_offset(config.latency_offset_s);
        follower.set_wait_for_me(config.wait_for_me, 0.250);
        Session {
            onset: EnergyGate::default_48k(),
            pitch,
            pitch_frame,
            history,
            scratch,
            judge,
            follower,
            calibration: None,
            staged: EventBatch::new(),
            miss_scratch: Vec::with_capacity(8),
            sample_rate: config.sample_rate as f64,
            offset_s: config.latency_offset_s,
        }
    }

    /// Skip the first `n` chart events (count-in). Must precede audio.
    pub fn set_count_in(&mut self, first_index: usize) {
        let expected = self.judge.expected().to_vec();
        let mode = self.judge.feedback_mode();
        let mut judge = Judge::new(expected, self.offset_s, first_index, 0.100, 0.05);
        judge.set_feedback_mode(mode);
        self.judge = judge;
    }

    /// Live input-latency offset (calibration estimate + refinements).
    pub fn latency_offset(&self) -> f64 {
        self.offset_s
    }

    /// Begin first-run calibration against a click grid ("strum chugs
    /// against the click — we're measuring your gear's delay"). While
    /// calibrating, onsets feed the calibration session, not the judge.
    pub fn start_calibration(&mut self, grid: BeatGrid) {
        self.calibration = Some(CalibrationSession::new(grid));
    }

    pub fn calibrating(&self) -> bool {
        self.calibration.is_some()
    }

    pub fn calibration_hits(&self) -> usize {
        self.calibration.as_ref().map(|c| c.hits()).unwrap_or(0)
    }

    /// Finish calibration: the offset goes live on the judge + follower.
    /// Returns `(offset_s, mad)`, or `None` if not calibrating.
    pub fn complete_calibration(&mut self) -> Option<(f64, f64)> {
        let cal = self.calibration.take()?;
        let (offset, mad) = cal.result();
        self.set_full_offset(offset);
        self.staged.push(CoachEvent::CalibrationComplete {
            offset_s: offset,
            mad,
        });
        Some((offset, mad))
    }

    fn set_full_offset(&mut self, offset: f64) {
        self.judge.set_offset(offset);
        self.follower.set_offset(offset);
        self.offset_s = offset;
    }

    /// Feed one audio block; drain the resulting UI events.
    /// Allocation-free after [`Session::new`].
    pub fn push_samples(
        &mut self,
        samples: &[f32],
        t_first: f64,
    ) -> BatchDrain<'_, CoachEvent, MAX_SESSION_EVENTS> {
        // Drain the detector first (bounded: ≤ MAX_STAGED per call), so the
        // detector borrow ends before judging touches `self` again.
        let mut onsets = [None; crate::onset::MAX_STAGED];
        let mut n_onsets = 0;
        for onset in self.onset.push_samples(samples, t_first) {
            if n_onsets < onsets.len() {
                onsets[n_onsets] = Some(onset);
                n_onsets += 1;
            }
        }
        let hop = self.onset.hop();
        for i in 0..n_onsets {
            let onset = onsets[i].expect("staged onset");
            self.staged.push(CoachEvent::Onset {
                t: onset.t,
                strength: onset.strength,
            });
            if self.calibration.is_some() {
                if let Some(cal) = self.calibration.as_mut() {
                    cal.add_hit(onset.t);
                }
                continue;
            }
            let hit = self.pitch_for_onset(samples, t_first, onset.t, hop);
            let verdict = self.judge.judge_hit(hit);
            let matched = self
                .follower
                .observe(onset.t)
                .unwrap_or(self.follower.position());
            self.staged.push(CoachEvent::Verdict(verdict));
            self.staged.push(CoachEvent::Position {
                matched,
                next_t: self.follower.next_event_t(),
            });
        }
        // Roll history forward (bounded memmove, never grows: a block at
        // or above capacity replaces history with its own tail).
        if samples.len() >= PITCH_HISTORY {
            self.history.clear();
            self.history
                .extend_from_slice(&samples[samples.len() - PITCH_HISTORY..]);
        } else {
            self.history.extend_from_slice(samples);
            if self.history.len() > PITCH_HISTORY {
                let excess = self.history.len() - PITCH_HISTORY;
                self.history.drain(..excess);
            }
        }
        // Sweeps trail the block end by the judge's margin, so in-flight
        // onsets still sitting in the queue can claim their notes.
        let t_end = t_first + samples.len() as f64 / self.sample_rate;
        self.miss_scratch.clear();
        self.judge.sweep_into(t_end, &mut self.miss_scratch);
        for miss in self.miss_scratch.drain(..) {
            if matches!(miss.grade, Grade::Miss) {
                let next = miss.expected_index + 1;
                if next > self.follower.position() {
                    self.follower.resync_to(next);
                }
            }
            self.staged.push(CoachEvent::Verdict(miss));
        }
        self.staged.drain()
    }

    /// Drain events staged outside `push_samples` (MIDI path, calibration).
    pub fn drain_staged(&mut self) -> BatchDrain<'_, CoachEvent, MAX_SESSION_EVENTS> {
        self.staged.drain()
    }

    /// Trivially-accurate judging for hex-pickup/MIDI owners: exact pitch,
    /// full confidence. Shared with the `midi` input path.
    pub fn midi_note_on(&mut self, midi_note: u8, t_secs: f64) -> Judgment {
        let hit = DetectedHit {
            t_secs,
            midi: Some(midi_note as f32),
            clarity: 1.0,
        };
        let verdict = self.judge.judge_hit(hit);
        let matched = self
            .follower
            .observe(t_secs)
            .unwrap_or(self.follower.position());
        self.staged.push(CoachEvent::Verdict(verdict));
        self.staged.push(CoachEvent::Position {
            matched,
            next_t: self.follower.next_event_t(),
        });
        verdict
    }

    /// Pitch-confirm one onset: the `pitch_frame` samples ending at the
    /// onset hop's end (history tail + current block head), copied into
    /// reusable scratch.
    fn pitch_for_onset(
        &mut self,
        block: &[f32],
        t_first: f64,
        onset_t: f64,
        hop: usize,
    ) -> DetectedHit {
        let mut idx = ((onset_t - t_first) * self.sample_rate).round() as isize + hop as isize;
        idx = idx.clamp(0, block.len() as isize);
        let end = idx as usize;
        let need = self.pitch_frame;
        // Fill scratch from history tail + block head (no allocation).
        let from_block = end.min(need);
        let from_hist = need - from_block;
        let hlen = self.history.len();
        let htake = from_hist.min(hlen);
        let mut w = 0;
        for &s in &self.history[hlen - htake..] {
            self.scratch[w] = s;
            w += 1;
        }
        // Zero-pad any missing history (session start).
        while w < from_hist {
            self.scratch[w] = 0.0;
            w += 1;
        }
        for &s in &block[end - from_block..end] {
            self.scratch[w] = s;
            w += 1;
        }
        let (midi, clarity) =
            match self
                .pitch
                .detect(&self.scratch, self.sample_rate as u32, onset_t)
            {
                Some(est) => (Some(est.midi), est.clarity),
                None => (None, 0.0),
            };
        DetectedHit {
            t_secs: onset_t,
            midi,
            clarity,
        }
    }

    // --- stats / state passthrough ---

    pub fn accuracy(&self) -> Option<f64> {
        self.judge.accuracy()
    }

    pub fn streak(&self) -> u32 {
        self.judge.streak()
    }

    pub fn best_streak(&self) -> u32 {
        self.judge.best_streak()
    }

    pub fn mean_error(&self) -> Option<f64> {
        self.judge.mean_error()
    }

    pub fn count(&self, grade: Grade) -> u64 {
        self.judge.count(grade)
    }

    pub fn chart_position(&self) -> usize {
        self.follower.position()
    }

    /// Wait-for-me gate for the transport (identity unless enabled).
    pub fn gate_chart_time(&self, song_t: f64) -> f64 {
        self.follower.gate_chart_time(song_t)
    }
}
