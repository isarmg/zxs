use super::*;
use json_codec::json;

fn file(value: &str) -> FilePath {
    json_codec::from_value(json!(value)).unwrap()
}

fn query(root: &str) -> Query {
    json_codec::from_value(json!({"history_root":root})).unwrap()
}

#[test]
fn shared_history_request_keeps_original_route_and_canonical_query() {
    let file_id = "file_000102030405060708090a0b0c0d0e0f";
    let root = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let request = query(root).request(file(file_id)).unwrap();
    assert_eq!(
        request.route(),
        "/api/file-manager/files/file_000102030405060708090a0b0c0d0e0f/shared-history",
    );
    assert_eq!(
        storage_contract::canonical_json(&request.query_fields()).unwrap(),
        concat!(
            "{\"history_root\":",
            "\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"}",
        )
        .as_bytes(),
    );
}

#[test]
fn shared_history_retains_contract_failures_until_request_admission() {
    let valid_file = "file_00000000000000000000000000000000";
    let valid_root = "a".repeat(64);
    for (file_id, root) in [
        ("malformed", valid_root.as_str()),
        ("ver_00000000000000000000000000000000", valid_root.as_str()),
        ("file_0000000000000000000000000000000A", valid_root.as_str()),
        (valid_file, ""),
        (
            valid_file,
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ),
        ("malformed", "malformed"),
    ] {
        let error = query(root).request(file(file_id)).unwrap_err();
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        assert!(matches!(
            error.source.as_deref(),
            Some(crate::http_boundary::ApiErrorSource::Contract(_)),
        ));
    }
}

#[test]
fn shared_history_query_structure_stays_strict() {
    for fields in [
        json!({}),
        json!({"history_root":null}),
        json!({"history_root":17}),
        json!({"history_root":"malformed","unexpected":true}),
    ] {
        assert!(json_codec::from_value::<Query>(fields).is_err());
    }
    assert!(json_codec::from_value::<FilePath>(json!(17)).is_err());
}
