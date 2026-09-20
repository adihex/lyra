//! Observability: request-ID propagation + in-process metrics over the
//! real encrypted loopback path (no mocks).

use lyra_core::PlayerCommand;
use lyra_remote::{host_with_echo, Client};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};

static PORT: AtomicU16 = AtomicU16::new(25100);

async fn spawn_host(dir: &std::path::Path) -> (std::sync::Arc<lyra_remote::Host>, SocketAddr) {
    let host = host_with_echo(dir.join("remote-key.bin")).unwrap();
    let port = PORT.fetch_add(1, Ordering::SeqCst);
    let h = std::sync::Arc::clone(&host);
    tokio::spawn(async move { lyra_remote::serve(h, port).await });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    (host, addr)
}

#[tokio::test]
async fn request_id_echoes_and_metrics_count() {
    let dir = std::env::temp_dir().join(format!("lyra-remote-obs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (host, addr) = spawn_host(&dir).await;

    let client = Client::new();
    let code = host.open_pairing();
    let mut s = client.pair(addr, "obs-phone", &code).await.unwrap();
    s.hello().await.unwrap();

    // Explicit ID propagates end to end: echoed in the response.
    let resp = s
        .send_with_id(&PlayerCommand::Toggle, "obs-req-1")
        .await
        .unwrap();
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["rid"], "obs-req-1");

    // Minted IDs work too and are well-formed.
    let resp2 = s.send(&PlayerCommand::Next).await.unwrap();
    assert_eq!(resp2["ok"], true);
    let rid = resp2["rid"].as_str().expect("server echoes rid");
    assert!(lyra_remote::trace::is_valid_request_id(rid));

    // Metrics counted the connection, pairing, and both commands.
    let m = host.metrics();
    assert!(m.connections_total >= 1, "{m:?}");
    assert_eq!(m.pairings_ok, 1);
    assert!(m.commands_total >= 2, "{m:?}");
    assert_eq!(m.command_errors, 0);

    let text = host.metrics_text();
    assert!(text.contains("lyra_remote_commands_total"));
    assert!(text.contains("lyra_remote_connections_total"));
}
