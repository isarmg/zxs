use super::*;
use crate::backend_directory_scopes::DirectoryScopesResponse;
use crate::{
    DirectoryBranchFilter, DirectoryBranchProjection, DirectoryScopeRows, ReadDirectoryScopes,
};

fn legacy_full(engine: &Engine, repo: &str) -> Value {
    let mut branches = engine
        .state()
        .directory_branches
        .values()
        .filter(|branch| branch.repository_id == repo)
        .collect::<Vec<_>>();
    branches.sort_by_key(|branch| {
        (
            std::cmp::Reverse(branch.sequence),
            branch.directory_branch_id.as_str(),
        )
    });
    let rows = branches
        .into_iter()
        .map(|branch| {
            let mut row = json_codec::to_value(branch).unwrap();
            row["current"] = json!(
                engine
                    .state()
                    .directory_selections
                    .get(&branch.directory_id)
                    == Some(&branch.directory_branch_id)
            );
            row["file_count"] = json!(
                branch
                    .entries
                    .iter()
                    .filter(|entry| entry.kind == storage_contract::NamespaceEntryKind::File)
                    .count()
                    .to_string()
            );
            row
        })
        .collect::<Vec<_>>();
    json!({"repository_id":repo,"directory_branches":rows})
}
fn compact(full: &Value) -> Value {
    let rows = full["directory_branches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let keys = [
                "directory_branch_id",
                "repository_id",
                "directory_id",
                "name",
                "parent_branch_id",
                "head_tree_id",
                "commit_id",
                "path",
                "materialized",
                "current",
                "sequence",
                "file_count",
            ];
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.into(), row[key].clone()))
                    .collect(),
            )
        })
        .collect::<Vec<_>>();
    json!({"repository_id":full["repository_id"],"directory_branches":rows})
}
#[test]
fn directory_scopes_capture_requested_shape_once_and_outlive_engine() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(
        &mut engine,
        &repo.main_branch_id,
        "Café/file.txt",
        b"content",
    );
    for path in ["", "Café"] {
        checkpoint(&mut engine, &repo, path, "owner");
        engine
            .create_directory_branches(
                &repo.repository_id,
                (if (path).is_empty() {
                    None
                } else {
                    Some(name_policy::LookupPath::parse(path).unwrap())
                })
                .as_ref(),
                &storage_contract::BranchName::parse("frozen").unwrap(),
                "owner",
                storage_contract::OperationId::generate().unwrap(),
            )
            .unwrap();
    }
    // Selection consumes a map keyed by id, not the scope presentation order.
    // Removing the intermediate presentation sort preserves every map byte.
    let repository = &engine.state().repositories[&repo.repository_id];
    for key in [
        None,
        Some(""),
        Some("café"),
        Some("cafe\u{301}"),
        Some("missing"),
    ] {
        let old: BTreeMap<_, _> =
            crate::engine_backend::directory_scopes::matching(&engine, repository, key)
                .unwrap()
                .into_iter()
                .map(|branch| (branch.directory_branch_id.as_str(), branch))
                .collect();
        let new: BTreeMap<_, _> =
            crate::engine_backend::directory_scopes::matching_unordered(&engine, repository, key)
                .unwrap()
                .map(|branch| (branch.directory_branch_id.as_str(), branch))
                .collect();
        assert_eq!(
            json_codec::to_vec(&old).unwrap(),
            json_codec::to_vec(&new).unwrap()
        );
        for (id, branch) in &new {
            assert!(std::ptr::eq(*branch, old[id]));
        }
    }
    let expected = legacy_full(&engine, &repo.repository_id);
    let expected_summary = compact(&expected);
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let request = |projection| ReadDirectoryScopes {
        repository_id: &repo.repository_id,
        filter: &DirectoryBranchFilter::All,
        projection,
    };
    let full = backend
        .directory_scopes(&identity, request(DirectoryBranchProjection::Full))
        .unwrap();
    let summary = backend
        .directory_scopes(&identity, request(DirectoryBranchProjection::Summary))
        .unwrap();
    assert!(matches!(&full.branches,DirectoryScopeRows::Full(rows) if rows.len()==4));
    assert!(matches!(&summary.branches,DirectoryScopeRows::Summary(rows) if rows.len()==4));
    let root_only = backend
        .directory_scopes(
            &identity,
            ReadDirectoryScopes {
                repository_id: &repo.repository_id,
                filter: &DirectoryBranchFilter::Root,
                projection: DirectoryBranchProjection::Summary,
            },
        )
        .unwrap();
    assert!(
        matches!(root_only.branches,DirectoryScopeRows::Summary(rows) if rows.len()==2 && rows.iter().all(|row|row.directory_id==repo.repository_id))
    );
    let nfd =
        DirectoryBranchFilter::Directory(name_policy::LookupPath::parse("CAFE\u{301}").unwrap());
    let nested = backend
        .directory_scopes(
            &identity,
            ReadDirectoryScopes {
                repository_id: &repo.repository_id,
                filter: &nfd,
                projection: DirectoryBranchProjection::Summary,
            },
        )
        .unwrap();
    assert!(
        matches!(nested.branches,DirectoryScopeRows::Summary(rows) if rows.len()==2 && rows.iter().all(|row|row.path=="Café"))
    );
    {
        let mut engine = backend.engine.lock().unwrap();
        checkpoint(&mut engine, &repo, "", "owner");
        engine
            .create_directory_branches(
                &repo.repository_id,
                None,
                &storage_contract::BranchName::parse("later").unwrap(),
                "owner",
                storage_contract::OperationId::generate().unwrap(),
            )
            .unwrap();
    }
    let full = DirectoryScopesResponse::from(full);
    let summary = DirectoryScopesResponse::from(summary);
    struct Probe<'a>(&'a EngineBackend, &'a DirectoryScopesResponse);
    impl Serialize for Probe<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let available = std::thread::scope(|scope| {
                scope
                    .spawn(|| self.0.engine.try_lock().is_ok())
                    .join()
                    .unwrap()
            });
            if !available {
                return Err(S::Error::custom("directory scopes retained engine lock"));
            }
            self.1.serialize(serializer)
        }
    }
    assert_eq!(
        json_codec::to_value(Probe(&backend, &full)).unwrap(),
        expected
    );
    assert_eq!(
        json_codec::to_value(Probe(&backend, &summary)).unwrap(),
        expected_summary
    );
    assert_eq!(
        storage_contract::canonical_json(&full).unwrap(),
        storage_contract::canonical_json(&expected).unwrap()
    );
    let missing = || ReadDirectoryScopes {
        repository_id: "missing",
        filter: &DirectoryBranchFilter::All,
        projection: DirectoryBranchProjection::Full,
    };
    let error = backend
        .directory_scopes(&identity, missing())
        .err()
        .unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(error.message, "repository not found");
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .directory_scopes(&identity, missing())
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    drop(auth);
    drop(backend);
    assert_eq!(json_codec::to_value(full).unwrap(), expected);
    assert_eq!(json_codec::to_value(summary).unwrap(), expected_summary);
}

fn all_scope_ids(
    backend: &EngineBackend,
    identity: &Identity,
    repo: &str,
    projection: DirectoryBranchProjection,
) -> BTreeMap<String, String> {
    let snapshot = backend
        .directory_scopes(
            identity,
            ReadDirectoryScopes {
                repository_id: repo,
                filter: &DirectoryBranchFilter::All,
                projection,
            },
        )
        .unwrap();
    match snapshot.branches {
        DirectoryScopeRows::Full(rows) => rows
            .into_iter()
            .map(|row| (row.branch.directory_branch_id, row.branch.directory_id))
            .collect(),
        DirectoryScopeRows::Summary(rows) => rows
            .into_iter()
            .map(|row| (row.directory_branch_id, row.directory_id))
            .collect(),
    }
}

#[test]
fn rejected_initialized_directory_deletion_preserves_current_scopes_data_and_authorization() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "dir/file", b"original");
    for path in [None, Some(name_policy::LookupPath::parse("dir").unwrap())] {
        checkpoint(
            &mut engine,
            &repo,
            path.as_ref().map_or("", name_policy::LookupPath::as_str),
            "owner",
        );
        engine
            .create_directory_branches(
                &repo.repository_id,
                path.as_ref(),
                &storage_contract::BranchName::parse("frozen").unwrap(),
                "owner",
                storage_contract::OperationId::generate().unwrap(),
            )
            .unwrap();
    }
    let old_id = engine
        .namespace_tree(&repo.main_branch_id)
        .unwrap()
        .get("dir")
        .unwrap()
        .entry_id
        .clone();
    let historical = engine
        .state()
        .directory_branches
        .values()
        .find(|branch| branch.directory_id == old_id && branch.name == "frozen")
        .unwrap()
        .clone();
    let expected_scopes: BTreeMap<_, _> = engine
        .state()
        .directory_branches
        .values()
        .filter(|branch| branch.repository_id == repo.repository_id)
        .map(|branch| {
            (
                branch.directory_branch_id.clone(),
                branch.directory_id.clone(),
            )
        })
        .collect();
    assert_eq!(expected_scopes.len(), 4);
    assert_eq!(
        expected_scopes
            .values()
            .filter(|id| *id == &repo.repository_id)
            .count(),
        2
    );
    let original = engine
        .namespace_tree(&repo.main_branch_id)
        .unwrap()
        .get("dir/file")
        .unwrap()
        .clone();
    let before = storage_contract::canonical_json(engine.state()).unwrap();
    let operation = storage_contract::OperationId::generate().unwrap();
    let error = engine
        .file_namespace(
            &repo.repository_id,
            storage_engine::NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::DeleteTree,
                "dir",
                None,
            )
            .unwrap(),
            "owner",
            operation,
        )
        .unwrap_err();
    assert_eq!(
        error.code(),
        storage_contract::ErrorCode::UnsupportedOperation
    );
    assert_eq!(
        storage_contract::canonical_json(engine.state()).unwrap(),
        before
    );
    assert!(!engine.state().operations.contains_key(&operation));

    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    for projection in [
        DirectoryBranchProjection::Full,
        DirectoryBranchProjection::Summary,
    ] {
        assert_eq!(
            all_scope_ids(&backend, &identity, &repo.repository_id, projection),
            expected_scopes
        );
    }
    let history_request = || {
        storage_contract::directory_history::DirectoryHistoryRequest::parse(
            &repo.repository_id,
            &old_id,
            storage_contract::directory_history::DirectoryHistoryCollection::Branches,
            BTreeMap::new(),
        )
        .unwrap()
    };
    let history = backend
        .directory_history(
            &identity,
            history_request(),
            storage_contract::history_window::HistoryWindowCancellation::new(),
        )
        .unwrap();
    assert!(history
        .branches
        .iter()
        .any(|branch| branch.directory_branch_id == historical.directory_branch_id));
    auth.logout(&token).unwrap();
    for projection in [
        DirectoryBranchProjection::Full,
        DirectoryBranchProjection::Summary,
    ] {
        let error = backend
            .directory_scopes(
                &identity,
                ReadDirectoryScopes {
                    repository_id: &repo.repository_id,
                    filter: &DirectoryBranchFilter::All,
                    projection,
                },
            )
            .err()
            .unwrap();
        assert_eq!(error.code, storage_contract::ErrorCode::Unauthorized);
    }
    let error = backend
        .directory_history(
            &identity,
            history_request(),
            storage_contract::history_window::HistoryWindowCancellation::new(),
        )
        .err()
        .unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::Unauthorized);
    drop(auth);
    drop(backend);

    let engine = Engine::open(&root.path().join("system")).unwrap();
    let namespace = engine.namespace_tree(&repo.main_branch_id).unwrap();
    assert_eq!(namespace.get("dir").unwrap().entry_id, old_id);
    assert_eq!(namespace.get("dir/file").unwrap(), &original);
    assert_eq!(
        engine.state().directory_branches[&historical.directory_branch_id],
        historical
    );
    assert!(!engine.state().operations.contains_key(&operation));
    let mut content = engine.open_content(&original.version_id).unwrap();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut content.file, &mut bytes).unwrap();
    assert_eq!(bytes, b"original");
}
