//! Operation history keeps distinct logical paths even when aliases share identity.
use super::*;
use std::collections::BTreeSet;

async fn prepare_aliases(app: &Router, token: &str) -> (Value, Value) {
    prepare_alias_composition(app, token, false).await
}

async fn prepare_alias_composition(
    app: &Router,
    token: &str,
    retain_occurrence: bool,
) -> (Value, Value) {
    manager_upload(app, token, "b/first.txt", b"shared body").await;
    manager_upload(app, token, "b/stay.txt", b"unrelated body").await;
    let listing = alias_listing(app, token).await;
    let repository = listing["context"]["repository_id"].as_str().unwrap();
    // Each branch belongs to the same initialized owner. A branch move can
    // therefore retain the same file identity at two logical locations.
    let first = manager_prepare_directory_branch(app, token, "b", "first").await;
    let target = manager_prepare_directory_branch(app, token, "b", "target").await;
    if retain_occurrence {
        let retained = manager_prepare_directory_branch(app, token, "b", "retained").await;
        introduce_alias(app, token, repository, &retained, &target, "b/retained.txt").await;
    }
    introduce_alias(app, token, repository, &first, &target, "b/second.txt").await;
    // The source view is now empty. Select it so both target aliases are
    // introduced by the next checkout, without constructing synthetic state.
    manager_checkpoint_and_select(app, token, repository, &first).await;
    let selected = manager_checkpoint_and_select(app, token, repository, &target).await;
    let listing = alias_listing(app, token).await;
    let first = alias_entry(&listing, "first.txt");
    let second = alias_entry(&listing, "second.txt");
    assert_same_file(first, second);
    if retain_occurrence {
        assert_same_file(first, alias_entry(&listing, "retained.txt"));
    }
    (first.clone(), selected)
}

async fn introduce_alias(
    app: &Router,
    token: &str,
    repository: &str,
    source: &str,
    target: &str,
    destination: &str,
) {
    let source = manager_scope(app, token, repository, source).await;
    let target = manager_scope(app, token, repository, target).await;
    assert!(source["directory_id"].is_string());
    assert_eq!(source["directory_id"], target["directory_id"]);
    success(
        app,
        token,
        "POST",
        "/api/file-manager/disks/P01/namespace-batch",
        Some(json!({
            "kind":"move", "operation_id":new_id("op").unwrap(), "items":[{
                "source":manager_batch_source(&source, "b/first.txt"),
                "target":manager_batch_target(&target, destination)
            }]
        })),
    )
    .await;
}

async fn alias_listing(app: &Router, token: &str) -> Value {
    success(
        app,
        token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=b",
        None,
    )
    .await
}

fn alias_entry<'a>(listing: &'a Value, name: &str) -> &'a Value {
    listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap()
}

fn assert_same_file(first: &Value, second: &Value) {
    assert_eq!(first["entry_id"], second["entry_id"]);
    assert_eq!(first["file_id"], second["file_id"]);
    assert_eq!(first["version_id"], second["version_id"]);
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
                && (event["file_id"] == file["file_id"] || event["action"] == "delete")
        })
        .cloned()
        .collect()
}

fn assert_paths(events: &[Value], action: &str, expected: &[&str]) {
    let rows = events
        .iter()
        .filter(|event| event["action"] == action)
        .collect::<Vec<_>>();
    for row in &rows {
        assert_eq!(row["principal"], "admin");
    }
    let paths = rows
        .iter()
        .map(|event| event["path"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(paths, expected.iter().copied().collect(), "{events:?}");
    if action == "delete" {
        for row in &rows {
            assert_eq!(
                row["file_id"], "",
                "deletion facts do not retain content identity"
            );
            assert!(row["version_id"].is_null());
            assert!(
                row["timestamp"]
                    .as_str()
                    .is_some_and(|time| time.ends_with('Z'))
            );
        }
    }
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
        "one row is required per removal, including repeated paths"
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
        "directory_git_checkout",
        &["b/first.txt", "b/second.txt"],
    );
}

#[tokio::test]
async fn workspace_history_keeps_both_deleted_alias_paths_with_retained_occurrence() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let (file, _) = prepare_alias_composition(&app, &token, true).await;
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
    let aliases = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["file_id"] == file["file_id"])
        .collect::<Vec<_>>();
    assert_eq!(aliases.len(), 1);
    assert_eq!(aliases[0]["name"], "retained.txt");
    assert_same_file(&file, aliases[0]);
    let events = operation_events(&app, &token, &deleted, &file).await;
    assert_paths(&events, "delete", &["b/first.txt", "b/second.txt"]);
}

#[tokio::test]
async fn workspace_alias_rename_is_not_reported_as_deletion() {
    for destination in ["b/renamed.txt", "b/FIRST.txt"] {
        let root = tempfile::tempdir().unwrap();
        let app = application(root.path());
        let token = login(&app, "admin", "integration-test-password").await;
        let (file, _) = prepare_aliases(&app, &token).await;
        let renamed = workspace_namespace(
            &app,
            &token,
            &[json!({"kind":"rename","path":"b/first.txt","destination":destination})],
        )
        .await;
        let events = operation_events(&app, &token, &renamed, &file).await;
        assert_paths(&events, "delete", &[]);
        assert_paths(&events, "file.path.rename", &[destination]);
        assert!(
            events
                .iter()
                .any(|event| event["action"] == "file.path.rename"
                    && event["previous_path"] == "b/first.txt")
        );
    }
}

#[tokio::test]
async fn workspace_history_records_each_single_alias_deletion() {
    for path in ["b/first.txt", "b/second.txt"] {
        let root = tempfile::tempdir().unwrap();
        let app = application(root.path());
        let token = login(&app, "admin", "integration-test-password").await;
        let (file, _) = prepare_aliases(&app, &token).await;
        let deleted =
            workspace_namespace(&app, &token, &[json!({"kind":"delete","path":path})]).await;
        let events = operation_events(&app, &token, &deleted, &file).await;
        assert_paths(&events, "delete", &[path]);
    }
}

#[tokio::test]
async fn workspace_history_keeps_deletion_when_survivor_reuses_path() {
    for (removed, survivor) in [
        ("b/first.txt", "b/second.txt"),
        ("b/second.txt", "b/first.txt"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let app = application(root.path());
        let token = login(&app, "admin", "integration-test-password").await;
        let (file, _) = prepare_aliases(&app, &token).await;
        let operation = workspace_namespace(
            &app,
            &token,
            &[
                json!({"kind":"delete","path":removed}),
                json!({"kind":"rename","path":survivor,"destination":removed}),
            ],
        )
        .await;
        let events = operation_events(&app, &token, &operation, &file).await;
        assert_paths(&events, "delete", &[removed]);
        assert_paths(&events, "file.path.rename", &[removed]);
    }
}

#[tokio::test]
async fn workspace_history_records_deleted_path_after_rename() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let (file, _) = prepare_aliases(&app, &token).await;
    let operation = workspace_namespace(
        &app,
        &token,
        &[
            json!({"kind":"rename","path":"b/first.txt","destination":"b/renamed.txt"}),
            json!({"kind":"delete","path":"b/renamed.txt"}),
        ],
    )
    .await;
    let events = operation_events(&app, &token, &operation, &file).await;
    assert_paths(&events, "delete", &["b/renamed.txt"]);
}

#[tokio::test]
async fn workspace_history_keeps_repeated_removal_of_same_path() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    // A retained occurrence keeps the stable file live while two ordered
    // removals of its other aliases preserve their separate audit ordinals.
    let (file, _) = prepare_alias_composition(&app, &token, true).await;
    let operation = workspace_namespace(
        &app,
        &token,
        &[
            json!({"kind":"delete","path":"b/first.txt"}),
            json!({"kind":"rename","path":"b/second.txt","destination":"b/first.txt"}),
            json!({"kind":"delete","path":"b/first.txt"}),
        ],
    )
    .await;
    let events = operation_events(&app, &token, &operation, &file).await;
    assert_paths(&events, "delete", &["b/first.txt", "b/first.txt"]);
    let ids = events
        .iter()
        .filter(|event| event["action"] == "delete")
        .map(|event| event["event_id"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().any(|id| id.ends_with(":delete:0")));
    assert!(ids.iter().any(|id| id.ends_with(":delete:1")));
    let reread = operation_events(&app, &token, &operation, &file).await;
    assert_eq!(
        events, reread,
        "activity identities must remain stable across reads"
    );
}

fn assert_deleted_alias(deleted: &Value, file: &Value, path: &str) -> Value {
    assert_eq!(deleted["operation_kind"], "file.delete.batch");
    let nodes = deleted["result"]["deletion_nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1, "aliases share one whole-file deletion");
    let node = &nodes[0];
    assert_eq!(node["file_id"], file["file_id"]);
    assert_eq!(node["path"], path);
    assert_eq!(node["author"], "admin");
    assert_eq!(node["operation_id"], deleted["operation_id"]);
    assert_eq!(node["sequence"], deleted["publication_sequence"]);
    assert!(node.get("version_id").is_none());
    node.clone()
}

fn alias_move_request(listing: &Value, source: &str, destination: &str) -> Value {
    let repository = &listing["context"]["repository_id"];
    let entry = alias_entry(listing, source.rsplit('/').next().unwrap());
    json!({
        "operation_id":new_id("op").unwrap(),
        "movement":{
            "source_repository_id":repository,
            "source_entry_id":entry["entry_id"],
            "source_path":source,
            "target_slot_id":"P01",
            "target_repository_id":repository,
            "target_path":destination,
            "replace":false,
            "target_entry_id":null
        }
    })
}

async fn assert_reserved_alias_path(app: &Router, token: &str, path: &str) {
    let before = alias_listing(app, token).await;
    let audit = success(app, token, "GET", "/api/file-manager/history", None).await;
    let (status, failure) = send(
        app,
        Some(token),
        "POST",
        "/api/file-manager/disks/P01/object-move",
        Some(alias_move_request(&before, "b/stay.txt", path)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{failure}");
    assert_eq!(failure["error"]["code"], "NAME_COLLISION");
    assert_eq!(alias_listing(app, token).await, before);
    assert_eq!(
        success(app, token, "GET", "/api/file-manager/history", None).await,
        audit
    );
}

async fn lifecycle_events(
    app: &Router,
    token: &str,
    operation: &Value,
    node: &Value,
) -> Vec<Value> {
    let events = operation_events(app, token, operation, node).await;
    let action = operation["operation_kind"].as_str().unwrap();
    assert_paths(&events, action, &[node["path"].as_str().unwrap()]);
    let row = events.iter().find(|event| event["action"] == action).unwrap();
    assert_eq!(row["file_id"], node["file_id"]);
    assert_eq!(row["node_id"], node["node_id"]);
    assert!(row["version_id"].is_null());
    assert!(row["branch_id"].is_null());
    assert!(row["timestamp"].as_str().unwrap().ends_with('Z'));
    events
}

async fn recreate_beside_deleted_aliases(app: &Router, token: &str, file: &Value) {
    // Re-creation uses a free path and an independent identity while the
    // deleted file retains both reserved alias locations and its bytes.
    manager_upload(app, token, "b/recreated.txt", b"shared body").await;
    let listing = alias_listing(app, token).await;
    let recreated = alias_entry(&listing, "recreated.txt");
    assert_ne!(recreated["entry_id"], file["entry_id"]);
    assert_ne!(recreated["file_id"], file["file_id"]);
    for entry in [file, recreated] {
        assert_eq!(
            version_bytes(app, token, entry["version_id"].as_str().unwrap()).await,
            b"shared body"
        );
    }
}

async fn restore_alias_batch(app: &Router, token: &str, deleted: &Value, file: &Value) {
    let original = deleted["operation_id"].as_str().unwrap();
    let preview = success(
        app,
        token,
        "GET",
        &format!("/api/file-deletion-batches/{original}"),
        None,
    )
    .await;
    assert_eq!(preview["deletion_nodes"], deleted["result"]["deletion_nodes"]);
    let restored = success(
        app,
        token,
        "POST",
        "/api/file-deletion-batches/undo",
        Some(json!({
            "deletion_operation_id":deleted["operation_id"],
            "operation_id":new_id("op").unwrap()
        })),
    )
    .await;
    let listing = alias_listing(app, token).await;
    for name in ["first.txt", "second.txt"] {
        assert_same_file(file, alias_entry(&listing, name));
    }
    for node in deleted["result"]["deletion_nodes"].as_array().unwrap() {
        lifecycle_events(app, token, &restored, node).await;
    }
}

#[tokio::test]
async fn namespace_batch_alias_deletion_retains_one_lifecycle_and_exact_replay() {
    for path in ["b/first.txt", "b/second.txt"] {
        let root = tempfile::tempdir().unwrap();
        let app = application(root.path());
        let token = login(&app, "admin", "integration-test-password").await;
        let (file, _) = prepare_aliases(&app, &token).await;
        let request = manager_prepare_namespace_request(&app, &token, "delete", path, None).await;
        let route = "/api/file-manager/disks/P01/namespace-batch";
        let deleted = success(&app, &token, "POST", route, Some(request.clone())).await;
        let node = assert_deleted_alias(&deleted, &file, path);
        let events = lifecycle_events(&app, &token, &deleted, &node).await;
        let listing = alias_listing(&app, &token).await;
        assert!(
            listing["entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|entry| entry["file_id"] != file["file_id"])
        );
        for reserved in ["b/first.txt", "b/second.txt"] {
            assert_reserved_alias_path(&app, &token, reserved).await;
        }
        recreate_beside_deleted_aliases(&app, &token, &file).await;
        restore_alias_batch(&app, &token, &deleted, &file).await;
        assert_eq!(
            success(&app, &token, "POST", route, Some(request.clone())).await,
            deleted
        );
        let repeated = manager_namespace(&app, &token, "delete", path, None).await;
        let second = assert_deleted_alias(&repeated, &file, path);
        lifecycle_events(&app, &token, &repeated, &second).await;
        assert_eq!(lifecycle_events(&app, &token, &deleted, &node).await, events);
        assert_ne!(second["node_id"], node["node_id"]);
        assert_ne!(second["operation_id"], node["operation_id"]);
        assert_eq!(
            success(&app, &token, "POST", route, Some(request)).await,
            deleted
        );
        let deletion_route = format!("/api/files/{}/deletion", file["file_id"].as_str().unwrap());
        assert_eq!(
            success(&app, &token, "GET", &deletion_route, None).await,
            second
        );
    }
}

#[tokio::test]
async fn single_file_lifecycle_history_keeps_original_path_after_object_move() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let (file, _) = prepare_aliases(&app, &token).await;
    let listing = alias_listing(&app, &token).await;
    let route = format!("/api/files/{}/deletion", file["file_id"].as_str().unwrap());
    let deleted = success(
        &app,
        &token,
        "POST",
        &route,
        Some(json!({
            "repository_id":listing["context"]["repository_id"],
            "path":"b/first.txt", "operation_id":new_id("op").unwrap()
        })),
    )
    .await;
    let node = &deleted["result"]["deletion_node"];
    let deletion_events = lifecycle_events(&app, &token, &deleted, node).await;
    let restored = success(
        &app,
        &token,
        "POST",
        &format!("{route}/undo"),
        Some(json!({
            "deletion_node_id":node["node_id"], "operation_id":new_id("op").unwrap()
        })),
    )
    .await;
    let undo_events = lifecycle_events(&app, &token, &restored, node).await;
    assert_ne!(deletion_events[0]["event_id"], undo_events[0]["event_id"]);
    let listing = alias_listing(&app, &token).await;
    let moved = success(
        &app,
        &token,
        "POST",
        "/api/file-manager/disks/P01/object-move",
        Some(alias_move_request(&listing, "b/first.txt", "b/renamed.txt")),
    )
    .await;
    let events = operation_events(&app, &token, &moved, &file).await;
    assert_paths(&events, "file.path.rename", &["b/renamed.txt"]);
    assert_paths(&events, "delete", &[]);
    assert_eq!(
        lifecycle_events(&app, &token, &deleted, node).await,
        deletion_events
    );
    assert_eq!(
        lifecycle_events(&app, &token, &restored, node).await,
        undo_events
    );
}

#[tokio::test]
async fn namespace_batch_history_keeps_each_deleted_file_and_its_restore() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let (file, _) = prepare_aliases(&app, &token).await;
    let mut request = manager_prepare_namespace_request(
        &app,
        &token,
        "delete",
        "b/first.txt",
        None,
    )
    .await;
    let sibling = manager_prepare_namespace_request(
        &app,
        &token,
        "delete",
        "b/stay.txt",
        None,
    )
    .await;
    request["items"]
        .as_array_mut()
        .unwrap()
        .push(sibling["items"][0].clone());
    let deleted = success(
        &app,
        &token,
        "POST",
        "/api/file-manager/disks/P01/namespace-batch",
        Some(request),
    )
    .await;
    let nodes = deleted["result"]["deletion_nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    assert_ne!(nodes[0]["file_id"], nodes[1]["file_id"]);
    let mut ids = BTreeSet::new();
    for node in nodes {
        let events = lifecycle_events(&app, &token, &deleted, node).await;
        assert!(ids.insert(events[0]["event_id"].as_str().unwrap().to_owned()));
    }
    restore_alias_batch(&app, &token, &deleted, &file).await;
    for node in nodes {
        let events = lifecycle_events(&app, &token, &deleted, node).await;
        assert!(ids.contains(events[0]["event_id"].as_str().unwrap()));
    }
}

#[tokio::test]
async fn activity_typed_route_retains_query_authentication_and_wire_order() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "activity.txt", b"activity body").await;
    let route = "/api/file-manager/history";
    let current = success(&app, &token, "GET", route, None).await;
    let with_query = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/history?unused=value",
        None,
    )
    .await;
    assert_eq!(current, with_query);
    assert_eq!(current["limit"], 200);
    let events = current["events"].as_array().unwrap();
    assert!(!events.is_empty());
    let keys = events
        .iter()
        .map(|event| {
            assert!(event["timestamp"].as_str().unwrap().ends_with('Z'));
            assert!(event["slot_id"].is_string());
            assert!(event["path"].is_string());
            assert!(event["action"].is_string());
            assert!(event["principal"].is_string());
            assert!(event["file_id"].is_string());
            assert!(event["message"].is_string());
            (
                std::cmp::Reverse(event["sequence"].as_str().unwrap().parse::<u64>().unwrap()),
                event["event_id"].as_str().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        send(&app, None, "GET", route, None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn move_routes_keep_invalid_request_responses_for_malformed_source_slots() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let before = success(&app, &token, "GET", "/api/file-manager/history", None).await;
    for endpoint in ["file-relocation", "object-move"] {
        for slot in ["0disk".to_owned(), "_disk".to_owned(), "A".repeat(65)] {
            let route = format!("/api/file-manager/disks/{slot}/{endpoint}");
            let (status, failure) =
                send(&app, Some(&token), "POST", &route, Some(json!({}))).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(failure["error"]["code"], "INVALID_REQUEST");
        }
    }
    assert_eq!(
        success(&app, &token, "GET", "/api/file-manager/history", None).await,
        before
    );
}
