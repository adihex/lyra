//! TorrentsCsvProvider — torrents-csv.com's DHT-scrape index behind a
//! tiny JSON API: /service/search?q=…&size=N → torrents[] with
//! infohash/seeders/size_bytes/created_unix. No scraping, no auth.
//! `LegalTier::Gray`. Rows synthesize magnets; resolve() is a re-wrap.

use crate::lossless;
use crate::{
    AddableTorrent, LegalTier, ProviderCaps, ProviderError, ResolvedTorrent, SearchQuery,
    SearchResult, TorrentProvider,
};
use async_trait::async_trait;
use serde::Deserialize;

const BASE: &str = "https://torrents-csv.com";

pub struct TorrentsCsvProvider {
    client: reqwest::Client,
    base: String,
}

#[derive(Debug, Deserialize)]
struct Row {
    infohash: String,
    name: String,
    size_bytes: Option<u64>,
    seeders: Option<u32>,
    created_unix: Option<i64>,
    id: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Resp {
    #[serde(default)]
    torrents: Vec<Row>,
}

impl Default for TorrentsCsvProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl TorrentsCsvProvider {
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

    fn result_from(&self, row: Row, q: &SearchQuery) -> Option<SearchResult> {
        let name = row.name.trim().to_string();
        if name.is_empty() || row.infohash.chars().all(|c| c == '0') {
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
        let ih = row.infohash.to_lowercase();
        Some(SearchResult {
            id: ih.clone(),
            provider: "torrents-csv".into(),
            magnet: Some(format!(
                "magnet:?xt=urn:btih:{ih}&dn={}",
                urlencoding::encode(&name)
            )),
            name,
            infohash: Some(ih),
            torrent_url: None,
            size_bytes: row.size_bytes,
            file_count: None,
            seeds: row.seeders,
            downloads: None,
            uploaded_at: row
                .created_unix
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0)),
            license: None,
            source_page: row
                .id
                .map(|id| format!("{}/#/detail/torrent/{id}", self.base)),
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
impl TorrentProvider for TorrentsCsvProvider {
    fn id(&self) -> &'static str {
        "torrents-csv"
    }
    fn display_name(&self) -> &str {
        "Torrents-CSV"
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
            "{}/service/search?q={}&size={}",
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
        Ok(resp
            .torrents
            .into_iter()
            .filter_map(|r| self.result_from(r, q))
            .collect())
    }

    async fn resolve(&self, r: &SearchResult) -> Result<ResolvedTorrent, ProviderError> {
        let magnet = r
            .magnet
            .clone()
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

    /// Recorded torrents-csv response shape — unix epochs, plain ints.
    const RESP: &str = r#"{"torrents":[
        {"infohash":"A5054ACECE05241EE5E1B243EACA177CAFFCDFEC","name":"Aerosmith - The Essential Hi-Res Aerosmith (24Bit-96kHz) FLAC 88","size_bytes":5059121927,"created_unix":1788238728,"seeders":38,"leechers":3,"completed":419,"scraped_date":1788257257,"id":32566},
        {"infohash":"bc770b815bf75e3bb6f319445802d16fe2e9c695","name":"Aerosmith - Night In The Ruts (1979) [FLAC] 88","size_bytes":265163033,"created_unix":1716443937,"seeders":25,"leechers":2,"completed":88,"scraped_date":1786879425,"id":53018},
        {"infohash":"0000000000000000000000000000000000000000","name":"","size_bytes":0,"seeders":0}
    ]}"#;

    #[test]
    fn rows_parse_and_classify() {
        let p = TorrentsCsvProvider::new();
        let q = SearchQuery {
            strict: false,
            ..SearchQuery::text("aerosmith")
        };
        let resp: Resp = serde_json::from_str(RESP).unwrap();
        let out: Vec<_> = resp
            .torrents
            .into_iter()
            .filter_map(|r| p.result_from(r, &q))
            .collect();
        assert_eq!(out.len(), 2); // zero-hash empty-name row dropped
        assert_eq!(out[0].provider, "torrents-csv");
        assert_eq!(out[0].seeds, Some(38));
        assert_eq!(out[0].lossless, Some(true));
        assert_eq!(out[0].bit_depth, Some(24));
        assert!(out[0].magnet.is_some());
    }
}
