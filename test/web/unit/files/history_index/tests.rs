use super::*;
use json_codec::json;

#[test]
fn indexed_records_borrow_the_snapshot_and_keep_first_match_order() {
    let snapshot: Snapshot<Value> = json!({
        "nodes": [{"node_id":"n", "note":"first"}, {"node_id":"n", "note":"second"}],
        "versions": [{"version_id":"v", "note":"body"}],
        "branches": [{"branch_id":"b", "name":"first"}, {"branch_id":"b", "name":"second"}]
    })
    .into();
    let index = HistoryIndex::new(snapshot.clone());
    let (position, node) = index.node("n").unwrap();
    assert_eq!(position, 0);
    assert!(std::ptr::eq(node, &snapshot["nodes"][0]));
    assert!(std::ptr::eq(
        index.branch("b").unwrap(),
        &snapshot["branches"][0]
    ));
    assert!(std::ptr::eq(
        index.version("v").unwrap(),
        &snapshot["versions"][0]
    ));
    assert!(index.node("missing").is_none());
    assert!(index.branch("missing").is_none());
    assert!(index.version("missing").is_none());
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
        state.update(|s| s.files.file_history = json!({"nodes":[{"node_id":"old"}]}).into());
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
        state.update(|s| s.files.file_history["nodes"][0]["note"] = "updated".into());
        assert_eq!(consumer.get().unwrap()["note"], "updated");
        assert_eq!(runs.load(Ordering::Relaxed), 2);
        state.update(|s| s.files.file_history = json!({"nodes":[{"node_id":"new"}]}).into());
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
fn indexed_lookup_matches_the_previous_scan_for_large_reordered_history() {
    for count in [1, 64, 1024] {
        let nodes: Vec<_> = (0..count).rev().map(|i| json!({
            "node_id":format!("node-{i}"), "note":format!("note {i}"), "tags":[format!("tag-{i}")]
        })).collect();
        let snapshot: Snapshot<Value> = json!({"nodes":nodes}).into();
        let index = HistoryIndex::new(snapshot.clone());
        for i in 0..count {
            let id = format!("node-{i}");
            let expected = history_nodes_ref(&snapshot)
                .iter()
                .enumerate()
                .find(|(_, node)| history_node_id(node) == id);
            assert_eq!(index.node(&id), expected);
        }
    }
}
