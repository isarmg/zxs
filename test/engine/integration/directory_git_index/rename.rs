//! Real namespace/workspace publication at initialized directory boundaries.
use storage_engine as engine_api;
#[path = "../../support/container_tags.rs"]
mod container_tags_fixture;
use super::*;
use std::os::unix::fs::MetadataExt;
use storage_contract::{Entry, NamespaceOperation, NamespaceOperationKind};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}

fn entry(engine: &Engine, repo: &Repository, path: &str) -> Entry {
    engine
        .namespace_tree(&repo.main_branch_id)
        .unwrap()
        .get(path)
        .unwrap()
        .clone()
}

fn rename(
    engine: &mut Engine,
    repo: &Repository,
    source: &str,
    destination: &str,
    operation: OperationId,
) -> Result<OperationResult> {
    engine.file_namespace(
        &repo.repository_id,
        NamespaceOperation::parse(NamespaceOperationKind::Rename, source, Some(destination))
            .unwrap(),
        "admin",
        operation,
    )
}

#[test]
fn directory_git_rename_uncommitted_round_trip_keeps_history_tags_and_container() {
    let (temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "box/keep", b"other file");
    initialize(&mut engine, &repo, "box");
    commit(&mut engine, &repo, "box");
    save(&mut engine, &repo, "outside", b"first body");
    let first = entry(&engine, &repo, "outside");
    save(&mut engine, &repo, "outside", b"second body");
    let source = entry(&engine, &repo, "outside");
    let context = engine.file_context(&repo.repository_id, "outside").unwrap();
    let node = engine.file_branch_head(&context.branch_id).unwrap();
    container_tags_fixture::edit(&mut engine, &source.file_id, &["retained".into()], &[], "admin", op())
        .unwrap();
    let metadata = engine.state().file_metadata_nodes.clone();
    let versions = engine.file_history(&source.file_id).unwrap();
    let inode = engine
        .open_content(&source.version_id)
        .unwrap()
        .file
        .metadata()
        .unwrap()
        .ino();
    rename(&mut engine, &repo, "outside", "box/inside", op()).unwrap();
    stage_owned(&mut engine, &repo, "box");
    let operation = op();
    let receipt = rename(&mut engine, &repo, "box/inside", "returned", operation).unwrap();
    let returned = entry(&engine, &repo, "returned");
    assert_eq!(returned.file_id, source.file_id);
    assert_eq!(returned.entry_id, source.entry_id);
    assert_eq!(returned.version_id, source.version_id);
    assert_eq!(engine.file_history(&source.file_id).unwrap(), versions);
    assert_eq!(engine.state().file_metadata_nodes, metadata);
    assert_eq!(version_bytes(&engine, &first.version_id), b"first body");
    assert_eq!(bytes(&engine, &repo, "returned"), b"second body");
    assert_eq!(
        engine
            .open_content(&returned.version_id)
            .unwrap()
            .file
            .metadata()
            .unwrap()
            .ino(),
        inode,
    );
    assert!(
        engine
            .namespace_tree(&repo.main_branch_id)
            .unwrap()
            .get("box/inside")
            .is_none()
    );
    // A later explicit commit may still publish the previously reviewed index.
    // It must not retroactively change the already successful move's receipt.
    let reviewed = status(&engine, &repo, "box");
    let committed = commit_reviewed(&mut engine, &repo, &reviewed, op()).unwrap();
    assert!(
        engine.state().directory_trees[committed.result["head_tree_id"].as_str().unwrap()]
            .entries
            .iter()
            .any(|entry| entry.file_id == source.file_id)
    );
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    let before_replay = storage_contract::canonical_json(engine.state()).unwrap();
    assert_eq!(
        rename(&mut engine, &repo, "box/inside", "returned", operation).unwrap(),
        receipt,
    );
    assert_eq!(
        storage_contract::canonical_json(engine.state()).unwrap(),
        before_replay
    );
    assert_eq!(entry(&engine, &repo, "returned"), returned);
    assert_eq!(version_bytes(&engine, &first.version_id), b"first body");
    assert_eq!(engine.state().file_metadata_nodes, metadata);
}

#[test]
fn directory_git_rename_committed_export_rejects_without_publication() {
    let (temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "box/file", b"retained body");
    initialize(&mut engine, &repo, "box");
    commit(&mut engine, &repo, "box");
    let before = storage_contract::canonical_json(engine.state()).unwrap();
    let source = entry(&engine, &repo, "box/file");
    let operation = op();
    assert_eq!(
        rename(&mut engine, &repo, "box/file", "outside", operation)
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedOperation,
    );
    assert_eq!(
        storage_contract::canonical_json(engine.state()).unwrap(),
        before
    );
    assert_eq!(entry(&engine, &repo, "box/file"), source);
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        rename(&mut engine, &repo, "box/file", "outside", operation)
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedOperation,
    );
    assert_eq!(entry(&engine, &repo, "box/file"), source);
    assert_eq!(bytes(&engine, &repo, "box/file"), b"retained body");
    assert!(
        engine
            .namespace_tree(&repo.main_branch_id)
            .unwrap()
            .get("outside")
            .is_none()
    );
}

#[test]
fn directory_git_rename_saved_after_reviewed_index_is_not_committed_by_that_index() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "box/keep", b"reviewed");
    initialize(&mut engine, &repo, "box");
    stage_owned(&mut engine, &repo, "box");
    save(&mut engine, &repo, "box/later", b"saved after staging");
    let source = entry(&engine, &repo, "box/later");
    let current = status(&engine, &repo, "box");
    let receipt = commit_reviewed(&mut engine, &repo, &current, op()).unwrap();
    let tree = &engine.state().directory_trees[receipt.result["head_tree_id"].as_str().unwrap()];
    assert!(
        tree.entries
            .iter()
            .all(|entry| entry.file_id != source.file_id)
    );
    rename(&mut engine, &repo, "box/later", "outside", op()).unwrap();
    assert_eq!(entry(&engine, &repo, "outside").file_id, source.file_id);
    assert_eq!(bytes(&engine, &repo, "outside"), b"saved after staging");
}

#[test]
fn directory_git_rename_workspace_replace_cannot_remove_destination_before_rejection() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "box/file", b"source");
    save(&mut engine, &repo, "outside", b"destination");
    initialize(&mut engine, &repo, "box");
    commit(&mut engine, &repo, "box");
    let source = entry(&engine, &repo, "box/file");
    let target = entry(&engine, &repo, "outside");
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    for operation in [
        NamespaceOperation::parse(NamespaceOperationKind::Delete, "outside", None).unwrap(),
        NamespaceOperation::parse(NamespaceOperationKind::Rename, "box/file", Some("outside"))
            .unwrap(),
    ] {
        engine
            .namespace_operation(&workspace.workspace_id, operation)
            .unwrap();
    }
    let before = storage_contract::canonical_json(engine.state()).unwrap();
    assert_eq!(
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: op(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "replace must remain atomic".into(),
                request_digest: None,
            })
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedOperation,
    );
    assert_eq!(
        storage_contract::canonical_json(engine.state()).unwrap(),
        before
    );
    assert_eq!(entry(&engine, &repo, "box/file"), source);
    assert_eq!(entry(&engine, &repo, "outside"), target);
    assert_eq!(bytes(&engine, &repo, "outside"), b"destination");
}

#[test]
fn directory_git_rename_tracks_repository_identity_when_its_directory_moves() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "outer/box/sub/file", b"retained");
    initialize(&mut engine, &repo, "outer/box");
    commit(&mut engine, &repo, "outer/box");
    let directory = status(&engine, &repo, "outer/box").directory_id;
    let source = entry(&engine, &repo, "outer/box/sub/file");
    rename(&mut engine, &repo, "outer/box", "moved", op()).unwrap();
    assert_eq!(status(&engine, &repo, "moved").directory_id, directory);
    assert_eq!(
        entry(&engine, &repo, "moved/sub/file").file_id,
        source.file_id
    );
    let before = storage_contract::canonical_json(engine.state()).unwrap();
    assert_eq!(
        rename(&mut engine, &repo, "moved/sub", "outside", op())
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedOperation,
    );
    assert_eq!(
        storage_contract::canonical_json(engine.state()).unwrap(),
        before
    );
    rename(&mut engine, &repo, "moved/sub/file", "moved/renamed", op()).unwrap();
    assert_eq!(
        entry(&engine, &repo, "moved/renamed").file_id,
        source.file_id
    );
}

#[test]
fn directory_git_rename_checks_other_branch_commits_not_only_current_head() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "box/keep", b"base");
    initialize(&mut engine, &repo, "box");
    commit(&mut engine, &repo, "box");
    let main = status(&engine, &repo, "box").directory_branch_id.unwrap();
    let topic = engine
        .create_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("box").unwrap()),
            &storage_contract::BranchName::parse("topic").unwrap(),
            "admin",
            op(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    engine
        .select_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("box").unwrap()),
            &storage_contract::BranchName::parse("topic").unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    save(&mut engine, &repo, "box/file", b"only committed on topic");
    let source = entry(&engine, &repo, "box/file");
    commit(&mut engine, &repo, "box");
    engine
        .select_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("box").unwrap()),
            &storage_contract::BranchName::parse("main").unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    // An explicit branch transfer is not the SMB ordinary rename operation.
    // It leaves a live file whose current Git HEAD has never contained it.
    let request = storage_contract::DirectoryNamespaceBatch {
        kind: storage_contract::DirectoryBatchKind::Move,
        items: vec![storage_contract::DirectoryNamespaceItem {
            source: storage_contract::DirectoryNamespaceSource {
                directory_branch_id: topic.clone(),
                expected_tree_id: engine.state().directory_branches[&topic]
                    .head_tree_id
                    .clone(),
                path: "box/file".into(),
                entry_id: source.entry_id.clone(),
            },
            target: Some(storage_contract::DirectoryNamespaceTarget {
                directory_branch_id: main,
                expected_tree_id: status(&engine, &repo, "box").working_tree_id.unwrap(),
                path: "box/file".into(),
            }),
        }],
    };
    engine
        .directory_namespace_batch(&repo.repository_id, &request, "admin", op())
        .unwrap();
    let head = status(&engine, &repo, "box").head_tree_id.unwrap();
    assert!(
        engine.state().directory_trees[&head]
            .entries
            .iter()
            .all(|entry| entry.file_id != source.file_id)
    );
    assert_eq!(entry(&engine, &repo, "box/file").file_id, source.file_id);
    let before = storage_contract::canonical_json(engine.state()).unwrap();
    assert_eq!(
        rename(&mut engine, &repo, "box/file", "outside", op())
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedOperation,
    );
    assert_eq!(
        storage_contract::canonical_json(engine.state()).unwrap(),
        before
    );
}

#[test]
fn directory_git_rename_same_final_path_cannot_hide_changed_repository_owner() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "box/file", b"retained");
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "other", None).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    initialize(&mut engine, &repo, "box");
    commit(&mut engine, &repo, "box");
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    for (source, destination) in [("box", "held"), ("other", "box"), ("held/file", "box/file")] {
        engine
            .namespace_operation(
                &workspace.workspace_id,
                NamespaceOperation::parse(
                    NamespaceOperationKind::Rename,
                    source,
                    Some(destination),
                )
                .unwrap(),
            )
            .unwrap();
    }
    let before = storage_contract::canonical_json(engine.state()).unwrap();
    assert_eq!(
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: op(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "same spelling does not preserve ownership".into(),
                request_digest: None,
            })
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedOperation,
    );
    assert_eq!(
        storage_contract::canonical_json(engine.state()).unwrap(),
        before
    );
}

#[test]
fn directory_git_rename_new_file_at_old_committed_path_is_not_the_historical_file() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "box/file", b"historical identity");
    initialize(&mut engine, &repo, "box");
    commit(&mut engine, &repo, "box");
    let original = entry(&engine, &repo, "box/file");
    let historical_head = status(&engine, &repo, "box").head_tree_id.unwrap();
    engine
        .move_object(
            &storage_contract::file_relocation::ObjectMoveRequest {
                source_repository_id: HistoryRepositoryId::parse(&repo.repository_id).unwrap(),
                source_entry_id: storage_contract::file_relocation::NamespaceEntryId::parse(
                    &original.entry_id,
                )
                .unwrap(),
                source_path: name_policy::LookupPath::parse("box/file").unwrap(),
                target_slot_id: repo.primary_slot_id.clone(),
                target_repository_id: Some(
                    HistoryRepositoryId::parse(&repo.repository_id).unwrap(),
                ),
                target_path: name_policy::PortablePath::parse("retained").unwrap(),
                replace: false,
                target_entry_id: None,
            },
            "admin",
            op(),
        )
        .unwrap();
    assert!(
        engine
            .namespace_tree(&repo.main_branch_id)
            .unwrap()
            .get("box/file")
            .is_none()
    );
    assert_eq!(bytes(&engine, &repo, "retained"), b"historical identity");
    assert_eq!(
        engine.state().directory_trees[&historical_head]
            .entries
            .iter()
            .find(|entry| entry.path() == "box/file"),
        Some(&original)
    );
    save(&mut engine, &repo, "box/file", b"new identity");
    let fresh = entry(&engine, &repo, "box/file");
    assert_ne!(fresh.file_id, original.file_id);
    rename(&mut engine, &repo, "box/file", "outside", op()).unwrap();
    assert_eq!(entry(&engine, &repo, "outside").file_id, fresh.file_id);
    assert_eq!(bytes(&engine, &repo, "outside"), b"new identity");
    assert_eq!(
        version_bytes(&engine, &original.version_id),
        b"historical identity"
    );
}
