//! Peak/RMS meters with PPM-style ballistics (fast attack, slow decay).
//! Per-channel — drives VU/peak bars and the clip indicator.

#[derive(Debug, Clone)]
pub struct Levels {
    peak: [f32; 2],
    /// Last-block RMS per channel, dBFS. Block-cadence, not read-cadence —
    /// every consumer sees the same window regardless of poll rate.
    rms: [f32; 2],
    decay: f32, // per-block decay factor
    /// Sticky clip latch — bit per channel, set at ≥ threshold, cleared by
    /// take_clip/clear_clip.
    clip: u8,
    /// Clip bits seen during the last push — feeds the frame's hold timer.
    block_clip: u8,
    clip_threshold: f32,
}

impl Levels {
    /// `decay_per_block`: e.g. 0.85 gives the classic fall-off.
    pub fn new(decay_per_block: f32) -> Self {
        Self {
            peak: [0.0; 2],
            rms: [-160.0; 2],
            decay: decay_per_block,
            clip: 0,
            block_clip: 0,
            clip_threshold: 1.0,
        }
    }

    /// Per-block update from interleaved stereo.
    pub fn push(&mut self, interleaved: &[f32]) {
        self.block_clip = 0;
        let mut acc = [0f64; 2];
        let mut n = 0u64;
        for frame in interleaved.as_chunks::<2>().0 {
            for (ch, s) in frame.iter().enumerate() {
                let a = s.abs();
                if !a.is_finite() {
                    continue;
                }
                if a > self.peak[ch] {
                    self.peak[ch] = a;
                }
                if a >= self.clip_threshold {
                    self.clip |= 1 << ch;
                    self.block_clip |= 1 << ch;
                }
                acc[ch] += (a * a) as f64;
            }
            n += 1;
        }
        if n > 0 {
            for (rms, &a) in self.rms.iter_mut().zip(acc.iter()) {
                *rms = ((a / n as f64).sqrt() as f32).max(1e-9).log10() * 20.0;
            }
        }
        // decay peaks once per block — call rate ≈ block cadence
        for p in &mut self.peak {
            *p *= self.decay;
        }
    }

    /// Peak (post-decay) and last-block RMS per channel, dBFS.
    pub fn read(&self) -> ([f32; 2], [f32; 2]) {
        let peak_db = [
            self.peak[0].max(1e-9).log10() * 20.0,
            self.peak[1].max(1e-9).log10() * 20.0,
        ];
        (peak_db, self.rms)
    }

    /// Clip bits set during the last `push` (bit0 L, bit1 R).
    pub fn last_clip(&self) -> u8 {
        self.block_clip
    }

    /// True if any sample hit/exceeded 0 dBFS since last call — drains the
    /// latch.
    pub fn take_clip(&mut self) -> bool {
        std::mem::replace(&mut self.clip, 0) != 0
    }
}
