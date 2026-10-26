use super::*;

#[test]
fn unavailable_and_invalid_samples_are_not_fabricated_zero_usage() {
    assert_eq!(normalized_cpu_usage(f32::NAN), None);
    assert_eq!(normalized_cpu_usage(f32::INFINITY), None);
    assert_eq!(normalized_cpu_usage(150.0), Some(100.0));
    assert!(memory_status(0, 0).total_bytes.is_none());
    assert_eq!(
        memory_status(u64::MAX, u64::MAX - 123)
            .used_bytes
            .as_deref(),
        Some("123")
    );
}

#[test]
fn real_host_sampler_starts_with_a_baseline_and_reads_memory() {
    let mut sampler = SystemSampler::new();
    let (cpu, memory) = sampler.sample();
    assert!(cpu.usage_percent.is_none());
    assert!(cpu.logical_count > 0);
    assert!(memory.total_bytes.is_some());
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    assert!(
        sampler
            .sample()
            .0
            .usage_percent
            .is_some_and(|usage| (0.0..=100.0).contains(&usage))
    );
}

struct MonitorBackend;
impl Backend for MonitorBackend {
    fn operation(
        &self,
        _: &crate::auth::Identity,
        _: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        unreachable!("this test backend does not query operations")
    }
    fn commit_workspace(
        &self,
        _: &crate::auth::Identity,
        _: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        unreachable!("this test backend does not commit workspaces")
    }

    fn snapshot_at(
        &self,
        _: &crate::auth::Identity,
        _: &str,
        _: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        unreachable!("this test backend does not serve snapshots")
    }

    fn authorize_current(
        &self,
        _identity: &crate::auth::Identity,
        _branch: &str,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn read_directory(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadDirectory<'_>,
    ) -> Result<crate::DirectorySnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn authorize_workspace_selection(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _path: &name_policy::PortablePath,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn create_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _command: crate::CreateWorkspace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn abort_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn published_upload_status(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _item: &str,
    ) -> Result<Option<crate::PublishedUploadStatus>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn seal_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<storage_contract::SealedWorkspace, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn commit_workspace_public(
        &self,
        _identity: &crate::auth::Identity,
        _command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn version_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _version: &str,
    ) -> Result<crate::VersionSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn authorize_version(
        &self,
        _identity: &crate::auth::Identity,
        _version: &str,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn repository_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn authorize_repository(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn capabilities(&self) -> Result<crate::Capabilities, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn create_write_session(
        &self,
        _identity: &crate::auth::Identity,
        _branch: &str,
    ) -> Result<crate::WorkspaceWriteSession, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn stage_namespace(
        &self,
        _identity: &crate::auth::Identity,
        _command: crate::StageNamespace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn authorize_upload(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _logical_size: Option<u64>,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn lookup_current(
        &self,
        _identity: &crate::auth::Identity,
        _branch: &str,
        _path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn lookup_at(
        &self,
        _: &crate::auth::Identity,
        _: &str,
        _: &str,
        _: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        unreachable!("this test backend does not serve snapshot lookups")
    }

    fn call(
        &self,
        _: crate::BackendAction,
        _identity: &Identity,
        _input: Value,
    ) -> Result<Value, ApiError> {
        panic!("private monitoring must not forge a user identity")
    }
    fn observe_disks(&self, _: &crate::InternalDiskObservation) -> Result<Value, ApiError> {
        Ok(json!({"disks":[]}))
    }
    fn upload(
        &self,
        _: &Identity,
        _: &str,
        _: &str,
        _: &name_policy::PortablePath,
        _: &crate::UploadSource,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        Err(ApiError::unsupported())
    }
    fn upload_generation(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _item: &str,
        _path: &name_policy::PortablePath,
        _source: &crate::UploadSource,
        _generation: u64,
    ) -> Result<storage_contract::StagedFile, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn content(&self, _: &Identity, _: &str) -> Result<crate::Content, ApiError> {
        Err(ApiError::unsupported())
    }
}

#[test]
fn unavailable_cache_falls_back_to_bounded_memory_without_losing_live_metrics() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("cache");
    std::fs::write(&directory, b"unwritable cache path").unwrap();
    let service = Telemetry::start(Arc::new(MonitorBackend), &directory).unwrap();
    let live = service.current().unwrap();
    assert!(live.cpu.logical_count > 0);
    assert!(live.memory.total_bytes.is_some());
    let history = service.history.lock().unwrap();
    assert!(!history.is_persistent());
    let now = date_time::OffsetDateTime::now_utc().unix_timestamp();
    assert!(!history.samples(now - 60, now, 5).is_empty());
    assert_eq!(std::fs::read(directory).unwrap(), b"unwritable cache path");
}

#[cfg(target_os = "linux")]
#[test]
fn read_only_sysfs_cache_falls_back_to_live_memory_monitoring() {
    let service = Telemetry::start(
        Arc::new(MonitorBackend),
        Path::new("/sys/zxs-telemetry-cache-unit-test"),
    )
    .unwrap();
    assert!(!service.history.lock().unwrap().is_persistent());
    assert!(service.current().unwrap().memory.total_bytes.is_some());
}

#[test]
fn supported_ranges_bound_chart_points_and_reject_other_strings() {
    for range in ["1min", "30min", "1h", "24h", "1month"] {
        let (seconds, interval) = json_codec::from_value::<HistoryRange>(json!(range))
            .unwrap()
            .parameters();
        assert!(seconds / interval <= 360);
    }
    assert!(json_codec::from_value::<HistoryRange>(json!("1year")).is_err());
    assert!(json_codec::from_value::<HistoryRange>(json!("../1h")).is_err());
}

#[test]
fn monitoring_rechecks_the_current_session_after_logout() {
    let root = crate::tests::private_tempdir();
    let mut auth =
        crate::auth::AuthStore::initialize(root.path(), "admin", "monitor-test-password").unwrap();
    let token = auth.login("admin", "monitor-test-password").unwrap().token;
    let captured = auth.authenticate(&token).unwrap();
    let auth = Mutex::new(auth);
    assert!(require_current_session(&auth, &captured).is_ok());
    auth.lock().unwrap().logout(&token).unwrap();
    assert_eq!(
        require_current_session(&auth, &captured).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[tokio::test]
async fn telemetry_admission_survives_cancellation_without_consuming_management_capacity() {
    use http_routing::{
        Router,
        body::Body,
        http::{Request, StatusCode},
    };
    use service_stack::ServiceExt;
    async fn request(app: &Router, path: &str, token: &str) -> StatusCode {
        app.clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }
    let root = crate::tests::private_tempdir();
    let config = crate::ServerConfig::loopback(root.path());
    config.prepare_spool().unwrap();
    let backend = Arc::new(MonitorBackend);
    let service = Telemetry::start(backend.clone(), &root.path().join("cache")).unwrap();
    let mut auth =
        crate::auth::AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let token = auth.login("admin", "a-long-test-password").unwrap().token;
    let spool_budget = crate::http_spool::HttpSpool::open_blocking(&config);
    let app = crate::router_shared(
        backend,
        Arc::new(Mutex::new(auth)),
        config.admit().unwrap(),
        service.clone(),
        spool_budget,
    )
    .unwrap();
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let mut releases = Vec::new();
    let mut jobs = Vec::new();
    for _ in 0..2 {
        let (release, wait) = std::sync::mpsc::channel();
        releases.push(release);
        let started = started.clone();
        jobs.push(tokio::spawn(telemetry_job(
            service.request_slots.clone(),
            move || {
                started.add_permits(1);
                wait.recv_timeout(Duration::from_secs(10))
                    .expect("invariant: test releases blocked telemetry worker");
                Ok(())
            },
        )));
    }
    tokio::time::timeout(Duration::from_secs(3), started.acquire_many(2))
        .await
        .unwrap()
        .unwrap()
        .forget();
    for path in ["/api/system/status", "/api/system/history?range=1min"] {
        assert_eq!(
            request(&app, path, &token).await,
            StatusCode::TOO_MANY_REQUESTS
        );
    }
    let canceled = jobs.pop().unwrap();
    canceled.abort();
    assert!(canceled.await.unwrap_err().is_cancelled());
    assert_eq!(service.request_slots.available_permits(), 0);
    assert_eq!(
        request(&app, "/api/system/status", &token).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    for path in ["/healthz", "/api/management/status"] {
        assert_eq!(request(&app, path, &token).await, StatusCode::OK);
    }
    for release in releases {
        release.send(()).unwrap();
    }
    jobs.pop().unwrap().await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while service.request_slots.available_permits() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for path in ["/api/system/status", "/api/system/history?range=1min"] {
        assert_eq!(request(&app, path, &token).await, StatusCode::OK);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn rejected_symlink_cache_preserves_target_and_management_across_restarts() {
    use http_routing::{body::Body, http::Request};
    use service_stack::ServiceExt;
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = crate::tests::private_tempdir();
    let target = root.path().join("external-cache-target");
    std::fs::create_dir(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750)).unwrap();
    let sentinel = target.join("sentinel");
    std::fs::write(&sentinel, b"preserve external contents").unwrap();
    std::fs::set_permissions(&sentinel, std::fs::Permissions::from_mode(0o640)).unwrap();
    let cache = root.path().join("telemetry-cache");
    symlink(&target, &cache).unwrap();
    let web = root.path().join("web");
    std::fs::create_dir(&web).unwrap();
    std::fs::write(web.join("index.html"), b"management page").unwrap();
    let auth_root = root.path().join("auth");
    drop(crate::auth::AuthStore::initialize(&auth_root, "admin", "monitor-test-password").unwrap());

    for _ in 0..3 {
        // Exercise fallback directly, including a history write, before
        // constructing the complete router with the same rejected cache.
        let service = Telemetry::start(Arc::new(MonitorBackend), &cache).unwrap();
        assert!(!service.history.lock().unwrap().is_persistent());
        service.refresh().unwrap();
        assert!(service.current().unwrap().memory.total_bytes.is_some());
        drop(service);

        let mut config = crate::ServerConfig::loopback(root.path());
        config.web_dir = Some(web.clone());
        let app = crate::router(
            Arc::new(MonitorBackend),
            crate::auth::AuthStore::open(&auth_root).unwrap(),
            config,
        )
        .expect("rejected optional cache must not block the management router");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"username":"admin","password":"monitor-test-password"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let login: Value = json_codec::from_slice(
            &http_routing::body::to_bytes(response.into_body(), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        let token = login["token"].as_str().unwrap();
        for path in [
            "/",
            "/healthz",
            "/api/management/status",
            "/api/auth/me",
            "/api/system/status",
            "/api/system/history?range=1min",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 200, "{path}");
            // Complete response lifetimes before reopening the same directories.
            http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
        }
        drop(app);
        assert_eq!(std::fs::read_link(&cache).unwrap(), target);
        assert_eq!(
            std::fs::read(&sentinel).unwrap(),
            b"preserve external contents"
        );
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o750
        );
        assert_eq!(
            std::fs::metadata(&sentinel).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 1);
    }
}

#[test]
fn history_range_rejects_enum_objects() {
    assert!(json_codec::from_value::<HistoryRange>(json!({"1min": null})).is_err());
}
