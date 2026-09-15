//! Waterfall spectrogram: bounded ring of spectrum frames, newest at front.

use crate::spectrum::SpectrumAnalyzer;
use std::collections::VecDeque;

pub struct Spectrogram {
    analyzer: SpectrumAnalyzer,
    rows: VecDeque<Vec<f32>>, // normalized rows
    capacity: usize,
    accum: Vec<f32>,
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
            accum: Vec::with_capacity(fft_size * 2),
        }
    }

    /// Feed interleaved stereo; appends one waterfall column per full window.
    pub fn push(&mut self, interleaved: &mut [f32]) -> usize {
        let mut added = 0;
        // push() may complete multiple windows if the block is big.
        loop {
            match self.analyzer.push(interleaved, &mut self.accum) {
                Some(frame) => {
                    self.rows
                        .push_front(SpectrumAnalyzer::normalized(&frame));
                    if self.rows.len() > self.capacity {
                        self.rows.pop_back();
                    }
                    added += 1;
                }
                None => return added,
            }
        }
    }

    /// Oldest→newest columns of normalized band values for the canvas.
    pub fn columns(&self) -> impl Iterator<Item = &Vec<f32>> {
        self.rows.iter().rev()
    }
}
