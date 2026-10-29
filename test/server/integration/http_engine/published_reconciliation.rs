//! Known publication is not a definite HTTP rejection when terminal mirrors fail.
use super::projection_failure::receipt_outcome;
use super::published_reconciliation_failure::{obstruct_terminal_mirror, restore_terminal_mirror};
use super::*;

#[tokio::test]
async fn terminal_workspace_failure_keeps_http_receipt_queryable_and_writes_fenced() {
    use storage_contract::client_protocol::{
        OperationStatus, decode_response, mutation_outcome_unknown, operation_status,
        prepare_commit,
    };
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("system");
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let mut engine = Engine::initialize(&system, config).unwrap();
    let mut auth = AuthStore::initialize(&system, "admin", "integration-test-password").unwrap();
    let token = auth
        .login("admin", "integration-test-password")
        .unwrap()
        .token;
    engine
        .persist_auth_state(
            auth.snapshot().unwrap(),
            "system_auth",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Publication before reconciliation").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let staged = engine
        .stage_bytes(&workspace.workspace_id, "published.txt", b"published bytes")
        .unwrap();
    let seal = engine
        .seal_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    let operation = storage_contract::OperationId::generate();
    let body = prepare_commit(
        json_codec::to_value(seal).unwrap(),
        &workspace.workspace_id,
        &operation.to_string(),
        "terminal mirror will fail",
        None,
    )
    .unwrap();
    let before = engine.state().sequence;
    let obstruction = obstruct_terminal_mirror(&mut engine, &workspace.workspace_id);
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend.protect_auth(auth).unwrap();
    let app =
        storage_server::router(backend.clone(), auth, ServerConfig::loopback(&system)).unwrap();
    let route = format!("/api/workspaces/{}/commit", workspace.workspace_id);

    // Invalid input still has its ordinary definite-rejection classification.
    let (status, _) = send(&app, Some(&token), "POST", &route, Some(json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!mutation_outcome_unknown(status.as_u16()));

    let (status, failure) = send(&app, Some(&token), "POST", &route, Some(body.clone())).await;
    let manifest = obstruction.join().unwrap();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{failure}");
    let decoded =
        decode_response(status.as_u16(), &json_codec::to_vec(&failure).unwrap()).unwrap_err();
    assert_eq!(decoded.code, storage_contract::ErrorCode::RecoveryRequired);
    assert!(!decoded.retryable);
    assert!(mutation_outcome_unknown(decoded.status));
    assert!(decoded.message.contains(&operation.to_string()));
    assert!(decoded.message.contains("published"));
    assert!(!decoded.message.contains(root.path().to_str().unwrap()));
    assert!(decoded.request_id.is_some());
    assert_eq!(
        std::fs::read(&staged.source_path).unwrap(),
        b"published bytes"
    );
    assert!(manifest.is_dir());
    for name in ["commit-request.json", "wire-request.json"] {
        assert!(manifest.with_file_name(name).is_file());
    }

    let queried = success(
        &app,
        &token,
        "GET",
        &format!("/api/operations/{operation}/outcome"),
        None,
    )
    .await;
    assert!(queried.get("current_health").is_none());
    assert_eq!(operation_status(&queried), Some(OperationStatus::Published));
    assert_eq!(queried["operation_id"], operation.to_string());
    assert_eq!(queried["request_digest"], body["request_digest"]);
    assert_eq!(
        queried["result"]["publication_sequence"],
        (before + 1).to_string()
    );
    assert_eq!(queried["result"]["outcome"], "COMMITTED_AND_PROTECTED");
    assert_eq!(queried["result"]["protection_at_publish"], "TARGET_MET");
    let replay = success(&app, &token, "POST", &route, Some(body)).await;
    assert_eq!(receipt_outcome(&replay), receipt_outcome(&queried));
    let (ready, _) = send(&app, Some(&token), "GET", "/readyz", None).await;
    assert_eq!(ready, StatusCode::SERVICE_UNAVAILABLE);
    let (write, _) = send(
        &app,
        Some(&token),
        "POST",
        "/api/workspaces",
        Some(json!({"branch_id":repo.main_branch_id})),
    )
    .await;
    assert_eq!(write, StatusCode::SERVICE_UNAVAILABLE);

    drop(app);
    drop(backend);
    restore_terminal_mirror(&manifest);
    let recovered = reopen_after_router_drop(&system).await;
    assert!(!recovered.is_read_only());
    assert_eq!(recovered.state().sequence, before + 1);
    assert_eq!(
        recovered.workspace(&workspace.workspace_id).unwrap().state,
        storage_contract::WorkspaceState::Committed
    );
    let receipt = recovered.operation(operation).unwrap();
    assert_eq!(
        receipt.commit_state,
        storage_contract::CommitState::Published
    );
    assert_eq!(receipt.sequence, before + 1);
}
