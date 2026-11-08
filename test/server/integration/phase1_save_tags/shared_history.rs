use super::*;
use storage_contract::shared_history::{SharedHistoryGraph, SharedHistoryRequest};

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
async fn shared_history_real_router_automatically_joins_transitive_files_with_exact_auth_and_root()
{
    let fixture = Fixture::new(&["primary"]).await;
    fixture.save("D01", "a", b"x").await;
    fixture.save("D01", "b", b"x").await;
    fixture.save("D01", "b", b"y").await;
    fixture.save("D01", "c", b"y").await;
    let c = fixture.entry("D01", "c").await;
    let file = c["file_id"].as_str().unwrap();
    let before = fixture.history(file).await;
    let request =
        SharedHistoryRequest::parse(file, before["identity"]["history_root"].as_str().unwrap())
            .unwrap();
    let url = format!("{}?history_root={}", request.route(), request.history_root);
    assert_eq!(
        status(&fixture, &url, false).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(&fixture, &format!("{url}&automatic_write=true"), true).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status(
            &fixture,
            &format!("{url}&history_root={}", request.history_root),
            true
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    let graph: SharedHistoryGraph =
        json_codec::from_value(fixture.json("GET", &url, None).await).unwrap();
    graph.validate(&request).unwrap();
    assert_eq!(graph.files.len(), 3);
    assert_eq!(graph.shared_contents.len(), 2);
    assert_eq!(fixture.history(file).await, before);
    let fixture = fixture.reopen().await;
    let cold: SharedHistoryGraph =
        json_codec::from_value(fixture.json("GET", &url, None).await).unwrap();
    assert_eq!(cold, graph);
    fixture.save("D01", "c", b"still editable").await;
    assert_eq!(status(&fixture, &url, true).await, StatusCode::CONFLICT);
    fixture
        .json("POST", "/api/auth/logout", Some(json!({})))
        .await;
    assert_eq!(status(&fixture, &url, true).await, StatusCode::UNAUTHORIZED);
}
