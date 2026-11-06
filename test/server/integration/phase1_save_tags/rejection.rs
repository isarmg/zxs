//! Rejected real Engine mutations must not change persisted file history.
use super::*;

fn tag_request(history: &Value, operation: &str, tag: &str) -> Value {
    let context = &history["identity"]["context"];
    json!({
        "targets": [{
            "branch_id": context["selected_branch_id"],
            "source_node_id": context["current_node_id"],
            "expected_node_id": context["current_node_id"]
        }],
        "repository_id": context["repository_id"],
        "path": context["path"],
        "add": [tag],
        "remove": [],
        "operation_id": operation
    })
}

async fn rejected(
    fixture: &Fixture,
    route: &str,
    body: Value,
    expected_status: StatusCode,
    expected_code: &str,
) {
    let response = tokio::time::timeout(
        Duration::from_secs(10),
        fixture.app.clone().oneshot(
            Request::builder()
                .method("POST")
                .uri(route)
                .header("authorization", format!("Bearer {}", fixture.token))
                .header("content-type", "application/json")
                .body(Body::from(json_codec::to_vec(&body).unwrap()))
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    assert_eq!(
        status,
        expected_status,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let body: Value = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], expected_code);
}

async fn login_again(fixture: &mut Fixture) {
    let result = request(
        &fixture.app,
        None,
        "POST",
        "/api/auth/login",
        Some(json!({"username":"admin", "password":"phase-one-test-password"})),
    )
    .await;
    fixture.token = result["token"].as_str().unwrap().to_owned();
}

#[tokio::test]
async fn revoked_file_mutations_leave_history_and_bytes_unchanged_after_cold_open() {
    let mut fixture = Fixture::new(&["primary"]).await;
    let path = "protected.txt";
    let bytes = b"published bytes survive rejected mutation";
    fixture.save("D01", path, bytes).await;
    let entry = fixture.entry("D01", path).await;
    let file = entry["file_id"].as_str().unwrap();
    let version = entry["version_id"].as_str().unwrap();
    let before = fixture.history(file).await;
    let versions = fixture.versions(file, &before).await;
    let route = format!("/api/file-manager/files/{file}/tags");
    let proposed = tag_request(&before, &new_id("op").unwrap(), "authorized-later");

    let logout = fixture
        .json("POST", "/api/auth/logout", Some(json!({})))
        .await;
    assert_eq!(logout["logged_out"], true);
    rejected(
        &fixture,
        &route,
        proposed.clone(),
        StatusCode::UNAUTHORIZED,
        "UNAUTHORIZED",
    )
    .await;
    rejected(
        &fixture,
        "/api/file-manager/disks/D01/write-sessions",
        json!({"path":path}),
        StatusCode::UNAUTHORIZED,
        "UNAUTHORIZED",
    )
    .await;

    login_again(&mut fixture).await;
    let fixture = fixture.reopen().await;
    assert_eq!(fixture.history(file).await, before);
    assert_eq!(fixture.entry("D01", path).await, entry);
    assert_eq!(fixture.versions(file, &before).await, versions);
    assert_eq!(fixture.body(version).await, bytes);

    // Reuse the exact rejected mutation, including its operation identity.
    let applied = fixture.json("POST", &route, Some(proposed)).await;
    assert_eq!(applied["result"]["content_versions_created"], "0");
    let after = fixture.history(file).await;
    assert_eq!(current_node(&after)["tags"], json!(["authorized-later"]));
    assert_ne!(
        current_node(&after)["node_id"],
        current_node(&before)["node_id"]
    );
    assert_eq!(fixture.body(version).await, bytes);
}

#[tokio::test]
async fn stale_tag_failure_is_atomic_and_does_not_consume_retry_identity() {
    let fixture = Fixture::new(&["primary"]).await;
    let path = "stale-target.txt";
    let bytes = b"only one body version";
    fixture.save("D01", path, bytes).await;
    let entry = fixture.entry("D01", path).await;
    let file = entry["file_id"].as_str().unwrap();
    let version = entry["version_id"].as_str().unwrap();
    let old = fixture.history(file).await;
    fixture.tags(file, &["first"], &[]).await;
    let before = fixture.history(file).await;
    let operation = new_id("op").unwrap();
    let route = format!("/api/file-manager/files/{file}/tags");
    rejected(
        &fixture,
        &route,
        tag_request(&old, &operation, "second"),
        StatusCode::CONFLICT,
        "FILE_BRANCH_CONFLICT",
    )
    .await;
    assert_eq!(fixture.history(file).await, before);

    let fixture = fixture.reopen().await;
    assert_eq!(fixture.history(file).await, before);
    assert_eq!(fixture.body(version).await, bytes);
    let applied = fixture
        .json(
            "POST",
            &route,
            Some(tag_request(&before, &operation, "second")),
        )
        .await;
    assert_eq!(applied["operation_id"], operation);
    assert_eq!(applied["result"]["content_versions_created"], "0");
    let after = fixture.history(file).await;
    assert_eq!(current_node(&after)["tags"], json!(["first", "second"]));
    assert_eq!(
        fixture
            .versions(file, &after)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let previous = current_node(&before);
    let retained = after["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["node_id"] == previous["node_id"])
        .unwrap();
    for field in [
        "node_id",
        "version_id",
        "kind",
        "sequence",
        "branch_id",
        "tags",
        "size",
        "content_hash",
    ] {
        assert!(previous.get(field).is_some(), "missing node field {field}");
        assert_eq!(
            retained[field], previous[field],
            "changed node field {field}"
        );
    }
    // Selection is live presentation state, not an immutable node property.
    assert_eq!(previous["current"], true);
    assert_eq!(retained["current"], false);

    let fixture = fixture.reopen().await;
    assert_eq!(fixture.history(file).await, after);
    assert_eq!(fixture.body(version).await, bytes);
}
