//! Bounded file-history routes at the HTTP boundary: strict parameters, root
//! binding, continuation and the exact page shapes consumed by the Web.
use super::*;

fn endpoint(path: &str, pairs: &[(&str, &str)]) -> String {
    if pairs.is_empty() {
        return path.to_owned();
    }
    let mut query = url_encoding::form::Serializer::new(String::new());
    for (key, value) in pairs {
        query.append_pair(key, value);
    }
    format!("{path}?{}", query.finish())
}

fn keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

async fn fixture() -> (tempfile::TempDir, Router, String, String, String, String) {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "report.txt", b"one").await;
    manager_upload(&app, &token, "report.txt", b"two").await;
    manager_upload(&app, &token, "report.txt", b"three").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let entry = &listing["entries"][0];
    let file = entry["file_id"].as_str().unwrap().to_owned();
    let repo = entry["repository_id"].as_str().unwrap().to_owned();
    let version = entry["version_id"].as_str().unwrap().to_owned();
    (root, app, token, file, repo, version)
}

#[tokio::test]
async fn parent_and_branch_http_previous_cursors_round_trip_across_three_pages() {
    let (_directory, app, token, file, _repo, base_version) = fixture().await;
    let file_route = format!("/api/file-manager/files/{file}");
    let history_route = format!("{file_route}/history");
    let mut inputs = Vec::new();
    for index in 0..6 {
        let created = success(
            &app,
            &token,
            "POST",
            &format!("{file_route}/branches"),
            Some(json!({
                "source_version_id":base_version, "source_node_id":base_version,
                "name":format!("roundtrip-{index}"), "operation_id":new_id("op")
            })),
        )
        .await;
        let branch = created["result"]["branch_id"].as_str().unwrap().to_owned();
        success(
            &app,
            &token,
            "POST",
            &format!("{file_route}/select"),
            Some(json!({
                "branch_id":branch, "operation_id":new_id("op")
            })),
        )
        .await;
        manager_upload(
            &app,
            &token,
            "report.txt",
            format!("body-{index}").as_bytes(),
        )
        .await;
        let page = success(&app, &token, "GET", &history_route, None).await;
        inputs.push(
            json!({"branch_id":branch,"expected_node_id":page["context"]["current_node_id"]}),
        );
    }
    success(
        &app,
        &token,
        "POST",
        &format!("{file_route}/select"),
        Some(json!({
            "branch_id":inputs[0]["branch_id"], "operation_id":new_id("op")
        })),
    )
    .await;
    let merged = success(&app, &token, "POST", &format!("{file_route}/merge"), Some(json!({
        "inputs":inputs, "chosen_node_id":base_version, "surviving_branch_id":inputs[0]["branch_id"],
        "confirmed":true,"operation_id":new_id("op")
    }))).await;
    let node = merged["result"]["node_id"].as_str().unwrap();
    let main = success(&app, &token, "GET", &history_route, None).await;
    let root = main["history_root"].as_str().unwrap();
    for (route, collection) in [
        (format!("{history_route}/nodes/{node}/parents"), "parents"),
        (format!("{history_route}/branches"), "branches"),
    ] {
        let first_url = endpoint(&route, &[("history_root", root), ("limit", "2")]);
        let mut pages = vec![success(&app, &token, "GET", &first_url, None).await];
        assert_eq!(
            pages[0],
            success(&app, &token, "GET", &first_url, None).await
        );
        while let Some(cursor) = pages.last().unwrap()["next_cursor"].as_str() {
            let next = success(
                &app,
                &token,
                "GET",
                &endpoint(&route, &[("cursor", cursor), ("limit", "2")]),
                None,
            )
            .await;
            assert_eq!(next["history_root"], root);
            assert!(next[collection].as_array().unwrap().len() <= 2);
            pages.push(next);
            assert!(pages.len() < 10);
        }
        assert!(pages.len() >= 3);
        let mut current = pages.last().unwrap().clone();
        for expected in pages[..pages.len() - 1].iter().rev() {
            current = success(
                &app,
                &token,
                "GET",
                &endpoint(
                    &route,
                    &[
                        ("cursor", current["previous_cursor"].as_str().unwrap()),
                        ("limit", "2"),
                    ],
                ),
                None,
            )
            .await;
            assert_eq!(&current, expected);
        }
        assert!(current["previous_cursor"].is_null());
    }
}

#[tokio::test]
async fn main_page_shape_strictness_and_cursor_navigation() {
    let (_root, app, token, file, repo, version) = fixture().await;
    let base = format!("/api/file-manager/files/{file}/history");
    let page = success(
        &app,
        &token,
        "GET",
        &endpoint(&base, &[("limit", "2")]),
        None,
    )
    .await;
    assert_eq!(
        keys(&page),
        [
            "anchor",
            "budget",
            "context",
            "file_id",
            "history_root",
            "history_tree_id",
            "next_cursor",
            "nodes",
            "previous_cursor",
            "repository_id"
        ]
    );
    assert_eq!(page["file_id"], file);
    assert_eq!(page["repository_id"], repo);
    let root = page["history_root"].as_str().unwrap().to_owned();
    assert_eq!(root.len(), 64);
    assert!(
        root.bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    assert!(page["anchor"].is_null());
    assert_eq!(page["budget"], json!({"limit":2,"parent_edges":2048}));
    assert_eq!(page["context"]["path"], "report.txt");
    assert_eq!(page["context"]["current_node_id"], version);
    assert_eq!(page["context"]["version_id"], version);
    assert_eq!(page["context"]["materialized"], true);
    assert_eq!(page["context"]["inherited"], false);
    let nodes = page["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0]["node_id"], version);
    assert_eq!(nodes[0]["node_id"], nodes[0]["version_id"]);
    assert_eq!(nodes[0]["kind"], "content");
    assert_eq!(nodes[0]["size"], "5");
    // Freshly saved bodies are recorded AVAILABLE, exactly as the previous
    // whole-history wire reported them; LIVE is only the absent-record default.
    assert_eq!(nodes[0]["content_state"], "AVAILABLE");
    assert!(
        nodes[0]["sequence"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > nodes[1]["sequence"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
    );
    assert_eq!(nodes[0]["author"]["inline"], "admin");
    assert!(nodes[0]["message"]["bytes"].is_number());
    assert_eq!(
        keys(&nodes[0]),
        [
            "author",
            "branch_id",
            "content_hash",
            "content_state",
            "created_at",
            "current",
            "download_name",
            "kind",
            "message",
            "node_id",
            "note",
            "operation_id",
            "parents",
            "path",
            "previous_path",
            "sequence",
            "size",
            "storage_name",
            "tags",
            "version_id"
        ]
    );
    // Parents: the second body's one parent is the first body, outside this window.
    assert_eq!(nodes[0]["parents"]["status"], "complete");
    assert!(nodes[0]["parents"]["cursor"].is_null());
    assert_eq!(nodes[0]["parents"]["edges"].as_array().unwrap().len(), 1);
    assert_eq!(nodes[0]["parents"]["edges"][0]["in_window"], true);
    assert_eq!(nodes[1]["parents"]["edges"][0]["in_window"], false);
    assert!(page["previous_cursor"].is_null());
    let next = page["next_cursor"].as_str().unwrap().to_owned();
    assert!(next.len() <= 32768 && next.bytes().all(|byte| byte.is_ascii_hexdigit()));
    // Continuation: the last page, then backward from it to the first.
    let last = success(
        &app,
        &token,
        "GET",
        &endpoint(&base, &[("limit", "2"), ("cursor", &next)]),
        None,
    )
    .await;
    assert_eq!(last["history_root"], root);
    assert_eq!(last["nodes"].as_array().unwrap().len(), 1);
    assert!(last["next_cursor"].is_null());
    let previous = last["previous_cursor"].as_str().unwrap().to_owned();
    let back = success(
        &app,
        &token,
        "GET",
        &endpoint(&base, &[("limit", "2"), ("cursor", &previous)]),
        None,
    )
    .await;
    assert_eq!(back["nodes"], page["nodes"]);
    assert!(back["previous_cursor"].is_null());
    // Strictness: unknown params, duplicate params, bad limits, cursor+anchor,
    // root/cursor disagreement, undecodable cursor, context mismatch.
    for (pairs, code, message) in [
        (
            vec![("unused", "1")],
            "INVALID_REQUEST",
            "unknown field unused",
        ),
        (
            vec![("limit", "0")],
            "INVALID_REQUEST",
            "limit must be between 1 and 200",
        ),
        (
            vec![("limit", "201")],
            "INVALID_REQUEST",
            "limit must be between 1 and 200",
        ),
        (
            vec![
                ("cursor", next.as_str()),
                ("anchor_node_id", version.as_str()),
            ],
            "INVALID_REQUEST",
            "cursor and anchor_node_id are mutually exclusive",
        ),
        (
            vec![
                ("cursor", next.as_str()),
                (
                    "history_root",
                    "00000000000000000000000000000000000000000000000000000000000000ff",
                ),
            ],
            "INVALID_REQUEST",
            "history_root differs from the cursor's history root",
        ),
        (
            vec![("cursor", "zz")],
            "INVALID_CURSOR",
            "invalid history cursor encoding",
        ),
        (
            vec![("cursor", next.as_str()), ("limit", "3")],
            "INVALID_REQUEST",
            "cursor does not match the history query",
        ),
        (
            vec![("history_root", "abc")],
            "INVALID_REQUEST",
            "history_root must be 64 lowercase hexadecimal digits",
        ),
        (
            vec![("repository_id", repo.as_str())],
            "INVALID_REQUEST",
            "repository_id and path must be supplied together",
        ),
    ] {
        let (status, response) =
            send(&app, Some(&token), "GET", &endpoint(&base, &pairs), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{pairs:?}: {response}");
        assert_eq!(response["error"]["code"], code, "{pairs:?}");
        assert_eq!(response["error"]["message"], message, "{pairs:?}");
    }
    let (status, response) = send(
        &app,
        Some(&token),
        "GET",
        &format!("{base}?limit=1&limit=2"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(response["error"]["message"], "duplicate query field");
    let (status, response) = send(
        &app,
        Some(&token),
        "GET",
        &endpoint(&base, &[("repository_id", &repo), ("path", "missing.txt")]),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{response}");
    assert_eq!(response["error"]["code"], "FILE_NOT_FOUND");
    let (status, response) = send(
        &app,
        Some(&token),
        "GET",
        &endpoint(
            &base,
            &[("anchor_node_id", "node_missing"), ("history_root", &root)],
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{response}");
    assert_eq!(response["error"]["code"], "FILE_NODE_NOT_FOUND");
    // Anchor positions the page.
    let anchored = success(
        &app,
        &token,
        "GET",
        &endpoint(
            &base,
            &[
                ("anchor_node_id", nodes[1]["node_id"].as_str().unwrap()),
                ("history_root", &root),
                ("limit", "1"),
            ],
        ),
        None,
    )
    .await;
    assert_eq!(
        anchored["anchor"],
        json!({"node_id": nodes[1]["node_id"], "in_page": true})
    );
    assert_eq!(anchored["nodes"][0]["node_id"], nodes[1]["node_id"]);
    assert!(anchored["previous_cursor"].is_string() && anchored["next_cursor"].is_string());
    // A cursor-bound page resolves the same context as an explicit one, but
    // the two are different queries.
    let explicit = success(
        &app,
        &token,
        "GET",
        &endpoint(
            &base,
            &[
                ("limit", "2"),
                ("repository_id", &repo),
                ("path", "REPORT.TXT"),
            ],
        ),
        None,
    )
    .await;
    assert_eq!(explicit["nodes"], page["nodes"]);
    assert_eq!(explicit["context"], page["context"]);
    assert_ne!(explicit["next_cursor"], page["next_cursor"]);
    let (status, response) = send(
        &app,
        Some(&token),
        "GET",
        &endpoint(
            &base,
            &[
                ("limit", "2"),
                ("cursor", &next),
                ("repository_id", &repo),
                ("path", "report.txt"),
            ],
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    // Unauthenticated continuation.
    let (status, _) = send(
        &app,
        None,
        "GET",
        &endpoint(&base, &[("cursor", &next), ("limit", "2")]),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn root_changes_after_a_write_on_the_same_file_and_logout_rejects_continuations() {
    let (_root, app, token, file, _repo, _version) = fixture().await;
    let base = format!("/api/file-manager/files/{file}/history");
    let page = success(
        &app,
        &token,
        "GET",
        &endpoint(&base, &[("limit", "1")]),
        None,
    )
    .await;
    let root = page["history_root"].as_str().unwrap().to_owned();
    let next = page["next_cursor"].as_str().unwrap().to_owned();
    // Branch and merge pages bind the same root.
    let branches = success(
        &app,
        &token,
        "GET",
        &endpoint(&format!("{base}/branches"), &[("history_root", &root)]),
        None,
    )
    .await;
    assert_eq!(
        keys(&branches),
        [
            "branches",
            "file_id",
            "history_root",
            "history_tree_id",
            "next_cursor",
            "previous_cursor",
            "selected_branch_id"
        ]
    );
    assert_eq!(branches["history_root"], root);
    assert_eq!(branches["history_tree_id"], page["history_tree_id"]);
    let branch = &branches["branches"][0];
    assert_eq!(branch["current"], true);
    assert_eq!(branch["pending_count"], 0);
    assert!(branch.get("pending_file_versions").is_none());
    assert_eq!(branch["file_id"], file);
    assert_eq!(branches["selected_branch_id"], branch["branch_id"]);
    let merges = success(
        &app,
        &token,
        "GET",
        &endpoint(&format!("{base}/merges"), &[("history_root", &root)]),
        None,
    )
    .await;
    assert_eq!(merges["merges"], json!([]));
    assert!(merges["next_cursor"].is_null());
    // A write on the same file: old root is 409 on every bound route.
    manager_upload(&app, &token, "report.txt", b"four").await;
    for path in [
        endpoint(&base, &[("cursor", &next), ("limit", "1")]),
        endpoint(&base, &[("history_root", &root)]),
        endpoint(&format!("{base}/branches"), &[("history_root", &root)]),
        endpoint(&format!("{base}/merges"), &[("history_root", &root)]),
        endpoint(
            &format!("/api/files/{file}/history"),
            &[("history_root", &root)],
        ),
    ] {
        let (status, response) = send(&app, Some(&token), "GET", &path, None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{path}: {response}");
        assert_eq!(response["error"]["code"], "HISTORY_CHANGED");
    }
    let fresh = success(
        &app,
        &token,
        "GET",
        &endpoint(&base, &[("limit", "1")]),
        None,
    )
    .await;
    assert_ne!(fresh["history_root"], root);
    let fresh_root = fresh["history_root"].as_str().unwrap().to_owned();
    let fresh_next = fresh["next_cursor"].as_str().unwrap().to_owned();
    success(&app, &token, "POST", "/api/auth/logout", None).await;
    for path in [
        endpoint(&base, &[("cursor", &fresh_next), ("limit", "1")]),
        endpoint(
            &format!("{base}/branches"),
            &[("history_root", &fresh_root)],
        ),
    ] {
        let (status, response) = send(&app, Some(&token), "GET", &path, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{response}");
    }
}

#[tokio::test]
async fn sub_collections_bind_the_root_and_slice_by_index() {
    let (_root, app, token, file, _repo, version) = fixture().await;
    let base = format!("/api/file-manager/files/{file}/history");
    let page = success(&app, &token, "GET", &base, None).await;
    let root = page["history_root"].as_str().unwrap().to_owned();
    let rooted = [("history_root", root.as_str())];
    // Parents of the current body: exactly one, complete.
    let parents = success(
        &app,
        &token,
        "GET",
        &endpoint(&format!("{base}/nodes/{version}/parents"), &rooted),
        None,
    )
    .await;
    assert_eq!(
        keys(&parents),
        [
            "file_id",
            "history_root",
            "history_tree_id",
            "next_cursor",
            "node_id",
            "parents",
            "previous_cursor"
        ]
    );
    assert_eq!(parents["node_id"], version);
    assert_eq!(parents["parents"].as_array().unwrap().len(), 1);
    assert_eq!(
        parents["parents"][0]["node_id"],
        page["nodes"][1]["node_id"]
    );
    assert!(parents["next_cursor"].is_null());
    let (status, response) = send(
        &app,
        Some(&token),
        "GET",
        &endpoint(&format!("{base}/nodes/{version}/parents"), &[]),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        response["error"]["message"],
        "history_root is required (query or cursor)"
    );
    let (status, response) = send(
        &app,
        Some(&token),
        "GET",
        &endpoint(&format!("{base}/nodes/node_missing/parents"), &rooted),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{response}");
    assert_eq!(response["error"]["code"], "FILE_NODE_NOT_FOUND");
    // Field slices: UTF-8 windows with exact completion.
    let message = success(
        &app,
        &token,
        "GET",
        &endpoint(
            &format!("{base}/nodes/{version}/fields/message"),
            &[("history_root", &root), ("max_bytes", "1")],
        ),
        None,
    )
    .await;
    assert_eq!(
        keys(&message),
        [
            "complete",
            "field",
            "file_id",
            "history_root",
            "history_tree_id",
            "next_offset",
            "node_id",
            "offset",
            "text",
            "total_bytes"
        ]
    );
    assert_eq!(message["field"], "message");
    let total = message["total_bytes"].as_u64().unwrap() as usize;
    if total == 0 {
        assert_eq!(message["complete"], true);
        assert!(message["next_offset"].is_null());
    } else {
        assert_eq!(message["complete"], false);
        assert_eq!(message["next_offset"], 1);
    }
    let author = success(
        &app,
        &token,
        "GET",
        &endpoint(&format!("{base}/nodes/{version}/fields/author"), &rooted),
        None,
    )
    .await;
    assert_eq!(author["text"], "admin");
    assert_eq!(author["complete"], true);
    let (status, response) = send(
        &app,
        Some(&token),
        "GET",
        &endpoint(&format!("{base}/nodes/{version}/fields/path"), &rooted),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(response["error"]["message"], "unknown field");
    let (status, _) = send(
        &app,
        Some(&token),
        "GET",
        &endpoint(
            &format!("{base}/nodes/{version}/fields/message"),
            &[("history_root", &root), ("max_bytes", "0")],
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Advanced body page and sources by index.
    let versions = success(
        &app,
        &token,
        "GET",
        &endpoint(&format!("/api/files/{file}/history"), &[("limit", "2")]),
        None,
    )
    .await;
    assert_eq!(
        keys(&versions),
        [
            "file_id",
            "history_root",
            "history_tree_id",
            "next_cursor",
            "previous_cursor",
            "versions"
        ]
    );
    assert_eq!(versions["history_root"], root);
    let records = versions["versions"].as_array().unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        records[0]["version_sequence"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            < records[1]["version_sequence"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
    );
    assert!(records[0].get("source_version_ids").is_none());
    assert_eq!(records[0]["source_count"], 0);
    assert_eq!(records[1]["source_count"], 1);
    let rest = success(
        &app,
        &token,
        "GET",
        &endpoint(
            &format!("/api/files/{file}/history"),
            &[
                ("limit", "2"),
                ("cursor", versions["next_cursor"].as_str().unwrap()),
            ],
        ),
        None,
    )
    .await;
    assert_eq!(rest["versions"].as_array().unwrap().len(), 1);
    assert_eq!(rest["versions"][0]["version_id"], version);
    assert!(rest["next_cursor"].is_null());
    let sources = success(
        &app,
        &token,
        "GET",
        &endpoint(
            &format!("/api/files/{file}/history/versions/{version}/sources"),
            &rooted,
        ),
        None,
    )
    .await;
    assert_eq!(
        keys(&sources),
        [
            "file_id",
            "history_root",
            "history_tree_id",
            "next_offset",
            "offset",
            "sources",
            "total",
            "version_id"
        ]
    );
    assert_eq!(sources["total"], 1);
    assert_eq!(
        sources["sources"],
        json!([{"index":0,"version_id":records[1]["version_id"]}])
    );
    assert!(sources["next_offset"].is_null());
    let (status, response) = send(
        &app,
        Some(&token),
        "GET",
        &format!("/api/files/{file}/history?unused=yes"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    let (status, response) = send(
        &app,
        Some(&token),
        "GET",
        "/api/files/file_missing/history",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{response}");
    assert_eq!(response["error"]["code"], "FILE_NOT_FOUND");
    let (status, response) = send(
        &app,
        Some(&token),
        "GET",
        &endpoint(&format!("{base}/merges/fmerge_missing/inputs"), &rooted),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{response}");
    assert_eq!(response["error"]["code"], "NOT_FOUND");
}

#[tokio::test]
async fn deleted_file_history_remains_readable_without_a_context_entry() {
    let (_root, app, token, file, _repo, _version) = fixture().await;
    manager_namespace(&app, &token, "delete", "report.txt", None).await;
    let page = success(
        &app,
        &token,
        "GET",
        &format!("/api/file-manager/files/{file}/history"),
        None,
    )
    .await;
    assert!(page["context"]["path"].is_null());
    assert!(page["context"]["current_node_id"].is_null());
    assert_eq!(
        keys(&page["context"]),
        ["current_node_id", "note", "path", "selected_branch_id"]
    );
    assert_eq!(page["nodes"].as_array().unwrap().len(), 3);
    assert!(page["next_cursor"].is_null());
}
