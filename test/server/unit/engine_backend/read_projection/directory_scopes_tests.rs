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
        engine
            .initialize_directory_scope(
                &repo.repository_id,
                (if path.is_empty() {
                    None
                } else {
                    Some(name_policy::LookupPath::parse(path).unwrap())
                })
                .as_ref(),
                "owner",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
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
                storage_contract::OperationId::generate(),
            )
            .unwrap();
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
