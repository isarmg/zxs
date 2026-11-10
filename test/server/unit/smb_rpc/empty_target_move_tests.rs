//! Empty-disk moves exercise the framed RPC and real native publication.
//! These tests do not load Samba or replace its separate SDK acceptance.
use super::*;
use storage_contract::file_relocation::{OBJECT_MOVE_KIND, ObjectMoveFact};

fn move_request(operation: OperationId, replace: bool) -> Request {
    Request::Namespace {
        kind: smb_protocol::protocol::NamespaceOperation::Rename,
        path: "p/source.txt".into(),
        destination: Some("b/arrived.txt".into()),
        replace,
        operation_id: operation.to_string(),
    }
}

fn seed_source(stream: &mut FrameStream) {
    for (create, bytes) in [(true, b"first"), (false, b"later")] {
        let (opened, file) = open(stream, "p/source.txt", true, create);
        save(stream, &opened, &file, bytes);
    }
}

fn root_on(shared: &Shared, slot: &str) -> storage_contract::Repository {
    shared
        .backend
        .repositories_snapshot(&identity(shared))
        .unwrap()
        .into_iter()
        .find(|repository| repository.file_manager_root && repository.primary_slot_id == slot)
        .unwrap()
}

fn read_body(stream: &mut FrameStream, path: &str) -> Vec<u8> {
    let (opened, _placeholder) = open(stream, path, false, false);
    let bytes = read_rpc(stream, opened.handle, 0, 4096);
    okay(
        stream,
        Request::Close {
            handle: opened.handle,
        },
    );
    bytes
}

#[test]
fn empty_disk_move_accepts_either_replacement_mode_and_keeps_history_after_reopen() {
    for replace in [false, true] {
        let (temp, shared, backend) = empty_fixture(true);
        let (mut stream, worker, _) = wire(shared.clone(), "alice");
        seed_source(&mut stream);
        assert_eq!(
            shared
                .backend
                .repositories_snapshot(&identity(&shared))
                .unwrap()
                .len(),
            1
        );
        let source = root_on(&shared, "P01");
        let original_operations = backend.inner.test_with_engine_locked(|engine| {
            engine
                .state()
                .operations
                .keys()
                .copied()
                .collect::<BTreeSet<_>>()
        });
        let operation = OperationId::generate().unwrap();
        let (result, descriptor) = okay(&mut stream, move_request(operation, replace));
        assert!(descriptor.is_none());
        assert_eq!(result["operation_kind"], OBJECT_MOVE_KIND);
        assert!(directory(&mut stream, "p").is_empty());
        assert_eq!(directory(&mut stream, "b"), vec!["arrived.txt"]);
        assert_eq!(read_body(&mut stream, "b/arrived.txt"), b"later");
        let target = root_on(&shared, "P02");
        let fact: ObjectMoveFact =
            json_codec::from_value(result["result"]["movement"].clone()).unwrap();
        assert_eq!(fact.request.target_repository_id, None);
        assert_eq!(fact.request.target_slot_id, "P02");
        assert_eq!(fact.target_repository_id.as_str(), target.repository_id);
        backend.inner.test_with_engine_locked(|engine| {
            let added: Vec<_> = engine
                .state()
                .operations
                .keys()
                .filter(|id| !original_operations.contains(*id))
                .copied()
                .collect();
            assert_eq!(added, vec![operation]);
        });
        assert!(
            !shared
                .spool
                .join(format!("{operation}.namespace.json"))
                .exists()
        );
        assert!(shared.file_writers.lock().unwrap().is_empty());
        close_wire(stream, worker);
        drop(shared);
        drop(backend);

        let engine = Engine::open(&temp.path().join("state")).unwrap();
        assert!(engine.list_tree(&source.main_branch_id).unwrap().is_empty());
        let entries = engine.list_tree(&target.main_branch_id).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), "arrived.txt");
        let history = engine.file_history(&entries[0].file_id).unwrap();
        assert_eq!(history.len(), 2);
        for (version, expected) in history.iter().zip([b"first", b"later"]) {
            let output = temp.path().join(format!("export-{}", version.version_id));
            engine.export_version(&version.version_id, &output).unwrap();
            assert_eq!(fs::read(output).unwrap(), expected);
        }
        let receipt = engine.state().operations.get(&operation).unwrap();
        assert_eq!(receipt.kind, OBJECT_MOVE_KIND);
        assert_eq!(
            receipt.commit_state,
            storage_contract::CommitState::Published
        );
        assert_eq!(
            engine
                .state()
                .events
                .iter()
                .filter(|event| event.operation_id == operation)
                .count(),
            1
        );
    }
}

#[test]
fn empty_disk_move_lost_reply_rejects_changed_mode_then_reconciles_same_operation() {
    for replace in [false, true] {
        let (_temp, shared, backend) = empty_fixture(true);
        let (mut stream, worker, _) = wire(shared.clone(), "alice");
        seed_source(&mut stream);
        let operation = OperationId::generate().unwrap();
        backend.fail.store(true, Ordering::SeqCst);
        let (reply, descriptor) =
            binding_tests::exchange(&mut stream, move_request(operation, replace));
        assert!(descriptor.is_none());
        assert!(
            matches!(reply, Response::Error { operation_id: Some(ref id), retryable: false, .. }
                if id == &operation.to_string()),
            "{reply:?}"
        );
        let record = shared.spool.join(format!("{operation}.namespace.json"));
        let captured = fs::read(&record).unwrap();
        let intent: Value = json_codec::from_slice(&captured).unwrap();
        assert_eq!(intent["format"], "smb-object-move");
        assert_eq!(intent["replace_requested"], replace);
        assert_eq!(intent["target_binding"], Value::Null);
        assert_eq!(intent["movement"]["target_repository_id"], Value::Null);
        assert_eq!(intent["movement"]["target_slot_id"], "P02");
        assert_eq!(intent["movement"]["target_entry_id"], Value::Null);
        assert_eq!(intent["target_file_id"], Value::Null);
        // The first call really published and removed the source. Retry must
        // use the retained request, not resolve that now-missing source again.
        assert!(directory(&mut stream, "p").is_empty());
        assert_eq!(read_body(&mut stream, "b/arrived.txt"), b"later");
        let sequence = backend
            .inner
            .test_with_engine_locked(|engine| engine.state().sequence);
        let (reply, _) = binding_tests::exchange(&mut stream, move_request(operation, !replace));
        assert!(
            matches!(
                reply,
                Response::Error {
                    code: storage_contract::ErrorCode::IdempotencyKeyReused,
                    ..
                }
            ),
            "{reply:?}"
        );
        assert_eq!(fs::read(&record).unwrap(), captured);
        okay(&mut stream, move_request(operation, replace));
        assert!(!record.exists());
        backend.inner.test_with_engine_locked(|engine| {
            assert_eq!(engine.state().sequence, sequence);
            assert_eq!(engine.state().operations[&operation].kind, OBJECT_MOVE_KIND);
        });
        assert!(shared.file_writers.lock().unwrap().is_empty());
        close_wire(stream, worker);
    }
}

#[test]
fn empty_disk_replace_intent_never_overwrites_a_target_created_after_capture() {
    let (_temp, shared, _backend) = empty_fixture(true);
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    seed_source(&mut stream);
    let generation = shared.account.read().unwrap().selection().generation;
    let mut share =
        default_share::DefaultShare::new(shared.clone(), identity(&shared), generation, 4096)
            .unwrap();
    let operation = OperationId::generate().unwrap();
    // This injection is thread-local, so dispatch on this test thread. It
    // retains the intent but stops before target-root creation or native move.
    namespace::fail_next_record_sync(false);
    assert!(share.dispatch(move_request(operation, true)).is_err());
    let record = shared.spool.join(format!("{operation}.namespace.json"));
    let captured = fs::read(&record).unwrap();
    assert_eq!(
        shared
            .backend
            .repositories_snapshot(&identity(&shared))
            .unwrap()
            .len(),
        1
    );
    let (opened, file) = open(&mut stream, "b/arrived.txt", true, true);
    save(&mut stream, &opened, &file, b"keep");
    let failure = share.dispatch(move_request(operation, true)).unwrap_err();
    assert_eq!(
        failure.error.code,
        storage_contract::ErrorCode::WorkspaceChanged
    );
    assert_eq!(fs::read(&record).unwrap(), captured);
    assert_eq!(read_body(&mut stream, "p/source.txt"), b"later");
    assert_eq!(read_body(&mut stream, "b/arrived.txt"), b"keep");
    assert!(shared.file_writers.lock().unwrap().is_empty());
    close_wire(stream, worker);
}
