use super::wire;
use crate::engine_backend::file_manager::activity;
use json_codec::Value;
use storage_contract::file_relocation::{NamespaceEntryId, ObjectMoveFact, ObjectMoveRequest};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};
use storage_contract::{CommitRequest, Config, FileDeletionNode, OperationId, OperationResult};
use storage_engine::{DeleteFileRequest, Engine, FileDeletionTarget, UndoFileDeletionRequest};

fn seed(root: &std::path::Path) -> (Engine, HistoryRepositoryId, HistoryFileId) {
    let mut config = Config::development([root.join("disk"), root.join("target")]).unwrap();
    config.disks[1].level = 1;
    config.disks[1].upstream_slot_id = None;
    let mut engine = Engine::initialize(&root.join("system"), config).unwrap();
    let repo = engine
        .create_disk_root("P01", "writer", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "writer")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "original.txt", b"retained body")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "writer".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "seed activity".into(),
            request_digest: None,
        })
        .unwrap();
    let entry = engine
        .namespace_tree(&repo.main_branch_id)
        .unwrap()
        .get("original.txt")
        .unwrap()
        .clone();
    (
        engine,
        HistoryRepositoryId::parse(&repo.repository_id).unwrap(),
        HistoryFileId::parse(&entry.file_id).unwrap(),
    )
}

fn rows(engine: &Engine) -> Vec<Value> {
    wire(activity::read(engine).unwrap())["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["action"] == "file.delete" || row["action"] == "file.delete.undo")
        .cloned()
        .collect()
}

fn assert_native_event(
    rows: &[Value],
    operation: &OperationResult,
    node: &FileDeletionNode,
    actor: &str,
    timestamp: u64,
) {
    let matching = rows
        .iter()
        .filter(|row| row["sequence"] == operation.sequence.to_string())
        .collect::<Vec<_>>();
    assert_eq!(matching.len(), 1);
    let event = matching[0];
    assert_eq!(event["action"], operation.kind);
    assert_eq!(event["principal"], actor);
    assert_eq!(event["file_id"], node.file_id);
    assert_eq!(event["path"], node.path.as_str());
    assert_eq!(event["node_id"], node.node_id);
    assert_eq!(
        event["timestamp"],
        crate::backend_activity::utc_timestamp(timestamp).unwrap()
    );
    assert!(event["version_id"].is_null());
}

#[test]
fn original_lifecycle_activity_retains_each_actor_and_survives_cold_reopen() {
    let root = crate::tests::private_tempdir();
    let (mut engine, repository_id, file_id) = seed(root.path());
    let path = name_policy::LookupPath::parse("original.txt").unwrap();
    let deletion_id = OperationId::generate().unwrap();
    let request = || DeleteFileRequest {
        target: FileDeletionTarget {
            repository_id: &repository_id,
            file_id: &file_id,
            path: &path,
        },
        principal: "deleter",
        operation_id: deletion_id,
    };
    let deleted = engine.soft_delete_file(request()).unwrap();
    let node: FileDeletionNode =
        json_codec::from_value(deleted.result["deletion_node"].clone()).unwrap();
    let restored = engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &file_id,
            deletion_node_id: &node.node_id,
            principal: "restorer",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    assert!(engine.file_deletion(file_id.as_str()).is_none());
    let events = rows(&engine);
    assert_eq!(events.len(), 2);
    assert_ne!(events[0]["event_id"], events[1]["event_id"]);
    assert_native_event(&events, &deleted, &node, "deleter", node.created_at);
    assert_native_event(
        &events,
        &restored,
        &node,
        "restorer",
        restored.result["timestamp"].as_str().unwrap().parse().unwrap(),
    );
    drop(engine);
    let mut engine = Engine::open(&root.path().join("system")).unwrap();
    assert_eq!(rows(&engine), events);
    assert_eq!(engine.soft_delete_file(request()).unwrap(), deleted);
    assert!(engine.file_deletion(file_id.as_str()).is_none());
    assert_eq!(rows(&engine), events);
}

#[test]
fn original_lifecycle_activity_survives_cross_repository_move_and_cold_reopen() {
    let root = crate::tests::private_tempdir();
    let (mut engine, repository_id, file_id) = seed(root.path());
    let path = name_policy::LookupPath::parse("original.txt").unwrap();
    let deleted = engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &repository_id,
                file_id: &file_id,
                path: &path,
            },
            principal: "deleter",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    let node: FileDeletionNode =
        json_codec::from_value(deleted.result["deletion_node"].clone()).unwrap();
    engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &file_id,
            deletion_node_id: &node.node_id,
            principal: "restorer",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    let events = rows(&engine);
    assert_eq!(events.len(), 2);
    let source = engine.state().repositories[repository_id.as_str()].clone();
    let entry = engine
        .namespace_tree(&source.main_branch_id)
        .unwrap()
        .get(path.as_str())
        .unwrap()
        .clone();
    let moved = engine
        .move_object(
            &ObjectMoveRequest {
                source_repository_id: repository_id.clone(),
                source_entry_id: NamespaceEntryId::parse(&entry.entry_id).unwrap(),
                source_path: path,
                target_slot_id: "B01".into(),
                target_repository_id: None,
                target_path: name_policy::PortablePath::parse("moved.txt").unwrap(),
                replace: false,
                target_entry_id: None,
            },
            "mover",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let fact: ObjectMoveFact = json_codec::from_value(moved.result["movement"].clone()).unwrap();
    let target = &engine.state().repositories[fact.target_repository_id.as_str()];
    let moved_entry = engine
        .namespace_tree(&target.main_branch_id)
        .unwrap()
        .get("moved.txt")
        .unwrap()
        .clone();
    assert_ne!(target.repository_id, source.repository_id);
    assert_ne!(target.history_tree_id, node.history_tree_id);
    assert_ne!(moved_entry.file_id, file_id.as_str());
    assert_ne!(moved_entry.entry_id, entry.entry_id);
    assert_ne!(moved_entry.version_id, entry.version_id);
    assert!(
        engine
            .namespace_tree(&source.main_branch_id)
            .unwrap()
            .get("original.txt")
            .is_none()
    );
    assert_eq!(rows(&engine), events);
    assert!(events.iter().all(|row| row["path"] == "original.txt"));
    assert!(events.iter().all(|row| row["slot_id"] == "P01"));
    drop(engine);
    let engine = Engine::open(&root.path().join("system")).unwrap();
    assert_eq!(rows(&engine), events);
    assert_eq!(
        engine.state().histories[&node.history_tree_id].repository_id,
        source.repository_id
    );
    assert_eq!(
        engine.state().repositories[repository_id.as_str()].history_tree_id,
        node.history_tree_id
    );
}
