use super::*;
use http_routing::http::Method;

#[tokio::test]
async fn entity_tag_precedes_modified_since_for_static_get_and_head() {
    let root = private_tempdir();
    let assets = root.path().join("web");
    std::fs::create_dir(&assets).unwrap();
    let current = b"<html>current asset</html>";
    std::fs::write(assets.join("index.html"), current).unwrap();
    let mut config = ServerConfig::loopback(root.path());
    config.web_dir = Some(assets);
    let app = application_config(root.path(), config);
    let initial = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/index.html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(initial.status(), StatusCode::OK);
    let etag = initial.headers()[header::ETAG].clone();
    let modified = initial.headers()[header::LAST_MODIFIED].clone();
    assert_eq!(
        http_routing::body::to_bytes(initial.into_body(), 8192)
            .await
            .unwrap(),
        current.as_slice()
    );

    for method in [Method::GET, Method::HEAD] {
        // This exact date alone must still reach ServeDir's real date parser.
        let date_only = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method.clone())
                    .uri("/index.html")
                    .header(header::IF_MODIFIED_SINCE, modified.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(date_only.status(), StatusCode::NOT_MODIFIED, "{method}");
        assert!(
            http_routing::body::to_bytes(date_only.into_body(), 8192)
                .await
                .unwrap()
                .is_empty()
        );

        // A cache can hold an old entity tag even when timestamps are equal.
        let updated = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method.clone())
                    .uri("/index.html")
                    .header(header::IF_NONE_MATCH, "W/\"previous-asset\"")
                    .header(header::IF_MODIFIED_SINCE, modified.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            updated.status(),
            StatusCode::OK,
            "{method}: stale entity tag"
        );
        assert_eq!(updated.headers()[header::ETAG], etag);
        assert_eq!(updated.headers()[header::LAST_MODIFIED], modified);
        assert_eq!(
            updated.headers()[header::CONTENT_LENGTH]
                .to_str()
                .unwrap()
                .parse::<usize>()
                .unwrap(),
            current.len()
        );
        let body = http_routing::body::to_bytes(updated.into_body(), 8192)
            .await
            .unwrap();
        if method == Method::HEAD {
            assert!(body.is_empty());
        } else {
            assert_eq!(body, current.as_slice());
        }
    }
}

#[tokio::test]
async fn static_head_ignores_range_while_get_retains_range_and_empty_file_rules() {
    let root = private_tempdir();
    let assets = root.path().join("web");
    std::fs::create_dir(&assets).unwrap();
    std::fs::write(assets.join("index.html"), b"abcdef").unwrap();
    std::fs::write(assets.join("empty.txt"), []).unwrap();
    let mut config = ServerConfig::loopback(root.path());
    config.web_dir = Some(assets);
    let app = application_config(root.path(), config);
    for range in [
        None,
        Some("bytes=1-3"),
        Some("bytes=99-"),
        Some("bytes=missing"),
    ] {
        let mut request = Request::builder().method(Method::HEAD).uri("/index.html");
        if let Some(range) = range {
            request = request.header(header::RANGE, range);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{range:?}");
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "6");
        assert!(!response.headers().contains_key(header::CONTENT_RANGE));
        assert!(
            http_routing::body::to_bytes(response.into_body(), 8192)
                .await
                .unwrap()
                .is_empty()
        );
    }
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/index.html")
                .header(header::RANGE, "bytes=1-3")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 1-3/6");
    assert_eq!(
        http_routing::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap()
            .as_ref(),
        b"bcd"
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri("/empty.txt")
                .header(header::RANGE, "bytes=0-")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes */0");
}
