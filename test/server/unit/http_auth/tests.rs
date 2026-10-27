use super::*;

#[test]
fn configured_external_origins_match_canonical_browser_origins() {
    let root = tempfile::tempdir().unwrap();
    for (configured, browser) in [
        ("https://EXAMPLE.COM:443", "https://example.com"),
        ("HTTPS://example.com", "https://example.com"),
        ("https://[0:0:0:0:0:0:0:1]:443", "https://[::1]"),
        ("https://Example.COM:8443", "https://example.com:8443"),
    ] {
        let mut config = ServerConfig::loopback(root.path());
        config.external_origin = Some(configured.into());
        let config = config.admit().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_str(browser).unwrap());
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        assert!(same_origin(&headers, &config, true).is_ok(), "{configured}");
        assert!(
            config.secure_cookie(),
            "{configured} must set Secure cookies"
        );
    }
}

#[test]
fn external_origin_normalization_preserves_scheme_host_and_port_boundaries() {
    let root = tempfile::tempdir().unwrap();
    let mut config = ServerConfig::loopback(root.path());
    config.external_origin = Some("https://EXAMPLE.COM:443".into());
    let config = config.admit().unwrap();
    for origin in [
        "null",
        "http://example.com",
        "https://example.com:8443",
        "https://example.com.evil.test",
        "https://example.com/path",
        "https://example.com?query=1",
        "https://example.com#fragment",
        "https://user@example.com",
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        assert_eq!(
            same_origin(&headers, &config, true).unwrap_err().status,
            403,
            "{origin}"
        );
    }
    let mut config = ServerConfig::loopback(root.path());
    config.external_origin = Some("https://example.com:8443".into());
    let config = config.admit().unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://example.com"),
    );
    assert_eq!(
        same_origin(&headers, &config, true).unwrap_err().status,
        403
    );
}

fn address(last: u8) -> IpAddr {
    IpAddr::from([192, 0, 2, last])
}

#[test]
fn distributed_normalized_username_attempts_slow_without_hard_lockout() {
    let attempts = Mutex::new(HashMap::new());
    for index in 0..MAX_ATTEMPTS {
        let username = if index % 2 == 0 { " Admin " } else { "ADMIN" };
        assert_eq!(
            admit_login(&attempts, address(index as u8), username).unwrap(),
            Duration::ZERO
        );
    }
    for (index, millis) in [
        (10, 250),
        (11, 500),
        (12, 1000),
        (13, 2000),
        (14, 4000),
        (15, 6000),
        (16, 6000),
    ] {
        assert_eq!(
            admit_login(&attempts, address(index), "admin").unwrap(),
            Duration::from_millis(millis)
        );
    }
    assert_eq!(attempts.lock().unwrap()["user:admin"].count, 17);
    // An unrelated guessed username cannot amplify pressure on the administrator.
    assert_eq!(
        admit_login(&attempts, address(100), "reader").unwrap(),
        Duration::ZERO
    );
}

#[test]
fn source_limit_cannot_be_evaded_by_rotating_username_guesses() {
    let attempts = Mutex::new(HashMap::new());
    for index in 0..MAX_ATTEMPTS {
        admit_login(&attempts, address(1), &format!("user{index}")).unwrap();
    }
    assert_eq!(
        admit_login(&attempts, address(1), "victim")
            .unwrap_err()
            .status,
        429
    );
    assert!(!attempts.lock().unwrap().contains_key("user:victim"));
    assert_eq!(
        admit_login(&attempts, address(2), "victim").unwrap(),
        Duration::ZERO
    );
}

fn proxy_config() -> ServerConfig {
    let mut config = ServerConfig::loopback(std::path::Path::new("/tmp/zxs-test"));
    config.external_origin = Some("https://storage.example.org".into());
    config.trusted_proxy_ips = vec!["127.0.0.1".parse().unwrap()];
    config
}

fn request_from(peer: &str, forwarded: Option<&str>) -> Request {
    let mut request = Request::new(http_routing::body::Body::empty());
    request
        .extensions_mut()
        .insert(http_routing::extract::ConnectInfo(
            peer.parse::<SocketAddr>().unwrap(),
        ));
    if let Some(value) = forwarded {
        request
            .headers_mut()
            .insert("x-forwarded-for", value.parse().unwrap());
    }
    request
}

#[test]
fn forwarding_is_ignored_by_default_and_from_untrusted_peers() {
    let default = ServerConfig::loopback(std::path::Path::new("/tmp/zxs-test"));
    assert_eq!(
        client_ip(
            &request_from("127.0.0.1:1234", Some("192.0.2.10")),
            &default
        )
        .unwrap(),
        "127.0.0.1".parse::<IpAddr>().unwrap()
    );
    assert_eq!(
        client_ip(
            &request_from("127.0.0.2:1234", Some("192.0.2.10")),
            &proxy_config()
        )
        .unwrap(),
        "127.0.0.2".parse::<IpAddr>().unwrap()
    );
    assert_eq!(
        client_ip(&request_from("127.0.0.1:1234", None), &default).unwrap(),
        "127.0.0.1".parse::<IpAddr>().unwrap()
    );
}

#[test]
fn trusted_proxy_requires_exact_single_ip_and_transport_metadata() {
    let config = proxy_config();
    for value in [
        None,
        Some(""),
        Some("garbage"),
        Some("192.0.2.1, 192.0.2.2"),
        Some("192.0.2.1:8080"),
        Some(" 192.0.2.1"),
    ] {
        assert!(client_ip(&request_from("127.0.0.1:1234", value), &config).is_err());
    }
    let mut duplicate = request_from("127.0.0.1:1234", Some("192.0.2.1"));
    duplicate
        .headers_mut()
        .append("x-forwarded-for", HeaderValue::from_static("192.0.2.2"));
    assert!(client_ip(&duplicate, &config).is_err());
    let missing_peer = Request::builder()
        .header("x-forwarded-for", "192.0.2.1")
        .body(http_routing::body::Body::empty())
        .unwrap();
    assert!(client_ip(&missing_peer, &config).is_err());
    assert_eq!(
        client_ip(
            &request_from("127.0.0.1:1234", Some("::ffff:192.0.2.1")),
            &config
        )
        .unwrap(),
        address(1)
    );
}

#[test]
fn proxy_clients_have_separate_budgets_but_cannot_rotate_guesses_to_bypass_source_limit() {
    let attempts = Mutex::new(HashMap::new());
    let config = proxy_config();
    let first = client_ip(&request_from("127.0.0.1:1234", Some("192.0.2.1")), &config).unwrap();
    let second = client_ip(&request_from("127.0.0.1:1234", Some("192.0.2.2")), &config).unwrap();
    for _ in 0..MAX_ATTEMPTS {
        admit_login(&attempts, first, "admin").unwrap();
    }
    assert_eq!(
        admit_login(&attempts, first, "reader").unwrap_err().status,
        429
    );
    assert_eq!(
        admit_login(&attempts, second, "admin").unwrap(),
        Duration::from_millis(250)
    );
}

#[test]
fn proxy_configuration_requires_loopback_peer_and_valid_https_origin() {
    let mut config = proxy_config();
    config.clone().admit().unwrap();
    config.external_origin = None;
    assert!(config.clone().admit().is_err());
    config.external_origin = Some("http://storage.example.org".into());
    assert!(config.clone().admit().is_err());
    config.external_origin = Some("https://storage.example.org".into());
    config.trusted_proxy_ips = vec![address(1)];
    assert!(config.clone().admit().is_err());
}

#[test]
fn invalid_usernames_never_allocate_retained_pending_or_counter_keys() {
    let pending = Arc::new(Mutex::new(HashSet::new()));
    let attempts = Mutex::new(HashMap::new());
    for username in [
        "x".repeat(1_000_000),
        "管理员".repeat(100_000),
        "é".to_owned(),
        "a\nb".to_owned(),
        "".to_owned(),
        " admin ".repeat(100),
    ] {
        assert_eq!(login_username_key(&username).unwrap_err().status, 401);
        assert!(PendingLogin::acquire(&pending, &username).is_err());
        assert_eq!(
            admit_login(&attempts, address(1), &username)
                .unwrap_err()
                .status,
            401
        );
        assert!(pending.lock().unwrap().is_empty());
        assert!(attempts.lock().unwrap().is_empty());
    }
    assert_eq!(login_username_key(" Admin ").unwrap(), "user:admin");
    let longest = "x".repeat(authentication::ADMINISTRATOR_USERNAME_MAX_BYTES);
    assert_eq!(
        login_username_key(&longest).unwrap().len(),
        5 + authentication::ADMINISTRATOR_USERNAME_MAX_BYTES
    );
    assert!(login_username_key(&(longest + "x")).is_err());
}

#[test]
fn pending_logins_are_per_canonical_username_bounded_and_release_on_drop() {
    let pending = Arc::new(Mutex::new(HashSet::new()));
    let first = PendingLogin::acquire(&pending, " Admin ").unwrap();
    assert!(PendingLogin::acquire(&pending, "ADMIN").is_err());
    let other = PendingLogin::acquire(&pending, "reader").unwrap();
    drop(first);
    let first = PendingLogin::acquire(&pending, "admin").unwrap();
    let mut guards = vec![first, other];
    for index in 2..MAX_PENDING_LOGINS {
        guards.push(PendingLogin::acquire(&pending, &format!("user{index}")).unwrap());
    }
    assert!(PendingLogin::acquire(&pending, "overflow").is_err());
    assert_eq!(pending.lock().unwrap().len(), MAX_PENDING_LOGINS);
    drop(guards);
    assert!(pending.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancelled_waiter_releases_username_guard_without_blocking_other_guesses() {
    let pending = Arc::new(Mutex::new(HashSet::new()));
    let waiting = pending.clone();
    let (ready, started) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let _guard = PendingLogin::acquire(&waiting, "admin").unwrap();
        ready.send(()).unwrap();
        tokio::time::sleep(Duration::from_secs(6)).await;
    });
    started.await.unwrap();
    assert!(PendingLogin::acquire(&pending, "admin").is_err());
    let other = PendingLogin::acquire(&pending, "reader").unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let admin = PendingLogin::acquire(&pending, "admin").unwrap();
    drop((admin, other));
    assert!(pending.lock().unwrap().is_empty());
}

#[test]
fn account_backoff_survives_active_window_boundary_and_cools_after_idle_minute() {
    let attempts = Mutex::new(HashMap::from([(
        "user:admin".into(),
        Attempts {
            since: Instant::now() - Duration::from_secs(59),
            count: 20,
        },
    )]));
    assert_eq!(
        admit_login(&attempts, address(1), "admin").unwrap(),
        Duration::from_secs(6)
    );
    assert!(attempts.lock().unwrap()["user:admin"].since.elapsed() < Duration::from_secs(1));
    attempts
        .lock()
        .unwrap()
        .get_mut("user:admin")
        .unwrap()
        .since = Instant::now() - ATTEMPT_WINDOW;
    assert_eq!(
        admit_login(&attempts, address(2), "admin").unwrap(),
        Duration::ZERO
    );
}

#[test]
fn expired_attempts_are_removed_and_key_capacity_is_never_exceeded() {
    let attempts = Mutex::new(HashMap::from([(
        "expired".into(),
        Attempts {
            since: Instant::now() - ATTEMPT_WINDOW,
            count: MAX_ATTEMPTS,
        },
    )]));
    admit_attempts(
        &attempts,
        ["ip:one".into(), "user:one".into()],
        "capacity",
        "attempts",
    )
    .unwrap();
    assert!(!attempts.lock().unwrap().contains_key("expired"));
    {
        let mut values = attempts.lock().unwrap();
        for index in values.len()..MAX_ATTEMPT_KEYS - 1 {
            values.insert(
                format!("existing:{index}"),
                Attempts {
                    since: Instant::now(),
                    count: 1,
                },
            );
        }
    }
    let error = admit_attempts(
        &attempts,
        ["new:one".into(), "new:two".into()],
        "capacity",
        "attempts",
    )
    .unwrap_err();
    assert_eq!(error.status, 429);
    assert_eq!(attempts.lock().unwrap().len(), MAX_ATTEMPT_KEYS - 1);
    assert!(!attempts.lock().unwrap().contains_key("new:one"));
}

async fn queued_session_job(
    auth: Arc<Mutex<crate::auth::AuthStore>>,
    slots: Arc<tokio::sync::Semaphore>,
    token: String,
) -> Result<(), ApiError> {
    auth_job(slots, move || lock(&auth).logout(&token)).await
}

#[tokio::test]
async fn session_write_admission_survives_cancellation_and_rejects_without_waiting() {
    let root = crate::tests::private_tempdir();
    let auth = Arc::new(Mutex::new(
        crate::auth::AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap(),
    ));
    let slots = Arc::new(tokio::sync::Semaphore::new(2));
    let token = authentication::random_token().unwrap();
    let held = Arc::new(tokio::sync::Semaphore::new(0));
    let (release, wait) = std::sync::mpsc::channel();
    let holder = std::thread::spawn({
        let auth = auth.clone();
        let held = held.clone();
        move || {
            let _guard = auth.lock().unwrap();
            held.add_permits(1);
            let _ = wait.recv_timeout(Duration::from_secs(10));
        }
    });
    held.acquire().await.unwrap().forget();
    let mut jobs = Vec::new();
    for _ in 0..2 {
        jobs.push(tokio::spawn(queued_session_job(
            auth.clone(),
            slots.clone(),
            token.clone(),
        )));
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while slots.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        queued_session_job(auth.clone(), slots.clone(), token.clone())
            .await
            .unwrap_err()
            .status,
        429
    );
    for job in jobs {
        job.abort();
        assert!(job.await.unwrap_err().is_cancelled());
    }
    assert_eq!(slots.available_permits(), 0);
    assert_eq!(
        auth_job(slots.clone(), || Ok(())).await.unwrap_err().status,
        429
    );
    assert_eq!(
        queued_session_job(auth.clone(), slots.clone(), token.clone())
            .await
            .unwrap_err()
            .status,
        429
    );
    release.send(()).unwrap();
    holder.join().unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while slots.available_permits() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    queued_session_job(auth, slots, token).await.unwrap();
}

#[derive(Default)]
struct RetryProbe {
    gate: Mutex<()>,
    calls: std::sync::atomic::AtomicUsize,
}

impl crate::Backend for RetryProbe {
    fn operation(
        &self,
        _: &crate::auth::Identity,
        _: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        unreachable!("test does not query operations")
    }
    fn commit_workspace(
        &self,
        _: &crate::auth::Identity,
        _: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        unreachable!("test does not commit workspaces")
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
    fn ready(&self) -> Result<crate::Readiness, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn workspace_capacity(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<storage_contract::WorkspaceCapacity, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn smb_binding_status(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<crate::SmbBindingStatus, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn update_smb_binding(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateSmbBinding<'_>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn protection_policy(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::ProtectionPolicy, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn placement_rule(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::PlacementRule, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn update_protection_policy(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateProtectionPolicy,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn update_placement_rule(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdatePlacementRule,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn compare_commits(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CompareCommits<'_>,
    ) -> Result<storage_contract::CommitComparison, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn disk_status(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::DiskStatus>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn integrity_check(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<storage_contract::CheckReport, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn create_merge_link(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateMergeLink,
    ) -> Result<storage_contract::MergeLink, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn create_repository(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateRepository,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn create_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateBranch,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn create_file_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn update_note(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateNote,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn merge_file_branches(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::MergeFileBranches,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn update_file_node_tags(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateFileNodeTags,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn create_disk_directory(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn create_disk_write_session(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateDiskWriteSession,
    ) -> Result<crate::DiskWriteSession, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn backup_configuration(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadBackupConfiguration,
    ) -> Result<storage_contract::BackupConfigurationSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn configure_backups(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ConfigureBackups,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn plan(
        &self,
        _identity: &crate::auth::Identity,
        _plan: &str,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn cancel_plan(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::CancelPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn register_disk(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<std::convert::Infallible, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn commit_plan(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::CommitPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn prepare_historical_save(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::PrepareHistoricalSave, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn prepare_purge(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::PreparePurge, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn prepare_split(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::PrepareSplit, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn namespace_batch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::NamespaceBatch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn select_file_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::SelectFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn rename_file_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::RenameFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn listing(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadListing,
    ) -> Result<crate::ListingSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn mutate_disk_directory(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::MutateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn initialize_directory_scope(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::InitializeDirectoryScope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn archive_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ArchiveBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn rename_directory_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::RenameDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn select_default_directory_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::SelectDefaultDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn directory_selection(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadDirectorySelection,
    ) -> Result<crate::DirectorySelectionSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn directory_scopes(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadDirectoryScopes<'_>,
    ) -> Result<crate::DirectoryScopesSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn default_directory_branch(
        &self,
        _identity: &crate::auth::Identity,
        _repository_id: &str,
    ) -> Result<crate::DefaultDirectoryBranchSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn file_history_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadFileHistory,
    ) -> Result<crate::FileHistorySnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn file_history(
        &self,
        _identity: &crate::auth::Identity,
        _file: &str,
    ) -> Result<Vec<storage_contract::FileVersion>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn topology_snapshot(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<crate::TopologySnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn placement_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::Placement, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn event_page(
        &self,
        _identity: &crate::auth::Identity,
        _after: u64,
    ) -> Result<crate::EventPage, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn activity_history(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<crate::ActivityHistorySnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn history_graph(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<crate::HistoryGraphSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn commit_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn resolve_entry(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ResolveEntry<'_>,
    ) -> Result<crate::ResolvedEntry, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn branch_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn branches_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<Vec<crate::BranchSnapshot>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn repositories_snapshot(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::Repository>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn tree_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadTree<'_>,
    ) -> Result<crate::TreeSnapshot, crate::ApiError> {
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

    fn retry_storage(&self, _: &Identity) -> Result<crate::StorageStatus, ApiError> {
        let _gate = self.gate.lock().unwrap();
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(crate::StorageStatus::Managed {
            state: crate::StorageState::Unavailable,
            recovering: true,
        })
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

    fn range_content(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadContent<'_>,
    ) -> Result<crate::Content, crate::ApiError> {
        match request.context {
            Some(_) => Err(crate::ApiError::unsupported()),
            None => self.content(identity, request.version),
        }
    }
    fn content(&self, _: &Identity, _: &str) -> Result<crate::Content, ApiError> {
        Err(ApiError::unsupported())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn storage_retry_backend_contention_never_blocks_runtime_or_releases_cancelled_capacity() {
    let root = crate::tests::private_tempdir();
    let mut store =
        crate::auth::AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let login = store.login("admin", "a-long-test-password").unwrap();
    let identity = store.authenticate(&login.token).unwrap();
    let auth = Arc::new(Mutex::new(store));
    let probe = Arc::new(RetryProbe::default());
    let backend: Arc<dyn crate::Backend> = Arc::new(crate::AuthorizedBackend {
        backend: probe.clone(),
        auth: crate::lock(&auth).reader(),
    });
    let slots = Arc::new(tokio::sync::Semaphore::new(2));
    let held = Arc::new(tokio::sync::Semaphore::new(0));
    let (release, wait) = std::sync::mpsc::channel();
    let holder = std::thread::spawn({
        let held = held.clone();
        let probe = probe.clone();
        move || {
            let _guard = probe.gate.lock().unwrap();
            held.add_permits(1);
            // Independent watchdog also releases the lock on a regression that
            // blocks this test's sole Tokio worker, rather than hanging the suite.
            let _ = wait.recv_timeout(Duration::from_secs(10));
        }
    });
    held.acquire().await.unwrap().forget();
    let mut jobs = Vec::new();
    for _ in 0..2 {
        jobs.push(tokio::spawn(storage_retry_job(
            backend.clone(),
            auth.clone(),
            slots.clone(),
            identity.clone(),
        )));
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while slots.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
        // The timer must run before the auth holder's external watchdog fires.
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(slots.available_permits(), 0);
    })
    .await
    .unwrap();
    assert_eq!(probe.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(
        storage_retry_job(
            backend.clone(),
            auth.clone(),
            slots.clone(),
            identity.clone()
        )
        .await
        .unwrap_err()
        .status,
        429,
    );
    for job in jobs {
        job.abort();
        assert!(job.await.unwrap_err().is_cancelled());
    }
    assert_eq!(slots.available_permits(), 0);
    assert_eq!(
        storage_retry_job(backend, auth.clone(), slots.clone(), identity)
            .await
            .unwrap_err()
            .status,
        429,
    );
    release.send(()).unwrap();
    holder.join().unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while slots.available_permits() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(probe.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn storage_retry_rechecks_a_completed_session_revocation() {
    let root = crate::tests::private_tempdir();
    let mut store =
        crate::auth::AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let login = store.login("admin", "a-long-test-password").unwrap();
    let identity = store.authenticate(&login.token).unwrap();
    let probe = Arc::new(RetryProbe::default());
    let backend: Arc<dyn crate::Backend> = Arc::new(crate::AuthorizedBackend {
        backend: probe.clone(),
        auth: store.reader(),
    });
    store.logout(&login.token).unwrap();
    let auth = Arc::new(Mutex::new(store));
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    assert_eq!(
        storage_retry_job(backend, auth.clone(), slots.clone(), identity)
            .await
            .unwrap_err()
            .status,
        401
    );
    assert_eq!(slots.available_permits(), 1);
    assert_eq!(probe.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn bearer_scheme_is_case_insensitive_without_normalizing_credentials() {
    for scheme in ["Bearer", "bearer", "BEARER", "bEaReR"] {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("{scheme} AbC-_123")).unwrap(),
        );
        assert_eq!(request_token(&headers).unwrap(), ("AbC-_123".into(), false));
    }
    for value in [
        "Bearer",
        "Bearer ",
        " Bearer token",
        "Basic token",
        "Bearer  token",
        "Bearer token extra",
        "Bearer token\t",
        "Bearer\ttoken",
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        assert_eq!(
            request_token(&headers).unwrap_err().status,
            401,
            "{value:?}"
        );
    }
    let mut headers = HeaderMap::new();
    headers.append(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer token"),
    );
    headers.append(
        header::AUTHORIZATION,
        HeaderValue::from_static("bearer token"),
    );
    assert_eq!(request_token(&headers).unwrap_err().status, 401);
}

#[tokio::test]
async fn session_identity_exposes_no_role_or_repository_grants() {
    let token = authentication::random_token().unwrap();
    let identity = Identity {
        admission_session: None,
        smb_binding: None,
        principal: "owner".into(),
    };
    let Json(value) = me(
        http_routing::Extension(identity),
        http_routing::Extension(SessionToken(token.clone())),
    )
    .await
    .unwrap();
    assert_eq!(
        value,
        json!({"principal":"owner", "csrf_token":authentication::derive_csrf_token(&token).unwrap()})
    );
}

#[test]
fn password_change_request_cannot_create_rename_disable_or_scope_an_account() {
    for (key, value) in [
        ("username", json!("other")),
        ("admin", json!(true)),
        ("repositories", json!([])),
        ("disabled", json!(false)),
    ] {
        let mut body =
            json!({"current_password":"current-password", "new_password":"new-password-value"});
        body[key] = value;
        assert!(
            storage_contract::decode_strict::<PasswordChange>(&json_codec::to_vec(&body).unwrap())
                .is_err()
        );
    }
}

#[test]
fn forwarded_address_parse_failure_preserves_source() {
    use std::error::Error as _;
    let error = client_ip(
        &request_from("127.0.0.1:1234", Some("not-an-ip")),
        &proxy_config(),
    )
    .unwrap_err();
    let mut source = error.source();
    while let Some(cause) = source {
        if cause.is::<std::net::AddrParseError>() {
            return;
        }
        source = cause.source();
    }
    panic!("missing address parse source");
}

#[test]
fn admitted_configuration_shares_paths_proxies_and_canonical_origin() {
    let config = proxy_config().admit().unwrap();
    let cloned = Arc::clone(&config);
    assert!(Arc::ptr_eq(&config, &cloned));
    assert!(std::ptr::eq(
        config.spool_dir.as_path(),
        cloned.spool_dir.as_path()
    ));
    assert!(std::ptr::eq(
        config.trusted_proxy_ips.as_ptr(),
        cloned.trusted_proxy_ips.as_ptr()
    ));
    assert!(std::ptr::eq(
        config.external_origin.as_ref().unwrap().0.as_ptr(),
        cloned.external_origin.as_ref().unwrap().0.as_ptr(),
    ));
}

#[test]
fn request_origin_admission_still_rejects_missing_duplicate_and_cross_site_headers() {
    let config = proxy_config().admit().unwrap();
    let mut headers = HeaderMap::new();
    assert!(same_origin(&headers, &config, false).is_ok());
    assert!(same_origin(&headers, &config, true).is_err());
    headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://storage.example.org"),
    );
    assert!(same_origin(&headers, &config, true).is_ok());
    headers.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
    assert!(same_origin(&headers, &config, true).is_err());
    headers.remove("sec-fetch-site");
    headers.append(
        header::ORIGIN,
        HeaderValue::from_static("https://storage.example.org"),
    );
    assert!(same_origin(&headers, &config, true).is_err());
}
