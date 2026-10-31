use super::*;
use crate::Commit;
use std::cell::Cell;

use super::super::oracle_tests::{context, fixture, oracle_node_keys, request};

type Key<'a> = (Reverse<u64>, &'a str);
type Selected<'a> = Selection<Key<'a>, (nodes::Node<'a>, Option<&'a Commit>)>;

// The previous per-candidate planner, kept only as a counted reference.
fn reference_selection<'a>(
    query: &Query<'a>,
    position: Position<Key<'a>>,
    headers: &Cell<usize>,
    entries: &Cell<usize>,
) -> Result<Selected<'a>> {
    let mut selector = Selector::new(position, query.request.limit.get())?;
    for candidate in nodes::all_nodes(query.state, query.scope(), query.cancel) {
        let node = candidate?;
        let version = &node.version(query.state).version_id;
        let mut first = None::<&Commit>;
        for commit in query.state.commits.values().inspect(|_| {
            headers.set(headers.get() + 1);
        }) {
            query.cancel.checkpoint()?;
            if commit.history_tree_id != query.scope().history {
                continue;
            }
            for entry in &commit.entries {
                entries.set(entries.get() + 1);
                query.cancel.checkpoint()?;
                if entry.file_id == query.scope().file
                    && &entry.version_id == version
                    && first.is_none_or(|first| commit.sequence < first.sequence)
                {
                    first = Some(commit);
                }
            }
        }
        selector.offer((Reverse(node.sequence(first)), node.id()), (node, first));
    }
    selector.finish()
}

fn keys(selection: &Selected<'_>) -> Vec<(u64, String)> {
    selection
        .entries
        .iter()
        .map(|((Reverse(sequence), id), _)| (*sequence, (*id).to_owned()))
        .collect()
}

fn expanded_fixture() -> MetadataView {
    let mut view = fixture();
    let state = Arc::make_mut(&mut view.state);
    let original = "ver_00000000000000000000000000000000";
    let body = state.versions[original].clone();
    let commit = state.commits["commit_00000000000000000000000000000000"].clone();
    // 67 content records cross the shared 64-record batch boundary. Cloning
    // the zero-source record keeps this fixture small and independent of n.
    for n in 1000..1050 {
        let mut body = body.clone();
        body.version_id = format!("ver_{n:032x}");
        body.version_sequence = u64::MAX;
        let mut commit = commit.clone();
        commit.commit_id = format!("commit_{n:032x}");
        commit.sequence = n;
        commit.entries[0].version_id = body.version_id.clone();
        state.versions.insert(body.version_id.clone(), body);
        state.commits.insert(commit.commit_id.clone(), commit);
    }
    let roots = BTreeMap::from([(
        "history_00000000000000000000000000000001".into(),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
    )]);
    view.history_window_roots = Some(HistoryRootBindings::recovered(&view.state, roots));
    view
}

#[test]
fn per_candidate_reference_counts_commit_traversals_and_matches_complete_oracle() {
    for (view, node_count, commit_headers, scoped_entries) in [
        (fixture(), 19, 342, 323),
        (expanded_fixture(), 69, 4692, 4623),
    ] {
        let context = context();
        let expected = oracle_node_keys(view.state());
        assert_eq!(expected.len(), node_count);
        for limit in [1, 2, 7, 64] {
            let request = request(HistoryCollection::Nodes, limit);
            let cancel = HistoryWindowCancellation::new();
            let query = Query::new(&view, &request, &context, &cancel).unwrap();
            let headers = Cell::new(0);
            let entries = Cell::new(0);
            let selected =
                reference_selection(&query, Position::Start, &headers, &entries).unwrap();
            assert_eq!(keys(&selected), expected[..expected.len().min(limit)]);
            assert_eq!(headers.get(), commit_headers);
            assert_eq!(entries.get(), scoped_entries);
        }
    }
}

fn actual_selection<'a>(
    query: &Query<'a>,
    position: Position<Key<'a>>,
    headers: &Cell<usize>,
) -> Result<Selected<'a>> {
    query.select_nodes(
        || {
            query.state.commits.values().inspect(|_| {
                headers.set(headers.get() + 1);
            })
        },
        position,
    )
}

#[test]
fn batched_selection_reduces_actual_commit_header_visits_at_every_page_budget() {
    for (view, baseline_headers, batched_headers) in
        [(fixture(), 342, 18), (expanded_fixture(), 4692, 136)]
    {
        let context = context();
        let expected = oracle_node_keys(view.state());
        for limit in [1, 2, 7, 64] {
            let request = request(HistoryCollection::Nodes, limit);
            let cancel = HistoryWindowCancellation::new();
            let query = Query::new(&view, &request, &context, &cancel).unwrap();
            let headers = Cell::new(0);
            let actual = actual_selection(&query, Position::Start, &headers).unwrap();
            assert_eq!(keys(&actual), expected[..expected.len().min(limit)]);
            assert_eq!(headers.get(), batched_headers);
            let reference_headers = Cell::new(0);
            let reference_entries = Cell::new(0);
            let reference = reference_selection(
                &query,
                Position::Start,
                &reference_headers,
                &reference_entries,
            )
            .unwrap();
            assert_eq!(reference_headers.get(), baseline_headers);
            assert_eq!(keys(&actual), keys(&reference));
            assert_eq!(
                (actual.has_before, actual.has_after),
                (reference.has_before, reference.has_after)
            );
            for ((_, (node, first)), (_, (_, previous))) in
                actual.entries.iter().zip(&reference.entries)
            {
                if matches!(node, nodes::Node::Content(_)) {
                    assert_eq!(
                        first.map(|commit| &commit.commit_id),
                        previous.map(|commit| &commit.commit_id)
                    );
                }
            }
        }
    }
}

fn public_nodes(
    view: &MetadataView,
    request: &HistoryWindowRequest,
) -> (Vec<(u64, String)>, HistoryPageCursors) {
    let context = context();
    let cancel = HistoryWindowCancellation::new();
    let HistoryWindowPage::Nodes { nodes, page, .. } = Query::new(view, request, &context, &cancel)
        .unwrap()
        .page()
        .unwrap()
    else {
        panic!("node request returned another collection")
    };
    (
        nodes
            .into_iter()
            .map(|node| (node.sequence, node.node_id.as_str().to_owned()))
            .collect(),
        page,
    )
}

#[test]
fn batched_public_pages_exhaust_oracle_with_forward_backward_and_every_anchor() {
    let view = expanded_fixture();
    let expected = oracle_node_keys(view.state());
    for limit in [1, 2, 7, 64] {
        let mut request = request(HistoryCollection::Nodes, limit);
        let mut page = public_nodes(&view, &request);
        let mut forward = Vec::new();
        for _ in 0..=expected.len() {
            assert!(!page.0.is_empty());
            assert!(page.0.len() <= limit);
            forward.extend_from_slice(&page.0);
            let Some(next) = page.1.next_cursor.clone() else {
                break;
            };
            request.cursor = Some(next);
            page = public_nodes(&view, &request);
        }
        assert_eq!(forward, expected, "forward budget {limit}");
        assert!(page.1.next_cursor.is_none());
        let mut backward = Vec::new();
        for _ in 0..=expected.len() {
            backward.push(page.0);
            let Some(previous) = page.1.previous_cursor else {
                break;
            };
            request.cursor = Some(previous);
            page = public_nodes(&view, &request);
        }
        backward.reverse();
        assert_eq!(backward.into_iter().flatten().collect::<Vec<_>>(), expected);
        for (index, (_, id)) in expected.iter().enumerate() {
            request.cursor = None;
            request.anchor_node_id = Some(HistoryNodeId::parse(id).unwrap());
            let page = public_nodes(&view, &request);
            assert_eq!(page.0, expected[index..(index + limit).min(expected.len())]);
        }
    }
}

#[test]
fn batched_selection_keeps_commit_id_ties_fallback_and_boundary_errors() {
    let mut view = fixture();
    let state = Arc::make_mut(&mut view.state);
    let version = "ver_00000000000000000000000000000002";
    let earlier = "commit_00000000000000000000000000000002";
    let later = "commit_000000000000000000000000000003e8";
    let mut tied = state.commits[earlier].clone();
    tied.commit_id = later.into();
    tied.message = "the later ID must not supply the first commit".into();
    state.commits.insert(later.into(), tied);
    let newer_id = "commit_000000000000000000000000000003e9";
    let lower_version = "ver_00000000000000000000000000000004";
    let mut lower = state.commits["commit_00000000000000000000000000000004"].clone();
    lower.commit_id = newer_id.into();
    lower.sequence = 0;
    state.commits.insert(newer_id.into(), lower);
    let mut pending = state.versions[version].clone();
    pending.version_id = "ver_000000000000000000000000000003e8".into();
    pending.version_sequence = 777;
    let pending_id = pending.version_id.clone();
    state.versions.insert(pending_id.clone(), pending);
    let roots = BTreeMap::from([(
        "history_00000000000000000000000000000001".into(),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
    )]);
    view.history_window_roots = Some(HistoryRootBindings::recovered(&view.state, roots));
    let context = context();
    let request = request(HistoryCollection::Nodes, 64);
    let cancel = HistoryWindowCancellation::new();
    let query = Query::new(&view, &request, &context, &cancel).unwrap();
    let actual = actual_selection(&query, Position::Start, &Cell::new(0)).unwrap();
    assert_eq!(keys(&actual), oracle_node_keys(view.state()));
    let (_, (_, first)) = actual
        .entries
        .iter()
        .find(|((_, id), _)| *id == version)
        .unwrap();
    assert_eq!(first.unwrap().commit_id, earlier);
    let (_, (_, first)) = actual
        .entries
        .iter()
        .find(|((_, id), _)| *id == lower_version)
        .unwrap();
    assert_eq!(first.unwrap().commit_id, newer_id);
    assert_eq!(first.unwrap().sequence, 0);
    let ((Reverse(sequence), _), (_, first)) = actual
        .entries
        .iter()
        .find(|((_, id), _)| *id == pending_id)
        .unwrap();
    assert_eq!(*sequence, 777);
    assert!(first.is_none());
    let first_key = actual.entries.first().unwrap().0;
    let last_key = actual.entries.last().unwrap().0;
    for (position, expected_flags) in [
        (Position::Before(first_key), (false, true)),
        (Position::After(last_key), (true, false)),
    ] {
        let empty = actual_selection(&query, position, &Cell::new(0)).unwrap();
        assert!(empty.entries.is_empty());
        assert_eq!((empty.has_before, empty.has_after), expected_flags);
    }
    for position in [
        Position::After((Reverse(0), "ver_ffffffffffffffffffffffffffffffff")),
        Position::Before((Reverse(0), "ver_ffffffffffffffffffffffffffffffff")),
        Position::At((Reverse(0), "ver_ffffffffffffffffffffffffffffffff")),
    ] {
        let error = actual_selection(&query, position, &Cell::new(0))
            .err()
            .unwrap();
        assert_eq!(error.code(), storage_contract::ErrorCode::InvalidRequest);
        assert_eq!(
            error.to_string(),
            "INVALID_REQUEST: history page boundary is outside the allowed collection"
        );
    }
}

#[test]
fn batched_selection_cancels_inside_a_commit_scan_before_returning_a_page() {
    let view = expanded_fixture();
    let context = context();
    let request = request(HistoryCollection::Nodes, 1);
    let cancel = HistoryWindowCancellation::new();
    let query = Query::new(&view, &request, &context, &cancel).unwrap();
    let headers = Cell::new(0);
    let error = query
        .select_nodes(
            || {
                view.state().commits.values().inspect(|_| {
                    headers.set(headers.get() + 1);
                    if headers.get() == 3 {
                        cancel.cancel();
                    }
                })
            },
            Position::Start,
        )
        .err()
        .unwrap();
    assert_eq!(error.code(), storage_contract::ErrorCode::OperationAborted);
    assert_eq!(headers.get(), 3);
    assert!(cancel.is_cancelled());
}

#[test]
fn revision_anchors_visit_zero_commit_headers_and_content_keeps_first_commit_order() {
    let view = fixture();
    let context = context();
    let request = request(HistoryCollection::Nodes, 7);
    let cancel = HistoryWindowCancellation::new();
    let query = Query::new(&view, &request, &context, &cancel).unwrap();
    let mut kinds = [0; 3];
    for candidate in nodes::all_nodes(query.state, query.scope(), &cancel) {
        let node = candidate.unwrap();
        let headers = Cell::new(0);
        let actual = query
            .anchor_position(
                node.id(),
                query.state.commits.values().inspect(|_| {
                    headers.set(headers.get() + 1);
                }),
            )
            .unwrap();
        let prior = nodes::first_commit(
            query.state,
            query.scope(),
            &node.version(query.state).version_id,
            &cancel,
        )
        .unwrap();
        let Position::At(key) = actual else {
            panic!("anchor must be exact");
        };
        assert_eq!(key, (Reverse(node.sequence(prior)), node.id()));
        match node {
            nodes::Node::Content(_) => {
                kinds[0] += 1;
                assert_eq!(headers.get(), query.state.commits.len());
            }
            nodes::Node::Path(_) => {
                kinds[1] += 1;
                assert_eq!(headers.get(), 0);
            }
            nodes::Node::Metadata(_) => {
                kinds[2] += 1;
                assert_eq!(headers.get(), 0);
            }
        }
    }
    assert!(kinds.into_iter().all(|count| count > 0));
}
