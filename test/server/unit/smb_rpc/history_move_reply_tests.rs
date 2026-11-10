use super::*;
use object_receipt_tests::{current, object_result};
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
    let expected = current(operation, false);
    let result = object_result(&expected);
    let original = result.result.clone();
    let reply = expected.published_object_reply(result, "alice").unwrap();
    assert_eq!(reply["operation_id"], operation.to_string());
    assert_eq!(
        reply["operation_kind"],
        storage_contract::file_relocation::OBJECT_MOVE_KIND
    );
    assert_eq!(reply["commit_state"], "PUBLISHED");
    assert_eq!(reply["result"], original);
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
        let expected = current(operation, false);
        let mut result = object_result(&expected);
        result.commit_state = state;
        assert_eq!(
            expected
                .published_object_reply(result, "alice")
                .unwrap_err()
                .code,
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
        let expected = current(operation, false);
        let mut result = object_result(&expected);
        result.protection_at_publish = protection;
        assert_eq!(
            expected
                .published_object_reply(result, "alice")
                .unwrap_err()
                .code,
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
        let expected = current(operation, false);
        let mut result = object_result(&expected);
        result.current_health = health;
        assert_eq!(
            expected
                .published_object_reply(result, "alice")
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::RecoveryRequired
        );
    }
}

#[test]
fn another_operation_or_copy_delete_receipt_cannot_complete_move() {
    let operation = OperationId::generate().unwrap();
    let another = OperationId::generate().unwrap();
    let expected = current(operation, false);
    let mut result = object_result(&expected);
    result.operation_id = another;
    assert_eq!(
        expected
            .published_object_reply(result, "alice")
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::StateInvalid
    );
    for kind in [
        "file.history.move",
        "file.delete",
        "file.history.copy",
        "workspace.commit",
    ] {
        let expected = current(operation, false);
        let mut result = object_result(&expected);
        result.kind = kind.into();
        assert_eq!(
            expected
                .published_object_reply(result, "alice")
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::StateInvalid
        );
    }
}

#[test]
fn target_name_accepts_portable_parent_paths_but_not_lookup_only_spellings() {
    for valid in ["目的.txt", "folder/file.txt"] {
        assert_eq!(crate::MoveTargetName::parse(valid).unwrap().as_str(), valid);
    }
    for invalid in [
        "",
        ".",
        "..",
        "/file.txt",
        "../file.txt",
        "folder/e\u{301}.txt",
    ] {
        assert!(crate::MoveTargetName::parse(invalid).is_err());
    }
}

#[path = "history_move_object_reply_tests.rs"]
mod object_receipt_tests;
