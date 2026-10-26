use super::*;
use crate::spool_budget::METADATA_ALLOWANCE;
use std::sync::atomic::{AtomicBool, Ordering};

fn config(root: &Path, bytes: u64) -> ServerConfig {
    let mut config = ServerConfig::loopback(root);
    config.http_spool_limits = SpoolLimits {
        bytes_per_account: bytes,
        bytes_total: bytes,
        recovery_reserve_bytes: 1,
    };
    config
}
fn request(
    token: &str,
    item: &str,
    generation: u64,
    length: u64,
    hash: &str,
    body: Body,
) -> Request {
    Request::builder()
        .method("PUT")
        .uri(format!("/api/workspaces/ws/files/{item}/content?path=file"))
        .header("authorization", format!("Bearer {token}"))
        .header("x-zxs-logical-size", length.to_string())
        .header("x-zxs-content-hash", hash)
        .header("x-zxs-upload-generation", generation.to_string())
        .body(body)
        .unwrap()
}
fn restart(root: &Path, bytes: u64) -> Router {
    router(
        Arc::new(TestBackend {
            path: root.join("content"),
        }),
        AuthStore::open(root).unwrap(),
        config(root, bytes),
    )
    .unwrap()
}
fn data_count(root: &Path) -> usize {
    std::fs::read_dir(root.join("api-spool"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "data")
        })
        .count()
}

#[tokio::test]
async fn completed_and_old_generations_use_quota_before_body_poll_and_survive_restart() {
    let root = crate::tests::private_tempdir();
    let limit = 2 * METADATA_ALLOWANCE + 8;
    let app = application_config(root.path(), config(root.path(), limit));
    let (token, _, _) = authenticated(&app).await;
    let hash = blake3::hash(b"data").to_hex().to_string();
    for generation in [1, 2, 2] {
        let response = app
            .clone()
            .oneshot(request(
                &token,
                "entry_00000000000000000000000000000001",
                generation,
                4,
                &hash,
                Body::from("data"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert_eq!(
        data_count(root.path()),
        2,
        "a retry reuses its physical file"
    );
    drop(app);
    let app = restart(root.path(), limit);
    // Login/session and control routes stay available even when the spool is full.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let polled = Arc::new(AtomicBool::new(false));
    let observed = polled.clone();
    let body = Body::from_stream(futures::stream::poll_fn(move |_| {
        observed.store(true, Ordering::SeqCst);
        std::task::Poll::Ready(Some(Ok::<_, std::io::Error>(Bytes::from_static(b"data"))))
    }));
    let response = app
        .clone()
        .oneshot(request(
            &token,
            "entry_00000000000000000000000000000001",
            3,
            4,
            &hash,
            body,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        !polled.load(Ordering::SeqCst),
        "quota admission must precede body consumption"
    );
    assert_eq!(data_count(root.path()), 2);
    let response = app
        .oneshot(request(
            &token,
            "entry_00000000000000000000000000000001",
            2,
            4,
            &hash,
            Body::from("data"),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "same-generation retry needs no second full-file budget"
    );
}

#[tokio::test]
async fn interrupted_and_checksum_failed_bytes_remain_charged_after_restart() {
    let root = crate::tests::private_tempdir();
    let limit = 2 * METADATA_ALLOWANCE + 8;
    let app = application_config(root.path(), config(root.path(), limit));
    let (token, _, _) = authenticated(&app).await;
    let body = Body::from_stream(futures::stream::iter([
        Ok(Bytes::from_static(b"abc")),
        Err(std::io::Error::other("disconnect")),
    ]));
    let response = app
        .clone()
        .oneshot(request(
            &token,
            "entry_00000000000000000000000000000001",
            1,
            4,
            &"0".repeat(64),
            body,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = app
        .clone()
        .oneshot(request(
            &token,
            "entry_00000000000000000000000000000001",
            2,
            5,
            &"0".repeat(64),
            Body::from("wrong"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let record = uploads::load(
        &root.path().join("api-spool"),
        "ws",
        "entry_00000000000000000000000000000001",
    )
    .unwrap()
    .unwrap();
    assert_eq!(record.state, crate::uploads::UploadState::ChecksumMismatch);
    drop(app);
    let app = restart(root.path(), limit);
    let response = app
        .oneshot(request(
            &token,
            "entry_0000000000000000000000000000000a",
            1,
            0,
            blake3::hash(b"").to_hex().as_str(),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(data_count(root.path()), 2);
    assert_eq!(
        std::fs::read(root.path().join("api-spool").join(record.data_file)).unwrap(),
        b"wrong"
    );
}

#[tokio::test]
async fn cancellation_settles_actual_unknown_tail_and_retry_never_overwrites_it() {
    let root = crate::tests::private_tempdir();
    let limit = METADATA_ALLOWANCE + 10;
    let app = application_config(root.path(), config(root.path(), limit));
    let (token, _, _) = authenticated(&app).await;
    let expected = b"abcdefghij";
    let hash = blake3::hash(expected).to_hex().to_string();
    let stream = futures::StreamExt::chain(
        futures::stream::once(async { Ok::<_, std::io::Error>(Bytes::from_static(b"abc")) }),
        futures::stream::pending(),
    );
    let upload_app = app.clone();
    let upload_request = request(
        &token,
        "entry_00000000000000000000000000000001",
        1,
        10,
        &hash,
        Body::from_stream(stream),
    );
    let upload = tokio::spawn(async move { upload_app.oneshot(upload_request).await });
    let record = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(record) = uploads::load(
                &root.path().join("api-spool"),
                "ws",
                "entry_00000000000000000000000000000001",
            )
            .unwrap()
                && std::fs::metadata(root.path().join("api-spool").join(&record.data_file))
                    .unwrap()
                    .len()
                    == 3
            {
                break record;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        record.persisted_bytes, 0,
        "small tail has not been declared durable"
    );
    let response = app
        .clone()
        .oneshot(request(
            &token,
            "entry_00000000000000000000000000000002",
            1,
            0,
            blake3::hash(b"").to_hex().as_str(),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    upload.abort();
    let _ = upload.await;
    // A cancelled request can still have one bounded blocking job finishing.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let response = app
                .clone()
                .oneshot(request(
                    &token,
                    "entry_00000000000000000000000000000001",
                    1,
                    10,
                    &hash,
                    Body::from("XYZdefghij"),
                ))
                .await
                .unwrap();
            if response.status() == StatusCode::CONFLICT {
                let bytes = http_routing::body::to_bytes(response.into_body(), 8192)
                    .await
                    .unwrap();
                let error: Value = json_codec::from_slice(&bytes).unwrap();
                if error["error"]["code"] == "UPLOAD_RANGE_CONFLICT" {
                    break;
                }
            } else {
                panic!("unexpected retry status: {}", response.status());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(root.path().join("api-spool").join(&record.data_file)).unwrap(),
        b"abc"
    );
    drop(app);
    let app = restart(root.path(), limit);
    let response = app
        .oneshot(request(
            &token,
            "entry_00000000000000000000000000000001",
            1,
            10,
            &hash,
            Body::from(expected.as_slice()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(data_count(root.path()), 1);
    assert_eq!(
        std::fs::read(root.path().join("api-spool").join(record.data_file)).unwrap(),
        expected
    );
}

#[tokio::test]
async fn upload_idle_timeout_preserves_tail_and_allows_verified_retry() {
    let root = crate::tests::private_tempdir();
    let mut settings = config(root.path(), METADATA_ALLOWANCE + 10);
    settings.http_transfer_limits.idle_timeout = Duration::from_millis(50);
    let app = application_config(root.path(), settings);
    let (token, _, _) = authenticated(&app).await;
    let hash = blake3::hash(b"abcdefghij").to_hex().to_string();
    let stream = futures::StreamExt::chain(
        futures::stream::once(async { Ok::<_, std::io::Error>(Bytes::from_static(b"abc")) }),
        futures::stream::pending(),
    );
    let response = app
        .clone()
        .oneshot(request(
            &token,
            "entry_00000000000000000000000000000007",
            1,
            10,
            &hash,
            Body::from_stream(stream),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    let body: Value = json_codec::from_slice(
        &http_routing::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["error"]["code"], "TRANSFER_TIMEOUT");
    let record = uploads::load(
        &root.path().join("api-spool"),
        "ws",
        "entry_00000000000000000000000000000007",
    )
    .unwrap()
    .unwrap();
    assert_eq!(record.persisted_bytes, 3);
    assert_eq!(
        std::fs::read(root.path().join("api-spool").join(record.data_file)).unwrap(),
        b"abc"
    );
    let response = app
        .oneshot(request(
            &token,
            "entry_00000000000000000000000000000007",
            1,
            10,
            &hash,
            Body::from("abcdefghij"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(data_count(root.path()), 1);
}

#[test]
fn upload_total_timeout_retains_unfinished_evidence_despite_progress() {
    let root = crate::tests::private_tempdir();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut settings = config(root.path(), METADATA_ALLOWANCE + 100);
        settings.http_transfer_limits.idle_timeout = Duration::from_millis(100);
        settings.http_transfer_limits.total_timeout = Duration::from_millis(180);
        let app = application_config(root.path(), settings);
        let (token, _, _) = authenticated(&app).await;
        let hash = blake3::hash(&[b'x'; 100]).to_hex().to_string();
        let stream = futures::stream::unfold((), |_| async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Some((Ok::<_, std::io::Error>(Bytes::from_static(b"x")), ()))
        });
        let response = app
            .oneshot(request(
                &token,
                "entry_00000000000000000000000000000008",
                1,
                100,
                &hash,
                Body::from_stream(stream),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    });
    // The total deadline ends HTTP waiting, not an admitted blocking progress
    // publication. Join those jobs before inspecting retained evidence; otherwise
    // load's identity check can race the legitimate atomic metadata replacement.
    drop(runtime);
    let record = uploads::load(
        &root.path().join("api-spool"),
        "ws",
        "entry_00000000000000000000000000000008",
    )
    .unwrap()
    .unwrap();
    assert_eq!(record.state, crate::uploads::UploadState::Incomplete);
    let bytes = std::fs::read(root.path().join("api-spool").join(record.data_file)).unwrap();
    assert!(!bytes.is_empty());
    assert!(bytes.len() < 100);
    assert!(bytes.iter().all(|&byte| byte == b'x'));
}

#[tokio::test]
async fn upload_slots_are_shared_across_sessions_and_items_and_released_on_cancel() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    let (other_token, _, _) = authenticated(&app).await;
    let hash = blake3::hash(b"").to_hex().to_string();
    let mut tasks = Vec::new();
    for (item, session) in [
        ("entry_00000000000000000000000000000004", &token),
        ("entry_00000000000000000000000000000005", &other_token),
    ] {
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let mut ready = Some(ready_tx);
        let stream = futures::stream::poll_fn(move |_| {
            if let Some(ready) = ready.take() {
                let _ = ready.send(());
            }
            std::task::Poll::Pending::<Option<Result<Bytes, std::io::Error>>>
        });
        let req = request(session, item, 1, 0, &hash, Body::from_stream(stream));
        let app = app.clone();
        tasks.push(tokio::spawn(async move { app.oneshot(req).await }));
        tokio::time::timeout(Duration::from_secs(5), ready_rx)
            .await
            .unwrap()
            .unwrap();
    }
    let response = app
        .clone()
        .oneshot(request(
            &token,
            "entry_00000000000000000000000000000006",
            1,
            0,
            &hash,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    // Reads have a separate pool and are not starved by uploads.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/versions/test/content")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    drop(response);
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
    let response = app
        .oneshot(request(
            &token,
            "entry_00000000000000000000000000000006",
            1,
            0,
            &hash,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn read_slots_remain_held_by_unconsumed_bodies() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    let download = || {
        Request::builder()
            .uri("/api/versions/test/content")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    };
    let first = app.clone().oneshot(download()).await.unwrap();
    let second = app.clone().oneshot(download()).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(
        app.clone().oneshot(download()).await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(first);
    assert_eq!(
        app.oneshot(download()).await.unwrap().status(),
        StatusCode::OK
    );
    drop(second);
}

#[tokio::test]
async fn checksum_bodies_share_read_slots_and_expire_without_polling() {
    let root = crate::tests::private_tempdir();
    let mut config = ServerConfig::loopback(root.path());
    config.http_transfer_limits.idle_timeout = Duration::from_millis(100);
    let app = application_config(root.path(), config);
    let (token, _, _) = authenticated(&app).await;
    let download = |suffix| {
        Request::builder()
            .uri(format!("/api/versions/test/{suffix}"))
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    };
    let first = app.clone().oneshot(download("checksums")).await.unwrap();
    let second = app.clone().oneshot(download("content")).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(
        first.headers()[header::CONTENT_LENGTH],
        (128 * 1024).to_string()
    );
    assert_eq!(
        app.clone()
            .oneshot(download("checksums"))
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        http_routing::body::to_bytes(first.into_body(), 256 * 1024)
            .await
            .is_err()
    );
    assert!(
        http_routing::body::to_bytes(second.into_body(), 8192)
            .await
            .is_err()
    );
    let response = app.oneshot(download("checksums")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        http_routing::body::to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap()
            .as_ref(),
        &[7; 128 * 1024]
    );
}

#[tokio::test]
async fn invalid_upload_item_ids_are_rejected_before_body_or_spool_publication() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    let hash = blake3::hash(b"data").to_hex().to_string();
    for item in [
        "item",
        "entry_",
        "entry_0000000000000000000000000000000A",
        "wrong_00000000000000000000000000000001",
    ] {
        let stream = futures::stream::poll_fn(
            |_| -> std::task::Poll<Option<Result<Bytes, std::io::Error>>> {
                panic!("invalid item must be rejected before polling its body")
            },
        );
        let response = app
            .clone()
            .oneshot(request(
                &token,
                item,
                1,
                4,
                &hash,
                Body::from_stream(stream),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{item}");
        assert_eq!(data_count(root.path()), 0);
    }
    let response = app
        .oneshot(request(
            &token,
            "entry_00000000000000000000000000000001",
            1,
            4,
            &hash,
            Body::from("data"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
