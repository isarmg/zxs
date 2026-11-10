//! Named and default shares use the same captured operation without widening a binding.
use super::*;
use smb_protocol::protocol::NamespaceOperation;
use storage_contract::file_relocation::{NamespaceEntryId, OBJECT_MOVE_KIND, ObjectMoveRequest};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

fn rename(operation: OperationId, replace: bool) -> Request {
    Request::Namespace {
        kind: NamespaceOperation::Rename,
        path: "report.txt".into(),
        destination: Some("renamed.txt".into()),
        replace,
        operation_id: operation.to_string(),
    }
}

#[test]
fn named_move_retries_the_exact_native_intent_after_a_lost_publication_reply() {
    let (_fixture, shared, backend, branch) = fixture();
    let mut client = named_session(shared.clone(), &branch);
    let (reader, _file) = open_file(&mut client, false);
    let before = client
        .find(&name_policy::LookupPath::parse("report.txt").unwrap())
        .unwrap()
        .unwrap();
    let operation = OperationId::generate().unwrap();
    backend.fail.store(true, Ordering::SeqCst);
    let error = client.dispatch(rename(operation, false)).unwrap_err();
    assert_eq!(error.operation_id, Some(operation.to_string()));
    assert!(!error.retryable);
    let record = shared.spool.join(format!("{operation}.namespace.json"));
    let bytes = fs::read(&record).unwrap();
    let intent: Value = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(intent["format"], "smb-object-move");
    assert_eq!(intent["source_binding"], intent["target_binding"]);
    assert!(intent.get("commit_request").is_none());
    assert_eq!(backend.move_requests.lock().unwrap().len(), 1);
    assert_eq!(
        client.dispatch(rename(operation, true)).unwrap_err().code,
        storage_contract::ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(fs::read(&record).unwrap(), bytes);
    assert_eq!(backend.move_requests.lock().unwrap().len(), 1);
    let (reply, _) = client.dispatch(rename(operation, false)).unwrap();
    let reply = json_reply(reply);
    assert_eq!(reply["operation_kind"], OBJECT_MOVE_KIND);
    assert_eq!(reply["commit_state"], "PUBLISHED");
    let requests = backend.move_requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    drop(requests);
    assert!(!record.exists());
    let after = client
        .find(&name_policy::LookupPath::parse("renamed.txt").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(after.entry_id, before.entry_id);
    assert_eq!(after.file_id, before.file_id);
    assert_eq!(after.version_id, before.version_id);
    assert_eq!(read_session(&mut client, reader.handle, 0, 3), b"old");
    client.close(reader.handle, false).unwrap();
    assert_eq!(
        client.dispatch(rename(operation, false)).unwrap_err().code,
        storage_contract::ErrorCode::OperationIdConflict
    );
    assert_eq!(backend.move_requests.lock().unwrap().len(), 2);
}

#[test]
fn named_move_keeps_unknown_retained_formats_without_submitting_or_rewriting() {
    let (_fixture, shared, backend, branch) = fixture();
    let mut client = named_session(shared.clone(), &branch);
    let operation = OperationId::generate().unwrap();
    *backend.move_result.lock().unwrap() =
        Some(Err(ApiError::io(io::Error::other("before publication"))));
    client.dispatch(rename(operation, false)).unwrap_err();
    let record = shared.spool.join(format!("{operation}.namespace.json"));
    let original = fs::read(&record).unwrap();
    let mut unknown: Value = json_codec::from_slice(&original).unwrap();
    unknown["format"] = json!("smb-directory-history-export");
    let unknown = json_codec::to_vec(&unknown).unwrap();
    fs::write(&record, &unknown).unwrap();
    assert_eq!(
        client.dispatch(rename(operation, false)).unwrap_err().code,
        storage_contract::ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(fs::read(&record).unwrap(), unknown);
    assert_eq!(backend.move_requests.lock().unwrap().len(), 1);
    fs::write(&record, original).unwrap();
    client.dispatch(rename(operation, false)).unwrap();
    assert_eq!(backend.move_requests.lock().unwrap().len(), 2);
}

fn command(client: &Session, operation: OperationId) -> crate::MoveCurrentObject {
    let binding = client.identity.smb_binding.as_ref().unwrap().clone();
    let entry = client
        .find(&name_policy::LookupPath::parse("report.txt").unwrap())
        .unwrap()
        .unwrap();
    let slot = client
        .shared
        .backend
        .topology_snapshot(&client.identity)
        .unwrap()
        .slots
        .into_iter()
        .find(|slot| slot.slot_id == "P01")
        .unwrap();
    crate::MoveCurrentObject {
        source_binding: binding.clone(),
        source_slot_id: slot.slot_id.clone(),
        source_disk_id: slot.disk_id.clone(),
        source_binding_generation: slot.binding_generation,
        target: crate::MoveObjectTarget::Repository {
            binding: binding.clone(),
            disk_id: slot.disk_id,
            binding_generation: slot.binding_generation,
        },
        movement: ObjectMoveRequest {
            source_repository_id: HistoryRepositoryId::parse(&binding.repository_id).unwrap(),
            source_entry_id: NamespaceEntryId::parse(&entry.entry_id).unwrap(),
            source_path: name_policy::LookupPath::parse("report.txt").unwrap(),
            target_slot_id: slot.slot_id,
            target_repository_id: Some(HistoryRepositoryId::parse(&binding.repository_id).unwrap()),
            target_path: name_policy::PortablePath::parse("renamed.txt").unwrap(),
            replace: false,
            target_entry_id: None,
        },
        expected_source_file_id: Some(HistoryFileId::parse(&entry.file_id).unwrap()),
        expected_target_file_id: None,
        operation_id: operation,
    }
}

#[test]
fn named_move_cannot_expand_its_original_binding_to_another_generation_or_empty_disk() {
    let (_fixture, shared, backend, branch) = fixture();
    let client = named_session(shared, &branch);
    let before = client.entries().unwrap();
    for changed in 0..3 {
        let operation = OperationId::generate().unwrap();
        let mut request = command(&client, operation);
        match changed {
            0 => request.source_binding.generation += 1,
            1 => {
                let crate::MoveObjectTarget::Repository { binding, .. } = &mut request.target
                else {
                    unreachable!()
                };
                binding.repository_id = new_id("repo").unwrap();
            }
            2 => {
                request.target = crate::MoveObjectTarget::EmptyDisk {
                    disk_id: request.source_disk_id.clone(),
                    binding_generation: request.source_binding_generation,
                }
            }
            _ => unreachable!(),
        }
        let error = backend
            .inner
            .move_current_object(&client.identity, request)
            .unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::Forbidden);
        assert_eq!(client.entries().unwrap(), before);
        assert!(
            backend
                .inner
                .operation_outcome(&client.identity, operation)
                .is_err()
        );
    }
}
