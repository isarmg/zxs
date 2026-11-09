use super::*;
#[test]
fn first_administrator_requires_confirmation_and_matching_password_before_access() {
    let request = || Create {
        input: Input {
            service_user: "zxs".into(),
            administrator: "admin".into(),
        },
        request_digest: "a".repeat(64),
        confirmation: "initialize-first-management-administrator".into(),
        external_writers_stopped: true,
        password: SecretString::new("initial-password-2026".into()),
        repeat_password: SecretString::new("different-password-2026".into()),
    };
    assert!(
        create(request())
            .unwrap_err()
            .to_string()
            .contains("no system access")
    );
    let mut wrong = request();
    wrong.confirmation = "initialize-system".into();
    assert!(
        create(wrong)
            .unwrap_err()
            .to_string()
            .contains("before system access")
    );
    assert!(checked_digest("../different-operation").is_err());
}
#[test]
fn first_administrator_request_has_no_path_or_enable_override() {
    let value = json_codec::json!({"input":{"service_user":"zxs","administrator":"admin"},
        "request_digest":"a".repeat(64),"confirmation":"initialize-first-management-administrator",
        "external_writers_stopped":true,"password":"initial-password-2026","repeat_password":"initial-password-2026"});
    let parsed: Create = json_codec::from_value(value.clone()).unwrap();
    assert_eq!(format!("{:?}", parsed.password), "[REDACTED]");
    let mut extra = value;
    extra["storage_initialized"] = true.into();
    assert!(json_codec::from_value::<Create>(extra).is_err());
}
