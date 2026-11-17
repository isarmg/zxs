//! A real earlier native decision becomes checkpoint-covered history. The
//! successor remains a separate truthful Unchanged receipt, not a fake node.
use crate::snapshot_worktree::drafts::DirectoryDraftIntent;
use crate::{Engine, ErrorCode};
use std::fs;
use std::sync::Arc;
use storage_contract::snapshot_worktree::draft_commit::{
    DirectoryDraftLookup, DirectoryDraftOutcome,
};

#[path = "../../snapshot_worktree/draft_gateway/fixture.rs"]
mod fixture;
use fixture::{Fixture, inventory, read_draft, selection, write_draft};

#[test]
fn native_directory_checkpoint_reuses_verified_prefix_and_rejects_missing_node() {
    let (fixture, mut engine) = Fixture::new();
    let principal = "admin".parse().unwrap();
    write_draft(&mut engine, &fixture.targets[0], fixture::FIRST_DRAFT);
    write_draft(&mut engine, &fixture.targets[1], fixture::SECOND_DRAFT);
    let first_selection = [selection(&engine, &fixture.targets[0])];
    let intent = fixture.intent(&first_selection, &principal);
    let first_operation = intent.operation;
    let message = intent.message;
    let changed = engine.commit_directory_working_set(intent).unwrap();
    let public = engine.directory_draft_receipt(DirectoryDraftLookup {
        source: fixture.source, operation_id: first_operation,
    }, &principal).unwrap();
    let DirectoryDraftOutcome::Changed { snapshot: head } = public.result else {
        panic!("the genuinely changed draft must create a new directory node");
    };
    let node = fixture.snapshot_path()
        .join(crate::persist::layout::snapshot::node(head.node_id).as_path());
    assert!(node.is_dir());
    drop(engine);

    // This actual cold open validates the native original and saves the normal
    // replay checkpoint. No test constructs a checkpoint or receipt capability.
    let mut engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.operation(first_operation).unwrap(), changed);
    let prepared = engine.prepare_recovery().unwrap();
    let checkpoint = crate::engine::recovery_checkpoint::load(
        &engine, &prepared.prefixes, &prepared.records,
    ).unwrap().expect("the normal completed replay must produce a usable checkpoint");
    assert_eq!(checkpoint.sequence, changed.sequence);
    drop(prepared);

    write_draft(&mut engine, &fixture.targets[0], fixture::FIRST_DRAFT);
    let next_selection = [selection(&engine, &fixture.targets[0])];
    let operation = storage_contract::OperationId::generate().unwrap();
    let unchanged = engine.commit_directory_working_set(DirectoryDraftIntent {
        source: head, selected: &next_selection, operation, principal: &principal, message,
    }).unwrap();
    let public = engine.directory_draft_receipt(DirectoryDraftLookup {
        source: head, operation_id: operation,
    }, &principal).unwrap();
    assert_eq!(public.result, DirectoryDraftOutcome::Unchanged { snapshot: head });
    assert!(unchanged.sequence > checkpoint.sequence);
    let prepared = engine.prepare_recovery().unwrap();
    let still_covered = crate::engine::recovery_checkpoint::load(
        &engine, &prepared.prefixes, &prepared.records,
    ).unwrap().expect("an append-only native successor must retain this checkpoint");
    assert_eq!(still_covered.sequence, changed.sequence);
    drop(prepared);
    let state = engine.state().clone();
    let nodes = inventory(&fixture.snapshot_path());
    let unselected = engine.file_working_state(&fixture.targets[1]).unwrap();
    drop(engine);

    let mut engine = Engine::open(&fixture.system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.state().operations, state.operations);
    assert_eq!(engine.state().branches, state.branches);
    assert_eq!(engine.state().versions, state.versions);
    assert_eq!(inventory(&fixture.snapshot_path()), nodes);
    assert_eq!(engine.file_working_state(&fixture.targets[1]).unwrap(), unselected);
    assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::SECOND_DRAFT);
    let installed = Arc::clone(&engine.state);
    let slot = engine.next_slot;
    let media = fixture.media();
    for (source, selected, operation, expected) in [
        (fixture.source, &first_selection, first_operation, &changed),
        (head, &next_selection, operation, &unchanged),
    ] {
        let actual = engine.commit_directory_working_set(DirectoryDraftIntent {
            source, selected, operation, principal: &principal, message,
        }).unwrap();
        assert_eq!(&actual, expected);
    }
    assert!(Arc::ptr_eq(&installed, &engine.state));
    assert_eq!(engine.next_slot, slot);
    assert_eq!(fixture.media(), media);
    drop(installed);
    drop(engine);

    // Even with a real saved checkpoint, every startup first authenticates the
    // original node. The compact observation cannot be issued for missing bytes.
    let parked = fixture.temporary.path().join("parked-decided-node");
    fs::rename(&node, &parked).unwrap();
    let injected = fixture.media();
    let error = match Engine::open(&fixture.system) {
        Ok(_) => panic!("checkpoint accepted a missing genuinely published node"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::ContainerLayoutInvalid, "{error:?}");
    assert_eq!(fixture.media(), injected);
    fs::rename(&parked, &node).unwrap();
    let engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.operation(first_operation).unwrap(), changed);
    assert_eq!(engine.operation(operation).unwrap(), unchanged);
    assert_eq!(inventory(&fixture.snapshot_path()), nodes);
    assert_eq!(read_draft(&engine, &fixture.targets[1]), fixture::SECOND_DRAFT);
}
