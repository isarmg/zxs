use super::super::request::{self, Kind, QueryKind};
use super::*;
use json_codec::json;
const OPERATION: &str = "op_00000000000000000000000000000001";
fn contract(kind: Kind, fields: &[(&str, &str)]) -> Contract {
    request::build(
        kind,
        &fields
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
        OPERATION,
        "admin",
    )
    .unwrap()
    .contract
}
fn plan_contract(commit: bool) -> Contract {
    request::plan_action(
        &json!({"plan_id":"p","state":"ready","digest":"digest"}),
        commit,
        true,
        OPERATION,
    )
    .unwrap()
    .contract
}
fn plan() -> Value {
    json!({"plan_id":"p","kind":"purge","principal":"admin","expected_sequence":"4","digest":"digest","input":{"history_tree_id":"h"},"state":"ready"})
}
fn raw_operation() -> Value {
    json!({"operation_id":"op_00000000000000000000000000000001", "kind":"plan.cancel", "commit_id":null,"history_tree_id":null,"branch_id":null,"commit_state":"PUBLISHED","protection_at_publish":"NOT_APPLICABLE","current_health":"UNKNOWN","request_digest":"digest","sequence":"4","result":{}})
}
fn wire_operation() -> Value {
    let mut value: Value = json_codec::from_str(include_str!(
        "../files/fixtures/namespace-batch-operation.json"
    ))
    .unwrap();
    // Project the captured diagnostic into the outcome-only wire contract.
    value.as_object_mut().unwrap().remove("current_health");
    value["operation_id"] = "op_00000000000000000000000000000001".into();
    value
}
#[test]
fn operation_contracts_reject_missing_wrong_and_nonterminal_receipts() {
    for commit in [true, false] {
        let c = plan_contract(commit);
        assert!(c.validate(raw_operation()).is_ok());
        for value in [Value::Null, json!({}), wire_operation()] {
            assert!(c.validate(value).is_err());
        }
        for (key, bad) in [("operation_id", "unrelated"), ("commit_state", "IN_DOUBT")] {
            let mut value = raw_operation();
            value[key] = bad.into();
            assert!(c.validate(value).is_err());
        }
    }
    for kind in [Kind::Smb, Kind::Policy, Kind::Rule] {
        let c = contract(
            kind,
            &[
                ("repository", "r"),
                ("branch", "b"),
                ("generation", "1"),
                ("copies", "1"),
                ("id", "p"),
                ("slot", "P01"),
            ],
        );
        assert!(c.validate(wire_operation()).is_ok());
        assert!(c.validate(raw_operation()).is_err());
        let mut value = wire_operation();
        value["operation_id"] = "unrelated".into();
        assert!(c.validate(value).is_err());
    }
}
#[test]
fn inspection_must_match_original_operation_and_terminal_state() {
    let c = request::operation_outcome(OPERATION).contract;
    assert!(c.validate(wire_operation()).is_ok());
    assert!(c.validate(raw_operation()).is_err());
    assert!(terminal(
        &wire_operation(),
        "op_00000000000000000000000000000001"
    ));
    let mut value = wire_operation();
    value["operation_id"] = "other".into();
    assert!(c.validate(value.clone()).is_err());
    assert!(!terminal(&value, "op_00000000000000000000000000000001"));
    value["operation_id"] = "op_00000000000000000000000000000001".into();
    value["commit_state"] = "IN_DOUBT".into();
    value["state"] = "in_doubt".into();
    value["protection_at_publish"] = "UNCONFIRMED".into();
    assert!(c.validate(value.clone()).is_ok());
    assert!(!terminal(&value, "op_00000000000000000000000000000001"));
}

#[test]
fn administration_uses_the_shared_current_wire_evidence() {
    use storage_contract::client_protocol::{OperationStatus, operation_status};
    let c = request::operation_outcome(OPERATION).contract;
    let mut content: Value = json_codec::from_str(include_str!(
        "../files/fixtures/damaged-published-save-operation.json"
    ))
    .unwrap();
    // Project the captured diagnostic into the outcome-only wire contract.
    content.as_object_mut().unwrap().remove("current_health");
    content["operation_id"] = "op_00000000000000000000000000000001".into();
    for valid in [wire_operation(), content] {
        assert!(c.validate(valid.clone()).is_ok());
        assert!(terminal(&valid, "op_00000000000000000000000000000001"));
        assert_eq!(operation_status(&valid), Some(OperationStatus::Published));
        let mut malformed = valid.clone();
        malformed["result"] = Value::Null;
        let mut incomplete = valid.clone();
        incomplete.as_object_mut().unwrap().remove("commit_state");
        let mut contradictory = valid.clone();
        contradictory["state"] = "in_doubt".into();
        let mut extra = valid.clone();
        extra["legacy"] = true.into();
        for invalid in [
            malformed,
            incomplete,
            contradictory,
            extra,
            json!({"operation":valid}),
            json!({"operation_id":"op_00000000000000000000000000000001", "state":"SUCCEEDED"}),
        ] {
            assert!(c.validate(invalid.clone()).is_err());
            assert!(!terminal(&invalid, "op_00000000000000000000000000000001"));
            assert_eq!(operation_status(&invalid), None);
        }
    }
}

#[test]
fn diagnostic_and_outcome_contracts_cannot_be_interchanged() {
    let outcome = wire_operation();
    let mut diagnostic = outcome.clone();
    diagnostic["current_health"] =
        json!({"state":"UNAVAILABLE","checked_at":"2026-10-24T05:33:19Z"});
    let inspect = request::operation(OPERATION).contract;
    let reconcile = request::operation_outcome(OPERATION).contract;
    assert!(inspect.validate(diagnostic.clone()).is_ok());
    assert!(inspect.validate(outcome.clone()).is_err());
    assert!(reconcile.validate(outcome).is_ok());
    assert!(reconcile.validate(diagnostic.clone()).is_err());
    diagnostic["current_health"] = Value::Null;
    assert!(inspect.validate(diagnostic.clone()).is_ok());
    diagnostic["operation_id"] = "unrelated".into();
    assert!(inspect.validate(diagnostic).is_err());
}
#[test]
fn domain_and_preview_contracts_accept_real_shapes_not_generic_receipts() {
    let repository = json!({"repository_id":"r", "name":"Repo", "history_tree_id":"h", "main_branch_id":"b", "primary_slot_id":"P01", "target_copies":1,"name_policy":"portable", "unicode_version":"17","smb_enabled":true,"smb_binding_generation":"1"});
    let c = contract(
        Kind::Repository,
        &[("name", "Repo"), ("slot", "P01"), ("copies", "1")],
    );
    assert!(c.validate(repository.clone()).is_ok());
    let mut missing_binding_state = repository.clone();
    missing_binding_state
        .as_object_mut()
        .unwrap()
        .remove("smb_enabled");
    assert!(c.validate(missing_binding_state).is_err());
    for (key, bad) in [
        ("name", json!("Other")),
        ("primary_slot_id", json!("P02")),
        ("target_copies", json!(2)),
        ("repository_id", json!("")),
        ("history_tree_id", json!("")),
        ("main_branch_id", json!("")),
        ("smb_binding_generation", json!(1)),
        ("unexpected", json!(true)),
    ] {
        let mut wrong = repository.clone();
        wrong[key] = bad;
        assert!(
            c.validate(wrong).is_err(),
            "repository accepted wrong {key}"
        );
    }
    let branch = json!({"branch_id":"b","history_tree_id":"h","name":"Branch","head_commit_id":"new","fork_commit_id":"source","branch_kind":"full","generation":"1","state":"active","pending_file_versions":{},"materialized":true,"inherited":false,"content_state":"LIVE"});
    assert!(
        contract(
            Kind::Branch,
            &[
                ("repository", "r"),
                ("name", "Branch"),
                ("commit", "source")
            ]
        )
        .validate(branch)
        .is_ok()
    );
    let link = json!({"merge_link_id":"m","history_tree_id":"h","source_branch_id":"s","source_commit_id":"sc","target_branch_id":"t","target_commit_id":"tc","author":"admin","operation_id":"op_00000000000000000000000000000001"});
    assert!(
        contract(
            Kind::Link,
            &[
                ("history", "h"),
                ("source_branch", "s"),
                ("source_commit", "sc"),
                ("target_branch", "t"),
                ("target_commit", "tc")
            ]
        )
        .validate(link)
        .is_ok()
    );
    for (action, kind) in [
        (Kind::Purge, "purge"),
        (Kind::Split, "split"),
        (Kind::Save, "historical_save"),
    ] {
        let c = contract(
            action,
            &[
                ("history", "h"),
                ("versions", "v"),
                ("commit", "c"),
                ("name", "saved"),
            ],
        );
        let plan = json!({"plan_id":"p","kind":kind,"principal":"admin","expected_sequence":"4","digest":"digest","input":{"history_tree_id":"h"},"state":"ready"});
        assert!(c.validate(plan.clone()).is_ok());
        let mut wrong = plan.clone();
        wrong["input"]["history_tree_id"] = "other".into();
        assert!(c.validate(wrong).is_err());
        for (key, bad) in [
            ("kind", "unknown"),
            ("principal", "other"),
            ("state", "committed"),
            ("digest", ""),
            ("plan_id", ""),
        ] {
            let mut wrong = plan.clone();
            wrong[key] = bad.into();
            assert!(
                c.validate(wrong).is_err(),
                "{action:?} accepted wrong {key}"
            );
        }
        assert!(c.validate(raw_operation()).is_err());
    }
}

#[test]
fn branch_creation_accepts_wire_projection_without_relaxing_domain_validation() {
    let c = contract(
        Kind::Branch,
        &[
            ("repository", "r"),
            ("name", "Branch"),
            ("commit", "source"),
        ],
    );
    // branch_wire always adds these three fields, including for empty branches.
    let empty = json!({"branch_id":"b","history_tree_id":"h","name":"Branch","head_commit_id":"new","fork_commit_id":"source","branch_kind":"full","generation":"1","state":"active","pending_file_versions":{},"materialized":true,"inherited":false,"content_state":"LIVE"});
    let mut pending = empty.clone();
    pending["pending_file_versions"] = json!({"file":"version"});
    pending["materialized"] = false.into();
    pending["inherited"] = true.into();
    pending["content_state"] = "PENDING".into();
    for valid in [empty, pending] {
        assert!(c.validate(valid.clone()).is_ok());
        for key in ["materialized", "inherited", "content_state"] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(c.validate(missing).is_err());
        }
        for (key, bad) in [
            ("materialized", valid["inherited"].clone()),
            ("inherited", valid["materialized"].clone()),
            ("content_state", "UNKNOWN".into()),
            ("name", "Other".into()),
            ("fork_commit_id", "other".into()),
            ("branch_id", "".into()),
            ("history_tree_id", "".into()),
            ("generation", json!(1)),
            ("unexpected", true.into()),
        ] {
            let mut malformed = valid.clone();
            malformed[key] = bad;
            assert!(c.validate(malformed).is_err());
        }
    }
}

#[test]
fn plan_query_checks_identity_and_shape_without_requiring_ready() {
    let request = request::query(
        QueryKind::Plan,
        &[("id".into(), "p".into())].into_iter().collect(),
    )
    .unwrap();
    let c = request.contract;
    for state in ["ready", "committed", "cancelled", "completed"] {
        let mut value = plan();
        value["state"] = state.into();
        assert!(c.validate(value).is_ok(), "GET plan rejected {state}");
    }
    for (key, bad) in [
        ("plan_id", json!("other")),
        ("digest", json!("")),
        ("input", Value::Null),
        ("input", json!([])),
        ("expected_sequence", json!(4)),
        ("state", json!("unknown")),
    ] {
        let mut value = plan();
        value[key] = bad;
        assert!(c.validate(value).is_err(), "GET plan accepted wrong {key}");
    }
    for key in [
        "plan_id",
        "kind",
        "principal",
        "expected_sequence",
        "digest",
        "input",
        "state",
    ] {
        let mut value = plan();
        value.as_object_mut().unwrap().remove(key);
        assert!(c.validate(value).is_err(), "GET plan accepted absent {key}");
    }
}

#[test]
fn merge_link_response_must_match_every_captured_endpoint_identity() {
    let c = contract(
        Kind::Link,
        &[
            ("history", "h"),
            ("source_branch", "s"),
            ("source_commit", "sc"),
            ("target_branch", "t"),
            ("target_commit", "tc"),
        ],
    );
    let link = json!({"merge_link_id":"m","history_tree_id":"h","source_branch_id":"s","source_commit_id":"sc","target_branch_id":"t","target_commit_id":"tc","author":"admin","operation_id":OPERATION});
    assert!(c.validate(link.clone()).is_ok());
    for key in [
        "history_tree_id",
        "source_branch_id",
        "source_commit_id",
        "target_branch_id",
        "target_commit_id",
        "operation_id",
    ] {
        let mut value = link.clone();
        value[key] = if key == "operation_id" {
            "op_00000000000000000000000000000002"
        } else {
            "other"
        }
        .into();
        assert!(
            c.validate(value).is_err(),
            "merge-link accepted wrong {key}"
        );
    }
}

#[test]
fn malformed_success_keeps_the_existing_unknown_mutation_classification() {
    use std::error::Error as _;
    let error = plan_contract(true).validate(Value::Null).unwrap_err();
    assert_eq!(
        error.code,
        storage_contract::ErrorCode::InvalidAdminResponse
    );
    assert_eq!(error.status, 200);
    assert!(error.source().is_none());
    assert!(!error.retryable);
    assert!(error.request_id.is_none());
    assert!(storage_contract::client_protocol::mutation_outcome_unknown(
        error.status
    ));
}
