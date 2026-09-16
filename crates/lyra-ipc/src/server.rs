//! [`Server`]: accept loop, per-connection threads, stale-socket reaping,
//! sidecar liveness file. All client-facing parse errors become protocol
//! error responses — the socket never panics the host app.

use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

use crate::dispatcher::{ApiError, DispatchCtx, Dispatcher, check_revision};
use crate::events::EventBus;
use crate::jobs::{JobStore, JobState};
use crate::paths::sidecar_path;
use crate::protocol::{
    ErrorCode, Event, Hello, Id, MAX_LINE_BYTES, OPERATIONS, Request, Response,
    capabilities_payload, is_async_op, is_mutating_op, to_line,
};

/// Bound socket server. Owns the broadcast bus and job store; the player
/// backend is the `dispatcher` (thin adapter in the later in-app wiring).
pub struct Server<D: Dispatcher> {
    dispatcher: Arc<D>,
    bus: Arc<EventBus>,
    jobs: Arc<JobStore>,
    inline_jobs: AtomicU64,
}

impl<D: Dispatcher> Server<D> {
    pub fn new(dispatcher: D) -> Self {
        Self {
            dispatcher: Arc::new(dispatcher),
            bus: Arc::new(EventBus::new()),
            jobs: Arc::new(JobStore::new()),
            inline_jobs: AtomicU64::new(0),
        }
    }

    pub fn ctx(&self) -> DispatchCtx {
        DispatchCtx {
            bus: Arc::clone(&self.bus),
            jobs: Arc::clone(&self.jobs),
        }
    }

    /// Bind `path`, reaping a stale socket when nothing answers there.
    /// Refuses with `AddrInUse` when a live server answers.
    pub fn serve_on_path(self, path: &Path) -> std::io::Result<ServerHandle> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }
        if path.exists() {
            match UnixStream::connect(path) {
                Ok(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AddrInUse,
                        format!("lyra server already live at {}", path.display()),
                    ));
                }
                Err(_) => {
                    // Stale socket: nobody answers → reap and rebind (§1.1).
                    let _ = std::fs::remove_file(path);
                    let _ = std::fs::remove_file(sidecar_path(path));
                }
            }
        }
        let listener = UnixListener::bind(path)?;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        write_sidecar(path);

        let this = Arc::new(self);
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);
        let socket_path = path.to_path_buf();
        let worker = Arc::clone(&this);
        listener.set_nonblocking(true)?;
        let thread = std::thread::Builder::new()
            .name("lyra-ipc-accept".into())
            .spawn(move || {
                while !flag.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let worker = Arc::clone(&worker);
                            std::thread::Builder::new()
                                .name("lyra-ipc-conn".into())
                                .spawn(move || worker.handle_conn(stream))
                                .ok();
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        Err(_) => {
                            if flag.load(Ordering::SeqCst) {
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                    }
                }
            })
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        Ok(ServerHandle {
            path: socket_path,
            shutdown,
            thread: Some(thread),
            _keep: this,
        })
    }

    fn handle_conn(&self, stream: UnixStream) {
        let writer_stream = match stream.try_clone() {
            Ok(s) => s,
            Err(_) => return,
        };
        // Single writer: responses and events share one bounded queue so
        // lines never interleave on the socket.
        let (tx, rx) = mpsc::sync_channel::<String>(128);
        std::thread::Builder::new()
            .name("lyra-ipc-writer".into())
            .spawn(move || {
                let mut s = writer_stream;
                for line in rx {
                    if s.write_all(line.as_bytes()).is_err() || s.flush().is_err() {
                        break;
                    }
                }
            })
            .ok();

        // Register the connection for broadcast (topics set by `subscribe`).
        let sub_id = self.bus.add_sender(tx.clone(), Vec::new());
        let _unsub = Unsub {
            bus: &self.bus,
            id: sub_id,
        };

        // Version handshake first line (§6 rule 9).
        let _ = tx.send(to_line(&Hello::new()));

        let mut reader = ConnReader::new(stream);
        let null_id = Id::Null;
        loop {
            let line = match reader.read_line() {
                Ok(None) => break, // EOF
                Ok(Some(l)) => l,
                Err(_) => {
                    let _ = tx.send(to_line(&Response::err(
                        null_id.clone(),
                        ErrorCode::ParseError,
                        format!("line exceeds {MAX_LINE_BYTES} bytes"),
                    )));
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let req: Request = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(to_line(&Response::err(
                        null_id.clone(),
                        ErrorCode::ParseError,
                        format!("invalid request: {e}"),
                    )));
                    continue;
                }
            };
            if req.method == "subscribe" {
                self.handle_subscribe(&req, &tx, sub_id);
                continue;
            }
            let resp = self.dispatch(&req);
            // Blocking send: the writer drains continuously; if the client is
            // gone the writer has exited and this fails fast.
            if tx.send(to_line(&resp)).is_err() {
                break;
            }
        }
    }

    fn handle_subscribe(&self, req: &Request, tx: &mpsc::SyncSender<String>, sub_id: u64) {
        let topics: Vec<String> = req
            .params
            .get("topics")
            .and_then(|t| t.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if topics.is_empty() {
            let _ = tx.send(to_line(&Response::err(
                req.id.clone(),
                ErrorCode::InvalidParam,
                "subscribe needs topics:[…]",
            )));
            return;
        }
        self.bus.update_subscriber(sub_id, topics.clone());
        let _ = tx.send(to_line(&Response::ok_subscribed(
            req.id.clone(),
            topics.clone(),
        )));
        // Replay retained topics so the new subscriber sees current state.
        for (seq, topic, data) in self.bus.retained_matching(&topics) {
            let _ = tx.try_send(to_line(&Event::new(seq, topic, data)));
        }
    }

    fn dispatch(&self, req: &Request) -> Response {
        if req.version != crate::protocol::PROTOCOL_VERSION {
            return Response::err(
                req.id.clone(),
                ErrorCode::InvalidParam,
                format!("unsupported version {}", req.version),
            );
        }
        let params = if req.params.is_null() {
            Value::Object(Default::default())
        } else {
            req.params.clone()
        };
        match req.method.as_str() {
            "capabilities" => {
                let payload = capabilities_payload();
                let mut r = Response::ok_result(req.id.clone(), payload.clone());
                r.operations = payload.get("operations").cloned();
                r
            }
            "state.get" => Response::ok_snapshot(req.id.clone(), self.dispatcher.snapshot()),
            "spectrum.get" => match guarded_call(
                &self.dispatcher,
                &self.ctx(),
                "spectrum.get",
                &params,
            ) {
                Ok(v) => Response::ok_result(req.id.clone(), v),
                Err(e) => Response::err(req.id.clone(), e.code, e.message),
            },
            "operation.submit" => self.submit(req, &params),
            "job.get" => {
                let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                match self.jobs.get(id) {
                    Some(job) => Response::ok_job(
                        req.id.clone(),
                        serde_json::to_value(&job).unwrap_or(Value::Null),
                        None,
                    ),
                    None => {
                        Response::err(req.id.clone(), ErrorCode::NotFound, format!("job {id}"))
                    }
                }
            }
            "job.cancel" => {
                let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                match self.jobs.cancel(id) {
                    Some(job) => {
                        let v = serde_json::to_value(&job).unwrap_or(Value::Null);
                        self.bus.emit("runtime.job", v.clone());
                        Response::ok_job(req.id.clone(), v, None)
                    }
                    None => Response::err(
                        req.id.clone(),
                        ErrorCode::NotFound,
                        format!("job {id} unknown or already terminal"),
                    ),
                }
            }
            "plugin.call" => match guarded_call(
                &self.dispatcher,
                &self.ctx(),
                "plugin.call",
                &params,
            ) {
                Ok(v) => Response::ok_result(req.id.clone(), v),
                Err(e) => Response::err(req.id.clone(), e.code, e.message),
            },
            // Bare op-table names route like operation.submit (thin convenience).
            other if OPERATIONS.iter().any(|(name, _)| *name == other) => {
                self.submit_direct(req, other, &params)
            }
            other => Response::err(
                req.id.clone(),
                ErrorCode::UnknownMethod,
                format!("unknown method: {other}"),
            ),
        }
    }

    /// `operation.submit {operation, params?, if_revision?, if_playlist_revision?}`.
    fn submit(&self, req: &Request, params: &Value) -> Response {
        let obj = params.as_object();
        let operation = obj
            .and_then(|o| o.get("operation"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if operation.is_empty() || !OPERATIONS.iter().any(|(n, _)| *n == operation) {
            return Response::err(
                req.id.clone(),
                ErrorCode::InvalidParam,
                "operation.submit needs a known operation",
            );
        }
        let inner = obj
            .and_then(|o| o.get("params"))
            .cloned()
            .unwrap_or(Value::Object(Default::default()));
        let if_rev = obj.and_then(|o| o.get("if_revision")).and_then(Value::as_i64);
        let if_pl = obj
            .and_then(|o| o.get("if_playlist_revision"))
            .and_then(Value::as_i64);
        self.run_operation(req, operation, &inner, if_rev, if_pl)
    }

    fn submit_direct(&self, req: &Request, operation: &str, params: &Value) -> Response {
        let obj = params.as_object();
        let if_rev = obj.and_then(|o| o.get("if_revision")).and_then(Value::as_i64);
        let if_pl = obj
            .and_then(|o| o.get("if_playlist_revision"))
            .and_then(Value::as_i64);
        // `params.params` nesting is not used on the direct path.
        let inner = obj
            .and_then(|o| o.get("params"))
            .cloned()
            .unwrap_or_else(|| params.clone());
        self.run_operation(req, operation, &inner, if_rev, if_pl)
    }

    fn run_operation(
        &self,
        req: &Request,
        operation: &str,
        params: &Value,
        if_rev: Option<i64>,
        if_pl: Option<i64>,
    ) -> Response {
        if let Err(msg) = check_revision(&self.dispatcher.snapshot(), if_rev, if_pl) {
            return Response::err(req.id.clone(), ErrorCode::Conflict, msg);
        }
        if is_async_op(operation) {
            let job = self.jobs.create(operation);
            let job_json = serde_json::to_value(&job).unwrap_or(Value::Null);
            self.bus.emit("runtime.job", job_json.clone());
            // Worker thread owns one clone each; never blocks the conn loop.
            let dispatcher = Arc::clone(&self.dispatcher);
            let bus = Arc::clone(&self.bus);
            let jobs = Arc::clone(&self.jobs);
            let op = operation.to_string();
            let params = params.clone();
            let job_id = job.id.clone();
            std::thread::Builder::new()
                .name(format!("lyra-job-{job_id}"))
                .spawn(move || {
                    let ctx = DispatchCtx {
                        bus: Arc::clone(&bus),
                        jobs: Arc::clone(&jobs),
                    };
                    if let Some(running) = jobs.set_running(&job_id) {
                        bus.emit(
                            "runtime.job",
                            serde_json::to_value(&running).unwrap_or(Value::Null),
                        );
                    }
                    match guarded_call(&dispatcher, &ctx, &op, &params) {
                        Ok(result) => {
                            if let Some(done) = jobs.succeed(&job_id, result) {
                                bus.emit(
                                    "runtime.job",
                                    serde_json::to_value(&done).unwrap_or(Value::Null),
                                );
                            }
                        }
                        Err(e) => {
                            if let Some(failed) =
                                jobs.fail(&job_id, format!("{:?}: {}", e.code, e.message))
                            {
                                bus.emit(
                                    "runtime.job",
                                    serde_json::to_value(&failed).unwrap_or(Value::Null),
                                );
                            }
                        }
                    }
                })
                .ok();
            let snap = self.dispatcher.snapshot();
            Response::ok_job(req.id.clone(), job_json, Some(snap))
        } else {
            match guarded_call(&self.dispatcher, &self.ctx(), operation, params) {
                Ok(result) => {
                    // Read-your-writes: mutating ops echo the post-commit snapshot.
                    let snapshot = if is_mutating_op(operation) {
                        let snap = self.dispatcher.snapshot();
                        self.bus.emit("runtime.state", snap.clone());
                        Some(snap)
                    } else {
                        None
                    };
                    let n = self.inline_jobs.fetch_add(1, Ordering::SeqCst) + 1;
                    let job = serde_json::json!({
                        "id": format!("inline-{n}"),
                        "operation": operation,
                        "state": JobState::Succeeded,
                        "result": result,
                    });
                    Response::ok_job(req.id.clone(), job, snapshot)
                }
                Err(e) => Response::err(req.id.clone(), e.code, e.message),
            }
        }
    }
}

/// Call into player code without ever panicking the host: panics become
/// `internal` error responses.
fn guarded_call<D: Dispatcher>(
    dispatcher: &Arc<D>,
    ctx: &DispatchCtx,
    method: &str,
    params: &Value,
) -> Result<Value, ApiError> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dispatcher.call(ctx, method, params)
    }));
    match result {
        Ok(r) => r,
        Err(_) => Err(ApiError::new(
            ErrorCode::Internal,
            format!("{method} panicked (caught at IPC boundary)"),
        )),
    }
}

/// Newline reader with an explicit leftover buffer: pipelined lines in one
/// write are all preserved, and over-long lines are drained (not kept) so
/// the connection survives.
struct ConnReader {
    stream: UnixStream,
    buf: Vec<u8>,
}

impl ConnReader {
    fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            buf: Vec::new(),
        }
    }

    fn read_line(&mut self) -> Result<Option<String>, ()> {
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
                let line = String::from_utf8_lossy(&self.buf[..pos + 1]).to_string();
                self.buf.drain(..pos + 1);
                return Ok(Some(line));
            }
            if self.buf.len() > MAX_LINE_BYTES {
                self.drain_until_newline();
                return Err(());
            }
            let mut chunk = [0u8; 8192];
            let n = self.stream.read(&mut chunk).map_err(|_| ())?;
            if n == 0 {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                let line = String::from_utf8_lossy(&self.buf).to_string();
                self.buf.clear();
                return Ok(Some(line));
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    fn drain_until_newline(&mut self) {
        self.buf.clear();
        let mut chunk = [0u8; 8192];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if let Some(pos) = chunk[..n].iter().position(|&b| b == b'\n') {
                        self.buf.extend_from_slice(&chunk[pos + 1..n]);
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }
}

struct Unsub<'a> {
    bus: &'a EventBus,
    id: u64,
}

impl Drop for Unsub<'_> {
    fn drop(&mut self) {
        self.bus.remove_subscriber(self.id);
    }
}

/// Handle returned to the embedder. `shutdown()` stops the accept loop and
/// removes the socket + sidecar (clean shutdown, §1.1).
pub struct ServerHandle {
    path: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    _keep: Arc<dyn Send + Sync>,
}

impl ServerHandle {
    pub fn socket_path(&self) -> &Path {
        &self.path
    }

    pub fn shutdown(mut self) {
        self.shutdown_inner();
    }

    fn shutdown_inner(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(sidecar_path(&self.path));
    }
}

impl Drop for ServerHandle {
    /// Drop detaches (crash-simulation leaves the socket behind so a later
    /// bind exercises stale-socket reaping). Call `shutdown()` for clean exit.
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn write_sidecar(path: &Path) {
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let sidecar = crate::protocol::Sidecar {
        pid: std::process::id(),
        protocol: crate::protocol::PROTOCOL_VERSION,
        version: env!("CARGO_PKG_VERSION").to_string(),
        started,
    };
    if let Ok(raw) = serde_json::to_string(&sidecar) {
        let _ = std::fs::write(sidecar_path(path), raw);
    }
}
