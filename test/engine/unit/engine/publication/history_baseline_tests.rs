use super::*;
use std::sync::Arc;

fn engine(root: &std::path::Path) -> Engine {
    let mut engine = Engine::initialize(
        &root.join("state"),
        Config::development([root.join("a"), root.join("b"), root.join("c")]),
    )
    .unwrap();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("first").unwrap(),
            "P01",
            1,
            "alice",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
}
fn config_hash(engine: &Engine) -> String {
    object_hash("publication-config", &engine.config).unwrap()
}

#[test]
fn history_baseline_requires_exact_state_allocation_snapshot_sequence_and_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    let base = engine.published_state_base.clone().unwrap();
    assert!(
        base.snapshot_basis
            .as_ref()
            .unwrap()
            .matches_snapshot(&base.snapshot)
    );
    assert!(base.matches(&engine.planning_context(), &config_hash(&engine)));
    assert!(!base.matches(&engine.planning_context(), "wrong config hash"));
    let saved = engine.snapshot.take();
    assert!(!base.matches(&engine.planning_context(), &config_hash(&engine)));
    engine.snapshot = saved;
    // Same values and sequence do not authorize another allocation. A Weak
    // baseline also forces Arc::make_mut to detach even with one strong owner.
    let bytes = canonical_json(engine.state()).unwrap();
    Arc::make_mut(&mut engine.state).plans.clear();
    assert_eq!(canonical_json(engine.state()).unwrap(), bytes);
    assert!(!base.matches(&engine.planning_context(), &config_hash(&engine)));
    engine
        .persist_auth_state(
            json!({"auth":"next"}),
            "alice",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert!(
        engine
            .published_state_base
            .as_ref()
            .unwrap()
            .matches(&engine.planning_context(), &config_hash(&engine))
    );
    let published = engine.published_state_base.as_ref().unwrap();
    assert!(
        published
            .snapshot_basis
            .as_ref()
            .unwrap()
            .matches_snapshot(engine.snapshot.as_deref().unwrap())
    );
    let root = engine.root.clone();
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    assert!(
        engine.published_state_base.is_none(),
        "restart never trusts an in-memory publication receipt"
    );
}

#[test]
fn preparation_and_indeterminate_failure_never_advance_the_baseline() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    let base = engine.published_state_base.clone().unwrap();
    engine.inject_fault_once("after_payloads");
    assert!(
        engine
            .persist_auth_state(
                json!({"auth":"prepared"}),
                "alice",
                storage_contract::OperationId::generate()
            )
            .is_err()
    );
    assert!(Arc::ptr_eq(
        &base,
        engine.published_state_base.as_ref().unwrap()
    ));
    assert!(base.matches(&engine.planning_context(), &config_hash(&engine)));
    engine.inject_fault_once("after_accept_1");
    let error = engine
        .persist_auth_state(
            json!({"auth":"accepted"}),
            "alice",
            storage_contract::OperationId::generate(),
        )
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::InDoubt);
    assert!(error.is_retryable());
    let source = std::error::Error::source(&error).expect("publication retains its cause");
    assert!(source.to_string().contains("after_accept_1"));
    assert!(engine.blocked);
    assert!(Arc::ptr_eq(
        &base,
        engine.published_state_base.as_ref().unwrap()
    ));
}

#[test]
fn post_publication_index_failure_does_not_mint_a_new_reuse_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    let base = engine.published_state_base.clone().unwrap();
    let index = engine.root.join("index.sqlite");
    std::fs::rename(&index, engine.root.join("saved-index.sqlite")).unwrap();
    std::fs::create_dir(&index).unwrap();
    let operation = storage_contract::OperationId::generate();
    assert!(
        engine
            .persist_auth_state(json!({"auth":"published"}), "alice", operation)
            .is_err()
    );
    assert!(engine.blocked);
    assert!(engine.state.operations.contains_key(&operation));
    assert!(Arc::ptr_eq(
        &base,
        engine.published_state_base.as_ref().unwrap()
    ));
    assert!(!base.matches(&engine.planning_context(), &config_hash(&engine)));
}
