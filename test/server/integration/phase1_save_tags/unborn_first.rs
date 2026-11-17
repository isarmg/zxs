//! Real birth and authenticated first-history publication, without a fake VersionId.
use super::{Fixture, json_response, request, router};
use http_routing::{body::Body, http::{Request, StatusCode}};
use json_codec::{Value, json};
use name_policy::{Name, PortablePath};
use service_stack::ServiceExt;
use std::os::unix::fs::PermissionsExt;
use storage_contract::file_draft::{BirthRequest, DraftLocation};
use storage_contract::{Config, Mode, OperationId};
use storage_engine::Engine;
use storage_server::auth::AuthStore;

async fn post(fixture: &Fixture, route: &str, token: bool, body: Value)
    -> http_routing::response::Response
{
    let mut builder = Request::builder().method("POST").uri(route)
        .header("content-type", "application/json");
    if token {
        builder = builder.header("authorization", format!("Bearer {}", fixture.token));
    }
    fixture.app.clone().oneshot(
        builder.body(Body::from(json_codec::to_vec(&body).unwrap())).unwrap(),
    ).await.unwrap()
}

#[tokio::test]
async fn authenticated_unborn_empty_first_commit_is_explicit_and_cold_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("system");
    let primary = root.path().join("primary");
    for path in [&system, &primary] {
        std::fs::create_dir(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = Config::independent_roots(
        Mode::Development, [(Name::parse("HTTP native primary").unwrap(), primary)],
    ).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repo = engine.create_disk_root(&slot, "admin", OperationId::generate().unwrap()).unwrap();
    let birth = BirthRequest {
        repository: repo.repository_id.parse().unwrap(),
        path: PortablePath::parse("first-empty.txt").unwrap(),
        expected_namespace_head: engine.state().branches[&repo.main_branch_id]
            .head_commit_id.parse().unwrap(),
    };
    let versions = engine.state().versions.clone();
    let branches = engine.state().branches.clone();
    let commits = engine.state().commits.clone();
    let birth_op = OperationId::generate().unwrap();
    engine.create_unborn_file(&birth, &"admin".parse().unwrap(), birth_op).unwrap();
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.state().branches, branches);
    assert_eq!(engine.state().commits, commits);
    let (file, record) = engine.state().file_containers.iter()
        .find(|(_, record)| record.created_by == birth_op).unwrap();
    let target = DraftLocation {
        repository: birth.repository, path: birth.path, file: *file, entry: record.namespace.entry,
    };
    assert_eq!(engine.unborn_working_state(&target).unwrap().logical_size(), 0);
    assert!(record.working.unborn().is_some());
    let password = authentication::random_token().unwrap();
    let auth = AuthStore::initialize(&system, "admin", &password).unwrap();
    let app = router(engine, auth, &system);
    let login = request(&app, None, "POST", "/api/auth/login", Some(json!({
        "username": "admin", "password": password,
    }))).await;
    let fixture = Fixture { root, app, token: login["token"].as_str().unwrap().into() };
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("repository_id", &target.repository.to_string())
        .append_pair("path", target.path.as_str())
        .append_pair("entry_id", &target.entry.to_string()).finish();
    let status_route = format!(
        "/api/file-manager/files/{}/unborn-working?{query}", target.file,
    );
    let route = format!("/api/file-manager/files/{}/first-commit", target.file);
    let before = fixture.json("GET", &status_route, None).await;
    assert_eq!(before["mode"], "manual");
    assert_eq!(before["logical_size"], "0");
    assert_eq!(before["expected"]["content"], json!({"state": "empty"}));
    assert!(before.get("base").is_none());
    assert!(before.get("version_id").is_none());
    let command = json!({
        "operation_id": OperationId::generate().unwrap(),
        "target": target, "expected": before["expected"], "message": "first explicit node",
    });
    assert_eq!(post(&fixture, &route, false, command.clone()).await.status(),
        StatusCode::UNAUTHORIZED);
    let mut legacy_base = command.clone();
    legacy_base["base"] = json!({});
    assert_eq!(post(&fixture, &route, true, legacy_base).await.status(), StatusCode::BAD_REQUEST);
    let mut stale = command.clone();
    stale["expected"]["revision"] = json!("0");
    assert!(!post(&fixture, &route, true, stale).await.status().is_success());
    assert_eq!(fixture.json("GET", &status_route, None).await, before);

    let fixture = fixture.reopen().await;
    assert_eq!(fixture.json("GET", &status_route, None).await, before);
    let receipt = json_response(post(&fixture, &route, true, command.clone()).await).await;
    // Browser refresh queries this authorized endpoint; it never replays the POST.
    let outcome = format!("/api/operations/{}/outcome", command["operation_id"].as_str().unwrap());
    assert_eq!(fixture.json("GET", &outcome, None).await, receipt);
    let entry = fixture.entry(&slot, "first-empty.txt").await;
    assert_eq!(entry["file_id"], target.file.to_string());
    assert_eq!(entry["entry_id"], target.entry.to_string());
    assert_eq!(fixture.body(entry["version_id"].as_str().unwrap()).await, b"");
    let history = fixture.history(&target.file.to_string()).await;
    assert_eq!(history["nodes"].as_array().unwrap().len(), 1);
    let fixture = fixture.reopen().await;
    assert_eq!(fixture.json("GET", &outcome, None).await, receipt);
    assert_eq!(json_response(post(&fixture, &route, true, command.clone()).await).await, receipt);
    assert_eq!(fixture.history(&target.file.to_string()).await, history);
    let mut second = command;
    second["operation_id"] = json!(OperationId::generate().unwrap());
    assert!(!post(&fixture, &route, true, second).await.status().is_success());
    assert_eq!(fixture.history(&target.file.to_string()).await, history);
}
