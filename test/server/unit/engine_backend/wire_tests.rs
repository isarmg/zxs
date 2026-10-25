use super::*;
use storage_contract::{
    CommitState, OperationResult, ProtectionAtPublish,
    client_protocol::{OperationStatus, operation_status},
};

fn domain(value: &Value) -> OperationResult {
    let content = value["operation_kind"] == "content_save";
    let result = &value["result"];
    OperationResult {
        operation_id: value["operation_id"].as_str().unwrap().into(),
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
        current_health: value["current_health"]["state"].as_str().unwrap().into(),
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
fn current_wire_matches_recorded_http_fixtures_and_the_shared_parser() {
    for expected in receipts() {
        let mut actual = operation_wire(domain(&expected)).unwrap();
        assert_eq!(operation_status(&actual), operation_status(&expected));
        assert!(matches!(
            operation_status(&actual),
            Some(OperationStatus::Published | OperationStatus::Degraded)
        ));
        // This one live observation timestamp is deliberately not frozen.
        assert!(actual["current_health"]["checked_at"].as_str().is_some());
        actual["current_health"]["checked_at"] = expected["current_health"]["checked_at"].clone();
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
fn stored_version_content_state_uses_canonical_tokens_including_sparse_available_default() {
    use storage_contract::ContentState;
    assert_eq!(stored_version_content_state(None), "AVAILABLE");
    for (state, expected) in [
        (ContentState::Available, "AVAILABLE"),
        (ContentState::Purged, "PURGED"),
        (ContentState::Migrated, "MIGRATED"),
    ] {
        assert_eq!(stored_version_content_state(Some(&state)), expected);
    }
}
