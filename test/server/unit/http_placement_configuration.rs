use super::*;
use json_codec::json;
fn body() -> Value {
    json!({"repository_id":"repo","path":"docs","primary_slot_id":"P01",
        "expected_scope_id":"directory","expected_rule_id":null,"expected_generation":"0",
        "operation_id":"op_00000000000000000000000000000001"})
}
#[test]
fn placement_configuration_requires_identity_and_string_generation_baselines() {
    let request = parse(body()).unwrap();
    assert_eq!(request.expected_rule_id, None);
    assert_eq!(request.expected_generation, 0);
    for field in [
        "repository_id",
        "path",
        "primary_slot_id",
        "expected_scope_id",
        "expected_rule_id",
        "expected_generation",
        "operation_id",
    ] {
        let mut invalid = body();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(parse(invalid).is_err(), "accepted missing {field}");
    }
    for (field, invalid) in [
        ("expected_scope_id", json!("")),
        ("expected_rule_id", json!("")),
        ("expected_generation", json!(0)),
        ("path", json!("../outside")),
        ("unexpected", json!(true)),
    ] {
        let mut value = body();
        value[field] = invalid;
        assert!(parse(value).is_err(), "accepted invalid {field}");
    }
}
