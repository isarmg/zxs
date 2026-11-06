//! Real router admission after both HTTP response futures have been cancelled.
use super::*;

const CAPACITY: &str = "/api/workspaces/capacity";
const WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
enum EndWait {
    Drop,
    Timeout,
}

async fn start_then_end_http_wait(app: Router, gate: &Gate, end: EndWait) {
    let mut response = Box::pin(request(app, CAPACITY));
    tokio::time::timeout(WAIT, async {
        tokio::select! {
            started = gate.started.acquire() => started.unwrap().forget(),
            response = &mut response => panic!(
                "blocked backend returned HTTP {} before cancellation", response.status()
            ),
        }
    })
    .await
    .expect("backend did not enter its gate");

    match end {
        EndWait::Drop => drop(response),
        EndWait::Timeout => {
            // Timeout owns the actual router response future. Timing out a
            // JoinHandle would merely detach the still-running HTTP waiter.
            assert!(
                tokio::time::timeout(Duration::ZERO, response)
                    .await
                    .is_err()
            );
        }
    }
}

async fn retained_overload(app: Router, path: &str, message: &str) -> String {
    let response = tokio::time::timeout(WAIT, request(app, path))
        .await
        .expect("overload must reject without entering the blocked backend");
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS, "{path}");
    let bytes = tokio::time::timeout(
        WAIT,
        http_routing::body::to_bytes(response.into_body(), 8192),
    )
    .await
    .expect("overload response body did not finish")
    .unwrap();
    // Decode the production HTTP error through the real client protocol too.
    let error = storage_contract::client_protocol::decode_response(429, &bytes).unwrap_err();
    assert_eq!(
        error.code,
        storage_contract::ErrorCode::ResourceLimitExceeded,
        "{path}"
    );
    assert_eq!(error.status, 429);
    assert_eq!(error.message, message);
    assert!(error.retryable);
    let request_id = error.request_id.expect("real overload lost its request ID");
    assert!(!request_id.is_empty());
    request_id
}

async fn assert_both_slots_remain_owned(end: EndWait) {
    let root = crate::tests::private_tempdir();
    let (app, gate, _release) = fixture_for(root.path(), 0, false, BlockedEndpoint::Capacity);
    for expected_calls in 1..=2 {
        start_then_end_http_wait(app.clone(), &gate, end).await;
        assert_eq!(gate.calls.load(Ordering::SeqCst), expected_calls);
        assert_eq!(gate.finished.available_permits(), 0);
    }

    let mut request_ids = std::collections::BTreeSet::new();
    for path in [CAPACITY, "/api/capabilities", "/readyz"] {
        let id = retained_overload(app.clone(), path, "management capacity reached").await;
        assert!(
            request_ids.insert(id),
            "different real rejections reused an ID"
        );
        assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
        assert_eq!(gate.finished.available_permits(), 0);
    }
    #[cfg(feature = "core")]
    for path in [
        "/api/admin/storage/discovered-disks",
        "/api/admin/disks/registrations",
    ] {
        let id =
            retained_overload(app.clone(), path, "management operation capacity reached").await;
        assert!(request_ids.insert(id), "setup rejection reused an ID");
        assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
        assert_eq!(gate.finished.available_permits(), 0);
    }
    let health = tokio::time::timeout(WAIT, request(app.clone(), "/healthz"))
        .await
        .expect("overload blocked independent health");
    assert_eq!(health.status(), StatusCode::OK);

    gate.release();
    tokio::time::timeout(WAIT, gate.finished.acquire_many(2))
        .await
        .expect("cancelled background workers failed to finish")
        .unwrap()
        .forget();

    // Gate completion precedes permit Drop. Observe actual admission with a
    // bounded wait instead of assuming a scheduler yield proves reclamation.
    let capacity = tokio::time::timeout(WAIT, async {
        loop {
            let response = request(app.clone(), CAPACITY).await;
            match response.status() {
                StatusCode::TOO_MANY_REQUESTS => tokio::task::yield_now().await,
                StatusCode::OK => {
                    let bytes = http_routing::body::to_bytes(response.into_body(), 8192)
                        .await
                        .unwrap();
                    break json_codec::from_slice::<storage_contract::WorkspaceCapacity>(&bytes)
                        .unwrap();
                }
                status => panic!("released backend returned unexpected HTTP {status}"),
            }
        }
    })
    .await
    .expect("management capacity did not recover after worker completion");
    assert_eq!(capacity, probe_capacity());
    assert_eq!(
        gate.calls.load(Ordering::SeqCst),
        3,
        "two cancelled reads plus one successful new read; rejects never enter the backend"
    );
}

#[tokio::test]
async fn two_dropped_http_waits_keep_management_slots_until_backend_completion() {
    assert_both_slots_remain_owned(EndWait::Drop).await;
}

#[tokio::test]
async fn two_timed_out_http_waits_keep_management_slots_until_backend_completion() {
    assert_both_slots_remain_owned(EndWait::Timeout).await;
}

#[cfg(feature = "core")]
#[path = "retained_management_slots_tcp.rs"]
mod tcp;
