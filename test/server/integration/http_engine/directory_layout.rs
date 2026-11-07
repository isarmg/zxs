use super::*;
use std::{collections::BTreeMap, fs, io::Read, os::unix::fs::MetadataExt, path::PathBuf};
use storage_contract::directory_git::DirectoryLayoutResult;

async fn list(app: &Router, token: &str) -> Value {
    success(
        app,
        token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=docs",
        None,
    )
    .await
}
fn containers(root: &Path) -> BTreeMap<String, (PathBuf, u64)> {
    fn visit(path: &Path, result: &mut BTreeMap<String, (PathBuf, u64)>) {
        let marker = path.join(".zxs-container-owner.json");
        if marker.is_file() {
            let id: String = json_codec::from_slice(&fs::read(marker).unwrap()).unwrap();
            assert!(
                result
                    .insert(id, (path.to_owned(), fs::metadata(path).unwrap().ino()))
                    .is_none()
            );
            return;
        }
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(&entry.path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, &mut result);
    result
}
fn route(listing: &Value) -> String {
    format!(
        "/api/repositories/{}/directory-layout/reconcile",
        listing["context"]["repository_id"].as_str().unwrap()
    )
}
fn body(listing: &Value) -> Value {
    json!({"expected_namespace_head":listing["context"]["base_head"]})
}

#[tokio::test]
async fn directory_layout_http_moves_real_container_once_and_cold_reads_saved_main() {
    let root = tempfile::tempdir().unwrap();
    let app = configured_application(
        root.path(),
        Config::development([root.path().join("d1")]).unwrap(),
    );
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/a.txt", b"saved before initialization").await;
    let initialized = manager_initialize_git(&app, &token, "docs").await;
    assert!(initialized["head_tree_id"].is_null());
    manager_upload(
        &app,
        &token,
        "docs/a.txt",
        b"saved without a directory commit",
    )
    .await;
    let listing = list(&app, &token).await;
    let before = containers(&root.path().join("d1"));
    assert_eq!(before.len(), 1);
    let wire = success(&app, &token, "POST", &route(&listing), Some(body(&listing))).await;
    let receipt: DirectoryLayoutResult = json_codec::from_value(wire.clone()).unwrap();
    assert_eq!(receipt.repository_id, listing["context"]["repository_id"]);
    assert_eq!(receipt.containers, 1);
    assert_eq!(
        (
            receipt.unavailable_containers,
            receipt.unavailable_histories
        ),
        (0, 0)
    );
    assert!(wire.get("operation_id").is_none());
    assert!(wire["state_sequence"].is_string());
    let after = containers(&root.path().join("d1"));
    for (id, (old_path, inode)) in &before {
        let (path, actual) = &after[id];
        assert_ne!(path, old_path);
        assert_eq!(
            actual, inode,
            "layout must move, not duplicate the container"
        );
        assert!(path.to_str().unwrap().contains("/directory-repositories/"));
        assert!(!old_path.exists());
    }
    assert_eq!(
        list(&app, &token).await,
        listing,
        "layout never changes the logical namespace"
    );
    let repeated = success(&app, &token, "POST", &route(&listing), Some(body(&listing))).await;
    assert_eq!(repeated, wire);
    assert_eq!(containers(&root.path().join("d1")), after);
    let version = listing["entries"][0]["version_id"]
        .as_str()
        .unwrap()
        .to_owned();
    drop(app);
    let engine = reopen_after_router_drop(&root.path().join("system")).await;
    let mut content = engine.open_content(&version).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"saved without a directory commit");
    assert_eq!(containers(&root.path().join("d1")), after);
}

#[tokio::test]
async fn directory_layout_http_rejects_stale_unknown_and_unauthorized_before_moving() {
    let root = tempfile::tempdir().unwrap();
    let app = configured_application(
        root.path(),
        Config::development([root.path().join("d1")]).unwrap(),
    );
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/a.txt", b"first").await;
    manager_initialize_git(&app, &token, "docs").await;
    let old = list(&app, &token).await;
    manager_upload(&app, &token, "docs/a.txt", b"second").await;
    let current = list(&app, &token).await;
    let before = containers(&root.path().join("d1"));
    let endpoint = route(&current);
    let stale = send(&app, Some(&token), "POST", &endpoint, Some(body(&old))).await;
    assert_eq!(stale.0, StatusCode::CONFLICT, "{}", stale.1);
    assert_eq!(stale.1["error"]["code"], "HEAD_CONFLICT");
    for invalid in [
        json!({}),
        json!({"expected_namespace_head":"bad"}),
        json!({"expected_namespace_head":current["context"]["base_head"],"operation_id":new_id("op").unwrap()}),
    ] {
        assert_eq!(
            send(&app, Some(&token), "POST", &endpoint, Some(invalid))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        send(&app, None, "POST", &endpoint, Some(body(&current)))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let revoked = revoked_session(&app).await;
    assert_eq!(
        send(
            &app,
            Some(&revoked),
            "POST",
            &endpoint,
            Some(body(&current))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(containers(&root.path().join("d1")), before);
}

#[tokio::test]
async fn directory_layout_http_does_not_initialize_an_ordinary_directory() {
    let root = tempfile::tempdir().unwrap();
    let app = configured_application(
        root.path(),
        Config::development([root.path().join("d1")]).unwrap(),
    );
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/a.txt", b"ordinary").await;
    let current = list(&app, &token).await;
    let before = containers(&root.path().join("d1"));
    assert_ne!(
        send(
            &app,
            Some(&token),
            "POST",
            &route(&current),
            Some(body(&current))
        )
        .await
        .0,
        StatusCode::OK
    );
    let repo = current["context"]["repository_id"].as_str().unwrap();
    let status = success(
        &app,
        &token,
        "GET",
        &format!("/api/repositories/{repo}/directory-git?path=docs"),
        None,
    )
    .await;
    assert_eq!(status["initialized"], false);
    assert_eq!(containers(&root.path().join("d1")), before);
}
