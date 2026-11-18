//! Real ordinary FILE branches, rather than a separate directory repository,
//! are the only history link and archive operands.
use super::*;
use std::os::unix::fs::PermissionsExt;

fn op() -> OperationId {
    OperationId::generate().unwrap()
}

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let disk = temp.path().join("primary");
    for path in [&system, &disk] {
        std::fs::create_dir(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = Config::independent_roots(storage_contract::Mode::Development, [
        (name_policy::Name::parse("Primary").unwrap(), disk),
    ]).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repo = engine.create_disk_root(&slot, "admin", op()).unwrap();
    for path in ["one.txt", "two.txt"] {
        let workspace = engine.create_workspace(&repo.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&workspace.workspace_id, path, b"same bytes").unwrap();
        engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id, operation_id: op(), principal: "admin".into(),
            expected_head: workspace.base_head, fencing_token: workspace.fencing_token,
            message: "independent FILE".into(), request_digest: None,
        }).unwrap();
    }
    (temp, engine, repo)
}

#[test]
fn file_relations_refuse_shared_roots_other_files_and_ancestry_without_this_file() {
    let (_temp, mut engine, repo) = fixture();
    let a = engine.namespace_tree(&repo.main_branch_id).unwrap().get("one.txt").unwrap().clone();
    let side = engine.create_file_branch(&a.file_id, &a.version_id,
        &storage_contract::BranchName::parse("side").unwrap(), "admin", op())
        .unwrap().branch_id.unwrap();
    let primary = engine.file_context(&repo.repository_id, "one.txt").unwrap().branch_id;
    let other = engine.file_context(&repo.repository_id, "two.txt").unwrap().branch_id;
    let primary_head = engine.state().branches[&primary].head_commit_id.clone();
    let side_head = engine.state().branches[&side].head_commit_id.clone();
    let other_head = engine.state().branches[&other].head_commit_id.clone();
    let root_head = engine.state().branches[&repo.main_branch_id].head_commit_id.clone();
    let cut = engine.state().histories[&repo.history_tree_id].root_commit_id.clone();
    let before = canonical_json(engine.state()).unwrap();
    for (source, source_commit, target, target_commit, expected) in [
        (&repo.main_branch_id, &root_head, &repo.main_branch_id, &root_head, ErrorCode::FileBranchRequired),
        (&primary, &primary_head, &other, &other_head, ErrorCode::FileBranchScope),
        (&primary, &cut, &side, &side_head, ErrorCode::CommitNotFound),
        (&primary, &primary_head, &side, &primary_head, ErrorCode::MergeEndpointMismatch),
    ] {
        let operation = op();
        assert_eq!(engine.add_merge_link(source, source_commit, target, target_commit, "admin", operation)
            .unwrap_err().code(), expected);
        assert!(!engine.state().operations.contains_key(&operation));
    }
    assert_eq!(engine.archive_branch(&repo.main_branch_id, "admin", op()).unwrap_err().code(),
        ErrorCode::MainBranchRequired);
    assert_eq!(engine.archive_branch(&primary, "admin", op()).unwrap_err().code(),
        ErrorCode::FileBranchSelected);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    let open = engine.create_workspace(&side, "admin").unwrap();
    assert_eq!(engine.archive_branch(&side, "admin", op()).unwrap_err().code(), ErrorCode::WorkspaceBusy);
    engine.abort_workspace(&open.workspace_id, "admin").unwrap();
}

#[test]
fn original_file_link_branch_creation_and_archive_survive_reselection_and_restart() {
    let (temp, mut engine, repo) = fixture();
    let a = engine.namespace_tree(&repo.main_branch_id).unwrap().get("one.txt").unwrap().clone();
    let primary = engine.file_context(&repo.repository_id, "one.txt").unwrap().branch_id;
    let name = storage_contract::BranchName::parse("side").unwrap();
    let create = op();
    let created = engine.create_file_branch(&a.file_id, &a.version_id, &name, "admin", create).unwrap();
    let side = created.branch_id.clone().unwrap();
    let side_head = engine.state().branches[&side].head_commit_id.clone();
    let primary_head = engine.state().branches[&primary].head_commit_id.clone();
    let operation = op();
    let link = engine.add_merge_link(&side, &side_head, &primary, &primary_head, "admin", operation).unwrap();
    assert_eq!(link.source_branch_id, side);
    assert_eq!(link.source_commit_id, side_head);
    assert_eq!(link.target_branch_id, primary);
    assert_eq!(link.target_commit_id, primary_head);
    let path = name_policy::PortablePath::parse("one.txt").unwrap();
    for branch in [&side, &primary] {
        engine.select_file_branch_at(&repo.repository_id, &path, &a.file_id, branch, "admin", op()).unwrap();
    }
    let archive = op();
    let archived = engine.archive_branch(&side, "admin", archive).unwrap();
    assert_eq!(engine.state().branches[&side].state, BranchState::Archived);
    for reopen in [false, true] {
        if reopen { drop(engine); engine = Engine::open(&temp.path().join("system")).unwrap(); }
        let before = canonical_json(engine.state()).unwrap();
        assert_eq!(engine.create_file_branch(&a.file_id, &a.version_id, &name, "admin", create).unwrap(), created);
        let replay = engine.add_merge_link(&side, &side_head, &primary, &primary_head, "admin", operation).unwrap();
        assert_eq!(canonical_json(&replay).unwrap(), canonical_json(&link).unwrap());
        assert_eq!(engine.archive_branch(&side, "admin", archive).unwrap(), archived);
        assert_eq!(engine.create_file_branch(&a.file_id, &a.version_id, &name, "other", create)
            .unwrap_err().code(), ErrorCode::IdempotencyKeyReused);
        assert_eq!(engine.add_merge_link(&side, &side_head, &primary, &side_head, "admin", operation)
            .unwrap_err().code(), ErrorCode::IdempotencyKeyReused);
        assert_eq!(engine.archive_branch(&side, "other", archive).unwrap_err().code(), ErrorCode::IdempotencyKeyReused);
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
    }
}
