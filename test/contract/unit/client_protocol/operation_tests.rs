use super::*;
use json_codec::json;

// Captured HTTP responses, not shorthand status objects. Metadata was captured
// by the namespace-batch probe; content fixtures include later damaged copies.
fn receipts() -> Vec<(Value, OperationStatus)> {
    [
        (
            include_str!("../../../web/unit/files/fixtures/namespace-batch-operation.json"),
            OperationStatus::Published,
        ),
        (
            include_str!("../../../web/unit/files/fixtures/damaged-published-save-operation.json"),
            OperationStatus::Published,
        ),
        (
            include_str!("../../../web/unit/files/fixtures/degraded-save-operation.json"),
            OperationStatus::Degraded,
        ),
    ]
    .into_iter()
    .map(|(body, status)| (json_codec::from_str(body).unwrap(), status))
    .collect()
}

#[test]
fn actual_metadata_and_content_receipts_share_one_closed_parser() {
    for (value, status) in receipts() {
        assert_eq!(operation_observation_status(&value), Some(status));
        for key in value.as_object().unwrap().keys() {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert_eq!(
                operation_observation_status(&missing),
                None,
                "missing {key}"
            );
        }
        for (key, malformed) in [
            ("operation_id", json!("")),
            ("operation_kind", json!(null)),
            ("request_digest", json!("digest")),
            ("state", json!("SUCCEEDED")),
            ("commit_state", json!("SUCCEEDED")),
            ("result", json!(null)),
            ("error", json!({})),
            ("current_health", json!({"state":"HEALTHY"})),
            ("unexpected", json!(true)),
        ] {
            let mut invalid = value.clone();
            invalid[key] = malformed;
            assert_eq!(
                operation_observation_status(&invalid),
                None,
                "invalid {key}"
            );
        }
        assert_eq!(
            operation_observation_status(&json!({"operation":value})),
            None
        );
    }
}

#[test]
fn legacy_and_local_markers_are_never_network_receipts() {
    for state in [
        "SUCCEEDED",
        "succeeded",
        "completed",
        "FAILED",
        "failed",
        "cancelled",
        "REJECTED",
        "in_doubt",
    ] {
        assert_eq!(
            operation_observation_status(&json!({"operation_id":"original","state":state})),
            None
        );
    }
    for commit in [
        "PUBLISHED",
        "ACKED",
        "PUBLISHED_DEGRADED",
        "ABORTED",
        "IN_DOUBT",
    ] {
        assert_eq!(
            operation_observation_status(&json!({"operation_id":"original","commit_state":commit})),
            None
        );
    }
}

#[test]
fn published_content_requires_complete_frozen_protection_evidence() {
    for (value, _) in receipts().into_iter().skip(1) {
        for path in [
            "/result",
            "/result/versions/0",
            "/result/versions/0/slots/0",
            "/current_health",
        ] {
            for key in value.pointer(path).unwrap().as_object().unwrap().keys() {
                let mut missing = value.clone();
                missing
                    .pointer_mut(path)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove(key);
                assert_eq!(
                    operation_observation_status(&missing),
                    None,
                    "missing {path}/{key}"
                );
            }
            let mut extra = value.clone();
            extra.pointer_mut(path).unwrap()["unexpected"] = true.into();
            assert_eq!(operation_observation_status(&extra), None, "extra {path}");
        }
        for (path, malformed) in [
            ("/result/branch_generation", json!(2)),
            (
                "/result/publication_sequence",
                json!("18446744073709551616"),
            ),
            ("/result/membership_epoch", json!("01")),
            ("/result/versions", json!([])),
            ("/result/versions/0/required_slots", json!(["P01", "P01"])),
            ("/result/versions/0/slots", json!([])),
            ("/result/versions/0/slots/0/copy_level", json!(0)),
            ("/result/versions/0/slots/0/binding_generation", json!(1)),
            ("/result/versions/0/slots/0/state", json!("COMPLETE")),
            ("/result/protection_at_publish", json!("UNCONFIRMED")),
            ("/current_health/state", json!("RECOVERING")),
            ("/current_health/checked_at", json!(null)),
        ] {
            let mut invalid = value.clone();
            *invalid.pointer_mut(path).unwrap() = malformed;
            assert_eq!(
                operation_observation_status(&invalid),
                None,
                "invalid {path}"
            );
        }
        let mut contradictory = value.clone();
        contradictory["result"]["outcome"] =
            if value["result"]["outcome"] == "COMMITTED_DEGRADED" {
                "COMMITTED_AND_PROTECTED"
            } else {
                "COMMITTED_DEGRADED"
            }
            .into();
        assert_eq!(operation_observation_status(&contradictory), None);
    }
}

#[test]
fn state_is_correlated_with_publication_instead_of_used_as_a_fallback() {
    for (value, _) in receipts() {
        let mut pending = value.clone();
        pending["commit_state"] = "IN_DOUBT".into();
        assert_eq!(operation_observation_status(&pending), None);
        pending["state"] = "in_doubt".into();
        if pending["operation_kind"] == "content_save" {
            pending["result"] = Value::Null;
        } else {
            pending["protection_at_publish"] = "UNCONFIRMED".into();
        }
        assert_eq!(operation_observation_status(&pending), None);
        pending["current_health"] = Value::Null;
        assert_eq!(
            operation_observation_status(&pending),
            Some(OperationStatus::Unknown)
        );
        pending["state"] = "completed".into();
        assert_eq!(operation_observation_status(&pending), None);
    }
    let mut degraded = receipts()[1].0.clone();
    degraded["commit_state"] = "PUBLISHED_DEGRADED".into();
    assert_eq!(operation_observation_status(&degraded), None);
}

#[test]
fn current_abort_shapes_are_rejected_without_inventing_publication() {
    for (mut value, _) in receipts() {
        value["commit_state"] = "ABORTED".into();
        let content = value["operation_kind"] == "content_save";
        value["state"] = if content { "processing" } else { "in_doubt" }.into();
        if content {
            value["result"] = Value::Null;
        }
        if content {
            assert_eq!(operation_observation_status(&value), None);
            value["state"] = "failed".into();
            assert_eq!(operation_observation_status(&value), None);
            value["error"] = json!({"code":"ABORTED", "message":"aborted", "retryable":false, "request_id":"req_11111111111111111111111111111111"});
        }
        assert_eq!(operation_observation_status(&value), None);
        value["current_health"] = Value::Null;
        assert_eq!(
            operation_observation_status(&value),
            Some(OperationStatus::Rejected)
        );
    }
}

#[test]
fn metadata_sequence_and_protection_have_current_types() {
    let value = receipts()[0].0.clone();
    for (key, invalid) in [
        ("publication_sequence", json!(12)),
        ("publication_sequence", json!("012")),
        ("publication_sequence", json!("18446744073709551616")),
        ("protection_at_publish", json!("UNCONFIRMED")),
        ("protection_at_publish", json!("UNPROTECTED")),
        ("result", json!([])),
    ] {
        let mut malformed = value.clone();
        malformed[key] = invalid;
        assert_eq!(operation_observation_status(&malformed), None);
    }
}

#[test]
fn frozen_copy_evidence_cannot_contradict_the_publication_label() {
    let value = receipts()[1].0.clone();
    for (path, invalid) in [
        ("/result/versions/0/target_copies", json!(2)),
        ("/result/versions/0/primary_slot_id", json!("B01")),
        ("/result/versions/0/slots/0/copy_level", json!(2)),
        ("/result/versions/0/slots/0/state", json!("UNAVAILABLE")),
        ("/result/versions/0/slots/1/slot_id", json!("P01")),
        (
            "/result/versions/0/slots/1/disk_id",
            value["result"]["versions"][0]["slots"][0]["disk_id"].clone(),
        ),
    ] {
        let mut malformed = value.clone();
        *malformed.pointer_mut(path).unwrap() = invalid;
        assert_eq!(
            operation_observation_status(&malformed),
            None,
            "contradictory {path}"
        );
    }
    let mut duplicate = value.clone();
    duplicate["result"]["versions"]
        .as_array_mut()
        .unwrap()
        .push(value["result"]["versions"][0].clone());
    assert_eq!(operation_observation_status(&duplicate), None);
    let mut degraded = receipts()[2].0.clone();
    for version in degraded["result"]["versions"].as_array_mut().unwrap() {
        for slot in version["slots"].as_array_mut().unwrap() {
            slot["state"] = "COMPLETE_VERIFIED".into();
        }
    }
    assert_eq!(operation_observation_status(&degraded), None);
}

#[test]
fn receipt_publication_states_reject_object_aliases() {
    for (value, _) in receipts() {
        for field in ["commit_state", "protection_at_publish"] {
            if let Some(token) = value.get(field).and_then(Value::as_str) {
                let mut invalid = value.clone();
                invalid[field] = json!({token:null});
                assert_eq!(
                    operation_observation_status(&invalid),
                    None,
                    "object alias for {field}"
                );
            }
        }
    }
}

#[test]
fn outcome_projection_is_closed_and_never_implies_current_health() {
    for (mut observed, status) in receipts() {
        let health = observed
            .as_object_mut()
            .unwrap()
            .remove("current_health")
            .unwrap();
        assert_eq!(operation_status(&observed), Some(status));
        assert_eq!(operation_observation_status(&observed), None);
        for key in observed.as_object().unwrap().keys() {
            let mut missing = observed.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert_eq!(operation_status(&missing), None, "missing {key}");
        }
        observed["current_health"] = health;
        assert_eq!(operation_status(&observed), None);
        assert_eq!(operation_observation_status(&observed), Some(status));
        observed["current_health"] = Value::Null;
        assert_eq!(operation_observation_status(&observed), Some(status));
        assert_eq!(operation_status(&observed), None);
    }
}
