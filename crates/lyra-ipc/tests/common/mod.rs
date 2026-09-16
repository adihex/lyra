//! Shared test harness: mock-backed server on a unique temp socket.
#![allow(dead_code)]

use lyra_ipc::{mock::MockDispatcher, Client, Server, ServerHandle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn temp_socket(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "lyra-test-{}-{}-{}-{}.sock",
        name,
        std::process::id(),
        n,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}

pub struct TestServer {
    pub path: PathBuf,
    pub handle: Option<ServerHandle>,
}

impl TestServer {
    pub fn start() -> Self {
        Self::start_with(MockDispatcher::new())
    }

    pub fn start_with(d: MockDispatcher) -> Self {
        let path = temp_socket("srv");
        let handle = Server::new(d)
            .serve_on_path(&path)
            .expect("bind test server");
        // Give the accept loop a moment to come up.
        let mut client = connect_retry(&path);
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        drop(client);
        Self {
            path,
            handle: Some(handle),
        }
    }

    pub fn client(&self) -> Client {
        let mut c = Client::connect(&self.path).expect("connect test client");
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c
    }

    pub fn shutdown(mut self) {
        if let Some(h) = self.handle.take() {
            h.shutdown();
        }
        assert!(
            !self.path.exists(),
            "socket file must be removed on clean shutdown"
        );
        assert!(
            !lyra_ipc::paths::sidecar_path(&self.path).exists(),
            "sidecar must be removed on clean shutdown"
        );
    }
}

fn connect_retry(path: &std::path::Path) -> Client {
    for _ in 0..50 {
        if let Ok(c) = Client::connect(path) {
            return c;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("test server never answered at {}", path.display());
}

pub fn submit_ok(
    c: &mut Client,
    operation: &str,
    params: serde_json::Value,
) -> lyra_ipc::protocol::Response {
    let r = c
        .call(
            "operation.submit",
            serde_json::json!({"operation": operation, "params": params}),
        )
        .expect("submit ok");
    assert!(r.ok, "expected ok response");
    r
}

pub fn revision_of(snap: &serde_json::Value) -> (i64, i64) {
    (
        snap["revision"].as_i64().unwrap(),
        snap["playlist_revision"].as_i64().unwrap(),
    )
}
