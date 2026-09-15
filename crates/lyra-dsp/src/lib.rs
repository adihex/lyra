//! lyra-dsp: real-time audio processing chain.
//!
//! Runs on the CoreAudio render thread — every node is allocation-free in
//! process(), so no locks, no allocs, no surprises at 192kHz.
//! Order: preamp → parametric EQ → crossfeed → gain-comp → limiter → dither.

/// RBJ-cookbook biquad. Stereo-interleaved processing, denormal-flushed.
#[derive(Debug, Clone)]
pub struct Biquad {
    b0: f32, b1: f32, b2: f32, a1: f32, a2: f32,
    z: [[f32; 2]; 2], // [channel][z1,z2]
}

impl Biquad {
    pub fn peaking_eq(sample_rate: f32, f0: f32, q: f32, db_gain: f32) -> Self {
        let a = 10f32.powf(db_gain / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * f0 / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let (cw0, a) = (w0.cos(), a);
        let norm = 1.0 / (1.0 + alpha / a);
        Self {
            b0: (1.0 + alpha * a) * norm,
            b1: (-2.0 * cw0) * norm,
            b2: (1.0 - alpha * a) * norm,
            a1: (-2.0 * cw0) * norm,
            a2: (1.0 - alpha / a) * norm,
            z: [[0.0; 2]; 2],
        }
    }

    pub fn low_shelf(sample_rate: f32, f0: f32, q: f32, db_gain: f32) -> Self {
        let a = 10f32.powf(db_gain / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * f0 / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cw0 = w0.cos();
        let sq = 2.0 * a.sqrt() * alpha;
        let norm = 1.0 / ((a + 1.0) + (a - 1.0) * cw0 + sq);
        Self {
            b0: a * ((a + 1.0) - (a - 1.0) * cw0 + sq) * norm,
            b1: 2.0 * a * ((a - 1.0) - (a + 1.0) * cw0) * norm,
            b2: a * ((a + 1.0) - (a - 1.0) * cw0 - sq) * norm,
            a1: -2.0 * ((a - 1.0) + (a + 1.0) * cw0) * norm,
            a2: ((a + 1.0) + (a - 1.0) * cw0 - sq) * norm,
            z: [[0.0; 2]; 2],
        }
    }

    /// One interleaved stereo frame.
    #[inline]
    pub fn process_frame(&mut self, frame: &mut [f32; 2]) {
        for (ch, x) in frame.iter_mut().enumerate() {
            let mut y = self.b0 * *x + self.z[ch][0];
            self.z[ch][0] = self.b1 * *x - self.a1 * y + self.z[ch][1];
            self.z[ch][1] = self.b2 * *x - self.a2 * y;
            // flush denormals — they cost 10-100x on the render thread
            if !y.is_normal() { y = 0.0; }
            *x = y;
        }
    }
}

/// A parametric band: one biquad + enable flag.
#[derive(Debug, Clone)]
pub struct EqBand {
    pub filter: Biquad,
    pub enabled: bool,
}

/// Chain of parametric bands — allocation-free process loop.
#[derive(Debug, Default)]
pub struct ParametricEq {
    pub bands: Vec<EqBand>,
}

impl ParametricEq {
    pub fn process(&mut self, interleaved: &mut [f32]) {
        for frame in interleaved.chunks_exact_mut(2) {
            let frame: &mut [f32; 2] = frame.try_into().unwrap();
            for band in &mut self.bands {
                if band.enabled {
                    band.filter.process_frame(frame);
                }
            }
        }
    }
}

/// Track gain in dB (ReplayGain / R128 target -18 LUFS-ish by default).
pub fn gain_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Simple lookahead-free peak limiter — catches clipping the EQ would cause.
/// Improvement over "auto preamp": protects without permanently lowering gain.
#[derive(Debug, Clone)]
pub struct SafetyLimiter {
    pub threshold: f32, // linear, e.g. 0.98
    env: f32,
    release: f32,
}

impl SafetyLimiter {
    pub fn new(sample_rate: f32, release_ms: f32) -> Self {
        Self {
            threshold: 0.98,
            env: 0.0,
            release: (-1.0 / (release_ms * 0.001 * sample_rate)).exp(),
        }
    }

    pub fn process(&mut self, interleaved: &mut [f32]) {
        for s in interleaved.iter_mut() {
            let peak = s.abs();
            self.env = if peak > self.env { peak } else { self.env * self.release };
            if self.env > self.threshold {
                *s *= self.threshold / self.env.max(1e-9);
            }
        }
    }
}
