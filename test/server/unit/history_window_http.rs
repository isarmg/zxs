use super::*;
use crate::auth::AuthStore;
use http::{Request, StatusCode, header};
use http_routing::Router;
use service_stack::ServiceExt;
use std::{
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

const FILE: &str = "file_00000000000000000000000000000001";
const NODE: &str = "ver_00000000000000000000000000000002";
const MERGE: &str = "fmerge_00000000000000000000000000000003";

fn owned(bytes: &[u8], slots: &Arc<tokio::sync::Semaphore>) -> Bytes {
    Bytes::from_owner(OwnedHistoryBytes {
        bytes: bytes.into(),
        _permit: slots.clone().try_acquire_owned().unwrap(),
    })
}

#[test]
fn history_response_body_holds_its_backend_slot_until_dropped() {
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let response = response(owned(b"{}", &slots));
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(slots.available_permits(), 0);
    assert!(slots.clone().try_acquire_owned().is_err());
    drop(response);
    assert_eq!(slots.available_permits(), 1);
}

#[test]
fn history_buffer_clones_and_slices_retain_one_permit_until_the_last_owner_drops() {
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let original = owned(b"abcdef", &slots);
    let clone = original.clone();
    let slice = original.slice(1..4);
    assert_eq!(slice.as_ref(), b"bcd");
    drop(original);
    drop(clone);
    assert_eq!(slots.available_permits(), 0);
    drop(slice);
    assert_eq!(slots.available_permits(), 1);
}

#[tokio::test]
async fn encoded_history_json_is_sent_verbatim_and_releases_its_buffer_owner() {
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let encoded =
        br#"{"kind":"node_field","text":"line\n\"quoted\"","total_bytes":"18446744073709551615"}"#;
    let response = response(owned(encoded, &slots));
    let bytes = http_routing::body::to_bytes(response.into_body(), encoded.len())
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), encoded);
    let value: json_codec::Value = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(value["text"], "line\n\"quoted\"");
    drop(bytes);
    assert_eq!(slots.available_permits(), 1);
}

#[test]
fn queued_history_cancellation_is_visible_before_worker_starts_and_retains_owners() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let root = crate::tests::private_tempdir();
        let backend = crate::tests::history_window_test_backend(root.path().join("content"));
        let backend_owner = Arc::downgrade(&backend);
        let auth = Arc::new(Mutex::new(
            AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap(),
        ));
        let auth_owner = Arc::downgrade(&auth);
        let occupied = Arc::new(tokio::sync::Semaphore::new(0));
        let started = Arc::clone(&occupied);
        let (release, resume) = mpsc::sync_channel(1);
        let blocker = tokio::task::spawn_blocking(move || {
            started.add_permits(1);
            resume.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        tokio::time::timeout(Duration::from_secs(10), occupied.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        let slots = Arc::new(tokio::sync::Semaphore::new(1));
        let entered = Arc::new(tokio::sync::Semaphore::new(0));
        let entered_worker = Arc::clone(&entered);
        let cancellation = HistoryWindowCancellation::new();
        let observed_cancellation = cancellation.clone();
        let cancel_on_drop = CancelHistoryWindowOnDrop(cancellation.clone());
        let waiting = crate::http_job::HttpBackendJob::new(
            backend,
            auth,
            slots.clone().try_acquire_owned().unwrap(),
        )
        .run_with_permit(move |_, permit| {
            assert!(
                cancellation.is_cancelled(),
                "queued cancellation must predate capture"
            );
            entered_worker.add_permits(1);
            Ok(Bytes::from_owner(OwnedHistoryBytes {
                bytes: b"{}".to_vec(),
                _permit: permit,
            }))
        });
        let waiter = tokio::spawn(async move {
            let _cancel_on_drop = cancel_on_drop;
            waiting.await
        });
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(observed_cancellation.is_cancelled());
        assert!(backend_owner.upgrade().is_some());
        assert!(auth_owner.upgrade().is_some());
        assert_eq!(slots.available_permits(), 0);
        assert_eq!(entered.available_permits(), 0);
        release.send(()).unwrap();
        blocker.await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while backend_owner.upgrade().is_some()
                || auth_owner.upgrade().is_some()
                || slots.available_permits() == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(entered.available_permits(), 1);
        assert_eq!(slots.available_permits(), 1);
    });
}

#[tokio::test]
async fn running_history_cancellation_signals_worker_without_releasing_owners_early() {
    let root = crate::tests::private_tempdir();
    let backend = crate::tests::history_window_test_backend(root.path().join("content"));
    let backend_owner = Arc::downgrade(&backend);
    let auth = Arc::new(Mutex::new(
        AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap(),
    ));
    let auth_owner = Arc::downgrade(&auth);
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let entered_worker = Arc::clone(&started);
    let (release, resume) = mpsc::sync_channel(1);
    let cancellation = HistoryWindowCancellation::new();
    let observed_cancellation = cancellation.clone();
    let cancel_on_drop = CancelHistoryWindowOnDrop(cancellation.clone());
    let waiting = crate::http_job::HttpBackendJob::new(
        backend,
        auth,
        slots.clone().try_acquire_owned().unwrap(),
    )
    .run_with_permit(move |_, permit| {
        assert!(!cancellation.is_cancelled());
        entered_worker.add_permits(1);
        resume.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(
            cancellation.is_cancelled(),
            "running worker must see the same signal"
        );
        Ok(Bytes::from_owner(OwnedHistoryBytes {
            bytes: b"{}".to_vec(),
            _permit: permit,
        }))
    });
    let waiter = tokio::spawn(async move {
        let _cancel_on_drop = cancel_on_drop;
        waiting.await
    });
    tokio::time::timeout(Duration::from_secs(10), started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert!(observed_cancellation.is_cancelled());
    assert!(backend_owner.upgrade().is_some());
    assert!(auth_owner.upgrade().is_some());
    assert_eq!(slots.available_permits(), 0);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while backend_owner.upgrade().is_some()
            || auth_owner.upgrade().is_some()
            || slots.available_permits() == 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(slots.available_permits(), 1);
}

#[tokio::test]
async fn completed_history_wait_cancels_only_signal_while_response_keeps_permit() {
    let root = crate::tests::private_tempdir();
    let backend = crate::tests::history_window_test_backend(root.path().join("content"));
    let backend_owner = Arc::downgrade(&backend);
    let auth = Arc::new(Mutex::new(
        AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap(),
    ));
    let auth_owner = Arc::downgrade(&auth);
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let cancellation = HistoryWindowCancellation::new();
    let observed_cancellation = cancellation.clone();
    let cancel_on_drop = CancelHistoryWindowOnDrop(cancellation.clone());
    let waiting = crate::http_job::HttpBackendJob::new(
        backend,
        auth,
        slots.clone().try_acquire_owned().unwrap(),
    )
    .run_with_permit(move |_, permit| {
        assert!(!cancellation.is_cancelled());
        Ok(Bytes::from_owner(OwnedHistoryBytes {
            bytes: b"{}".to_vec(),
            _permit: permit,
        }))
    });
    let bytes = async move {
        let _cancel_on_drop = cancel_on_drop;
        waiting.await
    }
    .await
    .unwrap();
    assert!(observed_cancellation.is_cancelled());
    assert!(backend_owner.upgrade().is_none());
    assert!(auth_owner.upgrade().is_none());
    assert_eq!(slots.available_permits(), 0);
    let response = response(bytes);
    assert_eq!(slots.available_permits(), 0);
    drop(response);
    assert_eq!(slots.available_permits(), 1);
}

fn endpoints() -> Vec<(String, bool)> {
    vec![
        (format!("/api/file-manager/files/{FILE}/history"), false),
        (format!("/api/files/{FILE}/history"), false),
        (
            format!("/api/file-manager/files/{FILE}/history/branches"),
            true,
        ),
        (
            format!("/api/file-manager/files/{FILE}/history/merges"),
            true,
        ),
        (
            format!("/api/file-manager/files/{FILE}/history/nodes/{NODE}/parents"),
            true,
        ),
        (
            format!("/api/files/{FILE}/history/versions/{NODE}/sources"),
            true,
        ),
        (
            format!("/api/file-manager/files/{FILE}/history/merges/{MERGE}/inputs"),
            true,
        ),
        (
            format!("/api/file-manager/files/{FILE}/history/merges/{MERGE}/parents"),
            true,
        ),
        (
            format!("/api/file-manager/files/{FILE}/history/nodes/{NODE}/fields/message"),
            true,
        ),
    ]
}

async fn login(app: &Router) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"username":"admin","password":"a-long-test-password"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = http_routing::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    let session: json_codec::Value = json_codec::from_slice(&bytes).unwrap();
    session["token"].as_str().unwrap().into()
}

async fn get(app: &Router, token: Option<&str>, uri: &str) -> (StatusCode, json_codec::Value) {
    let mut request = Request::builder().uri(uri);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    (status, json_codec::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn all_history_window_routes_require_auth_and_dispatch_valid_typed_requests() {
    let root = crate::tests::private_tempdir();
    let app = crate::tests::application(root.path());
    let token = login(&app).await;
    for (path, requires_root) in endpoints() {
        let (status, error) = get(&app, None, &path).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}: {error}");
        let uri = if requires_root {
            format!("{path}?history_root={}", "a".repeat(64))
        } else {
            path.clone()
        };
        let (status, error) = get(&app, Some(&token), &uri).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{uri}: {error}");
        assert_eq!(error["error"]["code"], "UNSUPPORTED_OPERATION");
        if requires_root {
            let (status, error) = get(&app, Some(&token), &path).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {error}");
        }
    }
}

#[tokio::test]
async fn both_existing_history_routes_reject_all_unknown_and_duplicate_parameters() {
    let root = crate::tests::private_tempdir();
    let app = crate::tests::application(root.path());
    let token = login(&app).await;
    for path in [
        format!("/api/file-manager/files/{FILE}/history"),
        format!("/api/files/{FILE}/history"),
    ] {
        for query in [
            "all=true",
            "unexpected=1",
            "limit=1&limit=2",
            "limit=1&%6cimit=2",
        ] {
            let (status, error) = get(&app, Some(&token), &format!("{path}?{query}")).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{path}?{query}: {error}");
            assert_eq!(error["error"]["code"], "INVALID_REQUEST");
        }
    }
}

#[tokio::test]
async fn auxiliary_routes_keep_node_version_merge_and_field_selectors_distinct() {
    let root = crate::tests::private_tempdir();
    let app = crate::tests::application(root.path());
    let token = login(&app).await;
    for path in [
        format!("/api/file-manager/files/{FILE}/history/nodes/{MERGE}/parents"),
        format!(
            "/api/files/{FILE}/history/versions/fmeta_00000000000000000000000000000004/sources"
        ),
        format!("/api/file-manager/files/{FILE}/history/merges/{NODE}/inputs"),
        format!("/api/file-manager/files/{FILE}/history/merges/{NODE}/parents"),
        format!("/api/file-manager/files/{FILE}/history/nodes/{NODE}/fields/parents"),
    ] {
        let uri = format!("{path}?history_root={}", "a".repeat(64));
        let (status, error) = get(&app, Some(&token), &uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {error}");
        assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    }
}
