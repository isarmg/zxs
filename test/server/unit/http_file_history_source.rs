use super::*;
use json_codec::json;

fn file(value: &str) -> FilePath {
    json_codec::from_value(json!(value)).unwrap()
}

fn query(node: &str, root: &str) -> Query {
    json_codec::from_value(json!({"node_id":node,"history_root":root})).unwrap()
}

#[test]
fn history_source_request_preserves_fixed_route_and_query_bytes() {
    let file_id = "file_000102030405060708090a0b0c0d0e0f";
    let node_id = "ver_101112131415161718191a1b1c1d1e1f";
    let root = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let request = query(node_id, root).request(file(file_id)).unwrap();
    assert_eq!(
        request.route(),
        "/api/file-manager/files/file_000102030405060708090a0b0c0d0e0f/merge-history/source",
    );
    let fields = storage_contract::canonical_json(&request.query_fields()).unwrap();
    let expected = concat!(
        "{\"history_root\":",
        "\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\",",
        "\"node_id\":\"ver_101112131415161718191a1b1c1d1e1f\"}",
    );
    assert_eq!(fields, expected.as_bytes());
}

#[test]
fn history_source_node_domain_is_exactly_version_anchor_and_metadata() {
    let file_id = "file_00000000000000000000000000000000";
    let root = "a".repeat(64);
    for prefix in ["ver", "fnode", "fmeta"] {
        let node = format!("{prefix}_00000000000000000000000000000000");
        let request = query(&node, &root).request(file(file_id)).unwrap();
        assert_eq!(request.node_id.as_str(), node);
    }
    for prefix in ["fdel", "fmerge", "file", "br"] {
        let node = format!("{prefix}_00000000000000000000000000000000");
        let error = query(&node, &root).request(file(file_id)).unwrap_err();
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        assert!(matches!(
            error.source.as_deref(),
            Some(crate::http_boundary::ApiErrorSource::Contract(_)),
        ));
    }
}

#[test]
fn history_source_contract_errors_are_deferred_but_query_shape_is_not() {
    let valid_file = "file_00000000000000000000000000000000";
    let valid_node = "ver_00000000000000000000000000000000";
    let valid_root = "a".repeat(64);
    for (file_id, node, root) in [
        ("malformed", valid_node, valid_root.as_str()),
        ("file_0000000000000000000000000000000A", valid_node, valid_root.as_str()),
        (valid_file, "ver_0000000000000000000000000000000A", valid_root.as_str()),
        (valid_file, valid_node, ""),
        (
            valid_file,
            valid_node,
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ),
    ] {
        let parsed = query(node, root);
        let error = parsed.request(file(file_id)).unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        assert!(matches!(
            error.source.as_deref(),
            Some(crate::http_boundary::ApiErrorSource::Contract(_)),
        ));
    }
    for fields in [
        json!({}),
        json!({"node_id":valid_node}),
        json!({"node_id":valid_node,"history_root":valid_root,"unexpected":true}),
        json!({"node_id":17,"history_root":valid_root}),
    ] {
        assert!(json_codec::from_value::<Query>(fields).is_err());
    }
    assert!(json_codec::from_value::<FilePath>(json!(17)).is_err());
}
