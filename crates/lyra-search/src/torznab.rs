//! TorznabProvider — the indexer protocol Jackett/Prowlarr speak. One
//! implementation = every indexer a user's aggregator carries, including
//! cards that already solve Cloudflare/bot walls. `LegalTier::External`:
//! user-configured endpoint, never shipped as a default.
//!
//! Search: GET {base}/api?t=search&q=…&cat=3000&apikey=… — RSS XML with
//! torznab:attr fields (seeders, size, infohash, magneturl). resolve()
//! prefers a direct magneturl attr; otherwise follows the item's
//! link/enclosure — Jackett's /dl/ endpoints either 302 to a magnet or
//! serve .torrent bytes, so redirects are handled manually (reqwest
//! can't follow Location: magnet:).

use crate::lossless;
use crate::{
    AddableTorrent, LegalTier, ProviderCaps, ProviderError, ResolvedTorrent, SearchQuery,
    SearchResult, TorrentProvider,
};
use async_trait::async_trait;
use std::sync::atomic::{AtomicU32, Ordering};

/// Torznab audio categories: Audio, MP3, Lossless, Audiobook… — caps at
/// the standard 30xx block.
const AUDIO_CATS: &str = "3000,3010,3020,3030,3040,3050,3060";

static INSTANCE_SEQ: AtomicU32 = AtomicU32::new(0);

/// A configured Torznab endpoint — one Jackett/Prowlarr indexer (or the
/// aggregator's "all" endpoint).
pub struct TorznabProvider {
    client: reqwest::Client,
    /// Redirect-off client for resolve() — Jackett /dl/ 302s to magnet:
    /// which reqwest can't follow (non-http scheme).
    no_redirect: reqwest::Client,
    /// Provider id — `torznab` for a single instance; registering a
    /// second endpoint gets torznab-2, -3, …
    id: String,
    /// Human label for logs/UI display name.
    name: String,
    /// Full base before /api — e.g. http://host:9117/api/v2.0/indexers/all
    base: String,
    apikey: String,
}

/// One parsed <item> → SearchResult field set.
#[derive(Clone)]
pub(crate) struct ParsedItem {
    pub title: String,
    pub link: Option<String>,
    pub magnet: Option<String>,
    pub infohash: Option<String>,
    pub size: Option<u64>,
    pub seeds: Option<u32>,
    pub files: Option<u32>,
    pub page: Option<String>,
}

impl TorznabProvider {
    pub fn new(base: impl Into<String>, apikey: impl Into<String>, name: Option<&str>) -> Self {
        let n = INSTANCE_SEQ.fetch_add(1, Ordering::Relaxed);
        let base = base.into().trim_end_matches('/').to_string();
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
            no_redirect: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
            id: if n == 0 {
                "torznab".into()
            } else {
                format!("torznab-{n}")
            },
            name: name.unwrap_or("Torznab").to_string(),
            base,
            apikey: apikey.into(),
        }
    }

    /// Parse the torznab RSS — channel/item level. Public-in-crate for
    /// fixtures and tests.
    pub(crate) fn parse_feed(xml: &str) -> Vec<ParsedItem> {
        let Ok(doc) = roxmltree::Document::parse(xml) else {
            return vec![];
        };
        doc.descendants()
            .filter(|n| n.has_tag_name("item"))
            .map(|item| {
                let mut p = ParsedItem {
                    title: String::new(),
                    link: None,
                    magnet: None,
                    infohash: None,
                    size: None,
                    seeds: None,
                    files: None,
                    page: None,
                };
                for c in item.children().filter(|n| n.is_element()) {
                    match c.tag_name().name() {
                        "title" => p.title = c.text().unwrap_or("").trim().to_string(),
                        "link" => p.link = c.text().map(|s| s.trim().to_string()),
                        "guid" => p.page = c.text().map(|s| s.trim().to_string()),
                        "size" => p.size = c.text().and_then(|s| s.trim().parse().ok()),
                        "enclosure" => {
                            if p.link.is_none() {
                                p.link = c.attribute("url").map(String::from);
                            }
                        }
                        "attr" => match c.attribute("name") {
                            Some("seeders") => {
                                p.seeds = c.attribute("value").and_then(|v| v.parse().ok())
                            }
                            Some("size") => {
                                if p.size.is_none() {
                                    p.size = c.attribute("value").and_then(|v| v.parse().ok())
                                }
                            }
                            Some("infohash") => {
                                p.infohash = c.attribute("value").map(|v| v.to_lowercase())
                            }
                            Some("magneturl") => p.magnet = c.attribute("value").map(String::from),
                            Some("files") => {
                                p.files = c.attribute("value").and_then(|v| v.parse().ok())
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
                p
            })
            .filter(|p| !p.title.is_empty())
            .collect()
    }

    fn result_from(&self, it: ParsedItem, q: &SearchQuery) -> Option<SearchResult> {
        let class = lossless::classify_title(&it.title);
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
        // Row id: infohash when known, else the link — resolve() keys
        // off whichever the indexer gave us.
        let id = it
            .infohash
            .clone()
            .or_else(|| it.link.clone())
            .unwrap_or_else(|| it.title.clone());
        let magnet = it
            .magnet
            .clone()
            .or_else(|| it.link.clone().filter(|l| l.starts_with("magnet:")));
        Some(SearchResult {
            id,
            provider: self.id.clone(),
            name: it.title,
            infohash: it.infohash,
            magnet,
            torrent_url: it.link.filter(|l| l.starts_with("http")),
            size_bytes: it.size,
            file_count: it.files,
            seeds: it.seeds,
            downloads: None,
            uploaded_at: None,
            license: None,
            source_page: it.page.filter(|p| p.starts_with("http")),
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
impl TorrentProvider for TorznabProvider {
    fn id(&self) -> &'static str {
        // Leak-per-instance: provider ids are &'static by trait. Runtime-
        // configured providers are born once per process — the leak is
        // bounded (a handful of endpoints), the trait is sealed.
        Box::leak(self.id.clone().into_boxed_str())
    }
    fn display_name(&self) -> &str {
        &self.name
    }
    fn legal_tier(&self) -> LegalTier {
        LegalTier::External
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
            "{}/api?t=search&cat={AUDIO_CATS}&q={}&apikey={}&limit={}",
            self.base,
            urlencoding::encode(q.text.trim()),
            urlencoding::encode(&self.apikey),
            q.limit.clamp(1, 50)
        );
        let body = self
            .client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(Self::parse_feed(&body)
            .into_iter()
            .filter_map(|it| self.result_from(it, q))
            .collect())
    }

    async fn resolve(&self, r: &SearchResult) -> Result<ResolvedTorrent, ProviderError> {
        // Magnet already on the row (magneturl attr or magnet link) —
        // zero extra fetches.
        if let Some(m) = r.magnet.clone().or_else(|| {
            r.infohash.as_ref().map(|ih| {
                format!(
                    "magnet:?xt=urn:btih:{ih}&dn={}",
                    urlencoding::encode(&r.name)
                )
            })
        }) {
            return Ok(ResolvedTorrent {
                result: r.clone(),
                files: vec![],
                addable: AddableTorrent::Magnet(m),
            });
        }
        // Jackett /dl/ link: follow manually — a 302 to magnet: can't be
        // followed by reqwest (non-http scheme); a 200 serves .torrent
        // bytes.
        let link = r
            .torrent_url
            .clone()
            .ok_or_else(|| ProviderError::Unavailable(format!("{}: no link", r.name)))?;
        let resp = self.no_redirect.get(&link).send().await?;
        let addable = if resp.status().is_redirection() {
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| ProviderError::Parse("redirect sans location".into()))?;
            if loc.starts_with("magnet:") {
                AddableTorrent::Magnet(loc.to_string())
            } else {
                AddableTorrent::TorrentUrl(loc.to_string())
            }
        } else {
            let bytes = resp.error_for_status()?.bytes().await?.to_vec();
            // Sanity: torrent files are bencoded dicts.
            if !bytes.starts_with(b"d") {
                return Err(ProviderError::Parse("dl endpoint: not a torrent".into()));
            }
            AddableTorrent::TorrentBytes(bytes)
        };
        Ok(ResolvedTorrent {
            result: r.clone(),
            files: vec![],
            addable,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded-shape torznab feed — Jackett's standard emission:
    /// <item> with title/link/guid/size + torznab:attr seeders, files,
    /// infohash, magneturl.
    const FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">
<channel>
<item>
  <title>Aerosmith - Toys In The Attic (1975) [FLAC]</title>
  <guid>https://idx.example/torrent/12345</guid>
  <link>http://jackett.local:9117/dl/card1/?jackett_apikey=k&amp;path=abc</link>
  <size>268435456</size>
  <pubDate>Mon, 01 Jan 2024 00:00:00 +0000</pubDate>
  <torznab:attr name="category" value="3040"/>
  <torznab:attr name="seeders" value="42"/>
  <torznab:attr name="peers" value="50"/>
  <torznab:attr name="files" value="9"/>
  <torznab:attr name="infohash" value="ABC123DEF4567890ABC123DEF4567890ABC12345"/>
  <torznab:attr name="magneturl" value="magnet:?xt=urn:btih:ABC123DEF4567890ABC123DEF4567890ABC12345&amp;dn=Aerosmith"/>
</item>
<item>
  <title>Aerosmith - Dream On (Single) MP3 320</title>
  <guid>https://idx.example/torrent/67890</guid>
  <link>http://jackett.local:9117/dl/card1/?jackett_apikey=k&amp;path=def</link>
  <size>11534336</size>
  <torznab:attr name="seeders" value="7"/>
</item>
<item><title></title></item>
</channel>
</rss>"#;

    #[test]
    fn feed_parses_items_and_attrs() {
        let items = TorznabProvider::parse_feed(FEED);
        assert_eq!(items.len(), 2); // empty-title item dropped
        let a = &items[0];
        assert_eq!(a.title, "Aerosmith - Toys In The Attic (1975) [FLAC]");
        assert_eq!(a.seeds, Some(42));
        assert_eq!(a.size, Some(268435456));
        assert_eq!(a.files, Some(9));
        assert_eq!(
            a.infohash.as_deref(),
            Some("abc123def4567890abc123def4567890abc12345")
        );
        assert!(a
            .magnet
            .as_deref()
            .unwrap()
            .starts_with("magnet:?xt=urn:btih:ABC123"));
        assert!(a
            .link
            .as_deref()
            .unwrap()
            .starts_with("http://jackett.local"));
    }

    #[test]
    fn rows_classify_and_strict_filters() {
        let p = TorznabProvider::new("http://jackett.local:9117/api/v2.0/indexers/x", "k", None);
        let items = TorznabProvider::parse_feed(FEED);
        let loose = SearchQuery {
            strict: false,
            ..SearchQuery::text("aerosmith")
        };
        let rows: Vec<_> = items
            .clone()
            .into_iter()
            .filter_map(|it| p.result_from(it, &loose))
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].formats, ["flac"]);
        assert_eq!(rows[0].lossless, Some(true));
        // INSTANCE_SEQ makes the id order-dependent under parallel tests.
        assert!(rows[0].provider.starts_with("torznab"));
        // Magnet lifted straight onto the row — resolve needs no fetch.
        assert!(rows[0].magnet.is_some());

        let strict = SearchQuery::text("aerosmith");
        let rows: Vec<_> = items
            .into_iter()
            .filter_map(|it| p.result_from(it, &strict))
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].formats, ["flac"]);
    }

    #[test]
    fn infohash_magnet_fallback_on_empty_attr() {
        let p = TorznabProvider::new("http://x", "k", None);
        let it = ParsedItem {
            title: "Show FLAC".into(),
            link: Some("http://j/dl/1".into()),
            magnet: None,
            infohash: Some("abc123def4567890abc123def4567890abc12345".into()),
            size: None,
            seeds: None,
            files: None,
            page: None,
        };
        let r = p.result_from(it, &SearchQuery::text("x")).unwrap();
        assert_eq!(r.id, "abc123def4567890abc123def4567890abc12345");
    }
}
