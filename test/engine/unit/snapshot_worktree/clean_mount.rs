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
