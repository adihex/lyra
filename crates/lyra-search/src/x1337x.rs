//! X1337Provider — 1337x HTML scrape. No official API; the Music
//! category search page (`/category-search/{q}/Music/1/`) is a stable
//! `<tr class="…">` table with seeds/leech/size columns, and each
//! torrent page carries the magnet href. LegalTier::Gray — mixed-
//! legality index, ranked below Clear providers at equal quality.
//!
//! Rows get codec hints from the *title* ("[FLAC]", "24bit") — the file
//! list isn't in search markup, so lossless stays a hint, not verified.
//! resolve() scrapes the torrent page for its magnet (infohash inside).

use crate::lossless;
use crate::{
    AddableTorrent, LegalTier, ProviderCaps, ProviderError, ResolvedTorrent,
    SearchQuery, SearchResult, TorrentProvider,
};
use async_trait::async_trait;

const BASE: &str = "https://1337x.to";
/// Browser-shaped UA — 1337x serves a bare 403 to reqwest's default.
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                  AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Safari/537.36";

pub struct X1337Provider {
    client: reqwest::Client,
    base: String,
}

impl Default for X1337Provider {
    fn default() -> Self {
        Self::new()
    }
}

impl X1337Provider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent(UA)
                .build()
                .unwrap_or_default(),
            base: BASE.into(),
        }
    }

    /// Alternate mirror / test server.
    pub fn with_base(base: impl Into<String>) -> Self {
        Self { base: base.into(), ..Self::new() }
    }

    /// One search-table row → SearchResult. Pure — fixtures test this
    /// without a network. Returns None for malformed/non-data rows.
    fn row_from(tr: &str, q: &SearchQuery, base: &str) -> Option<SearchResult> {
        // The name anchor is the LAST /torrent/{id}/{slug}/ link in the
        // row (the first is the category icon with empty text).
        let mut id_slug = None;
        let mut name = None;
        let mut rest = tr;
        while let Some(i) = rest.find("href=\"/torrent/") {
            let after = &rest[i + 15..];
            let end = after.find('"')?;
            let path = &after[..end]; // "{id}/{slug}/"
            if let Some(a) = after[end..].find('>').map(|j| &after[end + j + 1..]) {
                if let Some(close) = a.find("</a>") {
                    let text = unescape(strip_tags(&a[..close]).trim());
                    if !text.is_empty() {
                        name = Some(text);
                    }
                }
            }
            id_slug = Some(path.to_string());
            rest = &after[end..];
        }
        let id_slug = id_slug?;
        let name = name?;
        let id = id_slug.split('/').next()?.to_string();

        let seeds = col_num(tr, "coll-2").map(|n| n as u32);
        let size_bytes = col_text(tr, "coll-4").and_then(|s| parse_size(&s));

        // Codec hint from the title — "[FLAC]", "24bit", "MP3 320".
        let class = lossless::classify_title(&name);
        let (formats, lossless, depth, rate) = match class.as_ref() {
            Some(c) => (
                vec![c.codec.to_string()],
                Some(c.lossless),
                c.bit_depth,
                c.sample_rate,
            ),
            None => (Vec::new(), None, None, None),
        };
        // Strict drops rows the title *proves* are an unwanted codec;
        // unclassified rows stay (unknown ≠ lossy).
        if q.strict {
            if let Some(c) = &class {
                if !lossless::acceptable(q).contains(&c.codec) {
                    return None;
                }
            }
        }

        Some(SearchResult {
            id,
            provider: "x1337".into(),
            name,
            infohash: None,
            magnet: None,
            torrent_url: None,
            size_bytes,
            file_count: None,
            seeds,
            downloads: None,
            uploaded_at: None,
            license: None,
            source_page: Some(format!("{base}/torrent/{id_slug}")),
            files_preview: vec![],
            health: Default::default(),
            formats,
            lossless,
            bit_depth: depth,
            sample_rate: rate,
        })
    }

    /// First magnet href in a torrent page.
    fn magnet_from(page: &str) -> Option<String> {
        let i = page.find("href=\"magnet:")?;
        let after = &page[i + 6..];
        let end = after.find('"')?;
        Some(unescape(&after[..end]))
    }

    /// Infohash out of a magnet's xt=urn:btih: param.
    fn infohash_of(magnet: &str) -> Option<String> {
        let i = magnet.find("xt=urn:btih:")?;
        let h = &magnet[i + 12..];
        let end = h.find('&').unwrap_or(h.len());
        Some(h[..end].to_lowercase())
    }
}

/// Text of `<td class="… col …">` up to the first nested tag — 1337x
/// size cells wrap a `<span>` with the seed count inside the text.
fn col_text(tr: &str, col: &str) -> Option<String> {
    let i = tr.find(col)?;
    let after = &tr[i..];
    let open = after.find('>')?;
    let inner = &after[open + 1..];
    let end = inner.find('<')?;
    let t = inner[..end].trim();
    (!t.is_empty()).then(|| unescape(t))
}

fn col_num(tr: &str, col: &str) -> Option<u64> {
    col_text(tr, col)?.replace(',', "").parse().ok()
}

/// "1.4 GB" → bytes. 1337x renders B/KB/MB/GB/TB with one decimal.
fn parse_size(s: &str) -> Option<u64> {
    let mut it = s.split_whitespace();
    let n: f64 = it.next()?.parse().ok()?;
    let mult = match it.next()?.to_uppercase().as_str() {
        "B" => 1.0,
        "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return None,
    };
    Some((n * mult) as u64)
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// The handful of entities 1337x emits in titles and magnet hrefs.
fn unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

#[async_trait]
impl TorrentProvider for X1337Provider {
    fn id(&self) -> &'static str {
        "x1337"
    }
    fn display_name(&self) -> &str {
        "1337x"
    }
    fn legal_tier(&self) -> LegalTier {
        LegalTier::Gray
    }
    fn capabilities(&self) -> ProviderCaps {
        ProviderCaps { seeds_known: true, needs_refresh: false, local_index: false }
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, ProviderError> {
        let url = format!(
            "{}/category-search/{}/Music/1/",
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
        let cap = q.limit.clamp(1, 50);
        Ok(body
            .split("<tr")
            .skip(1) // before the first <tr> is the table head
            .filter_map(|chunk| Self::row_from(chunk, q, &self.base))
            .take(cap)
            .collect())
    }

    async fn resolve(&self, r: &SearchResult) -> Result<ResolvedTorrent, ProviderError> {
        let page_url = r
            .source_page
            .clone()
            .unwrap_or_else(|| format!("{}/torrent/{}/", self.base, r.id));
        let body = self
            .client
            .get(&page_url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let magnet = Self::magnet_from(&body)
            .ok_or_else(|| ProviderError::Unavailable(format!("{}: no magnet", r.id)))?;
        let mut result = r.clone();
        if result.infohash.is_none() {
            result.infohash = Self::infohash_of(&magnet);
        }
        Ok(ResolvedTorrent {
            result,
            files: vec![],
            addable: AddableTorrent::Magnet(magnet),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded-shape fixture: 1337x Music search rows (coll-1 name with
    /// icon anchor + name anchor, coll-2 seeds, coll-3 leech, coll-4
    /// size-with-nested-span).
    const ROWS: &str = r#"
<table class="table-list"><tbody>
<tr>
<td class="coll-1 name"><a class="icon" href="/sub/58/0/"><i class="flaticon-music"></i></a><a href="/torrent/6825214/Daft-Punk-Random-Access-Memories-2013-FLAC/">Daft Punk - Random Access Memories (2013) [FLAC]</a></td>
<td class="coll-2 seeds">1,204</td>
<td class="coll-3 leeches">57</td>
<td class="coll-4 mob-vp size">370.5 MB<span class="seeds">1204</span></td>
<td class="coll-5"><a href="/users/xyz">xyz</a></td>
</tr>
<tr>
<td class="coll-1 name"><a class="icon" href="/sub/58/0/"><i class="flaticon-music"></i></a><a href="/torrent/6825215/Daft-Punk-RAM-MP3-320/">Daft Punk - RAM MP3 320kbps</a></td>
<td class="coll-2 seeds">88</td>
<td class="coll-3 leeches">4</td>
<td class="coll-4 mob-vp size">96.2 MB<span class="seeds">88</span></td>
<td class="coll-5"><a href="/users/abc">abc</a></td>
</tr>
<tr><td class="coll-1 name"><a class="icon" href="/sub/1/0/"></a></td></tr>
</tbody></table>"#;

    const TORRENT_PAGE: &str = r#"
<div class="torrent-detail-page">
<ul class="download-links">
<li><a href="magnet:?xt=urn:btih:ABC123DEF4567890ABC123DEF4567890ABC12345&amp;dn=Daft+Punk+RAM&amp;tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce">Magnet Download</a></li>
</ul></div>"#;

    fn q() -> SearchQuery {
        SearchQuery { strict: false, ..SearchQuery::text("daft punk") }
    }

    #[test]
    fn rows_parse_name_seeds_size() {
        let rows: Vec<_> = ROWS
            .split("<tr")
            .skip(1)
            .filter_map(|c| X1337Provider::row_from(c, &q(), BASE))
            .collect();
        // Row 3 is icon-only (no name anchor) → dropped.
        assert_eq!(rows.len(), 2);
        let r = &rows[0];
        assert_eq!(r.id, "6825214");
        assert_eq!(r.provider, "x1337");
        assert_eq!(r.name, "Daft Punk - Random Access Memories (2013) [FLAC]");
        assert_eq!(r.seeds, Some(1204));
        assert_eq!(r.size_bytes, Some(370_500_000));
        assert_eq!(r.lossless, Some(true));
        assert_eq!(r.formats, ["flac"]);
        assert_eq!(
            r.source_page.as_deref(),
            Some("https://1337x.to/torrent/6825214/Daft-Punk-Random-Access-Memories-2013-FLAC/")
        );
    }

    #[test]
    fn strict_drops_verified_lossy() {
        let strict = SearchQuery::text("daft punk"); // strict: true
        let rows: Vec<_> = ROWS
            .split("<tr")
            .skip(1)
            .filter_map(|c| X1337Provider::row_from(c, &strict, BASE))
            .collect();
        // MP3 row proves lossy from the title → strict drops it; the
        // icon-only row never parses. Only the FLAC row survives.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].formats, ["flac"]);
        let loose = q();
        let rows: Vec<_> = ROWS
            .split("<tr")
            .skip(1)
            .filter_map(|c| X1337Provider::row_from(c, &loose, BASE))
            .collect();
        assert!(rows.iter().any(|r| r.formats == ["mp3"]));
    }

    #[test]
    fn magnet_scrapes_and_infohash_extracts() {
        let m = X1337Provider::magnet_from(TORRENT_PAGE).unwrap();
        assert!(m.starts_with("magnet:?xt=urn:btih:ABC123"));
        assert!(m.contains("&tr=udp")); // &amp; decoded
        assert_eq!(
            X1337Provider::infohash_of(&m).as_deref(),
            Some("abc123def4567890abc123def4567890abc12345")
        );
    }

    #[test]
    fn size_variants() {
        assert_eq!(parse_size("1.4 GB"), Some(1_400_000_000));
        assert_eq!(parse_size("96.2 MB"), Some(96_200_000));
        assert_eq!(parse_size("512 KB"), Some(512_000));
        assert_eq!(parse_size("nope"), None);
    }
}
