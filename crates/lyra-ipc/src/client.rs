//! Blocking socket client: `call()`, `subscribe()` with automatic
//! `state.get` resync on sequence gaps.

use serde_json::Value;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;
use thiserror::Error;

use crate::paths::candidates;
use crate::protocol::{ErrorBody, Event, Hello, Request, Response, line_is_event, to_line};

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("lyra is not running (no socket answered at {0})")]
    NotRunning(String),
    #[error("transport: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("server error [{code:?}]: {message}")]
    Server { code: crate::protocol::ErrorCode, message: String },
}

impl ClientError {
    /// CLI exit code (§6 rule 7): 0 ok, 2 not-running, 3 conflict, 4 invalid.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::NotRunning(_) => 2,
            Self::Server { code, .. } => match code {
                crate::protocol::ErrorCode::Conflict => 3,
                _ => 1,
            },
            Self::Protocol(_) => 4,
            Self::Io(_) => 2,
        }
    }
}

impl From<ErrorBody> for ClientError {
    fn from(e: ErrorBody) -> Self {
        Self::Server {
            code: e.code,
            message: e.message,
        }
    }
}

/// Blocking client over one Unix socket connection.
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
}

impl Client {
    pub fn connect(path: &Path) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path).map_err(|_| {
            ClientError::NotRunning(path.display().to_string())
        })?;
        Self::from_stream(stream)
    }

    /// Discovery order per §7: `LYRA_SOCKET` first, then well-known paths.
    pub fn discover_and_connect() -> Result<Self, ClientError> {
        let paths = candidates();
        for p in &paths {
            if let Ok(stream) = UnixStream::connect(p) {
                return Self::from_stream(stream);
            }
        }
        Err(ClientError::NotRunning(
            paths
                .first()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        ))
    }

    fn from_stream(stream: UnixStream) -> Result<Self, ClientError> {
        let writer = stream.try_clone()?;
        let mut c = Self {
            reader: BufReader::new(stream),
            writer,
            next_id: 0,
        };
        // Consume the hello line (§6 rule 9); missing hello = protocol error.
        let hello = c.read_line()?;
        let _: Hello = serde_json::from_str(hello.trim())
            .map_err(|e| ClientError::Protocol(format!("bad hello: {e}")))?;
        Ok(c)
    }

    pub fn set_read_timeout(&mut self, dur: Option<Duration>) -> std::io::Result<()> {
        self.reader.get_ref().set_read_timeout(dur)
    }

    fn read_line(&mut self) -> Result<String, ClientError> {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line)?;
        if n == 0 {
            return Err(ClientError::Protocol("connection closed".into()));
        }
        Ok(line)
    }

    fn send(&mut self, req: &Request) -> Result<(), ClientError> {
        self.writer.write_all(to_line(req).as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    /// Read lines until a method response arrives; stray events are skipped
    /// (they only occur on subscribed connections — use `subscribe()` there).
    fn read_response(&mut self) -> Result<Response, ClientError> {
        loop {
            let line = self.read_line()?;
            if line_is_event(&line) {
                continue;
            }
            let resp: Response = serde_json::from_str(line.trim())
                .map_err(|e| ClientError::Protocol(format!("bad response: {e}")))?;
            return Ok(resp);
        }
    }

    /// One request–response round trip. Server errors become `Err`.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Response, ClientError> {
        self.next_id += 1;
        let id = Value::from(format!("c{}", self.next_id));
        self.send(&Request::new(id, method, params))?;
        let resp = self.read_response()?;
        if resp.ok {
            Ok(resp)
        } else if let Some(e) = resp.error.clone() {
            Err(e.into())
        } else {
            Err(ClientError::Protocol("response ok=false without error".into()))
        }
    }

    pub fn state(&mut self) -> Result<Value, ClientError> {
        let r = self.call("state.get", Value::Object(Default::default()))?;
        r.snapshot
            .clone()
            .ok_or_else(|| ClientError::Protocol("state.get without snapshot".into()))
    }

    /// Open a push event stream on this connection. The returned
    /// [`Subscription`] owns the client; `call()` must not be used after.
    pub fn subscribe(mut self, topics: &[&str]) -> Result<Subscription, ClientError> {
        let params = serde_json::json!({"topics": topics});
        self.next_id += 1;
        let id = Value::from(format!("c{}", self.next_id));
        self.send(&Request::new(id, "subscribe", params))?;
        // The subscribe ack arrives before any replayed retained events.
        let ack = self.read_response()?;
        if !ack.ok {
            return Err(ack.error.clone().unwrap().into());
        }
        Ok(Subscription {
            client: self,
            last_seq: 0,
            pending: VecDeque::new(),
        })
    }
}

/// Item yielded by [`Subscription::next_event`].
#[derive(Debug)]
pub enum SubItem {
    Event(Event),
    /// A `seq` gap was seen; the client already re-fetched current state.
    Resync { snapshot: Value, from_seq: u64, to_seq: u64 },
}

pub struct Subscription {
    client: Client,
    last_seq: u64,
    pending: VecDeque<Event>,
}

impl Subscription {
    /// Next event, blocking. On a `seq` gap the client auto-issues `state.get`
    /// on the same connection and yields `Resync` first (spec §1.2).
    pub fn next_event(&mut self) -> Result<SubItem, ClientError> {
        if let Some(ev) = self.pending.pop_front() {
            self.last_seq = ev.seq;
            return Ok(SubItem::Event(ev));
        }
        loop {
            let line = self.client.read_line()?;
            if !line_is_event(&line) {
                // Stray response line (shouldn't happen); skip.
                continue;
            }
            let ev: Event = serde_json::from_str(line.trim())
                .map_err(|e| ClientError::Protocol(format!("bad event: {e}")))?;
            if self.last_seq != 0 && ev.seq != self.last_seq + 1 {
                let from = self.last_seq;
                let snapshot = self.resync_state()?;
                self.last_seq = ev.seq;
                self.pending.push_back(ev);
                return Ok(SubItem::Resync {
                    snapshot,
                    from_seq: from,
                    to_seq: self.last_seq,
                });
            }
            self.last_seq = ev.seq;
            return Ok(SubItem::Event(ev));
        }
    }

    /// `state.get` round trip on the subscribed connection: request lines and
    /// the response are framed the same way, so events arriving mid-resync
    /// are stashed into `pending` in order.
    fn resync_state(&mut self) -> Result<Value, ClientError> {
        self.client.next_id += 1;
        let id = Value::from(format!("c{}", self.client.next_id));
        self.client
            .send(&Request::new(id.clone(), "state.get", Value::Object(Default::default())))?;
        loop {
            let line = self.client.read_line()?;
            if line_is_event(&line) {
                if let Ok(ev) = serde_json::from_str::<Event>(line.trim()) {
                    self.pending.push_back(ev);
                }
                continue;
            }
            let resp: Response = serde_json::from_str(line.trim())
                .map_err(|e| ClientError::Protocol(format!("bad response: {e}")))?;
            if resp.id != id {
                continue;
            }
            return resp
                .snapshot
                .clone()
                .ok_or_else(|| ClientError::Protocol("state.get without snapshot".into()));
        }
    }

    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }
}
