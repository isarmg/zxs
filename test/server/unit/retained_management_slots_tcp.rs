//! Loopback transport coverage; the parent module separately drops router futures.
use super::*;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

struct Server {
    address: SocketAddr,
    stopped: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Server {
    async fn start(app: Router) -> Self {
        let listener = tokio::time::timeout(WAIT, TcpListener::bind("127.0.0.1:0"))
            .await
            .expect("loopback bind did not finish")
            .unwrap();
        let address = listener.local_addr().unwrap();
        let (stopped, shutdown) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            http_routing::serve(listener, app)
                .with_graceful_shutdown(async {
                    // Dropping the guard also requests shutdown.
                    match shutdown.await {
                        Ok(()) | Err(_) => {}
                    }
                })
                .await
        });
        Self {
            address,
            stopped: Some(stopped),
            task,
        }
    }

    async fn finish(mut self) {
        self.stopped.take().unwrap().send(()).unwrap();
        tokio::time::timeout(WAIT, &mut self.task)
            .await
            .expect("loopback server did not drain")
            .unwrap()
            .unwrap();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stopped) = self.stopped.take() {
            // An already-closed receiver needs no further shutdown notification.
            match stopped.send(()) {
                Ok(()) | Err(()) => {}
            }
        }
        self.task.abort();
    }
}

struct Client {
    address: SocketAddr,
    token: String,
}

impl Client {
    async fn send(&self, path: &str, token: Option<&str>) -> TcpStream {
        tokio::time::timeout(WAIT, async {
            let mut stream = TcpStream::connect(self.address).await.unwrap();
            let mut request = format!(
                "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
                self.address
            );
            if let Some(token) = token {
                request.push_str(&format!("Authorization: Bearer {token}\r\n"));
            }
            request.push_str("\r\n");
            stream.write_all(request.as_bytes()).await.unwrap();
            stream
        })
        .await
        .expect("loopback request did not finish sending")
    }

    async fn get(&self, path: &str) -> TcpResponse {
        receive(self.send(path, Some(&self.token)).await).await
    }
}

struct TcpResponse {
    status: u16,
    body: Vec<u8>,
}

async fn receive(stream: TcpStream) -> TcpResponse {
    // All selected routes return small, buffered JSON or health responses.
    const MAX_RESPONSE: usize = 16 * 1024;
    let mut bytes = Vec::new();
    tokio::time::timeout(
        WAIT,
        stream
            .take((MAX_RESPONSE + 1) as u64)
            .read_to_end(&mut bytes),
    )
    .await
    .expect("loopback response did not finish")
    .unwrap();
    assert!(bytes.len() <= MAX_RESPONSE, "oversized loopback response");
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP response lost its header boundary");
    let mut headers = std::str::from_utf8(&bytes[..split]).unwrap().split("\r\n");
    let mut status_line = headers.next().unwrap().splitn(3, ' ');
    assert_eq!(status_line.next(), Some("HTTP/1.1"));
    let status = status_line.next().unwrap().parse().unwrap();
    let mut length = None;
    for header in headers {
        let (name, value) = header.split_once(':').unwrap();
        assert!(
            !name.eq_ignore_ascii_case("transfer-encoding"),
            "buffered test route unexpectedly used transfer encoding"
        );
        if name.eq_ignore_ascii_case("content-length") {
            assert!(
                length
                    .replace(value.trim().parse::<usize>().unwrap())
                    .is_none()
            );
        }
    }
    let body = bytes[split + 4..].to_vec();
    assert_eq!(length, Some(body.len()), "HTTP response body was truncated");
    TcpResponse { status, body }
}

fn fixture(root: &Path) -> (Router, Arc<Gate>, Release, String) {
    let gate = Arc::new(Gate {
        open: Mutex::new(false),
        changed: Condvar::new(),
        started: tokio::sync::Semaphore::new(0),
        calls: AtomicUsize::new(0),
        finished: tokio::sync::Semaphore::new(0),
        cancellations: Mutex::new(Vec::new()),
    });
    let mut auth = AuthStore::initialize(root, "admin", "a-long-test-password").unwrap();
    let token = auth.login("admin", "a-long-test-password").unwrap().token;
    // No middleware injects credentials: they must cross the actual TCP boundary.
    let app = router(
        Arc::new(BackendProbe {
            blocked: BlockedEndpoint::Capacity,
            gate: Arc::clone(&gate),
            outcome: 0,
            auth: Mutex::new(None),
        }),
        auth,
        ServerConfig::new(root),
    )
    .unwrap();
    (app, Arc::clone(&gate), Release(gate), token)
}

async fn start_then_end_tcp_wait(client: &Client, gate: &Gate, end: EndWait) {
    let stream = client.send(CAPACITY, Some(&client.token)).await;
    // This future owns the socket, so ending the wait closes the connection.
    let mut response = Box::pin(receive(stream));
    tokio::time::timeout(WAIT, async {
        tokio::select! {
            started = gate.started.acquire() => started.unwrap().forget(),
            response = &mut response => panic!(
                "blocked backend returned TCP HTTP {} before cancellation", response.status
            ),
        }
    })
    .await
    .expect("TCP backend did not enter its gate");
    match end {
        EndWait::Drop => drop(response),
        EndWait::Timeout => {
            assert!(
                tokio::time::timeout(Duration::ZERO, response)
                    .await
                    .is_err()
            );
        }
    }
}

async fn assert_busy_routes(client: &Client, gate: &Gate) {
    let mut request_ids = std::collections::BTreeSet::new();
    for (path, message) in [
        (CAPACITY, "management capacity reached"),
        ("/api/capabilities", "management capacity reached"),
        ("/readyz", "management capacity reached"),
        (
            "/api/admin/storage/discovered-disks",
            "management operation capacity reached",
        ),
        (
            "/api/admin/disks/registrations",
            "management operation capacity reached",
        ),
    ] {
        let response = client.get(path).await;
        assert_eq!(response.status, 429, "{path}");
        let error =
            storage_contract::client_protocol::decode_response(429, &response.body).unwrap_err();
        assert_eq!(
            error.code,
            storage_contract::ErrorCode::ResourceLimitExceeded
        );
        assert_eq!(error.status, 429);
        assert_eq!(error.message, message, "{path}");
        assert!(error.retryable);
        let id = error.request_id.expect("TCP overload lost its request ID");
        assert!(!id.is_empty());
        assert!(request_ids.insert(id), "TCP rejections reused a request ID");
        assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
        assert_eq!(gate.finished.available_permits(), 0);
    }
    assert_eq!(client.get("/healthz").await.status, 200);
}

async fn assert_recovery(client: &Client, gate: &Gate) {
    gate.release();
    tokio::time::timeout(WAIT, gate.finished.acquire_many(2))
        .await
        .expect("disconnected background workers failed to finish")
        .unwrap()
        .forget();
    // The completion notification precedes permit Drop; only this explicit
    // post-release phase permits transient rejection while reclamation finishes.
    let capacity = tokio::time::timeout(WAIT, async {
        loop {
            let response = client.get(CAPACITY).await;
            match response.status {
                429 => tokio::task::yield_now().await,
                200 => {
                    break json_codec::from_slice::<storage_contract::WorkspaceCapacity>(
                        &response.body,
                    )
                    .unwrap();
                }
                status => panic!("released TCP backend returned unexpected HTTP {status}"),
            }
        }
    })
    .await
    .expect("TCP management capacity did not recover");
    assert_eq!(capacity, probe_capacity());
    assert_eq!(gate.calls.load(Ordering::SeqCst), 3);
}

async fn assert_retained_tcp_slots(end: EndWait) {
    let root = crate::tests::private_tempdir();
    let (app, gate, _release, token) = fixture(root.path());
    let server = Server::start(app).await;
    let client = Client {
        address: server.address,
        token,
    };
    let unauthenticated = receive(client.send(CAPACITY, None).await).await;
    assert_eq!(unauthenticated.status, 401);
    let error =
        storage_contract::client_protocol::decode_response(401, &unauthenticated.body).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::Unauthorized);
    assert_eq!(gate.calls.load(Ordering::SeqCst), 0);
    for expected_calls in 1..=2 {
        start_then_end_tcp_wait(&client, &gate, end).await;
        assert_eq!(gate.calls.load(Ordering::SeqCst), expected_calls);
        assert_eq!(gate.finished.available_permits(), 0);
    }
    assert_busy_routes(&client, &gate).await;
    assert_recovery(&client, &gate).await;
    server.finish().await;
}

#[tokio::test]
async fn two_disconnected_tcp_clients_keep_management_slots_until_workers_finish() {
    assert_retained_tcp_slots(EndWait::Drop).await;
}

#[tokio::test]
async fn two_timed_out_tcp_clients_keep_management_slots_until_workers_finish() {
    assert_retained_tcp_slots(EndWait::Timeout).await;
}
