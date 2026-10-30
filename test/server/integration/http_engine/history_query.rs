//! History windows bind bounded pages to the exact root and resolved context.
use super::*;

fn endpoint(file: &str, pairs: &[(&str, &str)]) -> String {
    let mut query = url_encoding::form::Serializer::new(String::new());
    for (key, value) in pairs {
        query.append_pair(key, value);
    }
    format!("/api/file-manager/files/{file}/history?{}", query.finish())
}

#[tokio::test]
async fn history_query_rejects_unknown_fields_and_preserves_paired_context() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "report.txt", b"history body").await;
    manager_upload(&app, &token, "report.txt", b"next history body").await;
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
    let repo = entry["repository_id"].as_str().unwrap();
    let context = [("repository_id", repo), ("path", "report.txt")];
    let plain = success(&app, &token, "GET", &endpoint(file, &[]), None).await;
    let scoped = success(&app, &token, "GET", &endpoint(file, &context), None).await;
    assert_eq!(plain["kind"], "nodes");
    assert_eq!(scoped["nodes"], plain["nodes"]);
    assert_eq!(scoped["context"], plain["context"]);
    assert_eq!(scoped["identity"]["context"], plain["identity"]["context"]);
    assert_eq!(history_root(&scoped), history_root(&plain));
    assert_ne!(
        scoped["identity"]["query_hash"],
        plain["identity"]["query_hash"]
    );
    assert_eq!(scoped["identity"]["context"]["repository_id"], repo);
    assert_eq!(scoped["identity"]["context"]["path"], "report.txt");
    assert_eq!(
        scoped["context"]["content"]["version_id"],
        entry["version_id"]
    );
    assert_eq!(
        scoped["identity"]["context"]["current_node_id"],
        entry["node_id"]
    );
    assert_eq!(
        scoped["identity"]["context"]["selected_branch_id"],
        entry["selected_branch_id"]
    );
    assert!(plain["page"]["previous_cursor"].is_null());
    assert!(plain["page"]["next_cursor"].is_null());

    let casefolded = success(
        &app,
        &token,
        "GET",
        &endpoint(file, &[("repository_id", repo), ("path", "REPORT.TXT")]),
        None,
    )
    .await;
    assert_eq!(casefolded, scoped);
    assert_eq!(
        plain
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["context", "edges", "identity", "kind", "nodes", "page"]
    );
    let first = success(
        &app,
        &token,
        "GET",
        &endpoint(file, &[("limit", "1")]),
        None,
    )
    .await;
    assert_eq!(first["identity"]["limit"], 1);
    assert_eq!(first["nodes"], json!([plain["nodes"][0]]));
    assert!(first["page"]["previous_cursor"].is_null());
    let cursor = first["page"]["next_cursor"].as_str().unwrap();
    let second = success(
        &app,
        &token,
        "GET",
        &endpoint(file, &[("cursor", cursor)]),
        None,
    )
    .await;
    assert_eq!(
        second["identity"]["query_hash"],
        first["identity"]["query_hash"]
    );
    assert_eq!(second["nodes"], json!([plain["nodes"][1]]));
    assert!(second["page"]["next_cursor"].is_null());
    let previous = second["page"]["previous_cursor"].as_str().unwrap();
    assert_eq!(
        success(
            &app,
            &token,
            "GET",
            &endpoint(file, &[("cursor", previous)]),
            None
        )
        .await,
        first
    );
    for pairs in [
        vec![("cursor", cursor), ("limit", "2")],
        vec![
            ("cursor", cursor),
            ("repository_id", repo),
            ("path", "report.txt"),
        ],
    ] {
        assert_eq!(
            send(&app, Some(&token), "GET", &endpoint(file, &pairs), None)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    for path in ["", "../report.txt", "/report.txt"] {
        let (status, response) = send(
            &app,
            Some(&token),
            "GET",
            &endpoint(file, &[("repository_id", repo), ("path", path)]),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    }
    for field in ["all", "mode", "unused", "repositoryId", "Path"] {
        for mut pairs in [vec![], context.to_vec()] {
            pairs.push((field, "unused"));
            let (status, response) =
                send(&app, Some(&token), "GET", &endpoint(file, &pairs), None).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
            assert_eq!(response["error"]["code"], "INVALID_REQUEST");
            assert_eq!(
                response["error"]["message"],
                format!("unknown field {field}")
            );
        }
    }
    for pairs in [
        vec![("limit", "0")],
        vec![("limit", "65")],
        vec![("limit", "01")],
        vec![("limit", "unused")],
        vec![("cursor", "unused")],
        vec![("limit", "1"), ("limit", "1")],
        vec![
            ("history_root", history_root(&plain)),
            ("history_root", history_root(&plain)),
        ],
    ] {
        let (status, error) = send(&app, Some(&token), "GET", &endpoint(file, &pairs), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    }
    let node = entry["node_id"].as_str().unwrap();
    let version = entry["version_id"].as_str().unwrap();
    for route in [
        format!("/api/file-manager/files/{file}/history/branches"),
        format!("/api/file-manager/files/{file}/history/merges"),
        format!("/api/file-manager/files/{file}/history/nodes/{node}/parents"),
        format!("/api/file-manager/files/{file}/history/nodes/{node}/fields/message"),
        format!("/api/files/{file}/history/versions/{version}/sources"),
        format!(
            "/api/file-manager/files/{file}/history/merges/fmerge_00000000000000000000000000000000/inputs"
        ),
        format!(
            "/api/file-manager/files/{file}/history/merges/fmerge_00000000000000000000000000000000/parents"
        ),
        endpoint(file, &[("anchor_node_id", node)]),
    ] {
        let (status, error) = send(&app, Some(&token), "GET", &route, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{route}: {error}");
        assert_eq!(error["error"]["code"], "INVALID_REQUEST");
    }
    let anchored = success(
        &app,
        &token,
        "GET",
        &endpoint(
            file,
            &[
                ("anchor_node_id", node),
                ("history_root", history_root(&plain)),
                ("limit", "1"),
            ],
        ),
        None,
    )
    .await;
    assert_eq!(anchored["nodes"], json!([plain["nodes"][0]]));
    let (status, _) = send(
        &app,
        Some(&token),
        "GET",
        &endpoint(
            file,
            &[
                ("anchor_node_id", node),
                ("history_root", history_root(&plain)),
                ("cursor", cursor),
            ],
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    for pair in context {
        let (status, response) =
            send(&app, Some(&token), "GET", &endpoint(file, &[pair]), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(response["error"]["code"], "INVALID_REQUEST");
        assert_eq!(
            response["error"]["message"],
            "repository_id and path must be supplied together"
        );
    }
    assert_eq!(
        success(&app, &token, "GET", &endpoint(file, &context), None).await,
        scoped
    );
    let (status, response) =
        send(&app, None, "GET", &endpoint(file, &[("limit", "1")]), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(response["error"]["code"], "UNAUTHORIZED");
}

#[tokio::test]
async fn history_window_roots_change_with_metadata_but_not_authentication() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "report.txt", b"root-bound body").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let file = listing["entries"][0]["file_id"].as_str().unwrap();
    let before = success(&app, &token, "GET", &endpoint(file, &[]), None).await;
    let session = login(&app, "admin", "integration-test-password").await;
    assert_eq!(
        success(&app, &session, "GET", &endpoint(file, &[]), None).await,
        before
    );
    success(
        &app,
        &token,
        "POST",
        &format!("/api/file-manager/files/{file}/metadata"),
        Some(json!({"note":"changed history metadata","operation_id":new_id("op")})),
    )
    .await;
    let current = success(&app, &session, "GET", &endpoint(file, &[]), None).await;
    assert_ne!(history_root(&current), history_root(&before));
    let old = format!(
        "/api/file-manager/files/{file}/history/branches?history_root={}",
        history_root(&before)
    );
    let (status, error) = send(&app, Some(&session), "GET", &old, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "HISTORY_CHANGED");
    success(&app, &session, "POST", "/api/auth/logout", None).await;
    let (status, error) = send(&app, Some(&session), "GET", &old, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error["error"]["code"], "UNAUTHORIZED");
}
