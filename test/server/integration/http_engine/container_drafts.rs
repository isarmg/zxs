use super::*;
use std::os::unix::fs::PermissionsExt;

fn manual_application(root: &Path) -> Router {
    let system = root.join("system");
    let disk = root.join("d1");
    for path in [&system, &disk] {
        std::fs::create_dir(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = Config::development([disk]).unwrap();
    let engine = Engine::initialize_manual_container_storage(
        &system, config, storage_contract::OperationId::generate().unwrap(),
    ).unwrap();
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend.protect_auth(
        AuthStore::initialize(&system, "admin", "integration-test-password").unwrap(),
    ).unwrap();
    storage_server::router(backend, auth, ServerConfig::new(&system)).unwrap()
}

#[tokio::test]
async fn manual_policy_is_context_bound_and_survives_cold_open_without_new_history() {
    let root = tempfile::tempdir().unwrap();
    let app = manual_application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "file.txt", b"original").await;
    let listing = success(&app, &token, "GET",
        "/api/file-manager/disks/P01/entries?path=", None).await;
    let file = listing["entries"][0]["file_id"].as_str().unwrap();
    let repository = listing["context"]["repository_id"].as_str().unwrap();
    let route = format!("/api/file-manager/files/{file}/edit-mode");
    let query = format!("?repository={repository}&path=file.txt");
    assert_eq!(success(&app, &token, "GET", &format!("{route}{query}"), None).await,
        json!({"state":"automatic", "generation":"0"}));
    let request = json!({
        "repository":repository, "path":"file.txt", "expected_generation":"0",
        "mode":"manual", "operation_id":new_id("op").unwrap(),
    });
    let mut wrong = request.clone();
    wrong["path"] = json!("absent.txt");
    assert_ne!(send(&app, Some(&token), "POST", &route, Some(wrong)).await.0,
        StatusCode::OK);
    success(&app, &token, "POST", &route, Some(request.clone())).await;
    assert_eq!(success(&app, &token, "GET", &format!("{route}{query}"), None).await,
        json!({"state":"manual_clean", "generation":"1"}));
    let mut stale = request;
    stale["operation_id"] = json!(new_id("op").unwrap());
    assert_ne!(send(&app, Some(&token), "POST", &route, Some(stale)).await.0,
        StatusCode::OK);
    let response = app.clone().oneshot(Request::builder()
        .uri(format!("/api/file-manager/files/{file}/current-content{query}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(http_routing::body::to_bytes(response.into_body(), 1024).await.unwrap(),
        b"original"[..]);
    let file: storage_contract::FileId = file.parse().unwrap();
    drop(app);
    let engine = reopen_after_router_drop(&root.path().join("system")).await;
    assert_eq!(engine.container_edit(file).unwrap(),
        storage_contract::container_edit::ContainerEdit::ManualClean {
            generation: storage_contract::Generation::new(1),
        });
    assert_eq!(engine.state().versions.values()
        .filter(|version| version.file_id == file.to_string()).count(), 1);
    assert!(engine.open_container_draft(file).unwrap().is_none());
}

#[tokio::test]
async fn cold_current_http_reads_shared_draft_without_changing_historical_bytes_or_head() {
    let root = tempfile::tempdir().unwrap();
    let app = manual_application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "file.txt", b"original").await;
    let listing = success(&app, &token, "GET",
        "/api/file-manager/disks/P01/entries?path=", None).await;
    let file: storage_contract::FileId = listing["entries"][0]["file_id"]
        .as_str().unwrap().parse().unwrap();
    let repository: storage_contract::RepositoryId = listing["context"]["repository_id"]
        .as_str().unwrap().parse().unwrap();
    drop(app);
    let system = root.path().join("system");
    let mut engine = reopen_after_router_drop(&system).await;
    let branch = engine.state().repositories[&repository.to_string()].main_branch_id.clone();
    let entry = engine.list_tree(&branch).unwrap().remove(0);
    let principal = storage_contract::Principal::new("admin".into()).unwrap();
    engine.set_container_commit_mode(&storage_contract::container_edit::SetCommitMode {
        file, expected_generation: storage_contract::Generation::new(0),
        mode: storage_contract::container_edit::CommitMode::Manual,
    }, &principal, storage_contract::OperationId::generate().unwrap()).unwrap();
    let versions = engine.state().versions.clone();
    let branches = engine.state().branches.clone();
    let commits = engine.state().commits.clone();
    let base = engine.current_container_draft_base(
        file, repository, entry.entry_id.parse().unwrap(),
    ).unwrap();
    let source = root.path().join("draft-source");
    std::fs::write(&source, b"shared uncommitted bytes").unwrap();
    let draft = storage_contract::container_edit::DraftRevision {
        operation: storage_contract::OperationId::generate().unwrap(),
        expected_generation: engine.container_edit(file).unwrap().generation(),
        principal: principal.clone(), base,
        content_hash: storage_contract::ContentHash::from_digest(
            blake3::hash(b"shared uncommitted bytes")),
        size: b"shared uncommitted bytes".len() as u64,
    };
    engine.save_container_draft(file, &draft, std::fs::File::open(source).unwrap()).unwrap();
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().branches, branches);
    assert_eq!(engine.state().commits, commits);
    drop(engine);
    let engine = Engine::open(&system).unwrap();
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().branches, branches);
    assert_eq!(engine.state().commits, commits);
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend.protect_auth(AuthStore::open(&system).unwrap()).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::new(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let route = format!("/api/file-manager/files/{file}");
    let query = format!("?repository={repository}&path=file.txt");
    let edit = success(&app, &token, "GET", &format!("{route}/edit-mode{query}"), None).await;
    assert_eq!(edit["state"], "manual_dirty");
    assert_eq!(edit["draft"]["operation"], json!(draft.operation));
    let listing = success(&app, &token, "GET",
        "/api/file-manager/disks/P01/entries?path=", None).await;
    assert_eq!(listing["entries"][0]["size"], "24");
    let response = app.clone().oneshot(Request::builder()
        .uri(format!("{route}/current-content{query}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-length"], "24");
    assert_eq!(http_routing::body::to_bytes(response.into_body(), 1024).await.unwrap(),
        b"shared uncommitted bytes"[..]);
    assert_eq!(version_bytes(&app, &token, &entry.version_id).await, b"original");
    let denied = send(&app, Some(&token), "POST", &format!("{route}/edit-mode"),
        Some(json!({"repository":repository,"path":"file.txt",
            "expected_generation":edit["generation"],"mode":"automatic",
            "operation_id":new_id("op").unwrap()}))).await;
    assert_ne!(denied.0, StatusCode::OK);
    assert_eq!(success(&app, &token, "GET", &format!("{route}/edit-mode{query}"), None).await,
        edit);
    let operation = new_id("op").unwrap();
    let mut prepared = success(&app, &token, "POST", &format!("{route}/prepare-draft-commit"),
        Some(json!({"repository":repository,"path":"file.txt",
            "expected_generation":edit["generation"],"draft_operation":draft.operation,
            "operation_id":operation}))).await;
    assert_eq!(prepared["operation_id"], operation);
    let retried_preparation = success(
        &app, &token, "POST", &format!("{route}/prepare-draft-commit"),
        Some(json!({"repository":repository,"path":"file.txt",
            "expected_generation":edit["generation"],"draft_operation":draft.operation,
            "operation_id":operation})),
    ).await;
    assert_eq!(retried_preparation, prepared);
    assert_eq!(success(&app, &token, "GET", &format!("{route}/edit-mode{query}"), None).await,
        edit, "preparation is not a historical publication");
    let workspace = prepared["workspace_id"].as_str().unwrap().to_owned();
    prepared["request_digest"] = request_digest("POST", "/api/workspaces/{workspace}/commit",
        json!({"workspace":workspace}), json!({}), &prepared).unwrap().into();
    drop(app);
    let engine = reopen_after_router_drop(&system).await;
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().branches, branches);
    assert_eq!(engine.state().commits, commits);
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend.protect_auth(AuthStore::open(&system).unwrap()).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::new(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let commit_route = format!("/api/workspaces/{workspace}/commit");
    let published = success(&app, &token, "POST", &commit_route, Some(prepared.clone())).await;
    let retried = success(&app, &token, "POST", &commit_route, Some(prepared)).await;
    assert_eq!(published["operation_id"], operation);
    assert_eq!(retried["operation_id"], operation);
    assert_eq!(success(&app, &token, "GET", &format!("{route}/edit-mode{query}"), None).await
        ["state"], "manual_clean");
    assert_eq!(version_bytes(&app, &token, &entry.version_id).await, b"original");
}
