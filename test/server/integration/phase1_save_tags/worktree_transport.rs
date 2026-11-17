//! Actual loopback HTTP, held-source edit and existing MOVE/COPY publication.
//! No Samba daemon or native SMB wire execution is claimed by this target.
use super::{Fixture, router};
use http_routing::Router;
use json_codec::{Value, json};
use name_policy::LookupPath;
use std::{collections::BTreeMap, fs, net::SocketAddr, os::unix::fs::MetadataExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use storage_contract::OperationId;
use storage_contract::snapshot_worktree::{
    CommitWorktree, EditWorktree, SnapshotHandle, WorktreeHandle, WorktreeStatus,
};
use storage_contract::snapshot_worktree::creation::{
    AllocateWorktree, WorktreeCreationRequest, WorktreeNaming,
};
use storage_contract::snapshot_worktree::outcome::{
    WorktreeCommitOutcome, WorktreeCommitReceipt, WorktreeCreationOutcome, WorktreeCreationReceipt,
};
use storage_engine::Engine;
use storage_server::auth::AuthStore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Http {
    address: SocketAddr,
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
    token: String,
}

impl Http {
    async fn start(app: Router, token: &str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            http_routing::serve(listener, app)
                .with_graceful_shutdown(async { let _ = stopped.await; })
                .await.unwrap();
        });
        Self { address, stop, task, token: token.into() }
    }

    async fn raw(
        &self, authenticated: bool, method: &str, route: &str,
        headers: &[(&str, String)], body: &[u8],
    ) -> (u16, Vec<u8>) {
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut connection = tokio::net::TcpStream::connect(self.address).await.unwrap();
            let mut head = format!(
                "{method} {route} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
                self.address,
            );
            if authenticated {
                head.push_str(&format!("Authorization: Bearer {}\r\n", self.token));
            }
            if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("content-length")) {
                head.push_str(&format!("Content-Length: {}\r\n", body.len()));
            }
            for (name, value) in headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            head.push_str("\r\n");
            connection.write_all(head.as_bytes()).await.unwrap();
            connection.write_all(body).await.unwrap();
            let mut response = Vec::new();
            connection.take(2 * 1024 * 1024).read_to_end(&mut response).await.unwrap();
            assert!(response.len() < 2 * 1024 * 1024);
            let split = response.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap();
            let header = std::str::from_utf8(&response[..split]).unwrap();
            assert!(!header.to_ascii_lowercase().contains("transfer-encoding:"));
            let status = header.split_whitespace().nth(1).unwrap().parse().unwrap();
            (status, response[split + 4..].to_vec())
        }).await.expect("actual HTTP fixture exceeded its request deadline")
    }

    async fn json(&self, method: &str, route: &str, value: Option<Value>) -> Value {
        let body = value.map(|value| json_codec::to_vec(&value).unwrap()).unwrap_or_default();
        let (status, bytes) = self.raw(true, method, route,
            &[("content-type", "application/json".into())], &body).await;
        let value: Value = json_codec::from_slice(&bytes).unwrap();
        assert_eq!(status, 200, "{method} {route}: {value}");
        value
    }

    async fn edit(&self, request: &EditWorktree, bytes: &[u8]) -> (u16, Value) {
        let (status, body) = self.raw(true, "PUT", &content_route(request.target), &[
            ("x-zxs-worktree-edit", json_codec::to_string(request).unwrap()),
        ], bytes).await;
        (status, json_codec::from_slice(&body).unwrap())
    }

    async fn read(&self, target: WorktreeHandle, path: &str) -> Vec<u8> {
        let query = url_encoding::form::Serializer::new(String::new())
            .append_pair("publication", &target.publication.to_string())
            .append_pair("path", path).finish();
        let (status, body) = self.raw(true, "GET",
            &format!("{}?{query}", content_route(target)), &[], &[]).await;
        assert_eq!(status, 200, "content read: {}", String::from_utf8_lossy(&body));
        body
    }

    async fn status(&self, target: WorktreeHandle) -> WorktreeStatus {
        json_codec::from_value(self.json("GET", &format!(
            "/api/directory-worktrees/{}?publication={}", target.worktree_id, target.publication,
        ), None).await).unwrap()
    }

    async fn close(self) {
        self.stop.send(()).unwrap();
        self.task.await.unwrap();
    }
}

fn content_route(target: WorktreeHandle) -> String {
    format!("/api/directory-worktrees/{}/content", target.worktree_id)
}
fn commit_route(target: WorktreeHandle) -> String {
    format!("/api/directory-worktrees/{}/commits", target.worktree_id)
}

async fn cold(fixture: Fixture) -> (tempfile::TempDir, Engine, String) {
    let Fixture { root, app, token } = fixture;
    drop(app);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match Engine::open(&root.path().join("system")) {
            Ok(engine) => return (root, engine, token),
            Err(error) if error.code() == storage_contract::ErrorCode::WriterAlreadyActive => {
                let left = deadline.saturating_duration_since(Instant::now());
                assert!(!left.is_zero(), "HTTP retained its writer: {error}");
                tokio::time::sleep(left.min(Duration::from_millis(10))).await;
            }
            Err(error) => panic!("cold worktree reopen failed: {error:?}"),
        }
    }
}
fn resume(root: tempfile::TempDir, engine: Engine, token: String) -> Fixture {
    let system = root.path().join("system");
    let auth = AuthStore::open(&system).unwrap();
    Fixture { root, app: router(engine, auth, &system), token }
}

fn inventory(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, here: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(here).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), output);
            } else {
                output.insert(entry.path().strip_prefix(root).unwrap().into(),
                    fs::read(entry.path()).unwrap());
            }
        }
    }
    let mut output = BTreeMap::new();
    visit(root, root, &mut output);
    output
}

async fn initialized(http: &Http, repository: &str) -> (Value, SnapshotHandle) {
    let context = http.json("GET",
        &format!("/api/repositories/{repository}/snapshot-root-context"), None).await;
    let snapshot = http.json("POST", "/api/directory-snapshots", Some(json!({
        "host":context["host"], "source":context["source"],
        "operation_id":OperationId::generate().unwrap(),
    }))).await;
    (context, json_codec::from_value(snapshot).unwrap())
}
async fn instance(
    http: &Http, repository: &str, context: &Value, source: SnapshotHandle, expected_name: &str,
) -> WorktreeHandle {
    let request: AllocateWorktree = json_codec::from_value(json!({
        "source":source, "parent_directory_id":context["host"]["directory_id"],
        "expected_root_generation":context["host"]["expected_root_generation"],
        "naming":WorktreeNaming::Automatic, "operation_id":OperationId::generate().unwrap(),
    })).unwrap();
    let target = json_codec::from_value(http.json("POST", "/api/directory-worktrees",
        Some(json_codec::to_value(&request).unwrap())).await).unwrap();
    let receipt: WorktreeCreationReceipt = json_codec::from_value(http.json("GET", &format!(
        "/api/repositories/{repository}/directory-worktrees/creations/{}", request.operation_id,
    ), None).await).unwrap();
    assert_eq!(receipt.repository_id, repository.parse().unwrap());
    assert_eq!(receipt.request, WorktreeCreationRequest::Automatic(request.clone()));
    assert_eq!(receipt.result, WorktreeCreationOutcome::Complete { target });
    assert_eq!(http.status(target).await.name.as_str(), expected_name);
    let retried: WorktreeHandle = json_codec::from_value(http.json(
        "POST", "/api/directory-worktrees", Some(json_codec::to_value(&request).unwrap()),
    ).await).unwrap();
    assert_eq!(retried, target);
    assert_ne!(target.publication, request.operation_id);
    target
}

#[tokio::test]
async fn tcp_worktree_edit_commit_cold_retry_keeps_exact_identity_and_move() {
    let fixture = Fixture::new(&["primary"]).await;
    let original = b"original body remains immutable";
    let untouched = b"unchanged body copied independently";
    fixture.save("D01", "original.txt", original).await;
    fixture.save("D01", "untouched.txt", untouched).await;
    let entry = fixture.entry("D01", "original.txt").await;
    let history = fixture.history(entry["file_id"].as_str().unwrap()).await;
    let repository = history["identity"]["context"]["repository_id"].as_str().unwrap();
    let http = Http::start(fixture.app.clone(), &fixture.token).await;
    let (context, original_node) = initialized(&http, repository).await;
    let target = instance(&http, repository, &context, original_node, "worktree-main-001").await;
    let independent = instance(
        &http, repository, &context, original_node, "worktree-main-002",
    ).await;
    assert_ne!(target.worktree_id, independent.worktree_id);
    assert_eq!(http.read(target, "original.txt").await, original);
    assert_eq!(http.read(independent, "original.txt").await, original);
    let status = http.status(target).await;
    let bytes = (0..70_003).map(|index| (index % 251) as u8).collect::<Vec<_>>();
    let edit = EditWorktree {
        target, path: name_policy::PortablePath::parse("original.txt").unwrap(), expected: status.revision,
        size: bytes.len() as u64,
        content_hash: storage_contract::ContentHash::from_digest(blake3::hash(&bytes)),
        operation_id: OperationId::generate().unwrap(),
    };
    let (code, saved) = http.edit(&edit, &bytes).await;
    assert_eq!(code, 200, "{saved}");
    let revision = json_codec::from_value(saved.clone()).unwrap();
    assert_eq!(http.edit(&edit, &bytes).await, (200, saved.clone()));
    assert_eq!(http.read(target, "original.txt").await, bytes);
    assert_eq!(http.read(target, "untouched.txt").await, untouched);
    assert_eq!(http.read(independent, "original.txt").await, original);
    let (partial, body) = http.raw(true, "GET", &format!(
        "{}?publication={}&path=original.txt", content_route(target), target.publication,
    ), &[("range", "bytes=65530-65545".into())], &[]).await;
    assert_eq!(partial, 206);
    assert_eq!(body, bytes[65530..65546]);
    let (first_edit, first_saved, first_bytes) = (edit, saved, bytes);
    let bytes = (0..80_009).map(|index| ((index + 19) % 241) as u8).collect::<Vec<_>>();
    let edit = EditWorktree {
        target, path: first_edit.path.clone(), expected: revision,
        size: bytes.len() as u64,
        content_hash: storage_contract::ContentHash::from_digest(blake3::hash(&bytes)),
        operation_id: OperationId::generate().unwrap(),
    };
    let (code, saved) = http.edit(&edit, &bytes).await;
    assert_eq!(code, 200, "{saved}");
    let revision = json_codec::from_value(saved.clone()).unwrap();
    assert_eq!(http.read(target, "original.txt").await, bytes);
    assert_eq!(http.read(independent, "original.txt").await, original);
    http.close().await;
    let (root, engine, token) = cold(fixture).await;
    let old_version = entry["version_id"].as_str().unwrap();
    let file = entry["file_id"].as_str().unwrap();
    let selected = engine.file_context(repository, "original.txt").unwrap();
    let original_head = engine.file_branch_head(&selected.branch_id).unwrap();
    assert_eq!(engine.state().versions.values()
        .filter(|version| version.file_id == file).count(), 1);
    assert_eq!(original_head, old_version);
    let path = LookupPath::parse("original.txt").unwrap();
    let opened = engine.open_directory_worktree_content(target, &path).unwrap();
    let metadata = opened.file.metadata().unwrap();
    let moved_inode = (metadata.dev(), metadata.ino());
    drop(opened);
    let original_unchanged = engine.open_directory_worktree_content(
        independent, &LookupPath::parse("untouched.txt").unwrap(),
    ).unwrap();
    let metadata = original_unchanged.file.metadata().unwrap();
    let copied_from = (metadata.dev(), metadata.ino());
    drop(original_unchanged);
    let fixture = resume(root, engine, token);
    let http = Http::start(fixture.app.clone(), &fixture.token).await;
    assert_eq!(http.edit(&first_edit, &first_bytes).await, (200, first_saved));
    assert_eq!(http.edit(&edit, &bytes).await, (200, saved));
    assert_eq!(http.read(target, "original.txt").await, bytes);
    let commit = CommitWorktree {
        target, expected: revision, message: "one actual explicit worktree commit".into(),
        operation_id: OperationId::generate().unwrap(),
    };
    let committed = http.json("POST", &commit_route(target),
        Some(json_codec::to_value(&commit).unwrap())).await;
    let node: SnapshotHandle = json_codec::from_value(committed.clone()).unwrap();
    let outcome_route = format!(
        "/api/repositories/{repository}/directory-worktrees/commits/{}", commit.operation_id,
    );
    let outcome: WorktreeCommitReceipt = json_codec::from_value(
        http.json("GET", &outcome_route, None).await,
    ).unwrap();
    assert_eq!(outcome.repository_id, repository.parse().unwrap());
    assert_eq!(outcome.request, commit);
    assert_eq!(outcome.result, WorktreeCommitOutcome::Complete { snapshot: node });
    assert_ne!(node.publication, commit.operation_id);
    assert_eq!(node.repository_id, original_node.repository_id);
    assert_eq!(node.branch_id, original_node.branch_id);
    assert_ne!(node.node_id, original_node.node_id);
    assert_eq!(http.read(target, "original.txt").await, bytes);
    assert_eq!(http.read(independent, "original.txt").await, original);
    http.close().await;
    let (root, engine, token) = cold(fixture).await;
    assert_eq!(engine.state().versions.values()
        .filter(|version| version.file_id == file).count(), 2);
    let original_record = engine.state().versions[old_version].clone();
    let successor = engine.file_branch_head(&selected.branch_id).unwrap();
    assert_ne!(successor, original_head);
    assert_eq!(engine.state().versions[&successor].file_id, file);
    assert_eq!(engine.state().versions[&successor].source_version_ids, [old_version]);
    let opened = engine.open_directory_worktree_content(target, &path).unwrap();
    let metadata = opened.file.metadata().unwrap();
    assert_eq!((metadata.dev(), metadata.ino()), moved_inode);
    drop(opened);
    let opened = engine.open_directory_worktree_content(
        target, &LookupPath::parse("untouched.txt").unwrap(),
    ).unwrap();
    let metadata = opened.file.metadata().unwrap();
    assert_ne!((metadata.dev(), metadata.ino()), copied_from);
    drop(opened);
    let sequence = engine.state().sequence;
    let receipt = engine.operation(commit.operation_id).unwrap();
    assert!(engine.directory_worktree_commit_receipt(
        repository.parse().unwrap(), commit.operation_id, &"another-owner".parse().unwrap(),
    ).is_err());
    let fixture = resume(root, engine, token);
    let http = Http::start(fixture.app.clone(), &fixture.token).await;
    let before = inventory(&fixture.root.path().join("primary/common-data"));
    let cold_outcome: WorktreeCommitReceipt = json_codec::from_value(
        http.json("GET", &outcome_route, None).await,
    ).unwrap();
    assert_eq!(cold_outcome, outcome);
    assert_eq!(inventory(&fixture.root.path().join("primary/common-data")), before);
    assert_eq!(http.json("POST", &commit_route(target),
        Some(json_codec::to_value(&commit).unwrap())).await, committed);
    assert_eq!(http.read(target, "original.txt").await, bytes);
    assert_eq!(inventory(&fixture.root.path().join("primary/common-data")), before);
    http.close().await;
    let (_root, engine, _token) = cold(fixture).await;
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(engine.operation(commit.operation_id).unwrap(), receipt);
    assert_eq!(engine.state().versions[old_version], original_record);
}

#[tokio::test]
async fn tcp_worktree_auth_shape_and_rejected_body_leave_current_bytes_unchanged() {
    let fixture = Fixture::new(&["primary"]).await;
    fixture.save("D01", "original.txt", b"untouched").await;
    fixture.save("D01", "second.txt", b"another original occurrence").await;
    let entry = fixture.entry("D01", "original.txt").await;
    let history = fixture.history(entry["file_id"].as_str().unwrap()).await;
    let repository = history["identity"]["context"]["repository_id"].as_str().unwrap();
    let http = Http::start(fixture.app.clone(), &fixture.token).await;
    let (context, node) = initialized(&http, repository).await;
    let target = instance(&http, repository, &context, node, "worktree-main-001").await;
    // A real, otherwise valid Named request cannot enter the public writer.
    // Original Named receipts remain queryable; this POST accepts automatic intent only.
    let named_operation = OperationId::generate().unwrap();
    let named = storage_contract::snapshot_worktree::CreateWorktree {
        source: node,
        parent_directory_id: json_codec::from_value(context["host"]["directory_id"].clone())
            .unwrap(),
        expected_root_generation: json_codec::from_value(
            context["host"]["expected_root_generation"].clone(),
        ).unwrap(),
        name: name_policy::Name::parse("client-chosen-name").unwrap(),
        operation_id: named_operation,
    };
    let all_before = inventory(fixture.root.path());
    let (status, _) = http.raw(true, "POST", "/api/directory-worktrees", &[
        ("content-type", "application/json".into()),
    ], &json_codec::to_vec(&named).unwrap()).await;
    assert_eq!(status, 400);
    assert_eq!(inventory(fixture.root.path()), all_before);
    let before = inventory(&fixture.root.path().join("primary/common-data"));
    let command = EditWorktree {
        target, path: name_policy::PortablePath::parse("original.txt").unwrap(), expected: http.status(target).await.revision,
        size: 7, content_hash: storage_contract::ContentHash::from_digest(blake3::hash(b"changed")),
        operation_id: OperationId::generate().unwrap(),
    };
    let encoded = json_codec::to_string(&command).unwrap();
    let route = content_route(target);
    let headers = [("x-zxs-worktree-edit", encoded.clone())];
    assert_eq!(http.raw(false, "PUT", &route, &headers, b"changed").await.0, 401);
    assert_eq!(http.raw(false, "GET", &format!(
        "{route}?publication={}&path=original.txt", target.publication,
    ), &[], &[]).await.0, 401);
    assert_eq!(http.edit(&command, b"corrupt").await.0, 400);
    assert_eq!(http.edit(&command, b"short").await.0, 400);
    assert_eq!(http.raw(true, "PUT", &route, &[
        ("x-zxs-worktree-edit", encoded.clone()), ("x-zxs-worktree-edit", encoded),
    ], b"changed").await.0, 400);
    let mut unknown = json_codec::to_value(&command).unwrap();
    unknown["source_path"] = "/not/an/authorized/body".into();
    assert_eq!(http.raw(true, "PUT", &route, &[
        ("x-zxs-worktree-edit", json_codec::to_string(&unknown).unwrap()),
    ], b"changed").await.0, 400);
    let mut wrong = command.clone();
    wrong.target.worktree_id = storage_contract::snapshot_worktree::WorktreeId::generate().unwrap();
    assert_eq!(http.raw(true, "PUT", &route, &[
        ("x-zxs-worktree-edit", json_codec::to_string(&wrong).unwrap()),
    ], b"changed").await.0, 400);
    assert_eq!(inventory(&fixture.root.path().join("primary/common-data")), before);
    assert_eq!(http.read(target, "original.txt").await, b"untouched");
    let (status, first) = http.edit(&command, b"changed").await;
    assert_eq!(status, 200, "{first}");
    let after = inventory(&fixture.root.path().join("primary/common-data"));
    let mut subsequent = command.clone();
    subsequent.operation_id = OperationId::generate().unwrap();
    subsequent.path = name_policy::PortablePath::parse("second.txt").unwrap();
    subsequent.expected = json_codec::from_value(first).unwrap();
    let (status, _) = http.edit(&subsequent, b"changed").await;
    assert_ne!(status, 200, "this slice does not yet support editing a second original occurrence");
    assert_eq!(inventory(&fixture.root.path().join("primary/common-data")), after);
    assert_eq!(http.read(target, "original.txt").await, b"changed");
    http.close().await;
    let (_root, engine, _token) = cold(fixture).await;
    assert!(engine.operation(named_operation).is_err());
    assert!(engine.operation(subsequent.operation_id).is_err());
}
