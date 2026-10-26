use super::*;
use json_codec::json;
use std::sync::Arc;
fn commit(id: &str, entries: Vec<Entry>) -> Arc<storage_contract::Commit> {
    Arc::new(storage_contract::Commit {
        commit_id: id.into(),
        history_tree_id: "history".into(),
        kind: "open-kind".into(),
        parent_commit_ids: vec![],
        tree_id: "tree".into(),
        sequence: 1,
        operation_id: storage_contract::OperationId::generate(),
        author: "owner".into(),
        message: "".into(),
        entries,
        pending_file_versions: Default::default(),
        namespace_deletions: None,
        provenance: None,
    })
}
fn entry(path: &str, version: &str) -> Entry {
    Entry {
        entry_id: format!("entry-{path}"),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: storage_contract::NamespaceEntryKind::File,
        file_id: "file".into(),
        version_id: version.into(),
        size: u64::MAX,
    }
}
#[test]
fn comparison_wire_preserves_change_variants_omission_order_and_entry_fields() {
    let left = commit(
        "left",
        vec![entry("keep.txt", "version"), entry("remove.txt", "old")],
    );
    let right = commit(
        "right",
        vec![entry("KEEP.txt", "version"), entry("add.txt", "new")],
    );
    let expected = json!({"left":"left","right":"right","changes":[
        {"path":"KEEP.txt","change":"modified","before":left.entries[0],"after":right.entries[0]},
        {"path":"remove.txt","change":"removed","before":left.entries[1]},
        {"path":"add.txt","change":"added","after":right.entries[1]}
    ]});
    let response = ComparisonResponse::from(CommitComparison::between(left, right));
    assert_eq!(json_codec::to_value(&response).unwrap(), expected);
    assert_eq!(
        storage_contract::canonical_json(&response).unwrap(),
        storage_contract::canonical_json(&expected).unwrap()
    );
    assert_eq!(
        expected["changes"][0]["before"]["size"],
        u64::MAX.to_string()
    );
    let empty = ComparisonResponse::from(CommitComparison::between(
        commit("left", vec![]),
        commit("right", vec![]),
    ));
    assert_eq!(
        storage_contract::canonical_json(&empty).unwrap(),
        br#"{"changes":[],"left":"left","right":"right"}"#
    );
}
