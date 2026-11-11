use super::*;
use std::error::Error as _;
use std::sync::Arc;

#[test]
fn oversized_historical_copy_count_retains_source_and_leaves_plan_unpublished() {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("primary")]).unwrap(),
    )
    .unwrap();
    let repository = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("integer boundary").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let source = engine.state.branches[&repository.main_branch_id]
        .head_commit_id
        .clone();
    let original = engine
        .create_historical_save_plan(
            &repository.history_tree_id,
            HistoricalSaveOptions {
                source_commit_id: source.clone(),
                name: "range rejection".into(),
                workspace_id: None,
                primary_slot_id: Some("P01".into()),
                target_copies: Some(1),
            },
            "admin",
        )
        .unwrap();
    let original_bytes = canonical_json(&original).unwrap();
    let operation = storage_contract::OperationId::generate().unwrap();

    // The public options use u32. Inject only the decoded plan's numeric input,
    // with a valid digest, so the real execution reaches its u64-to-u32 boundary.
    // This value is exactly representable by canonical JSON and needs no payload.
    let mut oversized = original.clone();
    oversized.input["target_copies"] = json!(u64::from(u32::MAX) + 1);
    oversized.digest = object_hash_value("plan", &oversized.input).unwrap();
    Arc::make_mut(&mut engine.state)
        .plans
        .insert(original.plan_id.clone(), oversized.clone());
    let state_before = canonical_json(engine.state.as_ref()).unwrap();
    let snapshot_before = engine.snapshot.clone();
    let next_slot_before = engine.next_slot;
    let blocked_before = engine.is_read_only();

    let error = engine
        .commit_historical_save_plan(&original.plan_id, "admin", operation)
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::PlanInvalid);
    assert!(!error.is_retryable());
    assert!(error.published_result().is_none());
    assert!(matches!(error, Error::Context { .. }));
    let cause = error.source().unwrap().downcast_ref::<Error>().unwrap();
    assert!(matches!(cause, Error::IntegerRange(_)));
    let integer_cause = cause.source().unwrap();
    assert!(integer_cause.is::<std::num::TryFromIntError>());
    assert!(integer_cause.source().is_none());

    assert_eq!(canonical_json(engine.state.as_ref()).unwrap(), state_before);
    assert_eq!(engine.snapshot, snapshot_before);
    assert_eq!(engine.next_slot, next_slot_before);
    assert_eq!(engine.is_read_only(), blocked_before);
    assert!(!engine.state.operations.contains_key(&operation));
    let retained = engine.plan(&original.plan_id).unwrap();
    assert_eq!(retained.state, PlanState::Ready);
    assert_eq!(
        canonical_json(&retained).unwrap(),
        canonical_json(&oversized).unwrap()
    );

    // Remove only the injected record. The original durable preview remains
    // ready, and the failed call has not consumed its operation identity.
    Arc::make_mut(&mut engine.state)
        .plans
        .remove(&original.plan_id);
    let restored = engine.plan(&original.plan_id).unwrap();
    assert_eq!(canonical_json(&restored).unwrap(), original_bytes);
    assert_eq!(restored.state, PlanState::Ready);
    let sequence = engine.state.sequence;
    let saved = engine
        .commit_historical_save_plan(&original.plan_id, "admin", operation)
        .unwrap();
    assert_eq!(saved.operation_id, operation);
    assert_eq!(engine.state.sequence, sequence + 1);
    assert!(matches!(saved.commit_state, CommitState::Published));
    assert_eq!(
        engine.plan(&original.plan_id).unwrap().state,
        PlanState::Completed
    );
    assert_eq!(
        engine.state.branches[&repository.main_branch_id].head_commit_id,
        source
    );
    assert!(saved.branch_id.is_some());
    assert!(saved.commit_id.is_some());
}
