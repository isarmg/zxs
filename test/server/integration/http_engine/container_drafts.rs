use super::*;

#[tokio::test]
async fn manual_policy_is_context_bound_and_survives_cold_open_without_new_history() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
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
    let app = application(root.path());
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
}
