use super::*;
#[test]
fn repository_creation_preserves_contract_error_details_and_sources() {
    let mut body = json_codec::json!({"name":"../bad","primary_slot_id":"P01","target_copies":1,"operation_id":storage_contract::OperationId::generate()});
    crate::tests::assert_contract_input(
        &parse(body.as_object().unwrap().clone()).err().unwrap(),
        &storage_contract::RepositoryName::parse("../bad").unwrap_err(),
    );
    body["operation_id"] = "bad".into();
    crate::tests::assert_contract_input(
        &parse(body.as_object().unwrap().clone()).err().unwrap(),
        &"bad".parse::<storage_contract::OperationId>().unwrap_err(),
    );
    body["unexpected"] = true.into();
    body["target_copies"] = Value::Null;
    assert_eq!(
        parse(body.as_object().unwrap().clone())
            .err()
            .unwrap()
            .message,
        "unknown field unexpected"
    );
}
#[test]
fn repository_creation_keeps_copy_representation_and_name_policy() {
    let body = json_codec::json!({"name":"a".repeat(200),"primary_slot_id":"P01","target_copies":0,"operation_id":"op_00000000000000000000000000000000"});
    let parsed = parse(body.as_object().unwrap().clone()).unwrap();
    assert_eq!(parsed.name.as_str().len(), 200);
    assert_eq!(parsed.target_copies, 0);
    let mut wrong = body.clone();
    wrong["target_copies"] = json_codec::json!(u64::MAX);
    assert!(parse(wrong.as_object().unwrap().clone()).is_err());
    let mut wrong = body;
    wrong["operation_id"] = "bad".into();
    wrong["name"] = "../bad".into();
    assert_eq!(
        parse(wrong.as_object().unwrap().clone())
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::InvalidId
    );
}
