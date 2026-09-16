//! lyra-torrent: BitTorrent as a library source — netlabels, etree-style
//! live recordings, CC/PD releases, artist-distributed masters.
//!
//! Two modes:
//!  1. **Download → import** (default): torrent completes into a managed
//!     dir, then the normal scanner imports it as local files.
//!  2. **Stream-while-downloading**: `TorrentFileSource` is a `ByteSource`,
//!     so `CachingSource` + `SourceMediaSource` + symphonia = start playing
//!     seconds after adding a magnet, pieces fetched in read order.
//!
//! Privacy note: DHT/peer traffic exposes your IP to the swarm — inherent
//! to BitTorrent. DHT can be disabled for tracker-only use.

use lyra_core::LyraError;
use lyra_fs::ByteSource;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tracing::info;

/// One torrent's file, as listed after metadata resolves.
#[derive(Debug, Clone)]
pub struct TorrentFileInfo {
    pub index: usize,
    pub path: String,
    pub len: u64,
}

/// Session-level configuration — mostly test/headless knobs; production
/// defaults are DHT on, OS-assigned listen port.
#[derive(Debug, Clone, Default)]
pub struct EngineConfig {
    pub disable_dht: bool,
    pub listen_port_range: Option<std::ops::Range<u16>>,
}

/// Per-add options. `initial_peers` + `disable_trackers` allow a
/// tracker-less local swarm (tests, trusted LAN seeds).
#[derive(Debug, Clone, Default)]
pub struct AddOpts {
    pub initial_peers: Option<Vec<std::net::SocketAddr>>,
    pub disable_trackers: bool,
    /// Overwrite existing files — also the "seed an existing complete
    /// folder" path: rqbit verifies instead of re-downloading.
    pub overwrite: bool,
    pub output_folder: Option<PathBuf>,
}

pub struct TorrentEngine {
    rt: Runtime,
    session: Arc<librqbit::Session>,
    download_dir: PathBuf,
}

impl TorrentEngine {
    pub fn new(download_dir: PathBuf) -> Result<Self, LyraError> {
        Self::new_with_config(download_dir, EngineConfig::default())
    }

    pub fn new_with_config(
        download_dir: PathBuf,
        cfg: EngineConfig,
    ) -> Result<Self, LyraError> {
        std::fs::create_dir_all(&download_dir)?;
        let rt = Runtime::new().map_err(|e| LyraError::Remote(e.to_string()))?;
        let opts = librqbit::SessionOptions {
            disable_dht: cfg.disable_dht,
            disable_dht_persistence: cfg.disable_dht,
            listen_port_range: cfg.listen_port_range,
            // JSON persistence → torrents survive relaunches with stable
            // ids (preferred_id), offline metadata ({hash}.torrent) and
            // piece bitmaps ({hash}.bitv) for resume. Dot-folder so the
            // orphan sweep skips it.
            persistence: Some(librqbit::SessionPersistenceConfig::Json {
                folder: Some(download_dir.join(".session")),
            }),
            ..Default::default()
        };
        let session = rt
            .block_on(librqbit::Session::new_with_opts(download_dir.clone(), opts))
            .map_err(|e| LyraError::Remote(format!("rqbit session: {e}")))?;
        Ok(Self { rt, session, download_dir })
    }

    /// This session's inbound peer port, if listening.
    pub fn listen_port(&self) -> Option<u16> {
        self.session.tcp_listen_port()
    }

    /// Add a magnet URI or local .torrent path. Returns torrent id.
    pub fn add(&self, spec: &str) -> Result<usize, LyraError> {
        // overwrite=true: player semantics — re-adding a torrent whose
        // output files exist must resume, not error (rqbit's default is
        // create_new → EEXIST). Existing pieces are hash-validated either
        // way, so partial downloads keep their progress.
        self.add_opts(
            spec,
            AddOpts { overwrite: true, ..Default::default() },
        )
    }

    pub fn add_opts(&self, spec: &str, mut add: AddOpts) -> Result<usize, LyraError> {
        // rqbit ignores `x.pe` peer hints — extract them ourselves so
        // trackerless magnets ("magnet:?xt=…&x.pe=1.2.3.4:6881") work.
        if spec.starts_with("magnet:") {
            for peer in spec
                .split('&')
                .filter_map(|kv| kv.strip_prefix("x.pe=").or_else(|| {
                    kv.strip_prefix("?x.pe=")
                }))
            {
                if let Ok(addr) = peer.parse::<std::net::SocketAddr>() {
                    add.initial_peers.get_or_insert_with(Vec::new).push(addr);
                }
            }
        }
        let source = if spec.starts_with("magnet:") {
            librqbit::AddTorrent::from_url(spec)
        } else {
            librqbit::AddTorrent::from_local_filename(spec)
                .map_err(|e| LyraError::Remote(format!("bad torrent file: {e}")))?
        };
        let opts = librqbit::AddTorrentOptions {
            initial_peers: add.initial_peers,
            disable_trackers: add.disable_trackers,
            overwrite: add.overwrite,
            output_folder: add
                .output_folder
                .map(|p| p.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let resp = self
            .rt
            .block_on(self.session.add_torrent(source, Some(opts)))
            .map_err(|e| LyraError::Remote(format!("add torrent: {e}")))?;
        let (id, handle) = match resp {
            librqbit::AddTorrentResponse::AlreadyManaged(id, h)
            | librqbit::AddTorrentResponse::Added(id, h) => (id, h),
            librqbit::AddTorrentResponse::ListOnly(_) => {
                return Err(LyraError::Remote("list-only response".into()))
            }
        };
        // Resolve magnet metadata before returning so callers can list
        // files — capped so a dead magnet can't hang the caller forever.
        let h = handle.clone();
        let init = self.rt.block_on(async move {
            tokio::time::timeout(
                std::time::Duration::from_secs(45),
                h.wait_until_initialized(),
            )
            .await
        });
        match init {
            Ok(Ok(())) => {}
            _ => return Err(LyraError::Remote("metadata resolve timed out".into())),
        }
        info!(id, "torrent added");
        Ok(id)
    }

    fn handle(&self, id: usize) -> Result<Arc<librqbit::ManagedTorrent>, LyraError> {
        self.session
            .get(librqbit::api::TorrentIdOrHash::Id(id))
            .ok_or_else(|| LyraError::Remote(format!("no torrent {id}")))
    }

    /// Files inside a torrent (after metadata resolve for magnets).
    pub fn files(&self, id: usize) -> Result<Vec<TorrentFileInfo>, LyraError> {
        let handle = self.handle(id)?;
        let meta = handle
            .metadata
            .load_full()
            .ok_or_else(|| LyraError::Remote("metadata not resolved yet".into()))?;
        let names = meta
            .info
            .iter_file_details()
            .map_err(|e| LyraError::Remote(e.to_string()))?;
        Ok(names
            .enumerate()
            .map(|(i, d)| TorrentFileInfo {
                index: i,
                path: d.filename.to_string().unwrap_or_else(|_| "<invalid>".into()),
                len: d.len,
            })
            .collect())
    }

    /// Progress snapshot for the UI.
    pub fn stats(&self, id: usize) -> Result<serde_json::Value, LyraError> {
        let handle = self.handle(id)?;
        let s = handle.stats();
        Ok(serde_json::json!({
            "id": id,
            "progress_bytes": s.progress_bytes,
            "total_bytes": s.total_bytes,
            "finished": s.finished,
        }))
    }

    /// ByteSource for stream-while-downloading: rqbit's FileStream is
    /// AsyncRead+AsyncSeek; pieces fetch on demand in read order, the
    /// CachingSource absorbs seek latency for the decoder.
    pub fn open_file(
        self: &Arc<Self>,
        id: usize,
        file_idx: usize,
    ) -> Result<Arc<TorrentFileSource>, LyraError> {
        let handle = self.handle(id)?;
        // rqbit's stream() spawns onto the ambient runtime — enter ours.
        let _guard = self.rt.enter();
        let stream = handle
            .clone()
            .stream(file_idx)
            .map_err(|e| LyraError::Remote(format!("stream file: {e}")))?;
        let len = stream.len();
        Ok(Arc::new(TorrentFileSource {
            engine: Arc::clone(self),
            stream: Mutex::new(Box::pin(stream)),
            file_idx,
            len,
        }))
    }

    /// Remove a torrent from the session. `delete_files` also wipes its
    /// downloaded data from disk — the disk-space reclaim path.
    pub fn remove(&self, id: usize, delete_files: bool) -> Result<(), LyraError> {
        self.rt
            .block_on(self.session.delete(
                librqbit::api::TorrentIdOrHash::Id(id),
                delete_files,
            ))
            .map_err(|e| LyraError::Remote(format!("remove torrent: {e}")))?;
        info!(id, delete_files, "torrent removed");
        Ok(())
    }

    /// Create a .torrent (v1) for a local file or directory — e.g. seeding
    /// your own library to another Lyra instance. Returns torrent bytes.
    pub fn create_torrent_bytes(&self, path: &std::path::Path) -> Result<Vec<u8>, LyraError> {
        let _guard = self.rt.enter();
        let res = self
            .rt
            .block_on(librqbit::create_torrent(
                path,
                librqbit::CreateTorrentOptions::default(),
            ))
            .map_err(|e| LyraError::Remote(format!("create torrent: {e}")))?;
        res.as_bytes()
            .map(|b| b.to_vec())
            .map_err(|e| LyraError::Remote(format!("serialize torrent: {e}")))
    }

    /// Local path of a completed file — the "import into library" hook.
    pub fn completed_path(&self, id: usize, file_idx: usize) -> Result<PathBuf, LyraError> {
        let files = self.files(id)?;
        let f = files
            .into_iter()
            .find(|f| f.index == file_idx)
            .ok_or_else(|| LyraError::Remote(format!("no file {file_idx}")))?;
        Ok(self.download_dir.join(&f.path))
    }

    /// Managed torrents in this session — the UI's source of truth across
    /// relaunches (JSON persistence restores them with stable ids).
    pub fn list(&self) -> Vec<(usize, String)> {
        self.session.with_torrents(|it| {
            it.map(|(id, h)| {
                (
                    id,
                    h.name().unwrap_or_else(|| format!("torrent #{id}")),
                )
            })
            .collect()
        })
    }

    /// Top-level entries in download_dir not claimed by any managed
    /// torrent — leftovers from sessions killed before a clean remove,
    /// or folders deleted from the session by hand.
    pub fn orphans(&self) -> Result<Vec<(String, u64)>, LyraError> {
        use std::collections::HashSet;
        let mut claimed: HashSet<String> = [".session".to_string()].into_iter().collect();
        // Multi-file torrents download under a dir named after the
        // torrent; file paths may also carry a top-level prefix.
        let (names, paths): (Vec<_>, Vec<_>) = self.session.with_torrents(|it| {
            it.map(|(_, h)| {
                let tops = h
                    .metadata
                    .load_full()
                    .map(|meta| {
                        meta.info
                            .iter_file_details()
                            .map(|details| {
                                details
                                    .filter_map(|d| d.filename.to_string().ok())
                                    .filter_map(|p| {
                                        p.split('/').next().map(str::to_string)
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                (h.name(), tops)
            })
            .unzip()
        });
        claimed.extend(names.into_iter().flatten());
        claimed.extend(paths.into_iter().flatten());
        let mut out = Vec::new();
        for e in std::fs::read_dir(&self.download_dir)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            if claimed.contains(&name) {
                continue;
            }
            out.push((name, entry_size(&e.path())));
        }
        Ok(out)
    }

    /// Delete every orphan entry — returns (removed, bytes freed).
    pub fn purge_orphans(&self) -> Result<(usize, u64), LyraError> {
        let mut freed = 0u64;
        let mut removed = 0usize;
        for (name, bytes) in self.orphans()? {
            let p = self.download_dir.join(&name);
            if p.is_dir() {
                std::fs::remove_dir_all(&p)?;
            } else {
                std::fs::remove_file(&p)?;
            }
            freed += bytes;
            removed += 1;
        }
        Ok((removed, freed))
    }
}

fn entry_size(p: &std::path::Path) -> u64 {
    if p.is_file() {
        return p.metadata().map(|m| m.len()).unwrap_or(0);
    }
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            total += entry_size(&e.path());
        }
    }
    total
}

/// rqbit's FileStream type isn't exported at the crate root — erase it
/// behind the traits we actually need.
trait StreamLike: tokio::io::AsyncRead + tokio::io::AsyncSeek + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncSeek + Unpin + Send> StreamLike for T {}

/// Streams one file of a managed torrent through rqbit's FileStream
/// (AsyncRead+AsyncSeek). Missing pieces are requested on read; sequential
/// playback gets effectively-in-order piece scheduling.
pub struct TorrentFileSource {
    engine: Arc<TorrentEngine>,
    stream: Mutex<std::pin::Pin<Box<dyn StreamLike>>>,
    file_idx: usize,
    len: u64,
}

impl ByteSource for TorrentFileSource {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let mut stream = self.stream.lock().unwrap();
        self.engine.rt.block_on(async {
            stream.seek(std::io::SeekFrom::Start(offset)).await?;
            stream.read(buf).await
        })
    }
    fn len(&self) -> u64 {
        self.len
    }
    fn describe(&self) -> String {
        format!("torrent://{}/{}", self.engine.download_dir.display(), self.file_idx)
    }
}
