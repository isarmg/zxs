use super::*;
#[test]
fn wrong_confirmation_fails_before_root_or_payload_access() {
    let error = prepare(Prepare {
        request_digest: "wrong".into(),
        confirmation: "initialize-system".into(),
    })
    .unwrap_err();
    assert!(error.to_string().contains("prepare-host-root-only"));
}
