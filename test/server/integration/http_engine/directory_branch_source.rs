//! Public request/receipt closure for a reviewed directory branch source.
use super::*;

const CREATE: &str = "/api/file-manager/disks/P01/directory-branches";

#[tokio::test]
async fn reviewed_directory_source_rejects_each_changed_identity_without_publication() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/a.txt", b"reviewed").await;
    manager_checkpoint_git(&app, &token, "docs").await;
    let source = manager_directory_branch_source(&app, &token, "docs").await;
    let before = manager_disk_files(root.path());
    for (field, value) in [
        ("repository_id", new_id("repo").unwrap()),
        ("directory_id", new_id("entry").unwrap()),
        ("directory_branch_id", new_id("dbr").unwrap()),
        ("working_tree_id", new_id("dtree").unwrap()),
        ("head_tree_id", new_id("dtree").unwrap()),
        ("index_generation", "18446744073709551615".into()),
        ("namespace_head", new_id("commit").unwrap()),
    ] {
        let operation = new_id("op").unwrap();
        let mut changed = source.clone();
        changed[field] = value.into();
        let (status, error) = send(
            &app,
            Some(&token),
            "POST",
            CREATE,
            Some(json!({
                "path":"docs", "name":"reviewed copy",
                "operation_id":operation, "source":changed
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{field}: {error}");
        assert_eq!(error["error"]["code"], "DIRECTORY_BRANCH_CONFLICT");
        assert_eq!(
            send(
                &app,
                Some(&token),
                "GET",
                &format!("/api/operations/{operation}"),
                None,
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(manager_disk_files(root.path()), before, "{field}");
    }
    let mut missing = json!({
        "path":"docs", "name":"reviewed copy", "operation_id":new_id("op").unwrap()
    });
    assert_eq!(
        send(&app, Some(&token), "POST", CREATE, Some(missing.clone()))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    missing["source"] = source.clone();
    missing["source"]["index_generation"] = json!(1);
    assert_eq!(
        send(&app, Some(&token), "POST", CREATE, Some(missing))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(manager_disk_files(root.path()), before);
}

#[tokio::test]
async fn reviewed_directory_original_http_request_replays_after_source_changes_and_reopen() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/a.txt", b"frozen").await;
    manager_checkpoint_git(&app, &token, "docs").await;
    let source = manager_directory_branch_source(&app, &token, "docs").await;
    let operation = new_id("op").unwrap();
    let body = json!({
        "path":"docs", "name":"frozen source", "operation_id":operation, "source":source
    });
    let receipt = success(&app, &token, "POST", CREATE, Some(body.clone())).await;
    let input = json!({
        "repository_id":source["repository_id"], "path":"docs",
        "name":"frozen source", "source":source
    });
    let digest = storage_contract::object_hash(
        "request",
        &json!({"kind":"directory.git.branch-create","principal":"admin","body":input}),
    )
    .unwrap();
    assert_eq!(receipt["request_digest"], digest);
    assert_eq!(receipt["result"]["input"], input);
    assert_eq!(receipt["result"]["materialized"], false);
    assert_eq!(receipt["result"]["selected"], false);
    assert_eq!(receipt["result"]["copied_files"], "0");
    assert_eq!(receipt["result"]["placeholder_files"], "0");

    manager_upload(&app, &token, "docs/a.txt", b"new saved content").await;
    manager_checkpoint_git(&app, &token, "docs").await;
    let fresh = manager_directory_branch_source(&app, &token, "docs").await;
    assert_ne!(fresh, source);
    let mut changed = body.clone();
    changed["source"] = fresh;
    let (status, error) = send(&app, Some(&token), "POST", CREATE, Some(changed)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "IDEMPOTENCY_KEY_REUSED");

    let mut new_request = body.clone();
    new_request["operation_id"] = new_id("op").unwrap().into();
    new_request["name"] = "new stale request".into();
    let (status, error) = send(&app, Some(&token), "POST", CREATE, Some(new_request)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "DIRECTORY_BRANCH_CONFLICT");
    manager_checkpoint_and_select(
        &app,
        &token,
        source["repository_id"].as_str().unwrap(),
        receipt["result"]["directory_branch_id"].as_str().unwrap(),
    )
    .await;
    let replay = success(&app, &token, "POST", CREATE, Some(body.clone())).await;
    for key in [
        "operation_id",
        "request_digest",
        "publication_sequence",
        "result",
    ] {
        assert_eq!(replay[key], receipt[key], "{key}");
    }
    let revoked = revoked_session(&app).await;
    for session in [None, Some(revoked.as_str())] {
        assert_eq!(
            send(&app, session, "POST", CREATE, Some(body.clone()))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }

    drop(app);
    let system = root.path().join("system");
    let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
    let auth = backend.open_auth(&system).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::new(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let replay = success(&app, &token, "POST", CREATE, Some(body)).await;
    for key in [
        "operation_id",
        "request_digest",
        "publication_sequence",
        "result",
    ] {
        assert_eq!(replay[key], receipt[key], "{key}");
    }
}
