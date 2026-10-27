use super::*;
use json_codec::json;
fn decode(body: Value) -> Result<RenameFileBranch, ApiError> {
    let body = crate::with_id(body, "file_id", "file".into())?;
    let Value::Object(body) = crate::with_id(body, "branch_id", "branch".into())? else {
        unreachable!("fixture binds an object")
    };
    parse(body)
}
#[test]
fn branch_rename_parses_once_and_preserves_operation_before_name_error_priority() {
    let operation = storage_contract::OperationId::generate();
    let parsed = decode(json!({"name":"Café","operation_id":operation})).unwrap();
    assert_eq!(parsed.file_id, "file");
    assert_eq!(parsed.branch_id, "branch");
    assert_eq!(parsed.name.as_str(), "Café");
    assert_eq!(parsed.name.key(), "café");
    assert_eq!(parsed.operation_id, operation);
    let boundary = format!("{}ab", "文".repeat(42));
    assert_eq!(boundary.len(), 128);
    assert_eq!(
        decode(json!({"name":boundary,"operation_id":operation}))
            .unwrap()
            .name
            .as_str(),
        boundary
    );

    for invalid in [
        "x".repeat(129),
        "文".repeat(43),
        "bad\nname".into(),
        "Cafe\u{301}".into(),
    ] {
        let error = decode(json!({"name":invalid,"operation_id":operation}))
            .err()
            .unwrap();
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
        assert!(std::error::Error::source(&error).is_some());
    }
    for body in [
        json!({"operation_id":operation}),
        json!({"name":null,"operation_id":operation}),
        json!({"name":"","operation_id":operation}),
        json!({"name":"Side","operation_id":operation,"unknown":0}),
        json!({"name":"Side","operation_id":operation,"file_id":"other"}),
        json!({"name":"Side","operation_id":operation,"branch_id":"other"}),
    ] {
        assert_eq!(
            decode(body).err().unwrap().status,
            http::StatusCode::BAD_REQUEST
        );
    }
    let error = decode(json!({"name":"x".repeat(129),"operation_id":"bad"}))
        .err()
        .unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
    assert!(std::error::Error::source(&error).is_some());
}
