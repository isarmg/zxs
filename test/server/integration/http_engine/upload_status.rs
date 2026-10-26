//! Status must expose a failed checksum without changing replay or recovery.
use super::*;

async fn put(
    app: &Router,
    token: &str,
    content_uri: &str,
    generation: u64,
    expected: &[u8],
    bytes: &[u8],
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(content_uri)
                .header("authorization", format!("Bearer {token}"))
                .header("x-zxs-logical-size", expected.len().to_string())
                .header(
                    "x-zxs-content-hash",
                    blake3::hash(expected).to_hex().as_str(),
                )
                .header("x-zxs-upload-generation", generation.to_string())
                .body(Body::from(bytes.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    (status, json_codec::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn checksum_failure_status_survives_recovery_and_requires_new_generation_for_correction() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let repo = success(
        &app,
        &token,
        "POST",
        "/api/repositories",
        Some(json!({
            "name":"Status", "primary_slot_id":"P01", "target_copies":3,
            "operation_id":new_id("op")
        })),
    )
    .await;
    let workspace = success(
        &app,
        &token,
        "POST",
        "/api/workspaces",
        Some(json!({
            "branch_id":repo["main_branch_id"]
        })),
    )
    .await;
    let ws = workspace["workspace_id"].as_str().unwrap();
    let item = new_id("entry");
    let base = format!("/api/workspaces/{ws}/files/{item}");
    let content_uri = format!("{base}/content?path=private-name.txt");
    let status_uri = format!("{base}/upload-status");
    let expected = b"right";
    let wrong = b"wrong";

    let (code, body) = put(&app, &token, &content_uri, 1, expected, wrong).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "CHECKSUM_MISMATCH");
    let before = success(&app, &token, "GET", &status_uri, None).await;
    // Exact public whitelist, including expected vs actual hash; no principal,
    // logical path, data_file, or additional error flags may leak from the record.
    assert_eq!(
        before,
        json!({
            "workspace_id":ws, "item_id":item, "upload_generation":"1",
            "logical_size":"5", "content_hash":blake3::hash(expected).to_hex().as_str(),
            "state":"CHECKSUM_MISMATCH", "sealed_manifest_hash":null,
            "durable_ranges":[{"offset":"0", "length":"5",
                "content_hash":blake3::hash(wrong).to_hex().as_str()}]
        })
    );
    drop(app);
    let system = root.path().join("system");
    let engine = reopen_after_router_drop(&system).await;
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend
        .protect_auth(AuthStore::open(&system).unwrap())
        .unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    assert_eq!(
        success(&app, &token, "GET", &status_uri, None).await,
        before
    );

    for (declared, bytes, code, error) in [
        (
            expected.as_slice(),
            wrong.as_slice(),
            StatusCode::BAD_REQUEST,
            "CHECKSUM_MISMATCH",
        ),
        (
            expected.as_slice(),
            expected.as_slice(),
            StatusCode::CONFLICT,
            "UPLOAD_RANGE_CONFLICT",
        ),
        (
            wrong.as_slice(),
            wrong.as_slice(),
            StatusCode::CONFLICT,
            "UPLOAD_GENERATION_CONFLICT",
        ),
    ] {
        let (actual_code, response) = put(&app, &token, &content_uri, 1, declared, bytes).await;
        assert_eq!(actual_code, code);
        assert_eq!(response["error"]["code"], error);
        assert_eq!(
            success(&app, &token, "GET", &status_uri, None).await,
            before
        );
    }

    // A new generation can be incomplete, then replay its verified prefix and
    // finish. A checksum failure is not a terminal workspace state.
    let (code, response) = put(&app, &token, &content_uri, 2, expected, &expected[..2]).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(response["error"]["code"], "UPLOAD_INCOMPLETE");
    let receiving = success(&app, &token, "GET", &status_uri, None).await;
    assert_eq!(receiving["state"], "RECEIVING");
    assert_eq!(receiving["upload_generation"], "2");
    assert_eq!(receiving["durable_ranges"][0]["length"], "2");
    assert_eq!(
        put(&app, &token, &content_uri, 2, expected, expected)
            .await
            .0,
        StatusCode::OK
    );
    let complete = success(&app, &token, "GET", &status_uri, None).await;
    assert_eq!(complete["state"], "COMPLETE");
    assert_eq!(complete["upload_generation"], "2");
    assert_eq!(complete["durable_ranges"][0]["length"], "5");

    let retained = std::fs::read_dir(system.join("api-spool"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("retained-")
        })
        .collect::<Vec<_>>();
    assert_eq!(retained.len(), 1);
    let record: Value = json_codec::from_slice(&std::fs::read(&retained[0]).unwrap()).unwrap();
    assert_eq!(record["state"], "checksum_mismatch");
    assert_eq!(
        std::fs::read(
            system
                .join("api-spool")
                .join(record["data_file"].as_str().unwrap())
        )
        .unwrap(),
        wrong
    );

    success(&app, &token, "POST", "/api/auth/logout", None).await;
    let (code, response) = send(&app, Some(&token), "GET", &status_uri, None).await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    assert_eq!(response["error"]["code"], "UNAUTHORIZED");
    assert!(response.get("durable_ranges").is_none());
}
