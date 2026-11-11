//! The first cases use the parent's verified Current projection. The final
//! case performs actual D3 retirement and reopens the ordinary publication.
use super::*;
use crate::{BatchUndoActor, UndoFileDeletionBatchRequest};

fn undo(
    fixture: &mut Fixture,
    original: OperationId,
    operation: OperationId,
) -> Result<OperationResult> {
    fixture
        .engine
        .undo_file_deletion_batch(UndoFileDeletionBatchRequest {
            deletion_operation_id: original,
            principal: BatchUndoActor::parse("admin").unwrap(),
            operation_id: operation,
        })
}

#[test]
fn membership_registry_current_file_batch_undo_restores_only_the_original_current_members() {
    let mut fixture = Fixture::new(true);
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/c",
        b"third",
    );
    let scope = initialize(&mut fixture, "dir");
    fixture.project(false);
    let b = current(&fixture, "dir/b");
    let c = current(&fixture, "dir/c");
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Delete,
        items: vec![
            item(&fixture, &scope, "dir/b", None),
            item(&fixture, &scope, "dir/c", None),
        ],
    };
    let deleted = apply(&mut fixture, &request, op()).unwrap();
    let frozen = fixture.engine.state().clone();
    let preview = fixture
        .engine
        .preflight_file_deletion_batch_undo(
            deleted.operation_id,
            BatchUndoActor::parse("admin").unwrap(),
        )
        .unwrap();
    assert_eq!(
        json_codec::to_value(preview.deletion_nodes).unwrap(),
        deleted.result["deletion_nodes"]
    );
    let historical_commits = frozen.commits.clone();
    let historical_trees = frozen.directory_trees.clone();
    let operation = op();
    let restored = undo(&mut fixture, deleted.operation_id, operation).unwrap();
    assert_eq!(
        restored.result["removed_deletion_nodes"],
        deleted.result["deletion_nodes"]
    );
    assert_eq!(current(&fixture, "dir/b"), b);
    assert_eq!(current(&fixture, "dir/c"), c);
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("dir/a")
            .is_none()
    );
    assert_eq!(fixture.engine.state().commits, historical_commits);
    assert_eq!(fixture.engine.state().directory_trees, historical_trees);
    assert_eq!(body(&fixture.engine, &b), b"retained");
    assert_eq!(body(&fixture.engine, &c), b"third");
    let after = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        undo(&mut fixture, deleted.operation_id, operation).unwrap(),
        restored
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), after);
}

#[test]
fn membership_registry_current_file_batch_undo_rejects_a_member_without_a_current_occurrence() {
    let mut fixture = Fixture::new(true);
    let scope = initialize(&mut fixture, "dir");
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Delete,
        items: vec![
            item(&fixture, &scope, "dir/a", None),
            item(&fixture, &scope, "dir/b", None),
        ],
    };
    let deleted = apply(&mut fixture, &request, op()).unwrap();
    fixture.project(false);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .preflight_file_deletion_batch_undo(
                deleted.operation_id,
                BatchUndoActor::parse("admin").unwrap()
            )
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedOperation
    );
    assert_eq!(
        undo(&mut fixture, deleted.operation_id, op())
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedOperation
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert_eq!(fixture.engine.state().file_deletions.len(), 2);
}

#[test]
fn disk_exit_current_file_batch_undo_real_d3_reopens_without_touching_the_retired_target() {
    for action in [
        MembershipExitAction::Deregister,
        MembershipExitAction::PlannedOffline,
    ] {
        let mut fixture = Fixture::new(true);
        save(
            &mut fixture.engine,
            &fixture.repo.main_branch_id,
            "dir/c",
            b"third",
        );
        let scope = initialize(&mut fixture, "dir");
        let disk = fixture.engine.disk("P01").unwrap().clone();
        let held = Arc::clone(fixture.engine.disk_resources.get(&disk).unwrap());
        fixture
            .engine
            .begin_disk_exit(&disk.disk_id, action, "admin", op())
            .unwrap();
        let archive = fixture
            .engine
            .build_managed_membership_control_archive()
            .unwrap();
        fixture
            .engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        fixture.engine.resume_membership_joint_decision().unwrap();
        let physical = files(&disk.root);
        fixture.engine.activate_pending_disk_exit().unwrap();
        held.root.io_binding().unwrap().closed_proof().unwrap();
        let b = current(&fixture, "dir/b");
        let c = current(&fixture, "dir/c");
        let request = DirectoryNamespaceBatch {
            kind: DirectoryBatchKind::Delete,
            items: vec![
                item(&fixture, &scope, "dir/b", None),
                item(&fixture, &scope, "dir/c", None),
            ],
        };
        let deleted = apply(&mut fixture, &request, op()).unwrap();
        let frozen_commits = fixture.engine.state().commits.clone();
        let frozen_trees = fixture.engine.state().directory_trees.clone();
        let operation = op();
        let restored = undo(&mut fixture, deleted.operation_id, operation).unwrap();
        assert_eq!(fixture.engine.state().commits, frozen_commits);
        assert_eq!(fixture.engine.state().directory_trees, frozen_trees);
        assert_eq!(current(&fixture, "dir/b"), b);
        assert_eq!(current(&fixture, "dir/c"), c);
        assert_eq!(files(&disk.root), physical);
        drop(held);
        drop(fixture.engine);
        fixture.engine = Engine::open(&fixture._temp.path().join("system")).unwrap();
        assert_eq!(
            undo(&mut fixture, deleted.operation_id, operation).unwrap(),
            restored
        );
        assert_eq!(current(&fixture, "dir/b"), b);
        assert_eq!(current(&fixture, "dir/c"), c);
        assert_eq!(body(&fixture.engine, &b), b"retained");
        assert_eq!(body(&fixture.engine, &c), b"third");
        assert!(fixture.engine.disk_resources.available(&disk).is_none());
        assert_eq!(files(&disk.root), physical);
        if action == MembershipExitAction::Deregister {
            assert_clean(&fixture);
            assert!(fixture.engine.registered_disk(&disk.disk_id).is_none());
        } else {
            assert!(fixture.engine.registered_disk(&disk.disk_id).is_some());
            assert_eq!(current(&fixture, "dir/a"), fixture.removed);
        }
    }
}
