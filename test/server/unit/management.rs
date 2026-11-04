use super::web_setup_response;
use crate::{Backend, projection_failure};
use crate::{ServerConfig, auth::AuthStore, management::*, router};
use http_routing::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use json_codec::{Value, json};
use service_stack::ServiceExt;

#[test]
fn managed_readiness_exposes_real_index_failure_without_rejecting_published_operation() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("pool");
    let management = root.path().join("management");
    let mut engine = storage_engine::Engine::initialize(
        &data,
        storage_contract::Config::development([root.path().join("disk")]).unwrap(),
    )
    .unwrap();
    let before = engine.state().sequence;
    let operation = storage_contract::OperationId::generate().unwrap();
    let index = data.join("index.sqlite");
    let projected_before = projection_failure::fail_projection(&index);
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("published").unwrap(),
            "P01",
            1,
            "admin",
            operation,
        )
        .unwrap();
    assert_eq!(engine.state().sequence, before + 1);
    assert_eq!(
        engine.state().operations[&operation].commit_state,
        storage_contract::CommitState::Published
    );
    let receipt =
        crate::operation_wire::operation_wire(engine.state().operations[&operation].clone().into())
            .unwrap();
    assert_eq!(
        projection_failure::receipt_outcome(&receipt)["commit_state"],
        "PUBLISHED"
    );
    assert_eq!(
        projection_failure::projection_sequence(&index),
        projected_before
    );
    let (backend, _auth) = open_service(&data, &management).unwrap();
    backend.inner.set(
        Some(std::sync::Arc::new(
            crate::engine_backend::EngineBackend::new(engine),
        )),
        crate::StorageState::Ready,
    );
    let ready = backend.ready().unwrap();
    assert_eq!(
        ready,
        crate::Readiness::Managed {
            history_index: crate::HistoryIndexStatus::Unavailable {
                code: storage_contract::ErrorCode::IndexRebuildFailed,
            },
        }
    );
    assert_eq!(
        json_codec::to_value(crate::backend_status::ReadinessResponse::from(ready)).unwrap(),
        json!({"ready":true,"history_index":{"state":"unavailable","code":"INDEX_REBUILD_FAILED"}})
    );
}

async fn request(
    app: &Router,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut r = Request::builder()
        .uri(path)
        .method(if body.is_some() { "POST" } else { "GET" })
        .header("content-type", "application/json");
    if let Some(token) = token {
        r = r.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(
            r.body(
                body.map(|b| Body::from(json_codec::to_vec(&b).unwrap()))
                    .unwrap_or_else(Body::empty),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, json_codec::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn missing_data_disks_leave_login_diagnostics_and_health_usable() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("pool");
    let management = root.path().join("management");
    let disks = [
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ];
    drop(
        storage_engine::Engine::initialize(
            &data,
            storage_contract::Config::development(disks.clone()).unwrap(),
        )
        .unwrap(),
    );
    for disk in disks {
        std::fs::remove_dir_all(disk).unwrap();
    }
    drop(AuthStore::initialize_management(&management, "admin", "a-long-test-password").unwrap());
    let (backend, auth) = open_service(&data, &management).unwrap();
    let app = router(backend.clone(), auth, ServerConfig::new(&management)).unwrap();
    assert_eq!(
        request(&app, "/healthz", None, None).await.0,
        StatusCode::OK
    );
    let (status, result) = request(
        &app,
        "/api/auth/login",
        None,
        Some(json!({"username":"admin","password":"a-long-test-password"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = result["token"].as_str().unwrap();
    assert_eq!(
        request(&app, "/api/auth/me", Some(token), None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "/api/system/storage", Some(token), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "/api/system/status", Some(token), None)
            .await
            .0,
        StatusCode::OK
    );
    let (status, error) = request(&app, "/api/admin/disks", Some(token), Some(json!(null))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["error"]["code"], "STORAGE_UNAVAILABLE");
    assert_eq!(
        request(&app, "/api/admin/disks", None, Some(json!({})))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let invalid = request(&app, "/api/repositories", Some(token), Some(json!({}))).await;
    assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
    let (status, error) = request(
        &app,
        "/api/repositories",
        Some(token),
        Some(json!({
            "name":"Unavailable", "primary_slot_id":"P01", "target_copies":3,
            "operation_id":storage_contract::OperationId::generate().unwrap()
        })),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["error"]["code"], "STORAGE_UNAVAILABLE");
    assert_eq!(
        request(&app, "/api/system/storage/retry", None, Some(json!({})))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    for _ in 0..200 {
        if !backend.management_status().recovering() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        backend.management_status().state(),
        crate::StorageState::Unavailable
    );
    let (status, capabilities) = request(&app, "/api/capabilities", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        capabilities,
        json!({"features":[],"max_upload_bytes":"0","storage":{"state":"unavailable","retryable":true,"recovering":false}})
    );
}
#[tokio::test]
async fn stale_legacy_credentials_do_not_open_management_when_engine_fails() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("pool");
    let management = root.path().join("management");
    drop(AuthStore::initialize(&data, "admin", "a-long-test-password").unwrap());
    std::fs::write(data.join("config.json"), "corrupt").unwrap();
    let (backend, auth) = open_service(&data, &management).unwrap();
    let app = router(backend, auth, ServerConfig::new(&management)).unwrap();
    assert_eq!(
        request(&app, "/healthz", None, None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "/api/management/status", None, None).await.1["authentication"],
        "recovery_required"
    );
    assert_eq!(
        request(
            &app,
            "/api/auth/login",
            None,
            Some(json!({"username":"admin","password":"a-long-test-password"}))
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(!management.join("management-auth.json").exists());
}
#[test]
fn replica_failure_cannot_roll_back_revocation_or_password_change() {
    let root = crate::tests::private_tempdir();
    let mut auth =
        AuthStore::initialize_management(root.path(), "admin", "a-long-test-password").unwrap();
    let old = auth.login("admin", "a-long-test-password").unwrap();
    auth.attach_replica(std::sync::Arc::new(|_| {
        Err(crate::ApiError::new(
            ::http::StatusCode::SERVICE_UNAVAILABLE,
            storage_contract::ErrorCode::InjectedFailure,
            "disk offline",
        ))
    }));
    auth.change_password(&old.token, "a-long-test-password", "a-new-test-password")
        .unwrap();
    assert!(auth.authenticate(&old.token).is_err());
    drop(auth);
    let mut auth = AuthStore::open_management(root.path()).unwrap();
    assert!(auth.authenticate(&old.token).is_err());
    assert!(auth.login("admin", "a-long-test-password").is_err());
    assert!(auth.login("admin", "a-new-test-password").is_ok());
}

#[tokio::test]
async fn blocked_replication_never_blocks_login_and_queued_sessions_revalidate() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("pool");
    let management = root.path().join("management");
    drop(
        storage_engine::Engine::initialize(
            &data,
            storage_contract::Config::development([
                root.path().join("d1"),
                root.path().join("d2"),
                root.path().join("d3"),
            ])
            .unwrap(),
        )
        .unwrap(),
    );
    let auth =
        AuthStore::initialize_management(&management, "admin", "a-long-test-password").unwrap();
    drop(auth);
    let (backend, auth) = open_service(&data, &management).unwrap();
    let app = router(backend.clone(), auth, ServerConfig::new(&management)).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if backend.management_status().state() == crate::StorageState::Ready {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let shared = backend.inner.auth.lock().unwrap().clone().unwrap();
    let session = shared
        .lock()
        .unwrap()
        .login("admin", "a-long-test-password")
        .unwrap();
    let stale = shared.lock().unwrap().authenticate(&session.token).unwrap();
    let engine = backend
        .inner
        .state
        .lock()
        .unwrap()
        .backend
        .engine()
        .cloned()
        .unwrap();
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let held_engine = engine.clone();
    let hold = std::thread::spawn(move || {
        held_engine.test_with_engine_locked(|_| {
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        })
    });
    held_rx.recv().unwrap();
    let queued = std::thread::spawn(move || engine.repositories_snapshot(&stale));
    shared.lock().unwrap().logout(&session.token).unwrap();
    // This is a deadlock guard, not a latency benchmark. The engine remains
    // locked until the result is captured, even on slower test runners.
    let login = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        request(
            &app,
            "/api/auth/login",
            None,
            Some(json!({"username":"admin","password":"a-long-test-password"})),
        ),
    )
    .await;
    // Always release the deliberately blocked worker before asserting.
    release_tx.send(()).unwrap();
    hold.join().unwrap();
    assert_eq!(login.unwrap().0, StatusCode::OK);
    assert_eq!(
        queued.join().unwrap().unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(
        request(&app, "/api/auth/me", Some(&session.token), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn protected_web_setup_installs_primary_then_allows_login_without_restart() {
    exercise_web_setup(&["one", "two", "three"], false).await;
}

#[tokio::test]
async fn single_disk_web_setup_ready_then_missing_disk_preserves_management() {
    exercise_web_setup(&["one"], true).await;
}

async fn exercise_web_setup(ids: &[&str], remove_data_disk: bool) {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("pool");
    let management = data.join("management");
    let discovery = root.path().join("disks");
    std::fs::create_dir(&discovery).unwrap();
    let mut disks = std::collections::BTreeMap::new();
    for &id in ids {
        let path = discovery.join(id);
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        disks.insert(id.to_owned(), path);
    }
    // No deployment candidate registry is needed for management setup or activation.
    let (backend, auth) = open_service_with_options(
        &data,
        &management,
        ManagementOptions {
            allow_development_setup: true,
            ..Default::default()
        },
    )
    .unwrap();
    let app = router(backend.clone(), auth, ServerConfig::new(&management)).unwrap();
    assert_eq!(
        request(&app, "/api/setup/status", None, None).await.0,
        StatusCode::OK
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/setup/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let status: Value = json_codec::from_slice(
        &http_routing::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(status["state"], "empty");
    for _ in 0..100 {
        if !backend.management_status().recovering() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/setup/initialize")
                .header("host", "127.0.0.1:7400")
                .header("origin", "http://127.0.0.1:7400")
                .header("content-type", "application/json")
                .body(Body::from(
                    json_codec::to_vec(
                        &json!({"username":"admin","password":"a-long-test-password"}),
                    )
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let code = response.status();
    let value: Value = json_codec::from_slice(
        &http_routing::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(code, StatusCode::OK, "{value:?}");
    web_setup_response::validate(value.clone()).unwrap();
    assert_eq!(value["storage_pending"], true);
    assert!(!data.join("config.json").exists());
    for path in disks.values() {
        assert!(std::fs::read_dir(path).unwrap().next().is_none());
    }
    assert!(data.join("management-authority.json").exists());
    assert!(management.join("management-auth.json").exists());
    assert_eq!(
        request(
            &app,
            "/api/auth/login",
            None,
            Some(json!({"username":"admin","password":"a-long-test-password"}))
        )
        .await
        .0,
        StatusCode::OK
    );

    assert_eq!(
        backend.management_status().state(),
        crate::StorageState::Unconfigured
    );
    let (_, login) = request(
        &app,
        "/api/auth/login",
        None,
        Some(json!({"username":"admin","password":"a-long-test-password"})),
    )
    .await;
    let token = login["token"].as_str().unwrap();
    let (code, pending) = request(&app, "/api/admin/storage/status", Some(token), None).await;
    assert_eq!(code, StatusCode::OK, "{pending:?}");
    assert_eq!(pending["state"], "pending");
    let (code, settings) = put_json(
        &app,
        "/api/admin/storage/discovery-root",
        token,
        json!({"expected_discovery_root":"/zxs/data/disks","discovery_root":discovery}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{settings:?}");
    assert_eq!(settings["discovery_root"], discovery.to_str().unwrap());
    let (code, stale) = put_json(
        &app,
        "/api/admin/storage/discovery-root",
        token,
        json!({"expected_discovery_root":"/zxs/data/disks","discovery_root":discovery}),
    )
    .await;
    assert_eq!(code, StatusCode::CONFLICT, "{stale:?}");
    assert_eq!(
        request(
            &app,
            "/api/admin/storage/directories?path=/",
            Some(token),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let mut selections = Vec::new();
    for path in disks.values() {
        let uri = format!(
            "/api/admin/storage/directories?path={}",
            storage_contract::client_protocol::uri_component_segment(path.to_str().unwrap())
        );
        let (code, listing) = request(&app, &uri, Some(token), None).await;
        assert_eq!(code, StatusCode::OK, "{listing:?}");
        let (code, selected) = request(
            &app,
            "/api/admin/storage/directories/inspect",
            Some(token),
            Some(json!({"mount_path":listing["mount_path"],"root_path":path})),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{selected:?}");
        selections.push(selected);
    }
    let (code, activated) = request(
        &app,
        "/api/admin/storage/activate",
        Some(token),
        Some(json!({"selections":selections,"mode":"development"})),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{activated:?}");
    super::web_activation_response::activation(activated).unwrap();
    wait_for_ready(&backend).await;
    let config: storage_contract::Config =
        json_codec::from_slice(&std::fs::read(data.join("config.json")).unwrap()).unwrap();
    assert_eq!(config.disks.len(), ids.len());
    assert_eq!(
        config
            .disks
            .iter()
            .filter(|disk| disk.metadata_member)
            .count(),
        ids.len()
    );
    let (code, repository) = request(&app, "/api/repositories", Some(token), Some(json!({"name":"single-copy","primary_slot_id":"P01","target_copies":1,"operation_id":storage_contract::new_id("op").unwrap()}))).await;
    assert_eq!(code, StatusCode::OK, "{repository:?}");
    if remove_data_disk {
        drop(app);
        drop(backend);
        std::fs::remove_dir_all(discovery.join(ids[0])).unwrap();
        let (backend, auth) = open_service(&data, &management).unwrap();
        let app = router(backend.clone(), auth, ServerConfig::new(&management)).unwrap();
        let (code, login) = request(
            &app,
            "/api/auth/login",
            None,
            Some(json!({"username":"admin","password":"a-long-test-password"})),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let token = login["token"].as_str().unwrap();
        assert_eq!(
            request(&app, "/healthz", None, None).await.0,
            StatusCode::OK
        );
        assert_eq!(
            request(&app, "/api/admin/accounts", Some(token), None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(&app, "/api/system/storage", Some(token), None)
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            request(&app, "/api/repositories", Some(token), None)
                .await
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(!root.path().join(ids[0]).exists());
    }
}

#[tokio::test]
async fn singleton_http_lifecycle_remains_authoritative_without_storage() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("pool");
    let management = root.path().join("management");
    std::fs::create_dir(&data).unwrap();
    std::fs::write(data.join("config.json"), "broken").unwrap();
    drop(AuthStore::initialize_management(&management, "admin", "a-long-test-password").unwrap());
    let (backend, auth) = open_service(&data, &management).unwrap();
    let mut config = ServerConfig::new(&management);
    let web = root.path().join("web");
    std::fs::create_dir(&web).unwrap();
    std::fs::write(web.join("index.html"), "management shell").unwrap();
    config.web_dir = Some(web);
    let app = router(backend, auth, config).unwrap();
    let asset = app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(asset.status(), StatusCode::OK);
    let (_, login) = request(
        &app,
        "/api/auth/login",
        None,
        Some(json!({"username":"admin","password":"a-long-test-password"})),
    )
    .await;
    let token = login["token"].as_str().unwrap();
    for body in [
        None,
        Some(json!({"username":"other","password":"a-reader-test-password"})),
    ] {
        assert_eq!(
            request(&app, "/api/admin/accounts", Some(token), body)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        request(
            &app,
            "/api/auth/login",
            None,
            Some(json!({"username":"other","password":"a-reader-test-password"}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "/api/auth/me", Some(token), None).await.1["principal"],
        "admin"
    );
    assert_eq!(request(&app, "/api/auth/password", Some(token), Some(json!({"current_password":"a-long-test-password","new_password":"a-new-test-password"}))).await.0, StatusCode::OK);
    assert_eq!(
        request(&app, "/api/auth/me", Some(token), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "/api/auth/login",
            None,
            Some(json!({"username":"admin","password":"a-new-test-password"}))
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[test]
fn managed_admission_never_falls_back_to_stale_disaster_recovery_identity() {
    use std::sync::{Arc, Mutex};
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("pool");
    let management = root.path().join("management");
    let engine = storage_engine::Engine::initialize(
        &data,
        storage_contract::Config::development([
            root.path().join("d1"),
            root.path().join("d2"),
            root.path().join("d3"),
        ])
        .unwrap(),
    )
    .unwrap();
    let backend = Arc::new(crate::engine_backend::EngineBackend::new(engine));
    let legacy = AuthStore::initialize(&data, "former-owner", "a-long-test-password").unwrap();
    drop(backend.protect_auth(legacy).unwrap());
    let primary =
        AuthStore::initialize_management(&management, "admin", "a-long-test-password").unwrap();
    let primary = Arc::new(Mutex::new(primary));
    backend
        .set_management_auth(crate::lock(&primary).reader())
        .unwrap();
    let caller = crate::auth::Identity {
        admission_session: None,
        smb_binding: None,
        principal: "former-owner".into(),
    };
    assert_eq!(
        backend.repositories_snapshot(&caller).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    let admin = primary
        .lock()
        .unwrap()
        .identity_for_principal("admin")
        .unwrap();
    drop(primary);
    assert_eq!(
        backend.repositories_snapshot(&admin).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn stale_readiness_retry_waits_for_existing_engine_holders() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("pool");
    let management = root.path().join("management");
    let engine = storage_engine::Engine::initialize(
        &data,
        storage_contract::Config::development([
            root.path().join("d1"),
            root.path().join("d2"),
            root.path().join("d3"),
        ])
        .unwrap(),
    )
    .unwrap();
    let held = std::sync::Arc::new(crate::engine_backend::EngineBackend::new(engine));
    let (backend, _auth) = open_service(&data, &management).unwrap();
    {
        let mut state = backend.inner.state.lock().unwrap();
        state.backend = BackendState::Engine(held.clone());
        state.status = super::StorageState::Ready;
        state.admitted = true;
        state.observed_at = std::time::Instant::now() - std::time::Duration::from_secs(16);
    }
    assert_eq!(
        backend.management_status().state(),
        crate::StorageState::Unavailable
    );
    let error = backend.inner.start_recovery(true).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::StorageDraining);
    assert!(error.retryable);
    assert!(
        !backend
            .inner
            .recovering
            .load(std::sync::atomic::Ordering::Acquire)
    );
    assert!(!backend.inner.state.lock().unwrap().admitted);
}

#[test]
fn completed_disk_observation_releases_engine_before_retry() {
    use std::sync::{Arc, Mutex};

    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("pool");
    let management = root.path().join("management");
    let disk = root.path().join("disk");
    let detached = root.path().join("detached-disk");
    let mut engine = storage_engine::Engine::initialize(
        &data,
        storage_contract::Config::development([disk.clone()]).unwrap(),
    )
    .unwrap();
    let auth =
        AuthStore::initialize_management(&management, "admin", "a-long-test-password").unwrap();
    engine
        .persist_auth_state(
            auth.snapshot().unwrap(),
            "system_auth",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    drop(auth);
    let (backend, mut auth) = open_service(&data, &management).unwrap();
    let token = auth.login("admin", "a-long-test-password").unwrap().token;
    let identity = auth.authenticate(&token).unwrap();
    *backend.inner.auth.lock().unwrap() = Some(Arc::new(Mutex::new(auth)));
    let engine = Arc::new(crate::engine_backend::EngineBackend::new(engine));
    let observed_engine = Arc::downgrade(&engine);
    backend.inner.set(Some(engine), super::StorageState::Ready);

    // Drive the production sampling boundary directly, without starting its
    // periodic thread or relying on a five-second timing window.
    std::fs::rename(&disk, &detached).unwrap();
    backend.inner.observe_storage_once();
    assert_eq!(
        backend.management_status().state(),
        crate::StorageState::ReadOnly
    );
    let observation = backend
        .observe_disks(&crate::InternalDiskObservation(()))
        .unwrap();
    assert!(!observation[0].online);
    assert_eq!(
        observed_engine.strong_count(),
        1,
        "only management state may retain the engine between observations"
    );

    // The first retry after restoring the original disk must reopen, not
    // report STORAGE_DRAINING for an observer that has already finished.
    std::fs::rename(&detached, &disk).unwrap();
    backend.retry_storage(&identity).unwrap();
    let worker = backend
        .inner
        .recovery_worker
        .lock()
        .unwrap()
        .take()
        .unwrap();
    worker.join().unwrap();
    assert!(observed_engine.upgrade().is_none());
    assert_eq!(
        backend.management_status().state(),
        crate::StorageState::Ready
    );
    backend.drain_management().unwrap();
}

#[tokio::test]
async fn activation_completion_uses_original_directories_and_preserves_current_account() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("system");
    let management = data.join("management");
    let discovery = root.path().join("disks");
    std::fs::create_dir(&discovery).unwrap();
    let disk = discovery.join("mounted-disk-subdirectory");
    std::fs::create_dir(&disk).unwrap();
    std::fs::set_permissions(&disk, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (backend, auth) = open_service_with_options(
        &data,
        &management,
        ManagementOptions {
            allow_development_setup: true,
            ..Default::default()
        },
    )
    .unwrap();
    let app = router(backend.clone(), auth, ServerConfig::new(&management)).unwrap();
    wait_for_idle(&backend).await;
    let (code, value) = first_account_json(
        &app,
        "/api/setup/initialize",
        Some(json!({"username":"admin","password":"a-long-test-password"})),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{value:?}");
    let (_, login) = request(
        &app,
        "/api/auth/login",
        None,
        Some(json!({"username":"admin","password":"a-long-test-password"})),
    )
    .await;
    let token = login["token"].as_str().unwrap();
    let auth = backend.inner.auth.lock().unwrap().clone().unwrap();
    let identity = auth.lock().unwrap().authenticate(token).unwrap();
    backend
        .setup_operation(
            "set_discovery_root",
            json!({"expected_discovery_root":"/zxs/data/disks","discovery_root":discovery}),
            None,
            Some(&identity),
        )
        .unwrap();
    let before = std::fs::read(management.join("management-auth.json")).unwrap();
    let listing = crate::storage_directories::browse_within(
        discovery.to_str().unwrap(),
        disk.to_str().unwrap(),
    )
    .unwrap();
    let selection = crate::storage_directories::inspect_within(
        discovery.to_str().unwrap(),
        &storage_contract::storage_directories::InspectRequest {
            mount_path: listing.mount_path,
            root_path: disk.to_str().unwrap().into(),
        },
        &[data.clone(), management.clone()],
    )
    .unwrap();
    let service = crate::setup_recovery::SetupRecovery::new(
        data.clone(),
        management.clone(),
        Default::default(),
    )
    .unwrap()
    .allow_development_mode();
    let error = service
        .activate(
            &identity,
            crate::setup_recovery::StorageActivationRequest {
                selections: vec![selection],
                mode: storage_engine::Mode::Development,
            },
            |_| Ok(()),
            |_| {
                Err(crate::ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    storage_contract::ErrorCode::InjectedFailure,
                    "stop before auth replica publication",
                ))
            },
        )
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::InjectedFailure);
    assert!(service.activation_admitted().is_err());
    let (code, status) = request(&app, "/api/admin/storage/status", Some(token), None).await;
    assert_eq!(code, StatusCode::OK, "{status:?}");
    assert_eq!(status["state"], "recovery_required");
    assert_eq!(status["completion_available"], true);
    let (code, value) = request(
        &app,
        "/api/admin/storage/complete-activation",
        Some(token),
        Some(json!({})),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{value:?}");
    super::web_activation_response::activation(value).unwrap();
    wait_for_ready(&backend).await;
    assert_eq!(
        std::fs::read(management.join("management-auth.json")).unwrap(),
        before
    );
    let (code, status) = request(&app, "/api/admin/storage/status", Some(token), None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(status["state"], "active");
    assert_eq!(status["completion_available"], false);
}

async fn wait_for_idle(backend: &ManagementBackend) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while backend.management_status().recovering() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn first_account_json(app: &Router, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let status = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/setup/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    let status: Value = json_codec::from_slice(
        &http_routing::body::to_bytes(status.into_body(), 65536)
            .await
            .unwrap(),
    )
    .unwrap();
    let Some(body) = body else {
        return (StatusCode::OK, status);
    };
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("host", "127.0.0.1:7400")
                .header("origin", "http://127.0.0.1:7400")
                .header("content-type", "application/json")
                .body(Body::from(json_codec::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let code = response.status();
    let value = json_codec::from_slice(
        &http_routing::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap(),
    )
    .unwrap();
    (code, value)
}

#[tokio::test]
async fn account_setup_ignores_unavailable_candidates_and_rejects_old_disk_fields_and_routes() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("pool");
    let management = data.join("management");
    let unavailable = root.path().join("not-mounted");
    let (backend, auth) = open_service_with_options(
        &data,
        &management,
        ManagementOptions {
            setup_disks: std::collections::BTreeMap::from([(
                "offline".into(),
                unavailable.clone(),
            )]),
            setup_configs: std::collections::BTreeMap::from([(
                "offline".into(),
                root.path().join("missing-preset.json"),
            )]),
            ..Default::default()
        },
    )
    .unwrap();
    let app = router(backend.clone(), auth, ServerConfig::new(&management)).unwrap();
    wait_for_idle(&backend).await;
    let (_, status) = first_account_json(&app, "/api/setup/status", None).await;
    assert_eq!(status["state"], "empty");
    let (code,_)=first_account_json(&app,"/api/setup/initialize",Some(json!({"username":"admin","password":"a-long-test-password","disk_ids":["offline"],"mode":"production"}))).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    for path in [
        "/api/setup/preset-preview",
        "/api/setup/initialize-preset",
        "/api/setup/complete-initialization",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(!response.status().is_success(), "obsolete route {path}");
    }
    assert!(!data.join("config.json").exists());
    assert!(!management.join("management-auth.json").exists());
    let (code, value) = first_account_json(
        &app,
        "/api/setup/initialize",
        Some(json!({"username":"admin","password":"a-long-test-password"})),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{value:?}");
    web_setup_response::validate(value).unwrap();
    assert!(!unavailable.exists());
    assert!(!data.join("config.json").exists());
    assert!(management.join("management-auth.json").exists());
}

async fn wait_for_ready(backend: &ManagementBackend) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if backend.management_status().state() == crate::StorageState::Ready {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("successful installation must reopen storage without a restart");
}

#[test]
fn invalid_runtime_workspace_limits_do_not_create_management_state() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("pool");
    let management = root.path().join("management");
    let result = open_service_with_options(
        &data,
        &management,
        ManagementOptions {
            retained_workspace_limits: Some(storage_engine::RetainedWorkspaceLimits {
                max_retained_workspaces: 0,
                max_retained_workspaces_per_principal: 1,
                max_retained_workspace_metadata_bytes: 1,
                max_retained_workspace_metadata_bytes_per_principal: 1,
            }),
            ..Default::default()
        },
    );
    assert!(result.is_err());
    assert!(!data.exists());
    assert!(!management.exists());
}

#[tokio::test]
async fn background_reopen_uses_local_workspace_limits_and_authenticated_capacity() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("pool");
    let management = root.path().join("management");
    let mut config = storage_engine::Config::development([root.path().join("disk")]).unwrap();
    config.max_retained_workspaces = 1;
    config.max_retained_workspaces_per_principal = 1;
    let mut engine = storage_engine::Engine::initialize(&data, config.clone()).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("capacity").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    drop(engine);
    let persisted = std::fs::read(data.join("config.json")).unwrap();
    let limits = storage_engine::RetainedWorkspaceLimits {
        max_retained_workspaces: 3,
        max_retained_workspaces_per_principal: 2,
        max_retained_workspace_metadata_bytes: config.max_retained_workspace_metadata_bytes + 100,
        max_retained_workspace_metadata_bytes_per_principal: config
            .max_retained_workspace_metadata_bytes_per_principal
            + 100,
    };
    drop(AuthStore::initialize_management(&management, "admin", "a-long-test-password").unwrap());
    let (backend, auth) = open_service_with_options(
        &data,
        &management,
        ManagementOptions {
            retained_workspace_limits: Some(limits),
            ..Default::default()
        },
    )
    .unwrap();
    let app = router(backend.clone(), auth, ServerConfig::new(&management)).unwrap();
    wait_for_ready(&backend).await;
    let (status, login) = request(
        &app,
        "/api/auth/login",
        None,
        Some(json!({"username":"admin","password":"a-long-test-password"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = login["token"].as_str().unwrap();
    let (status, capacity) = request(&app, "/api/workspaces/capacity", Some(token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(capacity["global"]["records"]["used"], "1");
    assert_eq!(capacity["global"]["records"]["limit"], "3");
    assert_eq!(capacity["principal"]["records"]["limit"], "2");
    assert_eq!(
        capacity["global"]["metadata_bytes"]["limit"],
        limits.max_retained_workspace_metadata_bytes.to_string()
    );
    assert_eq!(
        capacity["principal"]["metadata_bytes"]["limit"],
        limits
            .max_retained_workspace_metadata_bytes_per_principal
            .to_string()
    );
    assert_eq!(capacity["warning"], "ok");
    assert_eq!(capacity["automatic_retirement"], true);
    assert!(json_codec::from_value::<storage_contract::WorkspaceCapacity>(capacity).is_ok());
    assert_eq!(
        request(
            &app,
            "/api/workspaces",
            Some(token),
            Some(json!({"branch_id":repo.main_branch_id}))
        )
        .await
        .0,
        StatusCode::OK
    );
    // The sole authenticated administrator sees both global and own counters.
    // Query parameters cannot select another principal or override local limits.
    let (status, current) = request(&app, "/api/workspaces/capacity", Some(token), None).await;
    assert_eq!(status, StatusCode::OK);
    let query = "/api/workspaces/capacity?admin=false&principal=forged&max_retained_workspaces=999";
    let (status, queried) = request(&app, query, Some(token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(queried, current);
    assert_eq!(queried["global"]["records"]["used"], "2");
    assert_eq!(queried["global"]["records"]["limit"], "3");
    assert_eq!(queried["principal"]["records"]["used"], "2");
    assert_eq!(queried["principal"]["records"]["limit"], "2");
    assert!(json_codec::from_value::<storage_contract::WorkspaceCapacity>(queried).is_ok());
    assert_eq!(
        request(&app, query, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, query, Some("forged-session"), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let forged = crate::auth::Identity {
        principal: "forged".into(),
        admission_session: None,
        smb_binding: None,
    };
    assert_eq!(
        backend.workspace_capacity(&forged).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(std::fs::read(data.join("config.json")).unwrap(), persisted);
    backend.drain_management().unwrap();
}

#[test]
fn below_baseline_runtime_limits_fail_in_background_without_blocking_management() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("pool");
    let management = root.path().join("management");
    drop(
        storage_engine::Engine::initialize(
            &data,
            storage_engine::Config::development([root.path().join("disk")]).unwrap(),
        )
        .unwrap(),
    );
    let config = std::fs::read(data.join("config.json")).unwrap();
    drop(AuthStore::initialize_management(&management, "admin", "a-long-test-password").unwrap());
    let (backend, auth) = open_service_with_options(
        &data,
        &management,
        ManagementOptions {
            retained_workspace_limits: Some(storage_engine::RetainedWorkspaceLimits {
                max_retained_workspaces: 1,
                max_retained_workspaces_per_principal: 1,
                max_retained_workspace_metadata_bytes: 1,
                max_retained_workspace_metadata_bytes_per_principal: 1,
            }),
            ..Default::default()
        },
    )
    .expect("pure startup must not compare against the pool configuration");
    let auth = std::sync::Arc::new(std::sync::Mutex::new(auth));
    *backend.inner.auth.lock().unwrap() = Some(auth.clone());
    backend.inner.start_recovery(false).unwrap();
    let worker = backend
        .inner
        .recovery_worker
        .lock()
        .unwrap()
        .take()
        .unwrap();
    worker.join().unwrap();
    assert_eq!(
        backend.management_status().state(),
        crate::StorageState::Unavailable
    );
    assert!(
        auth.lock()
            .unwrap()
            .login("admin", "a-long-test-password")
            .is_ok()
    );
    assert_eq!(std::fs::read(data.join("config.json")).unwrap(), config);
}

async fn put_json(app: &Router, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("host", "127.0.0.1:7400")
                .header("origin", "http://127.0.0.1:7400")
                .header("content-type", "application/json")
                .body(Body::from(json_codec::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let code = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    (code, json_codec::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn public_first_account_is_keyless_origin_checked_and_single_winner() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("system");
    let management = temporary.path().join("management");
    let (backend, auth) = open_service(&data, &management).unwrap();
    let app = router(backend.clone(), auth, ServerConfig::new(&management)).unwrap();
    wait_for_idle(&backend).await;
    assert!(!management.join("bootstrap.key").exists());
    assert_eq!(
        request(&app, "/api/setup/status", None, None).await,
        (StatusCode::OK, json!({"state":"empty"}))
    );
    let rejected = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/setup/initialize")
                .header("host", "127.0.0.1:7400")
                .header("origin", "https://untrusted.invalid")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"username":"outsider","password":"a-long-test-password"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    assert!(!management.join("management-auth.json").exists());
    let (first, second) = tokio::join!(
        first_account_json(
            &app,
            "/api/setup/initialize",
            Some(json!({"username":"first","password":"a-long-test-password"}))
        ),
        first_account_json(
            &app,
            "/api/setup/initialize",
            Some(json!({"username":"second","password":"another-long-password"}))
        )
    );
    assert_eq!(
        [first.0, second.0]
            .iter()
            .filter(|code| **code == StatusCode::OK)
            .count(),
        1,
        "{first:?} {second:?}"
    );
    assert_eq!(
        request(&app, "/api/setup/status", None, None).await,
        (StatusCode::OK, json!({"state":"management_ready"}))
    );
    let before = std::fs::read(management.join("management-auth.json")).unwrap();
    let retry = first_account_json(
        &app,
        "/api/setup/initialize",
        Some(json!({"username":"replacement","password":"replacement-password"})),
    )
    .await;
    assert_eq!(retry.0, StatusCode::CONFLICT, "{retry:?}");
    assert_eq!(
        std::fs::read(management.join("management-auth.json")).unwrap(),
        before
    );
    assert!(!data.join("config.json").exists());
    assert!(!management.join("bootstrap.key").exists());
    backend.drain_management().unwrap();
}

#[test]
fn anonymous_account_initialization_cannot_drain_or_detach_an_existing_backend() {
    for status in [
        StorageState::Ready,
        StorageState::ReadOnly,
        StorageState::RecoveryRequired,
        StorageState::Unavailable,
    ] {
        for (held, recovering) in [(false, false), (true, false), (false, true), (true, true)] {
            let root = tempfile::tempdir().unwrap();
            let data = root.path().join("pool");
            let management = root.path().join("management");
            let engine = storage_engine::Engine::initialize(
                &data,
                storage_contract::Config::development([root.path().join("disk")]).unwrap(),
            )
            .unwrap();
            let existing = Arc::new(EngineBackend::new(engine));
            let weak = Arc::downgrade(&existing);
            let holder = held.then(|| existing.clone());
            let primary =
                AuthStore::initialize_management(&management, "admin", "a-long-test-password")
                    .unwrap();
            drop(primary);
            let (backend, auth) = open_service(&data, &management).unwrap();
            *crate::lock(&backend.inner.auth) = Some(Arc::new(Mutex::new(auth)));
            {
                let mut state = crate::lock(&backend.inner.state);
                state.backend = BackendState::Engine(existing);
                state.status = status;
                state.admitted = true;
                state.observed_at = std::time::Instant::now();
            }
            backend.inner.recovering.store(recovering, Ordering::SeqCst);
            let credentials = fs::read(management.join("management-auth.json")).unwrap();
            let authority = fs::read(data.join("management-authority.json")).unwrap();
            let error = backend
                .setup_operation(
                    "initialize",
                    json!({"username":"attacker","password":"a-different-password"}),
                    None,
                    None,
                )
                .unwrap_err();
            assert_eq!(error.code, storage_contract::ErrorCode::RecoveryRequired);
            {
                let state = crate::lock(&backend.inner.state);
                assert!(state.admitted, "{status:?}, held={held}");
                assert_eq!(state.status, status);
                assert!(Arc::ptr_eq(
                    state.backend.engine().unwrap(),
                    &weak.upgrade().unwrap()
                ));
            }
            assert_eq!(
                fs::read(management.join("management-auth.json")).unwrap(),
                credentials
            );
            assert_eq!(
                fs::read(data.join("management-authority.json")).unwrap(),
                authority
            );
            assert_eq!(backend.inner.recovering.load(Ordering::SeqCst), recovering);
            backend.inner.recovering.store(false, Ordering::SeqCst);
            drop(holder);
        }
    }
}

#[test]
fn first_account_refreshes_an_inflight_pre_account_observation_without_claiming_its_gate() {
    use std::sync::mpsc;
    use std::time::Duration;
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("system");
    let management = root.path().join("management");
    let (backend, auth) = open_service(&data, &management).unwrap();
    *crate::lock(&backend.inner.auth) = Some(Arc::new(Mutex::new(auth)));
    let (entered, entry) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let calls = Arc::new(AtomicU64::new(0));
    let worker_calls = calls.clone();
    backend.inner.recovering.store(true, Ordering::SeqCst);
    let inner = backend.inner.clone();
    *crate::lock(&backend.inner.recovery_worker) = Some(std::thread::spawn(move || {
        inner.run_recovery(move |inner| {
            if worker_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(5)).unwrap();
                // A startup result captured before account creation arrives late.
                inner.set(None, StorageState::Unavailable);
                Ok(())
            } else {
                inner.recover_once()
            }
        });
    }));
    entry.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        backend
            .setup_operation(
                "initialize",
                json!({"username":"admin","password":"a-long-test-password"}),
                None,
                None
            )
            .unwrap(),
        json!({"initialized":true,"storage_pending":true})
    );
    assert!(backend.inner.recovering.load(Ordering::SeqCst));
    release.send(()).unwrap();
    let worker = crate::lock(&backend.inner.recovery_worker).take().unwrap();
    worker.join().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        backend.management_status().state(),
        StorageState::Unconfigured
    );
    assert!(!backend.inner.recovering.load(Ordering::SeqCst));
    assert!(!data.join("config.json").exists());
}
