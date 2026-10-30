use super::*;
use crate::{CommitRequest, Config, Error, ErrorCode};
use std::cell::Cell;

fn pool() -> (tempfile::TempDir, Engine, storage_contract::Repository) {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([temporary.path().join("disk")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("activity").unwrap(),
            "P01",
            1,
            "owner",
            OperationId::generate(),
        )
        .unwrap();
    (temporary, engine, repo)
}

fn save(engine: &mut Engine, branch: &str, path: &str) -> OperationResult {
    save_content(engine, branch, path, b"body")
}

fn save_content(engine: &mut Engine, branch: &str, path: &str, body: &[u8]) -> OperationResult {
    let workspace = engine.create_workspace(branch, "owner").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, body)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: path.into(),
            request_digest: None,
        })
        .unwrap()
}

fn proceed() -> Result<()> {
    Ok(())
}
fn stop() -> Result<()> {
    Err(Error::new(ErrorCode::RequestTimeout, "request cancelled"))
}

fn file_history_projection(
    view: &FileHistoryReadView,
    file: &str,
    history: &str,
) -> impl PartialEq + std::fmt::Debug + use<> {
    use crate::business::file_history_pages::{ContextLocation, NodeField};

    let scope = view.file_history_scope(file, history, &proceed).unwrap();
    let nodes = scope.select_nodes(None, 20).unwrap();
    let records = nodes
        .keys
        .iter()
        .map(|key| scope.node_record(key).unwrap())
        .collect::<Vec<_>>();
    let parents = nodes
        .keys
        .iter()
        .map(|key| scope.node_parent_window(&key.node_id, None, 20).unwrap())
        .collect::<Vec<_>>();
    let fields = nodes
        .keys
        .iter()
        .map(|key| {
            (
                scope
                    .node_field(&key.node_id, NodeField::Author, 0, 128)
                    .unwrap(),
                scope
                    .node_field(&key.node_id, NodeField::Message, 0, 128)
                    .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let branches = scope.select_branches(None, 20).unwrap();
    let branch_records = branches
        .keys
        .iter()
        .map(|id| scope.branch_record(id).unwrap())
        .collect::<Vec<_>>();
    (
        nodes,
        records,
        parents,
        fields,
        branches,
        branch_records,
        scope.current_context(ContextLocation::Resolve).unwrap(),
    )
}

#[test]
fn file_history_view_warm_and_recovered_cold_projections_match() {
    let (temporary, mut engine, repo) = pool();
    save(&mut engine, &repo.main_branch_id, "history.txt");
    save_content(
        &mut engine,
        &repo.main_branch_id,
        "history.txt",
        b"replacement",
    );
    let entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let warm = engine.file_history_read_view().unwrap();
    assert!(warm.view.owners.is_some());
    assert!(Arc::ptr_eq(&warm.view.state, &engine.state));
    let expected = file_history_projection(&warm, &entry.file_id, &repo.history_tree_id);
    drop(warm);
    drop(engine);

    let engine = Engine::open(&temporary.path().join("system")).unwrap();
    let cold = engine.file_history_read_view().unwrap();
    assert!(cold.view.owners.is_none());
    assert!(cold.owners.get().is_none());
    assert!(engine.available_history_owners().is_none());
    assert_eq!(
        file_history_projection(&cold, &entry.file_id, &repo.history_tree_id),
        expected
    );
    assert!(cold.owners.get().is_some());
    assert!(engine.available_history_owners().is_some());
    assert!(std::ptr::eq(
        cold.owners.get().unwrap(),
        engine.history_owners().unwrap()
    ));
}

#[test]
fn file_history_view_retains_old_generation_resources_after_publication_and_engine_drop() {
    let (_temporary, mut engine, repo) = pool();
    save(&mut engine, &repo.main_branch_id, "history.txt");
    let entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let old = engine.file_history_read_view().unwrap();
    let expected = file_history_projection(&old, &entry.file_id, &repo.history_tree_id);
    save_content(
        &mut engine,
        &repo.main_branch_id,
        "history.txt",
        b"replacement",
    );
    let current = engine.file_history_read_view().unwrap();
    assert!(!Arc::ptr_eq(&old.view.state, &current.view.state));
    assert!(!Arc::ptr_eq(&old.owners, &current.owners));
    assert_ne!(
        file_history_projection(&current, &entry.file_id, &repo.history_tree_id),
        expected
    );
    drop(current);
    drop(engine);
    assert_eq!(
        file_history_projection(&old, &entry.file_id, &repo.history_tree_id),
        expected
    );
}

#[test]
fn file_history_cold_cancellation_leaves_shared_cell_empty_and_retry_publishes_complete_index() {
    let (temporary, mut engine, repo) = pool();
    save(&mut engine, &repo.main_branch_id, "history.txt");
    let entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    drop(engine);
    let engine = Engine::open(&temporary.path().join("system")).unwrap();
    let first = engine.file_history_read_view().unwrap();
    let second = engine.file_history_read_view().unwrap();
    assert!(Arc::ptr_eq(&first.owners, &second.owners));
    let calls = Cell::new(0);
    let cancel = || {
        calls.set(calls.get() + 1);
        if calls.get() == 4 { stop() } else { proceed() }
    };
    let failed = first.file_history_scope(&entry.file_id, &repo.history_tree_id, &cancel);
    assert_eq!(failed.err().unwrap().code(), ErrorCode::RequestTimeout);
    assert_eq!(calls.get(), 4);
    assert!(first.owners.get().is_none());
    assert!(engine.available_history_owners().is_none());
    let expected = file_history_projection(&second, &entry.file_id, &repo.history_tree_id);
    assert!(first.owners.get().is_some());
    assert_eq!(
        file_history_projection(&first, &entry.file_id, &repo.history_tree_id),
        expected
    );
}

#[test]
fn file_history_cold_namespace_failure_does_not_install_an_index_and_can_retry() {
    let (temporary, mut engine, repo) = pool();
    save(&mut engine, &repo.main_branch_id, "history.txt");
    let entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let warm = engine.file_history_read_view().unwrap();
    let expected = file_history_projection(&warm, &entry.file_id, &repo.history_tree_id);
    drop(warm);
    drop(engine);
    let engine = Engine::open(&temporary.path().join("system")).unwrap();
    let mut cold = engine.file_history_read_view().unwrap();
    cold.view.caches = Arc::new(SegmentCaches::new());
    let root = &cold.state().commits.values().next().unwrap().namespace.root;
    let kind = match root.kind {
        storage_contract::NamespaceRefKind::Page => storage_contract::NAMESPACE_PAGE_KIND,
        storage_contract::NamespaceRefKind::Index => storage_contract::NAMESPACE_INDEX_KIND,
    };
    let originals = cold
        .view
        .members
        .iter()
        .map(|member| {
            let path = crate::journal::object_path(member, kind, &root.hash);
            let bytes = std::fs::read(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    assert!(
        cold.file_history_scope(&entry.file_id, &repo.history_tree_id, &proceed)
            .is_err()
    );
    assert!(cold.owners.get().is_none());
    assert!(engine.available_history_owners().is_none());
    for (path, bytes) in originals {
        std::fs::write(path, bytes).unwrap();
    }
    assert_eq!(
        file_history_projection(&cold, &entry.file_id, &repo.history_tree_id),
        expected
    );
    assert!(engine.available_history_owners().is_some());
}

#[test]
fn file_history_cold_build_does_not_hold_cell_initializer_while_observer_is_blocked() {
    use std::{sync::mpsc, time::Duration};

    let (temporary, mut engine, repo) = pool();
    save(&mut engine, &repo.main_branch_id, "history.txt");
    let entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    drop(engine);
    let engine = Engine::open(&temporary.path().join("system")).unwrap();
    let first = engine.file_history_read_view().unwrap();
    let second = engine.file_history_read_view().unwrap();
    let cell = Arc::clone(&first.owners);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (completed_tx, completed_rx) = mpsc::channel();
    let file = entry.file_id.as_str();
    let history = repo.history_tree_id.as_str();

    std::thread::scope(|threads| {
        // Own the release sender in this scope closure: early assertion
        // unwinding drops it before scoped workers are joined.
        let release_first = release_tx;
        let first_worker = threads.spawn(move || {
            let calls = Cell::new(0);
            let observe = || {
                calls.set(calls.get() + 1);
                // Enter the derivation, not the view's first admission check.
                if calls.get() == 3 {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }
                Ok(())
            };
            first
                .file_history_scope(file, history, &observe)
                .unwrap()
                .select_nodes(None, 20)
                .unwrap()
        });
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(cell.get().is_none());
        let second_worker = threads.spawn(move || {
            let nodes = second
                .file_history_scope(file, history, &proceed)
                .unwrap()
                .select_nodes(None, 20)
                .unwrap();
            completed_tx.send(()).unwrap();
            nodes
        });
        // Always release before asserting: a regression that scans inside
        // OnceLock must fail the test, not leave its waiting worker stranded.
        let completed_while_first_blocked =
            completed_rx.recv_timeout(Duration::from_secs(10)).is_ok();
        release_first.send(()).unwrap();
        let first_nodes = first_worker.join().unwrap();
        let second_nodes = second_worker.join().unwrap();
        assert!(completed_while_first_blocked);
        assert_eq!(first_nodes, second_nodes);
    });
    assert!(std::ptr::eq(
        cell.get().unwrap(),
        engine.history_owners().unwrap()
    ));
}

#[test]
fn file_history_cold_owner_cell_survives_only_plans_rebinding_not_foreign_state() {
    let (temporary, mut engine, repo) = pool();
    save(&mut engine, &repo.main_branch_id, "history.txt");
    let entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    drop(engine);
    let mut engine = Engine::open(&temporary.path().join("system")).unwrap();
    let old = engine.file_history_read_view().unwrap();
    assert!(old.owners.get().is_none());
    engine.install_local_plan(storage_contract::Plan {
        plan_id: "local-owner-test".into(),
        kind: storage_contract::PlanKind::Purge,
        principal: "owner".into(),
        expected_sequence: engine.state.business_generation,
        digest: "0".repeat(64),
        input: json_codec::Value::Null,
        state: storage_contract::PlanState::Ready,
    });
    let rebound = engine.file_history_read_view().unwrap();
    assert!(!Arc::ptr_eq(&old.view.state, &rebound.view.state));
    assert!(Arc::ptr_eq(&old.owners, &rebound.owners));
    assert!(!old.state().plans.contains_key("local-owner-test"));
    assert!(rebound.state().plans.contains_key("local-owner-test"));
    let expected = file_history_projection(&old, &entry.file_id, &repo.history_tree_id);
    assert_eq!(
        file_history_projection(&rebound, &entry.file_id, &repo.history_tree_id),
        expected
    );
    Arc::make_mut(&mut engine.state)
        .versions
        .get_mut(&entry.version_id)
        .unwrap()
        .file_id = "foreign-file".into();
    assert_eq!(
        engine.file_history_read_view().err().unwrap().code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        file_history_projection(&old, &entry.file_id, &repo.history_tree_id),
        expected
    );
}

fn collect_receipts(
    mut receipts: crate::engine::operations::ReceiptsDescending<'_>,
) -> Result<Vec<storage_contract::OperationRecordEntry>> {
    let mut records = Vec::new();
    while let Some(record) = receipts.next_record() {
        records.push(record?.clone());
    }
    Ok(records)
}

#[test]
fn activity_plan_keeps_exact_root_receipts_and_namespace_after_publication_and_engine_drop() {
    let (_temporary, mut engine, repo) = pool();
    let first = save(&mut engine, &repo.main_branch_id, "first.txt");
    let plan = engine.published_read_view();
    assert!(Arc::ptr_eq(&plan.state, &engine.state));
    let expected = engine.list_tree(&repo.main_branch_id).unwrap();
    let receipts = collect_receipts(engine.receipts_descending()).unwrap();
    let second = save(&mut engine, &repo.main_branch_id, "second.txt");
    assert!(!Arc::ptr_eq(&plan.state, &engine.state));
    assert!(
        plan.published_receipt(second.operation_id, &proceed)
            .unwrap()
            .is_none()
    );
    drop(engine);
    assert_eq!(
        plan.namespace_tree(&repo.main_branch_id, &proceed)
            .unwrap()
            .entries()
            .unwrap(),
        expected
    );
    assert_eq!(
        plan.published_receipt(first.operation_id, &proceed)
            .unwrap()
            .unwrap(),
        first
    );
    assert_eq!(
        collect_receipts(plan.receipts_descending(&proceed)).unwrap(),
        receipts
    );
    assert_eq!(
        plan.receipt_by_sequence(first.sequence, &proceed)
            .unwrap()
            .unwrap()
            .result,
        first
    );
}

#[test]
fn published_view_keeps_pending_sources_and_current_contexts_on_its_exact_root() {
    let (_temporary, mut engine, repo) = pool();
    save(&mut engine, &repo.main_branch_id, "context.txt");
    let entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let key =
        crate::business::directory::context_key_from_key(&repo.repository_id, entry.lookup_key());
    let state = Arc::make_mut(&mut engine.state);
    state.file_contexts.remove(&key);
    state.file_selections.remove(&entry.file_id);
    state
        .branches
        .get_mut(&repo.main_branch_id)
        .unwrap()
        .pending_file_versions
        .insert(entry.file_id.clone(), entry.version_id.clone());
    let expected = engine
        .file_context_for_current_entry(&repo.repository_id, &entry)
        .unwrap()
        .into_owned();
    let synthesized = engine.published_read_view();
    assert!(matches!(
        synthesized.file_context_for_current_entry(&repo.repository_id, &entry),
        Some(Cow::Owned(_))
    ));
    let mut registered = expected.clone();
    registered.selection_generation = 7;
    Arc::make_mut(&mut engine.state)
        .file_contexts
        .insert(key.clone(), registered.clone());
    let explicit = engine.published_read_view();
    assert!(matches!(
        explicit.file_context_for_current_entry(&repo.repository_id, &entry),
        Some(Cow::Borrowed(_))
    ));
    let state = Arc::make_mut(&mut engine.state);
    state
        .file_contexts
        .get_mut(&key)
        .unwrap()
        .selection_generation = 8;
    state
        .branches
        .get_mut(&repo.main_branch_id)
        .unwrap()
        .pending_file_versions
        .remove(&entry.file_id);
    assert_eq!(
        engine.pending_file_source(&repo.main_branch_id, &entry.file_id),
        None
    );
    drop(engine);
    for view in [&synthesized, &explicit] {
        assert_eq!(
            view.pending_file_source(&repo.main_branch_id, &entry.file_id),
            Some(entry.version_id.as_str())
        );
    }
    assert_eq!(
        synthesized
            .file_context_for_current_entry(&repo.repository_id, &entry)
            .unwrap()
            .as_ref(),
        &expected
    );
    assert_eq!(
        explicit
            .file_context_for_current_entry(&repo.repository_id, &entry)
            .unwrap()
            .as_ref(),
        &registered
    );
}

#[test]
fn activity_capture_preserves_warm_owner_lookup_and_rejects_foreign_state_membership() {
    let (_temporary, mut engine, repo) = pool();
    save(&mut engine, &repo.main_branch_id, "owner.txt");
    let entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    engine.history_owners().unwrap();
    let warm = engine.published_read_view();
    assert!(warm.owners.is_some());
    let calls = Cell::new(0);
    let observe = || {
        calls.set(calls.get() + 1);
        Ok(())
    };
    assert_eq!(
        warm.file_history_owner(&entry.file_id, &observe).unwrap(),
        Some(repo.history_tree_id.as_str())
    );
    assert_eq!(calls.get(), 1);
    Arc::make_mut(&mut engine.state)
        .versions
        .get_mut(&entry.version_id)
        .unwrap()
        .file_id = "file_rebound".into();
    let cold = engine.published_read_view();
    assert!(cold.owners.is_none());
    assert!(engine.available_history_owners().is_none());
    assert_eq!(
        cold.file_history_owner(&entry.file_id, &proceed).unwrap(),
        None
    );
    assert_eq!(
        cold.file_history_owner("file_rebound", &proceed).unwrap(),
        Some(repo.history_tree_id.as_str())
    );
    assert_eq!(
        warm.file_history_owner(&entry.file_id, &proceed).unwrap(),
        Some(repo.history_tree_id.as_str())
    );
}

#[test]
fn activity_cold_capture_does_not_build_owner_index_and_namespace_observes_cancellation() {
    let (temporary, mut engine, repo) = pool();
    let result = save(&mut engine, &repo.main_branch_id, "cold.txt");
    drop(engine);
    let engine = Engine::open(&temporary.path().join("system")).unwrap();
    assert!(engine.available_history_owners().is_none());
    let plan = engine.published_read_view();
    assert!(plan.owners.is_none());
    assert!(engine.available_history_owners().is_none());
    let tree = plan
        .commit_namespace(result.commit_id.as_ref().unwrap(), &stop)
        .unwrap();
    assert_eq!(
        tree.entries().unwrap_err().code(),
        ErrorCode::RequestTimeout
    );
    assert_eq!(
        plan.published_receipt(result.operation_id, &stop)
            .unwrap_err()
            .code(),
        ErrorCode::RequestTimeout
    );
    assert_eq!(
        plan.receipts_descending(&stop)
            .next_record()
            .unwrap()
            .unwrap_err()
            .code(),
        ErrorCode::RequestTimeout
    );
    assert_eq!(
        plan.receipt_by_sequence(result.sequence, &stop)
            .unwrap_err()
            .code(),
        ErrorCode::RequestTimeout
    );
    assert_eq!(
        plan.file_history_owner("missing", &stop)
            .unwrap_err()
            .code(),
        ErrorCode::RequestTimeout
    );
}

#[test]
fn activity_plan_uses_captured_cold_receipt_segments_and_preserves_io_failures() {
    use storage_contract::{
        OPERATION_SEGMENT_KIND, OPERATION_SEGMENT_RECORDS, OperationLog, OperationRecordEntry,
        OperationSegment,
    };
    let (_temporary, mut engine, repo) = pool();
    let template = save(&mut engine, &repo.main_branch_id, "receipt.txt");
    let records: Vec<_> = (0..OPERATION_SEGMENT_RECORDS)
        .map(|i| {
            let mut result = template.clone();
            result.operation_id = OperationId::generate();
            result.sequence = i as u64 + 1;
            OperationRecordEntry {
                principal: "owner".into(),
                result,
            }
        })
        .collect();
    let mut log = OperationLog::from_records(records.clone()).unwrap();
    assert_eq!(log.sealed().len(), 1);
    let reference = log.sealed()[0].clone();
    let segment = OperationSegment {
        format: Default::default(),
        records: records.clone(),
    };
    for member in engine.members() {
        assert_eq!(
            crate::journal::store_object(&member, OPERATION_SEGMENT_KIND, &segment).unwrap(),
            reference.hash
        );
    }
    log.begin_tracking();
    assert!(log.fresh_seal().is_none());
    let locator = crate::journal::stored_map(
        &engine.members(),
        records
            .iter()
            .enumerate()
            .map(|(ordinal, entry)| {
                (
                    entry.result.operation_id.to_string(),
                    json_codec::Value::String(ordinal.to_string()),
                )
            })
            .collect(),
    );
    log.release_cold_index();
    assert!(log.cold_index().is_none());
    Arc::make_mut(&mut engine.state).operations = log;
    engine.operation_locator_root = Some(locator);
    let plan = engine.published_read_view();
    // A different publication's locator cannot alter this captured point read.
    engine.operation_locator_root = Some(crate::journal::stored_map(
        &engine.members(),
        json_codec::Map::new(),
    ));
    drop(engine);
    assert_eq!(
        plan.published_receipt(records[0].result.operation_id, &proceed)
            .unwrap()
            .unwrap(),
        records[0].result
    );
    assert_eq!(
        plan.receipt_by_sequence(1, &proceed).unwrap().unwrap(),
        records[0]
    );
    assert_eq!(
        collect_receipts(plan.receipts_descending(&proceed)).unwrap(),
        records.iter().rev().cloned().collect::<Vec<_>>()
    );
    let mut plan = plan;
    plan.caches = Arc::new(SegmentCaches::new());
    for member in &plan.members {
        std::fs::remove_file(crate::journal::object_path(
            member,
            OPERATION_SEGMENT_KIND,
            &reference.hash,
        ))
        .unwrap();
    }
    assert!(plan.receipt_by_sequence(1, &proceed).is_err());
    assert!(
        plan.published_receipt(records[0].result.operation_id, &proceed)
            .is_err()
    );
    assert!(
        plan.receipts_descending(&proceed)
            .next_record()
            .unwrap()
            .is_err()
    );
}

fn mixed_receipt_plan() -> (tempfile::TempDir, PublishedReadView) {
    use storage_contract::{
        OPERATION_SEGMENT_KIND, OPERATION_SEGMENT_RECORDS, OperationLog, OperationRecordEntry,
        OperationSegment,
    };
    let (temporary, mut engine, repo) = pool();
    let template = save(&mut engine, &repo.main_branch_id, "mixed.txt");
    let records: Vec<_> = (0..2 * OPERATION_SEGMENT_RECORDS + 2)
        .map(|index| {
            let mut result = template.clone();
            result.operation_id = OperationId::generate();
            result.sequence = index as u64 + 1;
            OperationRecordEntry {
                principal: "owner".into(),
                result,
            }
        })
        .collect();
    let mut log = OperationLog::from_records(records.clone()).unwrap();
    assert_eq!(log.sealed().len(), 2);
    assert_eq!(log.tail().len(), 2);
    for (reference, records) in log
        .sealed()
        .iter()
        .zip(records.chunks(OPERATION_SEGMENT_RECORDS))
    {
        let segment = OperationSegment {
            format: Default::default(),
            records: records.to_vec(),
        };
        for member in engine.members() {
            assert_eq!(
                crate::journal::store_object(&member, OPERATION_SEGMENT_KIND, &segment).unwrap(),
                reference.hash
            );
        }
    }
    log.begin_tracking();
    Arc::make_mut(&mut engine.state).operations = log;
    (temporary, engine.published_read_view())
}

#[test]
fn activity_receipt_cursor_borrows_tail_and_cold_cached_segments_in_reverse_order() {
    let (_temporary, plan) = mixed_receipt_plan();
    let mut receipts = plan.receipts_descending(&proceed);
    for expected in plan.state.operations.tail().iter().rev() {
        assert!(std::ptr::eq(
            receipts.next_record().unwrap().unwrap(),
            expected
        ));
    }
    for reference in plan.state.operations.sealed().iter().rev() {
        // The first advance loads this segment; the second cursor reads the
        // same admitted cache entry without copying either record.
        let mut cached_cursor = plan.receipts_descending(&proceed);
        let first = receipts.next_record().unwrap().unwrap();
        let segment = plan.reader(&proceed).operation_segment(reference).unwrap();
        assert!(std::ptr::eq(first, segment.last().unwrap()));
        loop {
            let cached = cached_cursor.next_record().unwrap().unwrap();
            if cached.result.sequence == first.result.sequence {
                assert!(std::ptr::eq(cached, first));
                break;
            }
        }
        for expected in segment[..segment.len() - 1].iter().rev() {
            assert!(std::ptr::eq(
                receipts.next_record().unwrap().unwrap(),
                expected
            ));
        }
    }
    assert!(receipts.next_record().is_none());
    assert!(receipts.next_record().is_none());
}

#[test]
fn activity_receipt_cursor_cancellation_keeps_position_and_segment_io_error_advances() {
    let (_temporary, plan) = mixed_receipt_plan();
    let cancelled = Cell::new(true);
    let observe = || if cancelled.get() { stop() } else { proceed() };
    let mut receipts = plan.receipts_descending(&observe);
    assert_eq!(
        receipts.next_record().unwrap().unwrap_err().code(),
        ErrorCode::RequestTimeout
    );
    cancelled.set(false);
    for expected in plan.state.operations.tail().iter().rev() {
        assert!(std::ptr::eq(
            receipts.next_record().unwrap().unwrap(),
            expected
        ));
    }
    let newest = plan.state.operations.sealed().last().unwrap();
    for member in &plan.members {
        std::fs::remove_file(crate::journal::object_path(
            member,
            storage_contract::OPERATION_SEGMENT_KIND,
            &newest.hash,
        ))
        .unwrap();
    }
    assert!(receipts.next_record().unwrap().is_err());
    let older = plan
        .reader(&proceed)
        .operation_segment(&plan.state.operations.sealed()[0])
        .unwrap();
    assert!(std::ptr::eq(
        receipts.next_record().unwrap().unwrap(),
        older.last().unwrap()
    ));
    cancelled.set(true);
    assert_eq!(
        receipts.next_record().unwrap().unwrap_err().code(),
        ErrorCode::RequestTimeout
    );
    cancelled.set(false);
    for expected in older[..older.len() - 1].iter().rev() {
        assert!(std::ptr::eq(
            receipts.next_record().unwrap().unwrap(),
            expected
        ));
    }
    assert!(receipts.next_record().is_none());
}

#[test]
fn activity_receipt_cursor_segment_checkpoint_cancellation_advances_past_segment() {
    let (_temporary, plan) = mixed_receipt_plan();
    let calls = Cell::new(0);
    let observe = || {
        calls.set(calls.get() + 1);
        if calls.get() == 4 { stop() } else { proceed() }
    };
    let mut receipts = plan.receipts_descending(&observe);
    for expected in plan.state.operations.tail().iter().rev() {
        assert!(std::ptr::eq(
            receipts.next_record().unwrap().unwrap(),
            expected
        ));
    }
    assert_eq!(
        receipts.next_record().unwrap().unwrap_err().code(),
        ErrorCode::RequestTimeout
    );
    assert_eq!(calls.get(), 4);
    let older = plan
        .reader(&proceed)
        .operation_segment(&plan.state.operations.sealed()[0])
        .unwrap();
    for expected in older.iter().rev() {
        assert!(std::ptr::eq(
            receipts.next_record().unwrap().unwrap(),
            expected
        ));
    }
    assert!(receipts.next_record().is_none());
}

#[test]
fn detached_comparison_owns_exact_entries_and_survives_writer_and_resource_drop() {
    let (_temporary, mut engine, repo) = pool();
    save_content(&mut engine, &repo.main_branch_id, "File.txt", b"old");
    let left = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    save_content(&mut engine, &repo.main_branch_id, "File.txt", b"new");
    let right = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let expected_before = engine.list_tree_at(&repo.main_branch_id, &left).unwrap();
    let expected_after = engine.list_tree_at(&repo.main_branch_id, &right).unwrap();
    let left_record = engine.state.commits.get_shared(&left).unwrap();
    let weak = Arc::downgrade(&left_record);
    let view = engine.published_read_view();
    save(&mut engine, &repo.main_branch_id, "later.txt");
    drop(engine);
    let before_count = Arc::strong_count(&left_record);
    let comparison = view.compare_commits(&left, &right, None).unwrap();
    assert_eq!(Arc::strong_count(&left_record), before_count + 1);
    drop(view);
    drop(left_record);
    assert!(weak.upgrade().is_some());
    let storage_contract::CommitChange::Modified { before, after } =
        comparison.changes().next().unwrap()
    else {
        panic!("expected modification")
    };
    assert_eq!(before, &expected_before[0]);
    assert_eq!(after, &expected_after[0]);
    assert_eq!(comparison.changes().len(), 1);
    drop(comparison);
    assert!(weak.upgrade().is_none());
}

#[test]
fn comparison_resolves_both_ids_before_io_then_reads_left_before_right_even_when_equal() {
    let (_temporary, mut engine, repo) = pool();
    save(&mut engine, &repo.main_branch_id, "left.txt");
    let left = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    save(&mut engine, &repo.main_branch_id, "right.txt");
    let right = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let mut view = engine.published_read_view();
    view.caches = Arc::new(SegmentCaches::new());
    let root = &view.state.commits[&left].namespace.root;
    let kind = match root.kind {
        storage_contract::NamespaceRefKind::Page => storage_contract::NAMESPACE_PAGE_KIND,
        storage_contract::NamespaceRefKind::Index => storage_contract::NAMESPACE_INDEX_KIND,
    };
    let originals = view
        .members
        .iter()
        .map(|member| {
            let path = crate::journal::object_path(member, kind, &root.hash);
            let bytes = std::fs::read(&path).unwrap();
            std::fs::write(&path, b"truncated").unwrap();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    // Right has an independently invalid reference, but cannot mask left I/O.
    Arc::make_mut(&mut view.state)
        .commits
        .get_mut(&right)
        .unwrap()
        .namespace
        .root
        .hash = "invalid-right".into();
    let calls = Cell::new(0);
    let observe = || {
        calls.set(calls.get() + 1);
        Ok(())
    };
    let missing = view
        .compare_commits(&left, "missing", Some(&observe))
        .unwrap_err();
    assert_eq!(missing.code(), ErrorCode::CommitNotFound);
    assert_eq!(missing.message(), "right commit missing");
    assert_eq!(calls.get(), 0);
    assert_eq!(
        view.compare_commits("missing", "missing", None)
            .unwrap_err()
            .message(),
        "left commit missing"
    );
    let left_failure = view.compare_commits(&left, &left, None).unwrap_err();
    let ordered_failure = view.compare_commits(&left, &right, None).unwrap_err();
    assert_eq!(ordered_failure.code(), left_failure.code());
    assert_eq!(ordered_failure.message(), left_failure.message());
    assert!(std::error::Error::source(&ordered_failure).is_some());
    for (path, bytes) in originals {
        std::fs::write(path, bytes).unwrap();
    }
    let right_failure = view.compare_commits(&left, &right, None).unwrap_err();
    assert_ne!(right_failure.message(), left_failure.message());
    // Same IDs traverse both namespaces: cancel upon entering the second side.
    let baseline_calls = Cell::new(0);
    let baseline = || {
        baseline_calls.set(baseline_calls.get() + 1);
        Ok(())
    };
    let equal = engine.published_read_view();
    equal
        .compare_commits(&left, &left, Some(&baseline))
        .unwrap();
    // For one entry: each side has start + page read + entry, then six algorithm checks.
    assert_eq!(baseline_calls.get(), 12);
    let count = Cell::new(0);
    let cancel = || {
        count.set(count.get() + 1);
        if count.get() == 4 {
            Err(storage_contract::Error::new(
                ErrorCode::RequestTimeout,
                "second namespace",
            ))
        } else {
            Ok(())
        }
    };
    assert_eq!(
        equal
            .compare_commits(&left, &left, Some(&cancel))
            .unwrap_err()
            .code(),
        ErrorCode::RequestTimeout
    );
    assert_eq!(count.get(), 4);
}
