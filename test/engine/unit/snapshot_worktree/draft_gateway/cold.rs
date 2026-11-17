//! Genuine native decisions through the internal command, then ordinary cold
//! startup. No decoded description is promoted to a Candidate or publication.
use super::DirectoryDraftIntent;
use crate::persist::directory_draft_commit::native;
#[cfg(feature = "fault-injection")]
use crate::persist::directory_draft_commit::DirectoryResult;
use crate::persist::layout;
use crate::Engine;
#[cfg(feature = "fault-injection")]
use crate::{Error, ErrorCode};
#[cfg(feature = "fault-injection")]
use std::fs;
use std::sync::Arc;
use storage_contract::file_draft::FileWorkingState;

#[path = "fixture.rs"]
mod fixture;
use fixture::{Fixture, inventory, read_draft, selection, write_draft};

#[test]
fn actual_directory_changed_and_unchanged_cold_retry_keep_unselected_drafts() {
    for (body, changed) in [
        (fixture::FIRST_DRAFT, true), (fixture::FIRST_ORIGINAL, false),
    ] {
        let (fixture, mut engine) = Fixture::new();
        let principal = "admin".parse().unwrap();
        write_draft(&mut engine, &fixture.targets[0], body);
        let unselected_receipt = write_draft(
            &mut engine, &fixture.targets[1], fixture::SECOND_DRAFT,
        );
        let selected = [selection(&engine, &fixture.targets[0])];
        let unselected = engine.file_working_state(&fixture.targets[1]).unwrap();
        let versions = engine.state().versions.len();
        let nodes = inventory(&fixture.snapshot_path());
        let intent = fixture.intent(&selected, &principal);
        let operation = intent.operation;
        let message = intent.message;
        let receipt = engine.commit_directory_working_set(intent).unwrap();
        let result = native::decode_record(
            &receipt, crate::disk::MAX_METADATA, engine.config.max_entries,
        ).unwrap();
        assert_eq!(!result.directory.is_unchanged(), changed);
        let published_nodes = inventory(&fixture.snapshot_path());
        if !changed { assert_eq!(published_nodes, nodes); }
        let state = engine.state().clone();
        drop(engine);

        let mut engine = Engine::open(&fixture.system).unwrap();
        assert!(!engine.is_read_only());
        assert_eq!(engine.operation(operation).unwrap(), receipt);
        assert_eq!(engine.operation(unselected_receipt.operation_id).unwrap(), unselected_receipt);
        assert_eq!(engine.state().operations, state.operations);
        assert_eq!(engine.state().versions.len(), versions + usize::from(changed));
        assert_eq!(engine.file_working_state(&fixture.targets[1]).unwrap(), unselected);
        assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::SECOND_DRAFT);
        assert!(matches!(engine.file_working_state(&fixture.targets[0]).unwrap(),
            FileWorkingState::ManualClean { .. }));
        assert_eq!(inventory(&fixture.snapshot_path()), published_nodes);
        let installed = Arc::clone(&engine.state);
        let slot = engine.next_slot;
        let media = fixture.media();
        assert_eq!(engine.commit_directory_working_set(DirectoryDraftIntent {
            source: fixture.source, selected: &selected, operation,
            principal: &principal, message,
        }).unwrap(), receipt);
        assert!(Arc::ptr_eq(&installed, &engine.state));
        assert_eq!(engine.next_slot, slot);
        assert_eq!(fixture.media(), media);
        drop(engine);
        let engine = Engine::open(&fixture.system).unwrap();
        assert_eq!(engine.operation(operation).unwrap(), receipt);
        assert_eq!(engine.state().versions.len(), versions + usize::from(changed));
        assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::SECOND_DRAFT);
    }
}

#[cfg(feature = "fault-injection")]
fn cause<T: std::error::Error + 'static>(error: &Error, predicate: impl Fn(&T) -> bool) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = current {
        if error.downcast_ref::<T>().is_some_and(&predicate) { return true; }
        current = error.downcast_ref::<Box<Error>>()
            .map(|boxed| boxed.as_ref() as &(dyn std::error::Error + 'static))
            .or_else(|| error.source());
    }
    false
}

#[cfg(feature = "fault-injection")]
fn copy_node(source: &std::path::Path, target: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir(target).unwrap();
    fs::set_permissions(target, fs::Permissions::from_mode(0o700)).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let next = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_node(&entry.path(), &next);
        } else {
            fs::copy(entry.path(), next).unwrap();
        }
    }
}

#[test]
#[cfg(feature = "fault-injection")]
fn actual_directory_accept_refuses_missing_foreign_or_corrupt_material_before_repair() {
    use crate::journal::{Candidate, RecordType};
    use crate::working_tree::snapshot::SnapshotStoreError;
    let (fixture, mut engine) = Fixture::new();
    let principal = "admin".parse().unwrap();
    write_draft(&mut engine, &fixture.targets[0], fixture::FIRST_DRAFT);
    write_draft(&mut engine, &fixture.targets[1], fixture::SECOND_DRAFT);
    let selected = [selection(&engine, &fixture.targets[0])];
    let before = Arc::clone(&engine.state);
    let unselected = engine.file_working_state(&fixture.targets[1]).unwrap();
    let intent = fixture.intent(&selected, &principal);
    let operation = intent.operation;
    let message = intent.message;
    let attempt_slot = engine.next_slot;
    engine.inject_fault_once("after_accept_1");
    let error = engine.commit_directory_working_set(intent).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InDoubt);
    assert!(matches!(&error, Error::Context { source, .. }
        if source.code() == ErrorCode::InjectedFailure
            && source.message().contains("after_accept_1")
    ), "{error:?}");
    assert!(Arc::ptr_eq(&before, &engine.state));
    assert!(!engine.state().operations.contains_key(&operation));

    // Read the real accepted operation's Candidate and its admitted original.
    // This is not a manufactured successful result or a named orphan scan.
    let hash = engine.journals.values().flat_map(|journal| journal.records())
        .find(|(record, _)| record.sequence == attempt_slot
            && record.record_type == RecordType::Accept)
        .unwrap().0.object_hash.clone();
    let candidate: Candidate = crate::publication_evidence::verified_object_at(
        &engine.disk_resources, &engine.members(), "candidate", &hash,
    ).unwrap();
    assert_eq!(candidate.operation_id, operation);
    let original = crate::publication_evidence::verified_recovery_snapshot_at(
        &engine.disk_resources, &engine.members(), &candidate.snapshot_hash,
    ).unwrap();
    let receipt = original.for_candidate(&candidate).unwrap().state()
        .operations[&operation].clone();
    let envelope = native::decode_record(
        &receipt, crate::disk::MAX_METADATA, engine.config.max_entries,
    ).unwrap();
    let DirectoryResult::Changed(changed) = &envelope.directory else {
        panic!("the genuinely accepted changed input must bind a newly prepared node");
    };
    let node = fixture.snapshot_path()
        .join(layout::snapshot::node(changed.construction.node).as_path());
    assert!(node.is_dir());
    let file_version = &candidate.versions[0].version;
    let record = &engine.state().file_containers[&fixture.targets[0].file];
    let owner = engine.config.disks.iter()
        .find(|disk| disk.disk_id == record.owner_disk.to_string()).unwrap();
    let scope = crate::engine::fresh_layout::containers::observe_replica(
        &engine.config, &engine.disk_resources, &engine.state, fixture.targets[0].file, owner,
    ).unwrap();
    let file_body = scope.body_path(file_version).unwrap();
    assert!(file_body.is_file());
    drop(scope);
    drop(original);
    drop(engine);

    let parked = fixture.temporary.path().join("held-accepted-node");
    fs::rename(&node, &parked).unwrap();
    let media = fixture.media();
    let error = Engine::open(&fixture.system).err().unwrap();
    assert!(cause::<std::io::Error>(&error, |error| error.kind() == std::io::ErrorKind::NotFound));
    assert_eq!(fixture.media(), media);

    // Identical manifest and BODY bytes cannot replace the decided node inode.
    copy_node(&parked, &node);
    let media = fixture.media();
    let error = Engine::open(&fixture.system).err().unwrap();
    assert!(cause::<crate::disk::DirectoryIoError>(&error, |error| {
        matches!(error, crate::disk::DirectoryIoError::IdentityChanged(_))
    }));
    assert_eq!(fixture.media(), media);
    fs::remove_dir_all(&node).unwrap();
    fs::rename(&parked, &node).unwrap();

    // A healthy directory snapshot cannot substitute for the FILE's own BODY.
    let parked_file = fixture.temporary.path().join("held-accepted-file-body");
    fs::rename(&file_body, &parked_file).unwrap();
    let media = fixture.media();
    let error = Engine::open(&fixture.system).err().unwrap();
    assert!(cause::<std::io::Error>(&error, |error| error.kind() == std::io::ErrorKind::NotFound));
    assert_eq!(fixture.media(), media);
    fs::rename(&parked_file, &file_body).unwrap();

    let payload = fs::read_dir(node.join(layout::snapshot::FILES.as_path())).unwrap()
        .next().unwrap().unwrap().path();
    let bytes = fs::read(&payload).unwrap();
    let mut damaged = bytes.clone();
    damaged[0] ^= 1;
    fs::write(&payload, &damaged).unwrap();
    let media = fixture.media();
    let error = Engine::open(&fixture.system).err().unwrap();
    assert!(cause::<SnapshotStoreError>(&error, |error| {
        matches!(error, SnapshotStoreError::Payload(_))
    }));
    assert_eq!(fixture.media(), media);
    fs::write(&payload, &bytes).unwrap();

    // Restoring the original entities permits only the already accepted result.
    let decided_node = inventory(&node);
    let mut engine = Engine::open(&fixture.system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.operation(operation).unwrap(), receipt);
    assert_eq!(engine.state().operations.len(), before.operations.len() + 1);
    assert_eq!(engine.state().versions.len(), before.versions.len() + 1);
    assert_eq!(engine.file_working_state(&fixture.targets[1]).unwrap(), unselected);
    assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::SECOND_DRAFT);
    let state = Arc::clone(&engine.state);
    let media = fixture.media();
    assert_eq!(engine.commit_directory_working_set(DirectoryDraftIntent {
        source: fixture.source, selected: &selected, operation,
        principal: &principal, message,
    }).unwrap(), receipt);
    assert!(Arc::ptr_eq(&state, &engine.state));
    assert_eq!(fixture.media(), media);
    assert_eq!(inventory(&node), decided_node);
    drop(engine);
    let engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.operation(operation).unwrap(), receipt);
    assert_eq!(engine.state().versions.len(), before.versions.len() + 1);
    assert_eq!(inventory(&node), decided_node);
}


#[test]
#[cfg(feature = "fault-injection")]
fn actual_unchanged_directory_partial_publication_cold_keeps_original_outcome() {
    use crate::journal::{Candidate, RecordType};
    let (fixture, mut engine) = Fixture::new();
    let principal = "admin".parse().unwrap();
    write_draft(&mut engine, &fixture.targets[0], fixture::FIRST_ORIGINAL);
    write_draft(&mut engine, &fixture.targets[1], fixture::SECOND_DRAFT);
    let selected = [selection(&engine, &fixture.targets[0])];
    let unselected = engine.file_working_state(&fixture.targets[1]).unwrap();
    let before = Arc::clone(&engine.state);
    let nodes = inventory(&fixture.snapshot_path());
    let intent = fixture.intent(&selected, &principal);
    let operation = intent.operation;
    let message = intent.message;
    let slot = engine.next_slot;
    engine.inject_fault_once("after_publication_1");
    let error = engine.commit_directory_working_set(intent).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InDoubt);
    assert!(matches!(&error, Error::Context { source, .. }
        if source.code() == ErrorCode::InjectedFailure
            && source.message().contains("after_publication_1")
    ), "{error:?}");
    assert!(Arc::ptr_eq(&before, &engine.state));
    let (accepted, _) = engine.journals.values().flat_map(|journal| journal.records())
        .find(|(record, _)| record.sequence == slot
            && record.record_type == RecordType::Accept).unwrap();
    let candidate: Candidate = crate::publication_evidence::verified_object_at(
        &engine.disk_resources, &engine.members(), "candidate", &accepted.object_hash,
    ).unwrap();
    assert!(candidate.versions.is_empty());
    let original = crate::publication_evidence::verified_recovery_snapshot_at(
        &engine.disk_resources, &engine.members(), &candidate.snapshot_hash,
    ).unwrap();
    let receipt = original.for_candidate(&candidate).unwrap().state()
        .operations[&operation].clone();
    let publication_count = engine.journals.values().flat_map(|journal| journal.records())
        .filter(|(record, _)| record.sequence == slot
            && record.record_type == RecordType::Publication).count();
    assert_eq!(publication_count, 1);
    assert!(publication_count < engine.members().len());
    let (published, _) = engine.journals.values().flat_map(|journal| journal.records())
        .find(|(record, _)| record.sequence == slot
            && record.record_type == RecordType::Publication).unwrap();
    let publication: crate::journal::Publication =
        crate::publication_evidence::verified_object_at(
            &engine.disk_resources, &engine.members(), "publication", &published.object_hash,
        ).unwrap();
    original.require_finalized_outcome(&candidate, &publication).unwrap();
    let mut foreign_outcome = publication.clone();
    foreign_outcome.result.result["not_in_the_original_decision"] = json_codec::json!(true);
    let media = fixture.media();
    assert!(matches!(original.require_finalized_outcome(&candidate, &foreign_outcome),
        Err(Error::OriginalReceipt(crate::OriginalReceiptError::Visible))));
    assert_eq!(fixture.media(), media);
    drop(original);
    drop(engine);

    let mut engine = Engine::open(&fixture.system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.operation(operation).unwrap(), receipt);
    assert_eq!(engine.state().versions, before.versions);
    assert_eq!(engine.state().branches, before.branches);
    assert_eq!(engine.state().operations.len(), before.operations.len() + 1);
    assert_eq!(inventory(&fixture.snapshot_path()), nodes);
    assert!(matches!(engine.file_working_state(&fixture.targets[0]).unwrap(),
        FileWorkingState::ManualClean { .. }));
    assert_eq!(engine.file_working_state(&fixture.targets[1]).unwrap(), unselected);
    assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::SECOND_DRAFT);
    let exact = Arc::clone(&engine.state);
    let media = fixture.media();
    assert_eq!(engine.commit_directory_working_set(DirectoryDraftIntent {
        source: fixture.source, selected: &selected, operation,
        principal: &principal, message,
    }).unwrap(), receipt);
    assert!(Arc::ptr_eq(&exact, &engine.state));
    assert_eq!(fixture.media(), media);
    drop(engine);
    let engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.operation(operation).unwrap(), receipt);
    assert_eq!(engine.state().versions, before.versions);
    assert_eq!(inventory(&fixture.snapshot_path()), nodes);
}
