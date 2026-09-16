//! `lyra-coach` — the LIVE input lane of Lyra's music-coach stack.
//!
//! Real-time onset + pitch detection on the player's guitar input, a Judge,
//! and a score follower. Ported from the dino-shred Python prototype
//! (`dino_shred/audio/detect.py`, `dino_shred/rhythm/{judge,conductor,
//! calibrate}.py`; ADRs 0003/0004), rewritten as allocation-free-after-init
//! Rust: input arrives via [`OnsetDetector::push_samples`] (the HAL/duplex
//! capture side is macOS's problem, not this crate's).
//!
//! Layout:
//! - [`conductor`] — drift-free beat grid (fixed tempo or beat table).
//! - [`onset`] — EnergyGate onset detector (ADR 0003, stage 1).
//! - [`pitch`] — `pitch-detection` MPM/YIN + SwiftF0 stub (`onnx` feature).
//! - [`judge`] — judgment windows ±30/±60/±100 ms (ADR 0004).
//! - [`follower`] — windowed monotonic score follower ("wait-for-me").
//! - [`session`] — ties the lane together; calibration; [`CoachEvent`] stream.
//! - [`midi_in`] — hex-pickup MIDI input (`midi` feature).

pub mod batch;
pub mod conductor;
pub mod follower;
pub mod judge;
pub mod onset;
pub mod pitch;
pub mod session;

#[cfg(feature = "midi")]
pub mod midi_in;

pub use conductor::BeatGrid;
pub use follower::Follower;
pub use judge::{
    DetectedHit, ExpectedEvent, FeedbackMode, Grade, Judge, Judgment, NotePolicy, GOOD_WINDOW,
    OK_WINDOW, PERFECT_WINDOW,
};
pub use onset::{EnergyGate, OnsetEvent};
pub use pitch::{MpmPitchDetector, PitchDetector, PitchEstimate, YinPitchDetector};
pub use session::{compute_offset, CalibrationSession, CoachEvent, Session, SessionConfig};
