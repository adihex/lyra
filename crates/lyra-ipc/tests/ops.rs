mod common;
use common::{revision_of, submit_ok, TestServer};

// Full method/op table from spec §1.2 is reachable through the dispatcher.
#[test]
fn method_table_round_trip() {
    let srv = TestServer::start();
    let mut c = srv.client();

    let caps = c.call("capabilities", serde_json::json!({})).unwrap();
    let ops = caps.operations.unwrap();
    let names: Vec<&str> = ops
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    for op in [
        "capabilities",
        "state.get",
        "spectrum.get",
        "operation.submit",
        "job.get",
        "job.cancel",
        "subscribe",
        "plugin.call",
        "play",
        "pause",
        "toggle",
        "stop",
        "next",
        "prev",
        "seek.absolute",
        "seek.relative",
        "volume",
        "volume.set",
        "speed",
        "shuffle",
        "repeat",
        "eq.get",
        "eq.set",
        "eq.band.set",
        "queue.list",
        "queue.play",
        "queue.enqueue",
        "queue.remove",
        "queue.move",
        "queue.clear",
        "library.search",
        "library.stats",
        "library.scan",
        "track.play",
        "track.queue",
        "url.load",
        "torrent.add",
        "lyrics.get",
        "device.list",
        "device.set",
    ] {
        assert!(names.contains(&op), "op table missing {op}");
    }

    // Transport + modes + eq + queue + library + sources + introspection.
    for (op, params) in [
        ("play", serde_json::json!({})),
        ("pause", serde_json::json!({})),
        ("toggle", serde_json::json!({})),
        ("seek.absolute", serde_json::json!({"position_s": 10.0})),
        ("seek.relative", serde_json::json!({"delta_s": 5.0})),
        ("volume.set", serde_json::json!({"volume": 0.5})),
        ("speed", serde_json::json!({"speed": 1.0})),
        ("shuffle", serde_json::json!({"enabled": true})),
        ("repeat", serde_json::json!({"mode": "all"})),
        ("eq.get", serde_json::json!({})),
        ("eq.set", serde_json::json!({"bands": vec![0.0f64; 10]})),
        (
            "eq.band.set",
            serde_json::json!({"band": 2, "gain_db": -1.5}),
        ),
        ("queue.list", serde_json::json!({})),
        ("queue.enqueue", serde_json::json!({"query": "aphex"})),
        ("queue.move", serde_json::json!({"from": 0, "to": 0})),
        ("queue.play", serde_json::json!({"index": 0})),
        ("library.search", serde_json::json!({"q": "coltrane"})),
        ("library.stats", serde_json::json!({})),
        ("track.play", serde_json::json!({"track_id": "t-boards"})),
        ("track.queue", serde_json::json!({"track_id": "t-boards"})),
        (
            "url.load",
            serde_json::json!({"url": "https://example.com/x.mp3"}),
        ),
        ("lyrics.get", serde_json::json!({})),
        ("spectrum.get", serde_json::json!({})),
        ("device.list", serde_json::json!({})),
        ("device.set", serde_json::json!({"device": "mock-dac"})),
        ("queue.remove", serde_json::json!({"index": 0})),
        ("queue.clear", serde_json::json!({})),
        ("stop", serde_json::json!({})),
    ] {
        let r = submit_ok(&mut c, op, params);
        assert!(r.job.is_some(), "{op} must return a job envelope");
    }

    // Read-your-writes: mutating ops echo the post-commit snapshot.
    let r = submit_ok(&mut c, "play", serde_json::json!({}));
    let snap = r.snapshot.expect("mutating op echoes snapshot");
    assert_eq!(snap["state"], "playing");

    // Unknown method / bad op → typed errors, never a dropped connection.
    let e = c.call("nope.method", serde_json::json!({})).unwrap_err();
    assert_eq!(e.exit_code(), 1);
    let e = c
        .call(
            "operation.submit",
            serde_json::json!({"operation": "nope.op"}),
        )
        .unwrap_err();
    assert!(e.to_string().contains("known operation"));
    // Server still alive after errors.
    let snap = c.state().unwrap();
    let (rev, _) = revision_of(&snap);
    assert!(rev > 1);

    srv.shutdown();
}
