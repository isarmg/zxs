use super::*;
use json_codec::json;

#[test]
fn share_update_preserves_field_priority_and_operation_error_source() {
    let parse = |value: json_codec::Value| UpdateBody::parse(value.as_object().unwrap());
    assert_eq!(
        parse(json!({"extra":true})).err().unwrap().message,
        "unknown field extra"
    );
    assert_eq!(
        parse(json!({})).err().unwrap().message,
        "enabled must be a boolean"
    );
    assert_eq!(
        parse(json!({"enabled":true})).err().unwrap().message,
        "missing string field expected_history_generation"
    );
    for generation in ["01", "-1", "18446744073709551616"] {
        let error = parse(json!({"enabled":true,"expected_history_generation":generation}))
            .err()
            .unwrap();
        assert!(std::error::Error::source(&error).is_some());
    }
    assert_eq!(
        parse(json!({"enabled":true,"expected_history_generation":"1"}))
            .err()
            .unwrap()
            .message,
        "missing string field branch_id"
    );
    let error=parse(json!({"enabled":true,"expected_history_generation":"1","branch_id":"branch","operation_id":"invalid"})).err().unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidId);
    assert!(std::error::Error::source(&error).is_some());
    let body=parse(json!({"enabled":false,"expected_history_generation":u64::MAX.to_string(),"branch_id":"branch","operation_id":"op_00000000000000000000000000000000"})).unwrap();
    assert_eq!(body.expected_history_generation, u64::MAX);
    assert!(!body.enabled);
}
#[test]
fn share_status_wire_retains_history_generation_decimal() {
    let status = crate::SmbBindingStatus {
        repository_id: "repo".into(),
        branch_id: "branch".into(),
        enabled: false,
        history_generation: u64::MAX,
    };
    assert_eq!(
        json_codec::to_value(SmbBindingResponse::from(status)).unwrap(),
        json!({"repository_id":"repo","branch_id":"branch","enabled":false,"history_generation":"18446744073709551615"})
    );
}
