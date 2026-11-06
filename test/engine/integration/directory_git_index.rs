//! Public-engine regressions for explicit index snapshots and live working files.
use std::io::Read;
use storage_engine::{
    CommitRequest, Config, DirectoryCommitRequest, Engine, ErrorCode, OperationId, OperationResult,
    Repository, Result, directory_git::*,
};

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([
            temp.path().join("p"),
            temp.path().join("b"),
            temp.path().join("c"),
        ])
        .unwrap(),
    )
    .unwrap();
    let repo = engine
        .create_disk_root("P01", "admin", OperationId::generate().unwrap())
        .unwrap();
    (temp, engine, repo)
}
fn save(engine: &mut Engine, repo: &Repository, path: &str, bytes: &[u8]) {
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "save file".into(),
            request_digest: None,
        })
        .unwrap();
}

fn status(engine: &Engine, repo: &Repository, path: &str) -> DirectoryGitStatus {
    engine
        .directory_git_status(&repo.repository_id, path)
        .unwrap()
}
fn initialize(engine: &mut Engine, repo: &Repository, path: &str) {
    let head = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    engine
        .initialize_directory_git(
            &repo.repository_id,
            &DirectoryGitInitializeBody {
                path: path.into(),
                expected_namespace_head: head,
                operation_id: OperationId::generate().unwrap(),
            },
            "admin",
        )
        .unwrap();
}
fn stage_owned(engine: &mut Engine, repo: &Repository, path: &str) {
    let s = status(engine, repo, path);
    engine
        .stage_directory_git_owned(
            &repo.repository_id,
            &DirectoryGitStageOwnedBody {
                directory_id: s.directory_id,
                directory_branch_id: s.directory_branch_id.unwrap(),
                expected_tree_id: s.working_tree_id.unwrap(),
                expected_index_generation: s.index_generation,
                operation_id: OperationId::generate().unwrap(),
            },
            "admin",
        )
        .unwrap();
}
fn commit_reviewed(
    engine: &mut Engine,
    repo: &Repository,
    s: &DirectoryGitStatus,
    operation: OperationId,
) -> Result<OperationResult> {
    engine.commit_directory_changes(DirectoryCommitRequest {
        repository_id: &repo.repository_id,
        directory_branch_id: s.directory_branch_id.as_ref().unwrap(),
        expected_tree_id: s.working_tree_id.as_ref().unwrap(),
        expected_index_generation: s.index_generation,
        message: "Review and commit directory",
        principal: "admin",
        operation_id: operation,
    })
}
// Fixture checkpoints explicitly stage first; index-isolation tests use commit_reviewed.
fn commit(engine: &mut Engine, repo: &Repository, path: &str) -> String {
    stage_owned(engine, repo, path);
    let s = status(engine, repo, path);
    commit_reviewed(engine, repo, &s, OperationId::generate().unwrap())
        .unwrap()
        .result["head_tree_id"]
        .as_str()
        .unwrap()
        .into()
}
fn stage_child(engine: &mut Engine, repo: &Repository, parent: &str, child: &str) {
    let p = status(engine, repo, parent);
    let c = status(engine, repo, child);
    engine
        .stage_directory_git_child(
            &repo.repository_id,
            &DirectoryGitStageChildBody {
                directory_id: p.directory_id,
                directory_branch_id: p.directory_branch_id.unwrap(),
                expected_index_generation: p.index_generation,
                child_directory_id: c.directory_id,
                expected_child_head_tree_id: c.head_tree_id.unwrap(),
                operation_id: OperationId::generate().unwrap(),
            },
            "admin",
        )
        .unwrap();
}
fn bytes(engine: &Engine, repo: &Repository, path: &str) -> Vec<u8> {
    let entry = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path() == path)
        .unwrap();
    version_bytes(engine, &entry.version_id)
}
fn version_bytes(engine: &Engine, version: &str) -> Vec<u8> {
    let mut result = Vec::new();
    engine
        .open_content(version)
        .unwrap()
        .file
        .read_to_end(&mut result)
        .unwrap();
    result
}

fn remove(
    engine: &mut Engine,
    repo: &Repository,
    path: &str,
    kind: storage_contract::NamespaceOperationKind,
) {
    engine
        .file_namespace(
            &repo.repository_id,
            storage_contract::NamespaceOperation::parse(kind, path, None).unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
}

#[test]
fn explicit_commit_freezes_staged_contents_and_preserves_later_work_across_restart() {
    let (temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "file", b"base");
    initialize(&mut engine, &repo, "");
    commit(&mut engine, &repo, "");
    save(&mut engine, &repo, "file", b"staged");
    stage_owned(&mut engine, &repo, "");
    let reviewed = status(&engine, &repo, "");
    let staged_tree = reviewed.working_tree_id.clone().unwrap();
    let staged_entries = engine.state().directory_trees[&staged_tree].entries.clone();
    save(&mut engine, &repo, "file", b"later working content");
    save(&mut engine, &repo, "not-staged", b"new working file");
    let ready = status(&engine, &repo, "");
    assert!(ready.staged && ready.unstaged);
    assert_eq!((ready.added, ready.modified, ready.deleted), (0, 1, 0));
    assert_eq!(ready.index_generation, reviewed.index_generation);
    let operation = OperationId::generate().unwrap();
    let result = commit_reviewed(&mut engine, &repo, &ready, operation).unwrap();
    let head = result.result["head_tree_id"].as_str().unwrap();
    assert_eq!(engine.state().directory_trees[head].entries, staged_entries);
    assert_eq!(bytes(&engine, &repo, "file"), b"later working content");
    assert_eq!(bytes(&engine, &repo, "not-staged"), b"new working file");
    let after = status(&engine, &repo, "");
    assert!(!after.staged && after.unstaged);
    assert_eq!((after.added, after.modified, after.deleted), (0, 0, 0));
    assert_eq!(
        engine.state().directory_git_repositories[&after.directory_id].branches
            [after.directory_branch_id.as_ref().unwrap()]
        .index_tree_id,
        staged_tree
    );
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(status(&engine, &repo, ""), after);
    assert_eq!(
        commit_reviewed(&mut engine, &repo, &ready, operation).unwrap(),
        result
    );
    assert_eq!(bytes(&engine, &repo, "file"), b"later working content");
    let next = commit(&mut engine, &repo, "");
    assert_ne!(next, head);
    assert!(!status(&engine, &repo, "").unstaged);
}

#[test]
fn unstaged_work_cannot_be_committed_or_discarded_by_branch_operations() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "file", b"base");
    initialize(&mut engine, &repo, "");
    let head = commit(&mut engine, &repo, "");
    let topic = engine
        .create_directory_branches(
            &repo.repository_id,
            None,
            &storage_contract::BranchName::parse("topic").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    save(&mut engine, &repo, "file", b"unstaged");
    let ready = status(&engine, &repo, "");
    assert!(ready.unstaged && !ready.staged);
    let result =
        commit_reviewed(&mut engine, &repo, &ready, OperationId::generate().unwrap()).unwrap();
    assert_eq!(result.result["no_change"], true);
    assert_eq!(result.result["head_tree_id"], head);
    assert_eq!(
        engine
            .create_directory_branches(
                &repo.repository_id,
                None,
                &storage_contract::BranchName::parse("another").unwrap(),
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryBranchConflict
    );
    assert_eq!(
        engine
            .select_default_directory_branch(
                &repo.repository_id,
                &topic,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryBranchConflict
    );
    assert_eq!(bytes(&engine, &repo, "file"), b"unstaged");
}

#[test]
fn restaging_a_renamed_child_mount_commits_its_path_without_advancing_its_pin() {
    let (temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "child/file", b"child");
    initialize(&mut engine, &repo, "child");
    let child_head = commit(&mut engine, &repo, "child");
    initialize(&mut engine, &repo, "");
    stage_child(&mut engine, &repo, "", "child");
    let parent_head = commit(&mut engine, &repo, "");
    engine
        .file_namespace(
            &repo.repository_id,
            storage_contract::NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "child",
                Some("renamed"),
            )
            .unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert!(status(&engine, &repo, "").needs_restage);
    stage_owned(&mut engine, &repo, "");
    let ready = status(&engine, &repo, "");
    assert!(ready.staged && ready.children[0].staged);
    assert!(!ready.needs_restage && !ready.unstaged);
    assert!(!ready.children[0].reference_changed);
    let result =
        commit_reviewed(&mut engine, &repo, &ready, OperationId::generate().unwrap()).unwrap();
    let head = result.result["head_tree_id"].as_str().unwrap();
    assert_ne!(head, parent_head);
    assert_eq!(
        engine.state().directory_trees[head].directory_refs[&ready.children[0].directory_id]
            .tree_id,
        child_head
    );
    assert!(
        engine.state().directory_trees[head]
            .entries
            .iter()
            .any(|entry| entry.path() == "renamed/file")
    );
    assert!(!status(&engine, &repo, "").staged);
    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(bytes(&engine, &repo, "renamed/file"), b"child");
    assert!(!status(&engine, &repo, "").staged);
}

#[test]
fn staging_a_child_reference_does_not_stage_the_parents_working_edits() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "own", b"parent base");
    save(&mut engine, &repo, "child/file", b"child base");
    initialize(&mut engine, &repo, "child");
    commit(&mut engine, &repo, "child");
    initialize(&mut engine, &repo, "");
    stage_child(&mut engine, &repo, "", "child");
    let first = commit(&mut engine, &repo, "");
    let owned = engine.state().directory_trees[&first]
        .entries
        .iter()
        .find(|entry| entry.path() == "own")
        .unwrap()
        .clone();
    save(&mut engine, &repo, "own", b"parent unstaged");
    save(&mut engine, &repo, "child/file", b"new child commit");
    let child_head = commit(&mut engine, &repo, "child");
    stage_child(&mut engine, &repo, "", "child");
    let ready = status(&engine, &repo, "");
    assert!(ready.staged && ready.unstaged);
    assert_eq!((ready.added, ready.modified, ready.deleted), (0, 0, 0));
    let result =
        commit_reviewed(&mut engine, &repo, &ready, OperationId::generate().unwrap()).unwrap();
    let tree = &engine.state().directory_trees[result.result["head_tree_id"].as_str().unwrap()];
    assert_eq!(
        tree.entries.iter().find(|entry| entry.path() == "own"),
        Some(&owned)
    );
    assert_eq!(
        tree.directory_refs[&ready.children[0].directory_id].tree_id,
        child_head
    );
    assert_eq!(bytes(&engine, &repo, "own"), b"parent unstaged");
    let after = status(&engine, &repo, "");
    assert!(!after.staged && after.unstaged && !after.children[0].dirty);
}

#[test]
fn moving_the_repository_root_does_not_change_its_relative_child_pins() {
    let (_temp, mut engine, repo) = fixture();
    // Isolate the mount reference. Owned files would also gain real path-history
    // nodes during this rename, independently of the child's relative path.
    save(&mut engine, &repo, "project/child/file", b"child");
    initialize(&mut engine, &repo, "project/child");
    commit(&mut engine, &repo, "project/child");
    initialize(&mut engine, &repo, "project");
    stage_child(&mut engine, &repo, "project", "project/child");
    let head = commit(&mut engine, &repo, "project");
    engine
        .file_namespace(
            &repo.repository_id,
            storage_contract::NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "project",
                Some("moved"),
            )
            .unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    stage_owned(&mut engine, &repo, "moved");
    let ready = status(&engine, &repo, "moved");
    assert!(
        !ready.staged && !ready.unstaged && !ready.needs_restage,
        "{ready:#?}"
    );
    assert!(!ready.children[0].staged && !ready.children[0].reference_changed);
    let result =
        commit_reviewed(&mut engine, &repo, &ready, OperationId::generate().unwrap()).unwrap();
    assert_eq!(result.result["no_change"], true);
    assert_eq!(result.result["head_tree_id"], head);
    assert_eq!(bytes(&engine, &repo, "moved/child/file"), b"child");
}

#[test]
fn staged_file_is_committed_without_resurrecting_its_later_working_deletion() {
    let (temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "file", b"base");
    initialize(&mut engine, &repo, "");
    commit(&mut engine, &repo, "");
    save(&mut engine, &repo, "file", b"staged");
    stage_owned(&mut engine, &repo, "");
    let staged = status(&engine, &repo, "");
    let expected = engine.state().directory_trees[staged.working_tree_id.as_ref().unwrap()]
        .entries
        .clone();
    remove(
        &mut engine,
        &repo,
        "file",
        storage_contract::NamespaceOperationKind::Delete,
    );
    let ready = status(&engine, &repo, "");
    assert!(ready.staged && ready.unstaged);
    assert_eq!((ready.added, ready.modified, ready.deleted), (0, 1, 0));
    let result =
        commit_reviewed(&mut engine, &repo, &ready, OperationId::generate().unwrap()).unwrap();
    let head = result.result["head_tree_id"].as_str().unwrap();
    assert_eq!(engine.state().directory_trees[head].entries, expected);
    assert!(engine.list_tree(&repo.main_branch_id).unwrap().is_empty());
    let after = status(&engine, &repo, "");
    assert!(!after.staged && after.unstaged);
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(status(&engine, &repo, ""), after);
    assert!(engine.list_tree(&repo.main_branch_id).unwrap().is_empty());
    assert_eq!(version_bytes(&engine, &expected[0].version_id), b"staged");
    let next = commit(&mut engine, &repo, "");
    assert!(engine.state().directory_trees[&next].entries.is_empty());
    assert!(!status(&engine, &repo, "").unstaged);
}

#[test]
fn staged_deletion_does_not_delete_a_recreated_working_file() {
    let (temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "file", b"old identity");
    initialize(&mut engine, &repo, "");
    let old = commit(&mut engine, &repo, "");
    let old_file = engine.state().directory_trees[&old].entries[0]
        .file_id
        .clone();
    remove(
        &mut engine,
        &repo,
        "file",
        storage_contract::NamespaceOperationKind::Delete,
    );
    stage_owned(&mut engine, &repo, "");
    save(&mut engine, &repo, "file", b"replacement");
    let ready = status(&engine, &repo, "");
    assert!(ready.staged && ready.unstaged);
    assert_eq!((ready.added, ready.modified, ready.deleted), (0, 0, 1));
    let result =
        commit_reviewed(&mut engine, &repo, &ready, OperationId::generate().unwrap()).unwrap();
    let head = result.result["head_tree_id"].as_str().unwrap();
    assert!(engine.state().directory_trees[head].entries.is_empty());
    assert_eq!(bytes(&engine, &repo, "file"), b"replacement");
    let after = status(&engine, &repo, "");
    assert!(!after.staged && after.unstaged);
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(status(&engine, &repo, ""), after);
    assert_eq!(bytes(&engine, &repo, "file"), b"replacement");
    assert_eq!(
        version_bytes(
            &engine,
            &engine.state().directory_trees[&old].entries[0].version_id
        ),
        b"old identity"
    );
    let next = commit(&mut engine, &repo, "");
    assert_ne!(
        engine.state().directory_trees[&next].entries[0].file_id,
        old_file
    );
    assert!(!status(&engine, &repo, "").unstaged);
}

#[test]
fn deleted_child_boundary_is_not_reused_by_a_same_path_replacement() {
    let (temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "child/file", b"old independent child");
    initialize(&mut engine, &repo, "child");
    let child_head = commit(&mut engine, &repo, "child");
    let child_id = status(&engine, &repo, "child").directory_id;
    initialize(&mut engine, &repo, "");
    stage_child(&mut engine, &repo, "", "child");
    let old = commit(&mut engine, &repo, "");
    remove(
        &mut engine,
        &repo,
        "child",
        storage_contract::NamespaceOperationKind::DeleteTree,
    );
    save(&mut engine, &repo, "child/new", b"new ordinary child");
    let ready = status(&engine, &repo, "");
    assert!(ready.needs_restage && ready.children.is_empty());
    let replacement = status(&engine, &repo, "child");
    assert!(!replacement.initialized);
    assert_ne!(replacement.directory_id, child_id);
    let before = engine.state().clone();
    assert_eq!(
        commit_reviewed(&mut engine, &repo, &ready, OperationId::generate().unwrap())
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryBranchConflict
    );
    assert_eq!(
        engine
            .stage_directory_git_child(
                &repo.repository_id,
                &DirectoryGitStageChildBody {
                    directory_id: ready.directory_id,
                    directory_branch_id: ready.directory_branch_id.unwrap(),
                    expected_index_generation: ready.index_generation,
                    child_directory_id: child_id.clone(),
                    expected_child_head_tree_id: child_head.clone(),
                    operation_id: OperationId::generate().unwrap(),
                },
                "admin",
            )
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryBranchConflict
    );
    assert_eq!(engine.state(), &before);
    stage_owned(&mut engine, &repo, "");
    save(&mut engine, &repo, "child/later", b"not staged");
    let ready = status(&engine, &repo, "");
    assert!(ready.staged && ready.unstaged && !ready.needs_restage);
    let result =
        commit_reviewed(&mut engine, &repo, &ready, OperationId::generate().unwrap()).unwrap();
    let head = result.result["head_tree_id"].as_str().unwrap();
    let tree = &engine.state().directory_trees[head];
    assert!(tree.directory_refs.is_empty());
    assert!(tree.entries.iter().any(|entry| entry.path() == "child/new"));
    assert!(
        !tree
            .entries
            .iter()
            .any(|entry| entry.path() == "child/later")
    );
    assert_eq!(
        engine.state().directory_trees[&old].directory_refs[&child_id].tree_id,
        child_head
    );
    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert!(!status(&engine, &repo, "child").initialized);
    assert!(status(&engine, &repo, "").unstaged);
    assert_eq!(bytes(&engine, &repo, "child/new"), b"new ordinary child");
    assert_eq!(bytes(&engine, &repo, "child/later"), b"not staged");
}

#[test]
fn inserting_an_intermediate_repository_drops_only_the_parents_direct_pin() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "middle/leaf/file", b"committed leaf");
    initialize(&mut engine, &repo, "middle/leaf");
    let leaf_head = commit(&mut engine, &repo, "middle/leaf");
    let leaf_id = status(&engine, &repo, "middle/leaf").directory_id;
    initialize(&mut engine, &repo, "");
    stage_child(&mut engine, &repo, "", "middle/leaf");
    let old = commit(&mut engine, &repo, "");
    let reviewed = status(&engine, &repo, "");
    save(&mut engine, &repo, "middle/leaf/file", b"dirty leaf");
    initialize(&mut engine, &repo, "middle");
    assert_eq!(
        commit_reviewed(
            &mut engine,
            &repo,
            &reviewed,
            OperationId::generate().unwrap()
        )
        .unwrap_err()
        .code(),
        ErrorCode::DirectoryBranchConflict
    );
    let ready = status(&engine, &repo, "");
    assert!(ready.needs_restage);
    assert_eq!(ready.children.len(), 1);
    assert_eq!(ready.children[0].path, "middle");
    assert!(ready.children[0].head_tree_id.is_none());
    stage_owned(&mut engine, &repo, "");
    let head = commit(&mut engine, &repo, "");
    assert!(engine.state().directory_trees[&head].entries.is_empty());
    assert!(
        engine.state().directory_trees[&head]
            .directory_refs
            .is_empty()
    );
    assert_eq!(
        engine.state().directory_trees[&old].directory_refs[&leaf_id].tree_id,
        leaf_head
    );
    assert!(status(&engine, &repo, "middle").head_tree_id.is_none());
    let leaf = status(&engine, &repo, "middle/leaf");
    assert_eq!(leaf.head_tree_id.as_deref(), Some(leaf_head.as_str()));
    assert!(leaf.unstaged && !leaf.staged);
    assert_eq!(bytes(&engine, &repo, "middle/leaf/file"), b"dirty leaf");
}

#[test]
fn moving_a_staged_file_changes_the_working_location_not_the_reviewed_snapshot() {
    let (temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "from/file", b"base");
    save(&mut engine, &repo, "to/keep", b"keep target directory");
    initialize(&mut engine, &repo, "");
    commit(&mut engine, &repo, "");
    save(&mut engine, &repo, "from/file", b"staged");
    stage_owned(&mut engine, &repo, "");
    let staged = status(&engine, &repo, "");
    let expected = engine.state().directory_trees[staged.working_tree_id.as_ref().unwrap()]
        .entries
        .clone();
    let file = expected
        .iter()
        .find(|entry| entry.path() == "from/file")
        .unwrap()
        .clone();
    engine
        .file_namespace(
            &repo.repository_id,
            storage_contract::NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "from/file",
                Some("to/file"),
            )
            .unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let ready = status(&engine, &repo, "");
    assert!(ready.staged && ready.unstaged);
    assert_eq!(engine.state().file_locations[&file.file_id], "to/file");
    let result =
        commit_reviewed(&mut engine, &repo, &ready, OperationId::generate().unwrap()).unwrap();
    let head = result.result["head_tree_id"].as_str().unwrap();
    assert_eq!(engine.state().directory_trees[head].entries, expected);
    assert_eq!(engine.state().file_locations[&file.file_id], "to/file");
    assert_eq!(bytes(&engine, &repo, "to/file"), b"staged");
    assert!(
        engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .iter()
            .all(|entry| entry.path() != "from/file")
    );
    let after = status(&engine, &repo, "");
    assert!(!after.staged && after.unstaged);
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(status(&engine, &repo, ""), after);
    assert_eq!(engine.state().file_locations[&file.file_id], "to/file");
    assert_eq!(version_bytes(&engine, &file.version_id), b"staged");
    let next = commit(&mut engine, &repo, "");
    assert!(
        engine.state().directory_trees[&next]
            .entries
            .iter()
            .any(|entry| entry.path() == "to/file" && entry.file_id == file.file_id)
    );
    assert!(!status(&engine, &repo, "").unstaged);
}
