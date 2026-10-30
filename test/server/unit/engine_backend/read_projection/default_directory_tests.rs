use super::*;
fn default_value(snapshot: &crate::DefaultDirectoryBranchSnapshot) -> Value {
    json_codec::from_slice(snapshot.body()).unwrap()
}

fn legacy_wire(engine: &Engine, repo: &storage_contract::Repository) -> Value {
    let state = engine.state();
    let selected = state.directory_selections.get(&repo.repository_id);
    let name = selected
        .and_then(|id| state.directory_branches.get(id))
        .map(|branch| branch.name.as_str())
        .unwrap_or("main");
    let mut branches = state
        .directory_branches
        .values()
        .filter(|branch| {
            branch.repository_id == repo.repository_id && branch.directory_id == repo.repository_id
        })
        .collect::<Vec<_>>();
    branches.sort_by_key(|branch| {
        (
            std::cmp::Reverse(branch.sequence),
            branch.directory_branch_id.as_str(),
        )
    });
    let branches=branches.into_iter().map(|branch|json!({"directory_branch_id":branch.directory_branch_id,"name":branch.name,"path":"","current":selected==Some(&branch.directory_branch_id),"materialized":branch.materialized,"head_tree_id":branch.head_tree_id,"file_count":branch.file_branches.len().to_string(),"parent_branch_id":branch.parent_branch_id})).collect::<Vec<_>>();
    json!({"repository_id":repo.repository_id,"namespace_branch_id":repo.main_branch_id,"directory_branch_id":selected,"name":name,"directory_branches":branches})
}
#[test]
fn default_directory_metadata_is_frozen_and_retains_admission_and_wire() {
    let (root, engine, repo, _) = fixture();
    let empty = legacy_wire(&engine, &repo);
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let initial = backend
        .default_directory_branch(&identity, &repo.repository_id, &tree_control())
        .unwrap();
    assert!(default_value(&initial)["directory_branch_id"].is_null());
    assert_eq!(default_value(&initial)["directory_branches"], json!([]));
    assert_eq!(default_value(&initial), empty);
    let expected = {
        let mut engine = backend.engine.lock().unwrap();
        save_projection_file(
            &mut engine,
            &repo.main_branch_id,
            "folder/file.txt",
            b"content",
        );
        engine
            .initialize_directory_scope(
                &repo.repository_id,
                None,
                "owner",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        engine
            .create_directory_branches(
                &repo.repository_id,
                None,
                &storage_contract::BranchName::parse("草稿").unwrap(),
                "owner",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        legacy_wire(&engine, &repo)
    };
    let snapshot = backend
        .default_directory_branch(&identity, &repo.repository_id, &tree_control())
        .unwrap();
    assert!(default_value(&snapshot)["directory_branch_id"].is_string());
    assert_eq!(
        default_value(&snapshot)["directory_branches"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    backend
        .engine
        .lock()
        .unwrap()
        .create_directory_branches(
            &repo.repository_id,
            None,
            &storage_contract::BranchName::parse("later").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let missing = backend
        .default_directory_branch(&identity, "missing-repository", &tree_control())
        .err()
        .unwrap();
    assert_eq!(missing.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(missing.message, "repository not found");
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .default_directory_branch(&identity, "missing-repository", &tree_control())
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert!(backend.engine.try_lock().is_ok());
    assert_eq!(default_value(&snapshot), expected);
    drop(auth);
    drop(backend);
    assert_eq!(default_value(&snapshot), expected);
}

#[test]
fn typed_default_selection_retains_receipt_digest_and_admission_order() {
    let (root, mut engine, repo, _) = fixture();
    engine
        .initialize_directory_scope(
            &repo.repository_id,
            None,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let selected = engine.state().directory_selections[&repo.repository_id].clone();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let operation = storage_contract::OperationId::generate();
    let command =
        |repository: &str, branch: &str, operation_id| crate::SelectDefaultDirectoryBranch {
            repository_id: repository.into(),
            directory_branch_id: branch.into(),
            operation_id,
        };
    let result = backend
        .select_default_directory_branch(
            &identity,
            command(&repo.repository_id, &selected, operation),
        )
        .unwrap();
    assert_eq!(
        backend
            .select_default_directory_branch(
                &identity,
                command(&repo.repository_id, &selected, operation)
            )
            .unwrap(),
        result
    );
    assert_eq!(
        backend
            .engine
            .lock()
            .unwrap()
            .select_default_directory_branch(
                &repo.repository_id,
                &selected,
                &identity.principal,
                operation
            )
            .unwrap(),
        result
    );
    let expected=storage_contract::object_hash("request",&json!({"kind":"directory.default.select","principal":identity.principal,"body":{"repository_id":repo.repository_id,"directory_branch_id":selected}})).unwrap();
    assert_eq!(result.request_digest, expected);
    // A changed digest wins over the missing branch's dynamic scope failure.
    assert_eq!(
        backend
            .select_default_directory_branch(
                &identity,
                command(&repo.repository_id, "missing-directory", operation)
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        backend
            .select_default_directory_branch(
                &identity,
                command(
                    &repo.repository_id,
                    "missing-directory",
                    storage_contract::OperationId::generate()
                )
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::DirectoryBranchScope
    );
    let missing = backend
        .select_default_directory_branch(
            &identity,
            command("missing-repository", "missing-directory", operation),
        )
        .unwrap_err();
    assert_eq!(missing.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(missing.message, "repository not found");
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .select_default_directory_branch(
                &identity,
                command("missing-repository", "missing-directory", operation)
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn typed_directory_rename_keeps_canonical_receipts_and_cached_name_collision_policy() {
    let (root, mut engine, repo, _) = fixture();
    engine
        .initialize_directory_scope(
            &repo.repository_id,
            None,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let branch = engine.state().directory_selections[&repo.repository_id].clone();
    engine
        .create_directory_branches(
            &repo.repository_id,
            None,
            &storage_contract::BranchName::parse("Straße").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let command =
        |repository: &str, branch: &str, name: &str, operation_id| crate::RenameDirectoryBranch {
            repository_id: repository.into(),
            directory_branch_id: branch.into(),
            name: storage_contract::BranchName::parse(name).unwrap(),
            operation_id,
        };
    let collision = backend
        .rename_directory_branch(
            &identity,
            command(
                &repo.repository_id,
                &branch,
                "STRASSE",
                storage_contract::OperationId::generate(),
            ),
        )
        .unwrap_err();
    assert_eq!(collision.code, storage_contract::ErrorCode::NameCollision);
    let operation = storage_contract::OperationId::generate();
    let result = backend
        .rename_directory_branch(
            &identity,
            command(&repo.repository_id, &branch, "Café", operation),
        )
        .unwrap();
    assert_eq!(result.result["name"], "Café");
    assert_eq!(
        backend
            .rename_directory_branch(
                &identity,
                command(&repo.repository_id, &branch, "Café", operation)
            )
            .unwrap(),
        result
    );
    assert_eq!(
        backend
            .engine
            .lock()
            .unwrap()
            .rename_directory_branch(
                &repo.repository_id,
                &branch,
                &storage_contract::BranchName::parse("Café").unwrap(),
                &identity.principal,
                operation
            )
            .unwrap(),
        result
    );
    let expected=storage_contract::object_hash("request",&json!({"kind":"directory.branch.rename","principal":identity.principal,"body":{"repository_id":repo.repository_id,"directory_branch_id":branch,"name":"Café"}})).unwrap();
    assert_eq!(result.request_digest, expected);
    assert_eq!(
        backend
            .rename_directory_branch(
                &identity,
                command(&repo.repository_id, &branch, "Changed", operation)
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        backend
            .rename_directory_branch(
                &identity,
                command(&repo.repository_id, "missing-branch", "Café", operation)
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        backend
            .rename_directory_branch(
                &identity,
                command(
                    &repo.repository_id,
                    "missing-branch",
                    "Café",
                    storage_contract::OperationId::generate()
                )
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::DirectoryBranchNotFound
    );
    let missing = backend
        .rename_directory_branch(
            &identity,
            command("missing-repository", "missing-branch", "Café", operation),
        )
        .unwrap_err();
    assert_eq!(missing.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(missing.message, "repository not found");
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .rename_directory_branch(
                &identity,
                command("missing-repository", "missing-branch", "Café", operation)
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn typed_scope_initialization_preserves_lookup_spelling_receipts_and_live_admission() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(
        &mut engine,
        &repo.main_branch_id,
        "Café/file.txt",
        b"content",
    );
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let operation = storage_contract::OperationId::generate();
    let command = |path: &str, operation_id| crate::InitializeDirectoryScope {
        repository_id: repo.repository_id.clone(),
        path: Some(name_policy::LookupPath::parse(path).unwrap()),
        operation_id,
    };
    let result = backend
        .initialize_directory_scope(&identity, command("Cafe\u{301}", operation))
        .unwrap();
    let branch = result.result["directory_branch_id"].as_str().unwrap();
    let tree = result.result["head_tree_id"].as_str().unwrap();
    {
        let engine = backend.engine.lock().unwrap();
        assert_eq!(engine.state().directory_branches[branch].path, "Café");
        assert_eq!(engine.state().directory_trees[tree].path, "Café");
        assert_eq!(engine.state().directory_trees[tree].entries.len(), 1);
        let expected=storage_contract::object_hash("request",&json!({"kind":"directory.scope.initialize","principal":identity.principal,"body":{"repository_id":repo.repository_id,"path":"Cafe\u{301}"}})).unwrap();
        assert_eq!(
            engine
                .state()
                .operations
                .tail_record(operation)
                .unwrap()
                .result
                .request_digest,
            expected
        );
    }
    let replay = backend
        .initialize_directory_scope(&identity, command("Cafe\u{301}", operation))
        .unwrap();
    assert_eq!(result.result, replay.result);
    assert_eq!(result.sequence, replay.sequence);
    assert_eq!(result.protection_at_publish, replay.protection_at_publish);
    assert_eq!(
        backend
            .initialize_directory_scope(&identity, command("Café", operation))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::IdempotencyKeyReused
    );
    let missing = backend
        .initialize_directory_scope(
            &identity,
            command("missing", storage_contract::OperationId::generate()),
        )
        .unwrap_err();
    assert_eq!(missing.code, storage_contract::ErrorCode::DirectoryNotFound);
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .initialize_directory_scope(&identity, command("missing", operation))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn detached_default_preserves_selection_and_releases_publication_and_writer() {
    let (_root, mut engine, repo, identity) = fixture();
    engine
        .initialize_directory_scope(
            &repo.repository_id,
            None,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let selected = engine.state().directory_selections[&repo.repository_id].clone();
    let expected = legacy_wire(&engine, &repo);
    let head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
    let commit = engine.state().commits.get_shared(head).unwrap();
    let weak = Arc::downgrade(&commit);
    drop(commit);
    let backend = EngineBackend::new(engine);
    let view = backend.engine.lock().unwrap().published_read_view();
    {
        let original = &view.state().directory_branches[&selected];
        let row = crate::DefaultDirectoryBranchRow {
            branch: original,
            current: true,
        };
        assert!(std::ptr::eq(
            row.branch,
            &view.state().directory_branches[&selected]
        ));
        assert_eq!(row.branch.name.as_ptr(), original.name.as_ptr());
        assert_eq!(row.branch.entries.as_ptr(), original.entries.as_ptr());
        let encoded = crate::DefaultDirectoryBranchSnapshot::encode(
            &repo.repository_id,
            &repo.main_branch_id,
            Some(crate::SelectedDirectoryBranch {
                directory_branch_id: &selected,
                name: &original.name,
            }),
            &[row],
        )
        .unwrap();
        assert_eq!(default_value(&encoded), expected);
    }
    let mut writer = backend.engine.lock().unwrap();
    writer
        .rename_directory_branch(
            &repo.repository_id,
            &selected,
            &storage_contract::BranchName::parse("renamed").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    // A later selection/name must not replace the captured selection.
    writer
        .create_directory_branches(
            &repo.repository_id,
            None,
            &storage_contract::BranchName::parse("later").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let other = writer
        .state()
        .directory_branches
        .values()
        .find(|branch| branch.directory_branch_id != selected)
        .unwrap()
        .directory_branch_id
        .clone();
    writer
        .select_default_directory_branch(
            &repo.repository_id,
            &other,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let repository_id = repo.repository_id.clone();
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        send.send(crate::engine_backend::default_directory::read(
            view,
            &repository_id,
            &tree_control(),
        ))
        .unwrap();
    });
    let result = receive.recv_timeout(std::time::Duration::from_secs(30));
    drop(writer);
    worker.join().unwrap();
    let snapshot = backend
        .finish_default_directory(
            &identity,
            result.expect("default reader waited for writer mutex"),
            &tree_control(),
        )
        .unwrap();
    assert_eq!(default_value(&snapshot), expected);
    drop(backend);
    assert!(
        weak.upgrade().is_none(),
        "encoded result retained old publication"
    );
    let allocation = snapshot.body().as_ptr();
    assert_eq!(snapshot.into_encoded().body().as_ptr(), allocation);
}

#[test]
fn default_scope_preserves_root_filter_ties_path_and_map_count() {
    let (_root, mut engine, repo, _) = fixture();
    engine
        .initialize_directory_scope(
            &repo.repository_id,
            None,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let mut state = engine.state().clone();
    let original = state.directory_branches.values().next().unwrap().clone();
    state.directory_branches.clear();
    for (id, sequence, repository, directory) in [
        (
            "z",
            u64::MAX - 1,
            repo.repository_id.as_str(),
            repo.repository_id.as_str(),
        ),
        (
            "b",
            u64::MAX,
            repo.repository_id.as_str(),
            repo.repository_id.as_str(),
        ),
        (
            "a",
            u64::MAX,
            repo.repository_id.as_str(),
            repo.repository_id.as_str(),
        ),
        ("nested", 9, repo.repository_id.as_str(), "nested"),
        ("foreign", 9, "foreign", repo.repository_id.as_str()),
    ] {
        let mut row = original.clone();
        row.directory_branch_id = id.into();
        row.sequence = sequence;
        row.repository_id = repository.into();
        row.directory_id = directory.into();
        row.path = "not-the-public-root-path".into();
        row.entries.clear();
        row.file_branches = [("file".into(), "branch".into())].into();
        state.directory_branches.insert(id.into(), row);
    }
    state
        .directory_selections
        .insert(repo.repository_id.clone(), "b".into());
    storage_engine::test_support::replace_state(&mut engine, state);
    let expected = legacy_wire(&engine, &repo);
    let actual = crate::engine_backend::default_directory::read(
        engine.published_read_view(),
        &repo.repository_id,
        &tree_control(),
    )
    .unwrap();
    let actual = default_value(&actual);
    assert_eq!(actual, expected);
    assert_eq!(
        actual["directory_branches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["directory_branch_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["a", "b", "z"]
    );
    for row in actual["directory_branches"].as_array().unwrap() {
        assert_eq!(row["path"], "");
        assert_eq!(row["file_count"], "1");
    }
}

#[test]
fn default_completion_reauthorizes_success_errors_and_cancellation() {
    for managed in [false, true] {
        let (root, engine, repo, _) = fixture();
        let backend = Arc::new(EngineBackend::new(engine));
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
        let control = crate::tests::history_read_control();
        let snapshot = crate::engine_backend::default_directory::read(
            backend.engine.lock().unwrap().published_read_view(),
            &repo.repository_id,
            &control,
        );
        let failure = || {
            ApiError::new(
                ::http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::ObjectCorrupt,
                "captured default metadata failed",
            )
        };
        assert_eq!(
            backend
                .finish_default_directory(&identity, Err(failure()), &control)
                .err()
                .unwrap()
                .code,
            storage_contract::ErrorCode::ObjectCorrupt
        );
        let cancelled = crate::tests::history_read_control();
        drop(cancelled.wait_owner());
        assert_eq!(
            backend
                .finish_default_directory(&identity, Err(failure()), &cancelled)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::RequestTimeout
        );
        assert_eq!(
            backend
                .default_directory_branch(&identity, &repo.repository_id, &cancelled)
                .err()
                .unwrap()
                .code,
            storage_contract::ErrorCode::RequestTimeout
        );
        assert_eq!(
            crate::engine_backend::default_directory::read(
                backend.engine.lock().unwrap().published_read_view(),
                &repo.repository_id,
                &cancelled
            )
            .err()
            .unwrap()
            .code,
            storage_contract::ErrorCode::RequestTimeout
        );
        auth.logout(&token).unwrap();
        for result in [snapshot, Err(failure())] {
            assert_eq!(
                backend
                    .finish_default_directory(&identity, result, &control)
                    .err()
                    .unwrap()
                    .code,
                storage_contract::ErrorCode::Unauthorized
            );
        }
        assert_eq!(
            backend
                .finish_default_directory(&identity, Err(failure()), &cancelled)
                .err()
                .unwrap()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
}

#[test]
#[should_panic(expected = "invariant: admitted directory selection references its branch")]
fn default_dangling_admitted_selection_is_not_silently_main() {
    let (_root, mut engine, repo, _) = fixture();
    let mut state = engine.state().clone();
    state
        .directory_selections
        .insert(repo.repository_id.clone(), "missing".into());
    storage_engine::test_support::replace_state(&mut engine, state);
    let _ = crate::engine_backend::default_directory::read(
        engine.published_read_view(),
        &repo.repository_id,
        &tree_control(),
    );
}
