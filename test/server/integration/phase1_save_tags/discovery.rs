//! The browser's cookie/CSRF HTTP routes backed by real discovery and activation.
//! These are Router integration tests, not browser or physical-disk acceptance.
use http_routing::{
    Router,
    body::Body,
    http::{Request, StatusCode, request::Builder},
};
use json_codec::{Value, json};
use service_stack::ServiceExt;
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use storage_contract::storage_directories::{
    CandidateSelection, DiscoveredDiskState, DiscoveredDisks, NamedStorageSelection,
};
use storage_server::{
    Backend, ServerConfig, StorageState,
    management::{ManagementBackend, ManagementOptions, open_service_with_options},
};

const ORIGIN: &str = "http://127.0.0.1:7400";
const DISCOVERY: &str = "/api/admin/storage/discovered-disks";
const ACTIVATE: &str = "/api/admin/storage/activate";

struct Session {
    token: String,
    csrf: String,
}

struct Fixture {
    backend: Arc<ManagementBackend>,
    app: Option<Router>,
    session: Option<Session>,
    data: PathBuf,
    management: PathBuf,
    discovery: PathBuf,
    drained: bool,
    _root: tempfile::TempDir,
}

impl Fixture {
    async fn new(names: &[&str]) -> Self {
        // Activation deliberately keeps its persistent-filesystem gate. The
        // runner must select a real temporary filesystem, not tmpfs.
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("system");
        let management = root.path().join("management");
        let discovery = root.path().join("disks");
        private(&discovery);
        for name in names {
            private(&discovery.join(name));
        }
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
        let mut fixture = Self {
            backend,
            app: Some(app),
            session: None,
            data,
            management,
            discovery,
            drained: false,
            _root: root,
        };
        fixture.wait_idle().await;
        let credentials = json!({"username":"admin","password":"phase-one-test-password"});
        let (status, initialized) = fixture
            .send(
                request("POST", "/api/setup/initialize"),
                Some(credentials.clone()),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{initialized:?}");
        assert_eq!(initialized["storage_pending"], true);
        let (status, login) = fixture
            .send(request("POST", "/api/auth/login"), Some(credentials))
            .await;
        assert_eq!(status, StatusCode::OK, "{login:?}");
        fixture.session = Some(Session {
            token: login["token"].as_str().unwrap().to_owned(),
            csrf: login["csrf_token"].as_str().unwrap().to_owned(),
        });
        fixture.assert_unactivated();
        fixture
    }

    fn cookie_request(&self, method: &str, route: &str) -> Builder {
        let session = self.session.as_ref().unwrap();
        request(method, route).header("cookie", format!("zxs_session={}", session.token))
    }

    fn browser_request(&self, method: &str, route: &str) -> Builder {
        self.cookie_request(method, route)
            .header("x-csrf-token", &self.session.as_ref().unwrap().csrf)
    }

    async fn send(&self, request: Builder, body: Option<Value>) -> (StatusCode, Value) {
        let body = match body {
            Some(value) => Body::from(json_codec::to_vec(&value).unwrap()),
            None => Body::empty(),
        };
        let response = self
            .app
            .as_ref()
            .unwrap()
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (status, json_codec::from_slice(&bytes).unwrap())
    }

    async fn json(&self, method: &str, route: &str, body: Option<Value>) -> Value {
        let (status, value) = self.send(self.browser_request(method, route), body).await;
        assert_eq!(status, StatusCode::OK, "{value:?}");
        value
    }

    async fn discovered(&self) -> DiscoveredDisks {
        let found: DiscoveredDisks =
            json_codec::from_value(self.json("GET", DISCOVERY, None).await).unwrap();
        assert!(!found.truncated);
        found
    }

    async fn selections(&self, names: &[&str]) -> Vec<NamedStorageSelection> {
        let found = self.discovered().await;
        let mut selections = Vec::new();
        for name in names {
            let candidate = found
                .candidates
                .iter()
                .find(|row| row.name == *name)
                .unwrap();
            let DiscoveredDiskState::Empty { selection } = &candidate.state else {
                panic!("expected an empty candidate: {:?}", candidate.state);
            };
            let inspected: CandidateSelection = json_codec::from_value(
                self.json(
                    "POST",
                    "/api/admin/storage/directories/inspect",
                    Some(json!({
                        "mount_path":selection.mount_path,
                        "root_path":selection.root_path
                    })),
                )
                .await,
            )
            .unwrap();
            assert_eq!(&inspected, selection);
            selections.push(NamedStorageSelection {
                selection: inspected,
                // This is the explicit name supplied by this test's user,
                // not a production fallback from a mount or directory name.
                name: name_policy::Name::parse(name).unwrap(),
            });
        }
        selections
    }

    async fn activate(&self, selections: &[NamedStorageSelection]) -> (StatusCode, Value) {
        self.send(
            self.browser_request("POST", ACTIVATE),
            Some(activation(selections)),
        )
        .await
    }

    async fn wait_idle(&self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while self.backend.management_status().recovering() {
            assert!(
                Instant::now() < deadline,
                "management worker did not finish"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    async fn wait_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        // Activation explicitly returns recovering=true; wait for the owned
        // native reopen, not an arbitrary delay or a retried failed listing.
        while self.backend.management_status().state() != StorageState::Ready {
            assert!(
                Instant::now() < deadline,
                "activated storage did not reopen"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    fn assert_unactivated(&self) {
        for name in [
            "config.json",
            "storage-activation-intent.json",
            "storage-activation-ready.json",
            "storage-activation-complete.json",
        ] {
            assert!(
                !self.data.join(name).try_exists().unwrap(),
                "unexpected {name}"
            );
        }
    }

    async fn stop(&mut self) {
        drop(self.app.take());
        let backend = Arc::clone(&self.backend);
        tokio::task::spawn_blocking(move || backend.drain_management())
            .await.unwrap().unwrap();
        self.drained = true;
    }

    async fn reopen(&mut self) {
        assert!(self.drained);
        let (backend, auth) = open_service_with_options(
            &self.data, &self.management,
            ManagementOptions {
                discovery_root: self.discovery.to_str().unwrap().parse().unwrap(),
                allow_development_setup: true,
                ..Default::default()
            },
        ).unwrap();
        self.app = Some(storage_server::router(
            backend.clone(), auth, ServerConfig::new(&self.management),
        ).unwrap());
        self.backend = backend;
        self.drained = false;
        self.wait_idle().await;
        let login = self.send(request("POST", "/api/auth/login"), Some(json!({
            "username":"admin", "password":"phase-one-test-password"
        }))).await;
        assert_eq!(login.0, StatusCode::OK, "{:?}", login.1);
        self.session = Some(Session {
            token: login.1["token"].as_str().unwrap().to_owned(),
            csrf: login.1["csrf_token"].as_str().unwrap().to_owned(),
        });
    }

    async fn finish(mut self) {
        drop(self.app.take());
        let backend = Arc::clone(&self.backend);
        tokio::task::spawn_blocking(move || backend.drain_management())
            .await
            .unwrap()
            .unwrap();
        self.drained = true;
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.app.take());
        if !self.drained {
            // A failed assertion must not remove the fixture under native
            // writers or replace the original failure with a second panic.
            if let Err(error) = self.backend.drain_management() {
                eprintln!("discovery fixture cleanup after failure: {error:?}");
            }
        }
    }
}

fn request(method: &str, route: &str) -> Builder {
    Request::builder()
        .method(method)
        .uri(route)
        .header("host", "127.0.0.1:7400")
        .header("origin", ORIGIN)
        .header("sec-fetch-site", "same-origin")
        .header("content-type", "application/json")
}

fn private(path: &Path) {
    fs::DirBuilder::new().mode(0o700).create(path).unwrap();
}

fn empty_identity(path: &Path) -> (u64, u64, u32) {
    let metadata = fs::metadata(path).unwrap();
    assert!(fs::read_dir(path).unwrap().next().is_none());
    (metadata.dev(), metadata.ino(), metadata.mode())
}

fn assert_created_roots(config: &storage_contract::Config) {
    for disk in &config.disks {
        let id = disk.disk_id.parse::<storage_contract::DiskId>().unwrap();
        let binding = &config.storage_roots[&id];
        for (relative, entity) in [
            ("common-data", binding.roots.common_data.entity),
            ("deleted-data", binding.roots.deleted_data.entity),
            ("zxs-data", binding.roots.zxs_data.entity),
            ("zxs-data/common-data", binding.roots.zxs_lifecycles.common_data.entity),
            ("zxs-data/deleted-data", binding.roots.zxs_lifecycles.deleted_data.entity),
        ] {
            let metadata = fs::metadata(disk.root.join(relative)).unwrap();
            assert_eq!((metadata.dev(), metadata.ino()), (entity.device(), entity.inode()));
        }
    }
}

fn activation(selections: &[NamedStorageSelection]) -> Value {
    json!({"selections":selections,"mode":"development"})
}

#[tokio::test]
async fn cookie_discovery_inspection_and_activation_produce_browsable_primary_roots() {
    let mut f = Fixture::new(&["资料盘", "Archive", "ordinary"]).await;
    let retained = f.discovery.join("ordinary/keep");
    fs::write(&retained, b"ordinary content stays unchanged").unwrap();
    let before = empty_identity(&f.discovery.join("资料盘"));
    let found = f.discovered().await;
    let ordinary = found
        .candidates
        .iter()
        .find(|row| row.name == "ordinary")
        .unwrap();
    assert!(matches!(
        &ordinary.state,
        DiscoveredDiskState::Unsupported {
            source: Some(_),
            ..
        }
    ));
    let query = super::path_query(f.discovery.to_str().unwrap());
    let legacy = f
        .json(
            "GET",
            &format!("/api/admin/storage/directories?{query}"),
            None,
        )
        .await;
    assert!(
        legacy["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["name"] != "ordinary")
    );
    let selected = f.selections(&["资料盘", "Archive"]).await;
    assert_eq!(empty_identity(&f.discovery.join("资料盘")), before);
    f.assert_unactivated();
    let (status, result) = f.activate(&selected).await;
    assert_eq!(status, StatusCode::OK, "{result:?}");
    assert_eq!(result["activated"], true);
    assert_eq!(result["recovering"], true);
    f.wait_ready().await;
    assert_eq!(f.backend.management_status().state(), StorageState::Ready);
    let intent_path = f.data.join("storage-activation-intent.json");
    let ready_path = f.data.join("storage-activation-ready.json");
    let complete_path = f.data.join("storage-activation-complete.json");
    let intent = fs::read(&intent_path).unwrap();
    let ready = fs::read(&ready_path).unwrap();
    let original: Value = json_codec::from_slice(&intent).unwrap();
    assert!(original["config"]["storage_roots"].as_object().unwrap().is_empty());
    let config: storage_contract::Config = json_codec::from_slice(
        &fs::read(f.data.join("config.json")).unwrap(),
    ).unwrap();
    assert_eq!(config.storage_roots.len(), 2);
    assert_created_roots(&config);

    // Simulate loss of the last SYSTEM receipt after real Web initialization
    // and credential publication. Cold completion must keep the original intent
    // and all actual roots, not rewrite the intent with today's Config.
    f.stop().await;
    fs::remove_file(&complete_path).unwrap();
    f.reopen().await;
    assert_eq!(f.backend.management_status().state(), StorageState::RecoveryRequired);
    let status = f.json("GET", "/api/admin/storage/status", None).await;
    assert_eq!(status["completion_available"], true);
    let completed = f.json(
        "POST", "/api/admin/storage/complete-activation", Some(json!({})),
    ).await;
    assert_eq!(completed["activated"], true);
    f.wait_ready().await;
    assert_eq!(fs::read(&intent_path).unwrap(), intent);
    assert_eq!(fs::read(&ready_path).unwrap(), ready);
    let completion = fs::read(&complete_path).unwrap();
    let complete_entity = fs::metadata(&complete_path).unwrap();
    let complete: Value = json_codec::from_slice(&completion).unwrap();
    let ready_record: Value = json_codec::from_slice(&ready).unwrap();
    assert_eq!(complete["intent_digest"], ready_record["intent_digest"]);
    f.stop().await;
    f.reopen().await;
    f.wait_ready().await;
    assert_eq!(fs::read(&intent_path).unwrap(), intent);
    assert_eq!(fs::read(&ready_path).unwrap(), ready);
    assert_eq!(fs::read(&complete_path).unwrap(), completion);
    let reopened = fs::metadata(&complete_path).unwrap();
    assert_eq!(
        (reopened.dev(), reopened.ino()),
        (complete_entity.dev(), complete_entity.ino()),
    );
    // An active backend does not expose offline completion as a reset route.
    let repeated = f.send(
        f.browser_request("POST", "/api/admin/storage/complete-activation"), Some(json!({})),
    ).await;
    assert_eq!(repeated.0, StatusCode::CONFLICT, "{:?}", repeated.1);
    assert_eq!(repeated.1["error"]["code"], "STORAGE_ACTIVE");
    assert_eq!(fs::read(&complete_path).unwrap(), completion);
    let reopened_config: storage_contract::Config = json_codec::from_slice(
        &fs::read(f.data.join("config.json")).unwrap(),
    ).unwrap();
    assert_eq!(reopened_config.storage_roots, config.storage_roots);
    assert_created_roots(&reopened_config);
    let disks = f.json("GET", "/api/file-manager/disks", None).await;
    let disks = disks["disks"].as_array().unwrap();
    assert_eq!(disks.len(), 2);
    for name in ["资料盘", "Archive"] {
        let disk = disks.iter().find(|row| row["name"] == name).unwrap();
        assert_eq!(disk["level"], 1);
        assert_eq!(disk["online"], true);
        assert_eq!(disk["writable"], true);
        let slot = disk["slot_id"].as_str().unwrap();
        let listing = f
            .json(
                "GET",
                &format!("/api/file-manager/disks/{slot}/entries?path="),
                None,
            )
            .await;
        assert!(listing["entries"].as_array().unwrap().is_empty());
    }
    assert!(
        f.discovered()
            .await
            .candidates
            .iter()
            .all(|row| row.name != "资料盘" && row.name != "Archive")
    );
    assert_eq!(
        fs::read(retained).unwrap(),
        b"ordinary content stays unchanged"
    );
    f.finish().await;
}

#[tokio::test]
async fn cookie_activation_rejects_missing_csrf_and_cross_origin_without_writes() {
    let f = Fixture::new(&["primary"]).await;
    let before = empty_identity(&f.discovery.join("primary"));
    assert_eq!(
        f.send(request("GET", DISCOVERY), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.send(
            f.browser_request("GET", &format!("{DISCOVERY}?path=/")),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let selected = f.selections(&["primary"]).await;
    let missing = f
        .send(
            f.cookie_request("POST", ACTIVATE),
            Some(activation(&selected)),
        )
        .await;
    assert_eq!(missing.0, StatusCode::FORBIDDEN, "{:?}", missing.1);
    let session = f.session.as_ref().unwrap();
    let cross_origin = Request::builder()
        .method("POST")
        .uri(ACTIVATE)
        .header("host", "127.0.0.1:7400")
        .header("origin", "http://untrusted.invalid")
        .header("sec-fetch-site", "cross-site")
        .header("content-type", "application/json")
        .header("cookie", format!("zxs_session={}", session.token))
        .header("x-csrf-token", &session.csrf);
    let rejected = f.send(cross_origin, Some(activation(&selected))).await;
    assert_eq!(rejected.0, StatusCode::FORBIDDEN, "{:?}", rejected.1);
    f.assert_unactivated();
    assert_eq!(empty_identity(&f.discovery.join("primary")), before);
    let (status, result) = f.activate(&selected).await;
    assert_eq!(status, StatusCode::OK, "{result:?}");
    f.wait_ready().await;
    f.finish().await;
}

#[tokio::test]
async fn cookie_activation_rejects_unbrowseable_names_before_intent_and_can_retry() {
    let f = Fixture::new(&["CON"]).await;
    let before = empty_identity(&f.discovery.join("CON"));
    let found = f.discovered().await;
    let candidate = found.candidates.iter().find(|row| row.name == "CON").unwrap();
    let DiscoveredDiskState::Empty { selection } = &candidate.state else {
        panic!("expected an empty candidate: {:?}", candidate.state);
    };
    let inspected: CandidateSelection = json_codec::from_value(f.json(
        "POST",
        "/api/admin/storage/directories/inspect",
        Some(json!({
            "mount_path":selection.mount_path,
            "root_path":selection.root_path
        })),
    ).await).unwrap();
    assert_eq!(&inspected, selection);
    // Invalid user names must reach the actual HTTP decoder, not fail while
    // constructing a typed Name in the test before sending the request.
    let (status, error) = f.send(
        f.browser_request("POST", ACTIVATE),
        Some(json!({
            "selections":[{"selection":inspected,"name":"CON"}],
            "mode":"development"
        })),
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error:?}");
    assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    f.assert_unactivated();
    assert_eq!(empty_identity(&f.discovery.join("CON")), before);
    fs::rename(f.discovery.join("CON"), f.discovery.join("reviewed")).unwrap();
    let selected = f.selections(&["reviewed"]).await;
    let (status, result) = f.activate(&selected).await;
    assert_eq!(status, StatusCode::OK, "{result:?}");
    f.wait_ready().await;
    let disks = f.json("GET", "/api/file-manager/disks", None).await;
    assert_eq!(disks["disks"][0]["name"], "reviewed");
    f.finish().await;
}

#[tokio::test]
async fn replaced_directory_and_logged_out_cookie_cannot_activate_reviewed_selection() {
    let f = Fixture::new(&["primary"]).await;
    let selected = f.selections(&["primary"]).await;
    fs::rename(f.discovery.join("primary"), f.discovery.join("retained")).unwrap();
    private(&f.discovery.join("primary"));
    let (status, error) = f.activate(&selected).await;
    assert_eq!(status, StatusCode::CONFLICT, "{error:?}");
    assert_eq!(error["error"]["code"], "DISK_CHANGED");
    f.assert_unactivated();
    empty_identity(&f.discovery.join("retained"));
    let replacement = empty_identity(&f.discovery.join("primary"));
    let selected = f.selections(&["primary"]).await;
    assert_eq!(
        f.json("POST", "/api/auth/logout", Some(json!({}))).await["logged_out"],
        true
    );
    assert_eq!(f.activate(&selected).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        f.send(f.browser_request("GET", DISCOVERY), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    f.assert_unactivated();
    assert_eq!(empty_identity(&f.discovery.join("primary")), replacement);
    f.finish().await;
}
