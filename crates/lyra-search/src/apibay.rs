//! ApibayProvider — The Pirate Bay's JSON API (apibay.org), the same
//! endpoint qBittorrent's TPB plugin speaks. No HTML scraping:
//! q.php?q=…&cat=100 (Audio) returns rows with info_hash, seeders,
//! size; f.php?id= returns the file list at resolve time.
//! `LegalTier::Gray` — mixed-legality index, ranked below Clear.
//!
//! Rows are self-contained: infohash → magnet is synthesized locally,
//! resolve() only hits f.php for the file list (best-effort — a dead
//! f.php still yields a playable magnet).

use crate::lossless;
use crate::{
    AddableTorrent, LegalTier, ProviderCaps, ProviderError, ResolvedTorrent, ResultFile,
    SearchQuery, SearchResult, TorrentProvider,
};
use async_trait::async_trait;
use serde::Deserialize;

const BASE: &str = "https://apibay.org";
/// Audio parent category — covers Music/FLAC/Audiobooks/Sound-clips.
const CAT_AUDIO: &str = "100";

pub struct ApibayProvider {
    client: reqwest::Client,
    base: String,
}

/// apibay serializes every field as a string.
#[derive(Debug, Deserialize)]
struct ApiRow {
    id: String,
    name: String,
    info_hash: String,
    seeders: Option<String>,
    size: Option<String>,
    num_files: Option<String>,
    added: Option<String>,
}

impl Default for ApibayProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ApibayProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            base: BASE.into(),
        }
    }

    pub fn with_base(base: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            ..Self::new()
        }
    }

    fn row_from(&self, row: ApiRow, q: &SearchQuery) -> Option<SearchResult> {
        // The API's no-results sentinel is a fake row with id "0".
        if row.id == "0" || row.info_hash.chars().all(|c| c == '0') {
            return None;
        }
        let class = lossless::classify_title(&row.name);
        if q.strict {
            if let Some(c) = &class {
                if !lossless::acceptable(q).contains(&c.codec) {
                    return None;
                }
            }
        }
        let (formats, lossless_flag, depth, rate) = match class.as_ref() {
            Some(c) => (
                vec![c.codec.to_string()],
                Some(c.lossless),
                c.bit_depth,
                c.sample_rate,
            ),
            None => (Vec::new(), None, None, None),
        };
        let id = row.id.clone();
        Some(SearchResult {
            id: id.clone(),
            provider: "apibay".into(),
            name: row.name,
            infohash: Some(row.info_hash.to_lowercase()),
            magnet: None,
            torrent_url: None,
            size_bytes: row.size.and_then(|s| s.parse().ok()),
            file_count: row.num_files.and_then(|s| s.parse().ok()),
            seeds: row.seeders.and_then(|s| s.parse().ok()),
            downloads: None,
            uploaded_at: row
                .added
                .and_then(|s| s.parse::<i64>().ok())
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0)),
            license: None,
            source_page: Some(format!("https://thepiratebay.org/description.php?id={id}")),
            files_preview: vec![],
            health: Default::default(),
            formats,
            lossless: lossless_flag,
            bit_depth: depth,
            sample_rate: rate,
        })
    }
}

#[async_trait]
impl TorrentProvider for ApibayProvider {
    fn id(&self) -> &'static str {
        "apibay"
    }
    fn display_name(&self) -> &str {
        "The Pirate Bay"
    }
    fn legal_tier(&self) -> LegalTier {
        LegalTier::Gray
    }
    fn capabilities(&self) -> ProviderCaps {
        ProviderCaps {
            seeds_known: true,
            needs_refresh: false,
            local_index: false,
        }
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, ProviderError> {
        let url = format!(
            "{}/q.php?q={}&cat={CAT_AUDIO}",
            self.base,
            urlencoding::encode(q.text.trim())
        );
        let rows: Vec<ApiRow> = self
            .client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| self.row_from(r, q))
            .take(q.limit.clamp(1, 50))
            .collect())
    }

    async fn resolve(&self, r: &SearchResult) -> Result<ResolvedTorrent, ProviderError> {
        let ih = r
            .infohash
            .as_deref()
            .ok_or_else(|| ProviderError::Unavailable(format!("{}: no infohash", r.id)))?;
        let magnet = format!(
            "magnet:?xt=urn:btih:{ih}&dn={}",
            urlencoding::encode(&r.name)
        );
        // File list is informational — f.php outages must not lose the magnet.
        let files: Vec<ResultFile> = match self
            .client
            .get(format!("{}/f.php?id={}", self.base, r.id))
            .send()
            .await
        {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(v) => parse_files(&v),
                Err(_) => vec![],
            },
            Err(_) => vec![],
        };
        Ok(ResolvedTorrent {
            result: r.clone(),
            files,
            addable: AddableTorrent::Magnet(magnet),
        })
    }
}

/// f.php rows: {"name":["dir","file.flac"],"size":["12345"]} — path
/// segments arrive as arrays.
fn parse_files(v: &serde_json::Value) -> Vec<ResultFile> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    let path = f["name"]
                        .as_array()
                        .map(|p| {
                            p.iter()
                                .filter_map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join("/")
                        })
                        .or_else(|| f["name"].as_str().map(String::from))?;
                    let size = f["size"]
                        .as_array()
                        .and_then(|s| s.first()?.as_str()?.parse().ok())
                        .or_else(|| f["size"].as_str().and_then(|s| s.parse().ok()))
                        .or_else(|| f["size"].as_u64());
                    Some(ResultFile { path, size })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded-shape apibay rows — all values strings, incl. the
    /// id:"0"/zero-hash no-results sentinel.
    const ROWS: &str = r#"[
        {"id":"6825214","name":"Aerosmith - Toys In The Attic (1975) [FLAC]","info_hash":"ABC123DEF4567890ABC123DEF4567890ABC12345","leechers":"12","seeders":"340","num_files":"9","size":"268435456","username":"x","added":"1700000000","status":"vip","category":"104","imdb":""},
        {"id":"6825215","name":"Aerosmith - Dream On MP3 320","info_hash":"DEF4567890ABC123DEF4567890ABC12345ABC123","leechers":"2","seeders":"18","num_files":"1","size":"11534336","username":"y","added":"1700000100","status":"member","category":"101","imdb":""},
        {"id":"0","name":"No results returned","info_hash":"0000000000000000000000000000000000000000","leechers":"0","seeders":"0","num_files":"0","size":"0","username":"","added":"0","status":"member","category":"0","imdb":""}
    ]"#;

    #[test]
    fn rows_parse_and_sentinel_drops() {
        let p = ApibayProvider::new();
        let q = SearchQuery {
            strict: false,
            ..SearchQuery::text("aerosmith")
        };
        let rows: Vec<ApiRow> = serde_json::from_str(ROWS).unwrap();
        let out: Vec<_> = rows.into_iter().filter_map(|r| p.row_from(r, &q)).collect();
        assert_eq!(out.len(), 2); // sentinel row dropped
        let flac = &out[0];
        assert_eq!(flac.provider, "apibay");
        assert_eq!(
            flac.infohash.as_deref(),
            Some("abc123def4567890abc123def4567890abc12345")
        );
        assert_eq!(flac.seeds, Some(340));
        assert_eq!(flac.size_bytes, Some(268435456));
        assert_eq!(flac.file_count, Some(9));
        assert_eq!(flac.lossless, Some(true));
        assert_eq!(flac.formats, ["flac"]);
        assert_eq!(out[1].formats, ["mp3"]);
        assert_eq!(out[1].lossless, Some(false));
    }

    #[test]
    fn strict_drops_title_verified_lossy() {
        let p = ApibayProvider::new();
        let q = SearchQuery::text("aerosmith"); // strict
        let rows: Vec<ApiRow> = serde_json::from_str(ROWS).unwrap();
        let out: Vec<_> = rows.into_iter().filter_map(|r| p.row_from(r, &q)).collect();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].formats, ["flac"]);
    }

    #[test]
    fn fphp_path_arrays_join() {
        let v = serde_json::json!([
            {"name":["Aerosmith - Toys","01 - Dream On.flac"],"size":["11453256"]},
            {"name":["Aerosmith - Toys","02 - Toys.flac"],"size":["10111213"]},
            {"name":"plain.txt","size":"100"}
        ]);
        let files = parse_files(&v);
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].path, "Aerosmith - Toys/01 - Dream On.flac");
        assert_eq!(files[0].size, Some(11453256));
        assert_eq!(files[2].path, "plain.txt");
    }
}
