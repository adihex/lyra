mod common;
use common::TestServer;
use lyra_ipc::mock::MockDispatcher;
use std::time::{Duration, Instant};

// Async-by-construction: submit → queued → running → succeeded; job.get polls.
#[test]
fn job_lifecycle() {
    let srv = TestServer::start();
    let mut c = srv.client();

    let r = c
        .call(
            "operation.submit",
            serde_json::json!({"operation": "library.scan", "params": {}}),
        )
        .unwrap();
    let job = r.job.unwrap();
    let id = job["id"].as_str().unwrap().to_string();
    assert!(
        job["state"] == "queued" || job["state"] == "running",
        "new job must start non-terminal, got {}",
        job["state"]
    );

    // Poll until terminal (bounded wait).
    let deadline = Instant::now() + Duration::from_secs(10);
    let terminal = loop {
        let r = c.call("job.get", serde_json::json!({"id": id})).unwrap();
        let state = r.job.as_ref().unwrap()["state"]
            .as_str()
            .unwrap()
            .to_string();
        if ["succeeded", "failed", "cancelled"].contains(&state.as_str()) {
            break state;
        }
        assert!(Instant::now() < deadline, "job never finished");
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(terminal, "succeeded");
    let r = c.call("job.get", serde_json::json!({"id": id})).unwrap();
    assert!(r.job.as_ref().unwrap()["result"]["scanned"] == 3);

    // Fast ops return succeeded inline.
    let r = c
        .call(
            "operation.submit",
            serde_json::json!({"operation": "play", "params": {}}),
        )
        .unwrap();
    assert_eq!(r.job.as_ref().unwrap()["state"], "succeeded");

    // Unknown job → not_found.
    assert!(c
        .call("job.get", serde_json::json!({"id": "job-999"}))
        .is_err());

    srv.shutdown();
}

// Cancel wins over a late worker finish (long job → deterministic cancel).
#[test]
fn job_cancel() {
    let mut d = MockDispatcher::new();
    d.job_work = Duration::from_secs(30);
    let srv = TestServer::start_with(d);
    let mut c = srv.client();

    let r = c
        .call(
            "operation.submit",
            serde_json::json!({"operation": "library.scan", "params": {}}),
        )
        .unwrap();
    let id = r.job.as_ref().unwrap()["id"].as_str().unwrap().to_string();

    let r = c.call("job.cancel", serde_json::json!({"id": id})).unwrap();
    assert_eq!(r.job.as_ref().unwrap()["state"], "cancelled");

    // Double-cancel of a terminal job → not_found.
    assert!(c.call("job.cancel", serde_json::json!({"id": id})).is_err());

    srv.shutdown();
}
