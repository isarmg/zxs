use super::*;
#[test]
fn typed_repository_creation_checks_new_routes_and_exact_permanent_receipts() {
    let (root, engine, _, _) = fixture();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let operation = storage_contract::OperationId::generate().unwrap();
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
        engine.state().operations[&operation].request_digest,
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
    let before = storage_contract::canonical_json(backend.engine.lock().unwrap().state()).unwrap();
    for (slot, copies, code) in [
        ("missing", 3, storage_contract::ErrorCode::SlotNotFound),
        ("P01", 0, storage_contract::ErrorCode::PlacementInvalid),
        ("P01", 4, storage_contract::ErrorCode::BackupRouteIncomplete),
    ] {
        // A new operation validates the route before checking the colliding
        // repository name. It cannot create a receipt for an invalid route.
        let new_operation = storage_contract::OperationId::generate().unwrap();
        assert_eq!(
            backend
                .create_repository(&identity, request("Café", slot, copies, new_operation))
                .unwrap_err()
                .code,
            code
        );
        // Reusing an operation instead validates its immutable actor/request
        // binding first. Replaying a receipt must not depend on its old route
        // still being writable after a later disk exit.
        assert_eq!(
            backend
                .create_repository(&identity, request("Café", slot, copies, operation))
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::IdempotencyKeyReused
        );
    }
    assert_eq!(
        backend
            .create_repository(&identity, request("Café", "P01", 2, operation))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        backend
            .create_repository(
                &identity,
                request(
                    "CAFÉ",
                    "P01",
                    3,
                    storage_contract::OperationId::generate().unwrap()
                )
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::NameCollision
    );
    assert_eq!(
        storage_contract::canonical_json(backend.engine.lock().unwrap().state()).unwrap(),
        before,
        "route, name and idempotency rejection must leave state and permanent receipts unchanged"
    );
    backend
        .update_smb_binding(
            &identity,
            crate::UpdateSmbBinding {
                repository: &created.repository_id,
                branch: &created.main_branch_id,
                enabled: false,
                expected_history_generation: 1,
                operation: storage_contract::OperationId::generate().unwrap(),
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
