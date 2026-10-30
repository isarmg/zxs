use super::*;
use crate::files::history::tests::{branch_wire, history, update_metadata};
use json_codec::json;

#[test]
fn indexed_records_borrow_only_the_loaded_window_and_body_notes() {
    let snapshot = history(json!({
        "nodes": [
            {"node_id":"metadata", "version_id":"v", "kind":"rename", "note":"path note"},
            {"node_id":"n", "version_id":"v", "kind":"content", "note":"body note"}
        ],
        "branches": [branch_wire(json!({"branch_id":"b", "name":"first"}))]
    }));
    let index = HistoryIndex::new(snapshot.clone());
    let (position, node) = index.node("n").unwrap();
    assert_eq!(position, 1);
    assert!(std::ptr::eq(node, &snapshot.nodes()[1]));
    assert!(std::ptr::eq(
        index.branch("b").unwrap(),
        &snapshot.branches()[0]
    ));
    assert!(std::ptr::eq(
        index.version("v").unwrap(),
        &snapshot.nodes()[1]
    ));
    assert_eq!(index.version("v").unwrap().body_note(), Some("body note"));
    assert_eq!(index.node("metadata").unwrap().1.note(), "path note");
    assert!(index.node("missing").is_none());
    assert!(index.branch("missing").is_none());
    assert!(index.version("missing").is_none());
}

#[test]
fn an_independent_current_summary_does_not_enter_the_window_index() {
    let snapshot = history(json!({
        "current_node_id":"current-outside", "version_id":"current-body",
        "nodes":[{"node_id":"loaded", "version_id":"loaded-body"}]
    }));
    let index = HistoryIndex::new(snapshot);
    assert_eq!(index.snapshot.current_node_id(), Some("current-outside"));
    assert!(index.snapshot.node("current-outside").is_some());
    assert!(index.node("current-outside").is_none());
    assert!(index.version("current-body").is_none());
    assert_eq!(index.node("loaded").unwrap().0, 0);
}

#[test]
fn shared_history_skips_unrelated_updates_and_replaces_positions_on_new_snapshot() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let owner = Owner::new();
    owner.with(|| {
        let state = RwSignal::new(State::default());
        state.update(|s| {
            s.files.file_history =
                crate::files::history::tests::history(json!({"nodes":[{"node_id":"old"}]}))
        });
        provide(state);
        let first = shared();
        let second = shared();
        assert!(first == second);
        let runs = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&runs);
        let consumer = Memo::new(move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
            first.with(|index| index.node("old").map(|(_, node)| node.clone()))
        });
        assert!(consumer.get().is_some());
        let before = first.with(|index| {
            assert!(index.node("old").is_some());
            index as *const HistoryIndex
        });
        assert_eq!(before, second.with(|index| index as *const HistoryIndex));
        state.update(|s| s.files.history_loading = true);
        first.with(|index| {
            assert_eq!(before, index as *const HistoryIndex);
            assert!(index.node("old").is_some());
        });
        assert!(consumer.get().is_some());
        assert_eq!(runs.load(Ordering::Relaxed), 1);
        state.update(|s| {
            update_metadata(&mut s.files.file_history, |value| {
                value["nodes"][0]["note"] = "updated".into()
            })
        });
        assert_eq!(consumer.get().unwrap().note(), "updated");
        assert_eq!(runs.load(Ordering::Relaxed), 2);
        state.update(|s| {
            s.files.file_history =
                crate::files::history::tests::history(json!({"nodes":[{"node_id":"new"}]}))
        });
        assert!(consumer.get().is_none());
        assert_eq!(runs.load(Ordering::Relaxed), 3);
        second.with(|index| {
            assert!(index.node("old").is_none());
            assert_eq!(index.node("new").unwrap().0, 0);
        });
        state.update(|s| s.files.file_history = Default::default());
        first.with(|index| assert!(index.node("new").is_none()));
    });
    owner.cleanup();
}

#[test]
fn indexed_lookup_matches_a_scan_of_each_replaced_window() {
    for count in [1, 16, 64] {
        let nodes: Vec<_> = (0..count).rev().map(|i| json!({
            "node_id":format!("node-{i}"), "note":format!("note {i}"), "tags":[format!("tag-{i}")]
        })).collect();
        let snapshot = history(json!({"nodes":nodes}));
        let index = HistoryIndex::new(snapshot.clone());
        for i in 0..count {
            let id = format!("node-{i}");
            let expected = snapshot.nodes()
                .iter()
                .enumerate()
                .find(|(_, node)| node.id() == id);
            assert_eq!(index.node(&id), expected);
        }
    }
}
