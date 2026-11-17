//! Empty-pool Manual saves, explicit versions and first editing of a FILE branch.
use super::*;
use name_policy::{Name, PortablePath};
use std::os::unix::fs::PermissionsExt;
use storage_contract::file_draft::{BirthRequest, DraftLocation};
use storage_contract::{Mode, OperationId};

fn saved_unborn(system: &Path, primary: &Path, bytes: &[u8]) -> (Engine, DraftLocation, String) {
    for path in [system, primary] {
        std::fs::create_dir(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(std::fs::read_dir(path).unwrap().count(), 0);
    }
    let config = Config::independent_roots(
        Mode::Development,
        [(Name::parse("HTTP branch primary").unwrap(), primary.to_path_buf())],
    ).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(system, config).unwrap();
    let repo = engine.create_disk_root(&slot, "admin", OperationId::generate().unwrap()).unwrap();
    let birth = BirthRequest {
        repository: repo.repository_id.parse().unwrap(),
        path: PortablePath::parse("history.txt").unwrap(),
        expected_namespace_head: engine.state().branches[&repo.main_branch_id]
            .head_commit_id.parse().unwrap(),
    };
    let versions = engine.state().versions.clone();
    let branches = engine.state().branches.clone();
    let commits = engine.state().commits.clone();
    let birth_op = OperationId::generate().unwrap();
    let principal = "admin".parse().unwrap();
    engine.create_unborn_file(&birth, &principal, birth_op).unwrap();
    let (file, record) = engine.state().file_containers.iter()
        .find(|(_, record)| record.created_by == birth_op).unwrap();
    let target = DraftLocation {
        repository: birth.repository,
        path: birth.path,
        file: *file,
        entry: record.namespace.entry,
    };
    assert!(record.working.unborn().is_some());
    let mut held = engine.open_native_working(&target, &principal, true).unwrap();
    assert_eq!(engine.write_native_working(&mut held, &principal, 0, bytes).unwrap(), bytes.len());
    assert!(engine.flush_native_working(&mut held, &principal).unwrap().is_some());
    let mut read = vec![0; bytes.len()];
    assert_eq!(engine.read_native_working(&held, &mut read, 0).unwrap(), bytes.len());
    assert_eq!(read, bytes);
    drop(held);
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().branches, branches);
    assert_eq!(engine.state().commits, commits);
    (engine, target, slot)
}

async fn write_draft(app: &Router, token: &str, file: &str, command: &Value, bytes: &[u8]) {
    let response = app.clone().oneshot(
        Request::builder()
            .method("PUT")
            .uri(format!("/api/file-manager/files/{file}/draft"))
            .header("authorization", format!("Bearer {token}"))
            .header("x-zxs-draft-command", json_codec::to_string(command).unwrap())
            .header("content-length", bytes.len().to_string())
            .body(Body::from(bytes.to_vec()))
            .unwrap(),
    ).await.unwrap();
    let status = response.status();
    let response = http_routing::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&response));
}

async fn selected(
    app: &Router, token: &str, listing: &str, target: &DraftLocation,
    branch: &str, version: &str, body: &[u8],
) {
    let page = success(app, token, "GET", listing, None).await;
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["file_id"], target.file.to_string());
    assert_eq!(entries[0]["entry_id"], target.entry.to_string());
    assert_eq!(entries[0]["name"], target.path.as_str());
    assert_eq!(entries[0]["selected_branch_id"], branch);
    assert_eq!(entries[0]["version_id"], version);
    assert_eq!(version_bytes(app, token, version).await, body);
}

async fn heads(app: &Router, token: &str, file: &str, count: usize) -> Value {
    let history = success(
        app, token, "GET", &format!("/api/file-manager/files/{file}/history"), None,
    ).await;
    assert_eq!(history["nodes"].as_array().unwrap().len(), count);
    let page = history_related(app, token, file, &history, "branches", None).await;
    assert!(page["page"]["next_cursor"].is_null());
    let branches = page["branches"].as_array().unwrap();
    Value::Array(branches.iter().map(|branch| json!({
        "branch_id": branch["branch_id"], "node_id": branch["node_id"],
        "head_commit_id": branch["head_commit_id"], "generation": branch["generation"],
        "content": branch["content"],
    })).collect())
}

fn branch<'a>(heads: &'a Value, id: &str) -> &'a Value {
    heads.as_array().unwrap().iter().find(|head| head["branch_id"] == id).unwrap()
}

async fn save(
    app: &Router, token: &str, target: &DraftLocation, query: &str, bytes: &[u8],
) -> Value {
    let file = target.file.to_string();
    let route = format!("/api/file-manager/files/{file}/working?{query}");
    let clean = success(app, token, "GET", &route, None).await;
    assert_eq!(clean["working"]["state"], "manual_clean");
    let command = json!({
        "operation_id": OperationId::generate().unwrap(),
        "command": {
            "action": "write", "target": target, "expected": clean["working"]["revision"],
            "base": clean["base"], "logical_size": bytes.len().to_string(),
            "content_hash": blake3::hash(bytes).to_hex().as_str(),
        },
    });
    write_draft(app, token, &file, &command, bytes).await;
    let dirty = success(app, token, "GET", &route, None).await;
    assert_eq!(dirty["working"]["state"], "manual_dirty");
    assert_eq!(dirty["base"], clean["base"]);
    assert_eq!(dirty["working"]["base"], clean["base"]);
    let response = app.clone().oneshot(
        Request::builder()
            .uri(format!("/api/file-manager/files/{file}/draft?{query}"))
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let read = http_routing::body::to_bytes(response.into_body(), 65536).await.unwrap();
    assert_eq!(read.as_ref(), bytes);
    dirty
}

async fn commit(
    app: &Router, token: &str, target: &DraftLocation, query: &str, dirty: &Value,
) -> Value {
    let route = format!("/api/file-manager/files/{}/working", target.file);
    success(app, token, "POST", &route, Some(json!({
        "operation_id": OperationId::generate().unwrap(),
        "command": {
            "action": "commit", "target": target, "expected": dirty["working"]["revision"],
            "base": dirty["base"], "message": "explicit history node",
        },
    }))).await;
    let clean = success(app, token, "GET", &format!("{route}?{query}"), None).await;
    assert_eq!(clean["working"]["state"], "manual_clean");
    assert_eq!(clean["base"]["branch_id"], dirty["base"]["branch_id"]);
    clean
}

#[tokio::test]
async fn manual_file_first_versions_and_pending_branch_edit() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("system");
    let original = b"first saved Manual body";
    let edited = b"second explicitly committed Manual body";
    let branched = b"first explicit edit on the retained V1 branch";
    let (engine, target, slot) = saved_unborn(&system, &root.path().join("primary"), original);
    let file = target.file.to_string();
    let password = authentication::random_token().unwrap();
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend.protect_auth(AuthStore::initialize(&system, "admin", &password).unwrap())
        .unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::new(&system)).unwrap();
    let token = login(&app, "admin", &password).await;
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("repository_id", &target.repository.to_string())
        .append_pair("path", target.path.as_str())
        .append_pair("entry_id", &target.entry.to_string()).finish();
    let unborn = success(&app, &token, "GET", &format!(
        "/api/file-manager/files/{file}/unborn-working?{query}",
    ), None).await;
    assert_eq!(unborn["mode"], "manual");
    assert_eq!(unborn["logical_size"], original.len().to_string());
    assert!(unborn.get("base").is_none());
    assert!(unborn.get("version_id").is_none());
    let first_route = format!("/api/file-manager/files/{file}/first-commit");
    let first_request = json!({
        "operation_id": OperationId::generate().unwrap(), "target": target,
        "expected": unborn["expected"], "message": "explicit first history node",
    });
    let first_receipt = success(
        &app, &token, "POST", &first_route, Some(first_request.clone()),
    ).await;
    let working_route = format!("/api/file-manager/files/{file}/working?{query}");
    let first = success(&app, &token, "GET", &working_route, None).await;
    assert_eq!(first["working"]["state"], "manual_clean");
    let first_version = first["base"]["version_id"].as_str().unwrap().to_owned();
    let original_branch = first["base"]["branch_id"].as_str().unwrap().to_owned();
    let listing = format!("/api/file-manager/disks/{slot}/entries");
    selected(&app, &token, &listing, &target, &original_branch, &first_version, original).await;
    let first_heads = heads(&app, &token, &file, 1).await;
    assert_eq!(first_heads.as_array().unwrap().len(), 1);

    let dirty = save(&app, &token, &target, &query, edited).await;
    assert_eq!(heads(&app, &token, &file, 1).await, first_heads);
    assert_eq!(version_bytes(&app, &token, &first_version).await, original);
    let second = commit(&app, &token, &target, &query, &dirty).await;
    let second_version = second["base"]["version_id"].as_str().unwrap().to_owned();
    assert_ne!(first_version, second_version);
    let graph_route = format!("/api/file-manager/files/{file}/history");
    let graph = success(&app, &token, "GET", &graph_route, None).await;
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(graph["edges"], json!([{
        "node_id": second["base"]["node_id"],
        "parent_node_id": first["base"]["node_id"], "endpoint": "in_window",
    }]));
    selected(&app, &token, &listing, &target, &original_branch, &second_version, edited).await;

    let created = success(&app, &token, "POST", &format!(
        "/api/file-manager/files/{file}/branches",
    ), Some(json!({
        "operation_id": OperationId::generate().unwrap(), "name": "retained first body",
        "source_version_id": first_version, "source_node_id": first["base"]["node_id"],
    }))).await;
    assert_eq!(created["result"]["inherited"], true);
    assert!(created["result"]["own_version_id"].is_null());
    let old_branch = created["result"]["branch_id"].as_str().unwrap().to_owned();
    let before_heads = heads(&app, &token, &file, 2).await;
    assert_eq!(before_heads.as_array().unwrap().len(), 2);
    assert_eq!(branch(&before_heads, &original_branch)["node_id"], second["base"]["node_id"]);
    assert_eq!(branch(&before_heads, &old_branch)["node_id"], first["base"]["node_id"]);
    assert_eq!(branch(&before_heads, &old_branch)["content"]["source"], json!({
        "kind": "inherited", "source_version_id": first_version,
    }));
    selected(&app, &token, &listing, &target, &original_branch, &second_version, edited).await;
    let selection_route = format!("/api/file-manager/files/{file}/select");
    success(&app, &token, "POST", &selection_route, Some(json!({
        "operation_id": OperationId::generate().unwrap(), "branch_id": old_branch,
        "repository_id": target.repository, "path": target.path,
    }))).await;
    selected(&app, &token, &listing, &target, &old_branch, &first_version, original).await;
    assert_eq!(heads(&app, &token, &file, 2).await, before_heads);

    // The inherited branch has no private Version yet, but its exact V1 is a
    // real base. A Manual save must not advance either branch or create V3.
    let dirty = save(&app, &token, &target, &query, branched).await;
    assert_eq!(dirty["base"]["branch_id"], old_branch);
    assert_eq!(dirty["base"]["version_id"], first_version);
    assert_eq!(heads(&app, &token, &file, 2).await, before_heads);
    assert_eq!(version_bytes(&app, &token, &first_version).await, original);
    assert_eq!(version_bytes(&app, &token, &second_version).await, edited);
    let (status, error) = send(&app, Some(&token), "POST", &selection_route, Some(json!({
        "operation_id": OperationId::generate().unwrap(), "branch_id": original_branch,
        "repository_id": target.repository, "path": target.path,
    }))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "FILE_BRANCH_CONFLICT");
    assert_eq!(success(&app, &token, "GET", &working_route, None).await, dirty);
    let third = commit(&app, &token, &target, &query, &dirty).await;
    let third_version = third["base"]["version_id"].as_str().unwrap().to_owned();
    assert_ne!(third_version, first_version);
    assert_ne!(third_version, second_version);
    selected(&app, &token, &listing, &target, &old_branch, &third_version, branched).await;
    let after_heads = heads(&app, &token, &file, 3).await;
    assert_eq!(branch(&after_heads, &original_branch), branch(&before_heads, &original_branch));
    assert_eq!(branch(&after_heads, &old_branch)["node_id"], third["base"]["node_id"]);
    assert_eq!(branch(&after_heads, &old_branch)["generation"], "2");
    assert_eq!(branch(&after_heads, &old_branch)["content"]["source"], json!({"kind": "owned"}));
    let graph = success(&app, &token, "GET", &graph_route, None).await;
    let parents = graph["edges"].as_array().unwrap().iter()
        .filter(|edge| edge["node_id"] == third["base"]["node_id"])
        .cloned().collect::<Vec<_>>();
    assert_eq!(parents, vec![json!({
        "node_id": third["base"]["node_id"],
        "parent_node_id": first["base"]["node_id"], "endpoint": "in_window",
    })]);
    success(&app, &token, "POST", &selection_route, Some(json!({
        "operation_id": OperationId::generate().unwrap(), "branch_id": original_branch,
        "repository_id": target.repository, "path": target.path,
    }))).await;
    selected(&app, &token, &listing, &target, &original_branch, &second_version, edited).await;
    assert_eq!(version_bytes(&app, &token, &first_version).await, original);
    assert_eq!(version_bytes(&app, &token, &third_version).await, branched);
    assert_eq!(heads(&app, &token, &file, 3).await, after_heads);
    drop(app);

    let engine = reopen_after_router_drop(&system).await;
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend.protect_auth(AuthStore::open(&system).unwrap()).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::new(&system)).unwrap();
    let token = login(&app, "admin", &password).await;
    assert_eq!(
        success(&app, &token, "POST", &first_route, Some(first_request)).await,
        first_receipt,
    );
    selected(&app, &token, &listing, &target, &original_branch, &second_version, edited).await;
    assert_eq!(version_bytes(&app, &token, &first_version).await, original);
    assert_eq!(version_bytes(&app, &token, &third_version).await, branched);
    assert_eq!(heads(&app, &token, &file, 3).await, after_heads);
}
