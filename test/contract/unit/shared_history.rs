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
            version_id: Some(version_id),
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
fn shared_history_contract_keeps_deletion_bodyless_and_ancestry_file_local() {
    let mut good = graph();
    good.nodes.push(SharedHistoryNode {
        node_id: SharedHistoryNodeId::parse(&id("fdel", 1)).unwrap(),
        file_id: good.files[0].file_id.clone(),
        kind: SharedHistoryNodeKind::Deletion,
        version_id: None,
        sequence: 2,
        parent_node_ids: vec![good.nodes[0].node_id.clone()],
    });
    good.nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    good.validate(&request()).unwrap();
    let mut bad = good.clone();
    bad.nodes[0].version_id = Some(bad.versions[0].version_id.clone());
    assert!(bad.validate(&request()).is_err());
    let mut bad = good.clone();
    bad.nodes[0].parent_node_ids = vec![bad.nodes[2].node_id.clone()];
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
        kind: SharedHistoryNodeKind::Tags,
        version_id: Some(graph.versions[0].version_id.clone()),
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
            version_id: Some(graph.versions[0].version_id.clone()),
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
