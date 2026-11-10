//! One real directory rename through the framed default-share protocol.
//! Directory member/history semantics belong to the public engine scenarios.
use super::*;
use storage_contract::file_relocation::{OBJECT_MOVE_KIND, ObjectMoveFact};

fn rename(operation: OperationId) -> Request {
    Request::Namespace {
        kind: smb_protocol::protocol::NamespaceOperation::Rename,
        path: "p/folder".into(),
        destination: Some("p/renamed".into()),
        replace: false,
        operation_id: operation.to_string(),
    }
}

fn create_directory(stream: &mut FrameStream) {
    okay(
        stream,
        Request::Namespace {
            kind: smb_protocol::protocol::NamespaceOperation::Mkdir,
            path: "p/folder".into(),
            destination: None,
            replace: false,
            operation_id: OperationId::generate().unwrap().to_string(),
        },
    );
    let (opened, file) = open(stream, "p/folder/note.txt", true, true);
    save(stream, &opened, &file, b"saved child");
}

#[test]
fn directory_rename_reconnect_reuses_the_captured_object_and_original_receipt() {
    let (_fixture, shared, backend) = empty_fixture(false);
    let (mut stream, worker, hello) = wire(shared.clone(), "alice");
    assert!(matches!(hello, Response::Ok { .. }), "{hello:?}");
    create_directory(&mut stream);
    let repository = shared
        .backend
        .repositories_snapshot(&identity(&shared))
        .unwrap()
        .into_iter()
        .find(|repository| repository.file_manager_root && repository.primary_slot_id == "P01")
        .unwrap();
    let original = shared
        .backend
        .lookup_current(
            &identity(&shared),
            &repository.main_branch_id,
            &name_policy::LookupPath::parse("folder").unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(original.kind, storage_contract::NamespaceEntryKind::Directory);
    let operation = OperationId::generate().unwrap();
    backend.fail.store(true, Ordering::SeqCst);
    let (reply, descriptor) = binding_tests::exchange(&mut stream, rename(operation));
    assert!(descriptor.is_none());
    assert!(
        matches!(reply, Response::Error { operation_id: Some(ref id), retryable: false, .. }
            if id == &operation.to_string()),
        "{reply:?}"
    );
    let record = shared.spool.join(format!("{operation}.namespace.json"));
    let retained = fs::read(&record).unwrap();
    let intent: Value = json_codec::from_slice(&retained).unwrap();
    assert_eq!(intent["format"], "smb-object-move");
    assert_eq!(intent["source_file_id"], Value::Null);
    assert_eq!(intent["target_file_id"], Value::Null);
    assert_eq!(intent["movement"]["source_entry_id"], original.entry_id);
    assert_eq!(
        intent["movement"]["target_repository_id"],
        repository.repository_id
    );
    assert_eq!(directory(&mut stream, "p"), vec!["renamed"]);
    let (reader, _file) = open(&mut stream, "p/renamed/note.txt", false, false);
    assert_eq!(read_rpc(&mut stream, reader.handle, 0, 4096), b"saved child");
    okay(&mut stream, Request::Close { handle: reader.handle });
    close_wire(stream, worker);

    let published = backend.inner.test_with_engine_locked(|engine| engine.state().clone());
    let original_receipt = published.operations.get(&operation).unwrap();
    assert_eq!(original_receipt.kind, OBJECT_MOVE_KIND);
    let fact: ObjectMoveFact =
        json_codec::from_value(original_receipt.result["movement"].clone()).unwrap();
    assert_eq!(fact.source_file_id, None);
    assert_eq!(fact.displaced_target_file_id, None);
    assert_eq!(fact.target_entry_id.as_str(), original.entry_id);
    let (mut stream, worker, hello) = wire(shared.clone(), "alice");
    assert!(matches!(hello, Response::Ok { .. }), "{hello:?}");
    let (reply, descriptor) = okay(&mut stream, rename(operation));
    assert!(descriptor.is_none());
    assert_eq!(reply["operation_id"], operation.to_string());
    assert_eq!(reply["operation_kind"], OBJECT_MOVE_KIND);
    assert_eq!(reply["result"], original_receipt.result);
    assert!(!record.exists());
    backend.inner.test_with_engine_locked(|engine| assert_eq!(engine.state(), &published));
    let calls = backend.move_requests.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], calls[1]);
    assert_eq!(calls[0]["source_file_id"], Value::Null);
    assert_eq!(calls[0]["source_entry_id"], original.entry_id);
    assert_eq!(calls[0]["source_slot_id"], intent["source_slot_id"]);
    assert_eq!(calls[0]["source_disk_id"], intent["source_disk"]["disk_id"]);
    assert_eq!(calls[0]["source_disk_generation"], intent["source_disk"]["binding_generation"]);
    assert_eq!(calls[0]["target_disk_id"], intent["target_disk"]["disk_id"]);
    assert_eq!(calls[0]["target_disk_generation"], intent["target_disk"]["binding_generation"]);
    assert!(shared.file_writers.lock().unwrap().is_empty());
    drop(calls);
    close_wire(stream, worker);
}
