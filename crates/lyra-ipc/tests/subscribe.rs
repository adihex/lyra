mod common;
use common::TestServer;
use lyra_ipc::client::SubItem;
use std::time::Duration;

// subscribe: retained replay on connect + ordered push events.
#[test]
fn subscribe_ordering_and_retained_replay() {
    let srv = TestServer::start();
    let mut c = srv.client();
    // Cause some state so retained topics exist before subscribing.
    c.call(
        "operation.submit",
        serde_json::json!({"operation": "play", "params": {}}),
    )
    .unwrap();

    let mut sub = c.subscribe(&["runtime.state", "runtime.job"]).unwrap();
    sub.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    // Retained replay: mid-song connect gets current state immediately.
    let mut saw_state = false;
    let mut last = 0u64;
    for _ in 0..4 {
        match sub.next_event().unwrap() {
            SubItem::Event(ev) => {
                assert!(ev.seq > last, "seq must increase");
                last = ev.seq;
                if ev.topic == "runtime.state" {
                    saw_state = true;
                    assert!(ev.data.get("revision").is_some());
                }
            }
            SubItem::Resync { .. } => panic!("no gap expected here"),
        }
        if saw_state {
            break;
        }
    }
    assert!(saw_state, "retained runtime.state must replay on subscribe");

    // Live push: a mutating op on another connection arrives in order.
    let mut c2 = srv.client();
    c2.call(
        "operation.submit",
        serde_json::json!({"operation": "pause", "params": {}}),
    )
    .unwrap();
    let mut got_live = false;
    for _ in 0..8 {
        match sub.next_event().unwrap() {
            SubItem::Event(ev) => {
                assert!(ev.seq > last);
                last = ev.seq;
                if ev.topic == "runtime.state" && ev.data["state"] == "paused" {
                    got_live = true;
                    break;
                }
            }
            SubItem::Resync { .. } => panic!("no gap expected here"),
        }
    }
    assert!(got_live, "live runtime.state push must arrive");

    // Async job emits runtime.job transitions with increasing seq.
    c2.call(
        "operation.submit",
        serde_json::json!({"operation": "library.scan", "params": {}}),
    )
    .unwrap();
    let mut saw_succeeded = false;
    for _ in 0..16 {
        match sub.next_event().unwrap() {
            SubItem::Event(ev) => {
                assert!(ev.seq > last);
                last = ev.seq;
                if ev.topic == "runtime.job" && ev.data["state"] == "succeeded" {
                    saw_succeeded = true;
                    break;
                }
            }
            SubItem::Resync { .. } => {}
        }
    }
    assert!(saw_succeeded, "runtime.job succeeded must arrive");

    srv.shutdown();
}

// Seq-gap resync against a scripted peer: events 1,2 then 5 (gap) — the
// client must issue state.get on the same conn and yield Resync first.
#[test]
fn seq_gap_triggers_state_resync() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};

    let path = common::temp_socket("gap");
    let listener = UnixListener::bind(&path).unwrap();
    let peer_path = path.clone();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(s.try_clone().unwrap());
        let mut line = String::new();
        // hello
        writeln!(
            s,
            "{}",
            serde_json::json!({
                "version": 2,
                "hello": {"app": "lyra", "protocol": 2, "version": "0.1.0", "capabilities": 40},
            })
        )
        .unwrap();
        // subscribe request → ack
        line.clear();
        reader.read_line(&mut line).unwrap();
        let req: serde_json::Value = serde_json::from_str(&line).unwrap();
        writeln!(
            s,
            "{}",
            serde_json::json!({
                "version": 2, "id": req["id"], "ok": true, "subscribed": ["runtime.state"],
            })
        )
        .unwrap();
        // events 1, 2, then 5 (gap at 3-4)
        for seq in [1u64, 2, 5] {
            writeln!(
                s,
                "{}",
                serde_json::json!({
                    "version": 2, "type": "event", "seq": seq,
                    "topic": "runtime.state", "data": {"revision": seq},
                })
            )
            .unwrap();
        }
        // expect the client's auto state.get on the same conn
        line.clear();
        reader.read_line(&mut line).unwrap();
        let req: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(req["method"], "state.get");
        writeln!(
            s,
            "{}",
            serde_json::json!({
                "version": 2, "id": req["id"], "ok": true,
                "snapshot": {"revision": 99, "playlist_revision": 9},
            })
        )
        .unwrap();
        // keep the conn open briefly so the client can finish reading
        std::thread::sleep(Duration::from_millis(500));
        drop(s);
        let _ = UnixStream::connect(&peer_path).err();
    });

    let client = lyra_ipc::Client::connect(&path).unwrap();
    let mut sub = client.subscribe(&["runtime.state"]).unwrap();
    sub.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    // events 1, 2 stream through
    for want in [1u64, 2] {
        match sub.next_event().unwrap() {
            SubItem::Event(ev) => assert_eq!(ev.seq, want),
            SubItem::Resync { .. } => panic!("no gap yet"),
        }
    }
    // event 5 jumps the seq → Resync with the fresh snapshot first…
    match sub.next_event().unwrap() {
        SubItem::Resync {
            snapshot,
            from_seq,
            to_seq,
        } => {
            assert_eq!(from_seq, 2);
            assert_eq!(to_seq, 5);
            assert_eq!(snapshot["revision"], 99);
        }
        SubItem::Event(ev) => panic!("expected resync, got event {}", ev.seq),
    }
    // …then the stashed event itself.
    match sub.next_event().unwrap() {
        SubItem::Event(ev) => assert_eq!(ev.seq, 5),
        SubItem::Resync { .. } => panic!("resync already handled"),
    }
    server.join().unwrap();
    std::fs::remove_file(&path).ok();
}
