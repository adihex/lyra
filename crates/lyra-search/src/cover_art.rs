//! Cover Art Archive lookups, resolved through MusicBrainz.
//!
//! Chain: MB release-group search → CAA `/release-group/{rgid}/front`
//! (307 → archive.org bytes), falling back to per-release fronts when the
//! group has none. MB search results don't carry cover-art flags, so we
//! probe CAA directly — its 404s are cheap and unthrottled.
//!
//! MB is rate-limited to 1 req/s and requires a real User-Agent; CAA has
//! no key. Failures are meant to be recorded in `artwork_fetch` by the
//! caller — `NotFound` is a cacheable answer, not a retry.

use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// MB demands a UA with contact info; anything generic gets 403s.
const UA: &str = "Lyra/0.1.0 ( https://github.com/adihex/lyra )";
const MB_GAP: Duration = Duration::from_millis(1100);
/// Refuse absurd payloads before they touch the image decoder.
const MAX_ART_BYTES: usize = 40 * 1024 * 1024;

static LAST_MB: Mutex<Option<Instant>> = Mutex::new(None);

#[derive(Debug)]
pub enum ArtFetch {
    NotFound,
    Http(u16),
    Net(String),
    BadBody,
}

impl std::fmt::Display for ArtFetch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "not found"),
            Self::Http(s) => write!(f, "http {s}"),
            Self::Net(e) => write!(f, "{e}"),
            Self::BadBody => write!(f, "non-image body"),
        }
    }
}

pub struct CoverArt {
    /// The MBID that produced art — release-group or release. Persisted
    /// in artwork_fetch for debugging/re-fetches.
    pub mbid: String,
    pub bytes: Vec<u8>,
    pub mime: String,
}

#[derive(Default)]
pub struct CoverArtClient {
    client: reqwest::Client,
}

#[derive(Deserialize)]
struct MbGroups {
    #[serde(rename = "release-groups", default)]
    groups: Vec<MbGroup>,
}

#[derive(Deserialize)]
struct MbGroup {
    id: String,
    #[serde(rename = "primary-type")]
    primary_type: Option<String>,
}

#[derive(Deserialize)]
struct MbReleases {
    #[serde(default)]
    releases: Vec<MbRelease>,
}

#[derive(Deserialize)]
struct MbRelease {
    id: String,
    status: Option<String>,
    #[serde(rename = "release-group")]
    group: Option<MbId>,
}

#[derive(Deserialize)]
struct MbRecordings {
    #[serde(default)]
    recordings: Vec<MbRecording>,
}

#[derive(Deserialize)]
struct MbRecording {
    #[serde(default)]
    releases: Vec<MbRelease>,
}

#[derive(Deserialize)]
struct MbId {
    id: String,
}

/// Lucene escaping for MB query values: `\` and `"` inside quotes.
fn lucene_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Tag decorations MB doesn't index: "(Single)", "[Deluxe Edition]",
/// "- Remaster", "- vol 1". Returns the cleaned title, or None when
/// stripping leaves nothing worth querying.
fn strip_decorations(album: &str) -> Option<String> {
    let mut s = album.to_string();
    loop {
        // Remove the last (...) or [...] group anywhere it trails.
        let cut = s
            .rfind('(')
            .filter(|&i| s[i..].ends_with(')'))
            .or_else(|| s.rfind('[').filter(|&i| s[i..].ends_with(']')));
        if let Some(i) = cut {
            s = s[..i].trim_end().to_string();
            continue;
        }
        // Or a " - suffix" tail.
        if let Some(i) = s.rfind(" - ") {
            s = s[..i].trim_end().to_string();
            continue;
        }
        break;
    }
    (!s.is_empty() && s != album).then_some(s)
}

impl CoverArtClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Serialise MusicBrainz calls to ≥1.1s apart — they 503 bursts.
    async fn mb_gate() {
        let wait = LAST_MB
            .lock()
            .ok()
            .and_then(|g| *g)
            .map(|t| MB_GAP.saturating_sub(t.elapsed()))
            .unwrap_or_default();
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        if let Ok(mut g) = LAST_MB.lock() {
            *g = Some(Instant::now());
        }
    }

    async fn mb_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &str,
    ) -> Result<T, ArtFetch> {
        let url = format!("https://musicbrainz.org/ws/2/{path}/?fmt=json&limit=8&query={query}");
        let mut attempt = 0;
        let resp = loop {
            Self::mb_gate().await;
            let resp = self
                .client
                .get(&url)
                .header(reqwest::header::USER_AGENT, UA)
                .send()
                .await
                .map_err(|e| ArtFetch::Net(e.to_string()))?;
            // 503 = rolling-window rate limit; one retry after a longer
            // pause covers the common burst case.
            if resp.status().as_u16() == 503 && attempt == 0 {
                attempt += 1;
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
            break resp;
        };
        if !resp.status().is_success() {
            return Err(ArtFetch::Http(resp.status().as_u16()));
        }
        resp.json().await.map_err(|e| ArtFetch::Net(e.to_string()))
    }

    /// CAA front probe — 307 follows to archive.org; 404 → Ok(None).
    async fn caa_front(&self, kind: &str, mbid: &str) -> Result<Option<CoverArt>, ArtFetch> {
        let url = format!("https://coverartarchive.org/{kind}/{mbid}/front");
        let img = self
            .client
            .get(&url)
            .header(reqwest::header::USER_AGENT, UA)
            .send()
            .await
            .map_err(|e| ArtFetch::Net(e.to_string()))?;
        if img.status().as_u16() == 404 {
            return Ok(None);
        }
        if !img.status().is_success() {
            return Err(ArtFetch::Http(img.status().as_u16()));
        }
        let mime = img
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !mime.starts_with("image/") {
            return Err(ArtFetch::BadBody);
        }
        let bytes = img
            .bytes()
            .await
            .map_err(|e| ArtFetch::Net(e.to_string()))?;
        if bytes.len() > MAX_ART_BYTES {
            return Err(ArtFetch::BadBody);
        }
        Ok(Some(CoverArt {
            mbid: mbid.to_string(),
            bytes: bytes.to_vec(),
            mime,
        }))
    }

    /// Front cover for an album. `artist` should be the album artist when
    /// known — MB's artist clause matches the release's credited name.
    /// `title` (the track title) unlocks a recording-level fallback for
    /// bootleg/ripped album tags that don't exist on MB at all.
    ///
    /// Chain per album candidate (raw, then decoration-stripped):
    /// release-group front (canonical) → release fronts. Then, if a track
    /// title is known: recording search → its releases' groups → fronts.
    pub async fn fetch_front(
        &self,
        artist: &str,
        album: &str,
        title: Option<&str>,
    ) -> Result<CoverArt, ArtFetch> {
        let mut albums = vec![album.to_string()];
        if let Some(stripped) = strip_decorations(album) {
            albums.push(stripped);
        }
        for cand in &albums {
            if let Some(art) = self.album_front(artist, cand).await? {
                return Ok(art);
            }
        }
        if let Some(art) = self.recording_front(artist, title).await? {
            return Ok(art);
        }
        Err(ArtFetch::NotFound)
    }

    /// Strict album lookup for one title variant — group fronts first,
    /// then individual release fronts (deluxe/promo editions often hold
    /// art the group lacks).
    async fn album_front(
        &self,
        artist: &str,
        album: &str,
    ) -> Result<Option<CoverArt>, ArtFetch> {
        let q = format!(
            "releasegroup:\"{}\" AND artist:\"{}\"",
            lucene_escape(album),
            lucene_escape(artist)
        );
        let groups: MbGroups = self
            .mb_get("release-group", &urlencoding::encode(&q))
            .await?;
        let mut order: Vec<&MbGroup> = groups.groups.iter().collect();
        order.sort_by_key(|g| if g.primary_type.as_deref() == Some("Album") { 0 } else { 1 });
        for g in order.into_iter().take(3) {
            match self.caa_front("release-group", &g.id).await? {
                Some(art) => return Ok(Some(art)),
                None => continue,
            }
        }

        let q = format!(
            "release:\"{}\" AND artist:\"{}\"",
            lucene_escape(album),
            lucene_escape(artist)
        );
        let rels: MbReleases = self
            .mb_get("release", &urlencoding::encode(&q))
            .await?;
        let mut order: Vec<&MbRelease> = rels.releases.iter().collect();
        order.sort_by_key(|r| if r.status.as_deref() == Some("Official") { 0 } else { 1 });
        for r in order.into_iter().take(3) {
            match self.caa_front("release", &r.id).await? {
                Some(art) => return Ok(Some(art)),
                None => continue,
            }
        }
        Ok(None)
    }

    /// Recording fallback — for album tags that aren't on MB ("remaster -
    /// vol 1"), match the *song* instead and take art from its canonical
    /// release. Returns None when no title is known.
    async fn recording_front(
        &self,
        artist: &str,
        title: Option<&str>,
    ) -> Result<Option<CoverArt>, ArtFetch> {
        let Some(title) = title.filter(|t| !t.is_empty()) else {
            return Ok(None);
        };
        let q = format!(
            "recording:\"{}\" AND artist:\"{}\"",
            lucene_escape(title),
            lucene_escape(artist)
        );
        let recs: MbRecordings = self
            .mb_get("recording", &urlencoding::encode(&q))
            .await?;
        // Collect (release-group id, release id) pairs across the top
        // recordings, group id first so canonical art wins.
        let mut pairs: Vec<(Option<String>, String)> = Vec::new();
        for r in recs.recordings.iter().flat_map(|r| r.releases.iter()).take(4) {
            pairs.push((r.group.as_ref().map(|g| g.id.clone()), r.id.clone()));
        }
        for (gid, rid) in pairs.into_iter().take(4) {
            if let Some(gid) = gid.as_deref() {
                match self.caa_front("release-group", gid).await? {
                    Some(art) => return Ok(Some(art)),
                    None => {}
                }
            }
            match self.caa_front("release", &rid).await? {
                Some(art) => return Ok(Some(art)),
                None => continue,
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_trailing_decorations() {
        assert_eq!(
            strip_decorations("Hysteria (Single)").as_deref(),
            Some("Hysteria")
        );
        assert_eq!(
            strip_decorations("Album [Deluxe Edition]").as_deref(),
            Some("Album")
        );
        assert_eq!(
            strip_decorations("remaster - vol 1").as_deref(),
            Some("remaster")
        );
        assert_eq!(
            strip_decorations("Greatest Hits - Disc 2 (Remastered)").as_deref(),
            Some("Greatest Hits")
        );
    }

    #[test]
    fn strip_leaves_clean_titles_alone() {
        assert_eq!(strip_decorations("Random Access Memories"), None);
        assert_eq!(strip_decorations("Led Zeppelin IV"), None);
        // Mid-title parens aren't decorations — "(Deluxe) Remaster" keeps
        // its tail since the group doesn't end the string.
        assert_eq!(strip_decorations("Am (Deluxe) Remaster"), None);
        // Stripping to nothing yields no candidate.
        assert_eq!(strip_decorations("(Single)"), None);
    }

    #[test]
    fn lucene_quotes_and_backslashes() {
        assert_eq!(lucene_escape("a\"b"), "a\\\"b");
        assert_eq!(lucene_escape("a\\b"), "a\\\\b");
        assert_eq!(lucene_escape("plain"), "plain");
    }
}
