use super::*;
use storage_contract::{CommitRequest, Config, NamespaceOperation};

fn control() -> crate::HistoryReadControl {
    crate::HistoryReadControl::until(std::time::Instant::now() + std::time::Duration::from_secs(30))
}

#[test]
fn activity_completion_reauthorizes_before_results_and_read_errors() {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([temporary.path().join("disk")]),
    )
    .unwrap();
    let identity = install_test_administrator(&mut engine, "owner");
    let backend = EngineBackend::new(engine);
    let plan = backend.engine.lock().unwrap().prepare_activity_read();
    let page = file_manager::activity::read(plan, &control()).unwrap();
    let failure = || {
        ApiError::new(
            ::http::StatusCode::INTERNAL_SERVER_ERROR,
            storage_contract::ErrorCode::ObjectCorrupt,
            "selected namespace is corrupt",
        )
    };
    assert_eq!(
        backend
            .finish_activity_history(&identity, Err(failure()), &control())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::ObjectCorrupt
    );
    install_test_administrator(&mut backend.engine.lock().unwrap(), "replacement");
    for page in [Ok(page), Err(failure())] {
        assert_eq!(
            backend
                .finish_activity_history(&identity, page, &control())
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
}

#[test]
fn activity_plan_remains_frozen_while_the_writer_publishes() {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([temporary.path().join("disk")]),
    )
    .unwrap();
    let identity = install_test_administrator(&mut engine, "owner");
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("audit").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let backend = EngineBackend::new(engine);
    let plan = backend.engine.lock().unwrap().prepare_activity_read();
    {
        let mut engine = backend.engine.lock().unwrap();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "owner")
            .unwrap();
        for kind in ["mkdir", "delete"] {
            engine
                .namespace_operation(
                    &workspace.workspace_id,
                    NamespaceOperation::parse(kind.parse().unwrap(), "later", None).unwrap(),
                )
                .unwrap();
        }
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "owner".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "later".into(),
                request_digest: None,
            })
            .unwrap();
    }
    let captured = backend
        .finish_activity_history(
            &identity,
            file_manager::activity::read(plan, &control()),
            &control(),
        )
        .unwrap();
    assert!(
        captured
            .events
            .iter()
            .all(|event| event.path != "audit/later")
    );
    assert!(
        backend
            .activity_history(&identity, &control())
            .unwrap()
            .events
            .iter()
            .any(|event| event.path == "audit/later")
    );
}

#[test]
fn activity_expired_scan_returns_timeout_and_releases_engine_lock() {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([temporary.path().join("disk")]),
    )
    .unwrap();
    let identity = install_test_administrator(&mut engine, "owner");
    let backend = EngineBackend::new(engine);
    let expired = crate::HistoryReadControl::until(std::time::Instant::now());
    assert_eq!(
        backend
            .activity_history(&identity, &expired)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::RequestTimeout
    );
    assert!(backend.engine.try_lock().is_ok());
}

#[test]
fn activity_current_tree_failure_precedes_an_earlier_parent_lookup_failure() {
    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let disk = temporary.path().join("disk");
    let mut engine = Engine::initialize(&system, Config::development([disk.clone()])).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("audit").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    let parent_id = workspace.base_head.clone();
    for path in ["a", "a/child"] {
        engine
            .namespace_operation(
                &workspace.workspace_id,
                NamespaceOperation::parse("mkdir".parse().unwrap(), path, None).unwrap(),
            )
            .unwrap();
    }
    let published = engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "two levels".into(),
            request_digest: None,
        })
        .unwrap();
    let commit_id = published.commit_id.unwrap();
    let current = engine.state().commits[&commit_id].namespace.root.clone();
    let parent = engine.state().commits[&parent_id].namespace.root.clone();
    let object_path = |reference: &storage_contract::NamespaceReference| {
        disk.join("metadata/objects")
            .join(reference.kind.as_str())
            .join(&reference.hash[..2])
            .join(format!("{}.json", reference.hash))
    };
    let page: storage_contract::NamespacePage =
        json_codec::from_slice(&std::fs::read(object_path(&current)).unwrap()).unwrap();
    let subtree = match &page.entries[0] {
        storage_contract::NamespacePageEntry::Directory { subtree, .. } => subtree.clone(),
        storage_contract::NamespacePageEntry::File { .. } => panic!("fixture root is a directory"),
    };
    assert_ne!(parent.hash, subtree.hash);
    drop(engine);
    // Reopen before corruption so the published metadata is admitted and the query cache is cold.
    let engine = Engine::open(&system).unwrap();
    let plan = engine.prepare_activity_read();
    std::fs::remove_file(object_path(&parent)).unwrap();
    std::fs::write(object_path(&subtree), b"invalid-json").unwrap();
    let observe = || Ok(());
    let parent_error = plan
        .commit_namespace(&parent_id, &observe)
        .unwrap()
        .get_key("a")
        .unwrap_err();
    let current_error = plan
        .commit_namespace(&commit_id, &observe)
        .unwrap()
        .entries()
        .unwrap_err();
    assert_ne!(parent_error.code(), current_error.code());
    let expected = error(current_error);
    let result = file_manager::activity::read(plan, &control()).unwrap_err();
    assert_eq!(result.code, expected.code);
    assert_eq!(result.message, expected.message);
}
