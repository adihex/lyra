//! repr(C) mirror of the LyraVizFrame ABI (docs/VIZ-CONTRACT.md).
//! The tap keeps one fully-formed frame — the FFI read is lock+memcpy+seq.

/// ABI-matched frame; field order is the contract — do not reorder.
/// bands: 64 log-spaced 0..1 · wave: 256 newest-last -1..1 · clip: bit0 L bit1 R.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VizFrame {
    pub bands: [f32; 64],
    pub wave_l: [f32; 256],
    pub wave_r: [f32; 256],
    pub peak: [f32; 2],
    pub rms: [f32; 2],
    pub bass: f32,
    pub beat: f32,
    pub level: f32,
    pub clip: u32,
    pub seq: u64,
}

impl Default for VizFrame {
    fn default() -> Self {
        Self {
            bands: [0.0; 64],
            wave_l: [0.0; 256],
            wave_r: [0.0; 256],
            peak: [0.0; 2],
            rms: [0.0; 2],
            bass: 0.0,
            beat: 0.0,
            level: 0.0,
            clip: 0,
            seq: 0,
        }
    }
}
