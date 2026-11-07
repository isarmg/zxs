//! The existing authenticated batch route reaches real separation publication.
use super::*;

fn occurrence(scope: &Value, path: &str) -> Value {
    scope["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == path)
        .unwrap()
        .clone()
}

#[tokio::test]
async fn separate_http_preserves_roles_rejects_stale_views_and_recovers_once() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "a/report.txt", b"frozen secondary body").await;
    manager_upload(&app, &token, "b/keep", b"unrelated").await;
    let directory = manager_initialize_git(&app, &token, "").await;
    let repo = directory["repository_id"].as_str().unwrap();
    let main = directory["directory_branch_id"].as_str().unwrap();
    let transfer = manager_prepare_directory_branch(&app, &token, "", "transfer").await;
    let other = manager_prepare_directory_branch(&app, &token, "", "other").await;
    manager_upload(&app, &token, "a/report.txt", b"latest retained save").await;
    let source = manager_scope(&app, &token, repo, &transfer).await;
    let target = manager_scope(&app, &token, repo, main).await;
    let route = "/api/file-manager/disks/P01/namespace-batch";
    success(
        &app,
        &token,
        "POST",
        route,
        Some(json!({
            "kind":"move", "operation_id":new_id("op").unwrap(), "items":[{
                "source":manager_batch_source(&source,"a/report.txt"),
                "target":manager_batch_target(&target,"b/report.txt")
            }]
        })),
    )
    .await;
    let before = manager_scope(&app, &token, repo, main).await;
    let scope_route = format!("/api/repositories/{repo}/directory-branches");
    for folder in ["a", "b"] {
        let summary = success(
            &app,
            &token,
            "GET",
            &format!("{scope_route}?path={folder}&summary=true&owner_main=true"),
            None,
        )
        .await;
        assert_eq!(summary["directory_branches"].as_array().unwrap().len(), 1);
        let owner = &summary["directory_branches"][0];
        assert_eq!(owner["directory_branch_id"], main);
        assert_eq!(owner["head_tree_id"], before["head_tree_id"]);
        assert_eq!(owner["path"], "");
        assert!(owner.get("entries").is_none());
    }
    for query in [
        "path=a&owner_main=true",
        "summary=true&owner_main=true",
        "path=a&summary=true&owner_main=false",
    ] {
        let (status, _) = send(
            &app,
            Some(&token),
            "GET",
            &format!("{scope_route}?{query}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let original = occurrence(&before, "a/report.txt");
    let secondary = occurrence(&before, "b/report.txt");
    let other_before = manager_scope(&app, &token, repo, &other).await;
    assert_eq!(original["file_id"], secondary["file_id"]);
    assert_ne!(original["version_id"], secondary["version_id"]);
    let body = json!({
        "kind":"separate", "operation_id":new_id("op").unwrap(), "items":[{
            "source":manager_batch_source(&before,"a/report.txt"),
            "target":manager_batch_target(&before,"b/report.txt")
        }]
    });
    let (status, _) = send(&app, None, "POST", route, Some(body.clone())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    for changed in ["stale-tree", "wrong-original", "not-main"] {
        let mut invalid = body.clone();
        invalid["operation_id"] = new_id("op").unwrap().into();
        match changed {
            "stale-tree" => invalid["items"][0]["target"]["expected_tree_id"] = "stale".into(),
            "wrong-original" => invalid["items"][0]["source"]["entry_id"] = "wrong".into(),
            "not-main" => {
                invalid["items"][0]["source"] = manager_batch_source(&other_before, "a/report.txt")
            }
            _ => unreachable!(),
        }
        let (status, value) = send(&app, Some(&token), "POST", route, Some(invalid)).await;
        assert_eq!(
            status,
            if changed == "wrong-original" {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::CONFLICT
            },
            "{changed}: {value}"
        );
        assert_eq!(manager_scope(&app, &token, repo, main).await, before);
    }
    let receipt = success(&app, &token, "POST", route, Some(body.clone())).await;
    let after = manager_scope(&app, &token, repo, main).await;
    assert_eq!(occurrence(&after, "a/report.txt"), original);
    let separated = occurrence(&after, "b/report.txt");
    for identity in ["entry_id", "file_id", "version_id"] {
        assert_ne!(separated[identity], secondary[identity]);
    }
    assert_eq!(
        version_bytes(&app, &token, original["version_id"].as_str().unwrap()).await,
        b"latest retained save"
    );
    assert_eq!(
        version_bytes(&app, &token, separated["version_id"].as_str().unwrap()).await,
        b"frozen secondary body"
    );
    assert_eq!(
        manager_scope(&app, &token, repo, &other).await,
        other_before
    );
    assert_eq!(
        success(&app, &token, "POST", route, Some(body.clone())).await,
        receipt
    );
    assert_eq!(manager_scope(&app, &token, repo, main).await, after);
    let main = main.to_owned();
    let operation = body["operation_id"].as_str().unwrap().to_owned();
    drop(app);
    let mut reopened = reopen_after_router_drop(&root.path().join("system")).await;
    let recovered = &reopened.state().directory_branches[&main];
    let retained = recovered
        .entries
        .iter()
        .find(|entry| entry.path() == "a/report.txt")
        .unwrap();
    let fresh = recovered
        .entries
        .iter()
        .find(|entry| entry.path() == "b/report.txt")
        .unwrap();
    assert_eq!(retained.file_id, original["file_id"].as_str().unwrap());
    assert_eq!(
        retained.version_id,
        original["version_id"].as_str().unwrap()
    );
    assert_eq!(fresh.file_id, separated["file_id"].as_str().unwrap());
    assert_eq!(fresh.version_id, separated["version_id"].as_str().unwrap());
    let history = reopened.file_history(&fresh.file_id).unwrap();
    assert_eq!(history.len(), 1);
    assert!(history[0].source_version_ids.is_empty());
    assert!(
        reopened
            .file_node_parents(&fresh.file_id, &fresh.version_id)
            .unwrap()
            .is_empty()
    );
    let expected = json_codec::from_value::<storage_contract::DirectoryNamespaceBatch>(
        json!({"kind":body["kind"],"items":body["items"]}),
    )
    .unwrap();
    let replay = reopened
        .directory_namespace_batch(repo, &expected, "admin", operation.parse().unwrap())
        .unwrap();
    assert_eq!(replay.result, receipt["result"]);
}

#[tokio::test]
async fn owning_main_summary_follows_nearest_owner_not_current_checkout() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "a/report.txt", b"a").await;
    manager_upload(&app, &token, "b/report.txt", b"b").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let route =
        format!("/api/repositories/{repo}/directory-branches?summary=true&owner_main=true&path=");
    let absent = success(&app, &token, "GET", &format!("{route}a"), None).await;
    assert!(absent["directory_branches"].as_array().unwrap().is_empty());
    let initialized = manager_initialize_git(&app, &token, "").await;
    let main = initialized["directory_branch_id"].as_str().unwrap();
    let topic = manager_prepare_directory_branch(&app, &token, "", "topic").await;
    manager_checkpoint_and_select(&app, &token, repo, &topic).await;
    let main_read = success(&app, &token, "GET", &format!("{route}a"), None).await;
    let main_scope = &main_read["directory_branches"][0];
    assert_eq!(main_read["directory_branches"].as_array().unwrap().len(), 1);
    assert_eq!(main_scope["directory_branch_id"], main);
    assert_eq!(main_scope["current"], false);
    assert_eq!(main_scope["path"], "");
    let child = manager_initialize_git(&app, &token, "b").await;
    let child_read = success(&app, &token, "GET", &format!("{route}b"), None).await;
    assert_eq!(
        child_read["directory_branches"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        child_read["directory_branches"][0]["directory_branch_id"],
        child["directory_branch_id"]
    );
    assert_eq!(child_read["directory_branches"][0]["path"], "b");
    assert_ne!(
        child_read["directory_branches"][0]["directory_branch_id"],
        main
    );
}
