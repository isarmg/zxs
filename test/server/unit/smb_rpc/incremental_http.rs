//! Real loopback HTTP, scoped to the ordinary incremental acceptance test.
use crate::{ServerConfig, auth::AuthStore, engine_backend::EngineBackend};
use json_codec::{Value, json};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use storage_engine::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) struct Http {
    runtime: tokio::runtime::Runtime,
    address: SocketAddr,
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
    token: String,
}

impl Http {
    pub(super) fn start(system: &Path, engine: Engine, password: &str) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (address, stop, task) = runtime.block_on(async {
            let backend = Arc::new(EngineBackend::new(engine));
            let auth = backend
                .protect_auth(AuthStore::open(system).unwrap())
                .unwrap();
            let app = crate::router(backend, auth, ServerConfig::new(system)).unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (stop, stopped) = tokio::sync::oneshot::channel();
            let task = tokio::spawn(async move {
                http_routing::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = stopped.await;
                    })
                    .await
                    .unwrap();
            });
            (address, stop, task)
        });
        let mut http = Self {
            runtime,
            address,
            stop,
            task,
            token: String::new(),
        };
        let login = http.json(
            "POST",
            "/api/auth/login",
            Some(json!({
                "username": "alice", "password": password,
            })),
        );
        http.token = login["token"].as_str().unwrap().to_owned();
        http
    }

    fn raw(&self, method: &str, route: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let (status, _, body) = self.request(method, route, body, "");
        (status, body)
    }

    fn request(&self, method: &str, route: &str, body: &[u8], headers: &str)
        -> (u16, String, Vec<u8>)
    {
        self.runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(30), async {
                let mut connection = tokio::net::TcpStream::connect(self.address).await.unwrap();
                let mut head = format!(
                    "{method} {route} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
                    self.address,
                );
                if !self.token.is_empty() {
                    head.push_str(&format!("Authorization: Bearer {}\r\n", self.token));
                }
                head.push_str(headers);
                head.push_str(&format!(
                    "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    body.len(),
                ));
                connection.write_all(head.as_bytes()).await.unwrap();
                connection.write_all(body).await.unwrap();
                let mut response = Vec::new();
                connection
                    .take(2 * 1024 * 1024)
                    .read_to_end(&mut response)
                    .await
                    .unwrap();
                assert!(response.len() < 2 * 1024 * 1024);
                let split = response
                    .windows(4)
                    .position(|bytes| bytes == b"\r\n\r\n")
                    .unwrap();
                let header = std::str::from_utf8(&response[..split]).unwrap();
                assert!(!header.to_ascii_lowercase().contains("transfer-encoding:"));
                let status = header.split_whitespace().nth(1).unwrap().parse().unwrap();
                (status, header.to_owned(), response[split + 4..].to_vec())
            })
            .await
            .expect("ordinary incremental HTTP exceeded its request deadline")
        })
    }

    pub(super) fn json(&self, method: &str, route: &str, body: Option<Value>) -> Value {
        let body = body.map_or_else(Vec::new, |body| json_codec::to_vec(&body).unwrap());
        let (status, bytes) = self.raw(method, route, &body);
        let value: Value = json_codec::from_slice(&bytes).unwrap();
        assert_eq!(status, 200, "{method} {route}: {value}");
        value
    }

    pub(super) fn bytes(&self, route: &str) -> Vec<u8> {
        let (status, bytes) = self.raw("GET", route, &[]);
        assert_eq!(
            status,
            200,
            "GET {route}: {}",
            String::from_utf8_lossy(&bytes)
        );
        bytes
    }

    pub(super) fn range(
        &self, route: &str, start: usize, end: usize, size: usize,
    ) -> (String, Vec<u8>) {
        let (status, headers, body) = self.request("GET", route, &[],
            &format!("Range: bytes={start}-{end}\r\n"));
        assert_eq!(status, 206);
        let value = |name: &str| headers.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim().to_owned())
        }).unwrap();
        assert_eq!(value("Content-Range"), format!("bytes {start}-{end}/{size}"));
        assert_eq!(value("Content-Length"), (end - start + 1).to_string());
        (value("ETag"), body)
    }

    pub(super) fn cold(self, system: &Path) -> Engine {
        let Self {
            runtime,
            address: _,
            stop,
            task,
            token: _,
        } = self;
        stop.send(()).unwrap();
        runtime.block_on(task).unwrap();
        let engine = runtime.block_on(async {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match Engine::open(system) {
                    Ok(engine) => break engine,
                    Err(error)
                        if error.code() == storage_contract::ErrorCode::WriterAlreadyActive =>
                    {
                        let left = deadline.saturating_duration_since(Instant::now());
                        assert!(!left.is_zero(), "HTTP retained its writer: {error}");
                        tokio::time::sleep(left.min(Duration::from_millis(10))).await;
                    }
                    Err(error) => panic!("incremental draft cold open failed: {error}"),
                }
            }
        });
        runtime.shutdown_timeout(Duration::from_secs(3));
        engine
    }
}
