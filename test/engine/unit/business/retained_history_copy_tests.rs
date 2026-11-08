//! Allocation-free encoded-length counting and pre-decode event count fences.
use super::*;

#[test]
fn retained_history_copy_budget_counts_exact_encoded_bytes_without_a_value_copy() {
    bounded(&"x".repeat(MAX_BYTES - 2)).unwrap();
    assert_eq!(
        bounded(&"x".repeat(MAX_BYTES - 1)).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
    // Escaped characters count their encoded representation, not string length.
    assert_eq!(
        bounded(&"\n".repeat(MAX_BYTES / 2)).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
}

#[test]
fn retained_history_copy_budget_rejects_event_count_before_decoding_any_node() {
    let receipt = OperationResult {
        operation_id: OperationId::generate().unwrap(),
        kind: RETAINED_KIND.into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: crate::CommitState::Published,
        protection_at_publish: crate::ProtectionAtPublish::TargetMet,
        current_health: crate::CurrentHealth::Healthy,
        request_digest: "count-only".into(),
        sequence: 1,
        result: json!({"retained_lifecycle":{"events":vec![Value::Null; 65_537]}}),
    };
    assert_eq!(
        read(&receipt, "not-decoded").unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
}
