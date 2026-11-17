use crate::snapshot_worktree::SnapshotHandle;
use crate::snapshot_worktree::discovery::SnapshotHeadDiscovery;
use crate::{BranchName, OperationId};
use json_codec::json;

#[test]
fn discovery_wire_distinguishes_absence_pending_and_completed_head() {
    let operation = OperationId::generate().unwrap();
    let snapshot = SnapshotHandle {
        publication: OperationId::generate().unwrap(),
        repository_id: crate::working_tree::SnapshotRepositoryId::generate().unwrap(),
        branch_id: crate::working_tree::SnapshotBranchId::generate().unwrap(),
        node_id: crate::working_tree::SnapshotNodeId::generate().unwrap(),
    };
    let cases = [
        (SnapshotHeadDiscovery::NotInitialized, json!({"state": "not_initialized"})),
        (SnapshotHeadDiscovery::Pending { operation },
            json!({"state": "pending", "operation": operation})),
        (SnapshotHeadDiscovery::Head {
            snapshot, branch_name: BranchName::parse("actual branch").unwrap(),
        }, json!({"state": "head", "snapshot": snapshot, "branch_name": "actual branch"})),
    ];
    for (value, wire) in cases {
        assert_eq!(json_codec::to_value(&value).unwrap(), wire);
        assert_eq!(json_codec::from_value::<SnapshotHeadDiscovery>(wire).unwrap(), value);
    }
    for invalid in [
        json!({"state": "pending"}),
        json!({"state": "pending", "operation": operation, "snapshot": snapshot}),
        json!({"state": "head", "snapshot": snapshot}),
        json!({"state": "head", "snapshot": snapshot, "branch_name": ""}),
        json!({"state": "unknown"}),
    ] {
        assert!(json_codec::from_value::<SnapshotHeadDiscovery>(invalid).is_err());
    }
}
