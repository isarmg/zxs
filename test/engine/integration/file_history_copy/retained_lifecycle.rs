//! Real copied audit, independent edits, fixed-prefix recovery and failure points.
use super::*;

fn delete_undo(engine: &mut Engine, repository: &Repository, entry: &Entry) -> FileDeletionNode {
    let node = delete(engine, repository, entry);
    engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &HistoryFileId::parse(&entry.file_id).unwrap(),
            deletion_node_id: &node.node_id,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    node
}
fn audit(path: &Path) -> Vec<json_codec::Value> {
    let mut events: Vec<_> = records(path)
        .into_iter()
        .filter(|((kind, _), _)| kind == "lifecycle")
        .map(|(_, value)| value)
        .collect();
    events.sort_by_key(|event| event["sequence"].as_str().unwrap().parse::<u64>().unwrap());
    events
}

#[test]
fn retained_copy_persists_all_undone_frontiers_without_turning_them_into_live_d() {
    for current in [false, true] {
        let Fixture {
            temp,
            mut engine,
            source,
            target,
        } = Fixture::new(current);
        let original = entry(&engine, &source, "source.txt");
        engine
            .create_file_branch(
                &original.file_id,
                &original.version_id,
                &BranchName::parse("review").unwrap(),
                "admin",
                op(),
            )
            .unwrap();
        let first = delete_undo(&mut engine, &source, &original);
        save(
            &mut engine,
            &source,
            "source.txt",
            b"saved after original D was undone",
        );
        let original = entry(&engine, &source, "source.txt");
        let second = delete_undo(&mut engine, &source, &original);
        assert_ne!(first.branch_heads, second.branch_heads);
        let request = request(&engine, &source, &target);
        let operation = op();
        let receipt = engine
            .copy_retained_file_history(&request, "admin", operation)
            .unwrap();
        assert_eq!(receipt.kind, "file.history.copy.retained");
        assert_eq!(
            receipt.result["retained_lifecycle"]["events"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        let copied = entry(&engine, &target, "copied.txt");
        let db = database(&engine, &target, &copied.file_id, "D04", "copied.txt");
        let imported = audit(&db);
        assert_eq!(
            json_codec::json!(imported),
            receipt.result["retained_lifecycle"]["events"]
        );
        assert_eq!(engine.file_deletion(&copied.file_id), None);
        assert_eq!(engine.file_deletion(&original.file_id), None);
        for event in &imported {
            assert_eq!(event["node"]["file_id"], copied.file_id);
            assert_eq!(event["node"]["history_tree_id"], target.history_tree_id);
            assert!(event["node"].get("path").is_none());
            assert!(event["node"].get("repository_id").is_none());
            assert_eq!(event["node"]["branch_heads"].as_object().unwrap().len(), 2);
        }
        assert_eq!(
            engine
                .undo_file_deletion(UndoFileDeletionRequest {
                    file_id: &HistoryFileId::parse(&copied.file_id).unwrap(),
                    deletion_node_id: imported[0]["node"]["node_id"].as_str().unwrap(),
                    principal: "admin",
                    operation_id: op(),
                })
                .unwrap_err()
                .code(),
            ErrorCode::FileNodeNotFound
        );
        // Later source deletion is not part of the already copied prefix.
        delete(&mut engine, &source, &original);
        save(
            &mut engine,
            &target,
            "copied.txt",
            b"independent destination edit",
        );
        assert_eq!(audit(&db), imported);
        assert_eq!(
            bytes(&engine, &original.version_id),
            b"saved after original D was undone"
        );
        assert_eq!(
            engine
                .copy_retained_file_history(&request, "admin", operation)
                .unwrap(),
            receipt
        );
        drop(engine);
        fs::remove_file(&db).unwrap();
        let mut engine = Engine::open(&temp.path().join("system")).unwrap();
        assert_eq!(audit(&db), imported);
        assert!(engine.file_deletion(&original.file_id).is_some());
        assert!(engine.file_deletion(&copied.file_id).is_none());
        assert_eq!(
            engine
                .copy_retained_file_history(&request, "admin", operation)
                .unwrap(),
            receipt
        );
    }
}

#[test]
fn retained_copy_of_copy_flattens_its_own_receipt_prefix_and_later_native_d() {
    let Fixture {
        temp,
        mut engine,
        source,
        target: _,
    } = Fixture::new(true);
    let target = engine.create_disk_root("D04", "admin", op()).unwrap();
    let original = entry(&engine, &source, "source.txt");
    delete_undo(&mut engine, &source, &original);
    let first = request(&engine, &source, &target);
    let copied_receipt = engine
        .copy_retained_file_history(&first, "admin", op())
        .unwrap();
    let first_file = entry(&engine, &target, "copied.txt");
    let current = delete(&mut engine, &target, &first_file);
    let imported_node = copied_receipt.result["retained_lifecycle"]["events"][0]["node"]["node_id"]
        .as_str()
        .unwrap();
    let before = engine.state().clone();
    assert_eq!(
        engine
            .undo_file_deletion(UndoFileDeletionRequest {
                file_id: &HistoryFileId::parse(&first_file.file_id).unwrap(),
                deletion_node_id: imported_node,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap_err()
            .code(),
        ErrorCode::FileNodeNotFound
    );
    assert_eq!(engine.state(), &before);
    let first_db = database(&engine, &target, &first_file.file_id, "D04", "copied.txt");
    let retained = audit(&first_db);
    assert_eq!(retained.len(), 3);
    drop(engine);
    fs::remove_file(&first_db).unwrap();
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    // Imported rows repeat source operation IDs but are not extra live
    // receipts. The target's own current D survives cold State admission.
    assert_eq!(audit(&first_db), retained);
    assert_eq!(engine.file_deletion(&first_file.file_id), Some(&current));
    assert!(
        engine
            .namespace_tree(&target.main_branch_id)
            .unwrap()
            .get("copied.txt")
            .is_none()
    );
    save(
        &mut engine,
        &source,
        "source.txt",
        b"source remains independently writable",
    );
    assert_eq!(audit(&first_db), retained);
    engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &HistoryFileId::parse(&first_file.file_id).unwrap(),
            deletion_node_id: &current.node_id,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    save(
        &mut engine,
        &target,
        "copied.txt",
        b"edited intermediate copy",
    );
    let destination = engine
        .create_repository(
            &RepositoryName::parse("second-target").unwrap(),
            "P01",
            1,
            "admin",
            op(),
        )
        .unwrap();
    let mut second = request(&engine, &target, &destination);
    second.source_path = "copied.txt".into();
    let receipt = engine
        .copy_retained_file_history(&second, "admin", op())
        .unwrap();
    let final_file = entry(&engine, &destination, "copied.txt");
    let db = database(
        &engine,
        &destination,
        &final_file.file_id,
        "P01",
        "copied.txt",
    );
    let imported = audit(&db);
    assert_eq!(imported.len(), 4);
    assert_eq!(
        json_codec::json!(imported),
        receipt.result["retained_lifecycle"]["events"]
    );
    assert!(
        imported
            .iter()
            .all(|event| event["node"]["file_id"] == final_file.file_id)
    );
    assert!(engine.file_deletion(&final_file.file_id).is_none());
    drop(engine);
    fs::remove_file(&db).unwrap();
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(audit(&db), imported);
    assert_eq!(
        bytes(&engine, &final_file.version_id),
        b"edited intermediate copy"
    );
}

#[test]
fn retained_copy_refuses_to_upgrade_old_graph_only_copy_receipts() {
    let Fixture {
        temp: _temp,
        mut engine,
        source,
        target,
    } = Fixture::new(true);
    let original = entry(&engine, &source, "source.txt");
    delete_undo(&mut engine, &source, &original);
    let first = request(&engine, &source, &target);
    let legacy = engine.copy_file_history(&first, "admin", op()).unwrap();
    assert!(legacy.result.get("retained_lifecycle").is_none());
    let destination = engine
        .create_repository(
            &RepositoryName::parse("no-upgrade").unwrap(),
            "P01",
            1,
            "admin",
            op(),
        )
        .unwrap();
    let mut second = request(&engine, &target, &destination);
    second.source_path = "copied.txt".into();
    let before = engine.state().clone();
    assert_eq!(
        engine
            .copy_retained_file_history(&second, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedOperation
    );
    assert_eq!(engine.state(), &before);
    assert_eq!(engine.operation(legacy.operation_id).unwrap(), legacy);
}

#[test]
fn retained_copy_failure_boundaries_keep_source_and_resume_the_same_complete_audit() {
    for (fault, decided) in [
        ("after_prepare_1", false),
        ("after_accept_1", true),
        ("after_container_names", true),
    ] {
        let Fixture {
            temp,
            mut engine,
            source,
            target,
        } = Fixture::new(true);
        let original = entry(&engine, &source, "source.txt");
        delete_undo(&mut engine, &source, &original);
        let request = request(&engine, &source, &target);
        let operation = op();
        engine.inject_fault_once(fault);
        assert_eq!(
            engine
                .copy_retained_file_history(&request, "admin", operation)
                .unwrap_err()
                .code(),
            if decided {
                ErrorCode::InDoubt
            } else {
                ErrorCode::InjectedFailure
            }
        );
        assert!(engine.file_deletion(&original.file_id).is_none());
        assert_eq!(entry(&engine, &source, "source.txt"), original);
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("system")).unwrap();
        let result = engine
            .copy_retained_file_history(&request, "admin", operation)
            .unwrap();
        let copied = entry(&engine, &target, "copied.txt");
        let db = database(&engine, &target, &copied.file_id, "D04", "copied.txt");
        assert_eq!(audit(&db).len(), 2);
        let before = engine.state().clone();
        assert_eq!(
            engine
                .copy_retained_file_history(&request, "admin", operation)
                .unwrap(),
            result
        );
        assert_eq!(engine.state(), &before);
        assert!(engine.file_deletion(&original.file_id).is_none());
    }
}
