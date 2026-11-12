//! Real HTTP admission and session revocation, before any business mutation.
//!
//! The existing backend probe counts dispatches and returns an injected error.
//! These tests do not claim Engine publication, upload durability or recovery.
use super::*;
use futures::StreamExt;
use http_routing::body::Bytes;
use tokio::sync::oneshot;

#[derive(Clone, Copy)]
enum WriteRoute {
    FileTags,
    DiskWriteSession,
    WorkspaceCommit,
}

impl WriteRoute {
    fn path(self) -> &'static str {
        match self {
            Self::FileTags => {
                "/api/file-manager/files/file_00000000000000000000000000000001/tags"
            }
            Self::DiskWriteSession => "/api/file-manager/disks/P01/write-sessions",
            Self::WorkspaceCommit => "/api/workspaces/workspace/commit",
        }
    }

    fn body(self) -> Vec<u8> {
        let body = match self {
            Self::FileTags => json!({
                "repository_id": "repo_00000000000000000000000000000001",
                "path": "file.txt",
                "expected_generation": "0",
                "tags": ["reviewed"],
                "operation_id": "op_00000000000000000000000000000001"
            }),
            Self::DiskWriteSession | Self::WorkspaceCommit => workspace_body("POST", self.path()),
        };
        json_codec::to_vec(&body).unwrap()
    }

    fn request(self, token: &str, body: Body) -> Request {
        Request::builder()
            .method("POST")
            .uri(self.path())
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(body)
            .unwrap()
    }
}

struct WriteFixture {
    app: Router,
    auth: Arc<Mutex<AuthStore>>,
    gate: Arc<Gate>,
    token: String,
}

impl WriteFixture {
    fn new(root: &Path) -> Self {
        let gate = Arc::new(Gate {
            open: Mutex::new(true),
            changed: Condvar::new(),
            started: tokio::sync::Semaphore::new(0),
            calls: AtomicUsize::new(0),
            finished: tokio::sync::Semaphore::new(0),
            cancellations: Mutex::new(Vec::new()),
        });
        let backend = Arc::new(BackendProbe {
            blocked: BlockedEndpoint::Backend,
            gate: gate.clone(),
            outcome: 1,
            auth: Mutex::new(None),
        });
        let mut auth = AuthStore::initialize(root, "admin", "a-long-test-password").unwrap();
        let token = auth.login("admin", "a-long-test-password").unwrap().token;
        let app = router(backend.clone(), auth, ServerConfig::new(root)).unwrap();
        let auth = backend.auth.lock().unwrap().clone().unwrap();
        Self {
            app,
            auth,
            gate,
            token,
        }
    }

    fn calls(&self) -> usize {
        self.gate.calls.load(Ordering::SeqCst)
    }

    async fn admitted_control(&self, route: WriteRoute, token: &str) {
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            self.app
                .clone()
                .oneshot(route.request(token, Body::from(route.body()))),
        )
        .await
        .unwrap()
        .unwrap();
        assert_error(response, StatusCode::CONFLICT, "INJECTED_FAILURE").await;
        assert_eq!(self.calls(), 1, "a valid retry must reach the backend once");
    }
}

// Reading the first JSON byte and polling the remainder proves the request has
// crossed initial HTTP authentication. There are no timers or spawned request
// tasks controlling the interleaving. Dropping the request drops its receiver.
fn paused_body(body: Vec<u8>) -> (Body, oneshot::Receiver<()>, oneshot::Sender<()>) {
    assert_eq!(body.first(), Some(&b'{'));
    let (entered, waiting) = oneshot::channel();
    let (resume, resumed) = oneshot::channel();
    let prefix = futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"{"))]);
    let remainder = futures::stream::once(async move {
        entered.send(()).unwrap();
        resumed.await.unwrap();
        Ok::<_, std::io::Error>(Bytes::copy_from_slice(&body[1..]))
    });
    (Body::from_stream(prefix.chain(remainder)), waiting, resume)
}

async fn assert_error(response: Response, status: StatusCode, code: &str) {
    assert_eq!(response.status(), status);
    let bytes = http_routing::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    let error: Value = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(error["error"]["code"], code);
}

async fn revoke_during_body(route: WriteRoute) {
    let root = crate::tests::private_tempdir();
    let fixture = WriteFixture::new(root.path());
    let (body, waiting, resume) = paused_body(route.body());
    let request = fixture
        .app
        .clone()
        .oneshot(route.request(&fixture.token, body));
    tokio::pin!(request);
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! {
            entered = waiting => entered.unwrap(),
            response = &mut request => panic!("request completed before body: {response:?}"),
        }
    })
    .await
    .unwrap();
    assert_eq!(fixture.calls(), 0);

    fixture.auth.lock().unwrap().logout(&fixture.token).unwrap();
    resume.send(()).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .unwrap()
        .unwrap();
    assert_error(response, StatusCode::UNAUTHORIZED, "UNAUTHORIZED").await;
    assert_eq!(fixture.calls(), 0, "revoked identity reached the backend");

    // Exact same route/body is admitted with a new session. Rejection must not
    // be a parser failure, a missing route or a permanently consumed slot.
    let fresh = fixture
        .auth
        .lock()
        .unwrap()
        .login("admin", "a-long-test-password")
        .unwrap()
        .token;
    fixture.admitted_control(route, &fresh).await;
}

async fn cancel_during_body(route: WriteRoute) {
    let root = crate::tests::private_tempdir();
    let fixture = WriteFixture::new(root.path());
    let (body, waiting, resume) = paused_body(route.body());
    {
        let request = fixture
            .app
            .clone()
            .oneshot(route.request(&fixture.token, body));
        tokio::pin!(request);
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                entered = waiting => entered.unwrap(),
                response = &mut request => panic!("request completed before body: {response:?}"),
            }
        })
        .await
        .unwrap();
        assert_eq!(fixture.calls(), 0);
    }
    assert!(resume.send(()).is_err(), "canceled body is still owned");
    assert_eq!(fixture.calls(), 0, "canceled request dispatched a mutation");
    fixture.admitted_control(route, &fixture.token).await;
}

async fn failed_body(route: WriteRoute) {
    let root = crate::tests::private_tempdir();
    let fixture = WriteFixture::new(root.path());
    // A complete-looking JSON prefix is not an admitted request if receiving
    // the rest of its transport fails. Nothing may dispatch from that prefix.
    let interrupted = futures::stream::iter([
        Ok(Bytes::from(route.body())),
        Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "test request stream interrupted",
        )),
    ]);
    let cases = [
        (
            Body::from_stream(interrupted),
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
        ),
        (
            Body::from(vec![b' '; crate::JSON_BODY_LIMIT + 1]),
            StatusCode::PAYLOAD_TOO_LARGE,
            "PAYLOAD_TOO_LARGE",
        ),
    ];
    for (body, status, code) in cases {
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            fixture
                .app
                .clone()
                .oneshot(route.request(&fixture.token, body)),
        )
        .await
        .unwrap()
        .unwrap();
        assert_error(response, status, code).await;
        assert_eq!(fixture.calls(), 0, "failed body dispatched a mutation");
    }
    fixture.admitted_control(route, &fixture.token).await;
}

#[tokio::test]
async fn file_tags_revalidate_session_after_streamed_json() {
    revoke_during_body(WriteRoute::FileTags).await;
}

#[tokio::test]
async fn disk_write_session_revalidates_session_after_streamed_json() {
    revoke_during_body(WriteRoute::DiskWriteSession).await;
}

#[tokio::test]
async fn workspace_commit_revalidates_session_after_streamed_json() {
    revoke_during_body(WriteRoute::WorkspaceCommit).await;
}

#[tokio::test]
async fn canceled_tag_body_never_dispatches_and_allows_retry() {
    cancel_during_body(WriteRoute::FileTags).await;
}

#[tokio::test]
async fn canceled_write_session_body_never_dispatches_and_allows_retry() {
    cancel_during_body(WriteRoute::DiskWriteSession).await;
}

#[tokio::test]
async fn canceled_commit_body_never_dispatches_and_allows_retry() {
    cancel_during_body(WriteRoute::WorkspaceCommit).await;
}

#[tokio::test]
async fn interrupted_or_oversized_tag_body_does_not_dispatch() {
    failed_body(WriteRoute::FileTags).await;
}

#[tokio::test]
async fn interrupted_or_oversized_write_session_body_does_not_dispatch() {
    failed_body(WriteRoute::DiskWriteSession).await;
}

#[tokio::test]
async fn interrupted_or_oversized_commit_body_does_not_dispatch() {
    failed_body(WriteRoute::WorkspaceCommit).await;
}
