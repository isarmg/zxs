use super::*;

fn path(state: &mut State, id: &str, parents: &[&str]) {
    state.file_path_nodes.insert(
        id.into(),
        FilePathNode {
            node_id: id.into(),
            history_tree_id: "history".into(),
            file_id: "file".into(),
            version_id: "body".into(),
            parent_node_ids: parents.iter().map(|p| (*p).into()).collect(),
            branch_id: "branch".into(),
            path: "new".into(),
            previous_path: "old".into(),
            operation_id: "operation".into(),
            author: "author".into(),
            sequence: 1,
            created_at: 1,
        },
    );
}

fn metadata(state: &mut State, id: &str, parents: &[&str], tags: &[&str]) {
    state.file_metadata_nodes.insert(
        id.into(),
        FileMetadataNode {
            node_id: id.into(),
            history_tree_id: "history".into(),
            file_id: "file".into(),
            version_id: "body".into(),
            parent_node_ids: parents.iter().map(|p| (*p).into()).collect(),
            branch_id: "branch".into(),
            kind: "tags".into(),
            tags: tags.iter().map(|t| (*t).into()).collect(),
            operation_id: "operation".into(),
            author: "author".into(),
            sequence: 1,
            created_at: 1,
        },
    );
}

fn merge(state: &mut State, id: &str, result: &str, parents: &[&str]) {
    state.file_branch_merges.insert(
        id.into(),
        FileBranchMerge {
            merge_id: id.into(),
            history_tree_id: "history".into(),
            file_id: "file".into(),
            input_branch_ids: Vec::new(),
            input_node_ids: Vec::new(),
            chosen_node_id: result.into(),
            result_node_id: result.into(),
            surviving_branch_id: "branch".into(),
            reused_node: true,
            parent_node_ids: parents.iter().map(|p| (*p).into()).collect(),
            operation_id: "operation".into(),
            sequence: 1,
        },
    );
}

fn body(state: &mut State, id: &str, file: &str) {
    state.versions.insert(
        id.into(),
        FileVersion {
            history_tree_id: "history".into(),
            version_id: id.into(),
            file_id: file.into(),
            creation_primary_slot_id: "P01".into(),
            logical_size: 0,
            version_sequence: 1,
            content_hash: "0".repeat(64),
            checksum_chunk_bytes: 262144,
            checksum_manifest_hash: "0".repeat(64),
            creation_name: "body".into(),
            storage_name: "v000001_20261020T000000Z_00000000.txt".into(),
            created_at: "2026-10-20T00:00:00Z".into(),
            source_version_ids: Vec::new(),
            created_as_full_checkpoint: true,
        },
    );
}

fn engine_with_state(state: State) -> (tempfile::TempDir, Engine) {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("disk")]),
    )
    .unwrap();
    engine.state = std::sync::Arc::new(state);
    (root, engine)
}

#[test]
fn projection_matches_uncached_nodes_and_serialized_bytes() {
    let mut state = State::default();
    metadata(&mut state, "tagged", &["tagged"], &["z", "a"]);
    metadata(&mut state, "empty", &["tagged"], &[]);
    path(&mut state, "tagged", &["missing"]); // metadata wins over a path ID
    path(&mut state, "left", &["tagged"]);
    path(&mut state, "right", &["empty"]);
    path(&mut state, "merged", &["left", "left", "right"]);
    path(&mut state, "cycle-a", &["cycle-b"]);
    path(&mut state, "cycle-b", &["cycle-a"]);
    path(&mut state, "cycle-tail", &["cycle-a", "tagged"]);
    path(&mut state, "dangling", &["missing"]);
    // Reverse insertion order must not change BTreeMap merge order.
    merge(&mut state, "z", "merged", &["other", "left", "merged"]);
    merge(&mut state, "a", "merged", &["right", "first", "first"]);
    merge(&mut state, "unrelated", "elsewhere", &["tagged"]);
    for (id, parents) in [
        ("body", vec!["left"]),
        ("source-only", vec!["left"]),
        ("override", vec!["right"]),
        ("empty-override", vec!["left"]),
    ] {
        state.versions.insert(
            id.into(),
            FileVersion {
                history_tree_id: "history".into(),
                version_id: id.into(),
                file_id: "file".into(),
                creation_primary_slot_id: "P01".into(),
                logical_size: 0,
                version_sequence: 1,
                content_hash: "0".repeat(64),
                checksum_chunk_bytes: 262144,
                checksum_manifest_hash: "0".repeat(64),
                creation_name: "body".into(),
                storage_name: "v000001_20261020T000000Z_00000000.txt".into(),
                created_at: "2026-10-20T00:00:00Z".into(),
                source_version_ids: parents.into_iter().map(str::to_owned).collect(),
                created_as_full_checkpoint: true,
            },
        );
    }
    state
        .file_node_parents
        .insert("override".into(), vec!["left".into()]);
    state
        .file_node_parents
        .insert("empty-override".into(), Vec::new());
    // A path node takes precedence over a body with the same ID.
    path(&mut state, "body", &["right"]);
    let nodes = [
        "body",
        "source-only",
        "override",
        "empty-override",
        "merged",
        "right",
        "left",
        "tagged",
        "empty",
        "cycle-tail",
        "cycle-b",
        "cycle-a",
        "dangling",
        "missing",
    ];
    let mut projection = NodeProjection::new(&state);
    for node in nodes.into_iter().chain(nodes.into_iter().rev()) {
        let expected = json_codec::json!({"parent_node_ids": node_parents(&state, node), "tags": node_tags(&state, node)});
        let actual = json_codec::json!({"parent_node_ids": projection.parents(node), "tags": projection.tags(node)});
        assert_eq!(
            json_codec::to_vec(&actual).unwrap(),
            json_codec::to_vec(&expected).unwrap(),
            "{node}"
        );
    }
    assert_eq!(
        projection.parents("merged"),
        ["left", "left", "right", "first", "other"]
    );
}

#[test]
fn projection_walks_a_linear_history_once_in_either_query_order() {
    const LENGTH: usize = 4096;
    let mut state = State::default();
    metadata(&mut state, "0", &[], &["tag"]);
    for n in 1..LENGTH {
        path(&mut state, &n.to_string(), &[&(n - 1).to_string()]);
    }
    for reverse in [false, true] {
        let mut projection = NodeProjection::new(&state);
        for n in 0..LENGTH {
            let id = if reverse { LENGTH - n - 1 } else { n }.to_string();
            assert_eq!(projection.tags(&id), &BTreeSet::from(["tag".into()]));
        }
        assert_eq!(projection.tag_visits, LENGTH);
        for n in 0..LENGTH {
            projection.tags(&n.to_string());
        }
        assert_eq!(
            projection.tag_visits, LENGTH,
            "replica lookups must not traverse again"
        );
    }
}

#[test]
fn batch_matches_public_single_node_results_including_partial_errors_and_scope() {
    let mut state = State::default();
    body(&mut state, "body", "file");
    body(&mut state, "foreign-body", "foreign");
    metadata(&mut state, "tags", &[], &["original"]);
    path(&mut state, "child", &["tags"]);
    path(&mut state, "cycle-a", &["cycle-b"]);
    path(&mut state, "cycle-b", &["cycle-a"]);
    path(&mut state, "dangling", &["absent"]);
    path(&mut state, "missing-body", &[]);
    state
        .file_path_nodes
        .get_mut("missing-body")
        .unwrap()
        .version_id = "absent".into();
    path(&mut state, "wrong-body", &[]);
    state
        .file_path_nodes
        .get_mut("wrong-body")
        .unwrap()
        .version_id = "foreign-body".into();
    path(&mut state, "foreign-node", &[]);
    state
        .file_path_nodes
        .get_mut("foreign-node")
        .unwrap()
        .file_id = "foreign".into();
    path(&mut state, "other-history", &["tags"]);
    state
        .file_path_nodes
        .get_mut("other-history")
        .unwrap()
        .history_tree_id = "other".into();
    // Public identity checks prefer the body, but labels and parents prefer
    // metadata. The batch must not unify those distinct precedence rules.
    body(&mut state, "overlap", "file");
    path(&mut state, "overlap", &["dangling"]);
    metadata(&mut state, "overlap", &["child", "child"], &["overlap"]);
    state
        .file_metadata_nodes
        .get_mut("overlap")
        .unwrap()
        .file_id = "foreign".into();
    path(&mut state, "foreign-parent", &["overlap"]);
    merge(&mut state, "a", "child", &["extra", "tags"]);
    state.file_branch_merges.get_mut("a").unwrap().file_id = "foreign".into();
    state
        .file_branch_merges
        .get_mut("a")
        .unwrap()
        .history_tree_id = "other".into();
    let (_root, engine) = engine_with_state(state);
    let ids = [
        "body",
        "tags",
        "child",
        "cycle-a",
        "cycle-b",
        "dangling",
        "missing-body",
        "wrong-body",
        "foreign-node",
        "foreign-body",
        "other-history",
        "overlap",
        "foreign-parent",
        "absent",
        "child",
    ];
    let summaries = engine.project_file_nodes(FileNodeProjectionRequest {
        file_id: "file",
        node_ids: &ids,
    });
    assert_eq!(
        summaries.len(),
        ids.len() - 1,
        "repeated requests share one result"
    );
    for id in ids {
        let expected = engine
            .file_node_parents("file", id)
            .and_then(|parent_node_ids| {
                Ok(FileNodeSummary {
                    parent_node_ids,
                    tags: engine.file_node_tags("file", id)?,
                })
            })
            .map_err(|error| error.code().to_owned());
        let actual = summaries[id]
            .as_ref()
            .map(Clone::clone)
            .map_err(|error| error.code().to_owned());
        assert_eq!(actual, expected, "{id}");
    }
    for id in ["missing-body", "wrong-body"] {
        assert_eq!(summaries[id].as_ref().unwrap_err().code(), "STATE_INVALID");
    }
    for id in ["foreign-node", "foreign-body", "absent"] {
        assert_eq!(
            summaries[id].as_ref().unwrap_err().code(),
            "FILE_NODE_NOT_FOUND"
        );
    }
    assert_eq!(
        summaries["overlap"].as_ref().unwrap().parent_node_ids,
        ["child", "child"]
    );
    assert_eq!(
        summaries["foreign-parent"].as_ref().unwrap().tags,
        BTreeSet::from(["overlap".into()])
    );
    assert_eq!(
        summaries["child"].as_ref().unwrap().parent_node_ids,
        ["tags", "extra"]
    );
    assert!(
        summaries["other-history"].is_ok(),
        "history visibility remains the caller's responsibility"
    );
}

#[test]
fn batch_counts_one_merge_scan_and_one_tag_walk_without_cross_request_cache() {
    const NODES: usize = 1024;
    const MERGES: usize = 257;
    let mut state = State::default();
    body(&mut state, "body", "file");
    metadata(&mut state, "0", &[], &["old"]);
    for n in 1..NODES {
        path(&mut state, &n.to_string(), &[&(n - 1).to_string()]);
    }
    for n in 0..MERGES {
        merge(
            &mut state,
            &format!("merge-{n}"),
            if n == 0 { "1023" } else { "unrelated" },
            &["0"],
        );
    }
    let (_root, mut engine) = engine_with_state(state);
    for reverse in [false, true] {
        let ids = (0..NODES)
            .map(|n| if reverse { NODES - n - 1 } else { n }.to_string())
            .collect::<Vec<_>>();
        let ids = ids.iter().map(String::as_str).collect::<Vec<_>>();
        MERGE_EVENT_VISITS.set(0);
        TAG_NODE_VISITS.set(0);
        let expected = ids
            .iter()
            .map(|id| {
                (
                    (*id).to_owned(),
                    FileNodeSummary {
                        parent_node_ids: engine.file_node_parents("file", id).unwrap(),
                        tags: engine.file_node_tags("file", id).unwrap(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(MERGE_EVENT_VISITS.get(), NODES * MERGES);
        assert_eq!(TAG_NODE_VISITS.get(), NODES * (NODES + 1) / 2);
        MERGE_EVENT_VISITS.set(0);
        TAG_NODE_VISITS.set(0);
        let actual = engine.project_file_nodes(FileNodeProjectionRequest {
            file_id: "file",
            node_ids: &ids,
        });
        assert_eq!(MERGE_EVENT_VISITS.get(), MERGES);
        assert_eq!(TAG_NODE_VISITS.get(), NODES);
        for (id, actual) in actual {
            assert_eq!(actual.unwrap(), expected[&id], "{id}");
        }
    }
    std::sync::Arc::make_mut(&mut engine.state)
        .file_metadata_nodes
        .get_mut("0")
        .unwrap()
        .tags = BTreeSet::from(["new".into()]);
    let fresh = engine.project_file_nodes(FileNodeProjectionRequest {
        file_id: "file",
        node_ids: &["1023"],
    });
    assert_eq!(
        fresh["1023"].as_ref().unwrap().tags,
        BTreeSet::from(["new".into()])
    );
}
