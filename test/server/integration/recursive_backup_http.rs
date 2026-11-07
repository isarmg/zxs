//! Authenticated HTTP uses the production writer and the existing backup reader.
use http_routing::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use json_codec::{Value, json};
use service_stack::ServiceExt;
use std::{path::Path, sync::Arc, time::Duration};
use storage_engine::{
    CommitRequest, Config, Engine, MembershipExitAction, OperationId, RepositoryName,
};
use storage_server::{ServerConfig, auth::AuthStore, engine_backend::EngineBackend};

fn operation() -> OperationId {
    OperationId::generate().unwrap()
}

fn router(engine: Engine, auth: AuthStore, system: &Path) -> Router {
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend.protect_auth(auth).unwrap();
    storage_server::router(backend, auth, ServerConfig::new(system)).unwrap()
}

async fn request(
    app: &Router,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(
            builder
                .body(Body::from(
                    json_codec::to_vec(&body.unwrap_or_else(|| json!({}))).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, json_codec::from_slice(&bytes).unwrap())
}

async fn success(
    app: &Router,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let (status, value) = request(app, Some(token), method, path, body).await;
    assert_eq!(status, StatusCode::OK, "{method} {path}: {value}");
    value
}

async fn reopen(system: &Path) -> Engine {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match Engine::open(system) {
            Ok(engine) => return engine,
            Err(error) if error.code() == storage_engine::ErrorCode::WriterAlreadyActive => {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                assert!(
                    !remaining.is_zero(),
                    "sampler retained writer lock: {error}"
                );
                tokio::time::sleep(remaining.min(Duration::from_millis(10))).await;
            }
            Err(error) => panic!("cold reopen failed: {error}"),
        }
    }
}

#[tokio::test]
async fn recursive_http_writes_each_tier_reads_its_tree_and_retries_after_cold_open() {
    let temp = tempfile::Builder::new()
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .tempdir()
        .unwrap();
    let system = temp.path().join("system");
    let mut config =
        Config::development((0..4).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    config.disks[3].level = 1;
    config.disks[3].upstream_slot_id = None;
    config.disks[3].metadata_member = false;
    let mut engine = Engine::initialize(&system, config.clone()).unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("Files").unwrap(),
            "P01",
            1,
            "admin",
            operation(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "ancestor/docs/report",
            b"recursive HTTP body",
        )
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "recursive HTTP fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let entry = engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("ancestor/docs/report")
        .unwrap()
        .clone();
    let version = engine.state().versions[&entry.version_id].clone();
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
    let auth = AuthStore::initialize(&system, "admin", "recursive-http-test-password").unwrap();
    let app = router(engine, auth, &system);
    let endpoint = format!(
        "/api/repositories/{}/recursive-backup-configuration",
        repository.repository_id
    );
    assert_eq!(
        request(&app, None, "GET", &endpoint, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, login) = request(
        &app,
        None,
        "POST",
        "/api/auth/login",
        Some(json!({"username":"admin", "password":"recursive-http-test-password"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let token = login["token"].as_str().unwrap();
    let empty_backup = success(
        &app,
        token,
        "GET",
        "/api/file-manager/disks/B01/entries",
        None,
    )
    .await;
    assert_eq!(empty_backup["entries"], json!([]));
    let selected = success(
        &app,
        token,
        "GET",
        &format!("{endpoint}?path=ancestor/docs"),
        None,
    )
    .await;
    let body = json!({
        "path":"ancestor/docs", "target_copies":3,
        "expected_generation":selected["configuration"]["generation"],
        "expected_scope_id":selected["configuration"]["scope"]["id"],
        "capture":selected["capture"], "operation_id":operation(),
    });
    assert_eq!(
        request(&app, None, "PUT", &endpoint, Some(body.clone()))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let receipt = success(&app, token, "PUT", &endpoint, Some(body.clone())).await;
    assert_eq!(receipt["operation_kind"], "backup.configure-recursive");
    for (index, slot, parent) in [
        (1, "B01", "disk-0/Files/ancestor/docs"),
        (2, "C01", "disk-1/disk-0/Files/ancestor/docs"),
    ] {
        let tree = success(
            &app,
            token,
            "GET",
            &format!("/api/file-manager/disks/{slot}/entries?path={parent}"),
            None,
        )
        .await;
        let row = tree["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["file_id"] == entry.file_id)
            .unwrap();
        assert_eq!(row["path"], format!("{parent}/report"));
        assert_eq!(row["version_id"], entry.version_id);
        assert_eq!(tree["backup"], true);
        assert_eq!(tree["writable"], false);
        assert_eq!(row["read_only"], true);
        let target = config.disks[index]
            .root
            .join("histories")
            .join(&version.history_tree_id)
            .join("file-containers")
            .join(&entry.file_id)
            .join("namespace")
            .join(parent)
            .join("report")
            .join("versions")
            .join(&version.storage_name);
        assert_eq!(std::fs::read(target).unwrap(), b"recursive HTTP body");
    }
    assert_eq!(
        success(&app, token, "PUT", &endpoint, Some(body.clone())).await,
        receipt
    );
    assert_eq!(
        request(
            &app,
            Some(token),
            "GET",
            &format!("{endpoint}?unexpected=1"),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let mut missing = body.clone();
    missing.as_object_mut().unwrap().remove("capture");
    assert_eq!(
        request(&app, Some(token), "PUT", &endpoint, Some(missing))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let fresh = success(
        &app,
        token,
        "GET",
        &format!("{endpoint}?path=ancestor/docs"),
        None,
    )
    .await;
    let mut forged = body.clone();
    forged["operation_id"] = json!(operation());
    forged["expected_generation"] = fresh["configuration"]["generation"].clone();
    forged["capture"] = fresh["capture"].clone();
    forged["capture"]["disks"]["B01"]["name"] = json!("not-the-direct-source");
    let (status, rejected) = request(&app, Some(token), "PUT", &endpoint, Some(forged)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    let token = token.to_owned();
    drop(app);
    let engine = reopen(&system).await;
    let app = router(engine, AuthStore::open(&system).unwrap(), &system);
    assert_eq!(
        success(&app, &token, "PUT", &endpoint, Some(body)).await,
        receipt
    );
    let tree = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/C01/entries?path=disk-1/disk-0/Files/ancestor/docs",
        None,
    )
    .await;
    assert!(
        tree["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["file_id"] == entry.file_id)
    );
}
