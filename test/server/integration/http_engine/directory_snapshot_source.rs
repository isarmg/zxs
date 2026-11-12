//! The protected preview consumes real current namespaces and verified payloads.
use super::*;
use storage_contract::history_window::HistoryWindowCancellation;
use storage_server::ReadDirectorySnapshotSource;

const ROUTE: &str = "/api/file-manager/directory-snapshot-source";
static PREVIEWS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn fixture() -> (tempfile::TempDir, Router, Arc<EngineBackend>, Identity) {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("system");
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ])
    .unwrap();
    let backend = Arc::new(EngineBackend::new(Engine::initialize(&system, config).unwrap()));
    let auth = backend
        .protect_auth(AuthStore::initialize(&system, "admin", "integration-test-password").unwrap())
        .unwrap();
    let identity = auth.sole_identity().unwrap();
    let app = storage_server::router(backend.clone(), auth, ServerConfig::new(&system)).unwrap();
    (root, app, backend, identity)
}

fn endpoint(branch: &str, path: Option<&str>) -> String {
    let mut query = url_encoding::form::Serializer::new(String::new());
    query.append_pair("branch_id", branch);
    if let Some(path) = path {
        query.append_pair("path", path);
    }
    format!("{ROUTE}?{}", query.finish())
}

async fn root_listing(app: &Router, token: &str) -> Value {
    success(app, token, "GET", "/api/file-manager/disks/P01/entries", None).await
}

#[tokio::test]
async fn authenticated_preview_matches_current_smb_paths_and_preserves_storage() {
    let _serial = PREVIEWS.lock().await;
    let (root, app, backend, identity) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "Docs/Report.txt", b"old content").await;
    manager_upload(&app, &token, "Docs/Report.txt", b"current content").await;
    manager_upload(&app, &token, "Docs/Nested/empty.txt", b"").await;
    manager_upload(&app, &token, "outside.txt", b"not in this preview").await;
    let listing = root_listing(&app, &token).await;
    let branch = listing["context"]["branch_id"].as_str().unwrap();
    let history = success(&app, &token, "GET", "/api/file-manager/history", None).await;
    let before = manager_disk_files(root.path());
    let preview = success(&app, &token, "GET", &endpoint(branch, Some("docs")), None).await;
    assert_eq!(preview["repository_id"], listing["context"]["repository_id"]);
    assert_eq!(preview["branch_id"], branch);
    assert_eq!(preview["root"], "Docs");
    assert_eq!(preview["initialization"], "uninitialized");
    assert_eq!(preview["can_commit"], false);
    assert_eq!(preview["physical_initialization_checked"], false);
    assert_eq!(preview["full_content_verified"], true);
    let entries = preview["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(preview["usage"]["entries"], 3);
    assert_eq!(preview["usage"]["unique_versions"], 2);
    assert_eq!(preview["usage"]["logical_bytes"], "15");
    for entry in entries {
        let relative = entry["path"].as_str().unwrap();
        let path = name_policy::LookupPath::parse(&format!("Docs/{relative}")).unwrap();
        let current = backend
            .lookup_current(&identity, branch, &path)
            .unwrap()
            .unwrap();
        assert_eq!(entry["entry_id"], current.entry_id);
        assert!(!relative.starts_with("Docs/"));
        assert!(entry.get("body").is_none());
        assert!(entry.get("tags").is_none());
        assert!(entry.get("note").is_none());
        if entry["kind"] == "file" {
            assert_eq!(entry["file_id"], current.file_id);
            assert_eq!(entry["version_id"], current.version_id);
            let version = backend.version_snapshot(&identity, &current.version_id).unwrap();
            assert_eq!(entry["size"], version.version.logical_size.to_string());
            assert_eq!(entry["content_hash"], version.version.content_hash.to_string());
        }
    }
    assert_eq!(root_listing(&app, &token).await, listing);
    assert_eq!(
        success(&app, &token, "GET", "/api/file-manager/history", None).await,
        history
    );
    assert_eq!(manager_disk_files(root.path()), before);
}

#[tokio::test]
async fn preview_requires_current_identity_and_strict_typed_query() {
    let _serial = PREVIEWS.lock().await;
    let (root, app, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/report.txt", b"stored").await;
    let listing = root_listing(&app, &token).await;
    let branch = listing["context"]["branch_id"].as_str().unwrap();
    let route = endpoint(branch, Some("docs"));
    let revoked = revoked_session(&app).await;
    for token in [None, Some(revoked.as_str())] {
        assert_eq!(
            send(&app, token, "GET", &route, None).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let before = manager_disk_files(root.path());
    for query in [
        format!("{route}&initialized=true"),
        format!("{route}&skip=.zxs"),
        format!("{route}&slot_id=B01"),
        format!("{route}&commit_id={}", new_id("commit").unwrap()),
        format!("{route}&branch_id={branch}"),
        format!("{route}&path=docs"),
        format!("{ROUTE}?path=docs"),
        format!("{ROUTE}?branch_id=invalid"),
        endpoint(branch, Some("")),
        endpoint(branch, Some("../docs")),
        endpoint(branch, Some("/docs")),
    ] {
        let (status, rejected) = send(&app, Some(&token), "GET", &query, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {rejected}");
    }
    let missing = endpoint(&new_id("br").unwrap(), Some("docs"));
    assert_eq!(
        send(&app, Some(&token), "GET", &missing, None).await.0,
        StatusCode::NOT_FOUND
    );
    for path in ["missing", "docs/report.txt"] {
        let (status, rejected) = send(
            &app,
            Some(&token),
            "GET",
            &endpoint(branch, Some(path)),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{rejected}");
    }
    assert_eq!(manager_disk_files(root.path()), before);
    let files = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=docs",
        None,
    )
    .await;
    let file_branch = files["entries"][0]["selected_branch_id"].as_str().unwrap();
    assert_ne!(file_branch, branch);
    let (status, rejected) = send(
        &app,
        Some(&token),
        "GET",
        &endpoint(file_branch, None),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
    assert_eq!(manager_disk_files(root.path()), before);
}

#[tokio::test]
async fn disabled_smb_source_is_rejected_until_current_binding_is_enabled() {
    let _serial = PREVIEWS.lock().await;
    let (root, app, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/report.txt", b"exposed body").await;
    let listing = root_listing(&app, &token).await;
    let repository = listing["context"]["repository_id"].as_str().unwrap();
    let branch = listing["context"]["branch_id"].as_str().unwrap();
    let binding_route = format!("/api/repositories/{repository}/smb-binding");
    let preview_route = endpoint(branch, Some("docs"));
    for enabled in [false, true] {
        let binding = success(&app, &token, "GET", &binding_route, None).await;
        success(
            &app,
            &token,
            "PUT",
            &binding_route,
            Some(json!({
                "branch_id":branch,
                "enabled":enabled,
                "expected_history_generation":binding["history_generation"],
                "operation_id":new_id("op").unwrap()
            })),
        )
        .await;
        let before = manager_disk_files(root.path());
        let (status, response) =
            send(&app, Some(&token), "GET", &preview_route, None).await;
        if enabled {
            assert_eq!(status, StatusCode::OK, "{response}");
            assert_eq!(response["entries"][0]["path"], "report.txt");
        } else {
            assert_eq!(status, StatusCode::CONFLICT, "{response}");
            assert_eq!(response["error"]["code"], "SMB_REBIND_REQUIRED");
            assert!(response.get("entries").is_none());
        }
        assert_eq!(manager_disk_files(root.path()), before);
    }
}

#[tokio::test]
async fn visible_management_material_at_any_depth_rejects_entire_preview() {
    let _serial = PREVIEWS.lock().await;
    let (root, app, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/good.txt", b"ordinary").await;
    manager_upload(&app, &token, "docs/nested/.zxs/existing", b"unregistered").await;
    let listing = root_listing(&app, &token).await;
    let branch = listing["context"]["branch_id"].as_str().unwrap();
    let before = manager_disk_files(root.path());
    for path in [
        None,
        Some("docs"),
        Some("docs/nested"),
        Some("docs/nested/.zxs"),
    ] {
        let (status, rejected) =
            send(&app, Some(&token), "GET", &endpoint(branch, path), None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
        assert_eq!(rejected["error"]["code"], "NAME_COLLISION");
        assert!(rejected.get("entries").is_none());
    }
    assert_eq!(manager_disk_files(root.path()), before);
}

#[tokio::test]
async fn cancelled_request_is_rejected_and_deleted_content_stays_absent() {
    let _serial = PREVIEWS.lock().await;
    let (root, app, backend, identity) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/stay.txt", b"stay").await;
    manager_upload(&app, &token, "docs/delete.txt", b"delete").await;
    let listing = root_listing(&app, &token).await;
    let branch = listing["context"]["branch_id"].as_str().unwrap();
    let path = name_policy::LookupPath::parse("docs/delete.txt").unwrap();
    let deleted = backend
        .lookup_current(&identity, branch, &path)
        .unwrap()
        .unwrap();
    success(
        &app,
        &token,
        "POST",
        &format!("/api/files/{}/deletion", deleted.file_id),
        Some(json!({
            "repository_id":listing["context"]["repository_id"],
            "path":"docs/delete.txt", "operation_id":new_id("op").unwrap()
        })),
    )
    .await;
    let before = manager_disk_files(root.path());
    let cancellation = HistoryWindowCancellation::new();
    cancellation.cancel();
    let request = ReadDirectorySnapshotSource {
        branch_id: branch.parse().unwrap(),
        path: Some(name_policy::LookupPath::parse("docs").unwrap()),
    };
    let cancelled = backend.directory_snapshot_source(&identity, request, cancellation);
    let failure = match cancelled {
        Ok(_) => panic!("cancelled source verification returned a preview"),
        Err(error) => error,
    };
    assert_eq!(failure.code, storage_contract::ErrorCode::RequestTimeout);
    let preview = success(&app, &token, "GET", &endpoint(branch, Some("docs")), None).await;
    assert_eq!(preview["entries"].as_array().unwrap().len(), 1);
    assert_eq!(preview["entries"][0]["path"], "stay.txt");
    assert_eq!(preview["usage"]["unique_versions"], 1);
    assert_eq!(manager_disk_files(root.path()), before);
}

#[tokio::test]
async fn corrupt_payload_fails_without_returning_a_partial_source_list() {
    let _serial = PREVIEWS.lock().await;
    let (root, app, _, _) = fixture();
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/good.txt", b"valid body").await;
    let original = b"payload that must be fully verified";
    manager_upload(&app, &token, "docs/corrupt.txt", original).await;
    let listing = root_listing(&app, &token).await;
    let branch = listing["context"]["branch_id"].as_str().unwrap();
    let payloads = manager_disk_files(root.path())
        .into_iter()
        .filter(|(_, bytes)| bytes.as_slice() == original.as_slice())
        .map(|(path, _)| path)
        .collect::<Vec<_>>();
    assert!(
        !payloads.is_empty(),
        "fixture must locate the actual body copies"
    );
    let mut corrupt = original.to_vec();
    corrupt[0] ^= 1;
    for path in payloads {
        let original_permissions = std::fs::metadata(&path).unwrap().permissions();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .unwrap();
        std::fs::write(&path, &corrupt).unwrap();
        std::fs::set_permissions(&path, original_permissions).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), corrupt);
    }
    let before = manager_disk_files(root.path());
    let (status, rejected) = send(
        &app,
        Some(&token),
        "GET",
        &endpoint(branch, Some("docs")),
        None,
    )
    .await;
    assert!(!status.is_success(), "{rejected}");
    assert!(rejected.get("entries").is_none());
    assert!(rejected.get("full_content_verified").is_none());
    assert_eq!(manager_disk_files(root.path()), before);
}
