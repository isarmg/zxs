use super::*;
use crate::{
    DirectorySelectionEntryInput, DirectorySelectionSourceInput, ReadDirectorySelection,
    backend_directory_selection::DirectorySelectionResponse,
};
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
        )
        .unwrap();
    assert_eq!(snapshot.branches.len(), 1);
    assert_eq!(snapshot.branches[0].entries.len(), 1);
    assert_eq!(
        backend
            .directory_selection(&identity, request(&repo.repository_id, "missing-tree"))
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::ListingChanged
    );
    let missing = backend
        .directory_selection(&identity, request("missing-repository", "missing-tree"))
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
            .directory_selection(&identity, request("missing-repository", "missing-tree"))
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    drop(auth);
    drop(backend);
    assert_eq!(json_codec::to_value(response).unwrap(), expected);
}
