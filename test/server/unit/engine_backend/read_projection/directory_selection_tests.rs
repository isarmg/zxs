use super::*;
use crate::{
    DirectorySelectionEntryInput, DirectorySelectionSourceInput, ReadDirectorySelection,
    backend_directory_selection::DirectorySelectionResponse,
};

fn selection_fixture() -> (
    tempfile::TempDir,
    Engine,
    storage_engine::Repository,
    Identity,
    storage_contract::DirectoryBranch,
) {
    let (root, mut engine, repo, identity) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "before.txt", b"before");
    engine
        .initialize_directory_scope(
            &repo.repository_id,
            None,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let branch = engine.state().directory_branches
        [&engine.state().directory_selections[&repo.repository_id]]
        .clone();
    (root, engine, repo, identity, branch)
}

fn selection_request(branch: &storage_contract::DirectoryBranch) -> ReadDirectorySelection {
    ReadDirectorySelection::parse(
        branch.repository_id.clone(),
        "",
        vec![DirectorySelectionSourceInput {
            directory_branch_id: branch.directory_branch_id.clone(),
            expected_tree_id: branch.head_tree_id.clone(),
        }],
        vec![DirectorySelectionEntryInput {
            entry_id: branch.entries[0].entry_id.clone(),
            path: "before.txt".into(),
        }],
    )
    .unwrap()
}

fn selection_wire(snapshot: crate::DirectorySelectionSnapshot) -> Value {
    json_codec::to_value(DirectorySelectionResponse::from(snapshot)).unwrap()
}

#[test]
fn selected_facts_run_with_writer_held_and_keep_the_captured_branch() {
    let (_root, engine, repo, identity, branch) = selection_fixture();
    let backend = EngineBackend::new(engine);
    let request = selection_request(&branch);
    let view = backend.engine.lock().unwrap().published_read_view();
    let expected = selection_wire(
        crate::engine_backend::directory_selection::read(
            backend.engine.lock().unwrap().published_read_view(),
            &request,
            &tree_control(),
        )
        .unwrap(),
    );
    let mut writer = backend.engine.lock().unwrap();
    writer
        .rename_directory_branch(
            &repo.repository_id,
            &branch.directory_branch_id,
            &storage_contract::BranchName::parse("renamed").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    save_projection_file(&mut writer, &repo.main_branch_id, "later.txt", b"later");
    let current = &writer.state().directory_branches[&branch.directory_branch_id];
    assert_ne!(current.name, branch.name);
    assert_ne!(current.head_tree_id, branch.head_tree_id);
    assert_eq!(current.entries.len(), branch.entries.len() + 1);
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        send.send(crate::engine_backend::directory_selection::read(
            view,
            &request,
            &tree_control(),
        ))
        .unwrap();
    });
    let result = receive.recv_timeout(std::time::Duration::from_secs(30));
    drop(writer);
    worker.join().unwrap();
    let result = result.expect("selected facts waited for the writer mutex");
    let snapshot = backend
        .finish_directory_selection(&identity, result, &tree_control())
        .unwrap();
    assert_eq!(selection_wire(snapshot), expected);
    assert_eq!(
        backend
            .directory_selection(&identity, selection_request(&branch), &tree_control())
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::ListingChanged
    );
}

#[test]
fn selection_completion_reauthorizes_success_errors_and_expiry() {
    for managed in [false, true] {
        let (root, engine, _repo, _identity, branch) = selection_fixture();
        let backend = EngineBackend::new(engine);
        let auth =
            AuthStore::initialize(root.path(), "owner", "integration-test-password").unwrap();
        let mut auth = if managed {
            backend.set_management_auth(auth.reader()).unwrap();
            auth
        } else {
            backend.protect_auth(auth).unwrap()
        };
        let token = auth
            .login("owner", "integration-test-password")
            .unwrap()
            .token;
        let identity = auth.authenticate(&token).unwrap();
        let request = selection_request(&branch);
        let snapshot = crate::engine_backend::directory_selection::read(
            backend.engine.lock().unwrap().published_read_view(),
            &request,
            &tree_control(),
        );
        let failure = || {
            crate::ApiError::new(
                ::http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::ObjectCorrupt,
                "selected namespace is corrupt",
            )
        };
        let error = backend
            .finish_directory_selection(&identity, Err(failure()), &tree_control())
            .err()
            .unwrap();
        assert_eq!(error.code, storage_contract::ErrorCode::ObjectCorrupt);
        assert_eq!(error.message, "selected namespace is corrupt");
        auth.logout(&token).unwrap();
        for result in [snapshot, Err(failure())] {
            assert_eq!(
                backend
                    .finish_directory_selection(&identity, result, &tree_control())
                    .err()
                    .unwrap()
                    .code,
                storage_contract::ErrorCode::Unauthorized
            );
        }
        let expired = crate::HistoryReadControl::until(std::time::Instant::now());
        assert_eq!(
            backend
                .finish_directory_selection(&identity, Err(failure()), &expired)
                .err()
                .unwrap()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
}

#[test]
fn selection_cancellation_precedes_detached_work_and_returns_timeout() {
    let (_root, engine, _repo, _identity, branch) = selection_fixture();
    let request = selection_request(&branch);
    for expired in [false, true] {
        let control = if expired {
            crate::HistoryReadControl::until(std::time::Instant::now())
        } else {
            tree_control()
        };
        if !expired {
            drop(control.wait_owner());
        }
        let error = crate::engine_backend::directory_selection::read(
            engine.published_read_view(),
            &request,
            &control,
        )
        .err()
        .unwrap();
        assert_eq!(error.code, storage_contract::ErrorCode::RequestTimeout);
        assert_eq!(error.status, ::http::StatusCode::REQUEST_TIMEOUT);
    }
}

#[test]
fn selected_sources_must_match_repository_scope_and_head() {
    let (_root, mut engine, repo, identity, branch) = selection_fixture();
    save_projection_file(
        &mut engine,
        &repo.main_branch_id,
        "folder/nested.txt",
        b"nested",
    );
    let nested_path = name_policy::LookupPath::parse("folder").unwrap();
    engine
        .initialize_directory_scope(
            &repo.repository_id,
            Some(&nested_path),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let nested = engine
        .state()
        .directory_branches
        .values()
        .find(|b| b.path == "folder")
        .unwrap()
        .clone();
    let other = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let backend = EngineBackend::new(engine);
    for (repository, path, id, head) in [
        (
            repo.repository_id.as_str(),
            "",
            nested.directory_branch_id.as_str(),
            nested.head_tree_id.as_str(),
        ),
        (
            other.repository_id.as_str(),
            "",
            branch.directory_branch_id.as_str(),
            branch.head_tree_id.as_str(),
        ),
        (
            repo.repository_id.as_str(),
            "folder",
            "missing-branch",
            "missing-head",
        ),
        (
            repo.repository_id.as_str(),
            "folder",
            nested.directory_branch_id.as_str(),
            "missing-head",
        ),
    ] {
        let request = ReadDirectorySelection::parse(
            repository.into(),
            path,
            vec![DirectorySelectionSourceInput {
                directory_branch_id: id.into(),
                expected_tree_id: head.into(),
            }],
            Vec::new(),
        )
        .unwrap();
        let error = backend
            .directory_selection(&identity, request, &tree_control())
            .err()
            .unwrap();
        assert_eq!(error.code, storage_contract::ErrorCode::ListingChanged);
        assert_eq!(
            error.message,
            "source scope changed; refresh before preparing a batch"
        );
    }
}

#[test]
fn scope_match_keeps_stable_directory_identity_and_missing_path_fallback() {
    use crate::engine_backend::directory_scopes::ScopeMatch;
    let (_root, mut engine, repo, _identity, branch) = selection_fixture();
    save_projection_file(
        &mut engine,
        &repo.main_branch_id,
        "folder/nested.txt",
        b"nested",
    );
    let head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
    let directory = engine
        .commit_namespace(head)
        .unwrap()
        .get_key("folder")
        .unwrap()
        .unwrap();
    let stable = ScopeMatch::resolve(&repo, Some("folder"), engine.commit_namespace(head)).unwrap();
    let mut candidate = branch.clone();
    candidate.directory_id = directory.entry_id;
    candidate.path = "previous-name".into();
    assert!(stable.matches(&candidate));
    candidate.directory_id = "other-directory".into();
    candidate.path = "folder".into();
    assert!(!stable.matches(&candidate));
    let fallback =
        ScopeMatch::resolve(&repo, Some("missing"), engine.commit_namespace(head)).unwrap();
    candidate.path = "Missing".into();
    assert!(fallback.matches(&candidate));
    candidate.repository_id = "other-repository".into();
    assert!(!fallback.matches(&candidate));
    let root = ScopeMatch::resolve(&repo, Some(""), None).unwrap();
    assert!(root.matches(&branch));
    let all = ScopeMatch::resolve(&repo, None, None).unwrap();
    assert!(all.matches(&branch));
}

#[test]
fn bounded_selection_freezes_only_requested_facts_and_rechecks_current_baselines() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "Café.txt", b"content");
    engine
        .initialize_directory_scope(
            &repo.repository_id,
            None,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let branch = engine.state().directory_branches
        [&engine.state().directory_selections[&repo.repository_id]]
        .clone();
    let entry = &branch.entries[0];
    let path = "CAFE\u{301}.txt";
    let request = |repository: &str, head: &str| {
        ReadDirectorySelection::parse(
            repository.into(),
            "",
            vec![DirectorySelectionSourceInput {
                directory_branch_id: branch.directory_branch_id.clone(),
                expected_tree_id: head.into(),
            }],
            vec![DirectorySelectionEntryInput {
                entry_id: entry.entry_id.clone(),
                path: path.into(),
            }],
        )
        .unwrap()
    };
    let row = json_codec::to_value(&branch).unwrap();
    let mut expected_row = json_codec::Map::new();
    for key in [
        "directory_branch_id",
        "repository_id",
        "directory_id",
        "name",
        "parent_branch_id",
        "head_tree_id",
        "commit_id",
        "path",
        "materialized",
        "sequence",
    ] {
        expected_row.insert(key.into(), row[key].clone());
    }
    expected_row.insert("current".into(), json!(true));
    expected_row.insert("file_count".into(), json!("1"));
    expected_row.insert(
        "entries".into(),
        json!([{"entry_id":entry.entry_id,"path":entry.path,"selected_path":path}]),
    );
    let expected = json!({"repository_id":repo.repository_id,"directory_branches":[expected_row]});
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let snapshot = backend
        .directory_selection(
            &identity,
            request(&repo.repository_id, &branch.head_tree_id),
            &crate::tests::history_read_control(),
        )
        .unwrap();
    assert_eq!(snapshot.branches.len(), 1);
    assert_eq!(snapshot.branches[0].entries.len(), 1);
    assert_eq!(
        backend
            .directory_selection(
                &identity,
                request(&repo.repository_id, "missing-tree"),
                &crate::tests::history_read_control()
            )
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::ListingChanged
    );
    let missing = backend
        .directory_selection(
            &identity,
            request("missing-repository", "missing-tree"),
            &crate::tests::history_read_control(),
        )
        .err()
        .unwrap();
    assert_eq!(missing.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(missing.message, "repository not found");
    backend
        .engine
        .lock()
        .unwrap()
        .rename_directory_branch(
            &repo.repository_id,
            &branch.directory_branch_id,
            &storage_contract::BranchName::parse("renamed").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let response = DirectorySelectionResponse::from(snapshot);
    struct Probe<'a>(&'a EngineBackend, &'a DirectorySelectionResponse);
    impl Serialize for Probe<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let available = std::thread::scope(|scope| {
                scope
                    .spawn(|| self.0.engine.try_lock().is_ok())
                    .join()
                    .unwrap()
            });
            if !available {
                return Err(S::Error::custom(
                    "selection serialization retained engine lock",
                ));
            }
            self.1.serialize(serializer)
        }
    }
    assert_eq!(
        json_codec::to_value(Probe(&backend, &response)).unwrap(),
        expected
    );
    assert_eq!(
        storage_contract::canonical_json(&response).unwrap(),
        storage_contract::canonical_json(&expected).unwrap()
    );
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .directory_selection(
                &identity,
                request("missing-repository", "missing-tree"),
                &crate::tests::history_read_control()
            )
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    drop(auth);
    drop(backend);
    assert_eq!(json_codec::to_value(response).unwrap(), expected);
}
