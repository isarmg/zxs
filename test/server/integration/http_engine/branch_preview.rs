//! Read-only branch browsing and explicit baseline-bound default publication.
use super::*;

#[tokio::test]
async fn directory_preview_is_a_read_and_only_explicit_default_restores_the_subtree() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "folder/file.txt", b"frozen").await;
    manager_upload(&app, &token, "outside.txt", b"outside").await;
    let root_page = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let repository = root_page["context"]["repository_id"].as_str().unwrap();
    let branch = manager_prepare_directory_branch(&app, &token, "folder", "frozen").await;
    manager_upload(&app, &token, "folder/file.txt", b"later").await;
    let request =
        manager_prepare_default_request(&app, &token, repository, &branch, &new_id("op").unwrap())
            .await;
    let tree = request["expected_tree_id"].as_str().unwrap();
    let default_route = format!("/api/repositories/{repository}/default-branch");
    let before = success(&app, &token, "GET", &default_route, None).await;
    let audit = success(&app, &token, "GET", "/api/file-manager/history", None).await;
    let preview = success(&app, &token, "GET", &format!(
        "/api/file-manager/disks/P01/entries?path=folder&directory_branch_id={branch}&expected_tree_id={tree}"
    ), None).await;
    assert_eq!(preview["preview"], true);
    assert_eq!(preview["writable"], false);
    assert_eq!(preview["entries"][0]["read_only"], true);
    assert_eq!(
        version_bytes(
            &app,
            &token,
            preview["entries"][0]["version_id"].as_str().unwrap()
        )
        .await,
        b"frozen"
    );
    assert_eq!(
        success(&app, &token, "GET", &default_route, None).await,
        before
    );
    assert_eq!(
        success(&app, &token, "GET", "/api/file-manager/history", None).await,
        audit
    );
    let live = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=folder",
        None,
    )
    .await;
    assert_ne!(
        live["entries"][0]["version_id"],
        preview["entries"][0]["version_id"]
    );
    let unchanged_bytes = manager_disk_files(root.path());
    for (field, prefix) in [("expected_head", "commit"), ("expected_tree_id", "dtree")] {
        let mut stale = request.clone();
        stale[field] = new_id(prefix).unwrap().into();
        stale["operation_id"] = new_id("op").unwrap().into();
        let (status, rejected) = send(&app, Some(&token), "PUT", &default_route, Some(stale)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{field}: {rejected}");
        assert_eq!(rejected["error"]["code"], "LISTING_CHANGED");
        assert_eq!(manager_disk_files(root.path()), unchanged_bytes);
    }
    let published = success(&app, &token, "PUT", &default_route, Some(request.clone())).await;
    assert_eq!(published["operation_kind"], "directory.git.checkout");
    assert_eq!(
        success(&app, &token, "PUT", &default_route, Some(request)).await,
        published
    );
    let live = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=folder",
        None,
    )
    .await;
    assert_eq!(
        live["entries"][0]["version_id"],
        preview["entries"][0]["version_id"]
    );
    let root_after = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    assert!(
        root_after["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "outside.txt")
    );
}

#[tokio::test]
async fn explicit_file_default_requires_the_viewed_node_and_replays_the_same_outcome() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "file.txt", b"content").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let entry = &listing["entries"][0];
    let file = entry["file_id"].as_str().unwrap();
    let repository = listing["context"]["repository_id"].as_str().unwrap();
    let created = success(&app, &token, "POST", &format!("/api/file-manager/files/{file}/branches"),
        Some(json!({"name":"draft","source_version_id":entry["version_id"],"operation_id":new_id("op").unwrap()}))).await;
    let branch = created["result"]["branch_id"].as_str().unwrap();
    let history = success(
        &app,
        &token,
        "GET",
        &format!("/api/file-manager/files/{file}/history"),
        None,
    )
    .await;
    let branches = history_related(&app, &token, file, &history, "branches", None).await;
    let viewed = branches["branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["branch_id"] == branch)
        .unwrap();
    let live = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    assert_eq!(
        live["entries"][0]["selected_branch_id"],
        entry["selected_branch_id"]
    );
    let route = format!("/api/file-manager/files/{file}/default-branch");
    let request = json!({"branch_id":branch,"repository_id":repository,"path":"file.txt",
        "expected_node_id":viewed["node_id"],"expected_head":live["context"]["base_head"],"operation_id":new_id("op").unwrap()});
    let mut stale = request.clone();
    stale["expected_node_id"] = "stale".into();
    stale["operation_id"] = new_id("op").unwrap().into();
    let (status, rejected) = send(&app, Some(&token), "PUT", &route, Some(stale)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    assert_eq!(rejected["error"]["code"], "LISTING_CHANGED");
    let published = success(&app, &token, "PUT", &route, Some(request.clone())).await;
    assert_eq!(published["operation_kind"], "file.default.set");
    assert_eq!(
        success(&app, &token, "PUT", &route, Some(request.clone())).await,
        published
    );
    let current = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    assert_eq!(current["entries"][0]["selected_branch_id"], branch);
    assert_eq!(current["entries"][0]["version_id"], entry["version_id"]);
    assert_eq!(
        send(&app, None, "PUT", &route, Some(request)).await.0,
        StatusCode::UNAUTHORIZED
    );
}
