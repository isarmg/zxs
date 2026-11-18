//! The structural disk namespace and a real FILE branch remain accessible
//! without exposing a second repository or cloning every FILE in the root.
use super::*;

#[tokio::test]
async fn registered_root_and_file_branch_survive_retirement_of_whole_tree_creators() {
    let fixture = Fixture::new(&["primary"]).await;
    fixture.save("D01", "project.zxs", b"opaque ordinary file").await;
    let entry = fixture.entry("D01", "project.zxs").await;
    let file = entry["file_id"].as_str().unwrap();
    let version = entry["version_id"].as_str().unwrap();
    let owners = fixture.json("GET", "/api/repositories", None).await;
    let roots = owners["repositories"].as_array().unwrap();
    assert_eq!(roots.len(), 1);
    let root = &roots[0];
    assert_eq!(root["file_manager_root"], true);
    assert_eq!(root["primary_slot_id"], "D01");
    let repo = root["repository_id"].as_str().unwrap();
    let branches = format!("/api/repositories/{repo}/branches");
    let before = fixture.json("GET", &branches, None).await;
    let main = before["branches"].as_array().unwrap().iter()
        .find(|branch| branch["branch_id"] == root["main_branch_id"]).unwrap();
    for (url, body) in [
        ("/api/repositories", json!({"name":"detached","primary_slot_id":"D01",
            "target_copies":1,"operation_id":new_id("op").unwrap()})),
        (branches.as_str(), json!({"source_commit_id":main["head_commit_id"],
            "name":"clone-every-file","operation_id":new_id("op").unwrap()})),
    ] {
        let response = fixture.app.clone().oneshot(
            Request::builder().method("POST").uri(url)
                .header("authorization", format!("Bearer {}", fixture.token))
                .header("content-type", "application/json")
                .body(Body::from(json_codec::to_vec(&body).unwrap())).unwrap()
        ).await.unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
    assert_eq!(fixture.json("GET", "/api/repositories", None).await, owners);
    assert_eq!(fixture.json("GET", &branches, None).await, before);

    let created = fixture.json("POST", &format!("/api/file-manager/files/{file}/branches"),
        Some(json!({"source_version_id":version,"source_node_id":version,"name":"review",
            "operation_id":new_id("op").unwrap()}))).await;
    let id = created["result"]["branch_id"].as_str().unwrap();
    let branch = fixture.json("GET", &format!("/api/branches/{id}"), None).await;
    assert_eq!(branch["file_id"].as_str(), Some(file));
    assert_eq!(fixture.body(version).await, b"opaque ordinary file");
}
