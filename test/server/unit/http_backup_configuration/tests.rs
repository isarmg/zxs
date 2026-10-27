use super::*;
use json_codec::json;

fn request() -> Value {
    json!({"repository_id":"repo","target_copies":2,"backup_prefixes":{"B01":"backup"},"expected_generation":"0","expected_scope_id":"scope","operation_id":"op_00000000000000000000000000000000"})
}

#[test]
fn root_and_full_width_generation_are_typed_without_changing_decimal_contract() {
    let parsed = parse(request()).unwrap();
    assert!(parsed.path.is_empty());
    assert_eq!(parsed.expected_generation, 0);
    assert_eq!(parsed.backup_prefixes["B01"], "backup");
    let mut body = request();
    body["expected_generation"] = u64::MAX.to_string().into();
    body["target_copies"] = u32::MAX.into();
    let parsed = parse(body).unwrap();
    assert_eq!(parsed.expected_generation, u64::MAX);
    assert_eq!(parsed.target_copies, u32::MAX);
    // Copy-count feasibility is a current topology decision, not an HTTP assumption.
    let mut body = request();
    body["target_copies"] = 0.into();
    assert_eq!(parse(body).unwrap().target_copies, 0);
}

#[test]
fn explicit_non_string_paths_never_mean_the_root() {
    for value in [Value::Null, json!(123), json!([]), json!({}), json!(false)] {
        let mut body = request();
        body["path"] = value;
        assert!(parse(body).is_err());
    }
    for path in ["", "folder"] {
        let mut body = request();
        body["path"] = path.into();
        assert_eq!(parse(body).unwrap().path, path);
    }
    assert_eq!(parse(request()).unwrap().path, "");
}

#[test]
fn wire_fields_reject_invalid_shapes_and_noncanonical_generation_before_backend() {
    for (key, value) in [
        ("path", json!(null)),
        ("path", json!("../escape")),
        ("target_copies", json!(-1)),
        ("target_copies", json!(1.5)),
        ("target_copies", json!(u64::from(u32::MAX) + 1)),
        ("target_copies", json!("2")),
        ("backup_prefixes", json!([])),
        ("backup_prefixes", json!({"B01":null})),
        ("expected_scope_id", json!("")),
        ("expected_generation", json!(0)),
        ("expected_generation", json!("00")),
        ("expected_generation", json!("+0")),
        ("expected_generation", json!(" 0")),
        ("expected_generation", json!("0.0")),
        ("expected_generation", json!("18446744073709551616")),
        ("operation_id", json!(null)),
        ("operation_id", json!("")),
        ("extra", json!(true)),
    ] {
        let mut body = request();
        body[key] = value;
        let error = parse(body).unwrap_err();
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST, "{key}");
        assert_eq!(
            error.code,
            storage_contract::ErrorCode::InvalidRequest,
            "{key}"
        );
    }
    for key in [
        "repository_id",
        "target_copies",
        "backup_prefixes",
        "expected_generation",
        "expected_scope_id",
        "operation_id",
    ] {
        let mut body = request();
        body.as_object_mut().unwrap().remove(key);
        assert!(parse(body).is_err(), "{key}");
    }
    let mut body = request();
    body["operation_id"] = "invalid".into();
    assert_eq!(
        parse(body).unwrap_err().code,
        storage_contract::ErrorCode::InvalidId
    );
}

#[test]
fn read_wire_keeps_nulls_empty_collections_numeric_counts_and_decimal_generations() {
    let snapshot = storage_contract::BackupConfigurationSnapshot {
        scope: storage_contract::BackupScope {
            id: "repo".into(),
            kind: storage_contract::NamespaceEntryKind::Directory,
            path: String::new(),
        },
        policy_id: None,
        effective_policy_id: Some("inherited".into()),
        generation: 0,
        effective_generation: u64::MAX,
        inherited: true,
        target_copies: 2,
        backup_prefixes: BTreeMap::new(),
        route_slot_ids: vec![],
        available_route_slot_ids: vec!["P01".into()],
        backup_routes: vec![vec!["P01".into()]],
        backup_slot_ids: std::collections::BTreeSet::new(),
        max_copies: 1,
        file_count: u64::MAX,
        backup_paths: BTreeMap::new(),
    };
    let response = crate::backend_backup_configuration::BackupConfigurationResponse::from(snapshot);
    let bytes = json_codec::to_vec(&response).unwrap();
    let wire: Value = storage_contract::strict_json(&bytes).unwrap();
    assert!(wire.get("can_manage").is_none());
    assert_eq!(
        wire,
        json!({"scope":{"id":"repo","kind":"directory","path":""},"policy_id":null,"effective_policy_id":"inherited","generation":"0","effective_generation":u64::MAX.to_string(),"inherited":true,"target_copies":2,"backup_prefixes":{},"route_slot_ids":[],"available_route_slot_ids":["P01"],"backup_routes":[["P01"]],"backup_slot_ids":[],"max_copies":1,"file_count":u64::MAX.to_string(),"backup_paths":{}})
    );
}
