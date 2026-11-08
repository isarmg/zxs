//! Production Router oneshot and temporary disks; no TCP or browser assertions.
use super::*;

fn mixed_fixture() -> Fixture {
    let mut fixture = Fixture::new(true);
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repository.main_branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "mixed/report", b"mixed HTTP body")
        .unwrap();
    fixture
        .engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "mixed HTTP fixture".into(),
            request_digest: None,
        })
        .unwrap();
    fixture.namespace(NamespaceOperationKind::Mkdir, "mixed/empty", None);
    fixture
}

fn tree_endpoint(repository: &Repository) -> String {
    format!(
        "/api/repositories/{}/recursive-backup-tree-configuration",
        repository.repository_id
    )
}

async fn mixed_body(app: &Router, token: &str, endpoint: &str, path: &str) -> Value {
    let selected = success(app, token, "GET", &format!("{endpoint}?path={path}"), None).await;
    assert_eq!(selected["configuration"]["file_count"], "1");
    json!({"path":path,"target_copies":3,
        "expected_generation":selected["configuration"]["generation"],
        "expected_scope_id":selected["configuration"]["scope"]["id"],
        "capture":selected["capture"],"operation_id":operation()})
}

#[tokio::test]
async fn mixed_http_publishes_files_and_empty_directories_together_and_cold_retries() {
    let fixture = mixed_fixture();
    let namespace = fixture
        .engine
        .namespace_tree(&fixture.repository.main_branch_id)
        .unwrap();
    let file = namespace.get("mixed/report").unwrap().clone();
    let empty_id = namespace.get("mixed/empty").unwrap().entry_id.clone();
    let version = fixture.engine.state().versions[&file.version_id].clone();
    let system = fixture.temp.path().join("system");
    let old_endpoint = endpoint(&fixture.repository);
    let endpoint = tree_endpoint(&fixture.repository);
    let (app, token) = login_app(fixture.engine, &system).await;
    let body = mixed_body(&app, &token, &old_endpoint, "mixed").await;
    let receipt = success(&app, &token, "PUT", &endpoint, Some(body.clone())).await;
    assert_eq!(receipt["operation_kind"], "backup.configure-recursive-tree");
    for (index, slot, prefix) in [
        (1, "B01", "disk-0/Files/mixed"),
        (2, "C01", "disk-1/disk-0/Files/mixed"),
    ] {
        let listing = success(&app, &token, "GET", &listing_route(slot, prefix), None).await;
        assert_eq!(listing["backup"], true);
        assert_eq!(listing["writable"], false);
        assert_eq!(listing["total_entries"], "2");
        let rows = listing["entries"].as_array().unwrap();
        let directory = rows.iter().find(|row| row["name"] == "empty").unwrap();
        assert_eq!(directory["kind"], "directory");
        assert_eq!(directory["entry_id"], empty_id);
        assert_eq!(directory["path"], format!("{prefix}/empty"));
        assert_eq!(directory["read_only"], true);
        let report = rows.iter().find(|row| row["name"] == "report").unwrap();
        assert_eq!(report["file_id"], file.file_id);
        assert_eq!(report["version_id"], file.version_id);
        assert_eq!(report["read_only"], true);
        let history = fixture.config.disks[index]
            .root
            .join("histories")
            .join(&version.history_tree_id);
        assert_eq!(
            std::fs::read(
                history
                    .join("file-containers")
                    .join(&file.file_id)
                    .join("namespace")
                    .join(prefix)
                    .join("report/versions")
                    .join(&version.storage_name)
            )
            .unwrap(),
            b"mixed HTTP body"
        );
        assert!(
            history
                .join("directory-backups")
                .join(receipt["operation_id"].as_str().unwrap())
                .join("namespace")
                .join(prefix)
                .join("empty")
                .is_dir()
        );
    }
    assert_eq!(
        request(&app, Some(&token), "PUT", &old_endpoint, Some(body.clone()))
            .await
            .0,
        StatusCode::CONFLICT
    );
    drop(app);
    let engine = reopen(&system).await;
    assert_eq!(engine.state().versions.len(), 1);
    for slot in ["B01", "C01"] {
        let rows = engine.published_backup_directories(slot).unwrap();
        assert!(rows.iter().any(|row| row.directory_id() == empty_id
            && row.operation_id().to_string() == receipt["operation_id"].as_str().unwrap()));
    }
    let app = router(engine, AuthStore::open(&system).unwrap(), &system);
    assert_eq!(
        success(&app, &token, "PUT", &endpoint, Some(body)).await,
        receipt
    );
    assert_eq!(
        success(
            &app,
            &token,
            "GET",
            &listing_route("C01", "disk-1/disk-0/Files/mixed"),
            None
        )
        .await["total_entries"],
        "2"
    );
}

#[tokio::test]
async fn mixed_http_rejects_file_scope_bad_shapes_forgery_and_revoked_session_without_publication()
{
    let fixture = mixed_fixture();
    let policies = fixture.engine.state().protection_policies.clone();
    let paths = fixture.engine.state().file_backup_paths.clone();
    let system = fixture.temp.path().join("system");
    let old_endpoint = endpoint(&fixture.repository);
    let endpoint = tree_endpoint(&fixture.repository);
    let (app, token) = login_app(fixture.engine, &system).await;
    let body = mixed_body(&app, &token, &old_endpoint, "mixed").await;
    let file_body = mixed_body(&app, &token, &old_endpoint, "mixed/report").await;
    let (status, error) = request(
        &app,
        Some(&token),
        "PUT",
        &endpoint,
        Some(file_body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{error}");
    let mut rejected_ids = vec![
        file_body["operation_id"].clone(),
        body["operation_id"].clone(),
    ];
    for field in ["capture", "expected_scope_id"] {
        let mut missing = body.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert_eq!(
            request(&app, Some(&token), "PUT", &endpoint, Some(missing))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut extra = body.clone();
    extra["file_body"] = json!("must not accept file contents");
    assert_eq!(
        request(&app, Some(&token), "PUT", &endpoint, Some(extra))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let mut forged = body.clone();
    forged["operation_id"] = json!(operation());
    rejected_ids.push(forged["operation_id"].clone());
    forged["capture"]["disks"]["B01"]["name"] = json!("forged-source");
    assert_eq!(
        request(&app, Some(&token), "PUT", &endpoint, Some(forged))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(&app, None, "PUT", &endpoint, Some(body.clone()))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    success(&app, &token, "POST", "/api/auth/logout", None).await;
    assert_eq!(
        request(&app, Some(&token), "PUT", &endpoint, Some(body))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    drop(app);
    let engine = reopen(&system).await;
    assert_eq!(engine.state().protection_policies, policies);
    assert_eq!(engine.state().file_backup_paths, paths);
    assert!(
        engine
            .published_backup_directories("B01")
            .unwrap()
            .is_empty()
    );
    for id in rejected_ids {
        assert!(
            !engine
                .state()
                .operations
                .keys()
                .any(|key| key.to_string() == id.as_str().unwrap())
        );
    }
}

#[tokio::test]
async fn legacy_recursive_http_remains_files_only_and_cannot_retry_as_tree_kind() {
    let fixture = mixed_fixture();
    let system = fixture.temp.path().join("system");
    let old_endpoint = endpoint(&fixture.repository);
    let endpoint = tree_endpoint(&fixture.repository);
    let (app, token) = login_app(fixture.engine, &system).await;
    let body = mixed_body(&app, &token, &old_endpoint, "mixed").await;
    let receipt = success(&app, &token, "PUT", &old_endpoint, Some(body.clone())).await;
    assert_eq!(receipt["operation_kind"], "backup.configure-recursive");
    let listing = success(
        &app,
        &token,
        "GET",
        &listing_route("B01", "disk-0/Files/mixed"),
        None,
    )
    .await;
    assert_eq!(listing["total_entries"], "1");
    assert_eq!(listing["entries"][0]["name"], "report");
    assert_eq!(
        request(&app, Some(&token), "PUT", &endpoint, Some(body.clone()))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        success(&app, &token, "PUT", &old_endpoint, Some(body)).await,
        receipt
    );
    drop(app);
    let engine = reopen(&system).await;
    assert!(
        engine
            .published_backup_directories("B01")
            .unwrap()
            .is_empty()
    );
}
