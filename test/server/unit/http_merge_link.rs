use super::*;
fn body() -> Map<String, Value> {
    json_codec::json!({"history_tree_id":"history","source_branch_id":"source_branch","source_commit_id":"source_commit","target_branch_id":"target_branch","target_commit_id":"target_commit","operation_id":"op_00000000000000000000000000000000"}).as_object().unwrap().clone()
}
#[test]
fn relation_body_preserves_field_order_and_operation_source() {
    let request = parse(body()).unwrap();
    assert_eq!(request.source_branch_id, "source_branch");
    assert_eq!(request.target_commit_id, "target_commit");
    let mut missing = body();
    missing.remove("source_commit_id");
    missing.insert("extra".into(), Value::Null);
    assert_eq!(
        parse(missing).err().unwrap().message,
        "missing string field source_commit_id"
    );
    let mut extra = body();
    extra.insert("extra".into(), Value::Null);
    extra.insert("operation_id".into(), json_codec::json!("bad"));
    assert_eq!(parse(extra).err().unwrap().message, "unknown field extra");
    let mut invalid = body();
    invalid.insert("operation_id".into(), json_codec::json!("bad"));
    let error = parse(invalid).err().unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidId);
    assert!(error.source.is_some());
}
