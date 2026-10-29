use super::*;
use json_codec::json;

#[test]
fn graph_all_deferred_commit_fields_and_lists_have_detail_controls() {
    let mut page = super::super::tests::page();
    page.history.provenance = Some(TextSummary::Deferred { bytes: 300 });
    let value = json!({
        "commit_id":"commit", "history_tree_id":"history-one", "kind":{"kind":"deferred","bytes":"300"},
        "parent_commit_ids":["parent"], "tree_id":{"kind":"deferred","bytes":"300"}, "sequence":"1",
        "operation_id":storage_contract::OperationId::generate(), "author":{"kind":"deferred","bytes":"300"},
        "message":{"kind":"deferred","bytes":"300"},
        "namespace":{"root":{"kind":"namespace-page","hash":"b".repeat(64)},"entries":"0"},
        "namespace_deletions":{"timestamp":"3","paths":"2"}, "pending_file_versions":"1", "provenance":{"kind":"deferred","bytes":"300"}
    });
    page.items = GraphItems::Commits(vec![json_codec::from_value(value).unwrap()]);
    let buttons = buttons(&page);
    assert_eq!(buttons.len(), 8);
    for name in [
        "history.provenance",
        "commit.kind",
        "commit.tree_id",
        "commit.author",
        "commit.message",
        "commit.provenance",
        "commit.pending_file_versions",
        "commit.namespace_deletions",
    ] {
        assert!(buttons.iter().any(|(label, _)| label == name), "{name}");
    }
    let encoded = json_codec::to_vec(&page).unwrap();
    assert!(
        super::super::wire::main_page(200, &encoded, "history-one", GraphCollection::Commits, None)
            .is_ok()
    );
}
