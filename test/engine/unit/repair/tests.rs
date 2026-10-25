use super::*;
use std::io::Read;
fn pending() -> (tempfile::TempDir, PathBuf, Config, String, String) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&root, config.clone()).unwrap();
    let repo = engine
        .create_repository("repair", "P01", 3, "admin", &new_id("op"))
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "a", b"verified original bytes")
        .unwrap();
    let operation = new_id("op");
    engine.inject_fault_once("after_accept_2");
    assert_eq!(
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: operation.clone(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: String::new(),
                request_digest: None
            })
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::InDoubt
    );
    drop(engine);
    (temp, root, config, repo.main_branch_id, operation)
}
#[test]
fn recovery_repairs_missing_replicas_without_replacing_the_candidate_or_route() {
    let (_temp, root, config, branch, operation) = pending();
    let histories = config.disks[0].root.join("histories");
    let history = fs::read_dir(&histories)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let version = fs::read_dir(history.join("versions"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let metadata: FileVersion = read_json(&version.join("version.json")).unwrap();
    fs::remove_dir_all(&version).unwrap();
    let backup = version_dir(
        &config.disks[1],
        &metadata.history_tree_id,
        &metadata.version_id,
    );
    fs::remove_file(backup.join("checksums.bin")).unwrap();
    let engine = Engine::open(&root).unwrap();
    assert_eq!(
        engine.operation(&operation).unwrap().commit_state,
        storage_contract::CommitState::Published
    );
    let entries = engine.list_tree(&branch).unwrap();
    assert_eq!(entries[0].version_id, metadata.version_id);
    let mut opened = engine.open_content(&metadata.version_id).unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"verified original bytes");
    assert!(engine.check().unwrap().issues.is_empty());
    assert!(
        fs::read_dir(config.disks[1].root.join("quarantine"))
            .unwrap()
            .next()
            .is_some()
    );
}
#[test]
fn recovery_uses_retained_workspace_bytes_when_all_candidate_replicas_are_lost() {
    let (_temp, root, config, branch, operation) = pending();
    for disk in &config.disks {
        fs::remove_dir_all(disk.root.join("histories")).unwrap();
    }
    let engine = Engine::open(&root).unwrap();
    assert_eq!(
        engine.operation(&operation).unwrap().commit_state,
        storage_contract::CommitState::Published
    );
    assert_eq!(engine.list_tree(&branch).unwrap().len(), 1);
    assert!(engine.check().unwrap().issues.is_empty());
}
