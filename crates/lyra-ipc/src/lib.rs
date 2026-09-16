//! Lyra agent-native IPC: NDJSON protocol v2 over a Unix domain socket.
//!
//! Layout per `docs/research/agent-native-design.md` §1:
//! - [`protocol`] — envelope, responses, events, error taxonomy, op table.
//! - [`dispatcher`] — the [`dispatcher::Dispatcher`] trait. The later in-app
//!   wiring (`lyra_ipc_start(path)` in lyra-ffi + an engine adapter) only needs
//!   to implement this one trait; jobs, revisions, broadcast and retained
//!   topics are all owned here.
//! - [`server`] — accept loop, per-connection threads, stale-socket reaping,
//!   sidecar liveness file, broadcast event bus.
//! - [`client`] — blocking [`client::Client`] with `call()`, `subscribe()`
//!   and automatic `state.get` resync on sequence gaps.
//! - [`paths`] — socket discovery precedence (`LYRA_SOCKET` first).
//! - [`mock`] — [`mock::MockDispatcher`] for tests and offline development.

pub mod client;
pub mod dispatcher;
pub mod events;
pub mod jobs;
pub mod mock;
pub mod paths;
pub mod protocol;
pub mod server;

pub use client::Client;
pub use dispatcher::{ApiError, DispatchCtx, Dispatcher};
pub use protocol::{ErrorCode, PROTOCOL_VERSION};
pub use server::{Server, ServerHandle};
