//! ArchiveOrgProvider — advancedsearch.php rows + /metadata/{id}
//! files[] classification. Default scope: mediatype:audio AND
//! collection:etree (taper-authorized live-music archive — the big
//! lossless catalog). resolve() → {id}_archive.torrent, magnet fallback.

use crate::lossless::{self, AudioClass};
use crate::{
    AddableTorrent, LegalTier, ProviderCaps, ProviderError, ResolvedTorrent, ResultFile,
    SearchQuery, SearchResult, TorrentProvider,
};
use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const BASE: &str = "https://archive.org";
/// IA's own trackers, baked into every _archive.torrent — reused when we
/// have to synthesize a magnet.
const IA_TRACKERS: &[&str] = &[
    "udp://bt1.archive.org:6699/announce",
    "udp://bt2.archive.org:6699/announce",
];

#[derive(Debug, Clone)]
struct IaFile {
    name: String,
    format: Option<String>,
    size: Option<u64>,
}

#[derive(Debug, Clone, Default)]
struct ItemMeta {
    files: Vec<IaFile>,
}

impl ItemMeta {
    fn from_json(v: &Value) -> Self {
        let files = v["files"]
            .as_array()
            .map(|fs| {
                fs.iter()
                    .filter_map(|f| {
                        Some(IaFile {
                            name: f["name"].as_str()?.to_string(),
                            format: f["format"].as_str().map(String::from),
                            size: f["size"]
                                .as_str()
                                .and_then(|s| s.parse().ok())
                                .or_else(|| f["size"].as_u64()),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { files }
    }

    fn torrent_name(&self, id: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|f| f.name == format!("{id}_archive.torrent"))
            .or_else(|| self.files.iter().find(|f| f.format.as_deref() == Some("Archive BitTorrent")))
            .map(|f| f.name.as_str())
    }
}

pub struct ArchiveOrgProvider {
    client: reqwest::Client,
    base: String,
    /// Lucene `collection:` scope — Some("etree") by default.
    collection: Option<String>,
    /// /metadata responses are reused by search rows and resolve().
    meta_cache: Mutex<HashMap<String, Arc<ItemMeta>>>,
}

impl Default for ArchiveOrgProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ArchiveOrgProvider {
    /// etree-scoped (Live Music Archive) provider.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base: BASE.into(),
            collection: Some("etree".into()),
            meta_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Whole-IA audio catalog (etree scope off) — still Clear-tier.
    pub fn all_collections() -> Self {
        Self { collection: None, ..Self::new() }
    }

    async fn meta(&self, id: &str) -> Result<Arc<ItemMeta>, ProviderError> {
        if let Some(m) = self.meta_cache.lock().unwrap().get(id) {
            return Ok(Arc::clone(m));
        }
        let v: Value = self
            .client
            .get(format!("{}/metadata/{id}", self.base))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let m = Arc::new(ItemMeta::from_json(&v));
        self.meta_cache.lock().unwrap().insert(id.into(), Arc::clone(&m));
        Ok(m)
    }

    /// Pure row builder — doc from advancedsearch, meta from /metadata
    /// (None when the fetch failed: row survives, unverifiable).
    fn result_from(doc: &Value, meta: Option<&ItemMeta>, q: &SearchQuery, base: &str) -> Option<SearchResult> {
        let id = strs(&doc["identifier"]).into_iter().next()?;
        let name = strs(&doc["title"]).into_iter().next().unwrap_or_else(|| id.clone());

        let mut classes: Vec<AudioClass> = Vec::new();
        let mut preview: Vec<ResultFile> = Vec::new();
        let mut others: Vec<ResultFile> = Vec::new();
        let mut torrent_name = None;
        let (mut formats, mut lossless, mut depth, mut rate) = (Vec::new(), None, None, None);
        let mut audio_count = 0u32;

        if let Some(m) = meta {
            torrent_name = m.torrent_name(&id).map(String::from);
            for f in &m.files {
                let class = f
                    .format
                    .as_deref()
                    .and_then(lossless::classify)
                    .or_else(|| lossless::classify(&f.name));
                match class {
                    Some(c) => {
                        audio_count += 1;
                        classes.push(c);
                        preview.push(ResultFile { path: f.name.clone(), size: f.size });
                    }
                    None => others.push(ResultFile { path: f.name.clone(), size: f.size }),
                }
            }
            let (f, l, d, r) = lossless::summarize(classes.iter());
            formats = f;
            lossless = l;
            depth = d;
            rate = r;
        }
        preview.extend(others);
        preview.truncate(10);

        // Format filter: a known file list without an acceptable codec is
        // rejected under strict, kept flagged otherwise. Unknown passes.
        let accepted = lossless::acceptable(q);
        if meta.is_some()
            && q.strict
            && !classes.iter().any(|c| accepted.contains(&c.codec))
        {
            return None;
        }

        Some(SearchResult {
            torrent_url: torrent_name
                .map(|t| format!("{base}/download/{id}/{t}")),
            source_page: Some(format!("{base}/details/{id}")),
            id,
            provider: "archive-org".into(),
            name,
            infohash: strs(&doc["btih"]).into_iter().next().map(|s| s.to_lowercase()),
            magnet: None,
            size_bytes: doc["item_size"].as_u64().or_else(|| {
                doc["item_size"].as_str().and_then(|s| s.parse().ok())
            }),
            file_count: meta.map(|_| audio_count),
            seeds: None,
            downloads: doc["downloads"].as_u64(),
            uploaded_at: None,
            license: strs(&doc["licenseurl"]).into_iter().next(),
            files_preview: preview,
            health: Default::default(),
            formats,
            lossless,
            bit_depth: depth,
            sample_rate: rate,
        })
    }
}

/// IA multi-valued fields serialize as either a scalar or an array.
fn strs(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Number(n) => vec![n.to_string()],
        Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str().map(String::from).or_else(|| x.as_u64().map(|n| n.to_string())))
            .collect(),
        _ => vec![],
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[async_trait]
impl TorrentProvider for ArchiveOrgProvider {
    fn id(&self) -> &'static str {
        "archive-org"
    }
    fn display_name(&self) -> &str {
        "Archive.org"
    }
    fn legal_tier(&self) -> LegalTier {
        LegalTier::Clear
    }
    fn capabilities(&self) -> ProviderCaps {
        ProviderCaps { seeds_known: false, needs_refresh: false, local_index: false }
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, ProviderError> {
        let text: String = q
            .text
            .chars()
            .filter(|c| c.is_alphanumeric() || " -_'.".contains(*c))
            .collect();
        let mut lucene = format!("({text}) AND mediatype:audio");
        if let Some(c) = &self.collection {
            lucene.push_str(&format!(" AND collection:{c}"));
        }
        let rows = q.limit.clamp(1, 50).to_string();
        let v: Value = self
            .client
            .get(format!("{}/advancedsearch.php", self.base))
            .query(&[
                ("q", lucene.as_str()),
                ("fl[]", "identifier"),
                ("fl[]", "title"),
                ("fl[]", "btih"),
                ("fl[]", "item_size"),
                ("fl[]", "downloads"),
                ("fl[]", "mediatype"),
                ("fl[]", "licenseurl"),
                ("fl[]", "collection"),
                ("rows", rows.as_str()),
                ("page", "1"),
                ("output", "json"),
                ("sort[]", "downloads desc"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let docs = v["response"]["docs"].as_array().cloned().unwrap_or_default();

        // Classify each row from its file list — the metadata call is
        // what makes rows verifiably lossless (and feeds files_preview).
        let rows = stream::iter(docs)
            .map(|doc| async move {
                let id = strs(&doc["identifier"]).into_iter().next().unwrap_or_default();
                let meta = self.meta(&id).await.ok();
                Self::result_from(&doc, meta.as_deref(), q, &self.base)
            })
            .buffer_unordered(8)
            .collect::<Vec<_>>()
            .await;
        Ok(rows.into_iter().flatten().collect())
    }

    async fn resolve(&self, r: &SearchResult) -> Result<ResolvedTorrent, ProviderError> {
        let meta = self.meta(&r.id).await?;
        let files = meta
            .files
            .iter()
            .map(|f| ResultFile { path: f.name.clone(), size: f.size })
            .collect();
        let addable = match meta.torrent_name(&r.id) {
            Some(t) => AddableTorrent::TorrentUrl(format!("{}/download/{}/{}", self.base, r.id, t)),
            None => {
                // Dark/restricted items lack _archive.torrent — magnet
                // fallback on btih + IA trackers.
                let ih = r
                    .infohash
                    .as_deref()
                    .ok_or_else(|| ProviderError::Unavailable(format!("{}: no torrent or btih", r.id)))?;
                let mut m = format!("magnet:?xt=urn:btih:{ih}&dn={}", urlencode(&r.name));
                for t in IA_TRACKERS {
                    m.push_str(&format!("&tr={}", urlencode(t)));
                }
                AddableTorrent::Magnet(m)
            }
        };
        Ok(ResolvedTorrent { result: r.clone(), files, addable })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded-response fixtures, shrunk: two advancedsearch docs and
    /// their /metadata files[] — one mixed FLAC+MP3 item, one MP3-only.
    const SEARCH_JSON: &str = r#"{"response":{"numFound":2,"docs":[
        {"identifier":"gd1977-05-08.sbd.flac16","title":"Grateful Dead Live at Cornell","btih":["ABC123DEF4567890ABC123DEF4567890ABC12345"],"item_size":168207899,"downloads":41234,"mediatype":"audio","collection":["etree","GratefulDead"],"licenseurl":"https://creativecommons.org/licenses/by-nc-nd/3.0/"},
        {"identifier":"pop-upload-mp3","title":"Some MP3-Only Upload","btih":"DEF4567890ABC123DEF4567890ABC12345ABC123","item_size":50201,"downloads":12,"mediatype":"audio"}
    ]}}"#;

    const META_FLAC: &str = r#"{"metadata":{"identifier":"gd1977-05-08.sbd.flac16"},"files":[
        {"name":"gd1977-05-08d1t01.flac","format":"Flac","size":"23456789"},
        {"name":"gd1977-05-08d1t02.flac","format":"Flac","size":"31234567"},
        {"name":"gd1977-05-08d1t01.mp3","format":"VBR MP3","size":"5234567"},
        {"name":"gd1977-05-08.txt","format":"Text","size":"1234"},
        {"name":"gd1977-05-08.sbd.flac16_archive.torrent","format":"Archive BitTorrent","size":"23456"}
    ]}"#;

    const META_MP3: &str = r#"{"metadata":{"identifier":"pop-upload-mp3"},"files":[
        {"name":"track1.mp3","format":"VBR MP3","size":"9234567"},
        {"name":"track2.ogg","format":"Ogg Vorbis","size":"7234567"},
        {"name":"pop-upload-mp3_archive.torrent","format":"Archive BitTorrent","size":"12345"}
    ]}"#;

    const META_24BIT: &str = r#"{"files":[
        {"name":"show-24bit.flac","format":"24bit Flac","size":"93456789"},
        {"name":"show_archive.torrent","format":"Archive BitTorrent","size":"999"}
    ]}"#;

    fn doc(i: usize) -> Value {
        serde_json::from_str::<Value>(SEARCH_JSON).unwrap()["response"]["docs"][i].clone()
    }

    #[test]
    fn rows_classify_lossless_vs_lossy() {
        let q = SearchQuery::text("dead");
        let flac_meta = ItemMeta::from_json(&serde_json::from_str(META_FLAC).unwrap());
        let mp3_meta = ItemMeta::from_json(&serde_json::from_str(META_MP3).unwrap());

        let r = ArchiveOrgProvider::result_from(&doc(0), Some(&flac_meta), &q, BASE).unwrap();
        assert_eq!(r.lossless, Some(true));
        assert_eq!(r.formats, ["flac", "mp3"]);
        assert_eq!(r.file_count, Some(3));
        assert_eq!(r.infohash.as_deref(), Some("abc123def4567890abc123def4567890abc12345"));
        assert_eq!(
            r.torrent_url.as_deref(),
            Some("https://archive.org/download/gd1977-05-08.sbd.flac16/gd1977-05-08.sbd.flac16_archive.torrent")
        );
        assert!(r.files_preview.iter().any(|f| f.path.ends_with(".flac")));

        // MP3/OGG-only item: rejected under strict…
        assert!(ArchiveOrgProvider::result_from(&doc(1), Some(&mp3_meta), &q, BASE).is_none());
        // …kept flagged when strict=false.
        let loose = SearchQuery { strict: false, ..SearchQuery::text("dead") };
        let r2 = ArchiveOrgProvider::result_from(&doc(1), Some(&mp3_meta), &loose, BASE).unwrap();
        assert_eq!(r2.lossless, Some(false));
        assert_eq!(r2.formats, ["mp3", "ogg"]);
    }

    #[test]
    fn bit_depth_from_format_label() {
        let q = SearchQuery::text("x");
        let m = ItemMeta::from_json(&serde_json::from_str(META_24BIT).unwrap());
        let d = serde_json::json!({"identifier":"show","title":"S"});
        let r = ArchiveOrgProvider::result_from(&d, Some(&m), &q, BASE).unwrap();
        assert_eq!(r.bit_depth, Some(24));
        assert_eq!(r.lossless, Some(true));
    }

    #[test]
    fn missing_metadata_survives_as_unknown() {
        let q = SearchQuery::text("dead");
        let r = ArchiveOrgProvider::result_from(&doc(0), None, &q, BASE).unwrap();
        assert_eq!(r.lossless, None);
        assert_eq!(r.torrent_url, None);
        assert_eq!(r.file_count, None);
    }

    #[test]
    fn magnet_fallback_when_no_archive_torrent() {
        let m = ItemMeta::from_json(&serde_json::json!({"files":[
            {"name":"a.flac","format":"Flac","size":"100"}
        ]}));
        assert!(m.torrent_name("x").is_none());
    }
}
