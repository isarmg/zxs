//! Exact directory reconstruction and actual native decision at the held hot boundary.
use super::{DirectoryPhysicalInput, DirectoryWorkingPlan};
use crate::engine::file_draft::{PreparedWorkingBatch, WorkingBatchResult};
use crate::persist::directory_draft_commit::{
    DescriptionError, DirectoryResult, native::{self, NativeError, NativePlan},
};
use crate::snapshot_worktree::RegistrationError;
use crate::Engine;
use std::sync::Arc;
use storage_contract::Sequence;
use storage_contract::file_draft::FileWorkingState;
use storage_contract::working_tree::SnapshotNodeId;

#[path = "fixture.rs"]
mod fixture;
use fixture::{Fixture, inventory, read_draft, selection, write_draft};

#[test]
fn actual_same_body_batch_describes_no_node_and_preserves_unselected_draft() {
    let (fixture, mut engine) = Fixture::new();
    let principal = "admin".parse().unwrap();
    let saved = write_draft(&mut engine, &fixture.targets[0], fixture::FIRST_ORIGINAL);
    let untouched = write_draft(&mut engine, &fixture.targets[1], fixture::SECOND_DRAFT);
    drop(engine);
    let engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.operation(saved.operation_id).unwrap(), saved);
    assert_eq!(engine.operation(untouched.operation_id).unwrap(), untouched);
    let selected = [selection(&engine, &fixture.targets[0])];
    let intent = fixture.intent(&selected, &principal);
    let operation = intent.operation;
    let message = intent.message;
    let pending = super::PendingDirectoryDrafts::capture(&engine, intent).unwrap();
    let original = inventory(&fixture.snapshot_path());
    let media = fixture.media();
    let published = Arc::clone(&engine.state);
    let slot = engine.next_slot;
    let snapshot = engine.snapshot.clone();
    let context = engine.planning_context();
    let batch = PreparedWorkingBatch::prepare(
        &context, Arc::clone(pending.selected()), operation, &principal, message,
    ).unwrap();
    assert!(batch.prepared().is_empty());
    assert!(matches!(batch.items()[0].result(), WorkingBatchResult::Retained(_)));
    let unused_node = SnapshotNodeId::generate().unwrap();
    let plan = pending.bind_working_batch(&context, batch, unused_node).unwrap();
    assert_eq!(plan.manifest().entries(), pending.original().manifest().entries());

    // Exercise the actual constructor's description below the pool/physical
    // boundary. No permit, writer, admitted snapshot or Published product is
    // obtained. The Engine still has only its genuinely saved draft receipts.
    let DirectoryWorkingPlan { input, batch, construction, projection } = plan;
    let (file_projection, prepared, files) = batch.into_publication(&context).unwrap();
    assert!(prepared.is_empty());
    let physical = DirectoryPhysicalInput::new(
        input, projection.into_manifest(), &context,
    ).unwrap();
    assert!(physical.is_unchanged());
    assert_eq!(physical.reference(), pending.original().manifest().reference());
    assert_ne!(physical.reference().node, unused_node);
    assert_eq!(physical.cost().payload_count(), 0);
    assert_eq!(physical.cost().total_bytes(), 0);
    let plan = NativePlan::from_batch(
        context.state, construction, &files, crate::business::now().unwrap(),
        context.config.membership_epoch(), context.config.max_entries,
    ).unwrap();
    let envelope = plan.feedback(&physical, pending.original().binding()).unwrap();
    assert!(envelope.directory.is_unchanged());
    assert!(envelope.contents.iter().all(Option::is_none));
    let result = envelope.receipt(
        context.state, Sequence::new(context.next_slot), crate::disk::MAX_METADATA,
        context.config.max_entries,
    ).unwrap();
    assert_eq!(native::decode(
        context.state, &result, crate::disk::MAX_METADATA, context.config.max_entries,
    ).unwrap(), envelope);
    assert_eq!(result.result["directory"]["format"], "directory_draft_unchanged");
    assert!(result.result["directory"].get("construction").is_none());
    assert!(result.result["directory"].get("node").is_none());
    let projected = super::reconstruction::replay(
        context.config, context.state, &context.snapshot_receipts().unwrap(),
        &result, crate::disk::MAX_METADATA, &files.first_reads().unwrap(),
        files.draft_reads(),
    ).unwrap();
    projected.require_file_projection(context.state, &file_projection).unwrap();
    // A complete index is bound to its exact source allocation, not merely
    // to equal visible words supplied beside an otherwise valid envelope.
    let same_value = context.state.as_ref().clone();
    assert!(matches!(super::reconstruction::replay(
        context.config, &same_value, &context.snapshot_receipts().unwrap(),
        &result, crate::disk::MAX_METADATA, &files.first_reads().unwrap(),
        files.draft_reads(),
    ), Err(super::DirectoryPublicationError::Source(
        super::DirectoryDraftError::Registration(RegistrationError::Source),
    ))));
    let projected = projected.into_state();
    assert_eq!(projected.versions, published.versions);
    assert_eq!(projected.branches, published.branches);
    assert_eq!(projected.file_containers[&fixture.targets[1].file],
        published.file_containers[&fixture.targets[1].file]);
    assert!(matches!(projected.file_containers[&fixture.targets[0].file].working,
        FileWorkingState::ManualClean { .. }));
    assert_eq!(projected.operations.len(), published.operations.len() + 1);
    assert_eq!(projected.events.len(), published.events.len() + 1);

    // A complete prospective result is not a published original source.
    // Do not let a State-only index turn it into a retry/head capability.
    assert!(!engine.state().operations.contains_key(&operation));
    assert!(super::retry(&engine, &super::DirectoryDraftIntent {
        source: fixture.source, selected: &selected, operation, principal: &principal, message,
    }).unwrap().is_none());
    let mut wrong_actor = result.clone();
    wrong_actor.result["directory"]["request"]["principal"] =
        json_codec::json!("different-directory-actor");
    assert!(matches!(native::decode_record(
        &wrong_actor, crate::disk::MAX_METADATA, context.config.max_entries,
    ), Err(NativeError::Shape)));

    let mut cardinality = envelope.clone();
    let DirectoryResult::Unchanged(value) = &mut cardinality.directory else {
        panic!("actual retained construction must have the unchanged result");
    };
    value.files.clear();
    assert!(matches!(cardinality.receipt(
        context.state, Sequence::new(context.next_slot), crate::disk::MAX_METADATA,
        context.config.max_entries,
    ), Err(NativeError::Description(DescriptionError::Cardinality))));
    let mut stale = envelope.clone();
    let DirectoryResult::Unchanged(value) = &mut stale.directory else {
        panic!("actual retained construction must have the unchanged result");
    };
    value.request.source.namespace_generation =
        value.request.source.namespace_generation.next().unwrap();
    let stale = stale.receipt(
        context.state, Sequence::new(context.next_slot), crate::disk::MAX_METADATA,
        context.config.max_entries,
    ).unwrap();
    assert!(matches!(super::reconstruction::replay(
        context.config, context.state, &context.snapshot_receipts().unwrap(),
        &stale, crate::disk::MAX_METADATA, &files.first_reads().unwrap(),
        files.draft_reads(),
    ), Err(super::DirectoryPublicationError::Projection)));
    let mut surplus = result.clone();
    surplus.result["directory"]["node"] = json_codec::json!(unused_node);
    assert!(matches!(native::decode_record(
        &surplus, crate::disk::MAX_METADATA, context.config.max_entries,
    ), Err(NativeError::Boundary(_))));
    let mut missing = envelope.clone();
    missing.working_after.clear();
    assert!(matches!(missing.receipt(
        context.state, Sequence::new(context.next_slot), crate::disk::MAX_METADATA,
        context.config.max_entries,
    ), Err(NativeError::Shape)));

    assert!(Arc::ptr_eq(&published, &engine.state));
    assert_eq!(engine.next_slot, slot);
    assert_eq!(engine.snapshot, snapshot);
    assert_eq!(engine.state().file_containers, published.file_containers);
    assert_eq!(fixture.media(), media);
    assert_eq!(inventory(&fixture.snapshot_path()), original);
    assert_eq!(read_draft(&engine, &fixture.targets[0]), fixture::FIRST_ORIGINAL);
    assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::SECOND_DRAFT);
}


#[test]
fn explicit_directory_working_commit_publishes_only_the_selected_changes() {
    let (fixture, mut engine) = Fixture::new();
    let principal = "admin".parse().unwrap();
    write_draft(&mut engine, &fixture.targets[0], fixture::FIRST_DRAFT);
    write_draft(&mut engine, &fixture.targets[1], fixture::SECOND_DRAFT);
    let original = inventory(&fixture.snapshot_path());
    let chosen = [selection(&engine, &fixture.targets[0])];
    let unselected = engine.file_working_state(&fixture.targets[1]).unwrap();
    let versions = engine.state().versions.len();
    let prior = engine.state().operations.len();
    let intent = fixture.intent(&chosen, &principal);
    let operation = intent.operation;
    let receipt = engine.commit_directory_working_set(intent).unwrap();
    assert_eq!(receipt.operation_id, operation);
    assert_eq!(engine.operation(operation).unwrap(), receipt);
    assert_eq!(engine.state().operations.len(), prior + 1);
    assert_eq!(engine.state().versions.len(), versions + 1);
    assert_eq!(engine.file_working_state(&fixture.targets[1]).unwrap(), unselected);
    assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::SECOND_DRAFT);
    let result = native::decode_record(
        &receipt,
        crate::disk::MAX_METADATA,
        engine.config.max_entries,
    )
    .unwrap();
    let DirectoryResult::Changed(changed) = result.directory else {
        panic!("the change must have published a new directory node");
    };
    let newer = inventory(&fixture.snapshot_path());
    for (name, entry) in original.into_iter().filter(|(name, _)| !name.as_os_str().is_empty()) {
        assert_eq!(newer.get(&name), Some(&entry));
    }
    let node = crate::persist::layout::snapshot::node(changed.construction.node);
    assert!(fixture.snapshot_path().join(node.as_path()).is_dir());

    let mut retry = fixture.intent(&chosen, &principal);
    retry.operation = operation;
    assert_eq!(engine.commit_directory_working_set(retry).unwrap(), receipt);
    assert_eq!(engine.state().versions.len(), versions + 1);
    assert_eq!(inventory(&fixture.snapshot_path()), newer);
}

#[test]
fn identical_directory_working_commit_clears_only_selected_draft_and_creates_no_node() {
    let (fixture, mut engine) = Fixture::new();
    let principal = "admin".parse().unwrap();
    write_draft(&mut engine, &fixture.targets[0], fixture::FIRST_ORIGINAL);
    write_draft(&mut engine, &fixture.targets[1], fixture::SECOND_DRAFT);
    let original = inventory(&fixture.snapshot_path());
    let chosen = [selection(&engine, &fixture.targets[0])];
    let unselected = engine.file_working_state(&fixture.targets[1]).unwrap();
    let versions = engine.state().versions.len();
    let prior = engine.state().operations.len();
    let intent = fixture.intent(&chosen, &principal);
    let operation = intent.operation;
    let receipt = engine.commit_directory_working_set(intent).unwrap();
    assert_eq!(engine.operation(operation).unwrap(), receipt);
    assert_eq!(engine.state().operations.len(), prior + 1);
    assert_eq!(engine.state().versions.len(), versions);
    assert_eq!(inventory(&fixture.snapshot_path()), original);
    assert!(matches!(
        engine.file_working_state(&fixture.targets[0]).unwrap(),
        FileWorkingState::ManualClean { .. },
    ));
    assert_eq!(engine.file_working_state(&fixture.targets[1]).unwrap(), unselected);
    assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::SECOND_DRAFT);
    let result = native::decode_record(
        &receipt, crate::disk::MAX_METADATA, engine.config.max_entries,
    )
    .unwrap();
    assert!(matches!(result.directory, DirectoryResult::Unchanged(_)));

    let mut retry = fixture.intent(&chosen, &principal);
    retry.operation = operation;
    assert_eq!(engine.commit_directory_working_set(retry).unwrap(), receipt);
    let mut wrong_retry = fixture.intent(&chosen, &principal);
    wrong_retry.operation = operation;
    wrong_retry.message = "a different request";
    assert!(engine.commit_directory_working_set(wrong_retry).is_err());
    assert_eq!(engine.state().versions.len(), versions);
    assert_eq!(inventory(&fixture.snapshot_path()), original);
}

#[test]
fn root_snapshot_selects_a_nested_file_without_copying_unselected_working_bytes() {
    use storage_contract::file_draft::DraftLocation;
    let (fixture, mut engine) = Fixture::with_nested_directory(true);
    let principal = "admin".parse().unwrap();
    let repository = fixture.targets[0].repository;
    let branch = engine.state().repositories[&repository.to_string()].main_branch_id.clone();
    let nested = engine.list_tree(&branch).unwrap().into_iter()
        .find(|entry| entry.path() == "Folder/Child/existing.txt").unwrap();
    let target = DraftLocation {
        repository, path: nested.path, entry: nested.entry_id.parse().unwrap(),
        file: nested.file_id.parse().unwrap(),
    };
    let saved = write_draft(&mut engine, &target, b"only nested report changes");
    write_draft(&mut engine, &fixture.targets[1], fixture::SECOND_DRAFT);
    let unselected = engine.file_working_state(&fixture.targets[1]).unwrap();
    let chosen = [selection(&engine, &target)];
    let mut wrong = chosen.clone();
    wrong[0].target.path = name_policy::PortablePath::parse("Folder/existing.txt").unwrap();
    assert!(super::PendingDirectoryDrafts::capture(
        &engine, fixture.intent(&wrong, &principal),
    ).is_err());
    let previous_versions = engine.state().versions.len();
    let intent = fixture.intent(&chosen, &principal);
    let operation = intent.operation;
    let result = engine.commit_directory_working_set(intent).unwrap();
    assert_eq!(engine.state().versions.len(), previous_versions + 1);
    assert!(engine.file_working_state(&target).unwrap().draft().is_none());
    let envelope = native::decode_record(&result, crate::disk::MAX_METADATA,
        engine.config.max_entries).unwrap();
    assert_eq!(envelope.directory.files().len(), 1);
    assert_eq!(envelope.directory.files()[0].target, target);
    assert!(envelope.directory.entries().iter().any(|entry| entry.path == target.path));
    drop(engine);
    let mut engine = Engine::open(&fixture.system).unwrap();
    let mut retry = fixture.intent(&chosen, &principal);
    retry.operation = operation;
    assert_eq!(engine.commit_directory_working_set(retry).unwrap(), result);
    assert_eq!(engine.operation(saved.operation_id).unwrap(), saved);
    assert_eq!(engine.file_working_state(&fixture.targets[1]).unwrap(), unselected);
    assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::SECOND_DRAFT);
}
