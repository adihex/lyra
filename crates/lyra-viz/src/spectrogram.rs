//! Waterfall spectrogram: bounded ring of spectrum frames, newest at front.

use crate::spectrum::{SpectrumAnalyzer, SpectrumConfig};
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
            analyzer: SpectrumAnalyzer::new(SpectrumConfig {
                sample_rate,
                fft_size,
                bands,
                f_hi: 22_000.0,
                db_floor: -90.0,
                attack: 0.9,
                decay: 0.6,
                ..SpectrumConfig::default()
            }),
            rows: VecDeque::with_capacity(width),
            capacity: width,
            bands,
        }
    }

    /// Feed interleaved stereo; appends one waterfall column per full window.
    pub fn push(&mut self, interleaved: &[f32]) -> usize {
        self.analyzer.accumulate(interleaved);
        let mut added = 0;
        while self.analyzer.drain_window() {
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
