//! Retained directory receipts project a group event even without file members.
use super::wire;
use crate::engine_backend::file_manager::activity;
use json_codec::Value;
use storage_contract::{
    CommitRequest, Config, FileDeletionNode, NamespaceOperation, NamespaceOperationKind,
    OperationId, OperationResult, Repository,
    directory_git::{DirectoryDeletionNode, DirectoryGitInitializeBody, UndoDirectoryDeletionBody},
    history_window::{HistoryFileId, HistoryRepositoryId},
};
use storage_engine::{DeleteFileRequest, Engine, FileDeletionTarget};

fn seed(root: &std::path::Path, members: bool) -> (Engine, Repository) {
    let config = Config::development((0..3).map(|n| root.join(format!("disk-{n}")))).unwrap();
    let mut engine = Engine::initialize(&root.join("system"), config).unwrap();
    let repo = engine
        .create_disk_root("P01", "writer", OperationId::generate().unwrap())
        .unwrap();
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "group", None).unwrap(),
            "writer",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    if members {
        let workspace = engine.create_workspace(&repo.main_branch_id, "writer").unwrap();
        for path in ["group/earlier", "group/current"] {
            engine.stage_bytes(&workspace.workspace_id, path, path.as_bytes()).unwrap();
        }
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "writer".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "directory audit".into(),
                request_digest: None,
            })
            .unwrap();
    }
    let head = engine.state().branches[&repo.main_branch_id].head_commit_id.clone();
    engine
        .initialize_directory_git(
            &repo.repository_id,
            &DirectoryGitInitializeBody {
                path: String::new(),
                expected_namespace_head: head,
                operation_id: OperationId::generate().unwrap(),
            },
            "writer",
        )
        .unwrap();
    (engine, repo)
}

fn rows(engine: &Engine) -> Vec<Value> {
    wire(activity::read(engine).unwrap())["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| {
            row["action"] == "directory.delete" || row["action"] == "directory.delete.undo"
        })
        .cloned()
        .collect()
}

fn assert_event(
    events: &[Value],
    operation: &OperationResult,
    node: &DirectoryDeletionNode,
    actor: &str,
    timestamp: u64,
) {
    let matching = events
        .iter()
        .filter(|row| row["sequence"] == operation.sequence.to_string())
        .collect::<Vec<_>>();
    assert_eq!(matching.len(), 1);
    let row = matching[0];
    assert_eq!(row["event_id"], format!("{}:{}", operation.operation_id, node.node_id));
    assert_eq!(row["action"], operation.kind);
    assert_eq!(row["slot_id"], "P01");
    assert_eq!(row["path"], node.path.as_str());
    assert_eq!(row["principal"], actor);
    assert_eq!(row["directory_branch_id"], node.directory_branch_id);
    assert_eq!(row["head_tree_id"], node.source_tree_id);
    assert_eq!(row["timestamp"], crate::backend_activity::utc_timestamp(timestamp).unwrap());
    assert_eq!(row["file_id"], "");
    assert!(row["version_id"].is_null());
    assert!(row.get("node_id").is_none());
}

fn delete_prior_file(engine: &mut Engine, repo: &Repository) -> FileDeletionNode {
    let entry = engine
        .namespace_tree(&repo.main_branch_id)
        .unwrap()
        .get("group/earlier")
        .unwrap()
        .clone();
    let deleted = engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &HistoryRepositoryId::parse(&repo.repository_id).unwrap(),
                file_id: &HistoryFileId::parse(&entry.file_id).unwrap(),
                path: &name_policy::LookupPath::parse(entry.path()).unwrap(),
            },
            principal: "earlier-deleter",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    json_codec::from_value(deleted.result["deletion_node"].clone()).unwrap()
}

fn lifecycle(members: bool) {
    let root = crate::tests::private_tempdir();
    let (mut engine, repo) = seed(root.path(), members);
    let earlier = members.then(|| delete_prior_file(&mut engine, &repo));
    let entry = engine
        .namespace_tree(&repo.main_branch_id)
        .unwrap()
        .get("group")
        .unwrap()
        .clone();
    let request = engine
        .prepare_current_directory_deletion(
            &repo.repository_id,
            &name_policy::LookupPath::parse("group").unwrap(),
            &entry.entry_id,
        )
        .unwrap()
        .unwrap();
    let deleted = engine
        .directory_namespace_batch(
            &repo.repository_id,
            &request,
            "deleter",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let node: DirectoryDeletionNode =
        json_codec::from_value(deleted.result["directory_deletion"].clone()).unwrap();
    for value in [json_codec::json!({}), json_codec::json!({"directory_deletion": null})] {
        let mut malformed = deleted.clone();
        malformed.result = value;
        let error = activity::deletion::record_operation(
            &engine,
            &mut std::collections::BTreeMap::new(),
            &malformed,
        )
        .unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::StateInvalid);
        assert_eq!(error.status, ::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert!(std::error::Error::source(&error).is_some());
    }
    assert_eq!(node.files.len(), if members { 2 } else { 0 });
    if let Some(earlier) = &earlier {
        assert!(!node.files[&earlier.file_id].created_by_directory);
        assert_eq!(node.files[&earlier.file_id].deletion_node_id, earlier.node_id);
        assert_eq!(
            node.files.values().filter(|member| member.created_by_directory).count(),
            1
        );
    }
    let events = rows(&engine);
    assert_eq!(events.len(), 1);
    assert_event(&events, &deleted, &node, "deleter", node.created_at);
    assert!(engine.namespace_tree(&repo.main_branch_id).unwrap().get("group").is_none());
    drop(engine);
    let mut engine = Engine::open(&root.path().join("system")).unwrap();
    assert_eq!(rows(&engine), events);
    assert_eq!(engine.state().operations[&deleted.operation_id], deleted);
    let undo = UndoDirectoryDeletionBody {
        owner_directory_id: node.owner_directory_id.clone(),
        directory_id: node.directory_id.clone(),
        deletion_node_id: node.node_id.clone(),
        operation_id: OperationId::generate().unwrap(),
    };
    let restored = engine
        .undo_directory_deletion(&repo.repository_id, &undo, "restorer")
        .unwrap();
    let events = rows(&engine);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["sequence"], restored.sequence.to_string());
    assert_eq!(events[1]["sequence"], deleted.sequence.to_string());
    assert_event(&events, &deleted, &node, "deleter", node.created_at);
    assert_event(
        &events,
        &restored,
        &node,
        "restorer",
        restored.result["timestamp"].as_str().unwrap().parse().unwrap(),
    );
    assert!(
        engine
            .directory_deletion(&repo.repository_id, &node.owner_directory_id, &node.directory_id)
            .is_none()
    );
    assert_eq!(
        engine.namespace_tree(&repo.main_branch_id).unwrap().get("group"),
        Some(&entry)
    );
    if let Some(earlier) = &earlier {
        assert_eq!(engine.file_deletion(&earlier.file_id), Some(earlier));
        let tree = engine.namespace_tree(&repo.main_branch_id).unwrap();
        assert!(tree.get("group/earlier").is_none());
        assert!(tree.get("group/current").is_some());
    }
    if members {
        let all = wire(activity::read(&engine).unwrap());
        let all = all["events"].as_array().unwrap();
        assert_eq!(all.iter().filter(|row| row["action"] == "file.delete").count(), 1);
        assert!(all.iter().all(|row| row["action"] != "file.delete.undo"));
    } else {
        engine
            .file_namespace(
                &repo.repository_id,
                NamespaceOperation::parse(
                    NamespaceOperationKind::Rename,
                    "group",
                    Some("renamed"),
                )
                .unwrap(),
                "mover",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        assert!(engine.namespace_tree(&repo.main_branch_id).unwrap().get("renamed").is_some());
        assert_eq!(rows(&engine), events);
    }
    assert_eq!(
        engine
            .directory_namespace_batch(
                &repo.repository_id,
                &request,
                "deleter",
                deleted.operation_id,
            )
            .unwrap(),
        deleted
    );
    assert_eq!(
        engine.undo_directory_deletion(&repo.repository_id, &undo, "restorer").unwrap(),
        restored
    );
    assert_eq!(rows(&engine), events);
    drop(engine);
    let engine = Engine::open(&root.path().join("system")).unwrap();
    assert_eq!(rows(&engine), events);
    assert_eq!(engine.state().operations[&deleted.operation_id], deleted);
    assert_eq!(engine.state().operations[&restored.operation_id], restored);
}

#[test]
fn directory_lifecycle_activity_retains_original_group_and_prior_file_deletion() {
    lifecycle(true);
}

#[test]
fn empty_directory_lifecycle_activity_survives_undo_replay_and_cold_reopen() {
    lifecycle(false);
}
