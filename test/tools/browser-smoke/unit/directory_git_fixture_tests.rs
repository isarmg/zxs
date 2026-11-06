use super::*;
use std::io::Read;
use storage_engine::{Config, DirectoryCommitRequest, directory_git::*};

fn status(engine: &Engine, repository: &str, path: &str) -> DirectoryGitStatus {
    engine.directory_git_status(repository, path).unwrap()
}
fn stage_child(engine: &mut Engine, parent: &DirectoryGitStatus, child: &DirectoryGitStatus) {
    engine
        .stage_directory_git_child(
            &parent.repository_id,
            &DirectoryGitStageChildBody {
                directory_id: parent.directory_id.clone(),
                directory_branch_id: parent.directory_branch_id.clone().unwrap(),
                expected_index_generation: parent.index_generation,
                child_directory_id: child.directory_id.clone(),
                expected_child_head_tree_id: child.head_tree_id.clone().unwrap(),
                operation_id: OperationId::generate().unwrap(),
            },
            PRINCIPAL,
        )
        .unwrap();
}
fn commit(
    engine: &mut Engine,
    status: &DirectoryGitStatus,
) -> storage_engine::Result<storage_engine::OperationResult> {
    engine.commit_directory_changes(DirectoryCommitRequest {
        repository_id: &status.repository_id,
        directory_branch_id: status.directory_branch_id.as_deref().unwrap(),
        expected_tree_id: status.working_tree_id.as_deref().unwrap(),
        expected_index_generation: status.index_generation,
        message: "Explicit browser parent/child review",
        principal: PRINCIPAL,
        operation_id: OperationId::generate().unwrap(),
    })
}
fn bytes(engine: &Engine, version: &str) -> Vec<u8> {
    let mut content = engine.open_content(version).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn child_browser_fixture_preserves_manual_boundaries_pins_and_uncommitted_work_on_reopen() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("pool");
    let mut engine = Engine::initialize(
        &root,
        Config::development(["P01", "B01", "C01"].map(|slot| temporary.path().join(slot))).unwrap(),
    )
    .unwrap();
    seed(&mut engine).unwrap();
    drop(engine);
    let mut engine = Engine::open(&root).unwrap();
    let repository = engine
        .state()
        .repositories
        .values()
        .find(|repo| repo.name == REPOSITORY)
        .unwrap()
        .clone();
    let parent = status(&engine, &repository.repository_id, "");
    let child = status(&engine, &repository.repository_id, "child");
    let unborn = status(&engine, &repository.repository_id, "unborn");
    let ordinary = status(&engine, &repository.repository_id, "ordinary");
    assert!(parent.needs_restage && parent.staged);
    assert_eq!(parent.children.len(), 2);
    assert!(child.unstaged && !child.staged && child.head_tree_id.is_some());
    assert!(unborn.staged && unborn.head_tree_id.is_none());
    assert!(!ordinary.initialized);
    assert_eq!(ordinary.owner.unwrap().directory_id, parent.directory_id);
    assert_eq!(
        commit(&mut engine, &parent).unwrap_err().code(),
        storage_engine::ErrorCode::DirectoryBranchConflict
    );

    // A fresh generation may pin a child before reviewing the new boundary.
    stage_child(&mut engine, &parent, &child);
    let pinned = status(&engine, &repository.repository_id, "");
    assert!(pinned.needs_restage);
    assert!(pinned.index_generation > parent.index_generation);
    assert_eq!(
        commit(&mut engine, &pinned).unwrap_err().code(),
        storage_engine::ErrorCode::DirectoryBranchConflict
    );
    engine
        .stage_directory_git_owned(
            &repository.repository_id,
            &DirectoryGitStageOwnedBody {
                directory_id: pinned.directory_id.clone(),
                directory_branch_id: pinned.directory_branch_id.clone().unwrap(),
                expected_tree_id: pinned.working_tree_id.clone().unwrap(),
                expected_index_generation: pinned.index_generation,
                operation_id: OperationId::generate().unwrap(),
            },
            PRINCIPAL,
        )
        .unwrap();
    let reviewed = status(&engine, &repository.repository_id, "");
    assert!(!reviewed.needs_restage);
    assert_eq!(
        reviewed
            .children
            .iter()
            .find(|row| row.directory_id == child.directory_id)
            .unwrap()
            .staged_tree_id,
        child.head_tree_id
    );
    commit(&mut engine, &reviewed).unwrap();
    let committed = status(&engine, &repository.repository_id, "");
    assert!(!committed.staged && !committed.needs_restage);
    assert_eq!(status(&engine, &repository.repository_id, "child"), child);
    assert_eq!(status(&engine, &repository.repository_id, "unborn"), unborn);
    let frozen = &engine.state().directory_trees[committed.head_tree_id.as_ref().unwrap()];
    assert_eq!(frozen.directory_refs.len(), 1);
    assert_eq!(
        frozen.directory_refs[&child.directory_id].tree_id,
        *child.head_tree_id.as_ref().unwrap()
    );
    assert!(
        !frozen
            .entries
            .iter()
            .any(|entry| entry.path().starts_with("unborn/"))
    );
    let frozen_child = frozen
        .entries
        .iter()
        .find(|entry| entry.path() == CHILD_FILE)
        .unwrap();
    assert_eq!(bytes(&engine, &frozen_child.version_id), COMMITTED);
    let current = engine.list_tree(&repository.main_branch_id).unwrap();
    let current_child = current
        .iter()
        .find(|entry| entry.path() == CHILD_FILE)
        .unwrap();
    assert_eq!(bytes(&engine, &current_child.version_id), WORKING);

    engine
        .stage_directory_git_owned(
            &repository.repository_id,
            &DirectoryGitStageOwnedBody {
                directory_id: child.directory_id.clone(),
                directory_branch_id: child.directory_branch_id.clone().unwrap(),
                expected_tree_id: child.working_tree_id.clone().unwrap(),
                expected_index_generation: child.index_generation,
                operation_id: OperationId::generate().unwrap(),
            },
            PRINCIPAL,
        )
        .unwrap();
    let staged_child = status(&engine, &repository.repository_id, "child");
    assert!(staged_child.staged && !staged_child.unstaged);
    commit(&mut engine, &staged_child).unwrap();
    let new_child = status(&engine, &repository.repository_id, "child");
    assert_ne!(new_child.head_tree_id, child.head_tree_id);
    let unchanged_parent = status(&engine, &repository.repository_id, "");
    assert!(!unchanged_parent.staged && !unchanged_parent.needs_restage);
    let reference = unchanged_parent
        .children
        .iter()
        .find(|row| row.directory_id == child.directory_id)
        .unwrap();
    assert!(reference.reference_changed && !reference.dirty && !reference.staged);
    assert_eq!(reference.committed_tree_id, child.head_tree_id);
    assert_eq!(reference.staged_tree_id, child.head_tree_id);
    stage_child(&mut engine, &unchanged_parent, &new_child);
    let reviewed = status(&engine, &repository.repository_id, "");
    commit(&mut engine, &reviewed).unwrap();
    let final_parent = status(&engine, &repository.repository_id, "");
    assert_eq!(
        final_parent
            .children
            .iter()
            .find(|row| row.directory_id == child.directory_id)
            .unwrap()
            .committed_tree_id,
        new_child.head_tree_id
    );
    assert!(!final_parent.staged && !final_parent.needs_restage);
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    assert_eq!(status(&engine, &repository.repository_id, ""), final_parent);
    assert_eq!(
        status(&engine, &repository.repository_id, "child"),
        new_child
    );
    assert_eq!(status(&engine, &repository.repository_id, "unborn"), unborn);
}
