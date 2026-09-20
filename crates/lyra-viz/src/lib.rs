//! lyra-viz: visualization taps — spectrum, spectrogram, waveform, meters,
//! oscilloscope, beat. All run on the DSP thread's tap point and emit
//! compact, draw-ready data (band dB values, peak rows) — the UI never
//! sees raw PCM.
//!
//! Memory discipline: everything here is fixed-budget. Rings are bounded,
//! the FFT buffer is allocated once, and nothing grows per track.

mod beat;
mod frame;
mod level;
mod osc;
mod spectrogram;
mod spectrum;
mod waveform;

pub use beat::BeatDetect;
pub use frame::VizFrame;
pub use level::Levels;
pub use osc::Oscilloscope;
pub use spectrogram::Spectrogram;
pub use spectrum::{SpectrumAnalyzer, SpectrumConfig};
pub use waveform::WaveformPeaks;
