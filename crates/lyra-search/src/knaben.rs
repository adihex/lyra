//! KnabenProvider — knaben.org's DHT/index API (api.knaben.org/v1),
//! the largest public magnet index. POST JSON, hits carry `hash`,
//! `magnetUrl` (trackers already merged), `seeders`, `bytes`, `category`
//! and `details` (proxied source page). `LegalTier::Gray`.
//!
//! Rows are self-contained — magnetUrl lands on the row, resolve() is
//! a pure re-wrap. `category` soft-gates non-audio hits ("Audio / …"
//! survives; Movies/Games/etc. drop) since the API's categoryId scheme
//! doesn't map cleanly onto torznab codes.

use crate::lossless;
use crate::{
    AddableTorrent, LegalTier, ProviderCaps, ProviderError, ResolvedTorrent, SearchQuery,
    SearchResult, TorrentProvider,
};
use async_trait::async_trait;
use serde::Deserialize;

const BASE: &str = "https://api.knaben.org/v1";

pub struct KnabenProvider {
    client: reqwest::Client,
    base: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hit {
    title: Option<String>,
    hash: Option<String>,
    magnet_url: Option<String>,
    seeders: Option<u32>,
    peers: Option<u32>,
    bytes: Option<u64>,
    category: Option<String>,
    details: Option<String>,
    date: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize)]
struct Resp {
    #[serde(default)]
    hits: Vec<Hit>,
}

impl Default for KnabenProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl KnabenProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
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

    fn result_from(&self, h: Hit, q: &SearchQuery) -> Option<SearchResult> {
        let title = h.title?.trim().to_string();
        if title.is_empty() {
            return None;
        }
        // Audio soft-gate: knaben indexes everything; for a music query
        // non-audio categories are pure noise. `category` can be null —
        // those rows stay (classify + ranking decide).
        if let Some(c) = &h.category {
            if !c.to_ascii_lowercase().contains("audio") {
                return None;
            }
        }
        let hash = h.hash?.to_lowercase();
        if hash.chars().all(|c| c == '0') {
            return None;
        }
        let class = lossless::classify_title(&title);
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
        let magnet = h.magnet_url.or_else(|| {
            Some(format!(
                "magnet:?xt=urn:btih:{hash}&dn={}",
                urlencoding::encode(&title)
            ))
        });
        Some(SearchResult {
            id: hash.clone(),
            provider: "knaben".into(),
            name: title,
            infohash: Some(hash),
            magnet,
            torrent_url: None,
            size_bytes: h.bytes,
            file_count: None,
            seeds: h.seeders.or(h.peers),
            downloads: None,
            uploaded_at: h.date,
            license: None,
            source_page: h.details.filter(|d| d.starts_with("http")),
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
impl TorrentProvider for KnabenProvider {
    fn id(&self) -> &'static str {
        "knaben"
    }
    fn display_name(&self) -> &str {
        "Knaben"
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
        let body = serde_json::json!({
            "search_type": "100%",
            "query": q.text.trim(),
            "size": q.limit.clamp(1, 50),
            "order_by": "seeders",
        });
        let resp: Resp = self
            .client
            .post(&self.base)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(resp
            .hits
            .into_iter()
            .filter_map(|h| self.result_from(h, q))
            .collect())
    }

    async fn resolve(&self, r: &SearchResult) -> Result<ResolvedTorrent, ProviderError> {
        let magnet = r.magnet.clone().or_else(|| {
            r.infohash.as_ref().map(|ih| {
                format!(
                    "magnet:?xt=urn:btih:{ih}&dn={}",
                    urlencoding::encode(&r.name)
                )
            })
        });
        let magnet =
            magnet.ok_or_else(|| ProviderError::Unavailable(format!("{}: no magnet", r.id)))?;
        Ok(ResolvedTorrent {
            result: r.clone(),
            files: vec![],
            addable: AddableTorrent::Magnet(magnet),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded-shape knaben hit fields (camelCase, magnetUrl full).
    const RESP: &str = r#"{
        "hits": [
            {"title":"Aerosmith - Toys In The Attic (1975) [FLAC]","hash":"ABC123DEF4567890ABC123DEF4567890ABC12345","magnetUrl":"magnet:?xt=urn:btih:ABC123DEF4567890ABC123DEF4567890ABC12345&dn=x","seeders":42,"peers":50,"bytes":268435456,"category":"Audio / FLAC","details":"https://knaben.xyz/x/1","date":"2024-01-01T00:00:00+00:00"},
            {"title":"Aerosmith - Dream On MP3 320","hash":"DEF4567890ABC123DEF4567890ABC12345ABC123","seeders":7,"bytes":11534336,"category":"Audio / MP3","details":"https://knaben.xyz/x/2"},
            {"title":"Aerosmith movie clip","hash":"1111111111111111111111111111111111111111","seeders":9,"bytes":999,"category":"Video / Movies"},
            {"title":"","hash":"2222222222222222222222222222222222222222"}
        ]
    }"#;

    #[test]
    fn hits_parse_gate_and_classify() {
        let p = KnabenProvider::new();
        let q = SearchQuery {
            strict: false,
            ..SearchQuery::text("aerosmith")
        };
        let resp: Resp = serde_json::from_str(RESP).unwrap();
        let out: Vec<_> = resp
            .hits
            .into_iter()
            .filter_map(|h| p.result_from(h, &q))
            .collect();
        // Video row + empty-title row drop; both Audio rows survive.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].provider, "knaben");
        assert_eq!(out[0].lossless, Some(true));
        assert_eq!(out[0].seeds, Some(42));
        assert_eq!(out[0].size_bytes, Some(268435456));
        assert!(out[0]
            .magnet
            .as_deref()
            .unwrap()
            .starts_with("magnet:?xt=urn:btih:ABC123"));
        assert_eq!(out[1].formats, ["mp3"]);
    }

    #[test]
    fn strict_drops_verified_lossy() {
        let p = KnabenProvider::new();
        let q = SearchQuery::text("aerosmith");
        let resp: Resp = serde_json::from_str(RESP).unwrap();
        let out: Vec<_> = resp
            .hits
            .into_iter()
            .filter_map(|h| p.result_from(h, &q))
            .collect();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].formats, ["flac"]);
    }

    #[tokio::test]
    async fn resolve_synthesizes_magnet_from_hash() {
        let p = KnabenProvider::new();
        let q = SearchQuery {
            strict: false,
            ..SearchQuery::text("x")
        };
        let resp: Resp = serde_json::from_str(RESP).unwrap();
        let row = resp
            .hits
            .into_iter()
            .nth(1) // MP3 row — no magnetUrl on it
            .and_then(|h| p.result_from(h, &q))
            .unwrap();
        let r = p.resolve(&row).await.unwrap();
        match r.addable {
            AddableTorrent::Magnet(m) => assert!(m.starts_with("magnet:?xt=urn:btih:def456")),
            _ => panic!("expected magnet"),
        }
    }
}
