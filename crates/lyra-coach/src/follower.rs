//! Score follower — `matchmaker` semantics, own implementation.
//!
//! The research survey (`docs/research/pluggable-engines.md` §10) says
//! *semantics, not a dependency*: matchmaker's online alignment is a
//! windowed nearest-match with a monotonicity constraint, ~300 lines of
//! Python. This is that idea in allocation-free Rust.
//!
//! The follower answers "where is the player in the chart right now" so
//! the chart can track player position and the transport can implement
//! "wait-for-me" practice mode (the song holds until the player arrives).
//! Judging stays in [`crate::judge`]; the follower only observes.
//!
//! Lookup semantics shared with the conductor seam: the chart is a sorted
//! time list (fixed-tempo grid or extracted beat table —
//! [`crate::conductor::BeatGrid`] answers the same nearest/range queries
//! for the quantizer side).

/// Score follower over a sorted chart-time list.
pub struct Follower {
    /// Chart event times in seconds (grid time, pre-offset).
    times: Vec<f64>,
    /// Calibration offset subtracted from observed hits (shares the
    /// judge's offset — same device, same detector delay).
    offset_s: f64,
    /// How far ahead of the current position a hit may match (events).
    window_ahead: usize,
    /// How far behind (0 = strictly monotonic, the matchmaker default).
    /// Small backtrack (1–2) tolerates flam/double-trigger duplicates.
    window_behind: usize,
    /// Max |hit − chart| to accept a match (seconds).
    match_window_s: f64,
    /// "Wait-for-me": the song holds past `grace_s` beyond the next
    /// unmatched event until the player arrives.
    wait_for_me: bool,
    grace_s: f64,
    /// Number of chart events matched so far (= position).
    pos: usize,
    /// Last accepted match (chart index + hit time), if any.
    last_match: Option<(usize, f64)>,
    /// Matches accepted outside ±good-window (timing drift signal for
    /// the difficulty controller).
    loose_matches: u64,
}

impl Follower {
    /// Build over chart times (must be sorted ascending). Allocates
    /// nothing beyond the caller's `times` vec (moved in).
    pub fn new(times: Vec<f64>) -> Self {
        Follower {
            times,
            offset_s: 0.0,
            window_ahead: 4,
            window_behind: 0,
            match_window_s: 0.150,
            wait_for_me: false,
            grace_s: 0.250,
            pos: 0,
            last_match: None,
            loose_matches: 0,
        }
    }

    /// Build from expected events (shares the judge's chart).
    pub fn from_expected(expected: &[crate::judge::ExpectedEvent]) -> Self {
        Follower::new(expected.iter().map(|e| e.t_secs).collect())
    }

    pub fn set_offset(&mut self, offset_s: f64) {
        self.offset_s = offset_s;
    }

    pub fn set_windows(&mut self, ahead: usize, behind: usize, match_window_s: f64) {
        self.window_ahead = ahead;
        self.window_behind = behind;
        self.match_window_s = match_window_s;
    }

    pub fn set_wait_for_me(&mut self, wait: bool, grace_s: f64) {
        self.wait_for_me = wait;
        self.grace_s = grace_s;
    }

    /// Chart position: number of events matched so far.
    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn len(&self) -> usize {
        self.times.len()
    }

    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    pub fn is_finished(&self) -> bool {
        self.pos >= self.times.len()
    }

    /// Time of the next unmatched chart event, if any.
    pub fn next_event_t(&self) -> Option<f64> {
        self.times.get(self.pos).copied()
    }

    pub fn loose_matches(&self) -> u64 {
        self.loose_matches
    }

    /// Observe one detected hit (stream-clock seconds, pre-offset).
    /// Returns the matched chart index, if the hit fell inside the
    /// windowed nearest-match.
    pub fn observe(&mut self, hit_t: f64) -> Option<usize> {
        if self.is_finished() {
            return None;
        }
        let t = hit_t - self.offset_s;
        let lo = self.pos.saturating_sub(self.window_behind);
        let hi = (self.pos + self.window_ahead).min(self.times.len().saturating_sub(1));
        let mut best: Option<(usize, f64)> = None;
        for i in lo..=hi {
            let err = (t - self.times[i]).abs();
            if err <= self.match_window_s && best.map(|(_, b)| err < b).unwrap_or(true) {
                best = Some((i, err));
            }
        }
        let (i, err) = best?;
        self.pos = i + 1;
        self.last_match = Some((i, hit_t));
        if err > crate::judge::GOOD_WINDOW {
            self.loose_matches += 1;
        }
        Some(i)
    }

    /// "Wait-for-me" gate: the chart time the transport is allowed to
    /// show/play. In normal mode this is the identity. In wait mode the
    /// song holds at `next_event + grace` until the player catches up —
    /// the accompaniment literally waits for the player.
    pub fn gate_chart_time(&self, song_t: f64) -> f64 {
        if !self.wait_for_me {
            return song_t;
        }
        match self.next_event_t() {
            None => song_t,
            Some(next) => song_t.min(next + self.grace_s),
        }
    }

    /// Advance past an expired chart event (a swept miss the player never
    /// played): position moves forward, never backward.
    pub fn resync_to(&mut self, index: usize) {
        self.pos = self.pos.max(index.min(self.times.len()));
    }

    /// Snap position to the chart after a seek/pause: the count of chart
    /// events at or before `song_t` (grid time). Never allocates.
    pub fn resync(&mut self, song_t: f64) {
        self.pos = self.times.partition_point(|&e| e <= song_t);
        self.last_match = None;
    }
}
