//! Published retained copies and imported D records through the lifecycle reader.
use crate::{Fixture, entry, op, request, save};
use storage_contract::{
    BranchName, ErrorCode,
    file_lifecycle::{Action, Page, Request},
    file_relocation::{NamespaceEntryId, ObjectMoveRequest},
    history_window::{HistoryFileId, HistoryRepositoryId, HistoryWindowCancellation},
};
use storage_engine::{DeleteFileRequest, Engine, FileDeletionTarget, Repository, UndoFileDeletionRequest};

fn page(engine: &Engine, repository: &Repository, file: &str) -> Page {
    let root = engine.metadata_view().history_root(&repository.history_tree_id).unwrap().to_owned();
    let request = Request::parse(file, &root, None, None, None, 64).unwrap();
    let read = engine.capture_file_lifecycle(request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    let page = read.page().unwrap();
    engine.recheck_file_lifecycle(&read).unwrap();
    page.validate(&request).unwrap();
    page
}
fn delete(engine: &mut Engine, repository: &Repository, file: &storage_contract::Entry) {
    engine.soft_delete_file(DeleteFileRequest {
        target: FileDeletionTarget {
            repository_id: &HistoryRepositoryId::parse(&repository.repository_id).unwrap(),
            file_id: &HistoryFileId::parse(&file.file_id).unwrap(),
            path: &name_policy::LookupPath::parse(file.path()).unwrap(),
        }, principal: "admin", operation_id: op(),
    }).unwrap();
}
#[test]
fn file_lifecycle_retained_copy_has_original_operations_and_graph_only_copy_refuses_unknown_prefix() {
    for current in [false, true] {
        let Fixture { temp, mut engine, source, target } = Fixture::new(current);
        let original = entry(&engine, &source, "source.txt");
        engine.create_file_branch(&original.file_id, &original.version_id,
            &BranchName::parse("review").unwrap(), "admin", op()).unwrap();
        delete(&mut engine, &source, &original);
        let deletion = engine.file_deletion(&original.file_id).unwrap().clone();
        engine.undo_file_deletion(UndoFileDeletionRequest {
            file_id: &HistoryFileId::parse(&original.file_id).unwrap(),
            deletion_node_id: &deletion.node_id, principal: "admin", operation_id: op(),
        }).unwrap();
        let expected = page(&engine, &source, &original.file_id).events;
        engine.copy_retained_file_history(&request(&engine, &source, &target), "admin", op()).unwrap();
        let copied = entry(&engine, &target, "copied.txt");
        let actual = page(&engine, &target, &copied.file_id).events;
        assert_eq!(actual.len(), 2);
        for (original, copied) in expected.iter().zip(&actual) {
            assert_eq!(original.operation_id, copied.operation_id);
            assert_eq!(original.sequence, copied.sequence);
            assert_eq!(original.action, copied.action);
            assert_eq!(original.occurred_at, copied.occurred_at);
            assert_ne!(original.deletion_id, copied.deletion_id);
        }
        if !current {
            let graph_target = engine.create_disk_root("D05", "admin", op()).unwrap();
            let mut graph_only = request(&engine, &source, &graph_target);
            graph_only.target_name = "graph.txt".into();
            engine.copy_file_history(&graph_only, "admin", op()).unwrap();
            let graph = entry(&engine, &graph_target, "graph.txt");
            let root = engine.metadata_view().history_root(&graph_target.history_tree_id)
                .unwrap().to_owned();
            let request = Request::parse(&graph.file_id, &root, None, None, None, 64).unwrap();
            let read = engine.capture_file_lifecycle(request, HistoryWindowCancellation::new()).unwrap();
            assert_eq!(read.page().unwrap_err().code(), ErrorCode::PublicationInvalid);
            drop(read);
        }
        drop(engine);
        let engine = Engine::open(&temp.path().join("system")).unwrap();
        assert_eq!(page(&engine, &target, &copied.file_id).events, actual);
    }
}

#[test]
fn file_lifecycle_cross_directory_import_is_distinct_without_a_fabricated_timestamp() {
    let Fixture { temp, mut engine, source, target } = Fixture::new(false);
    save(&mut engine, &source, "folder/hidden.txt", b"retained hidden body");
    let file = entry(&engine, &source, "folder/hidden.txt");
    engine.create_file_branch(&file.file_id, &file.version_id,
        &BranchName::parse("review").unwrap(), "admin", op()).unwrap();
    delete(&mut engine, &source, &file);
    let before = page(&engine, &source, &file.file_id);
    let folder = entry(&engine, &source, "folder");
    let operation = op();
    let request = ObjectMoveRequest {
        source_repository_id: HistoryRepositoryId::parse(&source.repository_id).unwrap(),
        source_entry_id: NamespaceEntryId::parse(&folder.entry_id).unwrap(),
        source_path: name_policy::LookupPath::parse("folder").unwrap(),
        target_slot_id: target.primary_slot_id.clone(),
        target_repository_id: Some(HistoryRepositoryId::parse(&target.repository_id).unwrap()),
        target_path: name_policy::PortablePath::parse("arrived").unwrap(),
        replace: false, target_entry_id: None,
    };
    engine.move_object(&request, "admin", operation).unwrap();
    let imported = engine.state().file_deletions.values().find(|node| {
        node.history_tree_id == target.history_tree_id && node.path.as_str() == "arrived/hidden.txt"
    }).unwrap().clone();
    let result = page(&engine, &target, &imported.file_id);
    assert_eq!(result.events.len(), 2);
    assert_eq!(result.events[0].operation_id, before.events[0].operation_id);
    assert_eq!(result.events[0].action, Action::Delete);
    assert_eq!(result.events[1].operation_id, operation);
    assert_eq!(result.events[1].action, Action::Import);
    assert!(result.events[1].occurred_at.is_none());
    assert_ne!(result.events[0].deletion_id, result.events[1].deletion_id);
    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(page(&engine, &target, &imported.file_id).events, result.events);
}
