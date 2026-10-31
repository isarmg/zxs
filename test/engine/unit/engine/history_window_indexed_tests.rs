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
    assert_eq!(crate::engine::index::scope_read_metrics(), ([0; 6], 0));
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
        assert_eq!(owner_visits.0, [18 + extra, 0, 0, 0, 0, 0]);
        crate::engine::index::reset_scope_read_metrics();
        let start = std::time::Instant::now();
        let snapshot = engine
            .capture_history_scope(request.file_id.as_str(), context.history_tree_id.as_str())
            .unwrap()
            .unwrap();
        let capture_elapsed = start.elapsed();
        assert_eq!(crate::engine::index::scope_read_metrics(), ([0; 6], 0));
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

#[test]
fn indexed_content_branches_are_complete_and_ignore_unrelated_pool_growth() {
    let mut metrics = Vec::new();
    for unrelated in [0, 257] {
        let temp = tempfile::tempdir().unwrap();
        let mut engine = indexed_engine(&temp, unrelated);
        let state = Arc::make_mut(&mut engine.state);
        let template = state.branches["br_00000000000000000000000000000000"].clone();
        for n in 0..unrelated {
            let mut branch = template.clone();
            branch.branch_id = format!("br_{:032x}", n + 0x20000);
            branch.file_id = Some(format!("file_{:032x}", n + 0x20000));
            state.branches.insert(branch.branch_id.clone(), branch);
        }
        // Existing fixture includes equal eligible heads, a pending branch,
        // migrated/merged states, a directory branch and a different history.
        engine.bind_recovered_history_roots(BTreeMap::from([(
            context().history_tree_id.as_str().to_owned(),
            "a".repeat(64),
        )]));
        engine.rebuild_index().unwrap();
        let view = engine.metadata_view();
        let request = request(HistoryCollection::Nodes, 64);
        let context = context();
        let cancel = HistoryWindowCancellation::new();
        crate::engine::index::reset_scope_read_metrics();
        projection::take_branch_headers();
        let selected = capture_indexed_selection(&engine, &view, &request, &context, &cancel)
            .unwrap()
            .unwrap();
        let actual = view
            .history_window_selected(&request, &context, &cancel, Some(&selected))
            .unwrap();
        assert_eq!(projection::take_branch_headers(), 0);
        metrics.push(crate::engine::index::scope_read_metrics());
        let reference = view.history_window(&request, &context, &cancel).unwrap();
        assert!(projection::take_branch_headers() > 0);
        assert_eq!(
            json_codec::to_value(actual).unwrap(),
            json_codec::to_value(reference).unwrap()
        );
    }
    assert_eq!(metrics[0], metrics[1]);
    assert_eq!(metrics[0].0[4], 21);
    eprintln!("content_branch unrelated branches 0/257: {metrics:?}; pool branch headers=0");
}

#[test]
fn missing_branch_membership_falls_back_instead_of_losing_content_branch() {
    use sqlite_access::{Connection, Executor};
    let (_temp, engine, request) = actual_branch_fixture();
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
        c.execute("DELETE FROM history_scope_members WHERE kind=4")
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

fn actual_branch_fixture() -> (tempfile::TempDir, Engine, HistoryWindowRequest) {
    let (temp, mut engine, request) = actual_read_fixture();
    let version = engine
        .state
        .versions
        .values()
        .find(|v| v.file_id == request.file_id.as_str())
        .unwrap()
        .version_id
        .clone();
    engine
        .create_file_branch(
            request.file_id.as_str(),
            &version,
            &storage_contract::BranchName::parse("indexed alternate").unwrap(),
            "admin",
            crate::OperationId::generate(),
        )
        .unwrap();
    (temp, engine, request)
}

#[test]
fn reopening_rebuilds_existing_four_kind_disposable_projection() {
    use sqlite_access::{Connection, Executor};
    let (_temp, engine, request) = actual_branch_fixture();
    let root = engine.root.clone();
    let history = engine
        .state
        .versions
        .values()
        .find(|v| v.file_id == request.file_id.as_str())
        .unwrap()
        .history_tree_id
        .clone();
    let expected = engine
        .state
        .branches
        .values()
        .filter(|b| {
            b.file_id.as_deref() == Some(request.file_id.as_str()) && b.history_tree_id == history
        })
        .count();
    assert!(expected > 0);
    drop(engine);
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut c =
            platform_io::sqlite::open_index_connection(&root.join("index.sqlite"), false, None)
                .await
                .unwrap();
        // Same schema and sequence marker, but old disposable data lacks kind 4.
        c.execute("DELETE FROM history_scope_members WHERE kind=4")
            .await
            .unwrap();
        c.close().await.unwrap();
    });
    let reopened = Engine::open(&root).unwrap();
    let count = reopened
        .with_history_scope(request.file_id.as_str(), &history, |r| {
            r.branches().try_fold(0, |n, b| b.map(|_| n + 1))
        })
        .unwrap();
    assert_eq!(count, Some(expected));
    assert!(matches!(
        reopened.index_health(),
        crate::engine::index::IndexHealth::Ready { .. }
    ));
}

#[test]
fn indexed_parent_previews_ignore_unrelated_merges_and_match_complete_oracle() {
    let mut metrics = Vec::new();
    for unrelated in [0, 257] {
        let temp = tempfile::tempdir().unwrap();
        let mut engine = indexed_engine(&temp, 0);
        let state = Arc::make_mut(&mut engine.state);
        let template = state.file_branch_merges.values().next().unwrap().clone();
        for n in 0..unrelated {
            let mut merge = template.clone();
            merge.merge_id = format!("fmerge_{:032x}", n + 0x20000);
            merge.file_id = format!("file_{:032x}", n + 0x20000);
            state
                .file_branch_merges
                .insert(merge.merge_id.clone(), merge);
        }
        engine.bind_recovered_history_roots(BTreeMap::from([(
            context().history_tree_id.as_str().to_owned(),
            "a".repeat(64),
        )]));
        engine.rebuild_index().unwrap();
        let view = engine.metadata_view();
        let request = request(HistoryCollection::Nodes, 64);
        let context = context();
        let cancel = HistoryWindowCancellation::new();
        crate::engine::index::reset_scope_read_metrics();
        nodes::take_parent_merge_headers();
        let selected = capture_indexed_selection(&engine, &view, &request, &context, &cancel)
            .unwrap()
            .unwrap();
        let actual = view
            .history_window_selected(&request, &context, &cancel, Some(&selected))
            .unwrap();
        assert_eq!(nodes::take_parent_merge_headers(), 0);
        metrics.push(crate::engine::index::scope_read_metrics());
        let reference = view.history_window(&request, &context, &cancel).unwrap();
        assert!(nodes::take_parent_merge_headers() > 0);
        assert_eq!(
            json_codec::to_value(actual).unwrap(),
            json_codec::to_value(reference).unwrap()
        );
    }
    assert_eq!(metrics[0], metrics[1]);
    assert!(metrics[0].0[5] > 0);
    eprintln!("parent previews unrelated merges 0/257: {metrics:?}; pool parent-merge headers=0");
}

#[test]
fn indexed_parent_preview_keeps_256_budget_more_cursor_and_reused_edges() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = indexed_engine(&temp, 0);
    let state = Arc::make_mut(&mut engine.state);
    let source = state.versions["ver_00000000000000000000000000000000"].clone();
    let template = state.file_branch_merges.values().next().unwrap().clone();
    for n in 0..300 {
        let mut version = source.clone();
        version.version_id = format!("ver_{:032x}", n + 0x40000);
        version.source_version_ids.clear();
        let mut merge = template.clone();
        merge.merge_id = format!("fmerge_{:032x}", n + 0x40000);
        merge.result_node_id = source.version_id.clone();
        merge.reused_node = true;
        // Duplicate input and self edge must keep original dedup/filter rules.
        merge.parent_node_ids = vec![
            version.version_id.clone(),
            version.version_id.clone(),
            source.version_id.clone(),
        ];
        state.versions.insert(version.version_id.clone(), version);
        state
            .file_branch_merges
            .insert(merge.merge_id.clone(), merge);
    }
    engine.bind_recovered_history_roots(BTreeMap::from([(
        context().history_tree_id.as_str().to_owned(),
        "a".repeat(64),
    )]));
    engine.rebuild_index().unwrap();
    let mut request = request(HistoryCollection::Nodes, 1);
    request.anchor_node_id = Some(HistoryNodeId::parse(&source.version_id).unwrap());
    let page = equal_page(&engine, &request);
    let HistoryWindowPage::Nodes { nodes, edges, .. } = page else {
        unreachable!()
    };
    assert_eq!(nodes.len(), 1);
    assert_eq!(edges.len(), MAX_HISTORY_PREVIEW_EDGES);
    assert!(matches!(nodes[0].parents, HistoryParentSet::More { .. }));
}

#[test]
fn omitted_merge_member_cannot_remove_parent_edges() {
    use sqlite_access::{Connection, Executor};
    let temp = tempfile::tempdir().unwrap();
    let engine = indexed_engine(&temp, 0);
    let view = engine.metadata_view();
    let request = request(HistoryCollection::Nodes, 64);
    let context = context();
    let cancel = HistoryWindowCancellation::new();
    let expected = view.history_window(&request, &context, &cancel).unwrap();
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut c = platform_io::sqlite::open_index_connection(
            &engine.root.join("index.sqlite"),
            false,
            None,
        )
        .await
        .unwrap();
        c.execute("DELETE FROM history_scope_members WHERE kind=5 AND id='fmerge_00000000000000000000000000000000'").await.unwrap();
        c.close().await.unwrap();
    });
    let selected = capture_indexed_selection(&engine, &view, &request, &context, &cancel).unwrap();
    assert!(selected.is_none());
    let actual = view
        .history_window_selected(&request, &context, &cancel, selected.as_ref())
        .unwrap();
    assert_eq!(
        json_codec::to_value(actual).unwrap(),
        json_codec::to_value(expected).unwrap()
    );
    assert!(matches!(
        engine.index_health(),
        crate::engine::index::IndexHealth::Unavailable(_)
    ));
}

const PATH_MERGE_METADATA: &str = "fmeta_00000000000000000000000000000000";
const PATH_MERGE_WINNER: &str = "fmerge_00000000000000000000000000070001";

fn metadata_merge_path_engine(
    temp: &tempfile::TempDir,
    terminal: &str,
    unrelated: usize,
) -> Engine {
    let mut engine = indexed_engine(temp, 0);
    let state = Arc::make_mut(&mut engine.state);
    state
        .file_metadata_nodes
        .get_mut(PATH_MERGE_METADATA)
        .unwrap()
        .kind = crate::FileMetadataKind::Merge;
    let template = state.file_branch_merges.values().next().unwrap().clone();
    for (n, chosen, reused) in [
        (0x70000, "ver_00000000000000000000000000000001", false),
        (0x70001, terminal, false),
        (0x70002, "ver_00000000000000000000000000000001", true),
    ] {
        let mut merge = template.clone();
        merge.merge_id = format!("fmerge_{n:032x}");
        merge.result_node_id = PATH_MERGE_METADATA.into();
        merge.chosen_node_id = chosen.into();
        merge.reused_node = reused;
        merge.parent_node_ids.clear();
        state
            .file_branch_merges
            .insert(merge.merge_id.clone(), merge);
    }
    let commit = state.commits["commit_00000000000000000000000000000000"].clone();
    for (n, path) in [(0x60000, "selected-first.txt"), (0x60001, "wrong-tie.txt")] {
        let mut copy = commit.clone();
        copy.commit_id = format!("commit_{n:032x}");
        copy.sequence = 0;
        copy.entries[0].relocate(name_policy::PortablePath::parse(path).unwrap());
        state.commits.insert(copy.commit_id.clone(), copy);
    }
    for n in 0..unrelated {
        let mut merge = template.clone();
        merge.merge_id = format!("fmerge_{:032x}", n + 0x80000);
        merge.file_id = format!("file_{:032x}", n + 0x80000);
        state
            .file_branch_merges
            .insert(merge.merge_id.clone(), merge);
    }
    engine.bind_recovered_history_roots(BTreeMap::from([(
        context().history_tree_id.as_str().to_owned(),
        "a".repeat(64),
    )]));
    engine.rebuild_index().unwrap();
    engine
}

#[test]
fn indexed_metadata_merge_paths_keep_last_match_reused_filter_and_terminal_oracle() {
    for terminal in [
        "fnode_00000000000000000000000000000000",
        "ver_00000000000000000000000000000000",
        "ver_00000000000000000000000000099999",
    ] {
        let mut measurements = Vec::new();
        for unrelated in [0, 257] {
            let temp = tempfile::tempdir().unwrap();
            let engine = metadata_merge_path_engine(&temp, terminal, unrelated);
            let view = engine.metadata_view();
            let request = request(HistoryCollection::Nodes, 64);
            let context = context();
            let cancel = HistoryWindowCancellation::new();
            crate::engine::index::reset_scope_read_metrics();
            nodes::take_path_merge_headers();
            nodes::take_first_commit_headers();
            let selected = capture_indexed_selection(&engine, &view, &request, &context, &cancel)
                .unwrap()
                .unwrap();
            let actual = view
                .history_window_selected(&request, &context, &cancel, Some(&selected))
                .unwrap();
            assert_eq!(nodes::take_path_merge_headers(), 0);
            assert_eq!(nodes::take_first_commit_headers(), 0);
            measurements.push(crate::engine::index::scope_read_metrics());
            let reference = view.history_window(&request, &context, &cancel).unwrap();
            assert!(nodes::take_path_merge_headers() > 0);
            assert_eq!(
                json_codec::to_value(&actual).unwrap(),
                json_codec::to_value(reference).unwrap()
            );
            let HistoryWindowPage::Nodes {
                nodes: page_nodes, ..
            } = actual
            else {
                unreachable!()
            };
            let metadata = page_nodes
                .iter()
                .find(|n| n.node_id.as_str() == PATH_MERGE_METADATA)
                .unwrap();
            let expected = if terminal.starts_with("fnode_") {
                "目录/终稿.txt"
            } else if terminal.ends_with("99999") {
                &engine.state.versions["ver_00000000000000000000000000000000"].creation_name
            } else {
                "selected-first.txt"
            };
            assert_eq!(metadata.path, expected);
        }
        assert_eq!(measurements[0], measurements[1], "{terminal}");
        assert!(measurements[0].0[5] > 0);
    }
}

#[test]
fn omitted_metadata_path_merge_member_falls_back_without_losing_path() {
    use sqlite_access::Connection;
    for replacement in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let engine = metadata_merge_path_engine(&temp, "fnode_00000000000000000000000000000000", 1);
        let view = engine.metadata_view();
        let request = request(HistoryCollection::Nodes, 64);
        let context = context();
        let cancel = HistoryWindowCancellation::new();
        let expected = view.history_window(&request, &context, &cancel).unwrap();
        platform_io::sqlite::block_on_sqlite_connection(async {
            let mut c = platform_io::sqlite::open_index_connection(
                &engine.root.join("index.sqlite"),
                false,
                None,
            )
            .await
            .unwrap();
            if replacement {
                let foreign = "fmerge_00000000000000000000000000080000";
                sqlite_access::query("DELETE FROM history_scope_members WHERE kind=5 AND id=?")
                    .bind(foreign)
                    .execute(&mut c)
                    .await
                    .unwrap();
                // Keep the target scope's row count unchanged, but forge a member.
                sqlite_access::query("UPDATE history_scope_members SET id=? WHERE kind=5 AND id=?")
                    .bind(foreign)
                    .bind(PATH_MERGE_WINNER)
                    .execute(&mut c)
                    .await
                    .unwrap();
            } else {
                sqlite_access::query("DELETE FROM history_scope_members WHERE kind=5 AND id=?")
                    .bind(PATH_MERGE_WINNER)
                    .execute(&mut c)
                    .await
                    .unwrap();
            }
            c.close().await.unwrap();
        });
        let selected =
            capture_indexed_selection(&engine, &view, &request, &context, &cancel).unwrap();
        assert!(selected.is_none());
        let actual = view
            .history_window_selected(&request, &context, &cancel, selected.as_ref())
            .unwrap();
        assert_eq!(
            json_codec::to_value(actual).unwrap(),
            json_codec::to_value(expected).unwrap()
        );
        assert!(matches!(
            engine.index_health(),
            crate::engine::index::IndexHealth::Unavailable(_)
        ));
    }
}

#[test]
fn metadata_merge_path_cycle_preserves_state_error_without_poisoning_index() {
    let temp = tempfile::tempdir().unwrap();
    let engine = metadata_merge_path_engine(&temp, PATH_MERGE_METADATA, 0);
    let view = engine.metadata_view();
    let request = request(HistoryCollection::Nodes, 64);
    let context = context();
    let cancel = HistoryWindowCancellation::new();
    let reference = view
        .history_window(&request, &context, &cancel)
        .unwrap_err();
    let actual = match capture_indexed_selection(&engine, &view, &request, &context, &cancel) {
        Err(error) => error,
        Ok(selected) => view
            .history_window_selected(&request, &context, &cancel, selected.as_ref())
            .unwrap_err(),
    };
    assert_eq!(reference.code(), storage_contract::ErrorCode::StateInvalid);
    assert_eq!(actual.code(), reference.code());
    assert_eq!(actual.to_string(), reference.to_string());
    assert!(matches!(
        engine.index_health(),
        crate::engine::index::IndexHealth::Ready { .. }
    ));
}

#[test]
fn cancelled_metadata_path_capture_does_not_disable_healthy_index() {
    let temp = tempfile::tempdir().unwrap();
    let engine = metadata_merge_path_engine(&temp, "fnode_00000000000000000000000000000000", 0);
    let view = engine.metadata_view();
    let request = request(HistoryCollection::Nodes, 64);
    let context = context();
    let cancel = HistoryWindowCancellation::new();
    cancel.cancel();
    assert!(capture_indexed_selection(&engine, &view, &request, &context, &cancel).is_err());
    assert!(matches!(
        engine.index_health(),
        crate::engine::index::IndexHealth::Ready { .. }
    ));
    // A fresh snapshot remains usable after the cancelled transaction closed.
    equal_page(&engine, &request);
}

fn auxiliary_collections() -> Vec<HistoryCollection> {
    vec![
        HistoryCollection::Branches,
        HistoryCollection::Merges,
        HistoryCollection::NodeParents {
            node_id: "ver_00000000000000000000000000000010".parse().unwrap(),
        },
    ]
}
#[test]
fn indexed_auxiliary_record_pages_match_complete_oracle_in_both_directions() {
    let temp = tempfile::tempdir().unwrap();
    let engine = indexed_engine(&temp, 31);
    for collection in auxiliary_collections() {
        for limit in [1, 3, 128] {
            let mut req = request(collection.clone(), limit);
            let mut previous = Vec::new();
            loop {
                let page = equal_page(&engine, &req);
                let cursors = match page {
                    HistoryWindowPage::Branches { branches, page, .. } => {
                        assert!(branches.iter().all(|branch| branch.branch_id.as_str()
                            != "br_00000000000000000000000000000014"));
                        page
                    }
                    HistoryWindowPage::Merges { page, .. }
                    | HistoryWindowPage::NodeParents { page, .. } => page,
                    _ => unreachable!(),
                };
                previous.extend(cursors.previous_cursor);
                match cursors.next_cursor {
                    Some(cursor) => req.cursor = Some(cursor),
                    None => break,
                }
            }
            for cursor in previous.into_iter().rev() {
                req.cursor = Some(cursor);
                equal_page(&engine, &req);
            }
        }
    }
}

#[test]
fn indexed_content_field_pages_keep_complete_utf8_and_exact_first_commit() {
    let temp = tempfile::tempdir().unwrap();
    let engine = indexed_engine(&temp, 19);
    let commit = &engine.state.commits["commit_00000000000000000000000000000010"];
    for (field, expected) in [
        (HistoryField::Message, &commit.message),
        (HistoryField::Author, &commit.author),
    ] {
        for limit in [4093, 8192] {
            let mut req = request(
                HistoryCollection::NodeField {
                    node_id: "ver_00000000000000000000000000000010".parse().unwrap(),
                    field,
                },
                limit,
            );
            let mut text = String::new();
            loop {
                let HistoryWindowPage::NodeField {
                    byte_offset,
                    total_bytes,
                    text: fragment,
                    end_of_field,
                    next_cursor,
                    ..
                } = equal_page(&engine, &req)
                else {
                    unreachable!()
                };
                assert_eq!(byte_offset, text.len() as u64);
                assert_eq!(total_bytes, expected.len() as u64);
                assert!(fragment.len() <= limit);
                text.push_str(&fragment);
                if end_of_field {
                    assert!(next_cursor.is_none());
                    break;
                }
                assert!(!fragment.is_empty());
                req.cursor = Some(next_cursor.expect("unfinished field has continuation"));
            }
            assert_eq!(&text, expected);
        }
    }
}

#[test]
fn indexed_auxiliary_missing_members_fall_back_and_bad_cursor_keeps_health() {
    use sqlite_access::Connection;
    let mut cases = auxiliary_collections()
        .into_iter()
        .zip([4, 5, 5])
        .collect::<Vec<_>>();
    cases.push((
        HistoryCollection::NodeField {
            node_id: "ver_00000000000000000000000000000010".parse().unwrap(),
            field: HistoryField::Message,
        },
        3,
    ));
    for (collection, kind) in cases {
        let temp = tempfile::tempdir().unwrap();
        let engine = indexed_engine(&temp, 0);
        let view = engine.metadata_view();
        let req = request(collection, 3);
        let ctx = context();
        let cancel = HistoryWindowCancellation::new();
        let reference = view.history_window(&req, &ctx, &cancel).unwrap();
        platform_io::sqlite::block_on_sqlite_connection(async {
            let mut c = platform_io::sqlite::open_index_connection(
                &engine.root.join("index.sqlite"),
                false,
                None,
            )
            .await
            .unwrap();
            sqlite_access::query("DELETE FROM history_scope_members WHERE kind=? AND file_id=? AND history_tree_id=?")
                .bind(kind as i64).bind(req.file_id.as_str()).bind(ctx.history_tree_id.as_str()).execute(&mut c).await.unwrap();
            c.close().await.unwrap();
        });
        let selected = capture_indexed_selection(&engine, &view, &req, &ctx, &cancel).unwrap();
        assert!(selected.is_none());
        let actual = view
            .history_window_selected(&req, &ctx, &cancel, selected.as_ref())
            .unwrap();
        assert_eq!(
            json_codec::to_value(actual).unwrap(),
            json_codec::to_value(reference).unwrap()
        );
        assert!(matches!(
            engine.index_health(),
            crate::engine::index::IndexHealth::Unavailable(_)
        ));
    }
    let temp = tempfile::tempdir().unwrap();
    let engine = indexed_engine(&temp, 0);
    let HistoryWindowPage::Branches { page, .. } =
        equal_page(&engine, &request(HistoryCollection::Branches, 1))
    else {
        unreachable!()
    };
    let mut wrong = request(HistoryCollection::Merges, 1);
    wrong.cursor = Some(page.next_cursor.unwrap());
    let view = engine.metadata_view();
    let ctx = context();
    let cancel = HistoryWindowCancellation::new();
    let reference = view.history_window(&wrong, &ctx, &cancel).unwrap_err();
    let actual = match capture_indexed_selection(&engine, &view, &wrong, &ctx, &cancel) {
        Err(error) => error,
        Ok(selected) => view
            .history_window_selected(&wrong, &ctx, &cancel, selected.as_ref())
            .unwrap_err(),
    };
    assert_eq!(actual.code(), reference.code());
    assert!(matches!(
        engine.index_health(),
        crate::engine::index::IndexHealth::Ready { .. }
    ));
}

#[test]
fn indexed_auxiliary_snapshot_keeps_old_view_after_publication_and_rebuild() {
    let mut collections = auxiliary_collections();
    collections.push(HistoryCollection::NodeField {
        node_id: "ver_00000000000000000000000000000010".parse().unwrap(),
        field: HistoryField::Message,
    });
    for collection in collections {
        let temp = tempfile::tempdir().unwrap();
        let mut engine = indexed_engine(&temp, 0);
        let view = engine.metadata_view();
        let req = request(collection, 7);
        let ctx = context();
        let snapshot = engine
            .capture_history_scope(req.file_id.as_str(), ctx.history_tree_id.as_str())
            .unwrap()
            .unwrap();
        let previous = Arc::clone(&engine.state);
        let state = Arc::make_mut(&mut engine.state);
        state.begin_tracking();
        state.branches.remove("br_00000000000000000000000000000000");
        state
            .file_branch_merges
            .remove("fmerge_00000000000000000000000000000000");
        state
            .commits
            .get_mut("commit_00000000000000000000000000000010")
            .unwrap()
            .message = "later publication".into();
        state.sequence += 1;
        engine.update_index(&previous);
        engine.rebuild_index().unwrap();
        let cancel = HistoryWindowCancellation::new();
        let selected = prepare_indexed_selection(&snapshot, &view, &req, &ctx, &cancel)
            .unwrap()
            .unwrap();
        let actual = view
            .history_window_selected(&req, &ctx, &cancel, Some(&selected))
            .unwrap();
        let expected = view.history_window(&req, &ctx, &cancel).unwrap();
        assert_eq!(
            json_codec::to_value(actual).unwrap(),
            json_codec::to_value(expected).unwrap()
        );
        engine.record_scope_failure(&snapshot);
        assert!(matches!(
            engine.index_health(),
            crate::engine::index::IndexHealth::Ready { .. }
        ));
    }
}
