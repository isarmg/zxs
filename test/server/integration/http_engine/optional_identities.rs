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
            &storage_contract::RepositoryName::parse("Optional identities").unwrap(),
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
    assert_eq!(ready["history_index"], json!({"state":"ready"}));
    assert_eq!(ready.as_object().unwrap().len(), 7);
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

#[tokio::test]
async fn typed_resolve_keeps_complete_version_wire_and_static_error_priority() {
    let (_root, app, branch, old_commit, current_commit) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let route = format!("/api/branches/{branch}/resolve");
    for (commit, size) in [(&old_commit, "3"), (&current_commit, "11")] {
        let resolved = success(
            &app,
            &token,
            "POST",
            &route,
            Some(json!({"path":"DATA.TXT","commit_id":commit})),
        )
        .await;
        assert_eq!(resolved.as_object().unwrap().len(), 11);
        assert_eq!(resolved["entry"]["size"], size);
        assert_eq!(resolved["entry"]["path"], "data.txt");
        assert_eq!(resolved["version_id"], resolved["entry"]["version_id"]);
        assert_eq!(resolved["own_version_id"], resolved["version_id"]);
        assert!(resolved["source_version_id"].is_null());
        assert_eq!(resolved["materialized"], true);
        assert_eq!(resolved["inherited"], false);
        assert_eq!(resolved["content_state"], "AVAILABLE");
        assert_eq!(resolved["visible_content_state"], "AVAILABLE");
        let version = resolved["version_id"].as_str().unwrap();
        let mut metadata = success(
            &app,
            &token,
            "GET",
            &format!("/api/versions/{version}"),
            None,
        )
        .await;
        metadata.as_object_mut().unwrap().remove("content_state");
        assert_eq!(resolved["version"], metadata);
    }
    for (route, body, code, message) in [
        (
            "/api/branches/missing/resolve",
            json!({"path":"../bad","unknown":true}),
            "INVALID_REQUEST",
            Some("unknown field unknown"),
        ),
        (
            "/api/branches/missing/resolve",
            json!({"path":"../bad","commit_id":null}),
            "INVALID_REQUEST",
            None,
        ),
        (
            "/api/branches/missing/resolve",
            json!({"path":"file.txt"}),
            "NOT_FOUND",
            Some("branch not found"),
        ),
    ] {
        let (_, error) = send(&app, Some(&token), "POST", route, Some(body)).await;
        assert_eq!(error["error"]["code"], code);
        if let Some(message) = message {
            assert_eq!(error["error"]["message"], message);
        }
    }
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&route)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"path":"data.txt","path":"other.txt"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn typed_commit_snapshot_matches_graph_records_and_keeps_missing_errors() {
    let (_root, app, _, old_commit, new_commit) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let repositories = success(&app, &token, "GET", "/api/repositories", None).await;
    let history = repositories["repositories"][0]["history_tree_id"]
        .as_str()
        .unwrap();
    let graph = success(
        &app,
        &token,
        "GET",
        &format!("/api/history-trees/{history}/graph"),
        None,
    )
    .await;
    for id in [old_commit, new_commit] {
        let expected = graph["commits"]
            .as_array()
            .unwrap()
            .iter()
            .find(|commit| commit["commit_id"] == id)
            .unwrap();
        let actual = success(
            &app,
            &token,
            "GET",
            &format!("/api/commits/{id}?commit_id=missing&unused=yes"),
            None,
        )
        .await;
        assert_eq!(actual, *expected);
        assert_eq!(
            storage_contract::canonical_json_value(&actual).unwrap(),
            storage_contract::canonical_json_value(expected).unwrap()
        );
        assert!(actual["kind"].is_string());
        assert!(actual["sequence"].is_string());
    }
    let (status, error) = send(&app, Some(&token), "GET", "/api/commits/missing", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "NOT_FOUND");
    assert_eq!(error["error"]["message"], "commit not found");
}

#[tokio::test]
async fn typed_history_graph_keeps_scope_sequence_order_and_raw_branch_shape() {
    let (_root, app, branch, old_commit, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let repositories = success(&app, &token, "GET", "/api/repositories", None).await;
    let repository = &repositories["repositories"][0];
    let repo = repository["repository_id"].as_str().unwrap();
    let history = repository["history_tree_id"].as_str().unwrap();
    let fork = success(
        &app,
        &token,
        "POST",
        &format!("/api/repositories/{repo}/branches"),
        Some(
            json!({"source_commit_id":old_commit,"name":"Graph fork","operation_id":new_id("op")}),
        ),
    )
    .await;
    let other=success(&app,&token,"POST","/api/repositories",Some(json!({"name":"Other graph","primary_slot_id":"P01","target_copies":1,"operation_id":new_id("op")}))).await;
    let graph = success(
        &app,
        &token,
        "GET",
        &format!("/api/history-trees/{history}/graph?history_tree_id=missing&unused=yes"),
        None,
    )
    .await;
    assert_eq!(graph.as_object().unwrap().len(), 5);
    assert_eq!(graph["history"]["history_tree_id"], history);
    assert_eq!(graph["boundary_marks"], json!([]));
    let commits = graph["commits"].as_array().unwrap();
    let sequences = commits
        .iter()
        .map(|commit| {
            storage_contract::parse_u64_decimal(commit["sequence"].as_str().unwrap()).unwrap()
        })
        .collect::<Vec<_>>();
    assert!(sequences.windows(2).all(|pair| pair[0] <= pair[1]));
    for commit in commits {
        assert_eq!(commit["history_tree_id"], history);
        let id = commit["commit_id"].as_str().unwrap();
        assert_eq!(
            *commit,
            success(&app, &token, "GET", &format!("/api/commits/{id}"), None).await
        );
    }
    let branches = graph["branches"].as_array().unwrap();
    assert!(branches.iter().any(|item| item["branch_id"] == branch));
    assert!(
        branches
            .iter()
            .any(|item| item["branch_id"] == fork["branch_id"])
    );
    for item in branches {
        assert_eq!(item["history_tree_id"], history);
        assert!(item.get("materialized").is_none());
        assert!(item.get("read_only").is_none());
    }
    let other_history = other["history_tree_id"].as_str().unwrap();
    assert!(
        commits
            .iter()
            .all(|commit| commit["history_tree_id"] != other_history)
    );
    let (status, error) = send(
        &app,
        Some(&token),
        "GET",
        "/api/history-trees/missing/graph",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["message"], "history not found");
}

#[tokio::test]
async fn typed_smb_binding_http_preserves_wire_replay_and_error_priority() {
    let (_root, app, branch, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let repos = success(&app, &token, "GET", "/api/repositories", None).await;
    let repository = repos["repositories"][0]["repository_id"].as_str().unwrap();
    let path = format!("/api/repositories/{repository}/smb-binding");
    let status = success(
        &app,
        &token,
        "GET",
        &format!("{path}?repository_id=missing&unused=yes"),
        None,
    )
    .await;
    assert_eq!(
        status,
        json!({"repository_id":repository,"branch_id":branch,"enabled":true,"history_generation":"1"})
    );
    let update = json!({"branch_id":branch,"enabled":false,"expected_history_generation":status["history_generation"],"operation_id":new_id("op")});
    let published = success(&app, &token, "PUT", &path, Some(update.clone())).await;
    let replay = success(&app, &token, "PUT", &path, Some(update.clone())).await;
    assert_eq!(published["operation_id"], replay["operation_id"]);
    assert_eq!(published["request_digest"], replay["request_digest"]);
    let disabled = success(&app, &token, "GET", &path, None).await;
    let mut expected = status.clone();
    expected["enabled"] = json!(false);
    expected["history_generation"] = json!("2");
    assert_eq!(disabled, expected);
    let mut conflict = update.clone();
    conflict["enabled"] = json!(true);
    conflict["expected_history_generation"] = json!("999");
    let (status, error) = send(&app, Some(&token), "PUT", &path, Some(conflict)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
    let mut invalid = update.clone();
    invalid["operation_id"] = json!("invalid");
    let (status, error) = send(&app, Some(&token), "PUT", &path, Some(invalid)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"]["code"], "INVALID_ID");
    let (status, error) = send(
        &app,
        Some(&token),
        "PUT",
        "/api/repositories/missing/smb-binding",
        Some(json!({"enabled":"invalid"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"]["message"], "enabled must be a boolean");
    let (status, error) = send(
        &app,
        Some(&token),
        "GET",
        "/api/repositories/missing/smb-binding",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["message"], "repository not found");
    assert_eq!(success(&app, &token, "GET", &path, None).await, disabled);
}

#[tokio::test]
async fn typed_event_page_http_preserves_cursor_and_unknown_query_policy() {
    let (_root, app, _, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let first = success(&app, &token, "GET", "/api/events?unused=yes", None).await;
    assert_eq!(first.as_object().unwrap().len(), 2);
    assert!(!first["events"].as_array().unwrap().is_empty());
    for event in first["events"].as_array().unwrap() {
        assert_eq!(event.as_object().unwrap().len(), 4);
        assert!(event["sequence"].is_string());
        event["operation_id"]
            .as_str()
            .unwrap()
            .parse::<storage_contract::OperationId>()
            .unwrap();
    }
    let cursor = first["next_cursor"].as_str().unwrap();
    assert_eq!(
        success(
            &app,
            &token,
            "GET",
            &format!("/api/events?after={cursor}"),
            None
        )
        .await,
        json!({"events":[],"next_cursor":cursor})
    );
    for after in ["", "-1", "01", "18446744073709551616"] {
        let (status, error) = send(
            &app,
            Some(&token),
            "GET",
            &format!("/api/events?after={after}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    }
    let (status, error) = send(
        &app,
        Some(&token),
        "GET",
        "/api/events?after=18446744073709551615",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"]["code"], "INVALID_CURSOR");
    let (status, error) = send(&app, None, "GET", "/api/events?after=invalid", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error["error"]["code"], "UNAUTHORIZED");
    assert_eq!(
        success(&app, &token, "GET", "/api/events?after=0", None).await,
        first
    );
}

#[tokio::test]
async fn typed_storage_metadata_http_has_one_topology_and_frozen_placement() {
    let (_root, app, _, _, commit) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let topology = success(
        &app,
        &token,
        "GET",
        "/api/backup-topologies/default?unused=yes",
        None,
    )
    .await;
    assert_eq!(topology["topology_id"], "default");
    assert_eq!(topology["generation"], "1");
    assert_eq!(topology.as_object().unwrap().len(), 3);
    let slots = topology["slots"].as_array().unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].as_object().unwrap().len(), 6);
    assert_eq!(slots[0]["slot_id"], "P01");
    assert!(slots[0]["upstream_slot_id"].is_null());
    assert!(slots[0].get("root").is_none());
    assert!(slots[0].get("capacity_bytes").is_none());
    for path in [
        "/api/backup-topologies/other",
        "/api/backup-topologies/DEFAULT",
        "/api/backup-topologies",
    ] {
        for bearer in [Some(token.as_str()), None] {
            let (status, error) = send(&app, bearer, "GET", path, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert_eq!(error["error"]["code"], "NOT_FOUND");
        }
    }
    let (status, _) = send(&app, None, "GET", "/api/backup-topologies/default", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let commit = success(&app, &token, "GET", &format!("/api/commits/{commit}"), None).await;
    let version = commit["entries"][0]["version_id"].as_str().unwrap();
    let placement = success(
        &app,
        &token,
        "GET",
        &format!("/api/versions/{version}/placement?version_id=missing"),
        None,
    )
    .await;
    assert_eq!(
        placement,
        json!({"version_id":version,"primary_slot_id":"P01","target_copies":1,"route_slot_ids":["P01"],"topology_generation":"1","binding_generations":{"P01":"1"}})
    );
    let (status, error) = send(
        &app,
        Some(&token),
        "GET",
        "/api/versions/missing/placement",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["message"], "version not found");
}

#[tokio::test]
async fn typed_protection_http_keeps_replay_wire_and_static_boundary_errors() {
    let (_root, app, _, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let repositories = success(&app, &token, "GET", "/api/repositories", None).await;
    let repository = repositories["repositories"][0]["repository_id"]
        .as_str()
        .unwrap();
    for is_policy in [true, false] {
        let (prefix, kind, id_key) = if is_policy {
            ("/api/policies", "policy", "policy_id")
        } else {
            ("/api/primary-placement-rules", "rule", "rule_id")
        };
        let id = new_id(kind);
        let path = format!("{prefix}/{id}");
        let mut body = json!({"repository_id":repository,"path_prefix":"Folder","expected_generation":"0","operation_id":new_id("op")});
        body[if is_policy {
            "target_copies"
        } else {
            "primary_slot_id"
        }] = if is_policy { json!(1) } else { json!("P01") };
        let published = success(&app, &token, "PUT", &path, Some(body.clone())).await;
        let record = &published["result"][kind];
        assert_eq!(record["generation"], "1");
        assert_eq!(
            success(
                &app,
                &token,
                "GET",
                &format!("{path}?{id_key}=missing"),
                None
            )
            .await,
            *record
        );
        let replay = success(&app, &token, "PUT", &path, Some(body.clone())).await;
        for key in [
            "operation_id",
            "operation_kind",
            "request_digest",
            "publication_sequence",
            "result",
        ] {
            assert_eq!(published[key], replay[key], "{key}");
        }
        let mut changed = body.clone();
        changed["path_prefix"] = json!("Other");
        let (status, error) = send(&app, Some(&token), "PUT", &path, Some(changed)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
        let mut invalid_path = body.clone();
        invalid_path["path_prefix"] = json!("../invalid");
        let (status, error) = send(&app, Some(&token), "PUT", &path, Some(invalid_path)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["code"], "NAME_INVALID_CHARACTER");
        let mut extra = body.clone();
        extra["unexpected"] = json!(true);
        let (status, error) = send(&app, Some(&token), "PUT", &path, Some(extra)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["message"], "unknown field unexpected");
        let mut malformed = body.clone();
        malformed["expected_generation"] = json!(1);
        let (status, _) = send(&app, None, "PUT", &path, Some(malformed.clone())).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, error) = send(&app, Some(&token), "PUT", &path, Some(malformed)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["code"], "INVALID_REQUEST");
        let mut invalid_operation = body.clone();
        invalid_operation["operation_id"] = json!("invalid");
        let (status, error) = send(&app, Some(&token), "PUT", &path, Some(invalid_operation)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["code"], "INVALID_ID");
        let mut update = body.clone();
        update["expected_generation"] = json!("1");
        update["operation_id"] = json!(new_id("op"));
        let applied = success(&app, &token, "PUT", &path, Some(update.clone())).await;
        assert_eq!(applied["result"][kind]["generation"], "2");
        update["operation_id"] = json!(new_id("op"));
        let (status, error) = send(&app, Some(&token), "PUT", &path, Some(update)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["error"]["code"], "CONFIG_CONFLICT");
        assert_eq!(
            success(&app, &token, "GET", &path, None).await,
            applied["result"][kind]
        );
        let (status, error) = send(
            &app,
            Some(&token),
            "GET",
            &format!("{prefix}/missing"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            error["error"]["message"],
            if is_policy {
                "protection policy not found"
            } else {
                "placement rule not found"
            }
        );
    }
}

#[tokio::test]
async fn typed_comparison_http_keeps_entry_wire_scope_and_identity_precedence() {
    let (_root, app, _, left, right) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let before = success(&app, &token, "GET", &format!("/api/commits/{left}"), None).await;
    let after = success(&app, &token, "GET", &format!("/api/commits/{right}"), None).await;
    let comparison = success(
        &app,
        &token,
        "POST",
        "/api/compare",
        Some(json!({"left_commit_id":left,"right_commit_id":right})),
    )
    .await;
    assert_eq!(
        comparison,
        json!({"left":left,"right":right,"changes":[{"path":"data.txt","change":"modified","before":before["entries"][0],"after":after["entries"][0]}]})
    );
    assert_eq!(
        success(
            &app,
            &token,
            "POST",
            "/api/compare",
            Some(json!({"left_commit_id":left,"right_commit_id":left}))
        )
        .await,
        json!({"left":left,"right":left,"changes":[]})
    );
    let other=success(&app,&token,"POST","/api/repositories",Some(json!({"name":"Comparison other","primary_slot_id":"P01","target_copies":1,"operation_id":new_id("op")}))).await;
    let branch = other["main_branch_id"].as_str().unwrap();
    let tree = success(
        &app,
        &token,
        "GET",
        &format!("/api/branches/{branch}/tree"),
        None,
    )
    .await;
    let other_commit = tree["commit_id"].as_str().unwrap();
    let cross = success(
        &app,
        &token,
        "POST",
        "/api/compare",
        Some(json!({"left_commit_id":left,"right_commit_id":other_commit})),
    )
    .await;
    assert_eq!(
        cross,
        json!({"left":left,"right":other_commit,"changes":[{"path":"data.txt","change":"removed","before":before["entries"][0]}]})
    );
    for (body, message) in [
        (json!(null), "expected object"),
        (json!({"unexpected":true}), "unknown field unexpected"),
        (
            json!({"right_commit_id":right}),
            "missing string field left_commit_id",
        ),
        (
            json!({"left_commit_id":left,"right_commit_id":""}),
            "missing string field right_commit_id",
        ),
    ] {
        let (status, error) = send(&app, Some(&token), "POST", "/api/compare", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["message"], message);
    }
    let (status, error) = send(
        &app,
        Some(&token),
        "POST",
        "/api/compare",
        Some(json!({"left_commit_id":left,"right_commit_id":"missing"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["message"], "comparison commit not found");
    let (status, _) = send(
        &app,
        None,
        "POST",
        "/api/compare",
        Some(json!({"unexpected":true})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let duplicate=app.clone().oneshot(Request::builder().method("POST").uri("/api/compare").header("authorization",format!("Bearer {token}")).header("content-type","application/json").body(Body::from(r#"{"left_commit_id":"left","left_commit_id":"other","right_commit_id":"right"}"#)).unwrap()).await.unwrap();
    assert_eq!(duplicate.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn typed_raw_file_history_http_preserves_version_records_and_path_binding() {
    let (_root, app, _, left, right) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let before = success(&app, &token, "GET", &format!("/api/commits/{left}"), None).await;
    let after = success(&app, &token, "GET", &format!("/api/commits/{right}"), None).await;
    let file = before["entries"][0]["file_id"].as_str().unwrap();
    let history = success(
        &app,
        &token,
        "GET",
        &format!("/api/files/{file}/history?file_id=missing&unused=yes"),
        None,
    )
    .await;
    assert_eq!(history.as_object().unwrap().len(), 2);
    assert_eq!(history["file_id"], file);
    let versions = history["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(
        versions[0]["version_id"],
        before["entries"][0]["version_id"]
    );
    assert_eq!(versions[1]["version_id"], after["entries"][0]["version_id"]);
    for version in versions {
        assert!(version.get("content_state").is_none());
        assert!(version["version_sequence"].is_string());
        assert!(version["logical_size"].is_string());
        let id = version["version_id"].as_str().unwrap();
        let mut single = success(&app, &token, "GET", &format!("/api/versions/{id}"), None).await;
        single.as_object_mut().unwrap().remove("content_state");
        assert_eq!(*version, single);
    }
    let (status, error) = send(
        &app,
        Some(&token),
        "GET",
        "/api/files/missing/history",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["message"], "file history not found");
    let (status, _) = send(
        &app,
        None,
        "GET",
        &format!("/api/files/{file}/history"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn typed_administrator_diagnostics_keep_wire_queries_and_offline_reports() {
    let (root, app, _, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let disks = success(&app, &token, "GET", "/api/admin/disks?unused=yes", None).await;
    assert_eq!(disks.as_object().unwrap().len(), 1);
    let disk = &disks["disks"][0];
    assert_eq!(disk.as_object().unwrap().len(), 10);
    assert_eq!(disk["slot_id"], "P01");
    assert_eq!(disk["online"], true);
    assert_eq!(disk["simulation"], true);
    assert!(disk["upstream_slot_id"].is_null());
    for field in ["available_bytes", "used_bytes"] {
        assert!(disk[field].as_str().unwrap().parse::<u64>().is_ok());
    }
    assert!(disk.get("root").is_none());
    let report = success(&app, &token, "GET", "/api/admin/check?unused=yes", None).await;
    assert_eq!(report.as_object().unwrap().len(), 4);
    for field in [
        "checked_versions",
        "healthy_replicas",
        "tombstoned_versions",
    ] {
        assert!(report[field].is_number());
    }
    assert!(report["healthy_replicas"].as_u64().unwrap() > 0);
    assert!(report["issues"].as_array().unwrap().is_empty());
    std::fs::rename(root.path().join("disk"), root.path().join("removed-disk")).unwrap();
    let disks = success(&app, &token, "GET", "/api/admin/disks", None).await;
    assert_eq!(disks["disks"][0]["online"], false);
    let report = success(&app, &token, "GET", "/api/admin/check", None).await;
    assert!(!report["issues"].as_array().unwrap().is_empty());
    for route in ["/api/admin/disks", "/api/admin/check"] {
        let (status, body) = send(&app, None, "GET", route, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "UNAUTHORIZED");
    }
}

#[tokio::test]
async fn typed_private_disk_samples_reach_host_http_without_json_reparse() {
    let (root, app, _, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let status = success(&app, &token, "GET", "/api/system/status", None).await;
    assert_eq!(status["disk_sampling_unavailable"], false);
    assert_eq!(status["disks"].as_array().unwrap().len(), 1);
    let disk = &status["disks"][0];
    assert_eq!(disk["id"], "P01");
    assert_eq!(disk["name"], "P01");
    assert_eq!(disk["online"], true);
    assert_eq!(disk["simulation"], true);
    for field in ["total_bytes", "available_bytes", "used_bytes"] {
        assert!(disk[field].as_str().unwrap().parse::<u64>().is_ok());
        assert!(
            status["memory"][field]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .is_ok()
        );
    }
    let history = success(&app, &token, "GET", "/api/system/history?range=1h", None).await;
    assert_eq!(history.as_object().unwrap().len(), 5);
    assert_eq!(history["range"], "1h");
    assert_eq!(history["interval_seconds"], 10);
    assert!(!history["samples"].as_array().unwrap().is_empty());
    let (status, _) = send(
        &app,
        Some(&token),
        "GET",
        "/api/system/history?range=invalid",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    std::fs::rename(root.path().join("disk"), root.path().join("detached-disk")).unwrap();
    let status = success(&app, &token, "GET", "/api/system/status", None).await;
    assert_eq!(status["disk_sampling_unavailable"], false);
    assert_eq!(status["disks"][0]["online"], false);
    for field in ["total_bytes", "available_bytes", "used_bytes"] {
        assert!(status["disks"][0][field].is_null());
    }
    for route in ["/api/system/status", "/api/system/history"] {
        let (status, body) = send(&app, None, "GET", route, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "UNAUTHORIZED");
    }
}

#[tokio::test]
async fn typed_file_manager_disks_keep_projection_without_creating_roots() {
    let (root, app, _, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let before = success(&app, &token, "GET", "/api/repositories", None).await;
    let disks = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks?unused=yes",
        None,
    )
    .await;
    assert_eq!(disks.as_object().unwrap().len(), 1);
    assert_eq!(disks["disks"].as_array().unwrap().len(), 1);
    let disk = &disks["disks"][0];
    assert_eq!(disk.as_object().unwrap().len(), 7);
    assert_eq!(disk["slot_id"], "P01");
    assert_eq!(disk["level"], 1);
    assert_eq!(disk["online"], true);
    assert_eq!(disk["writable"], true);
    assert_eq!(disk["simulation"], true);
    for field in ["available_bytes", "used_bytes"] {
        assert!(disk[field].as_str().unwrap().parse::<u64>().is_ok());
    }
    for field in [
        "disk_id",
        "upstream_slot_id",
        "metadata_member",
        "physical_device",
        "root",
    ] {
        assert!(disk.get(field).is_none());
    }
    assert_eq!(
        success(&app, &token, "GET", "/api/repositories", None).await,
        before
    );
    std::fs::rename(root.path().join("disk"), root.path().join("offline-disk")).unwrap();
    let disks = success(&app, &token, "GET", "/api/file-manager/disks", None).await;
    assert_eq!(disks["disks"][0]["online"], false);
    assert_eq!(
        disks["disks"][0]["writable"], true,
        "writable is the primary-level policy, not sampled availability"
    );
    let (status, body) = send(&app, None, "GET", "/api/file-manager/disks", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "UNAUTHORIZED");
}

#[tokio::test]
async fn typed_default_directory_http_preserves_empty_and_initialized_views() {
    let (_root, app, namespace, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let branch = success(
        &app,
        &token,
        "GET",
        &format!("/api/branches/{namespace}"),
        None,
    )
    .await;
    let history = branch["history_tree_id"].as_str().unwrap();
    let graph = success(
        &app,
        &token,
        "GET",
        &format!("/api/history-trees/{history}/graph"),
        None,
    )
    .await;
    let repository = graph["history"]["repository_id"].as_str().unwrap();
    let uri =
        format!("/api/repositories/{repository}/default-branch?repository_id=missing&unused=yes");
    let before = success(&app, &token, "GET", &uri, None).await;
    assert_eq!(before.as_object().unwrap().len(), 5);
    assert_eq!(before["repository_id"], repository);
    assert_eq!(before["namespace_branch_id"], namespace);
    assert_eq!(before["name"], "main");
    assert!(before["directory_branch_id"].is_null());
    assert!(before["directory_branches"].as_array().unwrap().is_empty());
    let initialized = success(
        &app,
        &token,
        "POST",
        &format!("/api/repositories/{repository}/directory-scopes"),
        Some(json!({"path":"","operation_id":storage_contract::OperationId::generate()})),
    )
    .await;
    let after = success(&app, &token, "GET", &uri, None).await;
    assert_eq!(
        after["directory_branch_id"],
        initialized["result"]["directory_branch_id"]
    );
    assert_eq!(after["directory_branches"].as_array().unwrap().len(), 1);
    let summary = &after["directory_branches"][0];
    assert_eq!(summary.as_object().unwrap().len(), 8);
    assert_eq!(summary["current"], true);
    assert_eq!(summary["path"], "");
    assert_eq!(summary["file_count"], "1");
    assert!(summary.get("entries").is_none());
    let (status, body) = send(
        &app,
        Some(&token),
        "GET",
        "/api/repositories/missing/default-branch",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "NOT_FOUND");
    assert_eq!(body["error"]["message"], "repository not found");
    assert_eq!(
        send(&app, None, "GET", &uri, None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn typed_directory_scopes_keep_full_summary_and_lookup_wire() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "Café/file.txt", b"data").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let uri = format!("/api/repositories/{repo}/directory-branches");
    let all = success(&app, &token, "GET", &uri, None).await;
    let summary = success(&app, &token, "GET", &format!("{uri}?summary=true"), None).await;
    assert_eq!(all["repository_id"], repo);
    let full_rows = all["directory_branches"].as_array().unwrap();
    assert!(!full_rows.is_empty());
    let summaries = summary["directory_branches"].as_array().unwrap();
    assert_eq!(full_rows.len(), summaries.len());
    for (full, small) in full_rows.iter().zip(summaries) {
        assert!(full["entries"].is_array());
        assert_eq!(small.as_object().unwrap().len(), 12);
        for (key, value) in small.as_object().unwrap() {
            assert_eq!(value, &full[key], "{key}");
        }
        assert!(small.get("entries").is_none());
        assert!(small["sequence"].is_string());
        assert_eq!(
            small["file_count"],
            full["entries"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|entry| entry["kind"] == "file")
                .count()
                .to_string()
        );
    }
    let root_only = success(
        &app,
        &token,
        "GET",
        &format!("{uri}?path=&summary=true"),
        None,
    )
    .await;
    assert!(
        root_only["directory_branches"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["directory_id"] == repo)
    );
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("path", "CAFE\u{301}")
        .append_pair("summary", "true")
        .finish();
    let nfd = success(&app, &token, "GET", &format!("{uri}?{query}"), None).await;
    let rows = nfd["directory_branches"].as_array().unwrap();
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|row| row["path"] == "Café"));
    for query in [
        "summary=false",
        "summary=TRUE",
        "extra=yes",
        "path=..%2Fbad",
    ] {
        assert_eq!(
            send(&app, Some(&token), "GET", &format!("{uri}?{query}"), None)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        send(
            &app,
            Some(&token),
            "GET",
            "/api/repositories/missing/directory-branches",
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, None, "GET", &format!("{uri}?summary=false"), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn typed_directory_selection_preserves_bounded_facts_and_current_tree_conflicts() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "Café/file.txt", b"data").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let uri = format!("/api/repositories/{repo}/directory-branches");
    let full = success(&app, &token, "GET", &uri, None).await;
    let branch = full["directory_branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|branch| branch["path"] == "Café" && branch["current"] == true)
        .unwrap();
    let entry = branch["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "file")
        .unwrap();
    let selected_path = "CAFE\u{301}/FILE.txt";
    let body = json!({"path":"CAFE\u{301}","sources":[{"directory_branch_id":branch["directory_branch_id"],"expected_tree_id":branch["head_tree_id"]}],"entries":[{"entry_id":entry["entry_id"],"path":selected_path}]});
    let response = success(&app, &token, "POST", &uri, Some(body.clone())).await;
    assert_eq!(response["repository_id"], repo);
    let rows = response["directory_branches"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].as_object().unwrap().len(), 13);
    for (key, value) in rows[0].as_object().unwrap() {
        if key != "entries" {
            assert_eq!(value, &branch[key], "{key}");
        }
    }
    assert_eq!(
        rows[0]["entries"],
        json!([{"entry_id":entry["entry_id"],"path":entry["path"],"selected_path":selected_path}])
    );
    let mut stale = body.clone();
    stale["sources"][0]["expected_tree_id"] = json!("missing-tree");
    let changed = send(&app, Some(&token), "POST", &uri, Some(stale)).await;
    assert_eq!(changed.0, StatusCode::CONFLICT);
    assert_eq!(changed.1["error"]["code"], "LISTING_CHANGED");
    for malformed in [json!(null), json!(12), json!([])] {
        let mut invalid = body.clone();
        invalid["path"] = malformed;
        assert_eq!(
            send(&app, Some(&token), "POST", &uri, Some(invalid))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut duplicate = body.clone();
    duplicate["entries"] = json!([body["entries"][0], body["entries"][0]]);
    assert_eq!(
        send(&app, Some(&token), "POST", &uri, Some(duplicate))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let mut oversized = body.clone();
    oversized["entries"] = json!(vec![body["entries"][0].clone(); 501]);
    let limited = send(&app, Some(&token), "POST", &uri, Some(oversized)).await;
    assert_eq!(limited.0, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.1["error"]["code"], "RESOURCE_LIMIT_EXCEEDED");
    let empty = success(
        &app,
        &token,
        "POST",
        &uri,
        Some(json!({"path":"","sources":[],"entries":[]})),
    )
    .await;
    assert_eq!(empty["directory_branches"], json!([]));
    assert_eq!(
        send(
            &app,
            Some(&token),
            "POST",
            "/api/repositories/missing/directory-branches",
            Some(body)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, None, "POST", &uri, Some(json!({}))).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn typed_directory_rename_preserves_public_receipt_and_name_error_priority() {
    let (_root, app, namespace, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    let branch = success(
        &app,
        &token,
        "GET",
        &format!("/api/branches/{namespace}"),
        None,
    )
    .await;
    let history = branch["history_tree_id"].as_str().unwrap();
    let graph = success(
        &app,
        &token,
        "GET",
        &format!("/api/history-trees/{history}/graph"),
        None,
    )
    .await;
    let repo = graph["history"]["repository_id"].as_str().unwrap();
    let initialized = success(
        &app,
        &token,
        "POST",
        &format!("/api/repositories/{repo}/directory-scopes"),
        Some(json!({"path":"","operation_id":storage_contract::OperationId::generate()})),
    )
    .await;
    let directory = initialized["result"]["directory_branch_id"]
        .as_str()
        .unwrap();
    let uri = format!("/api/repositories/{repo}/directory-branches/{directory}");
    let operation = storage_contract::OperationId::generate();
    let body = json!({"name":"Café","operation_id":operation});
    let result = success(&app, &token, "PATCH", &uri, Some(body.clone())).await;
    assert_eq!(result["operation_kind"], "directory.branch.rename");
    assert_eq!(result["result"]["name"], "Café");
    let replay = success(&app, &token, "PATCH", &uri, Some(body.clone())).await;
    for key in [
        "operation_id",
        "operation_kind",
        "request_digest",
        "result",
        "publication_sequence",
        "commit_state",
    ] {
        assert_eq!(replay[key], result[key], "{key}");
    }
    let changed = send(
        &app,
        Some(&token),
        "PATCH",
        &uri,
        Some(json!({"name":"Other","operation_id":operation})),
    )
    .await;
    assert_eq!(changed.0, StatusCode::CONFLICT);
    assert_eq!(changed.1["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
    let invalid = send(
        &app,
        Some(&token),
        "PATCH",
        &uri,
        Some(json!({"name":"bad/name","operation_id":"bad-id"})),
    )
    .await;
    assert_eq!(invalid.1["error"]["code"], "INVALID_ID");
    for (name, code) in [
        ("bad/name".into(), "NAME_INVALID_CHARACTER"),
        ("Cafe\u{301}".into(), "NAME_NOT_NORMALIZED"),
        ("CON.txt".into(), "NAME_RESERVED"),
        ("x".repeat(129), "METADATA_TOO_LONG"),
    ] {
        let invalid = send(
            &app,
            Some(&token),
            "PATCH",
            &uri,
            Some(json!({"name":name,"operation_id":operation})),
        )
        .await;
        assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
        assert_eq!(invalid.1["error"]["code"], code);
    }
    for key in ["repository_id", "directory_branch_id"] {
        let mut mismatch = body.clone();
        mismatch[key] = json!("other");
        assert_eq!(
            send(&app, Some(&token), "PATCH", &uri, Some(mismatch))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        send(
            &app,
            Some(&token),
            "PATCH",
            &format!("/api/repositories/{repo}/directory-branches/missing"),
            Some(json!({"name":"Valid","operation_id":storage_contract::OperationId::generate()}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &app,
            None,
            "PATCH",
            &uri,
            Some(json!({"name":"bad/name","operation_id":"bad-id"}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn typed_archive_http_keeps_raw_receipt_and_rejects_active_workspace() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let repo=success(&app,&token,"POST","/api/repositories",Some(json!({"name":"Archive","primary_slot_id":"P01","target_copies":3,"operation_id":storage_contract::OperationId::generate()}))).await;
    let repository = repo["repository_id"].as_str().unwrap();
    let history = repo["history_tree_id"].as_str().unwrap();
    let graph = success(
        &app,
        &token,
        "GET",
        &format!("/api/history-trees/{history}/graph"),
        None,
    )
    .await;
    let side=success(&app,&token,"POST",&format!("/api/repositories/{repository}/branches"),Some(json!({"source_commit_id":graph["history"]["root_commit_id"],"name":"side","operation_id":storage_contract::OperationId::generate()}))).await;
    let branch = side["branch_id"].as_str().unwrap();
    let uri = format!("/api/branches/{branch}/archive");
    let workspace = success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({"branch_id":branch})),
    )
    .await;
    let busy = send(
        &app,
        Some(&token),
        "POST",
        &uri,
        Some(json!({"operation_id":storage_contract::OperationId::generate()})),
    )
    .await;
    assert_eq!(busy.1["error"]["code"], "WORKSPACE_BUSY");
    success(
        &app,
        &token,
        "POST",
        &format!(
            "/api/workspaces/{}/abort",
            workspace["workspace_id"].as_str().unwrap()
        ),
        Some(json!({})),
    )
    .await;
    let operation = storage_contract::OperationId::generate();
    let body = json!({"operation_id":operation});
    let receipt = success(&app, &token, "POST", &uri, Some(body.clone())).await;
    assert_eq!(receipt.as_object().unwrap().len(), 11);
    assert_eq!(receipt["kind"], "branch.archive");
    assert_eq!(receipt["branch_id"], branch);
    assert!(receipt["current_health"].is_string());
    assert!(receipt["sequence"].is_string());
    assert!(receipt["commit_id"].is_null());
    assert!(receipt.get("operation_kind").is_none());
    assert!(receipt.get("publication_sequence").is_none());
    assert_eq!(
        success(&app, &token, "POST", &uri, Some(body.clone())).await,
        receipt
    );
    let archived = success(
        &app,
        &token,
        "GET",
        &format!("/api/branches/{branch}"),
        None,
    )
    .await;
    assert_eq!(archived["state"], "archived");
    let main = format!(
        "/api/branches/{}/archive",
        repo["main_branch_id"].as_str().unwrap()
    );
    let conflict = send(&app, Some(&token), "POST", &main, Some(body.clone())).await;
    assert_eq!(conflict.1["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
    let protected = send(
        &app,
        Some(&token),
        "POST",
        &main,
        Some(json!({"operation_id":storage_contract::OperationId::generate()})),
    )
    .await;
    assert_eq!(protected.1["error"]["code"], "MAIN_BRANCH_REQUIRED");
    assert_eq!(
        send(
            &app,
            Some(&token),
            "POST",
            "/api/branches/missing/archive",
            Some(body)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let invalid = send(
        &app,
        Some(&token),
        "POST",
        &uri,
        Some(json!({"operation_id":"bad-id"})),
    )
    .await;
    assert_eq!(invalid.1["error"]["code"], "INVALID_ID");
    let mismatch = send(
        &app,
        Some(&token),
        "POST",
        &uri,
        Some(json!({"branch_id":"other","operation_id":operation})),
    )
    .await;
    assert_eq!(mismatch.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        send(&app, None, "POST", &uri, Some(json!({}))).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn typed_branch_creation_keeps_receipt_snapshot_and_boundary_errors() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let repo=success(&app,&token,"POST","/api/repositories",Some(json!({"name":"Creation","primary_slot_id":"P01","target_copies":3,"operation_id":storage_contract::OperationId::generate()}))).await;
    let repository = repo["repository_id"].as_str().unwrap();
    let graph = success(
        &app,
        &token,
        "GET",
        &format!(
            "/api/history-trees/{}/graph",
            repo["history_tree_id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    let uri = format!("/api/repositories/{repository}/branches");
    let body = json!({"source_commit_id":graph["history"]["root_commit_id"],"name":"Café","operation_id":storage_contract::OperationId::generate()});
    let created = success(&app, &token, "POST", &uri, Some(body.clone())).await;
    assert_eq!(created["name"], "Café");
    assert_eq!(created["generation"], "1");
    assert_eq!(created["state"], "active");
    assert!(created.get("created_branch").is_none());
    assert_eq!(created["materialized"], true);
    assert_eq!(created["inherited"], false);
    assert_eq!(created["content_state"], "LIVE");
    assert!(created.get("file_id").is_none());
    success(
        &app,
        &token,
        "POST",
        &format!(
            "/api/branches/{}/archive",
            created["branch_id"].as_str().unwrap()
        ),
        Some(json!({"operation_id":storage_contract::OperationId::generate()})),
    )
    .await;
    assert_eq!(
        success(&app, &token, "POST", &uri, Some(body.clone())).await,
        created
    );
    let mut changed = body.clone();
    changed["name"] = "Different".into();
    assert_eq!(
        send(&app, Some(&token), "POST", &uri, Some(changed))
            .await
            .1["error"]["code"],
        "IDEMPOTENCY_KEY_REUSED"
    );
    let mut invalid = body.clone();
    invalid["operation_id"] = "bad".into();
    invalid["name"] = "../bad".into();
    let error = send(&app, Some(&token), "POST", &uri, Some(invalid)).await;
    assert_eq!(error.0, StatusCode::BAD_REQUEST);
    assert_eq!(error.1["error"]["code"], "INVALID_ID");
    let mut outside = body.clone();
    outside["source_commit_id"] = "missing".into();
    assert_eq!(
        send(&app, Some(&token), "POST", &uri, Some(outside))
            .await
            .1["error"]["code"],
        "HISTORY_NODE_NOT_FOUND"
    );
    let mut forged = body.clone();
    forged["repository_id"] = "other".into();
    assert_eq!(
        send(&app, Some(&token), "POST", &uri, Some(forged)).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(&app, None, "POST", &uri, Some(body)).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn typed_repository_creation_preserves_long_names_original_binding_and_route_priority() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let uri = "/api/repositories";
    let body = json!({"name":"R".repeat(200),"primary_slot_id":"P01","target_copies":3,"operation_id":storage_contract::OperationId::generate()});
    let created = success(&app, &token, "POST", uri, Some(body.clone())).await;
    assert_eq!(created["name"], body["name"]);
    assert_eq!(created["target_copies"], 3);
    assert_eq!(created["smb_binding_generation"], "1");
    assert!(created.get("file_manager_root").is_none());
    assert!(created.get("created_repository").is_none());
    let binding = format!(
        "/api/repositories/{}/smb-binding",
        created["repository_id"].as_str().unwrap()
    );
    success(&app,&token,"PUT",&binding,Some(json!({"branch_id":created["main_branch_id"],"enabled":false,"expected_history_generation":"1","operation_id":storage_contract::OperationId::generate()}))).await;
    assert_eq!(
        success(&app, &token, "POST", uri, Some(body.clone())).await,
        created
    );
    for (copies, code) in [
        (0, "PLACEMENT_INVALID"),
        (4, "BACKUP_ROUTE_INCOMPLETE"),
        (2, "IDEMPOTENCY_KEY_REUSED"),
    ] {
        let mut changed = body.clone();
        changed["target_copies"] = json!(copies);
        assert_eq!(
            send(&app, Some(&token), "POST", uri, Some(changed)).await.1["error"]["code"],
            code
        );
    }
    let mut invalid = body.clone();
    invalid["operation_id"] = "bad".into();
    invalid["name"] = "../bad".into();
    assert_eq!(
        send(&app, Some(&token), "POST", uri, Some(invalid)).await.1["error"]["code"],
        "INVALID_ID"
    );
    for copies in [json!(u64::MAX), json!("3"), json!(-1)] {
        let mut invalid = body.clone();
        invalid["target_copies"] = copies;
        assert_eq!(
            send(&app, Some(&token), "POST", uri, Some(invalid)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut forged = body.clone();
    forged["file_manager_root"] = json!(true);
    assert_eq!(
        send(&app, Some(&token), "POST", uri, Some(forged)).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(&app, None, "POST", uri, Some(body)).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn typed_merge_link_http_reuses_graph_wire_and_permanent_receipt() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let repo=success(&app,&token,"POST","/api/repositories",Some(json!({"name":"Links","primary_slot_id":"P01","target_copies":3,"operation_id":storage_contract::OperationId::generate()}))).await;
    let history = repo["history_tree_id"].as_str().unwrap();
    let graph_uri = format!("/api/history-trees/{history}/graph");
    let graph = success(&app, &token, "GET", &graph_uri, None).await;
    let root_commit = graph["history"]["root_commit_id"].clone();
    let side=success(&app,&token,"POST",&format!("/api/repositories/{}/branches",repo["repository_id"].as_str().unwrap()),Some(json!({"name":"side","source_commit_id":root_commit,"operation_id":storage_contract::OperationId::generate()}))).await;
    let uri = format!("/api/history-trees/{history}/merge-links");
    let body = json!({"source_branch_id":side["branch_id"],"source_commit_id":side["head_commit_id"],"target_branch_id":repo["main_branch_id"],"target_commit_id":root_commit,"operation_id":storage_contract::OperationId::generate()});
    let created = success(&app, &token, "POST", &uri, Some(body.clone())).await;
    assert_eq!(created.as_object().unwrap().len(), 8);
    assert_eq!(created["history_tree_id"], history);
    assert_eq!(created["operation_id"], body["operation_id"]);
    for field in [
        "source_branch_id",
        "source_commit_id",
        "target_branch_id",
        "target_commit_id",
    ] {
        assert_eq!(created[field], body[field]);
    }
    let graph = success(&app, &token, "GET", &graph_uri, None).await;
    assert_eq!(graph["merge_links"][0], created);
    assert_eq!(
        success(&app, &token, "POST", &uri, Some(body.clone())).await,
        created
    );
    let mut conflict = body.clone();
    conflict["source_branch_id"] = "missing".into();
    assert_eq!(
        send(&app, Some(&token), "POST", &uri, Some(conflict.clone()))
            .await
            .1["error"]["code"],
        "IDEMPOTENCY_KEY_REUSED"
    );
    conflict["operation_id"] = json!(storage_contract::OperationId::generate());
    assert_eq!(
        send(&app, Some(&token), "POST", &uri, Some(conflict))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let mut invalid = body.clone();
    invalid["operation_id"] = "bad".into();
    assert_eq!(
        send(&app, Some(&token), "POST", &uri, Some(invalid))
            .await
            .1["error"]["code"],
        "INVALID_ID"
    );
    let mut forged = body.clone();
    forged["history_tree_id"] = "other".into();
    assert_eq!(
        send(&app, Some(&token), "POST", &uri, Some(forged)).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(&app, None, "POST", &uri, Some(body)).await.0,
        StatusCode::UNAUTHORIZED
    );
}
