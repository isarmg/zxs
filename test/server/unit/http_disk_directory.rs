use super::*;
use json_codec::json;

fn decode(body: Value) -> Result<MutateDiskDirectory, ApiError> {
    let Value::Object(body) = crate::with_id(body, "slot_id", "P01".into())? else {
        unreachable!()
    };
    parse(body)
}

fn source() -> Value {
    json!({
        "repository_id": "repo_00000000000000000000000000000001",
        "directory_id": "entry_00000000000000000000000000000002",
        "directory_branch_id": "dbr_00000000000000000000000000000003",
        "working_tree_id": "dtree_00000000000000000000000000000004",
        "head_tree_id": "dtree_00000000000000000000000000000005",
        "index_generation": "9007199254740993",
        "namespace_head": "commit_00000000000000000000000000000006",
    })
}

#[test]
fn directory_commands_parse_optional_root_and_keep_static_error_priority() {
    let operation = storage_contract::OperationId::generate().unwrap();
    for path in [None, Some(""), Some("Café/Child")] {
        let mut body = json!({"name":"Side","operation_id":operation,"source":source()});
        if let Some(path) = path {
            body["path"] = json!(path);
        }
        let command = decode(body).unwrap();
        assert_eq!(
            command.path.as_ref().map(name_policy::LookupPath::key),
            path.filter(|path| !path.is_empty()).map(|_| "café/child")
        );
        assert_eq!(command.operation_id, operation);
        assert_eq!(command.name.key(), "side");
        let DiskDirectoryAction::ReviewedCreate(source) = command.action else {
            panic!("HTTP creation must carry a reviewed source");
        };
        assert_eq!(source.index_generation, 9_007_199_254_740_993);
    }
    for body in [
        json!({"path":"","operation_id":operation}),
        json!({"path":"","name":"Side","operation_id":"bad"}),
        json!({"path":"","name":"x".repeat(129),"operation_id":operation}),
        json!({"path":"","name":null,"operation_id":operation}),
        json!({"path":false,"name":"Side","operation_id":operation}),
        json!({"path":"Cafe\u{301}","name":"Side","operation_id":operation}),
        json!({"path":"../x","name":"Side","operation_id":operation}),
        json!({"name":"Side","operation_id":operation,"unknown":0}),
    ] {
        assert_eq!(
            decode(body).err().unwrap().status,
            http::StatusCode::BAD_REQUEST
        );
    }
    let error = decode(json!({"name":"x".repeat(129),"operation_id":"bad"}))
        .err()
        .unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::MetadataTooLong);
    assert!(std::error::Error::source(&error).is_some());
    let error = decode(json!({"name":"Side","operation_id":"bad"}))
        .err()
        .unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn directory_create_requires_complete_exact_source_without_wire_fallback() {
    let operation = storage_contract::OperationId::generate().unwrap();
    let body = json!({"path":"child","name":"Side","operation_id":operation,"source":source()});
    let mut missing = body.clone();
    missing.as_object_mut().unwrap().remove("source");
    assert!(decode(missing).is_err());
    for key in source().as_object().unwrap().keys() {
        let mut missing = body.clone();
        missing["source"].as_object_mut().unwrap().remove(key);
        assert!(decode(missing).is_err(), "{key}");
    }
    for generation in [
        json!(0),
        json!(1),
        json!("0"),
        json!("01"),
        json!("+1"),
        json!("18446744073709551616"),
        json!(null),
    ] {
        let mut changed = body.clone();
        changed["source"]["index_generation"] = generation;
        assert!(decode(changed).is_err());
    }
    for key in [
        "repository_id",
        "directory_id",
        "directory_branch_id",
        "working_tree_id",
        "head_tree_id",
        "namespace_head",
    ] {
        let mut changed = body.clone();
        changed["source"][key] = json!("");
        assert!(decode(changed).is_err(), "{key}");
    }
    let mut changed = body.clone();
    changed["source"]["unknown"] = json!(true);
    assert!(decode(changed).is_err());
    let mut root = body;
    root["source"]["directory_id"] = root["source"]["repository_id"].clone();
    root["source"]["index_generation"] = json!(u64::MAX.to_string());
    assert!(decode(root).is_ok());
}
