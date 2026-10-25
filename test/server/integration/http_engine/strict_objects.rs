//! Named request records must not admit positional JSON arrays at any depth.
use super::*;

#[tokio::test]
async fn namespace_batch_requires_named_nested_objects_before_mutation() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "report.txt", b"retain this file").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let branch = manager_directory_branch(&app, &token, "", "schema-check").await;
    let before = manager_scope(&app, &token, repo, &branch).await;
    let source = manager_batch_source(&before, "report.txt");
    let target = manager_batch_target(&before, "renamed.txt");
    let source_array = json!([
        source["directory_branch_id"],
        source["expected_tree_id"],
        source["path"],
        source["entry_id"]
    ]);
    let target_array = json!([
        target["directory_branch_id"],
        target["expected_tree_id"],
        target["path"]
    ]);
    let route = "/api/file-manager/disks/P01/namespace-batch";
    for item in [
        json!([source, target]),
        json!({"source":source_array,"target":target}),
        json!({"source":source,"target":target_array}),
    ] {
        let operation = new_id("op");
        let (status, rejected) = send(
            &app,
            Some(&token),
            "POST",
            route,
            Some(json!({"kind":"rename","items":[item],"operation_id":operation})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
        assert_eq!(rejected["error"]["code"], "INVALID_REQUEST");
        assert_eq!(manager_scope(&app, &token, repo, &branch).await, before);
    }
    // Legitimate arrays of named records remain supported.
    success(
        &app,
        &token,
        "POST",
        route,
        Some(
            json!({"kind":"rename","items":[{"source":source,"target":target}],
            "operation_id":new_id("op")}),
        ),
    )
    .await;
    let after = manager_scope(&app, &token, repo, &branch).await;
    assert_ne!(after["head_tree_id"], before["head_tree_id"]);
    assert!(
        after["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "renamed.txt")
    );
    assert!(
        after["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["path"] != "report.txt")
    );
}
