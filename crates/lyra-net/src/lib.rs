//! lyra-net: outbound integrations — Last.fm, MusicBrainz, Cover Art Archive,
//! LRCLIB. Deliberately thin: typed request builders + response types,
//! rustls-only TLS, no secrets baked into the binary.
//!
//! Improvement vs incumbent: API keys arrive via config/env at first-run
//! (or the user's own Last.fm app), never shipped in Info.plist.

use serde::{Deserialize, Serialize};

pub const MUSICBRAINZ_UA: &str = "Lyra/0.1.0 (https://github.com/lyra-player)";

#[derive(Clone)]
pub struct LrcLib {
    client: reqwest::Client,
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

impl LrcLib {
    pub fn new() -> Self {
        Self { client: reqwest::Client::new() }
    }

    /// Synced lyrics for a track.
    pub async fn get(
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
}

impl MusicBrainz {
    pub fn new() -> Self {
        Self { client: reqwest::Client::new() }
    }

    /// Release lookup. MusicBrainz policy: descriptive UA, max 1 req/s —
    /// callers go through a shared rate-limiter (TODO: governor crate).
    pub async fn release(&self, mbid: &str) -> Result<serde_json::Value, reqwest::Error> {
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
        Self { client: reqwest::Client::new(), api_key, secret }
    }

    /// Browser auth URL — user approves, we exchange token for session key.
    pub fn auth_url(&self) -> String {
        format!("https://www.last.fm/api/auth/?api_key={}", self.api_key)
    }

    // TODO: api_sig (md5 of sorted params+secret) per the Web Services spec —
    // implemented when scrobbling lands; the shape lives here so the seam is set.
}

/// Cover Art Archive — release artwork by MBID.
pub fn coverart_url(mbid: &str) -> String {
    format!("https://coverartarchive.org/release/{mbid}/front-500")
}
