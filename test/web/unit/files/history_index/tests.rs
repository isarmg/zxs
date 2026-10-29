use super::*;
use crate::files::history::tests::{branch_page, branch_wire, chain, node, window};
use json_codec::json;
use std::sync::Arc;

#[test]
fn indexed_records_borrow_the_resident_window_and_only_its_nodes() {
    let snapshot: Snapshot<_> = chain(3).into();
    let index = HistoryIndex::new(Some(snapshot.clone()));
    let (position, node) = index.node("n3").unwrap();
    assert_eq!(position, 0);
    assert!(std::ptr::eq(node, &snapshot.nodes[0]));
    assert_eq!(index.node("n1").unwrap().0, 2);
    assert!(index.node("missing").is_none());
    assert_eq!(index.node_ids().count(), 3);
    let empty = HistoryIndex::new(None);
    assert!(empty.node("n1").is_none());
    assert_eq!(empty.node_ids().count(), 0);
}

#[test]
fn branch_lookups_come_from_the_resident_branch_page_only() {
    let mut state = State::default();
    assert!(branch(&state, "b").is_none());
    state.files.history.install(chain(1));
    assert!(branch(&state, "b").is_none());
    state.files.history.branches = Some(branch_page(
        vec![branch_wire(json!({"branch_id":"b","name":"first"}))],
        None,
    ));
    assert_eq!(branch(&state, "b").unwrap().name(), "first");
    state
        .files
        .history
        .mark_stale(crate::files::history::state::StaleReason::RootChanged);
    assert!(
        branch(&state, "b").is_none(),
        "a stale root drops its branch page"
    );
}

#[test]
fn shared_index_skips_unrelated_updates_and_replacing_the_window_frees_the_old_page() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let owner = Owner::new();
    owner.with(|| {
        let state = RwSignal::new(State::default());
        state.update(|s| s.files.history.install(chain(2)));
        provide(state);
        let first = shared();
        let second = shared();
        assert!(first == second);
        let runs = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&runs);
        let consumer = Memo::new(move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
            first.with(|index| index.node("n2").map(|(_, node)| node.node_id.clone()))
        });
        assert!(consumer.get().is_some());
        let before = first.with(|index| index as *const HistoryIndex);
        assert_eq!(before, second.with(|index| index as *const HistoryIndex));
        // Auxiliary pages, selections and notices never rebuild the index.
        state.update(|s| {
            s.files.history.branches = Some(branch_page(vec![], None));
            s.files.checked_bytes = 1;
            s.notice = "x".into();
        });
        first.with(|index| assert_eq!(before, index as *const HistoryIndex));
        assert!(consumer.get().is_some());
        assert_eq!(runs.load(Ordering::Relaxed), 1);

        let old = state.with_untracked(|s| s.files.history.loaded().unwrap().clone());
        let weak = old.weak();
        drop(old);
        state.update(|s| {
            s.files
                .history
                .install(window(vec![node("new", 9, &[], &["new"])], json!({})))
        });
        assert!(consumer.get().is_none());
        assert_eq!(runs.load(Ordering::Relaxed), 2);
        second.with(|index| {
            assert!(index.node("n2").is_none());
            assert_eq!(index.node_ids().collect::<Vec<_>>(), ["new"]);
        });
        assert!(
            weak.upgrade().is_none(),
            "no index, memo or model slot retains the replaced page"
        );
        state.update(|s| s.files.history.clear());
        first.with(|index| assert!(index.node("new").is_none()));
    });
    owner.cleanup();
}

#[test]
fn indexed_lookup_matches_a_linear_scan_for_a_large_page() {
    for count in [1, 64, 200] {
        let ids = (0..count)
            .map(|i| format!("node-{i:03}"))
            .collect::<Vec<_>>();
        let page = ids.iter().map(String::as_str).collect::<Vec<_>>();
        let nodes = (0..count)
            .rev()
            .map(|i| node(&format!("node-{i:03}"), i as u64 + 1, &[], &page))
            .collect();
        let snapshot: Snapshot<_> =
            window(nodes, json!({"budget":{"limit":200,"parent_edges":2048}})).into();
        let index = HistoryIndex::new(Some(snapshot.clone()));
        for id in &ids {
            let expected = snapshot
                .nodes
                .iter()
                .enumerate()
                .find(|(_, node)| node.node_id == *id);
            assert_eq!(index.node(id), expected);
        }
    }
}
