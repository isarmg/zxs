use super::{
    DirectoryDraftError, DirectoryDraftIntent, DirectoryPhysicalError, DirectoryWorkingError,
    DirectoryWorkingPlan,
    PendingDirectoryDrafts,
};
use crate::engine::file_draft::{PreparedWorkingBatch, WorkingBatchResult};
use crate::working_tree::snapshot::SnapshotEntryContent;
use crate::{DraftError, Engine, Error};
use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::sync::Arc;
use storage_contract::file_draft::WorkingSelection;
use storage_contract::working_tree::SnapshotNodeId;

#[path = "draft_gateway/fixture.rs"]
mod fixture;
use fixture::{Fixture, inventory, read_draft, read_view, selection, write_draft};

fn read_plan(plan: &DirectoryWorkingPlan) -> Vec<(name_policy::PortablePath, Vec<u8>)> {
    let bodies = plan.bodies().unwrap();
    plan.manifest().entries().iter().filter_map(|entry| {
        let SnapshotEntryContent::File { material, .. } = &entry.content else { return None; };
        let mut bytes = Vec::new();
        bodies.open_payload(material).unwrap().read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes.len() as u64, material.size);
        assert_eq!(storage_contract::ContentHash::from_digest(blake3::hash(&bytes)),
            material.content_hash);
        Some((entry.path.clone(), bytes))
    }).collect()
}

#[test]
fn real_selected_drafts_pin_original_directory_and_fail_without_preparation() {
    let (fixture, mut engine) = Fixture::new();
    let principal = "admin".parse().unwrap();
    let original = inventory(&fixture.snapshot_path());
    let branches = engine.state().branches.clone();
    let versions = engine.state().versions.clone();
    let first_receipt = write_draft(&mut engine, &fixture.targets[0], fixture::FIRST_DRAFT);
    let second_receipt = write_draft(&mut engine, &fixture.targets[1], fixture::SECOND_DRAFT);
    assert_eq!(engine.state().branches, branches);
    assert_eq!(engine.state().versions, versions);
    assert_eq!(inventory(&fixture.snapshot_path()), original);

    // The sources below are issued by the real cold Engine, not copied State
    // rows, fabricated receipts, a local manifest, or an in-memory reader mock.
    drop(engine);
    let mut engine = Engine::open(&fixture.system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.operation(first_receipt.operation_id).unwrap(), first_receipt);
    assert_eq!(engine.operation(second_receipt.operation_id).unwrap(), second_receipt);
    assert_eq!(inventory(&fixture.snapshot_path()), original);
    assert_eq!(read_draft(&engine, &fixture.targets[0]), fixture::FIRST_DRAFT);
    assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::SECOND_DRAFT);
    let selected = [selection(&engine, &fixture.targets[0])];
    let unselected_working = engine.file_working_state(&fixture.targets[1]).unwrap();
    let before = Arc::clone(&engine.state);
    let slot = engine.next_slot;
    let snapshot = engine.snapshot.clone();
    // Cold FILE repair happened before this capture. From this point every
    // success/failure below is a read-side gateway call and must perform no I/O write.
    let media = fixture.media();
    let pending = PendingDirectoryDrafts::capture(
        &engine, fixture.intent(&selected, &principal),
    ).unwrap();
    assert_eq!(pending.selected().sources().len(), 1);
    assert_eq!(pending.selected().sources()[0].target(), &fixture.targets[0]);
    assert_eq!(pending.directory().source(), fixture.source);
    assert_eq!(pending.original().manifest().entries().len(), 2);
    for (file, target) in pending.files().zip(&fixture.targets) {
        assert_eq!(file.path(), &target.path);
        assert_eq!(file.file(), target.file);
    }
    assert_eq!(read_view(&pending), fixture.expected_view(fixture::FIRST_DRAFT));
    assert_eq!(engine.file_working_state(&fixture.targets[1]).unwrap(), unselected_working);
    assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::SECOND_DRAFT);
    let input = pending.node_input(&engine.planning_context()).unwrap();
    assert_eq!(input.logical_bytes(),
        (fixture::FIRST_DRAFT.len() + fixture::SECOND_ORIGINAL.len()) as u64);
    input.require_context(&engine.planning_context()).unwrap();
    assert!(Arc::ptr_eq(&before, &engine.state));
    assert_eq!(engine.next_slot, slot);
    assert_eq!(engine.snapshot, snapshot);
    assert_eq!(fixture.media(), media);

    // Retain the actual old node and parent; an identically-valued Engine
    // State allocation cannot reissue this observation's authority.
    let actual = Arc::clone(&engine.state);
    engine.state = Arc::new(engine.state.as_ref().clone());
    assert!(!Arc::ptr_eq(&engine.state, &actual));
    assert!(matches!(input.require_context(&engine.planning_context()),
        Err(DirectoryDraftError::SourceChanged)));
    assert_eq!(fixture.media(), media);
    engine.state = actual;
    input.require_context(&engine.planning_context()).unwrap();

    // The old held .zxs FD does not authorize a new directory at its name.
    // No proposed manifest or FILE outcome is minted to test this boundary.
    let repository = fixture.snapshot_path();
    let parked = fixture.temporary.path().join("original-repository-parked");
    fs::rename(&repository, &parked).unwrap();
    fs::create_dir(&repository).unwrap();
    fs::set_permissions(&repository, fs::Permissions::from_mode(0o700)).unwrap();
    let substituted = fixture.media();
    assert!(matches!(input.require_context(&engine.planning_context()),
        Err(DirectoryDraftError::OriginalRead(_))));
    assert_eq!(fixture.media(), substituted);
    assert_eq!(inventory(&parked), original);
    fs::remove_dir(&repository).unwrap();
    fs::rename(&parked, &repository).unwrap();
    input.require_context(&engine.planning_context()).unwrap();
    assert_eq!(fixture.media(), media);

    // A genuine later save on an unselected file invalidates the whole old
    // source occurrence, but must not consume either draft or publish a node.
    let old_second = selection(&engine, &fixture.targets[1]);
    let later = write_draft(&mut engine, &fixture.targets[1], fixture::LATER_SECOND_DRAFT);
    let after_later = Arc::clone(&engine.state);
    assert!(!Arc::ptr_eq(&before, &after_later));
    let later_slot = engine.next_slot;
    let later_snapshot = engine.snapshot.clone();
    let later_media = fixture.media();
    assert!(matches!(pending.require_engine(&engine), Err(DirectoryDraftError::SourceChanged)));
    assert!(matches!(input.require_context(&engine.planning_context()),
        Err(DirectoryDraftError::SourceChanged)));
    assert!(Arc::ptr_eq(&after_later, &engine.state));
    assert_eq!(fixture.media(), later_media);
    assert_eq!(inventory(&repository), original);
    assert_eq!(engine.state().branches, branches);
    assert_eq!(engine.state().versions, versions);
    drop(input);
    drop(pending);

    // Full-set CAS must finish before the first selected BODY is opened:
    // leave the first logical expectation valid, make its real leaf absent,
    // and use the real previous save expectation for the later selected file.
    let first = selection(&engine, &fixture.targets[0]);
    let requests: [WorkingSelection; 2] = [first.clone(), old_second.clone()];
    let first_payload = fixture.draft_path(&engine, &fixture.targets[0]);
    assert_eq!(fs::read(&first_payload).unwrap(), fixture::FIRST_DRAFT);
    let parked_payload = fixture.temporary.path().join("first-draft-parked");
    fs::rename(&first_payload, &parked_payload).unwrap();
    let missing_leaf = fixture.media();
    let actual_second = engine.file_working_state(&fixture.targets[1]).unwrap().revision();
    let error = PendingDirectoryDrafts::capture(
        &engine, fixture.intent(&requests, &principal),
    ).err().unwrap();
    assert!(matches!(error,
        DirectoryDraftError::Engine(Error::FileDraft(DraftError::RevisionChanged {
            expected, actual,
        })) if expected == old_second.expected.revision && actual == actual_second));
    assert!(Arc::ptr_eq(&after_later, &engine.state));
    assert_eq!(engine.next_slot, later_slot);
    assert_eq!(engine.snapshot, later_snapshot);
    assert_eq!(fixture.media(), missing_leaf);
    assert_eq!(fs::read(&parked_payload).unwrap(), fixture::FIRST_DRAFT);
    assert_eq!(inventory(&repository), original);
    fs::rename(&parked_payload, &first_payload).unwrap();
    assert_eq!(fixture.media(), later_media);

    // Fresh source observation still selects only the first saved draft. The
    // late unselected draft remains readable, with its own permanent receipt.
    let pending = PendingDirectoryDrafts::capture(
        &engine, fixture.intent(&[first], &principal),
    ).unwrap();
    assert_eq!(read_view(&pending), fixture.expected_view(fixture::FIRST_DRAFT));
    pending.node_input(&engine.planning_context()).unwrap()
        .require_context(&engine.planning_context()).unwrap();
    assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::LATER_SECOND_DRAFT);
    assert_eq!(engine.operation(later.operation_id).unwrap(), later);
    assert_eq!(engine.state().branches, branches);
    assert_eq!(engine.state().versions, versions);
    assert_eq!(inventory(&repository), original);
    assert_eq!(fixture.media(), later_media);
    // This gateway has created neither a directory node nor a FILE result.
    // Its future single Candidate consumer is intentionally not simulated here.
    assert!(Arc::ptr_eq(&after_later, &engine.state));
    assert_eq!(engine.next_slot, later_slot);
    assert_eq!(engine.snapshot, later_snapshot);

    // A real independent selection of the identical source/request cannot stand
    // in for the observation which belongs to this directory input.
    let directory = pending.directory();
    let other = PendingDirectoryDrafts::capture(&engine, DirectoryDraftIntent {
        source: directory.source(), selected: directory.selections(),
        operation: directory.operation(), principal: directory.principal(),
        message: directory.message(),
    }).unwrap();
    assert!(!Arc::ptr_eq(pending.selected(), other.selected()));
    let other_batch = PreparedWorkingBatch::prepare(
        &engine.planning_context(), Arc::clone(other.selected()), directory.operation(),
        directory.principal(), directory.message(),
    ).unwrap();
    assert!(matches!(pending.bind_working_batch(
        &engine.planning_context(), other_batch, SnapshotNodeId::generate().unwrap(),
    ), Err(DirectoryWorkingError::Batch(Error::FileDraft(DraftError::SourceChanged)))));
    assert_eq!(fixture.media(), later_media);
    drop(other);

    let different_request = PreparedWorkingBatch::prepare(
        &engine.planning_context(), Arc::clone(pending.selected()), directory.operation(),
        directory.principal(), "a different explicit request",
    ).unwrap();
    assert!(matches!(pending.bind_working_batch(
        &engine.planning_context(), different_request, SnapshotNodeId::generate().unwrap(),
    ), Err(DirectoryWorkingError::Request)));
    assert_eq!(fixture.media(), later_media);

    let batch = PreparedWorkingBatch::prepare(
        &engine.planning_context(), Arc::clone(pending.selected()), directory.operation(),
        directory.principal(), directory.message(),
    ).unwrap();
    let operation = directory.operation();
    let plan = pending.bind_working_batch(
        &engine.planning_context(), batch, SnapshotNodeId::generate().unwrap(),
    ).unwrap();
    assert!(Arc::ptr_eq(plan.files().sources(), pending.selected()));
    assert_eq!(plan.files().items().len(), 1);
    assert!(matches!(plan.files().items()[0].result(), WorkingBatchResult::Published(_)));
    assert_eq!(plan.files().prepared().len(), 1);
    assert_eq!(plan.files().state().file_containers[&fixture.targets[1].file],
        engine.state().file_containers[&fixture.targets[1].file]);
    assert_eq!(plan.files().state().operations, engine.state().operations);
    assert!(!engine.state().operations.contains_key(&operation));
    assert_eq!(plan.construction().request.operation, operation);
    assert_eq!(read_plan(&plan), fixture.expected_view(fixture::FIRST_DRAFT));
    assert_eq!(fixture.media(), later_media);
    assert_eq!(inventory(&repository), original);
    assert!(Arc::ptr_eq(&after_later, &engine.state));
    assert_eq!(engine.next_slot, later_slot);

    // The batch has not cleared the published selected draft or claimed a node.
    // Its same-source guard also survives the directory's manifest conversion.
    let working_before = engine.state().file_containers.clone();
    let actual = Arc::clone(&engine.state);
    engine.state = Arc::new(engine.state.as_ref().clone());
    assert!(matches!(plan.require_context(&engine.planning_context()),
        Err(DirectoryWorkingError::Gateway(DirectoryDraftError::SourceChanged))));
    engine.state = actual;
    plan.require_context(&engine.planning_context()).unwrap();
    drop(plan);
    drop(pending);
    drop(engine);
    let mut engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.state().file_containers, working_before);
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().branches, branches);
    assert!(!engine.state().operations.contains_key(&operation));
    assert_eq!(engine.operation(later.operation_id).unwrap(), later);
    assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::LATER_SECOND_DRAFT);
    assert_eq!(inventory(&repository), original);

    // A real same-base save supplies Retained, while the other selected file
    // supplies Published. One batch constructs both outcomes; no per-file
    // commit is issued to manufacture a completed directory transaction.
    let retained_save = write_draft(&mut engine, &fixture.targets[1], fixture::SECOND_ORIGINAL);
    let requests = [
        selection(&engine, &fixture.targets[1]), selection(&engine, &fixture.targets[0]),
    ];
    let pending = PendingDirectoryDrafts::capture(
        &engine, fixture.intent(&requests, &principal),
    ).unwrap();
    let source = Arc::clone(&engine.state);
    let slot = engine.next_slot;
    let media = fixture.media();
    let directory = pending.directory();
    let batch = PreparedWorkingBatch::prepare(
        &engine.planning_context(), Arc::clone(pending.selected()), directory.operation(),
        directory.principal(), directory.message(),
    ).unwrap();
    assert_eq!(batch.items().len(), 2);
    assert!(matches!(batch.items()[0].result(), WorkingBatchResult::Retained(_)));
    assert!(matches!(batch.items()[1].result(), WorkingBatchResult::Published(_)));
    assert_eq!(batch.prepared().len(), 1);
    assert_eq!(batch.state().versions.len(), versions.len() + 1);
    let operation = directory.operation();
    let plan = pending.bind_working_batch(
        &engine.planning_context(), batch, SnapshotNodeId::generate().unwrap(),
    ).unwrap();
    assert_eq!(plan.manifest().entries()[0].path, fixture.targets[0].path);
    assert_eq!(plan.manifest().entries()[1].path, fixture.targets[1].path);
    assert_eq!(read_plan(&plan), fixture.expected_view(fixture::FIRST_DRAFT));
    assert_eq!(plan.files().state().operations, engine.state().operations);
    assert_eq!(plan.files().state().operation_principals, engine.state().operation_principals);
    assert_eq!(plan.files().state().events, engine.state().events);
    for target in &fixture.targets {
        assert!(engine.state().file_containers[&target.file].working.draft().is_some());
        assert!(matches!(plan.files().state().file_containers[&target.file].working,
            storage_contract::file_draft::FileWorkingState::ManualClean { .. }));
    }
    assert!(Arc::ptr_eq(&source, &engine.state));
    assert_eq!(engine.next_slot, slot);
    assert_eq!(fixture.media(), media);
    assert_eq!(inventory(&repository), original);
    let working_before = engine.state().file_containers.clone();
    drop(plan);
    drop(pending);
    drop(engine);
    let engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.state().file_containers, working_before);
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().branches, branches);
    assert!(!engine.state().operations.contains_key(&operation));
    assert_eq!(engine.operation(retained_save.operation_id).unwrap(), retained_save);
    assert_eq!(read_draft(&engine, &fixture.targets[0]), fixture::FIRST_DRAFT);
    assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::SECOND_ORIGINAL);
    assert_eq!(inventory(&repository), original);

    // Continue the actual private producer through the shared aggregate gate,
    // FILE material preparation and independent EXCL full directory copy. It
    // still does not invoke decide or register an unreviewed native operation.
    let published = Arc::clone(&engine.state);
    let published_slot = engine.next_slot;
    let published_snapshot = engine.snapshot.clone();
    let untouched = engine.state().file_containers[&fixture.targets[1].file].clone();
    let operation;
    let prepared_node;
    {
        let selected = [selection(&engine, &fixture.targets[0])];
        let pending = PendingDirectoryDrafts::capture(
            &engine, fixture.intent(&selected, &principal),
        ).unwrap();
        let directory = pending.directory();
        operation = directory.operation();
        let batch = PreparedWorkingBatch::prepare(
            &engine.planning_context(), Arc::clone(pending.selected()), operation,
            directory.principal(), directory.message(),
        ).unwrap();
        let plan = pending.bind_working_batch(
            &engine.planning_context(), batch, SnapshotNodeId::generate().unwrap(),
        ).unwrap();
        let reference = plan.manifest().reference();
        prepared_node = repository.join(crate::persist::layout::snapshot::node(
            reference.node,
        ).as_path());
        assert!(!prepared_node.exists());
        let before = fixture.media();
        let parent = fs::metadata(&repository).unwrap();
        let input = plan.into_publication(
            &engine.planning_context(), crate::disk::MAX_METADATA,
        ).unwrap();
        assert_eq!(fixture.media(), before);
        let (decision, physical) = engine.planning_context().prepare_directory_material(
            input, crate::disk::MAX_METADATA,
        ).unwrap();
        assert!(prepared_node.is_dir());
        physical.require_context(&engine.planning_context()).unwrap();
        let proposal = decision.finish(
            &engine.planning_context(), physical.files(), physical.directory(),
        ).unwrap();
        proposal.require_context(
            &engine.planning_context(), physical.files(), physical.directory(),
        ).unwrap();
        assert_eq!(proposal.result().operation_id, operation);
        assert_eq!(proposal.state().operations.len(), published.operations.len() + 1);
        assert_eq!(proposal.state().operation_principals.len(),
            published.operation_principals.len() + 1);
        assert_eq!(proposal.state().events.len(), published.events.len() + 1);
        assert_eq!(proposal.state().file_containers[&fixture.targets[1].file], untouched);
        assert!(matches!(proposal.state().file_containers[&fixture.targets[0].file].working,
            storage_contract::file_draft::FileWorkingState::ManualClean { .. }));

        // Physical preparation and full logical feedback do not manufacture
        // the independent Published original/visible cursor required by retry.
        assert!(!engine.state().operations.contains_key(&operation));
        assert!(super::retry(&engine, &super::DirectoryDraftIntent {
            source: directory.source(), selected: &selected, operation,
            principal: directory.principal(), message: directory.message(),
        }).unwrap().is_none());
        let mut invalid = proposal.result().clone();
        invalid.result["directory"]["unknown"] = json_codec::json!(true);
        assert!(matches!(crate::persist::directory_draft_commit::native::decode_record(
            &invalid, crate::disk::MAX_METADATA, engine.config.max_entries,
        ), Err(crate::persist::directory_draft_commit::native::NativeError::Boundary(_))));

        let host = &pending.directory().host().namespace;
        let new_node = {
            let root = engine.namespace_host_root(host.repository_id, host.branch_id).unwrap();
            crate::working_tree::snapshot::HeldSnapshotNode::reopen_at_host(
                root.common().unwrap(), physical.directory().binding(),
            ).unwrap()
        };
        let body_view: Vec<_> = new_node.manifest().entries().iter().filter_map(|entry| {
            let SnapshotEntryContent::File { material, .. } = entry.content else { return None; };
            let mut bytes = Vec::new();
            new_node.open_payload(material.version_id).unwrap().read_to_end(&mut bytes).unwrap();
            Some((entry.path.clone(), bytes))
        }).collect();
        assert_eq!(body_view, fixture.expected_view(fixture::FIRST_DRAFT));
        let changed = physical.files().items().iter().find(|item| {
            matches!(item.result(), WorkingBatchResult::Published(_))
        }).unwrap();
        let owner = published.file_containers[&changed.target().file].owner_disk;
        let disk = engine.config.disks.iter()
            .find(|disk| disk.disk_id == owner.to_string()).unwrap();
        let container = crate::engine::fresh_layout::containers::observe_replica(
            &engine.config, &engine.disk_resources, &published, changed.target().file, disk,
        ).unwrap();
        assert_eq!(fs::read(container.body_path(changed.version()).unwrap()).unwrap(),
            fixture::FIRST_DRAFT);

        // A successful new EXCL object does not acquire the old source's
        // registration. Substituting an empty node at that name must fail the
        // real material guard before a Candidate or metadata write occurs.
        let parked = fixture.temporary.path().join("prepared-node-parked");
        fs::rename(&prepared_node, &parked).unwrap();
        fs::create_dir(&prepared_node).unwrap();
        fs::set_permissions(&prepared_node, fs::Permissions::from_mode(0o700)).unwrap();
        let fault = fixture.media();
        assert!(matches!(physical.directory().require_context(&engine.planning_context()),
            Err(DirectoryPhysicalError::Source(DirectoryDraftError::OriginalRead(_)))));
        assert!(proposal.require_material(physical.files(), physical.directory()).is_err());
        assert_eq!(fixture.media(), fault);
        fs::remove_dir(&prepared_node).unwrap();
        fs::rename(&parked, &prepared_node).unwrap();
        proposal.require_material(physical.files(), physical.directory()).unwrap();

        // Every old immutable child is byte/entity-identical. Only adding the
        // new child may change the parent directory's link count.
        let after = inventory(&repository);
        for (path, expected) in &original {
            if !path.as_os_str().is_empty() { assert_eq!(after.get(path), Some(expected)); }
        }
        let actual_parent = fs::metadata(&repository).unwrap();
        assert_eq!((actual_parent.dev(), actual_parent.ino(), actual_parent.mode()),
            (parent.dev(), parent.ino(), parent.mode()));
        assert!(after.len() > original.len());
        assert!(Arc::ptr_eq(&published, &engine.state));
        assert_eq!(engine.next_slot, published_slot);
        assert_eq!(engine.snapshot, published_snapshot);
        assert_eq!(engine.state().file_containers, published.file_containers);
        assert_eq!(engine.state().operations, published.operations);
        assert_eq!(engine.state().events, published.events);
        assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::SECOND_ORIGINAL);
    }
    drop(engine);
    let engine = Engine::open(&fixture.system).unwrap();
    assert!(!engine.state().operations.contains_key(&operation));
    assert_eq!(engine.state().file_containers, published.file_containers);
    assert_eq!(engine.state().versions, published.versions);
    assert_eq!(engine.state().branches, published.branches);
    assert_eq!(read_draft(&engine, &fixture.targets[0]), fixture::FIRST_DRAFT);
    assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::SECOND_ORIGINAL);
    assert!(prepared_node.is_dir()); // Retained unpublished material, never adopted.
}
