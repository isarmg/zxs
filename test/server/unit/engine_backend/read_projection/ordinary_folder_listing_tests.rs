use super::*;
use crate::backend_listing::ListingResponse;
use storage_contract::ErrorCode;

fn list(path: &str, entry: &storage_contract::Entry) -> crate::ReadListing {
    crate::ReadListing::parse("P01".into(), BTreeMap::from([
        ("path".into(), path.into()),
        ("selected_path".into(), entry.path.to_string()),
        ("selected_entry_id".into(), entry.entry_id.clone()),
    ])).unwrap()
}

#[test]
fn registered_folder_listings_own_the_original_file_row_and_serialize_unlocked() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "Café/project.zxs", b"original");
    let file = engine.namespace_tree(&repo.main_branch_id).unwrap()
        .get("Café/project.zxs").unwrap().clone();
    let head = engine.state().branches[&repo.main_branch_id].head_commit_id.clone();
    let (backend, _auth, _token, owner) = authenticated_backend(engine, root.path());
    let page = backend.listing(&owner, list("Café", &file)).unwrap();
    assert_eq!(page.total_entries, 1);
    assert_eq!(page.selected_entry.as_ref().unwrap().entry_id, file.entry_id);
    let response = ListingResponse::from(page);
    let before = json_codec::to_value(&response).unwrap();
    assert_eq!(before["context"]["repository_id"], repo.repository_id);
    assert_eq!(before["context"]["base_head"], head);
    assert_eq!(before["entries"][0]["entry_id"], file.entry_id);
    assert_eq!(before["entries"][0]["file_id"], file.file_id);
    assert_eq!(before["entries"][0]["size"], "8");
    assert_eq!(before["entries"][0]["version_id"], file.version_id);
    for old in ["directory_branches", "directory_branches_supported", "default_branch_scope", "preview"] {
        assert!(before.get(old).is_none());
    }

    save_projection_file(&mut backend.engine.lock().unwrap(), &repo.main_branch_id,
        "Café/project.zxs", b"replacement with distinct bytes");
    let later = backend.listing(&owner, list("Café", &file)).unwrap();
    assert_ne!(later.context.unwrap().base_head, head);
    assert_eq!(later.entries[0].entry_id, file.entry_id);
    assert_ne!(later.entries[0].size, file.size);
    struct Probe<'a>(&'a EngineBackend, &'a ListingResponse);
    impl Serialize for Probe<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            if !std::thread::scope(|scope|
                scope.spawn(|| self.0.engine.try_lock().is_ok()).join().unwrap()) {
                return Err(S::Error::custom("folder listing retained the engine lock"));
            }
            self.1.serialize(serializer)
        }
    }
    assert_eq!(json_codec::to_value(Probe(&backend, &response)).unwrap(), before);
    drop(backend);
    assert_eq!(json_codec::to_value(&response).unwrap(), before);
}

#[test]
fn retired_listing_queries_and_revoked_session_cannot_consume_a_registered_file() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "dir/ordinary.zxs", b"real FILE");
    let entry = engine.namespace_tree(&repo.main_branch_id).unwrap().get("dir/ordinary.zxs").unwrap().clone();
    let folder = engine.namespace_tree(&repo.main_branch_id).unwrap().get("dir").unwrap().clone();
    let (backend, mut auth, token, owner) = authenticated_backend(engine, root.path());
    let head = backend.engine.lock().unwrap().state().branches[&repo.main_branch_id].head_commit_id.clone();
    for forbidden in ["directory_branch_id", "expected_tree_id"] {
        let failed = crate::ReadListing::parse("P01".into(), BTreeMap::from([
            ("path".into(), "dir".into()), (forbidden.into(), "unusable".into())
        ])).err().expect("retired input cannot become current by fallback");
        assert_eq!(failed.code, ErrorCode::InvalidRequest);
    }
    assert_eq!(backend.listing(&owner, list("dir", &entry)).unwrap().entries[0].entry_id, entry.entry_id);
    auth.logout(&token).unwrap();
    for path in ["dir", "absent"] {
        assert_eq!(backend.listing(&owner, list(path, &entry)).err().unwrap().code, ErrorCode::Unauthorized);
    }
    drop(auth);
    drop(backend);
    let engine = Engine::open(&root.path().join("system")).unwrap();
    assert_eq!(engine.state().branches[&repo.main_branch_id].head_commit_id, head);
    let namespace = engine.namespace_tree(&repo.main_branch_id).unwrap();
    assert_eq!(namespace.get("dir").unwrap(), &folder);
    assert_eq!(namespace.get("dir/ordinary.zxs").unwrap(), &entry);
    let mut body = engine.open_content(&entry.version_id).unwrap();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut body.file, &mut bytes).unwrap();
    assert_eq!(bytes, b"real FILE");
}
