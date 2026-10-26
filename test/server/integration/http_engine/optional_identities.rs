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

#[tokio::test]
async fn typed_workspace_create_abort_preserve_strict_bodies_budget_and_wire() {
    let (root, app, branch, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let before = workspace_records(root.path());
    for invalid in [
        json!([branch]),
        json!(null),
        json!({}),
        json!({"branch_id":""}),
        json!({"branch_id":branch,"extra":true}),
    ] {
        let (status, error) =
            send(&app, Some(&token), "POST", "/api/workspaces", Some(invalid)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(workspace_records(root.path()), before);
    }
    let duplicate = format!(r#"{{"branch_id":"{branch}","branch_id":"{branch}"}}"#);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/workspaces")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(duplicate))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(workspace_records(root.path()), before);
    let created = success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({"branch_id":branch})),
    )
    .await;
    assert_eq!(created["branch_id"], branch);
    assert_eq!(created["principal"], "admin");
    assert_eq!(created["files"], json!({}));
    assert!(created["fencing_token"].is_string());
    assert!(
        !json_codec::to_string(&created)
            .unwrap()
            .contains("source_path")
    );
    let (status, error) = send(
        &app,
        Some(&token),
        "POST",
        "/api/workspaces",
        Some(json!({"branch_id":branch})),
    )
    .await;
    assert!(!status.is_success());
    assert_eq!(error["error"]["code"], "RESOURCE_LIMIT_EXCEEDED");
    let id = created["workspace_id"].as_str().unwrap();
    let route = format!("/api/workspaces/{id}/abort");
    for body in [
        json!([]),
        json!(null),
        json!({"workspace_id":null}),
        json!({"workspace_id":"other"}),
        json!({"extra":true}),
    ] {
        let (status, error) = send(&app, Some(&token), "POST", &route, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        let current = success(&app, &token, "GET", &format!("/api/workspaces/{id}"), None).await;
        assert_eq!(current["state"], "open");
    }
    let result = success(
        &app,
        &token,
        "POST",
        &route,
        Some(json!({"workspace_id":id})),
    )
    .await;
    assert_eq!(result, json!({"workspace_id":id,"state":"aborted"}));
    success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({"branch_id":branch})),
    )
    .await;
}

#[tokio::test]
async fn typed_namespace_boundary_rejects_bad_envelopes_before_staging() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let repo = success(&app,&token,"POST","/api/repositories",Some(json!({"name":"Typed namespace","primary_slot_id":"P01","target_copies":1,"operation_id":new_id("op")}))).await;
    let workspace = success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({"branch_id":repo["main_branch_id"]})),
    )
    .await;
    let ws = workspace["workspace_id"].as_str().unwrap();
    let route = format!("/api/workspaces/{ws}/namespace-operations");
    for body in [
        json!([]),
        json!({"kind":null,"path":"folder"}),
        json!({"kind":"mkdir","path":"folder","unknown":true}),
        json!({"kind":"mkdir","path":"folder","destination":7}),
        json!({"kind":"mkdir","path":"folder","workspace_id":"different"}),
        json!({"kind":"rename","path":"folder"}),
    ] {
        let (status, value) = send(&app, Some(&token), "POST", &route, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{value}");
        assert_eq!(value["error"]["code"], "INVALID_REQUEST");
    }
    let duplicate = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&route)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"kind":"mkdir","kind":"delete","path":"folder"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::BAD_REQUEST);
    let before = success(&app, &token, "GET", &format!("/api/workspaces/{ws}"), None).await;
    assert!(
        before["namespace_operations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let staged = success(
        &app,
        &token,
        "POST",
        &route,
        Some(json!({"kind":"mkdir","path":"Café","destination":null})),
    )
    .await;
    assert_eq!(staged["workspace_id"], ws);
    assert_eq!(
        staged["namespace_operations"][0],
        json!({"kind":"mkdir","path":"Café","destination":null})
    );
    let renamed = success(
        &app,
        &token,
        "POST",
        &route,
        Some(json!({"kind":"rename","path":"CAFE\u{301}","destination":"Renamed"})),
    )
    .await;
    assert_eq!(renamed["namespace_operations"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn typed_write_session_keeps_strict_admission_and_exact_lease_wire() {
    let (root, app, branch, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let route = format!("/api/branches/{branch}/write-sessions");
    let before = workspace_records(root.path());
    for body in [
        json!(null),
        json!([]),
        json!({"unknown":true}),
        json!({"branch_id":null}),
        json!({"branch_id":"other"}),
    ] {
        let (status, response) = send(&app, Some(&token), "POST", &route, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        assert_eq!(workspace_records(root.path()), before);
    }
    let duplicate = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&route)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"branch_id":"{branch}","branch_id":"{branch}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::BAD_REQUEST);
    assert_eq!(workspace_records(root.path()), before);
    let session = success(&app, &token, "POST", &route, Some(json!({}))).await;
    let workspace = success(
        &app,
        &token,
        "GET",
        &format!(
            "/api/workspaces/{}",
            session["workspace_id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    let created = workspace["created_at"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let ttl = Config::development([root.path().join("disk")]).workspace_ttl_seconds;
    assert_eq!(
        session,
        json!({"write_session_id":workspace["workspace_id"],"workspace_id":workspace["workspace_id"],"branch_id":branch,"principal":"admin","fencing_token":workspace["fencing_token"],"expires_at":created.saturating_add(ttl).to_string(),"scope":"workspace","state":"open"})
    );
    let (status, error) = send(&app, Some(&token), "POST", &route, Some(json!({}))).await;
    assert!(
        !status.is_success(),
        "write session must share the workspace budget"
    );
    assert_eq!(error["error"]["code"], "RESOURCE_LIMIT_EXCEEDED");
}

#[tokio::test]
async fn typed_version_metadata_preserves_frozen_history_and_current_wire() {
    let (root, app, branch, old_commit, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let resolved = success(
        &app,
        &token,
        "POST",
        &format!("/api/branches/{branch}/resolve"),
        Some(json!({"path":"data.txt","commit_id":old_commit})),
    )
    .await;
    let version = resolved["entry"]["version_id"].as_str().unwrap();
    let metadata = success(
        &app,
        &token,
        "GET",
        &format!("/api/versions/{version}?unused=accepted"),
        None,
    )
    .await;
    assert_eq!(metadata["logical_size"], "3");
    assert_eq!(metadata["content_state"], "AVAILABLE");
    assert_eq!(metadata["version_id"], version);
    // The current public metadata is the stored version record plus disposition,
    // not the latest branch version or a locally reconstructed request.
    fn find_version(path: &std::path::Path, version: &str) -> Option<Value> {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                if let Some(found) = find_version(&entry.path(), version) {
                    return Some(found);
                }
            } else if entry.file_name() == "version.json" {
                let value: Value =
                    json_codec::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap();
                if value["version_id"] == version {
                    return Some(value);
                }
            }
        }
        None
    }
    let mut expected = find_version(&root.path().join("disk"), version).unwrap();
    expected["content_state"] = json!("AVAILABLE");
    assert_eq!(metadata, expected);
    let (status, error) = send(&app, Some(&token), "GET", "/api/versions/missing", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "NOT_FOUND");
    assert_eq!(error["error"]["message"], "version not found");
}

#[tokio::test]
async fn typed_repository_snapshot_preserves_public_projection_and_missing_errors() {
    let (_root, app, _, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let listed = success(&app, &token, "GET", "/api/repositories", None).await;
    let expected = &listed["repositories"][0];
    let id = expected["repository_id"].as_str().unwrap();
    let actual = success(
        &app,
        &token,
        "GET",
        &format!("/api/repositories/{id}?unused=accepted"),
        None,
    )
    .await;
    assert_eq!(actual, *expected);
    assert!(actual["smb_binding_generation"].is_string());
    assert!(actual.get("file_manager_root").is_none());
    let (status, error) = send(&app, Some(&token), "GET", "/api/repositories/missing", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "NOT_FOUND");
    assert_eq!(error["error"]["message"], "repository not found");
}

#[tokio::test]
async fn typed_capabilities_are_public_and_keep_the_engine_upload_limit() {
    let (root, app, _, _, _) = fixture();
    let (status, capabilities) = send(&app, None, "GET", "/api/capabilities", None).await;
    assert_eq!(status, StatusCode::OK);
    let core_limit = Config::development([root.path().join("disk")]).max_file_bytes;
    let server_limit = ServerConfig::loopback(root.path()).max_upload_bytes;
    assert_eq!(
        capabilities["max_upload_bytes"],
        core_limit.min(server_limit).to_string()
    );
    assert_eq!(capabilities["name_policy"], name_policy::NAME_POLICY);
    assert_eq!(capabilities["platforms"], json!({"web":true,"smb":true}));
    assert!(capabilities.get("storage").is_none());
    let token = login(&app, "admin", "integration-test-password").await;
    let status = success(&app, &token, "GET", "/api/system/storage", None).await;
    assert_eq!(status, json!({"state":"ready","retryable":false}));
}

#[tokio::test]
async fn typed_readiness_is_anonymous_and_preserves_engine_membership_wire() {
    let (_root, app, _, _, _) = fixture();
    let (status, ready) = send(&app, None, "GET", "/readyz", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ready["ready"], true);
    assert_eq!(ready["mode"], "development");
    assert_eq!(ready["storage_disk_count"], 1);
    assert_eq!(ready["metadata_member_count"], 1);
    assert_eq!(ready["metadata_redundancy"], "none");
    assert!(ready["sequence"].as_str().unwrap().parse::<u64>().is_ok());
    assert_eq!(ready.as_object().unwrap().len(), 6);
}

#[tokio::test]
async fn contextual_content_uses_typed_lookup_and_preserves_ranges_and_error_order() {
    let (_root, app, branch, old_commit, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let repositories = success(&app, &token, "GET", "/api/repositories", None).await;
    let repo = repositories["repositories"][0]["repository_id"]
        .as_str()
        .unwrap();
    let resolved = success(
        &app,
        &token,
        "POST",
        &format!("/api/branches/{branch}/resolve"),
        Some(json!({"path":"data.txt","commit_id":old_commit})),
    )
    .await;
    let version = resolved["entry"]["version_id"].as_str().unwrap();
    for query in [
        format!("repository_id={repo}"),
        "path=data.txt".into(),
        "repository_id=&path=data.txt".into(),
        format!("repository_id={repo}&path="),
        format!("repository_id={repo}&path=..%2Fdata.txt"),
    ] {
        let (status, error) = send(
            &app,
            Some(&token),
            "GET",
            &format!("/api/versions/{version}/content?{query}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    }
    for (version, repository, path, code) in [
        ("missing", "missing", "data.txt", "NOT_FOUND"),
        ("missing", repo, "data.txt", "VERSION_NOT_FOUND"),
        (version, repo, "missing.txt", "FILE_NOT_FOUND"),
    ] {
        let (status, error) = send(
            &app,
            Some(&token),
            "GET",
            &format!("/api/versions/{version}/content?repository_id={repository}&path={path}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{error}");
        assert_eq!(error["error"]["code"], code);
    }
    let other=success(&app,&token,"POST","/api/repositories",Some(json!({"name":"Other","primary_slot_id":"P01","target_copies":1,"operation_id":new_id("op")}))).await;
    let (status, error) = send(
        &app,
        Some(&token),
        "GET",
        &format!(
            "/api/versions/{version}/content?repository_id={}&path=data.txt",
            other["repository_id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "FILE_NOT_FOUND");
    for method in ["GET", "HEAD"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!(
                        "/api/versions/{version}/content?repository_id={repo}&path=DATA.TXT"
                    ))
                    .header("authorization", format!("Bearer {token}"))
                    .header("range", "bytes=0-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()["content-range"], "bytes 0-1/3");
        assert!(
            response.headers()["content-disposition"]
                .to_str()
                .unwrap()
                .contains("filename=\"data.txt\"")
        );
        let body = http_routing::body::to_bytes(response.into_body(), 64)
            .await
            .unwrap();
        assert_eq!(
            body.as_ref(),
            if method == "HEAD" {
                b"".as_slice()
            } else {
                b"ol".as_slice()
            }
        );
    }
}

#[tokio::test]
async fn typed_tree_preserves_current_historical_and_explicit_missing_selection() {
    let (_root, app, branch, old_commit, new_commit) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let current = success(
        &app,
        &token,
        "GET",
        &format!("/api/branches/{branch}/tree?branch_id=forged&unused=yes"),
        None,
    )
    .await;
    let historical = success(
        &app,
        &token,
        "GET",
        &format!("/api/branches/{branch}/tree?commit_id={old_commit}"),
        None,
    )
    .await;
    for (tree, commit, size) in [
        (&current, &new_commit, "11"),
        (&historical, &old_commit, "3"),
    ] {
        assert_eq!(tree.as_object().unwrap().len(), 4);
        assert_eq!(tree["branch_id"], branch);
        assert_eq!(tree["commit_id"], *commit);
        assert_eq!(tree["pending_file_versions"], json!({}));
        assert_eq!(tree["entries"].as_array().unwrap().len(), 1);
        assert_eq!(tree["entries"][0]["path"], "data.txt");
        assert_eq!(tree["entries"][0]["size"], size);
        let resolved = success(
            &app,
            &token,
            "POST",
            &format!("/api/branches/{branch}/resolve"),
            Some(json!({"path":"data.txt","commit_id":commit})),
        )
        .await;
        assert_eq!(tree["entries"][0], resolved["entry"]);
    }
    assert_ne!(
        current["entries"][0]["version_id"],
        historical["entries"][0]["version_id"]
    );
    for query in ["commit_id=", "commit_id=missing"] {
        let (status, error) = send(
            &app,
            Some(&token),
            "GET",
            &format!("/api/branches/{branch}/tree?{query}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(error["error"]["code"], "NOT_FOUND");
    }
}

#[tokio::test]
async fn typed_repository_list_preserves_order_fields_and_ignores_query_identity() {
    let (_root, app, _, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let initial = success(&app, &token, "GET", "/api/repositories", None).await;
    let mut expected = initial["repositories"].as_array().unwrap().clone();
    for name in ["Second", "Third"] {
        let created=success(&app,&token,"POST","/api/repositories",Some(json!({"name":name,"primary_slot_id":"P01","target_copies":1,"operation_id":new_id("op")}))).await;
        expected.push(created);
    }
    expected.sort_by(|a, b| {
        a["repository_id"]
            .as_str()
            .unwrap()
            .cmp(b["repository_id"].as_str().unwrap())
    });
    let actual = success(
        &app,
        &token,
        "GET",
        "/api/repositories?principal=other&repository_id=missing&unused=yes",
        None,
    )
    .await;
    assert_eq!(actual, json!({"repositories":expected}));
    for repository in actual["repositories"].as_array().unwrap() {
        let id = repository["repository_id"].as_str().unwrap();
        assert_eq!(
            *repository,
            success(
                &app,
                &token,
                "GET",
                &format!("/api/repositories/{id}"),
                None
            )
            .await
        );
        assert!(repository["smb_binding_generation"].is_string());
        assert!(repository.get("file_manager_root").is_none());
    }
    let (status, error) = send(&app, None, "GET", "/api/repositories?principal=admin", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error["error"]["code"], "UNAUTHORIZED");
}

#[tokio::test]
async fn typed_branch_snapshots_preserve_forks_list_order_and_missing_errors() {
    let (_root, app, branch, old_commit, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let repositories = success(&app, &token, "GET", "/api/repositories", None).await;
    let repo = repositories["repositories"][0]["repository_id"]
        .as_str()
        .unwrap();
    let main = success(
        &app,
        &token,
        "GET",
        &format!("/api/branches/{branch}?branch_id=missing"),
        None,
    )
    .await;
    let fork=success(&app,&token,"POST",&format!("/api/repositories/{repo}/branches"),Some(json!({"source_commit_id":old_commit,"name":"Historical fork","operation_id":new_id("op")}))).await;
    let fork_id = fork["branch_id"].as_str().unwrap();
    assert_eq!(
        success(
            &app,
            &token,
            "GET",
            &format!("/api/branches/{fork_id}"),
            None
        )
        .await,
        fork
    );
    let mut expected = vec![main, fork.clone()];
    expected.sort_by(|a, b| {
        a["branch_id"]
            .as_str()
            .unwrap()
            .cmp(b["branch_id"].as_str().unwrap())
    });
    assert_eq!(
        success(
            &app,
            &token,
            "GET",
            &format!("/api/repositories/{repo}/branches?repository_id=missing&unused=yes"),
            None
        )
        .await,
        json!({"branches":expected})
    );
    assert_eq!(fork["fork_commit_id"], old_commit);
    assert!(fork["generation"].is_string());
    assert!(fork["pending_file_versions"].is_object());
    assert!(fork.get("file_id").is_none());
    assert!(fork.get("alias_of").is_none());
    for (route, message) in [
        ("/api/branches/missing", "branch not found"),
        ("/api/repositories/missing/branches", "repository not found"),
    ] {
        let (status, error) = send(&app, Some(&token), "GET", route, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(error["error"]["code"], "NOT_FOUND");
        assert_eq!(error["error"]["message"], message);
    }
}
