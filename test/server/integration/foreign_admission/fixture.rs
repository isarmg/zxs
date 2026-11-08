//! Actual management HTTP and a separate managed foreign pool, all temporary.
use http_routing::{
    Router,
    body::Body,
    http::{Request, StatusCode, request::Builder},
};
use json_codec::{Value, json};
use service_stack::ServiceExt;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use storage_contract::{
    CommitRequest, Config, OperationId, RepositoryName,
    storage_directories::{DirectorySource, DiscoveredDiskState, DiscoveredDisks},
    storage_foreign::{ForeignDiskPreview, ForeignDiskPreviewRequest},
};
use storage_server::{
    Backend, ServerConfig, StorageState,
    management::{ManagementBackend, ManagementOptions, open_service_with_options},
};

pub(super) const ORIGIN: &str = "http://127.0.0.1:7400";
pub(super) const PREVIEW: &str = "/api/admin/storage/foreign/preview";

pub(super) struct Fixture {
    pub app: Option<Router>,
    pub token: String,
    pub csrf: String,
    pub source: PathBuf,
    pub discovery: PathBuf,
    pub target: PathBuf,
    pub management: PathBuf,
    backend: Option<Arc<ManagementBackend>>,
    drained: bool,
    _root: tempfile::TempDir,
}

impl Fixture {
    pub async fn new() -> Self {
        Self::with_source(false).await
    }
    pub async fn multi_origin() -> Self {
        Self::with_source(true).await
    }
    async fn with_source(multi_origin: bool) -> Self {
        // The actual discovery/activation gate requires persistent storage.
        let root = tempfile::tempdir().unwrap();
        let discovery = root.path().join("disks");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&discovery)
            .unwrap();
        let target = discovery.join("primary");
        fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
        let source = discovery.join("foreign");
        create_foreign(&root.path().join("foreign-system"), &source, multi_origin);
        let management = root.path().join("management");
        let (backend, auth) = open_service_with_options(
            &root.path().join("system"),
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
        let mut fixture = Self {
            app: Some(app),
            token: String::new(),
            csrf: String::new(),
            source,
            discovery,
            target,
            management,
            backend: Some(backend),
            drained: false,
            _root: root,
        };
        fixture.wait_idle().await;
        let credentials = json!({"username":"admin","password":"foreign-http-test-password"});
        assert_eq!(
            fixture
                .send(
                    request("POST", "/api/setup/initialize"),
                    credentials.clone()
                )
                .await
                .0,
            StatusCode::OK
        );
        let (status, session) = fixture
            .send(request("POST", "/api/auth/login"), credentials)
            .await;
        assert_eq!(status, StatusCode::OK, "{session:?}");
        fixture.token = session["token"].as_str().unwrap().to_owned();
        fixture.csrf = session["csrf_token"].as_str().unwrap().to_owned();
        fixture
    }

    pub fn cookie(&self, method: &str, path: &str) -> Builder {
        request(method, path).header("cookie", format!("zxs_session={}", self.token))
    }
    pub fn browser(&self, method: &str, path: &str) -> Builder {
        self.cookie(method, path).header("x-csrf-token", &self.csrf)
    }
    pub async fn send(&self, request: Builder, value: Value) -> (StatusCode, Value) {
        self.raw(
            request
                .body(Body::from(json_codec::to_vec(&value).unwrap()))
                .unwrap(),
        )
        .await
    }
    pub async fn raw(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = tokio::time::timeout(
            Duration::from_secs(15),
            self.app.as_ref().unwrap().clone().oneshot(request),
        )
        .await
        .unwrap()
        .unwrap();
        let status = response.status();
        let bytes = http_routing::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        (status, json_codec::from_slice(&bytes).unwrap())
    }
    pub async fn json(&self, path: &str, value: Value) -> Value {
        let (status, response) = self.send(self.browser("POST", path), value).await;
        assert_eq!(status, StatusCode::OK, "{response:?}");
        response
    }
    async fn discovered(&self) -> DiscoveredDisks {
        let (status, response) = self
            .raw(
                self.browser("GET", "/api/admin/storage/discovered-disks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{response:?}");
        let found: DiscoveredDisks = json_codec::from_value(response).unwrap();
        assert!(!found.truncated);
        found
    }
    pub async fn selected(&self) -> DirectorySource {
        let found = self.discovered().await;
        let row = found
            .candidates
            .into_iter()
            .find(|row| row.name == "foreign")
            .unwrap();
        match row.state {
            DiscoveredDiskState::Zxs { source, .. } => source,
            other => panic!("expected the real foreign seal, got {other:?}"),
        }
    }
    pub async fn preview(&self, source: &DirectorySource) -> ForeignDiskPreview {
        json_codec::from_value(
            self.json(
                PREVIEW,
                json_codec::to_value(ForeignDiskPreviewRequest {
                    source: source.clone(),
                })
                .unwrap(),
            )
            .await,
        )
        .unwrap()
    }
    pub async fn activate_target(&self) -> String {
        let row = self
            .discovered()
            .await
            .candidates
            .into_iter()
            .find(|row| row.name == "primary")
            .unwrap();
        let DiscoveredDiskState::Empty { selection } = row.state else {
            panic!("target must be empty")
        };
        self.json(
            "/api/admin/storage/activate",
            json!({"selections":[selection],"mode":"development"}),
        )
        .await;
        let deadline = Instant::now() + Duration::from_secs(15);
        while self.backend.as_ref().unwrap().management_status().state() != StorageState::Ready {
            assert!(Instant::now() < deadline, "activation did not complete");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let (status, result) = self
            .raw(
                self.browser("GET", "/api/file-manager/disks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{result:?}");
        let disks = result["disks"].as_array().unwrap();
        assert_eq!(disks.len(), 1);
        let target = &disks[0];
        assert_eq!(target["name"], "primary");
        assert_eq!(target["level"], 1);
        assert_eq!(target["online"], true);
        assert_eq!(target["writable"], true);
        target["slot_id"].as_str().unwrap().to_owned()
    }
    async fn wait_idle(&self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while self
            .backend
            .as_ref()
            .unwrap()
            .management_status()
            .recovering()
        {
            assert!(
                Instant::now() < deadline,
                "management startup did not complete"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }
    async fn stop(&mut self) {
        drop(self.app.take());
        let backend = self.backend.take().unwrap();
        tokio::task::spawn_blocking(move || backend.drain_management())
            .await
            .unwrap()
            .unwrap();
        self.drained = true;
    }
    pub async fn reopen(&mut self) {
        self.stop().await;
        let (backend, auth) = open_service_with_options(
            &self._root.path().join("system"),
            &self.management,
            ManagementOptions {
                discovery_root: self.discovery.to_str().unwrap().parse().unwrap(),
                allow_development_setup: true,
                ..Default::default()
            },
        )
        .unwrap();
        self.app = Some(
            storage_server::router(backend.clone(), auth, ServerConfig::new(&self.management))
                .unwrap(),
        );
        self.backend = Some(backend);
        self.drained = false;
        self.wait_idle().await;
        let (status, session) = self
            .send(
                request("POST", "/api/auth/login"),
                json!({"username":"admin","password":"foreign-http-test-password"}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{session:?}");
        self.token = session["token"].as_str().unwrap().into();
        self.csrf = session["csrf_token"].as_str().unwrap().into();
    }
    pub async fn finish(mut self) {
        self.stop().await;
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.app.take());
        if !self.drained
            && let Some(backend) = self.backend.take()
            && let Err(error) = backend.drain_management()
        {
            eprintln!("foreign fixture cleanup after failure: {error:?}");
        }
    }
}

pub(super) fn request(method: &str, path: &str) -> Builder {
    Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:7400")
        .header("origin", ORIGIN)
        .header("sec-fetch-site", "same-origin")
        .header("content-type", "application/json")
}

pub(super) fn create_foreign(system: &Path, source: &Path, multi_origin: bool) {
    let mut config = if multi_origin {
        Config::development([
            system.with_extension("primary-a"),
            system.with_extension("primary-b"),
            source.to_path_buf(),
        ])
        .unwrap()
    } else {
        Config::development([source.to_path_buf()]).unwrap()
    };
    if multi_origin {
        config.disks[1].level = 1;
        config.disks[1].upstream_slot_id = None;
        config.disks[2].level = 2;
        config.disks[2].upstream_slot_id = Some("P01".into());
    }
    let mut engine = storage_engine::Engine::initialize(system, config).unwrap();
    add_history(
        &mut engine,
        "foreign-history",
        "P01",
        if multi_origin { 2 } else { 1 },
    );
    if multi_origin {
        add_history(&mut engine, "other-origin", "B01", 1);
    }
}
fn add_history(engine: &mut storage_engine::Engine, name: &str, slot: &str, copies: u32) {
    let repository = engine
        .create_repository(
            &RepositoryName::parse(name).unwrap(),
            slot,
            copies,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "document.txt",
            b"held foreign bytes",
        )
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
}

pub(super) fn bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(root: &Path, current: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(current).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                collect(root, &entry.path(), output);
            } else {
                output.insert(
                    entry.path().strip_prefix(root).unwrap().into(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut output = BTreeMap::new();
    collect(root, root, &mut output);
    output
}
