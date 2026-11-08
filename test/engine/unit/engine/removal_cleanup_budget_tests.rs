use super::*;
use std::sync::Arc;

#[test]
fn cleanup_scan_budget_does_not_revoke_metadata_but_real_source_limit_does() {
    let cancellation = HistoryWindowCancellation::new();
    let integrity = Arc::new(crate::engine::provenance::MetadataIntegrity::default());
    let epoch = integrity.capture();
    let mut budget = Budget {
        cancellation: &cancellation,
        bytes: 1,
        checkpoints: 2,
        io_exhausted: false,
    };
    integrity.observe(budget.read(1)).unwrap();
    assert_eq!(
        integrity.observe(budget.read(1)).unwrap_err().code(),
        ErrorCode::OperationAborted
    );
    assert!(budget.io_exhausted);
    assert!(epoch.matches(&integrity));
    assert_eq!(limit().code(), ErrorCode::ResourceLimit);
    integrity
        .observe::<()>(Err(Error::new(
            ErrorCode::ResourceLimit,
            "stored metadata exceeds its bound",
        )))
        .unwrap_err();
    assert!(!epoch.matches(&integrity));
}

#[test]
fn cleanup_scan_checkpoint_limit_and_cancellation_remain_distinct() {
    let cancellation = HistoryWindowCancellation::new();
    let mut budget = Budget {
        cancellation: &cancellation,
        bytes: 10,
        checkpoints: 0,
        io_exhausted: false,
    };
    assert_eq!(
        budget.read(0).unwrap_err().code(),
        ErrorCode::OperationAborted
    );
    assert!(budget.io_exhausted);
    let mut budget = Budget {
        cancellation: &cancellation,
        bytes: 10,
        checkpoints: 10,
        io_exhausted: false,
    };
    cancellation.cancel();
    assert_eq!(
        budget.read(0).unwrap_err().code(),
        ErrorCode::OperationAborted
    );
    assert!(!budget.io_exhausted);
}
