use crate::{Body, Duration, Fixture, Request, ServiceExt, StatusCode};
use json_codec::json;
use storage_contract::file_lifecycle::{Action, Page, Request as LifecycleRequest};

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
        "deletion_node_id": page.events[0].deletion_id, "operation_id": undo
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
    assert_eq!(page.events[0].action, Action::Undo);
    assert!(page.events[0].occurred_at.is_none());
    assert!(page.next_cursor.is_none());
    fixture.json("POST", "/api/auth/logout", Some(json!({}))).await;
    assert_eq!(status(&fixture, &url, true).await, StatusCode::UNAUTHORIZED);
}
