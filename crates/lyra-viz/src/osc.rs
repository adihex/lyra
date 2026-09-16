//! Oscilloscope + Lissajous trace: latest N mono/stereo samples, decimated
//! to a fixed draw width. Bounded ring — constant memory, no realloc:
//! eviction happens before push so len never exceeds capacity.

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
            self.counter = self.counter.wrapping_add(1);
            if self.counter % self.stride != 0 {
                continue;
            }
            if self.left.len() == self.capacity {
                self.left.pop_front();
                self.right.pop_front();
            }
            self.left.push_back(fin(frame[0]));
            self.right.push_back(fin(frame[1]));
        }
    }

    /// Copy the rings out newest-last into fixed buffers; zero-pads the
    /// front until the ring has seen `out.len()` decimated samples.
    pub fn copy_into(&self, l: &mut [f32], r: &mut [f32]) {
        copy_tail(&self.left, l);
        copy_tail(&self.right, r);
    }

    /// L/R traces for drawing. Lissajous = plot left against right.
    pub fn traces(&self) -> (Vec<f32>, Vec<f32>) {
        (
            self.left.iter().copied().collect(),
            self.right.iter().copied().collect(),
        )
    }
}

fn fin(s: f32) -> f32 {
    if s.is_finite() {
        s
    } else {
        0.0
    }
}

fn copy_tail(ring: &VecDeque<f32>, out: &mut [f32]) {
    let n = ring.len().min(out.len());
    let m = out.len() - n;
    out[..m].fill(0.0);
    for (o, &s) in out[m..].iter_mut().zip(ring.iter().skip(ring.len() - n)) {
        *o = s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_stays_at_capacity_newest_last() {
        let mut o = Oscilloscope::new(256, 1);
        // ramp: stereo frame i carries i*0.001 on both channels
        let pcm: Vec<f32> = (0..1000).flat_map(|i| [i as f32 * 0.001; 2]).collect();
        o.push(&pcm);
        let mut l = [0f32; 256];
        let mut r = [0f32; 256];
        o.copy_into(&mut l, &mut r);
        assert_eq!(l[255], 999.0 * 0.001);
        assert_eq!(l[0], 744.0 * 0.001);
        assert_eq!(l, r);
    }

    #[test]
    fn strided_and_pads_front_when_short() {
        let mut o = Oscilloscope::new(256, 4);
        let pcm: Vec<f32> = (0..400).flat_map(|i| [i as f32 * 0.001; 2]).collect();
        o.push(&pcm);
        let mut l = [1f32; 256];
        let mut r = [1f32; 256];
        o.copy_into(&mut l, &mut r);
        // kept: sample idx 3, 7, … 399 → 100 entries, tail-aligned
        assert!((l[255] - 0.399).abs() < 1e-6);
        assert!((l[156] - 0.003).abs() < 1e-6);
        assert!(l[..156].iter().all(|&s| s == 0.0));
    }
}
