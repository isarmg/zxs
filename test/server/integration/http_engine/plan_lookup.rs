//! Missing local plan preparations are not retryable storage failures.
use super::*;

#[tokio::test]
async fn absent_plan_returns_not_found_without_publishing_an_operation() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let baseline = success(&app, &token, "GET", "/api/events", None).await;
    let plan = new_id("plan");
    for (method, suffix, body) in [
        ("GET", "", None),
        (
            "POST",
            "/cancel",
            Some(json!({"operation_id":new_id("op")})),
        ),
        (
            "POST",
            "/commit",
            Some(json!({"operation_id":new_id("op"),"plan_digest":"0".repeat(64)})),
        ),
    ] {
        let (status, rejected) = send(
            &app,
            Some(&token),
            method,
            &format!("/api/plans/{plan}{suffix}"),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{rejected}");
        assert_eq!(rejected["error"]["code"], "PLAN_NOT_FOUND");
        assert_eq!(rejected["error"]["retryable"], false);
    }
    assert_eq!(
        success(&app, &token, "GET", "/api/events", None).await,
        baseline
    );
}

#[tokio::test]
async fn plan_read_and_cancel_preserve_raw_shapes_and_permanent_replay() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let repo = success(&app, &token, "POST", "/api/repositories", Some(json!({
        "name":"Plan HTTP", "primary_slot_id":"P01", "target_copies":1, "operation_id":new_id("op")
    }))).await;
    let branch = success(
        &app,
        &token,
        "GET",
        &format!("/api/branches/{}", repo["main_branch_id"].as_str().unwrap()),
        None,
    )
    .await;
    let plan = success(&app, &token, "POST", &format!("/api/history-trees/{}/historical-save-plans",repo["history_tree_id"].as_str().unwrap()), Some(json!({
        "source_commit_id":branch["head_commit_id"], "name":"Prepared", "workspace_id":null, "primary_slot_id":null, "target_copies":null
    }))).await;
    let path = format!("/api/plans/{}", plan["plan_id"].as_str().unwrap());
    assert_eq!(
        success(&app, &token, "GET", &format!("{path}?unused=kept"), None).await,
        plan
    );
    let cancel = format!("{path}/cancel");
    let operation = storage_contract::OperationId::generate();
    let body = json!({"operation_id":operation});
    for (invalid, code) in [
        (
            json!({"operation_id":operation,"unknown":null}),
            "INVALID_REQUEST",
        ),
        (json!({"operation_id":"bad"}), "INVALID_ID"),
        (json!({}), "INVALID_REQUEST"),
        (
            json!({"operation_id":operation,"plan_id":new_id("plan")}),
            "INVALID_REQUEST",
        ),
    ] {
        let (status, rejected) = send(&app, Some(&token), "POST", &cancel, Some(invalid)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
        assert_eq!(rejected["error"]["code"], code);
    }
    let receipt = success(&app, &token, "POST", &cancel, Some(body.clone())).await;
    let typed: storage_contract::OperationResult = json_codec::from_value(receipt.clone()).unwrap();
    assert_eq!(typed.operation_id, operation);
    assert_eq!(typed.kind, "plan.cancel");
    assert!(receipt["sequence"].is_string());
    assert!(receipt["current_health"].is_string());
    assert!(receipt.get("operation_kind").is_none());
    assert_eq!(
        success(&app, &token, "POST", &cancel, Some(body.clone())).await,
        receipt
    );
    let cancelled = success(&app, &token, "GET", &path, None).await;
    assert_eq!(cancelled["state"], "cancelled");
    let (status, closed) = send(
        &app,
        Some(&token),
        "POST",
        &cancel,
        Some(json!({"operation_id":new_id("op")})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(closed["error"]["code"], "PLAN_CLOSED");
    drop(app);
    let backend = Arc::new(EngineBackend::new(
        reopen_after_router_drop(&root.path().join("system")).await,
    ));
    let auth = backend.open_auth(&root.path().join("system")).unwrap();
    let app = storage_server::router(
        backend,
        auth,
        ServerConfig::loopback(&root.path().join("system")),
    )
    .unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    assert_eq!(
        success(&app, &token, "POST", &cancel, Some(body)).await,
        receipt
    );
    assert_eq!(success(&app, &token, "GET", &path, None).await, cancelled);
}

#[tokio::test]
async fn cancel_preserves_missing_plan_before_operation_errors_but_after_unknown_fields() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let path = format!("/api/plans/{}/cancel", new_id("plan"));
    for body in [
        json!({}),
        json!({"operation_id":"bad"}),
        json!({"operation_id":null}),
    ] {
        let (status, rejected) = send(&app, Some(&token), "POST", &path, Some(body)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(rejected["error"]["code"], "PLAN_NOT_FOUND");
    }
    let (status, rejected) = send(
        &app,
        Some(&token),
        "POST",
        &path,
        Some(json!({"unknown":null})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(rejected["error"]["code"], "INVALID_REQUEST");
}

#[tokio::test]
async fn commit_preserves_required_fields_before_lookup_and_format_errors_after_lookup() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let path = format!("/api/plans/{}/commit", new_id("plan"));
    for (body, expected) in [
        (json!({}), "INVALID_REQUEST"),
        (
            json!({"operation_id":null,"plan_digest":"digest"}),
            "INVALID_REQUEST",
        ),
        (
            json!({"operation_id":"bad","plan_digest":""}),
            "INVALID_REQUEST",
        ),
        (
            json!({"operation_id":"bad","plan_digest":"digest","confirmed":true}),
            "INVALID_REQUEST",
        ),
        (
            json!({"operation_id":"bad","plan_digest":"digest","confirmed":false}),
            "INVALID_REQUEST",
        ),
        (
            json!({"operation_id":"bad","plan_digest":"digest"}),
            "PLAN_NOT_FOUND",
        ),
    ] {
        let (_, rejected) = send(&app, Some(&token), "POST", &path, Some(body)).await;
        assert_eq!(rejected["error"]["code"], expected);
    }
}

#[tokio::test]
async fn committed_purge_and_split_http_retry_resume_cleanup_before_returning_receipt() {
    for split in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("system");
        let mut engine = Engine::initialize(
            &system,
            Config::development([
                root.path().join("d1"),
                root.path().join("d2"),
                root.path().join("d3"),
            ]),
        )
        .unwrap();
        let repo = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Cleanup HTTP").unwrap(),
                "P01",
                3,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let save = |engine: &mut Engine, bytes: &[u8]| {
            let workspace = engine
                .create_workspace(&repo.main_branch_id, "admin")
                .unwrap();
            engine
                .stage_bytes(&workspace.workspace_id, "file.txt", bytes)
                .unwrap();
            engine
                .commit_workspace(CommitRequest {
                    workspace_id: workspace.workspace_id,
                    operation_id: storage_contract::OperationId::generate(),
                    principal: "admin".into(),
                    expected_head: workspace.base_head,
                    fencing_token: workspace.fencing_token,
                    message: "cleanup".into(),
                    request_digest: None,
                })
                .unwrap()
                .commit_id
                .unwrap()
        };
        let cut = save(&mut engine, b"old bytes");
        let first = engine.list_tree(&repo.main_branch_id).unwrap()[0]
            .version_id
            .clone();
        save(&mut engine, b"latest bytes");
        let plan = if split {
            engine.create_split_plan(&repo.history_tree_id, &cut, "admin")
        } else {
            engine.create_purge_plan(&repo.history_tree_id, vec![first.clone()], "admin")
        }
        .unwrap();
        let directories = engine
            .config()
            .disks
            .iter()
            .map(|disk| {
                disk.root
                    .join("histories")
                    .join(&repo.history_tree_id)
                    .join("versions")
                    .join(&first)
            })
            .collect::<Vec<_>>();
        let payloads = directories
            .iter()
            .map(|directory| fixture_payload(directory, engine.state()))
            .collect::<Vec<_>>();
        let blocker = directories[0].join("purged.json");
        std::fs::create_dir(&blocker).unwrap();
        let backend = Arc::new(EngineBackend::new(engine));
        let auth = backend
            .protect_auth(
                AuthStore::initialize(&system, "admin", "integration-test-password").unwrap(),
            )
            .unwrap();
        let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
        let token = login(&app, "admin", "integration-test-password").await;
        let operation = storage_contract::OperationId::generate();
        let path = format!("/api/plans/{}/commit", plan.plan_id);
        let body = json!({"operation_id":operation,"plan_digest":plan.digest});
        for wrong in [
            json!({"operation_id":"bad","plan_digest":"wrong"}),
            json!({"operation_id":operation,"plan_digest":"wrong"}),
        ] {
            let (_, rejected) = send(&app, Some(&token), "POST", &path, Some(wrong)).await;
            assert_eq!(
                rejected["error"]["code"],
                if split {
                    "SPLIT_PLAN_STALE"
                } else {
                    "PLAN_STALE"
                }
            );
        }
        let (_, failed) = send(&app, Some(&token), "POST", &path, Some(body.clone())).await;
        assert_eq!(failed["error"]["code"], "UNSAFE_PATH");
        assert_eq!(
            success(
                &app,
                &token,
                "GET",
                &format!("/api/plans/{}", plan.plan_id),
                None
            )
            .await["state"],
            "committed"
        );
        assert!(payloads.iter().all(|path| path.exists()));
        let published_events = success(&app, &token, "GET", "/api/events", None).await;
        let (_, retried_failure) =
            send(&app, Some(&token), "POST", &path, Some(body.clone())).await;
        assert_eq!(retried_failure["error"]["code"], "UNSAFE_PATH");
        assert_eq!(
            success(&app, &token, "GET", "/api/events", None).await,
            published_events
        );
        assert!(payloads.iter().all(|path| path.exists()));
        std::fs::remove_dir(&blocker).unwrap();
        let receipt = success(&app, &token, "POST", &path, Some(body.clone())).await;
        assert_eq!(receipt["operation_id"], operation.to_string());
        assert_eq!(receipt["commit_state"], "PUBLISHED");
        assert!(receipt["current_health"].is_string());
        assert!(receipt.get("operation_kind").is_none());
        assert_eq!(
            success(&app, &token, "GET", "/api/events", None).await,
            published_events
        );
        assert!(payloads.iter().all(|path| !path.exists()));
        assert!(blocker.is_file());
        drop(app);
        let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
        let auth = backend.open_auth(&system).unwrap();
        let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
        let token = login(&app, "admin", "integration-test-password").await;
        let replay = success(&app, &token, "POST", &path, Some(body)).await;
        assert_eq!(replay["request_digest"], receipt["request_digest"]);
        assert_eq!(replay["result"], receipt["result"]);
        assert_eq!(replay["sequence"], receipt["sequence"]);
    }
}
