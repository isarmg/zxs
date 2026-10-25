use crate::{
    Backend, BackendAction,
    auth::{AuthStore, Identity},
    engine_backend::EngineBackend,
};
use json_codec::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use storage_contract::{Config, new_id};
use storage_engine::{CommitRequest, Engine};

const PASSWORD: &str = "integration-test-password";

fn fixture() -> (
    tempfile::TempDir,
    Arc<EngineBackend>,
    AuthStore,
    String,
    Identity,
    Value,
) {
    let root = crate::tests::private_tempdir();
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository("Operation", "P01", 3, "owner", &new_id("op"))
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "data.txt",
            b"fully verified content",
        )
        .unwrap();
    let operation = new_id("op");
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation.clone(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = AuthStore::initialize(root.path(), "owner", PASSWORD).unwrap();
    let mut auth = backend.protect_auth(auth).unwrap();
    let login = auth.login("owner", PASSWORD).unwrap();
    let identity = auth.authenticate(&login.token).unwrap();
    (
        root,
        backend,
        auth,
        login.token,
        identity,
        json!({"operation_id": operation}),
    )
}

#[test]
fn unauthorized_operations_never_start_payload_observation() {
    let (_root, backend, mut auth, token, identity, input) = fixture();
    let calls = Arc::new(AtomicUsize::new(0));
    let probe = calls.clone();
    *backend.operation_health_hook.lock().unwrap() = Some(Arc::new(move || {
        probe.fetch_add(1, Ordering::SeqCst);
    }));
    let foreign = Identity {
        principal: "other".into(),
        ..identity.clone()
    };
    let unknown_session = Identity {
        admission_session: Some("0".repeat(64)),
        ..identity.clone()
    };
    for denied in [&foreign, &unknown_session] {
        assert_eq!(
            backend
                .call(BackendAction::Operation, denied, input.clone())
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        backend
            .call(
                BackendAction::Operation,
                &identity,
                json!({"operation_id": new_id("op")})
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::OperationNotFound
    );
    // Account admission does not waive durable operation ownership. This is a
    // storage fixture owned by another principal, not a second login account.
    let foreign_operation = new_id("op");
    backend
        .engine
        .lock()
        .unwrap()
        .create_repository("Foreign receipt", "P01", 3, "other", &foreign_operation)
        .unwrap();
    assert_eq!(
        backend
            .call(
                BackendAction::Operation,
                &identity,
                json!({"operation_id": foreign_operation})
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Forbidden
    );
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .call(BackendAction::Operation, &identity, input)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn slow_operation_health_releases_engine_and_rechecks_logged_out_session() {
    use std::sync::mpsc;
    use std::time::Duration;
    let (_root, backend, mut auth, token, identity, input) = fixture();
    let (started, receive_started) = mpsc::sync_channel(0);
    let (release, receive_release) = mpsc::sync_channel(0);
    let receive_release = std::sync::Mutex::new(receive_release);
    *backend.operation_health_hook.lock().unwrap() = Some(Arc::new(move || {
        started.send(()).unwrap();
        receive_release.lock().unwrap().recv().unwrap();
    }));
    std::thread::scope(|scope| {
        let request =
            scope.spawn(|| backend.call(BackendAction::Operation, &identity, input.clone()));
        receive_started
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        // A deterministic barrier proves another thread can acquire Engine
        // before publishing the session revocation through that same mutex.
        if backend.engine.try_lock().is_err() {
            release.send(()).unwrap();
            panic!("health observation retained the Engine mutex");
        }
        let revoked = auth.logout(&token);
        release.send(()).unwrap();
        revoked.unwrap();
        assert_eq!(
            request.join().unwrap().unwrap_err().code,
            storage_contract::ErrorCode::Unauthorized
        );
    });
    *backend.operation_health_hook.lock().unwrap() = None;
    let login = auth.login("owner", PASSWORD).unwrap();
    let fresh_identity = auth.authenticate(&login.token).unwrap();
    assert!(
        backend
            .call(BackendAction::Operation, &fresh_identity, input)
            .is_ok()
    );
}

#[test]
fn health_observation_rechecks_password_generation_after_unlocked_io() {
    use std::{sync::mpsc, time::Duration};
    let (_root, backend, mut auth, token, identity, input) = fixture();
    // Rotation from another valid session must also revoke an in-flight read
    // authenticated with the first session, not only the rotating session.
    let rotating_session = auth.login("owner", PASSWORD).unwrap();
    let (started, receive_started) = mpsc::sync_channel(0);
    let (release, receive_release) = mpsc::sync_channel(0);
    let receive_release = std::sync::Mutex::new(receive_release);
    *backend.operation_health_hook.lock().unwrap() = Some(Arc::new(move || {
        started.send(()).unwrap();
        receive_release.lock().unwrap().recv().unwrap();
    }));
    std::thread::scope(|scope| {
        let request =
            scope.spawn(|| backend.call(BackendAction::Operation, &identity, input.clone()));
        receive_started
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        if backend.engine.try_lock().is_err() {
            release.send(()).unwrap();
            panic!("health observation retained the Engine mutex");
        }
        // Auth publishes its protected snapshot through the free Engine mutex;
        // the read must revalidate the new generation after unlocked I/O.
        let revoked = auth.change_password(
            &rotating_session.token,
            PASSWORD,
            "replacement-integration-password",
        );
        release.send(()).unwrap();
        revoked.unwrap();
        assert_eq!(
            request.join().unwrap().unwrap_err().code,
            storage_contract::ErrorCode::Unauthorized
        );
    });
    assert_eq!(
        auth.authenticate(&token).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(
        auth.authenticate(&rotating_session.token).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    *backend.operation_health_hook.lock().unwrap() = None;
    let login = auth
        .login("owner", "replacement-integration-password")
        .unwrap();
    let fresh_identity = auth.authenticate(&login.token).unwrap();
    assert!(
        backend
            .call(BackendAction::Operation, &fresh_identity, input)
            .is_ok()
    );
}

#[test]
fn operation_observation_preserves_publication_wire_facts() {
    let (_root, backend, _auth, _token, identity, input) = fixture();
    let before = backend
        .call(BackendAction::Operation, &identity, input.clone())
        .unwrap();
    assert_eq!(before["current_health"]["state"], "HEALTHY");
    let sequence = backend.engine.lock().unwrap().state().sequence;
    let replay = backend
        .call(BackendAction::Operation, &identity, input.clone())
        .unwrap();
    // Health timestamps are fresh observations, not publication facts.
    assert!(before["current_health"]["checked_at"].is_string());
    assert!(replay["current_health"]["checked_at"].is_string());
    assert_eq!(replay["current_health"]["state"], "HEALTHY");
    for field in [
        "operation_id",
        "operation_kind",
        "request_digest",
        "state",
        "commit_state",
        "result",
        "error",
    ] {
        assert_eq!(replay[field], before[field], "{field}");
    }
    assert!(before["result"]["publication_sequence"].is_string());
    assert_eq!(backend.engine.lock().unwrap().state().sequence, sequence);
    {
        let engine = backend.engine.lock().unwrap();
        for disk in &engine.config().disks {
            // Remove replicas after publication without changing the frozen
            // receipt. A new observation must never reuse prior health.
            std::fs::remove_dir_all(disk.root.join("histories")).unwrap();
        }
    }
    let after = backend
        .call(BackendAction::Operation, &identity, input)
        .unwrap();
    assert_eq!(after["current_health"]["state"], "UNAVAILABLE");
    assert_eq!(after["commit_state"], before["commit_state"]);
    assert_eq!(after["result"], before["result"]);
    assert_eq!(after["request_digest"], before["request_digest"]);
    assert_eq!(
        after["result"]["publication_sequence"],
        before["result"]["publication_sequence"]
    );
    assert_eq!(backend.engine.lock().unwrap().state().sequence, sequence);
}
