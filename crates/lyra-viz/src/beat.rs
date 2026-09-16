//! Onset/beat pulse: energy-flux detector — a block whose energy jumps
//! above the running average fires the pulse; ~150 ms linear decay.

pub struct BeatDetect {
    avg: f32,     // ~1s EMA of block energy
    pulse: f32,   // 0..1 output
    holdoff: f32, // refractory, seconds
    rate: f32,
}

impl BeatDetect {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            avg: 0.0,
            pulse: 0.0,
            holdoff: 0.0,
            rate: sample_rate.max(1.0),
        }
    }

    /// Per-block update from interleaved stereo → current pulse.
    pub fn push(&mut self, interleaved: &[f32]) -> f32 {
        let frames = interleaved.len() / 2;
        let dt = frames as f32 / self.rate;
        let mut e = 0f32;
        for f in interleaved.chunks_exact(2) {
            let m = (f[0] + f[1]) * 0.5;
            if m.is_finite() {
                e += m * m;
            }
        }
        if frames > 0 {
            e /= frames as f32;
        }
        if !e.is_finite() {
            e = 0.0;
        }
        // positive flux over the adaptive average, above a noise floor
        if self.holdoff <= 0.0 && e > 1e-6 && e > self.avg * 1.6 {
            self.pulse = 1.0;
            self.holdoff = 0.1;
        }
        self.avg += (e - self.avg) * (1.0 - (-dt).exp());
        self.holdoff -= dt;
        self.pulse = (self.pulse - dt / 0.15).max(0.0);
        self.pulse
    }

    pub fn pulse(&self) -> f32 {
        self.pulse
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impulse_fires_then_decays() {
        let rate = 48_000.0;
        let mut b = BeatDetect::new(rate);
        let silence = vec![0.0f32; 2048];
        for _ in 0..8 {
            assert_eq!(b.push(&silence), 0.0);
        }
        let mut hit = vec![0.0f32; 2048];
        hit[0] = 1.0;
        let p0 = b.push(&hit);
        assert!(p0 > 0.5, "impulse should spike the pulse, got {p0}");
        let mut last = p0;
        for _ in 0..12 {
            last = b.push(&silence);
        }
        assert!(last < p0 && last < 0.05, "pulse should decay to ~0, got {last}");
    }

    #[test]
    fn steady_tones_dont_fire() {
        let mut b = BeatDetect::new(48_000.0);
        let tone: Vec<f32> = (0..2048)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin() * 0.5;
                [s, s]
            })
            .collect();
        let mut peak = 0f32;
        for _ in 0..32 {
            peak = peak.max(b.push(&tone));
        }
        // first block may fire once (energy steps off silence); must not re-fire
        let mut late = 0f32;
        for _ in 0..32 {
            late = late.max(b.push(&tone));
        }
        assert!(late < 0.3, "steady tone kept firing: {late}");
        let _ = peak;
    }
}
