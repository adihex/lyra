//! `RemoteScanner`: walk a remote tree → filter audio → probe headers →
//! upsert into `lyra-store`. Batched (cursor checkpoint per batch),
//! cancellable (checked between files), resumable (cursor in the store's
//! `settings` table — no migration).
//!
//! Probing is header-only: the probe reads at most `cap_bytes` from offset
//! 0 through the `ByteSource` (a cached SFTP read in production) and hands
//! that prefix to `lyra-formats`. Tags/stream info degrade gracefully on
//! truncated prefixes (no duration instead of a failed scan) — the row is
//! always written so the next scan can refine it.

use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use lyra_core::{LibraryTrack, LyraError};
use lyra_store::Library;

use super::{ByteSource, RemoteEntry, RemoteProfile, SftpSource};

// ── seams ────────────────────────────────────────────────────────────────

/// Cooperative cancellation. Checked between files; never mid-read.
#[derive(Clone, Default)]
pub struct Cancel {
    flag: Arc<AtomicBool>,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    fn check(&self) -> Result<(), Cancelled> {
        match self.is_cancelled() {
            true => Err(Cancelled),
            false => Ok(()),
        }
    }
}

struct Cancelled;

/// Lists a remote tree. Live impl ([`SftpWalk`]) recurses over SFTP;
/// tests substitute an in-memory list.
pub trait RemoteWalk: Send + Sync {
    fn walk(&self, cancel: &Cancel) -> Result<Vec<RemoteEntry>, LyraError>;
}

/// Opens one remote file for header reads.
pub trait RemoteOpen: Send + Sync {
    fn open(&self, remote_path: &str) -> Result<Arc<dyn ByteSource>, LyraError>;
}

/// Turns header bytes into a library row.
pub struct ProbeHint {
    pub remote_path: String,
    pub size: u64,
    pub mtime: i64,
}

pub struct ProbedFile {
    pub track: LibraryTrack,
    pub mtime: i64,
    pub size_bytes: i64,
    pub art: Option<lyra_formats::EmbeddedArt>,
}

pub trait RemoteProbe: Send + Sync {
    fn probe(&self, src: &dyn ByteSource, hint: &ProbeHint) -> Result<ProbedFile, LyraError>;
}

// ── audio filter ─────────────────────────────────────────────────────────

/// Extensions the remote scanner accepts. `lyra-formats` plus the
/// container aliases it doesn't claim (`aac`/`alac` ride `m4a`, but
/// real-world drops use the bare extensions).
pub fn is_remote_audio(remote_path: &str) -> bool {
    let ext = Path::new(remote_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    lyra_formats::is_audio_ext(&ext) || matches!(ext.as_str(), "aac" | "alac" | "aifc")
}

// ── header probe ─────────────────────────────────────────────────────────

/// Prefix-reader probe: at most `cap_bytes` from offset 0, then the
/// standard `lyra-formats` file probe over a temp copy (extension kept so
/// magic-fallback still applies). Cover bytes ride along for the store to
/// ingest.
pub struct HeaderProbe {
    pub cap_bytes: u64,
}

impl Default for HeaderProbe {
    fn default() -> Self {
        Self { cap_bytes: 2 << 20 }
    }
}

impl RemoteProbe for HeaderProbe {
    fn probe(&self, src: &dyn ByteSource, hint: &ProbeHint) -> Result<ProbedFile, LyraError> {
        let ext = Path::new(&hint.remote_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin");
        let want = src.len().min(hint.size).min(self.cap_bytes) as usize;
        let mut prefix = vec![0u8; want];
        let mut got = 0;
        while got < want {
            let n = src.read_at(got as u64, &mut prefix[got..])?;
            if n == 0 {
                break;
            }
            got += n;
        }
        prefix.truncate(got);

        let tmp = std::env::temp_dir().join(format!(
            "lyra-probe-{}-{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            ext
        ));
        std::fs::write(&tmp, &prefix)?;
        let (track, art) = lyra_formats::probe_track_full(&tmp);
        let _ = std::fs::remove_file(&tmp);
        Ok(ProbedFile {
            track,
            mtime: hint.mtime,
            size_bytes: hint.size as i64,
            art,
        })
    }
}

// ── live walker + opener ─────────────────────────────────────────────────

/// Recursive SFTP listing under the profile root. Symlinks skipped (loop
/// safety); `.`/`..` skipped; cancel checked per directory.
pub struct SftpWalk {
    backend: super::sftp::Ssh2Backend,
    root: String,
}

impl SftpWalk {
    pub fn new(profile: &RemoteProfile, auth: &super::AuthMethod) -> Self {
        Self {
            backend: super::sftp::Ssh2Backend::new(profile).with_auth(auth.clone()),
            root: profile.root_path.clone(),
        }
    }
}

impl RemoteWalk for SftpWalk {
    fn walk(&self, cancel: &Cancel) -> Result<Vec<RemoteEntry>, LyraError> {
        let (_sess, sftp) = self.backend.connect()?;
        let mut entries = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            cancel
                .check()
                .map_err(|_| LyraError::Remote("scan cancelled".into()))?;
            let listing = sftp
                .readdir(Path::new(&dir))
                .map_err(|e| LyraError::Remote(format!("readdir {dir}: {e}")))?;
            for (name, stat) in listing {
                let name = name.to_string_lossy();
                if name == "." || name == ".." {
                    continue;
                }
                let full = format!("{}/{}", dir.trim_end_matches('/'), name);
                if stat.file_type() == ssh2::FileType::Directory {
                    stack.push(full);
                    continue;
                }
                if stat.file_type() == ssh2::FileType::Symlink {
                    continue;
                }
                entries.push(RemoteEntry {
                    path: full,
                    size: stat.size.unwrap_or(0),
                    mtime: stat.mtime.unwrap_or(0) as i64,
                });
            }
        }
        Ok(entries)
    }
}

/// Opens live [`SftpSource`]s — one session per file, each cached above.
pub struct SftpOpener {
    profile: RemoteProfile,
    auth: super::AuthMethod,
}

impl SftpOpener {
    pub fn new(profile: &RemoteProfile, auth: &super::AuthMethod) -> Self {
        Self {
            profile: profile.clone(),
            auth: auth.clone(),
        }
    }
}

impl RemoteOpen for SftpOpener {
    fn open(&self, remote_path: &str) -> Result<Arc<dyn ByteSource>, LyraError> {
        Ok(Arc::new(SftpSource::open_with_auth(
            &self.profile,
            remote_path,
            &self.auth,
        )?))
    }
}

// ── exec transport: hosts with no sftp subsystem ─────────────────────────
// Dropbear/restricted sshds answer exec channels but not sftp — the whole
// walk is one `find` round-trip, reads ride `ssh dd` (the v0 bootstrap).

/// One-shot `ssh find` listing of the profile root.
pub struct ExecWalk {
    profile: RemoteProfile,
}

impl ExecWalk {
    pub fn new(profile: &RemoteProfile) -> Self {
        Self { profile: profile.clone() }
    }
}

impl RemoteWalk for ExecWalk {
    fn walk(&self, _cancel: &Cancel) -> Result<Vec<RemoteEntry>, LyraError> {
        super::scan(&self.profile, &self.profile.root_path)
    }
}

/// Per-file `ssh dd` reads — chatty, so probes stay small (HeaderProbe's
/// 2 MiB cap) and playback wraps it in the block cache.
pub struct ExecOpen {
    profile: RemoteProfile,
}

impl ExecOpen {
    pub fn new(profile: &RemoteProfile) -> Self {
        Self { profile: profile.clone() }
    }
}

impl RemoteOpen for ExecOpen {
    fn open(&self, remote_path: &str) -> Result<Arc<dyn ByteSource>, LyraError> {
        Ok(Arc::new(super::SshExecFile::open_profile(&self.profile, remote_path)?))
    }
}

// ── scanner ──────────────────────────────────────────────────────────────

/// Per-file progress hook (UI / remote WS). Called synchronously on the
/// scanning thread — keep it cheap.
pub type ProgressCb = Arc<dyn Fn(&ScanProgress) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub done: usize,
    pub total: usize,
    pub path: String,
}

#[derive(Clone)]
pub struct ScanOptions {
    /// Cursor checkpoints every N files. Default 50.
    pub batch_size: usize,
    /// Resume past the stored cursor (left by a cancelled pass).
    /// Default true.
    pub resume: bool,
    pub progress: Option<ProgressCb>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            batch_size: 50,
            resume: true,
            progress: None,
        }
    }
}

impl std::fmt::Debug for ScanOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanOptions")
            .field("batch_size", &self.batch_size)
            .field("resume", &self.resume)
            .field("progress", &self.progress.is_some())
            .finish()
    }
}

impl ScanOptions {
    fn batch(&self) -> usize {
        match self.batch_size {
            0 => 50,
            n => n,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScanStats {
    pub walked: usize,
    pub probed: usize,
    pub skipped: usize,
    pub pruned: usize,
    pub cancelled: bool,
}

/// Walk → filter → probe → upsert. Generic over the three seams so the
/// whole pipeline runs hermetically in tests.
pub struct RemoteScanner<W, O, P> {
    walker: W,
    opener: O,
    probe: P,
    profile: RemoteProfile,
}

impl<W: RemoteWalk, O: RemoteOpen, P: RemoteProbe> RemoteScanner<W, O, P> {
    pub fn new(walker: W, opener: O, probe: P, profile: &RemoteProfile) -> Self {
        Self {
            walker,
            opener,
            probe,
            profile: profile.clone(),
        }
    }

    fn cursor_key(&self) -> String {
        format!(
            "scan_cursor:{}",
            self.profile.source_uri(&self.profile.root_path)
        )
    }

    fn root_uri(&self) -> String {
        self.profile.source_uri(&self.profile.root_path)
    }

    /// Full pass. Rows are keyed `sftp://host/path`; only new/mtime-changed
    /// files are (re)probed. Prune runs only on a non-cancelled pass.
    pub fn scan(
        &self,
        lib: &Library,
        cancel: &Cancel,
        options: &ScanOptions,
    ) -> Result<ScanStats, LyraError> {
        let mut stats = ScanStats::default();
        let origin = self.root_uri();
        lib.add_source(&origin, "ssh")?;

        let mut entries: Vec<RemoteEntry> = self
            .walker
            .walk(cancel)?
            .into_iter()
            .filter(|e| is_remote_audio(&e.path))
            .collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        stats.walked = entries.len();

        let mut start = 0;
        if options.resume {
            if let Some(cursor) = lib.get_setting(&self.cursor_key())? {
                start = entries.partition_point(|e| e.path <= cursor);
            }
        }
        let alive: Vec<String> = entries
            .iter()
            .map(|e| self.profile.source_uri(&e.path))
            .collect();

        let mut done = start;
        for entry in &entries[start..] {
            if cancel.is_cancelled() {
                stats.cancelled = true;
                break;
            }
            let dest = self.profile.source_uri(&entry.path);
            if lib.needs_scan(&dest, entry.mtime)? {
                let src = self.opener.open(&entry.path)?;
                let hint = ProbeHint {
                    remote_path: entry.path.clone(),
                    size: entry.size,
                    mtime: entry.mtime,
                };
                let probed = self.probe.probe(src.as_ref(), &hint)?;
                let mut track = probed.track;
                track.path.clone_from(&dest);
                if let Some(art) = &probed.art {
                    track.artwork_hash = lib.ingest_artwork(&art.data, art.mime, "embedded");
                }
                lib.upsert_track(&track, probed.mtime, probed.size_bytes)?;
                stats.probed += 1;
            } else {
                stats.skipped += 1;
            }
            done += 1;
            if let Some(cb) = &options.progress {
                cb(&ScanProgress {
                    done,
                    total: entries.len(),
                    path: dest.clone(),
                });
            }
            if done % options.batch() == 0 {
                lib.set_setting(&self.cursor_key(), &entry.path)?;
            }
        }

        if !stats.cancelled {
            stats.pruned = lib.prune_prefix(&alive, &origin)?;
            // Completed: clear the cursor so the next scan diffs from the
            // top instead of skipping everything past a stale endpoint.
            lib.clear_setting(&self.cursor_key())?;
        } else if done > start {
            let last_done = &entries[done - 1];
            lib.set_setting(&self.cursor_key(), &last_done.path)?;
        }
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::super::sftp::doubles::MemBackend;
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    // ── doubles ──

    struct MemWalk {
        entries: Vec<RemoteEntry>,
    }

    impl RemoteWalk for MemWalk {
        fn walk(&self, _cancel: &Cancel) -> Result<Vec<RemoteEntry>, LyraError> {
            Ok(self.entries.clone())
        }
    }

    struct MemOpener {
        dir: std::path::PathBuf,
        files: HashMap<String, Vec<u8>>,
    }

    impl RemoteOpen for MemOpener {
        fn open(&self, remote_path: &str) -> Result<Arc<dyn ByteSource>, LyraError> {
            let data = self.files.get(remote_path).cloned().unwrap_or_default();
            let name = remote_path.replace('/', "_");
            let backend = MemBackend::with_bytes(&self.dir, &name, &data);
            let src = SftpSource::open_with(backend, remote_path, remote_path)
                .map_err(|e| LyraError::Remote(e.to_string()))?;
            Ok(Arc::new(src))
        }
    }

    /// Minimal PCM16 mono WAV: `secs` seconds at 44.1 kHz.
    fn wav_bytes(secs: u32) -> Vec<u8> {
        let rate = 44_100u32;
        let n = rate * secs;
        let data_len = n * 2;
        let mut b = Vec::with_capacity(44 + data_len as usize);
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..n {
            let s = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate as f32).sin();
            b.extend_from_slice(&((s * 12000.0) as i16).to_le_bytes());
        }
        b
    }

    static IDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::Ordering;
        let d = std::env::temp_dir().join(format!(
            "lyra-scan-{tag}-{}-{}",
            std::process::id(),
            IDS.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn profile() -> RemoteProfile {
        RemoteProfile::new("nas", "/mnt/music")
    }

    fn entry(path: &str, size: u64, mtime: i64) -> RemoteEntry {
        RemoteEntry {
            path: path.into(),
            size,
            mtime,
        }
    }

    fn scanner(
        dir: &std::path::Path,
        entries: Vec<RemoteEntry>,
        files: HashMap<String, Vec<u8>>,
    ) -> RemoteScanner<MemWalk, MemOpener, HeaderProbe> {
        RemoteScanner::new(
            MemWalk { entries },
            MemOpener {
                dir: dir.to_path_buf(),
                files,
            },
            HeaderProbe::default(),
            &profile(),
        )
    }

    // ── filter ──

    #[test]
    fn audio_filter_shapes() {
        for good in [
            "/m/a.flac",
            "/m/a.FLAC",
            "/m/a.alac",
            "/m/a.m4a",
            "/m/a.mp4",
            "/m/a.wav",
            "/m/a.aiff",
            "/m/a.aif",
            "/m/a.mp3",
            "/m/a.aac",
            "/m/a.ogg",
            "/m/a.opus",
        ] {
            assert!(is_remote_audio(good), "{good}");
        }
        for bad in [
            "/m/a.txt",
            "/m/a.jpg",
            "/m/cover.png",
            "/m/a.cue",
            "/m/noext",
        ] {
            // cue sheets are catalogued elsewhere, not probed as audio here
            assert!(!is_remote_audio(bad), "{bad}");
        }
    }

    // ── header-only probe ──

    #[test]
    fn header_probe_reads_prefix_only() {
        let dir = tmpdir("prefix");
        let wav = wav_bytes(30); // ~2.6 MiB of PCM
        assert!(wav.len() > 2 << 20);
        let backend = MemBackend::with_bytes(&dir, "big.wav", &wav);
        let src = SftpSource::open_with(backend, "sftp://nas/big.wav", "/big.wav").unwrap();
        assert_eq!(src.len() as usize, wav.len());

        let probe = HeaderProbe {
            cap_bytes: 64 << 10,
        };
        let hint = ProbeHint {
            remote_path: "/big.wav".into(),
            size: wav.len() as u64,
            mtime: 7,
        };
        let out = probe.probe(&src, &hint).unwrap();
        assert_eq!(out.track.codec, "PCM");
        let dur = out.track.duration_secs.unwrap_or(0.0);
        assert!((dur - 30.0).abs() < 0.5, "duration {dur}");
        assert!(
            src.backend().max_read_offset() <= 64 << 10,
            "read past the cap"
        );
    }

    // ── diffing ──

    #[test]
    fn scan_upserts_and_skips_unchanged() {
        let dir = tmpdir("diff");
        let wav = wav_bytes(1);
        let entries = vec![
            entry("/mnt/music/a.wav", wav.len() as u64, 100),
            entry("/mnt/music/notes.txt", 10, 100), // filtered
            entry("/mnt/music/b.mp3", 5000, 100),   // tiny stub, tags may miss — row still lands
        ];
        let files: HashMap<_, _> = [
            ("/mnt/music/a.wav".into(), wav.clone()),
            ("/mnt/music/b.mp3".into(), vec![0u8; 5000]),
        ]
        .into();
        let lib = Library::open_memory().unwrap();
        let opts = ScanOptions {
            batch_size: 10,
            resume: true,
            progress: None,
        };

        let s1 = scanner(&dir, entries.clone(), files.clone())
            .scan(&lib, &Cancel::new(), &opts)
            .unwrap();
        assert_eq!(s1.walked, 2);
        assert_eq!(s1.probed, 2);
        assert_eq!(s1.skipped, 0);
        assert!(!s1.cancelled);

        let rows = lib.all_tracks().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .all(|t| t.path.starts_with("sftp://nas/mnt/music/")));
        let a = rows.iter().find(|t| t.path.ends_with("a.wav")).unwrap();
        assert_eq!(a.codec, "PCM");

        // second pass: everything unchanged → zero probing
        let s2 = scanner(&dir, entries, files)
            .scan(&lib, &Cancel::new(), &opts)
            .unwrap();
        assert_eq!(s2.probed, 0);
        assert_eq!(s2.skipped, 2);

        // completed passes leave no cursor behind — the next scan diffs
        // from the top instead of no-op'ing past a stale endpoint
        let cursor = format!("scan_cursor:{}", profile().source_uri("/mnt/music"));
        assert_eq!(lib.get_setting(&cursor).unwrap(), None);
    }

    #[test]
    fn mtime_change_reprobes() {
        let dir = tmpdir("mtime");
        let wav = wav_bytes(1);
        let files: HashMap<_, _> = [("/mnt/music/a.wav".into(), wav.clone())].into();
        let lib = Library::open_memory().unwrap();
        let opts = ScanOptions::default();

        let mk = |mtime| vec![entry("/mnt/music/a.wav", wav.len() as u64, mtime)];
        let s1 = scanner(&dir, mk(100), files.clone())
            .scan(&lib, &Cancel::new(), &opts)
            .unwrap();
        assert_eq!((s1.probed, s1.skipped), (1, 0));
        let s2 = scanner(&dir, mk(200), files)
            .scan(&lib, &Cancel::new(), &opts)
            .unwrap();
        assert_eq!((s2.probed, s2.skipped), (1, 0));
    }

    // ── cancel + resume ──

    /// Probe double that cancels the scan on the second file.
    struct CancellingProbe {
        cancel: Cancel,
        seen: StdMutex<usize>,
    }

    impl RemoteProbe for CancellingProbe {
        fn probe(&self, _src: &dyn ByteSource, hint: &ProbeHint) -> Result<ProbedFile, LyraError> {
            let mut seen = self.seen.lock().unwrap();
            *seen += 1;
            if *seen == 2 {
                self.cancel.cancel();
            }
            Ok(ProbedFile {
                track: lyra_core::LibraryTrack {
                    path: hint.remote_path.clone(),
                    title: None,
                    artist: None,
                    album: None,
                    album_artist: None,
                    genre: None,
                    year: None,
                    track_number: None,
                    duration_secs: None,
                    format: lyra_core::AudioFormat::Wav,
                    codec: "PCM".into(),
                    sample_rate: Some(44100),
                    channels: Some(1),
                    bits_per_sample: Some(16),
                    artwork_hash: None,
                },
                mtime: hint.mtime,
                size_bytes: hint.size as i64,
                art: None,
            })
        }
    }

    fn cancel_scanner(
        dir: &std::path::Path,
        entries: Vec<RemoteEntry>,
        cancel: Cancel,
        wav: Vec<u8>,
    ) -> RemoteScanner<MemWalk, MemOpener, CancellingProbe> {
        let files: HashMap<_, _> = entries
            .iter()
            .map(|e| (e.path.clone(), wav.clone()))
            .collect();
        RemoteScanner::new(
            MemWalk { entries },
            MemOpener {
                dir: dir.to_path_buf(),
                files,
            },
            CancellingProbe {
                cancel,
                seen: StdMutex::new(0),
            },
            &profile(),
        )
    }

    #[test]
    fn cancel_stops_and_resume_continues() {
        let dir = tmpdir("cancel");
        let wav = wav_bytes(1);
        let entries: Vec<_> = (0..6)
            .map(|i| entry(&format!("/mnt/music/t{i}.wav"), wav.len() as u64, 100))
            .collect();
        let lib = Library::open_memory().unwrap();
        let opts = ScanOptions {
            batch_size: 2,
            ..Default::default()
        };
        let cancel = Cancel::new();

        let s1 = cancel_scanner(&dir, entries.clone(), cancel.clone(), wav.clone())
            .scan(&lib, &cancel, &opts)
            .unwrap();
        assert!(s1.cancelled);
        assert_eq!(s1.probed + s1.skipped, 2); // stopped right after the cancel trip
        assert_eq!(lib.all_tracks().unwrap().len(), 2);

        // resume (default opts) picks up past the cursor: the two done
        // files are skipped by cursor, not by re-diffing
        let files: HashMap<_, _> = entries
            .iter()
            .map(|e| (e.path.clone(), wav.clone()))
            .collect();
        let s2 = scanner(&dir, entries, files)
            .scan(&lib, &Cancel::new(), &opts)
            .unwrap();
        assert!(!s2.cancelled);
        assert_eq!(s2.skipped, 0);
        assert_eq!(s2.probed, 4); // the rest
        assert_eq!(lib.all_tracks().unwrap().len(), 6);
    }

    // ── prune scoping ──

    #[test]
    fn prune_never_touches_other_roots() {
        let dir = tmpdir("prune");
        let wav = wav_bytes(1);
        let lib = Library::open_memory().unwrap();

        // a local row + a stale remote row from this root
        lib.upsert_track(
            &lyra_core::LibraryTrack {
                path: "/local/gone.flac".into(),
                title: None,
                artist: None,
                album: None,
                album_artist: None,
                genre: None,
                year: None,
                track_number: None,
                duration_secs: None,
                format: lyra_core::AudioFormat::Flac,
                codec: "FLAC".into(),
                sample_rate: None,
                channels: None,
                bits_per_sample: None,
                artwork_hash: None,
            },
            1,
            1,
        )
        .unwrap();
        lib.upsert_track(
            &lyra_core::LibraryTrack {
                path: "sftp://nas/mnt/music/stale.wav".into(),
                title: None,
                artist: None,
                album: None,
                album_artist: None,
                genre: None,
                year: None,
                track_number: None,
                duration_secs: None,
                format: lyra_core::AudioFormat::Wav,
                codec: "PCM".into(),
                sample_rate: None,
                channels: None,
                bits_per_sample: None,
                artwork_hash: None,
            },
            1,
            1,
        )
        .unwrap();

        let entries = vec![entry("/mnt/music/a.wav", wav.len() as u64, 100)];
        let files: HashMap<_, _> = [("/mnt/music/a.wav".into(), wav)].into();
        let stats = scanner(&dir, entries, files)
            .scan(&lib, &Cancel::new(), &ScanOptions::default())
            .unwrap();
        assert_eq!(stats.pruned, 1);
        let paths: Vec<_> = lib
            .all_tracks()
            .unwrap()
            .into_iter()
            .map(|t| t.path)
            .collect();
        assert!(paths.contains(&"/local/gone.flac".to_string()), "{paths:?}");
        assert!(
            paths.contains(&"sftp://nas/mnt/music/a.wav".to_string()),
            "{paths:?}"
        );
        assert!(
            !paths.contains(&"sftp://nas/mnt/music/stale.wav".to_string()),
            "{paths:?}"
        );
    }

    #[test]
    fn progress_hook_sees_each_file() {
        let dir = tmpdir("progress");
        let lib = Library::open_memory().unwrap();
        let wav = wav_bytes(1);
        let entries: Vec<_> = (0..3)
            .map(|i| entry(&format!("/mnt/music/p{i}.wav"), wav.len() as u64, 50))
            .collect();
        let files: HashMap<_, _> = entries
            .iter()
            .map(|e| (e.path.clone(), wav.clone()))
            .collect();
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let seen2 = seen.clone();
        let opts = ScanOptions {
            progress: Some(Arc::new(move |p: &ScanProgress| {
                seen2.lock().unwrap().push(p.done)
            })),
            ..Default::default()
        };
        scanner(&dir, entries, files)
            .scan(&lib, &Cancel::new(), &opts)
            .unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![1, 2, 3]);
    }
}
