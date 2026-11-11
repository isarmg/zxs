use super::*;

fn path(value: &str) -> OperationLookupPath {
    json_codec::from_value(json_codec::json!(value)).unwrap()
}

#[test]
fn operation_lookup_keeps_not_found_mapping_and_parse_source() {
    for value in [
        "",
        "wrong",
        "op_0000000000000000000000000000000A",
        "op_000000000000000000000000000000000",
        "br_00000000000000000000000000000000",
        " op_00000000000000000000000000000000",
    ] {
        let error = operation_id(path(value)).unwrap_err();
        assert_eq!(error.status, ::http::StatusCode::NOT_FOUND);
        assert_eq!(error.code, storage_contract::ErrorCode::OperationNotFound);
        let Some(crate::http_boundary::ApiErrorSource::Contract(source)) = error.source.as_deref()
        else {
            panic!("malformed operation lookup must retain its contract parse failure");
        };
        assert!(matches!(
            source,
            storage_contract::Error::InvalidIdentifier { prefix: "op", .. },
        ));
    }
}

#[test]
fn operation_lookup_path_retains_fixed_canonical_identity_bytes() {
    let original = "op_000102030405060708090a0b0c0d0e0f";
    let operation = operation_id(path(original)).unwrap();
    assert_eq!(operation.to_string(), original);
    assert_eq!(
        storage_contract::canonical_json(&operation).unwrap(),
        br#""op_000102030405060708090a0b0c0d0e0f""#,
    );
    assert!(json_codec::from_value::<OperationLookupPath>(json_codec::json!(17)).is_err());
    assert!(json_codec::from_value::<OperationLookupPath>(json_codec::json!(null)).is_err());
}
