//! The mode control uses the existing authenticated command, not an implicit save.
use super::*;

async fn automatic_save(app: &Router, token: &str, slot: &str, path: &str, bytes: &[u8]) -> Value {
    let workspace = success(
        app,
        token,
        "POST",
        &format!("/api/file-manager/disks/{slot}/write-sessions",),
        Some(json!({"path":path})),
    )
    .await;
    let id = workspace["workspace_id"].as_str().unwrap();
    upload(
        app,
        token,
        id,
        &new_id("entry").unwrap(),
        workspace["relative_path"].as_str().unwrap(),
        bytes,
    )
    .await;
    let mut sealed = success(
        app,
        token,
        "POST",
        &format!("/api/workspaces/{id}/seal"),
        Some(json!({})),
    )
    .await;
    sealed["message"] = "explicitly enabled automatic file save".into();
    sealed["request_digest"] = request_digest(
        "POST",
        "/api/workspaces/{workspace}/commit",
        json!({"workspace":id}),
        json!({}),
        &sealed,
    )
    .unwrap()
    .into();
    success(
        app,
        token,
        "POST",
        &format!("/api/workspaces/{id}/commit"),
        Some(sealed),
    )
    .await
}

async fn change_mode(
    app: &Router,
    token: &str,
    target: &DraftLocation,
    observed: &Value,
    mode: &str,
) -> Value {
    let request = json!({
        "operation_id": OperationId::generate().unwrap(),
        "command": {
            "action":"set_mode", "target":target,
            "expected":observed["working"]["revision"], "mode":mode,
        },
    });
    let route = format!("/api/file-manager/files/{}/working", target.file);
    let receipt = success(app, token, "POST", &route, Some(request.clone())).await;
    assert_eq!(receipt["operation_kind"], "file.working.mode");
    assert_eq!(receipt["result"]["request"], request["command"]);
    assert_eq!(
        success(app, token, "POST", &route, Some(request)).await,
        receipt
    );
    receipt
}

#[tokio::test]
async fn manual_default_explicit_automatic_saves_and_switch_back_keep_history_and_draft() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("system");
    let original = b"default manual first save";
    let (engine, target, slot) = saved_unborn(&system, &root.path().join("primary"), original);
    let password = authentication::random_token().unwrap();
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend
        .protect_auth(AuthStore::initialize(&system, "admin", &password).unwrap())
        .unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::new(&system)).unwrap();
    let token = login(&app, "admin", &password).await;
    let route = format!("/api/file-manager/files/{}/working", target.file);
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("repository_id", &target.repository.to_string())
        .append_pair("path", target.path.as_str())
        .append_pair("entry_id", &target.entry.to_string())
        .finish();
    let read = format!("{route}?{query}");
    let unborn = success(
        &app,
        &token,
        "GET",
        &format!(
            "/api/file-manager/files/{}/unborn-working?{query}",
            target.file,
        ),
        None,
    )
    .await;
    assert_eq!(unborn["mode"], "manual");
    success(
        &app,
        &token,
        "POST",
        &format!("/api/file-manager/files/{}/first-commit", target.file,),
        Some(json!({
            "operation_id":OperationId::generate().unwrap(), "target":target,
            "expected":unborn["expected"], "message":"first explicit Manual commit",
        })),
    )
    .await;
    let manual = success(&app, &token, "GET", &read, None).await;
    assert_eq!(manual["working"]["state"], "manual_clean");
    let history_route = format!("/api/file-manager/files/{}/history", target.file);
    let history = success(&app, &token, "GET", &history_route, None).await;
    assert_eq!(history["nodes"].as_array().unwrap().len(), 1);
    change_mode(&app, &token, &target, &manual, "automatic").await;
    let automatic = success(&app, &token, "GET", &read, None).await;
    assert_eq!(automatic["working"]["state"], "automatic");
    assert_eq!(automatic["base"], manual["base"]);
    assert_eq!(
        success(&app, &token, "GET", &history_route, None).await,
        history
    );

    let first = b"automatic save one";
    let second = b"automatic save two";
    let mut retained = vec![(
        manual["base"]["version_id"].as_str().unwrap().to_owned(),
        original.as_slice(),
    )];
    for (index, bytes) in [first.as_slice(), second.as_slice()]
        .into_iter()
        .enumerate()
    {
        automatic_save(&app, &token, &slot, target.path.as_str(), bytes).await;
        let current = success(&app, &token, "GET", &read, None).await;
        assert_eq!(current["working"], automatic["working"]);
        assert_eq!(current["base"]["branch_id"], automatic["base"]["branch_id"]);
        let version = current["base"]["version_id"].as_str().unwrap().to_owned();
        assert!(retained.iter().all(|(old, _)| old != &version));
        assert_eq!(version_bytes(&app, &token, &version).await, bytes);
        retained.push((version, bytes));
        assert_eq!(
            success(&app, &token, "GET", &history_route, None).await["nodes"]
                .as_array()
                .unwrap()
                .len(),
            index + 2
        );
    }
    let automatic = success(&app, &token, "GET", &read, None).await;
    change_mode(&app, &token, &target, &automatic, "manual").await;
    let draft = b"manual save does not publish a fourth historical node";
    let dirty = save(&app, &token, &target, &query, draft).await;
    assert_eq!(dirty["base"], automatic["base"]);
    assert_eq!(
        success(&app, &token, "GET", &history_route, None).await["nodes"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let blocked = send(
        &app,
        Some(&token),
        "POST",
        &route,
        Some(json!({
            "operation_id":OperationId::generate().unwrap(), "command":{
                "action":"set_mode", "target":target, "expected":dirty["working"]["revision"],
                "mode":"automatic",
            },
        })),
    )
    .await;
    assert_eq!(blocked.0, StatusCode::CONFLICT);
    assert_eq!(blocked.1["error"]["code"], "FILE_BRANCH_CONFLICT");
    assert_eq!(success(&app, &token, "GET", &read, None).await, dirty);

    let listing = format!("/api/file-manager/disks/{slot}/entries?path=");
    let before = success(&app, &token, "GET", &listing, None).await;
    let relocation = format!("/api/file-manager/disks/{slot}/file-relocation");
    let rename_request = |head: &Value| {
        json!({
            "operation_id": OperationId::generate().unwrap(),
            "relocation": {
                "repository_id": target.repository,
                "expected_head": head,
                "entry_id": target.entry,
                "file_id": target.file,
                "source": target.path,
                "destination": "renamed-after-commit.txt",
            }
        })
    };
    let (status, rejected) = send(
        &app,
        Some(&token),
        "POST",
        &relocation,
        Some(rename_request(&before["context"]["base_head"])),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    assert_eq!(rejected["error"]["code"], "FILE_BRANCH_CONFLICT");
    let after = success(&app, &token, "GET", &listing, None).await;
    assert_eq!(after["context"], before["context"]);
    assert_eq!(after["entries"], before["entries"]);
    assert_eq!(success(&app, &token, "GET", &read, None).await, dirty);

    let clean = commit(&app, &token, &target, &query, &dirty).await;
    assert_eq!(
        version_bytes(&app, &token, clean["base"]["version_id"].as_str().unwrap()).await,
        draft
    );
    assert_eq!(
        success(&app, &token, "GET", &history_route, None).await["nodes"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    for (version, bytes) in retained {
        assert_eq!(version_bytes(&app, &token, &version).await, bytes);
    }
    let committed = success(&app, &token, "GET", &listing, None).await;
    success(
        &app,
        &token,
        "POST",
        &relocation,
        Some(rename_request(&committed["context"]["base_head"])),
    )
    .await;
    let renamed = success(&app, &token, "GET", &listing, None).await;
    let entries = renamed["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "renamed-after-commit.txt");
    assert_eq!(entries[0]["file_id"], target.file.to_string());
    assert_eq!(entries[0]["entry_id"], target.entry.to_string());
    assert_eq!(entries[0]["version_id"], clean["base"]["version_id"]);
    assert_eq!(
        version_bytes(&app, &token, entries[0]["version_id"].as_str().unwrap()).await,
        draft
    );
}
