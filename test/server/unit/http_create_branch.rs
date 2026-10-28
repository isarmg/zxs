use super::*;
fn body() -> Map<String, Value> {
    json_codec::json!({"repository_id":"repository","source_commit_id":"source","name":"Café","operation_id":"op_00000000000000000000000000000000"}).as_object().unwrap().clone()
}
#[test]
fn creation_decodes_name_key_and_operation_once() {
    let request = parse(body()).unwrap();
    assert_eq!(request.repository_id, "repository");
    assert_eq!(request.source_commit_id, "source");
    assert_eq!(request.name.as_str(), "Café");
    assert_eq!(request.name.key(), "café");
}
#[test]
fn creation_preserves_contract_error_details_and_sources() {
    let mut invalid_name = body();
    invalid_name.insert("name".into(), "x".repeat(129).into());
    crate::tests::assert_contract_input(
        &parse(invalid_name).err().unwrap(),
        &storage_contract::BranchName::parse(&"x".repeat(129)).unwrap_err(),
    );
    let mut invalid_operation = body();
    invalid_operation.insert("operation_id".into(), "bad".into());
    crate::tests::assert_contract_input(
        &parse(invalid_operation).err().unwrap(),
        &"bad".parse::<storage_contract::OperationId>().unwrap_err(),
    );
}
#[test]
fn creation_preserves_static_failure_order() {
    let mut input = body();
    input.insert("extra".into(), Value::Null);
    input.remove("source_commit_id");
    assert_eq!(
        parse(input).err().unwrap().message,
        "missing string field source_commit_id"
    );
    let mut input = body();
    input.insert("name".into(), json_codec::json!("../bad"));
    input.insert("operation_id".into(), json_codec::json!("bad"));
    let error = parse(input).err().unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidId);
    let mut input = body();
    input.insert("name".into(), json_codec::json!("Cafe\u{301}"));
    assert!(parse(input).is_err());
}
