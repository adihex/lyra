//! Offline waveform peaks for the seekbar — computed at import time, stored
//! in the DB (BitMuse's `trackWaveform` equivalent), O(width) memory.

/// Min/max pair per display bucket.
#[derive(Debug, Clone, Copy, Default)]
pub struct Peak {
    pub min: f32,
    pub max: f32,
}

pub struct WaveformPeaks {
    pub peaks: Vec<Peak>,
    pub sample_rate: f32,
    /// Samples (per channel) each bucket covers.
    pub samples_per_bucket: u64,
}

impl WaveformPeaks {
    /// Compute `width` peaks from interleaved stereo. Two passes folded into
    /// one — reads each sample once, streaming-friendly for huge files.
    pub fn compute(interleaved: &[f32], channels: usize, width: usize, sample_rate: f32) -> Self {
        let frames = interleaved.len() / channels;
        let per_bucket = (frames / width.max(1)).max(1);
        let mut peaks = Vec::with_capacity(width);

        for b in 0..width {
            let start = b * per_bucket;
            let end = ((b + 1) * per_bucket).min(frames);
            let mut p = Peak { min: f32::MAX, max: f32::MIN };
            for f in start..end {
                // mono-downmix for the envelope
                let mut m = 0f32;
                for c in 0..channels {
                    m += interleaved[f * channels + c];
                }
                m /= channels as f32;
                p.min = p.min.min(m);
                p.max = p.max.max(m);
            }
            if p.min > p.max {
                p = Peak::default(); // empty tail bucket
            }
            peaks.push(p);
        }

        Self { peaks, sample_rate, samples_per_bucket: per_bucket as u64 }
    }

    /// Seekbar x → time, for click-to-seek.
    pub fn bucket_to_secs(&self, bucket: usize) -> f64 {
        bucket as f64 * self.samples_per_bucket as f64 / self.sample_rate as f64
    }
}
