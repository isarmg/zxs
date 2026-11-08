use super::*;
use json_codec::json;

fn input() -> json_codec::Value {
    json!({"deployment":{"service_user":"zxs","service_group":"zxs","listen":"0.0.0.0:7400","smb":{"account":"share","peer_uid":0}},
        "operation_id":"00112233445566778899aabbccddeeff","external_writers_stopped":false})
}
#[test]
fn operation_keys_have_fixed_lowercase_hex_shapes() {
    assert!(hex("00112233445566778899aabbccddeeff", 32).is_ok());
    assert!(hex(&"0".repeat(64), 64).is_ok());
    for invalid in [
        "",
        "../old",
        "00112233445566778899aabbccddeefF",
        "00112233445566778899aabbccddeeff\n",
    ] {
        assert!(hex(invalid, 32).is_err());
    }
}
#[test]
fn password_dto_rejects_unknown_fields_and_preserves_secrets_only_in_memory() {
    let mut body = json!({"input":input(),"request_digest":"0".repeat(64),"confirmation":"reset-existing-local-samba-password",
        "new_password":"test-password-only","repeat_password":"test-password-only"});
    let decoded: Reset = json_codec::from_value(body.clone()).unwrap();
    assert_eq!(decoded.new_password.expose(), "test-password-only");
    body["command"] = json!("anything");
    assert!(json_codec::from_value::<Reset>(body).is_err());
}
#[test]
fn missing_stop_confirmation_fails_before_any_system_target_access() {
    let request:Reset=json_codec::from_value(json!({"input":input(),"request_digest":"0".repeat(64),
        "confirmation":"reset-existing-local-samba-password","new_password":"test-password-only","repeat_password":"test-password-only"})).unwrap();
    let error = reset(request).err().unwrap();
    assert!(error.to_string().contains("acknowledgments"));
    assert!(!error.to_string().contains("test-password-only"));
}
#[test]
fn different_password_entries_fail_before_preparing_any_system_target() {
    let mut body = input();
    body["external_writers_stopped"] = json!(true);
    let request:Reset=json_codec::from_value(json!({"input":body,"request_digest":"0".repeat(64),
        "confirmation":"reset-existing-local-samba-password","new_password":"first-test-password","repeat_password":"second-test-password"})).unwrap();
    assert_eq!(
        reset(request).err().unwrap().to_string(),
        "new password entries do not match"
    );
}
#[test]
fn pending_diagnostic_never_formats_its_private_source() {
    let pending = Pending {
        request_digest: "a".repeat(64),
        source: "private-source-test-value".into(),
    };
    assert!(pending.to_string().contains(&"a".repeat(64)));
    assert!(!pending.to_string().contains("private-source-test-value"));
}
