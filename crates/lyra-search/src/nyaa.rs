//! NyaaProvider — nyaa.si's RSS search feed (anime/J-music stronghold,
//! Audio category c=2_0). No scraping: RSS items carry nyaa:infoHash,
//! nyaa:seeders, nyaa:size ("48.6 MiB"), nyaa:categoryId (2_1 =
//! Audio-Lossless — an authoritative lossless signal) and a <link>
//! straight to the .torrent — resolve() prefers TorrentUrl.
//! `LegalTier::Gray`.

use crate::lossless;
use crate::{
    AddableTorrent, LegalTier, ProviderCaps, ProviderError, ResolvedTorrent, SearchQuery,
    SearchResult, TorrentProvider,
};
use async_trait::async_trait;

const BASE: &str = "https://nyaa.si";

pub struct NyaaProvider {
    client: reqwest::Client,
    base: String,
}

#[derive(Default)]
pub(crate) struct Item {
    title: String,
    torrent_url: Option<String>,
    page: Option<String>,
    infohash: Option<String>,
    seeds: Option<u32>,
    downloads: Option<u64>,
    size: Option<u64>,
    /// "2_1" lossless / "2_2" lossy — nyaa's own audio split.
    category_id: Option<String>,
    uploaded_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// "48.6 MiB" / "1.2 GiB" → bytes. Unknown units return None.
pub(crate) fn human_size(s: &str) -> Option<u64> {
    let mut it = s.trim().split_whitespace();
    let n: f64 = it.next()?.parse().ok()?;
    let mult = match it.next()?.to_ascii_uppercase().as_str() {
        "B" => 1.0,
        "KIB" => 1024.0,
        "MIB" => 1024.0 * 1024.0,
        "GIB" => 1024.0 * 1024.0 * 1024.0,
        "TIB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * mult) as u64)
}

impl Default for NyaaProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl NyaaProvider {
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

    pub(crate) fn parse_feed(xml: &str) -> Vec<Item> {
        let Ok(doc) = roxmltree::Document::parse(xml) else { return vec![] };
        doc.descendants()
            .filter(|n| n.has_tag_name("item"))
            .map(|item| {
                let mut it = Item::default();
                for c in item.children().filter(|n| n.is_element()) {
                    let text = c.text().map(str::trim).unwrap_or("");
                    match (c.tag_name().namespace(), c.tag_name().name()) {
                        (_, "title") => it.title = text.to_string(),
                        (_, "link") => it.torrent_url = Some(text.to_string()),
                        (_, "guid") => it.page = Some(text.to_string()),
                        (_, "pubDate") => {
                            it.uploaded_at = chrono::DateTime::parse_from_rfc2822(text)
                                .ok()
                                .map(|d| d.with_timezone(&chrono::Utc))
                        }
                        (Some(ns), "infoHash") if ns.contains("nyaa") => {
                            it.infohash = Some(text.to_lowercase())
                        }
                        (Some(ns), "seeders") if ns.contains("nyaa") => {
                            it.seeds = text.parse().ok()
                        }
                        (Some(ns), "downloads") if ns.contains("nyaa") => {
                            it.downloads = text.parse().ok()
                        }
                        (Some(ns), "size") if ns.contains("nyaa") => {
                            it.size = human_size(text)
                        }
                        (Some(ns), "categoryId") if ns.contains("nyaa") => {
                            it.category_id = Some(text.to_string())
                        }
                        _ => {}
                    }
                }
                it
            })
            .filter(|it| !it.title.is_empty())
            .collect()
    }

    fn result_from(&self, it: Item, q: &SearchQuery) -> Option<SearchResult> {
        let class = lossless::classify_title(&it.title);
        // nyaa's 2_1 = Audio-Lossless is authoritative even when the
        // title omits a codec tag; 2_2 = Audio-Lossy.
        let cat_lossless = match it.category_id.as_deref() {
            Some("2_1") => Some(true),
            Some("2_2") => Some(false),
            _ => None,
        };
        let lossless_flag = cat_lossless.or(class.as_ref().map(|c| c.lossless));
        if q.strict && lossless_flag == Some(false) {
            return None;
        }
        if q.strict {
            if let Some(c) = &class {
                if !lossless::acceptable(q).contains(&c.codec) {
                    return None;
                }
            }
        }
        let (formats, depth, rate) = match class.as_ref() {
            Some(c) => (vec![c.codec.to_string()], c.bit_depth, c.sample_rate),
            None => (
                match lossless_flag {
                    Some(true) => vec!["flac".into()],
                    _ => vec![],
                },
                None,
                None,
            ),
        };
        let ih = it.infohash;
        let magnet = ih.as_ref().map(|ih| {
            format!("magnet:?xt=urn:btih:{ih}&dn={}", urlencoding::encode(&it.title))
        });
        Some(SearchResult {
            id: ih.clone()
                .or_else(|| it.torrent_url.clone())
                .unwrap_or_else(|| it.title.clone()),
            provider: "nyaa".into(),
            name: it.title,
            infohash: ih,
            magnet,
            torrent_url: it.torrent_url.filter(|u| u.starts_with("http")),
            size_bytes: it.size,
            file_count: None,
            seeds: it.seeds,
            downloads: it.downloads,
            uploaded_at: it.uploaded_at,
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
impl TorrentProvider for NyaaProvider {
    fn id(&self) -> &'static str {
        "nyaa"
    }
    fn display_name(&self) -> &str {
        "Nyaa"
    }
    fn legal_tier(&self) -> LegalTier {
        LegalTier::Gray
    }
    fn capabilities(&self) -> ProviderCaps {
        ProviderCaps { seeds_known: true, needs_refresh: false, local_index: false }
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, ProviderError> {
        let url = format!(
            "{}/?page=rss&q={}&c=2_0",
            self.base,
            urlencoding::encode(q.text.trim())
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
            .take(q.limit.clamp(1, 50))
            .filter_map(|it| self.result_from(it, q))
            .collect())
    }

    async fn resolve(&self, r: &SearchResult) -> Result<ResolvedTorrent, ProviderError> {
        // Direct .torrent link beats a synthesized magnet — real
        // trackers + exact file list for free.
        if let Some(u) = &r.torrent_url {
            return Ok(ResolvedTorrent {
                result: r.clone(),
                files: vec![],
                addable: AddableTorrent::TorrentUrl(u.clone()),
            });
        }
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

    /// Recorded nyaa RSS shape — nyaa: namespaced fields, human sizes.
    const FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss xmlns:atom="http://www.w3.org/2005/Atom" xmlns:nyaa="https://nyaa.si/xmlns/nyaa" version="2.0">
<channel>
<item>
  <title>[JMAX] Liella! - Skip Capsule [FLAC 48kHz/24bit]</title>
  <link>https://nyaa.si/download/2161757.torrent</link>
  <guid isPermaLink="true">https://nyaa.si/view/2161757</guid>
  <pubDate>Tue, 15 Sep 2026 12:01:35 -0000</pubDate>
  <nyaa:seeders>31</nyaa:seeders>
  <nyaa:leechers>33</nyaa:leechers>
  <nyaa:downloads>602</nyaa:downloads>
  <nyaa:infoHash>8CF6896060E45FFDE91FD48A9E8C3413879A0DED</nyaa:infoHash>
  <nyaa:categoryId>2_1</nyaa:categoryId>
  <nyaa:category>Audio - Lossless</nyaa:category>
  <nyaa:size>48.6 MiB</nyaa:size>
</item>
<item>
  <title>[JMAX] Liella! - Skip Capsule [MP3]</title>
  <link>https://nyaa.si/download/2161724.torrent</link>
  <guid isPermaLink="true">https://nyaa.si/view/2161724</guid>
  <nyaa:seeders>5</nyaa:seeders>
  <nyaa:infoHash>def4567890abc123def4567890abc123def45678</nyaa:infoHash>
  <nyaa:categoryId>2_2</nyaa:categoryId>
  <nyaa:size>9.2 MiB</nyaa:size>
</item>
</channel>
</rss>"#;

    #[test]
    fn human_sizes() {
        assert_eq!(human_size("48.6 MiB"), Some(50_960_793));
        assert_eq!(human_size("1.2 GiB"), Some(1_288_490_188));
        assert_eq!(human_size("512 B"), Some(512));
        assert_eq!(human_size("bogus"), None);
    }

    #[test]
    fn feed_parses_nyaa_fields() {
        let items = NyaaProvider::parse_feed(FEED);
        assert_eq!(items.len(), 2);
        let a = &items[0];
        assert_eq!(a.seeds, Some(31));
        assert_eq!(a.downloads, Some(602));
        assert_eq!(a.size, Some(50_960_793));
        assert_eq!(a.infohash.as_deref(), Some("8cf6896060e45ffde91fd48a9e8c3413879a0ded"));
        assert_eq!(a.torrent_url.as_deref(), Some("https://nyaa.si/download/2161757.torrent"));
        assert_eq!(a.category_id.as_deref(), Some("2_1"));
    }

    #[test]
    fn category_id_is_authoritative_lossless() {
        let p = NyaaProvider::new();
        let q = SearchQuery { strict: false, ..SearchQuery::text("x") };
        let items = NyaaProvider::parse_feed(FEED);
        let rows: Vec<_> =
            items.into_iter().filter_map(|it| p.result_from(it, &q)).collect();
        assert_eq!(rows[0].lossless, Some(true));
        assert_eq!(rows[1].lossless, Some(false));
        // Strict drops the 2_2 (Audio-Lossy) row outright.
        let rows: Vec<_> = NyaaProvider::parse_feed(FEED)
            .into_iter()
            .filter_map(|it| p.result_from(it, &SearchQuery::text("x")))
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].formats, ["flac"]);
    }
}
