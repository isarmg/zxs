use super::*;
use json_codec::json;
fn decode(body: Value) -> Result<CreateFileBranch, ApiError> {
    let Value::Object(body) = crate::with_id(body, "file_id", "file".into())? else {
        unreachable!("fixture binds an object")
    };
    parse(body)
}
#[test]
fn file_branch_source_variants_keep_absence_distinct_from_invalid_explicit_nodes() {
    let operation = storage_contract::OperationId::generate();
    let version =
        decode(json!({"source_version_id":"version","name":"Café","operation_id":operation}))
            .unwrap();
    assert_eq!(version.file_id, "file");
    assert_eq!(version.source_version_id, "version");
    assert_eq!(version.source_node_id, None);
    assert_eq!(version.name.key(), "café");
    assert_eq!(version.operation_id, operation);
    let node=decode(json!({"source_version_id":"version","source_node_id":"node","name":"Café","operation_id":operation})).unwrap();
    assert_eq!(node.source_node_id.as_deref(), Some("node"));
    for node in [
        Value::Null,
        json!(""),
        json!(0),
        json!(false),
        json!([]),
        json!({}),
    ] {
        let error=decode(json!({"source_version_id":"version","source_node_id":node,"name":"Side","operation_id":operation})).err().unwrap();
        assert_eq!(error.message, "source_node_id must be a nonempty node ID");
    }
    for body in [
        json!({"name":"Side","operation_id":operation}),
        json!({"source_version_id":null,"name":"Side","operation_id":operation}),
        json!({"source_version_id":"version","name":null,"operation_id":operation}),
        json!({"source_version_id":"version","name":"Cafe\u{301}","operation_id":operation}),
        json!({"source_version_id":"version","name":"x".repeat(129),"operation_id":operation}),
        json!({"source_version_id":"version","name":"Side","operation_id":operation,"unknown":0}),
        json!({"source_version_id":"version","name":"Side","operation_id":operation,"file_id":"other"}),
    ] {
        assert_eq!(
            decode(body).err().unwrap().status,
            http::StatusCode::BAD_REQUEST
        );
    }
    let error =
        decode(json!({"source_version_id":"version","name":"x".repeat(129),"operation_id":"bad"}))
            .err()
            .unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
    assert!(std::error::Error::source(&error).is_some());
}
