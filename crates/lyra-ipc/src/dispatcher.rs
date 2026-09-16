//! [`Dispatcher`]: the single seam between the socket server and the player.
//!
//! The later in-app wiring is a thin adapter: implement `call()` + `snapshot()`
//! against the engine. Jobs, revisions, subscriptions, retained topics and the
//! broadcast bus all live in this crate, so the adapter stays dumb.
//!
//! Method routing performed by the server:
//! - `state.get` → [`Dispatcher::snapshot`] (no adapter code needed)
//! - `capabilities` → op table in [`crate::protocol`] (no adapter code needed)
//! - `operation.submit {operation, params}` → revision guard → sync inline, or
//!   async via the shared [`crate::jobs::JobStore`] worker for
//!   `library.scan` / `torrent.add`
//! - `plugin.call` → `call("plugin.call", …)`
//! - any op-table name sent as a bare method (e.g. `play`) is routed to
//!   `call()` the same way `operation.submit` routes it.

use serde_json::Value;
use std::sync::Arc;
use thiserror::Error;

use crate::events::EventBus;
use crate::jobs::JobStore;
use crate::protocol::{ErrorBody, ErrorCode};

/// Error returned by [`Dispatcher::call`].
#[derive(Debug, Clone, Error)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn not_found(what: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, what.into())
    }

    pub fn invalid_param(what: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidParam, what.into())
    }

    pub fn body(&self) -> ErrorBody {
        ErrorBody::new(self.code, self.message.clone())
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

/// Context handed to every dispatcher call: emit broadcast events
/// (e.g. `plugin.<name>` topics, `library.scan` progress) here.
#[derive(Clone)]
pub struct DispatchCtx {
    pub bus: Arc<EventBus>,
    pub jobs: Arc<JobStore>,
}

impl DispatchCtx {
    pub fn emit(&self, topic: &str, data: Value) {
        self.bus.emit(topic, data);
    }
}

/// Player backend. Must never panic: the server converts `Err` into protocol
/// error responses, and wraps `call` in `catch_unwind` as a backstop.
pub trait Dispatcher: Send + Sync + 'static {
    /// Full snapshot for `state.get`. Must always contain numeric `revision`
    /// and `playlist_revision` (optimistic-concurrency guards, §1.2).
    fn snapshot(&self) -> Value;

    /// Execute one op-table method (`play`, `queue.enqueue`, `plugin.call`,
    /// …). Return the op result; the server attaches the post-commit snapshot
    /// for mutating ops (read-your-writes).
    fn call(&self, ctx: &DispatchCtx, method: &str, params: &Value) -> Result<Value, ApiError>;
}

/// Revision-guard check for `operation.submit` params.
/// Returns `Ok(())`, or the conflict message when stale.
pub fn check_revision(
    snapshot: &Value,
    if_revision: Option<i64>,
    if_playlist_revision: Option<i64>,
) -> Result<(), String> {
    if let Some(want) = if_revision {
        let have = snapshot
            .get("revision")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if want != have {
            return Err(format!("stale revision: have {have}, if_revision={want}"));
        }
    }
    if let Some(want) = if_playlist_revision {
        let have = snapshot
            .get("playlist_revision")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if want != have {
            return Err(format!(
                "stale playlist revision: have {have}, if_playlist_revision={want}"
            ));
        }
    }
    Ok(())
}
