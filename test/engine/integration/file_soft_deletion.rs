//! Public production Engine regression. No fault-injection features are required.
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};
use storage_engine::{
    BranchName, CommitRequest, Config, DeleteFileRequest, Engine, ErrorCode, FileDeletionTarget,
    FileNodeTagsUpdateRequest, FileTagTarget, NamespaceOperation, NamespaceOperationKind,
    OperationId, UndoFileDeletionRequest, Workspace,
};

fn request(workspace: Workspace) -> CommitRequest {
    CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "soft deletion regression".into(),
        request_digest: None,
    }
}

#[test]
fn deletion_joins_frontier_preserves_bytes_freezes_only_its_file_and_undo_survives_restart() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let mut engine = Engine::initialize(
        &root,
        Config::development([temp.path().join("disk")]).unwrap(),
    )
    .unwrap();
    let repository = engine
        .create_disk_root("P01", "admin", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "A.txt", b"retained A")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "B.txt", b"retained A")
        .unwrap();
    engine.commit_workspace(request(workspace)).unwrap();
    let original = engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("A.txt")
        .unwrap()
        .clone();
    let other = engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("B.txt")
        .unwrap()
        .clone();
    assert_ne!(original.file_id, other.file_id);
    let second = engine
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            &BranchName::parse("review").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let head = engine.file_branch_head(&second).unwrap();
    let tagged = engine
        .update_file_node_tags(FileNodeTagsUpdateRequest {
            file_id: &original.file_id,
            targets: &[FileTagTarget {
                branch_id: second.clone(),
                source_node_id: head.clone(),
                expected_node_id: head,
            }],
            add: &["reviewed".into()],
            remove: &[],
            location: None,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap()
        .result["nodes"][0]["node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let b_workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&b_workspace.workspace_id, "B.txt", b"B can still save")
        .unwrap();
    let before_namespace = engine
        .namespace_snapshot(&repository.main_branch_id)
        .unwrap();
    let before = engine.state().clone();
    let repository_id = HistoryRepositoryId::parse(&repository.repository_id).unwrap();
    let file_id = HistoryFileId::parse(&original.file_id).unwrap();
    let path = name_policy::LookupPath::parse("a.TXT").unwrap();
    let deletion_operation = OperationId::generate().unwrap();
    let delete = engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &repository_id,
                file_id: &file_id,
                path: &path,
            },
            principal: "admin",
            operation_id: deletion_operation,
        })
        .unwrap();
    let node = engine.file_deletion(&original.file_id).unwrap().clone();
    assert!(!engine.namespace_snapshot_matches(&before_namespace.witness()));
    assert_eq!(node.branch_heads.len(), 2);
    assert_eq!(node.parent_node_ids.len(), 2);
    assert!(node.parent_node_ids.contains(&tagged));
    assert_eq!(engine.state().versions, before.versions);
    assert_eq!(engine.state().placements, before.placements);
    assert_eq!(engine.state().branches, before.branches);
    assert!(
        engine
            .namespace_tree(&repository.main_branch_id)
            .unwrap()
            .get("A.txt")
            .is_none()
    );
    assert_eq!(
        engine
            .namespace_tree(&repository.main_branch_id)
            .unwrap()
            .visible_entries()
            .count(),
        1
    );
    assert!(
        engine
            .namespace_snapshot(&repository.main_branch_id)
            .unwrap()
            .tree()
            .get("A.txt")
            .is_none()
    );
    assert_eq!(
        engine
            .soft_delete_file(DeleteFileRequest {
                target: FileDeletionTarget {
                    repository_id: &repository_id,
                    file_id: &file_id,
                    path: &path
                },
                principal: "admin",
                operation_id: deletion_operation,
            })
            .unwrap(),
        delete
    );
    let export = temp.path().join("deleted-content.bin");
    engine
        .export_version(&original.version_id, &export)
        .unwrap();
    assert_eq!(std::fs::read(export).unwrap(), b"retained A");
    assert_eq!(
        engine
            .create_file_branch(
                &original.file_id,
                &original.version_id,
                &BranchName::parse("blocked").unwrap(),
                "admin",
                OperationId::generate().unwrap()
            )
            .unwrap_err()
            .code(),
        ErrorCode::BranchNotActive
    );
    let tag_error = engine
        .update_file_node_tags(FileNodeTagsUpdateRequest {
            file_id: &original.file_id,
            targets: &[FileTagTarget {
                branch_id: second.clone(),
                source_node_id: tagged.clone(),
                expected_node_id: tagged,
            }],
            add: &["blocked".into()],
            remove: &[],
            location: None,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap_err();
    assert_eq!(tag_error.code(), ErrorCode::BranchNotActive);
    assert_eq!(
        engine
            .file_namespace(
                &repository.repository_id,
                NamespaceOperation::parse(
                    NamespaceOperationKind::Rename,
                    "A.txt",
                    Some("renamed.txt")
                )
                .unwrap(),
                "admin",
                OperationId::generate().unwrap()
            )
            .unwrap_err()
            .code(),
        ErrorCode::BranchNotActive
    );
    engine.commit_workspace(request(b_workspace)).unwrap();
    let b = engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("B.txt")
        .unwrap()
        .clone();
    assert_eq!(b.file_id, other.file_id);
    assert_eq!(b.size, 16);
    engine
        .create_file_branch(
            &b.file_id,
            &b.version_id,
            &BranchName::parse("B review").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    // A new file born while A is deleted is not frozen by shared history or bytes.
    let new_workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&new_workspace.workspace_id, "C.txt", b"retained A")
        .unwrap();
    engine.commit_workspace(request(new_workspace)).unwrap();
    let new_file = engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("C.txt")
        .unwrap()
        .clone();
    assert_ne!(new_file.file_id, original.file_id);
    let new_branch = engine
        .create_file_branch(
            &new_file.file_id,
            &new_file.version_id,
            &BranchName::parse("C review").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let new_workspace = engine.create_workspace(&new_branch, "admin").unwrap();
    engine
        .stage_bytes(&new_workspace.workspace_id, "C.txt", b"new file can edit")
        .unwrap();
    engine.commit_workspace(request(new_workspace)).unwrap();
    let a_workspace = engine.create_workspace(&second, "admin").unwrap();
    engine
        .stage_bytes(&a_workspace.workspace_id, "A.txt", b"must not publish")
        .unwrap();
    let pending = a_workspace.workspace_id.clone();
    assert_eq!(
        engine
            .commit_workspace(request(a_workspace))
            .unwrap_err()
            .code(),
        ErrorCode::BranchNotActive
    );
    engine.abort_workspace(&pending, "admin").unwrap();
    let wrong = format!("fdel_{}", "0".repeat(32));
    let frozen = engine.state().clone();
    assert_eq!(
        engine
            .undo_file_deletion(UndoFileDeletionRequest {
                file_id: &file_id,
                deletion_node_id: &wrong,
                principal: "admin",
                operation_id: OperationId::generate().unwrap()
            })
            .unwrap_err()
            .code(),
        ErrorCode::FileNodeNotFound
    );
    assert_eq!(engine.state(), &frozen);
    drop(engine);
    let mut engine = Engine::open(&root).unwrap();
    assert_eq!(engine.file_deletion(&original.file_id), Some(&node));
    assert!(
        engine
            .namespace_tree(&repository.main_branch_id)
            .unwrap()
            .get("A.txt")
            .is_none()
    );
    let deleted_namespace = engine
        .namespace_snapshot(&repository.main_branch_id)
        .unwrap();
    let restore_operation = OperationId::generate().unwrap();
    let restored = engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &file_id,
            deletion_node_id: &node.node_id,
            principal: "admin",
            operation_id: restore_operation,
        })
        .unwrap();
    assert!(engine.file_deletion(&original.file_id).is_none());
    assert!(!engine.namespace_snapshot_matches(&deleted_namespace.witness()));
    assert_eq!(
        engine
            .namespace_tree(&repository.main_branch_id)
            .unwrap()
            .get("A.txt"),
        Some(&original)
    );
    assert_eq!(
        engine
            .undo_file_deletion(UndoFileDeletionRequest {
                file_id: &file_id,
                deletion_node_id: &node.node_id,
                principal: "admin",
                operation_id: restore_operation
            })
            .unwrap(),
        restored
    );
    engine
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            &BranchName::parse("restored").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &repository_id,
                file_id: &file_id,
                path: &path,
            },
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    let repeated = engine
        .file_deletion(&original.file_id)
        .unwrap()
        .node_id
        .clone();
    assert_ne!(repeated, node.node_id);
    assert_eq!(
        engine
            .undo_file_deletion(UndoFileDeletionRequest {
                file_id: &file_id,
                deletion_node_id: &node.node_id,
                principal: "admin",
                operation_id: OperationId::generate().unwrap(),
            })
            .unwrap_err()
            .code(),
        ErrorCode::FileNodeNotFound
    );
    assert_eq!(
        engine.file_deletion(&original.file_id).unwrap().node_id,
        repeated
    );
    engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &file_id,
            deletion_node_id: &repeated,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    assert!(engine.file_deletion(&original.file_id).is_none());
    assert!(
        engine
            .namespace_tree(&repository.main_branch_id)
            .unwrap()
            .get("A.txt")
            .is_some()
    );
}

#[test]
fn deleted_same_body_branches_cannot_change_selection_or_default_but_other_files_can() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let mut engine = Engine::initialize(
        &root,
        Config::development([temp.path().join("disk")]).unwrap(),
    )
    .unwrap();
    let repository = engine
        .create_disk_root("P01", "admin", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    for path in ["A.txt", "B.txt"] {
        engine
            .stage_bytes(&workspace.workspace_id, path, b"same bytes")
            .unwrap();
    }
    engine.commit_workspace(request(workspace)).unwrap();
    let entry = engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("A.txt")
        .unwrap()
        .clone();
    let other = engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("B.txt")
        .unwrap()
        .clone();
    let second = engine
        .create_file_branch(
            &entry.file_id,
            &entry.version_id,
            &BranchName::parse("same body A").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let other_branch = engine
        .create_file_branch(
            &other.file_id,
            &other.version_id,
            &BranchName::parse("same body B").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let selected_before = engine.state().file_selections[&entry.file_id].clone();
    let node = engine.file_branch_head(&second).unwrap();
    assert_eq!(node, engine.file_branch_head(&selected_before).unwrap());
    let path = name_policy::PortablePath::parse("A.txt").unwrap();
    let lookup = name_policy::LookupPath::from(&path);
    let file = HistoryFileId::parse(&entry.file_id).unwrap();
    let repo = HistoryRepositoryId::parse(&repository.repository_id).unwrap();
    engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &repo,
                file_id: &file,
                path: &lookup,
            },
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    let deletion = engine.file_deletion(&entry.file_id).unwrap().clone();
    let frozen = engine.state().clone();
    let expected_head = frozen.branches[&repository.main_branch_id]
        .head_commit_id
        .clone();
    assert_eq!(
        engine
            .select_file_branch(
                &entry.file_id,
                &second,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::BranchNotActive
    );
    assert_eq!(engine.state(), &frozen);
    assert_eq!(
        engine
            .select_file_branch_at(
                &repository.repository_id,
                &path,
                &entry.file_id,
                &second,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::BranchNotActive
    );
    assert_eq!(engine.state(), &frozen);
    assert_eq!(
        engine
            .set_file_default(storage_engine::FileDefaultSelection {
                file_id: &entry.file_id,
                branch_id: &second,
                repository_id: &repository.repository_id,
                path: &path,
                expected_node_id: &node,
                expected_head: &expected_head,
                principal: "admin",
                operation_id: OperationId::generate().unwrap(),
            })
            .unwrap_err()
            .code(),
        ErrorCode::BranchNotActive
    );
    assert_eq!(engine.state(), &frozen);
    engine
        .select_file_branch(
            &other.file_id,
            &other_branch,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(
        engine.state().file_selections[&entry.file_id],
        selected_before
    );
    assert_eq!(engine.state().file_selections[&other.file_id], other_branch);
    let context_key = format!("{}/{}", repository.repository_id, path.key());
    assert_eq!(
        engine.state().file_contexts.get(&context_key),
        frozen.file_contexts.get(&context_key)
    );
    engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &file,
            deletion_node_id: &deletion.node_id,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    assert_eq!(
        engine.state().file_selections[&entry.file_id],
        selected_before
    );
    assert_eq!(
        engine
            .namespace_tree(&repository.main_branch_id)
            .unwrap()
            .get("A.txt"),
        Some(&entry)
    );
    let expected_head = engine.state().branches[&repository.main_branch_id]
        .head_commit_id
        .clone();
    engine
        .set_file_default(storage_engine::FileDefaultSelection {
            file_id: &entry.file_id,
            branch_id: &second,
            repository_id: &repository.repository_id,
            path: &path,
            expected_node_id: &node,
            expected_head: &expected_head,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    engine
        .select_file_branch_at(
            &repository.repository_id,
            &path,
            &entry.file_id,
            &selected_before,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    engine
        .select_file_branch(
            &entry.file_id,
            &second,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    assert!(engine.file_deletion(&entry.file_id).is_none());
    assert_eq!(engine.state().file_selections[&entry.file_id], second);
    assert_eq!(engine.state().file_selections[&other.file_id], other_branch);
}
