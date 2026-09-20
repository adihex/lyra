//! Minimal in-process metrics for lyra-engine.
//!
//! A desktop app has no metrics server to scrape; these counters live in
//! the [`crate::Engine`] and are read via [`crate::Engine::metrics`]
//! (structured snapshot) or [`crate::Engine::metrics_text`]
//! (Prometheus-style text for logs and debug tooling). All updates are
//! lock-free atomics — safe to bump from the decode worker and the RT
//! callback alike.

use std::sync::atomic::{AtomicU64, Ordering};

/// Lock-free playback-event counters owned by [`crate::Engine`].
#[derive(Debug, Default)]
pub struct EngineMetrics {
    play_requests: AtomicU64,
    pause_requests: AtomicU64,
    resume_requests: AtomicU64,
    stop_requests: AtomicU64,
    seek_requests: AtomicU64,
    set_band_requests: AtomicU64,
    decode_blocks: AtomicU64,
    decode_errors: AtomicU64,
    eof_count: AtomicU64,
    underruns: AtomicU64,
}

impl EngineMetrics {
    pub(crate) fn inc_play(&self) {
        self.play_requests.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn inc_pause(&self) {
        self.pause_requests.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn inc_resume(&self) {
        self.resume_requests.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn inc_stop(&self) {
        self.stop_requests.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn inc_seek(&self) {
        self.seek_requests.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn inc_set_band(&self) {
        self.set_band_requests.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn inc_decode_blocks(&self) {
        self.decode_blocks.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn inc_decode_errors(&self) {
        self.decode_errors.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn inc_eof(&self) {
        self.eof_count.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn inc_underruns(&self) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
    }

    /// Point-in-time copy for logging / debug surfaces.
    #[must_use]
    pub fn snapshot(&self) -> EngineSnapshot {
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
        EngineSnapshot {
            play_requests: load(&self.play_requests),
            pause_requests: load(&self.pause_requests),
            resume_requests: load(&self.resume_requests),
            stop_requests: load(&self.stop_requests),
            seek_requests: load(&self.seek_requests),
            set_band_requests: load(&self.set_band_requests),
            decode_blocks: load(&self.decode_blocks),
            decode_errors: load(&self.decode_errors),
            eof_count: load(&self.eof_count),
            underruns: load(&self.underruns),
        }
    }
}

/// Point-in-time copy of [`EngineMetrics`]. Plain data — no serde
/// dependency needed; [`EngineSnapshot::render_text`] covers the debug
/// surface and [`EngineSnapshot::to_json`] covers structured-log shipping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineSnapshot {
    pub play_requests: u64,
    pub pause_requests: u64,
    pub resume_requests: u64,
    pub stop_requests: u64,
    pub seek_requests: u64,
    pub set_band_requests: u64,
    pub decode_blocks: u64,
    pub decode_errors: u64,
    pub eof_count: u64,
    pub underruns: u64,
}

impl EngineSnapshot {
    /// Prometheus exposition-style text.
    #[must_use]
    pub fn render_text(&self) -> String {
        let rows = [
            ("play_requests_total", self.play_requests),
            ("pause_requests_total", self.pause_requests),
            ("resume_requests_total", self.resume_requests),
            ("stop_requests_total", self.stop_requests),
            ("seek_requests_total", self.seek_requests),
            ("set_band_requests_total", self.set_band_requests),
            ("decode_blocks_total", self.decode_blocks),
            ("decode_errors_total", self.decode_errors),
            ("eof_total", self.eof_count),
            ("underruns_total", self.underruns),
        ];
        let mut out = String::new();
        for (name, value) in rows {
            out.push_str(&format!(
                "# HELP lyra_engine_{name} engine playback counter.\n\
                 # TYPE lyra_engine_{name} counter\n\
                 lyra_engine_{name} {value}\n"
            ));
        }
        out
    }

    /// Minimal JSON object for structured-log shipping (no serde needed).
    #[must_use]
    pub fn to_json(&self) -> String {
        format!(
            "{{\
             \"play_requests\":{play},\
             \"pause_requests\":{pause},\
             \"resume_requests\":{resume},\
             \"stop_requests\":{stop},\
             \"seek_requests\":{seek},\
             \"set_band_requests\":{band},\
             \"decode_blocks\":{blocks},\
             \"decode_errors\":{derr},\
             \"eof\":{eof},\
             \"underruns\":{under}\
             }}",
            play = self.play_requests,
            pause = self.pause_requests,
            resume = self.resume_requests,
            stop = self.stop_requests,
            seek = self.seek_requests,
            band = self.set_band_requests,
            blocks = self.decode_blocks,
            derr = self.decode_errors,
            eof = self.eof_count,
            under = self.underruns,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_accumulate_and_render() {
        let m = EngineMetrics::default();
        m.inc_play();
        m.inc_pause();
        m.inc_decode_blocks();
        m.inc_decode_errors();
        m.inc_underruns();
        let s = m.snapshot();
        assert_eq!(s.play_requests, 1);
        assert_eq!(s.decode_blocks, 1);
        assert_eq!(s.underruns, 1);
        let text = s.render_text();
        assert!(text.contains("lyra_engine_play_requests_total 1"));
        assert!(text.contains("lyra_engine_underruns_total 1"));
        let json = s.to_json();
        assert!(json.contains("\"play_requests\":1"));
    }
}
