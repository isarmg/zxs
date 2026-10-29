use super::*;

#[test]
fn typed_file_history_preserves_core_query_wire_and_admitted_snapshot_lifetime() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let file = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .file_id
        .clone();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"second");
    let versions = engine.file_history(&file).unwrap();
    let root_hash = engine.history_root(&repo.history_tree_id).unwrap();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let read = |query: BTreeMap<String, String>| {
        backend.file_version_page(
            &identity,
            crate::ReadVersionPage::parse(file.clone(), query).unwrap(),
        )
    };
    let page = read(BTreeMap::new()).unwrap();
    assert_eq!(
        page.versions,
        versions
            .iter()
            .map(crate::backend_file_history::VersionSummary::from)
            .collect::<Vec<_>>()
    );
    assert_eq!(page.history_root, root_hash.as_str());
    assert!(
        page.versions
            .windows(2)
            .all(|pair| pair[0].version_sequence <= pair[1].version_sequence)
    );
    // A later write on the same file changes the root: an explicit old root
    // is refused with HISTORY_CHANGED, while a fresh first page captures the
    // new one.
    save_projection_file(
        &mut backend.engine.lock().unwrap(),
        &repo.main_branch_id,
        "file.txt",
        b"later",
    );
    let stale = read(BTreeMap::from([(
        "history_root".to_owned(),
        page.history_root.clone(),
    )]))
    .unwrap_err();
    assert_eq!(stale.code, storage_contract::ErrorCode::HistoryChanged);
    assert_eq!(stale.status, http::StatusCode::CONFLICT);
    let fresh = read(BTreeMap::new()).unwrap();
    assert_ne!(fresh.history_root, page.history_root);
    assert_eq!(fresh.versions.len(), 3);
    let missing = backend
        .file_version_page(
            &identity,
            crate::ReadVersionPage::parse("missing".into(), BTreeMap::new()).unwrap(),
        )
        .unwrap_err();
    assert_eq!(missing.code, storage_contract::ErrorCode::FileNotFound);
    auth.logout(&token).unwrap();
    assert_eq!(
        read(BTreeMap::new()).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    // The page captured under the lock encodes after the lock is released:
    // record encoding never touches the engine mutex.
    struct Probe<'a>(&'a EngineBackend, &'a crate::VersionPage);
    impl Probe<'_> {
        fn encode(&self) -> Value {
            let unlocked = std::thread::scope(|scope| {
                scope
                    .spawn(|| self.0.engine.try_lock().is_ok())
                    .join()
                    .unwrap()
            });
            assert!(unlocked, "file history page encoding retained engine mutex");
            let encoded = self
                .1
                .encode(crate::backend_file_history::PageBudget::PROTOCOL)
                .unwrap();
            json_codec::from_slice(encoded.body()).unwrap()
        }
    }
    let encoded = Probe(&backend, &fresh).encode();
    assert_eq!(encoded["file_id"], file);
    assert_eq!(encoded["history_root"], fresh.history_root);
    assert_eq!(encoded["versions"].as_array().unwrap().len(), 3);
    for (wire, version) in encoded["versions"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&fresh.versions)
    {
        assert_eq!(wire["version_id"], version.version_id);
        assert_eq!(wire["logical_size"], version.logical_size.to_string());
        assert_eq!(wire["source_count"], version.source_count);
        assert!(wire.get("source_version_ids").is_none());
    }
    assert!(encoded["previous_cursor"].is_null() && encoded["next_cursor"].is_null());
    drop(auth);
    drop(backend);
    assert_eq!(
        json_codec::from_slice::<Value>(
            fresh
                .encode(crate::backend_file_history::PageBudget::PROTOCOL)
                .unwrap()
                .body()
        )
        .unwrap(),
        encoded
    );
}
