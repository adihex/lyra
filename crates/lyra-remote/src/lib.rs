//! lyra-remote: LAN remote-control — the layer BitMuse got wrong.
//!
//! Protocol (BLUEPRINT.md § remote):
//!   pairing   SPAKE2(6-digit code) → Noise_XXpsk3(psk=spake output)
//!             → host pins the client's X25519 static (device record)
//!   reconnect Noise_XX — mutual static-key auth; client static must
//!             match a pinned device, host static is TOFU-pinned client-side
//!   transport length-prefixed AEAD frames, JSON commands inside
//!
//! A 4-digit PIN is never a credential: the code only *binds* one pairing
//! handshake — credentials are the pinned X25519 keys it establishes.
//! Pairing attempts are throttled per-IP and device names are claims, not
//! identities (the pinned key is the identity).

use lyra_core::{LyraError, PlayerCommand};

pub mod metrics;
pub mod trace;

use sha2::{Digest, Sha256};
use snow::params::NoiseParams;
use snow::{Builder, HandshakeState, TransportState};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn, Instrument};

const MAX_CONN: usize = 4;
const THROTTLE_AFTER: u32 = 5;
const THROTTLE_WINDOW: Duration = Duration::from_secs(60);
const MAX_FRAME: usize = 1 << 16;
/// Patterns: pairing adds psk3 (SPAKE2 output) to XX.
const PAIR_PATTERN: &str = "Noise_XXpsk3_25519_ChaChaPoly_SHA256";
const CONN_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_SHA256";

fn params(s: &str) -> NoiseParams {
    NoiseParams::from_str(s).expect("noise params")
}

// ── Keys ─────────────────────────────────────────────────────────────────

/// A persisted X25519 keypair — the host's pinned identity.
pub struct StaticKey {
    pub private: [u8; 32],
    pub public: [u8; 32],
}

impl StaticKey {
    pub fn generate() -> Result<Self, LyraError> {
        let kp = Builder::new(params(CONN_PATTERN))
            .generate_keypair()
            .map_err(|e| LyraError::Remote(e.to_string()))?;
        let mut private = [0u8; 32];
        let mut public = [0u8; 32];
        private.copy_from_slice(&kp.private[..32]);
        public.copy_from_slice(&kp.public[..32]);
        Ok(Self { private, public })
    }

    /// Load or create at `path` (0600). Public key exposed for fingerprint UI.
    pub fn load_or_create(path: &std::path::Path) -> Result<Self, LyraError> {
        if let Ok(raw) = std::fs::read(path) {
            if raw.len() == 64 {
                let mut private = [0u8; 32];
                let mut public = [0u8; 32];
                private.copy_from_slice(&raw[..32]);
                public.copy_from_slice(&raw[32..]);
                return Ok(Self { private, public });
            }
        }
        let k = Self::generate()?;
        let mut blob = Vec::with_capacity(64);
        blob.extend_from_slice(&k.private);
        blob.extend_from_slice(&k.public);
        std::fs::write(path, &blob)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(k)
    }

    /// Short fingerprint for display/verification ("Lyra key 3F4A 9C…").
    pub fn fingerprint(&self) -> String {
        let h = Sha256::digest(self.public);
        h.iter()
            .take(6)
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// ── Device store ─────────────────────────────────────────────────────────

fn hex32(b: &[u8; 32]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Paired devices, keyed by their X25519 static — the key IS the identity,
/// the name is a display claim. SHA-256 of pubkey stored, compared in CT.
/// Persisted as JSON {hash_hex: name} so paired clients survive restarts.
#[derive(Default)]
pub struct DeviceStore {
    /// sha256(client_static) -> device name
    pub devices: HashMap<[u8; 32], String>,
    path: Option<PathBuf>,
}

impl DeviceStore {
    /// Load the persisted device list. Missing/corrupt file → empty store.
    pub fn load(path: PathBuf) -> Self {
        let devices = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
            .map(|m| {
                m.into_iter()
                    .filter_map(|(k, v)| unhex32(&k).map(|h| (h, v)))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            devices,
            path: Some(path),
        }
    }

    /// tmp + rename so a crash mid-write can't truncate the ACL. 0600.
    fn save(&self) {
        let Some(path) = &self.path else { return };
        let m: HashMap<String, &String> = self.devices.iter().map(|(k, v)| (hex32(k), v)).collect();
        let Ok(json) = serde_json::to_string_pretty(&m) else {
            return;
        };
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, json).is_err() {
            return;
        }
        if std::fs::rename(&tmp, path).is_err() {
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }

    fn contains(&self, static_key: &[u8]) -> bool {
        let h: [u8; 32] = Sha256::digest(static_key).into();
        self.devices.keys().any(|k| k.ct_eq(&h).into())
    }
    fn register(&mut self, static_key: &[u8], name: String) {
        let h: [u8; 32] = Sha256::digest(static_key).into();
        self.devices.insert(h, name);
        self.save();
    }
    fn revoke(&mut self, hash: &[u8; 32]) -> bool {
        let removed = self.devices.remove(hash).is_some();
        if removed {
            self.save();
        }
        removed
    }
}

// ── Throttle ─────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct Throttle {
    attempts: HashMap<SocketAddr, (u32, Instant)>,
}

impl Throttle {
    fn check(&mut self, addr: SocketAddr) -> Result<(), Duration> {
        let now = Instant::now();
        let entry = self.attempts.entry(addr).or_insert((0, now));
        if now.duration_since(entry.1) > THROTTLE_WINDOW {
            *entry = (0, now);
        }
        if entry.0 >= THROTTLE_AFTER {
            return Err(THROTTLE_WINDOW - now.duration_since(entry.1));
        }
        Ok(())
    }
    fn fail(&mut self, addr: SocketAddr) {
        let entry = self.attempts.entry(addr).or_insert((0, Instant::now()));
        entry.0 += 1;
    }
}

// ── Wire framing ─────────────────────────────────────────────────────────

async fn read_frame(s: &mut TcpStream) -> Result<Vec<u8>, LyraError> {
    let len = s.read_u32().await? as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(LyraError::Remote("bad frame".into()));
    }
    let mut buf = vec![0u8; len];
    s.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn write_frame(s: &mut TcpStream, data: &[u8]) -> Result<(), LyraError> {
    s.write_u32(data.len() as u32).await?;
    s.write_all(data).await?;
    Ok(())
}

// ── Command sink ─────────────────────────────────────────────────────────

/// Where commands land — the app injects the engine; tests inject an echo.
pub trait CommandSink: Send + Sync {
    fn handle(&self, cmd: &PlayerCommand) -> serde_json::Value;
}

struct EchoSink;
impl CommandSink for EchoSink {
    fn handle(&self, cmd: &PlayerCommand) -> serde_json::Value {
        serde_json::json!({"ok": true, "got": cmd})
    }
}

// ── Host ─────────────────────────────────────────────────────────────────

/// One open pairing window: a fresh code + the SPAKE2 responder keyed by it.
struct PairingSession {
    code: String,
}

pub struct Host {
    key: StaticKey,
    devices: Mutex<DeviceStore>,
    throttle: Mutex<Throttle>,
    pairing: Mutex<Option<PairingSession>>,
    sink: Arc<dyn CommandSink>,
    conns: Arc<Mutex<usize>>,
    metrics: Arc<metrics::HostMetrics>,
}

impl Host {
    pub fn new(key_path: &std::path::Path, sink: Arc<dyn CommandSink>) -> Result<Self, LyraError> {
        Ok(Self {
            key: StaticKey::load_or_create(key_path)?,
            devices: Mutex::new(DeviceStore::load(
                key_path.with_file_name("paired-devices.json"),
            )),
            throttle: Mutex::new(Throttle::default()),
            pairing: Mutex::new(None),
            sink,
            conns: Arc::new(Mutex::new(0)),
            metrics: Arc::new(metrics::HostMetrics::default()),
        })
    }

    /// Show this fingerprint next to the pairing code in the UI.
    pub fn fingerprint(&self) -> String {
        self.key.fingerprint()
    }

    /// Current in-process metrics snapshot (counters for connections,
    /// pairings, commands, rejects). The desktop app has no scrape
    /// endpoint; read this from debug tooling or [`Host::log_metrics`].
    #[must_use]
    pub fn metrics(&self) -> metrics::HostSnapshot {
        self.metrics.snapshot()
    }

    /// [`Host::metrics`] rendered as Prometheus exposition-style text.
    #[must_use]
    pub fn metrics_text(&self) -> String {
        self.metrics.render_text()
    }

    /// Emit the current snapshot as one structured log event — the
    /// low-tech "debug endpoint" for a desktop app: `RUST_LOG=lyra=info`
    /// plus log shipping gets these wherever they need to go.
    pub fn log_metrics(&self) {
        let s = self.metrics.snapshot();
        info!(
            connections_total = s.connections_total,
            pair_attempts = s.pair_attempts,
            pairings_ok = s.pairings_ok,
            connects_ok = s.connects_ok,
            rejects = s.rejects,
            commands_total = s.commands_total,
            command_errors = s.command_errors,
            "remote metrics"
        );
    }

    /// Open a pairing window — returns the 6-digit code to display.
    /// One window at a time; a new window rotates the code.
    pub fn open_pairing(&self) -> String {
        use rand::Rng;
        let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000u32));
        *self.pairing.lock().unwrap() = Some(PairingSession { code: code.clone() });
        code
    }

    pub fn close_pairing(&self) {
        *self.pairing.lock().unwrap() = None;
    }

    /// Device count (tests + UI badge).
    pub fn paired_count(&self) -> usize {
        self.devices.lock().unwrap().devices.len()
    }

    /// Paired devices for the UI: (sha256 hex of the pinned static, name).
    pub fn devices(&self) -> Vec<(String, String)> {
        self.devices
            .lock()
            .unwrap()
            .devices
            .iter()
            .map(|(k, v)| (hex32(k), v.clone()))
            .collect()
    }

    /// Remove a pinned device by its hash hex — it must re-pair to connect.
    pub fn revoke(&self, hash_hex: &str) -> bool {
        let Some(h) = unhex32(hash_hex) else {
            return false;
        };
        self.devices.lock().unwrap().revoke(&h)
    }

    // -- connection entry points -------------------------------------------

    async fn handle_conn(self: &Arc<Self>, mut sock: TcpStream, addr: SocketAddr) {
        // Every connection gets an operation ID at accept time; it rides
        // the `remote_conn` span so accept → handshake → command loop is
        // one trace even though the transport is raw TCP, not HTTP.
        let conn_id = trace::new_request_id();
        let span = tracing::info_span!("remote_conn", %addr, conn_id = %conn_id);
        let this = Arc::clone(self);
        async move {
            this.metrics.inc_connections();
            {
                let mut n = this.conns.lock().unwrap();
                if *n >= MAX_CONN {
                    this.metrics.inc_rejects();
                    warn!("conn limit");
                    return;
                }
                *n += 1;
            }
            let _ = this.dispatch(&mut sock, addr).await;
            *this.conns.lock().unwrap() -= 1;
        }
        .instrument(span)
        .await;
    }

    async fn dispatch(&self, sock: &mut TcpStream, addr: SocketAddr) -> Result<(), LyraError> {
        let hello = read_frame(sock).await?;
        let op: serde_json::Value =
            serde_json::from_slice(&hello).map_err(|_| LyraError::Remote("bad hello".into()))?;
        match op["op"].as_str() {
            Some("pair") => self.do_pair(sock, addr, &op).await,
            Some("connect") => self.do_connect(sock, addr).await,
            _ => Err(LyraError::Remote("unknown op".into())),
        }
    }

    /// SPAKE2 → XXpsk3 → pin client static → encrypted command loop.
    #[tracing::instrument(skip(self, sock, hello), fields(peer = %addr))]
    async fn do_pair(
        &self,
        sock: &mut TcpStream,
        addr: SocketAddr,
        hello: &serde_json::Value,
    ) -> Result<(), LyraError> {
        self.metrics.inc_pair_attempts();
        {
            let mut t = self.throttle.lock().unwrap();
            if t.check(addr).is_err() {
                self.metrics.inc_rejects();
                warn!("pair throttled");
                return Ok(());
            }
        }
        let code = self
            .pairing
            .lock()
            .unwrap()
            .as_ref()
            .map(|p| p.code.clone());
        let Some(code) = code else {
            self.metrics.inc_rejects();
            write_frame(sock, br#"{"error":"pairing_closed"}"#).await?;
            return Ok(());
        };
        let name = hello["name"].as_str().unwrap_or("device").to_string();
        let client_spake = read_frame(sock).await?;

        let (b_side, b_msg) = Spake2::<Ed25519Group>::start_b(
            &Password::new(code.as_bytes()),
            &Identity::new(b"lyra-remote"),
            &Identity::new(name.as_bytes()),
        );
        write_frame(sock, &b_msg).await?;
        let psk: [u8; 32] = match b_side.finish(&client_spake) {
            Ok(k) => {
                let mut p = [0u8; 32];
                p.copy_from_slice(&Sha256::digest(&k)[..32]);
                p
            }
            Err(_) => {
                self.throttle.lock().unwrap().fail(addr);
                self.metrics.inc_rejects();
                write_frame(sock, br#"{"error":"bad_code"}"#).await?;
                return Ok(());
            }
        };

        let hs = Builder::new(params(PAIR_PATTERN))
            .local_private_key(&self.key.private)
            .map_err(|e| LyraError::Remote(e.to_string()))?
            .psk(3, &psk)
            .map_err(|e| LyraError::Remote(e.to_string()))?
            .build_responder()
            .map_err(|e| LyraError::Remote(e.to_string()))?;

        let ts = match noise_handshake_responder(sock, hs).await {
            Ok(t) => t,
            Err(e) => {
                self.throttle.lock().unwrap().fail(addr);
                self.metrics.inc_rejects();
                return Err(e);
            }
        };
        let client_static = ts
            .get_remote_static()
            .ok_or_else(|| LyraError::Remote("no client static".into()))?
            .to_vec();
        self.devices
            .lock()
            .unwrap()
            .register(&client_static, name.clone());
        info!(%addr, %name, "device paired");
        self.metrics.inc_pairings_ok();
        self.command_loop(sock, ts, "paired").await
    }

    /// Plain XX — client static must already be pinned.
    #[tracing::instrument(skip(self, sock), fields(peer = %addr))]
    async fn do_connect(&self, sock: &mut TcpStream, addr: SocketAddr) -> Result<(), LyraError> {
        let hs = Builder::new(params(CONN_PATTERN))
            .local_private_key(&self.key.private)
            .map_err(|e| LyraError::Remote(e.to_string()))?
            .build_responder()
            .map_err(|e| LyraError::Remote(e.to_string()))?;
        let ts = noise_handshake_responder(sock, hs).await?;
        let client_static = ts
            .get_remote_static()
            .ok_or_else(|| LyraError::Remote("no client static".into()))?
            .to_vec();
        if !self.devices.lock().unwrap().contains(&client_static) {
            self.metrics.inc_rejects();
            warn!("unknown device key — closing");
            return Ok(());
        }
        info!("paired device connected");
        self.metrics.inc_connects_ok();
        self.command_loop(sock, ts, "connected").await
    }

    /// Encrypted JSON command loop until EOF. Each command carries the
    /// client's request ID (see [`trace`]) and runs inside a
    /// `remote_command` span, so one phone tap is one trace.
    #[tracing::instrument(skip(self, sock, ts))]
    async fn command_loop(
        &self,
        sock: &mut TcpStream,
        mut ts: TransportState,
        hello: &str,
    ) -> Result<(), LyraError> {
        send_enc(
            sock,
            &mut ts,
            &serde_json::json!({"event": hello, "fp": self.fingerprint()}),
        )
        .await?;
        let mut op_index: u64 = 0;
        loop {
            let ct = match read_frame(sock).await {
                Ok(f) => f,
                Err(_) => return Ok(()), // clean close
            };
            let mut pt = vec![0u8; ct.len()];
            let n = ts
                .read_message(&ct, &mut pt)
                .map_err(|_| LyraError::Remote("decrypt".into()))?;
            op_index += 1;
            let (request_id, cmd) = match trace::decode_command(&pt[..n]) {
                Ok(v) => v,
                Err(_) => {
                    self.metrics.inc_command_errors();
                    warn!(op = op_index, "bad command frame");
                    send_enc(sock, &mut ts, &serde_json::json!({"error":"bad_command"})).await?;
                    continue;
                }
            };
            let span = tracing::info_span!(
                "remote_command",
                request_id = %request_id,
                op = op_index
            );
            let resp = span.in_scope(|| self.sink.handle(&cmd));
            self.metrics.inc_commands();
            // Echo the request ID back (additive field — existing clients
            // reading `ok`/`event` are unaffected) so the phone can
            // correlate responses with the taps that caused them.
            let mut resp = resp;
            if let Some(obj) = resp.as_object_mut() {
                obj.insert("rid".to_string(), serde_json::Value::String(request_id));
            }
            send_enc(sock, &mut ts, &resp).await?;
        }
    }
}

/// Drive the responder side of an XX handshake over framed TCP.
async fn noise_handshake_responder(
    sock: &mut TcpStream,
    mut hs: HandshakeState,
) -> Result<TransportState, LyraError> {
    let mut buf = vec![0u8; MAX_FRAME];
    // XX: ->e / <-e,ee,s,es / ->s,se
    let m1 = read_frame(sock).await?;
    hs.read_message(&m1, &mut buf)
        .map_err(|e| LyraError::Remote(format!("hs1: {e}")))?;
    let n = hs
        .write_message(&[], &mut buf)
        .map_err(|e| LyraError::Remote(format!("hs2w: {e}")))?;
    write_frame(sock, &buf[..n]).await?;
    let m3 = read_frame(sock).await?;
    hs.read_message(&m3, &mut buf)
        .map_err(|e| LyraError::Remote(format!("hs3: {e}")))?;
    hs.into_transport_mode()
        .map_err(|e| LyraError::Remote(e.to_string()))
}

async fn send_enc(
    sock: &mut TcpStream,
    ts: &mut TransportState,
    v: &serde_json::Value,
) -> Result<(), LyraError> {
    let pt = serde_json::to_vec(v).map_err(|e| LyraError::Remote(e.to_string()))?;
    let mut buf = vec![0u8; pt.len() + 64];
    let n = ts
        .write_message(&pt, &mut buf)
        .map_err(|e| LyraError::Remote(e.to_string()))?;
    write_frame(sock, &buf[..n]).await
}

// ── Client ───────────────────────────────────────────────────────────────
// The phone-side implementation in Rust — doubles as the test driver and
// the reference for the eventual iOS companion app's protocol layer.

pub struct Client {
    key: StaticKey,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        Self {
            key: StaticKey::generate().expect("keygen"),
        }
    }

    /// Pair with `code` → encrypted session. `name` is the display claim.
    pub async fn pair(
        &self,
        addr: SocketAddr,
        name: &str,
        code: &str,
    ) -> Result<Session, LyraError> {
        let mut sock = TcpStream::connect(addr).await?;
        write_frame(
            &mut sock,
            &serde_json::json!({"op":"pair","name":name})
                .to_string()
                .into_bytes(),
        )
        .await?;

        let (a_side, a_msg) = Spake2::<Ed25519Group>::start_a(
            &Password::new(code.as_bytes()),
            &Identity::new(b"lyra-remote"),
            &Identity::new(name.as_bytes()),
        );
        write_frame(&mut sock, &a_msg).await?;
        let b_msg = read_frame(&mut sock).await?;
        // server may refuse inline
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b_msg) {
            if v.get("error").is_some() {
                return Err(LyraError::Remote(
                    v["error"].as_str().unwrap_or("pair refused").into(),
                ));
            }
        }
        let k = a_side
            .finish(&b_msg)
            .map_err(|_| LyraError::Remote("spake failed".into()))?;
        let mut psk = [0u8; 32];
        psk.copy_from_slice(&Sha256::digest(&k)[..32]);

        let hs = Builder::new(params(PAIR_PATTERN))
            .local_private_key(&self.key.private)
            .map_err(|e| LyraError::Remote(e.to_string()))?
            .psk(3, &psk)
            .map_err(|e| LyraError::Remote(e.to_string()))?
            .build_initiator()
            .map_err(|e| LyraError::Remote(e.to_string()))?;
        let ts = noise_handshake_initiator(&mut sock, hs).await?;
        Ok(Session { sock, ts })
    }

    /// Reconnect with the pinned static — no code needed.
    pub async fn connect(&self, addr: SocketAddr) -> Result<Session, LyraError> {
        let mut sock = TcpStream::connect(addr).await?;
        write_frame(&mut sock, br#"{"op":"connect"}"#).await?;
        let hs = Builder::new(params(CONN_PATTERN))
            .local_private_key(&self.key.private)
            .map_err(|e| LyraError::Remote(e.to_string()))?
            .build_initiator()
            .map_err(|e| LyraError::Remote(e.to_string()))?;
        let ts = noise_handshake_initiator(&mut sock, hs).await?;
        Ok(Session { sock, ts })
    }

    /// Reconnect but verify the host is still the pinned key (TOFU check
    /// client-side: pass the fingerprint learned at pairing time).
    pub async fn connect_verify(
        &self,
        addr: SocketAddr,
        expected_host_pub: &[u8; 32],
    ) -> Result<Session, LyraError> {
        let mut sock = TcpStream::connect(addr).await?;
        write_frame(&mut sock, br#"{"op":"connect"}"#).await?;
        let hs = Builder::new(params(CONN_PATTERN))
            .local_private_key(&self.key.private)
            .map_err(|e| LyraError::Remote(e.to_string()))?
            .remote_public_key(expected_host_pub)
            .map_err(|e| LyraError::Remote(e.to_string()))?
            .build_initiator()
            .map_err(|e| LyraError::Remote(e.to_string()))?;
        let ts = noise_handshake_initiator(&mut sock, hs).await?;
        Ok(Session { sock, ts })
    }
}

/// Drive the initiator side of an XX handshake.
async fn noise_handshake_initiator(
    sock: &mut TcpStream,
    mut hs: HandshakeState,
) -> Result<TransportState, LyraError> {
    let mut buf = vec![0u8; MAX_FRAME];
    let n = hs
        .write_message(&[], &mut buf)
        .map_err(|e| LyraError::Remote(format!("hs1w: {e}")))?;
    write_frame(sock, &buf[..n]).await?;
    let m2 = read_frame(sock).await?;
    hs.read_message(&m2, &mut buf)
        .map_err(|e| LyraError::Remote(format!("hs2: {e}")))?;
    let n = hs
        .write_message(&[], &mut buf)
        .map_err(|e| LyraError::Remote(format!("hs3w: {e}")))?;
    write_frame(sock, &buf[..n]).await?;
    hs.into_transport_mode()
        .map_err(|e| LyraError::Remote(e.to_string()))
}

/// An established encrypted session — post-pairing or post-connect.
pub struct Session {
    sock: TcpStream,
    ts: TransportState,
}

impl Session {
    /// The host's pinned static, learned during the handshake (pin it).
    pub fn host_static(&self) -> Option<[u8; 32]> {
        let s = self.ts.get_remote_static()?;
        let mut k = [0u8; 32];
        k.copy_from_slice(&s[..32]);
        Some(k)
    }

    /// Read the server's first frame ({event:"paired"|"connected",fp}).
    pub async fn hello(&mut self) -> Result<serde_json::Value, LyraError> {
        self.recv().await
    }

    pub async fn send(&mut self, cmd: &PlayerCommand) -> Result<serde_json::Value, LyraError> {
        self.send_with_id(cmd, &trace::new_request_id()).await
    }

    /// Send with an explicit request ID — the `X-Request-ID` handling:
    /// callers propagating an ID from elsewhere pass it here and the
    /// server echoes it back in the response's `rid` field.
    pub async fn send_with_id(
        &mut self,
        cmd: &PlayerCommand,
        request_id: &str,
    ) -> Result<serde_json::Value, LyraError> {
        let request_id = trace::normalize_request_id(Some(request_id));
        let span = tracing::info_span!("remote_request", request_id = %request_id);
        async move {
            let payload = serde_json::json!({ "rid": request_id, "cmd": cmd });
            send_enc(&mut self.sock, &mut self.ts, &payload).await?;
            self.recv().await
        }
        .instrument(span)
        .await
    }

    pub async fn recv(&mut self) -> Result<serde_json::Value, LyraError> {
        let ct = read_frame(&mut self.sock).await?;
        let mut pt = vec![0u8; ct.len()];
        let n = self
            .ts
            .read_message(&ct, &mut pt)
            .map_err(|_| LyraError::Remote("decrypt".into()))?;
        serde_json::from_slice(&pt[..n]).map_err(|e| LyraError::Remote(e.to_string()))
    }
}

// ── Serve ────────────────────────────────────────────────────────────────

/// Binds all interfaces — LAN reachability is the feature; the security
/// boundary is the handshake, not the bind address.
pub async fn serve(host: Arc<Host>, port: u16) -> Result<(), LyraError> {
    let listener = TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(LyraError::Io)?;
    info!(port, "lyra-remote listening");
    loop {
        let (sock, addr) = listener.accept().await.map_err(LyraError::Io)?;
        let h = Arc::clone(&host);
        tokio::spawn(async move { h.handle_conn(sock, addr).await });
    }
}

/// Default host on loopback-friendly config for tests/examples.
pub fn host_with_echo(key_path: PathBuf) -> Result<Arc<Host>, LyraError> {
    Host::new(&key_path, Arc::new(EchoSink)).map(Arc::new)
}
