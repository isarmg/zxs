//! Operation-level probes use a real registered Initial root, born FILEs,
//! write handles and complete cold Engine replay. No manually inserted State.
use super::*;
use crate::persist::snapshot_worktree::{self as codec, Record};
use storage_contract::snapshot_worktree::{CommitWorktree, SnapshotHandle};

fn prepare() -> (Fixture, Engine, Principal, CommitWorktree, WorktreeHandle) {
    let (fixture, mut engine) = Fixture::with_nested_directory(true);
    let principal: Principal = "admin".parse().unwrap();
    let root = engine.snapshot_root_context(fixture.targets[0].repository).unwrap();
    let allocate = || AllocateWorktree { source: fixture.source, parent_directory_id: root.host.directory_id,
        expected_root_generation: root.host.expected_root_generation, operation_id: OperationId::generate().unwrap(),
        naming: WorktreeNaming::Automatic };
    let first = engine.allocate_directory_worktree(&allocate(), &principal).unwrap();
    let other = engine.allocate_directory_worktree(&allocate(), &principal).unwrap();
    let scratch = tempfile::tempdir().unwrap();
    save(&mut engine, first, &principal, scratch.path(), "first.txt", b"superseded");
    save(&mut engine, first, &principal, scratch.path(), "Folder/Child/existing.txt", b"unchanged nested original");
    save(&mut engine, first, &principal, scratch.path(), "first.txt", b"sealed current");
    let request = CommitWorktree { target: first, expected: engine.directory_worktree_status(first).unwrap().revision,
        message: "whole existing set".into(), operation_id: OperationId::generate().unwrap() };
    (fixture, engine, principal, request, other)
}

fn assert_closed(engine: &Engine, request: &CommitWorktree, expected: SnapshotHandle,
    original: SnapshotHandle, other: WorktreeHandle) {
    let reserve = engine.state().operations.get(&request.operation_id).unwrap();
    let Record::ReserveContentBatch(plan) = codec::decode(reserve).unwrap().unwrap().record else { panic!("missing original"); };
    let published = engine.state().operations.get(&plan.publish_operation).unwrap();
    let Record::PublishContentBatch(result) = codec::decode(published).unwrap().unwrap().record else { panic!("missing batch"); };
    assert_eq!(plan.handle(), expected);
    assert_eq!(plan.source, original);
    assert_eq!(result.rows.len(), 2);
    assert_eq!(result.reserve, request.operation_id);
    let receipts = engine.snapshot_receipts().unwrap();
    let node = receipts.reopen_node(engine, expected).unwrap();
    let previous = receipts.reopen_node(engine, original).unwrap();
    assert_eq!(node.manifest().entries().len(), previous.manifest().entries().len());
    assert_eq!(node.manifest().operation_id(), plan.publish_operation);
    for (input, row) in plan.rows.iter().zip(&result.rows) {
        assert_eq!(engine.state().versions.get(&row.content.version.version_id).unwrap(), &row.content.version);
        assert_eq!(row.content.version.version_sequence, published.sequence);
        assert_eq!(row.file.path, input.edit.path);
        assert_eq!(row.file.material, input.edit.material);
        assert_eq!(engine.file_branch_head(&input.base.branch_id().to_string()).unwrap(), row.file.material.version_id.to_string());
    }
    for (now, old) in node.manifest().entries().iter().zip(previous.manifest().entries()) {
        if !plan.rows.iter().any(|r| r.edit.path == old.path) { assert_eq!(now, old); }
    }
    assert_eq!(bytes(engine, request.target, "first.txt"), b"sealed current");
    assert_eq!(bytes(engine, request.target, "Folder/Child/existing.txt"), b"unchanged nested original");
    assert_eq!(bytes(engine, request.target, "second.txt"), SECOND_ORIGINAL);
    assert_eq!(bytes(engine, other, "first.txt"), FIRST_ORIGINAL);
}

#[test]
fn one_business_candidate_closes_all_existing_occurrences_and_lost_response_replays_without_allocation() {
    let (fixture, mut engine, principal, request, other) = prepare();
    let original = fixture.source;
    let before = engine.state().events.len();
    let files = engine.state().versions.len();
    let expected = engine.commit_directory_worktree(&request, &principal).unwrap();
    assert_closed(&engine, &request, expected, original, other);
    assert_eq!(engine.state().events.len(), before + 2, "one occupancy then one business publication");
    assert_eq!(engine.state().versions.len(), files + 2);
    drop(engine);
    let mut engine = Engine::open(&fixture.system).unwrap();
    let media = fixture.media();
    assert_eq!(engine.commit_directory_worktree(&request, &principal).unwrap(), expected);
    assert!(engine.commit_directory_worktree(&request, &"stranger".parse().unwrap()).is_err());
    let mut conflicting = request.clone(); conflicting.message.push('!');
    assert!(engine.commit_directory_worktree(&conflicting, &principal).is_err());
    assert_eq!(engine.state().events.len(), before + 2);
    assert_closed(&engine, &request, expected, original, other);
    assert_eq!(fixture.media(), media, "an acknowledged replay must not reopen publication or rewrite either FILE");
}

#[cfg(feature = "fault-injection")]
#[test]
fn original_reservation_and_only_its_native_slot_survive_every_physical_install_boundary() {
    for cut in ["after_worktree_content_stage", "after_prepare_1", "after_accept_1", "after_finalize_1", "after_finalize",
        "after_commit_proofs", "after_worktree_content_move_1", "after_worktree_content_move_2",
        "after_worktree_content_node", "after_worktree_content_committing", "after_worktree_content_completed",
        "after_worktree_content_response"] {
        let (fixture, mut engine, principal, request, other) = prepare();
        let before = engine.state().events.len();
        engine.inject_fault_once("after_worktree_content_reserve");
        assert!(engine.commit_directory_worktree(&request, &principal).is_err());
        let reserve = engine.state().operations.get(&request.operation_id).unwrap().clone();
        let Record::ReserveContentBatch(plan) = codec::decode(&reserve).unwrap().unwrap().record else { panic!("not reserved"); };
        assert!(!engine.state().operations.contains_key(&plan.publish_operation));
        assert!(engine.directory_worktree_status(request.target).is_err());
        assert_eq!(bytes(&engine, other, "first.txt"), FIRST_ORIGINAL);
        drop(engine);
        let mut engine = Engine::open(&fixture.system).unwrap();
        engine.inject_fault_once(cut);
        assert!(engine.commit_directory_worktree(&request, &principal).is_err(), "{cut} did not interrupt the original");
        drop(engine);
        let mut engine = Engine::open(&fixture.system).unwrap();
        let expected = engine.commit_directory_worktree(&request, &principal).unwrap();
        assert_eq!(engine.state().operations.get(&request.operation_id), Some(&reserve), "{cut}");
        assert_eq!(expected, plan.handle(), "{cut} changed destination");
        assert_eq!(engine.state().events.len(), before + 2, "{cut} installed another business slot");
        assert_closed(&engine, &request, expected, fixture.source, other);
    }
}
