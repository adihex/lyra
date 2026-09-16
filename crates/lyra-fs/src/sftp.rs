//! `SftpSource`: random-access remote reads over SFTP via `ssh2` (sync,
//! session-per-source). The fixed 1 MiB `CachingSource` block layer above
//! absorbs per-read latency; this module only guarantees it never fetches
//! more than asked — no full-file transfers behind a `read_at`.
//!
//! The [`SftpBackend`]/[`SftpHandle`] seam keeps every behavior test
//! hermetic: unit tests run against a tempdir-backed double that mimics
//! the open → fstat → seek+read pattern. A live-server smoke test sits
//! behind `LYRA_TEST_SSH=user@host:/path/to/file`.

use std::io::{self, Read, Seek, SeekFrom};
use std::net::ToSocketAddrs;
use std::sync::Mutex;
use std::time::Duration;

use lyra_core::LyraError;

use super::{AuthMethod, ByteSource, RemoteProfile};

const KEEPALIVE_SECS: u32 = 10;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// One open remote file: length via fstat, reads via seek+read.
pub trait SftpHandle: Send {
    fn len(&mut self) -> io::Result<u64>;
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;
}

/// Opens named remote files. Implemented by [`Ssh2Backend`]; tests use a
/// tempdir-backed double. `Send + Sync` so sources compose with
/// `CachingSource` and cross threads like every other `ByteSource`.
pub trait SftpBackend: Send + Sync {
    type Handle: SftpHandle;
    fn open(&self, path: &str) -> io::Result<Self::Handle>;
}

// ── live ssh2 backend ────────────────────────────────────────────────────

/// Direct-TCP SFTP backend. `host` should be a resolvable name or IP —
/// `ssh(1)` config aliases are resolved through `ssh -G` (hostname, port,
/// user) when the host isn't dotted, so both `adi-linux` and raw hosts
/// work with one profile shape.
#[derive(Debug, Clone)]
pub struct Ssh2Backend {
    profile: RemoteProfile,
    auth: AuthMethod,
    timeout: Duration,
    strict_host_key: bool,
}

impl Ssh2Backend {
    pub fn new(profile: &RemoteProfile) -> Self {
        let auth = profile.default_auth();
        Self {
            profile: profile.clone(),
            auth,
            timeout: CONNECT_TIMEOUT,
            strict_host_key: false,
        }
    }

    pub fn with_auth(mut self, auth: AuthMethod) -> Self {
        self.auth = auth;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Fail the connect when the host key isn't in `~/.ssh/known_hosts`.
    /// Off by default (matches `SshExecFile` semantics, which trusts ssh's
    /// own config); enable for unattended/production use.
    pub fn with_strict_host_key(mut self) -> Self {
        self.strict_host_key = true;
        self
    }

    /// Open one authenticated session + SFTP channel. `pub(crate)` — the
    /// remote walker reuses it for directory listing.
    pub(crate) fn connect(&self) -> Result<(ssh2::Session, ssh2::Sftp), LyraError> {
        let (host, port) = self.profile.tcp_endpoint();
        let user = self.profile.login_user();
        let addr = (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|e| LyraError::Remote(format!("resolve {host}: {e}")))?
            .next()
            .ok_or_else(|| LyraError::Remote(format!("resolve {host}: no address")))?;
        let stream = std::net::TcpStream::connect_timeout(&addr, self.timeout)?;
        let mut sess =
            ssh2::Session::new().map_err(|e| LyraError::Remote(format!("ssh session: {e}")))?;
        sess.set_tcp_stream(stream);
        sess.handshake()
            .map_err(|e| LyraError::Remote(format!("ssh handshake {host}: {e}")))?;
        if self.strict_host_key {
            check_host_key(&sess, &host, port)?;
        }
        self.authenticate(&sess, &user)?;
        if !sess.authenticated() {
            return Err(LyraError::Remote(format!(
                "ssh auth failed for {user}@{host}"
            )));
        }
        sess.set_keepalive(true, KEEPALIVE_SECS);
        let sftp = sess
            .sftp()
            .map_err(|e| LyraError::Remote(format!("sftp subsystem: {e}")))?;
        Ok((sess, sftp))
    }

    fn authenticate(&self, sess: &ssh2::Session, user: &str) -> Result<(), LyraError> {
        let auth_err = |e: ssh2::Error| LyraError::Remote(format!("ssh auth: {e}"));
        match &self.auth {
            AuthMethod::Agent => {
                if sess.userauth_agent(user).is_err() {
                    auth_default_keys(sess, user)?;
                }
                Ok(())
            }
            AuthMethod::KeyFile { path, passphrase } => sess
                .userauth_pubkey_file(user, None, path, passphrase.as_deref())
                .map_err(auth_err),
            AuthMethod::Password(pw) => sess.userauth_password(user, pw).map_err(auth_err),
            AuthMethod::PasswordCallback(cb) => {
                let pw =
                    cb.0().ok_or_else(|| LyraError::Remote("password prompt declined".into()))?;
                sess.userauth_password(user, &pw).map_err(auth_err)
            }
        }
    }
}

/// `~/.ssh/id_ed25519` → `id_ecdsa` → `id_rsa`: first existing key that
/// the server accepts wins.
fn auth_default_keys(sess: &ssh2::Session, user: &str) -> Result<(), LyraError> {
    let home = std::env::var("HOME").unwrap_or_default();
    for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
        let key = std::path::PathBuf::from(format!("{home}/.ssh/{name}"));
        if !key.is_file() {
            continue;
        }
        if sess.userauth_pubkey_file(user, None, &key, None).is_ok() && sess.authenticated() {
            return Ok(());
        }
    }
    Err(LyraError::Remote(format!(
        "ssh auth failed for {user} (agent + default keys)"
    )))
}

fn check_host_key(sess: &ssh2::Session, host: &str, port: u16) -> Result<(), LyraError> {
    use ssh2::{CheckResult, KnownHostFileKind};
    let (key, _kind) = sess
        .host_key()
        .ok_or_else(|| LyraError::Remote("no server host key".into()))?;
    let home = std::env::var("HOME").unwrap_or_default();
    let mut kh = sess
        .known_hosts()
        .map_err(|e| LyraError::Remote(format!("known_hosts: {e}")))?;
    kh.read_file(
        std::path::Path::new(&format!("{home}/.ssh/known_hosts")),
        KnownHostFileKind::OpenSSH,
    )
    .map_err(|e| LyraError::Remote(format!("read known_hosts: {e}")))?;
    match kh.check_port(host, port, key) {
        CheckResult::Match => Ok(()),
        CheckResult::Mismatch => Err(LyraError::Remote(format!("HOST KEY MISMATCH for {host}"))),
        other => Err(LyraError::Remote(format!("unknown host {host}: {other:?}"))),
    }
}

/// `ssh -G host` effective-config lookup. Only the live path calls it;
/// [`parse_ssh_g`] holds the parsing for unit tests.
pub(crate) fn ssh_g(host: &str, key: &str) -> Option<String> {
    let out = std::process::Command::new("ssh")
        .args(["-G", host])
        .output()
        .ok()?;
    parse_ssh_g(&String::from_utf8_lossy(&out.stdout), key)
}

pub(crate) fn parse_ssh_g(output: &str, key: &str) -> Option<String> {
    output
        .lines()
        .filter_map(|l| l.split_once(' '))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_string())
}

impl SftpBackend for Ssh2Backend {
    type Handle = Ssh2Handle;
    fn open(&self, path: &str) -> io::Result<Self::Handle> {
        let (sess, sftp) = self
            .connect()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let file = sftp
            .open(std::path::Path::new(path))
            .map_err(io::Error::from)?;
        Ok(Ssh2Handle {
            _sess: sess,
            _sftp: sftp,
            file,
        })
    }
}

/// One open file on a live session. Owns session + channel + handle
/// (all `Send`), so the source stays `Sync` behind a `Mutex`.
pub struct Ssh2Handle {
    _sess: ssh2::Session,
    _sftp: ssh2::Sftp,
    file: ssh2::File,
}

impl SftpHandle for Ssh2Handle {
    fn len(&mut self) -> io::Result<u64> {
        let st = self.file.stat().map_err(io::Error::from)?;
        st.size
            .ok_or_else(|| io::Error::other("sftp fstat: no size"))
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(io::Error::from)?;
        let mut got = 0;
        while got < buf.len() {
            match self.file.read(&mut buf[got..]) {
                Ok(0) => break, // EOF
                Ok(n) => got += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(got)
    }
}

// ── source ───────────────────────────────────────────────────────────────

/// Random-access remote file: `len()` from fstat at open, `read_at` via
/// seek+read on a reused handle, one reconnect on a dead connection.
pub struct SftpSource<B: SftpBackend = Ssh2Backend> {
    path: String,
    origin: String,
    len: u64,
    handle: Mutex<B::Handle>,
    // Last: backends that lend their handles borrowed state (test doubles
    // holding raw pointers) must outlive the handle. Field drop order is
    // declaration order, so this drops last.
    backend: B,
}

impl SftpSource<Ssh2Backend> {
    /// Open `path` on `profile` with the profile's implied auth
    /// (key file if set, else agent).
    pub fn open(profile: &RemoteProfile, path: &str) -> Result<Self, LyraError> {
        let auth = profile.default_auth();
        Self::open_with_auth(profile, path, &auth)
    }

    /// Open with explicit credentials (password / prompt callback).
    pub fn open_with_auth(
        profile: &RemoteProfile,
        path: &str,
        auth: &AuthMethod,
    ) -> Result<Self, LyraError> {
        let backend = Ssh2Backend::new(profile).with_auth(auth.clone());
        Self::open_with(backend, &profile.source_uri(path), path)
            .map_err(|e| LyraError::Remote(format!("sftp open {}: {e}", profile.source_uri(path))))
    }
}

impl<B: SftpBackend> SftpSource<B> {
    /// Open over any backend — the test seam (and future transports).
    /// `origin` feeds `describe()`; `path` is passed back to the backend
    /// on reconnect.
    pub fn open_with(backend: B, origin: &str, path: &str) -> io::Result<Self> {
        let mut handle = backend.open(path)?;
        let len = handle.len()?;
        Ok(Self {
            path: path.into(),
            origin: origin.into(),
            len,
            handle: Mutex::new(handle),
            backend,
        })
    }

    #[cfg(test)]
    pub(crate) fn backend(&self) -> &B {
        &self.backend
    }
}

impl<B: SftpBackend> ByteSource for SftpSource<B> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || offset >= self.len {
            return Ok(0);
        }
        // Scope the guard: the reconnect path below re-locks the handle,
        // and a guard held across the match would deadlock it.
        let first = self.handle.lock().unwrap().read_at(offset, buf);
        match first {
            Ok(n) => Ok(n),
            Err(_) => {
                // One reconnect, then surface whatever the retry says —
                // a second failure is real (deleted file, perms), not a
                // dead connection.
                let mut fresh = self.backend.open(&self.path)?;
                let n = fresh.read_at(offset, buf)?;
                *self.handle.lock().unwrap() = fresh;
                Ok(n)
            }
        }
    }

    fn len(&self) -> u64 {
        self.len
    }

    fn describe(&self) -> String {
        self.origin.clone()
    }
}

// ── profile endpoint helpers ─────────────────────────────────────────────

impl RemoteProfile {
    /// TCP endpoint for direct-SSH backends. Dotted names / IPs / localhost
    /// go verbatim; anything else is treated as an `ssh(1)` alias and
    /// resolved via `ssh -G` (explicit non-22 port always wins).
    pub(crate) fn tcp_endpoint(&self) -> (String, u16) {
        let alias = self.host != "localhost"
            && !self.host.contains('.')
            && self.host.parse::<std::net::IpAddr>().is_err();
        if !alias {
            return (self.host.clone(), self.port);
        }
        let host = ssh_g(&self.host, "hostname").unwrap_or_else(|| self.host.clone());
        let port = if self.port != 22 {
            self.port
        } else {
            ssh_g(&self.host, "port")
                .and_then(|p| p.parse().ok())
                .unwrap_or(22)
        };
        (host, port)
    }

    /// Login name: profile → `ssh -G` → `$USER`.
    pub(crate) fn login_user(&self) -> String {
        if let Some(u) = &self.user {
            return u.clone();
        }
        let alias = !self.host.contains('.');
        if alias {
            if let Some(u) = ssh_g(&self.host, "user") {
                return u;
            }
        }
        std::env::var("USER").unwrap_or_else(|_| "lyra".into())
    }
}

// ── test double ──────────────────────────────────────────────────────────

/// Tempdir-backed [`SftpBackend`] that mimics the live call pattern
/// (open → fstat → seek+read on a reused `std::fs::File`). Lives behind
/// `#[cfg(test)]`; the scanner tests reuse it as well.
#[cfg(test)]
pub(crate) mod doubles {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Debug)]
    pub(crate) enum Op {
        Open(String),
        Stat,
        SeekRead { offset: u64, len: usize },
    }

    pub(crate) struct MemBackend {
        file_path: std::path::PathBuf,
        ops: Arc<Mutex<Vec<Op>>>,
        opens: Arc<AtomicUsize>,
        /// Fail this many `read_at` calls with `BrokenPipe` before serving.
        pub(crate) fail_reads: Arc<AtomicUsize>,
    }

    impl MemBackend {
        pub(crate) fn with_bytes(dir: &std::path::Path, name: &str, data: &[u8]) -> Self {
            let p = dir.join(name);
            std::fs::write(&p, data).unwrap();
            Self {
                file_path: p,
                ops: Arc::new(Mutex::new(Vec::new())),
                opens: Arc::new(AtomicUsize::new(0)),
                fail_reads: Arc::new(AtomicUsize::new(0)),
            }
        }

        pub(crate) fn opens(&self) -> usize {
            self.opens.load(Ordering::SeqCst)
        }

        pub(crate) fn max_read_offset(&self) -> u64 {
            self.ops
                .lock()
                .unwrap()
                .iter()
                .filter_map(|op| match op {
                    Op::SeekRead { offset, len } => Some(offset + *len as u64),
                    _ => None,
                })
                .max()
                .unwrap_or(0)
        }
    }

    pub(crate) struct MemHandle {
        file: std::fs::File,
        ops: Arc<Mutex<Vec<Op>>>,
        fail_reads: Arc<AtomicUsize>,
    }

    impl SftpHandle for MemHandle {
        fn len(&mut self) -> io::Result<u64> {
            self.ops.lock().unwrap().push(Op::Stat);
            Ok(self.file.metadata()?.len())
        }

        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
            if self.fail_reads.load(Ordering::SeqCst) > 0 {
                self.fail_reads.fetch_sub(1, Ordering::SeqCst);
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "injected"));
            }
            if buf.is_empty() {
                return Ok(0);
            }
            self.ops.lock().unwrap().push(Op::SeekRead {
                offset,
                len: buf.len(),
            });
            self.file.seek(SeekFrom::Start(offset))?;
            let mut got = 0;
            while got < buf.len() {
                match self.file.read(&mut buf[got..]) {
                    Ok(0) => break,
                    Ok(n) => got += n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
            Ok(got)
        }
    }

    impl SftpBackend for MemBackend {
        type Handle = MemHandle;
        fn open(&self, path: &str) -> io::Result<Self::Handle> {
            self.ops.lock().unwrap().push(Op::Open(path.into()));
            self.opens.fetch_add(1, Ordering::SeqCst);
            Ok(MemHandle {
                file: std::fs::File::open(&self.file_path)?,
                ops: Arc::clone(&self.ops),
                fail_reads: Arc::clone(&self.fail_reads),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::doubles::*;
    use super::*;
    use crate::CachingSource;
    use std::sync::atomic::Ordering;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "lyra-sftp-{}-{}-{}",
            tag,
            std::process::id(),
            next_id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    static IDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    fn next_id() -> usize {
        IDS.fetch_add(1, Ordering::SeqCst)
    }

    fn fixture(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 % 251) as u8).collect()
    }

    fn source(data: &[u8]) -> (SftpSource<MemBackend>, std::path::PathBuf) {
        let dir = tmpdir("src");
        let backend = MemBackend::with_bytes(&dir, "f.bin", data);
        let src = SftpSource::open_with(backend, "sftp://h/f.bin", "/f.bin").unwrap();
        (src, dir)
    }

    #[test]
    fn offsets_and_eof() {
        let data = fixture(3000);
        let (src, _dir) = source(&data);
        assert_eq!(src.len(), 3000);

        let mut buf = [0u8; 100];
        assert_eq!(src.read_at(0, &mut buf).unwrap(), 100);
        assert_eq!(&buf, &data[..100]);

        // straddles EOF → short read, not an error
        let mut buf = [0u8; 100];
        assert_eq!(src.read_at(2950, &mut buf).unwrap(), 50);
        assert_eq!(&buf[..50], &data[2950..]);

        // at/past EOF → 0; empty buf → 0 without touching the backend
        assert_eq!(src.read_at(3000, &mut buf).unwrap(), 0);
        assert_eq!(src.read_at(99999, &mut buf).unwrap(), 0);
        assert_eq!(src.read_at(0, &mut []).unwrap(), 0);

        // single open at construction, handle reused across reads
        assert_eq!(src.backend().opens(), 1);
    }

    #[test]
    fn empty_file() {
        let (src, _dir) = source(&[]);
        assert_eq!(src.len(), 0);
        let mut buf = [0u8; 8];
        assert_eq!(src.read_at(0, &mut buf).unwrap(), 0);
    }

    #[test]
    fn reconnect_once_then_heal() {
        let dir = tmpdir("heal");
        let backend = MemBackend::with_bytes(&dir, "f.bin", &fixture(512));
        backend.fail_reads.store(1, Ordering::SeqCst);
        let src = SftpSource::open_with(backend, "sftp://h/f", "/f").unwrap();
        assert_eq!(src.backend.opens(), 1);

        let mut buf = [0u8; 64];
        assert_eq!(src.read_at(0, &mut buf).unwrap(), 64);
        // failed read triggered exactly one reopen
        assert_eq!(src.backend.opens(), 2);
        assert_eq!(&buf, &fixture(512)[..64]);
    }

    #[test]
    fn double_failure_surfaces() {
        let dir = tmpdir("dblfail");
        let backend = MemBackend::with_bytes(&dir, "f.bin", &fixture(512));
        backend.fail_reads.store(5, Ordering::SeqCst);
        let src = SftpSource::open_with(backend, "sftp://h/f", "/f").unwrap();
        let mut buf = [0u8; 64];
        let err = src.read_at(0, &mut buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(src.backend.opens(), 2); // exactly one retry, not a loop
    }

    #[test]
    fn caching_interplay() {
        let data = fixture(3 * 1024 * 1024);
        let dir = tmpdir("cache");
        let backend = MemBackend::with_bytes(&dir, "f.bin", &data);
        let inner = SftpSource::open_with(backend, "sftp://h/f", "/f").unwrap();
        let cached = CachingSource::with_budget(inner, 256 << 20, true);

        // sequential 64 KiB reads across a 1 MiB block boundary
        let mut buf = vec![0u8; 64 << 10];
        for i in 0..20 {
            let off = i as u64 * 65536 + 12345;
            let n = cached.read_at(off, &mut buf).unwrap();
            assert_eq!(&buf[..n], &data[off as usize..off as usize + n]);
        }
        // repeat → served from cache, same bytes back
        for i in 0..20 {
            let off = i as u64 * 65536 + 12345;
            let n = cached.read_at(off, &mut buf).unwrap();
            assert_eq!(&buf[..n], &data[off as usize..off as usize + n]);
        }
        assert!(cached.describe().starts_with("cached:sftp://h/f"));
    }

    #[test]
    fn parse_ssh_g_shapes() {
        let out = "hostname 10.0.0.5\nport 2222\nuser adi\nidentityfile ~/.ssh/x\n";
        assert_eq!(parse_ssh_g(out, "hostname").as_deref(), Some("10.0.0.5"));
        assert_eq!(parse_ssh_g(out, "port").as_deref(), Some("2222"));
        assert_eq!(parse_ssh_g(out, "user").as_deref(), Some("adi"));
        assert_eq!(parse_ssh_g(out, "nope"), None);
    }

    #[test]
    fn endpoint_selection() {
        // dotted/IP/localhost go verbatim (no ssh -G call needed)
        let p = RemoteProfile::new("192.168.1.5", "/m");
        assert_eq!(p.tcp_endpoint(), ("192.168.1.5".into(), 22));
        let p = RemoteProfile::new("host.example.com", "/m").with_port(2022);
        assert_eq!(p.tcp_endpoint(), ("host.example.com".into(), 2022));
        // explicit port always wins, even for aliases
        let p = RemoteProfile::new("adi-linux", "/m").with_port(2222);
        assert_eq!(p.tcp_endpoint().1, 2222);
    }

    /// Live server smoke — skipped unless LYRA_TEST_SSH=user@host:/path.
    #[test]
    fn live_ssh_smoke() {
        let spec = std::env::var("LYRA_TEST_SSH").unwrap_or_default();
        if spec.is_empty() {
            return;
        }
        let (target, remote) = spec.split_once(':').expect("LYRA_TEST_SSH=user@host:/path");
        let (user, host) = target
            .split_once('@')
            .expect("LYRA_TEST_SSH=user@host:/path");
        let profile = RemoteProfile::new(host, "/").with_user(user);
        let src = SftpSource::open(&profile, remote).expect("sftp open");
        assert!(src.len() > 0, "empty remote file");
        let mut head = vec![0u8; 65536];
        let n = src.read_at(0, &mut head).expect("sftp read");
        assert!(n > 0);
        let cached = CachingSource::wrap(src);
        let mut again = vec![0u8; 65536];
        assert_eq!(cached.read_at(0, &mut again).unwrap(), n);
        assert_eq!(&head[..n], &again[..n]);
    }
}
