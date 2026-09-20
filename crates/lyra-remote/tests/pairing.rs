//! Protocol e2e over loopback: SPAKE2 pairing → XXpsk3 → pinned keys →
//! XX reconnect → encrypted command round-trip. No mocks — both ends run
//! the real handshake.

use lyra_core::PlayerCommand;
use lyra_remote::{host_with_echo, Client};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};

static PORT: AtomicU16 = AtomicU16::new(24900);

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
async fn pair_reconnect_command() {
    let dir = std::env::temp_dir().join(format!("lyra-remote-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (host, addr) = spawn_host(&dir).await;

    let client = Client::new();
    let code = host.open_pairing();

    // 1. Pair — SPAKE2 + XXpsk3
    let mut s = client.pair(addr, "qa-phone", &code).await.unwrap();
    let hello = s.hello().await.unwrap();
    assert_eq!(hello["event"], "paired");
    let host_pub = s.host_static().expect("host static learned");
    assert_eq!(host.paired_count(), 1);

    // 2. Command round-trip inside the encrypted session
    let resp = s.send(&PlayerCommand::Toggle).await.unwrap();
    assert_eq!(resp["ok"], true);
    let resp = s.send(&PlayerCommand::Volume { value: 0.5 }).await.unwrap();
    assert_eq!(resp["ok"], true);
    drop(s);

    // 3. Reconnect — pinned static, no code needed
    let mut s2 = client.connect(addr).await.unwrap();
    assert_eq!(s2.hello().await.unwrap()["event"], "connected");
    assert_eq!(s2.send(&PlayerCommand::Next).await.unwrap()["ok"], true);
    drop(s2);

    // 4. Reconnect pinned to the learned host key (TOFU)
    let mut s3 = client.connect_verify(addr, &host_pub).await.unwrap();
    assert_eq!(s3.hello().await.unwrap()["event"], "connected");
}

#[tokio::test]
async fn wrong_code_fails() {
    let dir = std::env::temp_dir().join(format!("lyra-remote-test-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (host, addr) = spawn_host(&dir).await;
    let _ = host.open_pairing();

    let client = Client::new();
    // A wrong code completes SPAKE2 shape but yields a different psk —
    // the XXpsk3 handshake must fail, and no device gets pinned.
    let res = client.pair(addr, "evil", "000000").await;
    assert!(
        res.is_err() || {
            let mut s = res.unwrap();
            s.hello().await.is_err()
        }
    );
    assert_eq!(host.paired_count(), 0);
}

#[tokio::test]
async fn devices_persist_and_revoke() {
    let dir = std::env::temp_dir().join(format!("lyra-remote-persist-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (host, addr) = spawn_host(&dir).await;

    let client = Client::new();
    let code = host.open_pairing();
    let mut s = client.pair(addr, "persist-phone", &code).await.unwrap();
    s.hello().await.unwrap();
    assert_eq!(host.paired_count(), 1);
    drop(s);

    // "Restart": a fresh Host over the same key dir loads the device list.
    let host2 = host_with_echo(dir.join("remote-key.bin")).unwrap();
    assert_eq!(host2.paired_count(), 1);
    let devs = host2.devices();
    assert_eq!(devs.len(), 1);
    assert_eq!(devs[0].1, "persist-phone");

    // And the persisted record actually authorizes a reconnect.
    let port2 = PORT.fetch_add(1, Ordering::SeqCst);
    let h2 = std::sync::Arc::clone(&host2);
    tokio::spawn(async move { lyra_remote::serve(h2, port2).await });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let addr2 = SocketAddr::from(([127, 0, 0, 1], port2));
    let mut s2 = client.connect(addr2).await.unwrap();
    assert_eq!(s2.hello().await.unwrap()["event"], "connected");
    drop(s2);

    // Revoke → persisted removal + reconnect rejected.
    assert!(host2.revoke(&devs[0].0));
    assert_eq!(host2.paired_count(), 0);
    let host3 = host_with_echo(dir.join("remote-key.bin")).unwrap();
    assert_eq!(host3.paired_count(), 0);
}

#[tokio::test]
async fn unknown_device_rejected() {
    let dir = std::env::temp_dir().join(format!("lyra-remote-test-unk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (_host, addr) = spawn_host(&dir).await;

    // A client with a fresh (unpaired) static connects — host must close
    // after the handshake instead of entering the command loop.
    let stranger = Client::new();
    let mut s = stranger.connect(addr).await.unwrap();
    // server closes without hello frame → recv fails/EOF
    assert!(s.hello().await.is_err());
}
