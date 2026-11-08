use super::*;

#[test]
fn repair_confirmation_precedes_root_payload_or_system_path_access() {
    for (confirmation, stopped) in [("install-program-only", true), ("repair-same-payload-program-only", false)] {
        let error = commit(Commit {
            request_digest: "invalid".into(), confirmation: confirmation.into(),
            external_administration_stopped: stopped,
        }).err().expect("confirmation must fail before host access");
        assert!(error.to_string().contains("before reading or writing"));
    }
}

#[test]
fn original_digest_and_wire_never_accept_an_uploaded_replacement_or_path() {
    for value in ["", "abc", &"A".repeat(64), &"a/".repeat(32)] { assert!(digest(value).is_err()); }
    assert!(digest(&"a".repeat(64)).is_ok());
    let body = r#"{"request_digest":"a","confirmation":"repair-same-payload-program-only","external_administration_stopped":true,"path":"/tmp/other"}"#;
    assert!(json_codec::from_str::<Commit>(body).is_err());
    assert!(json_codec::from_str::<Original>(r#"{"request_digest":"a","payload":"upload"}"#).is_err());
}
