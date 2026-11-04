use super::*;
use std::collections::BTreeMap;
use storage_contract::directory_history::*;

async fn typed_page(
    app: &Router,
    token: &str,
    request: &DirectoryHistoryRequest,
) -> DirectoryHistoryPage {
    let query = url_encoding::form::Serializer::new(String::new())
        .extend_pairs(request.query_fields())
        .finish();
    let value = success(
        app,
        token,
        "GET",
        &format!("{}?{query}", request.route()),
        None,
    )
    .await;
    let page: DirectoryHistoryPage = json_codec::from_value(value).unwrap();
    page.validate(request).unwrap();
    page
}
#[tokio::test]
async fn directory_history_http_is_read_only_root_bound_and_authenticated() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/a.txt", b"one").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let directory = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "docs")
        .unwrap()["entry_id"]
        .as_str()
        .unwrap();
    // A disk-root upload defaults to one primary copy. Publish actual backup
    // placement before exercising B01, rather than inventing a backup folder.
    let backup_route = format!("/api/repositories/{repo}/backup-configuration");
    let protection = success(&app, &token, "GET", &backup_route, None).await;
    assert_eq!(protection["target_copies"], 1);
    success(
        &app,
        &token,
        "PUT",
        &backup_route,
        Some(json!({
            "path":"", "target_copies":3,
            "backup_prefixes":{"B01":"P01","C01":"P01"},
            "expected_scope_id":protection["scope"]["id"],
            "expected_generation":protection["generation"],
            "operation_id":new_id("op").unwrap()
        })),
    )
    .await;
    let request = DirectoryHistoryRequest::parse(
        repo,
        directory,
        DirectoryHistoryCollection::Nodes,
        BTreeMap::from([("limit".into(), "1".into())]),
    )
    .unwrap();
    let first = typed_page(&app, &token, &request).await;
    assert_eq!(
        typed_page(&app, &token, &request).await,
        first,
        "a read must not mutate or initialize history"
    );
    for name in ["pending one", "pending two"] {
        success(
            &app,
            &token,
            "POST",
            "/api/file-manager/disks/P01/directory-branches",
            Some(json!({"path":"docs","name":name,"operation_id":new_id("op").unwrap()})),
        )
        .await;
    }
    let branch_request = DirectoryHistoryRequest::parse(
        repo,
        directory,
        DirectoryHistoryCollection::Branches,
        BTreeMap::new(),
    )
    .unwrap();
    let branches = typed_page(&app, &token, &branch_request).await;
    let pending = branches
        .branches
        .iter()
        .filter(|b| !b.materialized)
        .collect::<Vec<_>>();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].head_tree_id, pending[1].head_tree_id);
    let stale = format!(
        "{}?limit=1&history_root={}",
        request.route(),
        first.identity.history_root
    );
    let (status, error) = send(&app, Some(&token), "GET", &stale, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "HISTORY_CHANGED");
    for route in [request.route(), branch_request.route()] {
        assert_eq!(
            send(&app, None, "GET", &route, None).await.0,
            StatusCode::UNAUTHORIZED
        );
        let revoked = revoked_session(&app).await;
        assert_eq!(
            send(&app, Some(&revoked), "GET", &route, None).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            send(
                &app,
                Some(&token),
                "GET",
                &format!("{route}?limit=129"),
                None
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    // A read-only branch preview retains the original folder's history scope.
    let preview = success(
        &app,
        &token,
        "GET",
        &format!(
            "/api/file-manager/disks/P01/entries?path=docs&directory_branch_id={}&expected_tree_id={}",
            pending[0].directory_branch_id, pending[0].head_tree_id
        ),
        None,
    )
    .await;
    assert_eq!(preview["preview"], true);
    assert_eq!(preview["writable"], false);
    assert_eq!(preview["context"]["repository_id"], repo);
    assert_eq!(preview["entries"][0]["read_only"], true);
    assert_eq!(typed_page(&app, &token, &branch_request).await, branches);
    // Backup folders aggregate retained file paths; they are not original
    // directory identities and must never become fabricated history scopes.
    let backup = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/B01/entries?path=P01",
        None,
    )
    .await;
    let entry = backup["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "docs")
        .unwrap();
    let aggregate = entry["entry_id"].as_str().unwrap();
    assert_ne!(aggregate, directory);
    assert!(entry["repository_id"].is_null());
    assert_eq!(entry["read_only"], true);
    assert_eq!(entry["backup"], true);
    assert_eq!(backup["writable"], false);
    assert!(backup["context"].is_null());
    assert_eq!(backup["directory_branches_supported"], false);
    let (status, rejected) = send(
        &app,
        Some(&token),
        "GET",
        &format!("/api/repositories/{repo}/directories/{aggregate}/history"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(rejected["error"]["code"], "INVALID_ID");
    assert_eq!(typed_page(&app, &token, &branch_request).await, branches);
    assert_eq!(
        send(
            &app,
            Some(&token),
            "POST",
            "/api/file-manager/disks/B01/directory-branches",
            Some(json!({"path":"P01/docs","name":"denied","operation_id":new_id("op").unwrap()}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let before = typed_page(&app, &token, &request).await;
    drop(app);
    let system = root.path().join("system");
    let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
    let auth = backend.open_auth(&system).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::new(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    assert_eq!(typed_page(&app, &token, &request).await, before);
}
