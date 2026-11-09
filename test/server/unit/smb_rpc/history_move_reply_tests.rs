use super::*;
use storage_contract::{CommitState, CurrentHealth, OperationResult, ProtectionAtPublish};

fn receipt(operation_id: OperationId) -> OperationResult {
    OperationResult {
        operation_id,
        kind: "file.history.move".into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: CommitState::Published,
        protection_at_publish: ProtectionAtPublish::TargetMet,
        current_health: CurrentHealth::Healthy,
        request_digest: "frozen-original-request".into(),
        sequence: 12,
        result: json!({"source_deletion_node_id":"D-original"}),
    }
}

#[test]
fn move_reply_only_reports_the_exact_published_protected_healthy_operation() {
    let operation = OperationId::generate().unwrap();
    let reply = published_reply(receipt(operation), operation).unwrap();
    assert_eq!(reply["operation_id"], operation.to_string());
    assert_eq!(reply["operation_kind"], "file.history.move");
    assert_eq!(reply["commit_state"], "PUBLISHED");
    assert_eq!(reply["result"]["source_deletion_node_id"], "D-original");
}

#[test]
fn prepared_accepted_unknown_aborted_or_degraded_move_is_not_smb_success() {
    for state in [
        CommitState::Staging,
        CommitState::Sealed,
        CommitState::Prepared,
        CommitState::Deciding,
        CommitState::Decided,
        CommitState::Protected,
        CommitState::Acked,
        CommitState::Recovering,
        CommitState::RecoveryRequired,
        CommitState::InDoubt,
        CommitState::PublishedDegraded,
        CommitState::Aborted,
    ] {
        let operation = OperationId::generate().unwrap();
        let mut result = receipt(operation);
        result.commit_state = state;
        assert_eq!(
            published_reply(result, operation).unwrap_err().code,
            storage_contract::ErrorCode::RecoveryRequired,
            "{state:?}"
        );
    }
}

#[test]
fn published_without_complete_protection_or_health_is_not_smb_success() {
    for protection in [
        ProtectionAtPublish::NotApplicable,
        ProtectionAtPublish::BelowTarget,
        ProtectionAtPublish::Unconfirmed,
    ] {
        let operation = OperationId::generate().unwrap();
        let mut result = receipt(operation);
        result.protection_at_publish = protection;
        assert_eq!(
            published_reply(result, operation).unwrap_err().code,
            storage_contract::ErrorCode::RecoveryRequired
        );
    }
    for health in [
        CurrentHealth::Degraded,
        CurrentHealth::Unavailable,
        CurrentHealth::Unknown,
        CurrentHealth::Recovering,
    ] {
        let operation = OperationId::generate().unwrap();
        let mut result = receipt(operation);
        result.current_health = health;
        assert_eq!(
            published_reply(result, operation).unwrap_err().code,
            storage_contract::ErrorCode::RecoveryRequired
        );
    }
}

#[test]
fn another_operation_or_copy_delete_receipt_cannot_complete_move() {
    let operation = OperationId::generate().unwrap();
    let another = OperationId::generate().unwrap();
    assert_eq!(
        published_reply(receipt(another), operation)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::StateInvalid
    );
    for kind in ["file.delete", "file.history.copy", "workspace.commit"] {
        let mut result = receipt(operation);
        result.kind = kind.into();
        assert_eq!(
            published_reply(result, operation).unwrap_err().code,
            storage_contract::ErrorCode::StateInvalid
        );
    }
}

#[test]
fn target_name_accepts_one_portable_component_but_not_directory_destinations() {
    assert_eq!(
        crate::MoveTargetName::parse("目的.txt").unwrap().as_str(),
        "目的.txt"
    );
    assert_eq!(
        crate::MoveTargetName::parse("folder/file.txt")
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::UnsupportedOperation
    );
    for invalid in ["", ".", "..", "/file.txt", "../file.txt"] {
        assert!(crate::MoveTargetName::parse(invalid).is_err());
    }
}
