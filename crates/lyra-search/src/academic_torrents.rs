//! AcademicTorrentsProvider — AT has no server-side search; robots are
//! expected to pull the catalog and search locally. database.xml is
//! cached under the caller-supplied data dir, refreshed when >24h stale,
//! and queried by substring match. No per-file lists exist at this
//! layer, so results are `lossless: None` (unverifiable, not lossy).

use crate::{
    AddableTorrent, LegalTier, ProviderCaps, ProviderError, ResolvedTorrent, ResultFile,
    SearchQuery, SearchResult, TorrentProvider,
};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BASE: &str = "https://academictorrents.com";
const STALE_AFTER: Duration = Duration::from_secs(24 * 3600);
const CACHE_FILE: &str = "academictorrents-database.xml";

#[derive(Debug, Clone, Default)]
struct AtEntry {
    title: String,
    infohash: String,
    category: String,
    page_url: String,
    description: String,
    size_bytes: Option<u64>,
}

/// Parse database.xml (RSS: channel > item > title/infohash/category/
/// guid/description/size). Items without an infohash are dropped.
fn parse_database(xml: &str) -> Result<Vec<AtEntry>, ProviderError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| ProviderError::Parse(format!("database.xml: {e}")))?;
    let mut out = Vec::new();
    for item in doc.descendants().filter(|n| n.has_tag_name("item")) {
        let mut e = AtEntry::default();
        for c in item.children().filter(|n| n.is_element()) {
            let text = c.text().unwrap_or("").trim();
            match c.tag_name().name() {
                "title" => e.title = text.into(),
                "infohash" => e.infohash = text.to_lowercase(),
                "category" => e.category = text.into(),
                "guid" | "link" => {
                    if e.page_url.is_empty() {
                        e.page_url = text.into()
                    }
                }
                "description" => e.description = text.into(),
                "size" => e.size_bytes = text.parse().ok(),
                _ => {}
            }
        }
        if !e.infohash.is_empty() {
            out.push(e);
        }
    }
    Ok(out)
}

pub struct AcademicTorrentsProvider {
    client: reqwest::Client,
    base: String,
    cache_path: PathBuf,
    /// Parsed snapshot + when it was built (in-memory staleness).
    index: Mutex<Option<(Instant, Arc<Vec<AtEntry>>)>>,
}

impl AcademicTorrentsProvider {
    /// `data_dir` is where database.xml gets cached — caller supplies
    /// the app's data dir.
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            client: reqwest::Client::new(),
            base: BASE.into(),
            cache_path: data_dir.join(CACHE_FILE),
            index: Mutex::new(None),
        }
    }

    async fn entries(&self) -> Result<Arc<Vec<AtEntry>>, ProviderError> {
        if let Some((at, e)) = &*self.index.lock().unwrap() {
            if at.elapsed() < STALE_AFTER {
                return Ok(Arc::clone(e));
            }
        }
        // On disk and <24h old → parse without a fetch.
        if self.index.lock().unwrap().is_none() {
            if let Some(e) = self.from_fresh_cache() {
                *self.index.lock().unwrap() = Some((Instant::now(), Arc::clone(&e)));
                return Ok(e);
            }
        }
        match self.fetch().await {
            Ok(e) => {
                *self.index.lock().unwrap() = Some((Instant::now(), Arc::clone(&e)));
                Ok(e)
            }
            // Refresh failed — serve the stale snapshot rather than die.
            Err(err) => {
                if let Some((_, e)) = &*self.index.lock().unwrap() {
                    return Ok(Arc::clone(e));
                }
                if let Some(e) = self.from_stale_cache() {
                    *self.index.lock().unwrap() = Some((Instant::now(), Arc::clone(&e)));
                    return Ok(e);
                }
                Err(err)
            }
        }
    }

    fn from_fresh_cache(&self) -> Option<Arc<Vec<AtEntry>>> {
        let m = std::fs::metadata(&self.cache_path).ok()?;
        if m.modified().ok()?.elapsed().ok()? > STALE_AFTER {
            return None;
        }
        self.from_stale_cache()
    }

    fn from_stale_cache(&self) -> Option<Arc<Vec<AtEntry>>> {
        let xml = std::fs::read_to_string(&self.cache_path).ok()?;
        parse_database(&xml).ok().map(Arc::new)
    }

    async fn fetch(&self) -> Result<Arc<Vec<AtEntry>>, ProviderError> {
        let xml = self
            .client
            .get(format!("{}/database.xml", self.base))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let entries = Arc::new(parse_database(&xml)?);
        if let Some(d) = self.cache_path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&self.cache_path, &xml);
        Ok(entries)
    }
}

#[async_trait]
impl TorrentProvider for AcademicTorrentsProvider {
    fn id(&self) -> &'static str {
        "academic-torrents"
    }
    fn display_name(&self) -> &str {
        "Academic Torrents"
    }
    fn legal_tier(&self) -> LegalTier {
        LegalTier::Clear
    }
    fn capabilities(&self) -> ProviderCaps {
        ProviderCaps { seeds_known: false, needs_refresh: true, local_index: true }
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, ProviderError> {
        let entries = self.entries().await?;
        let toks: Vec<String> = q.text.split_whitespace().map(|t| t.to_lowercase()).collect();
        if toks.is_empty() {
            return Ok(vec![]);
        }
        let out = entries
            .iter()
            .filter(|e| {
                let hay = format!("{} {} {}", e.title, e.category, e.description).to_lowercase();
                toks.iter().all(|t| hay.contains(t))
            })
            .take(q.limit.clamp(1, 50))
            .map(|e| SearchResult {
                id: e.infohash.clone(),
                provider: "academic-torrents".into(),
                name: e.title.clone(),
                infohash: Some(e.infohash.clone()),
                magnet: None,
                torrent_url: Some(format!("{}/download/{}", self.base, e.infohash)),
                size_bytes: e.size_bytes,
                file_count: None,
                seeds: None,
                downloads: None,
                uploaded_at: None,
                license: None,
                source_page: if e.page_url.is_empty() { None } else { Some(e.page_url.clone()) },
                files_preview: vec![],
                health: Default::default(),
                formats: vec![],
                lossless: None,
                bit_depth: None,
                sample_rate: None,
            })
            .collect();
        Ok(out)
    }

    async fn resolve(&self, r: &SearchResult) -> Result<ResolvedTorrent, ProviderError> {
        let ih = r
            .infohash
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&r.id);
        if ih.is_empty() {
            return Err(ProviderError::Unavailable("no infohash".into()));
        }
        Ok(ResolvedTorrent {
            result: r.clone(),
            files: Vec::<ResultFile>::new(),
            addable: AddableTorrent::TorrentUrl(format!("{}/download/{ih}", self.base)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded database.xml, shrunk to three items.
    const DATABASE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel>
  <title>Academic Torrents</title>
  <item>
    <title>Free Music Archive - FLAC dataset</title>
    <category>dataset</category>
    <infohash>ABCD1234ABCD1234ABCD1234ABCD1234ABCD1234</infohash>
    <guid>https://academictorrents.com/details/abcd1234</guid>
    <description>Lossless audio collection from FMA.</description>
    <size>1073741824</size>
  </item>
  <item>
    <title>ImageNet 2012</title>
    <category>dataset</category>
    <infohash>1234ABCD1234ABCD1234ABCD1234ABCD1234ABCD</infohash>
    <guid>https://academictorrents.com/details/1234abcd</guid>
    <description>Images.</description>
    <size>158000000000</size>
  </item>
  <item>
    <title>Course: Signal Processing</title>
    <category>course</category>
    <infohash>FFFF0000FFFF0000FFFF0000FFFF0000FFFF0000</infohash>
    <description>Lecture videos.</description>
    <size>5368709120</size>
  </item>
</channel></rss>"#;

    #[test]
    fn parses_database_items() {
        let entries = parse_database(DATABASE_XML).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].infohash, "abcd1234abcd1234abcd1234abcd1234abcd1234");
        assert_eq!(entries[0].size_bytes, Some(1073741824));
        assert_eq!(entries[0].page_url, "https://academictorrents.com/details/abcd1234");
        // Third item has no guid — falls through fine.
        assert_eq!(entries[2].title, "Course: Signal Processing");
    }

    #[tokio::test]
    async fn local_substring_search() {
        let dir = std::env::temp_dir().join(format!("lyra-at-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(CACHE_FILE), DATABASE_XML).unwrap();
        let p = AcademicTorrentsProvider::new(dir.clone());
        let rs = p.search(&SearchQuery::text("music flac")).await.unwrap();
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].provider, "academic-torrents");
        assert_eq!(rs[0].lossless, None);
        assert_eq!(
            rs[0].torrent_url.as_deref(),
            Some("https://academictorrents.com/download/abcd1234abcd1234abcd1234abcd1234abcd1234")
        );
        let rs = p.search(&SearchQuery::text("signal")).await.unwrap();
        assert_eq!(rs.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
