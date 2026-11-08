use super::*;
use storage_contract::history_window::{HistoryTreeId, HistoryVersionId};
use storage_contract::shared_history::{
    SharedContent, SharedHistoryFile, SharedHistoryNode, SharedHistoryNodeId,
    SharedHistoryNodeKind, SharedHistoryVersion,
};
fn id(prefix: &str, n: u8) -> String {
    format!("{prefix}_{n:032x}")
}
fn file(n: u8) -> HistoryFileId {
    HistoryFileId::parse(&id("file", n)).unwrap()
}
fn version(n: u8) -> HistoryVersionId {
    HistoryVersionId::parse(&id("ver", n)).unwrap()
}
fn node(prefix: &str, n: u8) -> SharedHistoryNodeId {
    SharedHistoryNodeId::parse(&id(prefix, n)).unwrap()
}
fn target() -> Target {
    Target {
        epoch: 1,
        file: id("file", 1),
        entry: id("entry", 1),
        path: "a".into(),
        root: "a".repeat(64),
    }
}
fn graph() -> SharedHistoryGraph {
    let request = SharedHistoryRequest::parse(&id("file", 1), &"a".repeat(64)).unwrap();
    let versions = [(1, 1, "a"), (2, 2, "b"), (3, 1, "a")]
        .into_iter()
        .map(|(v, f, name)| SharedHistoryVersion {
            version_id: version(v),
            file_id: file(f),
            creation_name: name.into(),
            logical_size: 3,
            content_hash: "b".repeat(64).parse().unwrap(),
            content_state: Some(storage_contract::ContentState::Purged),
        })
        .collect::<Vec<_>>();
    let mut nodes = versions
        .iter()
        .enumerate()
        .map(|(index, v)| SharedHistoryNode {
            node_id: SharedHistoryNodeId::parse(v.version_id.as_str()).unwrap(),
            file_id: v.file_id.clone(),
            kind: SharedHistoryNodeKind::Content,
            version_id: Some(v.version_id.clone()),
            sequence: index as u64 + 1,
            parent_node_ids: if index == 2 {
                vec![node("ver", 1)]
            } else {
                vec![]
            },
        })
        .collect::<Vec<_>>();
    nodes.push(SharedHistoryNode {
        node_id: node("fdel", 4),
        file_id: file(2),
        kind: SharedHistoryNodeKind::Deletion,
        version_id: None,
        sequence: 4,
        parent_node_ids: vec![node("ver", 2)],
    });
    nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    let graph = SharedHistoryGraph {
        origin_file_id: request.file_id.clone(),
        history_tree_id: HistoryTreeId::parse(&id("history", 1)).unwrap(),
        history_root: request.history_root,
        files: vec![
            SharedHistoryFile {
                file_id: file(1),
                creation_name: "a".into(),
            },
            SharedHistoryFile {
                file_id: file(2),
                creation_name: "b".into(),
            },
        ],
        versions,
        nodes,
        branches: vec![],
        shared_contents: vec![SharedContent {
            logical_size: 3,
            content_hash: "b".repeat(64).parse().unwrap(),
            version_ids: vec![version(1), version(2), version(3)],
        }],
    };
    graph.validate(&request).unwrap();
    graph
}
#[test]
fn shared_history_panel_keeps_same_root_selection_and_original_request_identity() {
    let mut model = Model::default();
    model.set_target(Some(target()));
    model.receive(graph());
    model.select(&id("file", 2));
    let before = model.request().unwrap();
    let generation = model.generation;
    model.set_target(Some(target()));
    assert_eq!(model.selection, Selection::File(file(2)));
    assert_eq!(model.request().unwrap(), before);
    assert_eq!(before.file_id, file(1));
    assert_eq!(model.generation, generation);
    model.select(&id("file", 99));
    assert_eq!(model.selection, Selection::File(file(2)));
    model.select("all");
    assert_eq!(model.selection, Selection::All);
    assert_eq!(model.request().unwrap(), before);
}
#[test]
fn shared_history_panel_close_root_and_session_changes_reject_old_completions() {
    let mut model = Model::default();
    let original = target();
    model.set_target(Some(original.clone()));
    model.receive(graph());
    let generation = model.generation;
    model.close();
    assert!(!model.current(&original, generation));
    assert!(model.graph.is_none());
    model.receive(graph());
    let mut changed = original.clone();
    changed.root = "b".repeat(64);
    model.set_target(Some(changed));
    assert!(model.graph.is_none());
    model.receive(graph());
    let mut changed = original.clone();
    changed.epoch += 1;
    model.set_target(Some(changed));
    assert!(model.graph.is_none());
    assert_eq!(model.selection, Selection::All);
}
#[test]
fn shared_history_projection_keeps_repeated_content_events_and_bodyless_deletion() {
    let graph = graph();
    let projected = project(&graph, &Selection::All);
    assert_eq!(projected.cards.len(), 5);
    assert_eq!(projected.records.len(), 5);
    let hubs = projected
        .cards
        .iter()
        .filter(|(_, card)| card.shared)
        .collect::<Vec<_>>();
    assert_eq!(hubs.len(), 1);
    let hub = hubs[0].0;
    for n in [1, 2, 3] {
        let record = projected
            .records
            .iter()
            .find(|record| record.0 == id("ver", n))
            .unwrap();
        assert!(record.2.contains(hub));
    }
    let repeated = projected
        .records
        .iter()
        .find(|record| record.0 == id("ver", 3))
        .unwrap();
    assert!(repeated.2.contains(&id("ver", 1)));
    let deletion = projected
        .records
        .iter()
        .find(|record| record.0 == id("fdel", 4))
        .unwrap();
    assert_eq!(deletion.2, vec![id("ver", 2)]);
    assert!(!projected.cards[&deletion.0].shared);
    assert!(
        !projected.cards[&deletion.0]
            .lines
            .iter()
            .any(|line| line.contains("PURGED"))
    );
    assert!(
        projected
            .records
            .iter()
            .find(|record| &record.0 == hub)
            .unwrap()
            .2
            .is_empty()
    );
}
#[test]
fn shared_history_file_filter_is_local_and_preserves_other_file_provenance() {
    let graph = graph();
    let original = graph.clone();
    let mine = project(&graph, &Selection::File(file(1)));
    assert!(mine.cards.contains_key(&id("ver", 1)));
    assert!(mine.cards.contains_key(&id("ver", 3)));
    assert!(!mine.cards.contains_key(&id("ver", 2)));
    assert!(!mine.cards.contains_key(&id("fdel", 4)));
    assert!(
        mine.cards
            .values()
            .find(|card| card.shared)
            .unwrap()
            .lines
            .iter()
            .any(|line| line.contains(&id("file", 2)))
    );
    let other = project(&graph, &Selection::File(file(2)));
    assert!(other.cards.contains_key(&id("ver", 2)));
    assert!(other.cards.contains_key(&id("fdel", 4)));
    assert!(!other.cards.contains_key(&id("ver", 1)));
    assert_eq!(graph, original);
}
#[test]
fn shared_history_decode_rejects_crossed_identity_unknown_fields_and_missing_edges() {
    let request = SharedHistoryRequest::parse(&id("file", 1), &"a".repeat(64)).unwrap();
    assert!(endpoint(&request).contains("/shared-history?history_root="));
    let wire = json_codec::to_value(graph()).unwrap();
    assert!(decode(wire.clone(), &request).is_ok());
    for (field, value) in [
        ("origin_file_id", id("file", 2)),
        ("history_root", "b".repeat(64)),
        ("write_target", id("file", 2)),
    ] {
        let mut bad = wire.clone();
        bad[field] = value.into();
        assert!(decode(bad, &request).is_err());
    }
    let mut bad = wire;
    bad["shared_contents"] = json_codec::json!([]);
    assert!(decode(bad, &request).is_err());
}

#[test]
fn shared_history_branch_labels_keep_state_and_merge_destination_read_only() {
    use storage_contract::BranchState;
    use storage_contract::history_window::HistoryBranchId;
    use storage_contract::shared_history::SharedHistoryBranch;
    use web_interface::i18n::t;

    let mut graph = graph();
    let active = HistoryBranchId::parse(&id("br", 1)).unwrap();
    for (index, state) in [
        BranchState::Active,
        BranchState::Archived,
        BranchState::Migrated,
        BranchState::Merged,
    ]
    .into_iter()
    .enumerate()
    {
        graph.branches.push(SharedHistoryBranch {
            branch_id: HistoryBranchId::parse(&id("br", index as u8 + 1)).unwrap(),
            file_id: file(1),
            name: format!("branch-{}", index + 1),
            head_node_id: node("ver", 3),
            state,
            merged_into: (state == BranchState::Merged).then(|| active.clone()),
        });
    }
    let request = SharedHistoryRequest::parse(&id("file", 1), &"a".repeat(64)).unwrap();
    graph.validate(&request).unwrap();
    let original = graph.clone();
    let projection = project(&graph, &Selection::File(file(1)));
    let lines = &projection.cards[&id("ver", 3)].lines;
    for (index, label) in [
        t("活跃", "Active"),
        t("已归档", "Archived"),
        t("已迁出", "Migrated"),
        t("已合并", "Merged"),
    ]
    .iter()
    .enumerate()
    {
        assert!(
            lines
                .iter()
                .any(|line| { line.contains(&id("br", index as u8 + 1)) && line.ends_with(label) })
        );
    }
    assert!(lines.iter().any(|line| line
        == &format!(
            "{}: {} → {}",
            t(
                "分支合并归属（非节点祖先边）",
                "Branch merge destination (not event ancestry)"
            ),
            id("br", 4),
            active
        )));
    assert_eq!(graph, original);
    let mut without_branches = graph.clone();
    without_branches.branches.clear();
    assert_eq!(
        projection.records,
        project(&without_branches, &Selection::File(file(1))).records
    );
}
