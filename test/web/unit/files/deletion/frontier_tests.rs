use super::*;
use crate::files::FileOperation;
use crate::files::deletion::wire;
use json_codec::json;
use std::collections::BTreeMap;

fn id(prefix: &str, number: u32) -> String {
    format!("{prefix}_{number:032x}")
}
fn node(branches: u32) -> Arc<FileDeletionNode> {
    let heads = (1..=branches)
        .map(|number| (id("br", number), id("ver", 1)))
        .collect::<BTreeMap<_, _>>();
    let node = json_codec::from_value(json!({
        "node_id": id("fdel", 1), "history_tree_id": id("history", 1),
        "file_id": id("file", 1), "repository_id": id("repo", 1), "path": "same.txt",
        "branch_heads": heads, "parent_node_ids": [id("ver", 1)],
        "operation_id": id("op", 1), "author": "alice",
        "sequence": "1", "created_at": "123"
    }))
    .unwrap();
    wire::validate_node(&node).unwrap();
    Arc::new(node)
}
fn state(node: Arc<FileDeletionNode>) -> State {
    let mut state = State::default();
    state.authenticated = true;
    state.session_epoch = 7;
    state.files.deletion.open = true;
    state.files.deletion.generation = 8;
    state.files.deletion.node = Some(Inspection::Complete(node));
    state
}
fn target(state: &State) -> Target {
    Target {
        epoch: state.session_epoch,
        generation: state.files.deletion.generation,
        node: state
            .files
            .deletion
            .node
            .as_ref()
            .unwrap()
            .complete()
            .unwrap()
            .clone(),
    }
}

#[test]
fn every_branch_is_reviewable_in_bounded_pages_without_changing_the_complete_node() {
    let original = node(201);
    let mut state = state(original.clone());
    let target = target(&state);
    assert_eq!(target.range(&state), Some(0..100));
    assert!(target.destination(&state, Turn::Previous).is_none());
    target.turn(&mut state, Turn::Previous);
    assert_eq!(target.range(&state), Some(0..100));
    target.turn(&mut state, Turn::Next);
    assert_eq!(target.range(&state), Some(100..200));
    target.turn(&mut state, Turn::Next);
    assert_eq!(target.range(&state), Some(200..201));
    assert!(target.destination(&state, Turn::Next).is_none());
    target.turn(&mut state, Turn::Next);
    assert_eq!(target.range(&state), Some(200..201));
    target.turn(&mut state, Turn::Previous);
    assert_eq!(target.range(&state), Some(100..200));
    assert!(Arc::ptr_eq(
        state
            .files
            .deletion
            .node
            .as_ref()
            .unwrap()
            .complete()
            .unwrap(),
        &original,
    ));
    assert_eq!(original.branch_heads.len(), 201);
    assert_eq!(original.parent_node_ids, vec![id("ver", 1)]);
}

#[test]
fn exact_boundary_and_partial_final_pages_have_correct_navigation_limits() {
    for (count, last) in [(1, 0..1), (100, 0..100), (101, 100..101), (200, 100..200)] {
        let mut state = state(node(count));
        let target = target(&state);
        while target.destination(&state, Turn::Next).is_some() {
            target.turn(&mut state, Turn::Next);
        }
        assert_eq!(target.range(&state), Some(last));
        assert!(target.destination(&state, Turn::Next).is_none());
    }
}

#[test]
fn stale_page_click_cannot_turn_a_different_or_reread_deletion_node() {
    let mut state = state(node(201));
    let old = target(&state);
    let mut replacement = (*node(201)).clone();
    replacement.node_id = id("fdel", 2);
    state.files.deletion.node = Some(Inspection::Complete(Arc::new(replacement)));
    old.turn(&mut state, Turn::Next);
    assert!(old.range(&state).is_none());
    assert_eq!(state.files.deletion.frontier_page, 0);
    let fresh = target(&state);
    fresh.turn(&mut state, Turn::Next);
    assert_eq!(fresh.range(&state), Some(100..200));
    state.files.deletion.node = Some(Inspection::Complete(Arc::new((*fresh.node).clone())));
    fresh.turn(&mut state, Turn::Next);
    assert_eq!(state.files.deletion.frontier_page, 1);
}

#[test]
fn session_generation_loading_and_close_invalidate_old_frontier_controls() {
    let changes: [fn(&mut State); 5] = [
        |state| state.session_epoch += 1,
        |state| state.authenticated = false,
        |state| state.files.deletion.generation += 1,
        |state| state.files.deletion.loading = true,
        |state| state.files.deletion.close(),
    ];
    for change in changes {
        let mut state = state(node(201));
        let target = target(&state);
        change(&mut state);
        target.turn(&mut state, Turn::Next);
        assert_eq!(state.files.deletion.frontier_page, 0);
        assert!(target.range(&state).is_none());
    }
}

#[test]
fn frontier_review_keeps_an_unknown_original_operation_and_write_fence() {
    let mut state = state(node(201));
    let target = target(&state);
    state
        .files
        .operations
        .push(FileOperation::unknown("original"));
    target.turn(&mut state, Turn::Next);
    assert_eq!(target.range(&state), Some(100..200));
    assert!(crate::files::actions::pending(&state));
    assert_eq!(state.files.operations[0].id(), "original");
    assert!(state.files.deletion.attempt.is_none());
}
