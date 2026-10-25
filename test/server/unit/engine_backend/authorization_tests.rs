use super::*;

#[test]
fn preparation_conflicts_retain_retryable_contract() {
    for code in [
        "COMMIT_PREPARATION_BUSY",
        "COMMIT_PREPARATION_STALE",
        "PREPARATION_STALE",
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
        ("STALE_UPLOAD_GENERATION", "UPLOAD_GENERATION_CONFLICT"),
        ("UPLOAD_GENERATION_CONFLICT", "UPLOAD_GENERATION_CONFLICT"),
        ("FENCING_TOKEN_EXPIRED", "LEASE_EXPIRED"),
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
    for action in [
        crate::BackendAction::Repositories,
        crate::BackendAction::Disks,
        crate::BackendAction::Check,
    ] {
        assert_eq!(
            backend.call(action, &caller, json!({})).unwrap_err().code,
            "UNAUTHORIZED"
        );
    }
    assert!(
        backend
            .call(crate::BackendAction::Capabilities, &caller, json!({}))
            .is_ok()
    );
    assert!(
        backend
            .call(crate::BackendAction::Ready, &caller, json!({}))
            .is_ok()
    );
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
    assert!(
        backend
            .call(crate::BackendAction::Repositories, &identity, json!({}))
            .is_ok()
    );
    let stranger = Identity {
        principal: "other".into(),
        ..identity
    };
    assert_eq!(
        backend
            .call(crate::BackendAction::Repositories, &stranger, json!({}))
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
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
            &storage_engine::new_id("op"),
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
        "FORBIDDEN"
    );
    assert_eq!(
        workspace_authorize(&engine, &owner, "missing")
            .unwrap_err()
            .code,
        "WORKSPACE_NOT_FOUND"
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
