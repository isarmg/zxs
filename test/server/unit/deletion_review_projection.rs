use super::*;
use json_codec::json;
use storage_contract::{CommitState, OperationId, ProtectionAtPublish};

#[test]
fn unfinished_undo_keeps_its_original_publication_state() {
    for state in [
        CommitState::InDoubt,
        CommitState::RecoveryRequired,
        CommitState::Aborted,
    ] {
        let operation = OperationId::generate().unwrap();
        let wire = undo(OperationOutcome {
            operation_id: operation,
            kind: "file.delete.undo".into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: state,
            protection_at_publish: ProtectionAtPublish::NotApplicable,
            request_digest: "original-request-digest".into(),
            sequence: 0,
            result: json!(null),
        })
        .unwrap();
        assert_eq!(wire["operation_id"], json!(operation));
        assert_eq!(wire["commit_state"], json!(state));
        assert_eq!(wire["state"], "in_doubt");
        assert_eq!(wire["result"], json!(null));
    }
}
