//! Named request records must not admit positional JSON arrays at any depth.
use super::*;

#[tokio::test]
async fn namespace_batch_requires_named_nested_objects_before_mutation() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "report.txt", b"retain this file").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let branch = manager_directory_branch(&app, &token, "", "schema-check").await;
    let before = manager_scope(&app, &token, repo, &branch).await;
    let source = manager_batch_source(&before, "report.txt");
    let target = manager_batch_target(&before, "renamed.txt");
    let source_array = json!([
        source["directory_branch_id"],
        source["expected_tree_id"],
        source["path"],
        source["entry_id"]
    ]);
    let target_array = json!([
        target["directory_branch_id"],
        target["expected_tree_id"],
        target["path"]
    ]);
    let route = "/api/file-manager/disks/P01/namespace-batch";
    for item in [
        json!([source, target]),
        json!({"source":source_array,"target":target}),
        json!({"source":source,"target":target_array}),
    ] {
        let operation = new_id("op");
        let (status, rejected) = send(
            &app,
            Some(&token),
            "POST",
            route,
            Some(json!({"kind":"rename","items":[item],"operation_id":operation})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
        assert_eq!(rejected["error"]["code"], "INVALID_REQUEST");
        assert_eq!(manager_scope(&app, &token, repo, &branch).await, before);
    }
    for kind in [
        json!("unknown"),
        json!({"rename":null}),
        json!(null),
        json!(0),
        json!([]),
    ] {
        let (status, rejected) = send(
            &app, Some(&token), "POST", route,
            Some(json!({"kind":kind,"items":[{"source":source,"target":target}],"operation_id":new_id("op")})),
        ).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
        assert_eq!(rejected["error"]["code"], "INVALID_REQUEST");
        assert_eq!(manager_scope(&app, &token, repo, &branch).await, before);
    }
    // Legitimate arrays of named records remain supported.
    success(
        &app,
        &token,
        "POST",
        route,
        Some(
            json!({"kind":"rename","items":[{"source":source,"target":target}],
            "operation_id":new_id("op")}),
        ),
    )
    .await;
    let after = manager_scope(&app, &token, repo, &branch).await;
    assert_ne!(after["head_tree_id"], before["head_tree_id"]);
    assert!(
        after["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "renamed.txt")
    );
    assert!(
        after["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["path"] != "report.txt")
    );
}

#[tokio::test]
async fn operation_id_boundary_preserves_error_codes_and_canonical_wire() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let before = success(&app, &token, "GET", "/api/repositories", None).await;
    for (operation, code) in [
        (None, "INVALID_REQUEST"),
        (Some(Value::Null), "INVALID_REQUEST"),
        (Some(json!(7)), "INVALID_REQUEST"),
        (Some(json!("")), "INVALID_REQUEST"),
        (
            Some(json!("op_0000000000000000000000000000000A")),
            "INVALID_ID",
        ),
        (
            Some(json!("op_000000000000000000000000000000000")),
            "INVALID_ID",
        ),
        (Some(json!("wrong")), "INVALID_ID"),
    ] {
        let mut body = json!({"name":"Boundary","primary_slot_id":"P01","target_copies":1});
        if let Some(operation) = operation {
            body["operation_id"] = operation;
        }
        let (status, rejected) =
            send(&app, Some(&token), "POST", "/api/repositories", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
        assert_eq!(rejected["error"]["code"], code);
        assert_eq!(
            success(&app, &token, "GET", "/api/repositories", None).await,
            before
        );
    }
    for operation in ["wrong", "op_0000000000000000000000000000000A"] {
        let (status, rejected) = send(
            &app,
            Some(&token),
            "GET",
            &format!("/api/operations/{operation}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(rejected["error"]["code"], "OPERATION_NOT_FOUND");
        let (status, rejected) = send(
            &app,
            Some(&token),
            "POST",
            "/api/file-manager/disks/P01/namespace",
            Some(json!({"kind":"mkdir","path":"invalid","operation_id":operation})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(rejected["error"]["code"], "INVALID_REQUEST");
    }
    for (name, operation) in [
        ("Zero", "op_00000000000000000000000000000000"),
        ("NonV4", "op_ffffffffffffffffffffffffffffffff"),
    ] {
        let (status, missing) = send(
            &app,
            Some(&token),
            "GET",
            &format!("/api/operations/{operation}/outcome"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(missing["error"]["code"], "OPERATION_NOT_FOUND");
        let body =
            json!({"name":name,"primary_slot_id":"P01","target_copies":1,"operation_id":operation});
        let created = success(
            &app,
            &token,
            "POST",
            "/api/repositories",
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            success(&app, &token, "POST", "/api/repositories", Some(body)).await,
            created
        );
        let receipt = success(
            &app,
            &token,
            "GET",
            &format!("/api/operations/{operation}"),
            None,
        )
        .await;
        assert_eq!(receipt["operation_id"], operation);
    }
}

#[tokio::test]
async fn namespace_batch_preserves_null_digest_and_replays_after_reopen() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "report.txt", b"delete frozen entry").await;
    manager_upload(&app, &token, "stay.txt", b"retain sibling").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let branch = manager_directory_branch(&app, &token, "", "batch-replay").await;
    let before = manager_scope(&app, &token, repo, &branch).await;
    let source = manager_batch_source(&before, "report.txt");
    let route = "/api/file-manager/disks/P01/namespace-batch";
    let operation = new_id("op");
    let request = json!({"kind":"delete","items":[{"source":source}],"operation_id":operation});
    for (key, value, code) in [
        ("items", None, "INVALID_REQUEST"),
        ("items", Some(Value::Null), "INVALID_REQUEST"),
        ("items", Some(json!({})), "INVALID_REQUEST"),
        ("items", Some(json!([])), "INVALID_REQUEST"),
        ("operation_id", None, "INVALID_REQUEST"),
        ("operation_id", Some(Value::Null), "INVALID_REQUEST"),
        ("operation_id", Some(json!(7)), "INVALID_REQUEST"),
        ("operation_id", Some(json!("")), "INVALID_REQUEST"),
        ("operation_id", Some(json!("bad")), "INVALID_ID"),
        ("unknown", Some(json!(true)), "INVALID_REQUEST"),
        ("slot_id", Some(json!("other")), "INVALID_REQUEST"),
    ] {
        let mut invalid = request.clone();
        match value {
            Some(value) => {
                invalid[key] = value;
            }
            None => {
                invalid.as_object_mut().unwrap().remove(key);
            }
        }
        let (status, rejected) = send(&app, Some(&token), "POST", route, Some(invalid)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
        assert_eq!(rejected["error"]["code"], code);
        assert_eq!(manager_scope(&app, &token, repo, &branch).await, before);
    }
    for invalid in [
        json!({"kind":"rename","items":[{"source":source}],"operation_id":new_id("op")}),
        json!({"kind":"delete","items":[{"source":source,"target":manager_batch_target(&before,"else.txt")}],"operation_id":new_id("op")}),
        json!({"kind":"delete","items":[{"source":{"directory_branch_id":branch,"expected_tree_id":before["head_tree_id"],"entry_id":source["entry_id"],"path":"../report.txt"}}],"operation_id":new_id("op")}),
    ] {
        let (status, rejected) = send(&app, Some(&token), "POST", route, Some(invalid)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
        assert_eq!(manager_scope(&app, &token, repo, &branch).await, before);
    }
    let original = success(&app, &token, "POST", route, Some(request.clone())).await;
    let canonical = json!({"kind":"delete","items":[{"source":source,"target":null}]});
    assert_eq!(original["request_digest"], storage_contract::object_hash("request", &json!({"kind":"directory.namespace.batch","principal":"admin","body":{"repository_id":repo,"request":canonical}})).unwrap());
    let after_delete = manager_scope(&app, &token, repo, &branch).await;
    success(&app,&token,"POST",route,Some(json!({"kind":"rename","operation_id":new_id("op"),"items":[{"source":manager_batch_source(&after_delete,"stay.txt"),"target":manager_batch_target(&after_delete,"later.txt")}]}))).await;
    let later = manager_scope(&app, &token, repo, &branch).await;
    drop(app);
    let system = root.path().join("system");
    let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
    let auth = backend.open_auth(&system).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let revoked = login(&app, "admin", "integration-test-password").await;
    success(&app, &revoked, "POST", "/api/auth/logout", None).await;
    assert_eq!(
        send(&app, Some(&revoked), "POST", route, Some(request.clone()))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            Some(&token),
            "POST",
            "/api/file-manager/disks/B01/namespace-batch",
            Some(request.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let mut explicit_null = request.clone();
    explicit_null["items"][0]["target"] = Value::Null;
    for replay_request in [request.clone(), explicit_null] {
        let replay = success(&app, &token, "POST", route, Some(replay_request)).await;
        for field in [
            "operation_id",
            "request_digest",
            "publication_sequence",
            "result",
        ] {
            assert_eq!(replay[field], original[field], "{field}");
        }
        assert_eq!(manager_scope(&app, &token, repo, &branch).await, later);
    }
    let mut changed = request.clone();
    changed["items"][0]["source"]["path"] = "stay.txt".into();
    let (status, rejected) = send(&app, Some(&token), "POST", route, Some(changed)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(rejected["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
    let mut fresh = request;
    fresh["operation_id"] = new_id("op").into();
    let (status, rejected) = send(&app, Some(&token), "POST", route, Some(fresh)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(rejected["error"]["code"], "DIRECTORY_BRANCH_CONFLICT");
    assert_eq!(manager_scope(&app, &token, repo, &branch).await, later);
}
