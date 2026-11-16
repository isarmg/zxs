//! The socket/FD path publishes only the shared draft. A separately authenticated
//! HTTP request creates the FILE node; local custody files never authorize replay.
use super::*;
use http_routing::{Router, body::Body, http::Request as HttpRequest};
use json_codec::{Value, json};
use service_stack::ServiceExt;
use std::io::{Read, Write};
use storage_contract::file_draft::{DraftCommand, DraftLocation};
use storage_server::{ChangeFileWorking, ServerConfig, UploadSource};

struct Management {
    runtime: tokio::runtime::Runtime,
    root: tempfile::TempDir,
    app: Router,
    token: String,
}

impl Management {
    fn open(root: tempfile::TempDir, engine: Engine) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let system = root.path().join("state");
        let app = runtime.block_on(async {
            let backend = Arc::new(EngineBackend::new(engine));
            let auth = backend.protect_auth(AuthStore::open(&system).unwrap()).unwrap();
            assert!(auth.is_available());
            storage_server::router(backend, auth, ServerConfig::new(&system)).unwrap()
        });
        let mut result = Self { runtime, root, app, token: String::new() };
        let login = result.json("POST", "/api/auth/login", Some(json!({
            "username": "alice", "password": "phase-one-smb-password",
        })));
        result.token = login["token"].as_str().unwrap().to_owned();
        result
    }

    fn json(&self, method: &str, route: &str, body: Option<Value>) -> Value {
        self.runtime.block_on(async {
            let mut request = HttpRequest::builder().method(method).uri(route);
            if !self.token.is_empty() {
                request = request.header("authorization", format!("Bearer {}", self.token));
            }
            let body = match body {
                Some(body) => {
                    request = request.header("content-type", "application/json");
                    Body::from(json_codec::to_vec(&body).unwrap())
                }
                None => Body::empty(),
            };
            let response = self.app.clone().oneshot(request.body(body).unwrap()).await.unwrap();
            let status = response.status();
            let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
                .await.unwrap();
            let value: Value = json_codec::from_slice(&bytes).unwrap();
            assert!(status.is_success(), "{method} {route}: {status} {value}");
            value
        })
    }

    fn cold_open(self) -> (tempfile::TempDir, Engine) {
        let Self { runtime, root, app, token: _ } = self;
        drop(app);
        // Match the actual embedded HTTP shutdown contract. Retry only its
        // still-held writer lock, never an authentication or recovery failure.
        let engine = runtime.block_on(async {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match Engine::open(&root.path().join("state")) {
                    Ok(engine) => break engine,
                    Err(error) if error.code() == ErrorCode::WriterAlreadyActive => {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        assert!(!remaining.is_zero(), "HTTP retained its writer: {error}");
                        tokio::time::sleep(remaining.min(Duration::from_millis(10))).await;
                    }
                    Err(error) => panic!("cold open failed: {error}"),
                }
            }
        });
        runtime.shutdown_timeout(Duration::from_secs(2));
        (root, engine)
    }
}

fn stage(client: &Client, handle: u64, file: &File, bytes: &[u8]) {
    client.call(Request::PrepareWrite {
        handle, end: bytes.len() as u64, bytes: bytes.len() as u64,
    }).unwrap();
    file.write_all_at(bytes, 0).unwrap();
    file.set_len(bytes.len() as u64).unwrap();
    client.call(Request::Dirty { handle }).unwrap();
}

fn remote_code(error: &io::Error) -> ErrorCode {
    let remote = error.get_ref().unwrap().downcast_ref::<RemoteError>().unwrap();
    match &remote.code {
        RemoteErrorCode::Server(code) => *code,
        other => panic!("unexpected RPC error: {other:?}"),
    }
}

fn version_bytes(engine: &Engine, version: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    engine.open_content(version).unwrap().read_to_end(&mut bytes).unwrap();
    bytes
}

fn custody(root: &Path) -> Vec<(std::path::PathBuf, Value)> {
    let mut records = fs::read_dir(root.join("management/smb-spool")).unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().ends_with(".work.json"))
        .filter_map(|path| {
            let record: Value = json_codec::from_slice(&fs::read(&path).unwrap()).unwrap();
            (record["format"] == "smb-manual-draft-work").then_some((path, record))
        })
        .collect::<Vec<_>>();
    records.sort_by(|left, right| left.0.cmp(&right.0));
    records
}

fn retained(root: &Path, record: &Value, expected: &[u8]) -> std::path::PathBuf {
    let source = root.join("management/smb-spool").join(record["source"].as_str().unwrap());
    assert_eq!(fs::read(&source).unwrap(), expected);
    assert!(source.with_extension("budget.json").is_file());
    assert!(record.get("workspace_id").is_none());
    assert!(record.get("version_id").is_none());
    source
}

fn require_history(fixture: &Fixture, repository: &Repository, entry: &Entry, head: &str) {
    let current = fixture.backend.tree_snapshot(&fixture.identity(), ReadTree {
        branch: &repository.main_branch_id, commit: None,
    }).unwrap();
    assert_eq!(current.commit_id, head);
    assert_eq!(fixture.entry(&repository.main_branch_id, "manual.txt"), *entry);
    let versions = fixture.backend.file_history(&fixture.identity(), &entry.file_id).unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].version_id, entry.version_id);
}

#[test]
fn public_smb_manual_flush_and_close_keep_drafts_until_cold_authenticated_http_commit() {
    let mut fixture = Fixture::new();
    let client = fixture.connect();
    let original = b"original immutable FILE body";
    save(&client, "p/manual.txt", original, true);
    let repository = fixture.repository();
    let entry = fixture.entry(&repository.main_branch_id, "manual.txt");
    drop(client);
    let (root, engine) = fixture.cold_open();
    let web = Management::open(root, engine);
    let history_route = format!("/api/file-manager/files/{}/history", entry.file_id);
    // Read the FILE branch materialized by the real file-manager save.
    // This GET does not create it or substitute the directory main branch.
    let history = web.json("GET", &history_route, None);
    let target = DraftLocation {
        repository: repository.repository_id.parse().unwrap(),
        path: name_policy::PortablePath::parse("manual.txt").unwrap(),
        entry: entry.entry_id.parse().unwrap(),
        file: entry.file_id.parse().unwrap(),
    };
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("repository_id", &repository.repository_id)
        .append_pair("path", "manual.txt")
        .append_pair("entry_id", &entry.entry_id)
        .finish();
    let working_route = format!("/api/file-manager/files/{}/working", entry.file_id);
    let read_working = format!("{working_route}?{query}");
    let initial = web.json("GET", &read_working, None);
    assert_eq!(initial["working"]["state"], "automatic");
    assert_ne!(initial["base"]["branch_id"], repository.main_branch_id);
    web.json("POST", &working_route, Some(json!({
        "operation_id": OperationId::generate().unwrap(),
        "command": { "action": "set_mode", "target": target,
            "expected": initial["working"]["revision"], "mode": "manual" },
    })));
    let clean = web.json("GET", &read_working, None);
    assert_eq!(clean["working"]["state"], "manual_clean");
    let (root, engine) = web.cold_open();
    let mut fixture = Fixture::resume(root, engine);
    let client = fixture.connect();
    let head = fixture.backend.tree_snapshot(&fixture.identity(), ReadTree {
        branch: &repository.main_branch_id, commit: None,
    }).unwrap().commit_id;
    assert_eq!(read(&client, "p/manual.txt").1, original);

    let stale_bytes = b"stale unsaved SMB descriptor bytes";
    let (stale, stale_fd) = open(&client, "p/manual.txt", true, false, false);
    assert!(stale.workspace_id.is_none());
    assert_eq!(stale.version_id.as_deref(), Some(entry.version_id.as_str()));
    stage(&client, stale.handle, &stale_fd, stale_bytes);
    let before = fixture.backend.file_working(&fixture.identity(), target.clone()).unwrap();
    let competing = b"another authorized durable draft";
    let mut competing_fd = tempfile::tempfile().unwrap();
    competing_fd.write_all(competing).unwrap();
    let source = UploadSource::from_file(competing_fd, 1024 * 1024).unwrap();
    fixture.backend.write_file_draft(&fixture.identity(), ChangeFileWorking {
        operation_id: OperationId::generate().unwrap(),
        command: DraftCommand::Write {
            target: target.clone(), expected: before.working.revision(), base: before.base,
            content_hash: source.hash, logical_size: source.size,
        },
    }, &source).unwrap();
    let error = client.call(Request::Flush { handle: stale.handle }).unwrap_err();
    assert_eq!(remote_code(&error), ErrorCode::FileBranchConflict);
    let saved = fixture.backend.file_working(&fixture.identity(), target.clone()).unwrap();
    let pending = custody(fixture.root.path());
    assert_eq!(pending.len(), 1);
    let frozen_request = pending[0].1["publication"]["request"].clone();
    assert_eq!(pending[0].1["publication"]["state"], "pending");
    assert_eq!(frozen_request["command"]["expected"], clean["working"]["revision"]);
    let stale_source = retained(fixture.root.path(), &pending[0].1, stale_bytes);
    let charge = fs::read(stale_source.with_extension("budget.json")).unwrap();
    let error = client.call(Request::Close { handle: stale.handle }).unwrap_err();
    assert_eq!(remote_code(&error), ErrorCode::FileBranchConflict);
    let detached = custody(fixture.root.path());
    assert_eq!(detached[0].1["publication"]["request"], frozen_request);
    assert_eq!(detached[0].1["detached"], true);
    assert_eq!(fs::read(stale_source.with_extension("budget.json")).unwrap(), charge);
    retained(fixture.root.path(), &detached[0].1, stale_bytes);
    assert_eq!(fixture.backend.file_working(&fixture.identity(), target.clone()).unwrap().working,
        saved.working);
    assert_eq!(read(&client, "p/manual.txt").1, competing);
    require_history(&fixture, &repository, &entry, &head);
    drop(stale_fd);

    let first = b"first acknowledged SMB draft";
    let final_bytes = b"only the explicit Web commit may turn this into a FILE version";
    let (writer, fd) = open(&client, "p/manual.txt", true, false, true);
    assert!(writer.workspace_id.is_none());
    stage(&client, writer.handle, &fd, first);
    let flushed = client.call(Request::Flush { handle: writer.handle }).unwrap();
    assert_eq!(flushed["operation_kind"], "file.draft.write");
    assert_eq!(flushed["commit_state"], "PUBLISHED");
    let flushed_working = fixture.backend.file_working(&fixture.identity(), target.clone())
        .unwrap().working;
    client.call(Request::Flush { handle: writer.handle }).unwrap();
    assert_eq!(fixture.backend.file_working(&fixture.identity(), target.clone()).unwrap().working,
        flushed_working);
    let (pinned, placeholder) = open(&client, "p/manual.txt", false, false, false);
    assert!(pinned.version_id.is_none());
    assert!(pinned.workspace_id.is_none());
    stage(&client, writer.handle, &fd, final_bytes);
    // No Flush here: the actual terminal Close must publish the same Draft path.
    close(&client, writer.handle);
    drop(fd);
    let mut pinned_bytes = vec![0; first.len()];
    assert_eq!(client.read(pinned.handle, 0, &mut pinned_bytes).unwrap(), first.len());
    assert_eq!(pinned_bytes, first);
    close(&client, pinned.handle);
    drop(placeholder);
    assert_eq!(read(&client, "p/manual.txt").1, final_bytes);
    assert_eq!(client.stat("p/manual.txt".into()).unwrap().size, final_bytes.len() as u64);
    let (directory, directory_fd) = open(&client, "p", false, false, false);
    let listing: DirectoryPage = json_codec::from_value(client.call(Request::ReadDirectory {
        handle: directory.handle, restart: false,
    }).unwrap()).unwrap();
    assert!(!listing.more);
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].entry.size, final_bytes.len() as u64);
    close(&client, directory.handle);
    drop(directory_fd);
    require_history(&fixture, &repository, &entry, &head);
    assert_eq!(custody(fixture.root.path()).len(), 1, "successful Close cleaned only its own copy");
    let durable = fixture.backend.file_working(&fixture.identity(), target.clone()).unwrap();

    let unsaved = b"disconnected descriptor, not an automatically recovered draft";
    let (unflushed, fd) = open(&client, "p/manual.txt", true, false, false);
    stage(&client, unflushed.handle, &fd, unsaved);
    drop(fd);
    drop(client);
    let (root, engine) = fixture.cold_open();
    assert_eq!(engine.file_working_state(&target).unwrap(), durable.working);
    assert_eq!(engine.file_draft_base(&target).unwrap(), durable.base);
    assert_eq!(engine.file_history(&entry.file_id).unwrap().len(), 1);
    assert_eq!(version_bytes(&engine, &entry.version_id), original);
    let retained_records = custody(root.path());
    assert_eq!(retained_records.len(), 2);
    let unflushed_record = retained_records.iter()
        .find(|(_, record)| record["publication"]["state"] == "open").unwrap();
    retained(root.path(), &unflushed_record.1, unsaved);
    let failed_record = retained_records.iter()
        .find(|(_, record)| record["publication"]["state"] == "pending").unwrap();
    assert_eq!(failed_record.1["publication"]["request"], frozen_request);
    retained(root.path(), &failed_record.1, stale_bytes);
    let mut fixture = Fixture::resume(root, engine);
    let client = fixture.connect();
    // New sessions see the published shared draft, not either orphaned local copy.
    assert_eq!(read(&client, "p/manual.txt").1, final_bytes);
    drop(client);
    let (root, engine) = fixture.cold_open();
    let web = Management::open(root, engine);
    let dirty = web.json("GET", &read_working, None);
    assert_eq!(dirty["working"]["state"], "manual_dirty");
    assert_eq!(web.json("GET", &history_route, None)["nodes"], history["nodes"]);
    let commit = json!({
        "operation_id": OperationId::generate().unwrap(),
        "command": { "action": "commit", "target": target,
            "expected": dirty["working"]["revision"], "base": dirty["base"],
            "message": "explicit authenticated Web commit of SMB draft" },
    });
    let receipt = web.json("POST", &working_route, Some(commit.clone()));
    assert_eq!(receipt["operation_kind"], "file.draft.commit");
    assert_eq!(receipt["commit_state"], "PUBLISHED");
    let committed = web.json("GET", &read_working, None);
    assert_eq!(committed["working"]["state"], "manual_clean");
    assert_ne!(committed["base"]["version_id"], initial["base"]["version_id"]);
    let version = committed["base"]["version_id"].as_str().unwrap().to_owned();
    let (root, engine) = web.cold_open();
    assert_eq!(engine.file_history(&entry.file_id).unwrap().len(), 2);
    assert_eq!(version_bytes(&engine, &entry.version_id), original);
    assert_eq!(version_bytes(&engine, &version), final_bytes);
    let web = Management::open(root, engine);
    assert_eq!(web.json("POST", &working_route, Some(commit)), receipt);
    assert_eq!(web.json("GET", &read_working, None), committed);
    assert_eq!(custody(web.root.path()), retained_records);
    let (root, engine) = web.cold_open();
    assert_eq!(engine.file_history(&entry.file_id).unwrap().len(), 2);
    assert_eq!(version_bytes(&engine, &entry.version_id), original);
    assert_eq!(version_bytes(&engine, &version), final_bytes);
}
