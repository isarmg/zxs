//! One real authenticated route flow for the fresh format. This target is kept
//! in the existing HTTP integration binary, not a second implementation fixture.
use super::*;

async fn rejected(
    fixture: &Fixture,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = fixture.app.clone().oneshot(
        request.body(Body::from(json_codec::to_vec(&body).unwrap())).unwrap(),
    ).await.unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, json_codec::from_slice(&bytes).unwrap())
}

async fn write_draft(
    fixture: &Fixture,
    file: &str,
    request: &Value,
    bytes: &[u8],
) -> http_routing::response::Response {
    fixture.app.clone().oneshot(
        Request::builder()
            .method("PUT")
            .uri(format!("/api/file-manager/files/{file}/draft"))
            .header("authorization", format!("Bearer {}", fixture.token))
            .header("x-zxs-draft-command", json_codec::to_string(request).unwrap())
            .header("content-length", bytes.len().to_string())
            .body(Body::from(bytes.to_vec()))
            .unwrap(),
    ).await.unwrap()
}

async fn draft_bytes(
    fixture: &Fixture,
    route: &str,
    range: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder()
        .uri(route)
        .header("authorization", format!("Bearer {}", fixture.token));
    if let Some(range) = range {
        request = request.header("range", range);
    }
    let response = fixture.app.clone().oneshot(request.body(Body::empty()).unwrap())
        .await.unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, bytes.to_vec())
}

#[tokio::test]
async fn fresh_authenticated_file_draft_survives_cold_exact_retries() {
    let fixture = Fixture::new(&["primary"]).await;
    let original_body = b"original HTTP body";
    fixture.save("D01", "original.txt", original_body).await;
    let entry = fixture.entry("D01", "original.txt").await;
    let file = entry["file_id"].as_str().unwrap().to_owned();
    let history = fixture.history(&file).await;
    let original_node = current_node(&history).clone();
    let repository = history["identity"]["context"]["repository_id"]
        .as_str().unwrap().to_owned();
    let target = json!({
        "repository": repository,
        "path": "original.txt",
        "entry": entry["entry_id"],
        "file": file,
    });
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("repository_id", &repository)
        .append_pair("path", "original.txt")
        .append_pair("entry_id", entry["entry_id"].as_str().unwrap())
        .finish();
    let working_route = format!("/api/file-manager/files/{file}/working");
    let read_working = format!("{working_route}?{query}");
    let read_draft = format!("/api/file-manager/files/{file}/draft?{query}");
    let initial = fixture.json("GET", &read_working, None).await;
    assert_eq!(initial["working"]["state"], "automatic");
    let mode = json!({
        "operation_id": new_id("op").unwrap(),
        "command": {
            "action": "set_mode", "target": target,
            "expected": initial["working"]["revision"], "mode": "manual",
        },
    });
    let (status, _) = rejected(&fixture, None, "POST", &working_route, mode.clone()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let mode_receipt = fixture.json("POST", &working_route, Some(mode.clone())).await;
    let clean = fixture.json("GET", &read_working, None).await;
    assert_eq!(clean["working"]["state"], "manual_clean");
    assert_eq!(clean["base"], initial["base"]);
    let drafts = fixture.root.path().join("primary/common-data/original.txt/drafts");
    assert!(!drafts.exists(), "a mode change must not pre-create draft storage");

    let edited = b"independent manual draft HTTP bytes";
    let write = json!({
        "operation_id": new_id("op").unwrap(),
        "command": {
            "action": "write", "target": target,
            "expected": clean["working"]["revision"], "base": clean["base"],
            "content_hash": blake3::hash(edited).to_hex().as_str(),
            "logical_size": edited.len().to_string(),
        },
    });
    let failed = write_draft(&fixture, &file, &write, &vec![b'x'; edited.len()]).await;
    assert_eq!(failed.status(), StatusCode::BAD_REQUEST);
    assert_eq!(fixture.json("GET", &read_working, None).await, clean);
    assert!(!drafts.exists(), "rejected upload bytes must not create draft storage");
    let written = json_response(write_draft(&fixture, &file, &write, edited).await).await;
    let dirty = fixture.json("GET", &read_working, None).await;
    assert_eq!(dirty["working"]["state"], "manual_dirty");
    let draft_payload = dirty["working"]["payload"]["payload_id"].as_str().unwrap();
    assert_eq!(std::fs::read_dir(&drafts).unwrap().count(), 1);
    assert_eq!(std::fs::read(drafts.join(draft_payload)).unwrap(), edited);
    assert_eq!(fixture.history(&file).await["nodes"], history["nodes"]);
    assert_eq!(fixture.entry("D01", "original.txt").await["version_id"], entry["version_id"]);

    let fixture = fixture.reopen().await;
    assert_eq!(fixture.json("GET", &read_working, None).await, dirty);
    assert_eq!(draft_bytes(&fixture, &read_draft, None).await,
        (StatusCode::OK, edited.to_vec()));
    assert_eq!(draft_bytes(&fixture, &read_draft, Some("bytes=2-7")).await,
        (StatusCode::PARTIAL_CONTENT, edited[2..8].to_vec()));
    assert_eq!(fixture.body(entry["version_id"].as_str().unwrap()).await, original_body);
    let commit = json!({
        "operation_id": new_id("op").unwrap(),
        "command": {
            "action": "commit", "target": target,
            "expected": dirty["working"]["revision"], "base": dirty["base"],
            "message": "explicit HTTP manual commit",
        },
    });
    let committed = fixture.json("POST", &working_route, Some(commit.clone())).await;
    let selected = fixture.entry("D01", "original.txt").await;
    assert_eq!(selected["file_id"], entry["file_id"]);
    assert_ne!(selected["version_id"], entry["version_id"]);
    let committed_history = fixture.history(&file).await;
    let node = current_node(&committed_history);
    assert_eq!(node["version_id"], selected["version_id"]);
    assert_eq!(node["parents"], json!({"state": "complete"}));
    let parents = committed_history["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|edge| edge["node_id"] == node["node_id"])
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(parents, vec![json!({
        "node_id": node["node_id"],
        "parent_node_id": original_node["node_id"],
        "endpoint": "in_window",
    })]);
    let retained_node = committed_history["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["node_id"] == original_node["node_id"])
        .unwrap();
    assert_eq!(retained_node["current"], false);
    // branch_id reports a branch currently headed at this version, not its
    // immutable author. The existing selected FILE branch now heads the new
    // version; all other retained node/material fields remain unchanged.
    assert_eq!(original_node["branch_id"], initial["base"]["branch_id"]);
    assert_eq!(node["branch_id"], original_node["branch_id"]);
    assert!(retained_node["branch_id"].is_null());
    for field in [
        "node_id", "version_id", "kind", "path", "previous_path", "note", "size",
        "content_hash", "content_state", "download_name", "storage_name", "parents",
        "sequence", "fields",
    ] {
        assert_eq!(retained_node[field], original_node[field], "retained {field}");
    }
    assert_eq!(fixture.body(entry["version_id"].as_str().unwrap()).await, original_body);
    assert_eq!(fixture.body(selected["version_id"].as_str().unwrap()).await, edited);
    let committed_working = fixture.json("GET", &read_working, None).await;
    assert_eq!(committed_working["working"]["state"], "manual_clean");

    let fixture = fixture.reopen().await;
    assert_eq!(fixture.json("POST", &working_route, Some(commit)).await, committed);
    // Replay must reach the permanent receipt before the old revision/base gates.
    assert_eq!(json_response(write_draft(&fixture, &file, &write, edited).await).await, written);
    assert_eq!(std::fs::read_dir(&drafts).unwrap().count(), 1);
    assert_eq!(std::fs::read(drafts.join(draft_payload)).unwrap(), edited);
    assert_eq!(fixture.json("POST", &working_route, Some(mode)).await, mode_receipt);
    assert_eq!(fixture.json("GET", &read_working, None).await, committed_working);
    assert_eq!(fixture.history(&file).await, committed_history);
    assert_eq!(fixture.body(entry["version_id"].as_str().unwrap()).await, original_body);

    let final_entry = fixture.entry("D01", "original.txt").await;
    assert_eq!(final_entry["file_id"], selected["file_id"]);
    assert_eq!(final_entry["version_id"], selected["version_id"]);
}

/// The same ordinary FILE save that makes a real owner, commit and Version
/// must not grant access to a directory snapshot by any old read or write URL.
#[tokio::test]
async fn directory_snapshot_and_worktree_urls_do_not_publish_or_reinterpret_an_ordinary_zip() {
    let fixture = Fixture::new(&["primary"]).await;
    let bytes = b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";
    let saved = fixture.save("D01", "records/project.zxs", bytes).await;
    let listing = "/api/file-manager/disks/D01/entries?path=records";
    let before = fixture.json("GET", listing, None).await;
    let entries = before["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["name"], "project.zxs");
    let file = entry["file_id"].as_str().unwrap();
    let version = entry["version_id"].as_str().unwrap();
    let history = fixture.history(file).await;
    let repo = before["context"]["repository_id"].as_str().unwrap();
    let branch = before["context"]["branch_id"].as_str().unwrap();
    let operation = saved["operation_id"].as_str().unwrap();
    let worktree = new_id("worktree").unwrap();
    let base = format!("/api/repositories/{repo}");
    let target = format!("/api/directory-worktrees/{worktree}");
    let routes = [
        ("POST", "/api/file-manager/disks/D01/snapshot-root".to_owned()),
        ("GET", format!("{base}/snapshot-root-context")),
        ("GET", format!("{base}/snapshot-head")),
        ("GET", format!("{base}/snapshot-history")),
        ("POST", "/api/directory-snapshots".to_owned()),
        ("POST", "/api/directory-snapshots/commits".to_owned()),
        ("GET", format!("/api/directory-snapshots/commits/{operation}")),
        ("GET", format!("{base}/directory-worktrees/creations/{operation}")),
        ("GET", format!("{base}/directory-worktrees/commits/{operation}")),
        ("GET", format!("{base}/directory-worktrees")),
        ("POST", "/api/directory-worktrees".to_owned()),
        ("GET", format!("{target}?publication={operation}")),
        ("GET", format!("{target}/content?publication={operation}&path=project.zxs")),
        ("PUT", format!("{target}/content")),
        ("POST", format!("{target}/commits")),
        ("GET", format!("/api/file-manager/directory-snapshot-source?branch_id={branch}")),
    ];
    for (method, route) in &routes {
        for token in [None, Some(fixture.token.as_str())] {
            let (status, reply) = rejected(&fixture, token, method, route, json!({})).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {route}: {reply}");
            assert_eq!(reply["error"]["code"], "NOT_FOUND");
            // A protected handler denying a lookup is not a removed route.
            assert_eq!(reply["error"]["message"], "route not found");
        }
    }
    assert_eq!(fixture.json("GET", listing, None).await, before);
    assert_eq!(fixture.history(file).await, history);
    assert_eq!(fixture.body(version).await, bytes);
    let fixture = fixture.reopen().await;
    assert_eq!(fixture.entry("D01", "records/project.zxs").await, *entry);
    assert_eq!(fixture.body(version).await, bytes);
}
