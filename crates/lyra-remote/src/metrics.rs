//! Minimal in-process metrics for lyra-remote.
//!
//! A desktop app has no Prometheus server to scrape; these counters live in
//! the [`Host`] and are read via [`HostMetrics::snapshot`] (structured,
//! serializable) or [`HostMetrics::render_text`] (Prometheus-style text for
//! logs and debug tooling). All updates are lock-free atomics — safe to
//! bump from any connection task.

use std::sync::atomic::{AtomicU64, Ordering};

/// Lock-free counters owned by [`crate::Host`].
#[derive(Debug, Default)]
pub struct HostMetrics {
    connections_total: AtomicU64,
    pair_attempts: AtomicU64,
    pairings_ok: AtomicU64,
    connects_ok: AtomicU64,
    rejects: AtomicU64,
    commands_total: AtomicU64,
    command_errors: AtomicU64,
}

impl HostMetrics {
    pub(crate) fn inc_connections(&self) {
        self.connections_total.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn inc_pair_attempts(&self) {
        self.pair_attempts.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn inc_pairings_ok(&self) {
        self.pairings_ok.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn inc_connects_ok(&self) {
        self.connects_ok.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn inc_rejects(&self) {
        self.rejects.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn inc_commands(&self) {
        self.commands_total.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn inc_command_errors(&self) {
        self.command_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Point-in-time copy for logging / debug surfaces.
    #[must_use]
    pub fn snapshot(&self) -> HostSnapshot {
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
        HostSnapshot {
            connections_total: load(&self.connections_total),
            pair_attempts: load(&self.pair_attempts),
            pairings_ok: load(&self.pairings_ok),
            connects_ok: load(&self.connects_ok),
            rejects: load(&self.rejects),
            commands_total: load(&self.commands_total),
            command_errors: load(&self.command_errors),
        }
    }

    /// Prometheus exposition-style text — paste into logs or a debug view.
    #[must_use]
    pub fn render_text(&self) -> String {
        self.snapshot().render_text()
    }
}

/// Serializable point-in-time copy of [`HostMetrics`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct HostSnapshot {
    pub connections_total: u64,
    pub pair_attempts: u64,
    pub pairings_ok: u64,
    pub connects_ok: u64,
    pub rejects: u64,
    pub commands_total: u64,
    pub command_errors: u64,
}

impl HostSnapshot {
    /// Prometheus exposition-style text.
    #[must_use]
    pub fn render_text(&self) -> String {
        let rows = [
            ("connections_total", self.connections_total),
            ("pair_attempts", self.pair_attempts),
            ("pairings_ok", self.pairings_ok),
            ("connects_ok", self.connects_ok),
            ("rejects", self.rejects),
            ("commands_total", self.commands_total),
            ("command_errors", self.command_errors),
        ];
        let mut out = String::new();
        for (name, value) in rows {
            out.push_str(&format!(
                "# HELP lyra_remote_{name} remote-control counter.\n\
                 # TYPE lyra_remote_{name} counter\n\
                 lyra_remote_{name} {value}\n"
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_accumulate_and_render() {
        let m = HostMetrics::default();
        m.inc_connections();
        m.inc_connections();
        m.inc_pair_attempts();
        m.inc_pairings_ok();
        m.inc_connects_ok();
        m.inc_commands();
        m.inc_command_errors();
        m.inc_rejects();
        let s = m.snapshot();
        assert_eq!(s.connections_total, 2);
        assert_eq!(s.pair_attempts, 1);
        assert_eq!(s.command_errors, 1);
        let text = m.render_text();
        assert!(text.contains("lyra_remote_connections_total 2"));
        assert!(text.contains("lyra_remote_command_errors 1"));
        // JSON-serializable for structured-log shipping.
        let json = serde_json::to_value(s).unwrap();
        assert_eq!(json["connects_ok"], 1);
    }
}
