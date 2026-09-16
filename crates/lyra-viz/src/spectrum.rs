//! Real-time spectrum analyzer: Hann-windowed FFT → log-frequency bands
//! with attack/decay ballistics (the classic analyzer look).

use rustfft::num_complex::Complex;
use rustfft::Fft;
use std::sync::Arc;

/// One frame of analyzer output: `bands` dB values, 0..≈0 after normalization.
pub struct SpectrumFrame {
    /// Per-band magnitude in dB, floor-clamped to `db_floor`.
    pub bands: Vec<f32>,
    pub db_floor: f32,
}

pub struct SpectrumAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    fft_size: usize,
    window: Vec<f32>,
    scratch: Vec<Complex<f32>>,
    /// (first_bin, last_bin) per output band — geometric spacing.
    band_bins: Vec<(usize, usize)>,
    /// Smoothed output held between frames (attack fast, decay slow).
    smoothed: Vec<f32>,
    /// Mono downmix accumulator — bounded by the drain in `next`.
    accum: Vec<f32>,
    db_floor: f32,
    attack: f32,  // 0..1, applied when new value is higher
    decay: f32,   // 0..1, applied when new value is lower
}

impl SpectrumAnalyzer {
    /// `fft_size`: 2048/4096 typical. `bands`: e.g. 64. `sample_rate` sets
    /// the frequency mapping. `attack`/`decay` are per-frame lerp factors.
    pub fn new(
        sample_rate: f32,
        fft_size: usize,
        bands: usize,
        f_lo: f32,
        f_hi: f32,
        db_floor: f32,
        attack: f32,
        decay: f32,
    ) -> Self {
        let fft = rustfft::FftPlanner::new().plan_fft_forward(fft_size);
        let window: Vec<f32> = (0..fft_size)
            .map(|i| {
                0.5 - 0.5
                    * (2.0 * std::f32::consts::PI * i as f32 / (fft_size - 1) as f32).cos()
            })
            .collect();

        // Geometric band edges: f_lo..f_hi → bin ranges.
        let bin_hz = sample_rate / fft_size as f32;
        let max_bin = fft_size / 2;
        let mut band_bins = Vec::with_capacity(bands);
        for b in 0..bands {
            let f0 = f_lo * (f_hi / f_lo).powf(b as f32 / bands as f32);
            let f1 = f_lo * (f_hi / f_lo).powf((b + 1) as f32 / bands as f32);
            let i0 = ((f0 / bin_hz) as usize).clamp(1, max_bin - 1);
            let i1 = ((f1 / bin_hz) as usize).clamp(i0 + 1, max_bin);
            band_bins.push((i0, i1));
        }

        Self {
            fft,
            fft_size,
            window,
            scratch: vec![Complex::ZERO; fft_size],
            band_bins,
            smoothed: vec![db_floor; bands],
            accum: Vec::with_capacity(fft_size * 2),
            db_floor,
            attack,
            decay,
        }
    }

    /// Downmix interleaved stereo into the accumulator without computing —
    /// pair with `next()` to drain window by window.
    pub fn accumulate(&mut self, interleaved: &[f32]) {
        for frame in interleaved.chunks_exact(2) {
            self.accum.push((frame[0] + frame[1]) * 0.5);
        }
    }

    /// Compute one pending window into `smoothed`. False while accum holds
    /// less than fft_size. Alloc-free.
    pub fn next(&mut self) -> bool {
        if self.accum.len() < self.fft_size {
            return false;
        }
        // move accum out so the window read doesn't alias &mut self
        let mut acc = std::mem::take(&mut self.accum);
        self.compute_window(&acc[..self.fft_size]);
        acc.drain(..self.fft_size);
        self.accum = acc;
        true
    }

    /// Feed interleaved stereo; runs one FFT per full window. Returns
    /// windows completed. Alloc-free after the accumulator's first grow.
    pub fn feed(&mut self, interleaved: &[f32]) -> usize {
        self.accumulate(interleaved);
        let mut n = 0;
        while self.next() {
            n += 1;
        }
        n
    }

    /// Window + FFT + per-band peak + ballistics on exactly `fft_size`
    /// mono samples → `smoothed`. NaN/inf inputs clamp to the floor/ceiling
    /// instead of poisoning the smoothing state.
    fn compute_window(&mut self, mono: &[f32]) {
        for (i, s) in self.scratch.iter_mut().enumerate() {
            *s = Complex::new(mono[i] * self.window[i], 0.0);
        }
        self.fft.process(&mut self.scratch);

        for (b, &(i0, i1)) in self.band_bins.iter().enumerate() {
            // Per-band peak magnitude (not mean — peaks read better on music).
            let mut peak = 0f32;
            for c in &self.scratch[i0..i1] {
                peak = peak.max(c.norm());
            }
            // Normalize: window coherent gain ≈ 0.5, fft gain = N.
            let mag = peak / (self.fft_size as f32 * 0.25);
            let db = (20.0 * mag.max(1e-12).log10()).clamp(self.db_floor, 120.0);
            let prev = self.smoothed[b];
            let k = if db > prev { self.attack } else { self.decay };
            self.smoothed[b] = prev + (db - prev) * k;
        }
    }

    /// Compute one spectrum frame from exactly `fft_size` mono samples.
    pub fn compute(&mut self, mono: &[f32]) -> SpectrumFrame {
        self.compute_window(mono);
        SpectrumFrame {
            bands: self.smoothed.clone(),
            db_floor: self.db_floor,
        }
    }

    /// Current smoothed bands without feeding new audio — for UI polls
    /// between FFT completions.
    pub fn peek(&self) -> SpectrumFrame {
        SpectrumFrame {
            bands: self.smoothed.clone(),
            db_floor: self.db_floor,
        }
    }

    /// Normalized 0..1 bands into a caller buffer — the alloc-free path.
    /// Returns bands written.
    pub fn normalized_into(&self, out: &mut [f32]) -> usize {
        let n = self.smoothed.len().min(out.len());
        for (o, &db) in out[..n].iter_mut().zip(self.smoothed.iter()) {
            *o = ((db - self.db_floor) / -self.db_floor).clamp(0.0, 1.0);
        }
        n
    }

    /// Normalized 0..1 band values for direct drawing.
    pub fn normalized(frame: &SpectrumFrame) -> Vec<f32> {
        frame
            .bands
            .iter()
            .map(|db| ((db - frame.db_floor) / -frame.db_floor).clamp(0.0, 1.0))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_peaks_in_its_band() {
        let sr = 48000.0;
        let n = 4096;
        let freq = 1000.0;
        let mut a = SpectrumAnalyzer::new(sr, n, 48, 20.0, 20000.0, -80.0, 1.0, 1.0);
        let mono: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin() * 0.5)
            .collect();
        let frame = a.compute(&mono);
        let norm = SpectrumAnalyzer::normalized(&frame);
        let peak_idx = norm
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        // 1kHz in 48 geometric bands 20..20k: 20·1000^(b/48)=1k → b≈27.2.
        assert!((25..=29).contains(&peak_idx), "peak at band {peak_idx}");
        assert!(norm[peak_idx] > 0.5);
    }

    #[test]
    fn feed_drains_all_complete_windows() {
        let mut a = SpectrumAnalyzer::new(48_000.0, 1024, 16, 20.0, 20_000.0, -80.0, 1.0, 1.0);
        let stereo = vec![0.5f32; 1024 * 3 * 2];
        assert_eq!(a.feed(&stereo), 3);
        assert!(a.accum.len() < 1024);
    }
}
