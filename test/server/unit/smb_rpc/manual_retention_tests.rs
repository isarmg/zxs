//! Real permanent-result authorization and redundant-copy lifecycle.
use super::*;

fn frozen_request(shared: &Shared, operation: OperationId) -> crate::PersistContainerDraft {
    let bytes = fs::read(shared.spool.join(format!("manual-{operation}.work.json"))).unwrap();
    let value: Value = json_codec::from_slice(&bytes).unwrap();
    json_codec::from_value(value["request"].clone()).unwrap()
}

fn pending(shared: &Arc<Shared>, branch: &str) -> (Session, File, crate::PersistContainerDraft) {
    let mut client = session(shared.clone(), branch);
    let (opened, descriptor) = client.open("report.txt".into(), true, false, false, false, false)
        .unwrap();
    let file = File::from(descriptor);
    client.prepare_write(opened.handle, 5, 5).unwrap();
    file.write_all_at(b"first", 0).unwrap();
    client.dirty(opened.handle).unwrap();
    let Handle::Write(writer, _) = client.handles.get(&opened.handle).unwrap() else {
        panic!("manual writer expected");
    };
    let revision = manual_recovery::freeze(shared, &identity(shared), writer, 4096).unwrap();
    let request = frozen_request(shared, revision.operation);
    (client, file, request)
}

fn publish(shared: &Shared, request: &crate::PersistContainerDraft) {
    let directory = platform_io::fs_safety::PrivateDirectory::open_trusted(&shared.spool).unwrap();
    let name = platform_io::fs_safety::EntryName::new(
        format!("manual-{}", request.revision.operation),
    ).unwrap();
    let body = directory.open_payload(
        &name, platform_io::fs_safety::PayloadOpenMode::Existing,
    ).unwrap();
    shared.backend.persist_container_draft(&identity(shared), request.clone(), &body).unwrap();
}


fn retained_charge(shared: &Shared, operation: OperationId) -> u64 {
    let owner: Value = json_codec::from_slice(&fs::read(
        shared.spool.join(format!("manual-{operation}.budget.json")),
    ).unwrap()).unwrap();
    owner["admitted_end"].as_str().unwrap().parse::<u64>().unwrap() + 65_536
}

fn assert_retired(shared: &Shared, operation: OperationId) {
    assert!(!shared.spool.join(format!("manual-{operation}")).exists());
    assert!(shared.spool.join(format!("manual-{operation}.work.json")).is_file());
    let owner: Value = json_codec::from_slice(&fs::read(
        shared.spool.join(format!("manual-{operation}.budget.json")),
    ).unwrap()).unwrap();
    assert_eq!(owner["admitted_end"], "0");
}

#[test]
fn successful_saves_release_each_frozen_body_but_keep_query_records() {
    let (_root, shared, _backend, branch) = fresh();
    let mut client = session(shared.clone(), &branch);
    let (opened, descriptor) = client.open(
        "report.txt".into(), true, false, false, false, false,
    ).unwrap();
    let file = File::from(descriptor);
    let mut operations = Vec::new();
    for bytes in [b"first", b"later", b"third"] {
        client.prepare_write(opened.handle, 5, 5).unwrap();
        file.write_all_at(bytes, 0).unwrap();
        client.dirty(opened.handle).unwrap();
        let result = client.flush(opened.handle).unwrap();
        let operation = result["operation_id"].as_str().unwrap().parse().unwrap();
        assert!(!operations.contains(&operation));
        operations.push(operation);
        assert_retired(&shared, operation);
        assert_eq!(recover(&mut client, operation, false)["draft_persisted"], true);
    }
    let (read, descriptor) = client.open(
        "report.txt".into(), false, false, false, false, false,
    ).unwrap();
    drop(descriptor);
    assert_eq!(client.read(read.handle, 0, 5).unwrap().as_ref(), b"third");
}

#[test]
fn no_permanent_receipt_never_retires_frozen_body_or_budget() {
    let (_root, shared, _backend, branch) = fresh();
    let (_client, _file, request) = pending(&shared, &branch);
    let operation = request.revision.operation;
    let body = shared.spool.join(format!("manual-{operation}"));
    let owner = shared.spool.join(format!("manual-{operation}.budget.json"));
    let before = fs::read(&owner).unwrap();
    assert!(manual_retention::retire(&shared, &identity(&shared), &request, 4096).is_err());
    assert_eq!(fs::read(body).unwrap(), b"first");
    assert_eq!(fs::read(owner).unwrap(), before);
}

#[test]
fn permanent_receipt_rejects_changed_digest_and_owner_before_retirement() {
    let (_root, shared, _backend, branch) = fresh();
    let (_client, _file, request) = pending(&shared, &branch);
    publish(&shared, &request);
    let mut changed = request.clone();
    changed.revision.size += 1;
    assert!(manual_retention::retire(&shared, &identity(&shared), &changed, 4096).is_err());
    changed = request.clone();
    changed.revision.principal = "other".parse().unwrap();
    assert!(manual_retention::retire(&shared, &identity(&shared), &changed, 4096).is_err());
    let body = shared.spool.join(format!("manual-{}", request.revision.operation));
    assert_eq!(fs::read(body).unwrap(), b"first");
}

#[test]
fn altered_body_is_preserved_even_after_permanent_success() {
    let (_root, shared, _backend, branch) = fresh();
    let (_client, _file, request) = pending(&shared, &branch);
    publish(&shared, &request);
    let body = shared.spool.join(format!("manual-{}", request.revision.operation));
    fs::write(&body, b"other").unwrap();
    assert!(manual_retention::retire(&shared, &identity(&shared), &request, 4096).is_err());
    assert_eq!(fs::read(body).unwrap(), b"other");
}

#[test]
fn cold_retry_after_body_unlink_settles_budget_and_keeps_original_result() {
    let (root, shared, backend, branch) = fresh();
    let (client, file, request) = pending(&shared, &branch);
    publish(&shared, &request);
    let operation = request.revision.operation;
    fs::remove_file(shared.spool.join(format!("manual-{operation}"))).unwrap();
    drop((client, file, backend, shared));
    let shared = reopen(&root);
    manual_retention::retire(&shared, &identity(&shared), &request, 4096).unwrap();
    assert_retired(&shared, operation);
    let result = wire_recover(shared.clone(), operation, false);
    assert_eq!(result["draft_persisted"], true);
    manual_retention::retire(&shared, &identity(&shared), &request, 4096).unwrap();
    assert_eq!(
        wire_recover(shared.clone(), operation, false)["original_receipt"],
        result["original_receipt"],
    );
}

#[test]
fn missing_or_corrupt_record_preserves_acknowledged_body() {
    let (_root, shared, _backend, branch) = fresh();
    let (_client, _file, request) = pending(&shared, &branch);
    publish(&shared, &request);
    let operation = request.revision.operation;
    let record = shared.spool.join(format!("manual-{operation}.work.json"));
    let original = fs::read(&record).unwrap();
    fs::write(&record, b"invalid").unwrap();
    assert!(manual_retention::retire(&shared, &identity(&shared), &request, 4096).is_err());
    assert_eq!(fs::read(&record).unwrap(), b"invalid");
    fs::remove_file(&record).unwrap();
    assert!(manual_retention::retire(&shared, &identity(&shared), &request, 4096).is_err());
    assert_eq!(fs::read(shared.spool.join(format!("manual-{operation}"))).unwrap(), b"first");
    fs::write(&record, original).unwrap();
    manual_retention::retire(&shared, &identity(&shared), &request, 4096).unwrap();
    assert_retired(&shared, operation);
}

#[test]
fn cold_inventory_after_owner_settlement_preserves_metadata_charge_and_query() {
    let (root, shared, backend, branch) = fresh();
    let (client, file, request) = pending(&shared, &branch);
    publish(&shared, &request);
    let operation = request.revision.operation;
    manual_retention::retire(&shared, &identity(&shared), &request, 4096).unwrap();
    drop((client, file, backend, shared));
    let shared = reopen(&root);
    assert_retired(&shared, operation);
    assert_eq!(wire_recover(shared.clone(), operation, false)["draft_persisted"], true);
    manual_retention::retire(&shared, &identity(&shared), &request, 4096).unwrap();
    assert_retired(&shared, operation);
}

#[test]
#[cfg(feature = "fault-injection")]
fn every_retirement_interruption_preserves_evidence_until_retry_and_cold_recovery() {
    use budget::retention::Step;
    for step in [Step::BeforeUnlink, Step::AfterUnlink, Step::AfterSync,
        Step::BeforeOwner, Step::AfterOwner]
    {
        let (root, shared, backend, branch) = fresh();
        let (client, file, request) = pending(&shared, &branch);
        publish(&shared, &request);
        let operation = request.revision.operation;
        let before = retained_charge(&shared, operation);
        assert_eq!(before, 65_536 + 5);
        budget::retention::fail_at(step);
        assert!(manual_retention::retire(&shared, &identity(&shared), &request, 4096).is_err());
        let expected = if step == Step::AfterOwner { 65_536 } else { before };
        assert_eq!(retained_charge(&shared, operation), expected);
        let mut live = [0; 5];
        assert_eq!(file.read_at(&mut live, 0).unwrap(), 5);
        assert_eq!(&live, b"first");
        assert!(shared.spool.join(format!("manual-{operation}.work.json")).is_file());
        drop((client, file, backend, shared));
        let shared = reopen(&root);
        manual_retention::retire(&shared, &identity(&shared), &request, 4096).unwrap();
        assert_retired(&shared, operation);
        assert_eq!(retained_charge(&shared, operation), 65_536);
        assert_eq!(wire_recover(shared.clone(), operation, false)["draft_persisted"], true);
        let mut client = session(shared.clone(), &branch);
        let (read, descriptor) = client.open(
            "report.txt".into(), false, false, false, false, false,
        ).unwrap();
        drop(descriptor);
        assert_eq!(client.read(read.handle, 0, 5).unwrap().as_ref(), b"first");
    }
}

#[test]
fn unexpected_namespace_evidence_is_retained_without_freeing_capacity() {
    let (_root, shared, _backend, branch) = fresh();
    let (_client, _file, request) = pending(&shared, &branch);
    publish(&shared, &request);
    let operation = request.revision.operation;
    let name = platform_io::fs_safety::EntryName::new(
        format!("manual-{operation}.namespace.json"),
    ).unwrap();
    let directory = platform_io::fs_safety::PrivateDirectory::open_trusted(&shared.spool).unwrap();
    platform_io::fs_safety::AtomicFile::create(&directory, &name, b"unexpected").unwrap();
    let before = retained_charge(&shared, operation);
    assert!(manual_retention::retire(&shared, &identity(&shared), &request, 4096).is_err());
    assert_eq!(retained_charge(&shared, operation), before);
    assert_eq!(fs::read(shared.spool.join(name.as_path())).unwrap(), b"unexpected");
    assert_eq!(fs::read(shared.spool.join(format!("manual-{operation}"))).unwrap(), b"first");
}

#[test]
fn query_is_read_only_and_explicit_resume_retires_acknowledged_material() {
    let (_root, shared, _backend, branch) = fresh();
    let (_client, _file, request) = pending(&shared, &branch);
    publish(&shared, &request);
    let operation = request.revision.operation;
    let body = shared.spool.join(format!("manual-{operation}"));
    let owner = shared.spool.join(format!("manual-{operation}.budget.json"));
    let original_body = fs::read(&body).unwrap();
    let original_owner = fs::read(&owner).unwrap();
    let query = wire_recover(shared.clone(), operation, false);
    assert_eq!(query["draft_persisted"], true);
    assert_eq!(fs::read(&body).unwrap(), original_body);
    assert_eq!(fs::read(&owner).unwrap(), original_owner);
    let resumed = wire_recover(shared.clone(), operation, true);
    assert_eq!(resumed["original_receipt"], query["original_receipt"]);
    assert_retired(&shared, operation);
}
