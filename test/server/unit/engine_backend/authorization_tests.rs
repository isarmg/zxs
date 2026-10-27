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
    for result in [
        backend
            .prepare_historical_save(&caller, Err(ApiError::invalid("unknown field surprise")))
            .map(|_| ()),
        backend
            .prepare_purge(&caller, Err(ApiError::invalid("unknown field surprise")))
            .map(|_| ()),
        backend
            .prepare_split(&caller, Err(ApiError::invalid("unknown field surprise")))
            .map(|_| ()),
        backend
            .prepare_historical_save(
                &caller,
                Ok(crate::PrepareHistoricalSave {
                    history_tree_id: "missing".into(),
                    options: Err(ApiError::invalid("missing options")),
                }),
            )
            .map(|_| ()),
        backend
            .prepare_purge(
                &caller,
                Ok(crate::PreparePurge {
                    history_tree_id: "missing".into(),
                    version_ids: Err(ApiError::invalid("missing versions")),
                }),
            )
            .map(|_| ()),
        backend
            .prepare_split(
                &caller,
                Ok(crate::PrepareSplit {
                    history_tree_id: "missing".into(),
                    cut_commit_id: Err(ApiError::invalid("missing cut")),
                }),
            )
            .map(|_| ()),
        backend.register_disk(&caller).map(|never| match never {}),
        backend.plan(&caller, "missing").map(|_| ()),
        backend
            .commit_plan(&caller, Err(ApiError::invalid("missing fields")))
            .map(|_| ()),
        backend
            .cancel_plan(&caller, Err(ApiError::invalid("unknown field")))
            .map(|_| ()),
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
            &storage_contract::RepositoryName::parse("Borrowed workspace").unwrap(),
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
            &storage_contract::RepositoryName::parse("Upload admission").unwrap(),
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
            &storage_contract::RepositoryName::parse("Status").unwrap(),
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

#[test]
fn typed_plan_cancel_keeps_owner_and_receipt_ordering() {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        storage_engine::Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let identity = install_test_administrator(&mut engine, "admin");
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Plans").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let source = engine
        .state()
        .branches
        .get(&repo.main_branch_id)
        .unwrap()
        .head_commit_id
        .clone();
    let options = |name: &str| storage_engine::HistoricalSaveOptions {
        source_commit_id: source.clone(),
        name: name.into(),
        workspace_id: None,
        primary_slot_id: None,
        target_copies: None,
    };
    let own = engine
        .create_historical_save_plan(&repo.history_tree_id, options("Own"), "admin")
        .unwrap();
    let foreign = engine
        .create_historical_save_plan(&repo.history_tree_id, options("Foreign"), "other")
        .unwrap();
    let second = engine
        .create_historical_save_plan(&repo.history_tree_id, options("Second"), "admin")
        .unwrap();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Generation advance").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert!(engine.state().business_generation > own.expected_sequence);
    let backend = EngineBackend::new(engine);
    assert_eq!(backend.plan(&identity, &own.plan_id).unwrap(), own);
    assert_eq!(
        backend.plan(&identity, &foreign.plan_id).unwrap_err().code,
        storage_contract::ErrorCode::Forbidden
    );
    let rejected_operation = || Err(ApiError::invalid("missing string field operation_id"));
    assert_eq!(
        backend
            .cancel_plan(
                &identity,
                Ok(crate::CancelPlan {
                    plan_id: foreign.plan_id.clone(),
                    operation_id: rejected_operation()
                })
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Forbidden
    );
    assert_eq!(
        backend
            .cancel_plan(
                &identity,
                Ok(crate::CancelPlan {
                    plan_id: own.plan_id.clone(),
                    operation_id: rejected_operation()
                })
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::InvalidRequest
    );
    for (plan, digest, expected) in [
        (&foreign, "wrong", storage_contract::ErrorCode::Forbidden),
        (&own, "wrong", storage_contract::ErrorCode::PlanStale),
        (
            &own,
            own.digest.as_str(),
            storage_contract::ErrorCode::InvalidRequest,
        ),
    ] {
        assert_eq!(
            backend
                .commit_plan(
                    &identity,
                    Ok(crate::CommitPlan {
                        plan_id: plan.plan_id.clone(),
                        plan_digest: digest.into(),
                        operation_id: rejected_operation(),
                    })
                )
                .unwrap_err()
                .code,
            expected
        );
    }
    assert_eq!(
        backend
            .commit_plan(
                &identity,
                Ok(crate::CommitPlan {
                    plan_id: own.plan_id.clone(),
                    plan_digest: own.digest.clone(),
                    operation_id: Ok(storage_contract::OperationId::generate()),
                })
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::PlanStale
    );
    let operation = storage_contract::OperationId::generate();
    let request = || {
        Ok(crate::CancelPlan {
            plan_id: own.plan_id.clone(),
            operation_id: Ok(operation),
        })
    };
    let result = backend.cancel_plan(&identity, request()).unwrap();
    assert_eq!(backend.cancel_plan(&identity, request()).unwrap(), result);
    assert_eq!(
        backend
            .cancel_plan(
                &identity,
                Ok(crate::CancelPlan {
                    plan_id: second.plan_id,
                    operation_id: Ok(operation)
                })
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::IdempotencyKeyReused
    );

    assert_eq!(
        backend.plan(&identity, &own.plan_id).unwrap().state,
        storage_contract::PlanState::Cancelled
    );
    assert_eq!(
        backend
            .cancel_plan(
                &identity,
                Ok(crate::CancelPlan {
                    plan_id: foreign.plan_id,
                    operation_id: Ok(operation)
                })
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Forbidden
    );
    assert_eq!(
        backend
            .cancel_plan(
                &identity,
                Ok(crate::CancelPlan {
                    plan_id: own.plan_id,
                    operation_id: Ok(storage_contract::OperationId::generate())
                })
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::PlanClosed
    );
}

#[test]
fn typed_plan_disk_integrity_precedes_cancel_operation_errors_after_reopen() {
    let root = crate::tests::private_tempdir();
    let system = root.path().join("system");
    let mut engine = Engine::initialize(
        &system,
        storage_engine::Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let identity = install_test_administrator(&mut engine, "admin");
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Disk plans").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let source = engine
        .state()
        .branches
        .get(&repo.main_branch_id)
        .unwrap()
        .head_commit_id
        .clone();
    let plan = engine
        .create_historical_save_plan(
            &repo.history_tree_id,
            storage_engine::HistoricalSaveOptions {
                source_commit_id: source,
                name: "Prepared".into(),
                workspace_id: None,
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    drop(engine);
    let path = system.join("plans").join(format!("{}.json", plan.plan_id));
    let mut altered_input = plan.clone();
    altered_input.input["history_tree_id"] = storage_engine::new_id("history").into();
    let mut altered_identity = plan.clone();
    altered_identity.plan_id = storage_engine::new_id("plan");
    let mut missing_history = altered_input.clone();
    missing_history.digest = storage_contract::object_hash("plan", &missing_history.input).unwrap();
    for (altered, expected) in [
        (altered_input, storage_contract::ErrorCode::PlanInvalid),
        (altered_identity, storage_contract::ErrorCode::PlanInvalid),
        (missing_history, storage_contract::ErrorCode::NotFound),
    ] {
        std::fs::write(&path, json_codec::to_vec(&altered).unwrap()).unwrap();
        let backend = EngineBackend::new(Engine::open(&system).unwrap());
        assert_eq!(
            backend.plan(&identity, &plan.plan_id).unwrap_err().code,
            expected
        );
        assert_eq!(
            backend
                .cancel_plan(
                    &identity,
                    Ok(crate::CancelPlan {
                        plan_id: plan.plan_id.clone(),
                        operation_id: Err(ApiError::invalid("bad operation"))
                    })
                )
                .unwrap_err()
                .code,
            expected
        );
        assert_eq!(
            backend
                .commit_plan(
                    &identity,
                    Ok(crate::CommitPlan {
                        plan_id: plan.plan_id.clone(),
                        plan_digest: altered.digest.clone(),
                        operation_id: Err(ApiError::invalid("bad operation")),
                    })
                )
                .unwrap_err()
                .code,
            expected
        );
        assert_eq!(
            backend
                .cancel_plan(&identity, Err(ApiError::invalid("unknown field")))
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::InvalidRequest
        );
    }
}
