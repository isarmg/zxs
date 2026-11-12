use super::*;
fn id(prefix: &str, n: u16) -> String {
    format!("{prefix}_{n:032x}")
}
fn request() -> SharedHistoryRequest {
    SharedHistoryRequest::parse(&id("file", 1), &"a".repeat(64)).unwrap()
}
fn graph() -> SharedHistoryGraph {
    let mut graph = SharedHistoryGraph {
        origin_file_id: request().file_id,
        history_tree_id: HistoryTreeId::parse(&id("history", 1)).unwrap(),
        history_root: request().history_root,
        files: Vec::new(),
        versions: Vec::new(),
        nodes: Vec::new(),
        branches: Vec::new(),
        shared_contents: Vec::new(),
    };
    for n in 1..=2 {
        let file_id = HistoryFileId::parse(&id("file", n)).unwrap();
        let version_id = HistoryVersionId::parse(&id("ver", n)).unwrap();
        graph.files.push(SharedHistoryFile {
            file_id: file_id.clone(),
            creation_name: format!("file-{n}"),
        });
        graph.versions.push(SharedHistoryVersion {
            version_id: version_id.clone(),
            file_id: file_id.clone(),
            creation_name: format!("file-{n}"),
            logical_size: 3,
            content_hash: ContentHash::from_digest(blake3::hash(b"abc")),
            content_state: Some(ContentState::Purged),
        });
        graph.nodes.push(SharedHistoryNode {
            node_id: SharedHistoryNodeId::parse(version_id.as_str()).unwrap(),
            file_id,
            kind: SharedHistoryNodeKind::Content,
            version_id,
            sequence: 1,
            parent_node_ids: Vec::new(),
        });
    }
    graph.shared_contents.push(SharedContent {
        logical_size: 3,
        content_hash: graph.versions[0].content_hash,
        version_ids: graph
            .versions
            .iter()
            .map(|v| v.version_id.clone())
            .collect(),
    });
    graph
}
#[test]
fn shared_history_contract_preserves_separate_events_and_purged_relations() {
    let graph = graph();
    graph.validate(&request()).unwrap();
    crate::validate_api_path(&request().route()).unwrap();
    let wire = json_codec::to_value(&graph).unwrap();
    assert_eq!(wire["versions"][0]["logical_size"], "3");
    assert_eq!(
        json_codec::from_value::<SharedHistoryGraph>(wire).unwrap(),
        graph
    );
    assert_ne!(graph.nodes[0].file_id, graph.nodes[1].file_id);
}
#[test]
fn shared_history_contract_rejects_missing_false_empty_and_disconnected_relations() {
    let good = graph();
    let mut bad = good.clone();
    bad.shared_contents.clear();
    assert!(bad.validate(&request()).is_err());
    let mut bad = good.clone();
    bad.shared_contents[0].version_ids.pop();
    assert!(bad.validate(&request()).is_err());
    let mut bad = good.clone();
    bad.shared_contents[0].logical_size = 0;
    assert!(bad.validate(&request()).is_err());
    let mut bad = good.clone();
    bad.versions[1].content_hash = ContentHash::from_digest(blake3::hash(b"xyz"));
    bad.shared_contents.clear();
    assert!(bad.validate(&request()).is_err());
    let mut bad = good.clone();
    bad.nodes.pop();
    assert!(bad.validate(&request()).is_err());
    let mut bad = good.clone();
    bad.history_root = HistoryRoot::parse(&"b".repeat(64)).unwrap();
    assert!(bad.validate(&request()).is_err());
    let mut bad = good;
    bad.files.push(bad.files[0].clone());
    assert!(bad.validate(&request()).is_err());
}
#[test]
fn shared_history_contract_rejects_deletion_nodes_and_keeps_ancestry_file_local() {
    let good = graph();
    good.validate(&request()).unwrap();
    let deletion = id("fdel", 1);
    assert!(SharedHistoryNodeId::parse(&deletion).is_err());
    let wire = json_codec::to_value(&good).unwrap();
    for (field, value) in [
        ("node_id", json_codec::json!(deletion)),
        ("kind", json_codec::json!("deletion")),
    ] {
        let mut bad = wire.clone();
        bad["nodes"][0][field] = value;
        assert!(json_codec::from_value::<SharedHistoryGraph>(bad).is_err());
    }
    let mut bad = wire.clone();
    bad["nodes"][0]["version_id"] = json_codec::Value::Null;
    assert!(json_codec::from_value::<SharedHistoryGraph>(bad).is_err());
    let mut bad = good.clone();
    bad.nodes[0].parent_node_ids = vec![bad.nodes[1].node_id.clone()];
    assert!(bad.validate(&request()).is_err());
    let mut bad = good;
    bad.nodes[1].parent_node_ids = vec![bad.nodes[0].node_id.clone()];
    assert!(bad.validate(&request()).is_err());
}
#[test]
fn shared_history_contract_rejects_cycles_and_oversized_complete_claims() {
    let mut graph = graph();
    let parent = graph.nodes[0].node_id.clone();
    let node_id = SharedHistoryNodeId::parse(&id("fmeta", 1)).unwrap();
    graph.nodes[0].parent_node_ids.push(node_id.clone());
    graph.nodes.push(SharedHistoryNode {
        node_id,
        file_id: graph.files[0].file_id.clone(),
        kind: SharedHistoryNodeKind::Merge,
        version_id: graph.versions[0].version_id.clone(),
        sequence: 2,
        parent_node_ids: vec![parent],
    });
    graph.nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    assert!(graph.validate(&request()).is_err());
    graph.nodes.clear();
    graph.nodes.resize(
        MAX_SHARED_HISTORY_NODES + 1,
        SharedHistoryNode {
            node_id: SharedHistoryNodeId::parse(&id("ver", 1)).unwrap(),
            file_id: graph.files[0].file_id.clone(),
            kind: SharedHistoryNodeKind::Content,
            version_id: graph.versions[0].version_id.clone(),
            sequence: 1,
            parent_node_ids: vec![],
        },
    );
    assert!(graph.validate(&request()).is_err());
}
#[test]
fn shared_history_contract_requires_nullable_fields_and_strict_kinds() {
    let mut wire = json_codec::to_value(graph()).unwrap();
    wire["versions"][0]
        .as_object_mut()
        .unwrap()
        .remove("content_state");
    assert!(json_codec::from_value::<SharedHistoryGraph>(wire).is_err());
    let mut wire = json_codec::to_value(graph()).unwrap();
    wire["nodes"][0]
        .as_object_mut()
        .unwrap()
        .remove("version_id");
    assert!(json_codec::from_value::<SharedHistoryGraph>(wire).is_err());
    let mut wire = json_codec::to_value(graph()).unwrap();
    wire["nodes"][0]["kind"] = json_codec::json!({"content":null});
    assert!(json_codec::from_value::<SharedHistoryGraph>(wire).is_err());
    let mut wire = json_codec::to_value(graph()).unwrap();
    wire["automatic_file_identity_merge"] = true.into();
    assert!(json_codec::from_value::<SharedHistoryGraph>(wire).is_err());
}

#[test]
fn shared_history_contract_preserves_valid_aliases_and_rejects_cycles_or_false_active_sources() {
    let mut graph = graph();
    for n in 1..=2 {
        graph.branches.push(SharedHistoryBranch {
            branch_id: HistoryBranchId::parse(&id("br", n)).unwrap(),
            file_id: graph.files[0].file_id.clone(),
            name: format!("branch-{n}"),
            head_node_id: graph.nodes[0].node_id.clone(),
            state: if n == 1 {
                BranchState::Merged
            } else {
                BranchState::Active
            },
            merged_into: (n == 1).then(|| HistoryBranchId::parse(&id("br", 2)).unwrap()),
        });
    }
    graph.validate(&request()).unwrap();
    let mut bad = graph.clone();
    bad.branches[0].state = BranchState::Active;
    assert!(bad.validate(&request()).is_err());
    let mut bad = graph.clone();
    bad.branches[1].state = BranchState::Archived;
    assert!(bad.validate(&request()).is_err());
    let mut bad = graph;
    bad.branches[1].state = BranchState::Merged;
    bad.branches[1].merged_into = Some(bad.branches[0].branch_id.clone());
    assert!(bad.validate(&request()).is_err());
}

// Response fixtures exercise only the production contract validator. They are
// not a node-removal transaction or a proof that a disk record was deleted.
fn retained_version_fixture(graph: &mut SharedHistoryGraph, number: u16, owner: u16, bytes: &[u8]) {
    let version_id = HistoryVersionId::parse(&id("ver", number)).unwrap();
    let file_id = HistoryFileId::parse(&id("file", owner)).unwrap();
    graph.versions.push(SharedHistoryVersion {
        version_id: version_id.clone(),
        file_id: file_id.clone(),
        creation_name: format!("file-{owner}"),
        logical_size: bytes.len() as u64,
        content_hash: ContentHash::from_digest(blake3::hash(bytes)),
        content_state: Some(ContentState::Available),
    });
    graph.nodes.push(SharedHistoryNode {
        node_id: SharedHistoryNodeId::parse(version_id.as_str()).unwrap(),
        file_id,
        kind: SharedHistoryNodeKind::Content,
        version_id,
        sequence: u64::from(number),
        parent_node_ids: Vec::new(),
    });
    graph
        .versions
        .sort_by(|a, b| a.version_id.cmp(&b.version_id));
    graph.nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
}
fn shared_group_fixture(bytes: &[u8], versions: &[u16]) -> SharedContent {
    SharedContent {
        logical_size: bytes.len() as u64,
        content_hash: ContentHash::from_digest(blake3::hash(bytes)),
        version_ids: versions
            .iter()
            .map(|number| HistoryVersionId::parse(&id("ver", *number)).unwrap())
            .collect(),
    }
}
fn remove_version_fixture(graph: &mut SharedHistoryGraph, number: u16) {
    let version = id("ver", number);
    graph
        .versions
        .retain(|row| row.version_id.as_str() != version);
    graph.nodes.retain(|row| row.node_id.as_str() != version);
}
fn order_shared_group_fixtures(graph: &mut SharedHistoryGraph) {
    graph
        .shared_contents
        .sort_by_key(|group| (group.content_hash, group.logical_size));
}

#[test]
fn shared_history_contract_keeps_parallel_links_until_the_final_cross_file_key_is_absent() {
    let mut retained = graph();
    retained_version_fixture(&mut retained, 3, 1, b"def");
    retained_version_fixture(&mut retained, 4, 2, b"def");
    retained_version_fixture(&mut retained, 5, 1, b"only-left");
    retained_version_fixture(&mut retained, 6, 2, b"only-right");
    retained
        .shared_contents
        .push(shared_group_fixture(b"def", &[3, 4]));
    order_shared_group_fixtures(&mut retained);
    retained.validate(&request()).unwrap();

    let mut one_connection = retained;
    remove_version_fixture(&mut one_connection, 1);
    one_connection.shared_contents = vec![shared_group_fixture(b"def", &[3, 4])];
    one_connection.validate(&request()).unwrap();
    assert_eq!(one_connection.files.len(), 2);

    remove_version_fixture(&mut one_connection, 3);
    one_connection.shared_contents.clear();
    assert!(one_connection.validate(&request()).is_err());
    // Only the residual response shape is checked here; publishing either
    // component still requires an authoritative removal/replay protocol.
    for owner in [1, 2] {
        let file = HistoryFileId::parse(&id("file", owner)).unwrap();
        let mut separate = one_connection.clone();
        separate.origin_file_id = file.clone();
        separate.files.retain(|row| row.file_id == file);
        separate.versions.retain(|row| row.file_id == file);
        separate.nodes.retain(|row| row.file_id == file);
        let request = SharedHistoryRequest::parse(file.as_str(), &"a".repeat(64)).unwrap();
        separate.validate(&request).unwrap();
    }
}

#[test]
fn shared_history_contract_removing_file_local_metadata_keeps_shared_membership() {
    let mut retained = graph();
    let metadata = SharedHistoryNodeId::parse(&id("fmeta", 1)).unwrap();
    retained.nodes.push(SharedHistoryNode {
        node_id: metadata.clone(),
        file_id: retained.files[0].file_id.clone(),
        kind: SharedHistoryNodeKind::Merge,
        version_id: retained.versions[0].version_id.clone(),
        sequence: 3,
        parent_node_ids: vec![retained.nodes[0].node_id.clone()],
    });
    retained.nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    retained.validate(&request()).unwrap();
    let files = retained.files.clone();
    let shared = retained.shared_contents.clone();
    retained.nodes.retain(|row| row.node_id != metadata);
    retained.validate(&request()).unwrap();
    assert_eq!(retained.files, files);
    assert_eq!(retained.shared_contents, shared);
}

#[test]
fn shared_history_contract_preserves_the_alternative_path_after_one_cycle_link_is_absent() {
    let mut retained = graph();
    retained.files.push(SharedHistoryFile {
        file_id: HistoryFileId::parse(&id("file", 3)).unwrap(),
        creation_name: "file-3".into(),
    });
    retained_version_fixture(&mut retained, 3, 2, b"def");
    retained_version_fixture(&mut retained, 4, 3, b"def");
    retained_version_fixture(&mut retained, 5, 3, b"ghi");
    retained_version_fixture(&mut retained, 6, 1, b"ghi");
    retained
        .shared_contents
        .push(shared_group_fixture(b"def", &[3, 4]));
    retained
        .shared_contents
        .push(shared_group_fixture(b"ghi", &[5, 6]));
    order_shared_group_fixtures(&mut retained);
    retained.validate(&request()).unwrap();
    remove_version_fixture(&mut retained, 1);
    retained.shared_contents = vec![
        shared_group_fixture(b"def", &[3, 4]),
        shared_group_fixture(b"ghi", &[5, 6]),
    ];
    order_shared_group_fixtures(&mut retained);
    retained.validate(&request()).unwrap();
    assert_eq!(retained.files.len(), 3);
}
