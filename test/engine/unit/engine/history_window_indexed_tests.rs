use super::*;
use crate::engine::history_window::oracle_tests::{context, fixture, request};
use crate::{Config, Engine};
use std::sync::Arc;

fn indexed_engine(temp: &tempfile::TempDir, unrelated: usize) -> Engine {
    let mut engine = Engine::initialize(
        &temp.path().join("state"),
        Config::development([temp.path().join("p"), temp.path().join("b")]),
    )
    .unwrap();
    let view = fixture();
    let mut state = view.business_projection().clone();
    let template = state.versions.values().next().unwrap().clone();
    let commit = state.commits.values().next().unwrap().clone();
    for n in 0..unrelated {
        let mut version = template.clone();
        version.version_id = format!("ver_{:032x}", n + 0x10000);
        version.file_id = format!("file_{:032x}", n + 0x10000);
        version.history_tree_id = format!("history_{:032x}", n + 0x10000);
        let mut other = commit.clone();
        other.commit_id = format!("commit_{:032x}", n + 0x10000);
        other.history_tree_id = version.history_tree_id.clone();
        other.entries[0].file_id = version.file_id.clone();
        other.entries[0].version_id = version.version_id.clone();
        state.versions.insert(version.version_id.clone(), version);
        state.commits.insert(other.commit_id.clone(), other);
    }
    engine.state = Arc::new(state);
    engine.bind_recovered_history_roots(BTreeMap::from([(
        context().history_tree_id.as_str().to_owned(),
        view.history_root(context().history_tree_id.as_str())
            .unwrap()
            .to_owned(),
    )]));
    engine.rebuild_index().unwrap();
    engine
}

fn equal_page(engine: &Engine, request: &HistoryWindowRequest) -> HistoryWindowPage {
    let view = engine.metadata_view();
    let context = context();
    let cancel = HistoryWindowCancellation::new();
    let indexed = capture_indexed_selection(engine, &view, request, &context, &cancel)
        .unwrap()
        .expect("healthy scope index must be used");
    let actual = view
        .history_window_selected(request, &context, &cancel, Some(&indexed))
        .unwrap();
    let reference = view.history_window(request, &context, &cancel).unwrap();
    assert_eq!(
        json_codec::to_value(&actual).unwrap(),
        json_codec::to_value(&reference).unwrap()
    );
    actual
}

#[test]
fn scoped_selection_matches_complete_page_oracle_forward_backward_and_anchors() {
    let temp = tempfile::tempdir().unwrap();
    let engine = indexed_engine(&temp, 127);
    for collection in [HistoryCollection::Nodes, HistoryCollection::Versions] {
        for limit in [1, 2, 7, 64] {
            let mut request = request(collection.clone(), limit);
            let mut pages = Vec::new();
            loop {
                let page = equal_page(&engine, &request);
                let cursors = match page {
                    HistoryWindowPage::Nodes { page, .. }
                    | HistoryWindowPage::Versions { page, .. } => page,
                    _ => unreachable!(),
                };
                pages.push(cursors.previous_cursor);
                let Some(next) = cursors.next_cursor else {
                    break;
                };
                request.cursor = Some(next);
            }
            for previous in pages.into_iter().flatten() {
                request.cursor = Some(previous);
                equal_page(&engine, &request);
            }
        }
    }
    let view = engine.metadata_view();
    for (_, id) in
        crate::engine::history_window::oracle_tests::oracle_node_keys(view.business_projection())
    {
        let mut request = request(HistoryCollection::Nodes, 2);
        request.anchor_node_id = Some(HistoryNodeId::parse(&id).unwrap());
        request.history_root = Some(
            HistoryRoot::parse(
                view.history_root(context().history_tree_id.as_str())
                    .unwrap(),
            )
            .unwrap(),
        );
        equal_page(&engine, &request);
    }
}

#[test]
fn indexed_owner_matches_minimum_id_scan_and_retained_page_survives_later_publication() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = indexed_engine(&temp, 31);
    let request = request(HistoryCollection::Nodes, 7);
    let view = engine.metadata_view();
    let context = context();
    let cancel = HistoryWindowCancellation::new();
    let owner = engine
        .indexed_history_owner(request.file_id.as_str(), &cancel)
        .unwrap()
        .unwrap();
    let reference = nodes::owner_version(
        engine.state.versions.values(),
        request.file_id.as_str(),
        &cancel,
    )
    .unwrap()
    .unwrap();
    assert_eq!(owner, reference);
    let selected = capture_indexed_selection(&engine, &view, &request, &context, &cancel)
        .unwrap()
        .unwrap();
    let expected = view.history_window(&request, &context, &cancel).unwrap();
    let before = Arc::clone(&engine.state);
    Arc::make_mut(&mut engine.state).sequence += 1;
    engine.update_index(&before);
    let actual = view
        .history_window_selected(&request, &context, &cancel, Some(&selected))
        .unwrap();
    assert_eq!(
        json_codec::to_value(actual).unwrap(),
        json_codec::to_value(expected).unwrap()
    );
}

#[test]
fn unrelated_pool_growth_does_not_increase_scoped_selection_record_visits() {
    let mut measurements = Vec::new();
    for unrelated in [0, 257] {
        let temp = tempfile::tempdir().unwrap();
        let engine = indexed_engine(&temp, unrelated);
        let view = engine.metadata_view();
        let request = request(HistoryCollection::Nodes, 7);
        crate::engine::index::reset_scope_read_metrics();
        let selected = capture_indexed_selection(
            &engine,
            &view,
            &request,
            &context(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap()
        .unwrap();
        let measurement = crate::engine::index::scope_read_metrics();
        assert!(
            measurement.0[0] > 0
                && measurement.0[1] > 0
                && measurement.0[2] > 0
                && measurement.0[3] > 0
        );
        assert!(measurement.1 <= 64);
        assert!(matches!(selected.page, CapturedPage::Nodes(ref page) if page.entries.len() == 7));
        measurements.push(measurement);
    }
    assert_eq!(measurements[0], measurements[1]);
    eprintln!(
        "scoped page visits at unrelated histories 0/257: {:?}",
        measurements
    );
}

#[test]
fn snapshot_capture_reads_no_members_and_old_snapshot_is_valid_during_new_publication() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = indexed_engine(&temp, 127);
    let view = engine.metadata_view();
    let request = request(HistoryCollection::Nodes, 7);
    let context = context();
    crate::engine::index::reset_scope_read_metrics();
    let snapshot = engine
        .capture_history_scope(request.file_id.as_str(), context.history_tree_id.as_str())
        .unwrap()
        .unwrap();
    assert_eq!(crate::engine::index::scope_read_metrics(), ([0; 4], 0));
    let previous = Arc::clone(&engine.state);
    Arc::make_mut(&mut engine.state).sequence += 1;
    engine.update_index(&previous);
    assert!(matches!(
        engine.index_health(),
        crate::engine::index::IndexHealth::Ready { .. }
    ));
    let cancel = HistoryWindowCancellation::new();
    let selected = prepare_indexed_selection(&snapshot, &view, &request, &context, &cancel)
        .unwrap()
        .unwrap();
    let actual = view
        .history_window_selected(&request, &context, &cancel, Some(&selected))
        .unwrap();
    let expected = view.history_window(&request, &context, &cancel).unwrap();
    assert_eq!(
        json_codec::to_value(actual).unwrap(),
        json_codec::to_value(expected).unwrap()
    );
    // The read connection was consumed/closed; callers reuse only the bounded selection.
    assert!(
        prepare_indexed_selection(&snapshot, &view, &request, &context, &cancel)
            .unwrap()
            .is_none()
    );
}

#[test]
fn capture_owner_and_lock_free_selection_costs_are_explicit_as_target_history_grows() {
    for extra in [0, 50] {
        let temp = tempfile::tempdir().unwrap();
        let mut engine = indexed_engine(&temp, 257);
        let state = Arc::make_mut(&mut engine.state);
        let version = state.versions["ver_00000000000000000000000000000000"].clone();
        let commit = state.commits["commit_00000000000000000000000000000000"].clone();
        for n in 0..extra {
            let mut v = version.clone();
            v.version_id = format!("ver_{:032x}", n + 0x20000);
            let mut c = commit.clone();
            c.commit_id = format!("commit_{:032x}", n + 0x20000);
            c.entries[0].version_id = v.version_id.clone();
            state.versions.insert(v.version_id.clone(), v);
            state.commits.insert(c.commit_id.clone(), c);
        }
        engine.bind_recovered_history_roots(BTreeMap::from([(
            context().history_tree_id.as_str().to_owned(),
            "a".repeat(64),
        )]));
        engine.rebuild_index().unwrap();
        let request = request(HistoryCollection::Nodes, 7);
        let context = context();
        let cancel = HistoryWindowCancellation::new();
        crate::engine::index::reset_scope_read_metrics();
        let start = std::time::Instant::now();
        engine
            .indexed_history_owner(request.file_id.as_str(), &cancel)
            .unwrap()
            .unwrap();
        let owner_elapsed = start.elapsed();
        let owner_visits = crate::engine::index::scope_read_metrics();
        // Fixture also has one same-file version in another history; owner
        // admission must include it rather than assume one history per file.
        assert_eq!(owner_visits.0, [18 + extra, 0, 0, 0]);
        crate::engine::index::reset_scope_read_metrics();
        let start = std::time::Instant::now();
        let snapshot = engine
            .capture_history_scope(request.file_id.as_str(), context.history_tree_id.as_str())
            .unwrap()
            .unwrap();
        let capture_elapsed = start.elapsed();
        assert_eq!(crate::engine::index::scope_read_metrics(), ([0; 4], 0));
        let view = engine.metadata_view();
        let start = std::time::Instant::now();
        prepare_indexed_selection(&snapshot, &view, &request, &context, &cancel)
            .unwrap()
            .unwrap();
        let selection_elapsed = start.elapsed();
        let selection_visits = crate::engine::index::scope_read_metrics();
        assert_eq!(selection_visits.0[0], (17 + extra) * 2);
        assert!(selection_visits.1 <= 64);
        eprintln!(
            "target versions={} unrelated=257 owner={owner_visits:?}/{owner_elapsed:?}, snapshot_capture=0/{capture_elapsed:?}, lock_free_selection={selection_visits:?}/{selection_elapsed:?}",
            17 + extra
        );
    }
}

fn actual_read_fixture() -> (tempfile::TempDir, Engine, HistoryWindowRequest) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("state"),
        Config::development([temp.path().join("disk")]),
    )
    .unwrap();
    let repository = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("target").unwrap(),
            "P01",
            1,
            "admin",
            crate::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "file.txt",
            b"complete retained body",
        )
        .unwrap();
    engine
        .commit_workspace(crate::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: crate::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "indexed complete-scope fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let entry = engine
        .list_tree(&repository.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path() == "file.txt")
        .unwrap();
    let mut request = request(HistoryCollection::Nodes, 7);
    request.file_id = HistoryFileId::parse(&entry.file_id).unwrap();
    request.location = None;
    request.history_root = None;
    (temp, engine, request)
}

#[test]
fn actual_capture_repeat_and_corrupt_membership_fallback_keep_full_page() {
    use sqlite_access::{Connection, Executor};
    let (_temp, engine, request) = actual_read_fixture();
    let read = engine
        .capture_history_window(request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    let expected = read.page().unwrap();
    let repeated = read.page().unwrap();
    assert_eq!(
        json_codec::to_value(&expected).unwrap(),
        json_codec::to_value(&repeated).unwrap()
    );
    engine.recheck_history_window(&read).unwrap();
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = platform_io::sqlite::open_index_connection(
            &engine.root.join("index.sqlite"),
            false,
            None,
        )
        .await
        .unwrap();
        // Keep owner versions healthy, but omit the target commit entry. The
        // finite selection cannot silently turn the version into "unpublished".
        connection
            .execute("DELETE FROM history_scope_members WHERE kind=3")
            .await
            .unwrap();
        connection.close().await.unwrap();
    });
    let fallback = engine
        .capture_history_window(request, HistoryWindowCancellation::new())
        .unwrap();
    let actual = fallback.page().unwrap();
    assert_eq!(
        json_codec::to_value(actual).unwrap(),
        json_codec::to_value(expected).unwrap()
    );
    engine.recheck_history_window(&fallback).unwrap();
    assert!(matches!(
        engine.index_health(),
        crate::engine::index::IndexHealth::Unavailable(_)
    ));
}

#[test]
fn actual_cancelled_read_closes_snapshot_without_poisoning_index() {
    let (_temp, engine, request) = actual_read_fixture();
    let cancellation = HistoryWindowCancellation::new();
    let read = engine
        .capture_history_window(request, cancellation.clone())
        .unwrap();
    cancellation.cancel();
    assert!(read.page().is_err());
    assert!(matches!(
        engine.index_health(),
        crate::engine::index::IndexHealth::Ready { .. }
    ));
    assert!(engine.recheck_history_window(&read).is_err());
    assert!(matches!(
        engine.index_health(),
        crate::engine::index::IndexHealth::Ready { .. }
    ));
}

#[test]
fn indexed_version_fields_reuse_scoped_first_commits_without_pool_header_scans() {
    let mut visits = Vec::new();
    for unrelated in [0, 257] {
        let temp = tempfile::tempdir().unwrap();
        let mut engine = indexed_engine(&temp, unrelated);
        let state = Arc::make_mut(&mut engine.state);
        let original = state.versions["ver_00000000000000000000000000000000"].clone();
        let mut unpublished = original.clone();
        unpublished.version_id = format!("ver_{:032x}", 99);
        state
            .versions
            .insert(unpublished.version_id.clone(), unpublished);
        let commit = state.commits["commit_00000000000000000000000000000000"].clone();
        for (id, author) in [(20, "first commit-ID wins"), (21, "later tie must not win")] {
            let mut tied = commit.clone();
            tied.commit_id = format!("commit_{id:032x}");
            tied.sequence = 0;
            tied.author = author.into();
            state.commits.insert(tied.commit_id.clone(), tied);
        }
        engine.bind_recovered_history_roots(BTreeMap::from([(
            context().history_tree_id.as_str().to_owned(),
            "a".repeat(64),
        )]));
        engine.rebuild_index().unwrap();
        let view = engine.metadata_view();
        let request = request(HistoryCollection::Versions, 64);
        let context = context();
        let cancel = HistoryWindowCancellation::new();
        crate::engine::index::reset_scope_read_metrics();
        nodes::take_first_commit_headers();
        let selection = capture_indexed_selection(&engine, &view, &request, &context, &cancel)
            .unwrap()
            .unwrap();
        let actual = view
            .history_window_selected(&request, &context, &cancel, Some(&selection))
            .unwrap();
        let selection_visits = crate::engine::index::scope_read_metrics();
        assert_eq!(
            nodes::take_first_commit_headers(),
            0,
            "indexed Versions projection must not restart a pool-wide first_commit scan"
        );
        let reference = view.history_window(&request, &context, &cancel).unwrap();
        assert!(
            nodes::take_first_commit_headers() > 0,
            "reference must witness the previous traversal"
        );
        assert_eq!(
            json_codec::to_value(actual).unwrap(),
            json_codec::to_value(reference).unwrap()
        );
        visits.push(selection_visits);
    }
    assert_eq!(visits[0], visits[1]);
    eprintln!(
        "Versions first-commit capture at unrelated histories 0/257: {visits:?}; projected whole-pool first_commit headers=0"
    );
}

#[test]
fn omitted_commit_entry_cannot_become_empty_version_fields() {
    use sqlite_access::{Connection, Executor};
    let (_temp, engine, mut request) = actual_read_fixture();
    request.collection = HistoryCollection::Versions;
    let expected = engine
        .capture_history_window(request.clone(), HistoryWindowCancellation::new())
        .unwrap()
        .page()
        .unwrap();
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut c = platform_io::sqlite::open_index_connection(
            &engine.root.join("index.sqlite"),
            false,
            None,
        )
        .await
        .unwrap();
        c.execute("DELETE FROM history_scope_members WHERE kind=3")
            .await
            .unwrap();
        c.close().await.unwrap();
    });
    let read = engine
        .capture_history_window(request, HistoryWindowCancellation::new())
        .unwrap();
    let actual = read.page().unwrap();
    engine.recheck_history_window(&read).unwrap();
    assert_eq!(
        json_codec::to_value(actual).unwrap(),
        json_codec::to_value(expected).unwrap()
    );
    assert!(matches!(
        engine.index_health(),
        crate::engine::index::IndexHealth::Unavailable(_)
    ));
}
