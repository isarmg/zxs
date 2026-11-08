//! The unique-occurrence boundary is checked before any target preparation.
use super::*;

#[test]
fn a_shared_file_cannot_be_captured_as_a_single_path_move() {
    let temp = tempfile::tempdir().unwrap();
    let mut config =
        Config::development([temp.path().join("source"), temp.path().join("target")]).unwrap();
    config.disks[1].level = 1;
    config.disks[1].upstream_slot_id = None;
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let operation = || OperationId::generate().unwrap();
    let source = engine.create_disk_root("P01", "admin", operation()).unwrap();
    let target = engine.create_disk_root("B01", "admin", operation()).unwrap();
    let workspace = engine
        .create_workspace(&source.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "source.txt", b"source")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "move occurrence boundary".into(),
            request_digest: None,
        })
        .unwrap();
    let head = engine.state().branches[&source.main_branch_id]
        .head_commit_id
        .clone();
    let file = engine
        .namespace_tree(&source.main_branch_id)
        .unwrap()
        .get("source.txt")
        .unwrap()
        .clone();
    // A small State fixture for this admission boundary, not a claim that
    // mutation of an immutable commit is a supported publication operation.
    let mut second = file.clone();
    second.entry_id = crate::new_id("entry").unwrap();
    second.path = name_policy::PortablePath::parse("second.txt").unwrap();
    let commit = Arc::make_mut(&mut engine.state)
        .commits
        .get_mut(&head)
        .unwrap();
    commit.entries.push(second);
    commit
        .entries
        .sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
    let namespace = FileHistoryCopyRequest {
        source_repository_id: source.repository_id,
        source_path: "source.txt".into(),
        expected_source_head: head,
        target_repository_id: target.repository_id,
        target_name: "moved.txt".into(),
        expected_target_head: engine.state().branches[&target.main_branch_id]
            .head_commit_id
            .clone(),
    };
    let sequence = engine.state().sequence;
    let error = engine
        .capture_file_history_move(
            FileHistoryMoveRequest {
                namespace: &namespace,
                source_file_id: &HistoryFileId::parse(&file.file_id).unwrap(),
            },
            &HistoryWindowCancellation::new(),
        )
        .map(|_| ())
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::FileBranchScope);
    assert_eq!(engine.state().sequence, sequence);
    assert!(engine.file_deletion(&file.file_id).is_none());
    assert!(
        engine
            .namespace_tree(&target.main_branch_id)
            .unwrap()
            .entries()
            .is_empty()
    );
}
