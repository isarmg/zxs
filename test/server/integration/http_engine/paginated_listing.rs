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
