//! Real authenticated HTTP and temporary-disk projections, not browser or power-loss QA.
use super::*;
use std::collections::BTreeMap;
use storage_engine::{
    BackupConfigurationRequest, NamespaceOperation, NamespaceOperationKind, Repository,
};

struct Fixture {
    temp: tempfile::TempDir,
    config: Config,
    engine: Engine,
    repository: Repository,
}
impl Fixture {
    fn new(current: bool) -> Self {
        let temp = tempfile::Builder::new()
            .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .tempdir()
            .unwrap();
        let mut config =
            Config::development((0..4).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        config.disks[3].level = 1;
        config.disks[3].upstream_slot_id = None;
        config.disks[3].metadata_member = false;
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("Files").unwrap(),
                "P01",
                1,
                "admin",
                operation(),
            )
            .unwrap();
        if current {
            engine
                .begin_disk_exit(
                    &config.disks[3].disk_id,
                    MembershipExitAction::Deregister,
                    "admin",
                    operation(),
                )
                .unwrap();
            let archive = engine.build_managed_membership_control_archive().unwrap();
            engine
                .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
                .unwrap();
            engine.resume_membership_joint_decision().unwrap();
            engine.activate_pending_disk_exit().unwrap();
        }
        Self {
            temp,
            config,
            engine,
            repository,
        }
    }
    fn namespace(&mut self, kind: NamespaceOperationKind, path: &str, destination: Option<&str>) {
        self.engine
            .file_namespace(
                &self.repository.repository_id,
                NamespaceOperation::parse(kind, path, destination).unwrap(),
                "admin",
                operation(),
            )
            .unwrap();
    }
    fn configure(&mut self, path: &str) {
        let before = self
            .engine
            .backup_configuration(&self.repository.repository_id, path)
            .unwrap();
        let capture = self.engine.recursive_backup_capture().unwrap();
        self.engine
            .configure_recursive_backups(storage_engine::RecursiveBackupConfigurationRequest {
                repository_id: &self.repository.repository_id,
                path,
                expected_scope_id: &before.scope.id,
                target_copies: 3,
                expected_generation: before.generation,
                capture: &capture,
                principal: "admin",
                operation_id: operation(),
            })
            .unwrap();
    }
}

async fn login_app(engine: Engine, system: &Path) -> (Router, String) {
    let auth = AuthStore::initialize(system, "admin", "empty-recursive-http-password").unwrap();
    let app = router(engine, auth, system);
    let (status, login) = request(
        &app,
        None,
        "POST",
        "/api/auth/login",
        Some(json!({"username":"admin","password":"empty-recursive-http-password"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    (app, login["token"].as_str().unwrap().to_owned())
}
fn endpoint(repository: &Repository) -> String {
    format!(
        "/api/repositories/{}/recursive-backup-configuration",
        repository.repository_id
    )
}
async fn capture_body(app: &Router, token: &str, endpoint: &str, path: &str, copies: u32) -> Value {
    let selected = success(app, token, "GET", &format!("{endpoint}?path={path}"), None).await;
    assert_eq!(selected["configuration"]["file_count"], "0");
    json!({"path":path,"target_copies":copies,
        "expected_generation":selected["configuration"]["generation"],
        "expected_scope_id":selected["configuration"]["scope"]["id"],
        "capture":selected["capture"],"operation_id":operation()})
}
fn listing_route(slot: &str, path: &str) -> String {
    format!("/api/file-manager/disks/{slot}/entries?path={path}")
}
fn readonly(tree: &Value) {
    assert_eq!(tree["backup"], true);
    assert_eq!(tree["writable"], false);
    assert!(tree["context"].is_null());
    assert_eq!(tree["directory_file_count"], "0");
    for row in tree["entries"].as_array().unwrap() {
        assert_eq!(row["kind"], "directory");
        assert_eq!(row["read_only"], true);
        assert_eq!(row["backup"], true);
        assert!(row["repository_path"].is_null());
        assert_eq!(row["version_id"], "");
        assert_eq!(row["file_id"], "");
    }
}

#[tokio::test]
async fn empty_scope_http_lists_real_identities_cold_retries_and_policy_replacement() {
    let mut fixture = Fixture::new(true);
    for path in ["empty", "empty/one", "empty/two"] {
        fixture.namespace(NamespaceOperationKind::Mkdir, path, None);
    }
    let ids = ["empty/one", "empty/two"].map(|path| {
        fixture
            .engine
            .namespace_tree(&fixture.repository.main_branch_id)
            .unwrap()
            .get(path)
            .unwrap()
            .entry_id
            .clone()
    });
    let system = fixture.temp.path().join("system");
    let endpoint = endpoint(&fixture.repository);
    let (app, token) = login_app(fixture.engine, &system).await;
    assert_eq!(
        success(&app, &token, "GET", &listing_route("B01", ""), None).await["entries"],
        json!([])
    );
    let body = capture_body(&app, &token, &endpoint, "empty", 3).await;
    assert_eq!(
        request(&app, None, "PUT", &endpoint, Some(body.clone()))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let receipt = success(&app, &token, "PUT", &endpoint, Some(body.clone())).await;
    let mut captured = Vec::new();
    for (index, slot, prefix) in [
        (1, "B01", "disk-0/Files/empty"),
        (2, "C01", "disk-1/disk-0/Files/empty"),
    ] {
        let tree = success(&app, &token, "GET", &listing_route(slot, prefix), None).await;
        readonly(&tree);
        assert_eq!(tree["total_entries"], "2");
        for (row, (name, id)) in tree["entries"]
            .as_array()
            .unwrap()
            .iter()
            .zip([("one", &ids[0]), ("two", &ids[1])])
        {
            assert_eq!(row["entry_id"], *id);
            assert_eq!(row["path"], format!("{prefix}/{name}"));
        }
        captured.push(tree["entries"].clone());
        let physical = fixture.config.disks[index]
            .root
            .join("histories")
            .join(&fixture.repository.history_tree_id)
            .join("directory-backups")
            .join(receipt["operation_id"].as_str().unwrap())
            .join("namespace")
            .join(prefix)
            .join("one");
        assert!(physical.is_dir());
        std::fs::remove_dir(&physical).unwrap();
        let leaf = success(
            &app,
            &token,
            "GET",
            &listing_route(slot, &format!("{prefix}/one")),
            None,
        )
        .await;
        readonly(&leaf);
        assert_eq!(leaf["entries"], json!([]));
        assert!(
            !physical.exists(),
            "HTTP read must not repair an absent projection"
        );
    }
    let page = success(
        &app,
        &token,
        "GET",
        &format!("{}&limit=1", listing_route("B01", "disk-0/Files/empty")),
        None,
    )
    .await;
    let cursor = page["next_cursor"].as_str().unwrap().to_owned();
    assert_eq!(
        success(&app, &token, "PUT", &endpoint, Some(body.clone())).await,
        receipt
    );
    assert_eq!(
        request(
            &app,
            None,
            "GET",
            &listing_route("B01", "disk-0/Files/empty"),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    drop(app);
    let engine = reopen(&system).await;
    assert!(
        engine.state().versions.is_empty(),
        "directory snapshots do not invent file versions"
    );
    let app = router(engine, AuthStore::open(&system).unwrap(), &system);
    assert_eq!(
        success(&app, &token, "PUT", &endpoint, Some(body)).await,
        receipt
    );
    for ((slot, prefix), old) in [
        ("B01", "disk-0/Files/empty"),
        ("C01", "disk-1/disk-0/Files/empty"),
    ]
    .into_iter()
    .zip(captured)
    {
        assert_eq!(
            success(&app, &token, "GET", &listing_route(slot, prefix), None).await["entries"],
            old
        );
    }
    let replace = capture_body(&app, &token, &endpoint, "empty", 1).await;
    success(&app, &token, "PUT", &endpoint, Some(replace)).await;
    let (status, error) = request(
        &app,
        Some(&token),
        "GET",
        &format!(
            "{}&limit=1&cursor={cursor}",
            listing_route("B01", "disk-0/Files/empty")
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["error"]["code"], "LISTING_CHANGED");
    assert_eq!(
        success(&app, &token, "GET", &listing_route("B01", ""), None).await["entries"],
        json!([])
    );
    assert_eq!(
        request(
            &app,
            Some(&token),
            "GET",
            &listing_route("B01", "disk-0/Files/empty"),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn different_real_directories_with_one_frozen_path_conflict_before_paging() {
    let mut fixture = Fixture::new(true);
    fixture.namespace(NamespaceOperationKind::Mkdir, "empty", None);
    fixture.configure("empty");
    fixture.namespace(NamespaceOperationKind::Rename, "empty", Some("moved"));
    fixture.namespace(NamespaceOperationKind::Mkdir, "empty", None);
    fixture.configure("empty");
    let rows = fixture.engine.published_backup_directories("B01").unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].path(), rows[1].path());
    assert_ne!(rows[0].directory_id(), rows[1].directory_id());
    let system = fixture.temp.path().join("system");
    let (app, token) = login_app(fixture.engine, &system).await;
    for query in ["limit=1", "query=missing", "path=unrelated"] {
        let (status, error) = request(
            &app,
            Some(&token),
            "GET",
            &format!("/api/file-manager/disks/B01/entries?{query}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["error"]["code"], "BACKUP_PATH_CONFLICT");
    }
}

#[tokio::test]
async fn published_directory_cannot_be_hidden_by_a_conflicting_file_or_filter() {
    let mut fixture = Fixture::new(true);
    fixture.namespace(NamespaceOperationKind::Mkdir, "empty", None);
    fixture.configure("empty");
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repository.main_branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "other/empty", b"body")
        .unwrap();
    fixture
        .engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "conflict fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let before = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, "other/empty")
        .unwrap();
    fixture
        .engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &fixture.repository.repository_id,
            path: "other/empty",
            target_copies: 2,
            backup_prefixes: &BTreeMap::from([("B01".into(), "disk-0/Files".into())]),
            expected_generation: before.generation,
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    let system = fixture.temp.path().join("system");
    let (app, token) = login_app(fixture.engine, &system).await;
    for path in [
        "/api/file-manager/disks/B01/entries?limit=1",
        "/api/file-manager/disks/B01/entries?query=missing",
        "/api/file-manager/disks/B01/entries?path=unrelated",
    ] {
        let (status, error) = request(&app, Some(&token), "GET", path, None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["error"]["code"], "BACKUP_PATH_CONFLICT");
    }
}

#[tokio::test]
async fn initial_pool_file_backups_remain_readable_without_native_directory_authority() {
    let mut fixture = Fixture::new(false);
    assert!(fixture.engine.current_disk_registry_commitment().is_none());
    assert!(
        fixture
            .engine
            .published_backup_directories("B01")
            .unwrap()
            .is_empty()
    );
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repository.main_branch_id, "admin")
        .unwrap();
    fixture
        .engine
        .stage_bytes(&workspace.workspace_id, "report", b"body")
        .unwrap();
    fixture
        .engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "initial file fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let before = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, "report")
        .unwrap();
    fixture
        .engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &fixture.repository.repository_id,
            path: "report",
            target_copies: 2,
            backup_prefixes: &BTreeMap::from([("B01".into(), "archive".into())]),
            expected_generation: before.generation,
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    let system = fixture.temp.path().join("system");
    let (app, token) = login_app(fixture.engine, &system).await;
    let tree = success(&app, &token, "GET", &listing_route("B01", "archive"), None).await;
    assert_eq!(tree["entries"][0]["name"], "report");
    assert_eq!(tree["entries"][0]["kind"], "file");
    assert_eq!(tree["entries"][0]["backup"], true);
    assert_eq!(tree["writable"], false);
}
