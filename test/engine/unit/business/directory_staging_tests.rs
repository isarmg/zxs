use super::*;
use std::io::Read;
use storage_contract::directory_git::*;
use storage_contract::directory_history::{DirectoryHistoryCollection, DirectoryHistoryRequest};

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
    let head = engine.state.branches[&repo.main_branch_id]
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
fn history(
    engine: &Engine,
    repo: &Repository,
    directory: &str,
) -> storage_contract::directory_history::DirectoryHistoryPage {
    engine
        .capture_directory_history(
            DirectoryHistoryRequest::parse(
                &repo.repository_id,
                directory,
                DirectoryHistoryCollection::Nodes,
                BTreeMap::new(),
            )
            .unwrap(),
            storage_contract::history_window::HistoryWindowCancellation::new(),
        )
        .unwrap()
        .page()
        .unwrap()
}
fn bytes(engine: &Engine, repo: &Repository, path: &str) -> Vec<u8> {
    let entry = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path() == path)
        .unwrap();
    let mut result = Vec::new();
    engine
        .valid_replica(&entry.version_id)
        .unwrap()
        .0
        .open()
        .unwrap()
        .read_to_end(&mut result)
        .unwrap();
    result
}
#[test]
fn saves_do_not_initialize_repositories_and_plain_descendants_belong_to_nearest_owner() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "folder/deep/file.txt", b"one");
    assert!(!status(&engine, &repo, "").initialized);
    assert!(!status(&engine, &repo, "folder").initialized);
    assert!(status(&engine, &repo, "folder/deep").owner.is_none());
    initialize(&mut engine, &repo, "");
    let child = status(&engine, &repo, "folder/deep");
    assert!(!child.initialized);
    assert_eq!(child.owner.unwrap().directory_id, repo.repository_id);
    let versions = engine.state.versions.len();
    let tree = commit(&mut engine, &repo, "");
    assert!(
        engine.state.directory_trees[&tree]
            .entries
            .iter()
            .any(|entry| entry.path() == "folder/deep/file.txt")
    );
    assert!(
        engine.state.directory_trees[&tree]
            .directory_refs
            .is_empty()
    );
    assert_eq!(engine.state.versions.len(), versions);
    assert!(!status(&engine, &repo, "").staged);
    assert_eq!(history(&engine, &repo, &repo.repository_id).nodes.len(), 1);
}
#[test]
fn child_initialization_invalidates_parent_index_and_never_implicitly_commits_child() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "child/file", b"one");
    initialize(&mut engine, &repo, "");
    let stale = status(&engine, &repo, "");
    initialize(&mut engine, &repo, "child");
    let parent = status(&engine, &repo, "");
    assert!(parent.needs_restage);
    assert!(parent.index_generation > stale.index_generation);
    assert_eq!(
        commit_reviewed(&mut engine, &repo, &stale, OperationId::generate().unwrap())
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryBranchConflict
    );
    assert_eq!(
        commit_reviewed(
            &mut engine,
            &repo,
            &parent,
            OperationId::generate().unwrap()
        )
        .unwrap_err()
        .code(),
        ErrorCode::DirectoryBranchConflict
    );
    stage_owned(&mut engine, &repo, "");
    let tree = commit(&mut engine, &repo, "");
    assert!(engine.state.directory_trees[&tree].entries.is_empty());
    assert!(status(&engine, &repo, "child").head_tree_id.is_none());
    assert_eq!(bytes(&engine, &repo, "child/file"), b"one");
}
#[test]
fn parent_pins_reviewed_child_commit_while_normal_reads_return_dirty_current_bytes() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "child/file", b"one");
    initialize(&mut engine, &repo, "child");
    let first = commit(&mut engine, &repo, "child");
    initialize(&mut engine, &repo, "");
    stage_child(&mut engine, &repo, "", "child");
    let parent = commit(&mut engine, &repo, "");
    let child_id = status(&engine, &repo, "child").directory_id;
    assert_eq!(
        engine.state.directory_trees[&parent].directory_refs[&child_id].tree_id,
        first
    );
    save(&mut engine, &repo, "child/file", b"two");
    assert_eq!(bytes(&engine, &repo, "child/file"), b"two");
    let p = status(&engine, &repo, "");
    assert!(!p.staged);
    assert!(p.children[0].dirty);
    assert!(!p.children[0].reference_changed);
    let second = commit(&mut engine, &repo, "child");
    let p = status(&engine, &repo, "");
    assert!(!p.staged);
    assert!(p.children[0].reference_changed);
    assert!(!p.children[0].staged);
    save(&mut engine, &repo, "own", b"parent edit");
    let parent2 = commit(&mut engine, &repo, "");
    assert_eq!(
        engine.state.directory_trees[&parent2].directory_refs[&child_id].tree_id,
        first
    );
    let old_version = engine.state.directory_trees[&first]
        .entries
        .iter()
        .find(|entry| entry.path() == "child/file")
        .unwrap()
        .version_id
        .clone();
    assert_eq!(
        engine.state.directory_trees[&parent2]
            .entries
            .iter()
            .find(|entry| entry.path() == "child/file")
            .unwrap()
            .version_id,
        old_version
    );
    stage_child(&mut engine, &repo, "", "child");
    assert!(status(&engine, &repo, "").staged);
    let parent3 = commit(&mut engine, &repo, "");
    assert_eq!(
        engine.state.directory_trees[&parent3].directory_refs[&child_id].tree_id,
        second
    );
    assert_eq!(bytes(&engine, &repo, "child/file"), b"two");
}
#[test]
fn directory_index_and_heads_survive_cold_restart_and_receipts_replay() {
    let (temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "folder/file", b"saved");
    initialize(&mut engine, &repo, "");
    let before = status(&engine, &repo, "");
    let entries = engine.list_tree(&repo.main_branch_id).unwrap();
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(status(&engine, &repo, ""), before);
    let operation = OperationId::generate().unwrap();
    let result = commit_reviewed(&mut engine, &repo, &before, operation).unwrap();
    assert_eq!(
        commit_reviewed(&mut engine, &repo, &before, operation).unwrap(),
        result
    );
    let after = status(&engine, &repo, "");
    assert!(!after.staged);
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(status(&engine, &repo, ""), after);
    assert_eq!(engine.list_tree(&repo.main_branch_id).unwrap(), entries);
    assert_eq!(
        commit_reviewed(&mut engine, &repo, &before, operation).unwrap(),
        result
    );
}
#[test]
fn stale_working_tree_and_stale_index_tokens_cannot_commit_newer_changes() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "file", b"one");
    initialize(&mut engine, &repo, "");
    let s = status(&engine, &repo, "");
    save(&mut engine, &repo, "file", b"two");
    let before = engine.state.clone();
    assert_eq!(
        commit_reviewed(&mut engine, &repo, &s, OperationId::generate().unwrap())
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryBranchConflict
    );
    assert_eq!(engine.state, before);
    let s = status(&engine, &repo, "");
    stage_owned(&mut engine, &repo, "");
    assert_eq!(
        commit_reviewed(&mut engine, &repo, &s, OperationId::generate().unwrap())
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryBranchConflict
    );
}
#[test]
fn commit_faults_preserve_current_bytes_and_publish_only_this_repository() {
    for (fault, published) in [
        ("after_prepare_1", false),
        ("after_accept_2", true),
        ("after_local_publications", true),
    ] {
        let (temp, mut engine, repo) = fixture();
        save(&mut engine, &repo, "child/file", b"persist me");
        initialize(&mut engine, &repo, "child");
        initialize(&mut engine, &repo, "");
        let s = status(&engine, &repo, "");
        let op = OperationId::generate().unwrap();
        engine.inject_fault_once(fault);
        assert!(
            commit_reviewed(&mut engine, &repo, &s, op).is_err(),
            "{fault}"
        );
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("system")).unwrap();
        assert_eq!(bytes(&engine, &repo, "child/file"), b"persist me");
        assert_eq!(
            status(&engine, &repo, "").head_tree_id.is_some(),
            published,
            "{fault}"
        );
        assert!(status(&engine, &repo, "child").head_tree_id.is_none());
        commit_reviewed(&mut engine, &repo, &s, op).unwrap();
        assert!(status(&engine, &repo, "child").head_tree_id.is_none());
    }
}

#[test]
fn branch_creation_requires_initialization_and_does_not_include_dirty_owned_files() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "own", b"one");
    let name = storage_contract::BranchName::parse("topic").unwrap();
    assert_eq!(
        engine
            .create_directory_branches(
                &repo.repository_id,
                None,
                &name,
                "admin",
                OperationId::generate().unwrap()
            )
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryScopeRequired
    );
    initialize(&mut engine, &repo, "");
    assert_eq!(
        engine
            .create_directory_branches(
                &repo.repository_id,
                None,
                &name,
                "admin",
                OperationId::generate().unwrap()
            )
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryBranchConflict
    );
    let head = commit(&mut engine, &repo, "");
    let current = status(&engine, &repo, "").directory_branch_id.unwrap();
    let created = engine
        .create_directory_branches(
            &repo.repository_id,
            None,
            &name,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let id = created.result["directory_branch_id"].as_str().unwrap();
    assert_eq!(
        engine.state.directory_git_repositories[&repo.repository_id].branches[id]
            .head_tree_id
            .as_deref(),
        Some(head.as_str())
    );
    assert_eq!(
        status(&engine, &repo, "").directory_branch_id.as_deref(),
        Some(current.as_str())
    );
    assert_eq!(history(&engine, &repo, &repo.repository_id).nodes.len(), 1);
}

#[test]
fn parent_branch_checkout_preserves_dirty_independent_child_and_its_head() {
    let (temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "own", b"main");
    save(&mut engine, &repo, "child/file", b"child initial");
    initialize(&mut engine, &repo, "child");
    let child_head = commit(&mut engine, &repo, "child");
    initialize(&mut engine, &repo, "");
    stage_child(&mut engine, &repo, "", "child");
    commit(&mut engine, &repo, "");
    let main = status(&engine, &repo, "").directory_branch_id.unwrap();
    let child_branch = status(&engine, &repo, "child").directory_branch_id.unwrap();
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
    engine
        .select_default_directory_branch(
            &repo.repository_id,
            &topic,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    save(&mut engine, &repo, "own", b"topic");
    commit(&mut engine, &repo, "");
    save(&mut engine, &repo, "child/file", b"dirty child");
    let child_before = engine.state.directory_git_repositories
        [&status(&engine, &repo, "child").directory_id]
        .clone();
    engine
        .select_default_directory_branch(
            &repo.repository_id,
            &main,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(bytes(&engine, &repo, "own"), b"main");
    assert_eq!(bytes(&engine, &repo, "child/file"), b"dirty child");
    let child = status(&engine, &repo, "child");
    assert_eq!(child.head_tree_id.as_deref(), Some(child_head.as_str()));
    assert_eq!(
        child.directory_branch_id.as_deref(),
        Some(child_branch.as_str())
    );
    assert!(child.unstaged && !child.staged);
    assert_eq!(
        engine.state.directory_git_repositories[&child.directory_id],
        child_before
    );
    assert!(!status(&engine, &repo, "").staged);
    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(bytes(&engine, &repo, "child/file"), b"dirty child");
    assert!(!status(&engine, &repo, "").staged);
}

#[test]
fn parent_checkout_does_not_revalidate_a_childs_retired_file_branch() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "child/file", b"child initial");
    initialize(&mut engine, &repo, "child");
    commit(&mut engine, &repo, "child");
    initialize(&mut engine, &repo, "");
    stage_child(&mut engine, &repo, "", "child");
    commit(&mut engine, &repo, "");
    let topic = engine
        .create_directory_branches(
            &repo.repository_id,
            None,
            &storage_contract::BranchName::parse("frozen parent").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let entry = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path() == "child/file")
        .unwrap();
    let previous = engine
        .file_context(&repo.repository_id, "child/file")
        .unwrap()
        .branch_id;
    let alternate = engine
        .create_file_branch(
            &entry.file_id,
            &entry.version_id,
            &storage_contract::BranchName::parse("child alternate").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    engine
        .select_file_branch_at(
            &repo.repository_id,
            &entry.path,
            &entry.file_id,
            &alternate,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    save(
        &mut engine,
        &repo,
        "child/file",
        b"independent current child",
    );
    commit(&mut engine, &repo, "child");
    engine
        .archive_branch(&previous, "admin", OperationId::generate().unwrap())
        .unwrap();
    let child = status(&engine, &repo, "child");
    engine
        .create_directory_branches(
            &repo.repository_id,
            None,
            &storage_contract::BranchName::parse("after child archive").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    engine
        .select_default_directory_branch(
            &repo.repository_id,
            &topic,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(status(&engine, &repo, "child"), child);
    assert_eq!(
        bytes(&engine, &repo, "child/file"),
        b"independent current child"
    );
}

#[test]
fn parent_checkout_retains_ordinary_ancestors_of_a_new_nested_child_repository() {
    let (temp, mut engine, repo) = fixture();
    initialize(&mut engine, &repo, "");
    commit(&mut engine, &repo, "");
    let old = engine
        .create_directory_branches(
            &repo.repository_id,
            None,
            &storage_contract::BranchName::parse("before child").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    save(
        &mut engine,
        &repo,
        "ordinary/nested/child/file",
        b"child initial",
    );
    initialize(&mut engine, &repo, "ordinary/nested/child");
    commit(&mut engine, &repo, "ordinary/nested/child");
    stage_owned(&mut engine, &repo, "");
    stage_child(&mut engine, &repo, "", "ordinary/nested/child");
    commit(&mut engine, &repo, "");
    save(
        &mut engine,
        &repo,
        "ordinary/nested/child/file",
        b"dirty child",
    );
    let child_before = status(&engine, &repo, "ordinary/nested/child");
    let ancestors = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .filter(|entry| matches!(entry.path(), "ordinary" | "ordinary/nested"))
        .collect::<Vec<_>>();
    engine
        .select_default_directory_branch(
            &repo.repository_id,
            &old,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(
        bytes(&engine, &repo, "ordinary/nested/child/file"),
        b"dirty child"
    );
    assert_eq!(
        status(&engine, &repo, "ordinary/nested/child"),
        child_before
    );
    let current = engine.list_tree(&repo.main_branch_id).unwrap();
    for ancestor in &ancestors {
        assert!(current.contains(ancestor));
    }
    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        status(&engine, &repo, "ordinary/nested/child"),
        child_before
    );
    assert_eq!(
        bytes(&engine, &repo, "ordinary/nested/child/file"),
        b"dirty child"
    );
}

#[test]
fn parent_checkout_rejects_replacing_a_live_child_ancestor_identity() {
    for ancestor_was_directory in [false, true] {
        let (_temp, mut engine, repo) = fixture();
        save(
            &mut engine,
            &repo,
            if ancestor_was_directory {
                "ordinary/old"
            } else {
                "ordinary"
            },
            b"old ancestor",
        );
        initialize(&mut engine, &repo, "");
        commit(&mut engine, &repo, "");
        let old = engine
            .create_directory_branches(
                &repo.repository_id,
                None,
                &storage_contract::BranchName::parse("old ancestor").unwrap(),
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned();
        engine
            .file_namespace(
                &repo.repository_id,
                NamespaceOperation::parse(
                    if ancestor_was_directory {
                        NamespaceOperationKind::DeleteTree
                    } else {
                        NamespaceOperationKind::Delete
                    },
                    "ordinary",
                    None,
                )
                .unwrap(),
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        save(
            &mut engine,
            &repo,
            "ordinary/child/file",
            b"preserved child",
        );
        initialize(&mut engine, &repo, "ordinary/child");
        commit(&mut engine, &repo, "ordinary/child");
        stage_owned(&mut engine, &repo, "");
        stage_child(&mut engine, &repo, "", "ordinary/child");
        commit(&mut engine, &repo, "");
        let before = canonical_json(engine.state()).unwrap();
        let operation = OperationId::generate().unwrap();
        let error = engine
            .select_default_directory_branch(&repo.repository_id, &old, "admin", operation)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::DirectoryBranchConflict);
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
        assert!(!engine.state.operations.contains_key(&operation));
        assert_eq!(
            bytes(&engine, &repo, "ordinary/child/file"),
            b"preserved child"
        );
    }
}

#[test]
fn parent_checkout_cannot_resurrect_a_deleted_independent_child_repository() {
    for replace_path in [false, true] {
        let (_temp, mut engine, repo) = fixture();
        save(&mut engine, &repo, "child/file", b"retained child");
        initialize(&mut engine, &repo, "child");
        commit(&mut engine, &repo, "child");
        initialize(&mut engine, &repo, "");
        stage_child(&mut engine, &repo, "", "child");
        commit(&mut engine, &repo, "");
        let old = engine
            .create_directory_branches(
                &repo.repository_id,
                None,
                &storage_contract::BranchName::parse("before delete").unwrap(),
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned();
        engine
            .file_namespace(
                &repo.repository_id,
                NamespaceOperation::parse(NamespaceOperationKind::DeleteTree, "child", None)
                    .unwrap(),
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        if replace_path {
            save(&mut engine, &repo, "child/new", b"new ordinary directory");
        }
        stage_owned(&mut engine, &repo, "");
        commit(&mut engine, &repo, "");
        let before = canonical_json(engine.state()).unwrap();
        let operation = OperationId::generate().unwrap();
        let error = engine
            .select_default_directory_branch(&repo.repository_id, &old, "admin", operation)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::DirectoryBranchConflict);
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
        assert!(!engine.state.operations.contains_key(&operation));
        if replace_path {
            assert_eq!(
                bytes(&engine, &repo, "child/new"),
                b"new ordinary directory"
            );
        }
    }
}

#[test]
fn explicit_snapshot_read_uses_pinned_bytes_not_the_live_working_composition() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "file", b"first");
    initialize(&mut engine, &repo, "");
    let first = commit(&mut engine, &repo, "");
    let branch = status(&engine, &repo, "").directory_branch_id.unwrap();
    let old = engine.state.directory_trees[&first].entries.clone();
    save(&mut engine, &repo, "file", b"second");
    let raw = engine
        .preview_directory_branch(
            &repo.repository_id,
            &branch,
            status(&engine, &repo, "").working_tree_id.as_ref().unwrap(),
        )
        .unwrap();
    assert_ne!(raw.branch().entries, old);
    let snapshot = engine
        .preview_directory_branch(&repo.repository_id, &branch, &first)
        .unwrap();
    assert_eq!(snapshot.branch().entries, old);
    assert_eq!(bytes(&engine, &repo, "file"), b"second");
}

#[test]
fn reviewed_parent_commit_commutes_with_child_only_working_updates() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "child/file", b"one");
    initialize(&mut engine, &repo, "child");
    let child = commit(&mut engine, &repo, "child");
    initialize(&mut engine, &repo, "");
    stage_child(&mut engine, &repo, "", "child");
    commit(&mut engine, &repo, "");
    save(&mut engine, &repo, "own", b"reviewed parent");
    stage_owned(&mut engine, &repo, "");
    let reviewed = status(&engine, &repo, "");
    save(&mut engine, &repo, "child/file", b"concurrent child");
    assert_ne!(
        status(&engine, &repo, "").working_tree_id,
        reviewed.working_tree_id
    );
    let result = commit_reviewed(
        &mut engine,
        &repo,
        &reviewed,
        OperationId::generate().unwrap(),
    )
    .unwrap();
    let tree = &engine.state.directory_trees[result.result["head_tree_id"].as_str().unwrap()];
    let id = status(&engine, &repo, "child").directory_id;
    assert_eq!(tree.directory_refs[&id].tree_id, child);
    assert_eq!(bytes(&engine, &repo, "child/file"), b"concurrent child");
}

#[test]
fn inserting_an_intermediate_repository_invalidates_only_owned_boundary_review() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "middle/leaf/file", b"one");
    initialize(&mut engine, &repo, "middle/leaf");
    let leaf = commit(&mut engine, &repo, "middle/leaf");
    initialize(&mut engine, &repo, "");
    stage_child(&mut engine, &repo, "", "middle/leaf");
    commit(&mut engine, &repo, "");
    initialize(&mut engine, &repo, "middle");
    let parent = status(&engine, &repo, "");
    assert!(parent.needs_restage);
    assert_eq!(parent.children.len(), 1);
    assert_eq!(parent.children[0].path, "middle");
    stage_owned(&mut engine, &repo, "");
    let root = commit(&mut engine, &repo, "");
    assert!(
        engine.state.directory_trees[&root]
            .directory_refs
            .is_empty()
    );
    let middle = status(&engine, &repo, "middle");
    assert!(middle.head_tree_id.is_none());
    assert_eq!(
        middle.children[0].head_tree_id.as_deref(),
        Some(leaf.as_str())
    );
    stage_child(&mut engine, &repo, "middle", "middle/leaf");
    let middle = commit(&mut engine, &repo, "middle");
    stage_child(&mut engine, &repo, "", "middle");
    let root = commit(&mut engine, &repo, "");
    let tree = &engine.state.directory_trees[&root];
    assert_eq!(tree.directory_refs.len(), 1);
    assert_eq!(tree.directory_refs.values().next().unwrap().tree_id, middle);
    save(&mut engine, &repo, "middle/leaf/file", b"dirty grandchild");
    let root = status(&engine, &repo, "");
    assert!(!root.staged);
    assert!(root.children[0].dirty);
    assert!(!root.children[0].reference_changed);
    let middle = status(&engine, &repo, "middle");
    assert!(!middle.staged);
    assert!(middle.children[0].dirty);
}

#[test]
fn parent_batch_cannot_delete_an_ancestor_of_an_independent_child() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "plain/child/file", b"keep child");
    initialize(&mut engine, &repo, "plain/child");
    initialize(&mut engine, &repo, "");
    let status = status(&engine, &repo, "");
    let branch = status.directory_branch_id.unwrap();
    let tree = status.working_tree_id.unwrap();
    let entry = engine.state.directory_trees[&tree]
        .entries
        .iter()
        .find(|entry| entry.path() == "plain")
        .unwrap()
        .entry_id
        .clone();
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Delete,
        items: vec![DirectoryNamespaceItem {
            source: DirectoryNamespaceSource {
                directory_branch_id: branch,
                expected_tree_id: tree,
                path: "plain".into(),
                entry_id: entry,
            },
            target: None,
        }],
    };
    let before = engine.state.clone();
    let operation = OperationId::generate().unwrap();
    assert_eq!(
        engine
            .directory_namespace_batch(&repo.repository_id, &request, "admin", operation)
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryBranchScope
    );
    assert_eq!(engine.state, before);
    assert!(!engine.state.operations.contains_key(&operation));
    assert_eq!(bytes(&engine, &repo, "plain/child/file"), b"keep child");
}

#[test]
fn initializing_a_previously_committed_child_stages_removal_of_its_owned_contents() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "child/file", b"one");
    initialize(&mut engine, &repo, "");
    let before = commit(&mut engine, &repo, "");
    initialize(&mut engine, &repo, "child");
    assert!(status(&engine, &repo, "").needs_restage);
    stage_owned(&mut engine, &repo, "");
    let parent = status(&engine, &repo, "");
    assert!(parent.staged);
    assert_eq!(parent.deleted, 2);
    let after = commit(&mut engine, &repo, "");
    assert_ne!(before, after);
    assert!(engine.state.directory_trees[&after].entries.is_empty());
    assert!(
        engine.state.directory_trees[&after]
            .directory_refs
            .is_empty()
    );
    assert_eq!(engine.state.directory_trees[&before].entries.len(), 2);
    assert!(status(&engine, &repo, "child").head_tree_id.is_none());
    assert!(!status(&engine, &repo, "").staged);
    assert_eq!(bytes(&engine, &repo, "child/file"), b"one");
}

#[test]
fn detached_status_read_is_cancellable_and_rechecks_its_exact_observation() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "child/file", b"one");
    initialize(&mut engine, &repo, "");
    let cancellation = HistoryWindowCancellation::new();
    let read = engine
        .capture_directory_git_status(&repo.repository_id, "", cancellation.clone())
        .unwrap();
    let before = read.status().unwrap();
    engine.recheck_directory_git_status(&read).unwrap();
    save(&mut engine, &repo, "other", b"new");
    assert_eq!(before, read.status().unwrap());
    assert_eq!(
        engine
            .recheck_directory_git_status(&read)
            .unwrap_err()
            .code(),
        ErrorCode::ListingChanged
    );
    cancellation.cancel();
    assert_eq!(
        read.status().unwrap_err().code(),
        ErrorCode::OperationAborted
    );
    assert_eq!(
        engine
            .recheck_directory_git_status(&read)
            .unwrap_err()
            .code(),
        ErrorCode::OperationAborted
    );
    assert_eq!(
        engine
            .capture_directory_git_status(&repo.repository_id, "", cancellation)
            .err()
            .unwrap()
            .code(),
        ErrorCode::OperationAborted
    );
}

fn branch_source(engine: &Engine, repo: &Repository, path: &str) -> DirectoryBranchCreateSource {
    let current = status(engine, repo, path);
    DirectoryBranchCreateSource {
        repository_id: repo.repository_id.clone(),
        directory_id: current.directory_id,
        directory_branch_id: current.directory_branch_id.unwrap(),
        working_tree_id: current.working_tree_id.unwrap(),
        head_tree_id: current.head_tree_id.unwrap(),
        index_generation: current.index_generation,
        namespace_head: engine.state.branches[&repo.main_branch_id]
            .head_commit_id
            .clone(),
    }
}

#[test]
fn reviewed_branch_source_is_exact_and_durable_replay_does_not_read_a_new_source() {
    let (temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "child/file", b"reviewed");
    initialize(&mut engine, &repo, "child");
    commit(&mut engine, &repo, "child");
    let path = name_policy::LookupPath::parse("child").unwrap();
    let source = branch_source(&engine, &repo, "child");
    let name = storage_contract::BranchName::parse("reviewed branch").unwrap();
    for field in [
        "directory",
        "branch",
        "working",
        "head",
        "index",
        "namespace",
    ] {
        let mut changed = source.clone();
        match field {
            "directory" => changed.directory_id = new_id("entry").unwrap(),
            "branch" => changed.directory_branch_id = new_id("dbr").unwrap(),
            "working" => changed.working_tree_id = new_id("dtree").unwrap(),
            "head" => changed.head_tree_id = new_id("dtree").unwrap(),
            "index" => changed.index_generation += 1,
            "namespace" => changed.namespace_head = new_id("commit").unwrap(),
            _ => unreachable!(),
        }
        let before = engine.state.clone();
        assert_eq!(
            engine
                .create_directory_branch_from_source(
                    &changed,
                    Some(&path),
                    &name,
                    "admin",
                    OperationId::generate().unwrap(),
                )
                .unwrap_err()
                .code(),
            ErrorCode::DirectoryBranchConflict,
            "{field}",
        );
        assert_eq!(engine.state, before, "{field}");
    }
    let operation = OperationId::generate().unwrap();
    let receipt = engine
        .create_directory_branch_from_source(&source, Some(&path), &name, "admin", operation)
        .unwrap();
    assert_eq!(receipt.result["input"]["source"], json_codec::json!(source));
    let current_branch = source.directory_branch_id.clone();
    let created_branch = receipt.result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        status(&engine, &repo, "child").directory_branch_id,
        Some(current_branch)
    );
    // Restaging identical bytes still invalidates the reviewed index generation.
    stage_owned(&mut engine, &repo, "child");
    let fresh = branch_source(&engine, &repo, "child");
    assert_ne!(fresh.index_generation, source.index_generation);
    let before = engine.state.clone();
    assert_eq!(
        engine
            .create_directory_branch_from_source(
                &source,
                Some(&path),
                &storage_contract::BranchName::parse("unknown retry").unwrap(),
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::DirectoryBranchConflict,
    );
    assert_eq!(engine.state, before);
    assert_eq!(
        engine
            .create_directory_branch_from_source(&fresh, Some(&path), &name, "admin", operation)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused,
    );
    save(&mut engine, &repo, "child/file", b"newer saved source");
    commit(&mut engine, &repo, "child");
    engine
        .select_directory_branches(
            &repo.repository_id,
            Some(&path),
            &name,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(
        status(&engine, &repo, "child").directory_branch_id,
        Some(created_branch)
    );
    assert_eq!(
        engine
            .create_directory_branch_from_source(&source, Some(&path), &name, "admin", operation)
            .unwrap(),
        receipt,
    );
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        engine
            .create_directory_branch_from_source(&source, Some(&path), &name, "admin", operation)
            .unwrap(),
        receipt,
    );
}
