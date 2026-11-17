//! Real authenticated Born selection, one native decision and cold scoped retry.
//! No mock backend or decoded native receipt is used to claim publication.
use super::{Fixture, json_response, request, router};
use http_routing::{body::Body, http::{Request, StatusCode}};
use json_codec::{Value, json};
use service_stack::ServiceExt;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use storage_contract::{Config, Mode, OperationId};
use storage_contract::snapshot_worktree::draft_commit::{
    CommitDirectoryDrafts, DirectoryDraftOutcome, DirectoryDraftReceipt,
};
use storage_engine::Engine;
use storage_server::auth::AuthStore;

const ROUTE: &str = "/api/directory-snapshots/commits";

async fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("system");
    let config = Config::independent_roots(Mode::Development, [
        (name_policy::Name::parse("Directory primary").unwrap(), root.path().join("primary")),
        (name_policy::Name::parse("Directory other owner").unwrap(), root.path().join("other")),
    ]).unwrap();
    let engine = Engine::initialize(&system, config).unwrap();
    let username = format!("directory-{}", OperationId::generate().unwrap());
    let password = authentication::random_token().unwrap();
    let auth = AuthStore::initialize(&system, &username, &password).unwrap();
    let app = router(engine, auth, &system);
    let login = request(&app, None, "POST", "/api/auth/login", Some(json!({
        "username": username, "password": password,
    }))).await;
    Fixture { root, app, token: login["token"].as_str().unwrap().into() }
}

async fn response(
    fixture: &Fixture, authenticated: bool, method: &str, route: &str, body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(route);
    if authenticated {
        builder = builder.header("authorization", format!("Bearer {}", fixture.token));
    }
    let body = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            Body::from(json_codec::to_vec(&body).unwrap())
        }
        None => Body::empty(),
    };
    let response = fixture.app.clone().oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, json_codec::from_slice(&bytes).unwrap())
}

async fn target(fixture: &Fixture, slot: &str, path: &str) -> (Value, Value) {
    let entry = fixture.entry(slot, path).await;
    let history = fixture.history(entry["file_id"].as_str().unwrap()).await;
    (json!({
        "repository": history["identity"]["context"]["repository_id"],
        "path": path, "entry": entry["entry_id"], "file": entry["file_id"],
    }), entry)
}

fn query(target: &Value) -> String {
    url_encoding::form::Serializer::new(String::new())
        .append_pair("repository_id", target["repository"].as_str().unwrap())
        .append_pair("path", target["path"].as_str().unwrap())
        .append_pair("entry_id", target["entry"].as_str().unwrap()).finish()
}

async fn working(fixture: &Fixture, target: &Value) -> Value {
    fixture.json("GET", &format!("/api/file-manager/files/{}/working?{}",
        target["file"].as_str().unwrap(), query(target)), None).await
}

async fn draft(fixture: &Fixture, target: &Value) -> Vec<u8> {
    let response = fixture.app.clone().oneshot(Request::builder()
        .uri(format!("/api/file-manager/files/{}/draft?{}",
            target["file"].as_str().unwrap(), query(target)))
        .header("authorization", format!("Bearer {}", fixture.token))
        .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    http_routing::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap().to_vec()
}

async fn save_draft(fixture: &Fixture, target: &Value, body: &[u8]) -> Value {
    let mut state = working(fixture, target).await;
    let route = format!("/api/file-manager/files/{}/working", target["file"].as_str().unwrap());
    if state["working"]["state"] == "automatic" {
        fixture.json("POST", &route, Some(json!({
            "operation_id": OperationId::generate().unwrap(),
            "command": {"action":"set_mode", "target":target,
                "expected":state["working"]["revision"], "mode":"manual"},
        }))).await;
        state = working(fixture, target).await;
    }
    let command = json!({
        "operation_id": OperationId::generate().unwrap(),
        "command": {"action":"write", "target":target,
            "expected":state["working"]["revision"], "base":state["base"],
            "logical_size":body.len().to_string(),
            "content_hash":blake3::hash(body).to_hex().as_str()},
    });
    let response = fixture.app.clone().oneshot(Request::builder().method("PUT")
        .uri(format!("/api/file-manager/files/{}/draft", target["file"].as_str().unwrap()))
        .header("authorization", format!("Bearer {}", fixture.token))
        .header("x-zxs-draft-command", json_codec::to_string(&command).unwrap())
        .header("content-length", body.len().to_string())
        .body(Body::from(body.to_vec())).unwrap()).await.unwrap();
    json_response(response).await;
    let state = working(fixture, target).await;
    assert_eq!(state["working"]["state"], "manual_dirty");
    json!({"target":target, "expected":{
        "revision":state["working"]["revision"],
        "base":state["working"]["base"], "payload":state["working"]["payload"],
    }})
}

async fn initialize(fixture: &Fixture, target: &Value) -> Value {
    let context = fixture.json("GET", &format!("/api/repositories/{}/snapshot-root-context",
        target["repository"].as_str().unwrap()), None).await;
    fixture.json("POST", "/api/directory-snapshots", Some(json!({
        "host":context["host"], "source":context["source"],
        "operation_id":OperationId::generate().unwrap(),
    }))).await
}

fn lookup(source: &Value, operation: &Value) -> String {
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("source_publication", source["publication"].as_str().unwrap())
        .append_pair("repository_id", source["repository_id"].as_str().unwrap())
        .append_pair("branch_id", source["branch_id"].as_str().unwrap())
        .append_pair("node_id", source["node_id"].as_str().unwrap()).finish();
    format!("{ROUTE}/{}?{query}", operation.as_str().unwrap())
}

type Inventory = BTreeMap<PathBuf, (u64, u64, u32, Vec<u8>)>;
fn inventory(root: &Path) -> Inventory {
    fn walk(root: &Path, path: &Path, out: &mut Inventory) {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(!metadata.file_type().is_symlink());
        let bytes = if metadata.is_file() { fs::read(path).unwrap() } else { Vec::new() };
        out.insert(path.strip_prefix(root).unwrap().to_path_buf(),
            (metadata.dev(), metadata.ino(), metadata.mode(), bytes));
        if metadata.is_dir() {
            for child in fs::read_dir(path).unwrap() { walk(root, &child.unwrap().path(), out); }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}

fn no_native_fields(value: &Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                assert!(!["binding", "host", "root_entity", "inode", "device", "trace",
                    "working_after", "contents", "construction", "registration", "selected"]
                    .contains(&key.as_str()), "private field {key}");
                no_native_fields(value);
            }
        }
        Value::Array(values) => { for value in values { no_native_fields(value); } }
        _ => {}
    }
}

async fn assert_generic_shell(fixture: &Fixture, receipt: &Value) {
    for suffix in ["", "/outcome"] {
        let value = fixture.json("GET", &format!("/api/operations/{}{suffix}",
            receipt["operation_id"].as_str().unwrap()), None).await;
        assert_eq!(value["operation_kind"], "snapshot.working.commit");
        assert_eq!(value["operation_id"], receipt["operation_id"]);
        assert_eq!(value["commit_state"], receipt["commit_state"]);
        assert_eq!(value["publication_sequence"], receipt["publication_sequence"]);
        assert_eq!(value["request_digest"].as_str().unwrap().len(), 64);
        assert!(value["result"].is_null());
        no_native_fields(&value);
    }
}

// History pagination is bound to a published history root, which can advance
// even when all of this FILE's nodes, head, and graph remain unchanged.
fn assert_same_file_history(actual: &Value, expected: &Value) {
    let without_root = |value: &Value| {
        let mut value = value.clone();
        let removed = value["identity"].as_object_mut().unwrap().remove("history_root").unwrap();
        assert_eq!(removed.as_str().unwrap().len(), 64);
        value
    };
    assert_eq!(without_root(actual), without_root(expected));
}

#[tokio::test]
async fn authenticated_directory_draft_commit_is_scoped_selected_and_cold_idempotent() {
    let fixture = fixture().await;
    fixture.save("D01", "one.txt", b"one original").await;
    fixture.save("D01", "two.txt", b"two original").await;
    fixture.save("D02", "other.txt", b"other original").await;
    let (one, old_one) = target(&fixture, "D01", "one.txt").await;
    let (two, old_two) = target(&fixture, "D01", "two.txt").await;
    let (other, _) = target(&fixture, "D02", "other.txt").await;
    let source = initialize(&fixture, &one).await;
    let foreign_source = initialize(&fixture, &other).await;
    let first = save_draft(&fixture, &one, b"one explicit directory change").await;
    save_draft(&fixture, &two, b"unselected draft must survive").await;
    let foreign = save_draft(&fixture, &other, b"other directory draft").await;
    let fixture = fixture.reopen().await;
    let unselected = working(&fixture, &two).await;
    let foreign_working = working(&fixture, &other).await;
    let before_one = fixture.history(one["file"].as_str().unwrap()).await;
    let before_two = fixture.history(two["file"].as_str().unwrap()).await;
    let zxs = fixture.root.path().join("primary/common-data/.zxs");
    let old_node_bytes = inventory(&zxs);
    let command = json!({"source":source, "selected":[first],
        "message":"selected directory commit", "operation_id":OperationId::generate().unwrap()});
    assert_eq!(response(&fixture, false, "POST", ROUTE, Some(command.clone())).await.0,
        StatusCode::UNAUTHORIZED);
    let mut malformed = command.clone();
    malformed["selected"][0]["expected"]["inode"] = json!("1");
    assert_eq!(response(&fixture, true, "POST", ROUTE, Some(malformed)).await.0,
        StatusCode::BAD_REQUEST);
    let mut positional = command.clone();
    positional["selected"][0]["expected"] = json!([]);
    assert_eq!(response(&fixture, true, "POST", ROUTE, Some(positional)).await.0,
        StatusCode::BAD_REQUEST);
    let mut mixed = command.clone();
    mixed["selected"].as_array_mut().unwrap().push(foreign.clone());
    assert_eq!(response(&fixture, true, "POST", ROUTE, Some(mixed)).await.0,
        StatusCode::FORBIDDEN);
    let mut stale = command.clone();
    stale["selected"][0]["expected"]["revision"] = json!("0");
    let rejected = response(&fixture, true, "POST", ROUTE, Some(stale)).await;
    assert!(!rejected.0.is_success());
    assert_eq!(rejected.1["error"]["code"], "FILE_BRANCH_CONFLICT");
    assert_eq!(inventory(&zxs), old_node_bytes);
    assert_eq!(fixture.history(one["file"].as_str().unwrap()).await, before_one);
    assert_eq!(fixture.history(two["file"].as_str().unwrap()).await, before_two);
    assert_eq!(working(&fixture, &two).await, unselected);
    assert_eq!(working(&fixture, &other).await, foreign_working);

    let changed = fixture.json("POST", ROUTE, Some(command.clone())).await;
    let typed: DirectoryDraftReceipt = json_codec::from_value(changed.clone()).unwrap();
    assert!(matches!(typed.result, DirectoryDraftOutcome::Changed { .. }));
    assert_eq!(changed["result"]["outcome"], "changed");
    assert_ne!(changed["result"]["snapshot"]["node_id"], source["node_id"]);
    no_native_fields(&changed);
    let new_one = fixture.entry("D01", "one.txt").await;
    assert_ne!(new_one["version_id"], old_one["version_id"]);
    assert_eq!(fixture.body(new_one["version_id"].as_str().unwrap()).await,
        b"one explicit directory change");
    assert_eq!(fixture.body(old_one["version_id"].as_str().unwrap()).await, b"one original");
    assert_eq!(fixture.entry("D01", "two.txt").await["version_id"], old_two["version_id"]);
    let after_two = fixture.history(two["file"].as_str().unwrap()).await;
    assert_same_file_history(&after_two, &before_two);
    assert_eq!(working(&fixture, &two).await, unselected);
    assert_eq!(draft(&fixture, &two).await, b"unselected draft must survive");
    let after = inventory(&zxs);
    assert!(after.len() > old_node_bytes.len());
    for (path, entity) in &old_node_bytes { assert_eq!(after.get(path), Some(entity)); }
    let exact = lookup(&source, &changed["operation_id"]);
    assert_eq!(fixture.json("GET", &exact, None).await, changed);
    assert_eq!(response(&fixture, false, "GET", &exact, None).await.0,
        StatusCode::UNAUTHORIZED);
    let wrong = lookup(&foreign_source, &changed["operation_id"]);
    let denied = response(&fixture, true, "GET", &wrong, None).await;
    assert_eq!(denied.1["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
    assert_generic_shell(&fixture, &changed).await;

    let committed_history = fixture.history(one["file"].as_str().unwrap()).await;
    let committed_working = working(&fixture, &one).await;
    let fixture = fixture.reopen().await;
    assert_eq!(fixture.json("GET", &exact, None).await, changed);
    assert_eq!(fixture.json("POST", ROUTE, Some(command.clone())).await, changed);
    assert_eq!(inventory(&zxs), after);
    assert_eq!(fixture.history(one["file"].as_str().unwrap()).await, committed_history);
    assert_eq!(working(&fixture, &one).await, committed_working);
    assert_eq!(fixture.history(two["file"].as_str().unwrap()).await, after_two);
    assert_eq!(working(&fixture, &two).await, unselected);
    assert_eq!(draft(&fixture, &two).await, b"unselected draft must survive");
    let mut collision = command;
    collision["message"] = json!("different request");
    let denied = response(&fixture, true, "POST", ROUTE, Some(collision)).await;
    assert_eq!(denied.1["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
    assert_generic_shell(&fixture, &changed).await;

    let equal = save_draft(&fixture, &one, b"one explicit directory change").await;
    let current = changed["result"]["snapshot"].clone();
    let unchanged_command = json!({"source":current, "selected":[equal],
        "message":"validate identical selected draft",
        "operation_id":OperationId::generate().unwrap(),
    });
    let prior_history = fixture.history(one["file"].as_str().unwrap()).await;
    let unchanged = fixture.json("POST", ROUTE, Some(unchanged_command.clone())).await;
    assert_eq!(unchanged["result"], json!({"outcome":"unchanged", "snapshot":current}));
    assert_eq!(inventory(&zxs), after);
    assert_same_file_history(
        &fixture.history(one["file"].as_str().unwrap()).await, &prior_history,
    );
    assert_eq!(working(&fixture, &one).await["working"]["state"], "manual_clean");
    assert_eq!(working(&fixture, &two).await, unselected);
    let unchanged_lookup = lookup(&current, &unchanged["operation_id"]);
    let fixture = fixture.reopen().await;
    assert_eq!(fixture.json("GET", &unchanged_lookup, None).await, unchanged);
    assert_eq!(fixture.json("POST", ROUTE, Some(unchanged_command)).await, unchanged);
    assert_eq!(inventory(&zxs), after);
    assert_eq!(working(&fixture, &two).await, unselected);
    assert_generic_shell(&fixture, &unchanged).await;

    // A second real author uses the same production command, not an injected
    // receipt. The current HTTP administrator must not inspect that operation.
    let foreign_command: CommitDirectoryDrafts = json_codec::from_value(json!({
        "source":foreign_source, "selected":[foreign], "message":"another author",
        "operation_id":OperationId::generate().unwrap(),
    })).unwrap();
    let Fixture { root, app, token } = fixture;
    drop(app);
    let system = root.path().join("system");
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut engine = loop {
        match Engine::open(&system) {
            Ok(engine) => break engine,
            Err(error) if error.code() == storage_contract::ErrorCode::WriterAlreadyActive => {
                assert!(Instant::now() < deadline, "HTTP engine did not release its writer");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!("cold source failed: {error}"),
        }
    };
    let foreign_receipt = engine.commit_directory_drafts(
        &foreign_command, &"independent-author".parse().unwrap(),
    ).unwrap();
    let auth = AuthStore::open(&system).unwrap();
    let app = router(engine, auth, &system);
    let fixture = Fixture { root, app, token };
    let route = lookup(&foreign_source, &json!(foreign_receipt.operation_id));
    assert_eq!(response(&fixture, true, "GET", &route, None).await.0, StatusCode::FORBIDDEN);
    for suffix in ["", "/outcome"] {
        let route = format!("/api/operations/{}{suffix}", foreign_receipt.operation_id);
        assert_eq!(response(&fixture, true, "GET", &route, None).await.0,
            StatusCode::FORBIDDEN);
    }
    assert_eq!(working(&fixture, &two).await, unselected);
    assert_eq!(draft(&fixture, &two).await, b"unselected draft must survive");
}

fn native_file(
    engine: &mut Engine, repository: storage_contract::RepositoryId,
    path: &str, bytes: Option<&[u8]>,
) -> storage_contract::file_draft::DraftLocation {
    use storage_contract::file_draft::{BirthRequest, DraftLocation};
    let branch = engine.state().repositories[&repository.to_string()].main_branch_id.clone();
    let request = BirthRequest {
        repository, path: name_policy::PortablePath::parse(path).unwrap(),
        expected_namespace_head: engine.state().branches[&branch].head_commit_id.parse().unwrap(),
    };
    let operation = OperationId::generate().unwrap();
    let principal = "admin".parse().unwrap();
    engine.create_unborn_file(&request, &principal, operation).unwrap();
    let (file, record) = engine.state().file_containers.iter()
        .find(|(_, record)| record.created_by == operation).unwrap();
    let target = DraftLocation {
        repository, path: request.path, entry: record.namespace.entry, file: *file,
    };
    if let Some(bytes) = bytes {
        let mut handle = engine.open_native_working(&target, &principal, true).unwrap();
        assert_eq!(engine.write_native_working(&mut handle, &principal, 0, bytes).unwrap(),
            bytes.len());
        engine.flush_native_working(&mut handle, &principal).unwrap().unwrap();
    }
    assert!(engine.unborn_working_state(&target).is_ok());
    assert!(engine.state().versions.values().all(|version| {
        version.file_id != target.file.to_string()
    }));
    target
}

#[tokio::test]
async fn authenticated_mixed_directory_commit_publishes_once_and_keeps_unselected_drafts() {
    use storage_contract::snapshot_worktree::InitializeSnapshot;
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("system");
    let config = Config::independent_roots(Mode::Development, [
        (name_policy::Name::parse("Mixed primary").unwrap(), root.path().join("primary")),
    ]).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repo = engine.create_disk_root(&slot, "admin", OperationId::generate().unwrap()).unwrap();
    let repository = repo.repository_id.parse().unwrap();
    let workspace = engine.create_workspace(&repo.main_branch_id, "admin").unwrap();
    for (path, bytes) in [("existing.txt", b"existing original".as_slice()),
        ("unselected.txt", b"unselected original".as_slice())]
    {
        engine.stage_bytes(&workspace.workspace_id, path, bytes).unwrap();
    }
    engine.commit_workspace(storage_engine::CommitRequest {
        workspace_id: workspace.workspace_id, operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(), expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token, message: "actual initial versions".into(),
        request_digest: None,
    }).unwrap();
    let context = engine.snapshot_root_context(repository).unwrap();
    let source = engine.initialize_directory_snapshot(&InitializeSnapshot {
        host: context.host, source: context.source, operation_id: OperationId::generate().unwrap(),
    }, &"admin".parse().unwrap()).unwrap();
    let saved = native_file(&mut engine, repository, "new-saved.txt", Some(b"new selected body"));
    let empty = native_file(&mut engine, repository, "new-empty.txt", None);
    let untouched = native_file(&mut engine, repository, "unselected-new.txt",
        Some(b"new unselected draft"));
    let versions_before = engine.state().versions.len();
    assert_eq!(versions_before, 2);
    let password = authentication::random_token().unwrap();
    let auth = AuthStore::initialize(&system, "admin", &password).unwrap();
    let app = router(engine, auth, &system);
    let login = request(&app, None, "POST", "/api/auth/login", Some(json!({
        "username":"admin", "password":password,
    }))).await;
    let fixture = Fixture { root, app, token: login["token"].as_str().unwrap().into() };
    let (existing, _) = target(&fixture, &slot, "existing.txt").await;
    let (unselected, _) = target(&fixture, &slot, "unselected.txt").await;
    let born = save_draft(&fixture, &existing, b"existing selected body").await;
    save_draft(&fixture, &unselected, b"unselected shared draft").await;
    let unborn_status = |target: &storage_contract::file_draft::DraftLocation| {
        let target = json!(target);
        format!("/api/file-manager/files/{}/unborn-working?{}",
            target["file"].as_str().unwrap(), query(&target))
    };
    let saved_before = fixture.json("GET", &unborn_status(&saved), None).await;
    let empty_before = fixture.json("GET", &unborn_status(&empty), None).await;
    let untouched_before = fixture.json("GET", &unborn_status(&untouched), None).await;
    let unselected_before = working(&fixture, &unselected).await;
    let old_history = fixture.history(existing["file"].as_str().unwrap()).await;
    let unselected_history = fixture.history(unselected["file"].as_str().unwrap()).await;
    let operation = OperationId::generate().unwrap();
    // A first append before and after the Born item exercises one accumulated
    // namespace, not a loop of independently committed singleton requests.
    let command = json!({
        "source":source, "operation_id":operation, "message":"one real mixed selection",
        "selected":[
            {"target":saved, "expected":saved_before["expected"]},
            born,
            {"target":empty, "expected":empty_before["expected"]},
        ],
    });
    let receipt = fixture.json("POST", ROUTE, Some(command.clone())).await;
    let public: DirectoryDraftReceipt = json_codec::from_value(receipt.clone()).unwrap();
    let DirectoryDraftOutcome::Changed { snapshot } = public.result else {
        panic!("the mixed selection must publish a directory node");
    };
    assert_eq!(public.operation_id, operation);
    assert_eq!(snapshot.publication, operation);
    assert_eq!(snapshot.repository_id, source.repository_id);
    assert_eq!(snapshot.branch_id, source.branch_id);
    assert_ne!(snapshot.node_id, source.node_id);
    no_native_fields(&receipt);
    let history = fixture.history(existing["file"].as_str().unwrap()).await;
    assert_eq!(history["nodes"].as_array().unwrap().len(),
        old_history["nodes"].as_array().unwrap().len() + 1);
    for (target, bytes) in [(&saved, b"new selected body".as_slice()), (&empty, b"".as_slice())] {
        let entry = fixture.entry(&slot, target.path.as_str()).await;
        assert_eq!(entry["file_id"], target.file.to_string());
        assert_eq!(entry["entry_id"], target.entry.to_string());
        assert_eq!(fixture.body(entry["version_id"].as_str().unwrap()).await, bytes);
        let history = fixture.history(&target.file.to_string()).await;
        assert_eq!(history["nodes"].as_array().unwrap().len(), 1);
    }
    let existing_entry = fixture.entry(&slot, "existing.txt").await;
    assert_eq!(fixture.body(existing_entry["version_id"].as_str().unwrap()).await,
        b"existing selected body");
    assert_eq!(working(&fixture, &unselected).await, unselected_before);
    assert_eq!(draft(&fixture, &unselected).await, b"unselected shared draft");
    assert_eq!(fixture.json("GET", &unborn_status(&untouched), None).await, untouched_before);
    assert_same_file_history(&fixture.history(unselected["file"].as_str().unwrap()).await,
        &unselected_history);
    let fixture = fixture.reopen().await;
    let material = inventory(fixture.root.path());
    assert_eq!(fixture.json("POST", ROUTE, Some(command.clone())).await, receipt);
    let receipt_route = lookup(&json!(source), &json!(operation));
    assert_eq!(fixture.json("GET", &receipt_route, None).await, receipt);
    assert_eq!(inventory(fixture.root.path()), material);
    assert_eq!(fixture.history(existing["file"].as_str().unwrap()).await, history);
    assert_eq!(fixture.json("GET", &unborn_status(&untouched), None).await, untouched_before);
    assert_eq!(working(&fixture, &unselected).await, unselected_before);
}
