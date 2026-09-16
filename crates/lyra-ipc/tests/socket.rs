mod common;
use common::temp_socket;
use lyra_ipc::{mock::MockDispatcher, Server};

// Crash-simulation: dropping the handle without shutdown() leaves the socket
// behind; the next bind must reap it and succeed.
#[test]
fn stale_socket_reap() {
    let path = temp_socket("stale");
    let handle = Server::new(MockDispatcher::new())
        .serve_on_path(&path)
        .unwrap();
    assert!(path.exists());
    drop(handle); // no shutdown(): stale socket left behind
    assert!(path.exists(), "drop must detach, leaving the socket");

    // Rebind reaps the stale file and serves again.
    let handle2 = Server::new(MockDispatcher::new()).serve_on_path(&path);
    assert!(handle2.is_ok(), "stale socket must be reaped");
    let handle2 = handle2.unwrap();
    let mut c = lyra_ipc::Client::connect(&path).unwrap();
    c.call("state.get", serde_json::json!({})).unwrap();

    // A live server answers → second bind is refused, not clobbered.
    let err = Server::new(MockDispatcher::new())
        .serve_on_path(&path)
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
    // Original server still answers.
    let mut c = lyra_ipc::Client::connect(&path).unwrap();
    c.call("state.get", serde_json::json!({})).unwrap();

    handle2.shutdown();
    assert!(!path.exists());
}

// Parse garbage and wrong versions become error responses, not dead conns.
#[test]
fn parse_errors_never_kill_the_conn() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let path = temp_socket("parse");
    let handle = Server::new(MockDispatcher::new())
        .serve_on_path(&path)
        .unwrap();
    let s = UnixStream::connect(&path).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let mut reader = BufReader::new(s.try_clone().unwrap());
    let mut w = s;
    let mut line = String::new();
    reader.read_line(&mut line).unwrap(); // hello
    assert!(line.contains("hello"));

    // Garbage line → parse_error, connection survives.
    line.clear();
    writeln!(w, "this is not json").unwrap();
    reader.read_line(&mut line).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["ok"], false);
    assert_eq!(resp["error"]["code"], "parse_error");

    // Wrong version → invalid_param, connection survives.
    line.clear();
    writeln!(
        w,
        "{}",
        serde_json::json!({"version": 1, "id": "x", "method": "state.get"})
    )
    .unwrap();
    reader.read_line(&mut line).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["ok"], false);

    // Unknown method → unknown_method, then a good call still works.
    line.clear();
    writeln!(
        w,
        "{}",
        serde_json::json!({"version": 2, "id": "y", "method": "frobnicate"})
    )
    .unwrap();
    reader.read_line(&mut line).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["error"]["code"], "unknown_method");

    line.clear();
    writeln!(
        w,
        "{}",
        serde_json::json!({"version": 2, "id": "z", "method": "state.get"})
    )
    .unwrap();
    reader.read_line(&mut line).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["ok"], true);
    assert!(resp["snapshot"]["revision"].is_number());

    handle.shutdown();
}
