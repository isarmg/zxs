use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

struct TestServer {
    url: String,
    requests: Arc<AtomicUsize>,
    stop: oneshot::Sender<()>,
    worker: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&requests);
        let (stop, mut stopped) = oneshot::channel();
        let worker = tokio::spawn(async move {
            loop {
                let (mut stream, _) = tokio::select! {
                    _ = &mut stopped => return,
                    connection = listener.accept() => connection.unwrap(),
                };
                let mut received = Vec::new();
                let mut buffer = [0; 1024];
                loop {
                    let read = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(read, 0, "request ended before its headers");
                    received.extend_from_slice(&buffer[..read]);
                    assert!(received.len() <= 4096, "unexpected test request size");
                    if received.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                        break;
                    }
                }
                count.fetch_add(1, Ordering::SeqCst);
                stream
                    .write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
            }
        });
        Self {
            url,
            requests,
            stop,
            worker,
        }
    }

    async fn finish(self) -> usize {
        self.stop.send(()).unwrap();
        self.worker.await.unwrap();
        self.requests.load(Ordering::SeqCst)
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn retry_scope_excludes_other_hosts_before_classification() {
    runtime().block_on(async {
        let server = TestServer::start().await;
        let classified = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&classified);
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .retry(
                reqwest::retry::for_host("in-scope.invalid")
                    .no_budget()
                    .classify_fn(move |response| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        response.retryable()
                    }),
            )
            .build()
            .unwrap();
        let response = client.get(&server.url).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(server.finish().await, 1, "out-of-scope request was replayed");
        assert_eq!(classified.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn retry_scope_obeys_the_per_request_replay_limit() {
    runtime().block_on(async {
        for max_retries in [0, 1, 2] {
            let server = TestServer::start().await;
            let client = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .retry(
                    reqwest::retry::for_host("127.0.0.1")
                        .no_budget()
                        .max_retries_per_request(max_retries)
                        .classify_fn(|response| response.retryable()),
                )
                .build()
                .unwrap();
            let response = client.get(&server.url).send().await.unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(server.finish().await, max_retries as usize + 1);
        }
    });
}

#[test]
fn retry_scope_keeps_streaming_bodies_non_replayable() {
    runtime().block_on(async {
        let server = TestServer::start().await;
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .retry(
                reqwest::retry::for_host("127.0.0.1")
                    .no_budget()
                    .classify_fn(|response| response.retryable()),
            )
            .build()
            .unwrap();
        let body = reqwest::Body::wrap(reqwest::Body::from("payload"));
        let response = client.post(&server.url).body(body).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(server.finish().await, 1);
    });
}
