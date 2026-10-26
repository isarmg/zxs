use super::*;

#[test]
fn typed_smb_binding_keeps_canonical_receipt_and_distinct_generations() {
    let (root, engine, repo, _) = fixture();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let before = backend
        .smb_binding_status(&identity, &repo.repository_id)
        .unwrap();
    let operation = storage_contract::OperationId::generate();
    let update = |enabled, generation, operation| crate::UpdateSmbBinding {
        repository: &repo.repository_id,
        branch: &repo.main_branch_id,
        enabled,
        expected_history_generation: generation,
        operation,
    };
    let result = backend
        .update_smb_binding(
            &identity,
            update(false, before.history_generation, operation),
        )
        .unwrap();
    let replay = backend
        .update_smb_binding(
            &identity,
            update(false, before.history_generation, operation),
        )
        .unwrap();
    assert_eq!(result, replay);
    let after = backend
        .smb_binding_status(&identity, &repo.repository_id)
        .unwrap();
    assert!(!after.enabled);
    assert_eq!(after.history_generation, before.history_generation + 1);
    assert_eq!(
        backend
            .repository_snapshot(&identity, &repo.repository_id)
            .unwrap()
            .smb_binding_generation,
        repo.smb_binding_generation + 1
    );
    // The old core entrypoint must address the identical permanent receipt.
    let direct = backend
        .engine
        .lock()
        .unwrap()
        .set_smb_binding(
            &repo.repository_id,
            &repo.main_branch_id,
            false,
            before.history_generation,
            &identity.principal,
            operation,
        )
        .unwrap();
    assert_eq!(direct, result);
    let expected_digest = storage_contract::object_hash(
        "request",
        &json!({
            "kind":"smb.binding.update","principal":identity.principal,
            "body":{"repository_id":repo.repository_id,"branch_id":repo.main_branch_id,
                "enabled":false,"expected_generation":before.history_generation.to_string()}
        }),
    )
    .unwrap();
    assert_eq!(result.request_digest, expected_digest);
    assert_eq!(
        backend
            .update_smb_binding(
                &identity,
                update(true, before.history_generation + 2, operation)
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        backend
            .update_smb_binding(
                &identity,
                update(
                    true,
                    before.history_generation + 2,
                    storage_contract::OperationId::generate()
                )
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::ConfigConflict
    );
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .smb_binding_status(&identity, &repo.repository_id)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(
        backend
            .update_smb_binding(
                &identity,
                update(false, before.history_generation, operation)
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn typed_smb_binding_preserves_repository_priority_and_workspace_rejection() {
    let (root, engine, repo, _) = fixture();
    let (backend, _auth, _token, identity) = authenticated_backend(engine, root.path());
    let status = backend
        .smb_binding_status(&identity, &repo.repository_id)
        .unwrap();
    let workspace = backend
        .create_workspace(
            &identity,
            crate::CreateWorkspace {
                branch_id: repo.main_branch_id.clone(),
                source_commit_id: None,
            },
        )
        .unwrap();
    let request = || crate::UpdateSmbBinding {
        repository: &repo.repository_id,
        branch: &repo.main_branch_id,
        enabled: false,
        expected_history_generation: status.history_generation,
        operation: storage_contract::OperationId::generate(),
    };
    assert_eq!(
        backend
            .update_smb_binding(&identity, request())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::WorkspaceBusy
    );
    backend
        .seal_workspace(&identity, &workspace.workspace_id)
        .unwrap();
    assert_eq!(
        backend
            .update_smb_binding(&identity, request())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::WorkspaceBusy
    );
    backend
        .abort_workspace(&identity, &workspace.workspace_id)
        .unwrap();
    let operation = storage_contract::OperationId::generate();
    backend
        .update_smb_binding(
            &identity,
            crate::UpdateSmbBinding {
                operation,
                ..request()
            },
        )
        .unwrap();
    let error = backend
        .update_smb_binding(
            &identity,
            crate::UpdateSmbBinding {
                repository: "missing",
                operation,
                ..request()
            },
        )
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(error.message, "repository not found");
}
