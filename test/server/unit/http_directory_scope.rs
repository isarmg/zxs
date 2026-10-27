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
