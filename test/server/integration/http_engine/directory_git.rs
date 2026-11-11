use super::*;

async fn listing(app: &Router, token: &str, path: &str) -> Value {
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("path", path)
        .finish();
    success(
        app,
        token,
        "GET",
        &format!("/api/file-manager/disks/P01/entries?{query}"),
        None,
    )
    .await
}

async fn status(app: &Router, token: &str, repo: &str, path: &str) -> Value {
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("path", path)
        .finish();
    let value = success(
        app,
        token,
        "GET",
        &format!("/api/repositories/{repo}/directory-git?{query}"),
        None,
    )
    .await;
    let _: storage_contract::directory_git::DirectoryGitStatus =
        json_codec::from_value(value.clone()).unwrap();
    value
}

async fn initialize(app: &Router, token: &str, repo: &str, path: &str) -> Value {
    let current = listing(app, token, path).await;
    success(
        app,
        token,
        "POST",
        &format!("/api/repositories/{repo}/directory-git/initialize"),
        Some(json!({
            "path":path,
            "expected_namespace_head":current["context"]["base_head"],
            "operation_id":new_id("op").unwrap()
        })),
    )
    .await
}

async fn stage_owned(app: &Router, token: &str, repo: &str, current: &Value) -> Value {
    success(
        app,
        token,
        "POST",
        &format!("/api/repositories/{repo}/directory-git/stage"),
        Some(json!({
            "directory_id":current["directory_id"],
            "directory_branch_id":current["directory_branch_id"],
            "expected_tree_id":current["working_tree_id"],
            "expected_index_generation":current["index_generation"],
            "operation_id":new_id("op").unwrap()
        })),
    )
    .await
}

async fn commit(app: &Router, token: &str, repo: &str, current: &Value) -> Value {
    success(
        app,
        token,
        "POST",
        &format!(
            "/api/repositories/{repo}/directory-branches/{}/commits",
            current["directory_branch_id"].as_str().unwrap()
        ),
        Some(json!({
            "expected_tree_id":current["working_tree_id"],
            "expected_index_generation":current["index_generation"],
            "message":"Explicit directory checkpoint",
            "operation_id":new_id("op").unwrap()
        })),
    )
    .await
}

#[tokio::test]
async fn directory_git_reads_do_not_initialize_and_require_current_authentication() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/a.txt", b"one").await;
    let before = listing(&app, &token, "docs").await;
    let repo = before["context"]["repository_id"].as_str().unwrap();
    assert_eq!(before["directory_branches_supported"], false);
    let first = status(&app, &token, repo, "docs").await;
    assert_eq!(first["initialized"], false);
    assert_eq!(first["head_tree_id"], Value::Null);
    assert_eq!(first["owner"], Value::Null);
    assert_eq!(status(&app, &token, repo, "docs").await, first);
    assert_eq!(listing(&app, &token, "docs").await, before);

    let base = format!("/api/repositories/{repo}/directory-git");
    for (method, suffix) in [
        ("GET", "?path=docs"),
        ("POST", "/initialize"),
        ("POST", "/children/stage"),
        ("POST", "/stage"),
    ] {
        let route = format!("{base}{suffix}");
        assert_eq!(
            send(&app, None, method, &route, None).await.0,
            StatusCode::UNAUTHORIZED
        );
        let revoked = revoked_session(&app).await;
        assert_eq!(
            send(&app, Some(&revoked), method, &route, None).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    for query in [
        "",
        "?path=docs&unknown=1",
        "?path=docs&path=other",
        "?path=../docs",
        "?path=%2Fdocs",
    ] {
        let (code, body) = send(&app, Some(&token), "GET", &format!("{base}{query}"), None).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{query}: {body}");
    }
    let bytes = manager_disk_files(root.path());
    let events = success(&app, &token, "GET", "/api/events", None).await;
    let rejected_operation = new_id("op").unwrap();
    let retired = format!("/api/repositories/{repo}/directory-scopes");
    assert_eq!(
        send(
            &app,
            Some(&token),
            "POST",
            &retired,
            Some(json!({"path":"docs","operation_id":new_id("op").unwrap()}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &app,
            Some(&token),
            "POST",
            "/api/file-manager/disks/P01/directory-select",
            Some(json!({"path":"docs","name":"main","operation_id":rejected_operation}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND,
        "name-only selection cannot bypass exact branch identity and head CAS"
    );
    assert_eq!(
        send(
            &app,
            Some(&token),
            "GET",
            &format!("/api/operations/{rejected_operation}/outcome"),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        success(&app, &token, "GET", "/api/events", None).await,
        events
    );
    assert_eq!(manager_disk_files(root.path()), bytes);
    assert_eq!(status(&app, &token, repo, "docs").await, first);
}

#[tokio::test]
async fn directory_git_initialization_is_fenced_idempotent_and_exact_directory_only() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/nested/a.txt", b"one").await;
    let before = listing(&app, &token, "docs").await;
    let repo = before["context"]["repository_id"].as_str().unwrap();
    let route = format!("/api/repositories/{repo}/directory-git/initialize");
    let mut body = json!({"path":"docs","expected_namespace_head":before["context"]["base_head"],"operation_id":new_id("op").unwrap()});
    body["unexpected"] = true.into();
    assert_eq!(
        send(&app, Some(&token), "POST", &route, Some(body.clone()))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    body.as_object_mut().unwrap().remove("unexpected");
    let mut invalid_operation = body.clone();
    invalid_operation["operation_id"] = "not-an-operation".into();
    assert_eq!(
        send(&app, Some(&token), "POST", &route, Some(invalid_operation))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let stale = json!({"path":"docs","expected_namespace_head":new_id("commit").unwrap(),"operation_id":new_id("op").unwrap()});
    assert_eq!(
        send(&app, Some(&token), "POST", &route, Some(stale))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        status(&app, &token, repo, "docs").await["initialized"],
        false
    );
    let result = success(&app, &token, "POST", &route, Some(body.clone())).await;
    assert_eq!(
        success(&app, &token, "POST", &route, Some(body)).await,
        result
    );
    assert_eq!(
        listing(&app, &token, "docs").await["directory_branches_supported"],
        true
    );
    assert_eq!(
        listing(&app, &token, "docs/nested").await["directory_branches_supported"],
        false
    );
    let child = status(&app, &token, repo, "docs/nested").await;
    assert_eq!(child["initialized"], false);
    assert_eq!(child["owner"]["path"], "docs");
    let current = status(&app, &token, repo, "docs").await;
    assert_eq!(current["head_tree_id"], Value::Null);
    assert!(current["index_generation"].is_string());
    assert_eq!(current["children"], json!([]));
}

#[tokio::test]
async fn directory_git_staging_and_commit_require_exact_string_generation() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/a.txt", b"one").await;
    let repo = listing(&app, &token, "docs").await["context"]["repository_id"]
        .as_str()
        .unwrap()
        .to_owned();
    initialize(&app, &token, &repo, "docs").await;
    let current = status(&app, &token, &repo, "docs").await;
    let stage_route = format!("/api/repositories/{repo}/directory-git/stage");
    let stage = json!({"directory_id":current["directory_id"],"directory_branch_id":current["directory_branch_id"],"expected_tree_id":current["working_tree_id"],"expected_index_generation":current["index_generation"],"operation_id":new_id("op").unwrap()});
    for generation in [Value::Null, json!(1), json!("01")] {
        let mut invalid = stage.clone();
        invalid["expected_index_generation"] = generation;
        assert_eq!(
            send(&app, Some(&token), "POST", &stage_route, Some(invalid))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut missing = stage.clone();
    missing
        .as_object_mut()
        .unwrap()
        .remove("expected_index_generation");
    assert_eq!(
        send(&app, Some(&token), "POST", &stage_route, Some(missing))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let stage_result = success(&app, &token, "POST", &stage_route, Some(stage.clone())).await;
    assert_eq!(
        success(&app, &token, "POST", &stage_route, Some(stage.clone())).await,
        stage_result
    );
    let mut stale = stage;
    stale["operation_id"] = new_id("op").unwrap().into();
    assert_eq!(
        send(&app, Some(&token), "POST", &stage_route, Some(stale))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let staged = status(&app, &token, &repo, "docs").await;
    let commit_route = format!(
        "/api/repositories/{repo}/directory-branches/{}/commits",
        staged["directory_branch_id"].as_str().unwrap()
    );
    assert_eq!(send(&app, Some(&token), "POST", &commit_route, Some(json!({"expected_tree_id":staged["working_tree_id"],"message":"missing generation","operation_id":new_id("op").unwrap()}))).await.0, StatusCode::BAD_REQUEST);
    manager_upload(&app, &token, "docs/a.txt", b"later unstaged content").await;
    let reviewed = status(&app, &token, &repo, "docs").await;
    assert_eq!(reviewed["staged"], true);
    assert_eq!(reviewed["unstaged"], true);
    commit(&app, &token, &repo, &reviewed).await;
    let committed = status(&app, &token, &repo, "docs").await;
    assert!(committed["head_tree_id"].is_string());
    assert_eq!(committed["staged"], false);
    assert_eq!(committed["unstaged"], true);
    let branch = committed["directory_branch_id"].as_str().unwrap();
    let head = committed["head_tree_id"].as_str().unwrap();
    let snapshot = success(
        &app,
        &token,
        "GET",
        &format!("/api/file-manager/disks/P01/entries?path=docs&directory_branch_id={branch}&expected_tree_id={head}"),
        None,
    ).await;
    assert_eq!(
        version_bytes(
            &app,
            &token,
            snapshot["entries"][0]["version_id"].as_str().unwrap()
        )
        .await,
        b"one"
    );
    let working = listing(&app, &token, "docs").await;
    assert_eq!(
        version_bytes(
            &app,
            &token,
            working["entries"][0]["version_id"].as_str().unwrap()
        )
        .await,
        b"later unstaged content"
    );
    let system = root.path().join("system");
    drop(app);
    let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
    let auth = backend.open_auth(&system).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::new(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    assert_eq!(status(&app, &token, &repo, "docs").await, committed);
}

#[tokio::test]
async fn directory_git_parent_pins_only_explicitly_staged_child_commit() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/child/a.txt", b"one").await;
    let repo = listing(&app, &token, "docs").await["context"]["repository_id"]
        .as_str()
        .unwrap()
        .to_owned();
    initialize(&app, &token, &repo, "docs").await;
    initialize(&app, &token, &repo, "docs/child").await;
    let parent = status(&app, &token, &repo, "docs").await;
    let child = status(&app, &token, &repo, "docs/child").await;
    assert_eq!(parent["children"].as_array().unwrap().len(), 1);
    assert_eq!(child["head_tree_id"], Value::Null);
    let child_route = format!("/api/repositories/{repo}/directory-git/children/stage");
    let uncommitted = json!({"directory_id":parent["directory_id"],"directory_branch_id":parent["directory_branch_id"],"expected_index_generation":parent["index_generation"],"child_directory_id":child["directory_id"],"expected_child_head_tree_id":child["working_tree_id"],"operation_id":new_id("op").unwrap()});
    assert_eq!(
        send(&app, Some(&token), "POST", &child_route, Some(uncommitted))
            .await
            .0,
        StatusCode::CONFLICT
    );
    commit(&app, &token, &repo, &child).await;
    let child_committed = status(&app, &token, &repo, "docs/child").await;
    assert_eq!(
        status(&app, &token, &repo, "docs").await["head_tree_id"],
        Value::Null
    );
    stage_owned(
        &app,
        &token,
        &repo,
        &status(&app, &token, &repo, "docs").await,
    )
    .await;
    let parent_staged = status(&app, &token, &repo, "docs").await;
    let body = json!({"directory_id":parent_staged["directory_id"],"directory_branch_id":parent_staged["directory_branch_id"],"expected_index_generation":parent_staged["index_generation"],"child_directory_id":child_committed["directory_id"],"expected_child_head_tree_id":child_committed["head_tree_id"],"operation_id":new_id("op").unwrap()});
    let result = success(&app, &token, "POST", &child_route, Some(body.clone())).await;
    assert_eq!(
        success(&app, &token, "POST", &child_route, Some(body)).await,
        result
    );
    let parent_ready = status(&app, &token, &repo, "docs").await;
    commit(&app, &token, &repo, &parent_ready).await;
    let parent_committed = status(&app, &token, &repo, "docs").await;
    assert_eq!(
        parent_committed["children"][0]["committed_tree_id"],
        child_committed["head_tree_id"]
    );
    assert_eq!(
        status(&app, &token, &repo, "docs/child").await["head_tree_id"],
        child_committed["head_tree_id"]
    );
    manager_upload(&app, &token, "docs/child/b.txt", b"two").await;
    let dirty_child = status(&app, &token, &repo, "docs/child").await;
    assert_eq!(dirty_child["head_tree_id"], child_committed["head_tree_id"]);
    let dirty_parent = status(&app, &token, &repo, "docs").await;
    assert_eq!(
        dirty_parent["head_tree_id"],
        parent_committed["head_tree_id"]
    );
    assert_eq!(dirty_parent["children"][0]["dirty"], true);
    assert_eq!(
        dirty_parent["children"][0]["committed_tree_id"],
        child_committed["head_tree_id"]
    );
}

#[tokio::test]
async fn directory_git_workspace_delete_cannot_bypass_reversible_lifecycle() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/a.txt", b"original").await;
    manager_prepare_directory_branch(&app, &token, "docs", "frozen").await;
    let current = listing(&app, &token, "docs").await;
    let repository = current["context"]["repository_id"].as_str().unwrap();
    let original = status(&app, &token, repository, "docs").await;
    let version = current["entries"][0]["version_id"].as_str().unwrap();
    let workspace = success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({"branch_id":current["context"]["branch_id"]})),
    )
    .await;
    let workspace = workspace["workspace_id"].as_str().unwrap();
    success(
        &app,
        &token,
        "POST",
        &format!("/api/workspaces/{workspace}/namespace-operations"),
        Some(json!({"kind":"delete_tree","path":"docs"})),
    )
    .await;
    let mut commit = success(
        &app,
        &token,
        "POST",
        &format!("/api/workspaces/{workspace}/seal"),
        Some(json!({})),
    )
    .await;
    commit["request_digest"] = request_digest(
        "POST",
        "/api/workspaces/{workspace}/commit",
        json!({"workspace":workspace}),
        json!({}),
        &commit,
    )
    .unwrap()
    .into();
    let events = success(&app, &token, "GET", "/api/events", None).await;
    let bytes = manager_disk_files(root.path());
    let (code, rejected) = send(
        &app,
        Some(&token),
        "POST",
        &format!("/api/workspaces/{workspace}/commit"),
        Some(commit),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{rejected}");
    assert_eq!(rejected["error"]["code"], "UNSUPPORTED_OPERATION");
    assert_eq!(listing(&app, &token, "docs").await, current);
    assert_eq!(status(&app, &token, repository, "docs").await, original);
    assert_eq!(version_bytes(&app, &token, version).await, b"original");
    assert_eq!(manager_disk_files(root.path()), bytes);
    assert_eq!(
        success(&app, &token, "GET", "/api/events", None).await,
        events
    );
}
