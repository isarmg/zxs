use super::*;
#[test]
fn historical_source_request_is_exact_and_reuses_typed_identifiers() {
    let file = "file_00000000000000000000000000000000";
    let node = "ver_00000000000000000000000000000000";
    let request = FileHistorySourceRequest::parse(file, node, &"a".repeat(64)).unwrap();
    crate::validate_api_path(&request.route()).unwrap();
    assert_eq!(request.query_fields()["node_id"], node);
    assert_eq!(
        request.history_request().history_root,
        Some(request.history_root)
    );
    for (file, node, root) in [
        ("bad", node, "a".repeat(64)),
        (file, "bad", "a".repeat(64)),
        (file, node, "bad".into()),
    ] {
        assert!(FileHistorySourceRequest::parse(file, node, &root).is_err());
    }
}
