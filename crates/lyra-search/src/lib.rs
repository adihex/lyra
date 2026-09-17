//! lyra-search: torrent search over legal/public-domain indexes —
//! archive.org (etree live-music default scope) and Academic Torrents.
//! Lossless-first: providers share `lossless::classify`, queries filter
//! by codec family, results carry codec/bit-depth fields, and the merge
//! ranks lossless above lossy. Design: docs/research/torrent-search-design.md.

mod academic_torrents;
mod apibay;
mod archive_org;
mod engine;
mod torznab;
mod x1337x;
pub mod cover_art;
pub mod lossless;

pub use academic_torrents::AcademicTorrentsProvider;
pub use apibay::ApibayProvider;
pub use archive_org::ArchiveOrgProvider;
pub use cover_art::CoverArtClient;
pub use engine::SearchEngine;
pub use torznab::TorznabProvider;
pub use x1337x::X1337Provider;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("parse: {0}")]
    Parse(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("unavailable: {0}")]
    Unavailable(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegalTier {
    /// Public-domain / authorized catalogs (IA etree, Academic Torrents).
    Clear,
    /// Mixed-legality index — usable, ranked below Clear.
    Gray,
    /// User-configured endpoint (torznab etc.); never shipped default.
    External,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ProviderCaps {
    /// Provider publishes seed counts (P0 providers don't).
    pub seeds_known: bool,
    /// Provider needs a periodic index refresh.
    pub needs_refresh: bool,
    /// Searches a local snapshot, not a remote API.
    pub local_index: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    /// Provider ids to hit; None/empty = all.
    #[serde(default)]
    pub providers: Option<Vec<String>>,
    /// Acceptable codec ids; empty = lossless::FAMILY.
    #[serde(default)]
    pub formats: Vec<String>,
    /// Drop results verified to lack an acceptable format (default).
    /// false = keep them, flagged via `lossless: false` + `formats`.
    #[serde(default = "default_true")]
    pub strict: bool,
    /// Per-provider row cap (hard-capped at 50).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_true() -> bool {
    true
}
fn default_limit() -> usize {
    50
}

impl SearchQuery {
    pub fn text(t: impl Into<String>) -> Self {
        Self {
            text: t.into(),
            providers: None,
            formats: Vec::new(),
            strict: true,
            limit: default_limit(),
        }
    }

    /// Accepts a SearchQuery object or a bare JSON string (→ text).
    pub fn from_json(s: &str) -> Option<Self> {
        serde_json::from_str::<SearchQuery>(s)
            .ok()
            .or_else(|| serde_json::from_str::<String>(s).ok().map(Self::text))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HealthHint {
    #[default]
    Unknown,
    /// Lower-bound swarm signal from a DHT get_peers probe — "peers
    /// seen", not seeds.
    Probed { peers_seen: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultFile {
    pub path: String,
    #[serde(default)]
    pub size: Option<u64>,
}

/// One provider row. `Option` fields stay null/absent — never faked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Provider-scoped stable id (IA identifier, AT infohash).
    pub id: String,
    pub provider: String,
    pub name: String,
    #[serde(default)]
    pub infohash: Option<String>,
    #[serde(default)]
    pub magnet: Option<String>,
    #[serde(default)]
    pub torrent_url: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// Audio-file count where known — the "complete album" proxy.
    #[serde(default)]
    pub file_count: Option<u32>,
    /// None = unknown; never faked to 0.
    #[serde(default)]
    pub seeds: Option<u32>,
    /// Popularity signal (IA downloads).
    #[serde(default)]
    pub downloads: Option<u64>,
    #[serde(default)]
    pub uploaded_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub source_page: Option<String>,
    /// Top ~10 files, audio first.
    #[serde(default)]
    pub files_preview: Vec<ResultFile>,
    #[serde(default)]
    pub health: HealthHint,
    /// Audio codec ids present, deduped ("flac","mp3",…).
    #[serde(default)]
    pub formats: Vec<String>,
    /// Some(true) verified lossless, Some(false) verified lossy-only,
    /// None = file list unknown.
    #[serde(default)]
    pub lossless: Option<bool>,
    #[serde(default)]
    pub bit_depth: Option<u32>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
}

/// What resolve() hands to TorrentEngine — TorrentUrl preferred (real
/// trackers + exact file tree), Magnet synthesized only as fallback.
#[derive(Debug, Clone)]
pub enum AddableTorrent {
    Magnet(String),
    TorrentUrl(String),
    TorrentBytes(Vec<u8>),
}

impl Serialize for AddableTorrent {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(Some(2))?;
        match self {
            Self::Magnet(u) => {
                m.serialize_entry("kind", "magnet")?;
                m.serialize_entry("magnet", u)?;
            }
            Self::TorrentUrl(u) => {
                m.serialize_entry("kind", "torrent_url")?;
                m.serialize_entry("url", u)?;
            }
            Self::TorrentBytes(b) => {
                use base64::Engine;
                m.serialize_entry("kind", "torrent_b64")?;
                m.serialize_entry("data", &base64::engine::general_purpose::STANDARD.encode(b))?;
            }
        }
        m.end()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedTorrent {
    pub result: SearchResult,
    /// Full file list (cheap rows keep files_preview only).
    pub files: Vec<ResultFile>,
    pub addable: AddableTorrent,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderIssue {
    pub provider: String,
    pub error: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub provider_errors: Vec<ProviderIssue>,
}

#[async_trait]
pub trait TorrentProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &str;
    fn legal_tier(&self) -> LegalTier;
    fn capabilities(&self) -> ProviderCaps;

    /// Cheap rows: file lists stay preview-sized, seeds unknown.
    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, ProviderError>;

    /// Full resolution on demand: file list + AddableTorrent.
    async fn resolve(&self, r: &SearchResult) -> Result<ResolvedTorrent, ProviderError>;
}
