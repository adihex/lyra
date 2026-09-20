//! lyra-store: the library database. Rust-owned single writer (rusqlite
//! bundled — FTS5 compiled in), WAL, schema via rusqlite_migration.
//! Swift queries through FFI; it never sees SQL.
//!
//! Design notes:
//! - `tracks` is the source of truth; `tracks_fts` is a trigger-synced
//!   FTS5 external-content index over title/artist/album.
//! - Incremental scan: `needs_scan(path, mtime)` — only files whose mtime
//!   changed get re-probed; `prune_missing` drops vanished files.
//! - `sources` records library roots (local dirs today; ssh:// and
//!   torrent:// roots get the same row shape).

use lyra_core::{AudioFormat, LibraryTrack, LyraError};
use rusqlite::{params, Connection};
use rusqlite_migration::{Migrations, M};
use std::path::{Path, PathBuf};

mod query_counts;
pub use query_counts::QueryCounter;

/// (album, artist, album_artist, artwork_hash, title) for one track row.
type ArtQueryRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);
/// (state, hash, next_retry_at) ledger row for an album art fetch.
type ArtFetchRow = (String, Option<String>, Option<i64>);
/// (sample_rate, samples_per_bucket, min/max pairs) stored waveform peaks.
type WavePeaks = (f32, u64, Vec<(f32, f32)>);

const MIGRATIONS: &[&str] = &[
    // v1: tracks + FTS + sources + settings
    r#"
CREATE TABLE tracks (
    id            INTEGER PRIMARY KEY,
    path          TEXT NOT NULL UNIQUE,
    title         TEXT,
    artist        TEXT,
    album         TEXT,
    album_artist  TEXT,
    genre         TEXT,
    year          INTEGER,
    track_no      INTEGER,
    duration_secs REAL,
    format        TEXT NOT NULL DEFAULT 'unknown',
    codec         TEXT,
    sample_rate   INTEGER,
    channels      INTEGER,
    bit_depth     INTEGER,
    size_bytes    INTEGER,
    mtime         INTEGER NOT NULL DEFAULT 0,
    added_at      INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX idx_tracks_album  ON tracks(album);
CREATE INDEX idx_tracks_artist ON tracks(artist);

CREATE VIRTUAL TABLE tracks_fts USING fts5(
    title, artist, album,
    content='tracks', content_rowid='id'
);
CREATE TRIGGER tracks_ai AFTER INSERT ON tracks BEGIN
    INSERT INTO tracks_fts(rowid, title, artist, album)
    VALUES (new.id, new.title, new.artist, new.album);
END;
CREATE TRIGGER tracks_ad AFTER DELETE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album)
    VALUES ('delete', old.id, old.title, old.artist, old.album);
END;
CREATE TRIGGER tracks_au AFTER UPDATE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album)
    VALUES ('delete', old.id, old.title, old.artist, old.album);
    INSERT INTO tracks_fts(rowid, title, artist, album)
    VALUES (new.id, new.title, new.artist, new.album);
END;

CREATE TABLE sources (
    id       INTEGER PRIMARY KEY,
    uri      TEXT NOT NULL UNIQUE,   -- file://… ssh://host/path torrent://…
    kind     TEXT NOT NULL,          -- local | ssh | torrent
    added_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
"#,
    // v2: waveform seekbar peaks — O(width) min/max pairs per track,
    // computed at import time. Cascades with the track row.
    r#"
CREATE TABLE waveform_peaks (
    path               TEXT PRIMARY KEY REFERENCES tracks(path) ON DELETE CASCADE,
    sample_rate        REAL NOT NULL,
    samples_per_bucket INTEGER NOT NULL,
    peaks              BLOB NOT NULL   -- LE f32 pairs [min,max]
);
"#,
    // v3: artwork — content-addressed file cache, hash shared across an
    // album's tracks. artwork_hash '' = "checked, none" (don't re-probe);
    // NULL = never checked. artwork_fetch pre-creates the CAA negative
    // cache for the online-fetch phase.
    r#"
CREATE TABLE artwork (
    hash       TEXT PRIMARY KEY,     -- sha256 hex of encoded bytes
    rel_path   TEXT NOT NULL,        -- "<ab>/<hash>/full.<ext>" under artwork root
    source     TEXT NOT NULL,        -- 'embedded'|'file'|'caa'|'remote_ref'
    mime       TEXT NOT NULL,        -- sniffed, not declared
    width      INTEGER,
    height     INTEGER,
    byte_len   INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
-- No FK: artwork rows are shared across tracks and GC'd manually —
-- a REFERENCES clause would couple lifetimes we deliberately decoupled.
ALTER TABLE tracks ADD COLUMN artwork_hash TEXT;
CREATE TABLE artwork_fetch (
    album_key     TEXT PRIMARY KEY,  -- lower(album_artist)|'\x1f'|lower(album)
    mbid          TEXT,
    state         TEXT NOT NULL,     -- 'ok'|'not_found'|'error'|'skipped_no_mbid'
    http_status   INTEGER,
    attempts      INTEGER NOT NULL DEFAULT 1,
    attempted_at  INTEGER NOT NULL,
    next_retry_at INTEGER            -- NULL = never
);
"#,
    // v4: song-map registry (§4.5 of the transcription-pipeline spec).
    // One row per analyzed recording, keyed by BLAKE3 of the source bytes
    // so identical files share maps across local/torrent/remote copies.
    // `tracks.audio_hash` joins tracks to their map (NULL = never hashed).
    r#"
CREATE TABLE track_maps (
    audio_hash    TEXT PRIMARY KEY,  -- BLAKE3 of source bytes
    map_path      TEXT NOT NULL,     -- maps/<audio_hash>.lyramap
    pipeline_ver  TEXT NOT NULL,     -- e.g. "mapgen-0.1|beat_this-1.0|bp-icassp22"
    status        TEXT NOT NULL,     -- pending|grid|chords|notes|done|unsupported|failed
    overall_conf  REAL,
    updated_at    INTEGER NOT NULL
);
CREATE INDEX idx_track_maps_status ON track_maps(status);
ALTER TABLE tracks ADD COLUMN audio_hash TEXT;
"#,
    // v5: ledger remembers the artwork hash it produced — torrent rows
    // (and any metadata-keyed fetch) have no tracks.artwork_hash to
    // stamp, so the ledger is their only durable record. NULL on rows
    // predating the column; art_fetch_due treats ok-with-null-hash as
    // due once, so they backfill on the next fetch attempt.
    r#"
ALTER TABLE artwork_fetch ADD COLUMN hash TEXT;
"#,
];

/// AudioFormat <-> its serde-lowercase string ("flac", "m4a", …).
fn format_str(f: &AudioFormat) -> String {
    serde_json::to_value(f)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}
fn format_from(s: &str) -> AudioFormat {
    serde_json::from_str(&format!("\"{s}\"")).unwrap_or(AudioFormat::Unknown)
}

pub struct Library {
    conn: Connection,
    /// Content-addressed artwork cache root — derived as `<db dir>/artwork`
    /// (same convention as the torrents dir). None for in-memory libs:
    /// ingest becomes a no-op.
    artwork_dir: Option<PathBuf>,
    /// Statement counter for N+1 regression tests (see `query_counts`).
    queries: QueryCounter,
}

#[derive(Debug, Clone)]
pub struct Source {
    pub uri: String,
    pub kind: String,
}

/// Image sibling → folder-art rank (lower wins); None = not a cover
/// candidate. Dotfiles, `._*` AppleDouble, and booklet/back/inlay names
/// are excluded — they exist but are never the front cover.
fn folder_art_rank(dir: &Path, p: &Path) -> Option<u8> {
    let name = p.file_name()?.to_str()?.to_lowercase();
    if name.starts_with('.') {
        return None;
    }
    let stem = p.file_stem()?.to_str()?.to_lowercase();
    let ext = p.extension()?.to_str()?.to_lowercase();
    if !matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp"
    ) {
        return None;
    }
    if matches!(stem.as_str(), "back" | "inlay" | "booklet" | "disc" | "cd")
        || stem.starts_with("booklet")
    {
        return None;
    }
    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_lowercase());
    let size = p.metadata().ok()?.len();
    Some(match stem.as_str() {
        "cover" | "front" => 0,
        "folder" => 1,
        "album" | "artwork" | "albumart" | "coverart" => 2,
        s if Some(s) == dir_name.as_deref() => 3,
        _ if size >= 10 * 1024 => 4,
        _ => return None,
    })
}

impl Library {
    pub fn open(path: &Path) -> Result<Self, LyraError> {
        let mut conn = Connection::open(path)?;
        Self::init(&mut conn)?;
        Ok(Self {
            conn,
            artwork_dir: path.parent().map(|d| d.join("artwork")),
            queries: QueryCounter::new(),
        })
    }

    pub fn open_memory() -> Result<Self, LyraError> {
        let mut conn = Connection::open_in_memory()?;
        Self::init(&mut conn)?;
        Ok(Self {
            conn,
            artwork_dir: None,
            queries: QueryCounter::new(),
        })
    }

    fn init(conn: &mut Connection) -> Result<(), LyraError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Migrations::new(MIGRATIONS.iter().map(|s| M::up(s)).collect())
            .to_latest(conn)
            .map_err(|e| LyraError::Io(std::io::Error::other(e.to_string())))?;
        Ok(())
    }

    /// Insert or refresh a track row (keyed on path). `mtime`/`size_bytes`
    /// are filesystem facts, kept on the row for incremental scanning.
    pub fn upsert_track(
        &self,
        t: &LibraryTrack,
        mtime: i64,
        size_bytes: i64,
    ) -> Result<(), LyraError> {
        self.note_query();
        self.conn.execute(
            "INSERT INTO tracks (path, title, artist, album, album_artist,
                 genre, year, track_no, duration_secs, format, codec,
                 sample_rate, channels, bit_depth, size_bytes, mtime,
                 artwork_hash)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
             ON CONFLICT(path) DO UPDATE SET
                 title=excluded.title, artist=excluded.artist,
                 album=excluded.album, album_artist=excluded.album_artist,
                 genre=excluded.genre, year=excluded.year,
                 track_no=excluded.track_no,
                 duration_secs=excluded.duration_secs,
                 format=excluded.format, codec=excluded.codec,
                 sample_rate=excluded.sample_rate,
                 channels=excluded.channels, bit_depth=excluded.bit_depth,
                 size_bytes=excluded.size_bytes, mtime=excluded.mtime,
                 artwork_hash=excluded.artwork_hash",
            params![
                t.path,
                t.title,
                t.artist,
                t.album,
                t.album_artist,
                t.genre,
                t.year.map(|y| y as i64),
                t.track_number.map(|n| n as i64),
                t.duration_secs,
                format_str(&t.format),
                t.codec,
                t.sample_rate.map(|v| v as i64),
                t.channels.map(|v| v as i64),
                t.bits_per_sample.map(|v| v as i64),
                size_bytes,
                mtime,
                t.artwork_hash,
            ],
        )?;
        Ok(())
    }

    /// Does this file need (re)probing? True if unseen or mtime changed.
    pub fn needs_scan(&self, path: &str, mtime: i64) -> Result<bool, LyraError> {
        self.note_query();
        let known: Option<i64> = self
            .conn
            .query_row(
                "SELECT mtime FROM tracks WHERE path=?1",
                params![path],
                |r| r.get(0),
            )
            .ok();
        Ok(known != Some(mtime))
    }

    /// True when the row exists but embedded art was never checked
    /// (NULL hash) — first scan after the v3 migration, or new tracks
    /// whose probe somehow skipped art.
    pub fn needs_art(&self, path: &str) -> Result<bool, LyraError> {
        self.note_query();
        let is_null: bool = self
            .conn
            .query_row(
                "SELECT artwork_hash IS NULL FROM tracks WHERE path=?1",
                params![path],
                |r| r.get(0),
            )
            .unwrap_or(false);
        Ok(is_null)
    }

    /// Record the art outcome for a track without touching other columns.
    /// `None` writes '' — "checked, no embedded art" (sticky marker so
    /// artless files aren't re-probed every scan).
    pub fn set_track_artwork(&self, path: &str, hash: Option<&str>) -> Result<(), LyraError> {
        self.note_query();
        self.conn.execute(
            "UPDATE tracks SET artwork_hash=?1 WHERE path=?2",
            params![hash.unwrap_or(""), path],
        )?;
        Ok(())
    }

    // ── Online artwork fetch (artwork_fetch ledger) ───────────────────

    /// The artwork_fetch key convention — album artist when tagged, else
    /// track artist, both lowercased, \x1f-separated from lower(album).
    pub fn album_art_key(album_artist: Option<&str>, artist: Option<&str>, album: &str) -> String {
        format!(
            "{}\x1f{}",
            album_artist.or(artist).unwrap_or("").to_lowercase(),
            album.to_lowercase()
        )
    }

    /// What a CAA fetch needs to know about a track row:
    /// (album, artist, album_artist, artwork_hash, title).
    pub fn track_art_query(&self, path: &str) -> Result<Option<ArtQueryRow>, LyraError> {
        self.note_query();
        let row = self
            .conn
            .query_row(
                "SELECT album, artist, album_artist, artwork_hash, title FROM tracks WHERE path=?1",
                params![path],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .ok();
        Ok(row)
    }

    /// Fetch-ledger row for an album: (state, hash, next_retry_at).
    /// `hash` is the artwork hash the fetch produced — present only on
    /// 'ok' rows written since the column existed.
    pub fn art_fetch_row(&self, album_key: &str) -> Result<Option<ArtFetchRow>, LyraError> {
        self.note_query();
        let row = self
            .conn
            .query_row(
                "SELECT state, hash, next_retry_at FROM artwork_fetch WHERE album_key=?1",
                params![album_key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        Ok(row)
    }

    /// True when the ledger says this album may be queried now —
    /// no row yet, or a retryable row whose next_retry_at has passed.
    /// 'ok' and NULL-retry rows are terminal answers, not prompts —
    /// except ok-without-hash, a pre-v5 row that backfills once.
    pub fn art_fetch_due(&self, album_key: &str) -> Result<bool, LyraError> {
        Ok(match self.art_fetch_row(album_key)? {
            None => true,
            Some((state, hash, retry)) => {
                (state == "ok" && hash.is_none())
                    || (state != "ok"
                        && retry.is_some_and(|t| {
                            t <= std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs() as i64)
                                .unwrap_or(0)
                        }))
            }
        })
    }

    /// Upsert the ledger: attempts accumulates across retries,
    /// next_retry_at = NULL freezes the outcome permanently.
    pub fn art_fetch_record(
        &self,
        album_key: &str,
        mbid: Option<&str>,
        state: &str,
        http_status: Option<i64>,
        next_retry_at: Option<i64>,
        hash: Option<&str>,
    ) -> Result<(), LyraError> {
        self.note_query();
        self.conn.execute(
            "INSERT INTO artwork_fetch
                 (album_key, mbid, state, http_status, attempts, attempted_at, next_retry_at, hash)
             VALUES (?1,?2,?3,?4,1,unixepoch(),?5,?6)
             ON CONFLICT(album_key) DO UPDATE SET
                 mbid=excluded.mbid, state=excluded.state,
                 http_status=excluded.http_status,
                 attempts=artwork_fetch.attempts+1,
                 attempted_at=excluded.attempted_at,
                 next_retry_at=excluded.next_retry_at,
                 hash=excluded.hash",
            params![album_key, mbid, state, http_status, next_retry_at, hash],
        )?;
        Ok(())
    }

    /// Fan one fetched hash to every still-artless track on the album.
    /// Joins on lower(COALESCE(album_artist, artist)) — compilations tag
    /// album_artist; plain albums key off the track artist.
    pub fn set_album_artwork(
        &self,
        album: &str,
        artist_key: &str,
        hash: &str,
    ) -> Result<usize, LyraError> {
        self.note_query();
        let n = self.conn.execute(
            "UPDATE tracks SET artwork_hash=?1
             WHERE (artwork_hash IS NULL OR artwork_hash='')
               AND album=?2
               AND lower(COALESCE(album_artist, artist))=lower(?3)",
            params![hash, album, artist_key],
        )?;
        Ok(n)
    }

    /// Cache image bytes → content-addressed file row → hash.
    /// Writes `full.<ext>` + 64/256 JPEG thumbs once per unique image;
    /// the `artwork` row is shared across every track that resolves to it.
    /// Returns None when there's no cache dir (in-memory lib) or the
    /// bytes fail validation — art must never fail a scan.
    pub fn ingest_artwork(&self, bytes: &[u8], mime: &str, source: &str) -> Option<String> {
        use sha2::Digest;
        let root = self.artwork_dir.as_ref()?;

        // Header-only validation first — a corrupt image must not be cached.
        let probe = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        let (w, h) = probe.into_dimensions().ok()?;
        if u64::from(w) * u64::from(h) > 50_000_000 {
            return None;
        }

        let hash: String = sha2::Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let ext = match mime {
            "image/png" => "png",
            "image/gif" => "gif",
            "image/webp" => "webp",
            "image/bmp" => "bmp",
            _ => "jpg",
        };
        let rel = format!("{}/{hash}/full.{ext}", &hash[..2]);
        let dir = root.join(&hash[..2]).join(&hash);
        let full = dir.join(format!("full.{ext}"));
        if !full.exists() {
            std::fs::create_dir_all(&dir).ok()?;
            std::fs::write(&full, bytes).ok()?;
        }
        // Thumbs derive from the bytes, not from `full` being new — a
        // crash between the write and the thumb pass leaves full-only
        // entries, so check each thumb independently.
        let miss64 = !dir.join("64.jpg").exists();
        let miss256 = !dir.join("256.jpg").exists();
        if miss64 || miss256 {
            if let Ok(img) = image::load_from_memory(bytes) {
                if miss64 {
                    let _ = img
                        .thumbnail(64, 64)
                        .save_with_format(dir.join("64.jpg"), image::ImageFormat::Jpeg);
                }
                if miss256 {
                    let _ = img
                        .thumbnail(256, 256)
                        .save_with_format(dir.join("256.jpg"), image::ImageFormat::Jpeg);
                }
            }
        }
        self.note_query();
        self.conn
            .execute(
                "INSERT OR IGNORE INTO artwork
                     (hash, rel_path, source, mime, width, height, byte_len)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![hash, rel, source, mime, w, h, bytes.len() as i64],
            )
            .ok()?;
        Some(hash)
    }

    /// Drop rows whose path isn't in `alive` — the post-scan prune.
    /// Refuses to run on an empty list: an empty scan is almost always a
    /// scan failure, not a deleted library.
    pub fn prune_missing(&self, alive: &[String]) -> Result<usize, LyraError> {
        if alive.is_empty() {
            return Ok(0);
        }
        self.note_query();
        let existing: Vec<String> = self
            .conn
            .prepare("SELECT path FROM tracks")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let alive: std::collections::HashSet<&str> = alive.iter().map(String::as_str).collect();
        let stale: Vec<String> = existing
            .into_iter()
            .filter(|p| !alive.contains(p.as_str()))
            .collect();
        let mut removed = 0;
        self.note_query();
        let mut stmt = self.conn.prepare("DELETE FROM tracks WHERE path=?1")?;
        for p in &stale {
            self.note_query();
            removed += stmt.execute(params![p])?;
        }
        Ok(removed)
    }

    pub fn all_tracks(&self) -> Result<Vec<LibraryTrack>, LyraError> {
        self.query_tracks("SELECT * FROM tracks ORDER BY artist, album, track_no", [])
    }

    /// FTS5 prefix search over title/artist/album. Falls back to LIKE on
    /// queries that aren't valid FTS syntax (user typing punctuation).
    pub fn search(&self, query: &str) -> Result<Vec<LibraryTrack>, LyraError> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|w| format!("\"{}\"*", w.replace('"', "")))
            .collect();
        let fts = terms.join(" ");
        if fts.is_empty() {
            return self.all_tracks();
        }
        match self.query_tracks(
            "SELECT t.* FROM tracks t
             JOIN tracks_fts f ON t.id = f.rowid
             WHERE tracks_fts MATCH ?1
             ORDER BY rank",
            params![fts],
        ) {
            Ok(v) => Ok(v),
            Err(_) => self.query_tracks(
                "SELECT * FROM tracks WHERE
                     title LIKE ?1 OR artist LIKE ?1 OR album LIKE ?1
                 ORDER BY artist, album, track_no",
                params![format!("%{query}%")],
            ),
        }
    }

    fn query_tracks(
        &self,
        sql: &str,
        p: impl rusqlite::Params,
    ) -> Result<Vec<LibraryTrack>, LyraError> {
        self.note_query();
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt
            .query_map(p, |r| {
                let fmt: String = r.get("format")?;
                Ok(LibraryTrack {
                    path: r.get("path")?,
                    title: r.get("title")?,
                    artist: r.get("artist")?,
                    album: r.get("album")?,
                    album_artist: r.get("album_artist")?,
                    genre: r.get("genre")?,
                    year: r.get::<_, Option<i64>>("year")?.map(|v| v as u32),
                    track_number: r.get::<_, Option<i64>>("track_no")?.map(|v| v as u32),
                    duration_secs: r.get("duration_secs")?,
                    format: format_from(&fmt),
                    codec: r.get::<_, Option<String>>("codec")?.unwrap_or_default(),
                    sample_rate: r.get::<_, Option<i64>>("sample_rate")?.map(|v| v as u32),
                    channels: r.get::<_, Option<i64>>("channels")?.map(|v| v as u32),
                    bits_per_sample: r.get::<_, Option<i64>>("bit_depth")?.map(|v| v as u32),
                    artwork_hash: r
                        .get::<_, Option<String>>("artwork_hash")?
                        .filter(|s| !s.is_empty()), // '' = "checked, none"
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn add_source(&self, uri: &str, kind: &str) -> Result<(), LyraError> {
        self.note_query();
        self.conn.execute(
            "INSERT OR IGNORE INTO sources (uri, kind) VALUES (?1, ?2)",
            params![uri, kind],
        )?;
        Ok(())
    }

    /// Opaque key/value slot (the `settings` table). The remote scanner
    /// keeps its resume cursor here — no schema migration for scanner
    /// state, just a namespaced key.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>, LyraError> {
        self.note_query();
        let v: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                params![key],
                |r| r.get(0),
            )
            .ok();
        Ok(v)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), LyraError> {
        self.note_query();
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Drop a setting. The remote scanner clears its resume cursor on a
    /// completed pass — a stale end-of-list cursor would make every later
    /// scan a no-op and hide remote changes.
    pub fn clear_setting(&self, key: &str) -> Result<(), LyraError> {
        self.note_query();
        self.conn
            .execute("DELETE FROM settings WHERE key=?1", params![key])?;
        Ok(())
    }

    /// Like [`Library::prune_missing`], but scoped to one root prefix —
    /// a remote scan must never delete local rows (or another host's).
    /// Same empty-list guard: an empty scan is a failure, not a deletion.
    pub fn prune_prefix(&self, alive: &[String], prefix: &str) -> Result<usize, LyraError> {
        if alive.is_empty() {
            return Ok(0);
        }
        self.note_query();
        let existing: Vec<String> = self
            .conn
            .prepare("SELECT path FROM tracks")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let alive: std::collections::HashSet<&str> = alive.iter().map(String::as_str).collect();
        let stale: Vec<String> = existing
            .into_iter()
            .filter(|p| p.starts_with(prefix) && !alive.contains(p.as_str()))
            .collect();
        let mut removed = 0;
        self.note_query();
        let mut stmt = self.conn.prepare("DELETE FROM tracks WHERE path=?1")?;
        for p in &stale {
            self.note_query();
            removed += stmt.execute(params![p])?;
        }
        Ok(removed)
    }

    pub fn sources(&self) -> Result<Vec<Source>, LyraError> {
        self.note_query();
        let mut stmt = self.conn.prepare("SELECT uri, kind FROM sources")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Source {
                    uri: r.get(0)?,
                    kind: r.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

/// Outcome of one `sync_dir` pass — surfaced to the UI as scan progress.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct SyncStats {
    pub walked: u64,
    pub probed: u64,  // files actually re-probed (new or mtime-changed)
    pub skipped: u64, // unchanged — mtime matched the DB row
    pub pruned: u64,  // rows removed for vanished files
    pub elapsed_ms: u64,
}

impl Library {
    /// Incremental sync: walk `dir`, probe only new/mtime-changed audio
    /// files, upsert them, then prune rows for files that vanished.
    /// `dir` is also registered as a `local` source root.
    pub fn sync_dir(&self, dir: &Path) -> Result<SyncStats, LyraError> {
        let t0 = std::time::Instant::now();
        let mut stats = SyncStats::default();
        self.add_source(&format!("file://{}", dir.display()), "local")?;

        let mut alive = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        // Folder-art pass: image siblings collected during the walk,
        // assigned after to tracks whose embedded check found nothing.
        let mut art_candidates: std::collections::HashMap<PathBuf, Vec<(u8, PathBuf)>> =
            std::collections::HashMap::new();
        let mut needing_art: Vec<PathBuf> = Vec::new();
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d)? {
                let entry = entry?;
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if let Some(rank) = folder_art_rank(&d, &p) {
                    art_candidates.entry(d.clone()).or_default().push((rank, p));
                    continue;
                }
                let ext = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default();
                if !lyra_formats::is_audio_ext(&ext) {
                    continue;
                }
                stats.walked += 1;
                let path_str = p.display().to_string();
                let meta = std::fs::metadata(&p).ok();
                let mtime = meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let size = meta.map(|m| m.len() as i64).unwrap_or(0);
                alive.push(path_str.clone());

                if self.needs_scan(&path_str, mtime)? {
                    let (mut track, art) = lyra_formats::probe_track_full(&p);
                    track.artwork_hash = art
                        .and_then(|a| self.ingest_artwork(&a.data, a.mime, "embedded"))
                        .or_else(|| Some(String::new())); // '' = checked, none
                    if track.artwork_hash.as_deref() == Some("") {
                        needing_art.push(p);
                    }
                    self.upsert_track(&track, mtime, size)?;
                    stats.probed += 1;
                } else if self.needs_art(&path_str)? {
                    // Row predates art support — tags-only re-check.
                    let hash = lyra_formats::read_art(&p)
                        .and_then(|a| self.ingest_artwork(&a.data, a.mime, "embedded"));
                    self.set_track_artwork(&path_str, hash.as_deref())?;
                    if hash.is_none() {
                        needing_art.push(p);
                    }
                    stats.skipped += 1;
                } else {
                    stats.skipped += 1;
                }
            }
        }
        stats.pruned = self.prune_missing(&alive)? as u64;
        self.assign_folder_art(&art_candidates, needing_art, dir)?;
        stats.elapsed_ms = t0.elapsed().as_millis() as u64;
        Ok(stats)
    }

    /// Folder-art fallback for tracks whose embedded check found nothing:
    /// best-ranked image in the track's dir, then ≤2 ancestors up.
    /// Also retried for ''-hash rows under the scanned root so a
    /// later-added cover.jpg gets picked up on the next sync.
    fn assign_folder_art(
        &self,
        candidates: &std::collections::HashMap<PathBuf, Vec<(u8, PathBuf)>>,
        mut needing: Vec<PathBuf>,
        root: &Path,
    ) -> Result<(), LyraError> {
        if self.artwork_dir.is_none() {
            return Ok(());
        }
        // Stale '' rows under this scan root get another folder shot.
        self.note_query();
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM tracks WHERE artwork_hash=''")?;
        let stale: Vec<String> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let root_str = format!("{}/", root.display());
        for s in stale {
            if s.starts_with(&root_str) || s == root.display().to_string() {
                needing.push(PathBuf::from(s));
            }
        }
        for track_path in needing {
            // Track-stem override: <stem>.{jpg,png,…} beside the file wins.
            let stem_hit = track_path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|stem| {
                    track_path.parent().and_then(|d| {
                        ["jpg", "jpeg", "png", "webp"]
                            .iter()
                            .map(|e| d.join(format!("{stem}.{e}")))
                            .find(|p| p.is_file())
                    })
                });
            let hit = stem_hit.or_else(|| {
                let mut d = track_path.parent();
                for _ in 0..3 {
                    let dir = d?;
                    if let Some(cands) = candidates.get(dir) {
                        if let Some((_, p)) = cands.iter().min_by_key(|(r, _)| r) {
                            return Some(p.clone());
                        }
                    }
                    d = dir.parent();
                }
                None
            });
            if let Some(img_path) = hit {
                if let Ok(bytes) = std::fs::read(&img_path) {
                    let mime = match img_path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|s| s.to_lowercase())
                        .as_deref()
                    {
                        Some("png") => "image/png",
                        Some("webp") => "image/webp",
                        Some("gif") => "image/gif",
                        _ => "image/jpeg",
                    };
                    if let Some(h) = self.ingest_artwork(&bytes, mime, "file") {
                        let _ = self.set_track_artwork(&track_path.display().to_string(), Some(&h));
                    }
                }
            }
        }
        Ok(())
    }

    /// Sync an explicit list of files (NSOpenPanel multi-pick). Same
    /// probe/upsert path as sync_dir, but never prunes — a file pick isn't
    /// authoritative for what's gone from disk.
    pub fn sync_files(&self, files: &[PathBuf]) -> Result<SyncStats, LyraError> {
        let t0 = std::time::Instant::now();
        let mut stats = SyncStats::default();
        for p in files {
            if !p.is_file() {
                continue;
            }
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            if !lyra_formats::is_audio_ext(&ext) {
                continue;
            }
            stats.walked += 1;
            let path_str = p.display().to_string();
            let meta = std::fs::metadata(p).ok();
            let mtime = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let size = meta.map(|m| m.len() as i64).unwrap_or(0);
            let mut hash: Option<String> = None;
            // Only tracks whose embedded art was actually re-checked this
            // pass may take folder art — skipped rows keep theirs.
            let mut checked_embedded = false;
            if self.needs_scan(&path_str, mtime)? {
                let (mut track, art) = lyra_formats::probe_track_full(p);
                hash = art.and_then(|a| self.ingest_artwork(&a.data, a.mime, "embedded"));
                track.artwork_hash = hash.clone().or_else(|| Some(String::new()));
                self.upsert_track(&track, mtime, size)?;
                checked_embedded = true;
                stats.probed += 1;
            } else if self.needs_art(&path_str)? {
                hash = lyra_formats::read_art(p)
                    .and_then(|a| self.ingest_artwork(&a.data, a.mime, "embedded"));
                self.set_track_artwork(&path_str, hash.as_deref())?;
                checked_embedded = true;
                stats.skipped += 1;
            } else {
                stats.skipped += 1;
            }
            // Folder art: check the picked file's parent dir once.
            if checked_embedded && hash.is_none() {
                if let Some(d) = p.parent() {
                    let mut cands = std::collections::HashMap::new();
                    if let Ok(rd) = std::fs::read_dir(d) {
                        for e in rd.flatten() {
                            if let Some(r) = folder_art_rank(d, &e.path()) {
                                cands
                                    .entry(d.to_path_buf())
                                    .or_insert_with(Vec::new)
                                    .push((r, e.path()));
                            }
                        }
                    }
                    self.assign_folder_art(&cands, vec![p.clone()], d)?;
                }
            }
        }
        stats.elapsed_ms = t0.elapsed().as_millis() as u64;
        Ok(stats)
    }

    /// Store waveform peaks for a track (import-time). `peaks` is the
    /// min/max pair per display bucket from lyra-viz's WaveformPeaks.
    pub fn upsert_peaks(
        &self,
        path: &str,
        sample_rate: f32,
        samples_per_bucket: u64,
        peaks: &[(f32, f32)],
    ) -> Result<(), LyraError> {
        let mut blob = Vec::with_capacity(peaks.len() * 8);
        for (lo, hi) in peaks {
            blob.extend_from_slice(&lo.to_le_bytes());
            blob.extend_from_slice(&hi.to_le_bytes());
        }
        self.note_query();
        self.conn.execute(
            "INSERT INTO waveform_peaks (path, sample_rate, samples_per_bucket, peaks)
             VALUES (?1,?2,?3,?4)
             ON CONFLICT(path) DO UPDATE SET
                 sample_rate=excluded.sample_rate,
                 samples_per_bucket=excluded.samples_per_bucket,
                 peaks=excluded.peaks",
            params![path, sample_rate, samples_per_bucket as i64, blob],
        )?;
        Ok(())
    }

    /// Fetch stored peaks → (sample_rate, samples_per_bucket, pairs).
    pub fn peaks_for(&self, path: &str) -> Result<Option<WavePeaks>, LyraError> {
        self.note_query();
        let row = self
            .conn
            .query_row(
                "SELECT sample_rate, samples_per_bucket, peaks FROM waveform_peaks WHERE path=?1",
                params![path],
                |r| {
                    Ok((
                        r.get::<_, f32>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Vec<u8>>(2)?,
                    ))
                },
            )
            .ok();
        let Some((rate, spb, blob)) = row else {
            return Ok(None);
        };
        let mut peaks = Vec::with_capacity(blob.len() / 8);
        for pair in blob.as_chunks::<8>().0 {
            let lo = f32::from_le_bytes(pair[..4].try_into().unwrap());
            let hi = f32::from_le_bytes(pair[4..].try_into().unwrap());
            peaks.push((lo, hi));
        }
        Ok(Some((rate, spb as u64, peaks)))
    }

    // ── Song-map registry (track_maps, v4) ──────────────────────────────

    /// Upsert a map row (keyed on audio_hash). Re-running the pipeline at
    /// a new `pipeline_ver` replaces the row; the old .lyramap is orphaned
    /// by content hash, never mutated in place.
    pub fn upsert_map(&self, m: &TrackMapRow) -> Result<(), LyraError> {
        self.note_query();
        self.conn.execute(
            "INSERT INTO track_maps
                 (audio_hash, map_path, pipeline_ver, status, overall_conf, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(audio_hash) DO UPDATE SET
                 map_path=excluded.map_path,
                 pipeline_ver=excluded.pipeline_ver,
                 status=excluded.status,
                 overall_conf=excluded.overall_conf,
                 updated_at=excluded.updated_at",
            params![
                m.audio_hash,
                m.map_path,
                m.pipeline_ver,
                m.status,
                m.overall_conf,
                m.updated_at,
            ],
        )?;
        Ok(())
    }

    /// Map row for one recording, if analyzed.
    pub fn map_for_hash(&self, hash: &str) -> Result<Option<TrackMapRow>, LyraError> {
        self.note_query();
        let row = self
            .conn
            .query_row(
                "SELECT audio_hash, map_path, pipeline_ver, status, overall_conf, updated_at
                 FROM track_maps WHERE audio_hash=?1",
                params![hash],
                |r| {
                    Ok(TrackMapRow {
                        audio_hash: r.get(0)?,
                        map_path: r.get(1)?,
                        pipeline_ver: r.get(2)?,
                        status: r.get(3)?,
                        overall_conf: r.get(4)?,
                        updated_at: r.get(5)?,
                    })
                },
            )
            .ok();
        Ok(row)
    }

    /// All maps in a tier — drives the "ready for strum mode" queue
    /// (`status >= 'chords'`) and the regeneration backlog.
    pub fn maps_by_status(&self, status: &str) -> Result<Vec<TrackMapRow>, LyraError> {
        self.note_query();
        let mut stmt = self.conn.prepare(
            "SELECT audio_hash, map_path, pipeline_ver, status, overall_conf, updated_at
             FROM track_maps WHERE status=?1 ORDER BY updated_at DESC",
        )?;
        let rows = stmt
            .query_map(params![status], |r| {
                Ok(TrackMapRow {
                    audio_hash: r.get(0)?,
                    map_path: r.get(1)?,
                    pipeline_ver: r.get(2)?,
                    status: r.get(3)?,
                    overall_conf: r.get(4)?,
                    updated_at: r.get(5)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Join a track row to its analyzed bytes (NULL = never hashed).
    pub fn set_track_audio_hash(&self, path: &str, hash: &str) -> Result<(), LyraError> {
        self.note_query();
        self.conn.execute(
            "UPDATE tracks SET audio_hash=?1 WHERE path=?2",
            params![hash, path],
        )?;
        Ok(())
    }

    pub fn track_audio_hash(&self, path: &str) -> Result<Option<String>, LyraError> {
        self.note_query();
        let h: Option<String> = self
            .conn
            .query_row(
                "SELECT audio_hash FROM tracks WHERE path=?1",
                params![path],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        Ok(h)
    }
}

/// One `track_maps` row (§4.5): the content-addressed registry entry for a
/// generated `.lyramap`. `status` is one of
/// pending|grid|chords|notes|done|unsupported|failed.
#[derive(Debug, Clone)]
pub struct TrackMapRow {
    pub audio_hash: String,
    pub map_path: String,
    pub pipeline_ver: String,
    pub status: String,
    pub overall_conf: Option<f32>,
    pub updated_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(path: &str, title: &str, artist: &str) -> LibraryTrack {
        LibraryTrack {
            path: path.into(),
            title: Some(title.into()),
            artist: Some(artist.into()),
            album: Some("Alb".into()),
            album_artist: None,
            genre: None,
            year: Some(1977),
            track_number: Some(1),
            duration_secs: Some(180.0),
            format: AudioFormat::Flac,
            codec: "flac".into(),
            sample_rate: Some(44100),
            channels: Some(2),
            bits_per_sample: Some(16),
            artwork_hash: None,
        }
    }

    #[test]
    fn upsert_search_prune() {
        let lib = Library::open_memory().unwrap();
        lib.upsert_track(
            &track("/a/one.flac", "Scarlet Begonias", "Grateful Dead"),
            100,
            10e6 as i64,
        )
        .unwrap();
        lib.upsert_track(
            &track("/a/two.flac", "Fire on the Mountain", "Grateful Dead"),
            100,
            10e6 as i64,
        )
        .unwrap();
        assert_eq!(lib.all_tracks().unwrap().len(), 2);
        assert_eq!(lib.all_tracks().unwrap()[0].format, AudioFormat::Flac);

        // FTS prefix search
        assert_eq!(lib.search("scarlet").unwrap().len(), 1);
        assert_eq!(lib.search("grateful").unwrap().len(), 2);

        // incremental: same mtime → skip, new mtime → rescan
        assert!(!lib.needs_scan("/a/one.flac", 100).unwrap());
        assert!(lib.needs_scan("/a/one.flac", 200).unwrap());
        assert!(lib.needs_scan("/a/new.flac", 1).unwrap());

        // upsert same path replaces, doesn't duplicate
        lib.upsert_track(
            &track("/a/one.flac", "Scarlet Begonias", "Grateful Dead"),
            200,
            10e6 as i64,
        )
        .unwrap();
        assert_eq!(lib.all_tracks().unwrap().len(), 2);

        // prune keeps alive, drops stale; empty list never prunes
        assert_eq!(lib.prune_missing(&[]).unwrap(), 0);
        assert_eq!(lib.prune_missing(&["/a/one.flac".to_string()]).unwrap(), 1);
        assert_eq!(lib.all_tracks().unwrap().len(), 1);
    }

    /// 4×4 PNG built at test time — no fixture files needed.
    fn test_png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 60, 30, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    #[test]
    fn artwork_ingest_writes_cache_and_row() {
        // Unique dir per invocation (pid + nanos): parallel tests and
        // consecutive repeat-run sweeps must never share an artwork cache.
        let dir = std::env::temp_dir().join(format!(
            "lyra-art-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let lib = Library::open(&dir.join("lib.db")).unwrap(); // dir derives artwork/

        let png = test_png();
        let h = lib.ingest_artwork(&png, "image/png", "embedded").unwrap();
        let p = dir.join("artwork").join(&h[..2]).join(&h);
        assert!(p.join("full.png").is_file());
        assert!(p.join("64.jpg").is_file());
        assert!(p.join("256.jpg").is_file());
        // Same bytes dedupe to the same hash; junk never caches.
        assert_eq!(lib.ingest_artwork(&png, "image/png", "file").unwrap(), h);
        assert!(lib
            .ingest_artwork(b"not an image", "image/png", "embedded")
            .is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_roundtrip_and_clear() {
        let lib = Library::open_memory().unwrap();
        assert_eq!(lib.get_setting("k").unwrap(), None);
        lib.set_setting("k", "v1").unwrap();
        assert_eq!(lib.get_setting("k").unwrap().as_deref(), Some("v1"));
        lib.set_setting("k", "v2").unwrap();
        assert_eq!(lib.get_setting("k").unwrap().as_deref(), Some("v2"));
        lib.clear_setting("k").unwrap();
        assert_eq!(lib.get_setting("k").unwrap(), None);
    }

    #[test]
    fn prune_prefix_is_scoped() {
        let lib = Library::open_memory().unwrap();
        for p in [
            "/local/a.flac",
            "sftp://h/m/keep.flac",
            "sftp://h/m/stale.flac",
        ] {
            lib.upsert_track(&track(p, "T", "A"), 1, 1).unwrap();
        }
        // empty alive never prunes; scoped prune keeps other roots
        assert_eq!(lib.prune_prefix(&[], "sftp://h/m").unwrap(), 0);
        assert_eq!(
            lib.prune_prefix(&["sftp://h/m/keep.flac".to_string()], "sftp://h/m")
                .unwrap(),
            1
        );
        let paths: Vec<_> = lib
            .all_tracks()
            .unwrap()
            .into_iter()
            .map(|t| t.path)
            .collect();
        assert!(paths.contains(&"/local/a.flac".to_string()));
        assert!(paths.contains(&"sftp://h/m/keep.flac".to_string()));
    }

    #[test]
    fn artwork_hash_markers() {
        let lib = Library::open_memory().unwrap();
        lib.upsert_track(&track("/a/one.flac", "One", "A"), 100, 100)
            .unwrap();
        // NULL → needs art check; '' → checked, none; hash → done.
        assert!(lib.needs_art("/a/one.flac").unwrap());
        lib.set_track_artwork("/a/one.flac", None).unwrap();
        assert!(!lib.needs_art("/a/one.flac").unwrap());
        assert_eq!(lib.all_tracks().unwrap()[0].artwork_hash, None); // '' → None out
        lib.set_track_artwork("/a/one.flac", Some("abc123"))
            .unwrap();
        assert_eq!(
            lib.all_tracks().unwrap()[0].artwork_hash,
            Some("abc123".into())
        );
    }

    #[test]
    fn waveform_peaks_roundtrip_and_cascade() {
        let lib = Library::open_memory().unwrap();
        lib.upsert_track(
            &track("/a/one.flac", "Scarlet Begonias", "Grateful Dead"),
            100,
            10e6 as i64,
        )
        .unwrap();
        let peaks = vec![(-0.5f32, 0.9f32), (-0.2, 0.3), (0.0, 0.0)];
        lib.upsert_peaks("/a/one.flac", 44100.0, 735, &peaks)
            .unwrap();

        let (rate, spb, back) = lib.peaks_for("/a/one.flac").unwrap().unwrap();
        assert_eq!(rate, 44100.0);
        assert_eq!(spb, 735);
        assert_eq!(back, peaks);

        // upsert overwrites; unknown path yields None; FK must exist
        lib.upsert_peaks("/a/one.flac", 44100.0, 735, &peaks[..1])
            .unwrap();
        assert_eq!(lib.peaks_for("/a/one.flac").unwrap().unwrap().2.len(), 1);
        assert!(lib.peaks_for("/a/ghost.flac").unwrap().is_none());
        assert!(lib
            .upsert_peaks("/a/ghost.flac", 44100.0, 735, &peaks)
            .is_err());

        // pruning the track cascades its peaks
        lib.upsert_track(&track("/a/two.flac", "Fire", "GD"), 100, 1)
            .unwrap();
        lib.upsert_peaks("/a/two.flac", 44100.0, 735, &peaks)
            .unwrap();
        lib.prune_missing(&["/a/two.flac".to_string()]).unwrap();
        assert!(lib.peaks_for("/a/one.flac").unwrap().is_none());
        assert!(lib.peaks_for("/a/two.flac").unwrap().is_some());
    }

    #[test]
    fn sync_dir_is_incremental() {
        let dir = Path::new("/Users/adityabalakrishnan/Music/Lyra-Test");
        if !dir.exists() {
            return; // fixture absent on this machine
        }
        let lib = Library::open_memory().unwrap();

        let s1 = lib.sync_dir(dir).unwrap();
        assert_eq!(s1.walked, 3);
        assert_eq!(s1.probed, 3);
        assert_eq!(s1.skipped, 0);
        assert_eq!(lib.all_tracks().unwrap().len(), 3);

        // Second pass: mtimes unchanged → zero probing.
        let s2 = lib.sync_dir(dir).unwrap();
        assert_eq!(s2.walked, 3);
        assert_eq!(s2.probed, 0);
        assert_eq!(s2.skipped, 3);

        // Synced rows carry real stream info (fixture files are untagged,
        // so assert on codec/format rather than FTS title hits).
        let rows = lib.all_tracks().unwrap();
        assert!(rows.iter().all(|t| !t.codec.is_empty()));
        assert!(rows.iter().any(|t| t.format == AudioFormat::Aiff));
        assert_eq!(lib.sources().unwrap().len(), 1);
    }

    #[test]
    fn track_maps_registry() {
        let lib = Library::open_memory().unwrap();
        let row = TrackMapRow {
            audio_hash: "deadbeef".into(),
            map_path: "maps/deadbeef.lyramap".into(),
            pipeline_ver: "mapgen-0.1".into(),
            status: "done".into(),
            overall_conf: Some(0.82),
            updated_at: 1700000000,
        };
        assert!(lib.map_for_hash("deadbeef").unwrap().is_none());
        lib.upsert_map(&row).unwrap();
        let back = lib.map_for_hash("deadbeef").unwrap().unwrap();
        assert_eq!(back.map_path, "maps/deadbeef.lyramap");
        assert_eq!(back.status, "done");
        assert_eq!(back.overall_conf, Some(0.82));

        // Re-run at a new pipeline version replaces the row.
        let mut row2 = row.clone();
        row2.status = "grid".into();
        row2.pipeline_ver = "mapgen-0.2".into();
        lib.upsert_map(&row2).unwrap();
        assert_eq!(
            lib.map_for_hash("deadbeef").unwrap().unwrap().status,
            "grid"
        );
        assert!(lib.maps_by_status("done").unwrap().is_empty());
        assert_eq!(lib.maps_by_status("grid").unwrap().len(), 1);

        // Track ↔ map join via audio_hash (NULL = never hashed).
        lib.upsert_track(&track("/a/one.flac", "One", "A"), 100, 100)
            .unwrap();
        assert_eq!(lib.track_audio_hash("/a/one.flac").unwrap(), None);
        lib.set_track_audio_hash("/a/one.flac", "deadbeef").unwrap();
        assert_eq!(
            lib.track_audio_hash("/a/one.flac").unwrap(),
            Some("deadbeef".into())
        );
    }
}
