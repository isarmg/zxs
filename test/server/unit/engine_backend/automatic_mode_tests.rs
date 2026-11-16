//! Real HTTP management and framed SMB/SCM_RIGHTS share the actual admission
//! owner. Local custody is retained evidence, never an automatically adopted draft.
use super::*;
use crate::{auth::AuthStore, Backend, ReadTree, ServerConfig, SmbConfig, SpoolLimits};
use http_routing::{Router, body::Body, http::Request as HttpRequest};
use json_codec::{Value, json};
use service_stack::ServiceExt;
use smb_protocol::{
    client::{Client, RemoteError, RemoteErrorCode},
    protocol::{OpenResult, Request},
};
use std::{
    fs::{self, File},
    io::{self, Read},
    os::unix::fs::{FileExt, MetadataExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use storage_contract::{CommitState, Config, Entry, ErrorCode, OperationId, Repository};
use storage_contract::file_draft::DraftLocation;

struct Credentials {
    username: String,
    password: String,
}
impl Credentials {
    fn generate() -> Self {
        fn value<const N: usize>() -> String {
            let mut bytes = [0; N];
            system_entropy::fill(&mut bytes).unwrap();
            bytes.iter().map(|byte| format!("{byte:02x}")).collect()
        }
        Self { username: format!("user_{}", value::<12>()), password: value::<32>() }
    }
}

struct Fronts {
    root: tempfile::TempDir,
    runtime: tokio::runtime::Runtime,
    server: tokio::task::JoinHandle<io::Result<()>>,
    backend: Arc<EngineBackend>,
    app: Router,
    auth: Arc<Mutex<AuthStore>>,
    token: String,
    username: String,
}
impl Fronts {
    fn new(credentials: &Credentials) -> Self {
        let root = tempfile::tempdir().unwrap();
        let engine = Engine::initialize(&root.path().join("state"), Config::development([
            (name_policy::Name::parse("p").unwrap(), root.path().join("primary")),
        ]).unwrap()).unwrap();
        let auth = AuthStore::initialize(
            &root.path().join("state"), &credentials.username, &credentials.password,
        ).unwrap();
        Self::start(root, engine, auth, credentials)
    }

    fn resume(root: tempfile::TempDir, engine: Engine, credentials: &Credentials) -> Self {
        let auth = AuthStore::open(&root.path().join("state")).unwrap();
        Self::start(root, engine, auth, credentials)
    }

    fn start(
        root: tempfile::TempDir, engine: Engine, auth: AuthStore, credentials: &Credentials,
    ) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2)
            .max_blocking_threads(4).enable_all().build().unwrap();
        let backend = Arc::new(EngineBackend::new(engine));
        let auth = backend.protect_auth(auth).unwrap();
        let (app, auth) = runtime.block_on(async {
            // These are the same owned components as router()/serve(), retained
            // here so the two genuine transports use one credential owner.
            let config = ServerConfig::new(&root.path().join("state")).admit().unwrap();
            let spool = crate::http_spool::HttpSpool::open_blocking(&config);
            let managed = crate::ManagementRuntime::start(backend.clone(), auth, &config).unwrap();
            let shutdown = Arc::new(crate::service_lifecycle::EmbeddedManagementShutdown::new(
                managed.backend.clone(), managed.telemetry.clone(),
            ));
            let auth = managed.auth.clone();
            let app = crate::router_shared(
                managed.backend,
                managed.auth,
                config,
                managed.telemetry,
                spool,
                managed.smb_account,
            ).unwrap().layer(http_routing::Extension(shutdown));
            (app, auth)
        });
        let server = runtime.spawn(crate::smb_rpc::serve(SmbConfig {
            socket: root.path().join("run/bridge.sock"),
            spool_dir: root.path().join("management/smb-spool"),
            allowed_uid: rustix::process::geteuid().as_raw(),
            username: credentials.username.clone(), additional_users: Vec::new(),
            spool_limits: SpoolLimits {
                bytes_per_account: 16 * 1024 * 1024, bytes_total: 64 * 1024 * 1024,
                recovery_reserve_bytes: 1,
            },
        }, auth.clone(), backend.clone(), 4 * 1024 * 1024));
        let mut result = Self {
            root, runtime, server, backend, app, auth, token: String::new(),
            username: credentials.username.clone(),
        };
        let (status, login) = result.http("POST", "/api/auth/login", Some(json!({
            "username": credentials.username, "password": credentials.password,
        })));
        assert!(status.is_success());
        result.token = login["token"].as_str().unwrap().to_owned();
        result
    }

    fn http(
        &self, method: &str, route: &str, value: Option<Value>,
    ) -> (http::StatusCode, Value) {
        self.runtime.block_on(async {
            let mut request = HttpRequest::builder().method(method).uri(route);
            if !self.token.is_empty() {
                request = request.header("authorization", format!("Bearer {}", self.token));
            }
            let body = if let Some(value) = value {
                request = request.header("content-type", "application/json");
                Body::from(json_codec::to_vec(&value).unwrap())
            } else { Body::empty() };
            let response = self.app.clone()
                .oneshot(request.body(body).unwrap()).await.unwrap();
            let status = response.status();
            let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
                .await.unwrap();
            (status, json_codec::from_slice(&bytes).unwrap())
        })
    }

    fn connect(&self) -> Client {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match Client::connect_default(
                &self.root.path().join("run/bridge.sock"), &self.username,
            ) {
                Ok(client) => return client,
                Err(error) if matches!(error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) =>
                {
                    assert!(!self.server.is_finished(), "real bridge stopped during startup");
                    let left = deadline.saturating_duration_since(Instant::now());
                    assert!(!left.is_zero(), "real bridge was not ready: {error}");
                    std::thread::sleep(left.min(Duration::from_millis(5)));
                }
                Err(error) => panic!("real bridge refused connection: {error}"),
            }
        }
    }

    fn identity(&self) -> Identity {
        self.auth.lock().unwrap().sole_identity().unwrap()
    }

    fn cold(self) -> (tempfile::TempDir, Engine) {
        let Self { root, runtime, server, backend, app, auth, token: _, username: _ } = self;
        server.abort();
        assert!(runtime.block_on(server).unwrap_err().is_cancelled());
        drop(app);
        drop(auth);
        drop(backend);
        // Wait only for the actual embedded management shutdown to release its
        // writer. Authentication/recovery failures are never retried as locks.
        let engine = runtime.block_on(async {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match Engine::open(&root.path().join("state")) {
                    Ok(engine) => break engine,
                    Err(error) if error.code() == ErrorCode::WriterAlreadyActive => {
                        let left = deadline.saturating_duration_since(Instant::now());
                        assert!(!left.is_zero(), "management retained its writer: {error}");
                        tokio::time::sleep(left.min(Duration::from_millis(10))).await;
                    }
                    Err(error) => panic!("cold open failed: {error}"),
                }
            }
        });
        runtime.shutdown_timeout(Duration::from_secs(3));
        (root, engine)
    }

    fn target(&self, name: &str) -> (Repository, Entry, DraftLocation) {
        let identity = self.identity();
        let repositories = self.backend.repositories_snapshot(&identity).unwrap();
        assert_eq!(repositories.len(), 1);
        let repository = repositories.into_iter().next().unwrap();
        let tree = self.backend.tree_snapshot(&identity, ReadTree {
            branch: &repository.main_branch_id, commit: None,
        }).unwrap();
        let entry = tree.entries.into_iter().find(|entry| entry.path() == name).unwrap();
        let target = DraftLocation {
            repository: repository.repository_id.parse().unwrap(),
            path: entry.path.clone(), entry: entry.entry_id.parse().unwrap(),
            file: entry.file_id.parse().unwrap(),
        };
        (repository, entry, target)
    }

    fn mode_request(&self, target: &DraftLocation) -> Value {
        let snapshot = self.backend.file_working(&self.identity(), target.clone()).unwrap();
        json!({ "operation_id": OperationId::generate().unwrap(), "command": {
            "action": "set_mode", "target": target,
            "expected": snapshot.working.revision(), "mode": "manual",
        } })
    }
}

fn open(client: &Client, name: &str, create: bool) -> (OpenResult, File) {
    let (opened, fd) = client.open(Request::Open {
        path: format!("p/{name}"), writable: true, create, truncate: false,
        exclusive: create, write_through: false,
    }).unwrap();
    (opened, File::from(fd))
}
fn stage(client: &Client, opened: &OpenResult, file: &File, bytes: &[u8]) {
    client.call(Request::PrepareWrite {
        handle: opened.handle, end: bytes.len() as u64, bytes: bytes.len() as u64,
    }).unwrap();
    file.write_all_at(bytes, 0).unwrap();
    file.set_len(bytes.len() as u64).unwrap();
    client.call(Request::Dirty { handle: opened.handle }).unwrap();
}
fn save(client: &Client, name: &str, bytes: &[u8]) {
    let (opened, file) = open(client, name, true);
    stage(client, &opened, &file, bytes);
    client.call(Request::Flush { handle: opened.handle }).unwrap();
    client.call(Request::Close { handle: opened.handle }).unwrap();
}
fn remote(error: &io::Error) -> (ErrorCode, Option<String>) {
    let error = error.get_ref().unwrap().downcast_ref::<RemoteError>().unwrap();
    let RemoteErrorCode::Server(code) = &error.code else { panic!("non-server RPC error") };
    (*code, error.operation_id.clone())
}
fn custody(root: &Path, workspace: &str) -> (PathBuf, Value) {
    let mut records = fs::read_dir(root.join("management/smb-spool")).unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().ends_with(".work.json"))
        .filter_map(|path| {
            let record: Value = json_codec::from_slice(&fs::read(&path).unwrap()).unwrap();
            (record["workspace_id"] == workspace).then_some((path, record))
        });
    let result = records.next().unwrap();
    assert!(records.next().is_none());
    result
}
// Re-submit the original request, not a receipt manufactured from local
// custody. Backend/Engine still authenticate its actual workspace, principal,
// digest and permanent native completion; this record grants no replay right.
fn saved_request(record: &Value) -> crate::CommitWorkspace {
    crate::CommitWorkspace {
        workspace_id: record["workspace_id"].as_str().unwrap().to_owned(),
        operation_id: record["operation_id"].as_str().unwrap().parse().unwrap(),
        expected_head: record["expected_head"].as_str().unwrap().to_owned(),
        fencing_token: record["fencing_token"].as_str().unwrap().parse().unwrap(),
        message: "SMB full-file save".into(),
        request_digest: None,
    }
}

fn require_retained(source: &Path, identity: (u64, u64), bytes: &[u8], budget: &[u8]) {
    let actual = fs::metadata(source).unwrap();
    assert_eq!((actual.dev(), actual.ino()), identity);
    assert_eq!(fs::read(source).unwrap(), bytes);
    assert_eq!(fs::read(source.with_extension("budget.json")).unwrap(), budget);
}

#[test]
fn real_http_mode_switch_fences_old_auto_rpc_and_retains_undecided_bytes_across_cold() {
    let credentials = Credentials::generate();
    let fronts = Fronts::new(&credentials);
    let client = fronts.connect();
    save(&client, "target.txt", b"first immutable body");
    let (repository, first, target) = fronts.target("target.txt");
    let (opened, file) = open(&client, "target.txt", false);
    stage(&client, &opened, &file, b"already in automatic preparation");
    let request = fronts.mode_request(&target);
    let route = format!("/api/file-manager/files/{}/working", target.file);
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    *fronts.backend.commit_preparation_hook.lock().unwrap() = Some(Arc::new(move || {
        entered.send(()).unwrap();
        released.lock().unwrap().recv_timeout(Duration::from_secs(10)).unwrap();
    }));
    let flushing = client.clone();
    let handle = opened.handle;
    let worker = std::thread::spawn(move || flushing.call(Request::Flush { handle }));
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    let (status, busy) = fronts.http("POST", &route, Some(request.clone()));
    release.send(()).unwrap();
    let published = worker.join().unwrap().unwrap();
    *fronts.backend.commit_preparation_hook.lock().unwrap() = None;
    assert!(!status.is_success());
    assert_eq!(busy["error"]["code"], "COMMIT_PREPARATION_BUSY");
    assert_eq!(published["commit_state"], "PUBLISHED");
    assert_eq!(published["operation_kind"], "workspace.commit");
    let identity = fronts.identity();
    assert_eq!(fronts.backend.file_history(&identity, &first.file_id).unwrap().len(), 2);

    // The first publication record still names its original workspace. Dirty
    // changes its contents, not the source/record filename or held inode.
    let (record_path, original_record) =
        custody(fronts.root.path(), opened.workspace_id.as_deref().unwrap());
    let original_request = saved_request(&original_record);
    let bytes = b"not decided when the user disables automatic nodes";
    stage(&client, &opened, &file, bytes);
    let frozen: Value = json_codec::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    let source = PathBuf::from(frozen["source_path"].as_str().unwrap());
    let stamp = fs::metadata(&source).unwrap();
    let stamp = (stamp.dev(), stamp.ino());
    let charge = fs::read(source.with_extension("budget.json")).unwrap();
    let admitted: Value = json_codec::from_slice(&charge).unwrap();
    assert!(admitted["admitted_end"].as_str().unwrap().parse::<u64>().unwrap()
        < 3 * 1024 * 1024);
    let old_operation = frozen["operation_id"].as_str().unwrap().to_owned();
    let (status, switched) = fronts.http("POST", &route, Some(request.clone()));
    assert!(status.is_success());
    assert_eq!(switched["commit_state"], "PUBLISHED");
    let switched_state = fronts.backend.file_working(&identity, target.clone()).unwrap();
    let sequence = fronts.backend.engine.lock().unwrap().state().sequence;
    let replay = fronts.backend.commit_workspace(&identity, original_request).unwrap();
    assert_eq!(crate::operation_wire::operation_wire(replay.into()).unwrap(), published);
    assert_eq!(fronts.backend.engine.lock().unwrap().state().sequence, sequence);
    let head = fronts.backend.tree_snapshot(&identity, ReadTree {
        branch: &repository.main_branch_id, commit: None,
    }).unwrap().commit_id;
    // Mode is not a read revocation. This exact held FD still has authenticated
    // read access; no new write reservation or success is inferred from it.
    client.call(Request::AuthorizeRead { handle }).unwrap();
    let mut visible = vec![0; bytes.len()];
    file.read_exact_at(&mut visible, 0).unwrap();
    assert_eq!(visible, bytes);
    for command in [
        Request::PrepareWrite { handle, end: 3 * 1024 * 1024, bytes: 3 * 1024 * 1024 },
        Request::Flush { handle },
        Request::Close { handle },
    ] {
        let error = client.call(command).unwrap_err();
        assert_eq!(remote(&error).0, ErrorCode::FileBranchConflict);
        require_retained(&source, stamp, bytes, &charge);
    }
    let detached: Value = json_codec::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(detached["operation_id"], old_operation);
    assert_eq!(detached["workspace_id"], frozen["workspace_id"]);
    assert_eq!(detached["handle_state"], "detached");
    assert_eq!(detached["retained_bytes_synced"], true);
    assert_eq!(detached["published"], false);
    assert_eq!(fronts.backend.file_working(&identity, target.clone()).unwrap().working,
        switched_state.working);
    assert_eq!(fronts.backend.tree_snapshot(&identity, ReadTree {
        branch: &repository.main_branch_id, commit: None,
    }).unwrap().commit_id, head);
    assert_eq!(fronts.backend.file_history(&identity, &first.file_id).unwrap().len(), 2);
    save(&client, "unrelated.txt", b"another FILE remains writable");
    let (manual, manual_file) = open(&client, "target.txt", false);
    assert!(manual.workspace_id.is_none());
    stage(&client, &manual, &manual_file, b"a new real shared manual draft");
    let draft = client.call(Request::Flush { handle: manual.handle }).unwrap();
    assert_eq!(draft["operation_kind"], "file.draft.write");
    client.call(Request::Close { handle: manual.handle }).unwrap();
    drop(manual_file);
    drop(file);
    drop(client);
    let (root, engine) = fronts.cold();
    assert_eq!(engine.file_history(&first.file_id).unwrap().len(), 2);
    assert!(engine.file_working_state(&target).unwrap().draft().is_some());
    require_retained(&source, stamp, bytes, &charge);
    assert_eq!(
        json_codec::from_slice::<Value>(&fs::read(&record_path).unwrap()).unwrap(), detached,
    );
    // No cold constructor claims local custody as a shared draft or publishes it.
    assert!(!engine.state().operations.contains_key(&old_operation.parse().unwrap()));
    let fronts = Fronts::resume(root, engine, &credentials);
    let (status, repeated) = fronts.http("POST", &route, Some(request));
    assert!(status.is_success());
    assert_eq!(repeated, switched);
    require_retained(&source, stamp, bytes, &charge);
    let (_root, engine) = fronts.cold();
    assert_eq!(engine.file_history(&first.file_id).unwrap().len(), 2);
}


#[test]
fn accepted_auto_rpc_save_cold_completes_before_mode_switch_and_replays_only_original_request() {
    let credentials = Credentials::generate();
    let fronts = Fronts::new(&credentials);
    let client = fronts.connect();
    save(&client, "accepted.txt", b"original automatic node");
    let (_, first, target) = fronts.target("accepted.txt");
    let (opened, file) = open(&client, "accepted.txt", false);
    let bytes = b"the same native automatic decision must finish after restart";
    stage(&client, &opened, &file, bytes);
    let (record_path, original) =
        custody(fronts.root.path(), opened.workspace_id.as_deref().unwrap());
    let request = saved_request(&original);
    let operation = request.operation_id;
    let source = PathBuf::from(original["source_path"].as_str().unwrap());
    let stamp = fs::metadata(&source).unwrap();
    let stamp = (stamp.dev(), stamp.ino());
    let charge = fs::read(source.with_extension("budget.json")).unwrap();
    let mode = fronts.mode_request(&target);
    let route = format!("/api/file-manager/files/{}/working", target.file);
    fronts.backend.engine.lock().unwrap().inject_fault_once("after_accept_1");
    let failed = client.call(Request::Flush { handle: opened.handle }).unwrap_err();
    assert_eq!(remote(&failed).0, ErrorCode::CommitInDoubt);
    let identity = fronts.identity();
    let pending = fronts.backend.operation_outcome(&identity, operation).unwrap();
    assert_eq!(pending.commit_state, CommitState::InDoubt);
    assert_eq!(fronts.backend.file_history(&identity, &first.file_id).unwrap().len(), 1);
    let (status, blocked) = fronts.http("POST", &route, Some(mode.clone()));
    assert!(!status.is_success());
    assert_eq!(blocked["error"]["code"], "RECOVERY_REQUIRED");
    for command in [
        Request::Flush { handle: opened.handle },
        Request::Close { handle: opened.handle },
    ] {
        assert_eq!(remote(&client.call(command).unwrap_err()).0, ErrorCode::RecoveryRequired);
        assert_eq!(fronts.backend.operation_outcome(&identity, operation).unwrap(), pending);
        require_retained(&source, stamp, bytes, &charge);
    }
    let retained: Value = json_codec::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(retained["operation_id"], original["operation_id"]);
    assert_eq!(retained["workspace_id"], original["workspace_id"]);
    assert_eq!(retained["handle_state"], "detached");
    drop(file);
    drop(client);
    let (root, engine) = fronts.cold();
    let versions = engine.file_history(&first.file_id).unwrap();
    assert_eq!(versions.len(), 2);
    let actual = versions.iter().find(|version| version.version_id != first.version_id).unwrap();
    let mut body = Vec::new();
    engine.open_content(&actual.version_id).unwrap().file.read_to_end(&mut body).unwrap();
    assert_eq!(body, bytes);
    require_retained(&source, stamp, bytes, &charge);
    let fronts = Fronts::resume(root, engine, &credentials);
    let identity = fronts.identity();
    let published = fronts.backend.operation_outcome(&identity, operation).unwrap();
    assert_eq!(published.commit_state, CommitState::Published);
    let (status, changed) = fronts.http("POST", &route, Some(mode.clone()));
    assert!(status.is_success());
    let sequence = fronts.backend.engine.lock().unwrap().state().sequence;
    let replay = fronts.backend.commit_workspace(&identity, request).unwrap();
    assert_eq!(storage_contract::OperationOutcome::from(replay), published);
    let (status, repeated) = fronts.http("POST", &route, Some(mode));
    assert!(status.is_success());
    assert_eq!(repeated, changed);
    assert_eq!(fronts.backend.engine.lock().unwrap().state().sequence, sequence);
    assert_eq!(fronts.backend.file_history(&identity, &first.file_id).unwrap().len(), 2);
    require_retained(&source, stamp, bytes, &charge);
    let (_root, engine) = fronts.cold();
    assert_eq!(engine.file_history(&first.file_id).unwrap().len(), 2);
    assert_eq!(
        json_codec::from_slice::<Value>(&fs::read(record_path).unwrap()).unwrap(), retained,
    );
}
