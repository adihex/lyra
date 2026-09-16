//! SearchEngine: fan-out over providers with per-provider timeout and
//! error isolation, then merge — dedupe by infohash, lossless first.

use crate::{ProviderError, ProviderIssue, ResolvedTorrent, SearchQuery, SearchResponse, SearchResult, TorrentProvider};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

pub struct SearchEngine {
    providers: Vec<Arc<dyn TorrentProvider>>,
    timeout: Duration,
}

impl SearchEngine {
    pub fn new(providers: Vec<Arc<dyn TorrentProvider>>) -> Self {
        Self { providers, timeout: Duration::from_secs(8) }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// Fan out to the query's providers (all when unspecified). One dead
    /// or slow provider surfaces in provider_errors, never fails the query.
    pub async fn search(&self, q: &SearchQuery) -> SearchResponse {
        let jobs: Vec<_> = self
            .providers
            .iter()
            .filter(|p| {
                q.providers
                    .as_ref()
                    .map_or(true, |ids| ids.iter().any(|i| i == p.id()))
            })
            .map(|p| {
                let timeout = self.timeout;
                async move {
                    match tokio::time::timeout(timeout, p.search(q)).await {
                        Ok(Ok(rs)) => Ok(rs),
                        Ok(Err(e)) => Err((p.id(), e.to_string())),
                        Err(_) => Err((p.id(), "timeout".to_string())),
                    }
                }
            })
            .collect();
        let mut resp = SearchResponse::default();
        for r in futures::future::join_all(jobs).await {
            match r {
                Ok(rs) => resp.results.extend(rs),
                Err((provider, error)) => resp.provider_errors.push(ProviderIssue { provider: provider.into(), error }),
            }
        }
        // Provider-native relevance is already within each block; the
        // merge ranks across providers: lossless → album-ness →
        // depth/rate → downloads → name. Stable, so equal keys keep
        // provider order.
        resp.results.sort_by(|a, b| {
            rank_key(b)
                .cmp(&rank_key(a))
                .then_with(|| a.name.cmp(&b.name))
        });
        let mut seen = HashSet::new();
        resp.results.retain(|r| match &r.infohash {
            Some(ih) => seen.insert(ih.to_lowercase()),
            None => true,
        });
        resp
    }

    pub async fn resolve(&self, r: &SearchResult) -> Result<ResolvedTorrent, ProviderError> {
        let p = self
            .providers
            .iter()
            .find(|p| p.id() == r.provider)
            .ok_or_else(|| ProviderError::Unavailable(format!("no provider '{}'", r.provider)))?;
        p.resolve(r).await
    }
}

/// (tier, file_count, bit_depth, sample_rate, downloads, seeds) —
/// compared descending. tier: 2 verified lossless, 1 unknown, 0 lossy.
fn rank_key(r: &SearchResult) -> (u8, u64, u64, u64, u64, u64) {
    (
        match r.lossless {
            Some(true) => 2,
            None => 1,
            Some(false) => 0,
        },
        r.file_count.unwrap_or(0) as u64,
        r.bit_depth.unwrap_or(0) as u64,
        r.sample_rate.unwrap_or(0) as u64,
        r.downloads.unwrap_or(0),
        r.seeds.unwrap_or(0) as u64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LegalTier, ProviderCaps};
    use async_trait::async_trait;
    use std::sync::Mutex as StdMutex;

    struct Mock {
        id: &'static str,
        out: StdMutex<Option<Result<Vec<SearchResult>, ProviderError>>>,
        delay: Duration,
    }

    impl Mock {
        fn ok(id: &'static str, rs: Vec<SearchResult>) -> Self {
            Self { id, out: StdMutex::new(Some(Ok(rs))), delay: Duration::ZERO }
        }
        fn err(id: &'static str, msg: &str) -> Self {
            Self {
                id,
                out: StdMutex::new(Some(Err(ProviderError::Unavailable(msg.into())))),
                delay: Duration::ZERO,
            }
        }
        fn slow(id: &'static str, delay: Duration) -> Self {
            Self { id, out: StdMutex::new(Some(Ok(vec![]))), delay }
        }
    }

    #[async_trait]
    impl TorrentProvider for Mock {
        fn id(&self) -> &'static str {
            self.id
        }
        fn display_name(&self) -> &str {
            self.id
        }
        fn legal_tier(&self) -> LegalTier {
            LegalTier::Clear
        }
        fn capabilities(&self) -> ProviderCaps {
            ProviderCaps { seeds_known: false, needs_refresh: false, local_index: false }
        }
        async fn search(&self, _q: &SearchQuery) -> Result<Vec<SearchResult>, ProviderError> {
            if self.delay > Duration::ZERO {
                tokio::time::sleep(self.delay).await;
            }
            self.out.lock().unwrap().take().unwrap()
        }
        async fn resolve(&self, r: &SearchResult) -> Result<ResolvedTorrent, ProviderError> {
            Ok(ResolvedTorrent {
                result: r.clone(),
                files: vec![],
                addable: crate::AddableTorrent::TorrentUrl("https://x/t.torrent".into()),
            })
        }
    }

    fn result(id: &str, provider: &str, ih: Option<&str>, lossless: Option<bool>) -> SearchResult {
        SearchResult {
            id: id.into(),
            provider: provider.into(),
            name: format!("name-{id}"),
            infohash: ih.map(str::to_string),
            magnet: None,
            torrent_url: None,
            size_bytes: None,
            file_count: None,
            seeds: None,
            downloads: None,
            uploaded_at: None,
            license: None,
            source_page: None,
            files_preview: vec![],
            health: Default::default(),
            formats: vec![],
            lossless,
            bit_depth: None,
            sample_rate: None,
        }
    }

    #[tokio::test]
    async fn merges_and_isolates_failures() {
        let engine = SearchEngine::new(vec![
            Arc::new(Mock::ok("a", vec![result("1", "a", Some("AA"), Some(true))])),
            Arc::new(Mock::err("b", "down")),
            Arc::new(Mock::ok("c", vec![
                result("2", "c", Some("aa"), None),              // dup infohash (case)
                result("3", "c", Some("bb"), Some(false)),
            ])),
        ]);
        let resp = engine.search(&SearchQuery::text("x")).await;
        assert_eq!(resp.provider_errors.len(), 1);
        assert_eq!(resp.provider_errors[0].provider, "b");
        // "aa" dup: the lossless copy (provider a, ranked first) wins.
        let ihs: Vec<_> = resp.results.iter().filter_map(|r| r.infohash.clone()).collect();
        assert_eq!(ihs, ["AA", "bb"]);
        assert_eq!(resp.results[0].lossless, Some(true));
        assert_eq!(resp.results[1].lossless, Some(false));
    }

    #[tokio::test]
    async fn ranks_lossless_first_then_album_then_depth() {
        let mk = |id, fc, bd| SearchResult {
            file_count: fc,
            bit_depth: bd,
            lossless: Some(true),
            ..result(id, "a", Some(id), Some(true))
        };
        let engine = SearchEngine::new(vec![Arc::new(Mock::ok("a", vec![
            SearchResult { lossless: Some(false), ..result("lossy", "a", Some("l0"), Some(false)) },
            mk("flac-8trk", Some(8), None),
            mk("flac-8trk-24", Some(8), Some(24)),
            mk("flac-2trk", Some(2), Some(24)),
            SearchResult { lossless: None, ..result("unknown", "a", Some("u"), None) },
        ]))]);
        let resp = engine.search(&SearchQuery::text("x")).await;
        let ids: Vec<_> = resp.results.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["flac-8trk-24", "flac-8trk", "flac-2trk", "unknown", "lossy"]);
    }

    #[tokio::test]
    async fn slow_provider_times_out_not_blocks() {
        let engine = SearchEngine::new(vec![
            Arc::new(Mock::slow("slow", Duration::from_secs(30))),
            Arc::new(Mock::ok("fast", vec![result("1", "fast", None, Some(true))])),
        ])
        .with_timeout(Duration::from_millis(50));
        let resp = engine.search(&SearchQuery::text("x")).await;
        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.provider_errors[0].provider, "slow");
        assert_eq!(resp.provider_errors[0].error, "timeout");
    }

    #[tokio::test]
    async fn provider_filter_scopes_fanout() {
        let engine = SearchEngine::new(vec![
            Arc::new(Mock::ok("a", vec![result("1", "a", None, None)])),
            Arc::new(Mock::ok("b", vec![result("2", "b", None, None)])),
        ]);
        let q = SearchQuery { providers: Some(vec!["b".into()]), ..SearchQuery::text("x") };
        let resp = engine.search(&q).await;
        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].provider, "b");
    }
}
