//! Exercise the public HTTP boundary against the real durable engine without a socket.
use http_routing::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use json_codec::{Value, json};
use service_stack::ServiceExt;
use std::{path::Path, sync::Arc};
use storage_contract::{Config, new_id, request_digest};
use storage_engine::{CommitRequest, Engine};
use storage_server::{
    Backend, ServerConfig,
    auth::{AuthStore, Identity},
    engine_backend::EngineBackend,
};
#[path = "http_engine/backup_namespace.rs"]
mod backup_namespace;
#[path = "http_engine/daily_management.rs"]
mod daily_management;
#[path = "http_engine/disk_registration.rs"]
mod disk_registration;
#[path = "http_engine/file_metadata_operations.rs"]
mod file_metadata_operations;
#[path = "http_engine/history_alias_operations.rs"]
mod history_alias_operations;
#[path = "http_engine/history_pages.rs"]
mod history_pages;
#[path = "http_engine/history_query.rs"]
mod history_query;
#[path = "http_engine/optional_identities.rs"]
mod optional_identities;
#[path = "http_engine/paginated_listing.rs"]
mod paginated_listing;
#[path = "http_engine/plan_lookup.rs"]
mod plan_lookup;
#[path = "../unit/projection_failure.rs"]
mod projection_failure;
#[path = "http_engine/published_index_outcome.rs"]
mod published_index_outcome;
#[path = "http_engine/published_reconciliation.rs"]
mod published_reconciliation;
#[path = "../unit/published_reconciliation_failure.rs"]
mod published_reconciliation_failure;
#[path = "http_engine/strict_objects.rs"]
mod strict_objects;
#[path = "http_engine/upload_status.rs"]
mod upload_status;

/// Resolve only fixture payloads for deliberate corruption. Production reads
/// continue through the engine's verified replica and read-lease interfaces.
fn fixture_payload(
    version_directory: &Path,
    state: &storage_contract::State,
) -> std::path::PathBuf {
    let version: storage_contract::FileVersion =
        json_codec::from_slice(&std::fs::read(version_directory.join("version.json")).unwrap())
            .unwrap();
    let storage_name = version.storage_name;
    let history = version_directory.parent().unwrap().parent().unwrap();
    let stable = history.join("file-containers").join(&version.file_id);
    let named = if let Some(location) = state.file_locations.get(&version.file_id) {
        stable.join("namespace").join(location)
    } else {
        stable.join(
            state
                .file_names
                .get(&version.file_id)
                .unwrap_or(&version.creation_name),
        )
    };
    named.join("versions").join(storage_name)
}

async fn send(
    app: &Router,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(
            builder
                .body(Body::from(
                    json_codec::to_vec(&body.unwrap_or_else(|| json!({}))).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (status, json_codec::from_slice(&bytes).unwrap())
}
async fn success(
    app: &Router,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let action = body
        .as_ref()
        .and_then(|body| body.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let (status, value) = send(app, Some(token), method, path, body).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{method} {path} ({action}): {value}"
    );
    value
}
fn application(root: &Path) -> Router {
    let config = Config::development([root.join("d1"), root.join("d2"), root.join("d3")]);
    configured_application(root, config)
}
fn configured_application(root: &Path, config: Config) -> Router {
    let engine = Engine::initialize(&root.join("system"), config).unwrap();
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend
        .protect_auth(
            AuthStore::initialize(&root.join("system"), "admin", "integration-test-password")
                .unwrap(),
        )
        .unwrap();
    storage_server::router(backend, auth, ServerConfig::loopback(&root.join("system"))).unwrap()
}

async fn reopen_after_router_drop(system: &Path) -> Engine {
    // The periodic sampler can hold a temporary strong reference while its
    // current refresh finishes. Dropping the router stops future samples but
    // does not join that in-flight thread. Wait only for its real OS locks;
    // recovery failures and a still-live writer after two seconds remain errors.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match Engine::open(system) {
            Ok(engine) => return engine,
            Err(error) if error.code() == storage_contract::ErrorCode::WriterAlreadyActive => {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                assert!(
                    !remaining.is_zero(),
                    "old router retained its writer locks: {error}"
                );
                tokio::time::sleep(remaining.min(std::time::Duration::from_millis(10))).await;
            }
            Err(error) => panic!("fixture recovery failed after router shutdown: {error}"),
        }
    }
}

#[tokio::test]
async fn pool_upload_limit_is_checked_before_creating_spooled_bytes() {
    let root = tempfile::tempdir().unwrap();
    let mut config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    config.max_file_bytes = 16;
    let app = configured_application(root.path(), config);
    let token = login(&app, "admin", "integration-test-password").await;
    let repo = success(
        &app,
        &token,
        "POST",
        "/api/repositories",
        Some(json!({
            "name":"Small limit", "primary_slot_id":"P01", "target_copies":1,
            "operation_id":new_id("op")
        })),
    )
    .await;
    let workspace = success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({"branch_id":repo["main_branch_id"]})),
    )
    .await;
    let bytes = b"too large for this pool";
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/workspaces/{}/files/{}/content?path=large.txt",
                    workspace["workspace_id"].as_str().unwrap(),
                    new_id("entry")
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-zxs-logical-size", bytes.len().to_string())
                .header("x-zxs-content-hash", blake3::hash(bytes).to_hex().as_str())
                .body(Body::from(bytes.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        std::fs::read_dir(root.path().join("system/api-spool"))
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_name() != ".budget.lock")
            .count(),
        0
    );
}
async fn login(app: &Router, name: &str, password: &str) -> String {
    let (status, value) = send(
        app,
        None,
        "POST",
        "/api/auth/login",
        Some(json!({"username":name,"password":password})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    value["token"].as_str().unwrap().into()
}

/// Exercise real revocation rather than inventing a lower-privileged account.
async fn revoked_session(app: &Router) -> String {
    let token = login(app, "admin", "integration-test-password").await;
    success(app, &token, "POST", "/api/auth/logout", None).await;
    token
}

#[tokio::test]
async fn self_password_change_is_csrf_protected_and_restored_from_pool_control_state() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let (status, session) = send(
        &app,
        None,
        "POST",
        "/api/auth/login",
        Some(json!({
            "username":"admin", "password":"integration-test-password"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = session["token"].as_str().unwrap();
    let cookie = format!("zxs_session={token}");
    let body = json!({"current_password":"integration-test-password", "new_password":"rotated-test-password"});
    for (origin, csrf) in [
        ("http://127.0.0.1:7400", None),
        ("https://foreign.invalid", session["csrf_token"].as_str()),
        ("http://127.0.0.1:7400", Some("wrong-csrf-token")),
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/auth/password")
            .header("host", "127.0.0.1:7400")
            .header("origin", origin)
            .header("cookie", &cookie)
            .header("content-type", "application/json");
        if let Some(csrf) = csrf {
            request = request.header("x-csrf-token", csrf);
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(Body::from(json_codec::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let mut escalated = body.clone();
    escalated["username"] = "other-user".into();
    escalated["admin"] = true.into();
    assert_eq!(
        send(
            &app,
            Some(token),
            "POST",
            "/api/auth/password",
            Some(escalated)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/password")
                .header("host", "127.0.0.1:7400")
                .header("origin", "http://127.0.0.1:7400")
                .header("cookie", &cookie)
                .header("x-csrf-token", session["csrf_token"].as_str().unwrap())
                .header("content-type", "application/json")
                .body(Body::from(json_codec::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    assert_eq!(
        send(&app, Some(token), "GET", "/api/auth/me", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    drop(app);
    // A lost local auth cache must recover the new password from the pool's
    // authoritative, transactionally published control state.
    std::fs::remove_file(root.path().join("system/auth.json")).unwrap();
    let backend = Arc::new(EngineBackend::new(
        reopen_after_router_drop(&root.path().join("system")).await,
    ));
    let auth = backend.open_auth(&root.path().join("system")).unwrap();
    let app = storage_server::router(
        backend,
        auth,
        ServerConfig::loopback(&root.path().join("system")),
    )
    .unwrap();
    assert_eq!(
        send(
            &app,
            None,
            "POST",
            "/api/auth/login",
            Some(json!({"username":"admin", "password":"integration-test-password"}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert!(
        !login(&app, "admin", "rotated-test-password")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn maximum_password_login_and_change_accept_escaped_json_without_losing_body_bounds() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("system");
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let current_password = "\\".repeat(1024);
    let new_password = "\"".repeat(1024);
    let backend = Arc::new(EngineBackend::new(
        Engine::initialize(&system, config).unwrap(),
    ));
    let auth = backend
        .protect_auth(AuthStore::initialize(&system, "admin", &current_password).unwrap())
        .unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    // The login boundary must admit the same legal JSON encoding as a password
    // change, even when every decoded password byte uses a Unicode escape.
    let encoded_login = format!(
        "{{\"username\":\"admin\",\"password\":\"{}\"}}",
        r"\u005c".repeat(1024)
    );
    assert!(encoded_login.len() > 6 * 1024 && encoded_login.len() < 8 * 1024);
    let mut oversized_login = encoded_login.as_bytes().to_vec();
    oversized_login.resize(8 * 1024 + 1, b' ');
    let logged_in = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(encoded_login))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logged_in.status(), StatusCode::OK);
    let session: Value = json_codec::from_slice(
        &http_routing::body::to_bytes(logged_in.into_body(), 8192)
            .await
            .unwrap(),
    )
    .unwrap();
    let oversized = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(oversized_login))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(!oversized.headers().contains_key("set-cookie"));
    let token = session["token"].as_str().expect("login token missing");
    let csrf = session["csrf_token"].as_str().expect("CSRF token missing");
    let cookie = format!("zxs_session={token}");
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/api/auth/password")
            .header("host", "127.0.0.1:7400")
            .header("origin", "http://127.0.0.1:7400")
            .header("cookie", &cookie)
            .header("x-csrf-token", csrf)
            .header("content-type", "application/json")
    };
    let rejected = app
        .clone()
        .oneshot(
            request()
                .body(Body::from(
                    json_codec::to_vec(&json!({
                        "current_password":"incorrect-current-password",
                        "new_password":new_password
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    assert!(!rejected.headers().contains_key("set-cookie"));
    assert_eq!(
        send(&app, Some(token), "GET", "/api/auth/me", None).await.0,
        StatusCode::OK
    );

    // JSON Unicode escapes may use six bytes for each legal password byte.
    // Both decoded passwords stay at the primitive policy's 1024-byte limit.
    let encoded = format!(
        "{{\"current_password\":\"{}\",\"new_password\":\"{}\"}}",
        r"\u005c".repeat(1024),
        r"\u0022".repeat(1024)
    );
    assert!(encoded.len() > 12 * 1024 && encoded.len() < 16 * 1024);
    let changed = app
        .clone()
        .oneshot(request().body(Body::from(encoded)).unwrap())
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::OK);
    assert_eq!(
        send(&app, Some(token), "GET", "/api/auth/me", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            None,
            "POST",
            "/api/auth/login",
            Some(json!({"username":"admin", "password":new_password}))
        )
        .await
        .0,
        StatusCode::OK
    );
}

async fn upload(
    app: &Router,
    token: &str,
    workspace: &str,
    item: &str,
    path: &str,
    bytes: &[u8],
) -> Value {
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("path", path)
        .finish();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/workspaces/{workspace}/files/{item}/content?{query}"
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("x-zxs-logical-size", bytes.len().to_string())
                .header("x-zxs-content-hash", blake3::hash(bytes).to_hex().as_str())
                .header("x-zxs-upload-generation", "1")
                .body(Body::from(bytes.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "upload {path}: {}",
        String::from_utf8_lossy(&bytes)
    );
    let value: Value = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([
            "item_id",
            "path",
            "size",
            "content_hash",
            "dirty_generation",
            "base_version_id"
        ])
    );
    assert!(value["size"].is_string());
    assert!(value["dirty_generation"].is_string());
    value
}

async fn version_bytes(app: &Router, token: &str, version: &str) -> Vec<u8> {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/versions/{version}/content"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    http_routing::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap()
        .to_vec()
}

async fn manager_upload(app: &Router, token: &str, path: &str, bytes: &[u8]) -> Value {
    let workspace = success(
        app,
        token,
        "POST",
        "/api/file-manager/disks/P01/write-sessions",
        Some(json!({"path":path})),
    )
    .await;
    let id = workspace["workspace_id"].as_str().unwrap();
    upload(
        app,
        token,
        id,
        &new_id("entry"),
        workspace["relative_path"].as_str().unwrap(),
        bytes,
    )
    .await;
    let mut body = success(
        app,
        token,
        "POST",
        &format!("/api/workspaces/{id}/seal"),
        Some(json!({})),
    )
    .await;
    body["message"] = format!("Upload {path}").into();
    body["request_digest"] = request_digest(
        "POST",
        "/api/workspaces/{workspace}/commit",
        json!({"workspace":id}),
        json!({}),
        &body,
    )
    .unwrap()
    .into();
    success(
        app,
        token,
        "POST",
        &format!("/api/workspaces/{id}/commit"),
        Some(body),
    )
    .await
}

#[tokio::test]
async fn file_notes_are_authorized_bounded_persistent_and_download_uses_current_container_name() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "original.txt", b"immutable historical bytes").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let file = listing["entries"][0]["file_id"].as_str().unwrap();
    let version = listing["entries"][0]["version_id"].as_str().unwrap();

    let session = login(&app, "admin", "integration-test-password").await;
    let revoked = revoked_session(&app).await;
    assert_eq!(
        send(
            &app,
            Some(&revoked),
            "POST",
            &format!("/api/file-manager/files/{file}/metadata"),
            Some(json!({"note":"cannot edit","operation_id":new_id("op")}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    // Both metadata endpoints count UTF-8 bytes, reject every control character,
    // and accept an empty note as a deletion of the stored metadata.
    for endpoint in [
        format!("/api/file-manager/files/{file}/metadata"),
        format!("/api/file-manager/versions/{version}/metadata"),
    ] {
        for note in [
            "a".repeat(128),
            format!("{}ab", "文".repeat(42)),
            String::new(),
        ] {
            assert_eq!(note.len(), if note.is_empty() { 0 } else { 128 });
            let saved = success(
                &app,
                &token,
                "POST",
                &endpoint,
                Some(json!({"note":note,"operation_id":new_id("op")})),
            )
            .await;
            assert_eq!(saved["result"]["note"], note);
        }
        for note in [
            "a".repeat(129),
            "文".repeat(43),
            "line\nbreak".into(),
            "a\rb".into(),
            "a\tb".into(),
            "a\0b".into(),
            "a\u{7f}b".into(),
        ] {
            assert_eq!(
                send(
                    &app,
                    Some(&token),
                    "POST",
                    &endpoint,
                    Some(json!({"note":note,"operation_id":new_id("op")}))
                )
                .await
                .0,
                StatusCode::BAD_REQUEST
            );
        }
    }
    let metadata = json!({"note":"container note, second line","operation_id":new_id("op")});
    let saved = success(
        &app,
        &token,
        "POST",
        &format!("/api/file-manager/files/{file}/metadata"),
        Some(metadata.clone()),
    )
    .await;
    assert_eq!(
        success(
            &app,
            &token,
            "POST",
            &format!("/api/file-manager/files/{file}/metadata"),
            Some(metadata)
        )
        .await["result"],
        saved["result"]
    );
    success(
        &app,
        &token,
        "POST",
        &format!("/api/file-manager/versions/{version}/metadata"),
        Some(json!({"note":"historical note","operation_id":new_id("op")})),
    )
    .await;
    manager_namespace(
        &app,
        &token,
        "rename",
        "original.txt",
        Some("改名后的文件.txt"),
    )
    .await;
    let history = history_page(&app, &session, file).await;
    let body = content_nodes(&history)[0];
    assert_eq!(history["context"]["note"], "container note, second line");
    assert_eq!(body["note"], "historical note");
    assert!(
        body["storage_name"]
            .as_str()
            .unwrap()
            .starts_with("v000001_")
    );
    assert!(body.get("legacy").is_none());
    assert_eq!(body["download_name"], "改名后的文件.txt");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/versions/{version}/content"))
                .header("authorization", format!("Bearer {session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let disposition = response.headers()["content-disposition"].to_str().unwrap();
    assert!(disposition.contains("filename*=UTF-8''%E6%94%B9"));
    assert!(!disposition.contains("original.txt"));
    assert_eq!(
        &http_routing::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap()[..],
        b"immutable historical bytes"
    );
    manager_namespace(&app, &token, "delete", "改名后的文件.txt", None).await;
    let deleted = history_page(&app, &session, file).await;
    assert_eq!(
        content_nodes(&deleted)[0]["download_name"],
        "改名后的文件.txt",
        "deletion preserves the container's last name for historical downloads"
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/versions/{version}/content"))
                .header("authorization", format!("Bearer {session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("filename*=UTF-8''%E6%94%B9")
    );
    assert_eq!(
        &http_routing::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap()[..],
        b"immutable historical bytes"
    );
    drop(app);
    let backend = Arc::new(EngineBackend::new(
        reopen_after_router_drop(&root.path().join("system")).await,
    ));
    let auth = backend.open_auth(&root.path().join("system")).unwrap();
    let app = storage_server::router(
        backend,
        auth,
        ServerConfig::loopback(&root.path().join("system")),
    )
    .unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let history = history_page(&app, &token, file).await;
    assert_eq!(history["context"]["note"], "container note, second line");
    assert_eq!(content_nodes(&history)[0]["note"], "historical note");
}

#[tokio::test]
async fn file_history_exposes_durable_content_edges_and_rechecks_authentication() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let admin = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &admin, "tree.txt", b"first content").await;
    manager_upload(&app, &admin, "sibling.txt", b"independent file").await;
    let listing = success(
        &app,
        &admin,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let entries = listing["entries"].as_array().unwrap();
    let first = entries
        .iter()
        .find(|entry| entry["name"] == "tree.txt")
        .unwrap();
    let file = first["file_id"].as_str().unwrap();
    let original = first["version_id"].as_str().unwrap();
    let sibling = entries
        .iter()
        .find(|entry| entry["name"] == "sibling.txt")
        .unwrap();
    let history_path = format!("/api/file-manager/files/{file}/history");

    manager_upload(&app, &admin, "tree.txt", b"second main content").await;
    let main = history_page(&app, &admin, file).await;
    let main_version = current_node(&main).unwrap()["version_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(main_version, original);
    let branch = success(
        &app,
        &admin,
        "POST",
        &format!("/api/file-manager/files/{file}/branches"),
        Some(json!({"source_version_id":original,"name":"from-first","operation_id":new_id("op")})),
    )
    .await;
    let branch_id = branch["result"]["branch_id"].as_str().unwrap();
    let forked = history_page(&app, &admin, file).await;
    let pending = forked["branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|branch| branch["branch_id"] == branch_id)
        .unwrap();
    assert_eq!(pending["version_id"], original);
    assert_eq!(pending["inherited"], true);
    assert!(pending["own_version_id"].is_null());
    assert_eq!(pending["visible_content_state"], "AVAILABLE");
    assert_eq!(pending["materialized"], false);
    assert_eq!(pending["content_state"], "PENDING");
    assert_eq!(pending["source_version_id"], original);
    assert_eq!(pending["pending_count"], 1);
    assert_eq!(content_nodes(&forked).len(), 2);
    success(
        &app,
        &admin,
        "POST",
        &format!("/api/file-manager/files/{file}/select"),
        Some(json!({"branch_id":branch_id,"operation_id":new_id("op")})),
    )
    .await;
    manager_upload(&app, &admin, "tree.txt", b"new branch content").await;

    let session = login(&app, "admin", "integration-test-password").await;
    let history = history_page(&app, &session, file).await;
    let nodes = history["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 3);
    for node in nodes {
        assert!(node["node_id"].as_str().is_some_and(|id| !id.is_empty()));
        assert!(matches!(
            node["parents"]["status"].as_str(),
            Some("complete" | "more")
        ));
        assert!(node["parents"]["edges"].as_array().is_some());
        assert!(node["created_at"].is_string());
        assert!(node.get("legacy").is_none());
        assert!(node.get("source_version_ids").is_none());
    }
    let versions = content_nodes(&history);
    assert_eq!(versions.len(), 3);
    assert!(
        versions.windows(2).all(|pair| {
            pair[0]["sequence"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                > pair[1]["sequence"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
        }),
        "HTTP history remains newest first, independent of candidate-index order"
    );
    assert_eq!(
        version_sources(&app, &session, &history, original).await,
        json!([])
    );
    assert_eq!(
        version_sources(&app, &session, &history, &main_version).await,
        json!([original])
    );
    let newest = current_node(&history).unwrap();
    assert_eq!(
        version_sources(
            &app,
            &session,
            &history,
            newest["version_id"].as_str().unwrap()
        )
        .await,
        json!([original])
    );
    assert_ne!(newest["version_id"], main_version);
    assert_eq!(newest["branch_id"], branch_id);
    for version in &versions {
        assert_ne!(version["version_id"], sibling["version_id"]);
        let sources = version_sources(
            &app,
            &session,
            &history,
            version["version_id"].as_str().unwrap(),
        )
        .await;
        assert!(
            sources
                .as_array()
                .unwrap()
                .iter()
                .all(|parent| parent != &sibling["version_id"])
        );
    }
    let sibling_history = history_page(&app, &session, sibling["file_id"].as_str().unwrap()).await;
    assert_eq!(content_nodes(&sibling_history).len(), 1);
    assert_eq!(
        version_sources(
            &app,
            &session,
            &sibling_history,
            sibling["version_id"].as_str().unwrap()
        )
        .await,
        json!([])
    );
    // Body nodes expose their bounded summary fields; the advanced route
    // carries the raw version records.
    let advanced = success(
        &app,
        &session,
        "GET",
        &format!("/api/files/{file}/history"),
        None,
    )
    .await;
    assert_eq!(advanced["versions"].as_array().unwrap().len(), 3);
    assert_eq!(advanced["history_root"], history["history_root"]);

    // This small store uses the scan path. An unavailable query cache must not
    // change the API contract or authentication. Large-store indexed fallback is covered
    // by the core routing/corruption tests, not by this fixture.
    let revoked = revoked_session(&app).await;
    let index = root.path().join("system/index.sqlite");
    let saved_index = root.path().join("system/index.saved");
    std::fs::rename(&index, &saved_index).unwrap();
    std::fs::create_dir(&index).unwrap();
    assert_eq!(
        history_page(&app, &session, file).await,
        history,
        "an unavailable cache must preserve scan-path versions, order, and edges"
    );
    assert_eq!(
        send(&app, Some(&revoked), "GET", &history_path, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED,
        "an unavailable cache must not bypass session revocation"
    );
    std::fs::remove_dir(&index).unwrap();
    std::fs::rename(&saved_index, &index).unwrap();
}

#[tokio::test]
async fn file_manager_exposes_disk_directories_and_independent_file_branches() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let capabilities = success(&app, &token, "GET", "/api/capabilities", None).await;
    assert_eq!(capabilities["full_branches"], false);
    assert_eq!(capabilities["branch_creation"], "record-until-write");
    assert_eq!(capabilities["platforms"], json!({"web":true,"smb":true}));
    assert_eq!(
        send(&app, None, "GET", "/api/file-manager/disks", None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let disks = success(&app, &token, "GET", "/api/file-manager/disks", None).await;
    assert_eq!(disks["disks"][0]["slot_id"], "P01");
    assert_eq!(disks["disks"][0]["writable"], true);
    assert_eq!(disks["disks"][1]["writable"], false);
    assert!(
        success(
            &app,
            &token,
            "GET",
            "/api/file-manager/disks/P01/entries",
            None
        )
        .await["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let mkdir = json!({"kind":"mkdir","path":"documents","operation_id":new_id("op")});
    let first = success(
        &app,
        &token,
        "POST",
        "/api/file-manager/disks/P01/namespace",
        Some(mkdir.clone()),
    )
    .await;
    assert_eq!(
        success(
            &app,
            &token,
            "POST",
            "/api/file-manager/disks/P01/namespace",
            Some(mkdir)
        )
        .await["result"],
        first["result"]
    );
    // Backup browsing requires an explicit protection policy; disk roots start
    // with one primary copy even when backup disks are configured.
    let root_listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let repository = root_listing["context"]["repository_id"].as_str().unwrap();
    let backup_route = format!("/api/repositories/{repository}/backup-configuration");
    let backup = success(&app, &token, "GET", &backup_route, None).await;
    success(
        &app,
        &token,
        "PUT",
        &backup_route,
        Some(json!({
            "path":"", "target_copies":3, "backup_prefixes":{"B01":"P01","C01":"P01"},
            "expected_scope_id":backup["scope"]["id"], "expected_generation":backup["generation"],
            "operation_id":new_id("op")
        })),
    )
    .await;
    manager_upload(&app, &token, "documents/one.txt", b"main file").await;
    manager_upload(&app, &token, "documents/two.txt", b"untouched sibling").await;
    let entries = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=documents",
        None,
    )
    .await;
    let real_source_commit = entries["context"]["base_head"].clone();
    let files = entries["entries"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    let one = files.iter().find(|e| e["name"] == "one.txt").unwrap();
    let two = files.iter().find(|e| e["name"] == "two.txt").unwrap();
    let file = one["file_id"].as_str().unwrap();
    let create_body =
        json!({"source_version_id":one["version_id"],"name":"draft","operation_id":new_id("op")});
    let create_path = format!("/api/file-manager/files/{file}/branches");
    let branch = success(
        &app,
        &token,
        "POST",
        &create_path,
        Some(create_body.clone()),
    )
    .await;
    assert_eq!(branch["result"]["copied_files"], "0");
    assert_eq!(branch["result"]["placeholder_files"], "0");
    assert_eq!(
        success(&app, &token, "POST", &create_path, Some(create_body)).await["result"],
        branch["result"]
    );
    let branch_id = branch["result"]["branch_id"].as_str().unwrap();
    let pending_history = history_page(&app, &token, file).await;
    assert_eq!(content_nodes(&pending_history).len(), 1);
    let pending_branch = pending_history["branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|branch| branch["branch_id"] == branch_id)
        .unwrap();
    assert_eq!(pending_branch["version_id"], one["version_id"]);
    assert_eq!(pending_branch["inherited"], true);
    assert!(pending_branch["own_version_id"].is_null());
    assert_eq!(pending_branch["source_version_id"], one["version_id"]);
    assert_eq!(pending_branch["materialized"], false);
    assert_eq!(pending_branch["current"], false);
    let branch_info = success(
        &app,
        &token,
        "GET",
        &format!("/api/branches/{branch_id}"),
        None,
    )
    .await;
    assert_eq!(branch_info["content_state"], "PENDING");
    for suffix in [
        "tree".to_string(),
        format!(
            "tree?commit_id={}",
            branch_info["head_commit_id"].as_str().unwrap()
        ),
    ] {
        let tree = success(
            &app,
            &token,
            "GET",
            &format!("/api/branches/{branch_id}/{suffix}"),
            None,
        )
        .await;
        let pending_entry = tree["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["file_id"] == file)
            .unwrap();
        assert_eq!(pending_entry["version_id"], one["version_id"]);
        assert_eq!(pending_entry["size"], one["size"]);
    }
    let resolved = success(
        &app,
        &token,
        "POST",
        &format!("/api/branches/{branch_id}/resolve"),
        Some(json!({"path":"documents/one.txt"})),
    )
    .await;
    assert_eq!(resolved["version"]["version_id"], one["version_id"]);
    assert_eq!(resolved["inherited"], true);
    assert!(resolved["own_version_id"].is_null());
    assert_eq!(resolved["content_state"], "PENDING");
    // A main-directory workspace must retain the file branch it captured,
    // even when two selections happen to refer to the same lineage version.
    let stale = success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({"branch_id":entries["context"]["branch_id"]})),
    )
    .await;
    let stale_id = stale["workspace_id"].as_str().unwrap();
    upload(
        &app,
        &token,
        stale_id,
        &new_id("entry"),
        "documents/one.txt",
        b"preserved stale workspace bytes",
    )
    .await;
    success(
        &app,
        &token,
        "POST",
        &format!("/api/file-manager/files/{file}/select"),
        Some(json!({"branch_id":branch_id,"operation_id":new_id("op")})),
    )
    .await;
    let mut stale_commit = success(
        &app,
        &token,
        "POST",
        &format!("/api/workspaces/{stale_id}/seal"),
        Some(json!({})),
    )
    .await;
    stale_commit["message"] = "must not follow a changed selected branch".into();
    stale_commit["request_digest"] = request_digest(
        "POST",
        "/api/workspaces/{workspace}/commit",
        json!({"workspace":stale_id}),
        json!({}),
        &stale_commit,
    )
    .unwrap()
    .into();
    let (status, rejected) = send(
        &app,
        Some(&token),
        "POST",
        &format!("/api/workspaces/{stale_id}/commit"),
        Some(stale_commit),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(rejected["error"]["code"], "FILE_BRANCH_CONFLICT");
    success(
        &app,
        &token,
        "POST",
        &format!("/api/workspaces/{stale_id}/abort"),
        Some(json!({})),
    )
    .await;
    let pending_listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=documents",
        None,
    )
    .await;
    let pending_entry = pending_listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["file_id"] == file)
        .unwrap();
    assert_eq!(pending_entry["version_id"], one["version_id"]);
    assert_eq!(pending_entry["materialized"], false);
    assert_eq!(pending_entry["inherited"], true);
    assert!(pending_entry["own_version_id"].is_null());
    assert_eq!(pending_entry["size"], one["size"]);
    assert_eq!(pending_entry["source_version_id"], one["version_id"]);
    assert_eq!(pending_entry["selected_branch_id"], branch_id);
    let pending_backup = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/B01/entries?path=P01%2Fdocuments",
        None,
    )
    .await;
    assert_eq!(pending_backup["entries"].as_array().unwrap().len(), 2);
    assert_eq!(
        pending_backup["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["file_id"] == file)
            .unwrap()["version_id"],
        one["version_id"]
    );
    assert_eq!(
        version_bytes(&app, &token, one["version_id"].as_str().unwrap()).await,
        b"main file"
    );
    let before_aborted = history_page(&app, &token, file).await;
    let untouched = success(
        &app,
        &token,
        "POST",
        "/api/file-manager/disks/P01/write-sessions",
        Some(json!({"path":"documents/one.txt"})),
    )
    .await;
    success(
        &app,
        &token,
        "POST",
        &format!(
            "/api/workspaces/{}/abort",
            untouched["workspace_id"].as_str().unwrap()
        ),
        Some(json!({})),
    )
    .await;
    assert_eq!(history_page(&app, &token, file).await, before_aborted);
    drop(app);
    let backend = Arc::new(EngineBackend::new(
        reopen_after_router_drop(&root.path().join("system")).await,
    ));
    let auth = backend.open_auth(&root.path().join("system")).unwrap();
    let app = storage_server::router(
        backend,
        auth,
        ServerConfig::loopback(&root.path().join("system")),
    )
    .unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    assert_eq!(
        history_page(&app, &token, file).await,
        before_aborted,
        "a cold reopen binds the same exact history root"
    );
    manager_upload(&app, &token, "documents/one.txt", b"selected file draft").await;
    let entries = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=documents",
        None,
    )
    .await;
    assert_eq!(
        entries["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "two.txt")
            .unwrap()["version_id"],
        two["version_id"]
    );
    let history = history_page(&app, &token, file).await;
    assert_eq!(history["context"]["selected_branch_id"], branch_id);
    assert_eq!(history["branches"].as_array().unwrap().len(), 2);
    assert_eq!(content_nodes(&history).len(), 2);
    let materialized = history["branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|branch| branch["branch_id"] == branch_id)
        .unwrap();
    assert_eq!(materialized["materialized"], true);
    assert_eq!(materialized["inherited"], false);
    assert_eq!(materialized["own_version_id"], materialized["version_id"]);
    assert!(materialized["source_version_id"].is_null());
    assert!(materialized["version_id"].is_string());
    // Historical registration views keep the parent version after the branch
    // gets its own content, and never claim that parent as an owned version.
    let pending_commit = branch_info["head_commit_id"].as_str().unwrap();
    let old_pending = success(
        &app,
        &token,
        "GET",
        &format!("/api/branches/{branch_id}/tree?commit_id={pending_commit}"),
        None,
    )
    .await;
    let old_entry = old_pending["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["file_id"] == file)
        .unwrap();
    assert_eq!(old_entry["version_id"], one["version_id"]);
    assert_eq!(old_entry["size"], one["size"]);
    assert_eq!(
        old_pending["pending_file_versions"][file],
        one["version_id"]
    );
    let old_resolved = success(
        &app,
        &token,
        "POST",
        &format!("/api/branches/{branch_id}/resolve"),
        Some(json!({"path":"documents/one.txt","commit_id":pending_commit})),
    )
    .await;
    assert_eq!(old_resolved["version"]["version_id"], one["version_id"]);
    assert_eq!(old_resolved["inherited"], true);
    assert!(old_resolved["own_version_id"].is_null());
    assert_eq!(old_resolved["source_version_id"], one["version_id"]);
    assert_eq!(old_resolved["content_state"], "PENDING");
    let actual_source = success(
        &app,
        &token,
        "POST",
        &format!("/api/branches/{branch_id}/resolve"),
        Some(json!({"path":"documents/one.txt","commit_id":real_source_commit})),
    )
    .await;
    assert!(actual_source["version"].is_object());
    assert_eq!(actual_source["version"]["version_id"], one["version_id"]);
    assert!(
        content_nodes(&history)
            .iter()
            .all(|v| v["created_at"].is_string())
    );
    assert_eq!(
        send(
            &app,
            Some(&token),
            "POST",
            "/api/file-manager/disks/B01/write-sessions",
            Some(json!({"path":"P01/documents/one.txt"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let backup = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/B01/entries?path=P01%2Fdocuments",
        None,
    )
    .await;
    assert_eq!(backup["writable"], false);
    assert_eq!(backup["entries"].as_array().unwrap().len(), 2);
    assert_eq!(
        send(
            &app,
            Some(&token),
            "GET",
            "/api/file-manager/disks/P01/entries?path=..%2F..",
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let events = success(&app, &token, "GET", "/api/file-manager/history", None).await;
    assert!(
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "file_branch_pending"
                && e["content_state"] == "PENDING"
                && e["inherited"] == true
                && e["own_version_id"].is_null()
                && e["version_id"] == one["version_id"])
    );
    assert!(
        !events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "delete" && e["path"] == "documents/two.txt")
    );
    manager_namespace(
        &app,
        &token,
        "rename",
        "documents/two.txt",
        Some("documents/renamed.txt"),
    )
    .await;
    let deleted = manager_namespace(&app, &token, "delete", "documents/renamed.txt", None).await;
    assert_eq!(
        deleted["result"]["transfers"][0]["source"]["path"],
        "documents/renamed.txt"
    );
    let events = success(&app, &token, "GET", "/api/file-manager/history", None).await;
    assert!(events["events"].as_array().unwrap().iter().any(|e| {
        e["action"] == "directory.namespace.delete"
            && deleted["result"]["changes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|change| change["head_tree_id"] == e["head_tree_id"])
    }));
}

#[tokio::test]
async fn directory_branches_compose_shared_files_and_refuse_partial_selection() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "folder/a.txt", b"A").await;
    manager_upload(&app, &token, "folder/nested/b.txt", b"B").await;
    manager_upload(&app, &token, "folder/c.txt", b"C").await;
    manager_upload(&app, &token, "outside.txt", b"outside").await;
    let parent_folder = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=folder",
        None,
    )
    .await;
    let parent_nested = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=folder%2Fnested",
        None,
    )
    .await;
    let parent_a = parent_folder["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "a.txt")
        .unwrap();
    let parent_b = parent_nested["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "b.txt")
        .unwrap();
    let parent_c = parent_folder["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "c.txt")
        .unwrap();
    let before = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let outside = before["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "outside.txt")
        .unwrap()["version_id"]
        .clone();
    let body = json!({"path":"folder","name":"draft","operation_id":new_id("op")});
    let created = success(
        &app,
        &token,
        "POST",
        "/api/file-manager/disks/P01/directory-branches",
        Some(body.clone()),
    )
    .await;
    assert_eq!(created["result"]["count"], "3");
    assert_eq!(created["result"]["copied_files"], "0");
    assert_eq!(created["result"]["placeholder_files"], "0");
    assert_eq!(created["result"]["materialized"], false);
    assert_eq!(created["result"]["branches"].as_array().unwrap().len(), 3);
    assert_eq!(
        success(
            &app,
            &token,
            "POST",
            "/api/file-manager/disks/P01/directory-branches",
            Some(body)
        )
        .await["result"],
        created["result"]
    );
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=folder",
        None,
    )
    .await;
    assert_eq!(listing["directory_file_count"], "3");
    assert_eq!(listing["directory_branches_supported"], true);
    assert!(listing["directory_branches"].as_array().unwrap().len() >= 2);
    assert!(
        listing["directory_branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|branch| branch["current"] == true && branch["materialized"] == true)
    );
    success(
        &app,
        &token,
        "POST",
        "/api/file-manager/disks/P01/directory-select",
        Some(json!({"path":"folder","name":"draft","operation_id":new_id("op")})),
    )
    .await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=folder",
        None,
    )
    .await;
    assert!(
        listing["directory_branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|branch| branch["name"] == "draft" && branch["current"] == true)
    );
    let pending_a = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "a.txt")
        .unwrap();
    assert_eq!(pending_a["materialized"], false);
    assert!(pending_a["version_id"].is_string());
    assert_eq!(pending_a["inherited"], true);
    assert!(pending_a["own_version_id"].is_null());
    assert_eq!(pending_a["source_version_id"], parent_a["version_id"]);
    assert_eq!(pending_a["version_id"], parent_a["version_id"]);
    for (parent, expected) in [
        (parent_a, b"A".as_slice()),
        (parent_b, b"B".as_slice()),
        (parent_c, b"C".as_slice()),
    ] {
        let history = history_page(&app, &token, parent["file_id"].as_str().unwrap()).await;
        assert_eq!(
            content_nodes(&history).len(),
            1,
            "registering and selecting branches never copies content"
        );
        assert_eq!(history["context"]["inherited"], true);
        assert_eq!(history["context"]["version_id"], parent["version_id"]);
        assert!(history["context"]["own_version_id"].is_null());
        assert_eq!(
            history["branches"].as_array().unwrap().len(),
            1,
            "directory registration must reuse each existing shared file branch"
        );
        assert_eq!(
            version_bytes(
                &app,
                &token,
                history["context"]["version_id"].as_str().unwrap()
            )
            .await,
            expected
        );
    }
    manager_upload(
        &app,
        &token,
        "folder/a.txt",
        b"only A owns new branch bytes",
    )
    .await;
    let nested = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=folder%2Fnested",
        None,
    )
    .await;
    let pending_b = nested["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "b.txt")
        .unwrap();
    assert_eq!(pending_b["materialized"], false);
    assert!(pending_b["version_id"].is_string());
    assert_eq!(pending_b["inherited"], true);
    assert!(pending_b["own_version_id"].is_null());
    let b_history = history_page(&app, &token, pending_b["file_id"].as_str().unwrap()).await;
    assert_eq!(content_nodes(&b_history).len(), 1);
    let a_history = history_page(&app, &token, pending_a["file_id"].as_str().unwrap()).await;
    assert_eq!(content_nodes(&a_history).len(), 2);
    let mixed = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=folder",
        None,
    )
    .await;
    let written_a = mixed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "a.txt")
        .unwrap();
    let inherited_c = mixed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "c.txt")
        .unwrap();
    assert_eq!(written_a["materialized"], true);
    assert_eq!(written_a["inherited"], false);
    assert_eq!(written_a["own_version_id"], written_a["version_id"]);
    assert_ne!(written_a["version_id"], parent_a["version_id"]);
    assert_eq!(
        version_bytes(&app, &token, written_a["version_id"].as_str().unwrap()).await,
        b"only A owns new branch bytes"
    );
    for (inherited, parent, expected) in [
        (pending_b, parent_b, b"B".as_slice()),
        (inherited_c, parent_c, b"C".as_slice()),
    ] {
        assert_eq!(inherited["materialized"], false);
        assert_eq!(inherited["inherited"], true);
        assert_eq!(inherited["version_id"], parent["version_id"]);
        assert_eq!(inherited["source_version_id"], parent["version_id"]);
        assert_eq!(inherited["visible_content_state"], "AVAILABLE");
        assert!(inherited["own_version_id"].is_null());
        assert_eq!(
            version_bytes(&app, &token, inherited["version_id"].as_str().unwrap()).await,
            expected
        );
    }
    assert_eq!(
        version_bytes(&app, &token, parent_a["version_id"].as_str().unwrap()).await,
        b"A",
        "writing the branch never changes the parent version"
    );
    assert_eq!(
        version_bytes(&app, &token, outside.as_str().unwrap()).await,
        b"outside"
    );
    let after = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    assert_eq!(
        after["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "outside.txt")
            .unwrap()["version_id"],
        outside
    );
    manager_upload(&app, &token, "folder/new.txt", b"new").await;
    success(
        &app,
        &token,
        "POST",
        "/api/file-manager/disks/P01/directory-select",
        Some(json!({
            "path":"folder","name":"main","operation_id":new_id("op")
        })),
    )
    .await;
    let restored = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=folder",
        None,
    )
    .await;
    assert!(
        restored["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["name"] != "new.txt"),
        "P1 restores the complete old namespace, without rejecting new files absent from it"
    );
    assert_eq!(
        restored["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == "a.txt")
            .unwrap()["version_id"],
        parent_a["version_id"]
    );
    assert_eq!(
        send(
            &app,
            Some(&token),
            "POST",
            "/api/file-manager/disks/B01/directory-branches",
            Some(json!({"path":"P01/folder","name":"readonly","operation_id":new_id("op")}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    success(&app,&token,"POST","/api/repositories",Some(json!({"name":"Earlier collection","primary_slot_id":"P01","target_copies":3,"operation_id":new_id("op")}))).await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    assert_eq!(listing["directory_branches_supported"], false);
    assert_eq!(
        send(
            &app,
            Some(&token),
            "POST",
            "/api/file-manager/disks/P01/directory-branches",
            Some(json!({"path":"","name":"root partial forbidden","operation_id":new_id("op")}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn durable_http_save_seals_the_real_manifest_and_keeps_original_publication_result() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let repo=success(&app,&token,"POST","/api/repositories",Some(json!({"name":"HTTP repository","primary_slot_id":"P01","target_copies":3,"operation_id":new_id("op")}))).await;
    let branch = repo["main_branch_id"].as_str().unwrap();
    let workspace = success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({"branch_id":branch})),
    )
    .await;
    let ws = workspace["workspace_id"].as_str().unwrap();
    success(
        &app,
        &token,
        "POST",
        &format!("/api/workspaces/{ws}/namespace-operations"),
        Some(json!({"kind":"mkdir","path":"documents","destination":null})),
    )
    .await;
    let item = new_id("entry");
    let bytes = b"a complete independent full file";
    upload(&app, &token, ws, &item, "documents/file.txt", bytes).await;
    let captured = success(&app, &token, "GET", &format!("/api/workspaces/{ws}"), None).await;
    assert_eq!(captured["workspace_id"], ws);
    assert_eq!(captured["files"][&item]["size"], bytes.len().to_string());
    assert!(captured["files"][&item].get("source_path").is_none());
    let (missing_status, missing) = send(
        &app,
        Some(&token),
        "GET",
        &format!(
            "/api/workspaces/{ws}/files/{}/upload-status",
            new_id("entry")
        ),
        None,
    )
    .await;
    assert_eq!(missing_status, StatusCode::NOT_FOUND);
    assert_eq!(missing["error"]["code"], "NOT_FOUND");
    let (absent_status, absent) = send(
        &app,
        Some(&token),
        "GET",
        &format!(
            "/api/workspaces/{}/files/{item}/upload-status",
            new_id("workspace")
        ),
        None,
    )
    .await;
    assert_eq!(absent_status, StatusCode::NOT_FOUND);
    assert_eq!(absent["error"]["code"], "WORKSPACE_NOT_FOUND");

    let status = success(
        &app,
        &token,
        "GET",
        &format!("/api/workspaces/{ws}/files/{item}/upload-status"),
        None,
    )
    .await;
    assert_eq!(status["state"], "COMPLETE");
    assert_eq!(
        status["durable_ranges"][0]["content_hash"],
        blake3::hash(bytes).to_hex().as_str()
    );
    let mut body = success(
        &app,
        &token,
        "POST",
        &format!("/api/workspaces/{ws}/seal"),
        Some(json!({})),
    )
    .await;
    body["message"] = "Save a complete file".into();
    let digest = request_digest(
        "POST",
        "/api/workspaces/{workspace}/commit",
        json!({"workspace":ws}),
        json!({}),
        &body,
    )
    .unwrap();
    body["request_digest"] = digest.into();
    let sealed = success(
        &app,
        &token,
        "GET",
        &format!("/api/workspaces/{ws}/files/{item}/upload-status"),
        None,
    )
    .await;
    assert_eq!(sealed["state"], "SEALED");
    assert_eq!(sealed["sealed_manifest_hash"], body["sealed_manifest_hash"]);
    let saved = success(
        &app,
        &token,
        "POST",
        &format!("/api/workspaces/{ws}/commit"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(saved["state"], "completed");
    assert_eq!(saved["operation_kind"], "content_save");
    assert_eq!(saved["result"]["outcome"], "COMMITTED_AND_PROTECTED");
    assert_eq!(saved["result"]["protection_at_publish"], "TARGET_MET");
    assert_eq!(
        saved["result"]["versions"][0]["required_slots"],
        json!(["P01", "B01", "C01"])
    );
    let retry = success(
        &app,
        &token,
        "POST",
        &format!("/api/workspaces/{ws}/commit"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(retry["result"], saved["result"]);
    body["message"] = "A different request may not reuse this operation".into();
    let (status, _) = send(
        &app,
        Some(&token),
        "POST",
        &format!("/api/workspaces/{ws}/commit"),
        Some(body),
    )
    .await;
    assert!(!status.is_success());
    let version = saved["result"]["versions"][0]["version_id"]
        .as_str()
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/versions/{version}/content"))
                .header("authorization", format!("Bearer {token}"))
                .header("range", "bytes=2-9")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        &http_routing::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap()[..],
        &bytes[2..10]
    );
    let checksums = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/versions/{version}/checksums"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(checksums.status(), StatusCode::OK);
    assert!(
        !http_routing::body::to_bytes(checksums.into_body(), 65536)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn authentication_applies_to_direct_content_graph_and_write_paths() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let admin = login(&app, "admin", "integration-test-password").await;
    let repo = success(&app, &admin, "POST", "/api/repositories", Some(json!({"name":"Private","primary_slot_id":"P01","target_copies":1,"operation_id":new_id("op")}))).await;
    manager_upload(&app, &admin, "Private/body.txt", b"authenticated content").await;
    let listing = success(
        &app,
        &admin,
        "GET",
        "/api/file-manager/disks/P01/entries?path=Private",
        None,
    )
    .await;
    let version = listing["entries"][0]["version_id"].as_str().unwrap();
    assert_eq!(
        version_bytes(&app, &admin, version).await,
        b"authenticated content"
    );
    let revoked = revoked_session(&app).await;
    for token in [None, Some(revoked.as_str())] {
        for path in [
            format!(
                "/api/repositories/{}",
                repo["repository_id"].as_str().unwrap()
            ),
            format!(
                "/api/branches/{}/tree",
                repo["main_branch_id"].as_str().unwrap()
            ),
            format!(
                "/api/history-trees/{}/graph",
                repo["history_tree_id"].as_str().unwrap()
            ),
            format!("/api/versions/{version}/content"),
            "/api/auth/me".into(),
        ] {
            assert_eq!(
                send(&app, token, "GET", &path, None).await.0,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
        }
        assert_eq!(
            send(
                &app,
                token,
                "POST",
                "/api/workspaces",
                Some(json!({"branch_id":repo["main_branch_id"]}))
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    // Revoking one session does not discard durable content or a different
    // authenticated session belonging to the same administrator.
    assert_eq!(
        version_bytes(&app, &admin, version).await,
        b"authenticated content"
    );
}

#[tokio::test]
async fn historical_plan_publishes_atomically_and_management_rules_use_generation_checks() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let repo = success(
        &app,
        &token,
        "POST",
        "/api/repositories",
        Some(json!({
            "name":"Historical HTTP", "primary_slot_id":"P01", "target_copies":1,
            "operation_id":new_id("op")
        })),
    )
    .await;
    let repository = repo["repository_id"].as_str().unwrap();
    let branch = repo["main_branch_id"].as_str().unwrap();
    let history = repo["history_tree_id"].as_str().unwrap();
    let original = success(
        &app,
        &token,
        "GET",
        &format!("/api/branches/{branch}"),
        None,
    )
    .await;
    let source = original["head_commit_id"].as_str().unwrap();
    let policy = new_id("policy");
    let policy_path = format!("/api/policies/{policy}");
    let policy_body = json!({"repository_id":repository,"path_prefix":"","target_copies":3,
        "expected_generation":"0","operation_id":new_id("op")});
    success(&app, &token, "PUT", &policy_path, Some(policy_body.clone())).await;
    assert_eq!(
        success(&app, &token, "GET", &policy_path, None).await["target_copies"],
        3
    );
    let mut stale = policy_body;
    stale["operation_id"] = new_id("op").into();
    assert_eq!(
        send(&app, Some(&token), "PUT", &policy_path, Some(stale))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let rule = new_id("rule");
    let rule_path = format!("/api/primary-placement-rules/{rule}");
    success(
        &app,
        &token,
        "PUT",
        &rule_path,
        Some(json!({
            "repository_id":repository,"path_prefix":"","primary_slot_id":"P01",
            "expected_generation":"0","operation_id":new_id("op")
        })),
    )
    .await;
    assert_eq!(
        success(&app, &token, "GET", &rule_path, None).await["primary_slot_id"],
        "P01"
    );
    let workspace = success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({
            "branch_id":branch, "source_commit_id":source
        })),
    )
    .await;
    let ws = workspace["workspace_id"].as_str().unwrap();
    upload(
        &app,
        &token,
        ws,
        &new_id("entry"),
        "historical.txt",
        b"atomic historical full save",
    )
    .await;
    let plan = success(
        &app,
        &token,
        "POST",
        &format!("/api/history-trees/{history}/historical-save-plans"),
        Some(json!({
            "source_commit_id":source,"name":"Independent historical branch","workspace_id":ws,
            "primary_slot_id":null,"target_copies":null
        })),
    )
    .await;
    let plan_id = plan["plan_id"].as_str().unwrap();
    let operation = storage_contract::OperationId::generate();
    let body = json!({"plan_digest":plan["digest"],"operation_id":operation});
    let result = success(
        &app,
        &token,
        "POST",
        &format!("/api/plans/{plan_id}/commit"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(result["commit_state"], "PUBLISHED");
    assert_eq!(
        success(
            &app,
            &token,
            "POST",
            &format!("/api/plans/{plan_id}/commit"),
            Some(body)
        )
        .await,
        result
    );
    assert_eq!(
        success(
            &app,
            &token,
            "GET",
            &format!("/api/branches/{branch}"),
            None
        )
        .await["head_commit_id"],
        source
    );
    let independent = result["result"]["branch_id"].as_str().unwrap();
    let tree = success(
        &app,
        &token,
        "GET",
        &format!("/api/branches/{independent}/tree"),
        None,
    )
    .await;
    let version = tree["entries"][0]["version_id"].as_str().unwrap();
    let placement = success(
        &app,
        &token,
        "GET",
        &format!("/api/versions/{version}/placement"),
        None,
    )
    .await;
    assert_eq!(placement["target_copies"], 3);
    let binding_path = format!("/api/repositories/{repository}/smb-binding");
    let binding = success(&app, &token, "GET", &binding_path, None).await;
    success(&app, &token, "PUT", &binding_path, Some(json!({
        "branch_id":independent,"enabled":true,"expected_history_generation":binding["history_generation"],
        "operation_id":new_id("op")
    }))).await;
    assert_eq!(
        success(&app, &token, "GET", &binding_path, None).await["branch_id"],
        independent
    );
}

fn capture_wire(name: &str, wire: &Value) {
    if let Some(directory) = std::env::var_os("ZXS_TEST_WIRE_EVIDENCE_DIR") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join(name),
            json_codec::to_vec_pretty(wire).unwrap(),
        )
        .unwrap();
    }
}

fn seed_engine_authority(engine: &mut Engine, system: &Path) {
    let auth = AuthStore::initialize(system, "admin", "integration-test-password").unwrap();
    engine
        .persist_auth_state(
            auth.snapshot().unwrap(),
            "system_auth",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
}

async fn operation_over_http(
    engine: Engine,
    system: &Path,
    operation: storage_contract::OperationId,
    outcome_evidence: &str,
) -> Value {
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend
        .protect_auth(AuthStore::open(system).unwrap())
        .unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let outcome_path = format!("/api/operations/{operation}/outcome?unused=accepted");
    let outcome = success(&app, &token, "GET", &outcome_path, None).await;
    assert!(outcome.get("current_health").is_none());
    capture_wire(outcome_evidence, &outcome);
    let observation = success(
        &app,
        &token,
        "GET",
        &format!("/api/operations/{operation}?unused=accepted"),
        None,
    )
    .await;
    assert!(observation["current_health"]["checked_at"].is_string());
    let mut observed_outcome = observation.clone();
    observed_outcome
        .as_object_mut()
        .unwrap()
        .remove("current_health");
    assert_eq!(observed_outcome, outcome);
    assert_eq!(
        success(&app, &token, "GET", &outcome_path, None).await,
        outcome,
        "a diagnostic read must not add observations to the cheap outcome"
    );
    observation
}

#[tokio::test]
async fn recovered_permanent_loss_returns_degraded_save_wire_with_frozen_unavailable_slots() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("system");
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let mut engine = Engine::initialize(&system, config).unwrap();
    seed_engine_authority(&mut engine, &system);
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Lost candidate").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let lost = engine
        .stage_bytes(
            &workspace.workspace_id,
            "lost.txt",
            b"permanently lost original payload",
        )
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "healthy.txt",
            b"separately retained full payload",
        )
        .unwrap();
    let operation = storage_contract::OperationId::generate();
    engine.inject_fault_once("after_accept_2");
    assert!(
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: operation,
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "fault before publication".into(),
                request_digest: None,
            })
            .is_err()
    );
    // All complete copies plus the private staging source are deliberately lost.
    for disk in &engine.config().disks {
        let directory = disk
            .root
            .join("histories")
            .join(&repo.history_tree_id)
            .join("versions");
        for version in std::fs::read_dir(directory).unwrap() {
            let directory = version.unwrap().path();
            let metadata: storage_contract::FileVersion =
                json_codec::from_slice(&std::fs::read(directory.join("version.json")).unwrap())
                    .unwrap();
            if metadata.creation_name != "lost.txt" {
                continue;
            }
            let path = fixture_payload(&directory, engine.state());
            if path.exists() {
                std::fs::remove_file(path).unwrap();
            }
        }
    }
    std::fs::remove_file(lost.source_path).unwrap();
    drop(engine);
    let mut engine = Engine::open_for_recovery(&system).unwrap();
    let plan = engine
        .prepare_degraded_recovery(
            operation,
            "admin",
            "All fixed copies and the independent staged source were permanently lost",
        )
        .unwrap();
    engine
        .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "admin", true)
        .unwrap();
    let wire = operation_over_http(
        engine,
        &system,
        operation,
        "degraded-save-operation-outcome.json",
    )
    .await;
    assert_eq!(wire["state"], "completed");
    assert_eq!(wire["commit_state"], "PUBLISHED_DEGRADED");
    assert_eq!(wire["result"]["outcome"], "COMMITTED_DEGRADED");
    assert_eq!(wire["result"]["protection_at_publish"], "BELOW_TARGET");
    assert_eq!(wire["current_health"]["state"], "UNAVAILABLE");
    let versions = wire["result"]["versions"].as_array().unwrap();
    assert_eq!(
        versions
            .iter()
            .filter(|version| version["slots"]
                .as_array()
                .unwrap()
                .iter()
                .all(|slot| slot["state"] == "UNAVAILABLE"))
            .count(),
        1
    );
    assert_eq!(
        versions
            .iter()
            .filter(|version| version["slots"]
                .as_array()
                .unwrap()
                .iter()
                .all(|slot| slot["state"] == "COMPLETE_VERIFIED"))
            .count(),
        1
    );
    capture_wire("degraded-save-operation.json", &wire);
}

#[tokio::test]
async fn damage_after_normal_publication_keeps_original_target_met_in_save_wire() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    seed_engine_authority(&mut engine, &root.path().join("system"));
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Published evidence").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "published.txt",
            b"normal full protected publication",
        )
        .unwrap();
    let operation = storage_contract::OperationId::generate();
    let saved = engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation,
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "normal publication".into(),
            request_digest: None,
        })
        .unwrap();
    let version = saved.result["version_protection"][0]["version_id"]
        .as_str()
        .unwrap();
    let primary = engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.slot_id == "P01")
        .unwrap();
    let corrupted = fixture_payload(
        &primary
            .root
            .join("histories")
            .join(&repo.history_tree_id)
            .join("versions")
            .join(version),
        engine.state(),
    );
    std::fs::remove_file(&corrupted).unwrap();
    std::fs::write(&corrupted, b"corruption after successful publication").unwrap();
    let wire = operation_over_http(
        engine,
        &root.path().join("system"),
        operation,
        "damaged-published-save-operation-outcome.json",
    )
    .await;
    assert_eq!(wire["commit_state"], "PUBLISHED");
    assert_eq!(wire["result"]["outcome"], "COMMITTED_AND_PROTECTED");
    assert_eq!(wire["result"]["protection_at_publish"], "TARGET_MET");
    assert_eq!(wire["current_health"]["state"], "DEGRADED");
    assert!(
        wire["result"]["versions"][0]["slots"]
            .as_array()
            .unwrap()
            .iter()
            .all(|slot| slot["state"] == "COMPLETE_VERIFIED")
    );
    capture_wire("damaged-published-save-operation.json", &wire);
}

#[tokio::test]
async fn default_directory_branch_is_shared_authorized_and_persistent() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let admin = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &admin, "one.txt", b"frozen parent").await;
    manager_upload(&app, &admin, "folder/two.txt", b"second parent").await;
    let listing = success(
        &app,
        &admin,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let repository = listing["context"]["repository_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let namespace = listing["context"]["branch_id"].as_str().unwrap().to_owned();
    let original = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "one.txt")
        .unwrap()
        .clone();
    let api = format!("/api/repositories/{repository}/default-branch");
    let initial = success(&app, &admin, "GET", &api, None).await;
    let frozen_registration = success(
        &app,
        &admin,
        "POST",
        "/api/file-manager/disks/P01/directory-branches",
        Some(json!({
            "path":"","name":"frozen","operation_id":new_id("op")
        })),
    )
    .await;
    let frozen = frozen_registration["result"]["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    manager_upload(&app, &admin, "one.txt", b"later shared branch HEAD").await;
    let later = success(&app, &admin, "GET", &api, None).await;
    assert_eq!(
        later["directory_branch_id"], initial["directory_branch_id"],
        "ordinary saves keep the named directory branch selected"
    );
    assert_eq!(later["namespace_branch_id"], namespace);

    let later_tree = success(
        &app,
        &admin,
        "GET",
        &format!("/api/branches/{namespace}/tree"),
        None,
    )
    .await;
    let latest_a = later_tree["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "one.txt")
        .unwrap()["version_id"]
        .clone();

    let session = login(&app, "admin", "integration-test-password").await;
    assert_eq!(
        success(&app, &session, "GET", &api, None).await["namespace_branch_id"],
        namespace
    );
    let selection_operation = new_id("op");
    let selected = success(
        &app,
        &session,
        "PUT",
        &api,
        Some(json!({
            "directory_branch_id":frozen,"operation_id":selection_operation
        })),
    )
    .await;
    assert_eq!(selected["result"]["directory_branch_id"], frozen);
    let replay = success(
        &app,
        &session,
        "PUT",
        &api,
        Some(json!({"directory_branch_id":frozen,"operation_id":selection_operation})),
    )
    .await;
    // Mutation replay returns the permanent outcome, not a fabricated health
    // observation with a new timestamp.
    assert!(selected.get("current_health").is_none());
    assert!(replay.get("current_health").is_none());
    assert_eq!(replay, selected);
    let changed = send(
        &app,
        Some(&session),
        "PUT",
        &api,
        Some(json!({"directory_branch_id":"missing-directory","operation_id":selection_operation})),
    )
    .await;
    assert_eq!(changed.0, StatusCode::CONFLICT);
    assert_eq!(changed.1["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
    for body in [
        json!({"repository_id":"other","directory_branch_id":frozen,"operation_id":new_id("op")}),
        json!({"directory_branch_id":frozen,"operation_id":new_id("op"),"principal":"forged"}),
    ] {
        let invalid = send(&app, Some(&session), "PUT", &api, Some(body)).await;
        assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
        assert_eq!(invalid.1["error"]["code"], "INVALID_REQUEST");
    }
    let invalid = send(
        &app,
        Some(&session),
        "PUT",
        &api,
        Some(json!({"directory_branch_id":frozen,"operation_id":"bad-id"})),
    )
    .await;
    assert_eq!(invalid.1["error"]["code"], "INVALID_ID");
    assert_eq!(
        send(
            &app,
            None,
            "PUT",
            &api,
            Some(json!({"operation_id":"bad-id"}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );

    let current = success(&app, &session, "GET", &api, None).await;
    assert_eq!(current["directory_branch_id"], frozen);
    assert_eq!(current["namespace_branch_id"], namespace);
    let tree_path = format!("/api/branches/{namespace}/tree");
    let tree = success(&app, &session, "GET", &tree_path, None).await;
    let visible = tree["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "one.txt")
        .unwrap();
    assert_eq!(visible["version_id"], original["version_id"]);
    assert_eq!(
        version_bytes(&app, &session, visible["version_id"].as_str().unwrap()).await,
        b"frozen parent"
    );
    let repeated = success(
        &app,
        &session,
        "PUT",
        &api,
        Some(json!({
            "directory_branch_id":frozen,"operation_id":new_id("op")
        })),
    )
    .await;
    assert_eq!(repeated["result"]["no_change"], true);
    assert_eq!(
        success(&app, &session, "GET", &tree_path, None).await["commit_id"],
        tree["commit_id"]
    );

    let wrong = send(
        &app,
        Some(&session),
        "POST",
        "/api/file-manager/disks/P01/write-sessions",
        Some(json!({
            "path":"one.txt","branch_id":namespace
        })),
    )
    .await;
    assert_eq!(wrong.0, StatusCode::CONFLICT);
    assert_eq!(wrong.1["error"]["code"], "FILE_BRANCH_CONFLICT");
    let workspace = success(
        &app,
        &session,
        "POST",
        "/api/file-manager/disks/P01/write-sessions",
        Some(json!({
            "path":"one.txt","branch_id":original["selected_branch_id"]
        })),
    )
    .await;
    assert_eq!(workspace["branch_id"], namespace);
    assert_eq!(workspace["base_head"], tree["commit_id"]);
    success(
        &app,
        &session,
        "POST",
        &format!(
            "/api/workspaces/{}/abort",
            workspace["workspace_id"].as_str().unwrap()
        ),
        Some(json!({})),
    )
    .await;
    let nested = success(
        &app,
        &session,
        "GET",
        "/api/file-manager/disks/P01/entries?path=folder",
        None,
    )
    .await;
    let nested_id =
        nested["directory_branches"].as_array().unwrap()[0]["directory_branch_id"].clone();
    let wrong_scope = send(
        &app,
        Some(&session),
        "PUT",
        &api,
        Some(json!({"directory_branch_id":nested_id,"operation_id":new_id("op")})),
    )
    .await;
    assert_eq!(wrong_scope.1["error"]["code"], "DIRECTORY_BRANCH_SCOPE");

    // Saving another file from a frozen composition must not rewind a
    // shared branch whose newer HEAD belongs to a different directory view.
    manager_upload(&app, &session, "folder/two.txt", b"edited second file").await;
    let a_history = history_page(&app, &session, original["file_id"].as_str().unwrap()).await;
    let shared_a = a_history["branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|branch| branch["branch_id"] == original["selected_branch_id"])
        .unwrap();
    assert_eq!(
        shared_a["version_id"], latest_a,
        "saving a sibling must not rewind shared file branch HEAD"
    );
    success(
        &app,
        &session,
        "PUT",
        &api,
        Some(json!({"directory_branch_id":frozen,"operation_id":new_id("op")})),
    )
    .await;

    let rename_api = format!("/api/repositories/{repository}/directory-branches/{frozen}");
    let renamed = success(
        &app,
        &session,
        "PATCH",
        &rename_api,
        Some(json!({
            "name":"saved view","operation_id":new_id("op")
        })),
    )
    .await;
    assert_eq!(renamed["result"]["directory_branch_id"], frozen);
    assert_eq!(
        success(&app, &session, "GET", &api, None).await["name"],
        "saved view"
    );
    let audit = success(&app, &session, "GET", "/api/file-manager/history", None).await;
    for action in [
        "directory.branches.create",
        "directory.default.select",
        "directory.branch.rename",
    ] {
        assert!(
            audit["events"].as_array().unwrap().iter().any(|event| {
                event["action"] == action
                    && event["directory_branch_id"] == frozen
                    && event["file_id"] == ""
            }),
            "missing directory audit action {action}"
        );
    }
    assert!(
        audit["events"].as_array().unwrap().iter().any(|event| {
            event["action"] == "directory.tree.write"
                && event["directory_branch_id"] == frozen
                && event["head_tree_id"].is_string()
                && event["parent_tree_id"].is_string()
        }),
        "a real save appends an immutable subtree to the existing directory branch"
    );
    let overlong = send(
        &app,
        Some(&session),
        "PATCH",
        &rename_api,
        Some(json!({
            "name":"x".repeat(129),"operation_id":new_id("op")
        })),
    )
    .await;
    assert_eq!(overlong.0, StatusCode::BAD_REQUEST);
    assert_eq!(listing["default_branch_scope"], true);
    assert_eq!(nested["default_branch_scope"], false);

    success(&app, &session, "POST", "/api/auth/logout", None).await;
    assert_eq!(
        send(&app, Some(&session), "GET", &api, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            Some(&session),
            "PUT",
            &api,
            Some(json!({"directory_branch_id":frozen,"operation_id":new_id("op")}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let empty = success(&app, &admin, "POST", "/api/repositories", Some(json!({
        "name":"Empty view","primary_slot_id":"P01","target_copies":3,"operation_id":new_id("op")
    }))).await;
    let empty_id = empty["repository_id"].as_str().unwrap();
    let empty_default = success(
        &app,
        &admin,
        "GET",
        &format!("/api/repositories/{empty_id}/default-branch"),
        None,
    )
    .await;
    assert!(empty_default["directory_branch_id"].is_null());
    assert!(
        empty_default["directory_branches"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    capture_wire("default-directory-branch.json", &current);
    drop(app);
    let recovered = reopen_after_router_drop(&root.path().join("system")).await;
    assert_eq!(
        recovered.state().repositories[&repository].main_branch_id,
        namespace
    );
    assert_eq!(recovered.state().directory_selections[&repository], frozen);
    assert_eq!(
        recovered.state().directory_branches[&frozen].name,
        "saved view"
    );
    let visible = recovered
        .list_tree(&namespace)
        .unwrap()
        .into_iter()
        .find(|e| e.name() == "one.txt")
        .unwrap();
    assert_eq!(visible.version_id, original["version_id"]);
}

/// Capture the same explicit scope, immutable tree, and entry identity used by
/// the file manager batch UI. Destructive tests must not resolve a path at write time.
/// Read one bounded history page and its branch page as the whole-history
/// shape older regressions assert on: nodes in canonical order (one page is
/// enough for these fixtures), branches by id, and the context summary.
async fn history_page(app: &Router, token: &str, file: &str) -> Value {
    history_page_at(
        app,
        token,
        &format!("/api/file-manager/files/{file}/history"),
    )
    .await
}
async fn history_page_at(app: &Router, token: &str, route: &str) -> Value {
    let separator = if route.contains('?') { '&' } else { '?' };
    let mut page = success(
        app,
        token,
        "GET",
        &format!("{route}{separator}limit=200"),
        None,
    )
    .await;
    assert!(
        page["next_cursor"].is_null(),
        "regression fixtures must fit one history page"
    );
    let file = page["file_id"].as_str().unwrap().to_owned();
    let root = page["history_root"].as_str().unwrap().to_owned();
    let branches = success(
        app,
        token,
        "GET",
        &format!("/api/file-manager/files/{file}/history/branches?history_root={root}&limit=500"),
        None,
    )
    .await;
    assert!(branches["next_cursor"].is_null());
    page["branches"] = branches["branches"].clone();
    let merges = success(
        app,
        token,
        "GET",
        &format!("/api/file-manager/files/{file}/history/merges?history_root={root}&limit=200"),
        None,
    )
    .await;
    page["merges"] = merges["merges"].clone();
    page
}
/// Content nodes of a history page in canonical (newest first) order.
fn content_nodes(page: &Value) -> Vec<&Value> {
    page["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["kind"] == "content")
        .collect()
}
/// The node the context marks current, if the file still has an entry.
fn current_node(page: &Value) -> Option<&Value> {
    let current = page["context"]["current_node_id"].as_str()?;
    page["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["node_id"] == current)
}
/// Raw body lineage of one version through the advanced sources route.
async fn version_sources(app: &Router, token: &str, page: &Value, version: &str) -> Value {
    let file = page["file_id"].as_str().unwrap();
    let root = page["history_root"].as_str().unwrap();
    let sources = success(
        app,
        token,
        "GET",
        &format!("/api/files/{file}/history/versions/{version}/sources?history_root={root}"),
        None,
    )
    .await;
    Value::Array(
        sources["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|source| source["version_id"].clone())
            .collect(),
    )
}
/// Graph parents of one node through the parents route (all pages).
async fn node_parents(app: &Router, token: &str, page: &Value, node: &str) -> Value {
    let file = page["file_id"].as_str().unwrap();
    let root = page["history_root"].as_str().unwrap();
    let mut parents = Vec::new();
    let mut route =
        format!("/api/file-manager/files/{file}/history/nodes/{node}/parents?history_root={root}");
    loop {
        let response = success(app, token, "GET", &route, None).await;
        parents.extend(
            response["parents"]
                .as_array()
                .unwrap()
                .iter()
                .map(|parent| parent["node_id"].clone()),
        );
        match response["next_cursor"].as_str() {
            Some(cursor) => {
                route = format!(
                    "/api/file-manager/files/{file}/history/nodes/{node}/parents?cursor={cursor}"
                );
            }
            None => return Value::Array(parents),
        }
    }
}

async fn manager_namespace_request(
    app: &Router,
    token: &str,
    kind: &str,
    path: &str,
    destination: Option<&str>,
) -> Value {
    let parent = path
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("path", parent)
        .finish();
    let listing = success(
        app,
        token,
        "GET",
        &format!("/api/file-manager/disks/P01/entries?{query}"),
        None,
    )
    .await;
    let repository = listing["context"]["repository_id"].as_str().unwrap();
    let scope_query = url_encoding::form::Serializer::new(String::new())
        .append_pair(
            "path",
            listing["context"]["repository_path"].as_str().unwrap(),
        )
        .finish();
    let scopes = success(
        app,
        token,
        "GET",
        &format!("/api/repositories/{repository}/directory-branches?{scope_query}"),
        None,
    )
    .await;
    let scope = scopes["directory_branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|scope| scope["current"] == true)
        .expect("selected directory scope");
    let name = path.rsplit('/').next().unwrap();
    let relative = if scope["path"] == "" {
        name.to_owned()
    } else {
        format!("{}/{name}", scope["path"].as_str().unwrap())
    };
    let mut item = json!({"source":manager_batch_source(scope, &relative)});
    if let Some(destination) = destination {
        assert_eq!(
            destination
                .rsplit_once('/')
                .map(|(parent, _)| parent)
                .unwrap_or(""),
            parent
        );
        let name = destination.rsplit('/').next().unwrap();
        let relative = if scope["path"] == "" {
            name.to_owned()
        } else {
            format!("{}/{name}", scope["path"].as_str().unwrap())
        };
        item["target"] = manager_batch_target(scope, &relative);
    }
    json!({"kind":kind,"operation_id":new_id("op"),"items":[item]})
}

async fn manager_namespace(
    app: &Router,
    token: &str,
    kind: &str,
    path: &str,
    destination: Option<&str>,
) -> Value {
    let request = manager_namespace_request(app, token, kind, path, destination).await;
    success(
        app,
        token,
        "POST",
        "/api/file-manager/disks/P01/namespace-batch",
        Some(request),
    )
    .await
}

#[tokio::test]
async fn destructive_namespace_requires_identity_and_rejects_stale_path_aba() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "report.txt", b"original").await;
    let entries_route = "/api/file-manager/disks/P01/entries";
    let original = success(&app, &token, "GET", entries_route, None).await;
    for kind in ["rename", "delete", "delete_tree"] {
        let mut request = json!({"kind":kind,"path":"report.txt","operation_id":new_id("op")});
        if kind == "rename" {
            request["destination"] = json!("renamed.txt");
        }
        assert_eq!(
            send(
                &app,
                Some(&token),
                "POST",
                "/api/file-manager/disks/P01/namespace",
                Some(request)
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            success(&app, &token, "GET", entries_route, None).await,
            original
        );
    }
    let stale_rename =
        manager_namespace_request(&app, &token, "rename", "report.txt", Some("renamed.txt")).await;
    let stale_delete = manager_namespace_request(&app, &token, "delete", "report.txt", None).await;
    manager_namespace(&app, &token, "delete", "report.txt", None).await;
    manager_upload(&app, &token, "report.txt", b"replacement").await;
    let replacement = success(&app, &token, "GET", entries_route, None).await;
    assert_ne!(
        replacement["entries"][0]["entry_id"],
        original["entries"][0]["entry_id"]
    );
    for request in [stale_rename, stale_delete] {
        let (status, failure) = send(
            &app,
            Some(&token),
            "POST",
            "/api/file-manager/disks/P01/namespace-batch",
            Some(request),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(failure["error"]["code"], "DIRECTORY_BRANCH_CONFLICT");
        assert_eq!(
            success(&app, &token, "GET", entries_route, None).await,
            replacement
        );
    }
    // Even a current tree does not authorize a stale identity at the same path.
    let mut stale_identity =
        manager_namespace_request(&app, &token, "delete", "report.txt", None).await;
    stale_identity["items"][0]["source"]["entry_id"] = original["entries"][0]["entry_id"].clone();
    let (_, failure) = send(
        &app,
        Some(&token),
        "POST",
        "/api/file-manager/disks/P01/namespace-batch",
        Some(stale_identity),
    )
    .await;
    assert_eq!(failure["error"]["code"], "ENTRY_NOT_FOUND");
    assert_eq!(
        success(&app, &token, "GET", entries_route, None).await,
        replacement
    );
    assert_eq!(
        version_bytes(
            &app,
            &token,
            replacement["entries"][0]["version_id"].as_str().unwrap()
        )
        .await,
        b"replacement"
    );
}

async fn manager_directory_branch(app: &Router, token: &str, path: &str, name: &str) -> String {
    success(
        app,
        token,
        "POST",
        "/api/file-manager/disks/P01/directory-branches",
        Some(json!({
            "path":path,"name":name,"operation_id":new_id("op")
        })),
    )
    .await["result"]["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn manager_scope(app: &Router, token: &str, repo: &str, branch: &str) -> Value {
    success(
        app,
        token,
        "GET",
        &format!("/api/repositories/{repo}/directory-branches"),
        None,
    )
    .await["directory_branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|scope| scope["directory_branch_id"] == branch)
        .unwrap()
        .clone()
}

fn manager_batch_source(scope: &Value, path: &str) -> Value {
    let entry = scope["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == path)
        .unwrap();
    json!({"directory_branch_id":scope["directory_branch_id"],"expected_tree_id":scope["head_tree_id"],"path":path,"entry_id":entry["entry_id"]})
}
fn manager_batch_target(scope: &Value, path: &str) -> Value {
    json!({"directory_branch_id":scope["directory_branch_id"],"expected_tree_id":scope["head_tree_id"],"path":path})
}

#[tokio::test]
async fn branch_namespace_batch_preserves_unselected_views_and_is_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "a/report.txt", b"frozen source").await;
    manager_upload(&app, &token, "b/stay.txt", b"target sibling").await;
    let root_view = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let repo = root_view["context"]["repository_id"].as_str().unwrap();
    let a_m = manager_directory_branch(&app, &token, "a", "m").await;
    let a_n = manager_directory_branch(&app, &token, "a", "n").await;
    let a_r = manager_directory_branch(&app, &token, "a", "r").await;
    let b_l = manager_directory_branch(&app, &token, "b", "l").await;
    let b_p = manager_directory_branch(&app, &token, "b", "p").await;
    let b_q = manager_directory_branch(&app, &token, "b", "q").await;
    let m = manager_scope(&app, &token, repo, &a_m).await;
    let n = manager_scope(&app, &token, repo, &a_n).await;
    let r = manager_scope(&app, &token, repo, &a_r).await;
    let l = manager_scope(&app, &token, repo, &b_l).await;
    let p = manager_scope(&app, &token, repo, &b_p).await;
    let q = manager_scope(&app, &token, repo, &b_q).await;
    let original = m["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == "a/report.txt")
        .unwrap()
        .clone();
    let default_before = success(
        &app,
        &token,
        "GET",
        &format!("/api/repositories/{repo}/default-branch"),
        None,
    )
    .await;
    let request = json!({"kind":"move","operation_id":new_id("op"),"items":[
        {"source":manager_batch_source(&m,"a/report.txt"),"target":manager_batch_target(&l,"b/report.txt")},
        {"source":manager_batch_source(&n,"a/report.txt"),"target":manager_batch_target(&p,"b/report.txt")}
    ]});
    let route = "/api/file-manager/disks/P01/namespace-batch";
    let moved = success(&app, &token, "POST", route, Some(request.clone())).await;
    let repeated = success(&app, &token, "POST", route, Some(request.clone())).await;
    assert_eq!(repeated["operation_id"], moved["operation_id"]);
    assert_eq!(
        repeated["result"], moved["result"],
        "retry does not move or append trees twice"
    );
    for id in [&a_m, &a_n] {
        let scope = manager_scope(&app, &token, repo, id).await;
        assert!(
            scope["entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["file_id"] != original["file_id"])
        );
    }
    for id in [&b_l, &b_p] {
        let scope = manager_scope(&app, &token, repo, id).await;
        let moved = scope["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"] == "b/report.txt")
            .unwrap();
        assert_eq!(moved["file_id"], original["file_id"]);
        assert_eq!(moved["version_id"], original["version_id"]);
    }
    assert_eq!(
        manager_scope(&app, &token, repo, &a_r).await["head_tree_id"],
        r["head_tree_id"]
    );
    assert_eq!(
        manager_scope(&app, &token, repo, &b_q).await["head_tree_id"],
        q["head_tree_id"]
    );
    assert_eq!(
        version_bytes(&app, &token, original["version_id"].as_str().unwrap()).await,
        b"frozen source"
    );
    assert_eq!(
        success(
            &app,
            &token,
            "GET",
            &format!("/api/repositories/{repo}/default-branch"),
            None
        )
        .await["directory_branch_id"],
        default_before["directory_branch_id"],
        "batch editing inactive branches does not change SMB default"
    );

    let l = manager_scope(&app, &token, repo, &b_l).await;
    let p = manager_scope(&app, &token, repo, &b_p).await;
    let copied=success(&app,&token,"POST",route,Some(json!({"kind":"copy","operation_id":new_id("op"),"items":[
        {"source":manager_batch_source(&l,"b/report.txt"),"target":manager_batch_target(&l,"b/copy.txt")},
        {"source":manager_batch_source(&p,"b/report.txt"),"target":manager_batch_target(&p,"b/copy.txt")}
    ]}))).await;
    assert_eq!(copied["state"], "completed");
    let l = manager_scope(&app, &token, repo, &b_l).await;
    let p = manager_scope(&app, &token, repo, &b_p).await;
    let copy_l = l["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == "b/copy.txt")
        .unwrap();
    let copy_p = p["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == "b/copy.txt")
        .unwrap();
    assert_ne!(copy_l["file_id"], original["file_id"]);
    assert_eq!(
        copy_l["file_id"], copy_p["file_id"],
        "one copy identity shared by selected target scopes"
    );
    assert_ne!(copy_l["version_id"], original["version_id"]);
    assert_eq!(
        version_bytes(&app, &token, copy_l["version_id"].as_str().unwrap()).await,
        b"frozen source"
    );
    let copy_version = copy_l["version_id"].clone();

    success(&app,&token,"POST",route,Some(json!({"kind":"rename","operation_id":new_id("op"),"items":[
        {"source":manager_batch_source(&l,"b/copy.txt"),"target":manager_batch_target(&l,"b/客户报告.txt")},
        {"source":manager_batch_source(&p,"b/copy.txt"),"target":manager_batch_target(&p,"b/客户报告.txt")}
    ]}))).await;
    let l = manager_scope(&app, &token, repo, &b_l).await;
    let p = manager_scope(&app, &token, repo, &b_p).await;
    assert!(
        l["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["path"] == "b/客户报告.txt" && e["version_id"] == copy_version)
    );
    // Deleting one branch keeps the other branch's same immutable copy readable.
    success(
        &app,
        &token,
        "POST",
        route,
        Some(json!({"kind":"delete","operation_id":new_id("op"),"items":[
            {"source":manager_batch_source(&l,"b/客户报告.txt")}
        ]})),
    )
    .await;
    assert!(
        manager_scope(&app, &token, repo, &b_l).await["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["path"] != "b/客户报告.txt")
    );
    assert_eq!(
        manager_scope(&app, &token, repo, &b_p).await["head_tree_id"],
        p["head_tree_id"]
    );
    assert_eq!(
        version_bytes(&app, &token, copy_version.as_str().unwrap()).await,
        b"frozen source"
    );

    // One stale mapping prevents the entire batch, including the otherwise valid first row.
    let p = manager_scope(&app, &token, repo, &b_p).await;
    let l = manager_scope(&app, &token, repo, &b_l).await;
    let conflict = send(
        &app,
        Some(&token),
        "POST",
        route,
        Some(json!({"kind":"delete","operation_id":new_id("op"),"items":[
            {"source":manager_batch_source(&p,"b/客户报告.txt")},
            {"source":manager_batch_source(&m,"a/report.txt")}
        ]})),
    )
    .await;
    assert_eq!(conflict.0, StatusCode::CONFLICT);
    assert_eq!(
        manager_scope(&app, &token, repo, &b_p).await["head_tree_id"],
        p["head_tree_id"]
    );
    assert_eq!(
        manager_scope(&app, &token, repo, &b_l).await["head_tree_id"],
        l["head_tree_id"]
    );
    assert_eq!(
        success(
            &app,
            &token,
            "GET",
            &format!("/api/repositories/{repo}/directory-branches?path=b"),
            None
        )
        .await["directory_branches"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let readonly = send(
        &app,
        Some(&token),
        "POST",
        "/api/file-manager/disks/B01/namespace-batch",
        Some(request),
    )
    .await;
    assert_eq!(readonly.0, StatusCode::FORBIDDEN);

    let session = login(&app, "admin", "integration-test-password").await;
    let visible = manager_scope(&app, &session, repo, &b_p).await;
    success(&app, &session, "POST", "/api/auth/logout", None).await;
    let denied = send(
        &app,
        Some(&session),
        "POST",
        route,
        Some(json!({"kind":"delete","operation_id":new_id("op"),"items":[
            {"source":manager_batch_source(&visible,"b/客户报告.txt")}
        ]})),
    )
    .await;
    assert_eq!(denied.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        manager_scope(&app, &token, repo, &b_p).await["head_tree_id"],
        visible["head_tree_id"]
    );
    let audit = success(&app, &token, "GET", "/api/file-manager/history", None).await;
    for action in [
        "directory.namespace.move",
        "directory.namespace.copy",
        "directory.namespace.rename",
        "directory.namespace.delete",
    ] {
        assert!(
            audit["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["action"] == action && event["directory_branch_id"].is_string()),
            "missing branch-scoped audit {action}"
        );
    }
    capture_wire("branch-namespace-batch.json", &moved);
}

#[tokio::test]
async fn moved_file_aliases_keep_independent_path_contexts_for_history_and_write() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "a/report.txt", b"original").await;
    manager_upload(&app, &token, "b/stay.txt", b"sibling").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=a",
        None,
    )
    .await;
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let original = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "report.txt")
        .unwrap()
        .clone();
    assert_eq!(original["repository_path"], "a/report.txt");
    let m = manager_directory_branch(&app, &token, "a", "m").await;
    let n = manager_directory_branch(&app, &token, "a", "n").await;
    let l = manager_directory_branch(&app, &token, "b", "l").await;
    let m0 = manager_scope(&app, &token, repo, &m).await;
    let l0 = manager_scope(&app, &token, repo, &l).await;
    success(&app,&token,"POST","/api/file-manager/disks/P01/namespace-batch",Some(json!({"kind":"move","operation_id":new_id("op"),"items":[{
        "source":manager_batch_source(&m0,"a/report.txt"),"target":manager_batch_target(&l0,"b/report.txt")
    }]}))).await;
    for (path, name) in [("a", "n"), ("b", "l")] {
        success(
            &app,
            &token,
            "POST",
            "/api/file-manager/disks/P01/directory-select",
            Some(json!({"path":path,"name":name,"operation_id":new_id("op")})),
        )
        .await;
    }
    let a_before = manager_scope(&app, &token, repo, &n).await;
    let file = original["file_id"].as_str().unwrap();
    let history_route = |path: &str| {
        format!("/api/file-manager/files/{file}/history?repository_id={repo}&path={path}")
    };
    assert_eq!(
        success(&app, &token, "GET", &history_route("a/report.txt"), None).await["context"]["version_id"],
        original["version_id"]
    );
    manager_upload(&app, &token, "b/report.txt", b"written in b").await;
    let a =
        success(&app, &token, "GET", &history_route("a/report.txt"), None).await["context"].clone();
    let b =
        success(&app, &token, "GET", &history_route("b/report.txt"), None).await["context"].clone();
    assert_eq!(a["version_id"], original["version_id"]);
    assert_ne!(b["version_id"], original["version_id"]);
    assert_ne!(a["selected_branch_id"], b["selected_branch_id"]);
    assert_eq!(a["inherited"], true);
    assert_eq!(b["inherited"], false);
    let index = root.path().join("system/index.sqlite");
    let saved_index = root.path().join("system/index.saved");
    std::fs::rename(&index, &saved_index).unwrap();
    std::fs::create_dir(&index).unwrap();
    for (path, expected) in [("a/report.txt", &a), ("b/report.txt", &b)] {
        assert_eq!(
            &success(&app, &token, "GET", &history_route(path), None).await["context"],
            expected,
            "the small-store scan must retain alias selection with an unavailable cache"
        );
    }
    std::fs::remove_dir(&index).unwrap();
    std::fs::rename(&saved_index, &index).unwrap();
    assert_eq!(
        manager_scope(&app, &token, repo, &n).await["head_tree_id"],
        a_before["head_tree_id"],
        "b write must not alter the retained a alias"
    );
    assert_eq!(
        version_bytes(&app, &token, a["version_id"].as_str().unwrap()).await,
        b"original"
    );
    assert_eq!(
        version_bytes(&app, &token, b["version_id"].as_str().unwrap()).await,
        b"written in b"
    );
    success(&app,&token,"POST",&format!("/api/file-manager/files/{file}/select"),Some(json!({
        "repository_id":repo,"path":"a/report.txt","branch_id":b["selected_branch_id"],"operation_id":new_id("op")
    }))).await;
    assert_eq!(
        success(&app, &token, "GET", &history_route("a/report.txt"), None).await["context"]["version_id"],
        b["version_id"]
    );
    assert_eq!(
        success(&app, &token, "GET", &history_route("b/report.txt"), None).await["context"]["version_id"],
        b["version_id"]
    );
    assert_eq!(
        send(
            &app,
            Some(&token),
            "GET",
            &history_route("missing.txt"),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let a_scope = manager_scope(&app, &token, repo, &n).await;
    success(&app,&token,"POST","/api/file-manager/disks/P01/namespace-batch",Some(json!({"kind":"rename","operation_id":new_id("op"),"items":[{
        "source":manager_batch_source(&a_scope,"a/report.txt"),"target":manager_batch_target(&a_scope,"a/archive.txt")
    }]}))).await;
    for (path, name) in [
        ("a/archive.txt", "archive.txt"),
        ("b/report.txt", "report.txt"),
    ] {
        let uri = format!(
            "/api/versions/{}/content?repository_id={repo}&path={path}",
            original["version_id"].as_str().unwrap()
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(&uri)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()["content-disposition"]
                .to_str()
                .unwrap()
                .contains(&format!("filename=\"{name}\""))
        );
        assert_eq!(
            http_routing::body::to_bytes(response.into_body(), 64)
                .await
                .unwrap()
                .as_ref(),
            b"original"
        );
    }
    let mismatched = format!(
        "/api/versions/{}/content?repository_id={repo}&path=b/stay.txt",
        original["version_id"].as_str().unwrap()
    );
    assert_eq!(
        send(&app, Some(&token), "GET", &mismatched, None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn public_file_branch_label_patch_preserves_content_and_directory_nodes() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let admin = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &admin, "report.txt", b"unchanged bytes").await;
    let listing = success(
        &app,
        &admin,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let file = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "report.txt")
        .unwrap();
    let id = file["file_id"].as_str().unwrap();
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let created=success(&app,&admin,"POST",&format!("/api/file-manager/files/{id}/branches"),Some(json!({"source_version_id":file["version_id"],"name":"draft","operation_id":new_id("op")}))).await;
    let branch = created["result"]["branch_id"].as_str().unwrap();
    let route = format!("/api/file-manager/files/{id}/branches/{branch}");
    let scopes = success(
        &app,
        &admin,
        "GET",
        &format!("/api/repositories/{repo}/directory-branches"),
        None,
    )
    .await;
    let before = history_page(&app, &admin, id).await;
    let body = json!({"name":"客户主稿","operation_id":new_id("op")});
    let result = success(&app, &admin, "PATCH", &route, Some(body.clone())).await;
    assert_eq!(result["result"]["name"], "客户主稿");
    assert_eq!(result["operation_kind"], "file.branch.rename");
    let digest=storage_contract::object_hash("request",&json!({"kind":"file.branch.rename","principal":"admin","body":{"file_id":id,"branch_id":branch,"name":"客户主稿"}})).unwrap();
    assert_eq!(result["request_digest"], digest);
    let mut conflicting = body.clone();
    conflicting["name"] = "Other".into();
    let (status, conflict) = send(&app, Some(&admin), "PATCH", &route, Some(conflicting)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["error"]["code"], "IDEMPOTENCY_KEY_REUSED");

    assert_eq!(
        success(&app, &admin, "PATCH", &route, Some(body.clone())).await["result"],
        result["result"]
    );
    let after = history_page(&app, &admin, id).await;
    assert_eq!(after["nodes"], before["nodes"]);
    assert_eq!(
        after["context"]["selected_branch_id"],
        before["context"]["selected_branch_id"]
    );
    assert_ne!(
        after["history_root"], before["history_root"],
        "a label change is a new root"
    );
    assert!(
        after["branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["branch_id"] == branch && item["name"] == "客户主稿")
    );
    assert_eq!(
        success(
            &app,
            &admin,
            "GET",
            &format!("/api/repositories/{repo}/directory-branches"),
            None
        )
        .await,
        scopes
    );
    assert_eq!(
        version_bytes(&app, &admin, file["version_id"].as_str().unwrap()).await,
        b"unchanged bytes"
    );
    let collision = send(
        &app,
        Some(&admin),
        "PATCH",
        &route,
        Some(json!({"name":"MAIN","operation_id":new_id("op")})),
    )
    .await;
    assert_eq!(collision.0, StatusCode::CONFLICT);
    assert_eq!(collision.1["error"]["code"], "NAME_COLLISION");
    assert_eq!(
        send(
            &app,
            Some(&admin),
            "PATCH",
            &route,
            Some(json!({"name":"文".repeat(43),"operation_id":new_id("op")}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    for invalid in [
        json!({"name":"x".repeat(129),"operation_id":new_id("op")}),
        json!({"name":"bad\nname","operation_id":new_id("op")}),
        json!({"name":"Cafe\u{301}","operation_id":new_id("op")}),
        json!({"name":"Side","operation_id":"bad"}),
        json!({"name":"Side","operation_id":new_id("op"),"unknown":0}),
        json!({"name":"Side","operation_id":new_id("op"),"file_id":"other"}),
        json!({"name":"Side","operation_id":new_id("op"),"branch_id":"other"}),
        json!({"operation_id":new_id("op")}),
    ] {
        assert_eq!(
            send(&app, Some(&admin), "PATCH", &route, Some(invalid))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(history_page(&app, &admin, id).await, after);
    }
    assert_eq!(
        send(&app, None, "PATCH", &route, Some(json!({"name":null})))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );

    let revoked = revoked_session(&app).await;
    assert_eq!(
        send(&app, Some(&revoked), "PATCH", &route, Some(body.clone()))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let audit = success(&app, &admin, "GET", "/api/file-manager/history", None).await;
    assert!(
        audit["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["action"] == "file.branch.rename"
                && event["branch_id"] == branch
                && event["message"] == "客户主稿")
    );
    let unchanged = success(
        &app,
        &admin,
        "PATCH",
        &route,
        Some(json!({"name":"客户主稿","operation_id":new_id("op")})),
    )
    .await;
    assert_eq!(unchanged["result"]["no_change"], true);
    success(
        &app,
        &admin,
        "PATCH",
        &route,
        Some(json!({"name":"New label","operation_id":new_id("op")})),
    )
    .await;
    drop(app);
    let system = root.path().join("system");
    let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
    let auth = backend.open_auth(&system).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    let admin = login(&app, "admin", "integration-test-password").await;
    let replay = success(&app, &admin, "PATCH", &route, Some(body)).await;
    for key in [
        "operation_id",
        "operation_kind",
        "request_digest",
        "publication_sequence",
        "result",
        "protection_at_publish",
    ] {
        assert_eq!(replay[key], result[key], "{key}");
    }
    let current = history_page(&app, &admin, id).await;
    assert!(
        current["branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["branch_id"] == branch && item["name"] == "New label")
    );
}

#[tokio::test]
async fn rename_nodes_reuse_bodies_and_preserve_graph_parents_notes_and_branch_sources() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let admin = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &admin, "documents/one.txt", b"original one").await;
    manager_upload(&app, &admin, "documents/nested/two.txt", b"original two").await;
    let listing = success(
        &app,
        &admin,
        "GET",
        "/api/file-manager/disks/P01/entries?path=documents",
        None,
    )
    .await;
    let one = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "one.txt")
        .unwrap();
    let file = one["file_id"].as_str().unwrap();
    let body = one["version_id"].as_str().unwrap();
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let history_route = format!("/api/file-manager/files/{file}/history");
    let before = history_page(&app, &admin, file).await;
    assert_eq!(before["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(before["context"]["current_node_id"], body);
    manager_namespace(&app, &admin, "rename", "documents", Some("archive")).await;
    let after = history_page_at(
        &app,
        &admin,
        &format!("{history_route}?repository_id={repo}&path=archive/one.txt"),
    )
    .await;
    assert_eq!(
        content_nodes(&after)
            .iter()
            .map(|node| node["version_id"].clone())
            .collect::<Vec<_>>(),
        content_nodes(&before)
            .iter()
            .map(|node| node["version_id"].clone())
            .collect::<Vec<_>>(),
        "renaming a folder reuses the real content body"
    );
    assert_eq!(after["nodes"].as_array().unwrap().len(), 2);
    let path_node = after["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["kind"] == "rename")
        .unwrap()
        .clone();
    let node = path_node["node_id"].as_str().unwrap();
    assert!(node.starts_with("fnode_"));
    assert_eq!(path_node["version_id"], body);
    assert_eq!(path_node["parents"]["status"], "complete");
    assert_eq!(
        path_node["parents"]["edges"],
        json!([{"node_id":body,"in_window":true}])
    );
    assert_eq!(
        node_parents(&app, &admin, &after, node).await,
        json!([body])
    );
    assert_eq!(path_node["path"], "archive/one.txt");
    assert_eq!(path_node["previous_path"], "documents/one.txt");
    assert_eq!(after["context"]["current_node_id"], node);
    let nested = success(
        &app,
        &admin,
        "GET",
        "/api/file-manager/disks/P01/entries?path=archive/nested",
        None,
    )
    .await;
    let two = &nested["entries"][0];
    let two_file = two["file_id"].as_str().unwrap();
    let two_node = two["node_id"].as_str().unwrap();
    assert!(two_node.starts_with("fnode_"));
    assert_ne!(two_node, node);
    let two_history = history_page(&app, &admin, two_file).await;
    assert_eq!(content_nodes(&two_history).len(), 1);
    assert_eq!(two_history["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(version_bytes(&app, &admin, body).await, b"original one");
    let scopes = success(
        &app,
        &admin,
        "GET",
        &format!("/api/repositories/{repo}/directory-branches"),
        None,
    )
    .await;
    let note_route = format!("/api/file-manager/files/{file}/nodes/{node}");
    success(
        &app,
        &admin,
        "PATCH",
        &note_route,
        Some(json!({"note":"路径改名备注","operation_id":new_id("op")})),
    )
    .await;
    success(
        &app,
        &admin,
        "POST",
        &format!("/api/file-manager/versions/{body}/metadata"),
        Some(json!({"note":"正文备注","operation_id":new_id("op")})),
    )
    .await;
    let noted = history_page(&app, &admin, file).await;
    assert_eq!(
        noted["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["node_id"] == node)
            .unwrap()["note"],
        "路径改名备注"
    );
    assert_eq!(
        noted["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["node_id"] == body)
            .unwrap()["note"],
        "正文备注"
    );
    assert_eq!(
        success(
            &app,
            &admin,
            "GET",
            &format!("/api/repositories/{repo}/directory-branches"),
            None
        )
        .await,
        scopes,
        "notes add no directory subtree"
    );
    for invalid in ["x".repeat(129), "文".repeat(43), "bad\nline".into()] {
        assert_eq!(
            send(
                &app,
                Some(&admin),
                "PATCH",
                &note_route,
                Some(json!({"note":invalid,"operation_id":new_id("op")}))
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let wrong_note = format!("/api/file-manager/files/{file}/nodes/{two_node}");
    assert_eq!(
        send(
            &app,
            Some(&admin),
            "PATCH",
            &wrong_note,
            Some(json!({"note":"wrong file","operation_id":new_id("op")}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let branch_route = format!("/api/file-manager/files/{file}/branches");
    assert_eq!(send(&app,Some(&admin),"POST",&branch_route,Some(json!({"source_version_id":body,"source_node_id":two_node,"name":"foreign","operation_id":new_id("op")}))).await.0,StatusCode::NOT_FOUND);
    let branch=success(&app,&admin,"POST",&branch_route,Some(json!({"source_version_id":body,"source_node_id":node,"name":"from-path","operation_id":new_id("op")}))).await;
    let branch_id = branch["result"]["branch_id"].as_str().unwrap();
    let registered = history_page(&app, &admin, file).await;
    assert_eq!(registered["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(content_nodes(&registered).len(), 1);
    assert!(
        registered["branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|branch| branch["branch_id"] == branch_id && branch["file_node_id"] == node)
    );
    success(&app,&admin,"POST",&format!("/api/file-manager/files/{file}/select"),Some(json!({"repository_id":repo,"path":"archive/one.txt","branch_id":branch_id,"operation_id":new_id("op")}))).await;
    assert_eq!(
        history_page(&app, &admin, file).await["context"]["current_node_id"],
        node
    );
    manager_upload(&app, &admin, "archive/one.txt", b"new body after path node").await;
    let written = history_page(&app, &admin, file).await;
    assert_eq!(content_nodes(&written).len(), 2);
    assert_eq!(written["nodes"].as_array().unwrap().len(), 3);
    let current_body = written["context"]["version_id"].as_str().unwrap();
    assert_ne!(current_body, body);
    assert_eq!(written["context"]["current_node_id"], current_body);
    let content = current_node(&written).unwrap();
    assert_eq!(content["kind"], "content");
    assert_eq!(
        content["parents"]["edges"],
        json!([{"node_id":node,"in_window":true}])
    );
    assert_eq!(
        node_parents(&app, &admin, &written, current_body).await,
        json!([node])
    );
    assert_eq!(
        version_sources(&app, &admin, &written, current_body).await,
        json!([body]),
        "real body provenance remains separate from the file-node graph"
    );
    assert_eq!(written["nodes"][0]["node_id"], current_body);
    assert_eq!(version_bytes(&app, &admin, body).await, b"original one");
    assert_eq!(
        version_bytes(&app, &admin, current_body).await,
        b"new body after path node"
    );
    success(&app,&admin,"POST",&branch_route,Some(json!({"source_version_id":current_body,"source_node_id":current_body,"name":"content-source","operation_id":new_id("op")}))).await;
    assert_eq!(send(&app,Some(&admin),"POST",&branch_route,Some(json!({"source_version_id":current_body,"source_node_id":node,"name":"mismatched-body","operation_id":new_id("op")}))).await.0,StatusCode::NOT_FOUND);

    let session = login(&app, "admin", "integration-test-password").await;
    let revoked = revoked_session(&app).await;
    assert_eq!(
        send(
            &app,
            Some(&revoked),
            "PATCH",
            &note_route,
            Some(json!({"note":"denied","operation_id":new_id("op")}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        history_page(&app, &session, file).await["nodes"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let audit = success(&app, &admin, "GET", "/api/file-manager/history", None).await;
    for action in ["file.path.rename", "file.path.metadata"] {
        assert!(
            audit["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["action"] == action && event["node_id"] == node)
        );
    }
    capture_wire("file-path-history.json", &written);
}

#[tokio::test]
async fn engine_upload_generation_conflicts_reach_http_as_conflicts() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let backend = Arc::new(EngineBackend::new(
        Engine::initialize(&root.path().join("system"), config).unwrap(),
    ));
    let auth = backend
        .protect_auth(
            AuthStore::initialize(
                &root.path().join("system"),
                "admin",
                "integration-test-password",
            )
            .unwrap(),
        )
        .unwrap();
    let app = storage_server::router(
        backend.clone(),
        auth,
        ServerConfig::loopback(&root.path().join("system")),
    )
    .unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let repo = success(
        &app,
        &token,
        "POST",
        "/api/repositories",
        Some(json!({
            "name":"Cross-entry uploads", "primary_slot_id":"P01", "target_copies":1,
            "operation_id":new_id("op")
        })),
    )
    .await;
    let workspace = success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({
            "branch_id":repo["main_branch_id"]
        })),
    )
    .await;
    let ws = workspace["workspace_id"].as_str().unwrap();
    let item = new_id("entry");
    let source = root.path().join("source");
    std::fs::write(&source, b"original").unwrap();
    // A different adapter can stage directly in the engine without HTTP spool
    // metadata. The engine's conflict must keep the same public HTTP semantics.
    backend
        .upload_generation(
            &Identity {
                admission_session: None,
                smb_binding: None,
                principal: "admin".into(),
            },
            ws,
            &item,
            &name_policy::PortablePath::parse("file.txt").unwrap(),
            &storage_server::UploadSource::from_file(
                std::fs::File::open(&source).unwrap(),
                u64::MAX,
            )
            .unwrap(),
            2,
        )
        .unwrap();
    for generation in [1, 2] {
        let bytes = b"different";
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!(
                        "/api/workspaces/{ws}/files/{item}/content?path=file.txt"
                    ))
                    .header("authorization", format!("Bearer {token}"))
                    .header("x-zxs-logical-size", bytes.len().to_string())
                    .header("x-zxs-content-hash", blake3::hash(bytes).to_hex().as_str())
                    .header("x-zxs-upload-generation", generation.to_string())
                    .body(Body::from(bytes.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::CONFLICT,
            "generation {generation}"
        );
        let body: Value = json_codec::from_slice(
            &http_routing::body::to_bytes(response.into_body(), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["error"]["code"], "UPLOAD_GENERATION_CONFLICT");
    }
}

#[tokio::test]
async fn interrupted_new_upload_generation_reports_its_own_durable_progress() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let repo = success(
        &app,
        &token,
        "POST",
        "/api/repositories",
        Some(json!({
            "name":"Upload generations", "primary_slot_id":"P01", "target_copies":3,
            "operation_id":new_id("op")
        })),
    )
    .await;
    let workspace = success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({
            "branch_id":repo["main_branch_id"]
        })),
    )
    .await;
    let ws = workspace["workspace_id"].as_str().unwrap();
    let item = new_id("entry");
    upload(&app, &token, ws, &item, "file.txt", b"original").await;
    let bytes = b"replacement bytes";
    for complete in [false, true] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!(
                        "/api/workspaces/{ws}/files/{item}/content?path=file.txt"
                    ))
                    .header("authorization", format!("Bearer {token}"))
                    .header("x-zxs-logical-size", bytes.len().to_string())
                    .header("x-zxs-content-hash", blake3::hash(bytes).to_hex().as_str())
                    .header("x-zxs-upload-generation", "2")
                    .body(Body::from(if complete {
                        bytes.to_vec()
                    } else {
                        bytes[..5].to_vec()
                    }))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if complete {
                StatusCode::OK
            } else {
                StatusCode::BAD_REQUEST
            }
        );
        let status = success(
            &app,
            &token,
            "GET",
            &format!("/api/workspaces/{ws}/files/{item}/upload-status"),
            None,
        )
        .await;
        assert_eq!(status["upload_generation"], "2");
        assert_eq!(
            status["state"],
            if complete { "COMPLETE" } else { "RECEIVING" }
        );
        assert_eq!(status["logical_size"], bytes.len().to_string());
        assert_eq!(
            status["content_hash"],
            blake3::hash(bytes).to_hex().as_str()
        );
        assert_eq!(
            status["durable_ranges"][0]["length"],
            if complete {
                bytes.len().to_string()
            } else {
                "5".into()
            }
        );
    }
    success(
        &app,
        &token,
        "POST",
        &format!("/api/workspaces/{ws}/seal"),
        Some(json!({})),
    )
    .await;
    let status = success(
        &app,
        &token,
        "GET",
        &format!("/api/workspaces/{ws}/files/{item}/upload-status"),
        None,
    )
    .await;
    assert_eq!(status["upload_generation"], "2");
    assert_eq!(status["state"], "SEALED");
    assert!(status["sealed_manifest_hash"].is_string());
}

#[tokio::test]
async fn directory_scope_initialization_accepts_root_but_requires_a_string_path() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let repo = success(
        &app,
        &token,
        "POST",
        "/api/repositories",
        Some(json!({
            "name":"Root scope", "primary_slot_id":"P01", "target_copies":3,
            "operation_id":new_id("op")
        })),
    )
    .await;
    let endpoint = format!(
        "/api/repositories/{}/directory-scopes",
        repo["repository_id"].as_str().unwrap()
    );
    for path in [
        None,
        Some(Value::Null),
        Some(json!(12)),
        Some(json!("../outside")),
    ] {
        let mut body = json!({"operation_id":new_id("op")});
        if let Some(path) = path {
            body["path"] = path;
        }
        assert_eq!(
            send(&app, Some(&token), "POST", &endpoint, Some(body))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let operation = storage_contract::OperationId::generate();
    let initialized = success(
        &app,
        &token,
        "POST",
        &endpoint,
        Some(json!({"path":"", "operation_id":operation})),
    )
    .await;
    assert_eq!(initialized["state"], "completed");
    assert!(initialized["result"]["directory_branch_id"].is_string());
    assert_eq!(initialized["operation_kind"], "directory.scope.initialize");
    assert!(initialized["publication_sequence"].is_string());
    assert!(initialized.get("current_health").is_none());
    let replay = success(
        &app,
        &token,
        "POST",
        &endpoint,
        Some(json!({"path":"", "operation_id":operation})),
    )
    .await;
    for key in [
        "operation_id",
        "request_digest",
        "result",
        "commit_state",
        "protection_at_publish",
        "publication_sequence",
    ] {
        assert_eq!(initialized[key], replay[key], "{key}");
    }
    let (status, changed) = send(
        &app,
        Some(&token),
        "POST",
        &endpoint,
        Some(json!({"path":"missing", "operation_id":operation})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(changed["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
    let (status, unauthenticated) = send(
        &app,
        None,
        "POST",
        &endpoint,
        Some(json!({"path":17, "operation_id":operation})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(unauthenticated["error"]["code"], "UNAUTHORIZED");
}

#[tokio::test]
async fn workspace_capacity_is_live_for_the_single_administrator() {
    let root = tempfile::tempdir().unwrap();
    let mut config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    config.max_retained_workspaces = 1;
    config.max_retained_workspaces_per_principal = 1;
    let app = configured_application(root.path(), config);
    let path = "/api/workspaces/capacity";
    assert_eq!(
        send(&app, None, "GET", path, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let admin = login(&app, "admin", "integration-test-password").await;
    let before = success(&app, &admin, "GET", path, None).await;
    assert_eq!(before["global"]["records"]["used"], "0");
    assert_eq!(before["principal"]["records"]["used"], "0");
    assert_eq!(before["warning"], "ok");
    let repo = success(&app, &admin, "POST", "/api/repositories", Some(json!({"name":"Capacity","primary_slot_id":"P01","target_copies":1,"operation_id":new_id("op")}))).await;
    success(
        &app,
        &admin,
        "POST",
        "/api/workspaces",
        Some(json!({"branch_id":repo["main_branch_id"]})),
    )
    .await;
    let current = success(&app, &admin, "GET", path, None).await;
    assert_eq!(current["global"]["records"]["used"], "1");
    assert_eq!(current["principal"]["records"]["used"], "1");
    assert_eq!(current["warning"], "exhausted");
    assert_eq!(current["automatic_retirement"], true);
    assert!(json_codec::from_value::<storage_contract::WorkspaceCapacity>(current.clone()).is_ok());
    // Arbitrary query selectors cannot create a second identity or hide global capacity.
    assert_eq!(
        success(
            &app,
            &admin,
            "GET",
            "/api/workspaces/capacity?admin=false&principal=other",
            None
        )
        .await,
        current
    );
    let second = login(&app, "admin", "integration-test-password").await;
    assert_eq!(success(&app, &second, "GET", path, None).await, current);
    success(&app, &second, "POST", "/api/auth/logout", None).await;
    assert_eq!(
        send(&app, Some(&second), "GET", path, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(success(&app, &admin, "GET", path, None).await, current);
    success(&app, &admin, "POST", "/api/auth/password", Some(json!({"current_password":"integration-test-password","new_password":"rotated-integration-password"}))).await;
    assert_eq!(
        send(&app, Some(&admin), "GET", path, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let fresh = login(&app, "admin", "rotated-integration-password").await;
    assert_eq!(success(&app, &fresh, "GET", path, None).await, current);
}

#[tokio::test]
async fn removed_account_and_grant_routes_cannot_create_a_second_identity() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let repo = success(&app, &token, "POST", "/api/repositories", Some(json!({"name":"Single identity","primary_slot_id":"P01","target_copies":1,"operation_id":new_id("op")}))).await;
    for admin in [false, true] {
        assert_eq!(send(&app, Some(&token), "POST", "/api/admin/accounts", Some(json!({"username":"second-account","password":"second-account-password","admin":admin,"repositories":[],"disabled":false}))).await.0, StatusCode::NOT_FOUND);
    }
    assert_eq!(
        send(&app, Some(&token), "GET", "/api/admin/accounts", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(send(&app, Some(&token), "PUT", &format!("/api/repositories/{}/grants", repo["repository_id"].as_str().unwrap()), Some(json!({"grants":{"read":["second-account"],"write":["second-account"],"manage":["second-account"]},"operation_id":new_id("op")}))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        send(
            &app,
            None,
            "POST",
            "/api/auth/login",
            Some(json!({"username":"second-account","password":"second-account-password"}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let me = success(&app, &token, "GET", "/api/auth/me", None).await;
    assert_eq!(me["principal"], "admin");
    assert!(me.get("admin").is_none());
    assert!(me.get("repositories").is_none());
    let repositories = success(&app, &token, "GET", "/api/repositories", None).await;
    assert_eq!(repositories["repositories"].as_array().unwrap().len(), 1);
    assert!(repositories["repositories"][0].get("grants").is_none());
    assert!(
        repositories["repositories"][0]
            .get("account_scope_repository_id")
            .is_none()
    );
}

fn manager_disk_files(root: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn visit(path: &Path, output: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(&entry.path(), output);
            } else {
                output.insert(entry.path(), std::fs::read(entry.path()).unwrap());
            }
        }
    }
    let mut output = std::collections::BTreeMap::new();
    for disk in ["d1", "d2", "d3"] {
        visit(&root.join(disk), &mut output);
    }
    output
}

#[tokio::test]
async fn typed_disk_directory_rejects_raw_errors_before_lazy_root_and_keeps_replay() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let before = manager_disk_files(root.path());
    let operation = storage_contract::OperationId::generate();
    for route in ["directory-branches", "directory-select"] {
        let endpoint = format!("/api/file-manager/disks/P01/{route}");
        for body in [
            json!({"path":"","operation_id":operation}),
            json!({"path":"","name":"branch","operation_id":"bad"}),
            json!({"path":"","name":"x".repeat(129),"operation_id":operation}),
            json!({"path":"","name":null,"operation_id":operation}),
            json!({"path":123,"name":"branch","operation_id":operation}),
            json!({"path":"../outside","name":"branch","operation_id":operation}),
            json!({"name":"branch","operation_id":operation,"extra":true}),
        ] {
            let (status, response) = send(&app, Some(&token), "POST", &endpoint, Some(body)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
            assert!(
                success(&app, &token, "GET", "/api/repositories", None).await["repositories"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(manager_disk_files(root.path()), before);
        }
    }
    for name in ["main", "MAIN"] {
        let (status, response) = send(
            &app,
            Some(&token),
            "POST",
            "/api/file-manager/disks/P01/directory-branches",
            Some(json!({"name":name,"operation_id":operation})),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(response["error"]["code"], "NAME_COLLISION");
        assert_eq!(manager_disk_files(root.path()), before);
    }
    let created = success(
        &app,
        &token,
        "POST",
        "/api/file-manager/disks/P01/directory-branches",
        Some(json!({"name":"side","operation_id":operation})),
    )
    .await;
    assert_eq!(created["operation_kind"], "directory.branches.create");
    let repositories = success(&app, &token, "GET", "/api/repositories", None).await;
    let repo = &repositories["repositories"][0]["repository_id"];
    let digest=storage_contract::object_hash("request",&json!({"kind":"directory.branches.create","principal":"admin","body":{"repository_id":repo,"path":"","name":"side"}})).unwrap();
    assert_eq!(created["request_digest"], digest);
    assert_eq!(created["result"]["path"], "");
    let replay = success(
        &app,
        &token,
        "POST",
        "/api/file-manager/disks/P01/directory-branches",
        Some(json!({"path":"","name":"side","operation_id":operation})),
    )
    .await;
    for key in [
        "result",
        "publication_sequence",
        "request_digest",
        "operation_id",
    ] {
        assert_eq!(created[key], replay[key], "{key}");
    }
    let selected = success(
        &app,
        &token,
        "POST",
        "/api/file-manager/disks/P01/directory-select",
        Some(json!({"name":"SIDE","operation_id":storage_contract::OperationId::generate()})),
    )
    .await;
    assert_eq!(
        selected["result"]["directory_branch_id"],
        created["result"]["directory_branch_id"]
    );
    let (status, response) = send(
        &app,
        None,
        "POST",
        "/api/file-manager/disks/P01/directory-select",
        Some(json!({"name":null})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(response["error"]["code"], "UNAUTHORIZED");
}

#[tokio::test]
async fn typed_file_branch_sources_keep_distinct_permanent_receipts() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "report.txt", b"frozen source").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let entry = &listing["entries"][0];
    let file = entry["file_id"].as_str().unwrap();
    let version = entry["version_id"].as_str().unwrap();
    let route = format!("/api/file-manager/files/{file}/branches");
    let before = success(
        &app,
        &token,
        "GET",
        &format!("/api/file-manager/files/{file}/history"),
        None,
    )
    .await;
    for node in [Value::Null, json!(""), json!(false), json!([])] {
        assert_eq!(send(&app,Some(&token),"POST",&route,Some(json!({"source_version_id":version,"source_node_id":node,"name":"Side","operation_id":new_id("op")}))).await.0,StatusCode::BAD_REQUEST);
        assert_eq!(
            success(
                &app,
                &token,
                "GET",
                &format!("/api/file-manager/files/{file}/history"),
                None
            )
            .await,
            before
        );
    }
    let mut receipts = Vec::new();
    for (name, node) in [("Version source", None), ("Node source", Some(version))] {
        let mut body = json!({"source_version_id":version,"name":name,"operation_id":new_id("op")});
        let mut canonical = json!({"file_id":file,"source_version_id":version,"name":name});
        if let Some(node) = node {
            body["source_node_id"] = node.into();
            canonical["source_node_id"] = node.into();
        }
        let created = success(&app, &token, "POST", &route, Some(body.clone())).await;
        assert_eq!(
            created["request_digest"],
            storage_contract::object_hash(
                "request",
                &json!({"kind":"file.branch.create","principal":"admin","body":canonical})
            )
            .unwrap()
        );
        assert_eq!(created["result"]["content_state"], "PENDING");
        assert_eq!(created["result"]["materialized"], false);
        assert_eq!(created["result"]["selected"], false);
        let mut changed = body.clone();
        if node.is_some() {
            changed.as_object_mut().unwrap().remove("source_node_id");
        } else {
            changed["source_node_id"] = version.into();
        }
        let (status, error) = send(&app, Some(&token), "POST", &route, Some(changed)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
        receipts.push((body, created));
    }
    let revoked = revoked_session(&app).await;
    assert_eq!(
        send(
            &app,
            Some(&revoked),
            "POST",
            &route,
            Some(receipts[0].0.clone())
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(version_bytes(&app, &token, version).await, b"frozen source");
    drop(app);
    let system = root.path().join("system");
    let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
    let auth = backend.open_auth(&system).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    for (body, created) in receipts {
        let replay = success(&app, &token, "POST", &route, Some(body)).await;
        for key in [
            "operation_id",
            "request_digest",
            "publication_sequence",
            "result",
        ] {
            assert_eq!(replay[key], created[key], "{key}");
        }
    }
}

#[tokio::test]
async fn typed_notes_reject_absence_and_null_but_clear_and_replay_each_real_map() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "report.txt", b"body").await;
    manager_namespace(&app, &token, "rename", "report.txt", Some("renamed.txt")).await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let file = listing["entries"][0]["file_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let version = listing["entries"][0]["version_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let history_route = format!("/api/file-manager/files/{file}/history");
    let history = success(&app, &token, "GET", &history_route, None).await;
    let node = history["context"]["current_node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(node.starts_with("fnode_"));
    let mut receipts = Vec::new();
    for (method, route, kind, id_key, canonical) in [
        (
            "POST",
            format!("/api/file-manager/files/{file}/metadata"),
            "file.metadata",
            "file_id",
            json!({"file_id":file,"version_id":null,"note":""}),
        ),
        (
            "POST",
            format!("/api/file-manager/versions/{version}/metadata"),
            "version.metadata",
            "version_id",
            json!({"file_id":file,"version_id":version,"note":""}),
        ),
        (
            "PATCH",
            format!("/api/file-manager/files/{file}/nodes/{node}"),
            "file.path.metadata",
            "node_id",
            json!({"file_id":file,"node_id":node,"note":""}),
        ),
    ] {
        let note = "  Cafe\u{301}  ";
        let written = success(
            &app,
            &token,
            method,
            &route,
            Some(json!({"note":note,"operation_id":new_id("op")})),
        )
        .await;
        assert_eq!(written["result"]["note"], note);
        let before = success(&app, &token, "GET", &history_route, None).await;
        let mut mismatch = json!({"note":"","operation_id":new_id("op")});
        mismatch[id_key] = "other".into();
        for invalid in [
            json!({"operation_id":new_id("op")}),
            json!({"note":null,"operation_id":new_id("op")}),
            json!({"note":false,"operation_id":new_id("op")}),
            json!({"note":"x".repeat(129),"operation_id":new_id("op")}),
            json!({"note":"bad\ntext","operation_id":new_id("op")}),
            json!({"note":"","operation_id":"bad"}),
            json!({"note":"","operation_id":new_id("op"),"unknown":0}),
            mismatch,
        ] {
            assert_eq!(
                send(&app, Some(&token), method, &route, Some(invalid))
                    .await
                    .0,
                StatusCode::BAD_REQUEST
            );
            assert_eq!(
                success(&app, &token, "GET", &history_route, None).await,
                before
            );
        }
        assert_eq!(
            send(&app, None, method, &route, Some(json!({"note":null})))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let clear = json!({"note":"","operation_id":new_id("op")});
        let cleared = success(&app, &token, method, &route, Some(clear.clone())).await;
        assert_eq!(cleared["result"]["note"], "");
        assert_eq!(
            cleared["request_digest"],
            storage_contract::object_hash(
                "request",
                &json!({"kind":kind,"principal":"admin","body":canonical})
            )
            .unwrap()
        );
        let mut conflict = clear.clone();
        conflict["note"] = "other".into();
        let (status, error) = send(&app, Some(&token), method, &route, Some(conflict)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
        receipts.push((method, route, clear, cleared));
    }
    drop(app);
    let system = root.path().join("system");
    let engine = reopen_after_router_drop(&system).await;
    assert!(!engine.state().file_notes.contains_key(&file));
    assert!(!engine.state().version_notes.contains_key(&version));
    assert!(!engine.state().file_path_notes.contains_key(&node));
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend.open_auth(&system).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let revoked = revoked_session(&app).await;
    for (method, route, clear, original) in receipts {
        success(
            &app,
            &token,
            method,
            &route,
            Some(json!({"note":"later","operation_id":new_id("op")})),
        )
        .await;
        assert_eq!(
            send(&app, Some(&revoked), method, &route, Some(clear.clone()))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let replay = success(&app, &token, method, &route, Some(clear)).await;
        for key in [
            "operation_id",
            "operation_kind",
            "request_digest",
            "publication_sequence",
            "result",
        ] {
            assert_eq!(replay[key], original[key], "{key}");
        }
    }
    assert_eq!(version_bytes(&app, &token, &version).await, b"body");
    drop(app);
    let engine = reopen_after_router_drop(&system).await;
    assert_eq!(engine.state().file_notes[&file], "later");
    assert_eq!(engine.state().version_notes[&version], "later");
    assert_eq!(engine.state().file_path_notes[&node], "later");
}

#[tokio::test]
async fn disk_mkdir_rejects_raw_mutations_before_any_lazy_root_or_disk_write() {
    for existing in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let app = application(root.path());
        let token = login(&app, "admin", "integration-test-password").await;
        if existing {
            success(&app,&token,"POST","/api/repositories",Some(json!({"name":"Existing","primary_slot_id":"P01","target_copies":1,"operation_id":new_id("op")}))).await;
            success(
                &app,
                &token,
                "POST",
                "/api/file-manager/disks/P01/namespace",
                Some(json!({"kind":"mkdir","path":"Existing/folder","operation_id":new_id("op")})),
            )
            .await;
        }
        let path = if existing {
            "Existing/folder"
        } else {
            "folder"
        };
        let before = success(&app, &token, "GET", "/api/repositories", None).await;
        let events = success(&app, &token, "GET", "/api/events", None).await;
        let bytes = manager_disk_files(root.path());
        let route = "/api/file-manager/disks/P01/namespace";
        let operation = new_id("op");
        for body in [
            json!({"kind":"unknown","path":path,"operation_id":operation}),
            json!({"kind":"mkdir","path":path}),
            json!({"kind":"mkdir","path":path,"operation_id":"bad"}),
            json!({"kind":"mkdir","path":null,"operation_id":operation}),
            json!({"kind":"mkdir","path":123,"operation_id":operation}),
            json!({"kind":"mkdir","path":"","operation_id":operation}),
            json!({"kind":"mkdir","operation_id":operation}),
            json!({"kind":"mkdir","path":"../escape","operation_id":operation}),
            json!({"kind":"mkdir","path":"Cafe\u{301}","operation_id":operation}),
            json!({"kind":"mkdir","path":path,"operation_id":operation,"unknown":true}),
            json!({"kind":"rename","path":path,"destination":123,"operation_id":operation}),
            json!({"kind":"rename","path":path,"operation_id":operation}),
            json!({"kind":"rename","path":path,"destination":"../escape","operation_id":operation}),
            json!({"kind":"delete","path":path,"operation_id":operation}),
            json!({"kind":"delete_tree","path":path,"operation_id":operation}),
        ] {
            let (status, rejected) = send(&app, Some(&token), "POST", route, Some(body)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
            assert_eq!(rejected["error"]["code"], "INVALID_REQUEST");
            assert_eq!(
                success(&app, &token, "GET", "/api/repositories", None).await,
                before
            );
            assert_eq!(
                success(&app, &token, "GET", "/api/events", None).await,
                events
            );
            assert_eq!(manager_disk_files(root.path()), bytes);
        }
    }
}

#[tokio::test]
async fn disk_mkdir_keeps_pre_root_admission_and_resolved_receipts() {
    let root = tempfile::tempdir().unwrap();
    let mut config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    config.max_entries = 1;
    let app = configured_application(root.path(), config);
    let token = login(&app, "admin", "integration-test-password").await;
    let before = manager_disk_files(root.path());
    let route = "/api/file-manager/disks/P01/namespace";
    let (status, rejected) = send(
        &app,
        Some(&token),
        "POST",
        route,
        Some(json!({"kind":"mkdir","path":"one/two","operation_id":new_id("op")})),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(rejected["error"]["code"], "RESOURCE_LIMIT_EXCEEDED");
    assert_eq!(manager_disk_files(root.path()), before);
    let used = new_id("op");
    success(&app,&token,"POST","/api/repositories",Some(json!({"name":"Existing","primary_slot_id":"P01","target_copies":1,"operation_id":used}))).await;
    let before = manager_disk_files(root.path());
    let (status, rejected) = send(
        &app,
        Some(&token),
        "POST",
        route,
        Some(json!({"kind":"mkdir","path":"fresh","operation_id":used})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(rejected["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
    assert_eq!(manager_disk_files(root.path()), before);
    for path in ["Existing", "existing"] {
        assert_eq!(
            send(
                &app,
                Some(&token),
                "POST",
                route,
                Some(json!({"kind":"mkdir","path":path,"operation_id":new_id("op")}))
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(manager_disk_files(root.path()), before);
    }
    let mut receipts = Vec::new();
    for (display, relative) in [("Existing/Café", "Café"), ("root-folder", "root-folder")] {
        let request = json!({"kind":"mkdir","path":display,"operation_id":new_id("op")});
        let receipt = success(&app, &token, "POST", route, Some(request.clone())).await;
        let entries = success(
            &app,
            &token,
            "GET",
            &format!(
                "/api/file-manager/disks/P01/entries?{}",
                url_encoding::form::Serializer::new(String::new())
                    .append_pair("path", display)
                    .finish()
            ),
            None,
        )
        .await;
        let repo = entries["context"]["repository_id"].as_str().unwrap();
        assert_eq!(receipt["request_digest"],storage_contract::object_hash("request",&json!({"kind":"file.namespace","principal":"admin","body":{"repository_id":repo,"change":{"kind":"mkdir","path":relative,"destination":null}}})).unwrap());
        if display.starts_with("Existing/") {
            let mut alias = request.clone();
            alias["path"] = "EXISTING/Café".into();
            receipts.push((alias, receipt.clone()));
        }
        receipts.push((request, receipt));
    }
    drop(app);
    let system = root.path().join("system");
    let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
    let auth = backend.open_auth(&system).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let revoked = login(&app, "admin", "integration-test-password").await;
    success(&app, &revoked, "POST", "/api/auth/logout", None).await;
    // Snapshot after the intentional session-control publications, not before them.
    success(&app, &token, "GET", "/api/events", None).await;
    let before = manager_disk_files(root.path());
    let (status, rejected) = send(
        &app,
        Some(&token),
        "POST",
        "/api/file-manager/disks/B01/namespace",
        Some(json!({"kind":"mkdir","path":"fresh","operation_id":new_id("op")})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(rejected["error"]["code"], "DISK_READ_ONLY");
    for (request, original) in receipts {
        assert_eq!(
            send(&app, Some(&revoked), "POST", route, Some(request.clone()))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            send(
                &app,
                Some(&token),
                "POST",
                "/api/file-manager/disks/B01/namespace",
                Some(request.clone())
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        let replay = success(&app, &token, "POST", route, Some(request)).await;
        for field in [
            "operation_id",
            "request_digest",
            "publication_sequence",
            "result",
        ] {
            assert_eq!(replay[field], original[field], "{field}");
        }
        assert_eq!(manager_disk_files(root.path()), before);
    }
}

#[tokio::test]
async fn disk_write_sessions_parse_before_lazy_root_and_remain_fresh_workspaces() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let route = "/api/file-manager/disks/P01/write-sessions";
    let repositories = success(&app, &token, "GET", "/api/repositories", None).await;
    let events = success(&app, &token, "GET", "/api/events", None).await;
    let bytes = manager_disk_files(root.path());
    for body in [
        json!({}),
        json!({"path":null}),
        json!({"path":123}),
        json!({"path":""}),
        json!({"path":"../escape"}),
        json!({"path":"Cafe\u{301}"}),
        json!({"path":"file.txt","branch_id":null}),
        json!({"path":"file.txt","branch_id":123}),
        json!({"path":"file.txt","branch_id":""}),
        json!({"path":"file.txt","branch_id":"bad"}),
        json!({"path":"file.txt","operation_id":new_id("op")}),
        json!({"path":"file.txt","unknown":true}),
        json!({"path":"file.txt","slot_id":"other"}),
    ] {
        let (status, rejected) = send(&app, Some(&token), "POST", route, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
        assert_eq!(rejected["error"]["code"], "INVALID_REQUEST");
        assert_eq!(
            success(&app, &token, "GET", "/api/repositories", None).await,
            repositories
        );
        assert_eq!(
            success(&app, &token, "GET", "/api/events", None).await,
            events
        );
        assert!(
            manager_disk_files(root.path()) == bytes,
            "invalid request changed disk bytes"
        );
    }
    let (status, rejected) = send(
        &app,
        Some(&token),
        "POST",
        route,
        Some(json!({"path":"file.txt","branch_id":new_id("br")})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(rejected["error"]["code"], "DIRECTORY_NOT_FOUND");
    assert_eq!(
        success(&app, &token, "GET", "/api/repositories", None).await,
        repositories
    );
    assert!(
        manager_disk_files(root.path()) == bytes,
        "requesting a missing branch created a root"
    );
    assert_eq!(
        send(
            &app,
            Some(&token),
            "POST",
            "/api/file-manager/disks/B01/write-sessions",
            Some(json!({"path":"file.txt"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert!(
        manager_disk_files(root.path()) == bytes,
        "backup request created a root"
    );
    success(&app,&token,"POST","/api/repositories",Some(json!({"name":"Existing","primary_slot_id":"P01","target_copies":1,"operation_id":new_id("op")}))).await;
    let bytes = manager_disk_files(root.path());
    assert_eq!(
        send(
            &app,
            Some(&token),
            "POST",
            route,
            Some(json!({"path":"existing"}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert!(
        manager_disk_files(root.path()) == bytes,
        "workspace over repository root changed disk bytes"
    );
    for (display, relative) in [
        ("Existing/Café.txt", "Café.txt"),
        ("root-file.txt", "root-file.txt"),
    ] {
        let request = json!({"path":display});
        let first = success(&app, &token, "POST", route, Some(request.clone())).await;
        let second = success(&app, &token, "POST", route, Some(request)).await;
        assert_ne!(
            first["workspace_id"], second["workspace_id"],
            "this route has no idempotent replay contract"
        );
        for workspace in [first, second] {
            assert_eq!(workspace["relative_path"], relative);
            assert_eq!(workspace["display_path"], display);
            assert_eq!(workspace["state"], "open");
            assert!(workspace["fencing_token"].is_string());
            assert!(workspace.get("operation_id").is_none());
            assert!(workspace.get("request_digest").is_none());
            assert!(workspace.get("result").is_none());
            assert!(!workspace.to_string().contains("source_path"));
            success(
                &app,
                &token,
                "POST",
                &format!(
                    "/api/workspaces/{}/abort",
                    workspace["workspace_id"].as_str().unwrap()
                ),
                Some(json!({})),
            )
            .await;
        }
    }
    let revoked = login(&app, "admin", "integration-test-password").await;
    success(&app, &revoked, "POST", "/api/auth/logout", None).await;
    assert_eq!(
        send(
            &app,
            Some(&revoked),
            "POST",
            route,
            Some(json!({"path":"new-file.txt"}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn disk_write_session_tracks_inherited_selection_and_merged_aliases() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "file.txt", b"frozen").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let file = listing["entries"][0]["file_id"].as_str().unwrap();
    let version = listing["entries"][0]["version_id"].as_str().unwrap();
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let mut branches = Vec::new();
    for name in ["left", "right"] {
        let branch = success(
            &app,
            &token,
            "POST",
            &format!("/api/file-manager/files/{file}/branches"),
            Some(json!({"name":name,"source_version_id":version,"operation_id":new_id("op")})),
        )
        .await;
        branches.push(branch["result"]["branch_id"].as_str().unwrap().to_owned());
    }
    let [left, right]: [String; 2] = branches.try_into().unwrap();
    success(
        &app,
        &token,
        "POST",
        &format!("/api/file-manager/files/{file}/select"),
        Some(json!({"branch_id":right,"operation_id":new_id("op")})),
    )
    .await;
    let frozen = manager_directory_branch(&app, &token, "", "frozen").await;

    success(
        &app,
        &token,
        "PUT",
        &format!("/api/repositories/{repo}/default-branch"),
        Some(json!({"directory_branch_id":frozen,"operation_id":new_id("op")})),
    )
    .await;
    let route = "/api/file-manager/disks/P01/write-sessions";
    let (status, rejected) = send(
        &app,
        Some(&token),
        "POST",
        route,
        Some(json!({"path":"file.txt","branch_id":left})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(rejected["error"]["code"], "FILE_BRANCH_CONFLICT");
    let workspace = success(
        &app,
        &token,
        "POST",
        route,
        Some(json!({"path":"file.txt","branch_id":right})),
    )
    .await;
    let context = &workspace["selected_file_contexts"][format!("{repo}/file.txt")];
    assert_eq!(context["branch_id"], right);
    assert!(context["inherited_from"].is_string());
    success(
        &app,
        &token,
        "POST",
        &format!(
            "/api/workspaces/{}/abort",
            workspace["workspace_id"].as_str().unwrap()
        ),
        Some(json!({})),
    )
    .await;
    success(&app,&token,"POST",&format!("/api/file-manager/files/{file}/merge"),Some(json!({"inputs":[{"branch_id":left,"expected_node_id":version},{"branch_id":right,"expected_node_id":version}],"chosen_node_id":version,"surviving_branch_id":left,"confirmed":true,"operation_id":new_id("op")}))).await;
    let (status, rejected) = send(
        &app,
        Some(&token),
        "POST",
        route,
        Some(json!({"path":"file.txt","branch_id":right})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(rejected["error"]["code"], "FILE_BRANCH_CONFLICT");
    let survivor = success(
        &app,
        &token,
        "POST",
        route,
        Some(json!({"path":"file.txt","branch_id":left})),
    )
    .await;
    assert_eq!(
        survivor["selected_file_contexts"][format!("{repo}/file.txt")]["branch_id"],
        left
    );
}

#[tokio::test]
async fn historical_prepare_preserves_history_admission_and_fresh_plan_contract() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let repo=success(&app,&token,"POST","/api/repositories",Some(json!({"name":"Prepare","primary_slot_id":"P01","target_copies":1,"operation_id":new_id("op")}))).await;
    let history = repo["history_tree_id"].as_str().unwrap();
    let branch = repo["main_branch_id"].as_str().unwrap();
    let head = success(
        &app,
        &token,
        "GET",
        &format!("/api/branches/{branch}"),
        None,
    )
    .await;
    let source = head["head_commit_id"].as_str().unwrap();
    let route = format!("/api/history-trees/{history}/historical-save-plans");
    let missing = "/api/history-trees/missing/historical-save-plans";
    let invalid = [
        json!({}),
        json!({"source_commit_id":null,"name":"saved"}),
        json!({"source_commit_id":source,"name":5}),
        json!({"source_commit_id":source,"name":"saved","workspace_id":5}),
        json!({"source_commit_id":source,"name":"saved","primary_slot_id":true}),
        json!({"source_commit_id":source,"name":"saved","target_copies":-1}),
        json!({"source_commit_id":source,"name":"saved","target_copies":4294967296u64}),
    ];
    let before = success(&app, &token, "GET", "/api/events", None).await;
    let bytes = manager_disk_files(root.path());
    for body in invalid {
        let (status, error) = send(&app, Some(&token), "POST", missing, Some(body.clone())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(error["error"]["code"], "NOT_FOUND");
        let (status, error) = send(&app, Some(&token), "POST", &route, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    }
    for body in [json!({"operation_id":new_id("op")}), json!({"unknown":1})] {
        assert_eq!(
            send(&app, Some(&token), "POST", missing, Some(body))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        success(&app, &token, "GET", "/api/events", None).await,
        before
    );
    assert!(
        manager_disk_files(root.path()) == bytes,
        "invalid plan preparation changed disk data"
    );
    let body = json!({"source_commit_id":source,"name":"saved"});
    let first = success(&app, &token, "POST", &route, Some(body.clone())).await;
    let second = success(&app, &token, "POST", &route, Some(body)).await;
    assert_ne!(first["plan_id"], second["plan_id"]);
    assert_eq!(first["input"], second["input"]);
    assert_eq!(first["digest"], second["digest"]);
    assert_eq!(first["kind"], "historical_save");
    assert_eq!(first["state"], "ready");
    assert!(first["expected_sequence"].is_string());
    assert!(first.get("operation_id").is_none());
    assert_eq!(first["input"]["history_tree_id"], history);
    assert_eq!(first["input"]["source_commit_id"], source);
    assert_eq!(first["input"]["workspace_id"], Value::Null);
    assert_eq!(first["input"]["sealed_manifest_hash"], Value::Null);
    assert_eq!(first["input"]["full_copy_bytes"], "0");
    let explicit=success(&app,&token,"POST",&route,Some(json!({"source_commit_id":source,"name":"saved","workspace_id":null,"primary_slot_id":null,"target_copies":null}))).await;
    assert_eq!(first["input"], explicit["input"]);
    assert_eq!(first["digest"], explicit["digest"]);
    let revoked = login(&app, "admin", "integration-test-password").await;
    success(&app, &revoked, "POST", "/api/auth/logout", None).await;
    assert_eq!(
        send(
            &app,
            Some(&revoked),
            "POST",
            missing,
            Some(json!({"name":null}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    drop(app);
    let system = root.path().join("system");
    let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
    let auth = backend.open_auth(&system).unwrap();
    let reopened = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    let token = login(&reopened, "admin", "integration-test-password").await;
    assert_eq!(
        success(
            &reopened,
            &token,
            "GET",
            &format!("/api/plans/{}", first["plan_id"].as_str().unwrap()),
            None
        )
        .await,
        first
    );
}

#[tokio::test]
async fn purge_prepare_preserves_history_priority_pins_and_sorted_fresh_plans() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("system");
    let mut engine = Engine::initialize(
        &system,
        Config::development([
            root.path().join("d1"),
            root.path().join("d2"),
            root.path().join("d3"),
        ]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Purge preparation").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let mut versions = Vec::new();
    for bytes in [
        b"old".as_slice(),
        b"middle".as_slice(),
        b"current".as_slice(),
    ] {
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file.txt", bytes)
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: String::new(),
                request_digest: None,
            })
            .unwrap();
        versions.push(
            engine
                .list_tree(&repo.main_branch_id)
                .unwrap()
                .into_iter()
                .find(|entry| entry.kind == storage_contract::NamespaceEntryKind::File)
                .unwrap()
                .version_id,
        );
    }
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend
        .protect_auth(AuthStore::initialize(&system, "admin", "integration-test-password").unwrap())
        .unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let route = format!("/api/history-trees/{}/purge-plans", repo.history_tree_id);
    let missing = "/api/history-trees/missing/purge-plans";
    let baseline = success(&app, &token, "GET", "/api/events", None).await;
    let bytes = manager_disk_files(root.path());
    for path in [route.as_str(), missing] {
        let (status, error) =
            send(&app, Some(&token), "POST", path, Some(json!({"unknown":1}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    }

    for body in [
        json!({}),
        json!({"version_ids":null}),
        json!({"version_ids":{}}),
        json!({"version_ids":"version"}),
        json!({"version_ids":[null]}),
        json!({"version_ids":[3]}),
    ] {
        let (status, error) = send(&app, Some(&token), "POST", missing, Some(body.clone())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(error["error"]["code"], "NOT_FOUND");
        let (status, error) = send(&app, Some(&token), "POST", &route, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    }
    for (ids, code) in [
        (json!([]), "INVALID_REQUEST"),
        (json!([""]), "VERSION_NOT_FOUND"),
        (json!([versions[2]]), "CONTENT_PINNED"),
    ] {
        let (_, error) = send(
            &app,
            Some(&token),
            "POST",
            &route,
            Some(json!({"version_ids":ids})),
        )
        .await;
        assert_eq!(error["error"]["code"], code);
    }
    assert_eq!(
        success(&app, &token, "GET", "/api/events", None).await,
        baseline
    );
    assert!(
        manager_disk_files(root.path()) == bytes,
        "rejected purge preparation changed disk data"
    );
    let first = success(
        &app,
        &token,
        "POST",
        &route,
        Some(json!({"version_ids":[versions[1],versions[0],versions[1]]})),
    )
    .await;
    let second = success(
        &app,
        &token,
        "POST",
        &route,
        Some(json!({"version_ids":[versions[0],versions[1]]})),
    )
    .await;
    let mut sorted = versions[..2].to_vec();
    sorted.sort();
    assert_eq!(
        first["input"],
        json!({"history_tree_id":repo.history_tree_id,"version_ids":sorted})
    );
    assert_eq!(
        first["digest"],
        storage_contract::object_hash("plan", &first["input"]).unwrap()
    );
    assert_eq!(first["input"], second["input"]);
    assert_eq!(first["digest"], second["digest"]);
    assert_ne!(first["plan_id"], second["plan_id"]);
    assert_eq!(first["kind"], "purge");
    assert_eq!(first["state"], "ready");
    assert!(first["expected_sequence"].is_string());
    assert!(first.get("operation_id").is_none());
    assert_eq!(
        send(
            &app,
            Some(&token),
            "POST",
            missing,
            Some(json!({"operation_id":new_id("op")}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let revoked = login(&app, "admin", "integration-test-password").await;
    success(&app, &revoked, "POST", "/api/auth/logout", None).await;
    assert_eq!(
        send(
            &app,
            Some(&revoked),
            "POST",
            missing,
            Some(json!({"version_ids":null}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    drop(app);
    let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
    let auth = backend.open_auth(&system).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    assert_eq!(
        success(
            &app,
            &token,
            "GET",
            &format!("/api/plans/{}", first["plan_id"].as_str().unwrap()),
            None
        )
        .await,
        first
    );
    assert_eq!(version_bytes(&app, &token, &versions[0]).await, b"old");
}

#[tokio::test]
async fn split_prepare_preserves_admission_cut_rules_workspace_pins_and_plan_wire() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("system");
    let mut engine = Engine::initialize(
        &system,
        Config::development([
            root.path().join("d1"),
            root.path().join("d2"),
            root.path().join("d3"),
        ]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Split preparation").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let root_commit = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"cut contents")
        .unwrap();
    let cut = engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap()
        .commit_id
        .unwrap();
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend
        .protect_auth(AuthStore::initialize(&system, "admin", "integration-test-password").unwrap())
        .unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let route = format!("/api/history-trees/{}/split-plans", repo.history_tree_id);
    let missing = "/api/history-trees/missing/split-plans";
    let baseline = success(&app, &token, "GET", "/api/events", None).await;
    let bytes = manager_disk_files(root.path());
    for path in [route.as_str(), missing] {
        let (status, error) =
            send(&app, Some(&token), "POST", path, Some(json!({"unknown":1}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    }

    for body in [
        json!({}),
        json!({"cut_commit_id":null}),
        json!({"cut_commit_id":{}}),
        json!({"cut_commit_id":[]}),
        json!({"cut_commit_id":3}),
        json!({"cut_commit_id":""}),
    ] {
        let (status, error) = send(&app, Some(&token), "POST", missing, Some(body.clone())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(error["error"]["code"], "NOT_FOUND");
        let (status, error) = send(&app, Some(&token), "POST", &route, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    }
    for (cut, code) in [
        ("missing", "COMMIT_NOT_FOUND"),
        (root_commit.as_str(), "SPLIT_ROOT_FORBIDDEN"),
    ] {
        let (_, error) = send(
            &app,
            Some(&token),
            "POST",
            &route,
            Some(json!({"cut_commit_id":cut})),
        )
        .await;
        assert_eq!(error["error"]["code"], code);
    }
    assert_eq!(
        success(&app, &token, "GET", "/api/events", None).await,
        baseline
    );
    assert!(
        manager_disk_files(root.path()) == bytes,
        "rejected split preparation changed disk data"
    );
    let workspace = success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({"branch_id":repo.main_branch_id})),
    )
    .await;
    let (_, busy) = send(
        &app,
        Some(&token),
        "POST",
        &route,
        Some(json!({"cut_commit_id":cut})),
    )
    .await;
    assert_eq!(busy["error"]["code"], "WORKSPACE_BUSY");
    success(
        &app,
        &token,
        "POST",
        &format!(
            "/api/workspaces/{}/abort",
            workspace["workspace_id"].as_str().unwrap()
        ),
        Some(json!({})),
    )
    .await;
    let first = success(
        &app,
        &token,
        "POST",
        &route,
        Some(json!({"cut_commit_id":cut})),
    )
    .await;
    let second = success(
        &app,
        &token,
        "POST",
        &route,
        Some(json!({"cut_commit_id":cut})),
    )
    .await;
    assert_eq!(
        first["input"],
        json!({"history_tree_id":repo.history_tree_id,"cut_commit_id":cut})
    );
    assert_eq!(
        first["digest"],
        storage_contract::object_hash("plan", &first["input"]).unwrap()
    );
    assert_eq!(first["input"], second["input"]);
    assert_eq!(first["digest"], second["digest"]);
    assert_ne!(first["plan_id"], second["plan_id"]);
    assert_eq!(first["kind"], "split");
    assert_eq!(first["state"], "ready");
    assert!(first["expected_sequence"].is_string());
    assert!(first.get("operation_id").is_none());
    assert_eq!(
        send(
            &app,
            Some(&token),
            "POST",
            missing,
            Some(json!({"operation_id":new_id("op")}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let revoked = login(&app, "admin", "integration-test-password").await;
    success(&app, &revoked, "POST", "/api/auth/logout", None).await;
    assert_eq!(
        send(
            &app,
            Some(&revoked),
            "POST",
            missing,
            Some(json!({"cut_commit_id":null}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    drop(app);
    let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
    let auth = backend.open_auth(&system).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    assert_eq!(
        success(
            &app,
            &token,
            "GET",
            &format!("/api/plans/{}", first["plan_id"].as_str().unwrap()),
            None
        )
        .await,
        first
    );
}
