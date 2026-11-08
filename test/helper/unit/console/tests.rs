use super::*;
use http_routing::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use json_codec::{Value, json};
use service_stack::ServiceExt;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use storage_engine::{
    OperationId,
    disk_authority::{
        Digest, DiskSelection, FileWrite, Identity, IndependentDisk, Origin, Role, Token,
    },
};

fn state() -> State {
    let (stopping, _receiver) = watch::channel(false);
    State::new(
        SocketAddr::from(([0, 0, 0, 0], 9040)),
        "test-csrf".into(),
        stopping,
    )
}
fn post(path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("host", "127.0.0.1:9040")
        .header("origin", "http://127.0.0.1:9040")
        .header("x-zxs-helper-csrf", "test-csrf")
        .header("content-type", "application/json")
        .body(Body::from(json_codec::to_vec(&body).unwrap()))
        .unwrap()
}
async fn json_body(response: Response) -> Value {
    json_codec::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap()
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            result.extend(files(&entry.path()));
        } else {
            result.insert(entry.path(), fs::read(entry.path()).unwrap());
        }
    }
    result
}
fn fixture(root: &Path) -> (DiskSelection, Digest) {
    let identity = Identity {
        origin: Origin {
            pool_id: Token::new("pool").unwrap(),
            disk_id: Token::new("disk").unwrap(),
            slot_id: Token::new("P01").unwrap(),
            binding_generation: 1,
        },
        incarnation: Digest::body(b"incarnation"),
        role: Role::Primary,
    };
    let path = root.join("disk");
    let disk = IndependentDisk::initialize(&path, identity.clone()).unwrap();
    let review = disk
        .review_file(FileWrite {
            operation: OperationId::generate().unwrap(),
            expected_base: None,
            history: &Token::new("history").unwrap(),
            file: &Token::new("file").unwrap(),
            bytes: b"verified body",
        })
        .unwrap();
    disk.execute(&review).unwrap();
    let report = disk.report().unwrap();
    (
        DiskSelection {
            path,
            identity,
            known_checkpoint: report.checkpoint,
        },
        report.copies[0].commit,
    )
}

#[test]
fn console_defaults_to_all_ipv4_interfaces_and_requires_root_without_starting_anything() {
    assert_eq!(
        Options::default().listen,
        SocketAddr::from(([0, 0, 0, 0], 9040))
    );
    assert!(require_root(0).is_ok());
    assert!(
        require_root(1000)
            .unwrap_err()
            .to_string()
            .contains("manually as root")
    );
}

#[tokio::test]
async fn index_and_status_are_directly_openable_and_do_not_claim_a_service_lock() {
    let app = http::router(state());
    for route in ["/", "/console.js", "/console.css", "/api/status"] {
        let request = Request::builder()
            .uri(route)
            .header("host", "127.0.0.1:9040")
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["x-frame-options"], "DENY");
        assert!(!response.headers().contains_key("set-cookie"));
        assert!(
            !response
                .headers()
                .contains_key("access-control-allow-origin")
        );
        if route == "/api/status" {
            let report = json_body(response).await;
            assert_eq!(report["authentication"], "none");
            assert_eq!(report["csrf"], "test-csrf");
            assert_eq!(report["service_exclusion_established"], false);
            assert_eq!(report["source_writes_available"], false);
        }
    }
}

#[tokio::test]
async fn browser_post_boundary_rejects_missing_duplicate_or_cross_origin_headers_before_disk_access()
 {
    let app = http::router(state());
    for (name, value) in [
        ("origin", "http://evil.example"),
        ("origin", "null"),
        ("sec-fetch-site", "cross-site"),
        ("x-zxs-helper-csrf", "wrong"),
        ("host", "evil.example:9040"),
        ("host", "127.0.0.1:9000"),
    ] {
        let mut request = post("/api/authority/check", json!({}));
        request.headers_mut().insert(name, value.parse().unwrap());
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    for name in ["origin", "host", "x-zxs-helper-csrf"] {
        let mut request = post("/api/authority/check", json!({}));
        request.headers_mut().remove(name);
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    let mut request = post("/api/authority/check", json!({}));
    request
        .headers_mut()
        .append("origin", "http://127.0.0.1:9040".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let mut request = post("/api/authority/check", json!({}));
    request
        .headers_mut()
        .insert("content-type", "text/plain".parse().unwrap());
    assert_eq!(
        app.oneshot(request).await.unwrap().status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
}

#[tokio::test]
async fn exact_disk_check_and_download_use_real_proofs_without_source_or_system_writes() {
    let root = tempfile::tempdir().unwrap();
    let (selection, commit) = fixture(root.path());
    let before = files(root.path());
    let app = http::router(state());
    let response = app
        .clone()
        .oneshot(post(
            "/api/authority/check",
            json!({"selection":selection,"writers_stopped":true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let report = json_body(response).await;
    assert_eq!(report["state"], "complete_success");
    assert_eq!(report["copies"][0]["commit"], commit.to_string());
    assert_eq!(report["copies"][0]["content"]["status"], "verified");
    let response = app
        .oneshot(post(
            "/api/authority/export",
            json!({"selection":selection,"writers_stopped":true,"commit":commit}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["x-zxs-commit"].to_str().unwrap(),
        commit.to_string()
    );
    assert_eq!(
        response.headers()["x-zxs-body-digest"].to_str().unwrap(),
        Digest::body(b"verified body").to_string()
    );
    assert_eq!(
        &to_bytes(response.into_body(), 1 << 20).await.unwrap()[..],
        b"verified body"
    );
    assert_eq!(before, files(root.path()));
}

#[tokio::test]
async fn no_operator_acknowledgment_no_read_and_active_job_is_not_queued() {
    let root = tempfile::tempdir().unwrap();
    let (selection, _) = fixture(root.path());
    let state = state();
    let app = http::router(state.clone());
    let response = app
        .clone()
        .oneshot(post(
            "/api/authority/check",
            json!({"selection":selection,"writers_stopped":false}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let permit = state.jobs.clone().try_acquire_owned().unwrap();
    let response = app
        .oneshot(post("/api/authority/check", json!({})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(response).await["error"]["code"], "helper_busy");
    drop(permit);
}

#[tokio::test]
async fn missing_or_corrupt_body_is_never_downloaded_and_wrong_incarnation_rejects_the_check() {
    let root = tempfile::tempdir().unwrap();
    let (selection, commit) = fixture(root.path());
    let body = selection.path.join("bodies").join(format!("{commit}.bin"));
    fs::remove_file(&body).unwrap();
    let app = http::router(state());
    let response = app
        .clone()
        .oneshot(post(
            "/api/authority/check",
            json!({"selection":selection,"writers_stopped":true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["state"], "content_issues");
    let response = app
        .clone()
        .oneshot(post(
            "/api/authority/export",
            json!({"selection":selection,"writers_stopped":true,"commit":commit}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    fs::write(&body, b"wrong body").unwrap();
    fs::set_permissions(&body, fs::Permissions::from_mode(0o600)).unwrap();
    let response = app
        .clone()
        .oneshot(post(
            "/api/authority/export",
            json!({"selection":selection,"writers_stopped":true,"commit":commit}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let mut wrong = selection;
    wrong.identity.incarnation = Digest::body(b"other");
    let response = app
        .oneshot(post(
            "/api/authority/check",
            json!({"selection":wrong,"writers_stopped":true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn unknown_routes_query_parameters_and_unbounded_input_do_not_reach_a_writer() {
    let app = http::router(state());
    let response = app
        .clone()
        .oneshot(post("/api/password/reset", json!({})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = app
        .clone()
        .oneshot(post("/api/authority/check?path=/tmp", json!({})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = app
        .oneshot(post(
            "/api/authority/check",
            json!({"padding":"x".repeat(MAX_REQUEST_BYTES)}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn shutdown_requires_exact_confirmation_and_stops_new_disk_admission() {
    let state = state();
    let app = http::router(state.clone());
    let response = app
        .clone()
        .oneshot(post("/api/shutdown", json!({"confirmation":"yes"})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!*state.stopping.borrow());
    let response = app
        .clone()
        .oneshot(post("/api/shutdown", json!({"confirmation":"stop-helper"})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(*state.stopping.borrow());
    let response = app
        .oneshot(post("/api/authority/check", json!({})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn installation_payload_is_explicit_and_wrong_confirmation_never_opens_a_target() {
    let app = http::router(state());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/install/payload")
                .header("host", "127.0.0.1:9040")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let metadata = json_body(response).await;
    assert_eq!(metadata["destination"], "/usr/local/bin/zxs");
    assert_eq!(metadata["repair_available"], false);
    assert_eq!(metadata["network_download"], false);
    let response = app
        .oneshot(post(
            "/api/install/commit",
            json!({"request_digest":"unused","confirmation":"not-confirmed"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(
        json_body(response).await["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no installation was attempted")
    );
}

#[tokio::test]
async fn first_storage_routes_require_the_same_origin_before_initialization() {
    for route in [
        "/api/initialization/storage-preview",
        "/api/initialization/storage-recover-preview",
        "/api/initialization/storage-create",
    ] {
        let mut request = post(route, json!({}));
        request
            .headers_mut()
            .insert("origin", "http://foreign.example".parse().unwrap());
        let response = http::router(state()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn samba_password_routes_reject_cross_origin_before_host_or_program_access() {
    let app = http::router(state());
    for path in [
        "/api/credentials/samba-preview",
        "/api/credentials/samba-reset",
        "/api/credentials/samba-status",
    ] {
        let mut request = post(path, json!({}));
        request
            .headers_mut()
            .insert("origin", "http://untrusted.invalid:9040".parse().unwrap());
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
#[tokio::test]
async fn samba_password_malformed_secret_body_does_not_echo_it_or_start_a_program() {
    let response = http::router(state())
        .oneshot(post(
            "/api/credentials/samba-reset",
            json!({"new_password":"must-not-echo-test-password"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert_eq!(body["error"]["code"], "credential_request_json");
    assert!(!body.to_string().contains("must-not-echo-test-password"));
}
