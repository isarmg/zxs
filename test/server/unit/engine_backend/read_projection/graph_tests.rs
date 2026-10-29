use super::*;

#[test]
fn typed_graph_shares_only_selected_payloads_and_preserves_the_frozen_legacy_response() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let old = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"second");
    let head = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let fork = engine
        .create_branch(
            &old,
            &storage_contract::BranchName::parse("fork").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .add_merge_link(
            &fork.branch_id,
            &fork.head_commit_id,
            &repo.main_branch_id,
            &head,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let other = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let other_head = &engine.state().branches[&other.main_branch_id].head_commit_id;
    let excluded = engine.state().commits.get_shared(other_head).unwrap();
    let expected = json_codec::to_value(engine.graph(&repo.history_tree_id).unwrap()).unwrap();
    let (backend, auth, _token, identity) = authenticated_backend(engine, root.path());
    let snapshot = backend
        .history_graph(&identity, &repo.history_tree_id)
        .unwrap();
    assert_eq!(
        Arc::strong_count(&excluded),
        2,
        "unselected history must not acquire graph handles"
    );
    assert!(
        snapshot
            .commits
            .windows(2)
            .all(|pair| pair[0].sequence <= pair[1].sequence)
    );
    {
        let mut engine = backend.engine.lock().unwrap();
        for commit in &snapshot.commits {
            let record = &engine.state().commits[&commit.commit_id];
            assert!(std::ptr::eq(commit.as_ref(), record));
            assert_eq!(commit.history_tree_id, repo.history_tree_id);
        }
        save_projection_file(&mut engine, &repo.main_branch_id, "later.txt", b"later");
    }
    struct Probe<'a>(
        &'a EngineBackend,
        &'a crate::backend_graph::HistoryGraphResponse,
    );
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
                    "graph serialization retained engine mutex",
                ));
            }
            self.1.serialize(serializer)
        }
    }
    let response = crate::backend_graph::HistoryGraphResponse::from(snapshot);
    assert_eq!(
        json_codec::to_value(Probe(&backend, &response)).unwrap(),
        expected
    );
    drop(auth);
    drop(backend);
    assert_eq!(
        storage_contract::canonical_json(&response).unwrap(),
        storage_contract::canonical_json_value(&expected).unwrap()
    );
    assert_eq!(Arc::strong_count(&excluded), 1);
}

#[test]
fn typed_graph_keeps_history_error_and_rechecks_authority_before_missing_history() {
    let (root, engine, repo, _) = fixture();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let missing = backend.history_graph(&identity, "missing").unwrap_err();
    assert_eq!(missing.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(missing.message, "history not found");
    backend
        .history_graph(&identity, &repo.history_tree_id)
        .unwrap();
    auth.logout(&token).unwrap();
    for history in [&repo.history_tree_id, "missing"] {
        assert_eq!(
            backend.history_graph(&identity, history).unwrap_err().code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
}

#[test]
fn scoped_graph_commit_capture_matches_full_graph_after_state_replacement() {
    let (_root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"second");
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other graph").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let compare = |engine: &Engine| {
        let expected = json_codec::to_value(engine.graph(&repo.history_tree_id).unwrap()).unwrap();
        let mut snapshot = crate::engine_backend::graph::capture(engine, &repo.history_tree_id);
        snapshot.commits.sort_by_key(|commit| commit.sequence);
        let actual = crate::backend_graph::HistoryGraphResponse::from(snapshot);
        assert_eq!(json_codec::to_value(actual).unwrap(), expected);
    };
    compare(&engine);
    let mut state = engine.state().clone();
    state.branches.get_mut(&repo.main_branch_id).unwrap().state =
        storage_contract::BranchState::Migrated;
    let ids = state.commits.keys().cloned().collect::<Vec<_>>();
    for id in ids {
        state.commits.get_mut(&id).unwrap().sequence = 7;
    }
    storage_engine::test_support::replace_state(&mut engine, state);
    compare(&engine);
}
