//! SolidTorrentsProvider — solidtorrents.to's DHT-index JSON API:
//! /api/v1/search?q=…&category=Audio&sort=seeders → results[] with
//! title/info_hash/size/seeders (or swarm.seeders)/magnet. `Gray` tier.
//! ISP-blocked on some networks — failures isolate into
//! provider_errors like every other provider; fixtures cover the parse.

use crate::lossless;
use crate::{
    AddableTorrent, LegalTier, ProviderCaps, ProviderError, ResolvedTorrent, SearchQuery,
    SearchResult, TorrentProvider,
};
use async_trait::async_trait;
use serde::Deserialize;

const BASE: &str = "https://solidtorrents.to";

pub struct SolidTorrentsProvider {
    client: reqwest::Client,
    base: String,
}

#[derive(Debug, Deserialize)]
struct Row {
    #[serde(rename = "_id", default)]
    id: Option<String>,
    title: Option<String>,
    info_hash: Option<String>,
    magnet: Option<String>,
    seeders: Option<u32>,
    size: Option<serde_json::Value>,
    num_files: Option<u32>,
    swarm: Option<Swarm>,
    imported: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize)]
struct Swarm {
    seeders: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct Resp {
    #[serde(default)]
    results: Vec<Row>,
}

impl Default for SolidTorrentsProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SolidTorrentsProvider {
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
        Self { base: base.into(), ..Self::new() }
    }

    fn result_from(&self, row: Row, q: &SearchQuery) -> Option<SearchResult> {
        let name = row.title?.trim().to_string();
        let ih = row.info_hash?.to_lowercase();
        if name.is_empty() || ih.chars().all(|c| c == '0') {
            return None;
        }
        let class = lossless::classify_title(&name);
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
        let size = row.size.and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str()?.parse().ok())
                .or_else(|| v.as_f64().map(|f| f as u64))
        });
        let seeds = row.seeders.or_else(|| row.swarm.and_then(|s| s.seeders));
        Some(SearchResult {
            id: row.id.unwrap_or_else(|| ih.clone()),
            provider: "solidtorrents".into(),
            name: name.clone(),
            infohash: Some(ih.clone()),
            magnet: row.magnet.or_else(|| {
                Some(format!(
                    "magnet:?xt=urn:btih:{ih}&dn={}",
                    urlencoding::encode(&name)
                ))
            }),
            torrent_url: None,
            size_bytes: size,
            file_count: row.num_files,
            seeds,
            downloads: None,
            uploaded_at: row.imported,
            license: None,
            source_page: Some(format!("{}/torrents/{ih}", self.base)),
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
impl TorrentProvider for SolidTorrentsProvider {
    fn id(&self) -> &'static str {
        "solidtorrents"
    }
    fn display_name(&self) -> &str {
        "SolidTorrents"
    }
    fn legal_tier(&self) -> LegalTier {
        LegalTier::Gray
    }
    fn capabilities(&self) -> ProviderCaps {
        ProviderCaps { seeds_known: true, needs_refresh: false, local_index: false }
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, ProviderError> {
        let url = format!(
            "{}/api/v1/search?q={}&category=Audio&sort=seeders&limit={}",
            self.base,
            urlencoding::encode(q.text.trim()),
            q.limit.clamp(1, 50)
        );
        let resp: Resp = self
            .client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(resp.results.into_iter().filter_map(|r| self.result_from(r, q)).collect())
    }

    async fn resolve(&self, r: &SearchResult) -> Result<ResolvedTorrent, ProviderError> {
        let magnet = r.magnet.clone().or_else(|| {
            r.infohash.as_ref().map(|ih| {
                format!("magnet:?xt=urn:btih:{ih}&dn={}", urlencoding::encode(&r.name))
            })
        });
        let magnet = magnet
            .ok_or_else(|| ProviderError::Unavailable(format!("{}: no magnet", r.id)))?;
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

    /// Recorded-shape solidtorrents rows — swarm.seeders variant +
    /// string size variant both covered.
    const RESP: &str = r#"{"results":[
        {"_id":"x1","title":"Aerosmith - Toys In The Attic [FLAC]","info_hash":"ABC123DEF4567890ABC123DEF4567890ABC12345","size":268435456,"num_files":9,"seeders":55,"category":"Audio","imported":"2024-01-01T00:00:00.000Z"},
        {"_id":"x2","title":"Aerosmith - Dream On (Single)","info_hash":"def4567890abc123def4567890abc123def45678","size":"11534336","swarm":{"seeders":12,"leechers":1},"category":"Audio"}
    ]}"#;

    #[test]
    fn rows_parse_size_and_swarm_variants() {
        let p = SolidTorrentsProvider::new();
        let q = SearchQuery { strict: false, ..SearchQuery::text("aerosmith") };
        let resp: Resp = serde_json::from_str(RESP).unwrap();
        let out: Vec<_> =
            resp.results.into_iter().filter_map(|r| p.result_from(r, &q)).collect();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].provider, "solidtorrents");
        assert_eq!(out[0].size_bytes, Some(268435456));
        assert_eq!(out[0].seeds, Some(55));
        assert_eq!(out[0].lossless, Some(true));
        assert_eq!(out[1].size_bytes, Some(11534336));
        assert_eq!(out[1].seeds, Some(12)); // swarm.seeders fallback
        assert!(out[1].magnet.is_some()); // synthesized from infohash
    }
}
