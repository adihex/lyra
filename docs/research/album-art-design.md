{"id":"cli:pane:close","result":{"type":"ok"}}
# Lyra — Album Art Pipeline

Date: 2026-09-16. Grounded against the real tree at `~/lyra-src` (uncommitted working
state): `crates/lyra-formats`, `lyra-store`, `lyra-net`, `lyra-ffi`, `lyra-fs`,
`lyra-core`, `app/Sources/LyraApp/{ContentView,Bridge,LyraApp,MediaKeys}.swift`,
`entitlements.plist`, `Cargo.toml`. Symphonia APIs verified against symphonia /
symphonia-core **0.6.1** docs.rs — the version pinned in `lyra-formats/Cargo.toml`
(0.6 restructured probing: `ProbeResult` is gone, `Probe::probe()` returns the
`FormatReader` directly; call shapes below are 0.6-correct).

Scope: an image on screen for every album/track — embedded first, filesystem
second, network third — with a content-addressed cache inside the app container, a
path/hash-only FFI boundary, and an async display layer that never blocks the
custom `LazyVStack` table.

**Corrections vs the task brief**: the enrich client is `crates/lyra-net`
(there is no `lyra-enrich`), tags are read by **lofty** not symphonia, and there is
no `albums` table — album identity is the `(album, album_artist)` string pair on
`tracks`. All three change where the seams land; details below.

Precedence (fixed order, first hit wins):

| # | Source | Scope | Where it runs |
|---|--------|-------|---------------|
| 1 | Embedded picture (`lofty::Picture` `CoverFront`, else any image picture; symphonia `Visual` for pathless sources) | track→album | `probe_track` during scan |
| 2 | Filesystem sibling (`cover.*`, `folder.*`, `front.*`, …) | folder→album | `Library::sync_dir` dir walk |
| 3 | Cover Art Archive via `lyra-net` | album (MB release) | post-import, consent-gated |
| 4 | Placeholder (album-initial tile) | — | Swift-side, zero cost |

Per-track art (a unique embedded image on one track — a single inside a
compilation) is supported via a per-track override, but the default is
album-level: 12 tracks of one album resolve to one shared image.

---

## 1. Embedded extraction

### 1.1 The seam: `probe_track`, and lofty before symphonia

Every import path funnels through one function — extend it once:

- `lyra_formats::probe_track(p)` — `crates/lyra-formats/src/lib.rs:367` — calls
  `probe` + `stream_info` + `read_tags`, returns `LibraryTrack`.
- Callers: `scan_dir` (lib.rs:355), `Library::sync_dir`
  (`crates/lyra-store/src/lib.rs:347`), `Library::sync_files` (lib.rs:389) — the
  `needs_scan` mtime gate means art extraction only runs for new/changed files.

`read_tags` (lib.rs:399) already does
`lofty::probe::Probe::open(path).guess_file_type().read()` → `TaggedFile` →
`primary_tag()`. That parsed tag **already contains the pictures** —
`tag.pictures()` is a getter, not a re-parse:

```rust
// crates/lyra-formats/src/lib.rs — extend read_tags (or a sibling read_art)
use lofty::picture::{Picture, PictureType};
use lofty::tag::Tag as _; // pictures() lives on lofty::tag::Tag

let pics: &[Picture] = tag.pictures();
let front = pics.iter().find(|p| p.pic_type() == PictureType::CoverFront)
    .or_else(|| pics.iter().find(|p| is_image(p.data()))); // magic-byte sniff
// Picture::data() -> &[u8] (encoded image), mime_type() -> Option<&MimeType>
```

Why lofty-first rather than symphonia `Visual`s:

- **Zero extra file opens.** `stream_info`'s symphonia probe exists but its reader
  is dropped inside `stream_info_media` (lib.rs:81) — surfacing visuals means
  restructuring it; `read_tags` returns the tag anyway.
- **Coverage.** lofty reads pictures across FLAC PICTURE, ID3v2 APIC, MP4 `covr`,
  Vorbis `METADATA_BLOCK_PICTURE`, APEv2 cover items, and — critically for this
  library — **DSF/DFF, WavPack, AIFF, Monkey's**, which have no symphonia metadata
  path at all. Lyra's format list (`AudioFormat` in `lyra-core`) includes all of
  these; symphonia visuals would silently miss them.
- symphonia is still the right tool for **pathless sources**: `stream_info_media`
  already accepts `impl MediaSource`, and `lyra-fs::SourceMediaSource`
  (`crates/lyra-fs/src/lib.rs:267`) wraps any `ByteSource` as Read+Seek — torrent
  files, SSH streams. For those, `reader.metadata()` (below) is the only seam;
  lofty can also take a `Read+Seek` via `Probe::new(...)` — either works, but keep
  one code path for "give me bytes + a tag reader".

### 1.2 Symphonia path (pathless sources + cross-check)

```rust
// crates/lyra-formats/src/lib.rs — sibling to stream_info_media
use symphonia::core::common::Limit;             // {None, Default, Maximum(usize)}
use symphonia::core::formats::{FormatOptions, probe::Hint};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, StandardVisualKey, Visual};

fn visuals_from(source: impl symphonia::core::io::MediaSource + 'static,
                ext: &str) -> Result<Vec<Visual>, LyraError> {
    let mss = MediaSourceStream::new(Box::new(source), Default::default());
    let mut hint = Hint::new();
    if !ext.is_empty() { hint.with_extension(ext); }

    // MetadataOptions is #[non_exhaustive] — builder methods only.
    let meta_opts = MetadataOptions::default()
        .limit_visual_bytes(Limit::Maximum(32 << 20))  // 32 MiB per picture
        .limit_tag_bytes(Limit::Default);

    // 0.6: probe() -> Box<dyn FormatReader>; metadata attaches to the reader.
    let mut reader = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), meta_opts)
        .map_err(|e| LyraError::Decode(e.to_string()))?;

    let mut md = reader.metadata();                 // Metadata<'_>
    let mut out = Vec::new();
    loop {                                          // walk ALL revisions — see below
        if let Some(rev) = md.current() {           // &MetadataRevision
            out.extend(rev.media.visuals.iter().cloned());
            for pt in &rev.per_track {              // PerTrackMetadata{track_id:u64, metadata}
                out.extend(pt.metadata.visuals.iter().cloned());
            }
        }
        if md.pop().is_none() { break; }
    }
    Ok(out)
}
```

Verified 0.6.1 types: `Visual { media_type: Option<String>, dimensions:
Option<Size>, color_mode: Option<ColorMode>, usage: Option<StandardVisualKey>,
tags: Vec<Tag>, data: Box<[u8]> }` — `data` is *encoded* bytes; `dimensions` is
declared metadata and may lie. `StandardVisualKey` (non-exhaustive, 20 variants):
`FrontCover`, `BackCover`, `Leaflet`, `Media`, `FileIcon`, `OtherIcon`,
`Illustration`, `ArtistPerformer`, `BandArtistLogo`, `PublisherStudioLogo`,
`ScreenCapture`, …`Other`. `limit_visual_bytes` is enforced per-reader *before*
allocation — the primary defense against a corrupt 4 GiB APIC frame. Already-
enabled features cover it: `all-meta` (id3v1+id3v2+ape) plus `isomp4`, `ogg`,
`flac`, `wav`, `aiff` are all on in `lyra-formats/Cargo.toml`.

**Revision trap** (real in this codebase's terms): an MP3 with ID3v2 front +
ID3v1 tail produces two revisions; the *latest* has no visuals. `md.skip_to_latest()`
alone misses the APIC — gather across all revisions (loop above), or pick the
revision containing a usable `FrontCover`. Same trap applies to tags: prefer the
richest revision, not the newest.

### 1.3 Cover selection & validation (shared by both paths)

```rust
fn is_image(d: &[u8]) -> bool {
    matches!(d, [0xFF,0xD8,0xFF,..]                 // JPEG
        | [0x89,b'P',b'N',b'G',..]                // PNG
        | [b'G',b'I',b'F',b'8',..]                // GIF
        | [b'R',b'I',b'F',b'F',..]                // WEBP-in-RIFF
        | [0x42,0x4D,..])                         // BMP
}
```

- MIME is advisory — sniff magic bytes; `media_type` lies constantly in the wild.
- ID3v2 APIC with MIME `-->`/`image/url`: `data` is a *URL string*, not pixels.
  Drop it locally; record as `remote_ref` so §3 can fetch it when online-fetch
  is enabled.
- `FileIcon`/`OtherIcon` (or lofty `PictureType::FileIcon`) are 32×32 junk —
  below any real cover.
- Validate once at ingest: `image::ImageReader::new(Cursor::new(bytes))
  .with_guessed_format()?.into_dimensions()` — header-only probe, no full decode —
  then reject `w*h > 50 MP` or unknown format. (`image` is a new workspace dep;
  it also generates thumbs in §4.4. `sha2` likewise — content hash below.)

### 1.4 Post-probe flow (Rust, on the scan worker)

1. `pick_cover` → encoded bytes.
2. SHA-256 → content hash `h` (the dedup key).
3. Header-validate → real `mime` + `w×h`; skip+log on failure (never fail a scan).
4. `Library::upsert_artwork(h, …)` → write
   `<appSupport>/Lyra/artwork/<h[0..2]>/<h>/full.<ext>` if absent → set
  `tracks.artwork_hash` (album consensus rule in §4.3).

`probe_track` returns `LibraryTrack` — add `artwork_hash: Option<String>` to
`LibraryTrack` in `lyra-core` (serde camelCase → `artworkHash` on the wire) and a
side-channel in `probe_track` for the raw bytes (the hash is all the DB needs; the
bytes go straight to the cache).

Effort: ~1–1.5 days — `read_tags` extension, hash+cache write, validation, and
fixture tests per format incl. one corrupt APIC.

---

## 2. Filesystem fallback

Runs inside `Library::sync_dir`'s existing `read_dir` loop
(`crates/lyra-store/src/lib.rs:319`) — it already visits every directory entry;
non-audio siblings are currently filtered by `is_audio_ext` and skipped. Intercept
image names before that filter. `sync_files` (panel multi-pick, lib.rs:363) checks
the picked files' parent dir once.

Filename ranking (case-insensitive, per folder):

1. `cover.{jpg,jpeg,png,webp}` / `front.{…}` — modern rippers, Bandcamp
2. `folder.{jpg,jpeg,gif}` — Windows Media Player convention
3. `album.{…}`, `artwork.{…}`, `albumart.{…}`, `coverart.{…}`
4. `<folder-name>.{jpg,png}`
5. Any `*.{jpg,png}` ≥ 10 KiB, largest-by-bytes — last resort. Skip dotfiles and
   `._*` AppleDouble files.

Scoping rules:

- Sibling art is readable under the **same security-scoped bookmark** as the audio
  — `restoreBookmarks`/`storeBookmark` in `ContentView.swift` (lines 257–280)
  re-grant the folder scope at launch; the scan runs inside it. **Copy bytes into
  the container cache during scan while scope is alive** — the cache row is the
  durable reference, never the source path (bookmarks go stale on moves/remounts).
- Track-level override: `<track-stem>.{jpg,png}` beside `track.flac`.
- Multi-disc: prefer the innermost folder containing the track, then walk up ≤ 2
  levels (`Album/CD1/` → `Album/`).
- `booklet*.pdf`, `back.*`, `inlay.*`, `disc.*`/`cd*.jpg` — recognized, not used
  as front cover; worth logging `source` on the artwork row for a future gallery.

Embedded wins over folder art when both exist (it was deliberately muxed); the
`source` column keeps the audit trail for a later "prefer folder art" setting.

Remote/torrent caveat: folder art only exists for `local` sources. SSH sources get
it for free later via `lyra_fs::scan`'s `find -printf` (add image extensions to
the `-iname` list, lib.rs:318) + a `dd`-fetch of the matched file.

Effort: ~0.5–1 day — ranking table, readdir interception, copy+dedup.

---

## 3. Online fetch — CAA via `crates/lyra-net`

The existing client is thin and stays that way — extend it, don't build a second
HTTP stack:

- `MusicBrainz::release(mbid)` — `crates/lyra-net/src/lib.rs:72` — WS2 JSON,
  `MUSICBRAINZ_UA` (lib.rs:10) already set.
- `coverart_url(mbid)` — lib.rs:118 — returns
  `https://coverartarchive.org/release/{mbid}/front-500`. **Note it hardcodes the
  500px thumb**: fine for display but wrong for the `full` slot — add a sibling
  for the JSON listing (`GET /release/{mbid}/`) so we can pick `image` (full) or
  `thumbnails.1200` when the full image is > 8 MiB. `/front-250|500|1200` and
  `/release-group/{rgid}/front` also exist.
- `reqwest` rustls-only (lyra-net/Cargo.toml) — outbound already permitted by
  `com.apple.security.network.client` in `entitlements.plist`.
- The rate-limit TODO is already in the file ("max 1 req/s — callers go through a
  shared rate-limiter (TODO: governor crate)", lib.rs:71): implement a
  `Mutex<Instant>`-gated 1.1 s bucket on the `MusicBrainz`/CAA call path (or
  `governor`), honor `Retry-After` on 503, exponential backoff on 5xx. CAA rides
  archive.org — same bucket.

### 3.1 Getting a release MBID

- **Tagged**: Picard/beets write `MusicBrainz Album Id` (ID3 TXXX),
  `MUSICBRAINZ_ALBUMID` (Vorbis), `----:com.apple.iTunes:MusicBrainz Album Id`
  (MP4). `TagMap` (`lyra-core/src/lib.rs:38`) has no MBID field — add
  `musicbrainz_release_id: Option<String>` read in `read_tags` via
  `lofty::tag::ItemKey::MusicBrainzReleaseId` (lofty maps all three containers).
  Exact-match path, no fuzzy risk.
- **Untagged**: text-search `release/?query=release:"<album>" AND artist:"<aa>"`
  against WS2; accept only `score >= 90` and artist match. Wrong art is worse
  than none. Fingerprinting (AcoustID) is a later refinement if the blueprint's
  enrichment milestone wants it.
- Store the MBID on `tracks`/`artwork` so a re-match detects "MBID changed →
  refetch".

### 3.2 Consent, offline, negative cache

- Fetching discloses library contents — gate on a `settings` key
  (`settings` table exists, migration v1, `lyra-store/src/lib.rs:69`):
  `enrich.online=0` default + first-run prompt + per-album "Fetch artwork"
  action in `trackMenu` (`ContentView.swift:950`) as album-scoped consent.
- Offline: skip the queue when a fetch would fail-fast; never let a connect
  error mark a row `not_found`.
- Negative cache — misses are sticky until metadata changes:

```sql
-- appended to MIGRATIONS as v3 alongside the artwork tables
CREATE TABLE artwork_fetch (
    album_key     TEXT PRIMARY KEY,    -- lower(album_artist)|'\x1f'|lower(album)
    mbid          TEXT,
    state         TEXT NOT NULL,       -- 'ok'|'not_found'|'error'|'skipped_no_mbid'
    http_status   INTEGER,
    attempts      INTEGER NOT NULL DEFAULT 1,
    attempted_at  INTEGER NOT NULL,
    next_retry_at INTEGER              -- NULL = never
);
```

`not_found` → `next_retry_at = +90d` (or MBID change); `error` → `24h*2^attempts`
cap 14d/8 attempts; manual fetch bypasses once.

Effort: ~2–3 days — queue, consent key + UI affordances, table, mock-server tests.

---

## 4. Storage & cache

### 4.1 Location — the established pattern, verbatim

The codebase already solved this twice:

- `LyraLibrary.init` (`Bridge.swift:42–49`): `Application Support/Lyra/` +
  `library.db`.
- `LyraTorrent.init` (`Bridge.swift:209–216`): creates `Lyra/torrents`, passes the
  path to `lyra_torrent_init`. Comment says it outright: "downloads land inside
  the app container so the sandbox permits reads/writes".

Art follows identically: Swift creates `Application Support/Lyra/artwork/` and
passes it once — either a `lyra_artwork_init(dir: *const c_char) -> c_int` in
`lyra-ffi` (mirror of `lyra_torrent_init`, lib.rs:591) or, simpler, derive it in
Rust from the `library.db` path already passed to `lyra_lib_open` (its parent dir
is `Lyra/`). Recommendation: explicit `lyra_artwork_init` — the torrent/remote
precedent, testable without a DB. Set `NSURLIsExcludedFromBackupKey` on the dir —
rebuildable data, keeps backup size honest.

### 4.2 Files, not blobs

Hashed file cache, not SQLite `BLOB`s — `NSImage(contentsOfFile:)`/ImageIO read
files directly with zero FFI copies. The `waveform_peaks` BLOB precedent
(`lyra-store` lib.rs:74) is per-track small data; art doesn't fit that pattern.

```
Application Support/Lyra/artwork/
  <sha256[0..2]>/<sha256>/
    full.<ext>      # original bytes verbatim (write-back fidelity, hi-res UI)
    256.jpg         # precomputed (hero/mini-player @2x)
    64.jpg          # precomputed (table rows @2x @27-32pt)
```

### 4.3 Schema — extend `MIGRATIONS` as v3

There is no `albums` table — `tracks.album`/`tracks.album_artist` strings are the
identity (`query_tracks`, `upsert_track` in `lyra-store`). So album-level art is a
per-track column pointing at a shared hash row; dedup is the `UNIQUE` hash:

```sql
-- MIGRATIONS v3 — lyra-store/src/lib.rs, append to the const array
CREATE TABLE artwork (
    hash       TEXT PRIMARY KEY,       -- sha256 hex of encoded bytes
    rel_path   TEXT NOT NULL,          -- "ab/abcd…/full.jpg" under artwork root
    source     TEXT NOT NULL,          -- 'embedded'|'file'|'caa'|'remote_ref'
    mime       TEXT NOT NULL,          -- sniffed, not declared
    width      INTEGER, height INTEGER,
    byte_len   INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
ALTER TABLE tracks ADD COLUMN artwork_hash TEXT REFERENCES artwork(hash);
-- artwork_fetch from §3.2 rides in the same migration.
```

- 12 tracks → one `artwork` row → `tracks.artwork_hash` on each. `ON CONFLICT`
  upsert on `artwork` makes re-scans idempotent.
- Track override is the same column — a track whose embedded art differs from its
  album-mates simply carries a different hash. Album-consensus rule: first-seen
  embedded art in an album folder becomes the folder default; differing tracks
  keep their own hash. Deliberately simple; deterministic ordering
  (`track_no`/`disk`) is a cheap hardening if rescans flip it.
- GC: `prune_missing` (lib.rs:187) drops track rows — art rows are refcounted
  separately: a weekly/lazy `DELETE FROM artwork WHERE hash NOT IN (SELECT
  artwork_hash FROM tracks …)` + unlink. Note `waveform_peaks` cascades on
  `tracks(path)`; `artwork` must NOT cascade (rows are shared across tracks).

### 4.4 Thumbnails — Rust at ingest, `image` crate

Generate `64.jpg`/`256.jpg` when `full` is written — same scan worker, once per
*unique* image:

```rust
let img = image::ImageReader::new(std::io::Cursor::new(&bytes))
    .with_guessed_format()?      // real format, not declared mime
    .decode()?;                  // after the §1.3 header check passed
img.thumbnail(256, 256)          // aspect-fit, fast path
    .write_to(&mut f256, image::ImageFormat::Jpeg)?; // q~80 via JpegEncoder for control
```

`image` and `sha2` are new workspace deps (`Cargo.toml` `[workspace.dependencies]`).
Alternative considered: Swift-side `CGImageSourceCreateThumbnailAtIndex` with
`kCGImageSourceThumbnailMaxPixelSize` — hardware-fast and lazy, but puts decode on
the scroll path. Precompute keeps the table's render loop a pure file read.

Effort: ~1.5 days — schema, hashing, validation, thumbs, GC.

---

## 5. FFI boundary — hash on the row, paths composed in Swift

Existing conventions in `crates/lyra-ffi/src/lib.rs`: heap `*mut c_char` JSON out
(freed via `lyra_string_free`), opaque handles, `#[no_mangle] extern "C"`, errors
as JSON — and `panic = "unwind"` is kept workspace-wide precisely so a
decode/format bug can't unwind across FFI (`Cargo.toml:23`).

Minimal additions:

- `LibraryTrack` gains `artwork_hash` (serde → `artworkHash`); `Track.init`
  (`ContentView.swift:22`) already maps unknown keys lazily — one line:
  `artworkHash = d["artworkHash"] as? String`. Then `lyra_lib_tracks` /
  `lyra_lib_search` carry art refs **with no new FFI calls at all**.
- Swift composes the path itself — it owns the root:
  `appSupport/Lyra/artwork/<h[0..2]>/<h>/{64,256,full}.jpg`. Deterministic, no
  per-row call.
- One new call for the hero/uncached case + manual fetch:
  `lyra_artwork_fetch(lib, album_key, size_class)` → path or NULL — mirrors the
  `lyra_lib_*` shape.
- **Not** base64/byte-array: the `lyra_engine_viz` JSON lesson (lib.rs:242 —
  "60 Hz path: no JSON, no alloc") applies — don't ship pixels through CString.
  The cache dir is in-container; `NSImage(contentsOfFile:)` reads it directly.
- Byte-serving FFI is only warranted later for the remote (`lyra-remote` Host —
  agents/phone clients can't see the container); add `artwork.get` to the remote
  protocol then.

Effort: ~0.5 day.

---

## 6. Display (SwiftUI)

### 6.1 Surfaces — where they actually are

| Surface | Code | Size |
|---|---|---|
| Transport bar | `nowPlayingBar`, `ContentView.swift:966` — HStack at line 976, insert `ArtworkImage` left of the title VStack (40pt) | `256` |
| Menu-bar mini player | `MiniPlayerView`, `LyraApp.swift:52` — VStack top, add 48pt image above title | `256` |
| Track table | `trackRow` (`ContentView.swift:908`) inside `LazyVStack` (859), 27pt rows, `trackCols` widths (871) — optional art in the Album cell, or a dedicated leading column | `64` |
| Now Playing / Control Center | `MediaKeys.publish` (`MediaKeys.swift:42`) — add `MPMediaItemPropertyArtwork` via `MPMediaItemArtwork(boundsSize:requestHandler:)` | `256`/`full` |
| Empty-state placeholder | the `🪐` tile pattern at `ContentView.swift:820` is the aesthetic to match — album-initial monogram on `Ui.surface` | — |

### 6.2 Async load — the table is a custom LazyVStack, act accordingly

`trackTable` (`ContentView.swift:852`) is `GeometryReader` → `trackHeader` +
`ScrollView{LazyVStack{trackRow}}`, rebuilt no-diff via `.id(vm.contentID)`.
Decode must be off the main path:

```swift
final class ArtworkCache {
    static let shared = ArtworkCache()
    private let mem = NSCache<NSString, NSImage>()  // countLimit ~300
    private var inflight = Set<String>()
    // key: "<hash>/<size>" — miss → detached decode via ImageIO at display size
}

struct ArtworkImage: View {
    let hash: String?; let size: Int               // 64 or 256
    @State private var img: NSImage?
    var body: some View {
        Group { if let img { Image(nsImage: img) } else { placeholder } }
            .task(id: hash) { img = await ArtworkCache.shared.load(hash, size) }
    }
}
```

Rules:

- Decode with `CGImageSourceCreateThumbnailAtIndex` +
  `kCGImageSourceThumbnailMaxPixelSize` — reads the pre-sized thumb file, never
  materializes the full image for a 27pt row. This is also the hero fallback when
  `full` is huge (BLUEPRINT §Footprint already mandates: "decode off main thread,
  downscale to display size, LRU disk cache" — this design conforms exactly).
- Fixed frame + placeholder so rows don't re-layout on arrival.
- `.task(id:)` cancels on scroll-away; NSCache makes re-scroll ~free. Cap
  concurrent decodes (~4); prefetch ±20 rows on appear via a fire-and-forget
  `prefetch(hash)`.
- Hero: keep last image until the next decodes — crossfade
  `.easeInOut(0.25)`, no flash-to-placeholder mid-album.
- `MediaKeys.publish` gains an `artwork:` param:
  `MPMediaItemArtwork(boundsSize:)` whose handler loads `256`/`full` — free
  Control Center/lock-screen art off the same cache.

Effort: ~1.5 days — cache, view, transport-bar + mini-player + MediaKeys wiring,
one scroll-perf pass at 10k rows.

---

## 7. Write path — defer to the tag-editing milestone

lofty is **already the read path** (`read_tags`), so write-back later is the same
crate: `lofty::picture::Picture` + `Tag::push_picture()` + `save_to_path` handles
APIC/covr/PICTURE per format. (`audex` covers DSF/DFF — already declared for the
same reason.) `entitlements.plist` already grants
`com.apple.security.files.user-selected.read-write`, so the sandbox isn't the
blocker — correctness is: embedding art rewrites user files, and that belongs in
the blueprint's tag-editing milestone with backup/verify, not in a display
feature. Until then the art pipeline is strictly read-only.

Note for that milestone: `full` is stored verbatim, so "embed this cover into the
files" already has the bytes; CAA-fetched art is the obvious embed candidate.

---

## 8. Phased plan

| Phase | Contents | Est. |
|---|---|---|
| **P0** | `read_tags`/`probe_track` picture extraction (lofty primary, symphonia `visuals()` for `MediaSource` sources); `image`+`sha2` deps; `artwork` v3 migration + `tracks.artwork_hash`; `Lyra/artwork/` cache via `lyra_artwork_init`; FS sibling fallback in `sync_dir`; `artworkHash` on `LibraryTrack`; transport-bar + mini-player + MediaKeys art | ~4–5 d |
| **P1** | CAA fetch in `lyra-net` (JSON listing + `coverart_url` variant), `MusicBrainz Album Id` into `TagMap`, 1.1 s shared bucket (kills the `governor` TODO), `enrich.online` setting + first-run prompt, `artwork_fetch` negative cache | ~2–3 d |
| **P2** | `ArtworkCache` + `ArtworkImage`, 64px in `trackRow`/album cell, prefetch, 10k-row scroll-perf pass | ~2–3 d |
| **P3 (later)** | Embed/write-back inside the tag-editing milestone; Get-Info "replace art"; gallery for back/leaflet/booklet scans; `artwork.get` on the remote socket | bundled |

P0 alone covers most well-tagged lossless libraries with zero network. P1 closes
untagged/ripped gaps. P2 is polish on a working base.

## 9. Honest risks

- **Malformed/mislabeled pictures.** `media_type` lies; APIC `image/url`
  indirection exists; `FileIcon` junk. Mitigate: `limit_visual_bytes` (symphonia),
  magic-byte sniff, header-only dimension probe, 50 MP cap, skip+log — a bad
  picture must never fail `probe_track` (it's called inside `sync_dir`'s loop; an
  exception there stalls the scan).
- **Huge embedded images.** 30 MB PNGs ship in real files. `full` is verbatim so
  disk cost is bounded by what's already in the user's library; decode is
  capped + worker-side only. BLUEPRINT §Footprint explicitly calls out the
  incumbent's unbounded artwork cache — the hashed-file + precomputed-thumb design
  is the counter.
- **Decompression bombs.** Valid header, absurd pixels → the dimension pre-check +
  `image::Limits` bound it before decode.
- **Consensus-hash flips.** Sampler folders where every track embeds different
  art: first-seen becomes "album" art — a rescan with different file order can
  change which. Harmless but visible; prefer `disk 1/track 1` for determinism.
- **Bookmark expiry for folder art.** Scope is alive at scan time only; the cache
  copy is the durable record. A refactor that stores source paths instead of
  copying would regress silently — call it out in code review.
- **Container path management.** Swift resolves the container dir; Rust must treat
  stored paths as root-relative (bundle-id/team changes relocate containers;
  `cargo test` runs unsandboxed with a different root). `lyra_artwork_init` takes
  the path per-run — same as `lyra_torrent_init`.
- **CAA coverage/consent.** No MBID → no fetch → intentional-looking placeholder.
  Consent-off means silence: no retries, no error spam in `scanStatus`.
- **Torrent/SSH sources.** Folder art doesn't exist for them; embedded art needs
  the `SourceMediaSource` path (works — `lyra_torrent_probe` already reads header
  regions on demand). Scope P0 to `Source::file`; torrent art rides the same
  machinery when the remote-scan milestone lands.
- **Album identity is strings.** `(album, album_artist)` collisions ("Greatest
  Hits") can share art across different albums — album_key dedup is a heuristic,
  and the `artwork` hash dedup means a collision only costs a wrong-but-harmless
  shared image. MBID arrival (P1) makes this precise.

## 10. Open questions

- `coverart_url` hardcodes `front-500` — keep it for thumb-only contexts, but the
  JSON listing is the right primary call; confirm lyra-net shouldn't grow a
  `coverart(mbid) -> CaaImage` type instead of URL-string helpers.
- Hero at `full` vs capped 1024 — decide with real cache sizes after P0.
- "Prefer folder art over embedded" as a setting — cheap later via `source`.
- Does `probe_track` belong in lyra-store's sync loop long-term (it already
  crosses the crate boundary — `lyra-store` depends on `lyra-formats`), or does
  art deserve its own pass that can be re-run without re-probing audio? Current
  mtime-gating makes conflation cheap; a "rebuild artwork cache" maintenance verb
  is the escape hatch.
