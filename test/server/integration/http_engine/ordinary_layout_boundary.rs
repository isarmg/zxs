//! An obsolete management URL cannot relocate a stable FILE or mutate any
//! data-disk material. Use genuine authenticated saves and a native cold read.
use super::*;
use std::{collections::BTreeMap, fs, io::Read, os::unix::fs::MetadataExt, path::PathBuf};

#[derive(PartialEq, Eq)]
enum EntryMaterial {
    Directory,
    File(Vec<u8>),
    Link(PathBuf),
}
type DiskMaterial = BTreeMap<PathBuf, (u64, u64, EntryMaterial)>;

/// Walk only the generated fixture disks. Include directory identities and
/// empty directories as well as exact bytes; never follow a symbolic link.
fn material(root: &Path) -> DiskMaterial {
    fn visit(path: &Path, result: &mut DiskMaterial) {
        let meta = fs::symlink_metadata(path).unwrap();
        let value = if meta.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(&entry.unwrap().path(), result);
            }
            EntryMaterial::Directory
        } else if meta.file_type().is_symlink() {
            EntryMaterial::Link(fs::read_link(path).unwrap())
        } else {
            assert!(meta.is_file());
            EntryMaterial::File(fs::read(path).unwrap())
        };
        assert!(result.insert(path.to_owned(), (meta.dev(), meta.ino(), value)).is_none());
    }
    let mut result = BTreeMap::new();
    for slot in ["d1", "d2", "d3"] {
        visit(&root.join(slot), &mut result);
    }
    result
}

#[tokio::test]
async fn retired_layout_route_cannot_move_registered_file_material() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let path = "docs/project.zxs";
    // Two standard empty ZIP archives with different ZIP comments. Both are
    // opaque bytes to the ordinary FILE store; its version IDs stay separate.
    let mut first = b"PK\x05\x06".to_vec();
    first.extend_from_slice(&[0; 18]);
    let mut second = first.clone();
    second[20] = 1;
    second.push(b'x');
    let list = "/api/file-manager/disks/P01/entries?path=docs";
    manager_upload(&app, &token, path, &first).await;
    let previous = success(&app, &token, "GET", list, None).await;
    manager_upload(&app, &token, path, &second).await;
    let current = success(&app, &token, "GET", list, None).await;
    let file = current["entries"][0]["file_id"].as_str().unwrap();
    let old_version = previous["entries"][0]["version_id"].as_str().unwrap();
    let version = current["entries"][0]["version_id"].as_str().unwrap();
    assert_eq!(previous["entries"][0]["file_id"], file);
    assert_ne!(old_version, version);
    assert_ne!(previous["context"]["base_head"], current["context"]["base_head"]);
    let context = &current["context"];
    let repo = context["repository_id"].as_str().unwrap();
    let endpoint = format!("/api/repositories/{repo}/directory-layout/reconcile");
    let branch = context["branch_id"].as_str().unwrap();
    let events = success(&app, &token, "GET", "/api/events", None).await;
    let before = material(root.path());
    assert!(!before.is_empty());
    for head in [&context["base_head"], &previous["context"]["base_head"]] {
        let (status, rejected) = send(&app, Some(&token), "POST", &endpoint,
            Some(json!({"expected_namespace_head":head}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(rejected["error"]["code"], "NOT_FOUND");
        assert_eq!(rejected["error"]["message"], "route not found");
    }
    assert!(material(root.path()) == before, "removed route changed names, identity or data");
    assert_eq!(success(&app, &token, "GET", list, None).await, current);
    assert_eq!(success(&app, &token, "GET", "/api/events", None).await, events);

    drop(app);
    let engine = reopen_after_router_drop(&root.path().join("system")).await;
    let tree = engine.list_tree(branch).unwrap();
    let entry = tree.iter().find(|entry| entry.path() == path).unwrap();
    assert_eq!(entry.file_id, file);
    assert_eq!(entry.version_id, version);
    for (id, expected) in [(old_version, &first), (version, &second)] {
        let mut content = engine.open_content(id).unwrap();
        let mut bytes = Vec::new();
        content.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(&bytes, expected);
    }
}
