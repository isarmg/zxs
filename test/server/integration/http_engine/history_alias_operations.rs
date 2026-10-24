//! Operation history keeps distinct logical paths even when aliases share identity.
use super::*;
use std::collections::BTreeSet;

async fn select_directory(app: &Router, token: &str, path: &str, name: &str) -> Value {
    success(
        app,
        token,
        "POST",
        "/api/file-manager/disks/P01/directory-select",
        Some(json!({"path":path,"name":name,"operation_id":new_id("op")})),
    )
    .await
}

async fn prepare_aliases(app: &Router, token: &str) -> (Value, Value) {
    manager_upload(app, token, "a/report.txt", b"shared body").await;
    manager_upload(app, token, "b/stay.txt", b"unrelated body").await;
    let listing = success(
        app,
        token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=a",
        None,
    )
    .await;
    let repository = listing["context"]["repository_id"].as_str().unwrap();
    let first = manager_directory_branch(app, token, "a", "first").await;
    let second = manager_directory_branch(app, token, "a", "second").await;
    let target = manager_directory_branch(app, token, "b", "target").await;
    for (source, destination) in [(&first, "b/first.txt"), (&second, "b/second.txt")] {
        let source = manager_scope(app, token, repository, source).await;
        let target = manager_scope(app, token, repository, &target).await;
        success(
            app,
            token,
            "POST",
            "/api/file-manager/disks/P01/namespace-batch",
            Some(json!({"kind":"move","operation_id":new_id("op"),"items":[{
                "source":manager_batch_source(&source,"a/report.txt"),
                "target":manager_batch_target(&target,destination)
            }]})),
        )
        .await;
    }
    // The source views are now empty. Select one so only the two target aliases
    // remain in the live namespace, without constructing synthetic state.
    select_directory(app, token, "a", "first").await;
    let selected = select_directory(app, token, "b", "target").await;
    let listing = success(
        app,
        token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=b",
        None,
    )
    .await;
    let entries = listing["entries"].as_array().unwrap();
    let first = entries
        .iter()
        .find(|entry| entry["name"] == "first.txt")
        .unwrap();
    let second = entries
        .iter()
        .find(|entry| entry["name"] == "second.txt")
        .unwrap();
    assert_eq!(first["entry_id"], second["entry_id"]);
    assert_eq!(first["file_id"], second["file_id"]);
    assert_eq!(first["version_id"], second["version_id"]);
    (first.clone(), selected)
}

async fn workspace_namespace(app: &Router, token: &str, changes: &[Value]) -> Value {
    let workspace = success(
        app,
        token,
        "POST",
        "/api/file-manager/disks/P01/write-sessions",
        Some(json!({"path":"b/first.txt"})),
    )
    .await;
    let workspace = workspace["workspace_id"].as_str().unwrap();
    for change in changes {
        success(
            app,
            token,
            "POST",
            &format!("/api/workspaces/{workspace}/namespace-operations"),
            Some(change.clone()),
        )
        .await;
    }
    let mut commit = success(
        app,
        token,
        "POST",
        &format!("/api/workspaces/{workspace}/seal"),
        Some(json!({})),
    )
    .await;
    commit["request_digest"] = request_digest(
        "POST",
        "/api/workspaces/{workspace}/commit",
        json!({"workspace":workspace}),
        json!({}),
        &commit,
    )
    .unwrap()
    .into();
    success(
        app,
        token,
        "POST",
        &format!("/api/workspaces/{workspace}/commit"),
        Some(commit),
    )
    .await
}

async fn operation_events(
    app: &Router,
    token: &str,
    operation: &Value,
    file: &Value,
) -> Vec<Value> {
    let audit = success(app, token, "GET", "/api/file-manager/history", None).await;
    assert_eq!(audit["limit"], 200);
    let events = audit["events"].as_array().unwrap();
    assert!(events.len() <= 200);
    assert!(events.windows(2).all(|pair| {
        pair[0]["sequence"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= pair[1]["sequence"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
    }));
    events
        .iter()
        .filter(|event| {
            event["sequence"] == operation["publication_sequence"]
                && event["file_id"] == file["file_id"]
        })
        .cloned()
        .collect()
}

fn assert_paths(events: &[Value], action: &str, expected: &[&str]) {
    let rows = events
        .iter()
        .filter(|event| event["action"] == action)
        .collect::<Vec<_>>();
    let paths = rows
        .iter()
        .map(|event| event["path"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(paths, expected.iter().copied().collect(), "{events:?}");
    let ids = rows
        .iter()
        .map(|event| event["event_id"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ids.len(),
        expected.len(),
        "each logical path needs a distinct audit identity"
    );
    assert_eq!(
        rows.len(),
        expected.len(),
        "one operation must not duplicate a path row"
    );
}

#[tokio::test]
async fn selection_history_keeps_two_introduced_alias_paths() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let (file, selected) = prepare_aliases(&app, &token).await;
    let events = operation_events(&app, &token, &selected, &file).await;
    assert_paths(
        &events,
        "directory_selection",
        &["b/first.txt", "b/second.txt"],
    );
}

#[tokio::test]
async fn workspace_history_keeps_both_deleted_alias_paths() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let (file, _) = prepare_aliases(&app, &token).await;
    let deleted = workspace_namespace(
        &app,
        &token,
        &[
            json!({"kind":"delete","path":"b/first.txt"}),
            json!({"kind":"delete","path":"b/second.txt"}),
        ],
    )
    .await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=b",
        None,
    )
    .await;
    assert!(
        listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["file_id"] != file["file_id"])
    );
    let events = operation_events(&app, &token, &deleted, &file).await;
    assert_paths(&events, "delete", &["b/first.txt", "b/second.txt"]);
}

#[tokio::test]
async fn workspace_alias_rename_is_not_reported_as_deletion() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let (file, _) = prepare_aliases(&app, &token).await;
    let renamed = workspace_namespace(
        &app,
        &token,
        &[json!({"kind":"rename","path":"b/first.txt","destination":"b/renamed.txt"})],
    )
    .await;
    let events = operation_events(&app, &token, &renamed, &file).await;
    assert_paths(&events, "delete", &[]);
    assert_paths(&events, "file.path.rename", &["b/renamed.txt"]);
    assert!(
        events
            .iter()
            .any(|event| event["action"] == "file.path.rename"
                && event["previous_path"] == "b/first.txt")
    );
}
