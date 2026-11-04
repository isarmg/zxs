use super::*;

fn state() -> State {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state.files.writable = true;
    state.files.selected = Some(crate::files::test_entries::entry(json!({})));
    state.files.file_history = crate::files::history::tests::history(json!({
        "selected_branch_id":"left",
        "branches":[
            {"branch_id":"left","name":"Left","node_id":"left-new"},
            {"branch_id":"right","name":"Right","node_id":"right-new","read_only":true}
        ],
        "nodes":[
            {"node_id":"left-old","version_id":"body-left","branch_id":"left","sequence":1},
            {"node_id":"left-new","version_id":"body-left-new","branch_id":"left","sequence":2},
            {"node_id":"right-old","version_id":"body-right","branch_id":"right","sequence":3},
            {"node_id":"off-page","version_id":"body-other","branch_id":"unloaded","sequence":4}
        ]
    }));
    let mut draft = super::super::tests::draft();
    draft.selected.clear();
    draft.chosen = None;
    draft.historical = Some(Selection {
        sources: BTreeMap::new(),
        name: "Merged history".into(),
    });
    state.files.merge = Some(draft);
    state
}

#[test]
fn graph_selects_exact_old_nodes_and_keeps_current_heads_as_separate_bases() {
    let mut state = state();
    assert!(!change_source(&mut state, 1, "left-old").unwrap());
    assert!(!change_source(&mut state, 1, "right-old").unwrap());
    let draft = state.files.merge.as_mut().unwrap();
    assert!(request(draft).is_err(), "result must be explicitly chosen");
    draft.chosen = Some(ChosenNode {
        id: "left-old".into(),
        version: "body-left".into(),
    });
    let body = request(draft).unwrap();
    assert_eq!(body["inputs"][0]["source_node_id"], "left-old");
    assert_eq!(body["inputs"][0]["expected_node_id"], "left-new");
    assert_eq!(body["inputs"][1]["expected_node_id"], "right-new");
    assert_eq!(body["chosen_node_id"], "left-old");
    assert_eq!(body["name"], "Merged history");
    assert_eq!(body["confirmed"], true);
    assert!(body.get("surviving_branch_id").is_none());
    assert_eq!(draft.review_inventory().node_ids, ["left-old", "right-old"]);
}

#[test]
fn two_nodes_from_one_branch_remain_two_distinct_historical_inputs() {
    let mut state = state();
    change_source(&mut state, 1, "left-old").unwrap();
    change_source(&mut state, 1, "left-new").unwrap();
    let draft = state.files.merge.as_mut().unwrap();
    draft.chosen = Some(ChosenNode {
        id: "left-old".into(),
        version: "body-left".into(),
    });
    assert_eq!(
        request(draft).unwrap()["inputs"].as_array().unwrap().len(),
        2
    );
    assert_eq!(draft.review_inventory().branch_ids, ["left"]);
    change_source(&mut state, 1, "left-old").unwrap();
    assert_eq!(state.files.merge.as_ref().unwrap().selected.len(), 1);
    change_source(&mut state, 1, "left-new").unwrap();
    assert!(state.files.merge.as_ref().unwrap().selected.is_empty());
}

#[test]
fn unloaded_branch_is_read_before_request_and_same_root_read_needs_no_fake_head() {
    let mut state = state();
    change_source(&mut state, 1, "left-old").unwrap();
    assert!(change_source(&mut state, 1, "off-page").unwrap());
    let draft = state.files.merge.as_mut().unwrap();
    draft.chosen = Some(ChosenNode {
        id: "left-old".into(),
        version: "body-left".into(),
    });
    assert!(request(draft).is_err());
    let mut branches = draft.selected.values().cloned().collect::<Vec<_>>();
    branches.push(SelectedBranch {
        id: "unloaded".into(),
        name: "Other".into(),
        expected: "other-new".into(),
    });
    let nodes = draft
        .historical
        .as_ref()
        .unwrap()
        .sources
        .values()
        .map(|source| source.node.clone())
        .collect::<Vec<_>>();
    draft
        .resolve_review("root-one".into(), &nodes, &branches, 20)
        .unwrap();
    assert!(draft.resolved.is_none());
    assert_eq!(
        request(draft).unwrap()["inputs"][1]["expected_node_id"],
        "other-new"
    );
}

#[test]
fn historical_review_never_substitutes_a_new_body_for_a_selected_old_node() {
    let mut state = state();
    change_source(&mut state, 1, "left-old").unwrap();
    let draft = state.files.merge.as_mut().unwrap();
    let branches = draft.selected.values().cloned().collect::<Vec<_>>();
    let changed = [ChosenNode {
        id: "left-old".into(),
        version: "wrong-body".into(),
    }];
    assert!(
        draft
            .resolve_review("root-two".into(), &changed, &branches, 20)
            .is_err()
    );
    assert_eq!(draft.root(), "root-one");
}

#[test]
fn source_selection_respects_stale_panels_write_admission_and_action_budget() {
    let mut state = state();
    assert!(!change_source(&mut state, 2, "left-old").unwrap());
    assert_eq!(state.files.merge.as_ref().unwrap().input_count(), 0);
    state.files.mutation_pending = true;
    assert!(!change_source(&mut state, 1, "left-old").unwrap());
    state.files.mutation_pending = false;
    state.files.merge.as_mut().unwrap().max_entries = 1;
    change_source(&mut state, 1, "left-old").unwrap();
    assert!(change_source(&mut state, 1, "left-new").is_err());
    assert_eq!(state.files.merge.as_ref().unwrap().input_count(), 1);
    state.files.merge.as_mut().unwrap().root = "changed".into();
    assert!(change_source(&mut state, 1, "right-old").is_err());
}

#[test]
fn navigation_does_not_require_manual_node_or_branch_identifiers() {
    let source = include_str!("../../../../../src/web/src/files/merge/graph.rs");
    assert!(source.contains("data-history-merge-input"));
    assert!(source.contains("data-history-merge-result"));
    assert!(!source.contains("Exact history node ID"));
}

#[test]
fn a_missing_source_branch_is_never_guessed_from_the_current_view() {
    let mut state = state();
    state.files.file_history = crate::files::history::tests::history(json!({
        "selected_branch_id":"left",
        "branches":[{"branch_id":"left","node_id":"left-new"}],
        "nodes":[{"node_id":"unknown-old","version_id":"old-body","sequence":1}]
    }));
    assert!(change_source(&mut state, 1, "unknown-old").is_err());
    assert_eq!(state.files.merge.as_ref().unwrap().input_count(), 0);
}

#[test]
fn resolved_source_keeps_exact_clicked_facts_after_the_page_is_replaced() {
    let mut state = state();
    state.files.file_history = crate::files::history::tests::history(json!({"nodes":[]}));
    insert_resolved_source(
        &mut state,
        1,
        ChosenNode {
            id: "older-page-node".into(),
            version: "immutable-body".into(),
        },
        "Original label".into(),
        SelectedBranch {
            id: "historical-branch".into(),
            name: "Archived".into(),
            expected: "newer-head".into(),
        },
    )
    .unwrap();
    let draft = state.files.merge.as_ref().unwrap();
    let source = &draft.historical.as_ref().unwrap().sources["older-page-node"];
    assert_eq!(source.node.version, "immutable-body");
    assert_eq!(source.label, "Original label");
    assert_eq!(draft.selected["historical-branch"].expected, "newer-head");
    assert_eq!(draft.input_count(), 1);
    let before = draft.clone();
    assert!(
        insert_resolved_source(
            &mut state,
            1,
            ChosenNode {
                id: "other-node".into(),
                version: "other-body".into()
            },
            "Other".into(),
            SelectedBranch {
                id: "historical-branch".into(),
                name: "Archived".into(),
                expected: "unexpected-head".into()
            }
        )
        .is_err()
    );
    assert_eq!(
        state.files.merge.as_ref().unwrap().input_count(),
        before.input_count()
    );
}
