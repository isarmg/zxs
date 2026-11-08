use super::*;
use storage_contract::content_matches::{ContentMatchPage, ContentMatchRequest};
fn route(request: &ContentMatchRequest) -> String {
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
    tokio::time::timeout(
        Duration::from_secs(10),
        fixture
            .app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap()),
    )
    .await
    .unwrap()
    .unwrap()
    .status()
}
#[tokio::test]
async fn content_matches_real_router_is_read_only_authenticated_and_root_bound() {
    let fixture = Fixture::new(&["primary"]).await;
    fixture.save("D01", "a", b"matching").await;
    fixture.save("D01", "b", b"matching").await;
    let a = fixture.entry("D01", "a").await;
    let b = fixture.entry("D01", "b").await;
    let file = a["file_id"].as_str().unwrap();
    let version = a["version_id"].as_str().unwrap();
    let before = fixture.history(file).await;
    let request = ContentMatchRequest::parse(
        file,
        version,
        before["identity"]["history_root"].as_str().unwrap(),
        None,
        1,
    )
    .unwrap();
    let url = route(&request);
    assert_eq!(
        status(&fixture, &url, false).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(&fixture, &format!("{url}&unexpected=true"), true).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status(&fixture, &url.replace("limit=1", "limit=65"), true).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status(&fixture, &format!("{url}&limit=1"), true).await,
        StatusCode::BAD_REQUEST
    );
    let page: ContentMatchPage =
        json_codec::from_value(fixture.json("GET", &url, None).await).unwrap();
    page.validate(&request).unwrap();
    assert_eq!(page.matches.len(), 1);
    assert_eq!(
        page.matches[0].file_id.as_str(),
        b["file_id"].as_str().unwrap()
    );
    assert!(page.next_after_version_id.is_none());
    assert_eq!(fixture.history(file).await, before);
    assert_eq!(fixture.body(version).await, b"matching");
    let fixture = fixture.reopen().await;
    let cold: ContentMatchPage =
        json_codec::from_value(fixture.json("GET", &url, None).await).unwrap();
    assert_eq!(cold, page);
    fixture.save("D01", "b", b"new body").await;
    assert_eq!(status(&fixture, &url, true).await, StatusCode::CONFLICT);
    fixture
        .json("POST", "/api/auth/logout", Some(json!({})))
        .await;
    assert_eq!(status(&fixture, &url, true).await, StatusCode::UNAUTHORIZED);
}
