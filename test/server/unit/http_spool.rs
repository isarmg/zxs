use super::*;
use crate::{
    Backend, BackendAction, Content,
    auth::{AuthStore, Identity},
};
use http_routing::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use json_codec::{Value, json};
use service_stack::ServiceExt;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

const DEADLINE: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Probe(AtomicUsize);
impl Backend for Probe {
    fn call(&self, _: BackendAction, _: &Identity, _: Value) -> Result<Value, ApiError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ApiError::unsupported())
    }
    fn observe_disks(&self, _: &crate::InternalDiskObservation) -> Result<Value, ApiError> {
        Ok(json!({"disks":[]}))
    }
    fn upload(
        &self,
        _: &Identity,
        _: &str,
        _: &str,
        _: &str,
        _: &crate::UploadSource,
    ) -> Result<Value, ApiError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ApiError::unsupported())
    }
    fn content(&self, _: &Identity, _: &str) -> Result<Content, ApiError> {
        Err(ApiError::unsupported())
    }
}

async fn response(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
) -> http_routing::response::Response {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}
async fn json_body(response: http_routing::response::Response) -> Value {
    json_codec::from_slice(
        &http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn stalled_real_inventory_keeps_management_responsive_and_uploads_pending() {
    let root = crate::tests::private_tempdir();
    let mut config = ServerConfig::loopback(root.path());
    config.http_spool_limits.bytes_total = 65536 + 7;
    config.http_spool_limits.bytes_per_account = 65536 + 7;
    config.prepare_spool().unwrap();
    let unknown = config.spool_dir.join("retained-unknown");
    std::fs::write(&unknown, b"payload").unwrap();
    let web = root.path().join("web");
    std::fs::create_dir(&web).unwrap();
    std::fs::write(web.join("index.html"), b"management").unwrap();
    config.web_dir = Some(web);
    let auth =
        AuthStore::initialize(&root.path().join("auth"), "admin", "a-long-test-password").unwrap();
    let probe = Arc::new(Probe::default());
    let runtime = crate::ManagementRuntime::start(probe.clone(), auth, &config).unwrap();
    let (spool, startup) = HttpSpool::pending();
    let app = crate::router_shared(
        runtime.backend,
        runtime.auth,
        config.clone(),
        runtime.telemetry,
        spool.clone(),
    )
    .unwrap();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let worker_config = config.clone();
    let job = tokio::spawn(startup.run_with(move || {
        // Finish real inventory and retain its exclusive lock, but withhold
        // its result to model a slow startup job completion boundary.
        let budget = worker_config.open_http_spool().unwrap();
        started.send(()).unwrap();
        blocked.recv_timeout(DEADLINE).unwrap();
        Some(budget)
    }));
    tokio::time::timeout(DEADLINE, waiting)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(DEADLINE, tokio::time::sleep(Duration::from_millis(10)))
        .await
        .unwrap();
    assert!(!job.is_finished());
    assert!(SpoolBudget::recover(&config.spool_dir, config.http_spool_limits.clone()).is_err());
    for path in ["/", "/healthz", "/api/management/status"] {
        assert_eq!(
            response(&app, "GET", path, None).await.status(),
            StatusCode::OK
        );
    }
    let status = json_body(response(&app, "GET", "/api/management/status", None).await).await;
    assert_eq!(status["auxiliary"]["http_uploads"], "pending");
    assert!(!status.to_string().contains(root.path().to_str().unwrap()));
    let login = app
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
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let login = json_body(login).await;
    let token = login["token"].as_str().unwrap();
    for path in [
        "/api/auth/me",
        "/api/system/status",
        "/api/system/history?range=1min",
    ] {
        assert_eq!(
            response(&app, "GET", path, Some(token)).await.status(),
            StatusCode::OK
        );
    }
    for (method, path) in [
        ("PUT", "/api/workspaces/ws/files/item/content?path=file"),
        ("GET", "/api/workspaces/ws/files/item/upload-status"),
    ] {
        assert_eq!(
            response(&app, method, path, None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let pending = response(&app, method, path, Some(token)).await;
        assert_eq!(pending.status(), StatusCode::SERVICE_UNAVAILABLE);
        let error = json_body(pending).await;
        assert_eq!(error["error"]["code"], "HTTP_UPLOADS_PENDING");
        assert_eq!(error["error"]["retryable"], true);
    }
    let csrf = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/workspaces/ws/files/item/content?path=file")
                .header("cookie", cookie)
                .header("host", "127.0.0.1:7400")
                .header("origin", "http://127.0.0.1:7400")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(csrf.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        probe.0.load(Ordering::SeqCst),
        0,
        "pending staging must not dispatch backend work"
    );
    assert_eq!(std::fs::read_dir(&config.spool_dir).unwrap().count(), 2);
    release.send(()).unwrap();
    tokio::time::timeout(DEADLINE, job).await.unwrap().unwrap();
    assert_eq!(spool.status(), "ready");
    assert_eq!(
        json_body(response(&app, "GET", "/api/management/status", None).await).await["auxiliary"]["http_uploads"],
        "ready"
    );
    // Unknown retained bytes remain charged; readiness never creates capacity.
    let error = spool
        .budget()
        .unwrap()
        .reserve("admin", "new", 0)
        .err()
        .unwrap();
    assert_eq!(error.code, "RESOURCE_LIMIT_EXCEEDED");
    assert_eq!(std::fs::read(&unknown).unwrap(), b"payload");
}

#[tokio::test]
async fn failed_inventory_and_panicked_job_become_terminal_unavailable() {
    for failure in ["occupied", "unsafe_inventory", "locked", "panic"] {
        let root = crate::tests::private_tempdir();
        let config = ServerConfig::loopback(root.path());
        let owner = match failure {
            "occupied" => {
                std::fs::write(&config.spool_dir, b"evidence").unwrap();
                None
            }
            "unsafe_inventory" => {
                config.prepare_spool().unwrap();
                std::fs::create_dir(config.spool_dir.join("unexpected")).unwrap();
                None
            }
            "locked" => config.open_http_spool(),
            _ => None,
        };
        let (state, startup) = HttpSpool::pending();
        assert_eq!(state.status(), "pending");
        if failure == "panic" {
            startup.run_with(|| panic!("injected startup panic")).await;
        } else {
            startup.run(config.clone()).await;
        }
        assert_eq!(state.status(), "unavailable", "{failure}");
        assert_eq!(
            state.budget().err().unwrap().code,
            "HTTP_UPLOADS_UNAVAILABLE"
        );
        if failure == "occupied" {
            assert_eq!(std::fs::read(&config.spool_dir).unwrap(), b"evidence");
        }
        drop(owner);
        // No automatic retry after the cause changes or another lock releases.
        tokio::task::yield_now().await;
        assert_eq!(state.status(), "unavailable");
    }
}

#[test]
fn dropped_publisher_never_becomes_ready_and_runtime_joins_inventory() {
    let root = crate::tests::private_tempdir();
    let config = ServerConfig::loopback(root.path());
    let (state, startup) = HttpSpool::pending();
    let observed = state.clone();
    let (release, blocked) = std::sync::mpsc::channel();
    let (cancelled, cancellation) = std::sync::mpsc::channel();
    let (finished, completion) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let (started, waiting) = tokio::sync::oneshot::channel();
            let job = tokio::spawn(startup.run_with(move || {
                let budget = config.open_http_spool().unwrap();
                started.send(()).unwrap();
                blocked.recv_timeout(DEADLINE).unwrap();
                Some(budget)
            }));
            waiting.await.unwrap();
            job.abort();
            assert!(job.await.unwrap_err().is_cancelled());
            cancelled.send(()).unwrap();
        });
        drop(runtime); // Must join the already running blocking inventory.
        finished.send(()).unwrap();
    });
    cancellation.recv_timeout(DEADLINE).unwrap();
    assert!(matches!(
        completion.recv_timeout(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    assert_eq!(observed.status(), "pending");
    assert_eq!(
        observed.budget().err().unwrap().code,
        "HTTP_UPLOADS_PENDING"
    );
    release.send(()).unwrap();
    completion.recv_timeout(DEADLINE).unwrap();
    worker.join().unwrap();
    assert_eq!(
        state.status(),
        "pending",
        "dropped publisher must never install a late budget"
    );
    let config = ServerConfig::loopback(root.path());
    assert!(
        config.open_http_spool().is_some(),
        "unpublished completed inventory must release its lock"
    );
}

#[tokio::test]
async fn dropping_unstarted_token_performs_no_inventory() {
    let root = crate::tests::private_tempdir();
    let config = ServerConfig::loopback(root.path());
    let (state, startup) = HttpSpool::pending();
    let future = startup.run(config.clone());
    drop(future);
    assert_eq!(state.status(), "pending");
    assert!(!config.spool_dir.exists());
}
