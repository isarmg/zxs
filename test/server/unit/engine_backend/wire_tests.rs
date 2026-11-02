use super::*;
use crate::operation_wire::operation_wire;
use json_codec::json;
use storage_contract::{
    CommitState, OperationOutcome, ProtectionAtPublish,
    client_protocol::{OperationStatus, operation_status},
};

fn domain(value: &Value) -> OperationOutcome {
    let content = value["operation_kind"] == "content_save";
    let result = &value["result"];
    OperationOutcome {
        operation_id: value["operation_id"].as_str().unwrap().parse().unwrap(),
        kind: if content {
            "workspace.commit"
        } else {
            value["operation_kind"].as_str().unwrap()
        }
        .into(),
        commit_id: result["commit_id"].as_str().map(str::to_owned),
        history_tree_id: None,
        branch_id: result["branch_id"].as_str().map(str::to_owned),
        commit_state: json_codec::from_value(value["commit_state"].clone()).unwrap(),
        protection_at_publish: json_codec::from_value(
            if content { result } else { value }["protection_at_publish"].clone(),
        )
        .unwrap(),
        request_digest: value["request_digest"].as_str().unwrap().into(),
        sequence: storage_contract::parse_u64_decimal(
            if content { result } else { value }["publication_sequence"]
                .as_str()
                .unwrap(),
        )
        .unwrap(),
        result: if content {
            json!({"branch_generation":result["branch_generation"],"membership_epoch":result["membership_epoch"],"version_protection":result["versions"]})
        } else {
            result.clone()
        },
    }
}

fn receipts() -> Vec<Value> {
    [
        include_str!("../../../web/unit/files/fixtures/namespace-batch-operation.json"),
        include_str!("../../../web/unit/files/fixtures/damaged-published-save-operation.json"),
        include_str!("../../../web/unit/files/fixtures/degraded-save-operation.json"),
    ]
    .into_iter()
    .map(|body| json_codec::from_str(body).unwrap())
    .collect()
}

#[test]
fn outcome_wire_matches_derived_fixture_fields_and_the_shared_parser() {
    for mut expected in receipts() {
        let actual = operation_wire(domain(&expected)).unwrap();
        // This comparison derives the outcome projection in memory; captured
        // fixture evidence is not rewritten to claim a new HTTP execution.
        expected.as_object_mut().unwrap().remove("current_health");
        assert_eq!(operation_status(&actual), operation_status(&expected));
        assert!(matches!(
            operation_status(&actual),
            Some(OperationStatus::Published | OperationStatus::Degraded)
        ));
        assert!(actual.get("current_health").is_none());
        assert_eq!(
            json_codec::to_vec(&actual).unwrap(),
            json_codec::to_vec(&expected).unwrap()
        );
        assert_eq!(actual, expected);
    }
}

#[test]
fn unknown_and_aborted_projection_have_one_current_shape_per_operation_kind() {
    for expected in receipts().into_iter().take(2) {
        let content = expected["operation_kind"] == "content_save";
        for commit in [
            CommitState::InDoubt,
            CommitState::RecoveryRequired,
            CommitState::Aborted,
        ] {
            let mut operation = domain(&expected);
            operation.commit_state = commit;
            operation.protection_at_publish = ProtectionAtPublish::Unconfirmed;
            let actual = operation_wire(operation).unwrap();
            assert_eq!(
                operation_status(&actual),
                Some(if commit == CommitState::Aborted {
                    OperationStatus::Rejected
                } else {
                    OperationStatus::Unknown
                })
            );
            if content && commit == CommitState::Aborted {
                assert_eq!(actual["state"], "failed");
                assert!(actual["result"].is_null());
                let error = &actual["error"];
                assert_eq!(error["code"], "OPERATION_ABORTED");
                assert_eq!(error["retryable"], false);
                storage_contract::validate_id(error["request_id"].as_str().unwrap(), "req")
                    .unwrap();
                assert_eq!(actual["error"].as_object().unwrap().len(), 4);
                let mut old = actual.clone();
                old["state"] = "processing".into();
                old["error"] = Value::Null;
                assert_eq!(operation_status(&old), None);
            } else {
                assert_eq!(actual["state"], "in_doubt");
                assert!(actual["error"].is_null());
                assert_eq!(actual["result"].is_null(), content);
            }
        }
    }
}

#[test]
fn stored_version_content_state_preserves_sparse_available_and_explicit_dispositions() {
    use storage_contract::ContentState;
    assert_eq!(stored_version_content_state(None), ContentState::Available);
    for state in [
        ContentState::Available,
        ContentState::Purged,
        ContentState::Migrated,
    ] {
        assert_eq!(stored_version_content_state(Some(&state)), state);
    }
}

#[test]
fn operation_payloads_keep_owned_array_and_string_buffers() {
    let fixtures = receipts();
    let mut ordinary = domain(&fixtures[0]);
    ordinary.result = json!({"payload": [{"text": "retained response payload"}]});
    let array = ordinary.result["payload"].as_array().unwrap().as_ptr();
    let text = ordinary.result["payload"][0]["text"]
        .as_str()
        .unwrap()
        .as_ptr();
    let expected = json_codec::to_vec(&ordinary.result).unwrap();
    let wire = operation_wire(ordinary).unwrap();
    assert_eq!(
        wire["result"]["payload"].as_array().unwrap().as_ptr(),
        array
    );
    assert_eq!(
        wire["result"]["payload"][0]["text"]
            .as_str()
            .unwrap()
            .as_ptr(),
        text
    );
    assert_eq!(json_codec::to_vec(&wire["result"]).unwrap(), expected);

    for fixture in &fixtures[1..] {
        let content = domain(fixture);
        let versions = &content.result["version_protection"];
        let array = versions.as_array().unwrap().as_ptr();
        let version_id = versions[0]["version_id"].as_str().unwrap().as_ptr();
        let expected = json_codec::to_vec(versions).unwrap();
        let wire = operation_wire(content).unwrap();
        let versions = &wire["result"]["versions"];
        assert_eq!(versions.as_array().unwrap().as_ptr(), array);
        assert_eq!(
            versions[0]["version_id"].as_str().unwrap().as_ptr(),
            version_id
        );
        assert_eq!(json_codec::to_vec(versions).unwrap(), expected);
    }
}

#[test]
fn completed_save_keeps_evidence_rejection_precedence() {
    let fixture = &receipts()[1];
    for result in [
        Value::Null,
        json!({}),
        json!({"version_protection": null}),
        json!({"version_protection": {}}),
    ] {
        let mut operation = domain(fixture);
        operation.result = result;
        let error = operation_wire(operation).unwrap_err();
        assert_eq!(error.status, ::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.code, storage_contract::ErrorCode::StateInvalid);
        assert_eq!(
            error.message,
            "frozen publication protection evidence is missing"
        );
    }
    for (result, field) in [
        (json!({"version_protection": []}), "branch_generation"),
        (
            json!({"version_protection": [], "branch_generation": ""}),
            "branch_generation",
        ),
        (
            json!({"version_protection": [], "branch_generation": "1"}),
            "membership_epoch",
        ),
        (
            json!({"version_protection": [], "branch_generation": "1", "membership_epoch": null}),
            "membership_epoch",
        ),
    ] {
        let mut operation = domain(fixture);
        operation.result = result;
        let error = operation_wire(operation).unwrap_err();
        assert_eq!(error.status, ::http::StatusCode::BAD_REQUEST);
        assert_eq!(error.message, format!("missing string field {field}"));
    }
}

#[test]
fn ordinary_payload_values_preserve_wire_bytes_without_shape_restrictions() {
    for result in [
        Value::Null,
        json!(false),
        json!(123),
        json!("Café\n"),
        json!([]),
        json!({}),
        json!([{"nested":[null, true, "Straße"]}]),
    ] {
        let mut operation = domain(&receipts()[0]);
        operation.result = result;
        let expected = json!({"operation_id":operation.operation_id,"operation_kind":operation.kind,"request_digest":operation.request_digest,"state":"completed","commit_state":operation.commit_state,"protection_at_publish":operation.protection_at_publish,"publication_sequence":operation.sequence.to_string(),"result":operation.result,"error":null});
        let actual = operation_wire(operation).unwrap();
        assert_eq!(
            json_codec::to_vec(&actual).unwrap(),
            json_codec::to_vec(&expected).unwrap()
        );
    }
}

#[test]
fn operation_phase_matrix_keeps_existing_result_and_error_shapes() {
    let states = [
        CommitState::Staging,
        CommitState::Sealed,
        CommitState::Prepared,
        CommitState::Deciding,
        CommitState::Decided,
        CommitState::Protected,
        CommitState::Published,
        CommitState::Acked,
        CommitState::Recovering,
        CommitState::RecoveryRequired,
        CommitState::InDoubt,
        CommitState::PublishedDegraded,
        CommitState::Aborted,
    ];
    for kind in ["workspace.commit", "directory.namespace.batch"] {
        for protection in [
            ProtectionAtPublish::NotApplicable,
            ProtectionAtPublish::TargetMet,
            ProtectionAtPublish::BelowTarget,
            ProtectionAtPublish::Unconfirmed,
        ] {
            for state in states {
                let mut operation = domain(&receipts()[1]);
                operation.kind = kind.into();
                operation.protection_at_publish = protection;
                operation.commit_state = state;
                let original_result = json_codec::to_vec(&operation.result).unwrap();
                let wire = operation_wire(operation).unwrap();
                let content =
                    kind == "workspace.commit" && protection != ProtectionAtPublish::NotApplicable;
                let expected_phase = if content {
                    match state {
                        CommitState::Published
                        | CommitState::Acked
                        | CommitState::PublishedDegraded => "completed",
                        CommitState::Aborted => "failed",
                        CommitState::InDoubt | CommitState::RecoveryRequired => "in_doubt",
                        _ => "processing",
                    }
                } else if matches!(state, CommitState::Published | CommitState::Acked) {
                    "completed"
                } else {
                    "in_doubt"
                };
                assert_eq!(wire["state"], expected_phase);
                assert_eq!(wire["commit_state"], json!(state));
                assert_eq!(wire.as_object().unwrap().len(), if content { 7 } else { 9 });
                if content && expected_phase == "completed" {
                    assert_eq!(wire["result"].as_object().unwrap().len(), 8);
                    assert_eq!(wire["result"]["protection_at_publish"], json!(protection));
                    assert!(wire["result"]["versions"].is_array());
                } else if content {
                    assert!(wire["result"].is_null());
                } else {
                    assert_eq!(
                        json_codec::to_vec(&wire["result"]).unwrap(),
                        original_result
                    );
                    assert_eq!(wire["protection_at_publish"], json!(protection));
                }
                if content && state == CommitState::Aborted {
                    assert_eq!(wire["error"]["code"], "OPERATION_ABORTED");
                    assert_eq!(wire["error"]["retryable"], false);
                    assert_eq!(wire["error"].as_object().unwrap().len(), 4);
                } else {
                    assert!(wire["error"].is_null());
                }
            }
        }
    }
}
