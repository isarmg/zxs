use super::*;

fn backend() -> (
    tempfile::TempDir,
    EngineBackend,
    Identity,
    storage_contract::Repository,
) {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        storage_engine::Config::development([root.path().join("disk")]).unwrap(),
    )
    .unwrap();
    let identity = install_test_administrator(&mut engine, "owner");
    let repository = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Locked admission").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    (root, EngineBackend::new(engine), identity, repository)
}

#[test]
fn admitted_access_holds_the_owner_lock_and_rechecks_replaced_credentials() {
    let (_root, backend, owner, _) = backend();
    let (mut engine, current) = backend.lock_current(&owner).unwrap();
    assert_eq!(current.principal, owner.principal);
    assert!(matches!(
        backend.engine.try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    ));
    let replacement = install_test_administrator(&mut engine, "replacement");
    drop(engine);

    let error = backend.lock_current(&owner).map(|_| ()).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::Unauthorized);
    assert!(
        backend.engine.try_lock().is_ok(),
        "rejection must release the acquired lock"
    );
    let (engine, current) = backend.lock_current(&replacement).unwrap();
    assert_eq!(current.principal, replacement.principal);
    assert!(matches!(
        backend.engine.try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    ));
    drop(engine);
    assert!(backend.engine.try_lock().is_ok());
}

#[test]
fn admitted_access_rechecks_the_exact_smb_binding_on_reacquisition() {
    let (_root, backend, identity, repository) = backend();
    let bound = Identity {
        smb_binding: Some(crate::auth::SmbBinding {
            repository_id: repository.repository_id.clone(),
            branch_id: repository.main_branch_id.clone(),
            generation: repository.smb_binding_generation,
        }),
        ..identity
    };
    let (mut engine, current) = backend.lock_current(&bound).unwrap();
    assert_eq!(current.smb_binding, bound.smb_binding);
    let generation = engine.state().histories[&repository.history_tree_id].generation;
    engine
        .set_smb_binding(
            &repository.repository_id,
            &repository.main_branch_id,
            true,
            generation,
            &current.principal,
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    drop(engine);

    let error = backend.lock_current(&bound).map(|_| ()).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::SmbRebindRequired);
    assert_eq!(error.status, 409);
    assert!(backend.engine.try_lock().is_ok());
}
