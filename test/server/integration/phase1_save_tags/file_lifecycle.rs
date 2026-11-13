use crate::{Body, Duration, Fixture, Request, ServiceExt, StatusCode};
use json_codec::{Value, json};
use storage_contract::file_lifecycle::{Detail, Page, Request as LifecycleRequest};

fn route(request: &LifecycleRequest) -> String {
    let mut query = url_encoding::form::Serializer::new(String::new());
    for (name, value) in request.query_fields() {
        query.append_pair(&name, &value);
    }
    format!("{}?{}", request.route(), query.finish())
}
async fn status(fixture: &Fixture, uri: &str, authorized: bool) -> StatusCode {
    let mut request = Request::builder().uri(uri);
    if authorized {
        request = request.header("authorization", format!("Bearer {}", fixture.token));
    }
    tokio::time::timeout(Duration::from_secs(10), fixture.app.clone()
        .oneshot(request.body(Body::empty()).unwrap()))
        .await.unwrap().unwrap().status()
}

async fn rename_request(fixture: &Fixture, repository: &str, before: &str, after: &str) -> Value {
    let scopes = fixture
        .json(
            "GET",
            &format!("/api/repositories/{repository}/directory-branches?path="),
            None,
        )
        .await;
    let scope = scopes["directory_branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|scope| scope["current"] == true)
        .unwrap();
    let entry = scope["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == before)
        .unwrap();
    json!({
        "kind":"rename",
        "operation_id":storage_contract::OperationId::generate().unwrap(),
        "items":[{
            "source":{
                "directory_branch_id":scope["directory_branch_id"],
                "expected_tree_id":scope["head_tree_id"],
                "path":before,
                "entry_id":entry["entry_id"],
            },
            "target":{
                "directory_branch_id":scope["directory_branch_id"],
                "expected_tree_id":scope["head_tree_id"],
                "path":after,
            },
        }],
    })
}

fn assert_exact_receipt(replay: &Value, original: &Value) {
    for field in ["operation_id", "request_digest", "publication_sequence", "result"] {
        assert_eq!(replay[field], original[field], "{field}");
    }
}

async fn lifecycle_pages(fixture: &Fixture, file: &str) -> Vec<storage_contract::file_lifecycle::Event> {
    let history = fixture.history(file).await;
    let mut request = LifecycleRequest::parse(
        file,
        history["identity"]["history_root"].as_str().unwrap(),
        None,
        None,
        None,
        1,
    )
    .unwrap();
    let mut events = Vec::new();
    loop {
        let page: Page = json_codec::from_value(
            fixture.json("GET", &route(&request), None).await,
        )
        .unwrap();
        page.validate(&request).unwrap();
        events.extend(page.events);
        request.cursor = page.next_cursor;
        if request.cursor.is_none() {
            return events;
        }
        assert!(events.len() < 8, "fixture continuation must make finite progress");
    }
}
#[tokio::test]
async fn file_lifecycle_real_router_delete_undo_auth_current_root_and_read_only() {
    let fixture = Fixture::new(&["primary"]).await;
    fixture.save("D01", "a", b"retained").await;
    let entry = fixture.entry("D01", "a").await;
    let file = entry["file_id"].as_str().unwrap();
    let before = fixture.history(file).await;
    let request = LifecycleRequest::parse(file,
        before["identity"]["history_root"].as_str().unwrap(), None, None, None, 1).unwrap();
    let url = route(&request);
    assert_eq!(status(&fixture, &url, false).await, StatusCode::UNAUTHORIZED);
    assert_eq!(status(&fixture, &format!("{url}&extra=true"), true).await,
        StatusCode::BAD_REQUEST);
    assert_eq!(status(&fixture, &url.replace("limit=1", "limit=65"), true).await,
        StatusCode::BAD_REQUEST);
    let page: Page = json_codec::from_value(fixture.json("GET", &url, None).await).unwrap();
    page.validate(&request).unwrap();
    assert!(page.events.is_empty());
    assert_eq!(fixture.history(file).await, before);
    let delete = storage_contract::OperationId::generate().unwrap();
    fixture.json("POST", &format!("/api/files/{file}/deletion"), Some(json!({
        "repository_id": before["identity"]["context"]["repository_id"],
        "path": before["identity"]["context"]["path"], "operation_id": delete
    }))).await;
    assert_eq!(status(&fixture, &url, true).await, StatusCode::CONFLICT);
    let hidden = fixture.history(file).await;
    let request = LifecycleRequest::parse(file,
        hidden["identity"]["history_root"].as_str().unwrap(), None, None, None, 1).unwrap();
    let page: Page = json_codec::from_value(fixture.json("GET", &route(&request), None).await)
        .unwrap();
    assert!(page.context.path.is_none());
    assert_eq!(page.events[0].operation_id, delete);
    assert!(page.events[0].occurred_at.is_some());
    let undo = storage_contract::OperationId::generate().unwrap();
    fixture.json("POST", &format!("/api/files/{file}/deletion/undo"), Some(json!({
        "deletion_node_id": page.events[0].source_id().to_string(), "operation_id": undo
    }))).await;
    let fixture = fixture.reopen().await;
    let restored = fixture.history(file).await;
    let mut request = LifecycleRequest::parse(file,
        restored["identity"]["history_root"].as_str().unwrap(), None, None, None, 1).unwrap();
    let page: Page = json_codec::from_value(fixture.json("GET", &route(&request), None).await)
        .unwrap();
    request.cursor = page.next_cursor;
    assert!(request.cursor.is_some());
    let url = route(&request);
    let page: Page = json_codec::from_value(fixture.json("GET", &url, None).await).unwrap();
    page.validate(&request).unwrap();
    assert_eq!(page.events[0].operation_id, undo);
    assert!(matches!(&page.events[0].detail, Detail::Undo { .. }));
    assert!(page.events[0].occurred_at.is_none());
    assert!(page.next_cursor.is_none());

    let listing = fixture
        .json("GET", "/api/file-manager/disks/D01/entries", None)
        .await;
    let repository = listing["context"]["repository_id"].as_str().unwrap();
    fixture
        .json(
            "POST",
            &format!("/api/repositories/{repository}/directory-git/initialize"),
            Some(json!({
                "path":"",
                "expected_namespace_head":listing["context"]["base_head"],
                "operation_id":storage_contract::OperationId::generate().unwrap(),
            })),
        )
        .await;
    let before_rename = fixture.history(file).await;
    let mut receipts = Vec::new();
    let write_route = "/api/file-manager/disks/D01/namespace-batch";
    for (before, after) in [("a", "renamed"), ("renamed", "final-name")] {
        let body = rename_request(&fixture, repository, before, after).await;
        let receipt = fixture.json("POST", write_route, Some(body.clone())).await;
        assert_eq!(receipt["result"]["path_changes"].as_array().unwrap().len(), 1);
        assert_exact_receipt(
            &fixture.json("POST", write_route, Some(body.clone())).await,
            &receipt,
        );
        receipts.push((body, receipt));
    }
    assert_eq!(status(&fixture, &url, true).await, StatusCode::CONFLICT);
    let after_rename = fixture.history(file).await;
    let mut expected_nodes = before_rename["nodes"].clone();
    for node in expected_nodes.as_array_mut().unwrap() {
        let old = node["storage_name"].as_str().unwrap();
        let (identity, name) = old.split_once('-').unwrap();
        assert_eq!(identity.len(), 15);
        assert_eq!(name, "a");
        node["storage_name"] = format!("{identity}-final-name").into();
        node["download_name"] = "final-name".into();
    }
    assert_eq!(after_rename["nodes"], expected_nodes);
    assert_eq!(
        after_rename["identity"]["context"]["current_node_id"],
        before_rename["identity"]["context"]["current_node_id"],
    );
    assert_eq!(fixture.entry("D01", "final-name").await["file_id"], file);
    let events = lifecycle_pages(&fixture, file).await;
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].operation_id, delete);
    assert_eq!(events[1].operation_id, undo);
    for ((before_path, after_path), (event, (_, receipt))) in
        [("a", "renamed"), ("renamed", "final-name")]
            .into_iter()
            .zip(events[2..].iter().zip(&receipts))
    {
        let Detail::PathChanged { event_id, before, after } = &event.detail else {
            panic!("actual namespace rename must publish a path fact");
        };
        assert_eq!(event_id.to_string(), receipt["result"]["path_changes"][0]["event_id"]);
        assert_eq!(event.operation_id.to_string(), receipt["operation_id"]);
        assert_eq!(before.repository_id.to_string(), repository);
        assert_eq!(after.repository_id, before.repository_id);
        assert_eq!(before.path.as_str(), before_path);
        assert_eq!(after.path.as_str(), after_path);
        assert!(event.occurred_at.is_some());
    }
    let fixture = fixture.reopen().await;
    assert_eq!(lifecycle_pages(&fixture, file).await, events);
    assert_eq!(fixture.history(file).await["nodes"], expected_nodes);
    assert_eq!(fixture.body(entry["version_id"].as_str().unwrap()).await, b"retained");
    for (body, receipt) in receipts {
        assert_exact_receipt(&fixture.json("POST", write_route, Some(body)).await, &receipt);
    }
    fixture.json("POST", "/api/auth/logout", Some(json!({}))).await;
    assert_eq!(status(&fixture, &url, true).await, StatusCode::UNAUTHORIZED);
}
