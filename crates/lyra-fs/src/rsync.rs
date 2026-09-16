//! `RsyncSource`: progressive staging of a remote file to a local spool
//! dir via `rsync -e ssh`, served as [`ByteSource`] from the partial file.
//!
//! Playback never waits for a full transfer: `read_at` returns as soon as
//! the requested bytes have landed (`--inplace` keeps them in the spool
//! file as they arrive) and only blocks — bounded by a timeout — when the
//! caller is ahead of the transfer. Dropping the source kills the child.

use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::config::rsync_ssh;
use super::{ByteSource, RemoteProfile};

const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunState {
    Running,
    DoneOk,
    Failed,
}

/// A remote file being staged locally, readable before it finishes.
pub struct RsyncSource {
    origin: String,
    local_path: PathBuf,
    total_len: u64,
    child: Mutex<Option<Child>>,
    state: Mutex<RunState>,
    read_timeout: Duration,
}

impl RsyncSource {
    /// Stage `remote_path` from `profile` into `spool_dir` and serve it.
    /// `total_len` is the expected size (from scan/fstat) — `len()` reports
    /// it so cache layers can size reads; the actual file wins at EOF.
    pub fn spawn(
        profile: &RemoteProfile,
        remote_path: &str,
        spool_dir: &Path,
        total_len: u64,
    ) -> io::Result<Self> {
        std::fs::create_dir_all(spool_dir)?;
        let local_path = spool_path(spool_dir, remote_path);
        let e = rsync_ssh(profile);
        let mut cmd = Command::new("rsync");
        cmd.args(["-a", "--inplace", "--partial", "-e", &e]);
        cmd.arg(format!("{}:{remote_path}", profile.ssh_target()));
        cmd.arg(&local_path);
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        Self::launch(cmd, &profile.source_uri(remote_path), local_path, total_len)
    }

    /// Test seam: wrap an already-built transfer command (a slow copier, a
    /// sleeper, `exit 3`). Anything that grows `local_path` works.
    pub(crate) fn launch(
        mut cmd: Command,
        origin: &str,
        local_path: PathBuf,
        total_len: u64,
    ) -> io::Result<Self> {
        let child = cmd.spawn()?;
        Ok(Self {
            origin: origin.into(),
            local_path,
            total_len,
            child: Mutex::new(Some(child)),
            state: Mutex::new(RunState::Running),
            read_timeout: DEFAULT_READ_TIMEOUT,
        })
    }

    pub fn with_read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout;
        self
    }

    /// Where the staged bytes live (complete or partial).
    pub fn local_path(&self) -> &Path {
        &self.local_path
    }

    /// Bytes rsync has written so far.
    pub fn staged_len(&self) -> u64 {
        std::fs::metadata(&self.local_path)
            .map(|m| m.len())
            .unwrap_or(0)
    }

    /// Reap-if-exited and cache the terminal state. Never blocks.
    fn poll(&self) -> RunState {
        if *self.state.lock().unwrap() != RunState::Running {
            return *self.state.lock().unwrap();
        }
        let done = match self.child.lock().unwrap().as_mut() {
            Some(c) => match c.try_wait() {
                Ok(Some(status)) => Some(status.success()),
                Ok(None) => None,
                Err(_) => Some(false),
            },
            None => Some(false),
        };
        if let Some(ok) = done {
            // Reap so we never leave a zombie; Drop becomes a no-op.
            let _ = self.child.lock().unwrap().take().map(|mut c| c.wait());
            *self.state.lock().unwrap() = if ok {
                RunState::DoneOk
            } else {
                RunState::Failed
            };
        }
        *self.state.lock().unwrap()
    }

    pub fn is_complete(&self) -> bool {
        self.poll() == RunState::DoneOk
    }

    /// Block until the transfer finishes or `timeout` elapses. A failed
    /// rsync returns an error immediately; it never waits out the clock.
    pub fn wait_complete(&self, timeout: Duration) -> io::Result<()> {
        let t0 = Instant::now();
        loop {
            match self.poll() {
                RunState::DoneOk => return Ok(()),
                RunState::Failed => {
                    return Err(io::Error::other(format!("rsync failed: {}", self.origin)));
                }
                RunState::Running => {
                    if t0.elapsed() > timeout {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("rsync staging timed out: {}", self.origin),
                        ));
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
            }
        }
    }

    fn read_staged(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let f = File::open(&self.local_path)?;
        Ok(f.read_at(buf, offset)?)
    }
}

impl Drop for RsyncSource {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait(); // reap — no zombies, no orphans past this
        }
    }
}

/// Collision-proof spool name: basename + hash of the full remote path,
/// extension preserved.
fn spool_path(spool_dir: &Path, remote_path: &str) -> PathBuf {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    remote_path.hash(&mut h);
    let base = Path::new(remote_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("staged");
    let ext = Path::new(remote_path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let safe: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    spool_dir.join(format!("{}-{:x}{ext}", safe, h.finish()))
}

impl ByteSource for RsyncSource {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || offset >= self.total_len {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(self.total_len - offset) as usize;
        let deadline = Instant::now() + self.read_timeout;
        loop {
            let staged = self.staged_len();
            if staged > offset {
                let n =
                    self.read_staged(offset, &mut buf[..want.min((staged - offset) as usize)])?;
                if n > 0 {
                    return Ok(n);
                }
                // 0 despite staged > offset: racing truncate — re-poll.
            }
            match self.poll() {
                RunState::DoneOk => {
                    // Final size wins over the estimate.
                    let staged = self.staged_len();
                    if offset >= staged {
                        return Ok(0);
                    }
                    return self
                        .read_staged(offset, &mut buf[..want.min((staged - offset) as usize)]);
                }
                RunState::Failed => {
                    return Err(io::Error::other(format!("rsync failed: {}", self.origin)));
                }
                RunState::Running => {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("staged read timed out: {}", self.origin),
                        ));
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
            }
        }
    }

    fn len(&self) -> u64 {
        self.total_len
    }

    fn describe(&self) -> String {
        format!("rsync:{} -> {}", self.origin, self.local_path.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static IDS: AtomicUsize = AtomicUsize::new(0);

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "lyra-rsync-{tag}-{}-{}",
            std::process::id(),
            IDS.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Slow copier: 8 × 32 KiB chunks with a pause, then done.
    fn chunked_copy(src: &Path, dst: &Path) -> Command {
        let mut c = Command::new("sh");
        c.args([
            "-c",
            &format!(
                "for i in 1 2 3 4 5 6 7 8; do dd if='{}' of='{}' bs=32768 skip=$((i-1)) seek=$((i-1)) count=1 conv=notrunc status=none; sleep 0.1; done",
                src.display(),
                dst.display()
            ),
        ]);
        c
    }

    fn fixture(bytes: usize) -> Vec<u8> {
        (0..bytes).map(|i| (i * 13 % 251) as u8).collect()
    }

    #[test]
    fn progressive_read_beats_transfer() {
        let dir = tmpdir("prog");
        let data = fixture(256 * 1024);
        let src_file = dir.join("orig.bin");
        std::fs::write(&src_file, &data).unwrap();
        let spool = dir.join("spool");
        std::fs::create_dir_all(&spool).unwrap();
        let partial = spool_path(&spool, "/remote/orig.bin");
        // pre-create so conv=notrunc has a target from byte 0
        std::fs::write(&partial, vec![0u8; data.len()]).unwrap();
        std::fs::remove_file(&partial).unwrap();

        let t0 = Instant::now();
        let rs = RsyncSource::launch(
            chunked_copy(&src_file, &partial),
            "test:/remote/orig.bin",
            partial.clone(),
            data.len() as u64,
        )
        .unwrap();

        // First chunk lands after ~0.1s; the whole transfer takes ~0.8s.
        // Staged reads return short while the writer is mid-chunk, so drain
        // like a real consumer — the point is it completes long before the
        // transfer finishes, never blocking on it.
        let mut head = vec![0u8; 4096];
        let mut got = 0;
        while got < head.len() {
            got += rs.read_at(got as u64, &mut head[got..]).unwrap();
        }
        assert_eq!(&head, &data[..4096]);
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "read blocked on full transfer?"
        );

        rs.wait_complete(Duration::from_secs(15)).unwrap();
        assert!(rs.is_complete());
        assert_eq!(std::fs::read(&partial).unwrap(), data);

        // post-completion random access + EOF clamp
        let mut buf = vec![0u8; 1024];
        let off = data.len() as u64 - 100;
        assert_eq!(rs.read_at(off, &mut buf).unwrap(), 100);
        assert_eq!(&buf[..100], &data[off as usize..]);
        assert_eq!(rs.read_at(data.len() as u64, &mut buf).unwrap(), 0);
        assert_eq!(rs.len(), data.len() as u64);
    }

    #[test]
    fn failed_child_errors_dont_hang() {
        let dir = tmpdir("fail");
        let partial = spool_path(&dir, "/r/f.bin");
        let mut fail = Command::new("sh");
        fail.args(["-c", "exit 3"]);
        let rs = RsyncSource::launch(fail, "test:/r/f.bin", partial, 100).unwrap();
        assert!(rs.wait_complete(Duration::from_secs(5)).is_err());
        let mut buf = [0u8; 8];
        assert!(rs.read_at(0, &mut buf).is_err());
    }

    #[test]
    fn read_timeout_when_stalled() {
        let dir = tmpdir("stall");
        let partial = spool_path(&dir, "/r/s.bin");
        let mut sleep = Command::new("sh");
        sleep.args(["-c", "exec sleep 30"]);
        let rs = RsyncSource::launch(sleep, "test:/r/s.bin", partial, 1 << 20)
            .unwrap()
            .with_read_timeout(Duration::from_millis(200));
        let mut buf = [0u8; 8];
        let err = rs.read_at(0, &mut buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn drop_kills_child() {
        let dir = tmpdir("kill");
        let sentinel = dir.join("ticks");
        let partial = spool_path(&dir, "/r/k.bin");
        let mut looper = Command::new("sh");
        looper.args([
            "-c",
            &format!(
                "while true; do date >> '{}'; sleep 0.05; done",
                sentinel.display()
            ),
        ]);
        {
            let _rs = RsyncSource::launch(looper, "test:/r/k.bin", partial, 1 << 20).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            assert!(sentinel.is_file(), "child never started?");
        } // Drop → SIGKILL + reap
        let size_at_drop = std::fs::metadata(&sentinel).map(|m| m.len()).unwrap_or(0);
        assert!(size_at_drop > 0);
        std::thread::sleep(Duration::from_millis(400));
        let size_later = std::fs::metadata(&sentinel).map(|m| m.len()).unwrap_or(0);
        assert_eq!(size_at_drop, size_later, "child kept running after drop");
    }

    #[test]
    fn spool_naming() {
        let dir = Path::new("/spool");
        let a = spool_path(dir, "/mnt/music/Weird Name (2024).flac");
        assert!(a.starts_with(dir));
        assert_eq!(a.extension().and_then(|e| e.to_str()), Some("flac"));
        assert_ne!(a, spool_path(dir, "/mnt/music/Weird Name (2025).flac"));
        assert_ne!(a, spool_path(dir, "/other/Weird Name (2024).flac"));
    }
}
