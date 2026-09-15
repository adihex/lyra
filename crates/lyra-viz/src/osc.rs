//! Oscilloscope + Lissajous trace: latest N mono/stereo samples, decimated
//! to a fixed draw width. Bounded ring — constant memory.

use std::collections::VecDeque;

pub struct Oscilloscope {
    left: VecDeque<f32>,
    right: VecDeque<f32>,
    capacity: usize,
    /// Keep every nth sample — set ≈ sample_rate / (width × fps) for a stable trace.
    stride: u32,
    counter: u32,
}

impl Oscilloscope {
    pub fn new(capacity: usize, stride: u32) -> Self {
        Self {
            left: VecDeque::with_capacity(capacity),
            right: VecDeque::with_capacity(capacity),
            capacity,
            stride: stride.max(1),
            counter: 0,
        }
    }

    pub fn push(&mut self, interleaved: &[f32]) {
        for frame in interleaved.chunks_exact(2) {
            self.counter += 1;
            if self.counter % self.stride != 0 {
                continue;
            }
            self.left.push_back(frame[0]);
            self.right.push_back(frame[1]);
            if self.left.len() > self.capacity {
                self.left.pop_front();
                self.right.pop_front();
            }
        }
    }

    /// L/R traces for drawing. Lissajous = plot left against right.
    pub fn traces(&self) -> (Vec<f32>, Vec<f32>) {
        (
            self.left.iter().copied().collect(),
            self.right.iter().copied().collect(),
        )
    }
}
