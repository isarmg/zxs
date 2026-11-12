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

async fn note_target_fixture() -> (tempfile::TempDir, Router, String, String, String) {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "version-note.txt", b"version note").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let file = listing["entries"][0]["file_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let version = listing["entries"][0]["version_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (root, app, token, file, version)
}

#[tokio::test]
async fn version_note_missing_paths_precede_used_operation_and_keep_body_identity_order() {
    let (_root, app, token, _, version) = note_target_fixture().await;
    let operation = storage_contract::OperationId::from_random([1; 16]);
    let body = json!({"note":"historical note","operation_id":operation});
    success(
        &app,
        &token,
        "POST",
        &format!("/api/file-manager/versions/{version}/metadata"),
        Some(body.clone()),
    )
    .await;
    for missing in [
        "malformed",
        "ver_0123456789ABCDEF0123456789abcdef",
        "file_0123456789abcdef0123456789abcdef",
        "ver_0123456789abcdef0123456789abcdef",
    ] {
        let route = format!("/api/file-manager/versions/{missing}/metadata");
        let mut matching = body.clone();
        matching["version_id"] = json!(missing);
        let (status, result) = send(&app, Some(&token), "POST", &route, Some(matching)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(result["error"]["code"], "VERSION_NOT_FOUND");
        for supplied in [Value::Null, json!(9), json!("different")] {
            let mut conflicting = body.clone();
            conflicting["version_id"] = supplied;
            let (status, result) = send(
                &app,
                Some(&token),
                "POST",
                &route,
                Some(conflicting),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(result["error"]["code"], "INVALID_REQUEST");
        }
    }
}

fn version_note_clear_digest(file: &str, version: &str) -> String {
    let body = json!({"file_id":file,"version_id":version,"note":""});
    assert_eq!(
        storage_contract::canonical_json(&body).unwrap(),
        format!("{{\"file_id\":\"{file}\",\"note\":\"\",\"version_id\":\"{version}\"}}").as_bytes(),
    );
    storage_contract::object_hash(
        "request",
        &json!({"kind":"version.metadata","principal":"admin","body":body}),
    )
    .unwrap()
}

#[tokio::test]
async fn version_note_wire_digest_and_permanent_receipts_survive_clear_replay_and_reopen() {
    let (root, app, token, file, version) = note_target_fixture().await;
    let route = format!("/api/file-manager/versions/{version}/metadata");
    let operation = storage_contract::OperationId::from_random([2; 16]);
    let clear = json!({"note":"","operation_id":operation,"version_id":version});
    let original = success(&app, &token, "POST", &route, Some(clear.clone())).await;
    assert_eq!(original["operation_kind"], "version.metadata");
    assert_eq!(original["result"]["file_id"], file);
    assert_eq!(original["result"]["version_id"], version);
    assert_eq!(original["result"]["note"], "");
    let digest = version_note_clear_digest(&file, &version);
    assert_eq!(original["request_digest"], digest);
    success(
        &app,
        &token,
        "POST",
        &route,
        Some(json!({
            "note":"later note",
            "operation_id":storage_contract::OperationId::from_random([3; 16]),
        })),
    )
    .await;
    let replay = success(&app, &token, "POST", &route, Some(clear)).await;
    assert_eq!(
        storage_contract::canonical_json(&replay).unwrap(),
        storage_contract::canonical_json(&original).unwrap(),
    );
    drop(app);
    let engine = reopen_after_router_drop(&root.path().join("system")).await;
    assert_eq!(engine.state().version_notes[&version], "later note");
    let receipt = &engine.state().operations[&operation];
    assert_eq!(receipt.request_digest, digest);
    let bytes = storage_contract::canonical_json(receipt).unwrap();
    let decoded: storage_contract::OperationResult = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(storage_contract::canonical_json(&decoded).unwrap(), bytes);
    drop(engine);
    let reopened = Engine::open(&root.path().join("system")).unwrap();
    assert_eq!(
        storage_contract::canonical_json(&reopened.state().operations[&operation]).unwrap(),
        bytes,
    );
    assert_eq!(reopened.state().version_notes[&version], "later note");
}

#[tokio::test]
async fn file_note_missing_paths_precede_used_operation_and_keep_body_identity_order() {
    let (_root, app, token, file, _) = note_target_fixture().await;
    let operation = storage_contract::OperationId::from_random([1; 16]);
    let body = json!({"note":"historical note","operation_id":operation});
    success(
        &app,
        &token,
        "POST",
        &format!("/api/file-manager/files/{file}/metadata"),
        Some(body.clone()),
    )
    .await;
    for missing in [
        "malformed",
        "file_0123456789ABCDEF0123456789abcdef",
        "ver_0123456789abcdef0123456789abcdef",
        "file_0123456789abcdef0123456789abcdef",
    ] {
        let route = format!("/api/file-manager/files/{missing}/metadata");
        let mut matching = body.clone();
        matching["file_id"] = json!(missing);
        let (status, result) = send(&app, Some(&token), "POST", &route, Some(matching)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(result["error"]["code"], "FILE_NOT_FOUND");
        for supplied in [Value::Null, json!(9), json!("different")] {
            let mut conflicting = body.clone();
            conflicting["file_id"] = supplied;
            let (status, result) = send(
                &app,
                Some(&token),
                "POST",
                &route,
                Some(conflicting),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(result["error"]["code"], "INVALID_REQUEST");
        }
    }
}

fn file_note_clear_digest(file: &str) -> String {
    let body = json!({"file_id":file,"version_id":null,"note":""});
    assert_eq!(
        storage_contract::canonical_json(&body).unwrap(),
        format!("{{\"file_id\":\"{file}\",\"note\":\"\",\"version_id\":null}}").as_bytes(),
    );
    storage_contract::object_hash(
        "request",
        &json!({"kind":"file.metadata","principal":"admin","body":body}),
    )
    .unwrap()
}

#[tokio::test]
async fn file_note_wire_digest_and_permanent_receipts_survive_clear_replay_and_reopen() {
    let (root, app, token, file, _) = note_target_fixture().await;
    let route = format!("/api/file-manager/files/{file}/metadata");
    let operation = storage_contract::OperationId::from_random([2; 16]);
    let clear = json!({"note":"","operation_id":operation,"file_id":file});
    let original = success(&app, &token, "POST", &route, Some(clear.clone())).await;
    assert_eq!(original["operation_kind"], "file.metadata");
    assert_eq!(original["result"]["file_id"], file);
    assert_eq!(original["result"]["version_id"], Value::Null);
    assert_eq!(original["result"]["note"], "");
    let digest = file_note_clear_digest(&file);
    assert_eq!(original["request_digest"], digest);
    success(
        &app,
        &token,
        "POST",
        &route,
        Some(json!({
            "note":"later note",
            "operation_id":storage_contract::OperationId::from_random([3; 16]),
        })),
    )
    .await;
    let replay = success(&app, &token, "POST", &route, Some(clear)).await;
    assert_eq!(
        storage_contract::canonical_json(&replay).unwrap(),
        storage_contract::canonical_json(&original).unwrap(),
    );
    drop(app);
    let engine = reopen_after_router_drop(&root.path().join("system")).await;
    assert_eq!(engine.state().file_notes[&file], "later note");
    let receipt = &engine.state().operations[&operation];
    assert_eq!(receipt.request_digest, digest);
    let bytes = storage_contract::canonical_json(receipt).unwrap();
    let decoded: storage_contract::OperationResult = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(storage_contract::canonical_json(&decoded).unwrap(), bytes);
    drop(engine);
    let reopened = Engine::open(&root.path().join("system")).unwrap();
    assert_eq!(
        storage_contract::canonical_json(&reopened.state().operations[&operation]).unwrap(),
        bytes,
    );
    assert_eq!(reopened.state().file_notes[&file], "later note");
}

async fn node_note_fixture() -> (tempfile::TempDir, Router, String, String, [String; 2]) {
    let (root, app, token, file, version) = note_target_fixture().await;
    let branch = success(
        &app,
        &token,
        "POST",
        &format!("/api/file-manager/files/{file}/branches"),
        Some(json!({
            "source_version_id":version,
            "source_node_id":version,
            "name":"Note target",
            "operation_id":new_id("op").unwrap(),
        })),
    )
    .await["result"]["branch_id"].as_str().unwrap().to_owned();
    success(
        &app,
        &token,
        "POST",
        &format!("/api/file-manager/files/{file}/select"),
        Some(json!({"branch_id":branch,"operation_id":new_id("op").unwrap()})),
    )
    .await;
    manager_namespace(&app, &token, "rename", "version-note.txt", Some("node-note.txt")).await;
    let listing = success(
        &app, &token, "GET", "/api/file-manager/disks/P01/entries", None,
    )
    .await;
    let anchor = listing["entries"][0]["node_id"].as_str().unwrap().to_owned();
    // Container labels do not create metadata nodes. A real historical merge
    // of the retained content and its rename anchor supplies the second kind.
    let merged = success(
        &app,
        &token,
        "POST",
        &format!("/api/file-manager/files/{file}/merge-history"),
        Some(json!({
            "inputs":[
                {"branch_id":branch,"source_node_id":version,"expected_node_id":anchor},
                {"branch_id":branch,"source_node_id":anchor,"expected_node_id":anchor}
            ],
            "chosen_node_id":version,
            "name":"Node note metadata",
            "confirmed":true,
            "operation_id":new_id("op").unwrap()
        })),
    )
    .await;
    let metadata = merged["result"]["node_id"].as_str().unwrap().to_owned();
    assert!(anchor.starts_with("fnode_"));
    assert!(metadata.starts_with("fmeta_"));
    (root, app, token, file, [anchor, metadata])
}

#[tokio::test]
async fn node_note_unknown_paths_preserve_body_file_and_idempotency_order() {
    let (_root, app, token, file, _) = note_target_fixture().await;
    let operation = storage_contract::OperationId::from_random([13; 16]);
    success(
        &app, &token, "POST", &format!("/api/file-manager/files/{file}/metadata"),
        Some(json!({"note":"","operation_id":operation})),
    )
    .await;
    for node in [
        "malformed",
        "fnode_0123456789ABCDEF0123456789abcdef",
        "fnode_0123456789abcdef0123456789abcdef",
        "fmeta_0123456789abcdef0123456789abcdef",
        "ver_0123456789abcdef0123456789abcdef",
        "fdel_0123456789abcdef0123456789abcdef",
        "fmerge_0123456789abcdef0123456789abcdef",
    ] {
        let route = format!("/api/file-manager/files/{file}/nodes/{node}");
        for (operation_id, expected_status, expected_code) in [
            (new_id("op").unwrap(), StatusCode::NOT_FOUND, "FILE_NODE_NOT_FOUND"),
            (operation.to_string(), StatusCode::CONFLICT, "IDEMPOTENCY_KEY_REUSED"),
        ] {
            let (status, result) = send(
                &app, Some(&token), "PATCH", &route,
                Some(json!({"note":"","operation_id":operation_id,"file_id":file,"node_id":node})),
            )
            .await;
            assert_eq!(status, expected_status);
            assert_eq!(result["error"]["code"], expected_code);
        }
        assert_node_body_errors(&app, &token, &route, operation).await;
    }
    for missing in ["malformed", "file_0123456789abcdef0123456789abcdef"] {
        let route = format!("/api/file-manager/files/{missing}/nodes/malformed");
        let (status, result) = send(
            &app, Some(&token), "PATCH", &route,
            Some(json!({"note":"","operation_id":operation})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(result["error"]["code"], "FILE_NOT_FOUND");
    }
}

async fn assert_node_body_errors(
    app: &Router,
    token: &str,
    route: &str,
    operation: storage_contract::OperationId,
) {
    let (status, result) = send(app, None, "PATCH", route, Some(json!(null))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(result["error"]["code"], "UNAUTHORIZED");
    for body in [
        json!({"note":null,"operation_id":operation}),
        json!({"note":"","operation_id":"bad"}),
        json!({"note":"","operation_id":operation,"unknown":0}),
        json!({"note":"","operation_id":operation,"file_id":null}),
        json!({"note":"","operation_id":operation,"file_id":7}),
        json!({"note":"","operation_id":operation,"file_id":"different"}),
        json!({"note":"","operation_id":operation,"node_id":null}),
        json!({"note":"","operation_id":operation,"node_id":7}),
        json!({"note":"","operation_id":operation,"node_id":"different"}),
    ] {
        let (status, result) = send(app, Some(token), "PATCH", route, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(result["error"]["code"], "INVALID_REQUEST");
    }
}

fn node_note_clear_digest(file: &str, node: &str) -> String {
    let body = json!({"file_id":file,"node_id":node,"note":""});
    assert_eq!(
        storage_contract::canonical_json(&body).unwrap(),
        format!("{{\"file_id\":\"{file}\",\"node_id\":\"{node}\",\"note\":\"\"}}").as_bytes(),
    );
    storage_contract::object_hash(
        "request",
        &json!({"kind":"file.path.metadata","principal":"admin","body":body}),
    )
    .unwrap()
}

async fn node_note_clear_and_replay(
    app: &Router,
    token: &str,
    file: &str,
    node: &str,
    operation: storage_contract::OperationId,
) -> (Value, String) {
    let route = format!("/api/file-manager/files/{file}/nodes/{node}");
    let clear = json!({"note":"","operation_id":operation,"file_id":file,"node_id":node});
    let original = success(app, token, "PATCH", &route, Some(clear.clone())).await;
    assert_eq!(original["operation_kind"], "file.path.metadata");
    assert_eq!(original["result"]["file_id"], file);
    assert_eq!(original["result"]["node_id"], node);
    assert_eq!(original["result"]["note"], "");
    let digest = node_note_clear_digest(file, node);
    assert_eq!(original["request_digest"], digest);
    success(
        app, token, "PATCH", &route,
        Some(json!({"note":"later note","operation_id":new_id("op").unwrap()})),
    )
    .await;
    let replay = success(app, token, "PATCH", &route, Some(clear)).await;
    assert_eq!(
        storage_contract::canonical_json(&replay).unwrap(),
        storage_contract::canonical_json(&original).unwrap(),
    );
    (original, digest)
}

#[tokio::test]
async fn node_note_both_prefixes_keep_wire_digest_receipts_and_cold_replay() {
    let (root, app, token, file, nodes) = node_note_fixture().await;
    let mut receipts = Vec::new();
    for (node, operation) in nodes.iter().zip([
        storage_contract::OperationId::from_random([14; 16]),
        storage_contract::OperationId::from_random([15; 16]),
    ]) {
        let (wire, digest) = node_note_clear_and_replay(
            &app, &token, &file, node, operation,
        )
        .await;
        receipts.push((node.clone(), operation, wire, digest));
    }
    drop(app);
    let mut engine = reopen_after_router_drop(&root.path().join("system")).await;
    let mut stored = Vec::new();
    for (node, operation, wire, digest) in receipts {
        assert_eq!(engine.state().file_path_notes[&node], "later note");
        let receipt = &engine.state().operations[&operation];
        assert_eq!(receipt.request_digest, digest);
        assert_eq!(receipt.result, wire["result"]);
        let bytes = storage_contract::canonical_json(receipt).unwrap();
        let decoded: storage_contract::OperationResult = json_codec::from_slice(&bytes).unwrap();
        assert_eq!(storage_contract::canonical_json(&decoded).unwrap(), bytes);
        let replay = engine.set_file_node_note(
            &file, &node, &storage_contract::Note::parse("".into()).unwrap(), "admin", operation,
        )
        .unwrap();
        assert_eq!(storage_contract::canonical_json(&replay).unwrap(), bytes);
        assert_eq!(engine.state().file_path_notes[&node], "later note");
        stored.push((node, operation, bytes));
    }
    drop(engine);
    let reopened = Engine::open(&root.path().join("system")).unwrap();
    for (node, operation, bytes) in stored {
        assert_eq!(
            storage_contract::canonical_json(&reopened.state().operations[&operation]).unwrap(),
            bytes,
        );
        assert_eq!(reopened.state().file_path_notes[&node], "later note");
    }
}
