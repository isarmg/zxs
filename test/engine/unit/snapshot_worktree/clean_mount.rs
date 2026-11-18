//! Real same-node siblings, ordinary post-Hide reuse and exact old receipts.
use super::*;
use storage_contract::snapshot_worktree::mount_change::{MountAction, MountChange, MountOutcome, RestoreNaming};
use name_policy::Name;

#[test]
fn hide_releases_only_the_visible_name_and_admin_restore_reuses_only_the_original_instance() {
    let (fixture, mut engine, actor, commit, sibling) = prepare();
    let initial = engine.directory_worktree_status(commit.target).unwrap();
    let hide = |visible, committed| MountChange {
        target: commit.target, expected: commit.expected, operation_id: OperationId::generate().unwrap(),
        action: MountAction::Hide { committed, visible },
    };
    assert!(engine.change_directory_worktree_mount(&hide(initial.visible, fixture.source), &actor).is_err());
    let committed = engine.commit_directory_worktree(&commit, &actor).unwrap();
    let files = engine.state().versions.len();
    let commits = engine.state().commits.len();
    let hide = hide(initial.visible, committed);
    let hidden = engine.change_directory_worktree_mount(&hide, &actor).unwrap();
    assert_eq!(hidden.outcome, MountOutcome::Hidden {});
    assert!(engine.directory_worktree_status(commit.target).is_err());
    assert_eq!(bytes(&engine, sibling, "first.txt"), FIRST_ORIGINAL);
    let owner = fixture.targets[0].repository;
    let root = engine.snapshot_root_context(owner).unwrap();
    let fresh = engine.allocate_directory_worktree(&AllocateWorktree {
        source: fixture.source, parent_directory_id: root.host.directory_id,
        expected_root_generation: root.host.expected_root_generation, operation_id: OperationId::generate().unwrap(),
        naming: WorktreeNaming::Automatic,
    }, &actor).unwrap();
    assert_ne!(fresh, commit.target);
    assert_eq!(engine.directory_worktree_status(fresh).unwrap().name, initial.name);
    let restore = |naming| MountChange {
        target: commit.target, expected: commit.expected, operation_id: OperationId::generate().unwrap(),
        action: MountAction::Restore { deletion: hide.operation_id, naming },
    };
    let collision = restore(RestoreNaming::Explicit { name: Name::parse(initial.name.as_str()).unwrap() });
    assert!(engine.change_directory_worktree_mount(&collision, &actor).is_err());
    assert!(!engine.state().operations.contains_key(&collision.operation_id));
    let restore = restore(RestoreNaming::OriginalOrAutomatic {});
    let restored = engine.change_directory_worktree_mount(&restore, &actor).unwrap();
    let state = engine.directory_worktree_status(commit.target).unwrap();
    assert_eq!(restored.outcome, MountOutcome::Restored { name: Name::parse(state.name.as_str()).unwrap() });
    assert_eq!(state.visible, restore.operation_id);
    assert_ne!(state.name, initial.name);
    assert_eq!(state.revision, initial.revision);
    assert_eq!(bytes(&engine, commit.target, "first.txt"), b"sealed current");
    assert_eq!(bytes(&engine, fresh, "first.txt"), FIRST_ORIGINAL);
    assert_eq!(engine.change_directory_worktree_mount(&hide, &actor).unwrap(), hidden);
    assert!(engine.directory_worktree_mount_receipt(owner, hide.operation_id, &"stranger".parse().unwrap()).is_err());
    let stale = MountChange { operation_id: OperationId::generate().unwrap(), ..hide };
    assert!(engine.change_directory_worktree_mount(&stale, &actor).is_err());
    assert!(!engine.state().operations.contains_key(&stale.operation_id));
    assert_eq!(engine.state().versions.len(), files);
    assert_eq!(engine.state().commits.len(), commits);
}

fn command(commit: &CommitWorktree, action: MountAction) -> MountChange {
    MountChange { target: commit.target, expected: commit.expected, operation_id: OperationId::generate().unwrap(), action }
}

#[test]
fn cold_replays_only_the_two_native_round_trips_and_rejects_an_unowned_canonical_ledger_frame() {
    use crate::persist::layout::{snapshot,worktree};
    let (fixture, mut engine, actor, request, sibling) = prepare();
    let committed = engine.commit_directory_worktree(&request,&actor).unwrap();
    let owner = fixture.targets[0].repository;
    let original = engine.directory_worktree_status(request.target).unwrap();
    let mut visible = original.visible;
    let mut history = Vec::new();
    let (versions,commits) = (engine.state().versions.clone(),engine.state().commits.clone());
    let mut after = None;
    for _ in 0..2 {
        let hide = command(&request, MountAction::Hide { committed, visible });
        let hidden = engine.change_directory_worktree_mount(&hide,&actor).unwrap();
        history.push(hidden.clone());
        drop(engine);
        engine = Engine::open(&fixture.system).unwrap();
        assert!(engine.directory_worktree_mounts(owner).unwrap().iter().all(|s| s.target != request.target));
        assert_eq!(engine.directory_worktree_mount_receipt(owner,hide.operation_id,&actor).unwrap(),hidden);
        assert_eq!(bytes(&engine,sibling,"first.txt"), FIRST_ORIGINAL);
        let restore = command(&request, MountAction::Restore { deletion: hide.operation_id, naming: RestoreNaming::OriginalOrAutomatic {} });
        let restored = engine.change_directory_worktree_mount(&restore,&actor).unwrap();
        assert_eq!(restored.outcome,MountOutcome::Restored { name: Name::parse(original.name.as_str()).unwrap() });
        history.push(restored);
        let Record::ReserveCleanMount(plan) = codec::decode(&engine.state().operations[&restore.operation_id]).unwrap().unwrap().record
            else { panic!("missing original reserve") };
        let Record::PublishCleanMount(result) = codec::decode(&engine.state().operations[&plan.publish_operation]).unwrap().unwrap().record
            else { panic!("missing exact mount result") };
        after = Some((plan,result));
        visible = restore.operation_id;
        drop(engine);
        engine = Engine::open(&fixture.system).unwrap();
        let current = engine.directory_worktree_status(request.target).unwrap();
        assert_eq!(current.visible,visible);
        assert_eq!(current.revision, original.revision);
        assert_eq!(bytes(&engine,request.target,"first.txt"),b"sealed current");
        for row in &history {
            assert_eq!(engine.directory_worktree_mount_receipt(owner,row.request.operation_id,&actor).unwrap(),*row);
        }
    }
    assert_eq!(engine.state().versions,versions);
    assert_eq!(engine.state().commits,commits);
    let (mut fake,last) = after.unwrap();
    drop(engine);
    // Same registration, source, worktree and predecessor, a syntactically
    // valid fresh Hide. There is no matching native reserve/decision.
    fake.before = last.checkpoint;
    fake.request = command(&request,MountAction::Hide { committed, visible });
    fake.resolved = crate::persist::snapshot_worktree::mount_change::Resolved::Hide {};
    let rogue = crate::directory_worktree::ledger::mount_test_frame(&fake).unwrap();
    let directory = fixture.snapshot_path().join(snapshot::node(fake.source.node_id).as_path())
        .join(snapshot::WORKTREES.as_path()).join(worktree::ledger(request.target.worktree_id).as_path());
    let path = directory.join(worktree::frame(last.checkpoint.sequence.next().unwrap()).as_path());
    fs::write(&path, &rogue).unwrap();
    assert!(Engine::open(&fixture.system).is_err());
    assert_eq!(fs::read(path).unwrap(),rogue);
}

#[cfg(feature = "fault-injection")]
#[test]
fn hide_and_restore_keep_their_original_native_occupancy_across_all_install_boundaries() {
    for cut in ["after_worktree_clean_mount_reserve", "after_prepare_1", "after_accept_1",
        "after_finalize_1", "after_finalize", "after_worktree_clean_mount_frame", "after_worktree_clean_mount_response"] {
        let (fixture,mut engine,actor,request,sibling) = prepare();
        let committed = engine.commit_directory_worktree(&request,&actor).unwrap();
        let original = engine.directory_worktree_status(request.target).unwrap();
        let hide = command(&request,MountAction::Hide { committed, visible: original.visible });
        let restore = command(&request,MountAction::Restore { deletion: hide.operation_id, naming: RestoreNaming::OriginalOrAutomatic {} });
        let files = engine.state().versions.clone();
        for change in [&hide,&restore] {
            if cut != "after_worktree_clean_mount_reserve" {
                let pending = engine.reserve_directory_worktree_mount(change,&actor).unwrap();
                assert_eq!(pending.outcome,MountOutcome::Pending {});
                assert!(engine.directory_worktree_status(request.target).is_err());
                assert_eq!(bytes(&engine,sibling,"first.txt"),FIRST_ORIGINAL);
            }
            engine.inject_fault_once(cut);
            assert!(engine.change_directory_worktree_mount(change,&actor).is_err(), "{cut}");
            drop(engine);
            engine = Engine::open(&fixture.system).unwrap();
            let result = engine.change_directory_worktree_mount(change,&actor).unwrap();
            assert!(!matches!(result.outcome,MountOutcome::Pending {}));
            assert_eq!(engine.state().versions,files);
            assert_eq!(engine.change_directory_worktree_mount(change,&actor).unwrap(), result);
        }
        assert_eq!(engine.directory_worktree_status(request.target).unwrap().visible,restore.operation_id);
    }
}
