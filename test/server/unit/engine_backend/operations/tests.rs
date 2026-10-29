use crate::{
    Backend,
    auth::{AuthStore, Identity},
    engine_backend::EngineBackend,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use storage_contract::{Config, OperationId};
use storage_engine::{CommitRequest, Engine};

const PASSWORD: &str = "integration-test-password";

type OperationFixture = (
    tempfile::TempDir,
    Arc<EngineBackend>,
    AuthStore,
    String,
    Identity,
    OperationId,
);

fn fixture() -> OperationFixture {
    fixture_with_files(1, b"fully verified content")
}

fn fixture_with_files(files: usize, bytes: &[u8]) -> OperationFixture {
    let root = crate::tests::private_tempdir();
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Operation").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    for index in 0..files {
        engine
            .stage_bytes(&workspace.workspace_id, &format!("data-{index}.txt"), bytes)
            .unwrap();
    }
    let operation = storage_contract::OperationId::generate();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation,
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
    (root, backend, auth, login.token, identity, operation)
}

#[test]
fn unauthorized_operations_never_start_payload_observation() {
    let (_root, backend, mut auth, token, identity, operation_id) = fixture();
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
            backend.operation(denied, operation_id).unwrap_err().code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        backend
            .operation(&identity, OperationId::generate())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::OperationNotFound
    );
    // Account admission does not waive durable operation ownership. This is a
    // storage fixture owned by another principal, not a second login account.
    let foreign_operation = storage_contract::OperationId::generate();
    backend
        .engine
        .lock()
        .unwrap()
        .create_repository(
            &storage_contract::RepositoryName::parse("Foreign receipt").unwrap(),
            "P01",
            3,
            "other",
            foreign_operation,
        )
        .unwrap();
    assert_eq!(
        backend
            .operation(&identity, foreign_operation)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Forbidden
    );
    auth.logout(&token).unwrap();
    assert_eq!(
        backend.operation(&identity, operation_id).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn slow_operation_health_releases_engine_and_rechecks_logged_out_session() {
    use std::sync::mpsc;
    use std::time::Duration;
    let (_root, backend, mut auth, token, identity, operation_id) = fixture();
    let (started, receive_started) = mpsc::sync_channel(0);
    let (release, receive_release) = mpsc::sync_channel(0);
    let receive_release = std::sync::Mutex::new(receive_release);
    *backend.operation_health_hook.lock().unwrap() = Some(Arc::new(move || {
        started.send(()).unwrap();
        receive_release.lock().unwrap().recv().unwrap();
    }));
    std::thread::scope(|scope| {
        let request = scope.spawn(|| backend.operation(&identity, operation_id));
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
    assert!(backend.operation(&fresh_identity, operation_id).is_ok());
}

#[test]
fn health_observation_rechecks_password_generation_after_unlocked_io() {
    use std::{sync::mpsc, time::Duration};
    let (_root, backend, mut auth, token, identity, operation_id) = fixture();
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
        let request = scope.spawn(|| backend.operation(&identity, operation_id));
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
    assert!(backend.operation(&fresh_identity, operation_id).is_ok());
}

#[test]
fn operation_observation_preserves_publication_wire_facts() {
    let (_root, backend, _auth, _token, identity, operation_id) = fixture();
    let before = backend
        .operation(&identity, operation_id)
        .and_then(crate::operation_wire::operation_observation_wire)
        .unwrap();
    assert_eq!(before["current_health"]["state"], "HEALTHY");
    let sequence = backend.engine.lock().unwrap().state().sequence;
    let replay = backend
        .operation(&identity, operation_id)
        .and_then(crate::operation_wire::operation_observation_wire)
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
        .operation(&identity, operation_id)
        .and_then(crate::operation_wire::operation_observation_wire)
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

#[test]
fn pending_owner_discovery_failure_is_not_a_missing_operation_response() {
    use std::error::Error as _;
    let (_root, backend, _auth, _token, identity, _published) = fixture();
    let operation = storage_contract::OperationId::generate();
    {
        let mut engine = backend.engine.lock().unwrap();
        engine.inject_fault_once("after_accept_1");
        assert!(
            engine
                .create_repository(
                    &storage_contract::RepositoryName::parse("pending-owner").unwrap(),
                    "P01",
                    3,
                    &identity.principal,
                    operation
                )
                .is_err()
        );
        for disk in &engine.config().disks {
            std::fs::write(
                disk.root.join("metadata/application-log/journal.bin"),
                b"invalid journal frame",
            )
            .unwrap();
        }
    }
    let error = backend.operation(&identity, operation).unwrap_err();
    assert_ne!(error.code, storage_contract::ErrorCode::NotFound);
    assert!(error.source().is_some());
    let outcome_error = backend.operation_outcome(&identity, operation).unwrap_err();
    assert_eq!(outcome_error.code, error.code);
    assert!(outcome_error.source().is_some());
}

#[test]
fn outcome_lookup_does_not_start_health_or_depend_on_current_payloads() {
    let (_root, backend, _auth, _token, identity, operation_id) = fixture();
    *backend.operation_health_hook.lock().unwrap() = Some(Arc::new(|| {
        panic!("outcome-only lookup started full-content health checking");
    }));
    let before = backend
        .operation_outcome(&identity, operation_id)
        .unwrap()
        .unwrap();
    {
        let engine = backend.engine.lock().unwrap();
        for disk in &engine.config().disks {
            std::fs::remove_dir_all(disk.root.join("histories")).unwrap();
        }
    }
    let after = backend
        .operation_outcome(&identity, operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(before, after);
    let wire = crate::operation_wire::operation_wire(after).unwrap();
    assert!(wire.get("current_health").is_none());
    assert_eq!(
        storage_contract::client_protocol::operation_status(&wire),
        Some(storage_contract::client_protocol::OperationStatus::Published)
    );
    *backend.operation_health_hook.lock().unwrap() = None;
    let observation = backend
        .operation(&identity, operation_id)
        .and_then(crate::operation_wire::operation_observation_wire)
        .unwrap();
    assert_eq!(observation["current_health"]["state"], "UNAVAILABLE");
    assert_eq!(observation["result"], wire["result"]);
}

#[test]
fn outcome_lookup_rechecks_current_identity_and_owner() {
    let (_root, backend, mut auth, token, identity, operation_id) = fixture();
    let foreign_operation = storage_contract::OperationId::generate();
    backend
        .engine
        .lock()
        .unwrap()
        .create_repository(
            &storage_contract::RepositoryName::parse("Foreign outcome").unwrap(),
            "P01",
            3,
            "other",
            foreign_operation,
        )
        .unwrap();
    assert_eq!(
        backend
            .operation_outcome(&identity, foreign_operation)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Forbidden
    );
    assert!(
        backend
            .operation_outcome(&identity, OperationId::generate())
            .unwrap()
            .is_none()
    );
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .operation_outcome(&identity, OperationId::generate())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(
        backend
            .operation_outcome(&identity, operation_id)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn existing_receipt_with_missing_history_is_not_an_absent_operation() {
    let (_root, backend, _auth, _token, identity, operation_id) = fixture();
    let outcome = backend
        .operation_outcome(&identity, operation_id)
        .unwrap()
        .unwrap();
    let history = outcome.history_tree_id.unwrap();
    {
        let mut engine = backend.engine.lock().unwrap();
        let mut state = engine.state().clone();
        assert!(state.histories.remove(&history).is_some());
        storage_engine::test_support::replace_state(&mut engine, state);
    }
    let error = backend
        .operation_outcome(&identity, operation_id)
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(error.message, "history not found");
    assert!(
        backend
            .operation_outcome(&identity, OperationId::generate())
            .unwrap()
            .is_none()
    );
}

#[test]
fn metadata_without_a_published_commit_scope_is_explicitly_not_observed() {
    let (_root, backend, _auth, _token, identity, _published) = fixture();
    let operation_id = OperationId::generate();
    {
        let mut engine = backend.engine.lock().unwrap();
        let file_id = engine
            .state()
            .versions
            .values()
            .next()
            .unwrap()
            .file_id
            .clone();
        engine
            .set_file_note(
                &file_id,
                None,
                &storage_contract::Note::parse("metadata only".into()).unwrap(),
                &identity.principal,
                operation_id,
            )
            .unwrap();
    }
    let observed = backend.operation(&identity, operation_id).unwrap();
    assert!(observed.outcome().commit_id.is_none());
    assert!(observed.health().is_none());
    let wire = crate::operation_wire::operation_observation_wire(observed).unwrap();
    assert!(wire["current_health"].is_null());
    assert_eq!(
        storage_contract::client_protocol::operation_observation_status(&wire),
        Some(storage_contract::client_protocol::OperationStatus::Published)
    );
}

#[test]
fn accepted_pending_operation_has_an_outcome_and_no_checked_health() {
    let (_root, backend, _auth, _token, identity, published) = fixture();
    let operation_id = OperationId::generate();
    {
        let mut engine = backend.engine.lock().unwrap();
        let branch = engine
            .operation_record(published)
            .unwrap()
            .result()
            .branch_id
            .clone()
            .unwrap();
        let workspace = engine
            .create_workspace(&branch, &identity.principal)
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "pending.txt", b"pending content")
            .unwrap();
        engine.inject_fault_once("after_accept_1");
        assert!(
            engine
                .commit_workspace(CommitRequest {
                    workspace_id: workspace.workspace_id,
                    operation_id,
                    principal: identity.principal.clone(),
                    expected_head: workspace.base_head,
                    fencing_token: workspace.fencing_token,
                    message: String::new(),
                    request_digest: None,
                })
                .is_err()
        );
    }
    let pending = backend
        .operation_outcome(&identity, operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(pending.commit_state, storage_contract::CommitState::InDoubt);
    assert_eq!(
        pending.protection_at_publish,
        storage_contract::ProtectionAtPublish::Unconfirmed
    );
    let observed = backend.operation(&identity, operation_id).unwrap();
    assert_eq!(observed.outcome(), &pending);
    assert!(observed.health().is_none());
    let wire = crate::operation_wire::operation_observation_wire(observed).unwrap();
    assert!(wire["current_health"].is_null());
    assert_eq!(
        storage_contract::client_protocol::operation_observation_status(&wire),
        Some(storage_contract::client_protocol::OperationStatus::Unknown)
    );
}

#[test]
fn foreign_pending_owner_is_rejected_by_both_operation_projections() {
    let (_root, backend, _auth, _token, identity, _published) = fixture();
    let operation_id = OperationId::generate();
    {
        let mut engine = backend.engine.lock().unwrap();
        engine.inject_fault_once("after_accept_1");
        assert!(
            engine
                .create_repository(
                    &storage_contract::RepositoryName::parse("Foreign pending outcome").unwrap(),
                    "P01",
                    3,
                    "other",
                    operation_id,
                )
                .is_err()
        );
    }
    *backend.operation_health_hook.lock().unwrap() = Some(Arc::new(|| {
        panic!("foreign pending operation reached a health observation");
    }));
    assert_eq!(
        backend
            .operation_outcome(&identity, operation_id)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Forbidden
    );
    assert_eq!(
        backend.operation(&identity, operation_id).unwrap_err().code,
        storage_contract::ErrorCode::Forbidden
    );
}

#[test]
#[ignore = "same-workload outcome and complete diagnostic measurement; run in GitHub Actions"]
fn outcome_and_diagnostic_500_file_measurement() {
    const FILES: usize = 500;
    const FILE_BYTES: usize = 4096;
    const REPETITIONS: usize = 3;
    let bytes = vec![0x5a; FILE_BYTES];
    let (_root, backend, _auth, _token, identity, operation_id) = fixture_with_files(FILES, &bytes);
    let (copies, sequence) = {
        let engine = backend.engine.lock().unwrap();
        assert_eq!(engine.state().versions.len(), FILES);
        (engine.config().disks.len(), engine.state().sequence)
    };
    let expected = backend
        .operation_outcome(&identity, operation_id)
        .unwrap()
        .unwrap();
    println!(
        "operation_measurement files={FILES} file_bytes={FILE_BYTES} total_bytes={} replicas={copies} repetitions={REPETITIONS}",
        FILES * FILE_BYTES
    );
    for repetition in 1..=REPETITIONS {
        let started = std::time::Instant::now();
        let outcome = backend
            .operation_outcome(&identity, operation_id)
            .unwrap()
            .unwrap();
        let outcome_elapsed = started.elapsed();
        let started = std::time::Instant::now();
        let observed = backend.operation(&identity, operation_id).unwrap();
        let diagnostic_elapsed = started.elapsed();
        assert_eq!(outcome, expected);
        assert_eq!(observed.outcome(), &expected);
        assert_eq!(
            observed.health().unwrap().state(),
            storage_contract::CurrentHealth::Healthy
        );
        assert!(!observed.health().unwrap().checked_at().is_empty());
        assert_eq!(backend.engine.lock().unwrap().state().sequence, sequence);
        println!(
            "operation_measurement repetition={repetition} outcome_ns={} diagnostic_ns={}",
            outcome_elapsed.as_nanos(),
            diagnostic_elapsed.as_nanos()
        );
    }
}
