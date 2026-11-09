use super::*;
#[test]
fn reset_confirmation_and_repeat_mismatch_reject_before_system_access() {
    let request = || Reset {
        input: Input {
            service_user: "not-a-real-account".into(),
        },
        request_digest: "a".repeat(64),
        confirmation: "reset-existing-zxs-administrator-password".into(),
        external_writers_stopped: true,
        new_password: SecretString::new("new-password-2026".into()),
        repeat_password: SecretString::new("different-password-2026".into()),
    };
    let error = reset(request()).unwrap_err();
    assert!(
        error.to_string().contains("before") || error.to_string().contains("no credential access")
    );
    assert!(!error.to_string().contains("new-password-2026"));
    let mut wrong = request();
    wrong.confirmation = "reset".into();
    assert!(
        reset(wrong)
            .unwrap_err()
            .to_string()
            .contains("acknowledgments")
    );
}
#[test]
fn password_request_is_strict_and_secrets_are_redacted_on_drop_wrappers() {
    let value = json_codec::json!({"input":{"service_user":"zxs"},"request_digest":"a".repeat(64),
        "confirmation":"reset-existing-zxs-administrator-password","external_writers_stopped":true,
        "new_password":"secret-for-test-2026","repeat_password":"secret-for-test-2026"});
    let request: Reset = json_codec::from_value(value.clone()).unwrap();
    assert_eq!(request.new_password.expose(), "secret-for-test-2026");
    assert_eq!(format!("{:?}", request.new_password), "[REDACTED]");
    let mut unknown = value;
    unknown["admin_username"] = "somebody-else".into();
    assert!(json_codec::from_value::<Reset>(unknown).is_err());
}
#[test]
fn reset_digest_cannot_become_an_arbitrary_journal_path() {
    assert!(request_digest(&"a".repeat(64)).is_ok());
    for value in ["../outside", "", "A", "not-a-digest"] {
        assert!(request_digest(value).is_err());
    }
}
