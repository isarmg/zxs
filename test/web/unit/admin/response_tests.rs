use super::*;
use json_codec::json;
fn contract(path: &str, body: Value) -> Contract {
    Contract::new(
        "POST",
        path,
        Some(&body),
        "op_00000000000000000000000000000001",
        "admin",
    )
}
fn raw_operation() -> Value {
    json!({"operation_id":"op_00000000000000000000000000000001", "kind":"plan.cancel", "commit_id":null,"history_tree_id":null,"branch_id":null,"commit_state":"PUBLISHED","protection_at_publish":"NOT_APPLICABLE","current_health":"UNKNOWN","request_digest":"digest","sequence":"4","result":{}})
}
fn wire_operation() -> Value {
    let mut value: Value = json_codec::from_str(include_str!(
        "../files/fixtures/namespace-batch-operation.json"
    ))
    .unwrap();
    value["operation_id"] = "op_00000000000000000000000000000001".into();
    value
}
#[test]
fn operation_contracts_reject_missing_wrong_and_nonterminal_receipts() {
    for path in ["/api/plans/p/commit", "/api/plans/p/cancel"] {
        let c = contract(path, Value::Null);
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
    for path in [
        "/api/repositories/r/smb-binding",
        "/api/policies/p",
        "/api/primary-placement-rules/r",
    ] {
        let c = contract(path, Value::Null);
        assert!(c.validate(wire_operation()).is_ok());
        assert!(c.validate(raw_operation()).is_err());
        let mut value = wire_operation();
        value["operation_id"] = "unrelated".into();
        assert!(c.validate(value).is_err());
    }
}
#[test]
fn inspection_must_match_original_operation_and_terminal_state() {
    let c = Contract::new(
        "GET",
        "/api/operations/op_00000000000000000000000000000001",
        None,
        "",
        "admin",
    );
    assert!(c.validate(wire_operation()).is_ok());
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
    let c = Contract::new(
        "GET",
        "/api/operations/op_00000000000000000000000000000001",
        None,
        "",
        "admin",
    );
    let mut content: Value = json_codec::from_str(include_str!(
        "../files/fixtures/damaged-published-save-operation.json"
    ))
    .unwrap();
    content["operation_id"] = "op_00000000000000000000000000000001".into();
    for valid in [wire_operation(), content] {
        assert!(c.validate(valid.clone()).is_ok());
        assert!(terminal(&valid, "op_00000000000000000000000000000001"));
        assert_eq!(operation_status(&valid), Some(OperationStatus::Published));
        let mut malformed = valid.clone();
        malformed["result"] = Value::Null;
        let mut incomplete = valid.clone();
        incomplete.as_object_mut().unwrap().remove("current_health");
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
fn domain_and_preview_contracts_accept_real_shapes_not_generic_receipts() {
    let repository = json!({"repository_id":"r", "name":"Repo", "history_tree_id":"h", "main_branch_id":"b", "primary_slot_id":"P01", "target_copies":1,"name_policy":"portable", "unicode_version":"17","smb_enabled":true,"smb_binding_generation":"1"});
    let c = contract(
        "/api/repositories",
        json!({"name":"Repo","primary_slot_id":"P01","target_copies":1}),
    );
    assert!(c.validate(repository.clone()).is_ok());
    let mut missing_binding_state = repository.clone();
    missing_binding_state
        .as_object_mut()
        .unwrap()
        .remove("smb_enabled");
    assert!(c.validate(missing_binding_state).is_err());
    let mut wrong = repository;
    wrong["name"] = "Other".into();
    assert!(c.validate(wrong).is_err());
    let branch = json!({"branch_id":"b","history_tree_id":"h","name":"Branch","head_commit_id":"new","fork_commit_id":"source","branch_kind":"full","generation":"1","state":"active","pending_file_versions":{},"materialized":true,"inherited":false,"content_state":"LIVE"});
    assert!(
        contract(
            "/api/repositories/r/branches",
            json!({"name":"Branch","source_commit_id":"source"})
        )
        .validate(branch)
        .is_ok()
    );
    let link = json!({"merge_link_id":"m","history_tree_id":"h","source_branch_id":"s","source_commit_id":"sc","target_branch_id":"t","target_commit_id":"tc","author":"admin","operation_id":"op_00000000000000000000000000000001"});
    assert!(
        contract("/api/history-trees/h/merge-links", link.clone())
            .validate(link)
            .is_ok()
    );
    for (suffix, kind) in [
        ("purge-plans", "purge"),
        ("split-plans", "split"),
        ("historical-save-plans", "historical_save"),
    ] {
        let c = contract(&format!("/api/history-trees/h/{suffix}"), Value::Null);
        let plan = json!({"plan_id":"p","kind":kind,"principal":"admin","expected_sequence":"4","digest":"digest","input":{"history_tree_id":"h"},"state":"ready"});
        assert!(c.validate(plan.clone()).is_ok());
        let mut wrong = plan;
        wrong["input"]["history_tree_id"] = "other".into();
        assert!(c.validate(wrong).is_err());
        assert!(c.validate(raw_operation()).is_err());
    }
}

#[test]
fn branch_creation_accepts_wire_projection_without_relaxing_domain_validation() {
    let c = contract(
        "/api/repositories/r/branches",
        json!({"name":"Branch","source_commit_id":"source"}),
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
fn retired_account_and_acl_writes_have_no_success_contract() {
    for path in ["/api/admin/accounts", "/api/repositories/r/grants"] {
        for value in [json!({"updated":true}), raw_operation(), wire_operation()] {
            assert!(contract(path, Value::Null).validate(value).is_err());
        }
    }
}
