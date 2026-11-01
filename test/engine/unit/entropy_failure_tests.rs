use super::*;

fn assert_entropy_error(error: &Error) {
    assert!(
        matches!(
            error,
            Error::Contract(storage_contract::Error::IdentifierGeneration { source, .. })
                if source.raw_os_error() == Some(5)
        ),
        "expected typed EIO entropy source, got {error:?}"
    );
    assert!(std::error::Error::source(error).is_some());
    assert_eq!(error.code(), storage_contract::ErrorCode::InternalError);
}

#[test]
#[ignore = "requires test/contract/entropy/entropy-io.sh getrandom fault boundary"]
fn entropy_fault_preserves_unpublished_state_and_workspace() {
    let marker = std::path::PathBuf::from(
        std::env::var_os("ZXS_TEST_ENTROPY_FAIL_FILE")
            .expect("entropy-io.sh supplies the isolated fault marker"),
    );
    assert!(
        !marker.exists(),
        "fault must be disabled during fixture setup"
    );
    // Keep real pool evidence under the driver's directory on failure; only
    // the successful shell driver removes this tree.
    let directory = marker.parent().unwrap();
    let root = directory.join("state");
    let config = Config::development([
        directory.join("primary"),
        directory.join("backup"),
        directory.join("tertiary"),
    ])
    .unwrap();
    let mut engine = Engine::initialize(&root, config).unwrap();
    let operation = OperationId::generate().unwrap();
    let name = storage_contract::RepositoryName::parse("entropy-boundary").unwrap();
    let before = canonical_json(engine.state()).unwrap();
    let snapshot = engine.snapshot.clone();
    let next_slot = engine.next_slot;

    fs::write(&marker, b"fail").unwrap();
    let error = engine
        .create_repository(&name, "P01", 3, "admin", operation)
        .unwrap_err();
    fs::remove_file(&marker).unwrap();
    assert_entropy_error(&error);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(engine.snapshot, snapshot);
    assert_eq!(engine.next_slot, next_slot);
    assert!(!engine.blocked);
    assert!(engine.operation_owner(operation).unwrap().is_none());

    // Reopen the real pool: the rejected planner did not publish disk authority.
    drop(engine);
    let mut engine = Engine::open(&root).unwrap();
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert!(engine.operation_owner(operation).unwrap().is_none());
    let repository = engine
        .create_repository(&name, "P01", 3, "admin", operation)
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    let local_record = root
        .join("workspaces")
        .join(&workspace.workspace_id)
        .join("workspace.json");
    let primary_record = engine
        .workspace_data_directory(&workspace)
        .unwrap()
        .join("workspace.json");
    let local_before = fs::read(&local_record).unwrap();
    let primary_before = fs::read(&primary_record).unwrap();
    let published_before = canonical_json(engine.state()).unwrap();

    fs::write(&marker, b"fail").unwrap();
    // An already-published idempotent request consumes its existing identity.
    let replay = engine
        .create_repository(&name, "P01", 3, "admin", operation)
        .unwrap();
    assert_eq!(replay.repository_id, repository.repository_id);
    let error = engine
        .seal_workspace(&workspace.workspace_id, "admin")
        .unwrap_err();
    fs::remove_file(&marker).unwrap();
    assert_entropy_error(&error);
    assert_eq!(
        engine.workspace(&workspace.workspace_id).unwrap().state,
        WorkspaceState::Open
    );
    assert_eq!(fs::read(&local_record).unwrap(), local_before);
    assert_eq!(fs::read(&primary_record).unwrap(), primary_before);
    assert_eq!(canonical_json(engine.state()).unwrap(), published_before);
    assert!(!engine.blocked);

    // The process and workspace remain usable when the real entropy source resumes.
    engine
        .seal_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    assert_eq!(
        engine.workspace(&workspace.workspace_id).unwrap().state,
        WorkspaceState::Sealed
    );
    println!("\nZXS_ENTROPY_FAULT_COMPLETE");
}
