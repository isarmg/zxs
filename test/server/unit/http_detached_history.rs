use super::*;
use json_codec::json;

#[test]
fn detached_query_retains_first_page_and_exact_cursor_identifiers() {
    let first: Query = json_codec::from_value(json!({})).unwrap();
    assert_eq!(first.0.unwrap(), Request::default());
    let root = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let file = "file_000102030405060708090a0b0c0d0e0f";
    let page: Query = json_codec::from_value(json!({"snapshot":root,"after_file":file})).unwrap();
    let request = page.0.unwrap();
    assert_eq!(request.snapshot.unwrap().to_string(), root);
    assert_eq!(request.after_file.unwrap().as_str(), file);
    let empty: Query = json_codec::from_value(json!({"snapshot":null,"after_file":null})).unwrap();
    assert_eq!(empty.0.unwrap(), Request::default());
}

#[test]
fn detached_query_preserves_contract_errors_and_paired_cursor_admission() {
    let root = "a".repeat(64);
    let file = "file_00000000000000000000000000000000";
    for fields in [
        json!({"snapshot":root}),
        json!({"after_file":file}),
        json!({"snapshot":"malformed","after_file":file}),
        json!({"snapshot":root,"after_file":"malformed"}),
        json!({"snapshot":root,"after_file":"ver_00000000000000000000000000000000"}),
        json!({"snapshot":root,"after_file":"file_0000000000000000000000000000000A"}),
        json!({"snapshot":"A".repeat(64),"after_file":file}),
        json!({"snapshot":"","after_file":""}),
    ] {
        let parsed: Query = json_codec::from_value(fields).unwrap();
        let error = ApiError::contract_input(parsed.0.unwrap_err());
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        assert!(matches!(
            error.source.as_deref(),
            Some(crate::http_boundary::ApiErrorSource::Contract(_)),
        ));
    }
}

#[test]
fn detached_query_shape_errors_are_still_deserializer_rejections() {
    for fields in [
        json!({"snapshot":17}),
        json!({"after_file":[]}),
        json!({"unknown":true}),
        json!([]),
    ] {
        assert!(json_codec::from_value::<Query>(fields).is_err());
    }
}
