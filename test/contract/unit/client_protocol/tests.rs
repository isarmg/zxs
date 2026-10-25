use super::*;

#[test]
fn logout_confirmation_accepts_only_the_current_closed_response() {
    let current: Value = json_codec::from_str(include_str!(
        "../../../tools/contract-check/fixtures/authentication/LogoutResponse.json"
    ))
    .unwrap();
    assert!(logout_confirmed(&current));
    for value in [
        Value::Null,
        json!([]),
        json!(true),
        json!({}),
        json!({"logged_out":false}),
        json!({"logged_out":"true"}),
        json!({"logged_out":true,"admin":true}),
        json!({"logged_out":true,"extra":null}),
    ] {
        assert!(!logout_confirmed(&value), "accepted {value}");
    }
    assert!(!logout_confirmed(&decode_response(204, b"").unwrap()));
    assert!(decode_response(200, br#"{"logged_out":true,"logged_out":true}"#).is_err());
}

#[test]
fn password_change_confirmation_requires_both_true_fields_and_no_extras() {
    let current: Value = json_codec::from_str(include_str!(
        "../../../tools/contract-check/fixtures/authentication/PasswordChangeResponse.json"
    ))
    .unwrap();
    assert!(password_change_confirmed(&current));
    for value in [Value::Null, json!([]), json!(true), json!({})] {
        assert!(!password_change_confirmed(&value));
    }
    for field in ["changed", "reauthentication_required"] {
        let mut missing = current.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(!password_change_confirmed(&missing));
        for replacement in [Value::Null, json!(false), json!(1), json!("true")] {
            let mut invalid = current.clone();
            invalid[field] = replacement;
            assert!(!password_change_confirmed(&invalid));
        }
    }
    let mut extra = current;
    extra["admin"] = true.into();
    assert!(!password_change_confirmed(&extra));
    assert!(!password_change_confirmed(
        &decode_response(200, b"").unwrap()
    ));
}

#[test]
fn seal_identity_is_checked_before_preparation() {
    for seal in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"workspace_id":"other"}),
    ] {
        assert_eq!(
            prepare_commit(seal, "workspace_test", "op_test", "save", None)
                .unwrap_err()
                .code(),
            crate::ErrorCode::InvalidResponse
        );
    }
}

#[test]
fn preparation_has_one_canonical_precondition_and_digest() {
    let seal = json!({"workspace_id":"workspace_test", "manifest_digest":"abc", "expected_head":"sealed_head", "request_digest":"stale", "operation_id":"stale", "message":"stale"});
    let absent = prepare_commit(seal.clone(), "workspace_test", "op_test", "save", None).unwrap();
    let empty =
        prepare_commit(seal.clone(), "workspace_test", "op_test", "save", Some("")).unwrap();
    assert_eq!(absent, empty);
    assert_eq!(absent["expected_head"], "sealed_head");
    assert_eq!(absent["operation_id"], "op_test");
    assert_eq!(absent["message"], "save");
    let with_head =
        prepare_commit(seal, "workspace_test", "op_test", "save", Some("head_test")).unwrap();
    assert_eq!(with_head["expected_head"], "head_test");
    assert_ne!(with_head["request_digest"], absent["request_digest"]);
    assert_eq!(
        absent["request_digest"],
        crate::request_digest(
            "POST",
            "/api/workspaces/{workspace}/commit",
            json!({"workspace":"workspace_test"}),
            json!({}),
            &absent
        )
        .unwrap()
    );
}

#[test]
fn response_json_rejects_duplicate_aliases_and_retains_telemetry_decimals() {
    for body in [
        br#"{"token":"a","token":"b"}"#.as_slice(),
        br#"{"items":[{"code":1,"co\u0064e":2}]}"#,
        b"{} {}",
        br#"{"x":1e999}"#,
    ] {
        assert_eq!(
            decode_response(200, body).unwrap_err().code,
            crate::ErrorCode::InvalidResponse
        );
    }
    let body = br#"{"cpu":8.899999618530273,"bytes":"18446744073709551615"}"#;
    assert!(decode_response(200, body).unwrap()["cpu"].is_f64());
    assert!(crate::strict_json(body).is_err());
}

#[test]
fn status_and_publication_classification_survive_all_response_failures() {
    for status in [0, 200, 204, 302, 400, 401, 409, 422, 429, 500, 503] {
        let failure = decode_response(status, b"secret-not-json").unwrap_err();
        assert_eq!(failure.status, status);
        assert!(!failure.message.contains("secret"));
        assert_eq!(
            mutation_outcome_unknown(status),
            !(400..500).contains(&status)
        );
    }
    assert!(!publication_attempted(Some(false)));
    assert!(publication_attempted(Some(true)));
    assert!(publication_attempted(None));
    assert_eq!(decode_response(204, b"").unwrap(), Value::Null);
    assert_eq!(
        decode_response(404, b"").unwrap_err().code,
        crate::ErrorCode::HttpError
    );
}

#[test]
fn response_envelopes_preserve_fields_and_share_a_bounded_size() {
    let error = decode_response(503, br#"{"error":{"code":"STORAGE_UNAVAILABLE","message":"offline","retryable":false,"request_id":"req_test"}}"#).unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::StorageUnavailable);
    assert_eq!(error.request_id.as_deref(), Some("req_test"));
    assert!(!error.retryable);
    let mut bytes = vec![b' '; MAX_RESPONSE_BYTES];
    bytes[..2].copy_from_slice(b"{}");
    assert!(decode_response(200, &bytes).is_ok());
    bytes.push(b' ');
    let error = decode_response(409, &bytes).unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::ResponseTooLarge);
    assert_eq!(error.status, 409);
}

#[test]
fn unknown_remote_code_is_an_explicit_decode_failure_with_source() {
    use std::error::Error as _;
    let failure = decode_response(
        503,
        br#"{"error":{"code":"FUTURE_UNKNOWN_CODE","message":"private","retryable":false}}"#,
    )
    .unwrap_err();
    assert_eq!(failure.status, 503);
    assert_eq!(failure.code, crate::ErrorCode::InvalidResponse);
    assert!(!failure.message.contains("private"));
    assert!(matches!(failure.source, Some(ResponseDecodeError::Code(_))));
    assert!(
        failure
            .source()
            .unwrap()
            .source()
            .unwrap()
            .is::<json_codec::Error>()
    );
    let failure = decode_response(200, b"{").unwrap_err();
    assert!(matches!(failure.source, Some(ResponseDecodeError::Json(_))));
}
