use super::*;
use service_stack::ServiceExt;
use std::{
    fs::{File, FileTimes},
    sync::Arc,
};

static FIXTURE: Bundle = Bundle {
    source_commit: "fixture",
    source_tree: "fixture",
    manifest_sha256: "fixture",
    modified_seconds: 1_700_000_000,
    files: &[
        Asset {
            path: "api/hidden",
            bytes: b"must not escape",
        },
        Asset {
            path: "empty.txt",
            bytes: b"",
        },
        Asset {
            path: "index.html",
            bytes: b"<html>abcdef</html>",
        },
        Asset {
            path: "nested/index.html",
            bytes: b"nested",
        },
        Asset {
            path: "pkg/app.wasm",
            bytes: b"\0asm0123456789",
        },
        Asset {
            path: "pkg/app.wasm.gz",
            bytes: &[
                31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 99, 72, 44, 206, 53, 48, 52, 50, 54, 49, 53, 51,
                183, 176, 4, 0, 103, 234, 149, 92, 14, 0, 0, 0,
            ],
        },
    ],
};

fn protected(app: Router, tags: BTreeMap<String, String>) -> Router {
    let tags = Arc::new(tags);
    app.layer(middleware::from_fn(move |request: Request, next: Next| {
        let tags = Arc::clone(&tags);
        async move { super::super::validate_with_etags(&tags, request, next).await }
    }))
    .layer(middleware::from_fn(crate::security_headers))
}

async fn capture(
    app: Router,
    method: Method,
    path: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, http::HeaderMap, Vec<u8>) {
    let mut request = Request::builder().method(method).uri(path);
    for (key, value) in headers {
        request = request.header(*key, *value);
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let mut body = http_routing::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap()
        .to_vec();
    // Each independent request receives its own random correlation ID. Prove
    // its shape, then compare every other error field and all HTTP headers.
    if status == StatusCode::NOT_FOUND && !body.is_empty() {
        let mut value: json_codec::Value = json_codec::from_slice(&body).unwrap();
        let id = value["error"]["request_id"].as_str().unwrap();
        assert!(
            id.starts_with("req_")
                && id.len() == 36
                && id[4..].bytes().all(|b| b.is_ascii_hexdigit())
        );
        value["error"]["request_id"] = json_codec::Value::String(format!("req_{}", "0".repeat(32)));
        body = json_codec::to_vec(&value).unwrap();
    }
    (status, headers, body)
}

#[tokio::test]
async fn immutable_memory_http_matches_actual_disk_service_oracle() {
    let root = tempfile::tempdir().unwrap();
    for asset in FIXTURE.files {
        let path = root.path().join(asset.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, asset.bytes).unwrap();
        File::open(path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(
                SystemTime::UNIX_EPOCH + Duration::from_secs(FIXTURE.modified_seconds),
            ))
            .unwrap();
    }
    let disk = protected(
        super::super::fallback(root.path().into()),
        super::super::static_etags(root.path()).unwrap(),
    );
    let memory = protected(route(&FIXTURE), validators(FIXTURE.files));
    let tag = format!("W/\"{}\"", blake3::hash(FIXTURE.files[2].bytes).to_hex());
    let modified = httpdate::fmt_http_date(
        SystemTime::UNIX_EPOCH + Duration::from_secs(FIXTURE.modified_seconds),
    );
    let old = "Wed, 01 Jan 2020 00:00:00 GMT";
    let cases: Vec<Vec<(&str, &str)>> = vec![
        vec![],
        vec![("range", "bytes=1-3")],
        vec![("range", "bytes=-2")],
        vec![("range", "bytes=99-")],
        vec![("range", "bytes=missing")],
        vec![("range", "bytes=0-1,3-4")],
        vec![("accept-encoding", "gzip")],
        vec![("accept-encoding", "gzip;q=0")],
        vec![("accept-encoding", "gzip;q=0.5, identity;q=1")],
        vec![("accept-encoding", "identity;q=0.1, gzip;q=0.5")],
        vec![("accept-encoding", "*")],
        vec![("accept-encoding", "gzip"), ("range", "bytes=1-3")],
        vec![("if-modified-since", &modified)],
        vec![("if-unmodified-since", old)],
        vec![("if-none-match", &tag)],
        vec![("if-none-match", "*")],
        vec![
            ("if-none-match", "W/\"stale\""),
            ("if-modified-since", &modified),
        ],
    ];
    for method in [Method::GET, Method::HEAD, Method::POST] {
        for path in [
            "/",
            "/index.html",
            "/index.html?query=1",
            "/pkg/app.wasm",
            "/empty.txt",
            "/nested",
            "/nested/?q=1",
            "/absent",
            "/api/hidden",
            "/api/unknown",
            "/%2e%2e/index.html",
            "/pkg/%61pp.wasm",
        ] {
            for headers in &cases {
                let actual = capture(memory.clone(), method.clone(), path, headers).await;
                let expected = capture(disk.clone(), method.clone(), path, headers).await;
                assert_eq!(actual, expected, "{method} {path} {headers:?}");
            }
        }
    }
}

#[tokio::test]
async fn memory_assets_do_not_need_a_working_directory_or_any_asset_files() {
    let response = capture(
        protected(route(&FIXTURE), validators(FIXTURE.files)),
        Method::GET,
        "/index.html",
        &[],
    )
    .await;
    assert_eq!(response.0, StatusCode::OK);
    assert_eq!(response.2, FIXTURE.files[2].bytes);
    assert_eq!(response.1["x-content-type-options"], "nosniff");
    assert_eq!(response.1[header::CACHE_CONTROL], "no-cache");
    assert!(
        response.1["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
}

#[test]
fn rejects_parent_path_and_malformed_quality() {
    assert!(normalized_path("/%2e%2e/index.html").is_none());
    for value in ["q=1.1", "q=2", "q=-1", "q=0.0001", "other=1"] {
        assert!(quality(value).is_none());
    }
}

#[tokio::test]
async fn decoded_api_aliases_never_expose_static_bytes() {
    for path in ["/%61pi/hidden", "/api%2fhidden", "/%61%70%69/hidden"] {
        let response = capture(route(&FIXTURE), Method::GET, path, &[]).await;
        assert_eq!(response.0, StatusCode::NOT_FOUND);
        assert!(
            !response
                .2
                .windows(b"must not escape".len())
                .any(|part| part == b"must not escape")
        );
    }
}
