//! Public engine baseline, not first-account/helper startup or FILE-local Draft support.
use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
};
use storage_contract::{BranchName, CommitState, ErrorCode, Mode, OperationId, WorkspaceState};
use storage_engine::{CommitRequest, Config, Engine, Entry, ValidatedConfig, Workspace};

fn request(workspace: &Workspace, message: &str) -> CommitRequest {
    CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: message.into(),
        request_digest: None,
    }
}

fn entry(engine: &Engine, branch: &str) -> Entry {
    let entries = engine.list_tree(branch).unwrap();
    assert_eq!(entries.len(), 1);
    entries.into_iter().next().unwrap()
}

fn body(engine: &Engine, version: &str) -> Vec<u8> {
    let mut opened = engine.open_content(version).unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    bytes
}

fn staged_body(engine: &Engine, workspace: &str, item: &str) -> Vec<u8> {
    let mut opened = engine.open_workspace_content(workspace, item, "admin").unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn empty_held_pool_stages_reopens_commits_and_selects_real_file_history() {
    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let disk = temporary.path().join("primary");
    for path in [&system, &disk] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(fs::read_dir(path).unwrap().count(), 0);
    }
    let identity = fs::metadata(&disk).unwrap();
    let selected = fs::File::open(&disk).unwrap();
    let config = Config::independent_roots(Mode::Development, [disk.clone()]).unwrap();
    let registration = config.disks[0].clone();
    let mut engine = Engine::initialize_held(
        &system,
        ValidatedConfig::try_from(config).unwrap(),
        vec![selected],
    )
    .unwrap();
    assert_eq!(engine.state().sequence, 0);
    assert!(engine.state().versions.is_empty());
    drop(engine);
    engine = Engine::open(&system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.state().sequence, 0);
    assert_eq!(engine.config().disks[0].disk_id, registration.disk_id);
    assert_eq!(engine.config().disks[0].binding_generation, registration.binding_generation);
    let reopened = fs::metadata(&disk).unwrap();
    assert_eq!((reopened.dev(), reopened.ino()), (identity.dev(), identity.ino()));

    let repository = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Fresh files").unwrap(),
            &registration.slot_id,
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let first_workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&first_workspace.workspace_id, "notes.txt", b"first body").unwrap();
    let first_request = request(&first_workspace, "first explicit save");
    let first_receipt = engine.commit_workspace(first_request.clone()).unwrap();
    assert_eq!(first_receipt.commit_state, CommitState::Published);
    let first = entry(&engine, &repository.main_branch_id);
    let first_version = engine.state().versions[&first.version_id].clone();
    assert_eq!(body(&engine, &first.version_id), b"first body");

    let branch = engine
        .create_file_branch(
            &first.file_id,
            &first.version_id,
            &BranchName::parse("edited").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let original_branch = engine.state().file_selections[&first.file_id].clone();
    assert_ne!(branch, original_branch);
    assert_eq!(engine.state().versions.len(), 1);
    assert_eq!(engine.file_branch_head(&branch).unwrap(), first.version_id);
    engine
        .select_file_branch(&first.file_id, &branch, "admin", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine.create_workspace(&branch, "admin").unwrap();
    let before_staging = storage_engine::canonical_json(engine.state()).unwrap();
    let staged = engine
        .stage_bytes(&workspace.workspace_id, "notes.txt", b"second body, explicitly committed")
        .unwrap();
    let commit = request(&workspace, "explicit save after restart");
    assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_staging);
    assert_eq!(engine.file_branch_head(&branch).unwrap(), first.version_id);
    assert_eq!(body(&engine, &first.version_id), b"first body");

    // This is the existing leased Workspace contract. A restart must not turn
    // upload completion into a history commit or discard the explicit input.
    drop(engine);
    engine = Engine::open(&system).unwrap();
    let recovered = engine.workspace(&workspace.workspace_id).unwrap();
    assert_eq!(recovered.state, WorkspaceState::Open);
    assert_eq!(recovered.base_head, workspace.base_head);
    assert_eq!(recovered.fencing_token, workspace.fencing_token);
    assert_eq!(
        storage_engine::canonical_json(&recovered.files[&staged.item_id]).unwrap(),
        storage_engine::canonical_json(&staged).unwrap(),
    );
    assert_eq!(engine.file_branch_head(&branch).unwrap(), first.version_id);
    assert_eq!(engine.state().versions.len(), 1);
    assert_eq!(
        staged_body(&engine, &workspace.workspace_id, &staged.item_id),
        b"second body, explicitly committed",
    );
    let before_commit = storage_engine::canonical_json(engine.state()).unwrap();
    let mut wrong_actor = commit.clone();
    wrong_actor.principal = "another actor".into();
    assert_eq!(
        engine.commit_workspace(wrong_actor).unwrap_err().code(),
        ErrorCode::PermissionDenied,
    );
    assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_commit);
    assert!(!engine.state().operations.contains_key(&commit.operation_id));

    let receipt = engine.commit_workspace(commit.clone()).unwrap();
    assert_eq!(receipt.commit_state, CommitState::Published);
    let second = entry(&engine, &repository.main_branch_id);
    assert_eq!(second.file_id, first.file_id);
    assert_eq!(second.entry_id, first.entry_id);
    assert_ne!(second.version_id, first.version_id);
    assert_eq!(engine.state().versions.len(), 2);
    assert_eq!(engine.state().versions[&first.version_id], first_version);
    assert_eq!(engine.file_branch_head(&branch).unwrap(), second.version_id);
    assert_eq!(
        engine.file_node_version(&first.file_id, &second.version_id).unwrap(),
        second.version_id,
    );
    assert_eq!(
        engine.file_node_parents(&first.file_id, &second.version_id).unwrap(),
        vec![first.version_id.clone()],
    );
    for version in [&first.version_id, &second.version_id] {
        let version_row = &engine.state().versions[version];
        assert_eq!(version_row.file_id, first.file_id);
        assert_eq!(version_row.history_tree_id, repository.history_tree_id);
        assert_eq!(version_row.creation_primary_slot_id, registration.slot_id);
        let placement = &engine.state().placements[version];
        assert_eq!(placement.primary_slot_id, registration.slot_id);
        assert_eq!(placement.route_slot_ids, vec![registration.slot_id.clone()]);
        assert_eq!(
            placement.binding_generations[&registration.slot_id],
            registration.binding_generation,
        );
    }
    assert_eq!(body(&engine, &first.version_id), b"first body");
    assert_eq!(body(&engine, &second.version_id), b"second body, explicitly committed");

    let select_operation = OperationId::generate().unwrap();
    let selection = engine
        .select_file_branch(&first.file_id, &original_branch, "admin", select_operation)
        .unwrap();
    assert_eq!(entry(&engine, &repository.main_branch_id).version_id, first.version_id);
    assert_eq!(engine.state().versions.len(), 2);
    drop(engine);
    engine = Engine::open(&system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(entry(&engine, &repository.main_branch_id).version_id, first.version_id);
    assert_eq!(engine.file_branch_head(&branch).unwrap(), second.version_id);
    assert_eq!(body(&engine, &first.version_id), b"first body");
    assert_eq!(body(&engine, &second.version_id), b"second body, explicitly committed");
    let before_retry = storage_engine::canonical_json(engine.state()).unwrap();
    assert_eq!(engine.commit_workspace(commit.clone()).unwrap(), receipt);
    assert_eq!(engine.commit_workspace(first_request).unwrap(), first_receipt);
    assert_eq!(
        engine
            .select_file_branch(&first.file_id, &original_branch, "admin", select_operation)
            .unwrap(),
        selection,
    );
    assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_retry);
    let mut changed_request = commit;
    changed_request.message = "different request under the same operation".into();
    assert_eq!(
        engine.commit_workspace(changed_request).unwrap_err().code(),
        ErrorCode::IdempotencyKeyReused,
    );
    assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_retry);
}
