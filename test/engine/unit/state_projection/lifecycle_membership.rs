//! Pure derived-index limits. These inputs do not claim to be published State.
use super::*;

#[test]
fn lifecycle_membership_index_counts_bounded_file_operation_memberships() {
    let file = "file_00000000000000000000000000000001";
    let other = "file_00000000000000000000000000000002";
    let mut index = LifecycleMembershipIndex::default();
    let operation: OperationId = "op_00000000000000000000000000000001".parse().unwrap();
    assert!(index.observe(operation, &DeletionOwner::Files(vec![file, other]), false).unwrap());
    assert!(index.observe(operation, &DeletionOwner::File(file), true).unwrap());
    assert_eq!(index.count, 3);
    let first = index.for_file(&HistoryFileId::parse(file).unwrap());
    let second = index.for_file(&HistoryFileId::parse(other).unwrap());
    assert_eq!(first.deletions.keys().copied().collect::<Vec<_>>(), [operation]);
    assert_eq!(first.copies.keys().copied().collect::<Vec<_>>(), [operation]);
    assert_eq!(second.deletions.keys().copied().collect::<Vec<_>>(), [operation]);
    assert!(second.copies.is_empty());
    assert!(index.observe(operation, &DeletionOwner::File(file), false).unwrap());
    assert_eq!(index.count, 3);
}

#[test]
fn lifecycle_membership_cache_overflow_drops_optimization_not_target_members() {
    use crate::state_projection::lifecycles::LifecycleSource;
    use storage_contract::{CommitState, CurrentHealth, ProtectionAtPublish};

    let file = HistoryFileId::parse("file_00000000000000000000000000000001").unwrap();
    let mut state = crate::State::default();
    for sequence in 1..=storage_contract::file_lifecycle::MAX_AUDIT_EVENTS + 1 {
        let operation_id = format!("op_{sequence:032x}").parse().unwrap();
        state.operations.insert(operation_id, storage_contract::OperationResult {
            operation_id, kind: "file.delete".into(), commit_id: None,
            history_tree_id: None, branch_id: None, commit_state: CommitState::Published,
            protection_at_publish: ProtectionAtPublish::NotApplicable,
            current_health: CurrentHealth::Healthy, request_digest: "membership-only".into(),
            sequence: sequence as u64,
            result: json_codec::json!({"deletion_node":{"file_id":file.as_str()}}),
        });
    }
    let source = LifecycleSource::capture(&state, &file, || Ok(())).unwrap();
    assert_eq!(source.view().deletions().unwrap().len(),
        storage_contract::file_lifecycle::MAX_AUDIT_EVENTS + 1);
    assert!(source.into_index().is_none());
}
