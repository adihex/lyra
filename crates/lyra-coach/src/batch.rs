//! Fixed-capacity event staging for RT paths.
//!
//! Every per-block path in this crate (`push_samples`, session fan-out)
//! stages events into an inline array and hands the caller a drain. No
//! allocation after init, no `Vec::push` in the callback, overflow saturates
//! (oldest kept — a full stage means the consumer is already behind, and
//! inventing timestamps would be worse than dropping).

/// Inline staging buffer: up to `N` events, allocation-free.
pub struct EventBatch<T: Copy, const N: usize> {
    slot: [Option<T>; N],
    len: usize,
}

impl<T: Copy, const N: usize> Default for EventBatch<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy, const N: usize> EventBatch<T, N> {
    pub const fn new() -> Self {
        EventBatch {
            slot: [None; N],
            len: 0,
        }
    }

    /// Stage one event. Drops (debug-asserts) on overflow.
    pub fn push(&mut self, ev: T) {
        if self.len < N {
            self.slot[self.len] = Some(ev);
            self.len += 1;
        }
        debug_assert!(self.len <= N, "EventBatch overflow: consumer is behind");
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Drain staged events, resetting the stage.
    pub fn drain(&mut self) -> BatchDrain<'_, T, N> {
        let len = self.len;
        self.len = 0;
        BatchDrain {
            slot: &mut self.slot,
            len,
            pos: 0,
        }
    }
}

/// One-shot iterator over a drained batch.
pub struct BatchDrain<'a, T: Copy, const N: usize> {
    slot: &'a mut [Option<T>; N],
    len: usize,
    pos: usize,
}

impl<T: Copy, const N: usize> Iterator for BatchDrain<'_, T, N> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        if self.pos >= self.len {
            return None;
        }
        let ev = self.slot[self.pos].take();
        self.pos += 1;
        ev
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.len - self.pos;
        (n, Some(n))
    }
}

impl<T: Copy, const N: usize> ExactSizeIterator for BatchDrain<'_, T, N> {}
