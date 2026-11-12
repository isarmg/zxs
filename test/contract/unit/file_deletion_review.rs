use super::*;
use json_codec::json;

fn summary() -> json_codec::Value {
    json!({
        "file_id":"file_11111111111111111111111111111111",
        "node_id":"fdel_11111111111111111111111111111111",
        "repository_id":"repo_11111111111111111111111111111111",
        "history_tree_id":"history_11111111111111111111111111111111",
        "path":"large.txt", "operation_id":"op_11111111111111111111111111111111",
        "author":"alice", "sequence":"9007199254740993", "created_at":"123",
        "branch_count":"65536", "parent_count":"65536"
    })
}
#[test]
fn summary_keeps_full_width_decimal_counts_and_original_identity() {
    let value = summary();
    let bytes = crate::canonical_json(&value).unwrap();
    let decoded: Summary = crate::decode_strict(&bytes).unwrap();
    assert_eq!(decoded.sequence.get(), 9_007_199_254_740_993);
    assert_eq!(decoded.branch_count, 65_536);
    assert_eq!(crate::canonical_json(&decoded).unwrap(), bytes);
}
#[test]
fn malformed_summary_identity_and_noncanonical_numbers_are_rejected() {
    for (field, value) in [
        ("node_id", json!("ver_11111111111111111111111111111111")),
        ("branch_count", json!(4097)),
        ("parent_count", json!("04097")),
        ("author", json!("")),
    ] {
        let mut malformed = summary();
        malformed[field] = value;
        assert!(json_codec::from_value::<Summary>(malformed).is_err());
    }
}
#[test]
fn audit_pages_require_explicit_continuations_and_preserve_parent_order() {
    let parent = json!({
        "file_id":"file_11111111111111111111111111111111",
        "deletion_node_id":"fdel_11111111111111111111111111111111",
        "offset":"100",
        "entries":["ver_11111111111111111111111111111111",
            "fmeta_22222222222222222222222222222222"],
        "next_offset":null
    });
    let page: ParentPage = json_codec::from_value(parent.clone()).unwrap();
    assert_eq!(page.offset, 100);
    assert_eq!(
        page.entries[0].to_string(),
        "ver_11111111111111111111111111111111"
    );
    assert_eq!(json_codec::to_value(page).unwrap(), parent);
    let mut malformed = parent.clone();
    malformed["entries"][0] = json!("fdel_11111111111111111111111111111111");
    assert!(json_codec::from_value::<ParentPage>(malformed).is_err());
    let malformed_head = json!({
        "branch_id":"br_11111111111111111111111111111111",
        "node_id":"fdel_11111111111111111111111111111111"
    });
    assert!(json_codec::from_value::<BranchHead>(malformed_head).is_err());
    let mut missing = parent;
    missing.as_object_mut().unwrap().remove("next_offset");
    assert!(json_codec::from_value::<ParentPage>(missing).is_err());
    let branch = json!({
        "file_id":"file_11111111111111111111111111111111",
        "deletion_node_id":"fdel_11111111111111111111111111111111",
        "entries":[]
    });
    assert!(json_codec::from_value::<BranchPage>(branch).is_err());
}
#[test]
fn compact_undo_body_has_the_original_two_wire_fields() {
    let body: UndoBody = json_codec::from_value(json!({
        "deletion_node_id":"fdel_11111111111111111111111111111111",
        "operation_id":"op_22222222222222222222222222222222"
    }))
    .unwrap();
    let expected = concat!(
        "{\"deletion_node_id\":\"fdel_11111111111111111111111111111111\",",
        "\"operation_id\":\"op_22222222222222222222222222222222\"}",
    );
    assert_eq!(crate::canonical_json(&body).unwrap(), expected.as_bytes());
}
