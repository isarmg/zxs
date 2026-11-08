//! Real TCP HTTP, real management authority and real independent-disk verifier.
use json_codec::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    net::SocketAddr,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    time::Duration,
};
use storage_contract::{
    storage_directories::{DirectorySource, DiscoveredDiskState, DiscoveredDisks},
    storage_independent_topology::{IndependentDiskPreview, IndependentDiskPreviewRequest},
};
use storage_engine::disk_authority::{
    Digest, FileWrite, Identity, IndependentDisk, Origin, Role, Token,
};
use storage_server::{
    Backend, ServerConfig, StorageState,
    management::{ManagementOptions, open_service_with_options},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ENDPOINT: &str = "/api/admin/storage/independent/preview";
const PASSWORD: &str = "independent-http-test-password";

fn independent(path: &Path) {
    let token = |value| Token::new(value).unwrap();
    let disk = IndependentDisk::initialize(
        path,
        Identity {
            origin: Origin {
                pool_id: token("origin_pool"),
                disk_id: token("origin_disk"),
                slot_id: token("origin_slot"),
                binding_generation: 1,
            },
            incarnation: Digest::body(b"independent"),
            role: Role::Primary,
        },
    )
    .unwrap();
    let review = disk
        .review_file(FileWrite {
            operation: storage_contract::OperationId::generate().unwrap(),
            expected_base: None,
            history: &token("history"),
            file: &token("file"),
            bytes: b"retained content",
        })
        .unwrap();
    disk.execute(&review).unwrap();
}

fn bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(root: &Path, current: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(current).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                collect(root, &entry.path(), result);
            } else {
                result.insert(
                    entry.path().strip_prefix(root).unwrap().into(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    collect(root, root, &mut result);
    result
}

async fn request(
    address: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (u16, Value) {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
        if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("origin")) {
            head.push_str(&format!("Origin: http://{address}\r\n"));
        }
        for (name, value) in headers { head.push_str(&format!("{name}: {value}\r\n")); }
        head.push_str("\r\n"); stream.write_all(head.as_bytes()).await.unwrap(); stream.write_all(body).await.unwrap();
        let mut response = Vec::new(); stream.take(1024 * 1024 + 4096).read_to_end(&mut response).await.unwrap();
        assert!(response.len() < 1024 * 1024 + 4096);
        let boundary = response.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap();
        let header = std::str::from_utf8(&response[..boundary]).unwrap();
        let status = header.split_whitespace().nth(1).unwrap().parse().unwrap();
        let body = &response[boundary + 4..];
        (status, if body.is_empty() { Value::Null } else { json_codec::from_slice(body).unwrap() })
    }).await.expect("HTTP request must finish within its test deadline")
}

struct Fixture {
    _root: tempfile::TempDir,
    source: PathBuf,
    known: PathBuf,
    address: SocketAddr,
    token: String,
    csrf: String,
    stop: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl Fixture {
    async fn start() -> Self {
        let root = tempfile::tempdir().unwrap();
        let discovery = root.path().join("disks");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&discovery)
            .unwrap();
        let data = root.path().join("system");
        let management = data.join("management");
        let known = discovery.join("known");
        fs::DirBuilder::new().mode(0o700).create(&known).unwrap();
        let source = discovery.join("independent");
        independent(&source);
        let (backend, auth) = open_service_with_options(
            &data,
            &management,
            ManagementOptions {
                discovery_root: discovery.to_str().unwrap().parse().unwrap(),
                allow_development_setup: true,
                ..Default::default()
            },
        )
        .unwrap();
        let app =
            storage_server::router(backend.clone(), auth, ServerConfig::new(&management)).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            http_routing::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
        });
        let (status, initialized) = request(
            address,
            "POST",
            "/api/setup/initialize",
            &[],
            &json_codec::to_vec(&json!({"username":"admin","password":PASSWORD})).unwrap(),
        )
        .await;
        assert_eq!(status, 200, "{initialized}");
        let (status, login) = request(
            address,
            "POST",
            "/api/auth/login",
            &[],
            &json_codec::to_vec(&json!({"username":"admin","password":PASSWORD})).unwrap(),
        )
        .await;
        assert_eq!(status, 200, "{login}");
        let bearer = format!("Bearer {}", login["token"].as_str().unwrap());
        let (status, discovered) = request(
            address,
            "GET",
            "/api/admin/storage/discovered-disks",
            &[("authorization", &bearer)],
            &[],
        )
        .await;
        assert_eq!(status, 200, "{discovered}");
        let discovered: DiscoveredDisks = json_codec::from_value(discovered).unwrap();
        let candidate = discovered
            .candidates
            .into_iter()
            .find(|candidate| Path::new(&candidate.path) == known)
            .unwrap();
        let DiscoveredDiskState::Empty { selection } = candidate.state else {
            panic!("fixture target must be empty")
        };
        let (status, activated) = request(
            address,
            "POST",
            "/api/admin/storage/activate",
            &[("authorization", &bearer)],
            &json_codec::to_vec(&json!({"selections":[selection],"mode":"development"})).unwrap(),
        )
        .await;
        assert_eq!(status, 200, "{activated}");
        tokio::time::timeout(Duration::from_secs(10), async {
            while backend.management_status().state() != StorageState::Ready {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the initialized fixture pool must reopen");
        Self {
            _root: root,
            source,
            known,
            address,
            token: login["token"].as_str().unwrap().into(),
            csrf: login["csrf_token"].as_str().unwrap().into(),
            stop,
            server,
        }
    }
    async fn observed(&self) -> DirectorySource {
        let bearer = format!("Bearer {}", self.token);
        let (status, value) = request(
            self.address,
            "GET",
            "/api/admin/storage/discovered-disks",
            &[("authorization", &bearer)],
            &[],
        )
        .await;
        assert_eq!(status, 200, "{value}");
        let disks: DiscoveredDisks = json_codec::from_value(value).unwrap();
        let candidate = disks
            .candidates
            .into_iter()
            .find(|candidate| Path::new(&candidate.path) == self.source)
            .unwrap();
        let DiscoveredDiskState::Unsupported {
            source: Some(source),
            ..
        } = candidate.state
        else {
            panic!("independent format must not become managed registration authority")
        };
        source
    }
    async fn preview(&self, source: DirectorySource) -> (u16, Value) {
        let bearer = format!("Bearer {}", self.token);
        request(
            self.address,
            "POST",
            ENDPOINT,
            &[("authorization", &bearer)],
            &json_codec::to_vec(&IndependentDiskPreviewRequest { source }).unwrap(),
        )
        .await
    }
    async fn finish(self) {
        self.stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), self.server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn independent_preview_over_tcp_reads_metadata_without_body_bytes_or_writes() {
    let f = Fixture::start().await;
    for entry in fs::read_dir(f.source.join("bodies")).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    let source = f.observed().await;
    let before = bytes(&f.source);
    let known = bytes(&f.known);
    let (status, value) = f.preview(source.clone()).await;
    assert_eq!(status, 200, "{value}");
    let output: IndependentDiskPreview = json_codec::from_value(value).unwrap();
    assert_eq!(output.source, source);
    assert_eq!(output.topology.root().sequence, 1);
    assert_eq!(output.topology.nodes().len(), 1);
    assert_eq!(output.inventory.root(), output.topology.root());
    assert_eq!(output.inventory.versions().len(), 1);
    assert_eq!(
        output.inventory.versions()[0].history_id.as_str(),
        "history"
    );
    assert_eq!(output.inventory.versions()[0].file_id.as_str(), "file");
    assert_eq!(
        output.inventory.versions()[0].owner,
        output.topology.root().identity
    );
    assert!(output.topology.relations().is_empty());
    assert_eq!(bytes(&f.source), before);
    assert_eq!(bytes(&f.known), known);
    f.finish().await;
}

#[tokio::test]
async fn independent_preview_over_tcp_preserves_auth_csrf_and_request_boundaries() {
    let f = Fixture::start().await;
    let source = f.observed().await;
    let before = bytes(&f.source);
    let known = bytes(&f.known);
    let input = json_codec::to_vec(&IndependentDiskPreviewRequest { source }).unwrap();
    let bearer = format!("Bearer {}", f.token);
    let authorized = [("authorization", bearer.as_str())];
    let cookie = format!("zxs_session={}", f.token);
    assert_eq!(
        request(f.address, "POST", ENDPOINT, &[], b"not json")
            .await
            .0,
        401
    );
    assert_eq!(
        request(f.address, "POST", ENDPOINT, &[("cookie", &cookie)], &input)
            .await
            .0,
        403
    );
    let browser = [
        ("cookie", cookie.as_str()),
        ("x-csrf-token", f.csrf.as_str()),
    ];
    assert_eq!(
        request(f.address, "POST", ENDPOINT, &browser, &input)
            .await
            .0,
        200
    );
    assert_eq!(
        request(
            f.address,
            "POST",
            ENDPOINT,
            &[
                ("authorization", &bearer),
                ("origin", "https://untrusted.example")
            ],
            &input
        )
        .await
        .0,
        403
    );
    assert_eq!(
        request(
            f.address,
            "POST",
            &format!("{ENDPOINT}?execute=true"),
            &authorized,
            &input
        )
        .await
        .0,
        400
    );
    let mut unknown: Value = json_codec::from_slice(&input).unwrap();
    unknown["execute"] = true.into();
    for invalid in [
        json_codec::to_vec(&unknown).unwrap(),
        b"{\"source\":null,\"source\":null}".to_vec(),
    ] {
        assert_eq!(
            request(f.address, "POST", ENDPOINT, &authorized, &invalid)
                .await
                .0,
            400
        );
    }
    assert_eq!(
        request(
            f.address,
            "POST",
            ENDPOINT,
            &authorized,
            &vec![b' '; 64 * 1024 + 1]
        )
        .await
        .0,
        413
    );
    assert_eq!(
        request(f.address, "POST", "/api/auth/logout", &authorized, b"{}")
            .await
            .0,
        200
    );
    assert_eq!(
        request(f.address, "POST", ENDPOINT, &authorized, &input)
            .await
            .0,
        401
    );
    assert_eq!(bytes(&f.source), before);
    assert_eq!(bytes(&f.known), known);
    f.finish().await;
}

#[tokio::test]
async fn independent_preview_over_tcp_rejects_replaced_out_of_scope_and_corrupt_sources() {
    let f = Fixture::start().await;
    let source = f.observed().await;
    let original = bytes(&f.source);
    let moved = f.source.with_extension("moved");
    fs::rename(&f.source, &moved).unwrap();
    independent(&f.source);
    let replacement = bytes(&f.source);
    let (status, error) = f.preview(source).await;
    assert_eq!(status, 409, "{error}");
    assert_eq!(error["error"]["code"], "DISK_CHANGED", "{error}");
    assert_eq!(bytes(&moved), original);
    assert_eq!(bytes(&f.source), replacement);
    let source = f.observed().await;
    for (path, status, code) in [
        (f.known.as_path(), 409, "CONFIG_CONFLICT"),
        (f.source.join("objects").as_path(), 400, "UNSAFE_PATH"),
        (f._root.path(), 409, "CONFIG_CONFLICT"),
    ] {
        let mut outside = source.clone();
        outside.path = path.to_string_lossy().into_owned();
        let (actual, error) = f.preview(outside).await;
        assert_eq!(actual, status, "{error}");
        assert_eq!(error["error"]["code"], code, "{error}");
    }
    let metadata = fs::read_dir(f.source.join("objects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(metadata, b"corrupt metadata").unwrap();
    let corrupted = bytes(&f.source);
    let (status, error) = f.preview(source).await;
    assert_eq!(status, 409, "{error}");
    assert_eq!(error["error"]["code"], "RECOVERY_REQUIRED", "{error}");
    assert_eq!(
        error["error"]["message"],
        "independent disk source inspection failed"
    );
    assert_eq!(bytes(&f.source), corrupted);
    assert_eq!(bytes(&moved), original);
    f.finish().await;
}

#[tokio::test]
async fn independent_preview_after_rediscovery_of_moved_disk_retains_portable_history() {
    let mut f = Fixture::start().await;
    let old_source = f.observed().await;
    let (status, value) = f.preview(old_source.clone()).await;
    assert_eq!(status, 200, "{value}");
    let original: IndependentDiskPreview = json_codec::from_value(value).unwrap();
    let moved = f.source.with_extension("new-host-mount");
    fs::rename(&f.source, &moved).unwrap();
    f.source = moved;
    let before = bytes(&f.source);
    let known = bytes(&f.known);
    assert_ne!(f.preview(old_source).await.0, 200);
    let observed = f.observed().await;
    let (status, value) = f.preview(observed.clone()).await;
    assert_eq!(status, 200, "{value}");
    let output: IndependentDiskPreview = json_codec::from_value(value).unwrap();
    assert_eq!(output.source, observed);
    assert_eq!(output.inventory, original.inventory);
    assert_eq!(output.topology, original.topology);
    assert_eq!(bytes(&f.source), before);
    assert_eq!(bytes(&f.known), known);
    f.finish().await;
}
