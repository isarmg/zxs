//! Unsupported history filters fail at HTTP instead of implying pagination.
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
    assert_eq!(scoped, plain);
    assert_eq!(scoped["repository_id"], repo);
    assert_eq!(scoped["path"], "report.txt");
    assert_eq!(scoped["version_id"], entry["version_id"]);
    assert_eq!(scoped["current_node_id"], entry["node_id"]);
    assert_eq!(scoped["selected_branch_id"], entry["selected_branch_id"]);

    let casefolded = success(
        &app,
        &token,
        "GET",
        &endpoint(file, &[("repository_id", repo), ("path", "REPORT.TXT")]),
        None,
    )
    .await;
    assert_eq!(casefolded, plain);
    assert_eq!(
        plain
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec![
            "branches",
            "content_state",
            "current_node_id",
            "file_id",
            "inherited",
            "materialized",
            "merges",
            "node_id",
            "nodes",
            "note",
            "own_version_id",
            "path",
            "repository_id",
            "selected_branch_id",
            "source_version_id",
            "version_id",
            "versions",
            "visible_content_state",
        ]
    );
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
    for field in ["limit", "cursor", "repositoryId", "Path"] {
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
