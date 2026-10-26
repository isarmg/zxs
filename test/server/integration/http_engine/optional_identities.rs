//! Supplied historical identities must never fall back to the current HEAD.
use super::*;

fn invalid_identities() -> Vec<Value> {
    vec![
        json!(null),
        json!(false),
        json!(true),
        json!(123),
        json!({}),
        json!([]),
        json!(""),
    ]
}

fn fixture() -> (tempfile::TempDir, Router, String, String, String) {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("system");
    let mut config = Config::development([root.path().join("disk")]);
    config.max_workspaces = 1;
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repository = engine
        .create_repository(
            "Optional identities",
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let mut commits = Vec::new();
    for bytes in [b"old".as_slice(), b"new content".as_slice()] {
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "data.txt", bytes)
            .unwrap();
        let result = engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: String::new(),
                request_digest: None,
            })
            .unwrap();
        commits.push(result.commit_id.unwrap());
    }
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend
        .protect_auth(AuthStore::initialize(&system, "admin", "integration-test-password").unwrap())
        .unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    (
        root,
        app,
        repository.main_branch_id,
        commits.remove(0),
        commits.remove(0),
    )
}

fn workspace_records(root: &Path) -> std::collections::BTreeSet<std::path::PathBuf> {
    std::fs::read_dir(root.join("system/workspaces"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

#[tokio::test]
async fn supplied_source_identity_rejects_bad_types_without_creating_workspace() {
    let (root, app, branch, old_commit, current_commit) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let before = workspace_records(root.path());
    for invalid in invalid_identities() {
        let (status, response) = send(
            &app,
            Some(&token),
            "POST",
            "/api/workspaces",
            Some(json!({"branch_id":branch,"source_commit_id":invalid})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{invalid}: {response}");
        assert_eq!(response["error"]["code"], "INVALID_REQUEST");
        assert_eq!(workspace_records(root.path()), before);
    }
    // A one-workspace budget also detects any silently created live workspace.
    let historical = success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({"branch_id":branch,"source_commit_id":old_commit})),
    )
    .await;
    assert_eq!(historical["purpose"], "historical_branch");
    assert_eq!(historical["base_head"], old_commit);
    success(
        &app,
        &token,
        "POST",
        &format!(
            "/api/workspaces/{}/abort",
            historical["workspace_id"].as_str().unwrap()
        ),
        Some(json!({})),
    )
    .await;
    let current = success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({"branch_id":branch})),
    )
    .await;
    assert_eq!(current["purpose"], "normal");
    assert_eq!(current["base_head"], current_commit);
}

#[tokio::test]
async fn supplied_resolve_identity_rejects_bad_types_instead_of_returning_head() {
    let (_root, app, branch, old_commit, current_commit) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let path = format!("/api/branches/{branch}/resolve");
    for invalid in invalid_identities() {
        let (status, response) = send(
            &app,
            Some(&token),
            "POST",
            &path,
            Some(json!({"path":"data.txt","commit_id":invalid})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{invalid}: {response}");
        assert_eq!(response["error"]["code"], "INVALID_REQUEST");
    }
    let historical = success(
        &app,
        &token,
        "POST",
        &path,
        Some(json!({"path":"data.txt","commit_id":old_commit})),
    )
    .await;
    let current = success(
        &app,
        &token,
        "POST",
        &path,
        Some(json!({"path":"data.txt"})),
    )
    .await;
    assert_eq!(historical["commit_id"], old_commit);
    assert_eq!(current["commit_id"], current_commit);
    assert_eq!(historical["entry"]["size"], "3");
    assert_eq!(current["entry"]["size"], "11");
    assert_ne!(
        historical["entry"]["version_id"],
        current["entry"]["version_id"]
    );
}
