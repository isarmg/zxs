use super::*;
use storage_engine::{CommitRequest, Config, FileNodeTagsUpdateRequest, FileTagTarget, new_id};

fn save(
    engine: &mut Engine,
    branch: &str,
    path: &str,
    bytes: &[u8],
    message: &str,
) -> storage_contract::Entry {
    let workspace = engine.create_workspace(branch, "owner").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: message.into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .list_tree(branch)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == path)
        .unwrap()
}

fn normalized(value: &mut Value, ids: &BTreeMap<&str, &str>) {
    match value {
        Value::String(text) => {
            if let Some(label) = ids.get(text.as_str()) {
                *text = (*label).into();
            }
        }
        Value::Array(values) => {
            for value in values {
                normalized(value, ids);
            }
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(key.as_str(), "created_at" | "storage_name" | "operation_id") {
                    *value = format!("<{key}>").into();
                } else {
                    normalized(value, ids);
                }
            }
        }
        _ => {}
    }
}

#[test]
fn file_history_matches_preprojection_graph_json() {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let repo = engine
        .create_disk_root("P01", "owner", &new_id("op"))
        .unwrap();
    // Both sequence-tied initial commits carry the same message, so random
    // commit IDs cannot make the graph fixture nondeterministic.
    let first = save(
        &mut engine,
        &repo.main_branch_id,
        "before.txt",
        b"old",
        "Initialize file main branch",
    );
    let branch = engine
        .file_context(&repo.repository_id, "before.txt")
        .unwrap()
        .branch_id;
    let tagged = engine
        .update_file_node_tags(FileNodeTagsUpdateRequest {
            file_id: &first.file_id,
            targets: &[FileTagTarget {
                branch_id: branch.clone(),
                source_node_id: first.version_id.clone(),
                expected_node_id: first.version_id.clone(),
            }],
            add: &["review".into(), "草稿".into()],
            remove: &[],
            location: None,
            principal: "owner",
            operation_id: &new_id("op"),
        })
        .unwrap()
        .result["nodes"][0]["node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                "before.txt",
                Some("after.txt"),
            )
            .unwrap(),
            "owner",
            &new_id("op"),
        )
        .unwrap();
    let renamed = engine
        .file_context(&repo.repository_id, "after.txt")
        .unwrap()
        .node_id
        .unwrap();
    let second = save(
        &mut engine,
        &repo.main_branch_id,
        "after.txt",
        b"new",
        "graph fixture",
    );
    let before = json_codec::to_vec(engine.state()).unwrap();
    let mut actual = super::super::file_history(&engine, &json!({"file_id": first.file_id}))
        .unwrap()["nodes"]
        .clone();
    normalized(
        &mut actual,
        &BTreeMap::from([
            (first.file_id.as_str(), "file"),
            (first.version_id.as_str(), "body-old"),
            (second.version_id.as_str(), "body-new"),
            (tagged.as_str(), "tags"),
            (renamed.as_str(), "renamed"),
            (branch.as_str(), "branch"),
        ]),
    );
    // Captured from the pre-batch production path. Only generated identities,
    // wall-clock values and the derived storage filename are normalized.
    let expected: Value = json_codec::from_str(include_str!("history_nodes_golden.json")).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(json_codec::to_vec(engine.state()).unwrap(), before);
}

#[test]
fn summary_consumption_retains_overlapping_rows() {
    let expected = storage_engine::FileNodeSummary {
        parent_node_ids: vec!["first".into(), "first".into(), "last".into()],
        tags: BTreeSet::from(["label".into()]),
    };
    let mut summaries = BTreeMap::from([(
        "overlap".into(),
        PendingSummary {
            remaining_rows: 3,
            summary: expected.clone(),
        },
    )]);
    for _ in 0..3 {
        assert_eq!(take_summary(&mut summaries, "overlap"), expected);
    }
    assert!(summaries.is_empty());
}

#[test]
fn history_propagates_projection_error_instead_of_inventing_empty_graph() {
    let root = crate::tests::private_tempdir();
    let engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let versions = [HistoryVersion {
        id: "missing",
        sequence: 10,
        download_name: "file.txt",
        wire: json!({}),
    }];
    let error = unified_history(
        &engine,
        "file",
        "history",
        &versions,
        &BTreeMap::new(),
        None,
    )
    .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::FileNodeNotFound);
}

#[test]
fn display_order_retains_full_typed_sequences_and_id_ties() {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let repo = engine
        .create_disk_root("P01", "owner", &new_id("op"))
        .unwrap();
    let first = save(
        &mut engine,
        &repo.main_branch_id,
        "file.txt",
        b"first",
        "first",
    );
    let second = save(
        &mut engine,
        &repo.main_branch_id,
        "file.txt",
        b"second",
        "second",
    );
    let third = save(
        &mut engine,
        &repo.main_branch_id,
        "file.txt",
        b"third",
        "third",
    );
    let versions = [
        (&first.version_id, 10),
        (&third.version_id, u64::MAX),
        (&second.version_id, 10),
    ]
    .map(|(id, sequence)| HistoryVersion {
        id,
        sequence,
        download_name: "file.txt",
        wire: json!({}),
    });
    let nodes = unified_history(
        &engine,
        &first.file_id,
        &repo.history_tree_id,
        &versions,
        &BTreeMap::new(),
        Some(&first.version_id),
    )
    .unwrap();
    let mut ties = [&first.version_id, &second.version_id];
    ties.sort();
    assert_eq!(
        nodes
            .iter()
            .map(|node| node["node_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [&third.version_id, ties[0], ties[1]]
    );
    assert_eq!(nodes[0]["sequence"], u64::MAX.to_string());
    for node in nodes {
        assert_eq!(node["current"], node["node_id"] == first.version_id);
    }
}
