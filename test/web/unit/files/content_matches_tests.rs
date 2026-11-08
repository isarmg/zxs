use super::*;
use storage_contract::history_window::*;
fn id(prefix: &str, n: u8) -> String {
    format!("{prefix}_{n:032x}")
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
fn page(request: &ContentMatchRequest) -> ContentMatchPage {
    ContentMatchPage {
        file_id: request.file_id.clone(),
        source_node_id: request.node_id.clone(),
        source_version_id: HistoryVersionId::parse(&id("ver", 1)).unwrap(),
        history_tree_id: HistoryTreeId::parse(&id("history", 1)).unwrap(),
        history_root: request.history_root,
        logical_size: 3,
        content_hash: "b".repeat(64).parse().unwrap(),
        source_content_state: Some(storage_contract::ContentState::Available),
        matches: vec![],
        next_after_version_id: None,
    }
}
#[test]
fn match_panel_preserves_independent_selection_for_same_history_window() {
    let mut model = Model::default();
    let target = target();
    model.set_target(
        Some(target.clone()),
        Some(HistoryNodeId::parse(&id("ver", 1)).unwrap()),
    );
    let request = model.request(false).unwrap();
    model.page = Some(page(&request));
    let generation = model.generation;
    model.set_target(
        Some(target.clone()),
        Some(HistoryNodeId::parse(&id("ver", 2)).unwrap()),
    );
    assert_eq!(model.node.as_ref().unwrap().as_str(), id("ver", 1));
    assert_eq!(model.generation, generation);
    assert!(model.page.is_some());
    assert!(model.current(&target, &id("ver", 1), generation));
}
#[test]
fn match_panel_new_source_and_dismissal_reject_obsolete_completions() {
    let mut model = Model::default();
    let target = target();
    model.set_target(
        Some(target.clone()),
        Some(HistoryNodeId::parse(&id("ver", 1)).unwrap()),
    );
    let generation = model.generation;
    model.select_node(HistoryNodeId::parse(&id("ver", 2)).unwrap());
    assert!(!model.current(&target, &id("ver", 1), generation));
    let generation = model.generation;
    model.close();
    assert!(!model.current(&target, &id("ver", 2), generation));
    assert!(!model.loading);
}
#[test]
fn match_panel_changed_root_or_session_clears_previous_results() {
    let mut model = Model::default();
    let target = target();
    model.set_target(
        Some(target.clone()),
        Some(HistoryNodeId::parse(&id("ver", 1)).unwrap()),
    );
    model.page = Some(page(&model.request(false).unwrap()));
    let generation = model.generation;
    let mut changed = target.clone();
    changed.root = "b".repeat(64);
    model.set_target(
        Some(changed),
        Some(HistoryNodeId::parse(&id("ver", 2)).unwrap()),
    );
    assert!(!model.current(&target, &id("ver", 1), generation));
    assert!(model.page.is_none());
    let mut changed = target.clone();
    changed.epoch += 1;
    model.set_target(
        Some(changed),
        Some(HistoryNodeId::parse(&id("ver", 1)).unwrap()),
    );
    assert!(!model.current(&target, &id("ver", 1), model.generation));
}
#[test]
fn match_panel_exact_route_and_strict_decode_do_not_accept_crossed_nodes() {
    let mut model = Model::default();
    model.set_target(
        Some(target()),
        Some(HistoryNodeId::parse(&id("ver", 1)).unwrap()),
    );
    let request = model.request(false).unwrap();
    assert!(endpoint(&request).contains("/content-matches?"));
    assert!(model.request(true).is_none());
    let value = json_codec::to_value(page(&request)).unwrap();
    assert!(decode(value.clone(), &request).is_ok());
    for (field, value) in [
        ("file_id", id("file", 2)),
        ("source_node_id", id("ver", 2)),
        ("history_root", "b".repeat(64)),
    ] {
        let mut crossed = json_codec::to_value(page(&request)).unwrap();
        crossed[field] = value.into();
        assert!(decode(crossed, &request).is_err());
    }
}
