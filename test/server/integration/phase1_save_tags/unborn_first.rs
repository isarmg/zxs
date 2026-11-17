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

async fn native_unborn_fixture()->(Fixture,DraftLocation,String){
    let (server,target,slot,())=native_unborn_fixture_with(|_,_|()).await;(server,target,slot)
}
async fn native_unborn_fixture_with<T>(prepare:impl FnOnce(&mut Engine,&DraftLocation)->T)->(Fixture,DraftLocation,String,T){
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
    let original=prepare(&mut engine,&target);
    let password = authentication::random_token().unwrap();
    let auth = AuthStore::initialize(&system, "admin", &password).unwrap();
    let app = router(engine, auth, &system);
    let login = request(&app, None, "POST", "/api/auth/login", Some(json!({
        "username": "admin", "password": password,
    }))).await;
    let fixture = Fixture { root, app, token: login["token"].as_str().unwrap().into() };
    (fixture,target,slot,original)
}
fn status_route(target:&DraftLocation)->String {
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("repository_id", &target.repository.to_string())
        .append_pair("path", target.path.as_str())
        .append_pair("entry_id", &target.entry.to_string()).finish();
    format!(
        "/api/file-manager/files/{}/unborn-working?{query}", target.file,
    )
}

#[tokio::test]
async fn authenticated_unborn_empty_first_commit_is_explicit_and_cold_idempotent() {
    let (fixture,target,slot)=native_unborn_fixture().await;
    let status_route=status_route(&target);
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

#[tokio::test]
async fn real_unborn_mode_route_rejects_other_commands_and_replays_only_its_original_after_birth() {
    let (fixture,target,_)=native_unborn_fixture().await;
    let status=status_route(&target);
    let change=format!("/api/file-manager/files/{}/unborn-working/mode",target.file);
    let first=format!("/api/file-manager/files/{}/first-commit",target.file);
    let before=fixture.json("GET",&status,None).await;
    assert_eq!(before.get("automatic"),Some(&Value::Null));
    let op=OperationId::generate().unwrap();
    let mut enable=json!({"operation_id":op,"command":{"action":"set_mode","target":target,
        "expected":before["expected"]["revision"],"mode":{"mode":"automatic","policy":"twenty_saves_in_sixty_seconds"}}});
    assert_eq!(post(&fixture,&change,false,enable.clone()).await.status(),StatusCode::UNAUTHORIZED);
    let mut other=enable.clone();other["command"]=json!({"action":"discard","target":target,"expected":before["expected"]["revision"]});
    assert_eq!(post(&fixture,&change,true,other).await.status(),StatusCode::BAD_REQUEST);
    let receipt=json_response(post(&fixture,&change,true,enable.clone()).await).await;
    assert_eq!(receipt["result"]["request"],enable["command"]);
    let armed=fixture.json("GET",&status,None).await;
    assert_eq!(armed["mode"],"automatic");assert!(armed["automatic"].is_object());
    let mut manual=enable.clone();manual["operation_id"]=json!(OperationId::generate().unwrap());
    manual["command"]["expected"]=armed["expected"]["revision"].clone();
    manual["command"]["mode"]=json!({"mode":"manual"});
    assert!(post(&fixture,&change,true,manual).await.status().is_success());
    let latest=fixture.json("GET",&status,None).await;
    assert_eq!(latest["mode"],"manual");assert!(latest["automatic"].is_null());
    let publish=json!({"operation_id":OperationId::generate().unwrap(),"target":target,"expected":latest["expected"],
        "message":"explicit first node after the original mode decision"});
    assert!(post(&fixture,&first,true,publish).await.status().is_success());
    let fixture=fixture.reopen().await;
    assert_eq!(json_response(post(&fixture,&change,true,enable.clone()).await).await,receipt);
    enable["operation_id"]=json!(OperationId::generate().unwrap());
    assert!(!post(&fixture,&change,true,enable).await.status().is_success());
}

// Authenticated HTTP reads the immutable native publication itself, not the
// similarly named current node, 'saved' status or an operation-name guess.
#[tokio::test]
async fn genuine_automatic_publication_is_read_only_in_its_original_private_window_after_reopen() {
    use storage_contract::automatic_observation::{Binding,NodeKey,Position,Reply,Request as WindowRequest};
    use storage_contract::file_draft::{DraftCommand,CommitMode};
    let (fixture,target,_,node)=native_unborn_fixture_with(|engine,target|{
        let actor="admin".parse().unwrap();
        let working=engine.unborn_working_state(target).unwrap();
        let enable=DraftCommand::SetMode{target:target.clone(),expected:working.revision(),mode:CommitMode::Automatic.into()};
        engine.change_unborn_working_mode(&enable,&actor,OperationId::generate().unwrap()).unwrap();
        let mut writer=engine.open_native_working(target,&actor,true).unwrap();
        let bytes=b"actual publication, not a save observation";
        assert_eq!(engine.write_native_working(&mut writer,&actor,0,bytes).unwrap(),bytes.len());
        let save=engine.flush_native_working(&mut writer,&actor).unwrap().unwrap();drop(writer);
        let prep=engine.prepare_automatic_file(target.file).unwrap().execute().unwrap();
        let node=engine.finish_automatic_file(prep).unwrap();
        let working=engine.file_working_state(target).unwrap();
        let decision=*working.automatic_controller().unwrap().last_node().unwrap();
        assert_eq!(decision.operation,node.operation_id);assert_eq!(decision.save.operation,save.operation_id);
        decision
    }).await;
    let base=format!("/api/file-manager/repositories/{}/automatic-nodes",target.repository);
    let second=node.published_at.unix_seconds();
    let path=format!("{base}?start={second}&end={}",second+1);
    let no_login=fixture.app.clone().oneshot(Request::builder().method("GET").uri(&path).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(no_login.status(),StatusCode::UNAUTHORIZED);
    let read=|value:Value|json_codec::from_value::<Reply>(value).unwrap();
    let first=read(fixture.json("GET",&path,None).await);
    assert_eq!(first.path,None);assert_eq!(first.page.rows.len(),1);
    let row=&first.page.rows[0];
    assert_eq!(row.key,NodeKey{file:target.file,node:node.node});assert_eq!(row.operation,node.operation);
    assert_eq!(row.sequence,node.sequence);assert_eq!(row.published_at,node.published_at);
    assert!(first.next_binding.is_none()&&first.page.next.is_none());
    let fake=WindowRequest{scope:first.page.request.scope,position:Position::After{observed:first.page.observed,last:row.key}};
    let guessed=Binding::for_session(&"admin".parse().unwrap(),"not-the-admitted-login",&fake).unwrap();
    let forged=format!("{path}&observed={}&last_file={}&last_node={}&binding={guessed}",first.page.observed,row.key.file,row.key.node);
    let rejected=fixture.app.clone().oneshot(Request::builder().method("GET").uri(forged)
        .header("authorization",format!("Bearer {}",fixture.token)).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(rejected.status(),StatusCode::BAD_REQUEST);
    let older=read(fixture.json("GET",&format!("{base}?start=0&end={second}"),None).await);
    assert!(older.page.rows.is_empty()&&older.page.next.is_none());
    let fixture=fixture.reopen().await;
    let cold=read(fixture.json("GET",&path,None).await);
    assert_eq!(cold.page.rows,first.page.rows);
}
