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
    let workspace = engine.create_workspace(branch, "owner").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, b"body")
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

#[test]
fn activity_plan_keeps_exact_root_receipts_and_namespace_after_publication_and_engine_drop() {
    let (_temporary, mut engine, repo) = pool();
    let first = save(&mut engine, &repo.main_branch_id, "first.txt");
    let plan = engine.prepare_activity_read();
    assert!(Arc::ptr_eq(&plan.state, &engine.state));
    let expected = engine.list_tree(&repo.main_branch_id).unwrap();
    let receipts = engine
        .receipts_descending()
        .collect::<Result<Vec<_>>>()
        .unwrap();
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
        plan.receipts_descending(&proceed)
            .collect::<Result<Vec<_>>>()
            .unwrap(),
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
fn activity_capture_preserves_warm_owner_lookup_and_rejects_foreign_state_membership() {
    let (_temporary, mut engine, repo) = pool();
    save(&mut engine, &repo.main_branch_id, "owner.txt");
    let entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    engine.history_owners().unwrap();
    let warm = engine.prepare_activity_read();
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
    let cold = engine.prepare_activity_read();
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
    let plan = engine.prepare_activity_read();
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
            .next()
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
    let plan = engine.prepare_activity_read();
    // A different publication's locator cannot alter this captured point read.
    engine.operation_locator_root = Some(crate::journal::stored_map(
        &engine.members(),
        std::collections::BTreeMap::new(),
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
        plan.receipts_descending(&proceed)
            .collect::<Result<Vec<_>>>()
            .unwrap(),
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
    assert!(plan.receipts_descending(&proceed).next().unwrap().is_err());
}
