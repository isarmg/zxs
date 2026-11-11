//! Historical choice, current CAS and permanent operation identity cross the real HTTP boundary.
use super::*;

const CREATE: &str = "/api/file-manager/disks/P01/directory-branches/from-history";

fn body(current: &Value, tree: &Value, name: &str) -> Value {
    json!({
        "path": "docs", "name": name, "operation_id": new_id("op").unwrap(),
        "source": {"current": current, "tree_id": tree}
    })
}

#[tokio::test]
async fn historical_directory_branch_preserves_current_and_replays_exact_receipt_after_reopen() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/a.txt", b"old committed content").await;
    manager_checkpoint_git(&app, &token, "docs").await;
    let old = manager_directory_branch_source(&app, &token, "docs").await;
    let old_scope = manager_scope(
        &app, &token, old["repository_id"].as_str().unwrap(),
        old["directory_branch_id"].as_str().unwrap(),
    ).await;
    manager_upload(&app, &token, "docs/a.txt", b"current committed content").await;
    manager_checkpoint_git(&app, &token, "docs").await;
    let current = manager_directory_branch_source(&app, &token, "docs").await;
    assert_ne!(old["head_tree_id"], current["head_tree_id"]);
    let request = body(&current, &old["head_tree_id"], "old snapshot");
    let receipt = success(&app, &token, "POST", CREATE, Some(request.clone())).await;
    assert_eq!(receipt["operation_kind"], "directory.git.branch-create-history");
    assert_eq!(receipt["result"]["head_tree_id"], old["head_tree_id"]);
    assert_eq!(receipt["result"]["selected"], false);
    assert_eq!(receipt["result"]["materialized"], false);
    assert_eq!(receipt["result"]["copied_files"], "0");
    assert_eq!(receipt["result"]["placeholder_files"], "0");
    assert_eq!(manager_directory_branch_source(&app, &token, "docs").await, current);
    let input = json!({
        "repository_id": current["repository_id"], "path": "docs", "name": "old snapshot",
        "source": request["source"]
    });
    let digest = storage_contract::object_hash("request", &json!({
        "kind": "directory.git.branch-create-history", "principal": "admin", "body": input
    })).unwrap();
    assert_eq!(receipt["request_digest"], digest);
    assert_eq!(receipt["result"]["input"], input);
    let repo = current["repository_id"].as_str().unwrap();
    let branch = receipt["result"]["directory_branch_id"].as_str().unwrap();
    let created = manager_scope(&app, &token, repo, branch).await;
    assert_eq!(created["head_tree_id"], old["head_tree_id"]);
    assert_eq!(created["entries"], old_scope["entries"]);
    let audit = success(&app, &token, "GET", "/api/file-manager/history", None).await;
    assert!(audit["events"].as_array().unwrap().iter().any(|event| {
        event["action"] == "directory.git.branch-create-history"
            && event["directory_branch_id"] == branch
            && event["head_tree_id"] == old["head_tree_id"]
    }));
    let defaults = success(
        &app, &token, "GET", &format!("/api/repositories/{repo}/default-branch"), None,
    ).await;
    let tree = success(
        &app, &token, "GET",
        &format!("/api/branches/{}/tree", defaults["namespace_branch_id"].as_str().unwrap()),
        None,
    ).await;
    assert_eq!(tree["commit_id"], current["namespace_head"]);

    manager_upload(&app, &token, "docs/a.txt", b"later saved content").await;
    manager_checkpoint_git(&app, &token, "docs").await;
    let replay = success(&app, &token, "POST", CREATE, Some(request.clone())).await;
    for key in ["operation_id", "request_digest", "publication_sequence", "result"] {
        assert_eq!(replay[key], receipt[key], "{key}");
    }
    let mut changed = request.clone();
    changed["source"]["tree_id"] = current["head_tree_id"].clone();
    let (status, error) = send(&app, Some(&token), "POST", CREATE, Some(changed)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
    let mut stale = request.clone();
    stale["operation_id"] = new_id("op").unwrap().into();
    let (status, error) = send(&app, Some(&token), "POST", CREATE, Some(stale)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "DIRECTORY_BRANCH_CONFLICT");
    let revoked = revoked_session(&app).await;
    for session in [None, Some(revoked.as_str())] {
        assert_eq!(
            send(&app, session, "POST", CREATE, Some(request.clone())).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    drop(app);
    let system = root.path().join("system");
    let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
    let auth = backend.open_auth(&system).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::new(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let replay = success(&app, &token, "POST", CREATE, Some(request)).await;
    for key in ["operation_id", "request_digest", "publication_sequence", "result"] {
        assert_eq!(replay[key], receipt[key], "{key}");
    }
    assert_eq!(manager_scope(&app, &token, repo, branch).await["entries"], old_scope["entries"]);
}

#[tokio::test]
async fn historical_directory_branch_rejects_changed_cas_and_nonhistorical_or_foreign_sources() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/a.txt", b"initial").await;
    manager_upload(&app, &token, "other/b.txt", b"foreign").await;
    manager_checkpoint_git(&app, &token, "other").await;
    let foreign = manager_directory_branch_source(&app, &token, "other").await;
    manager_checkpoint_git(&app, &token, "docs").await;
    let old = manager_directory_branch_source(&app, &token, "docs").await;
    manager_upload(&app, &token, "docs/a.txt", b"newer").await;
    let working = manager_initialize_git(&app, &token, "docs").await["working_tree_id"].clone();
    manager_checkpoint_git(&app, &token, "docs").await;
    let current = manager_directory_branch_source(&app, &token, "docs").await;
    let before = manager_disk_files(root.path());
    let events = success(&app, &token, "GET", "/api/events", None).await;
    let mut rejected = Vec::new();
    for (field, value) in [
        ("repository_id", new_id("repo").unwrap()),
        ("directory_id", new_id("entry").unwrap()),
        ("directory_branch_id", new_id("dbr").unwrap()),
        ("working_tree_id", new_id("dtree").unwrap()),
        ("head_tree_id", new_id("dtree").unwrap()),
        ("index_generation", u64::MAX.to_string()),
        ("namespace_head", new_id("commit").unwrap()),
    ] {
        let mut request = body(&current, &old["head_tree_id"], field);
        request["source"]["current"][field] = value.into();
        rejected.push(request);
    }
    for tree in [working, foreign["head_tree_id"].clone(), json!(new_id("dtree").unwrap())] {
        rejected.push(body(&current, &tree, "invalid historical source"));
    }
    for request in rejected {
        let operation = request["operation_id"].as_str().unwrap().to_owned();
        let (status, error) = send(&app, Some(&token), "POST", CREATE, Some(request)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["error"]["code"], "DIRECTORY_BRANCH_CONFLICT");
        assert_eq!(
            send(&app, Some(&token), "GET", &format!("/api/operations/{operation}"), None)
                .await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(manager_disk_files(root.path()), before);
        assert_eq!(success(&app, &token, "GET", "/api/events", None).await, events);
    }
}

#[tokio::test]
async fn historical_directory_branch_keeps_unstaged_and_staged_work_fenced() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/a.txt", b"committed").await;
    manager_checkpoint_git(&app, &token, "docs").await;
    let old = manager_directory_branch_source(&app, &token, "docs").await;
    manager_upload(&app, &token, "docs/a.txt", b"pending work").await;
    for staged in [false, true] {
        if staged {
            let current = manager_initialize_git(&app, &token, "docs").await;
            let repo = current["repository_id"].as_str().unwrap();
            success(
                &app, &token, "POST", &format!("/api/repositories/{repo}/directory-git/stage"),
                Some(json!({
                    "directory_id": current["directory_id"],
                    "directory_branch_id": current["directory_branch_id"],
                    "expected_tree_id": current["working_tree_id"],
                    "expected_index_generation": current["index_generation"],
                    "operation_id": new_id("op").unwrap()
                })),
            ).await;
        }
        let current = manager_directory_branch_source(&app, &token, "docs").await;
        let before = manager_disk_files(root.path());
        let request = body(&current, &old["head_tree_id"], "preserve pending work");
        let (status, error) = send(&app, Some(&token), "POST", CREATE, Some(request)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["error"]["code"], "DIRECTORY_BRANCH_CONFLICT");
        assert_eq!(manager_disk_files(root.path()), before);
        assert_eq!(manager_directory_branch_source(&app, &token, "docs").await, current);
    }
}
