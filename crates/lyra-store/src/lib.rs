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
use rusqlite_migration::{M, Migrations};
use std::path::Path;

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
}

#[derive(Debug, Clone)]
pub struct Source {
    pub uri: String,
    pub kind: String,
}

impl Library {
    pub fn open(path: &Path) -> Result<Self, LyraError> {
        let mut conn = Connection::open(path)?;
        Self::init(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn open_memory() -> Result<Self, LyraError> {
        let mut conn = Connection::open_in_memory()?;
        Self::init(&mut conn)?;
        Ok(Self { conn })
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
        self.conn.execute(
            "INSERT INTO tracks (path, title, artist, album, album_artist,
                 genre, year, track_no, duration_secs, format, codec,
                 sample_rate, channels, bit_depth, size_bytes, mtime)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
             ON CONFLICT(path) DO UPDATE SET
                 title=excluded.title, artist=excluded.artist,
                 album=excluded.album, album_artist=excluded.album_artist,
                 genre=excluded.genre, year=excluded.year,
                 track_no=excluded.track_no,
                 duration_secs=excluded.duration_secs,
                 format=excluded.format, codec=excluded.codec,
                 sample_rate=excluded.sample_rate,
                 channels=excluded.channels, bit_depth=excluded.bit_depth,
                 size_bytes=excluded.size_bytes, mtime=excluded.mtime",
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
            ],
        )?;
        Ok(())
    }

    /// Does this file need (re)probing? True if unseen or mtime changed.
    pub fn needs_scan(&self, path: &str, mtime: i64) -> Result<bool, LyraError> {
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

    /// Drop rows whose path isn't in `alive` — the post-scan prune.
    /// Refuses to run on an empty list: an empty scan is almost always a
    /// scan failure, not a deleted library.
    pub fn prune_missing(&self, alive: &[String]) -> Result<usize, LyraError> {
        if alive.is_empty() {
            return Ok(0);
        }
        let existing: Vec<String> = self
            .conn
            .prepare("SELECT path FROM tracks")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let alive: std::collections::HashSet<&str> =
            alive.iter().map(String::as_str).collect();
        let stale: Vec<String> = existing
            .into_iter()
            .filter(|p| !alive.contains(p.as_str()))
            .collect();
        let mut removed = 0;
        let mut stmt = self.conn.prepare("DELETE FROM tracks WHERE path=?1")?;
        for p in &stale {
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
                    track_number: r
                        .get::<_, Option<i64>>("track_no")?
                        .map(|v| v as u32),
                    duration_secs: r.get("duration_secs")?,
                    format: format_from(&fmt),
                    codec: r.get::<_, Option<String>>("codec")?.unwrap_or_default(),
                    sample_rate: r
                        .get::<_, Option<i64>>("sample_rate")?
                        .map(|v| v as u32),
                    channels: r
                        .get::<_, Option<i64>>("channels")?
                        .map(|v| v as u32),
                    bits_per_sample: r
                        .get::<_, Option<i64>>("bit_depth")?
                        .map(|v| v as u32),
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn add_source(&self, uri: &str, kind: &str) -> Result<(), LyraError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO sources (uri, kind) VALUES (?1, ?2)",
            params![uri, kind],
        )?;
        Ok(())
    }

    pub fn sources(&self) -> Result<Vec<Source>, LyraError> {
        let mut stmt = self.conn.prepare("SELECT uri, kind FROM sources")?;
        let rows = stmt
            .query_map([], |r| Ok(Source { uri: r.get(0)?, kind: r.get(1)? }))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

/// Outcome of one `sync_dir` pass — surfaced to the UI as scan progress.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct SyncStats {
    pub walked: u64,
    pub probed: u64,   // files actually re-probed (new or mtime-changed)
    pub skipped: u64,  // unchanged — mtime matched the DB row
    pub pruned: u64,   // rows removed for vanished files
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
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d)? {
                let entry = entry?;
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
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
                    let track = lyra_formats::probe_track(&p);
                    self.upsert_track(&track, mtime, size)?;
                    stats.probed += 1;
                } else {
                    stats.skipped += 1;
                }
            }
        }
        stats.pruned = self.prune_missing(&alive)? as u64;
        stats.elapsed_ms = t0.elapsed().as_millis() as u64;
        Ok(stats)
    }
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
        }
    }

    #[test]
    fn upsert_search_prune() {
        let lib = Library::open_memory().unwrap();
        lib.upsert_track(&track("/a/one.flac", "Scarlet Begonias", "Grateful Dead"), 100, 10e6 as i64)
            .unwrap();
        lib.upsert_track(&track("/a/two.flac", "Fire on the Mountain", "Grateful Dead"), 100, 10e6 as i64)
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
        lib.upsert_track(&track("/a/one.flac", "Scarlet Begonias", "Grateful Dead"), 200, 10e6 as i64)
            .unwrap();
        assert_eq!(lib.all_tracks().unwrap().len(), 2);

        // prune keeps alive, drops stale; empty list never prunes
        assert_eq!(lib.prune_missing(&[]).unwrap(), 0);
        assert_eq!(
            lib.prune_missing(&["/a/one.flac".to_string()]).unwrap(),
            1
        );
        assert_eq!(lib.all_tracks().unwrap().len(), 1);
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
}
