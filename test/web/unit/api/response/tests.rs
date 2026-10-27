use super::*;

#[test]
fn mutation_decode_failures_are_uncertain_except_for_explicit_client_errors() {
    for status in [200, 201, 202, 204, 302, 500, 503] {
        let error = decode(status, b"<html>upstream response lost</html>").unwrap_err();
        assert!(error.mutation_outcome_unknown(), "status {status}");
    }
    for status in [400, 401, 403, 404, 409, 422, 429] {
        let error = decode(status, b"invalid JSON").unwrap_err();
        assert!(!error.mutation_outcome_unknown(), "status {status}");
    }
}

#[test]
fn real_cpu_response_decimals_and_full_width_byte_strings_are_accepted() {
    let bytes = br#"{"cpu":{"usage_percent":8.899999618530273,"logical_count":8},"memory":{"total_bytes":"18446744073709551615"},"disks":[]}"#;
    let value = decode(200, bytes).unwrap();
    assert_eq!(value["cpu"]["usage_percent"], 8.899999618530273);
    assert_eq!(value["cpu"]["logical_count"].as_u64(), Some(8));
    assert_eq!(value["memory"]["total_bytes"], u64::MAX.to_string());
    // This response-only exception must not weaken persisted/write JSON.
    assert!(storage_contract::strict_json(bytes).is_err());
}

#[test]
fn first_cpu_sample_can_remain_unavailable() {
    let value = decode(200, br#"{"cpu":{"usage_percent":null}}"#).unwrap();
    assert!(value["cpu"]["usage_percent"].is_null());
}

#[test]
fn duplicate_keys_are_rejected_at_every_depth_including_escaped_aliases() {
    for bytes in [
        br#"{"token":"first","token":"second"}"#.as_slice(),
        br#"{"cpu":{"usage_percent":1.2,"usage_percent":2.3}}"#,
        br#"{"items":[{"code":"first","co\u0064e":"second"}]}"#,
    ] {
        let error = decode(200, bytes).unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidResponse);
        assert_eq!(error.status, 200);
    }
}

#[test]
fn malformed_json_and_non_finite_numbers_are_rejected_without_exposing_body() {
    for bytes in [
        b"\xef\xbb\xbf{}".as_slice(),
        br#"{"cpu":NaN}"#,
        br#"{"cpu":Infinity}"#,
        br#"{"cpu":1e999}"#,
        br#"{"cpu":"\ud800"}"#,
        b"{} {}",
        b"<html>private-token</html>",
    ] {
        let error = decode(502, bytes).unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidResponse);
        assert_eq!(error.status, 502);
        assert!(!error.message.contains("private-token"));
        assert!(!error.message.contains("HTML"));
    }
}

#[test]
fn json_errors_keep_actual_status_code_retryability_and_request_id() {
    let bytes = br#"{"error":{"code":"DISK_UNAVAILABLE","message":"A disk is offline","retryable":true,"request_id":"req_test"}}"#;
    let error = decode(503, bytes).unwrap_err();
    assert_eq!(error.status, 503);
    assert_eq!(error.code, storage_contract::ErrorCode::DiskUnavailable);
    assert_eq!(error.message, "A disk is offline");
    assert!(error.retryable);
    assert_eq!(error.request_id.as_deref(), Some("req_test"));
}

#[test]
fn published_reconciliation_error_is_uncertain_without_inviting_mutation_retry() {
    let bytes = br#"{"error":{"code":"RECOVERY_REQUIRED","message":"operation original is already published; retain the original operation","retryable":false,"request_id":"published-request"}}"#;
    let error = decode(503, bytes).unwrap_err();
    assert_eq!(error.status, 503);
    assert_eq!(error.code, storage_contract::ErrorCode::RecoveryRequired);
    assert!(!error.retryable);
    assert!(error.mutation_outcome_unknown());
    assert_eq!(error.request_id.as_deref(), Some("published-request"));
    assert!(error.message.contains("original is already published"));

    let rejected = decode(
        400,
        br#"{"error":{"code":"UNSAFE_PATH","message":"unsafe path","retryable":false}}"#,
    )
    .unwrap_err();
    assert_eq!(rejected.code, storage_contract::ErrorCode::UnsafePath);
    assert!(!rejected.mutation_outcome_unknown());
}

#[test]
fn empty_http_errors_keep_their_status() {
    let error = decode(404, b"").unwrap_err();
    assert_eq!(error.status, 404);
    assert_eq!(error.code, storage_contract::ErrorCode::HttpError);
}

#[test]
fn protocol_decode_sources_survive_the_browser_adapter_and_cloning() {
    use std::error::Error as _;
    let failure = decode(200, b"{").unwrap_err();
    let cloned = failure.clone();
    let source = failure.source().expect("decode source");
    assert!(
        source.source().is_some(),
        "the original JSON error remains chained"
    );
    assert!(std::sync::Arc::ptr_eq(
        failure.source.as_ref().unwrap(),
        cloned.source.as_ref().unwrap()
    ));
}
