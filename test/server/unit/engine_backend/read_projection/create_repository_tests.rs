use super::*;
#[test]
fn typed_repository_creation_keeps_route_before_receipt_and_original_binding() {
    let (root, engine, _, _) = fixture();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let operation = storage_contract::OperationId::generate();
    let request = |name: &str, slot: &str, copies, operation_id| crate::CreateRepository {
        name: storage_contract::RepositoryName::parse(name).unwrap(),
        primary_slot_id: slot.into(),
        target_copies: copies,
        operation_id,
    };
    let created = backend
        .create_repository(&identity, request("Café", "P01", 3, operation))
        .unwrap();
    let expected=storage_contract::object_hash("request",&json!({"kind":"repository.create","principal":identity.principal,"body":{"name":"Café","primary_slot_id":"P01","target_copies":3}})).unwrap();
    let mut engine = backend.engine.lock().unwrap();
    assert_eq!(
        engine
            .state()
            .operations
            .tail_record(operation)
            .unwrap()
            .result
            .request_digest,
        expected
    );
    assert_eq!(
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Café").unwrap(),
                "P01",
                3,
                &identity.principal,
                operation
            )
            .unwrap(),
        created
    );
    drop(engine);
    for (slot, copies, code) in [
        ("missing", 3, storage_contract::ErrorCode::SlotNotFound),
        ("P01", 0, storage_contract::ErrorCode::PlacementInvalid),
        ("P01", 4, storage_contract::ErrorCode::BackupRouteIncomplete),
        ("P01", 2, storage_contract::ErrorCode::IdempotencyKeyReused),
    ] {
        assert_eq!(
            backend
                .create_repository(&identity, request("Café", slot, copies, operation))
                .unwrap_err()
                .code,
            code
        );
    }
    assert_eq!(
        backend
            .create_repository(
                &identity,
                request("CAFÉ", "P01", 3, storage_contract::OperationId::generate())
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::NameCollision
    );
    backend
        .update_smb_binding(
            &identity,
            crate::UpdateSmbBinding {
                repository: &created.repository_id,
                branch: &created.main_branch_id,
                enabled: false,
                expected_history_generation: 1,
                operation: storage_contract::OperationId::generate(),
            },
        )
        .unwrap();
    assert_eq!(
        backend
            .create_repository(&identity, request("Café", "P01", 3, operation))
            .unwrap(),
        created
    );
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .create_repository(&identity, request("Café", "missing", 0, operation))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}
