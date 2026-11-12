use crate::operation_wire::operation_wire;
use json_codec::{Value, json};
use storage_contract::{CommitState, Generation, OperationId, OperationOutcome, ProtectionAtPublish};
use storage_contract::file_container_tags::{KIND, Replacement};

fn outcome(changed: bool) -> OperationOutcome {
    let command = crate::container_tags_tests::sample_command();
    let replacement = Replacement {
        generation: Generation::new(if changed { 8 } else { 7 }),
        tags: command.tags.clone(),
        request: command,
        changed,
    };
    let mut result = json!(replacement);
    result["timestamp"] = json!("123");
    result["membership_epoch"] = json!("0");
    result["version_protection"] = json!([]);
    OperationOutcome {
        operation_id: OperationId::from_random([4; 16]),
        kind: KIND.into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: CommitState::Published,
        protection_at_publish: ProtectionAtPublish::NotApplicable,
        request_digest: "a".repeat(64),
        sequence: 8,
        result,
    }
}

#[test]
fn completed_container_receipts_expose_exact_typed_outcomes_without_framework_fields() {
    for changed in [false, true] {
        for state in [CommitState::Published, CommitState::Acked] {
            let mut operation = outcome(changed);
            operation.commit_state = state;
            let wire = operation_wire(operation).unwrap();
            assert_eq!(wire["state"], "completed");
            assert_eq!(wire["result"]["changed"], changed);
            let result: Replacement = json_codec::from_value(wire["result"].clone()).unwrap();
            result
                .validate(&crate::container_tags_tests::sample_command())
                .unwrap();
            for field in ["timestamp", "membership_epoch", "version_protection"] {
                assert!(wire["result"].get(field).is_none());
            }
        }
    }
}

#[test]
fn malformed_persisted_container_outcomes_do_not_become_successful_http_responses() {
    for (field, value) in [
        ("generation", json!("7")),
        ("tags", json!([])),
        ("unexpected", json!(true)),
        ("request", Value::Null),
    ] {
        let mut operation = outcome(true);
        operation.result[field] = value;
        let error = operation_wire(operation).unwrap_err();
        assert_eq!(error.status, http::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.code, storage_contract::ErrorCode::StateInvalid);
    }
}

#[test]
fn uncertain_container_operation_is_not_claimed_as_a_completed_replacement() {
    let mut operation = outcome(true);
    operation.commit_state = CommitState::InDoubt;
    operation.result = Value::Null;
    let wire = operation_wire(operation).unwrap();
    assert_eq!(wire["state"], "in_doubt");
    assert!(wire["result"].is_null());
}
