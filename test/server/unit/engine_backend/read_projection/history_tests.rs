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
            &tree_control(),
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
            &tree_control(),
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

#[test]
fn version_page_finishes_captured_snapshot_but_rechecks_current_authorization() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let file = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .file_id
        .clone();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let capture = || {
        let engine = backend.engine.lock().unwrap();
        crate::engine_backend::current_protected_identity(&engine, &identity).unwrap();
        capture_version(
            &engine,
            crate::ReadVersionPage::parse(file.clone(), BTreeMap::new()).unwrap(),
        )
        .unwrap()
    };
    let old = capture();
    save_projection_file(
        &mut backend.engine.lock().unwrap(),
        &repo.main_branch_id,
        "file.txt",
        b"second",
    );
    let old_page = backend
        .finish_file_history_page(&identity, &file, old.read(), &tree_control())
        .unwrap();
    let fresh = backend
        .file_version_page(
            &identity,
            crate::ReadVersionPage::parse(file.clone(), BTreeMap::new()).unwrap(),
            &tree_control(),
        )
        .unwrap();
    assert_eq!(old_page.versions.len(), 1);
    assert_eq!(fresh.versions.len(), 2);
    assert_ne!(old_page.history_root, fresh.history_root);
    let pending = capture();
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .finish_file_history_page(&identity, &file, pending.read(), &tree_control())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    // Completion authorization precedes disclosure of a detached read failure.
    assert_eq!(
        backend
            .finish_file_history_page(
                &identity,
                &file,
                Err::<crate::VersionPage, _>(ApiError::invalid("detached failure")),
                &tree_control()
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(
        backend
            .file_version_page(
                &identity,
                crate::ReadVersionPage::parse("missing".into(), BTreeMap::new()).unwrap(),
                &tree_control()
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn empty_merge_page_finishes_captured_snapshot_and_rechecks_current_authorization() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let file = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .file_id
        .clone();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let capture = || {
        let engine = backend.engine.lock().unwrap();
        crate::engine_backend::current_protected_identity(&engine, &identity).unwrap();
        let root = engine.history_root(&repo.history_tree_id).unwrap();
        capture_merges(
            &engine,
            crate::ReadHistoryMerges::parse(
                file.clone(),
                BTreeMap::from([("history_root".into(), root.as_str().to_owned())]),
            )
            .unwrap(),
        )
        .unwrap()
    };
    let old = capture();
    save_projection_file(
        &mut backend.engine.lock().unwrap(),
        &repo.main_branch_id,
        "file.txt",
        b"second",
    );
    assert!(backend.engine.try_lock().is_ok());
    let old_page = backend
        .finish_file_history_page(&identity, &file, old.read(), &tree_control())
        .unwrap();
    assert!(old_page.merges.is_empty());
    let fresh = capture().read().unwrap();
    assert_ne!(old_page.history_root, fresh.history_root);
    // The common completion boundary also rejects a no-longer-resolvable file.
    assert_eq!(
        backend
            .finish_file_history_page(&identity, "missing", Ok(fresh), &tree_control())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::FileNotFound
    );
    let pending = capture();
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .finish_file_history_page(&identity, &file, pending.read(), &tree_control())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(
        backend
            .finish_file_history_page(
                &identity,
                &file,
                Err::<crate::HistoryMergesPage, _>(ApiError::invalid("detached failure")),
                &tree_control()
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn detached_file_history_completion_rechecks_authorization_before_success_and_read_error() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let root_hash = backend
        .engine
        .lock()
        .unwrap()
        .history_root(&repo.history_tree_id)
        .unwrap();
    let capture = || {
        let engine = backend.engine.lock().unwrap();
        crate::engine_backend::current_protected_identity(&engine, &identity).unwrap();
        crate::engine_backend::file_manager::history_pages::capture(&engine, &entry.file_id, None)
            .unwrap()
    };
    let pending = (0..9).map(|_| capture()).collect::<Vec<_>>();
    let failing = capture();
    auth.logout(&token).unwrap();
    let query = || BTreeMap::from([("history_root".to_owned(), root_hash.as_str().to_owned())]);
    let control = tree_control();
    for (kind, captured) in pending.into_iter().enumerate() {
        let read = match kind {
            0 => captured
                .file_history(
                    crate::ReadFileHistory::parse(entry.file_id.clone(), query()).unwrap(),
                    &control,
                )
                .map(|page| page.history_root),
            1 => captured
                .branches(
                    crate::ReadHistoryBranches::parse(entry.file_id.clone(), query()).unwrap(),
                    &control,
                )
                .map(|page| page.history_root),
            2 => captured
                .node_parents(
                    crate::ReadNodeParents::parse(
                        entry.file_id.clone(),
                        entry.version_id.clone(),
                        query(),
                    )
                    .unwrap(),
                    &control,
                )
                .map(|page| page.history_root),
            3 => captured
                .node_field(
                    crate::ReadNodeField::parse(
                        entry.file_id.clone(),
                        entry.version_id.clone(),
                        "message".into(),
                        query(),
                    )
                    .unwrap(),
                    &control,
                )
                .map(|page| page.history_root),
            4 => captured
                .merges(
                    crate::ReadHistoryMerges::parse(entry.file_id.clone(), query()).unwrap(),
                    &control,
                )
                .map(|page| page.history_root),
            5 => captured
                .version_page(
                    crate::ReadVersionPage::parse(entry.file_id.clone(), query()).unwrap(),
                    &control,
                )
                .map(|page| page.history_root),
            6..=8 => {
                let request = crate::backend_file_history::ReadIndexedSlice::parse(
                    entry.file_id.clone(),
                    if kind == 8 { "version_id" } else { "merge_id" },
                    if kind == 8 {
                        entry.version_id.clone()
                    } else {
                        "missing-merge".into()
                    },
                    query(),
                )
                .unwrap();
                match kind {
                    6 => captured
                        .merge_inputs(request, &control)
                        .map(|page| page.history_root),
                    7 => captured
                        .merge_parents(request, &control)
                        .map(|page| page.history_root),
                    8 => captured
                        .version_sources(request, &control)
                        .map(|page| page.history_root),
                    _ => unreachable!(),
                }
            }
            _ => unreachable!(),
        };
        if matches!(kind, 6 | 7) {
            assert_eq!(
                read.as_ref().unwrap_err().status,
                http::StatusCode::NOT_FOUND
            );
        } else {
            assert_eq!(read.as_ref().unwrap(), root_hash.as_str());
        }
        assert_eq!(
            backend
                .finish_file_history_page(&identity, &entry.file_id, read, &control)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
    // Real detached node-membership failure, not an injected API result.
    let error = failing.node_field(
        crate::ReadNodeField::parse(
            entry.file_id.clone(),
            "missing-node".into(),
            "message".into(),
            query(),
        )
        .unwrap(),
        &control,
    );
    assert_eq!(
        error.as_ref().unwrap_err().code,
        storage_contract::ErrorCode::FileNodeNotFound
    );
    assert_eq!(
        backend
            .finish_file_history_page(&identity, &entry.file_id, error, &control)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}

struct PendingVersion(
    crate::engine_backend::file_manager::history_pages::FileHistoryCapture,
    crate::ReadVersionPage,
);
impl PendingVersion {
    fn read(self) -> Result<crate::VersionPage, ApiError> {
        self.0.version_page(self.1, &tree_control())
    }
}
fn capture_version(
    engine: &Engine,
    request: crate::ReadVersionPage,
) -> Result<PendingVersion, ApiError> {
    Ok(PendingVersion(
        crate::engine_backend::file_manager::history_pages::capture(
            engine,
            request.file_id.as_str(),
            request.root.as_ref(),
        )?,
        request,
    ))
}
struct PendingMerges(
    crate::engine_backend::file_manager::history_pages::FileHistoryCapture,
    crate::ReadHistoryMerges,
);
impl PendingMerges {
    fn read(self) -> Result<crate::HistoryMergesPage, ApiError> {
        self.0.merges(self.1, &tree_control())
    }
}
fn capture_merges(
    engine: &Engine,
    request: crate::ReadHistoryMerges,
) -> Result<PendingMerges, ApiError> {
    Ok(PendingMerges(
        crate::engine_backend::file_manager::history_pages::capture(
            engine,
            request.0.file_id.as_str(),
            Some(&request.0.root),
        )?,
        request,
    ))
}

#[test]
fn remaining_history_completion_checks_current_file_before_control_and_result() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let hash = engine.history_root(&repo.history_tree_id).unwrap();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let expired = crate::HistoryReadControl::until(std::time::Instant::now());
    let query = || BTreeMap::from([("history_root".into(), hash.as_str().to_owned())]);
    for kind in 0..5 {
        let captured = {
            let engine = backend.engine.lock().unwrap();
            crate::engine_backend::file_manager::history_pages::capture(
                &engine,
                &entry.file_id,
                None,
            )
            .unwrap()
        };
        let read = match kind {
            0 => captured
                .merges(
                    crate::ReadHistoryMerges::parse(entry.file_id.clone(), query()).unwrap(),
                    &tree_control(),
                )
                .map(|page| page.history_root),
            1 => captured
                .version_page(
                    crate::ReadVersionPage::parse(entry.file_id.clone(), query()).unwrap(),
                    &tree_control(),
                )
                .map(|page| page.history_root),
            2..=4 => {
                let request = crate::backend_file_history::ReadIndexedSlice::parse(
                    entry.file_id.clone(),
                    if kind == 4 { "version_id" } else { "merge_id" },
                    if kind == 4 {
                        entry.version_id.clone()
                    } else {
                        "missing".into()
                    },
                    query(),
                )
                .unwrap();
                match kind {
                    2 => captured
                        .merge_inputs(request, &tree_control())
                        .map(|page| page.history_root),
                    3 => captured
                        .merge_parents(request, &tree_control())
                        .map(|page| page.history_root),
                    4 => captured
                        .version_sources(request, &tree_control())
                        .map(|page| page.history_root),
                    _ => unreachable!(),
                }
            }
            _ => unreachable!(),
        };
        assert_eq!(
            backend
                .finish_file_history_page(&identity, &entry.file_id, read, &expired)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::RequestTimeout
        );
    }
    assert_eq!(
        backend
            .finish_file_history_page(&identity, "missing", Ok(()), &expired)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::FileNotFound
    );
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .finish_file_history_page(&identity, "missing", Ok(()), &expired)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}
