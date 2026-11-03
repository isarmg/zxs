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
            storage_contract::OperationId::generate().unwrap(),
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
            storage_contract::OperationId::generate().unwrap(),
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

#[test]
fn bounded_selection_preserves_multiple_source_order_and_empty_queries() {
    let (_root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "Café.txt", b"content");
    engine
        .initialize_directory_scope(
            &repo.repository_id,
            None,
            "owner",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    engine
        .create_directory_branches(
            &repo.repository_id,
            None,
            &storage_contract::BranchName::parse("frozen").unwrap(),
            "owner",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let mut branches = engine
        .state()
        .directory_branches
        .values()
        .collect::<Vec<_>>();
    branches.reverse();
    assert_eq!(branches.len(), 2);
    let entry = &branches[0].entries[0];
    let query = |source_count, select_entry, stale: Option<usize>| {
        ReadDirectorySelection::parse(
            repo.repository_id.clone(),
            "",
            branches
                .iter()
                .take(source_count)
                .enumerate()
                .map(|(index, branch)| DirectorySelectionSourceInput {
                    directory_branch_id: branch.directory_branch_id.clone(),
                    expected_tree_id: if stale == Some(index) {
                        "stale-tree".into()
                    } else {
                        branch.head_tree_id.clone()
                    },
                })
                .collect(),
            if select_entry {
                vec![DirectorySelectionEntryInput {
                    entry_id: entry.entry_id.clone(),
                    path: "CAFE\u{301}.txt".into(),
                }]
            } else {
                vec![]
            },
        )
        .unwrap()
    };
    for (source_count, select_entry) in [(2, true), (2, false), (0, true), (0, false)] {
        let request = query(source_count, select_entry, None);
        let snapshot = crate::engine_backend::directory_selection::read(&engine, &request).unwrap();
        assert_eq!(snapshot.branches.len(), source_count);
        for (actual, expected) in snapshot.branches.iter().zip(&branches) {
            assert_eq!(
                actual.summary.directory_branch_id,
                expected.directory_branch_id
            );
            assert_eq!(actual.entries.len(), usize::from(select_entry));
            if select_entry {
                assert_eq!(actual.entries[0].entry_id, entry.entry_id);
                assert_eq!(actual.entries[0].path, "Café.txt");
                assert_eq!(actual.entries[0].selected_path, "CAFE\u{301}.txt");
            }
        }
    }
    for stale_index in 0..branches.len() {
        let request = query(2, true, Some(stale_index));
        let error = crate::engine_backend::directory_selection::read(&engine, &request)
            .err()
            .unwrap();
        assert_eq!(error.code, storage_contract::ErrorCode::ListingChanged);
        assert_eq!(
            error.message,
            "source scope changed; refresh before preparing a batch"
        );
    }
}
