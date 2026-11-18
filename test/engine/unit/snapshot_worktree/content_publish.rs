//! Operation-level probes use a real registered Initial root, born FILEs,
//! write handles and complete cold Engine replay. No manually inserted State.
use super::*;
use crate::persist::snapshot_worktree::{self as codec, Record};
use storage_contract::snapshot_worktree::{CommitWorktree, SnapshotHandle};

fn prepare() -> (Fixture, Engine, Principal, CommitWorktree, WorktreeHandle) {
    let (fixture, mut engine) = Fixture::with_nested_directory(true);
    let principal: Principal = "admin".parse().unwrap();
    // One real ManualClean container and one Automatic nested container.
    // Neither has a shared working draft for this private worktree to clear.
    use storage_contract::file_draft::{CommitMode, DraftCommand};
    let first_target = &fixture.targets[0];
    let expected = engine.file_working_state(first_target).unwrap().revision();
    engine.change_file_working(&DraftCommand::SetMode { target: first_target.clone(), expected, mode: CommitMode::Manual },
        &principal, OperationId::generate().unwrap()).unwrap();
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
        let working = &engine.state().file_containers[&input.original.file_id].working;
        assert_eq!(working, &input.clean);
        if input.edit.path.as_str() == "first.txt" {
            assert!(matches!(working, storage_contract::file_draft::FileWorkingState::ManualClean { .. }));
        } else { assert!(matches!(working, storage_contract::file_draft::FileWorkingState::Automatic { .. })); }
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
    drop(engine);
    // Startup just wrote its verified historical checkpoint. Both the reserve
    // and outcome now need their original preflight, not a skipped predecessor.
    let engine = Engine::open(&fixture.system).unwrap();
    assert_closed(&engine, &request, expected, original, other);
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
        let (source_paths, destination_names) = {
            use crate::snapshot_worktree::{commit::content::Originals, registry::ContentRetry};
            let index = engine.snapshot_receipts().unwrap();
            let Some(ContentRetry::Reserved(exact)) = index.retry_content(&request, &principal).unwrap() else { panic!("not frozen"); };
            let source = Originals::retained(&engine, &index, &exact).unwrap();
            let planned = source.project_reserved(&engine.planning_context(), &principal, exact.plan()).unwrap();
            let sparse = source.sparse.directory().path();
            let paths = plan.rows.iter().map(|r| sparse.join(r.edit.leaf.entry().as_path())).collect::<Vec<_>>();
            let names = plan.rows.iter().map(|r| crate::persist::layout::snapshot::file(
                &planned.manifest.payloads()[&r.edit.material.version_id])).collect::<Vec<_>>();
            (paths, names)
        };
        engine.inject_fault_once(cut);
        assert!(engine.commit_directory_worktree(&request, &principal).is_err(), "{cut} did not interrupt the original");
        use std::os::unix::fs::MetadataExt;
        let moved = match cut { "after_worktree_content_move_1" => 1,
            "after_worktree_content_move_2" | "after_worktree_content_node" | "after_worktree_content_committing"
                | "after_worktree_content_completed" | "after_worktree_content_response" => 2, _ => 0 };
        let at_node = matches!(cut, "after_worktree_content_node" | "after_worktree_content_committing"
            | "after_worktree_content_completed" | "after_worktree_content_response");
        let snapshot_layout = fixture.snapshot_path();
        let final_node = snapshot_layout.join(crate::persist::layout::snapshot::node(plan.destination_node).as_path());
        assert_eq!(final_node.exists(), at_node, "{cut}");
        for (i, row) in plan.rows.iter().enumerate() {
            if i >= moved {
                let meta = fs::symlink_metadata(&source_paths[i]).unwrap();
                assert_eq!((meta.dev(), meta.ino()), (row.edit.entity.device(), row.edit.entity.inode()), "{cut}");
            } else { assert!(!source_paths[i].exists(), "{cut}"); }
            if at_node {
                let path = final_node.join(crate::persist::layout::snapshot::FILES.as_path()).join(destination_names[i].as_path());
                let meta = fs::symlink_metadata(path).unwrap();
                assert_eq!((meta.dev(), meta.ino()), (row.edit.entity.device(), row.edit.entity.inode()), "{cut}");
            }
        }
        let ledger = snapshot_layout.join(crate::persist::layout::snapshot::node(plan.source.node_id).as_path())
            .join(crate::persist::layout::snapshot::WORKTREES.as_path()).join(
                crate::persist::layout::worktree::ledger(plan.request.target.worktree_id).as_path());
        let committing = plan.before.sequence.next().unwrap();
        let completed = committing.next().unwrap();
        let has_commit = matches!(cut, "after_worktree_content_committing" | "after_worktree_content_completed" | "after_worktree_content_response");
        let has_completed = matches!(cut, "after_worktree_content_completed" | "after_worktree_content_response");
        assert_eq!(ledger.join(crate::persist::layout::worktree::frame(committing).as_path()).exists(), has_commit, "{cut}");
        assert_eq!(ledger.join(crate::persist::layout::worktree::frame(completed).as_path()).exists(), has_completed, "{cut}");
        drop(engine);
        let mut engine = Engine::open(&fixture.system).unwrap();
        let expected = engine.commit_directory_worktree(&request, &principal).unwrap();
        assert_eq!(engine.state().operations.get(&request.operation_id), Some(&reserve), "{cut}");
        assert_eq!(expected, plan.handle(), "{cut} changed destination");
        assert_eq!(engine.state().events.len(), before + 2, "{cut} installed another business slot");
        assert_closed(&engine, &request, expected, fixture.source, other);
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn two_dirty_instances_competing_for_the_same_heads_never_publish_a_replacement_or_implicit_fork() {
    let (fixture, mut engine, principal, first, other) = prepare();
    let scratch = tempfile::tempdir().unwrap();
    save(&mut engine, other, &principal, scratch.path(), "first.txt", b"second instance");
    save(&mut engine, other, &principal, scratch.path(), "Folder/Child/existing.txt", b"other nested");
    let contender = CommitWorktree { target: other, expected: engine.directory_worktree_status(other).unwrap().revision,
        message: "do not silently fork this batch".into(), operation_id: OperationId::generate().unwrap() };
    engine.inject_fault_once("after_worktree_content_reserve");
    assert!(engine.commit_directory_worktree(&first, &principal).is_err());
    let before = engine.state().events.len();
    assert!(engine.commit_directory_worktree(&contender, &principal).is_err());
    assert_eq!(engine.state().events.len(), before);
    assert!(!engine.state().operations.contains_key(&contender.operation_id));
    drop(engine);
    let mut engine = Engine::open(&fixture.system).unwrap();
    engine.commit_directory_worktree(&first, &principal).unwrap();
    assert!(engine.commit_directory_worktree(&contender, &principal).is_err());
    assert!(!engine.state().operations.contains_key(&contender.operation_id));
    assert_eq!(engine.state().events.len(), before + 1);
    assert_eq!(bytes(&engine, other, "first.txt"), b"second instance");
    assert_eq!(bytes(&engine, other, "Folder/Child/existing.txt"), b"other nested");
}

#[cfg(feature = "fault-injection")]
#[test]
fn abandoned_occupancy_prepare_is_not_a_permanent_reservation_or_permission_to_rebind_another_actor() {
    let (fixture, mut engine, principal, request, other) = prepare();
    let before = engine.state().events.len();
    engine.inject_fault_once("after_prepare_1");
    assert!(engine.commit_directory_worktree(&request, &principal).is_err());
    assert!(!engine.state().operations.contains_key(&request.operation_id));
    drop(engine);
    let mut engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.state().events.len(), before);
    assert_eq!(bytes(&engine, other, "first.txt"), FIRST_ORIGINAL);
    assert_eq!(bytes(&engine, request.target, "first.txt"), b"sealed current");
    let snapshot = fixture.media();
    let mut changed = request.clone(); changed.message.push('!');
    assert!(engine.commit_directory_worktree(&changed, &principal).is_err());
    assert!(engine.commit_directory_worktree(&request, &"other".parse().unwrap()).is_err());
    assert_eq!(fixture.media(), snapshot, "unaccepted input cannot authorize an alternative actor/plan");
    let handle = engine.commit_directory_worktree(&request, &principal).unwrap();
    assert_eq!(engine.state().events.len(), before + 2);
    assert_closed(&engine, &request, handle, fixture.source, other);
}

#[test]
fn pending_originals_freeze_only_their_effective_future_file_routes_in_live_and_cold_slots() {
    use crate::snapshot_worktree::commit::content::{self, Originals};

    let (fixture, mut engine, principal, request, other) = prepare();
    let repository = fixture.targets[0].repository.to_string();
    let first = fixture.targets[0].file.to_string();
    let (primary, copies) = engine.new_file_placement_for_file(
        &repository, Some(&first), "first.txt",
    ).unwrap();
    assert_eq!(copies, 1);
    let other_slot = engine.config().disks.iter().find(|d| d.level == 1 && d.slot_id != primary)
        .unwrap().slot_id.clone();
    let policy = crate::new_id("policy").unwrap();
    engine.update_protection_policy(crate::ProtectionPolicyUpdate {
        policy_id: &policy,
        repository_id: &repository,
        path_prefix: "Folder",
        target_copies: 1,
        expected_generation: 0,
        principal: principal.as_str(),
        operation_id: OperationId::generate().unwrap(),
    }).unwrap();

    let originals = {
        let receipts = engine.snapshot_receipts().unwrap();
        Originals::capture(&engine, &receipts, &request).unwrap()
    };
    content::reserve(&mut engine, originals, &principal).unwrap();
    let place = |engine: &mut Engine, path: &str, slot: &str, operation| {
        engine.update_placement_rule(crate::PlacementRuleUpdate {
            rule_id: &crate::new_id("rule").unwrap(),
            repository_id: &repository,
            path_prefix: path,
            primary_slot_id: slot,
            expected_generation: 0,
            principal: principal.as_str(),
            operation_id: operation,
        })
    };
    let blocked = OperationId::generate().unwrap();
    assert!(place(&mut engine, "FIRST.txt", &other_slot, blocked).is_err());
    assert!(!engine.state().operations.contains_key(&blocked));
    // Same effective slot on a reserved nested occurrence; different slot on
    // the untouched third FILE. Neither change can thaw a reserved original.
    place(&mut engine, "Folder/Child", &primary, OperationId::generate().unwrap()).unwrap();
    place(&mut engine, "second.txt", &other_slot, OperationId::generate().unwrap()).unwrap();
    drop(engine);

    let mut engine = Engine::open(&fixture.system).unwrap();
    let fences = engine.snapshot_receipts().unwrap().content_replay_fences();
    let reserved = &engine.state().operations[&request.operation_id];
    fences.require_next(engine.state(), reserved, engine.config().max_entries).unwrap();
    let mut moved = engine.state().clone();
    let rule = moved.placement_rules.values().find(|r| r.path_prefix == "Folder/Child")
        .unwrap().rule_id.clone();
    moved.placement_rules.get_mut(&rule).unwrap().primary_slot_id = other_slot;
    assert!(fences.require_next(&moved, reserved, engine.config().max_entries).is_err());
    let mut extra_copy = engine.state().clone();
    extra_copy.protection_policies.get_mut(&policy).unwrap().target_copies = 2;
    assert!(fences.require_next(&extra_copy, reserved, engine.config().max_entries).is_err());

    let handle = engine.commit_directory_worktree(&request, &principal).unwrap();
    assert_closed(&engine, &request, handle, fixture.source, other);
}

#[cfg(feature = "fault-injection")]
#[path = "content_install_obstructions.rs"]
mod install_obstructions;

#[path = "clean_mount.rs"]
mod clean_mount;
