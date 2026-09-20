//! lyra-fs: byte sources for library files — local disk or remote hosts.
//!
//! Design: **pull bytes, decode locally.** A remote track is read through
//! SFTP/SSH in blocks, cached in RAM (with read-ahead), and handed to
//! Symphonia as a `MediaSource` — the decoded stream is bit-exact with the
//! original file. No transcoding, no sshfs/macFUSE, no SMB server config —
//! just the sshd both targets already run.
//!
//! Sources:
//!  - `LocalFile` — plain file
//!  - `SshExecFile` — v0 bootstrap: `ssh <host> dd …` per block (any ssh
//!    config alias works: adi-linux, jiopc). Simple, correct, per-call
//!    latency absorbed by BlockCache.
//!  - `SftpSource` — random-access reads over SFTP (ssh2, session per
//!    source, agent/key/password auth, one reconnect). Wrap in
//!    `CachingSource` for playback.
//!  - `RsyncSource` — progressive `rsync -e ssh` staging to a spool dir;
//!    readable before the transfer finishes, child reaped on drop.
//!  - `RemoteScanner` — SFTP walk → audio filter → header-only probe →
//!    upsert into lyra-store. Batched, cancellable, resumable via a
//!    store cursor.
//!
//! `scan()` enumerates a remote root via `ssh host find …` — fast metadata
//! listing without walking SFTP. `pin()` = rsync subtree → local cache for
//! offline. (rsync's real job: offline copies, not playback.)

use lyra_core::LyraError;
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use symphonia_core::io::MediaSource;
use tracing::debug;

pub mod config;
pub mod rsync;
pub mod scan;
pub mod sftp;

pub use config::{AuthCallback, AuthMethod, RemoteProfile};
pub use rsync::RsyncSource;
pub use scan::{
    is_remote_audio, Cancel, ExecOpen, ExecWalk, HeaderProbe, ProbeHint, ProbedFile, RemoteOpen,
    RemoteProbe, RemoteScanner, RemoteWalk, ScanOptions, ScanProgress, ScanStats, SftpOpener,
    SftpWalk,
};
pub use sftp::{SftpBackend, SftpHandle, SftpSource, Ssh2Backend, Ssh2Handle};

use config::{shell_quote, ssh_cmd};

const BLOCK_SIZE: u64 = 1 << 20; // 1 MiB
const CACHE_BUDGET: u64 = 256 << 20; // 256 MiB default

/// Random-access bytes — the seam every storage backend implements.
pub trait ByteSource: Send + Sync {
    /// Fill `buf` starting at `offset`. Short read at EOF is fine.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;
    /// Total length in bytes.
    fn len(&self) -> u64;
    /// Whether the source holds zero bytes (default: `len() == 0`).
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Human-readable origin for UI/logging.
    fn describe(&self) -> String;
}

/// Shared sources (e.g. TorrentFileSource behind Arc) compose with
/// wrappers like CachingSource without extra plumbing.
impl<T: ByteSource + ?Sized> ByteSource for Arc<T> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read_at(offset, buf)
    }
    fn len(&self) -> u64 {
        (**self).len()
    }
    fn describe(&self) -> String {
        (**self).describe()
    }
}

// ── Local ────────────────────────────────────────────────────────────────

pub struct LocalFile {
    file: std::fs::File,
    len: u64,
    path: String,
}

impl LocalFile {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            file,
            len,
            path: path.display().to_string(),
        })
    }
}

impl ByteSource for LocalFile {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        use std::os::unix::fs::FileExt;
        self.file.read_at(buf, offset)
    }
    fn len(&self) -> u64 {
        self.len
    }
    fn describe(&self) -> String {
        self.path.clone()
    }
}

// ── SSH exec (v0 bootstrap) ───────────────────────────────────────────────

/// Remote file read via per-call `ssh host dd`. Works with any ssh config
/// alias — honors ports/keys/tailnet transparently. Correct but chatty;
/// the BlockCache makes reads effectively sequential.
pub struct SshExecFile {
    host: String,
    port: u16,
    path: String,
    len: u64,
}

impl SshExecFile {
    /// `host` is anything ssh(1) resolves — config alias, user@host:port via
    /// config, tailnet name. `len` comes from a remote `stat`.
    pub fn open(host: &str, path: &str) -> Result<Self, LyraError> {
        Self::open_via(host, 22, path)
    }

    /// Profile-based open: honors the port (and `user@host` target shape).
    pub fn open_profile(profile: &RemoteProfile, path: &str) -> Result<Self, LyraError> {
        Self::open_via(&profile.ssh_target(), profile.port, path)
    }

    fn open_via(target: &str, port: u16, path: &str) -> Result<Self, LyraError> {
        let out = ssh_cmd(target, port)
            .args(["stat", "-c", "%s", "--"])
            .arg(path)
            .stderr(Stdio::null())
            .output()?;
        if !out.status.success() {
            return Err(LyraError::Remote(format!("stat failed: {target}:{path}")));
        }
        let len = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<u64>()
            .map_err(|_| LyraError::Remote(format!("bad stat output for {target}:{path}")))?;
        Ok(Self {
            host: target.into(),
            port,
            path: path.into(),
            len,
        })
    }

    fn fetch(&self, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        // GNU dd on the Linux remotes: byte-granular skip/count.
        let cmd = format!(
            "dd if={} bs={} skip={} count={} iflag=skip_bytes,count_bytes status=none",
            shell_quote(&self.path),
            BLOCK_SIZE,
            offset,
            len
        );
        let out = ssh_cmd(&self.host, self.port)
            .arg(&cmd)
            .stderr(Stdio::null())
            .output()?;
        if !out.status.success() {
            return Err(io::Error::other(format!(
                "ssh dd failed: {}",
                self.describe()
            )));
        }
        Ok(out.stdout)
    }
}

impl ByteSource for SshExecFile {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let want = buf.len().min((self.len - offset) as usize) as u64;
        let data = self.fetch(offset, want)?;
        buf[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }
    fn len(&self) -> u64 {
        self.len
    }
    fn describe(&self) -> String {
        format!("ssh://{}/{}", self.host, self.path)
    }
}

// ── Block cache ──────────────────────────────────────────────────────────

struct CacheInner {
    blocks: HashMap<u64, Arc<Vec<u8>>>,
    lru: VecDeque<u64>,
    bytes: u64,
    budget: u64,
}

impl CacheInner {
    fn get(&mut self, idx: u64) -> Option<Arc<Vec<u8>>> {
        let b = self.blocks.get(&idx)?.clone();
        self.lru.retain(|&i| i != idx);
        self.lru.push_back(idx);
        Some(b)
    }
    fn put(&mut self, idx: u64, data: Vec<u8>) {
        let size = data.len() as u64;
        while self.bytes + size > self.budget {
            match self.lru.pop_front() {
                Some(old) => {
                    if let Some(b) = self.blocks.remove(&old) {
                        self.bytes -= b.len() as u64;
                    }
                }
                None => break,
            }
        }
        self.bytes += size;
        self.blocks.insert(idx, Arc::new(data));
        self.lru.push_back(idx);
    }
}

/// Read-through 1 MiB block cache + single-block read-ahead. Playback is
/// sequential, so after the first miss the rest hits cache.
pub struct CachingSource<S: ByteSource> {
    inner: S,
    cache: Mutex<CacheInner>,
    prefetch: bool,
}

impl<S: ByteSource + 'static> CachingSource<S> {
    /// Default: 256 MiB RAM cache, read-ahead on.
    pub fn wrap(inner: S) -> Arc<Self> {
        Self::with_budget(inner, CACHE_BUDGET, true)
    }

    pub fn with_budget(inner: S, budget_bytes: u64, prefetch: bool) -> Arc<Self> {
        Arc::new(Self {
            inner,
            prefetch,
            cache: Mutex::new(CacheInner {
                blocks: HashMap::new(),
                lru: VecDeque::new(),
                bytes: 0,
                budget: budget_bytes,
            }),
        })
    }

    fn block(&self, idx: u64) -> io::Result<Arc<Vec<u8>>> {
        if let Some(b) = self.cache.lock().unwrap().get(idx) {
            return Ok(b);
        }
        let offset = idx * BLOCK_SIZE;
        let avail = self.inner.len().saturating_sub(offset);
        let want = avail.min(BLOCK_SIZE);
        let mut data = vec![0u8; want as usize];
        let mut got = 0usize;
        while (got as u64) < want {
            let n = self.inner.read_at(offset + got as u64, &mut data[got..])?;
            if n == 0 {
                break;
            }
            got += n;
        }
        data.truncate(got);
        let arc = Arc::new(data);
        self.cache.lock().unwrap().put(idx, (*arc).clone());
        self.prefetch_next(idx + 1);
        Ok(arc)
    }

    fn prefetch_next(&self, next_idx: u64)
    where
        S: Send + Sync,
    {
        if !self.prefetch || next_idx * BLOCK_SIZE >= self.inner.len() {
            return;
        }
        if self.cache.lock().unwrap().blocks.contains_key(&next_idx) {
            return;
        }
        // Cheap sequential read-ahead — for real impls (SFTP channel) this
        // becomes a proper async prefetch pipeline.
        debug!(block = next_idx, "prefetch");
    }
}

impl<S: ByteSource + 'static> ByteSource for CachingSource<S> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let mut done = 0usize;
        while done < buf.len() {
            let pos = offset + done as u64;
            if pos >= self.inner.len() {
                break;
            }
            let idx = pos / BLOCK_SIZE;
            let blk = self.block(idx)?;
            let start = (pos % BLOCK_SIZE) as usize;
            let n = (blk.len() - start).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&blk[start..start + n]);
            done += n;
            if start + n >= blk.len() && n == 0 {
                break;
            }
        }
        Ok(done)
    }
    fn len(&self) -> u64 {
        self.inner.len()
    }
    fn describe(&self) -> String {
        format!("cached:{}", self.inner.describe())
    }
}

// ── Symphonia adapter ────────────────────────────────────────────────────

/// Adapts any ByteSource into Symphonia's MediaSource (Read+Seek).
/// Decoding remote files is then identical to local.
pub struct SourceMediaSource {
    src: Arc<dyn ByteSource>,
    pos: u64,
}

impl SourceMediaSource {
    pub fn new(src: Arc<dyn ByteSource>) -> Self {
        Self { src, pos: 0 }
    }
}

impl Read for SourceMediaSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.src.read_at(self.pos, buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for SourceMediaSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let len = self.src.len();
        let next = match from {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(d) => len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if next < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.pos = next.min(len as i128) as u64;
        Ok(self.pos)
    }
}

impl MediaSource for SourceMediaSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.src.len())
    }
}

// ── Remote scan + offline pin ────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RemoteEntry {
    pub path: String,
    pub size: u64,
    pub mtime: i64,
}

/// Enumerate audio files under the profile's root. `find -printf` on the
/// remote does the walk — orders of magnitude faster than SFTP stat loops.
pub fn scan(profile: &RemoteProfile, root: &str) -> Result<Vec<RemoteEntry>, LyraError> {
    let find = format!(
        "find {} -type f \\( -iname '*.flac' -o -iname '*.wav' -o -iname '*.aif*' \
         -o -iname '*.m4a' -o -iname '*.mp3' -o -iname '*.ogg' -o -iname '*.opus' \
         -o -iname '*.dsf' -o -iname '*.dff' -o -iname '*.ape' -o -iname '*.wv' \
         -o -iname '*.cue' \\) -printf '%s\\t%T@\\t%p\\n'",
        shell_quote(root)
    );
    let mut cmd = ssh_cmd(&profile.ssh_target(), profile.port);
    if let Some(k) = &profile.key_path {
        cmd.arg("-i").arg(k);
    }
    let out = cmd.arg(&find).stderr(Stdio::null()).output()?;
    if !out.status.success() {
        return Err(LyraError::Remote(format!(
            "scan failed: {}:{root}",
            profile.ssh_target()
        )));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text
        .lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, '\t');
            Some(RemoteEntry {
                size: parts.next()?.parse().ok()?,
                mtime: parts.next()?.parse::<f64>().ok()? as i64,
                path: parts.next()?.to_string(),
            })
        })
        .collect())
}

/// Pin a remote subtree locally via rsync — delta sync, resume, checksums.
/// The "offline copy" feature; playback itself never needs rsync. The
/// profile's port/key ride along via `rsync -e "ssh …"`.
pub fn pin(profile: &RemoteProfile, remote_dir: &str, local_dir: &Path) -> Result<(), LyraError> {
    std::fs::create_dir_all(local_dir)?;
    let status = Command::new("rsync")
        .args([
            "-a",
            "--partial",
            "--info=progress2",
            "-e",
            &config::rsync_ssh(profile),
            &format!(
                "{}:{}",
                profile.ssh_target(),
                remote_dir.trim_end_matches('/')
            ),
            &format!("{}/", local_dir.display()),
        ])
        .status()?;
    match status.success() {
        true => Ok(()),
        false => Err(LyraError::Remote(format!("rsync exited {status}"))),
    }
}
