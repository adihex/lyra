//! EnergyGate onset detector — port of `dino_shred/audio/detect.py` (ADR 0003, stage 1).
//!
//! Per-hop RMS in dB with a rising-edge threshold, hysteresis re-arming, and
//! a refractory period. Runs inside the audio callback: no allocation after
//! [`EnergyGate::new`], O(hop) work per hop, all state is a few floats plus
//! one hop-sized scratch buffer.
//!
//! Fidelity vs Python: identical arithmetic (f64 accumulation,
//! `20·log10(rms + 1e-10)`), identical state machine (disarm on crossing,
//! emit only outside the refractory window, re-arm below
//! `threshold − hysteresis`). Event timestamps are the stream-clock time of
//! the triggering hop's first sample, exactly like `t_frame` in `process()`.

use crate::batch::{BatchDrain, EventBatch};

/// An onset: stream-clock seconds of the hit + dB above threshold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OnsetEvent {
    /// Stream-clock seconds of the triggering hop's first sample.
    pub t: f64,
    /// dB above threshold (`rms_db − threshold_db`).
    pub strength: f32,
    /// Which detector fired (always `"energy"` here).
    pub detector: &'static str,
}

/// Max events one [`EnergyGate::push_samples`] call can stage.
///
/// The 80 ms refractory period bounds real output to ~1 event per call at
/// audio block sizes; 8 is headroom, never allocated (inline array).
pub const MAX_STAGED: usize = 8;

/// Energy-gate onset detector. See module docs.
pub struct EnergyGate {
    sample_rate: f64,
    hop: usize,
    threshold_db: f64,
    hysteresis_db: f64,
    refractory_s: f64,
    armed: bool,
    last_onset_t: f64,
    /// Hop accumulator. Capacity fixed at `hop` from construction; only
    /// `len` moves in the RT path.
    buf: Vec<f32>,
    /// Stream-clock time of `buf[0]`.
    t_buf_start: f64,
    staged: EventBatch<OnsetEvent, MAX_STAGED>,
}

impl EnergyGate {
    /// Build a detector. Allocates the hop buffer once, here.
    pub fn new(
        sample_rate: u32,
        hop: usize,
        threshold_db: f32,
        hysteresis_db: f32,
        refractory_s: f64,
    ) -> Self {
        assert!(hop > 0, "hop must be non-zero");
        let mut buf = Vec::with_capacity(hop);
        buf.resize(hop, 0.0);
        buf.clear();
        EnergyGate {
            sample_rate: sample_rate as f64,
            hop,
            threshold_db: threshold_db as f64,
            hysteresis_db: hysteresis_db as f64,
            refractory_s,
            armed: true,
            last_onset_t: -1e9,
            buf,
            t_buf_start: 0.0,
            staged: EventBatch::new(),
        }
    }

    /// Python defaults: 48 kHz, 256-sample hop (5.33 ms), −30 dB gate,
    /// 6 dB hysteresis, 80 ms refractory.
    pub fn default_48k() -> Self {
        EnergyGate::new(48_000, 256, -30.0, 6.0, 0.08)
    }

    pub fn hop(&self) -> usize {
        self.hop
    }

    /// Direct port of `EnergyGate.process(frame, t_frame)`: classify one
    /// full hop. `t_frame` is the stream-clock time of `frame[0]`.
    pub fn process_frame(&mut self, frame: &[f32], t_frame: f64) -> Option<OnsetEvent> {
        debug_assert_eq!(frame.len(), self.hop);
        let (thr, hys, refr) = (self.threshold_db, self.hysteresis_db, self.refractory_s);
        Self::classify_static(
            frame,
            t_frame,
            &mut self.armed,
            &mut self.last_onset_t,
            thr,
            hys,
            refr,
        )
    }

    /// Feed arbitrary-length input; hop-sized frames are classified as they
    /// fill. `t_first` is the stream-clock time of `samples[0]`; partial-hop
    /// leftovers carry over to the next call with their timestamps intact.
    ///
    /// Allocation-free: returns a drain over an inline staging array.
    /// If the input stream jumps (non-contiguous `t_first` with a partial
    /// hop pending), the stale partial hop is dropped and the clock resyncs.
    ///
    /// Consume the drain promptly: the next `push_samples` call resets the
    /// stage and discards any unconsumed events.
    pub fn push_samples(&mut self, samples: &[f32], t_first: f64) -> OnsetDrain<'_> {
        if self.buf.is_empty() {
            self.t_buf_start = t_first;
        } else {
            let expected = self.t_buf_start + self.buf.len() as f64 / self.sample_rate;
            if (t_first - expected).abs() > 0.5 / self.sample_rate {
                self.buf.clear();
                self.t_buf_start = t_first;
            }
        }
        let hop_dt = self.hop as f64 / self.sample_rate;
        let (thr, hys, refr) = (self.threshold_db, self.hysteresis_db, self.refractory_s);
        for &s in samples {
            self.buf.push(s);
            if self.buf.len() == self.hop {
                // Static classifier so the frame borrow and the state
                // borrow never alias.
                let t_frame = self.t_buf_start;
                let ev = Self::classify_static(
                    &self.buf,
                    t_frame,
                    &mut self.armed,
                    &mut self.last_onset_t,
                    thr,
                    hys,
                    refr,
                );
                self.t_buf_start += hop_dt;
                self.buf.clear();
                if let Some(ev) = ev {
                    self.staged.push(ev);
                }
            }
        }
        self.staged.drain()
    }

    /// The `process_frame` state machine as a static fn so `push_samples`
    /// can run it without double-borrowing `self.buf`.
    #[allow(clippy::too_many_arguments)]
    fn classify_static(
        frame: &[f32],
        t_frame: f64,
        armed: &mut bool,
        last_onset_t: &mut f64,
        threshold_db: f64,
        hysteresis_db: f64,
        refractory_s: f64,
    ) -> Option<OnsetEvent> {
        let mut sum = 0.0f64;
        for &s in frame {
            let s = s as f64;
            sum += s * s;
        }
        let rms = (sum / frame.len() as f64).sqrt();
        let rms_db = 20.0 * (rms + 1e-10).log10();

        if *armed && rms_db >= threshold_db {
            *armed = false;
            if t_frame - *last_onset_t >= refractory_s {
                *last_onset_t = t_frame;
                return Some(OnsetEvent {
                    t: t_frame,
                    strength: (rms_db - threshold_db) as f32,
                    detector: "energy",
                });
            }
        } else if !*armed && rms_db < threshold_db - hysteresis_db {
            *armed = true;
        }
        None
    }
}

/// Drain over the events staged by one [`EnergyGate::push_samples`] call.
pub type OnsetDrain<'a> = BatchDrain<'a, OnsetEvent, MAX_STAGED>;
