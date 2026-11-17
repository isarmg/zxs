//! Use actual published versions, then isolate each in-memory reference in
//! turn. The synthetic edge cases do not stand in for durable operation records.
use crate::{CommitRequest, Config, DirectoryReference, DirectoryTree, Engine, Entry, ErrorCode, Repository};
use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, sync::Arc};
use storage_contract::{BranchName, OperationId};

fn op() -> OperationId { OperationId::generate().unwrap() }

fn fixture() -> (tempfile::TempDir, Engine, Repository, Entry) {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let disk = temp.path().join("disk");
    for path in [&system, &disk] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut engine = Engine::initialize(&system, Config::development([
        (name_policy::Name::parse("Disk").unwrap(), disk),
    ]).unwrap()).unwrap();
    let repo = engine.create_disk_root("P01", "admin", op()).unwrap();
    let mut entries = Vec::new();
    for body in [b"old".as_slice(), b"current".as_slice()] {
        let workspace = engine.create_workspace(&repo.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&workspace.workspace_id, "a", body).unwrap();
        engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id, operation_id: op(), principal: "admin".into(),
            expected_head: workspace.base_head, fencing_token: workspace.fencing_token,
            message: "selection guard".into(), request_digest: None,
        }).unwrap();
        entries.push(engine.namespace_tree(&repo.main_branch_id).unwrap().get("a").unwrap().clone());
    }
    (temp, engine, repo, entries.remove(0))
}

fn assert_pinned(engine: &Engine, repo: &Repository, old: &Entry) {
    assert_eq!(engine.check_purge(&repo.history_tree_id, std::slice::from_ref(&old.version_id))
        .unwrap_err().code(), ErrorCode::ContentPinned);
}

#[test]
fn inherited_branch_is_a_real_purge_pin_before_and_after_cold_open() {
    let (temp, mut engine, repo, old) = fixture();
    engine.check_purge(&repo.history_tree_id, std::slice::from_ref(&old.version_id)).unwrap();
    engine.create_file_branch(&old.file_id, &old.version_id, &BranchName::parse("frozen").unwrap(),
        "admin", op()).unwrap();
    assert_pinned(&engine, &repo, &old);
    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_pinned(&engine, &repo, &old);
}

#[test]
fn pinned_inheritance_is_checked_separately_on_branch_and_admitted_head() {
    let (_temp, mut engine, repo, old) = fixture();
    let original = engine.state.clone();
    let head = engine.state.branches[&repo.main_branch_id].head_commit_id.clone();
    for on_branch in [true, false] {
        engine.state = original.clone();
        let state = Arc::make_mut(&mut engine.state);
        let pending = if on_branch {
            &mut state.branches.get_mut(&repo.main_branch_id).unwrap().pending_file_versions
        } else {
            &mut state.commits.get_mut(&head).unwrap().pending_file_versions
        };
        pending.insert(old.file_id.clone(), old.version_id.clone());
        assert_pinned(&engine, &repo, &old);
    }
}

fn tree(id: &str, commit: &str, entries: Vec<Entry>) -> DirectoryTree {
    DirectoryTree {
        tree_id: id.into(), directory_branch_id: "test-composition".into(), parent_tree_id: None,
        commit_id: commit.into(), path: String::new(), entries, file_branches: BTreeMap::new(),
        file_contexts: BTreeMap::new(), inherited_files: BTreeMap::new(),
        directory_refs: BTreeMap::new(), sequence: 1,
    }
}

#[test]
fn selected_directory_follows_frozen_child_ids_not_mutable_head_or_historical_parent() {
    let (_temp, mut engine, repo, old) = fixture();
    let commit = engine.state.branches[&repo.main_branch_id].head_commit_id.clone();
    let left = "unit-selected-tree";
    let child = "unit-frozen-child";
    let state = Arc::make_mut(&mut engine.state);
    state.directory_trees.insert(child.into(), tree(child, &commit, vec![old.clone()]));
    let mut parent = tree(left, &commit, Vec::new());
    parent.parent_tree_id = Some(child.into());
    state.directory_trees.insert(left.into(), parent);
    state.directory_tree_selections.insert("unit-directory".into(), left.into());
    // An immutable historical parent on its own must stay purgeable.
    engine.check_purge(&repo.history_tree_id, std::slice::from_ref(&old.version_id)).unwrap();
    let reference = DirectoryReference { directory_branch_id: "other-live-head".into(), tree_id: child.into() };
    Arc::make_mut(&mut engine.state).directory_trees.get_mut(left).unwrap()
        .directory_refs.insert("child-entry".into(), reference);
    assert_pinned(&engine, &repo, &old);
    Arc::make_mut(&mut engine.state).directory_trees.remove(child);
    assert_eq!(engine.check_purge(&repo.history_tree_id, std::slice::from_ref(&old.version_id))
        .unwrap_err().code(), ErrorCode::PublicationInvalid);
}
