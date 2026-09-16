//! Waterfall spectrogram: bounded ring of spectrum frames, newest at front.

use crate::spectrum::SpectrumAnalyzer;
use std::collections::VecDeque;

pub struct Spectrogram {
    analyzer: SpectrumAnalyzer,
    rows: VecDeque<Vec<f32>>, // normalized rows
    capacity: usize,
    bands: usize,
}

impl Spectrogram {
    /// `width` = retained time columns. Each pushed column is one FFT frame.
    pub fn new(sample_rate: f32, fft_size: usize, bands: usize, width: usize) -> Self {
        Self {
            analyzer: SpectrumAnalyzer::new(
                sample_rate, fft_size, bands, 20.0, 22_000.0, -90.0, 0.9, 0.6,
            ),
            rows: VecDeque::with_capacity(width),
            capacity: width,
            bands,
        }
    }

    /// Feed interleaved stereo; appends one waterfall column per full window.
    pub fn push(&mut self, interleaved: &[f32]) -> usize {
        self.analyzer.accumulate(interleaved);
        let mut added = 0;
        while self.analyzer.next() {
            let mut row = vec![0.0; self.bands];
            self.analyzer.normalized_into(&mut row);
            self.rows.push_front(row);
            if self.rows.len() > self.capacity {
                self.rows.pop_back();
            }
            added += 1;
        }
        added
    }

    /// Oldest→newest columns of normalized band values for the canvas.
    pub fn columns(&self) -> impl Iterator<Item = &Vec<f32>> {
        self.rows.iter().rev()
    }
}
