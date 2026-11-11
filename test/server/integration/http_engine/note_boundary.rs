use super::*;

#[tokio::test]
async fn note_identity_fields_preserve_wire_errors_and_matching_identity() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "note.txt", b"note boundary").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let file = listing["entries"][0]["file_id"].as_str().unwrap();
    let version = listing["entries"][0]["version_id"].as_str().unwrap();
    for node in [
        version.to_owned(),
        new_id("fdel").unwrap(),
        new_id("fmerge").unwrap(),
    ] {
        let route = format!("/api/file-manager/files/{file}/nodes/{node}");
        let (status, result) = send(
            &app,
            Some(&token),
            "PATCH",
            &route,
            Some(json!({"note":"","operation_id":new_id("op").unwrap()})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(result["error"]["code"], "FILE_NODE_NOT_FOUND");
    }
    for (route, key, id) in [
        (
            format!("/api/file-manager/files/{file}/metadata"),
            "file_id",
            file,
        ),
        (
            format!("/api/file-manager/versions/{version}/metadata"),
            "version_id",
            version,
        ),
    ] {
        let mut body = json!({"note":"same identity","operation_id":new_id("op").unwrap()});
        body[key] = json!(id);
        let saved = success(&app, &token, "POST", &route, Some(body)).await;
        assert_eq!(saved["result"]["note"], "same identity");
        for supplied in [Value::Null, json!(7), json!("different")] {
            let mut body = json!({"note":"","operation_id":new_id("op").unwrap()});
            body[key] = supplied;
            let (status, result) = send(&app, Some(&token), "POST", &route, Some(body)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(result["error"]["code"], "INVALID_REQUEST");
        }
        let duplicate = format!(
            "{{\"{key}\":\"{id}\",\"{key}\":\"{id}\",\"note\":\"\",\"operation_id\":\"{}\"}}",
            new_id("op").unwrap()
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&route)
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(duplicate))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let result: Value = json_codec::from_slice(&bytes).unwrap();
        assert_eq!(result["error"]["code"], "INVALID_REQUEST");
        let (status, result) = send(
            &app,
            Some(&token),
            "POST",
            &route,
            Some(json!({"note":"","operation_id":new_id("op").unwrap(),"unknown":true})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(result["error"]["code"], "INVALID_REQUEST");
    }
}

#[tokio::test]
async fn note_authorization_and_body_errors_precede_target_lookup() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    for (route, missing) in [
        ("/api/file-manager/files/malformed/metadata", "FILE_NOT_FOUND"),
        (
            "/api/file-manager/versions/malformed/metadata",
            "VERSION_NOT_FOUND",
        ),
    ] {
        let (status, result) = send(&app, None, "POST", route, Some(json!(null))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(result["error"]["code"], "UNAUTHORIZED");
        let (status, result) = send(
            &app,
            Some(&token),
            "POST",
            route,
            Some(json!({"note":null,"operation_id":"bad"})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(result["error"]["code"], "INVALID_REQUEST");
        let (status, result) = send(
            &app,
            Some(&token),
            "POST",
            route,
            Some(json!({"note":"","operation_id":new_id("op").unwrap()})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(result["error"]["code"], missing);
    }
}
