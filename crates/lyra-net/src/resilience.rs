//! Retry-with-backoff + circuit breaker for outbound integrations.
//!
//! MusicBrainz asks for ≤1 req/s citizenship and LRCLIB is a free shared
//! service; both can answer 429/503 or drop connections. Every external
//! call in this crate therefore goes through two guards:
//!
//! 1. [`RetryPolicy::execute`] — retries *transient* failures (timeouts,
//!    connects, HTTP 429/5xx) with exponential backoff; permanent errors
//!    (4xx, decode failures) return immediately.
//! 2. [`CircuitBreaker`] — counts consecutive failures and, once the
//!    threshold trips, degrades to single probe attempts (no retry storms)
//!    until a probe succeeds.
//!
//! The breaker deliberately never *refuses* a call with a synthetic error:
//! this crate's error type is `reqwest::Error`, and inventing one would be
//! theater. Instead an open breaker skips the retry loop and issues one
//! probe — success closes it, failure keeps it open. All transitions and
//! retries are `tracing` events carrying the operation name and attempt, so
//! outages show up in logs with context instead of silent `Err`s.

use std::fmt::Display;
use std::time::{Duration, Instant};

// ── Retry ────────────────────────────────────────────────────────────────

/// How many times to try a transient-failing operation and how long to
/// wait between attempts. `max_attempts` counts the first try.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(5),
        }
    }
}

impl RetryPolicy {
    /// Backoff after `failed_attempt` consecutive failures (1-based):
    /// `base * 2^(n-1)`, capped at `max_delay`.
    #[must_use]
    pub fn delay_for(&self, failed_attempt: u32) -> Duration {
        let shift = failed_attempt.saturating_sub(1).min(10);
        let doubled = self
            .base_delay
            .checked_mul(1 << shift)
            .unwrap_or(self.max_delay);
        doubled.min(self.max_delay)
    }

    /// Run `op` until it succeeds, fails non-retryably, or runs out of
    /// attempts. `is_retryable` classifies the error; retries sleep with
    /// backoff in between. Never holds the caller's locks across `.await`
    /// — the sleeps happen here, outside any guard.
    pub async fn execute<T, E, F, Fut>(
        &self,
        op_name: &str,
        mut op: F,
        mut is_retryable: impl FnMut(&E) -> bool,
    ) -> Result<T, E>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
        E: Display,
    {
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            match op().await {
                Ok(v) => {
                    if attempt > 1 {
                        tracing::info!(op = op_name, attempt, "succeeded after retry");
                    }
                    return Ok(v);
                }
                Err(e) => {
                    let exhausted = attempt >= self.max_attempts.max(1);
                    if !exhausted && is_retryable(&e) {
                        let delay = self.delay_for(attempt);
                        tracing::warn!(
                            op = op_name,
                            attempt,
                            delay_ms = delay.as_millis() as u64,
                            error = %e,
                            "transient failure, retrying with backoff"
                        );
                        tokio::time::sleep(delay).await;
                    } else {
                        if exhausted {
                            tracing::warn!(
                                op = op_name,
                                attempt,
                                error = %e,
                                "giving up after max attempts"
                            );
                        }
                        return Err(e);
                    }
                }
            }
        }
    }
}

/// True for failures worth retrying: timeouts, connection failures, and
/// HTTP 429 / 5xx. Everything else (4xx, URL/builder bugs, body decode)
/// is permanent — retrying those just burns the shared services' budget.
#[must_use]
pub fn is_transient_request_error(err: &reqwest::Error) -> bool {
    if err.is_timeout() || err.is_connect() {
        return true;
    }
    err.status()
        .is_some_and(|s| s.as_u16() == 429 || s.is_server_error())
}

// ── Circuit breaker ──────────────────────────────────────────────────────

/// Breaker state. `HalfOpen` is the single-probe phase after the open
/// timeout elapses — one call goes through to test the water.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

/// Consecutive-failure breaker. Not thread-safe on its own — holders keep
/// it behind a `Mutex` and only lock around [`CircuitBreaker::allow`] /
/// record calls, never across `.await` (workspace `await_holding_lock`).
#[derive(Debug)]
pub struct CircuitBreaker {
    state: BreakerState,
    consecutive_failures: u32,
    consecutive_successes: u32,
    failure_threshold: u32,
    success_threshold: u32,
    opened_at: Option<Instant>,
    open_timeout: Duration,
}

impl CircuitBreaker {
    /// Trip open after `failure_threshold` consecutive failures; allow a
    /// probe after `open_timeout`. One probe success closes the breaker.
    #[must_use]
    pub fn new(failure_threshold: u32, open_timeout: Duration) -> Self {
        Self {
            state: BreakerState::Closed,
            consecutive_failures: 0,
            consecutive_successes: 0,
            failure_threshold: failure_threshold.max(1),
            success_threshold: 1,
            opened_at: None,
            open_timeout,
        }
    }

    #[must_use]
    pub fn state(&self) -> BreakerState {
        // `Open` may have timed out since the last `allow()` — report the
        // effective state so debug surfaces don't show a stale Open.
        if self.state == BreakerState::Open
            && self
                .opened_at
                .is_some_and(|t| t.elapsed() >= self.open_timeout)
        {
            return BreakerState::HalfOpen;
        }
        self.state
    }

    /// Whether a call may proceed now. Advances `Open` → `HalfOpen` once
    /// the timeout has elapsed. Returns `false` only while open and the
    /// timeout has *not* elapsed — callers then issue a single probe
    /// attempt (see module docs) rather than refusing outright.
    pub fn allow(&mut self) -> bool {
        match self.state {
            BreakerState::Closed | BreakerState::HalfOpen => true,
            BreakerState::Open => {
                if self
                    .opened_at
                    .is_some_and(|t| t.elapsed() >= self.open_timeout)
                {
                    self.state = BreakerState::HalfOpen;
                    self.consecutive_successes = 0;
                    tracing::info!("breaker half-open: probe attempt allowed");
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Record a call outcome. Success closes (or half-closes) the breaker;
    /// failure counts toward the trip threshold, or re-opens from half-open.
    pub fn record_success(&mut self) {
        match self.state {
            BreakerState::HalfOpen => {
                self.consecutive_successes += 1;
                self.consecutive_failures = 0;
                if self.consecutive_successes >= self.success_threshold {
                    self.state = BreakerState::Closed;
                    self.opened_at = None;
                    tracing::info!("breaker closed: probe succeeded");
                }
            }
            BreakerState::Closed => {
                self.consecutive_failures = 0;
            }
            BreakerState::Open => {
                // Late success while nominally open (a probe issued via
                // `state()` visibility) — treat like a half-open probe.
                self.state = BreakerState::Closed;
                self.consecutive_failures = 0;
                self.opened_at = None;
                tracing::info!("breaker closed: probe succeeded");
            }
        }
    }

    /// Record a call failure with the operation name for log context.
    pub fn record_failure(&mut self, op_name: &str) {
        match self.state {
            BreakerState::HalfOpen => {
                self.state = BreakerState::Open;
                self.opened_at = Some(Instant::now());
                tracing::warn!(op = op_name, "breaker re-opened: probe failed");
            }
            BreakerState::Closed => {
                self.consecutive_failures += 1;
                if self.consecutive_failures >= self.failure_threshold {
                    self.state = BreakerState::Open;
                    self.opened_at = Some(Instant::now());
                    tracing::warn!(
                        op = op_name,
                        failures = self.consecutive_failures,
                        "breaker opened"
                    );
                }
            }
            BreakerState::Open => {
                self.opened_at = Some(Instant::now());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn backoff_doubles_and_caps() {
        let p = RetryPolicy {
            max_attempts: 5,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(250),
        };
        assert_eq!(p.delay_for(1), Duration::from_millis(100));
        assert_eq!(p.delay_for(2), Duration::from_millis(200));
        // 400 capped to 250.
        assert_eq!(p.delay_for(3), Duration::from_millis(250));
        assert_eq!(p.delay_for(100), Duration::from_millis(250));
    }

    #[tokio::test]
    async fn retry_succeeds_after_transient_failures() {
        let p = RetryPolicy {
            max_attempts: 4,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
        };
        let calls = AtomicU32::new(0);
        let r = p
            .execute(
                "test-op",
                || {
                    let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    async move {
                        if n < 3 {
                            Err::<u32, String>("boom".into())
                        } else {
                            Ok(n)
                        }
                    }
                },
                |_: &String| true,
            )
            .await;
        assert_eq!(r.unwrap(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_gives_up_after_max_attempts() {
        let p = RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
        };
        let calls = AtomicU32::new(0);
        let r = p
            .execute(
                "test-op",
                || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Err::<u32, String>("down".into()) }
                },
                |_: &String| true,
            )
            .await;
        assert!(r.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn non_retryable_fails_immediately() {
        let p = RetryPolicy {
            max_attempts: 5,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
        };
        let calls = AtomicU32::new(0);
        let r = p
            .execute(
                "test-op",
                || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Err::<u32, String>("bad request".into()) }
                },
                |_: &String| false,
            )
            .await;
        assert!(r.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn breaker_opens_half_opens_and_closes() {
        let mut b = CircuitBreaker::new(2, Duration::from_millis(20));
        assert_eq!(b.state(), BreakerState::Closed);
        assert!(b.allow());
        b.record_failure("op");
        assert!(b.allow(), "one failure stays closed");
        b.record_failure("op");
        assert_eq!(b.state(), BreakerState::Open);
        assert!(!b.allow(), "open breaker blocks");
        // Success resets the consecutive count while closed.
        let mut b2 = CircuitBreaker::new(3, Duration::from_secs(30));
        b2.record_failure("op");
        b2.record_success();
        b2.record_failure("op");
        b2.record_failure("op");
        assert!(b2.allow(), "still closed after reset");
        std::thread::sleep(Duration::from_millis(30));
        assert!(b.allow(), "probe allowed after timeout");
        assert_eq!(b.state(), BreakerState::HalfOpen);
        b.record_success();
        assert_eq!(b.state(), BreakerState::Closed);
        assert!(b.allow());
    }

    #[test]
    fn failed_probe_reopens() {
        let mut b = CircuitBreaker::new(1, Duration::from_millis(10));
        b.record_failure("op");
        assert_eq!(b.state(), BreakerState::Open);
        std::thread::sleep(Duration::from_millis(15));
        assert!(b.allow());
        b.record_failure("op");
        assert_eq!(b.state(), BreakerState::Open);
        assert!(!b.allow());
    }
}
