//! Real HTTP cursor binding, auth refresh, global search and invalidation.
use super::*;
fn endpoint(pairs: &[(&str, &str)]) -> String {
    let mut query = url_encoding::form::Serializer::new(String::new());
    for (key, value) in pairs {
        query.append_pair(key, value);
    }
    format!("/api/file-manager/disks/P01/entries?{}", query.finish())
}
#[tokio::test]
async fn paginated_listing_rechecks_auth_and_rejects_changed_snapshots() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    for path in ["folder/zeta.txt", "folder/alpha.txt", "folder/beta.txt"] {
        manager_upload(&app, &token, path, b"data").await;
    }
    let first = success(
        &app,
        &token,
        "GET",
        &endpoint(&[("path", "folder"), ("limit", "1")]),
        None,
    )
    .await;
    let repository = first["context"]["repository_id"].as_str().unwrap();
    success(
        &app,
        &token,
        "POST",
        &format!("/api/repositories/{repository}/directory-scopes"),
        Some(json!({"path":"folder","operation_id":new_id("op")})),
    )
    .await;
    let first = success(
        &app,
        &token,
        "GET",
        &endpoint(&[("path", "folder"), ("limit", "1")]),
        None,
    )
    .await;
    assert_eq!(first["entries"][0]["name"], "alpha.txt");
    assert_eq!(first["total_entries"], "3");
    let cursor = first["next_cursor"].as_str().unwrap();
    let next_path = endpoint(&[("path", "folder"), ("limit", "1"), ("cursor", cursor)]);
    // A new login publishes auth state but leaves the business snapshot valid.
    let second_token = login(&app, "admin", "integration-test-password").await;
    let second = success(&app, &second_token, "GET", &next_path, None).await;
    assert_eq!(second["entries"][0]["name"], "beta.txt");
    assert_eq!(second["snapshot"], first["snapshot"]);
    let mismatched = endpoint(&[
        ("path", "folder"),
        ("limit", "1"),
        ("query", "alpha"),
        ("cursor", cursor),
    ]);
    let (status, error) = send(&app, Some(&token), "GET", &mismatched, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    let filtered = success(
        &app,
        &token,
        "GET",
        &endpoint(&[("path", "folder"), ("limit", "1"), ("query", "ZETA")]),
        None,
    )
    .await;
    assert_eq!(filtered["entries"][0]["name"], "zeta.txt");
    assert_eq!(filtered["total_entries"], "1");
    for branch in first["directory_branches"].as_array().unwrap() {
        for key in [
            "entries",
            "file_contexts",
            "file_branches",
            "inherited_files",
            "directory_refs",
        ] {
            assert!(branch.get(key).is_none());
        }
    }
    let repository = first["context"]["repository_id"].as_str().unwrap();
    let branch = first["directory_branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|branch| branch["current"] == true)
        .unwrap();
    let scopes_path = format!("/api/repositories/{repository}/directory-branches");
    let scope_request = json!({"path":"folder","sources":[{"directory_branch_id":branch["directory_branch_id"],"expected_tree_id":branch["head_tree_id"]}],"entries":[{"entry_id":first["entries"][0]["entry_id"],"path":"folder/alpha.txt"}]});
    let ready_before = success(&app, &token, "GET", "/readyz", None).await;
    let facts = success(
        &app,
        &token,
        "POST",
        &scopes_path,
        Some(scope_request.clone()),
    )
    .await;
    assert_eq!(
        facts["directory_branches"][0]["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        facts["directory_branches"][0]["entries"][0]["selected_path"],
        "folder/alpha.txt"
    );
    let ready_after = success(&app, &token, "GET", "/readyz", None).await;
    assert!(ready_before["sequence"].is_string());
    assert_eq!(ready_before["sequence"], ready_after["sequence"]);
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&scopes_path)
                .header("cookie", format!("zxs_session={token}"))
                .header("content-type", "application/json")
                .body(Body::from(json_codec::to_vec(&scope_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        denied.status(),
        StatusCode::FORBIDDEN,
        "read-only POST retains normal cookie CSRF admission"
    );
    manager_upload(&app, &token, "folder/new.txt", b"new").await;
    let (status, error) = send(&app, Some(&token), "GET", &next_path, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "LISTING_CHANGED");
    let (status, error) = send(
        &app,
        Some(&token),
        "POST",
        &scopes_path,
        Some(scope_request),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "LISTING_CHANGED");
    success(&app, &token, "POST", "/api/auth/logout", None).await;
    assert_eq!(
        send(&app, Some(&token), "GET", &next_path, None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn canonical_equivalent_queries_resolve_and_browse_without_rewriting_display_names() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    for path in ["Café/Å.txt", "Café/가.txt"] {
        manager_upload(&app, &token, path, b"body").await;
    }
    let canonical = success(
        &app,
        &token,
        "GET",
        &endpoint(&[("path", "Café"), ("limit", "1")]),
        None,
    )
    .await;
    let queried = success(
        &app,
        &token,
        "GET",
        &endpoint(&[("path", "Cafe\u{301}"), ("limit", "1")]),
        None,
    )
    .await;
    assert_eq!(queried["entries"], canonical["entries"]);
    assert_eq!(queried["total_entries"], "2");
    let cursor = queried["next_cursor"].as_str().unwrap();
    let next = success(
        &app,
        &token,
        "GET",
        &endpoint(&[("path", "Cafe\u{301}"), ("limit", "1"), ("cursor", cursor)]),
        None,
    )
    .await;
    assert_eq!(next["entries"][0]["path"], "Café/가.txt");
    let branch = canonical["context"]["branch_id"].as_str().unwrap();
    let resolve = format!("/api/branches/{branch}/resolve");
    for (stored, query) in [
        ("Café/Å.txt", "Cafe\u{301}/A\u{30a}.txt"),
        ("Café/가.txt", "Cafe\u{301}/\u{1100}\u{1161}.txt"),
    ] {
        let expected = success(&app, &token, "POST", &resolve, Some(json!({"path":stored}))).await;
        let actual = success(&app, &token, "POST", &resolve, Some(json!({"path":query}))).await;
        assert_eq!(actual, expected);
    }
    let workspace = success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({"branch_id":branch})),
    )
    .await;
    let ws = workspace["workspace_id"].as_str().unwrap();
    let route = format!("/api/workspaces/{ws}/namespace-operations");
    let before = success(&app, &token, "GET", &format!("/api/workspaces/{ws}"), None).await;
    for request in [
        json!({"kind":"mkdir","path":"new-e\u{301}"}),
        json!({"kind":"rename","path":"Café/Å.txt","destination":"new-e\u{301}.txt"}),
    ] {
        let (status, response) = send(&app, Some(&token), "POST", &route, Some(request)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        assert_eq!(response["error"]["code"], "NAME_NOT_NORMALIZED");
        assert_eq!(
            success(&app, &token, "GET", &format!("/api/workspaces/{ws}"), None).await,
            before
        );
    }
    let (status, response) = send(
        &app,
        Some(&token),
        "POST",
        "/api/file-manager/disks/P01/write-sessions",
        Some(json!({"path":"new-e\u{301}.txt"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(response["error"]["code"], "INVALID_REQUEST");
    assert_eq!(
        success(
            &app,
            &token,
            "GET",
            &endpoint(&[("path", "Café"), ("limit", "1")]),
            None
        )
        .await,
        canonical
    );
    for request in [
        json!({"kind":"rename","path":"Cafe\u{301}/A\u{30a}.txt","destination":"renamed.txt"}),
        json!({"kind":"delete","path":"Cafe\u{301}/\u{1100}\u{1161}.txt"}),
    ] {
        success(&app, &token, "POST", &route, Some(request)).await;
    }
    let mut commit = success(
        &app,
        &token,
        "POST",
        &format!("/api/workspaces/{ws}/seal"),
        Some(json!({})),
    )
    .await;
    commit["request_digest"] = request_digest(
        "POST",
        "/api/workspaces/{workspace}/commit",
        json!({"workspace":ws}),
        json!({}),
        &commit,
    )
    .unwrap()
    .into();
    success(
        &app,
        &token,
        "POST",
        &format!("/api/workspaces/{ws}/commit"),
        Some(commit),
    )
    .await;
    let after = success(&app, &token, "GET", &endpoint(&[]), None).await;
    let entries = after["entries"].as_array().unwrap();
    assert!(entries.iter().any(|entry| entry["path"] == "Café"));
    let renamed = entries
        .iter()
        .find(|entry| entry["path"] == "renamed.txt")
        .unwrap();
    for field in ["entry_id", "file_id", "version_id"] {
        assert_eq!(renamed[field], canonical["entries"][0][field], "{field}");
    }
    assert_eq!(
        success(
            &app,
            &token,
            "GET",
            &endpoint(&[("path", "Cafe\u{301}")]),
            None
        )
        .await["entries"],
        json!([])
    );
}
