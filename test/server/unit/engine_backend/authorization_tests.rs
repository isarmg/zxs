use super::*;

#[test]
fn preparation_conflicts_retain_retryable_contract() {
    for code in [
        storage_contract::ErrorCode::CommitPreparationBusy,
        storage_contract::ErrorCode::CommitPreparationStale,
        storage_contract::ErrorCode::PreparationStale,
    ] {
        let mapped = error(storage_engine::Error::new(code, "retry preparation").retryable());
        assert_eq!(mapped.status, 409);
        assert_eq!(mapped.code, code);
        assert!(mapped.retryable);
    }
}

#[test]
fn upload_generation_and_lease_errors_preserve_conflict_status() {
    for (core_code, api_code) in [
        (
            storage_contract::ErrorCode::StaleUploadGeneration,
            storage_contract::ErrorCode::UploadGenerationConflict,
        ),
        (
            storage_contract::ErrorCode::UploadGenerationConflict,
            storage_contract::ErrorCode::UploadGenerationConflict,
        ),
        (
            storage_contract::ErrorCode::FencingTokenExpired,
            storage_contract::ErrorCode::LeaseExpired,
        ),
    ] {
        let error = error(storage_engine::Error::new(
            core_code,
            "retry needs new state",
        ));
        assert_eq!(error.status, 409, "{core_code}");
        assert_eq!(error.code, api_code);
    }
}

#[test]
fn missing_credentials_do_not_admit_even_a_named_administrator() {
    let root = crate::tests::private_tempdir();
    let engine = Engine::initialize(
        &root.path().join("system"),
        storage_engine::Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let backend = EngineBackend::new(engine);
    let caller = Identity {
        principal: "admin".into(),
        admission_session: None,
        smb_binding: None,
    };
    // Anonymous probes have separate typed entrances; every remaining generic
    // action must reject before inspecting its fields or doing storage work.
    for &action in crate::BackendAction::ALL {
        assert_eq!(
            backend.call(action, &caller, json!({})).unwrap_err().code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
    for result in [
        backend.disk_status(&caller).map(|_| ()),
        backend.integrity_check(&caller).map(|_| ()),
    ] {
        assert_eq!(
            result.unwrap_err().code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
    assert!(backend.capabilities().is_ok());
    assert!(backend.ready().is_ok());
}

#[test]
fn a_valid_singleton_admits_its_owner_but_not_arbitrary_callers() {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        storage_engine::Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let identity = install_test_administrator(&mut engine, "owner");
    let backend = EngineBackend::new(engine);
    assert!(backend.repositories_snapshot(&identity).is_ok());
    let stranger = Identity {
        principal: "other".into(),
        ..identity
    };
    assert_eq!(
        backend.repositories_snapshot(&stranger).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn workspace_authorization_borrows_records_and_preserves_owner_checks() {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        storage_engine::Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let owner = install_test_administrator(&mut engine, "owner");
    let repo = engine
        .create_repository(
            "Borrowed workspace",
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"content")
        .unwrap();
    let borrowed = workspace_authorize(&engine, &owner, &workspace.workspace_id).unwrap();
    assert!(std::ptr::eq(
        borrowed,
        engine.workspace_ref(&workspace.workspace_id).unwrap()
    ));
    assert_eq!(borrowed.files.len(), 1);
    let snapshot = engine.workspace(&workspace.workspace_id).unwrap();
    assert!(!std::ptr::eq(&snapshot, borrowed));
    assert_eq!(snapshot.files.len(), borrowed.files.len());
    let mut stranger = owner.clone();
    stranger.principal = "stranger".into();
    assert_eq!(
        workspace_authorize(&engine, &stranger, &workspace.workspace_id)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Forbidden
    );
    assert_eq!(
        workspace_authorize(&engine, &owner, "missing")
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::WorkspaceNotFound
    );
    engine
        .stage_bytes(&workspace.workspace_id, "second.txt", b"new")
        .unwrap();
    assert_eq!(snapshot.files.len(), 1);
    assert_eq!(
        engine
            .workspace_ref(&workspace.workspace_id)
            .unwrap()
            .files
            .len(),
        2
    );
}

#[test]
fn typed_upload_admission_preserves_owner_state_expiry_and_size_precedence() {
    let root = crate::tests::private_tempdir();
    let system = root.path().join("system");
    let disk = root.path().join("disk");
    let mut engine =
        Engine::initialize(&system, storage_engine::Config::development([disk.clone()])).unwrap();
    let owner = install_test_administrator(&mut engine, "owner");
    let repository = engine
        .create_repository(
            "Upload admission",
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let mut expired = engine
        .create_workspace(&repository.main_branch_id, "owner")
        .unwrap();
    let fresh = engine
        .create_workspace(&repository.main_branch_id, "owner")
        .unwrap();
    let foreign = engine
        .create_workspace(&repository.main_branch_id, "other")
        .unwrap();
    drop(engine);
    // An expired, otherwise valid persisted workspace exercises the actual
    // recovery boundary without a wall-clock sleep or production test hook.
    expired.created_at = 0;
    for parent in [system.join("workspaces"), disk.join("staging/workspaces")] {
        std::fs::write(
            parent.join(&expired.workspace_id).join("workspace.json"),
            json_codec::to_vec(&expired).unwrap(),
        )
        .unwrap();
    }
    let backend = EngineBackend::new(Engine::open(&system).unwrap());
    let maximum = Some(u64::MAX);
    assert_eq!(
        backend
            .authorize_upload(&owner, "missing", maximum)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::WorkspaceNotFound
    );
    assert_eq!(
        backend
            .authorize_upload(&owner, &foreign.workspace_id, maximum)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Forbidden
    );
    assert_eq!(
        backend
            .authorize_upload(&owner, &expired.workspace_id, maximum)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::LeaseExpired
    );
    backend
        .abort_workspace(&owner, &expired.workspace_id)
        .unwrap();
    assert_eq!(
        backend
            .authorize_upload(&owner, &expired.workspace_id, maximum)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::WorkspaceClosed
    );
    assert_eq!(
        backend
            .authorize_upload(&owner, &fresh.workspace_id, maximum)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::ResourceLimitExceeded
    );
    backend
        .authorize_upload(&owner, &fresh.workspace_id, None)
        .unwrap();
    backend
        .authorize_upload(&owner, &fresh.workspace_id, Some(0))
        .unwrap();
}

#[test]
fn typed_workspace_status_distinguishes_admission_errors_from_absent_items() {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        storage_engine::Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let owner = install_test_administrator(&mut engine, "owner");
    let repo = engine
        .create_repository(
            "Status",
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let owned = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    let foreign = engine
        .create_workspace(&repo.main_branch_id, "other")
        .unwrap();
    let backend = EngineBackend::new(engine);
    assert_eq!(
        backend
            .published_upload_status(&owner, "missing", "missing")
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::WorkspaceNotFound
    );
    assert_eq!(
        backend
            .published_upload_status(&owner, &foreign.workspace_id, "missing")
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Forbidden
    );
    assert!(
        backend
            .published_upload_status(&owner, &owned.workspace_id, "missing")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        backend
            .workspace(&owner, &foreign.workspace_id)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Forbidden
    );
    let snapshot = backend.workspace(&owner, &owned.workspace_id).unwrap();
    backend
        .abort_workspace(&owner, &owned.workspace_id)
        .unwrap();
    assert_eq!(snapshot.state, storage_contract::WorkspaceState::Open);
    assert_eq!(
        backend
            .workspace(&owner, &owned.workspace_id)
            .unwrap()
            .state,
        storage_contract::WorkspaceState::Aborted
    );
    assert!(
        backend
            .published_upload_status(&owner, &owned.workspace_id, "missing")
            .unwrap()
            .is_none()
    );
}

#[test]
fn upload_system_clock_failure_is_server_error_with_source() {
    use std::{
        error::Error as _,
        time::{Duration, UNIX_EPOCH},
    };
    assert_eq!(upload_time(UNIX_EPOCH).unwrap(), 0);
    assert_eq!(
        upload_time(UNIX_EPOCH + Duration::new(42, 999_999_999)).unwrap(),
        42
    );
    let error = upload_time(UNIX_EPOCH - Duration::from_nanos(1)).unwrap_err();
    assert_eq!(error.status, ::http::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error.code, storage_contract::ErrorCode::IoError);
    assert!(
        error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .is::<std::time::SystemTimeError>()
    );
    let engine_error = storage_engine::Error::from(
        (UNIX_EPOCH - Duration::from_secs(1))
            .duration_since(UNIX_EPOCH)
            .unwrap_err(),
    );
    let mapped = super::error(engine_error);
    assert_eq!(mapped.status, ::http::StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        mapped
            .source()
            .unwrap()
            .source()
            .unwrap()
            .source()
            .unwrap()
            .is::<std::time::SystemTimeError>()
    );
}
