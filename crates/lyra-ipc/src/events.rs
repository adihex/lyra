//! Broadcast event bus: monotonic `seq`, retained core topics, per-connection
//! bounded queues (a wedged client drops events instead of growing memory).

use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use crate::protocol::{Event, to_line};

/// Topics retained so a mid-song connect gets current state immediately.
fn is_retained(topic: &str) -> bool {
    matches!(
        topic,
        "runtime.state" | "runtime.playback" | "runtime.job" | "library.scan" | "queue"
    ) || topic.starts_with("plugin.")
}

/// `pattern` (from `subscribe.topics`) matches `topic`.
/// Supports exact names, `plugin.*` prefixes and a bare `*`.
pub fn topic_matches(pattern: &str, topic: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix(".*") {
        return topic == prefix || topic.starts_with(&format!("{prefix}."));
    }
    pattern == topic
}

struct Subscriber {
    id: u64,
    topics: Vec<String>,
    tx: mpsc::SyncSender<String>,
}

struct Inner {
    retained: HashMap<String, (u64, Value)>,
    subscribers: Vec<Subscriber>,
    next_sub: u64,
}

pub struct EventBus {
    seq: AtomicU64,
    inner: Mutex<Inner>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            seq: AtomicU64::new(0),
            inner: Mutex::new(Inner {
                retained: HashMap::new(),
                subscribers: Vec::new(),
                next_sub: 1,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Publish an event. Returns the assigned `seq`. Never blocks: full
    /// per-connection queues drop the event (backpressure, §1.3).
    pub fn emit(&self, topic: &str, data: Value) -> u64 {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let line = to_line(&Event::new(seq, topic, data.clone()));
        let mut inner = self.lock();
        if is_retained(topic) {
            inner.retained.insert(topic.to_string(), (seq, data));
        }
        for sub in &inner.subscribers {
            if sub.topics.iter().any(|p| topic_matches(p, topic)) {
                let _ = sub.tx.try_send(line.clone());
            }
        }
        seq
    }

    /// Current sequence counter (last assigned `seq`, 0 when idle).
    pub fn current_seq(&self) -> u64 {
        self.seq.load(Ordering::SeqCst)
    }

    /// Register a connection with a caller-owned sender (responses and
    /// events share one queue on the connection's writer thread).
    pub fn add_sender(&self, tx: mpsc::SyncSender<String>, topics: Vec<String>) -> u64 {
        let mut inner = self.lock();
        let id = inner.next_sub;
        inner.next_sub += 1;
        inner.subscribers.push(Subscriber { id, topics, tx });
        id
    }

    /// Retained events matching any of `patterns`, for post-subscribe replay.
    pub fn retained_matching(&self, patterns: &[String]) -> Vec<(u64, String, Value)> {
        let inner = self.lock();
        let mut out: Vec<(u64, String, Value)> = inner
            .retained
            .iter()
            .filter(|(topic, _)| patterns.iter().any(|p| topic_matches(p, topic)))
            .map(|(topic, (seq, data))| (*seq, topic.clone(), data.clone()))
            .collect();
        out.sort_by_key(|(seq, _, _)| *seq);
        out
    }
    pub fn update_subscriber(&self, id: u64, topics: Vec<String>) {
        let mut inner = self.lock();
        if let Some(sub) = inner.subscribers.iter_mut().find(|s| s.id == id) {
            sub.topics = topics;
        }
    }

    pub fn remove_subscriber(&self, id: u64) {
        let mut inner = self.lock();
        inner.subscribers.retain(|s| s.id != id);
    }
}

pub type SharedBus = Arc<EventBus>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_replay_and_matching() {
        assert!(topic_matches("plugin.*", "plugin.lastfm"));
        assert!(topic_matches("*", "queue"));
        assert!(!topic_matches("queue", "runtime.job"));
        let bus = EventBus::new();
        bus.emit("runtime.state", serde_json::json!({"revision": 3}));
        let matched = bus.retained_matching(&["runtime.state".to_string()]);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].0, 1);
        assert_eq!(matched[0].1, "runtime.state");
        // Live delivery through a caller-owned queue.
        let (tx, rx) = mpsc::sync_channel(8);
        let id = bus.add_sender(tx, vec!["queue".to_string()]);
        bus.emit("queue", serde_json::json!({"length": 1}));
        bus.emit("runtime.job", serde_json::json!({}));
        let line = rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
        let ev: Event = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(ev.topic, "queue");
        bus.remove_subscriber(id);
    }
}
