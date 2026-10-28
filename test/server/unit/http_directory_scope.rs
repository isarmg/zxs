use super::*;
use json_codec::json;
fn decode(body: Value) -> Result<InitializeDirectoryScope, ApiError> {
    let Value::Object(body) = crate::with_id(body, "repository_id", "repository".into())? else {
        unreachable!()
    };
    parse(body)
}
#[test]
fn initialization_parses_root_and_canonical_path_once() {
    let operation = storage_contract::OperationId::generate();
    let root = decode(json!({"path":"","operation_id":operation})).unwrap();
    assert!(root.path.is_none());
    assert_eq!(root.repository_id, "repository");
    assert_eq!(root.operation_id, operation);
    let child = decode(json!({"path":"Café/Child","operation_id":operation})).unwrap();
    let path = child.path.unwrap();
    assert_eq!(path.as_str(), "Café/Child");
    assert_eq!(path.key(), name_policy::path_key("Café/Child"));
    for path in ["../x", "/x", "x/", "Cafe\u{301}", "CON"] {
        assert!(
            decode(json!({"path":path,"operation_id":operation})).is_err(),
            "{path}"
        );
    }
    for path in [Value::Null, json!(3), json!(true)] {
        assert_eq!(
            decode(json!({"path":path,"operation_id":operation}))
                .err()
                .unwrap()
                .message,
            "path must be a string"
        );
    }
    for body in [
        json!({"operation_id":operation}),
        json!({"path":""}),
        json!({"path":"","operation_id":"bad"}),
        json!({"path":"","operation_id":operation,"extra":1}),
        json!({"repository_id":"other","path":"","operation_id":operation}),
    ] {
        assert!(decode(body).is_err());
    }
}

#[test]
fn initialization_preserves_contract_errors_and_path_before_operation_priority() {
    let invalid = decode(json!({"path":"","operation_id":"bad"}))
        .err()
        .unwrap();
    crate::tests::assert_contract_input(
        &invalid,
        &"bad".parse::<storage_contract::OperationId>().unwrap_err(),
    );
    let invalid = decode(json!({"path":"../bad","operation_id":"bad"}))
        .err()
        .unwrap();
    assert_eq!(invalid.code, storage_contract::ErrorCode::InvalidRequest);
    let path_error: storage_contract::Error = name_policy::PortablePath::parse("../bad")
        .unwrap_err()
        .into();
    assert_eq!(invalid.message, path_error.to_string());
    assert!(std::error::Error::source(&invalid).is_some());
    assert_eq!(
        decode(json!({"path":null,"operation_id":"bad","unexpected":true}))
            .err()
            .unwrap()
            .message,
        "unknown field unexpected"
    );
}
