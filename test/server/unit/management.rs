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
        storage_contract::Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let before = engine.state().sequence;
    let operation = storage_contract::OperationId::generate();
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
        engine
            .state()
            .operations
            .tail_record(operation)
            .unwrap()
            .result
            .commit_state,
        storage_contract::CommitState::Published
    );
    let receipt = crate::operation_wire::operation_wire(
        engine
            .state()
            .operations
            .tail_record(operation)
            .unwrap()
            .result
            .clone(),
    )
    .unwrap();
    assert_eq!(
        projection_failure::receipt_without_observation_time(&receipt)["commit_state"],
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
            storage_contract::Config::development(disks.clone()),
        )
        .unwrap(),
    );
    for disk in disks {
        std::fs::remove_dir_all(disk).unwrap();
    }
    drop(AuthStore::initialize_management(&management, "admin", "a-long-test-password").unwrap());
    let (backend, auth) = open_service(&data, &management).unwrap();
    let app = router(backend.clone(), auth, ServerConfig::loopback(&management)).unwrap();
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
            "operation_id":storage_contract::OperationId::generate()
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
    let app = router(backend, auth, ServerConfig::loopback(&management)).unwrap();
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
            ]),
        )
        .unwrap(),
    );
    let auth =
        AuthStore::initialize_management(&management, "admin", "a-long-test-password").unwrap();
    drop(auth);
    let (backend, auth) = open_service(&data, &management).unwrap();
    let app = router(backend.clone(), auth, ServerConfig::loopback(&management)).unwrap();
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
    let engine = backend.inner.state.lock().unwrap().backend.clone().unwrap();
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
    let mut disks = std::collections::BTreeMap::new();
    for &id in ids {
        let path = root.path().join(id);
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        disks.insert(id.to_owned(), path);
    }
    let (backend, auth) = open_service_with_disks(&data, &management, disks, true).unwrap();
    let key = "a".repeat(64);
    std::fs::write(management.join("bootstrap.key"), &key).unwrap();
    std::fs::set_permissions(
        management.join("bootstrap.key"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let app = router(backend.clone(), auth, ServerConfig::loopback(&management)).unwrap();
    assert_eq!(
        request(&app, "/api/setup/status", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/setup/status")
                .header("x-zxs-bootstrap", &key)
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
    let response=app.clone().oneshot(Request::builder().method("POST").uri("/api/setup/initialize")
        .header("host","127.0.0.1:7400").header("origin","http://127.0.0.1:7400").header("x-zxs-bootstrap",&key).header("x-csrf-token",status["csrf_token"].as_str().unwrap()).header("content-type","application/json")
        .body(Body::from(json_codec::to_vec(&json!({"mode":"development","disk_ids":ids,"username":"admin","password":"a-long-test-password"})).unwrap())).unwrap()).await.unwrap();
    let code = response.status();
    let value: Value = json_codec::from_slice(
        &http_routing::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(code, StatusCode::OK, "{value:?}");
    assert_eq!(value["restart_required"], false);
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

    for _ in 0..200 {
        if !backend.management_status().recovering() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        backend.management_status().state(),
        crate::StorageState::Ready
    );
    let config: storage_contract::Config =
        json_codec::from_slice(&std::fs::read(data.join("config.json")).unwrap()).unwrap();
    assert_eq!(config.disks.len(), ids.len());
    assert_eq!(
        config.disks.iter().filter(|d| d.metadata_member).count(),
        ids.len()
    );
    let (_, login) = request(
        &app,
        "/api/auth/login",
        None,
        Some(json!({"username":"admin","password":"a-long-test-password"})),
    )
    .await;
    let token = login["token"].as_str().unwrap();
    let (code, repository) = request(&app, "/api/repositories", Some(token), Some(json!({"name":"single-copy","primary_slot_id":"P01","target_copies":1,"operation_id":storage_contract::new_id("op")}))).await;
    assert_eq!(code, StatusCode::OK, "{repository:?}");
    if remove_data_disk {
        drop(app);
        drop(backend);
        std::fs::remove_dir_all(root.path().join(ids[0])).unwrap();
        let (backend, auth) = open_service(&data, &management).unwrap();
        let app = router(backend.clone(), auth, ServerConfig::loopback(&management)).unwrap();
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
    let mut config = ServerConfig::loopback(&management);
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
        ]),
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
        ]),
    )
    .unwrap();
    let held = std::sync::Arc::new(crate::engine_backend::EngineBackend::new(engine));
    let (backend, _auth) = open_service(&data, &management).unwrap();
    {
        let mut state = backend.inner.state.lock().unwrap();
        state.backend = Some(held.clone());
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
        storage_contract::Config::development([disk.clone()]),
    )
    .unwrap();
    let auth =
        AuthStore::initialize_management(&management, "admin", "a-long-test-password").unwrap();
    engine
        .persist_auth_state(
            auth.snapshot().unwrap(),
            "system_auth",
            storage_contract::OperationId::generate(),
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
async fn web_completion_requires_bound_pristine_pre_authority_setup_proof() {
    use std::os::unix::fs::PermissionsExt;
    for (interrupted_owned_setup, authority_already_claimed) in
        [(true, false), (true, true), (false, false)]
    {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("pool");
        let management = data.join("management");
        let mut disks = std::collections::BTreeMap::new();
        for id in ["one", "two", "three"] {
            let path = root.path().join(id);
            std::fs::create_dir(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            disks.insert(id.to_owned(), path);
        }
        let (backend, auth) =
            open_service_with_disks(&data, &management, disks.clone(), true).unwrap();
        let key = "a".repeat(64);
        std::fs::write(management.join("bootstrap.key"), &key).unwrap();
        std::fs::set_permissions(
            management.join("bootstrap.key"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        if interrupted_owned_setup {
            let service = crate::setup_recovery::SetupRecovery::new(
                data.clone(),
                management.clone(),
                disks.clone(),
            )
            .unwrap()
            .allow_development_mode();
            let permit = service.authorize_bootstrap(&key).unwrap();
            let request=json_codec::from_value(json!({"mode":"development","disk_ids":["one","two","three"],"username":"admin","password":"a-long-test-password"})).unwrap();
            let error = service
                .initialize(&permit, request, |_engine, _, _| {
                    Err(crate::ApiError::new(
                        ::http::StatusCode::INTERNAL_SERVER_ERROR,
                        storage_contract::ErrorCode::InjectedFailure,
                        "injected failure before authority publication",
                    ))
                })
                .unwrap_err();
            assert_eq!(error.code, storage_contract::ErrorCode::InjectedFailure);
        } else {
            drop(
                storage_engine::Engine::initialize(
                    &data,
                    storage_contract::Config::development([
                        disks["one"].clone(),
                        disks["two"].clone(),
                        disks["three"].clone(),
                    ]),
                )
                .unwrap(),
            );
        }
        if authority_already_claimed {
            claim_authority(&data).unwrap();
        }
        let completion_expected = interrupted_owned_setup && !authority_already_claimed;
        assert!(!management.join("management-auth.json").exists());
        assert_eq!(
            data.join("management-authority.json").exists(),
            authority_already_claimed
        );
        let app = router(backend.clone(), auth, ServerConfig::loopback(&management)).unwrap();
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
                    .uri("/api/setup/status")
                    .header("x-zxs-bootstrap", &key)
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
        assert_eq!(
            status["completion_available"], completion_expected,
            "{status:?}"
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/setup/complete-initialization")
                    .header("host", "127.0.0.1:7400")
                    .header("origin", "http://127.0.0.1:7400")
                    .header("x-zxs-bootstrap", &key)
                    .header("x-csrf-token", status["csrf_token"].as_str().unwrap())
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
        if completion_expected {
            assert_eq!(code, StatusCode::OK, "{value:?}");
            assert_eq!(value["completed"], true);
            wait_for_ready(&backend).await;
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
        } else {
            assert!(!code.is_success(), "{value:?}");
            assert!(!management.join("management-auth.json").exists());
            assert_eq!(
                data.join("management-authority.json").exists(),
                authority_already_claimed
            );
        }
    }
}

async fn bootstrap_json(
    app: &Router,
    key: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let status = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/setup/status")
                .header("x-zxs-bootstrap", key)
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
                .header("x-zxs-bootstrap", key)
                .header("x-csrf-token", status["csrf_token"].as_str().unwrap())
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
async fn preset_web_setup_is_lazy_digest_bound_and_preserves_custom_configuration() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("pool");
    let management = data.join("management");
    let mut disks = std::collections::BTreeMap::new();
    for id in ["one", "two", "three", "four"] {
        let path = root.path().join(id);
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        disks.insert(id.to_owned(), path);
    }
    let mut config = storage_contract::Config::development([
        disks["one"].clone(),
        disks["two"].clone(),
        disks["three"].clone(),
    ]);
    let mut extra = config.disks[2].clone();
    extra.slot_id = "D01".into();
    extra.level = 4;
    extra.upstream_slot_id = Some("C01".into());
    extra.disk_id = storage_contract::new_id("disk");
    extra.root = disks["four"].clone();
    extra.metadata_member = false;
    config.disks.push(extra);
    config.max_workspaces = 7;
    config.checksum_chunk_bytes = 1_048_576;
    let preset = root.path().join("custom-config.json");
    std::fs::write(&preset, json_codec::to_vec(&config).unwrap()).unwrap();
    std::fs::set_permissions(&preset, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (backend, auth) = open_service_with_options(
        &data,
        &management,
        ManagementOptions {
            setup_disks: disks,
            setup_configs: std::collections::BTreeMap::from([
                ("custom".into(), preset.clone()),
                (
                    "offline".into(),
                    root.path().join("unavailable-preset.json"),
                ),
            ]),
            allow_development_setup: true,
            retained_workspace_limits: None,
            scrub_policy: crate::integrity_scrub::IntegrityScrubPolicy::default(),
        },
    )
    .unwrap();
    let key = "b".repeat(64);
    std::fs::write(management.join("bootstrap.key"), &key).unwrap();
    std::fs::set_permissions(
        management.join("bootstrap.key"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let app = router(backend.clone(), auth, ServerConfig::loopback(&management)).unwrap();
    assert_eq!(
        request(&app, "/healthz", None, None).await.0,
        StatusCode::OK
    );
    for _ in 0..100 {
        if !backend.management_status().recovering() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let (_, status) = bootstrap_json(&app, &key, "/api/setup/status", None).await;
    assert_eq!(status["config_presets"], json!(["custom", "offline"]));
    let (code, preview) = bootstrap_json(
        &app,
        &key,
        "/api/setup/preset-preview",
        Some(json!({"preset_id":"custom"})),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{preview:?}");
    assert!(!preview.to_string().contains(root.path().to_str().unwrap()));
    assert_eq!(
        preview["configuration"]["disks"].as_array().unwrap().len(),
        4
    );
    config.max_workspaces = 8;
    std::fs::write(&preset, json_codec::to_vec(&config).unwrap()).unwrap();
    let (code,_)=bootstrap_json(&app,&key,"/api/setup/initialize-preset",Some(json!({"preset_id":"custom","config_digest":preview["config_digest"],"username":"admin","password":"a-long-test-password"}))).await;
    assert!(!code.is_success());
    assert!(!data.join("config.json").exists());
    assert!(!management.join("management-auth.json").exists());
    let (code, preview) = bootstrap_json(
        &app,
        &key,
        "/api/setup/preset-preview",
        Some(json!({"preset_id":"custom"})),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    let (code,value)=bootstrap_json(&app,&key,"/api/setup/initialize-preset",Some(json!({"preset_id":"custom","config_digest":preview["config_digest"],"username":"admin","password":"a-long-test-password"}))).await;
    assert_eq!(code, StatusCode::OK, "{value:?}");
    wait_for_ready(&backend).await;
    let persisted: Value =
        json_codec::from_slice(&std::fs::read(data.join("config.json")).unwrap()).unwrap();
    assert_eq!(persisted, json_codec::to_value(config).unwrap());
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
    let mut config = storage_engine::Config::development([root.path().join("disk")]);
    config.max_retained_workspaces = 1;
    config.max_retained_workspaces_per_principal = 1;
    let mut engine = storage_engine::Engine::initialize(&data, config.clone()).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("capacity").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
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
    let app = router(backend.clone(), auth, ServerConfig::loopback(&management)).unwrap();
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
            storage_engine::Config::development([root.path().join("disk")]),
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
