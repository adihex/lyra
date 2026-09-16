mod common;
use common::{revision_of, submit_ok, TestServer};

// Optimistic concurrency: stale if_revision / if_playlist_revision → conflict.
#[test]
fn revision_conflict() {
    let srv = TestServer::start();
    let mut c = srv.client();

    let snap = c.state().unwrap();
    let (rev, plrev) = revision_of(&snap);

    // Fresh guard succeeds and bumps the revision.
    let r = c
        .call(
            "operation.submit",
            serde_json::json!({"operation": "play", "params": {}, "if_revision": rev}),
        )
        .unwrap();
    assert!(r.ok);
    let (rev2, _) = revision_of(&r.snapshot.unwrap());
    assert_eq!(rev2, rev + 1);

    // Stale guard → conflict (exit 3), state untouched.
    let e = c
        .call(
            "operation.submit",
            serde_json::json!({"operation": "pause", "params": {}, "if_revision": rev}),
        )
        .unwrap_err();
    assert_eq!(e.exit_code(), 3);
    assert!(
        e.to_string().contains("Conflict")
            || e.to_string().contains("conflict")
            || format!("{e:?}").contains("Conflict")
    );
    let snap = c.state().unwrap();
    assert_eq!(snap["state"], "playing");

    // Playlist guard: enqueue bumps playlist_revision; stale retry conflicts.
    let r = submit_ok(
        &mut c,
        "queue.enqueue",
        serde_json::json!({"query": "aphex"}),
    );
    let (_, plrev2) = revision_of(&r.snapshot.unwrap());
    assert!(plrev2 > plrev);
    let e = c
        .call(
            "operation.submit",
            serde_json::json!({
                "operation": "queue.clear",
                "params": {},
                "if_playlist_revision": plrev,
            }),
        )
        .unwrap_err();
    assert_eq!(e.exit_code(), 3);

    srv.shutdown();
}

// Two connections can't clobber: second writer with a stale read loses.
#[test]
fn concurrent_clients_revision_guard() {
    let srv = TestServer::start();
    let mut a = srv.client();
    let mut b = srv.client();

    let ra = revision_of(&a.state().unwrap()).0;
    let rb = revision_of(&b.state().unwrap()).0;
    assert_eq!(ra, rb);

    // A commits first.
    let r = a
        .call(
            "operation.submit",
            serde_json::json!({"operation": "volume.set",
                "params": {"volume": 0.3}, "if_revision": ra}),
        )
        .unwrap();
    assert!(r.ok);

    // B's write is based on the same stale read → conflict.
    let e = b
        .call(
            "operation.submit",
            serde_json::json!({"operation": "volume.set",
                "params": {"volume": 0.9}, "if_revision": rb}),
        )
        .unwrap_err();
    assert_eq!(e.exit_code(), 3);

    // B re-reads and retries → wins.
    let fresh = revision_of(&b.state().unwrap()).0;
    let r = b
        .call(
            "operation.submit",
            serde_json::json!({"operation": "volume.set",
                "params": {"volume": 0.9}, "if_revision": fresh}),
        )
        .unwrap();
    assert!(r.ok);
    assert_eq!(a.state().unwrap()["volume"], 0.9);

    srv.shutdown();
}
