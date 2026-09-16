//! Protocol v2 types: envelope, responses, events, errors, op table.
//!
//! NDJSON both directions, one minified JSON object per line.
//! Envelope: `{"version":2,"id":<client>,"method":<verb>,"params":{…}}`.
//! Responses echo `id` + `version`. Events carry monotonic `seq`.
//! Position ticks are never events — clients poll `state.get`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol version spoken on the wire.
pub const PROTOCOL_VERSION: u32 = 2;
/// App name announced in the `hello` line.
pub const APP_NAME: &str = "lyra";
/// Maximum accepted line length (1 MiB). Longer lines get a protocol error.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Client-chosen correlation id: string or number, echoed back verbatim.
pub type Id = Value;

/// A client request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub version: u32,
    pub id: Id,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    pub fn new(id: impl Into<Id>, method: impl Into<String>, params: Value) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            id: id.into(),
            method: method.into(),
            params,
        }
    }

    /// Params as an object; non-object params decode as `{}`.
    pub fn params_obj(&self) -> serde_json::Map<String, Value> {
        self.params
            .as_object()
            .cloned()
            .unwrap_or_default()
    }
}

/// Stable error taxonomy (§6 rule 2). Never parse `message`; match `code`.
/// `retryable` tells agents whether retrying can succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotFound,
    Conflict,
    InvalidParam,
    NotRunning,
    JobFailed,
    PermissionDenied,
    Busy,
    UnknownMethod,
    ParseError,
    Internal,
}

impl ErrorCode {
    pub fn retryable(self) -> bool {
        matches!(self, Self::Conflict | Self::Busy)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl ErrorBody {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        let retryable = code.retryable();
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }
}

/// A server response. Exactly one of the payload fields is set on success;
/// `error` is set on failure. `snapshot` rides along on mutating ops
/// (read-your-writes, §6 rule 8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub version: u32,
    pub id: Id,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operations: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscribed: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

impl Response {
    fn base(id: Id, ok: bool) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            id,
            ok,
            result: None,
            snapshot: None,
            job: None,
            operations: None,
            subscribed: None,
            error: None,
        }
    }

    pub fn ok_result(id: Id, result: Value) -> Self {
        let mut r = Self::base(id, true);
        r.result = Some(result);
        r
    }

    pub fn ok_snapshot(id: Id, snapshot: Value) -> Self {
        let mut r = Self::base(id, true);
        r.snapshot = Some(snapshot);
        r
    }

    pub fn ok_job(id: Id, job: Value, snapshot: Option<Value>) -> Self {
        let mut r = Self::base(id, true);
        r.job = Some(job);
        r.snapshot = snapshot;
        r
    }

    pub fn ok_subscribed(id: Id, topics: Vec<String>) -> Self {
        let mut r = Self::base(id, true);
        r.subscribed = Some(Value::Array(
            topics.into_iter().map(Value::String).collect(),
        ));
        r
    }

    pub fn err(id: Id, code: ErrorCode, message: impl Into<String>) -> Self {
        let mut r = Self::base(id, false);
        r.error = Some(ErrorBody::new(code, message));
        r
    }

    /// The primary payload, for generic clients.
    pub fn payload(&self) -> Option<&Value> {
        self.result
            .as_ref()
            .or(self.snapshot.as_ref())
            .or(self.job.as_ref())
            .or(self.operations.as_ref())
            .or(self.subscribed.as_ref())
    }

    pub fn is_conflict(&self) -> bool {
        matches!(
            self.error.as_ref().map(|e| e.code),
            Some(ErrorCode::Conflict)
        )
    }
}

/// First line the server sends on every new connection (§6 rule 9).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub version: u32,
    pub hello: HelloBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloBody {
    pub app: String,
    pub protocol: u32,
    pub version: String,
    pub capabilities: usize,
}

impl Hello {
    pub fn new() -> Self {
        Self {
            version: PROTOCOL_VERSION,
            hello: HelloBody {
                app: APP_NAME.to_string(),
                protocol: PROTOCOL_VERSION,
                version: env!("CARGO_PKG_VERSION").to_string(),
                capabilities: OPERATIONS.len(),
            },
        }
    }
}

/// A push event on a subscribed connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub version: u32,
    #[serde(rename = "type")]
    pub kind: String,
    pub seq: u64,
    pub topic: String,
    pub data: Value,
}

impl Event {
    pub fn new(seq: u64, topic: impl Into<String>, data: Value) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            kind: "event".to_string(),
            seq,
            topic: topic.into(),
            data,
        }
    }
}

/// Returns true if a raw line looks like an event (vs a method response).
pub fn line_is_event(line: &str) -> bool {
    // Cheap pre-parse: events carry `"type":"event"`. Fall back to false.
    line.contains("\"event\"")
}

/// Serialize a value as one minified NDJSON line (with trailing `\n`).
pub fn to_line(v: &impl Serialize) -> String {
    let mut s = serde_json::to_string(v).expect("protocol value is serializable");
    s.push('\n');
    s
}

/// Sidecar liveness file `control.sock.json` (§1.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sidecar {
    pub pid: u32,
    pub protocol: u32,
    pub version: String,
    pub started: u64,
}

/// One entry of the machine-readable op table returned by `capabilities`
/// (§6 rule 1: agents self-discover, no docs needed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationDef {
    pub name: &'static str,
    pub params: Value,
}

fn schema(props: &[(&str, &str)], required: &[&str]) -> Value {
    let properties: serde_json::Map<String, Value> = props
        .iter()
        .map(|(k, t)| {
            (
                k.to_string(),
                serde_json::json!({"type": t}),
            )
        })
        .collect();
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

fn no_params() -> Value {
    serde_json::json!({"type": "object", "properties": {}})
}

/// Full method/op table: envelope methods plus every `operation.submit` op
/// from §1.2 (transport, modes, eq, queue, library, sources, introspection).
pub static OPERATIONS: &[(&str, fn() -> Value)] = &[
    // envelope methods
    ("capabilities", no_params),
    ("state.get", no_params),
    ("spectrum.get", no_params),
    ("operation.submit", || {
        schema(
            &[
                ("operation", "string"),
                ("params", "object"),
                ("if_revision", "integer"),
                ("if_playlist_revision", "integer"),
            ],
            &["operation"],
        )
    }),
    ("job.get", || schema(&[("id", "string")], &["id"])),
    ("job.cancel", || schema(&[("id", "string")], &["id"])),
    ("subscribe", || schema(&[("topics", "array")], &["topics"])),
    ("plugin.call", || {
        schema(
            &[("plugin", "string"), ("command", "string"), ("args", "object")],
            &["plugin", "command"],
        )
    }),
    // transport
    ("play", no_params),
    ("pause", no_params),
    ("toggle", no_params),
    ("stop", no_params),
    ("next", no_params),
    ("prev", no_params),
    ("seek.absolute", || {
        schema(&[("position_s", "number")], &["position_s"])
    }),
    ("seek.relative", || {
        schema(&[("delta_s", "number")], &["delta_s"])
    }),
    ("volume", || schema(&[("volume", "number")], &["volume"])),
    ("volume.set", || schema(&[("volume", "number")], &["volume"])),
    ("speed", || schema(&[("speed", "number")], &["speed"])),
    // modes
    ("shuffle", || schema(&[("enabled", "boolean")], &["enabled"])),
    ("repeat", || schema(&[("mode", "string")], &["mode"])),
    // eq
    ("eq.get", no_params),
    ("eq.set", || {
        schema(&[("bands", "array"), ("preamp", "number")], &["bands"])
    }),
    ("eq.band.set", || {
        schema(&[("band", "integer"), ("gain_db", "number")], &[
            "band", "gain_db",
        ])
    }),
    // queue
    ("queue.list", no_params),
    ("queue.play", || schema(&[("index", "integer")], &["index"])),
    ("queue.enqueue", || {
        schema(
            &[
                ("track_id", "string"),
                ("query", "string"),
                ("position", "integer"),
                ("idempotency_key", "string"),
            ],
            &[],
        )
    }),
    ("queue.remove", || schema(&[("index", "integer")], &["index"])),
    ("queue.move", || {
        schema(&[("from", "integer"), ("to", "integer")], &["from", "to"])
    }),
    ("queue.clear", no_params),
    // library
    ("library.search", || {
        schema(
            &[
                ("q", "string"),
                ("type", "string"),
                ("limit", "integer"),
            ],
            &["q"],
        )
    }),
    ("library.stats", no_params),
    ("library.scan", || schema(&[("path", "string")], &[])),
    ("track.play", || schema(&[("track_id", "string")], &["track_id"])),
    ("track.queue", || schema(&[("track_id", "string")], &["track_id"])),
    // sources
    ("url.load", || schema(&[("url", "string")], &["url"])),
    ("torrent.add", || {
        schema(&[("magnet", "string"), ("path", "string")], &[])
    }),
    ("lyrics.get", || {
        schema(&[("track_id", "string")], &["track_id"])
    }),
    // introspection
    ("device.list", no_params),
    ("device.set", || schema(&[("device", "string")], &["device"])),
];

/// Ops that run as retained jobs instead of inline results.
pub fn is_async_op(operation: &str) -> bool {
    matches!(operation, "library.scan" | "torrent.add")
}

/// Ops that mutate playback/queue state (bump `revision` / trigger
/// `runtime.state` broadcast + snapshot echo).
pub fn is_mutating_op(operation: &str) -> bool {
    !matches!(
        operation,
        "capabilities"
            | "state.get"
            | "spectrum.get"
            | "job.get"
            | "job.cancel"
            | "subscribe"
            | "eq.get"
            | "queue.list"
            | "library.search"
            | "library.stats"
            | "lyrics.get"
            | "device.list"
    )
}

pub fn capabilities_payload() -> Value {
    let operations: Vec<Value> = OPERATIONS
        .iter()
        .map(|(name, schema_fn)| {
            serde_json::json!({"name": name, "params": schema_fn()})
        })
        .collect();
    serde_json::json!({
        "protocol": PROTOCOL_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        "operations": operations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ndjson_round_trip() {
        let req = Request::new("abc", "state.get", serde_json::json!({}));
        let line = to_line(&req);
        assert!(line.ends_with('\n'));
        assert_eq!(line.lines().count(), 1);
        let back: Request = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(back.version, 2);
        assert_eq!(back.method, "state.get");

        let resp = Response::ok_snapshot(req.id.clone(), serde_json::json!({"revision": 1}));
        let line = to_line(&resp);
        let back: Response = serde_json::from_str(line.trim()).unwrap();
        assert!(back.ok);
        assert!(back.snapshot.is_some());
        assert!(back.error.is_none());

        let err = Response::err(req.id, ErrorCode::Conflict, "stale revision");
        assert!(err.is_conflict());
        assert!(err.error.as_ref().unwrap().retryable);
        let line = to_line(&err);
        let back: Response = serde_json::from_str(line.trim()).unwrap();
        assert!(!back.ok);

        let ev = Event::new(41, "runtime.playback", serde_json::json!({}));
        let line = to_line(&ev);
        assert!(line_is_event(&line));
        let back: Event = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(back.seq, 41);
        assert!(!line_is_event(&to_line(&resp)));
    }
}
