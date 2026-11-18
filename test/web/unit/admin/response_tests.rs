use super::super::purge::fixtures::{self, HISTORY, PLAN, VERSION, plan};
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
    )
    .unwrap()
    .contract
}
fn plan_contract(commit: bool) -> Contract {
    let validated = Contract::Plan { id: PLAN.into() }.validate(plan()).unwrap();
    let Action::Plan(plan) = validated.action else {
        panic!("expected plan")
    };
    request::plan_action(Some(&plan), commit, true, OPERATION)
        .unwrap()
        .contract
}
fn raw_operation(commit: bool) -> Value {
    fixtures::receipt(commit, OPERATION)
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
fn terminal(value: &Value, operation: &str) -> bool {
    matches!(
        request::operation_outcome(operation)
            .contract
            .validate(value.clone()),
        Ok(ValidatedAdminResponse {
            action: Action::Outcome {
                status: OperationStatus::Published
                    | OperationStatus::Degraded
                    | OperationStatus::Rejected,
                ..
            },
            ..
        })
    )
}
#[test]
fn operation_contracts_reject_missing_wrong_and_nonterminal_receipts() {
    for commit in [true, false] {
        let c = plan_contract(commit);
        assert!(c.validate(raw_operation(commit)).is_ok());
        for value in [Value::Null, json!({}), wire_operation()] {
            assert!(c.validate(value).is_err());
        }
        for (key, bad) in [("operation_id", "unrelated"), ("commit_state", "IN_DOUBT")] {
            let mut value = raw_operation(commit);
            value[key] = bad.into();
            assert!(c.validate(value).is_err());
        }
    }
    for kind in [Kind::Policy, Kind::Rule] {
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
        assert!(c.validate(raw_operation(false)).is_err());
        let mut value = wire_operation();
        value["operation_id"] = "unrelated".into();
        assert!(c.validate(value).is_err());
    }
}
#[test]
fn inspection_must_match_original_operation_and_terminal_state() {
    let c = request::operation_outcome(OPERATION).contract;
    assert!(c.validate(wire_operation()).is_ok());
    assert!(c.validate(raw_operation(false)).is_err());
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
fn plan_query_checks_identity_and_shape_without_requiring_ready() {
    let request = request::query(
        QueryKind::Plan,
        &[("id".into(), PLAN.into())].into_iter().collect(),
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
fn malformed_success_preserves_its_decode_source() {
    use std::error::Error as _;
    let error = plan_contract(true).validate(Value::Null).unwrap_err();
    assert!(matches!(error, ResponseError::Decode(_)));
    assert!(error.source().is_some());
}

#[test]
fn only_a_canonical_explicit_file_version_purge_can_be_reviewed() {
    let query = Contract::Plan { id: PLAN.into() };
    for kind in ["historical_save", "split", "other"] {
        let mut value = plan();
        value["kind"] = kind.into();
        assert!(query.validate(value).is_err());
    }
    let bad_inputs = [
        json!({"history_tree_id": HISTORY, "cut_commit_id": "commit_00000000000000000000000000000001"}),
        json!({"history_tree_id": HISTORY, "version_ids": []}),
        json!({"history_tree_id": HISTORY, "version_ids": [VERSION, VERSION]}),
        json!({"history_tree_id": HISTORY, "version_ids": ["file_00000000000000000000000000000001"]}),
        json!({"history_tree_id": "not a history", "version_ids": [VERSION]}),
        json!({"history_tree_id": HISTORY, "version_ids": [VERSION], "source_commit_id": "extra"}),
    ];
    for input in bad_inputs {
        let mut value = plan();
        value["digest"] = storage_contract::object_hash_value("plan", &input).unwrap().into();
        value["input"] = input;
        assert!(query.validate(value).is_err());
    }
    let mut changed = plan();
    changed["input"]["version_ids"][0] = "ver_00000000000000000000000000000002".into();
    assert!(query.validate(changed).is_err());
    let Action::Plan(review) = query.validate(plan()).unwrap().action else {
        panic!("expected an ordinary FILE purge review");
    };
    assert!(review.belongs_to(Some(&"admin".parse().unwrap())));
    assert!(!review.belongs_to(None));
    assert!(!review.belongs_to(Some(&"different".parse().unwrap())));
}

#[test]
fn plan_receipt_must_match_reviewed_action_plan_and_exact_file_version_set() {
    for commit in [true, false] {
        let contract = plan_contract(commit);
        assert!(contract.validate(raw_operation(!commit)).is_err());
        let mut wrong_plan = raw_operation(commit);
        wrong_plan["result"]["plan_id"] = "plan_00000000000000000000000000000002".into();
        assert!(contract.validate(wrong_plan).is_err());
        let mut changed = raw_operation(commit);
        if commit {
            changed["result"]["purged_versions"] = json!([]);
        } else {
            changed["result"]["cancelled_plan_hash"] = "a".repeat(64).into();
        }
        assert!(contract.validate(changed).is_err());
    }
}
