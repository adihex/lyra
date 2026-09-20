//! lyra-net: outbound integrations — Last.fm, MusicBrainz, Cover Art Archive,
//! LRCLIB. Deliberately thin: typed request builders + response types,
//! rustls-only TLS, no secrets baked into the binary.
//!
//! Improvement vs incumbent: API keys arrive via config/env at first-run
//! (or the user's own Last.fm app), never shipped in Info.plist.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub mod resilience;
pub use resilience::{BreakerState, CircuitBreaker, RetryPolicy};

pub const MUSICBRAINZ_UA: &str = "Lyra/0.1.0 (https://github.com/lyra-player)";

/// Defaults: 5 consecutive failures trip the breaker, one probe per 30 s.
fn default_breaker() -> Mutex<CircuitBreaker> {
    Mutex::new(CircuitBreaker::new(5, Duration::from_secs(30)))
}

#[derive(Clone)]
pub struct LrcLib {
    client: reqwest::Client,
    retry: RetryPolicy,
    breaker: Arc<Mutex<CircuitBreaker>>,
}

#[derive(Debug, Deserialize)]
pub struct LrcResult {
    #[serde(rename = "trackName")]
    pub track: String,
    #[serde(rename = "artistName")]
    pub artist: String,
    #[serde(rename = "syncedLyrics")]
    pub synced: Option<String>,
    #[serde(rename = "plainLyrics")]
    pub plain: Option<String>,
}

impl Default for LrcLib {
    fn default() -> Self {
        Self::new()
    }
}

impl LrcLib {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            retry: RetryPolicy::default(),
            breaker: Arc::new(default_breaker()),
        }
    }

    /// Test/tuning seam: custom retry + breaker (e.g. short timeouts).
    pub fn with_policy(retry: RetryPolicy, breaker: CircuitBreaker) -> Self {
        Self {
            client: reqwest::Client::new(),
            retry,
            breaker: Arc::new(Mutex::new(breaker)),
        }
    }

    /// Effective breaker state for debug surfaces.
    #[must_use]
    pub fn breaker_state(&self) -> BreakerState {
        self.breaker.lock().unwrap().state()
    }

    /// Synced lyrics for a track, with retry-with-backoff on transient
    /// failures and breaker degradation (single probe, no retry storm)
    /// while the breaker is open. 404 stays `Ok(None)` — a cacheable
    /// answer, not an error.
    pub async fn get(
        &self,
        track: &str,
        artist: &str,
        album: &str,
        duration_secs: u64,
    ) -> Result<Option<LrcResult>, reqwest::Error> {
        if self.probe_only() {
            tracing::warn!(op = "lrclib.get", "breaker open: single probe attempt");
            let r = self.get_once(track, artist, album, duration_secs).await;
            self.record("lrclib.get", &r);
            return r;
        }
        let r = self
            .retry
            .execute(
                "lrclib.get",
                || self.get_once(track, artist, album, duration_secs),
                resilience::is_transient_request_error,
            )
            .await;
        self.record("lrclib.get", &r);
        r
    }

    /// True when the breaker is open and the probe timeout has not
    /// elapsed — the caller should issue one attempt, not a retry loop.
    /// The lock is released before any `.await` (workspace lint).
    fn probe_only(&self) -> bool {
        !self.breaker.lock().unwrap().allow()
    }

    fn record<T>(&self, op: &str, r: &Result<T, reqwest::Error>) {
        let mut b = self.breaker.lock().unwrap();
        match r {
            Ok(_) => b.record_success(),
            Err(_) => b.record_failure(op),
        }
    }

    async fn get_once(
        &self,
        track: &str,
        artist: &str,
        album: &str,
        duration_secs: u64,
    ) -> Result<Option<LrcResult>, reqwest::Error> {
        let resp = self
            .client
            .get("https://lrclib.net/api/get")
            .query(&[
                ("track_name", track),
                ("artist_name", artist),
                ("album_name", album),
                ("duration", &duration_secs.to_string()),
            ])
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(resp.json().await?))
    }
}

#[derive(Clone)]
pub struct MusicBrainz {
    client: reqwest::Client,
    retry: RetryPolicy,
    breaker: Arc<Mutex<CircuitBreaker>>,
}

impl Default for MusicBrainz {
    fn default() -> Self {
        Self::new()
    }
}

impl MusicBrainz {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            retry: RetryPolicy::default(),
            breaker: Arc::new(default_breaker()),
        }
    }

    /// Test/tuning seam: custom retry + breaker (e.g. short timeouts).
    pub fn with_policy(retry: RetryPolicy, breaker: CircuitBreaker) -> Self {
        Self {
            client: reqwest::Client::new(),
            retry,
            breaker: Arc::new(Mutex::new(breaker)),
        }
    }

    /// Effective breaker state for debug surfaces.
    #[must_use]
    pub fn breaker_state(&self) -> BreakerState {
        self.breaker.lock().unwrap().state()
    }

    /// Release lookup, with retry-with-backoff on transient failures and
    /// breaker degradation while open. MusicBrainz policy: descriptive UA,
    /// max 1 req/s — callers go through a shared rate-limiter (TODO(#5):
    /// governor crate); the retry delays here are on top of that, not a
    /// substitute (429s still back off).
    pub async fn release(&self, mbid: &str) -> Result<serde_json::Value, reqwest::Error> {
        if !self.breaker.lock().unwrap().allow() {
            tracing::warn!(
                op = "musicbrainz.release",
                "breaker open: single probe attempt"
            );
            let r = self.release_once(mbid).await;
            let mut b = self.breaker.lock().unwrap();
            match &r {
                Ok(_) => b.record_success(),
                Err(_) => b.record_failure("musicbrainz.release"),
            }
            return r;
        }
        let r = self
            .retry
            .execute(
                "musicbrainz.release",
                || self.release_once(mbid),
                resilience::is_transient_request_error,
            )
            .await;
        {
            let mut b = self.breaker.lock().unwrap();
            match &r {
                Ok(_) => b.record_success(),
                Err(_) => b.record_failure("musicbrainz.release"),
            }
        }
        r
    }

    async fn release_once(&self, mbid: &str) -> Result<serde_json::Value, reqwest::Error> {
        self.client
            .get(format!("https://musicbrainz.org/ws/2/release/{mbid}"))
            .header(reqwest::header::USER_AGENT, MUSICBRAINZ_UA)
            .query(&[("fmt", "json"), ("inc", "artists+recordings")])
            .send()
            .await?
            .json()
            .await
    }
}

/// Last.fm: scrobble + now-playing. Auth is token→session (user authorizes
/// in browser, we exchange for a session key kept in Keychain — never prefs).
/// client/secret are used when the api_sig signer lands (md5 sorted-params).
#[allow(dead_code)]
pub struct LastFm {
    client: reqwest::Client,
    api_key: String,
    secret: String, // injected at runtime; NOT compiled in
}

#[derive(Debug, Serialize)]
pub struct Scrobble {
    pub artist: String,
    pub track: String,
    pub album: Option<String>,
    pub timestamp_unix: u64,
    pub duration_secs: Option<u64>,
}

impl LastFm {
    pub fn new(api_key: String, secret: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            secret,
        }
    }

    /// Browser auth URL — user approves, we exchange token for session key.
    pub fn auth_url(&self) -> String {
        format!("https://www.last.fm/api/auth/?api_key={}", self.api_key)
    }

    // TODO(#6): api_sig (md5 of sorted params+secret) per the Web Services spec —
    // implemented when scrobbling lands; the shape lives here so the seam is set.
}

/// Cover Art Archive — release artwork by MBID.
pub fn coverart_url(mbid: &str) -> String {
    format!("https://coverartarchive.org/release/{mbid}/front-500")
}
