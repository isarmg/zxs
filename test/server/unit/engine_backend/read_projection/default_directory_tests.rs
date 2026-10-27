use super::*;
use crate::backend_default_directory::DefaultDirectoryBranchResponse;

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
        .default_directory_branch(&identity, &repo.repository_id)
        .unwrap();
    assert!(initial.selected.is_none());
    assert!(initial.directory_branches.is_empty());
    assert_eq!(
        json_codec::to_value(DefaultDirectoryBranchResponse::from(initial)).unwrap(),
        empty
    );
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
        .default_directory_branch(&identity, &repo.repository_id)
        .unwrap();
    assert!(snapshot.selected.is_some());
    assert_eq!(snapshot.directory_branches.len(), 2);
    let response = DefaultDirectoryBranchResponse::from(snapshot);
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
        .default_directory_branch(&identity, "missing-repository")
        .err()
        .unwrap();
    assert_eq!(missing.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(missing.message, "repository not found");
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .default_directory_branch(&identity, "missing-repository")
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    struct Probe<'a>(&'a EngineBackend, &'a DefaultDirectoryBranchResponse);
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
                    "default-directory serialization retained engine mutex",
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
    drop(auth);
    drop(backend);
    assert_eq!(json_codec::to_value(response).unwrap(), expected);
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
            engine.state().operations[&operation].request_digest,
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
