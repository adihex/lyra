//! lyra-viz: visualization taps — spectrum, spectrogram, waveform, meters,
//! oscilloscope. All run on the DSP thread's tap point and emit compact,
//! draw-ready data (band dB values, peak rows) — the UI never sees raw PCM.
//!
//! Memory discipline: everything here is fixed-budget. Rings are bounded,
//! the FFT buffer is allocated once, and nothing grows per track.

mod level;
mod osc;
mod spectrum;
mod spectrogram;
mod waveform;

pub use level::Levels;
pub use osc::Oscilloscope;
pub use spectrum::SpectrumAnalyzer;
pub use spectrogram::Spectrogram;
pub use waveform::WaveformPeaks;
