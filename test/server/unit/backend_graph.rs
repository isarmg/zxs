use super::*;
use storage_contract::history_graph::{GraphItems, HistorySummary};

fn query(fields: &[(&str, &str)]) -> BTreeMap<String, String> {
    fields
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect()
}
#[test]
fn graph_queries_reject_unknown_fields_and_wrong_cursor_scope() {
    assert!(ReadHistoryGraph::page("history".into(), BTreeMap::new()).is_err());
    assert!(
        ReadHistoryGraph::page(
            "history".into(),
            query(&[("collection", "commits"), ("all", "true")])
        )
        .is_err()
    );
    for limit in ["0", "129", "18446744073709551616"] {
        assert!(
            ReadHistoryGraph::page(
                "history".into(),
                query(&[("collection", "commits"), ("limit", limit)])
            )
            .is_err()
        );
    }
    let token = cursor(
        "history",
        &"a".repeat(64),
        GraphCollection::Commits,
        PageLimit::new(50).unwrap(),
        GraphKey::Commit {
            sequence: 7,
            commit_id: "commit".into(),
        },
    )
    .unwrap();
    let request = ReadHistoryGraph::page(
        "history".into(),
        query(&[("collection", "commits"), ("cursor", &token)]),
    )
    .unwrap();
    assert_eq!(request.root.unwrap().as_str(), "a".repeat(64));
    assert!(matches!(
        request.selection,
        GraphRead::Page {
            after: Some(GraphKey::Commit { sequence: 7, .. }),
            ..
        }
    ));
    for (history, collection, limit) in [
        ("other", "commits", "50"),
        ("history", "branches", "50"),
        ("history", "commits", "49"),
    ] {
        assert!(
            ReadHistoryGraph::page(
                history.into(),
                query(&[
                    ("collection", collection),
                    ("limit", limit),
                    ("cursor", &token)
                ])
            )
            .is_err()
        );
    }
    for token in ["xx".to_owned(), "a".repeat(32_770)] {
        assert!(
            ReadHistoryGraph::page(
                "history".into(),
                query(&[("collection", "commits"), ("cursor", &token)])
            )
            .is_err()
        );
    }
}

#[test]
fn graph_cursor_key_and_root_are_boundary_checked() {
    for (root, key) in [
        (
            "a".repeat(63),
            GraphKey::Commit {
                sequence: 1,
                commit_id: "commit".into(),
            },
        ),
        (
            "a".repeat(64),
            GraphKey::Id {
                id: "branch".into(),
            },
        ),
        (
            "a".repeat(64),
            GraphKey::Commit {
                sequence: 1,
                commit_id: String::new(),
            },
        ),
    ] {
        let token = cursor(
            "history",
            &root,
            GraphCollection::Commits,
            PageLimit::new(50).unwrap(),
            key,
        )
        .unwrap();
        assert!(
            ReadHistoryGraph::page(
                "history".into(),
                query(&[("collection", "commits"), ("cursor", &token)])
            )
            .is_err()
        );
    }
}

#[test]
fn graph_auxiliary_requests_require_root_and_closed_field_names() {
    let root = "a".repeat(64);
    assert!(
        ReadHistoryGraph::history_field("history".into(), "provenance".into(), BTreeMap::new())
            .is_err()
    );
    assert!(
        ReadHistoryGraph::commit_field(
            "history".into(),
            "commit".into(),
            "namespace".into(),
            query(&[("history_root", &root)])
        )
        .is_err()
    );
    assert!(
        ReadHistoryGraph::commit_field(
            "history".into(),
            "commit".into(),
            "message".into(),
            query(&[("history_root", &root), ("limit", "3")])
        )
        .is_err()
    );
    let request = ReadHistoryGraph::commit_field(
        "history".into(),
        "commit".into(),
        "message".into(),
        query(&[("history_root", &root), ("offset", "8")]),
    )
    .unwrap();
    assert!(matches!(
        request.selection,
        GraphRead::Text {
            offset: 8,
            target: TextTarget::Commit {
                field: CommitField::Message,
                ..
            },
            ..
        }
    ));
    assert!(
        ReadHistoryGraph::commit_pending(
            "history".into(),
            "commit".into(),
            query(&[("history_root", &root), ("after", "")])
        )
        .is_err()
    );
    assert!(
        ReadHistoryGraph::deletions(
            "history".into(),
            "commit".into(),
            query(&[("history_root", &root), ("offset", "-1")])
        )
        .is_err()
    );
}

#[test]
fn graph_response_encodes_shared_wire_and_retains_explicit_empty_collection() {
    let response = HistoryGraphResponse::Graph(GraphPage {
        history: HistorySummary {
            history_tree_id: "history".into(),
            repository_id: "repository".into(),
            root_commit_id: "root".into(),
            main_branch_id: None,
            generation: u64::MAX,
            state: storage_contract::HistoryState::Archived,
            provenance: None,
        },
        history_root: "a".repeat(64),
        items: GraphItems::Commits(Vec::new()),
        next_cursor: None,
    });
    let encoded = response.encode().unwrap();
    let value: json_codec::Value = json_codec::from_slice(encoded.body()).unwrap();
    assert_eq!(value["history"]["generation"], u64::MAX.to_string());
    assert_eq!(value["items"]["collection"], "commits");
    assert_eq!(value["items"]["records"], json_codec::json!([]));
    assert!(value["next_cursor"].is_null());
}

#[test]
fn graph_auxiliary_wire_preserves_absent_and_empty_text() {
    for text in [None, Some(String::new())] {
        let response = HistoryGraphResponse::Text(TextPage {
            history_tree_id: "history".into(),
            history_root: "a".repeat(64),
            target: TextTarget::History {
                field: HistoryField::Provenance,
            },
            total_bytes: 0,
            offset: 0,
            text: text.clone(),
            next_offset: None,
        });
        let value: TextPage = json_codec::from_slice(response.encode().unwrap().body()).unwrap();
        assert_eq!(value.text, text);
        assert_eq!(value.total_bytes, 0);
    }
}

#[test]
fn merge_link_mutation_wire_remains_full_record() {
    let link = storage_contract::MergeLink {
        merge_link_id: "link".into(),
        history_tree_id: "history".into(),
        source_branch_id: "source".into(),
        source_commit_id: "source-commit".into(),
        target_branch_id: "target".into(),
        target_commit_id: "target-commit".into(),
        author: "owner".into(),
        operation_id: storage_contract::OperationId::generate(),
    };
    assert_eq!(
        json_codec::to_value(MergeLinkRecord::from(link.clone())).unwrap(),
        json_codec::to_value(link).unwrap()
    );
}
