use super::*;
use json_codec::json;
fn body() -> Value {
    json!({"repository_id":"repo","path":"docs","primary_slot_id":"P01",
        "expected_scope_id":"directory","expected_rule_id":null,"expected_generation":"0",
        "operation_id":"op_00000000000000000000000000000001"})
}
#[test]
fn placement_configuration_requires_identity_and_string_generation_baselines() {
    let request = parse(body()).unwrap();
    assert_eq!(request.expected_rule_id, None);
    assert_eq!(request.expected_generation, 0);
    for field in [
        "repository_id",
        "path",
        "primary_slot_id",
        "expected_scope_id",
        "expected_rule_id",
        "expected_generation",
        "operation_id",
    ] {
        let mut invalid = body();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(parse(invalid).is_err(), "accepted missing {field}");
    }
    for (field, invalid) in [
        ("expected_scope_id", json!("")),
        ("expected_rule_id", json!("")),
        ("expected_generation", json!(0)),
        ("path", json!("../outside")),
        ("unexpected", json!(true)),
    ] {
        let mut value = body();
        value[field] = invalid;
        assert!(parse(value).is_err(), "accepted invalid {field}");
    }
}

fn placement_fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/tools/contract-check/fixtures/placement-configuration")
        .join(name);
    storage_contract::strict_json(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn placement_configuration_real_response_dto_matches_schema_fixtures() {
    use storage_contract::{
        PlacementConfigurationSnapshot, PlacementScope, PrimaryPlacementBlock,
        PrimaryPlacementTarget,
    };
    let repository_id = "repo_00000000000000000000000000000001";
    for (name, scope, rule_id, generation, inherited, required_copies, targets) in [
        (
            "read-root.json",
            PlacementScope {
                id: repository_id.into(),
                path: String::new(),
            },
            None,
            0,
            true,
            1,
            vec![("P01", "/srv/zxs/primary", None)],
        ),
        (
            "read-directory.json",
            PlacementScope {
                id: "entry_00000000000000000000000000000002".into(),
                path: "docs".into(),
            },
            Some("rule_00000000000000000000000000000003".into()),
            u64::MAX,
            false,
            3,
            vec![
                ("P01", "/srv/zxs/primary", None),
                (
                    "P02",
                    "/srv/zxs/secondary",
                    Some(PrimaryPlacementBlock::InsufficientCopies),
                ),
                (
                    "P03",
                    "/srv/zxs/archive",
                    Some(PrimaryPlacementBlock::MissingBackupPrefixes),
                ),
            ],
        ),
    ] {
        let snapshot = PlacementConfigurationSnapshot {
            repository_id: repository_id.into(),
            repository_name: "工作资料".into(),
            scope,
            rule_id,
            generation,
            primary_slot_id: "P01".into(),
            inherited,
            required_copies,
            targets: targets
                .into_iter()
                .map(|(slot, root, unavailable_reason)| PrimaryPlacementTarget {
                    slot_id: slot.into(),
                    root: root.into(),
                    unavailable_reason,
                })
                .collect(),
        };
        let response =
            crate::backend_placement_configuration::PlacementConfigurationResponse::from(snapshot);
        let bytes = json_codec::to_vec(&response).unwrap();
        let wire = storage_contract::strict_json(&bytes).unwrap();
        assert_eq!(wire, placement_fixture(name), "{name}");
    }
}

#[test]
fn placement_configuration_real_request_dto_matches_schema_fixtures() {
    let repository_id = "repo_00000000000000000000000000000001";
    for (name, path, scope, rule, generation) in [
        ("create.json", "", repository_id, None, 0),
        (
            "update.json",
            "docs",
            "entry_00000000000000000000000000000002",
            Some("rule_00000000000000000000000000000003"),
            u64::MAX,
        ),
    ] {
        let body = placement_fixture(name);
        let request =
            parse(crate::with_id(body.clone(), "repository_id", repository_id.into()).unwrap())
                .unwrap();
        assert_eq!(request.repository_id, repository_id);
        assert_eq!(request.path, path);
        assert_eq!(request.primary_slot_id, "P01");
        assert_eq!(request.expected_scope_id, scope);
        assert_eq!(request.expected_rule_id.as_deref(), rule);
        assert_eq!(request.expected_generation, generation);
        assert_eq!(
            json_codec::to_value(request.operation_id).unwrap(),
            body["operation_id"]
        );
        let mut explicit = body.clone();
        explicit["repository_id"] = repository_id.into();
        assert!(
            parse(crate::with_id(explicit, "repository_id", repository_id.into()).unwrap()).is_ok()
        );
        let mut mismatched = body;
        mismatched["repository_id"] = "repo_00000000000000000000000000000005".into();
        let error = crate::with_id(mismatched, "repository_id", repository_id.into()).unwrap_err();
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
    }
}

#[test]
fn placement_configuration_real_request_dto_rejects_shared_schema_cases() {
    let repository_id = "repo_00000000000000000000000000000001";
    let valid = placement_fixture("create.json");
    let invalid_cases = placement_fixture("invalid-write-fields.json");
    let invalid_cases = invalid_cases.as_array().unwrap();
    assert_eq!(invalid_cases.len(), 25);
    for case in invalid_cases {
        let field = case["field"].as_str().unwrap();
        let mut invalid = valid.clone();
        invalid[field] = case["value"].clone();
        let error = parse(crate::with_id(invalid, "repository_id", repository_id.into()).unwrap())
            .unwrap_err();
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST, "{case}");
        assert_eq!(
            error.code,
            storage_contract::ErrorCode::InvalidRequest,
            "{case}"
        );
    }
    for field in valid.as_object().unwrap().keys() {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            parse(crate::with_id(missing, "repository_id", repository_id.into()).unwrap()).is_err(),
            "missing {field}"
        );
    }
}
