//! Peak/RMS meters with PPM-style ballistics (fast attack, slow decay).
//! Per-channel — drives VU/peak bars and the clip indicator.

#[derive(Debug, Clone)]
pub struct Levels {
    peak: [f32; 2],
    rms_acc: [f64; 2],
    rms_n: u64,
    rms: [f32; 2],
    decay: f32, // per-block decay factor
    /// Clip latch — set when peak exceeds threshold, cleared by UI after read.
    clipped: bool,
    clip_threshold: f32,
}

impl Levels {
    /// `decay_per_block`: e.g. 0.85 gives the classic fall-off.
    pub fn new(decay_per_block: f32) -> Self {
        Self {
            peak: [0.0; 2],
            rms_acc: [0.0; 2],
            rms_n: 0,
            rms: [0.0; 2],
            decay: decay_per_block,
            clipped: false,
            clip_threshold: 1.0,
        }
    }

    /// Per-block update from interleaved stereo.
    pub fn push(&mut self, interleaved: &[f32]) {
        for frame in interleaved.chunks_exact(2) {
            for (ch, s) in frame.iter().enumerate() {
                let a = s.abs();
                if a > self.peak[ch] {
                    self.peak[ch] = a;
                }
                if a >= self.clip_threshold {
                    self.clipped = true;
                }
                self.rms_acc[ch] += (a * a) as f64;
            }
            self.rms_n += 1;
        }
        // decay peaks once per block — call rate ≈ block cadence
        for p in &mut self.peak {
            *p *= self.decay;
        }
    }

    /// Peak (post-decay) and block RMS per channel, dBFS.
    pub fn read(&mut self) -> ([f32; 2], [f32; 2]) {
        if self.rms_n > 0 {
            for ch in 0..2 {
                self.rms[ch] = ((self.rms_acc[ch] / self.rms_n as f64).sqrt() as f32)
                    .max(1e-9)
                    .log10()
                    * 20.0;
            }
            self.rms_acc = [0.0; 2];
            self.rms_n = 0;
        }
        let peak_db = [
            self.peak[0].max(1e-9).log10() * 20.0,
            self.peak[1].max(1e-9).log10() * 20.0,
        ];
        (peak_db, self.rms)
    }

    /// True if any sample hit/exceeded 0 dBFS since last call.
    pub fn take_clip(&mut self) -> bool {
        std::mem::replace(&mut self.clipped, false)
    }
}
