use super::*;
use http_routing::{
    body::{Body, Bytes},
    http::StatusCode,
};
use service_stack::ServiceExt;
use std::time::Duration;
// Authentication roots obey the same 0700 contract as deployed control state.
// tempfile's default directory mode is 0777 filtered by umask, not 0700.
pub(crate) fn private_tempdir() -> tempfile::TempDir {
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().unwrap()
}
struct TestBackend {
    path: PathBuf,
}
impl Backend for TestBackend {
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
        action: crate::BackendAction,
        _identity: &Identity,
        _input: Value,
    ) -> Result<Value, ApiError> {
        match action {
            crate::BackendAction::Capabilities
            | crate::BackendAction::UploadAuthorize
            | crate::BackendAction::WorkspaceAuthorize
            | crate::BackendAction::CreateRepository => Ok(json!({"accepted":true})),
            BackendAction::DirectoryBranchScopes | BackendAction::DirectoryBranchSelection => {
                Ok(json!({"action":action.as_str()}))
            }
            _ => Err(ApiError::unsupported()),
        }
    }
    fn upload(
        &self,
        _identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
    ) -> Result<Value, ApiError> {
        let mut bytes = vec![0; source.size as usize];
        std::os::unix::fs::FileExt::read_exact_at(&source.file, &mut bytes, 0)
            .map_err(ApiError::io)?;
        Ok(
            json!({"workspace_id":workspace,"item_id":item,"path":path,"size":bytes.len().to_string(),"content_hash":blake3::hash(&bytes).to_hex().to_string()}),
        )
    }
    fn checksums(&self, _identity: &Identity, _version: &str) -> Result<Vec<u8>, ApiError> {
        Ok(vec![7; 128 * 1024])
    }
    fn content(&self, _identity: &Identity, _version: &str) -> Result<Content, ApiError> {
        Ok(Content {
            checksum_manifest: None,
            fallback_files: Vec::new(),
            file: std::fs::File::open(&self.path).map_err(ApiError::io)?,
            size: 10,
            hash: blake3::hash(b"0123456789").to_hex().to_string(),
            lease: Box::new(()),
            download_name: None,
        })
    }
}
fn application(root: &Path) -> Router {
    application_config(root, ServerConfig::loopback(root))
}
fn application_config(root: &Path, config: ServerConfig) -> Router {
    std::fs::write(root.join("content"), b"0123456789").unwrap();
    let auth = AuthStore::initialize(root, "admin", "a-long-test-password").unwrap();
    router(
        Arc::new(TestBackend {
            path: root.join("content"),
        }),
        auth,
        config,
    )
    .unwrap()
}

#[tokio::test]
async fn static_assets_never_shadow_unknown_api_routes_or_their_error_envelope() {
    let root = crate::tests::private_tempdir();
    let assets = root.path().join("web");
    std::fs::create_dir_all(assets.join("api")).unwrap();
    std::fs::write(assets.join("index.html"), b"<html>fixture</html>").unwrap();
    std::fs::write(assets.join("api/unknown"), b"must never be an API response").unwrap();
    let mut config = ServerConfig::loopback(root.path());
    config.web_dir = Some(assets);
    let app = application_config(root.path(), config);
    for path in ["/api", "/api/unknown", "/api/missing", "/missing.css"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::IF_NONE_MATCH, "*")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        if http_boundary::is_api_path(path) {
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
        let error: Value = json_codec::from_slice(
            &http_routing::body::to_bytes(response.into_body(), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(error["error"]["code"], "NOT_FOUND");
    }
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert_eq!(
        http_routing::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
        "<html>fixture</html>"
    );
}
#[test]
fn router_prepares_a_fresh_nested_spool_before_monitor_cache_initialization() {
    let root = crate::tests::private_tempdir();
    let nested = root.path().join("fresh/nested/system");
    let auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let app = router(
        Arc::new(TestBackend {
            path: root.path().join("content"),
        }),
        auth,
        ServerConfig::loopback(&nested),
    );
    assert!(
        app.is_ok(),
        "fresh nested spool must initialize before telemetry"
    );
    assert!(nested.join("api-spool").is_dir());
    assert!(nested.join("telemetry-cache").is_dir());
}

#[test]
fn an_unwritable_monitor_cache_does_not_prevent_the_business_router_starting() {
    let root = crate::tests::private_tempdir();
    let auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    // A preexisting ordinary file deterministically prevents cache writes,
    // including when tests run as root and chmod would not deny access.
    std::fs::write(root.path().join("telemetry-cache"), b"keep this file").unwrap();
    let app = router(
        Arc::new(TestBackend {
            path: root.path().join("content"),
        }),
        auth,
        ServerConfig::loopback(root.path()),
    );
    assert!(
        app.is_ok(),
        "optional monitoring persistence must not stop business routes"
    );
    assert_eq!(
        std::fs::read(root.path().join("telemetry-cache")).unwrap(),
        b"keep this file"
    );
}

async fn authenticated(app: &Router) -> (String, String, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"username":"admin","password":"a-long-test-password"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let body: Value = json_codec::from_slice(
        &http_routing::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
    )
    .unwrap();
    (
        body["token"].as_str().unwrap().into(),
        body["csrf_token"].as_str().unwrap().into(),
        cookie,
    )
}

#[tokio::test]
async fn cookie_mutations_require_csrf_and_foreign_origins_are_rejected() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, csrf, cookie) = authenticated(&app).await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/repositories")
                .header("cookie", &cookie)
                .header("host", "127.0.0.1:7400")
                .header("origin", "http://127.0.0.1:7400")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/repositories")
                .header("cookie", &cookie)
                .header("x-csrf-token", csrf)
                .header("host", "127.0.0.1:7400")
                .header("origin", "http://127.0.0.1:7400")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header("authorization", format!("Bearer {token}"))
                .header("origin", "https://foreign.invalid")
                .header("host", "127.0.0.1:7400")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn authenticated_range_download_returns_exact_immutable_bytes() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/versions/test/content")
                .header("authorization", format!("Bearer {token}"))
                .header("range", "bytes=2-5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 2-5/10");
    assert_eq!(
        http_routing::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
        "2345"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn authentication_lock_wait_does_not_block_the_async_executor() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let root = crate::tests::private_tempdir();
    let mut auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let token = auth.login("admin", "a-long-test-password").unwrap().token;
    let (started, waiting) = tokio::sync::oneshot::channel();
    let started = Mutex::new(Some(started));
    let (release, released) = std::sync::mpsc::channel();
    let released = Mutex::new(released);
    let armed = Arc::new(AtomicBool::new(false));
    let timed_out = Arc::new(AtomicBool::new(false));
    let (hook_armed, hook_timeout) = (armed.clone(), timed_out.clone());
    auth.attach_backup(Arc::new(move |_| {
        if hook_armed.load(Ordering::SeqCst)
            && let Some(started) = started.lock().unwrap().take()
        {
            let _ = started.send(());
            if released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(2))
                .is_err()
            {
                hook_timeout.store(true, Ordering::SeqCst);
            }
        }
        Ok(())
    }))
    .unwrap();
    armed.store(true, Ordering::SeqCst);
    let app = router(
        Arc::new(TestBackend {
            path: root.path().join("unused"),
        }),
        auth,
        ServerConfig::loopback(root.path()),
    )
    .unwrap();
    let login_app = app.clone();
    let login = tokio::spawn(async move {
        login_app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .body(Body::from(
                        r#"{"username":"admin","password":"a-long-test-password"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    });
    waiting.await.unwrap();
    let release_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        release.send(()).unwrap();
    });
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    release_task.await.unwrap();
    assert_eq!(login.await.unwrap().status(), StatusCode::OK);
    assert!(
        !timed_out.load(Ordering::SeqCst),
        "authentication starved the timer that releases its blocking backup"
    );
}

#[tokio::test]
async fn interrupted_upload_retains_durable_bytes_and_rejects_different_retry() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    let stream = futures::stream::iter([
        Ok(Bytes::from_static(b"original")),
        Err(std::io::Error::other("connection interrupted")),
    ]);
    let request = Request::builder()
        .method("PUT")
        .uri("/api/workspaces/ws/files/item/content?path=file")
        .header("authorization", format!("Bearer {token}"))
        .header("x-zxs-logical-size", "20")
        .header("x-zxs-content-hash", "0".repeat(64))
        .body(Body::from_stream(stream))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let record = uploads::load(&root.path().join("api-spool"), "ws", "item")
        .unwrap()
        .unwrap();
    assert_eq!(record.persisted_bytes, 8);
    assert_eq!(
        std::fs::read(root.path().join("api-spool").join(&record.data_file)).unwrap(),
        b"original"
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/workspaces/ws/files/item/content?path=file")
                .header("authorization", format!("Bearer {token}"))
                .header("x-zxs-logical-size", "20")
                .header("x-zxs-content-hash", "0".repeat(64))
                .body(Body::from("different retry data"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        uploads::load(&root.path().join("api-spool"), "ws", "item")
            .unwrap()
            .unwrap()
            .data_file,
        record.data_file
    );
    let data_files = std::fs::read_dir(root.path().join("api-spool"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "data")
        })
        .count();
    assert_eq!(
        data_files, 1,
        "conflicting retries must remove their unreferenced staging file"
    );
}

#[tokio::test]
async fn durable_retry_progress_appends_without_copying_the_same_generation() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    let spool = root.path().join("api-spool");
    for content in ["original", "original plus", "original plus more"] {
        let stream = futures::stream::iter([
            Ok(Bytes::copy_from_slice(content.as_bytes())),
            Err(std::io::Error::other("interrupted")),
        ]);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/workspaces/ws/files/item/content?path=file")
                    .header("authorization", format!("Bearer {token}"))
                    .header("x-zxs-logical-size", "30")
                    .header("x-zxs-content-hash", "0".repeat(64))
                    .body(Body::from_stream(stream))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let current = uploads::load(&spool, "ws", "item").unwrap().unwrap();
        assert_eq!(
            std::fs::read(spool.join(current.data_file)).unwrap(),
            content.as_bytes()
        );
        assert_eq!(
            std::fs::read_dir(&spool)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path
                    .extension()
                    .is_some_and(|extension| extension == "data"))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn unpublished_retry_cleanup_covers_short_long_and_cancelled_bodies() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    let bytes = vec![42u8; 2 * 1024 * 1024];
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let request = |body| {
        Request::builder()
            .method("PUT")
            .uri("/api/workspaces/ws/files/item/content?path=file")
            .header("authorization", format!("Bearer {token}"))
            .header("x-zxs-logical-size", bytes.len().to_string())
            .header("x-zxs-content-hash", &hash)
            .body(body)
            .unwrap()
    };
    assert_eq!(
        app.clone()
            .oneshot(request(Body::from(bytes.clone())))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let spool = root.path().join("api-spool");
    let original = uploads::load(&spool, "ws", "item").unwrap().unwrap();
    // A retry must not supersede old progress until it has checked all of it.
    for body in [
        Body::from(vec![42u8; 1024]),
        Body::from(vec![42u8; bytes.len() + 1]),
        Body::from_stream(futures::stream::iter([
            Ok(Bytes::from(vec![42u8; 1024])),
            Err(std::io::Error::other("interrupted")),
        ])),
    ] {
        assert!(
            !app.clone()
                .oneshot(request(body))
                .await
                .unwrap()
                .status()
                .is_success()
        );
    }
    let (sent, received) = tokio::sync::oneshot::channel();
    let stream = futures::stream::once(async move {
        let _ = sent.send(());
        Ok::<_, std::io::Error>(Bytes::from(vec![42u8; 1024]))
    });
    let stream = futures::StreamExt::chain(stream, futures::stream::pending());
    let retry_app = app.clone();
    let retry_request = request(Body::from_stream(stream));
    let retry = tokio::spawn(async move { retry_app.oneshot(retry_request).await });
    received.await.unwrap();
    retry.abort();
    let _ = retry.await;
    // Cancellation may leave a running prefix-check job holding the guard.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let files = std::fs::read_dir(&spool)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "data")
                })
                .count();
            if files == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let current = uploads::load(&spool, "ws", "item").unwrap().unwrap();
    assert_eq!(current.data_file, original.data_file);
    assert_eq!(std::fs::read(spool.join(current.data_file)).unwrap(), bytes);
}

#[tokio::test]
async fn successful_new_generation_preserves_old_bytes_and_retry_reuses_its_file() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    let spool = root.path().join("api-spool");
    let mut records = Vec::new();
    for (generation, bytes) in [(1, "original"), (2, "replacement"), (2, "replacement")] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/workspaces/ws/files/item/content?path=file")
                    .header("authorization", format!("Bearer {token}"))
                    .header("x-zxs-logical-size", bytes.len().to_string())
                    .header(
                        "x-zxs-content-hash",
                        blake3::hash(bytes.as_bytes()).to_hex().as_str(),
                    )
                    .header("x-zxs-upload-generation", generation.to_string())
                    .body(Body::from(bytes))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        records.push(uploads::load(&spool, "ws", "item").unwrap().unwrap());
        assert_eq!(
            std::fs::read(spool.join(&records[0].data_file)).unwrap(),
            b"original"
        );
    }
    assert_eq!(records[1].data_file, records[2].data_file);
    assert_eq!(
        std::fs::read(spool.join(&records[2].data_file)).unwrap(),
        b"replacement"
    );
    let retained = std::fs::read_dir(&spool)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("retained-")
        })
        .collect::<Vec<_>>();
    assert_eq!(retained.len(), 1);
    let retained: uploads::UploadRecord =
        json_codec::from_slice(&std::fs::read(&retained[0]).unwrap()).unwrap();
    assert_eq!(retained.generation, 1);
    assert_eq!(retained.data_file, records[0].data_file);
}

#[test]
fn disconnected_upload_keeps_its_claim_until_publication_and_status_are_durable() {
    upload_publication_keeps_its_claim(true);
}

#[test]
fn upload_publication_timeout_reports_an_unknown_outcome_until_status_is_durable() {
    upload_publication_keeps_its_claim(false);
}

fn upload_publication_keeps_its_claim(disconnect: bool) {
    struct BlockingUpload {
        delegate: TestBackend,
        started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        gate: Arc<(Mutex<bool>, std::sync::Condvar)>,
    }
    impl Backend for BlockingUpload {
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
            action: crate::BackendAction,
            identity: &Identity,
            input: Value,
        ) -> Result<Value, ApiError> {
            self.delegate.call(action, identity, input)
        }
        fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
            self.delegate.content(identity, version)
        }
        fn upload(
            &self,
            identity: &Identity,
            workspace: &str,
            item: &str,
            path: &name_policy::PortablePath,
            source: &crate::UploadSource,
        ) -> Result<Value, ApiError> {
            if let Some(started) = self.started.lock().unwrap().take() {
                let _ = started.send(());
            }
            let (ready, wake) = &*self.gate;
            let mut ready = ready.lock().unwrap();
            while !*ready {
                ready = wake.wait(ready).unwrap();
            }
            self.delegate
                .upload(identity, workspace, item, path, source)
        }
    }
    struct Release(Arc<(Mutex<bool>, std::sync::Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0.0.lock().unwrap() = true;
            self.0.1.notify_all();
        }
    }
    let root = crate::tests::private_tempdir();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let release = Release(gate.clone());
        let mut config = ServerConfig::loopback(root.path());
        if !disconnect {
            config.http_transfer_limits.total_timeout = Duration::from_secs(3);
            config.http_transfer_limits.idle_timeout = Duration::from_secs(3);
        }
        let app = router(
            Arc::new(BlockingUpload {
                delegate: TestBackend {
                    path: root.path().join("content"),
                },
                started: Mutex::new(Some(started)),
                gate,
            }),
            auth,
            config,
        )
        .unwrap();
        let (token, _, _) = authenticated(&app).await;
        let request = || {
            Request::builder()
                .method("PUT")
                .uri("/api/workspaces/ws/files/item/content?path=file")
                .header("authorization", format!("Bearer {token}"))
                .header("x-zxs-logical-size", "4")
                .header(
                    "x-zxs-content-hash",
                    blake3::hash(b"body").to_hex().to_string(),
                )
                .body(Body::from("body"))
                .unwrap()
        };
        let pending = tokio::spawn(app.clone().oneshot(request()));
        tokio::time::timeout(Duration::from_secs(5), started_rx)
            .await
            .unwrap()
            .unwrap();
        let timed_out = if disconnect {
            pending.abort();
            assert!(pending.await.unwrap_err().is_cancelled());
            None
        } else {
            Some(pending.await.unwrap().unwrap())
        };
        let retry = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(retry.status(), StatusCode::CONFLICT);
        let error: Value = json_codec::from_slice(
            &http_routing::body::to_bytes(retry.into_body(), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(error["error"]["code"], "FILE_WRITE_BUSY");
        drop(release);
        if let Some(response) = timed_out {
            // The admitted backend can complete despite the HTTP timeout.
            // It is therefore not a definite client-side rejection (4xx).
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert!(storage_contract::client_protocol::mutation_outcome_unknown(
                response.status().as_u16()
            ));
            let body: Value = json_codec::from_slice(
                &http_routing::body::to_bytes(response.into_body(), 8192)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(body["error"]["code"], "TRANSFER_TIMEOUT");
            assert!(
                body["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("status")
            );
        }
    });
    // Wait for admitted blocking publication, including its final atomic status
    // replacement, before reading that status or removing the temporary root.
    drop(runtime);
    assert_eq!(
        uploads::load(&root.path().join("api-spool"), "ws", "item")
            .unwrap()
            .unwrap()
            .state,
        crate::uploads::UploadState::Complete
    );
}

#[path = "tests/spool_budget.rs"]
mod spool_budget_tests;

#[path = "tests/http_boundary.rs"]
mod http_boundary_tests;

#[path = "tests/auxiliary_startup.rs"]
mod auxiliary_startup_tests;

#[tokio::test]
async fn workspace_capacity_requires_authentication_and_preserves_unsupported_backend() {
    let root = private_tempdir();
    let app = application(root.path());
    let request = Request::builder()
        .uri("/api/workspaces/capacity")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let (token, _, _) = authenticated(&app).await;
    for scheme in ["Bearer", "bearer", "BEARER"] {
        let request = Request::builder()
            .uri("/api/workspaces/capacity")
            .header(header::AUTHORIZATION, format!("{scheme} {token}"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::NOT_IMPLEMENTED
        );
    }
    let request = Request::builder()
        .uri("/api/workspaces/capacity")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", token.to_uppercase()),
        )
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn directory_scope_routes_select_distinct_actions_and_preserve_csrf() {
    let root = private_tempdir();
    let app = application(root.path());
    let (_, csrf, cookie) = authenticated(&app).await;
    let uri = "/api/repositories/repo/directory-branches";
    for (method, expected) in [
        ("GET", BackendAction::DirectoryBranchScopes),
        ("POST", BackendAction::DirectoryBranchSelection),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::COOKIE, &cookie)
                    .header(header::HOST, "127.0.0.1:7400")
                    .header(header::ORIGIN, "http://127.0.0.1:7400")
                    .header("x-csrf-token", &csrf)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = json_codec::from_slice(
            &http_routing::body::to_bytes(response.into_body(), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["action"], expected.as_str());
    }
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header(header::HOST, "127.0.0.1:7400")
                .header(header::ORIGIN, "http://127.0.0.1:7400")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn upload_path_admission_rejects_nonportable_names_and_keeps_display_spelling() {
    let root = private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    for (index, path) in ["/outside", "CON", "e\u{301}.txt", "Café.txt"]
        .iter()
        .enumerate()
    {
        let item = format!("item-{index}");
        let query = url_encoding::form::Serializer::new(String::new())
            .append_pair("path", path)
            .finish();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/workspaces/ws/files/{item}/content?{query}"))
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-length", "4")
                    .header(
                        "x-zxs-content-hash",
                        blake3::hash(b"body").to_hex().to_string(),
                    )
                    .body(Body::from("body"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body: Value = json_codec::from_slice(
            &http_routing::body::to_bytes(response.into_body(), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        if *path == "Café.txt" {
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["path"], *path);
            assert_eq!(body["size"], "4");
        } else {
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"]["code"], "INVALID_REQUEST");
            assert!(
                uploads::load(&root.path().join("api-spool"), "ws", &item)
                    .unwrap()
                    .is_none()
            );
        }
    }
}
