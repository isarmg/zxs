use super::*;
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
