//! Route refusal is measured alongside the surviving registered FILE service.
use super::*;

#[tokio::test]
async fn removed_directory_history_routes_leave_the_registered_opaque_file_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let zip = b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";
    manager_upload(&app, &token, "folder/project.zxs", zip).await;
    let listing = "/api/file-manager/disks/P01/entries?path=folder";
    let before = success(&app, &token, "GET", listing, None).await;
    let repo = before["context"]["repository_id"].as_str().unwrap();
    let branch = before["context"]["branch_id"].as_str().unwrap();
    let head = before["context"]["base_head"].as_str().unwrap();
    let row = &before["entries"][0];
    let file = row["file_id"].as_str().unwrap();
    let version = row["version_id"].as_str().unwrap();
    assert_eq!(row["name"], "project.zxs");
    let events = success(&app, &token, "GET", "/api/events", None).await;
    let root_path = format!("/api/repositories/{repo}");
    for (verb, uri) in [
        ("POST", "/api/file-manager/disks/P01/namespace-batch".into()),
        ("POST", "/api/file-manager/disks/P01/directory-branches".into()),
        ("POST", "/api/file-manager/disks/P01/directory-branches/from-history".into()),
        ("GET", format!("{root_path}/default-branch")), ("PUT", format!("{root_path}/default-branch")),
        ("GET", format!("{root_path}/directories/{repo}/history")),
        ("GET", format!("{root_path}/directories/{repo}/history/branches")),
        ("GET", format!("{root_path}/directory-git")),
        ("POST",format!("{root_path}/directory-git/initialize")),
        ("POST",format!("{root_path}/directory-git/children/stage")),
        ("POST",format!("{root_path}/directory-git/stage")),
        ("GET", format!("{root_path}/directory-branches")), ("POST",format!("{root_path}/directory-branches")),
        ("GET",format!("{root_path}/directory-branches/{branch}/staging")),
        ("POST",format!("{root_path}/directory-branches/{branch}/commits")),
        ("GET",format!("{root_path}/directory-deletions/{repo}")),
        ("GET",format!("{root_path}/directory-deletions/{repo}/{branch}")),
        ("POST",format!("{root_path}/directory-deletions/undo")),
    ] {
        let body = json!({"operation_id":new_id("op").unwrap(),"expected_tree_id":head});
        let (status, response) = send(&app, Some(&token), verb, &uri, Some(body)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{verb} {uri}: {response}");
        assert_eq!(response["error"]["message"], "route not found");
    }
    assert_eq!(success(&app,&token,"GET",listing,None).await, before);
    assert_eq!(success(&app,&token,"GET","/api/events",None).await,events);
    assert_eq!(version_bytes(&app,&token,version).await, zip);
    drop(app);
    let engine = reopen_after_router_drop(&root.path().join("system")).await;
    let admitted = engine.list_tree(branch).unwrap();
    let reopened = admitted.iter().find(|entry| entry.path() == "folder/project.zxs").unwrap();
    assert_eq!(reopened.file_id, file);
    assert_eq!(reopened.version_id, version);
}
