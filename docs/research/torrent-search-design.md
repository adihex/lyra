# Lyra — Torrent Search + Tracker Management Design

Date: 2026-09-16. Verified against `librqbit 8.1.1` (tag `v8.1.1`, repo `github.com/ikatson/rqbit`
— note the repo moved; `rqbit/rqbit` 404s) with 9.0.1 deltas called out. All external endpoints
below were probed live.

Lyra's missing pieces are **search** (finding torrents) and **tracker management** (improving peer
discovery). They are separate concerns sharing one result type. This document covers the rqbit API
surface available to build on, prior-art search architectures, reusable crates, legal posture, and a
recommended phased design.

---

## 1. rqbit API surface (what we actually have to work with)

Source: `crates/librqbit/src/session.rs`, `torrent_state/{mod,stats}.rs`, `crates/dht/src/dht.rs`,
`crates/tracker_comms/`, all @ v8.1.1.

### 1.1 DHT

| Capability | Status in librqbit 8.x | Detail |
|---|---|---|
| Session-wide DHT toggle | ✅ `SessionOptions::disable_dht: bool` | Construction-time only — toggling requires `Session` rebuild |
| DHT persistence | ✅ `disable_dht_persistence`, `dht_config: Option<PersistentDhtConfig>` | Persists routing table + port across restarts |
| Access to DHT handle | ✅ `Session::get_dht() -> Option<&Dht>` | `librqbit_dht::Dht` |
| `get_peers` (BEP5) | ✅ `Dht::get_peers(info_hash, announce_port) -> RequestPeersStream` | Stream of discovered peer addrs — **usable as an optional swarm-probe to estimate health of a search result** |
| DHT stats | ✅ `Dht::stats() -> DhtStats { id, outstanding_requests, routing_table_size }`; `Api::api_dht_stats()`, `api_dht_table()`, `clone_routing_table()` | Enough for a "DHT: N nodes" status chip |
| Custom bootstrap nodes | ✅ `DhtConfig::bootstrap_addrs` | |
| **`sample_infohashes` (BEP51)** | ❌ **Not implemented** | No `sample_*` anywhere in `crates/dht`. librqbit-dht is a client-side lookup impl, not a crawler. **DHT sweeping is impossible without a second DHT impl or custom KRPC.** |

### 1.2 Metadata-only fetch (magnet → info dict, BEP9)

- Public path (8.x and 9.x): `Session::add_torrent(magnet, AddTorrentOptions { list_only: true, .. })`
  → `AddTorrentResponse::ListOnly(ListOnlyResponse { info_hash, info, only_files, output_folder,
  seen_peers, torrent_bytes })`. Resolves via DHT + trackers + `initial_peers`, then BEP9
  `ut_metadata`. `torrent_bytes` is a synthesized valid .torrent.
- `Session::resolve_magnet(...)` exists but is **private** (`async fn`, session.rs:1420 @8.1.1;
  still private @9.0.1). HTTP `POST /torrents/resolve_magnet` exists in the 9.x router only — not
  reachable from the library API in either version.
- **Consequence**: to preview files for a search result we don't need `resolve_magnet` at all — our
  providers hand us real `.torrent` URLs; parse with `torrent_from_bytes`. For magnet-only results,
  `list_only` is the fetch path.

### 1.3 Trackers

| Capability | Status | Detail |
|---|---|---|
| Per-add custom trackers | ✅ `AddTorrentOptions::trackers: Option<Vec<String>>` | Merged with the magnet's `tr=` params + .torrent announce-list + session trackers |
| Session-wide tracker injection | ✅ `SessionOptions::trackers: HashSet<Url>` | Doc comment: "tracker URLs to always use for each torrent" — **the clean hook for ngosang-style boost** |
| `disable_trackers`, `force_tracker_interval` per add | ✅ | |
| `initial_peers` per add + `POST /torrents/{id}/add_peers` | ✅ | Peer injection (different from trackers) |
| Magnet `tr=` parsing | ✅ `Magnet { trackers: Vec<String>, name, .. }` | |
| HTTP + UDP announce, BEP12 announce-list | ✅ `tracker_comms` crate | Private torrents → first tracker only (`is_private` branch) |
| **Add/remove trackers on an existing torrent** | ❌ | `ManagedTorrentShared.trackers: HashSet<Url>` is reachable read-only via `handle.shared()`; no `add_trackers`/`remove_tracker` in `Api` or HTTP routes (9.x either). **Trackers are fixed at add-time.** Workaround: delete + re-add into the same output folder (`overwrite` resumes existing files). |
| **Per-tracker status/scrape stats** | ❌ | `tracker_comms` announces internally; nothing per-tracker is surfaced in `TorrentStats` |

### 1.4 Stats / health signals

- `ManagedTorrent::stats() -> TorrentStats { state, file_progress, error, progress_bytes,
  uploaded_bytes, total_bytes, finished, live: Option<LiveStats> }`
- `LiveStats.snapshot: StatsSnapshot` → `AggregatePeerStats { queued, connecting, live, seen, dead,
  not_needed, steals }` — `seen`/`live` give a real swarm-contact signal for the UI.
- `Api::api_peer_stats` for per-peer detail. No seed/leech counts per tracker anywhere.

### 1.5 Notable gap

**No BEP19 webseeds** (open issue ikatson/rqbit#500). archive.org and academictorrents torrents both
embed GetRight-style webseeds; rqbit ignores them and relies on tracker+DHT peers only. For
long-tail IA items the swarm can be thin — mitigations in §5 (extra trackers + IA direct-HTTP
fallback for files).

---

## 2. Prior-art search architectures

| Approach | Exemplars | Integration weight | Maintenance / legal risk | Result quality | Verdict for Lyra |
|---|---|---|---|---|---|
| **App-bundled scraper plugins** | qBittorrent search: Python engines (`nova2`/`nova2dl` API, `search(what, cat)` prints pipe-delimited rows, `download_torrent(url)`), engines dir `nova3/engines`, official repo `qbittorrent/search-plugins` + a larger "unofficial plugins" list | High in our context: embeds a Python runtime in a macOS Rust app; a Rust port = hand-writing per-site scrapers | Constant breakage as sites redesign/change domains (the official engine set has repeatedly shrunk); unofficial lists are mostly pirate indexers — shipping them in-app is the risky part | Best available: real torrent sites with seeds/leechers/category metadata | **Copy the pluggable-provider pattern, not the mechanism.** No Python, no bundled pirate scrapers |
| **Torznab proxy sidecar** | Jackett (.NET service; `GET /api/v2.0/indexers/{idx}/results/torznab?apikey=&q=` → XML w/ magnet, seeds, leechers; ~500 indexer defs), Prowlarr (same torznab + Cardigann YAML defs, syncs into *arr apps) | Lyra-side cost is tiny — torznab is XML-over-HTTP, one provider impl. But it requires the *user* to run Jackett/Prowlarr | Near-zero legal exposure **for us** — the indexers live in the user's own infra; Jackett/Prowlarr absorb the per-site maintenance | Excellent (this is what the *arr ecosystem standardizes on) | **P1 provider type**: "Torznab endpoint (URL + API key)". Don't bundle Jackett itself |
| **Self-crawl DHT index** | bitmagnet (Go, MIT): runs as DHT node, harvests infohashes, BEP9 metadata fetch, content classifier, Postgres + GraphQL + Servarr API | Highest: crawler + store + index + classifier is a whole subsystem | Grayest option — actively indexes arbitrary swarm content and announces presence; a permanent crawl job inside a music player is disproportionate | Everything, including junk; seed counts unreliable; needs days of warm-up | **P2 only**, heavily descoped (see §3 passive sampling), or never |
| **Standalone DHT crawlers** | magnetico (Python, AGPL, unmaintained since 2022), dhtcrawler2, many Go/Rust BEP51 spiders | Medium-high: needs KRPC + routing table + metadata fetch pipeline | Same gray area as bitmagnet, minus the polish | Raw infohash stream — no names/metadata until you BEP9-fetch each one | Not worth building for a music player; note that **no Rust crate implements BEP51** anyway (see §3) |
| **Webtorrent ecosystem** | `bittorrent-dht`, `webtorrent-tracker` (WebSocket trackers for browser peers), `magnet-uri`, `parse-torrent` | N/A as code (JS), useful as precedent | The ecosystem has **no search layer** — apps delegate discovery to external/provider plugins (cf. Stremio + Torrentio addon) | Depends entirely on the external source | Confirms the pattern: **media players don't own the index; they plug into providers** |
| **Pure download engines** | aria2 (RPC, own DHT, no search) | N/A | — | — | Confirms search is always a separate layer; nothing to reuse |
| **Hosted torrent-search APIs** | apibay (TPB), bitsearch, snowfl, etc. | Trivial to add behind the provider trait | Index is pirate-leaning → keep out of the default set | Good | Possible P1 user-added providers; not shipped |

---

## 3. Reusable Rust crates

| Need | Crate | Verdict |
|---|---|---|
| magnet → metadata | — | **Reuse librqbit itself**: `add_torrent` + `list_only` (§1.2). Do not write a BEP9 fetcher. |
| magnet parsing | `librqbit::Magnet` (already parses `xt`, `dn`, `tr=`, `x.pe`) | Reuse; `magnet_url` crate is redundant |
| .torrent parse | `librqbit::torrent_from_bytes` / `TorrentMetaV1` | Reuse; `lava-torrent`/`torrex`/`bendy` unnecessary for our path |
| DHT get_peers probe | `session.get_dht().get_peers(ih, port)` | Reuse the session DHT — no extra crate |
| DHT crawling (P2 only) | `mainline` v8.0.0 (Pubky): client/server modes, `get_peers`, `announce_peer`, BEP44, `get_closest_nodes`, **`request_filter` hook that observes all incoming queries** | The only maintained Rust DHT worth taking. **No BEP51** (supports BEP5/42/43/44, IPv4-only) — but passive sampling doesn't need it: a server-mode node sees `get_peers`/`announce_peer` infohashes in its request filter, which is exactly how magnetico harvested most of its index. `librqbit-dht` can't do this (client-only, no inbound-query hook). |
| Tracker scrape (per-tracker seed counts) | — | No usable crate. ~150 LOC of BEP15 `scrape` over UDP if ever wanted; recommend skipping — DHT probe covers the "is this alive?" question |
| HTTP + JSON | `reqwest`, `serde` | Already in rqbit's dep tree |
| RSS/XML (AT database, torznab) | `quick-xml` or `roxmltree` (read-only, lighter) | New small dep, fine |
| Site HTML scraping (P1 etree etc.) | `scraper` | Write per-provider; accept fragility; nothing worth vendoring |
| Local search over AT catalog | `rusqlite` FTS5, or plain token-match over the XML | AT catalog is small (thousands of items); even naive matching is fine to start |

---

## 4. Legal posture

Lyra is personal-use; the default search surface must stay on legal/public-domain indexes.

**Verified P0 sources:**

- **archive.org** — `GET https://archive.org/advancedsearch.php` is live and returns JSON:
  `?q=<Lucene>&fl[]=identifier&fl[]=title&fl[]=btih&fl[]=item_size&fl[]=downloads&fl[]=mediatype
  &fl[]=collection&fl[]=licenseurl&rows=50&page=N&output=json&sort[]=downloads desc`.
  - `btih` (infohash) is available as a `fl[]` field on items that carry it → direct magnet.
  - Torrent file convention verified: `GET /metadata/{id}` → `files[]` contains
    `{id}_archive.torrent` → download at `https://archive.org/download/{id}/{id}_archive.torrent`.
    (Edge cases: dark/restricted items lack it — `resolve()` must check `files[]`.)
  - IA runs its own trackers (`bt1/bt2.archive.org:6699`) baked into those torrents.
  - `mediatype:audio` + `collection:etree` is a *music-player-perfect* default scope — etree is the
    taper-authorized live-music archive (Grateful Dead et al., noncommercial trading explicitly
    permitted). `licenseurl` lets us label/filter CC & PD content.
  - Honest caveat: IA hosts mixed-legality user uploads; the provider queries the whole catalog but
    we should rank/filter toward `mediatype:audio` + collection/license filters rather than claim IA
    is uniformly "legal". Framing is "open media index", not a rights guarantee.
- **academictorrents.com** — research datasets/papers/courses (a 501(c)3 nonprofit project).
  - **No server-side search API** — explicitly documented: robots are expected to download the
    catalog and search locally. `GET https://academictorrents.com/database.xml` (verified live,
    nightly-updated RSS): each `<item>` has `title`, `category`, `infohash`, `guid`/details link,
    `description`, `size`. `rss.xml` = recent-entries feed for incremental refresh.
  - `GET https://academictorrents.com/apiv2/entry/{infohash}` → JSON (bibtex-style metadata;
    verified live). `POST` variants exist for upload/modify — unused.
  - `.torrent` at `https://academictorrents.com/download/{infohash}` → `application/x-bittorrent`
    (verified).
  - Design consequence: this provider = **local snapshot + local search** (refresh daily/if-stale,
    parse once into a small in-memory or sqlite index), not a per-keystroke remote call. That also
    makes it the lowest-latency provider.

Everything else (torznab user endpoints, site scrapers like bt.etree.org — legal live-music tracker,
currently 403s non-browser agents so it'd need a real UA and is P1 at best) goes behind the provider
trait as **user-enabled, non-default** providers. We do not ship pirate-index adapters in-tree.

---

## 5. Recommended architecture

### 5.1 Where it lives

**New crate `crates/lyra-search`** (recommended over a `lyra-torrent` submodule):

- Search is HTTP/JSON/XML work with zero BitTorrent dependency except at the handoff boundary —
  keeping it out of `lyra-torrent` keeps the engine crate small, lets search compile and test
  standalone, and avoids pulling provider deps into the torrent path.
- The only shared surface is a result type + `AddableTorrent`; if the workspace prefers fewer
  crates, `lyra_torrent::search` is an acceptable fallback — the trait design is identical.

### 5.2 Core types

```rust
#[async_trait]
pub trait TorrentProvider: Send + Sync {
    fn id(&self) -> &'static str;                    // "archive-org", "academic-torrents"
    fn display_name(&self) -> &str;
    fn legal_tier(&self) -> LegalTier;               // Clear | Gray | External(user-configured)
    fn capabilities(&self) -> ProviderCaps;          // {seeds_known, needs_refresh, local_index}

    async fn search(&self, q: &SearchQuery)
        -> Result<Vec<SearchResult>, ProviderError>;

    /// Cheap rows in; full resolution on demand (file list, exact sizes, magnet).
    async fn resolve(&self, r: &SearchResult)
        -> Result<ResolvedTorrent, ProviderError>;
}

pub struct SearchResult {
    pub id: String,                       // provider-scoped stable id
    pub provider: String,
    pub name: String,
    pub infohash: Option<String>,         // hex btih when known upfront
    pub magnet: Option<String>,
    pub torrent_url: Option<String>,
    pub size_bytes: Option<u64>,
    pub file_count: Option<u32>,
    pub seeds: Option<u32>,               // None = unknown — never fake 0
    pub uploaded_at: Option<DateTime<Utc>>,
    pub license: Option<String>,          // IA licenseurl / AT bibtex license
    pub source_page: Option<String>,
    pub files_preview: Vec<ResultFile>,   // top ~10 {path, size}
    pub health: HealthHint,               // Unknown | Probed{peers_seen: u32}
}

pub enum AddableTorrent {                 // what resolve() hands to the engine
    Magnet(String),
    TorrentUrl(String),                   // preferred: real trackers + file list
    TorrentBytes(Vec<u8>),
}
```

- **Both P0 providers emit `torrent_url`, not magnet, as primary** — the .torrent carries real
  tracker lists + exact file tree; magnets get synthesized only as fallback (infohash + `dn=` +
  `tr=` from our tracker-list manager + IA/AT trackers).
- **`resolve()` unifies file-list fidelity**: fetch the .torrent → `torrent_from_bytes` → exact
  `files[]` for the detail expand. IA could also use its `metadata` API; parsing the torrent is
  cheaper and provider-agnostic.

### 5.3 Search fan-out

`SearchEngine { providers: Vec<Arc<dyn TorrentProvider>> }` → `join_all` with per-provider timeout
(~8s) and per-provider error isolation (one dead provider must not fail the query). Merge order:
provider-native relevance, cross-provider tie-break on `seeds` → `downloads`/popularity → name.
Dedupe by `infohash` when present.

### 5.4 Handoff to TorrentEngine

```
SearchResult → resolve() → AddableTorrent
  → TorrentEngine::add(
       AddTorrent::from_url(magnet_or_torrent_url) | AddTorrent::from_bytes(bytes),
       AddTorrentOptions { trackers: tracker_mgr.extra_trackers(), .. })
```

`AddTorrent::from_url` already accepts `magnet:`, `http(s)://…​.torrent`, and local paths — the
existing engine path needs no new ingest code, just the options plumbing.

### 5.5 Tracker management (the second missing piece)

Layered, honest about rqbit's limits:

1. **Session level**: `SessionOptions::trackers` ← populated from a bundled snapshot of
   `ngosang/trackerslist` `trackers_best.txt` (verified live, committed daily) at `Session` build
   time. This is the "always use for each torrent" hook — covers tracker-less magnets entirely.
2. **Per-add level**: `AddTorrentOptions::trackers` from user prefs / provider-supplied extras.
3. **`TrackerListManager`** (lives in `lyra-torrent`): fetch → cache on disk → refresh if >7 days
   stale → parse (blank-line separated, both `udp://` and `https://` supported by rqbit's
   `tracker_comms`) → dedupe → cap ~20. Bundled fallback snapshot for offline/first-run.
4. **UI**: show a torrent's effective tracker list from `handle.shared().trackers` — read-only.
   **No post-add mutation exists**; "update trackers on this torrent" = remove + re-add into the
   same output folder (`overwrite` resumes completed files). Call this out in UI copy.
5. No per-tracker health in rqbit → tracker UI shows the list + aggregate peer stats only. Don't
   promise per-tracker status we can't deliver.

### 5.6 The seeds problem

Neither P0 provider publishes seed counts. Mitigations, in order of cost:

- Show `downloads` (IA) / `category` (AT) as popularity signal.
- **Optional swarm probe**: on row-expand (or top-N on scroll), `session.get_dht()
  .get_peers(infohash, None)` for ~5s, count unique peers → `health: Probed{peers_seen}`. It's a
  lower-bound swarm signal (nodes ≠ seeds) — label it "peers seen", not "seeds". Cheap: reuses the
  running DHT, no new deps, works for any result with an infohash.
- IA-specific fallback: every IA file is also directly fetchable at
  `https://archive.org/download/{id}/{file}` — if a stale IA torrent won't fill the buffer, Lyra
  *could* stream the file over plain HTTPS. Optional; mention in code TODO, not in scope for P0.

### 5.7 SwiftUI contract

Search endpoint returns JSON mirroring `SearchResult` (snake_case, all `Option`s absent-vs-zero
preserved):

```json
GET /search?q=coltrane&providers=archive-org,academic-torrents&category=audio
{
  "results": [{
    "id": "gd1977-05-08…", "provider": "archive-org", "name": "…",
    "infohash": "abc…", "magnet": null, "torrent_url": "https://archive.org/download/…_archive.torrent",
    "size_bytes": 168207899, "file_count": 21, "seeds": null,
    "license": "https://creativecommons.org/…", "source_page": "https://archive.org/details/…",
    "files_preview": [{"path": "gd77-05-08d1t01.flac", "size": 12345678}],
    "health": {"kind": "unknown"}
  }],
  "provider_errors": [{"provider": "academic-torrents", "error": "refresh in progress"}]
}
```

Table columns: Name · Provider · Size · Files · Peers (async probe badge) · License · Date.
Row actions: **Stream** (resolve → add → pick largest audio file → existing stream path),
**Download**, expand → `files_preview` → full list from `resolve()`. Incremental rendering is a
nice-to-have (SSE/NDJSON or poll-per-provider); P0 can just await the merged response.

---

## 6. Effort + phases

**P0 — search on legal indexes (~7–9 working days incl. UI + tests)**

| Piece | Est. |
|---|---|
| `lyra-search` crate: trait, `SearchEngine` fan-out, error model | 1 d |
| `ArchiveOrgProvider`: advancedsearch → rows; `resolve()` via metadata API/`_archive.torrent`; `mediatype:audio` (+`collection:etree`) default scope | 2 d |
| `AcademicTorrentsProvider`: database.xml fetch/parse/staleness-refresh + local search; `apiv2/entry` for detail; `/download/{ih}` resolve | 2 d |
| Engine handoff (`AddableTorrent` → `TorrentEngine::add` + `AddTorrentOptions.trackers`) + `TrackerListManager` + `SessionOptions.trackers` wiring | 1 d |
| SwiftUI results table + expand/resolve + probe badge | 2 d |
| Fixture-recorded provider tests (both APIs verified live above; record and replay) | 1 d |

**P1 (~3–4 d)**: `TorznabProvider` (user-entered base URL + API key → works with the user's own
Jackett/Prowlarr — biggest quality unlock for zero bundled legal surface); provider settings UI;
batched swarm-probe; maybe an etree scraper behind a flag.

**P2 (defer, possibly forever)**: opt-in passive DHT sampling — `mainline` server-mode node whose
`request_filter` logs observed infohashes → BEP9-fetch via `list_only` → local index. No BEP51
exists in any maintained Rust DHT crate, so it's passive-only unless we write KRPC. High effort,
nontrivial legal grayness, low marginal value for a music player over P0+P1. **Recommendation:
document, don't build.**

### Risks / open questions

- IA `advancedsearch` can be slow — budget timeouts; consider `rows<=50` + `sort[]=downloads desc`.
- `database.xml` grows over time; parse incrementally and cache parsed form.
- rqbit 8→9 upgrade: `resolve_magnet` stays private, HTTP API gains `/resolve_magnet` — no design
  change either way; `list_only` remains the library path.
- If per-tracker status or runtime tracker editing ever becomes a must-have, it's an upstream rqbit
  feature request, not a Lyra-side workaround.
