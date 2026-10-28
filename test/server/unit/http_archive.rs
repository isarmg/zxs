use super::*;
use json_codec::json;
fn decode(body: Value) -> Result<ArchiveBranch, ApiError> {
    let Value::Object(body) = crate::with_id(body, "branch_id", "branch".into())? else {
        unreachable!()
    };
    parse(body)
}
#[test]
fn archive_ingress_preserves_field_unknown_and_operation_error_order() {
    let operation = storage_contract::OperationId::generate();
    let parsed = decode(json!({"operation_id":operation})).unwrap();
    assert_eq!(parsed.branch_id, "branch");
    assert_eq!(parsed.operation_id, operation);
    assert_eq!(
        decode(json!({"branch_id":"other","operation_id":operation}))
            .err()
            .unwrap()
            .message,
        "path and body identity disagree"
    );
    for value in [json!(null), json!(false), json!(17), json!(""), json!([])] {
        assert_eq!(
            decode(json!({"operation_id":value,"unexpected":true}))
                .err()
                .unwrap()
                .message,
            "missing string field operation_id"
        );
    }
    assert_eq!(
        decode(json!({"unexpected":true})).err().unwrap().message,
        "missing string field operation_id"
    );
    assert_eq!(
        decode(json!({"operation_id":"bad","unexpected":true}))
            .err()
            .unwrap()
            .message,
        "unknown field unexpected"
    );
    let invalid = decode(json!({"operation_id":"bad"})).err().unwrap();
    crate::tests::assert_contract_input(
        &invalid,
        &"bad".parse::<storage_contract::OperationId>().unwrap_err(),
    );
    assert!(storage_contract::strict_json(br#"{"operation_id":"a","operation_id":"b"}"#).is_err());
    assert!(decode(json!([])).is_err());
}
