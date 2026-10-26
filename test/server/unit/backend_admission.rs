use super::*;
use http_routing::{body::Body, http::StatusCode};
use service_stack::ServiceExt;
use std::sync::{
    Condvar,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
    started: tokio::sync::Semaphore,
    calls: AtomicUsize,
    finished: tokio::sync::Semaphore,
}
impl Gate {
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
// Always unblock workers if a test assertion fails, too.
struct Release(Arc<Gate>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
#[derive(Clone, Copy)]
enum BlockedEndpoint {
    Backend,
    Capabilities,
    Ready,
    Capacity,
}
struct BackendProbe {
    blocked: BlockedEndpoint,
    gate: Arc<Gate>,
    outcome: u8,
    auth: Mutex<Option<Arc<Mutex<AuthStore>>>>,
}
impl BackendProbe {
    fn wait(&self) -> Result<(), ApiError> {
        self.gate.calls.fetch_add(1, Ordering::SeqCst);
        self.gate.started.add_permits(1);
        let open = self.gate.open.lock().unwrap();
        drop(self.gate.changed.wait_while(open, |open| !*open).unwrap());
        self.gate.finished.add_permits(1);
        match self.outcome {
            1 => Err(ApiError::conflict(
                storage_contract::ErrorCode::InjectedFailure,
                "backend error",
            )),
            2 => panic!("test backend panic"),
            _ => Ok(()),
        }
    }
}
impl Backend for BackendProbe {
    fn operation(
        &self,
        _: &crate::auth::Identity,
        _: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
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
    ) -> Result<storage_contract::Workspace, ApiError> {
        self.wait()?;
        // This probe exercises queued error completion, not workspace storage.
        Err(ApiError::unsupported())
    }
    fn abort_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<(), ApiError> {
        self.wait()
    }

    fn workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn published_upload_status(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _item: &str,
    ) -> Result<Option<crate::PublishedUploadStatus>, crate::ApiError> {
        self.wait()?;
        Ok(Some(crate::PublishedUploadStatus {
            generation: 1,
            logical_size: 0,
            content_hash: storage_contract::ContentHash::from_digest(blake3::hash(b"")),
            state: crate::PublishedUploadState::Complete,
        }))
    }
    fn seal_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<storage_contract::SealedWorkspace, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn commit_workspace_public(
        &self,
        _identity: &crate::auth::Identity,
        _command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn version_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _version: &str,
    ) -> Result<crate::VersionSnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn authorize_version(
        &self,
        _identity: &crate::auth::Identity,
        _version: &str,
    ) -> Result<(), crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn repository_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn authorize_repository(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<(), crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn capabilities(&self) -> Result<crate::Capabilities, crate::ApiError> {
        if matches!(self.blocked, BlockedEndpoint::Capabilities) {
            self.wait()?;
        }
        Ok(crate::Capabilities::for_engine(4096))
    }
    fn ready(&self) -> Result<crate::Readiness, crate::ApiError> {
        if matches!(self.blocked, BlockedEndpoint::Ready) {
            self.wait()?;
        }
        Ok(crate::Readiness::Managed)
    }
    fn workspace_capacity(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<storage_contract::WorkspaceCapacity, crate::ApiError> {
        self.wait()?;
        Ok(probe_capacity())
    }
    fn branch_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn branches_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<Vec<crate::BranchSnapshot>, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn repositories_snapshot(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::Repository>, crate::ApiError> {
        self.wait()?;
        Ok(Vec::new())
    }
    fn tree_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadTree<'_>,
    ) -> Result<crate::TreeSnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn create_write_session(
        &self,
        _identity: &crate::auth::Identity,
        _branch: &str,
    ) -> Result<crate::WorkspaceWriteSession, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn stage_namespace(
        &self,
        _identity: &crate::auth::Identity,
        _command: crate::StageNamespace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        self.wait()?;
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

    fn start_management(&self, auth: Arc<Mutex<AuthStore>>) {
        *self.auth.lock().unwrap() = Some(auth);
    }
    fn call(&self, _action: BackendAction, _: &Identity, _: Value) -> Result<Value, ApiError> {
        self.wait().map(|()| json!({}))
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
    fn content(&self, _: &Identity, _: &str) -> Result<Content, ApiError> {
        Err(ApiError::unsupported())
    }
}
fn fixture(root: &Path, outcome: u8, open: bool) -> (Router, Arc<Gate>, Release) {
    fixture_for(root, outcome, open, BlockedEndpoint::Backend)
}
fn fixture_for(
    root: &Path,
    outcome: u8,
    open: bool,
    blocked: BlockedEndpoint,
) -> (Router, Arc<Gate>, Release) {
    let gate = Arc::new(Gate {
        open: Mutex::new(open),
        changed: Condvar::new(),
        started: tokio::sync::Semaphore::new(0),
        calls: AtomicUsize::new(0),
        finished: tokio::sync::Semaphore::new(0),
    });
    let mut auth = AuthStore::initialize(root, "admin", "a-long-test-password").unwrap();
    let token = auth.login("admin", "a-long-test-password").unwrap().token;
    let app = router(
        Arc::new(BackendProbe {
            blocked,
            gate: gate.clone(),
            outcome,
            auth: Mutex::new(None),
        }),
        auth,
        ServerConfig::loopback(root),
    )
    .unwrap()
    .layer(http_routing::middleware::from_fn(
        move |mut request: Request, next: http_routing::middleware::Next| {
            let token = token.clone();
            async move {
                request
                    .headers_mut()
                    .insert("authorization", format!("Bearer {token}").parse().unwrap());
                next.run(request).await
            }
        },
    ));
    (app, gate.clone(), Release(gate))
}
async fn request(app: Router, path: &str) -> Response {
    app.oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}
async fn overloaded(app: Router, path: &str) {
    let response = tokio::time::timeout(Duration::from_secs(5), request(app, path))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = http_routing::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    let body: Value = json_codec::from_slice(&body).unwrap();
    assert_eq!(body["error"]["code"], "RESOURCE_LIMIT_EXCEEDED");
    assert_eq!(body["error"]["retryable"], true);
}

#[tokio::test]
async fn backend_admission_saturates_without_waiters_and_survives_http_cancellation() {
    let root = crate::tests::private_tempdir();
    let (app, gate, _release) = fixture(root.path(), 0, false);
    let mut jobs = Vec::new();
    for _ in 0..HTTP_BACKEND_SLOTS {
        let app = app.clone();
        jobs.push(tokio::spawn(async move {
            request(app, "/api/repositories").await
        }));
    }
    tokio::time::timeout(
        Duration::from_secs(5),
        gate.started.acquire_many(HTTP_BACKEND_SLOTS as u32),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    overloaded(app.clone(), "/api/repositories").await;
    overloaded(app.clone(), "/api/workspaces/ws/files/item/upload-status").await;
    overloaded(app.clone(), "/api/repositories").await;
    assert_eq!(
        request(app.clone(), "/readyz").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        request(app.clone(), "/api/capabilities").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        request(app.clone(), "/healthz").await.status(),
        StatusCode::OK
    );
    assert_eq!(gate.calls.load(Ordering::SeqCst), HTTP_BACKEND_SLOTS);
    let cancelled = jobs.pop().unwrap();
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    overloaded(app.clone(), "/api/repositories").await;
    assert_eq!(gate.calls.load(Ordering::SeqCst), HTTP_BACKEND_SLOTS);
    gate.release();
    for job in jobs {
        assert_eq!(job.await.unwrap().status(), StatusCode::OK);
    }
    gate.finished
        .acquire_many(HTTP_BACKEND_SLOTS as u32)
        .await
        .unwrap()
        .forget();
    *gate.open.lock().unwrap() = false;
    // Refill the entire pool, proving even the cancelled request's job releases
    // its permit. Retry only until that job has actually returned, not by sleep.
    let mut jobs = Vec::new();
    for _ in 0..HTTP_BACKEND_SLOTS {
        let app = app.clone();
        jobs.push(tokio::spawn(async move {
            loop {
                let response = request(app.clone(), "/api/repositories").await;
                if response.status() != StatusCode::TOO_MANY_REQUESTS {
                    return response;
                }
                tokio::task::yield_now().await;
            }
        }));
    }
    tokio::time::timeout(
        Duration::from_secs(5),
        gate.started.acquire_many(HTTP_BACKEND_SLOTS as u32),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    overloaded(app, "/api/repositories").await;
    gate.release();
    for job in jobs {
        assert_eq!(job.await.unwrap().status(), StatusCode::OK);
    }
    assert_eq!(gate.calls.load(Ordering::SeqCst), HTTP_BACKEND_SLOTS * 2);
}

#[tokio::test]
async fn backend_admission_releases_after_success_error_and_panic() {
    for (outcome, status) in [
        (0, StatusCode::OK),
        (1, StatusCode::CONFLICT),
        (2, StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let root = crate::tests::private_tempdir();
        let (app, gate, _release) = fixture(root.path(), outcome, true);
        for _ in 0..HTTP_BACKEND_SLOTS * 2 {
            assert_eq!(
                request(app.clone(), "/api/repositories").await.status(),
                status
            );
        }
        assert_eq!(gate.calls.load(Ordering::SeqCst), HTTP_BACKEND_SLOTS * 2);
    }
}

// A single worker makes accidental synchronous lock waits observable. The
// holder has an independent wall-clock escape so regressions cannot hang CI.
#[tokio::test(flavor = "current_thread")]
async fn management_status_remains_live_during_authentication_contention() {
    let root = crate::tests::private_tempdir();
    let gate = Arc::new(Gate {
        open: Mutex::new(true),
        changed: Condvar::new(),
        started: tokio::sync::Semaphore::new(0),
        calls: AtomicUsize::new(0),
        finished: tokio::sync::Semaphore::new(0),
    });
    let backend = Arc::new(BackendProbe {
        blocked: BlockedEndpoint::Backend,
        gate,
        outcome: 0,
        auth: Mutex::new(None),
    });
    let auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let app = router(backend.clone(), auth, ServerConfig::loopback(root.path())).unwrap();
    let auth = backend.auth.lock().unwrap().clone().unwrap();
    let held = Arc::new(tokio::sync::Semaphore::new(0));
    let (release, wait) = std::sync::mpsc::channel();
    let holder = std::thread::spawn({
        let auth = auth.clone();
        let held = held.clone();
        move || {
            let _guard = auth.lock().unwrap();
            held.add_permits(1);
            let _ = wait.recv_timeout(Duration::from_secs(3));
        }
    });
    held.acquire().await.unwrap().forget();
    let status = tokio::spawn(request(app.clone(), "/api/management/status"));
    let health = tokio::spawn(request(app.clone(), "/healthz"));
    tokio::time::sleep(Duration::from_millis(20)).await;
    // Both requests and this independent timer progress before releasing auth.
    assert!(matches!(
        auth.try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    ));
    assert!(status.is_finished());
    assert!(health.is_finished());
    assert_eq!(health.await.unwrap().status(), StatusCode::OK);
    let response = status.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = http_routing::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    let value: Value = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(value["authentication"], "busy");
    release.send(()).unwrap();
    holder.join().unwrap();
    let response = request(app, "/api/management/status").await;
    let bytes = http_routing::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    let value: Value = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(value["authentication"], "ready");
}

#[tokio::test]
async fn upload_status_retains_read_admission_after_cancellation() {
    let root = crate::tests::private_tempdir();
    let (app, gate, _release) = fixture(root.path(), 0, false);
    let path = "/api/workspaces/ws/files/item/upload-status";
    let mut jobs = Vec::new();
    for _ in 0..2 {
        jobs.push(tokio::spawn(request(app.clone(), path)));
    }
    tokio::time::timeout(Duration::from_secs(5), gate.started.acquire_many(2))
        .await
        .unwrap()
        .unwrap()
        .forget();
    overloaded(app.clone(), path).await;
    let canceled = jobs.pop().unwrap();
    canceled.abort();
    assert!(canceled.await.unwrap_err().is_cancelled());
    overloaded(app.clone(), path).await;
    assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
    assert_eq!(request(app, "/healthz").await.status(), StatusCode::OK);
    gate.release();
    assert_eq!(jobs.pop().unwrap().await.unwrap().status(), StatusCode::OK);
    gate.finished.acquire_many(2).await.unwrap().forget();
    assert_eq!(
        gate.calls.load(Ordering::SeqCst),
        2,
        "one typed snapshot per request"
    );
}

async fn workspace_request(app: Router, path: &str) -> Response {
    let body = if path == "/api/workspaces" {
        json!({"branch_id":"branch"})
    } else if path.ends_with("/namespace-operations") {
        json!({"kind":"mkdir","path":"directory"})
    } else if path.ends_with("/commit") {
        json!({"workspace_id":"workspace","operation_id":"op_00000000000000000000000000000000","repository_id":"repo","branch_id":"branch","expected_head":"head","fencing_token":"1","message":"","requested_protection":"strict","file_placement":{},"expected_topology_generation":"1","sealed_manifest_hash":"manifest","request_digest":"digest"})
    } else {
        json!({})
    };
    app.oneshot(
        Request::builder()
            .method(
                if path == "/api/workspaces/workspace"
                    || path == "/api/versions/version"
                    || path.starts_with("/api/branches/branch/tree")
                    || path == "/api/repositories/repository"
                    || path == "/api/branches/branch"
                    || path == "/api/repositories/repository/branches"
                    || path.starts_with("/api/operations/")
                {
                    "GET"
                } else {
                    "POST"
                },
            )
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(json_codec::to_vec(&body).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn typed_workspace_jobs_retain_admission_until_blocking_completion_after_cancellation() {
    for path in [
        "/api/branches/branch/write-sessions",
        "/api/versions/version",
        "/api/branches/branch/tree?commit_id=commit",
        "/api/repositories/repository",
        "/api/branches/branch",
        "/api/repositories/repository/branches",
        "/api/operations/op_00000000000000000000000000000000",
        "/api/workspaces",
        "/api/workspaces/workspace/abort",
        "/api/workspaces/workspace/seal",
        "/api/workspaces/workspace/namespace-operations",
        "/api/workspaces/workspace/commit",
        "/api/workspaces/workspace",
    ] {
        let root = crate::tests::private_tempdir();
        let (app, gate, _release) = fixture(root.path(), 0, false);
        let mut jobs = Vec::new();
        for _ in 0..HTTP_BACKEND_SLOTS {
            jobs.push(tokio::spawn(workspace_request(app.clone(), path)));
        }
        tokio::time::timeout(
            Duration::from_secs(5),
            gate.started.acquire_many(HTTP_BACKEND_SLOTS as u32),
        )
        .await
        .unwrap()
        .unwrap()
        .forget();
        assert_eq!(
            workspace_request(app.clone(), path).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        let cancelled = jobs.pop().unwrap();
        cancelled.abort();
        assert!(cancelled.await.unwrap_err().is_cancelled());
        assert_eq!(
            workspace_request(app.clone(), path).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(gate.calls.load(Ordering::SeqCst), HTTP_BACKEND_SLOTS);
        gate.release();
        for job in jobs {
            assert_ne!(job.await.unwrap().status(), StatusCode::TOO_MANY_REQUESTS);
        }
        gate.finished
            .acquire_many(HTTP_BACKEND_SLOTS as u32)
            .await
            .unwrap()
            .forget();
        loop {
            let response = workspace_request(app.clone(), path).await;
            if response.status() != StatusCode::TOO_MANY_REQUESTS {
                break;
            }
            tokio::task::yield_now().await;
        }
    }
}

#[tokio::test]
async fn typed_management_jobs_hold_separate_slots_after_http_cancellation() {
    for (blocked, path) in [
        (BlockedEndpoint::Capabilities, "/api/capabilities"),
        (BlockedEndpoint::Ready, "/readyz"),
        (BlockedEndpoint::Capacity, "/api/workspaces/capacity"),
    ] {
        let root = crate::tests::private_tempdir();
        let (app, gate, _release) = fixture_for(root.path(), 0, false, blocked);
        let mut jobs = Vec::new();
        for _ in 0..2 {
            jobs.push(tokio::spawn(request(app.clone(), path)));
        }
        tokio::time::timeout(Duration::from_secs(5), gate.started.acquire_many(2))
            .await
            .unwrap()
            .unwrap()
            .forget();
        overloaded(app.clone(), path).await;
        let cancelled = jobs.pop().unwrap();
        cancelled.abort();
        assert!(cancelled.await.unwrap_err().is_cancelled());
        overloaded(app.clone(), path).await;
        assert_eq!(
            request(app.clone(), "/healthz").await.status(),
            StatusCode::OK
        );
        assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
        gate.release();
        assert_eq!(jobs.pop().unwrap().await.unwrap().status(), StatusCode::OK);
        gate.finished.acquire_many(2).await.unwrap().forget();
        loop {
            let response = request(app.clone(), path).await;
            if response.status() != StatusCode::TOO_MANY_REQUESTS {
                assert_eq!(response.status(), StatusCode::OK);
                break;
            }
            tokio::task::yield_now().await;
        }
    }
}

fn probe_capacity() -> storage_contract::WorkspaceCapacity {
    let measure = storage_contract::CapacityMeasure {
        used: 1,
        limit: 8,
        remaining: 7,
    };
    let scope = storage_contract::WorkspaceCapacityScope {
        records: measure.clone(),
        metadata_bytes: measure,
    };
    storage_contract::WorkspaceCapacity {
        global: scope.clone(),
        principal: scope,
        warning: storage_contract::CapacityWarning::Ok,
        automatic_retirement: false,
    }
}
